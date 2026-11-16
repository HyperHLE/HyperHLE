/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

//! Quick menu: the pause button in the top-left corner of the emulated screen,
//! and the panel it opens while an app runs.
//!
//! It is the mid-game face of the settings screen: performance display,
//! FPS limiter, RAM limiter and the Cheat Engine-style trainer, plus
//! "Resume" and "Exit emulator". Edits are collected while the panel is open
//! and handed to the emulator loop as one batch when it closes (see
//! [`take_applied`]), so the game never sees a half-finished edit.
//!
//! Drawing and input follow `crate::trainer_ui`: the overlay is drawn
//! host-side on top of the presented frame, and it receives raw window-space
//! touch coordinates from `crate::window` before they reach the guest.

use crate::gles::GLES;
use std::sync::atomic::Ordering;
use std::sync::Mutex;

const COL_PANEL: (f32, f32, f32, f32) = (0.05, 0.05, 0.06, 0.94);
const COL_BUTTON: (f32, f32, f32, f32) = (0.08, 0.08, 0.10, 0.82);
const COL_ROW: (f32, f32, f32, f32) = (0.16, 0.16, 0.18, 1.0);
const COL_TEXT: (f32, f32, f32, f32) = (0.95, 0.95, 0.95, 1.0);
const COL_TEXT_DIM: (f32, f32, f32, f32) = (0.62, 0.62, 0.66, 1.0);
const COL_ACCENT: (f32, f32, f32, f32) = (0.00, 0.75, 0.35, 1.0);
const COL_EXIT: (f32, f32, f32, f32) = (0.80, 0.24, 0.24, 1.0);

/// The FPS limiter choices, matching the values the settings screen offers.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FpsChoice {
    Default,
    Thirty,
    Sixty,
    Off,
}

impl FpsChoice {
    fn label(self) -> &'static str {
        match self {
            FpsChoice::Default => "DEFAULT",
            FpsChoice::Thirty => "30 FPS",
            FpsChoice::Sixty => "60 FPS",
            FpsChoice::Off => "OFF",
        }
    }

    /// The `Options::fps_limit` value this choice means.
    pub fn limit(self) -> Option<f64> {
        match self {
            FpsChoice::Default | FpsChoice::Sixty => Some(60.0),
            FpsChoice::Thirty => Some(30.0),
            FpsChoice::Off => None,
        }
    }

    fn next(self) -> Self {
        match self {
            FpsChoice::Default => FpsChoice::Thirty,
            FpsChoice::Thirty => FpsChoice::Sixty,
            FpsChoice::Sixty => FpsChoice::Off,
            FpsChoice::Off => FpsChoice::Default,
        }
    }

    pub fn from_limit(limit: Option<f64>) -> Self {
        match limit {
            None => FpsChoice::Off,
            Some(fps) if fps <= 31.0 => FpsChoice::Thirty,
            Some(_) => FpsChoice::Sixty,
        }
    }
}

/// The RAM limiter choices the panel cycles through, in MiB. `None` is
/// "Default", i.e. the cap derived from the Device model.
const RAM_CHOICES: &[Option<u32>] = &[
    None,
    Some(32),
    Some(64),
    Some(128),
    Some(256),
    Some(512),
    Some(1024),
    Some(2048),
];

fn ram_label(choice: Option<u32>) -> String {
    match choice {
        None => "DEFAULT".to_string(),
        Some(mb) if mb >= 1024 && mb.is_multiple_of(1024) => format!("{} GB", mb / 1024),
        Some(mb) => format!("{mb} MB"),
    }
}

fn next_ram_choice(choice: Option<u32>) -> Option<u32> {
    let index = RAM_CHOICES
        .iter()
        .position(|candidate| *candidate == choice)
        .unwrap_or(0);
    RAM_CHOICES[(index + 1) % RAM_CHOICES.len()]
}

// Widget ids. Everything the panel draws is also a touch target, and the ids
// travel through `QuickMenuUi::pending` between the down and the up.
const W_NONE: u16 = 0;
const W_BUTTON: u16 = 1;
/// Inside the panel but not on a row: swallow the gesture so the guest never
/// sees a Moved/Ended without the matching Began.
const W_SWALLOW: u16 = 2;
const W_PERF: u16 = 10;
const W_FPS: u16 = 11;
const W_RAM: u16 = 12;
const W_TRAINER: u16 = 13;
const W_RESUME: u16 = 14;
const W_EXIT: u16 = 15;

struct QuickMenuUi {
    enabled: bool,
    open: bool,
    /// The widget the current gesture started on, if it started on us.
    pending: u16,
    perf_display: bool,
    fps: FpsChoice,
    /// Preserve non-preset limits until the user actually changes this row.
    fps_limit: Option<f64>,
    ram: Option<u32>,
    trainer: bool,
}

impl QuickMenuUi {
    const fn new() -> Self {
        Self {
            enabled: false,
            open: false,
            pending: W_NONE,
            perf_display: false,
            fps: FpsChoice::Default,
            fps_limit: Some(60.0),
            ram: None,
            trainer: false,
        }
    }

    fn begin_editing(&mut self, perf_display: bool) {
        self.open = true;
        // F1 or an environment override may have changed the actual display
        // since the game started or since the panel was last opened.
        self.perf_display = perf_display;
    }

    fn fps_label(&self) -> String {
        match self.fps_limit {
            Some(fps) if fps != 30.0 && fps != 60.0 => format!("{fps} FPS"),
            _ => self.fps.label().to_string(),
        }
    }

    fn applied(&self) -> Applied {
        Applied {
            perf_display: self.perf_display,
            fps_limit: self.fps_limit,
            memory_limit_mb: self.ram,
            trainer: self.trainer,
        }
    }
}

static UI: Mutex<QuickMenuUi> = Mutex::new(QuickMenuUi::new());

/// The values the user settled on, handed to the emulator loop when the panel
/// closes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Applied {
    pub perf_display: bool,
    pub fps_limit: Option<f64>,
    pub memory_limit_mb: Option<u32>,
    pub trainer: bool,
}

static APPLIED: Mutex<Option<Applied>> = Mutex::new(None);
/// Set when a batch of edits is waiting in `APPLIED`, so the emulator loop can
/// skip the mutex on the overwhelming majority of iterations.
static APPLIED_PENDING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
static EXIT_REQUESTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Publishes the settings the app was launched with, so the panel starts from
/// what is actually in effect, and remembers whether `--quick-menu` is on.
pub fn init(
    enabled: bool,
    perf_display: bool,
    fps_limit: Option<f64>,
    memory_limit_mb: Option<u32>,
    trainer: bool,
) {
    let Ok(mut ui) = UI.lock() else { return };
    ui.enabled = enabled;
    ui.open = false;
    ui.pending = W_NONE;
    ui.perf_display = perf_display;
    ui.fps = FpsChoice::from_limit(fps_limit);
    ui.fps_limit = fps_limit;
    ui.ram = memory_limit_mb;
    ui.trainer = trainer;
    // Static state survives game sessions; never replay an old batch or exit.
    if let Ok(mut applied) = APPLIED.lock() {
        *applied = None;
    }
    APPLIED_PENDING.store(false, Ordering::Relaxed);
    EXIT_REQUESTED.store(false, Ordering::Relaxed);
}

/// Hides the quick menu. Used by the app picker, which is an `Environment`
/// of its own: the button belongs to games, and the static state outlives a
/// session, so the picker has to clear it explicitly.
pub fn disable() {
    let Ok(mut ui) = UI.lock() else { return };
    ui.enabled = false;
    ui.open = false;
    ui.pending = W_NONE;
}

fn queue_applied(applied: Applied) {
    if let Ok(mut slot) = APPLIED.lock() {
        *slot = Some(applied);
        APPLIED_PENDING.store(true, Ordering::Relaxed);
    }
}

/// Takes the batch of edits the user applied by closing the panel, if any.
pub fn take_applied() -> Option<Applied> {
    if !APPLIED_PENDING.swap(false, Ordering::Relaxed) {
        return None;
    }
    APPLIED.lock().ok().and_then(|mut applied| applied.take())
}

/// Whether "Exit emulator" was tapped since the last check.
pub fn take_exit_request() -> bool {
    EXIT_REQUESTED.swap(false, Ordering::Relaxed)
}

#[derive(Clone, Copy)]
struct Rect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

impl Rect {
    fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

struct Panel {
    rect: Rect,
    rows: Vec<(u16, Rect)>,
}

struct Layout {
    button: Rect,
    panel: Option<Panel>,
}

/// Where the button and (when open) the panel are, in viewport pixel space.
///
/// Shared by the draw and the touch paths so the targets line up with what is
/// drawn; coordinates are viewport-relative because the GL viewport already
/// offsets drawing and the touch handlers subtract the origin.
fn compute_layout(ui: &QuickMenuUi, viewport: (u32, u32, u32, u32)) -> Layout {
    let (_vx, _vy, vw, vh) = viewport;
    let (vw, vh) = (vw as f32, vh as f32);
    // Scale with the screen so the panel is usable on a 480x320 viewport and
    // not comically small on a tablet.
    let s = (vh / 480.0).min(vw / 320.0).clamp(0.35, 3.0);
    let size = 30.0 * s;
    let button = Rect {
        x: 6.0 * s,
        y: 6.0 * s,
        w: size,
        h: size,
    };
    let panel = ui.open.then(|| {
        let widest = (260.0 * s).max(120.0);
        let width = (vw - 12.0 * s).clamp(120.0, widest);
        let row_height = 26.0 * s;
        let ids = [
            W_PERF,
            W_FPS,
            W_RAM,
            W_TRAINER,
            W_RESUME,
            W_EXIT,
        ];
        let x = 6.0 * s;
        let y = button.y + button.h + 6.0 * s;
        let mut rows = Vec::with_capacity(ids.len());
        // The first row is the panel's header.
        let mut row_y = y + row_height;
        for id in ids {
            rows.push((
                id,
                Rect {
                    x: x + 4.0 * s,
                    y: row_y,
                    w: width - 8.0 * s,
                    h: row_height - 2.0 * s,
                },
            ));
            row_y += row_height;
        }
        Panel {
            rect: Rect {
                x,
                y,
                w: width,
                h: row_y - y + 4.0 * s,
            },
            rows,
        }
    });
    Layout { button, panel }
}

/// Builds the overlay scene for the current state, or `None` when there is
/// nothing to draw.
unsafe fn build_scene(
    gles: &mut dyn GLES,
    viewport: (u32, u32, u32, u32),
) -> Option<crate::trainer_ui::OverlayScene> {
    let ui = UI.lock().ok()?;
    if !ui.enabled {
        return None;
    }
    let layout = compute_layout(&ui, viewport);
    let (vw, vh) = (viewport.2 as f32, viewport.3 as f32);
    let s = (vh / 480.0).min(vw / 320.0).clamp(0.35, 3.0);
    let mut scene = crate::trainer_ui::OverlayScene::new();

    // The button: a pause glyph (two bars) in a dark rounded-ish square.
    scene.rect(
        layout.button.x,
        layout.button.y,
        layout.button.w,
        layout.button.h,
        COL_BUTTON,
    );
    let bar_width = (layout.button.w * 0.14).max(2.0);
    let bar_height = layout.button.h * 0.52;
    let bar_y = layout.button.y + (layout.button.h - bar_height) / 2.0;
    let bar_x = layout.button.x + layout.button.w * 0.3;
    scene.rect(bar_x, bar_y, bar_width, bar_height, COL_TEXT);
    scene.rect(
        bar_x + bar_width * 2.0,
        bar_y,
        bar_width,
        bar_height,
        COL_TEXT,
    );

    let Some(panel) = layout.panel else {
        return Some(scene);
    };
    scene.rect(panel.rect.x, panel.rect.y, panel.rect.w, panel.rect.h, COL_PANEL);
    let text_x = panel.rect.x + 8.0 * s;
    let font = 11.0 * s;
    let header_y = panel.rect.y + 6.0 * s;
    scene.text(
        gles,
        "QUICK MENU",
        text_x,
        header_y,
        font,
        COL_TEXT_DIM,
    );

    let fps_label = ui.fps_label();
    for (id, row) in &panel.rows {
        let (label, value, color) = match *id {
            W_PERF => (
                "PERF DISPLAY",
                if ui.perf_display { "ON" } else { "OFF" },
                COL_ROW,
            ),
            W_FPS => ("FPS LIMIT", fps_label.as_str(), COL_ROW),
            W_RAM => ("RAM LIMIT", "", COL_ROW),
            W_TRAINER => (
                "CHEAT ENGINE",
                if ui.trainer { "ON" } else { "OFF" },
                COL_ROW,
            ),
            W_RESUME => ("RESUME", "", COL_ACCENT),
            _ => ("EXIT EMULATOR", "", COL_EXIT),
        };
        scene.rect(row.x, row.y, row.w, row.h, color);
        let label_width = scene.text(
            gles,
            label,
            row.x + 4.0 * s,
            row.y + 4.0 * s,
            font,
            COL_TEXT,
        );
        let value_text = match *id {
            W_RAM => ram_label(ui.ram).to_string(),
            _ if !value.is_empty() => value.to_string(),
            _ => String::new(),
        };
        if !value_text.is_empty() {
            let value_x = row.x + 4.0 * s + label_width + 8.0 * s;
            scene.text(
                gles,
                &value_text,
                value_x,
                row.y + 4.0 * s,
                font,
                COL_TEXT,
            );
        }
    }
    Some(scene)
}

/// Draws the quick menu on a fixed-function (GLES 1.x) present path.
pub unsafe fn draw(gles: &mut dyn GLES, viewport: (u32, u32, u32, u32)) {
    let Some(scene) = build_scene(gles, viewport) else {
        return;
    };
    crate::trainer_ui::draw_scene(gles, viewport, &scene);
}

/// Draws the quick menu on a native OpenGL ES 2.0 present path.
pub unsafe fn draw_es2(
    gles: &mut dyn GLES,
    viewport: (u32, u32, u32, u32),
    context_token: usize,
) {
    crate::trainer_ui::prepare_overlay_context(context_token);
    let Some(scene) = build_scene(gles, viewport) else {
        return;
    };
    crate::trainer_ui::draw_scene_es2(gles, viewport, context_token, &scene);
}

/// Takes a touch that lands on the button or the panel.
pub fn touch_down(abs: (f32, f32), viewport: (u32, u32, u32, u32)) -> bool {
    let Ok(mut ui) = UI.lock() else { return false };
    if !ui.enabled {
        return false;
    }
    let layout = compute_layout(&ui, viewport);
    let (x, y) = (
        abs.0 - viewport.0 as f32,
        abs.1 - viewport.1 as f32,
    );
    if layout.button.contains(x, y) {
        ui.pending = W_BUTTON;
        return true;
    }
    if let Some(panel) = &layout.panel {
        if panel.rect.contains(x, y) {
            for (id, rect) in &panel.rows {
                if rect.contains(x, y) {
                    ui.pending = *id;
                    return true;
                }
            }
            ui.pending = W_SWALLOW;
            return true;
        }
    }
    if ui.open {
        // The panel is up: taps on the game behind it must not sneak through
        // to the guest while its settings are being edited.
        ui.pending = W_SWALLOW;
        return true;
    }
    false
}

/// Swallows motion while a press that started on the overlay is in progress.
pub fn touch_motion(_abs: (f32, f32), _viewport: (u32, u32, u32, u32)) -> bool {
    UI.lock().map(|ui| ui.enabled && ui.pending != W_NONE).unwrap_or(false)
}

/// Finishes the gesture: taps act, releases elsewhere do nothing.
pub fn touch_up(abs: (f32, f32), viewport: (u32, u32, u32, u32)) -> bool {
    let Ok(mut ui) = UI.lock() else { return false };
    if !ui.enabled {
        return false;
    }
    let pending = std::mem::replace(&mut ui.pending, W_NONE);
    if pending == W_NONE {
        return false;
    }
    let layout = compute_layout(&ui, viewport);
    let (x, y) = (
        abs.0 - viewport.0 as f32,
        abs.1 - viewport.1 as f32,
    );
    let hit = |id: u16| {
        if id == W_BUTTON {
            layout.button.contains(x, y)
        } else {
            layout
                .panel
                .as_ref()
                .and_then(|panel| {
                    panel
                        .rows
                        .iter()
                        .find(|(row_id, _)| *row_id == id)
                        .map(|(_, rect)| *rect)
                })
                .is_some_and(|rect| rect.contains(x, y))
        }
    };
    match pending {
        W_BUTTON => {
            if hit(W_BUTTON) {
                if ui.open {
                    ui.open = false;
                    queue_applied(ui.applied());
                } else {
                    ui.begin_editing(crate::gles::perf_overlay::enabled());
                }
            }
        }
        W_PERF => {
            if hit(W_PERF) {
                ui.perf_display = !ui.perf_display;
            }
        }
        W_FPS => {
            if hit(W_FPS) {
                ui.fps = ui.fps.next();
                ui.fps_limit = ui.fps.limit();
            }
        }
        W_RAM => {
            if hit(W_RAM) {
                ui.ram = next_ram_choice(ui.ram);
            }
        }
        W_TRAINER => {
            if hit(W_TRAINER) {
                ui.trainer = !ui.trainer;
            }
        }
        W_RESUME => {
            if hit(W_RESUME) {
                ui.open = false;
                queue_applied(ui.applied());
            }
        }
        W_EXIT => {
            if hit(W_EXIT) {
                ui.open = false;
                EXIT_REQUESTED.store(true, Ordering::Relaxed);
            }
        }
        _ => {}
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fps_choices_map_to_limits_and_cycle() {
        assert_eq!(FpsChoice::Default.limit(), Some(60.0));
        assert_eq!(FpsChoice::Thirty.limit(), Some(30.0));
        assert_eq!(FpsChoice::Sixty.limit(), Some(60.0));
        assert_eq!(FpsChoice::Off.limit(), None);
        assert_eq!(FpsChoice::Default.next(), FpsChoice::Thirty);
        assert_eq!(FpsChoice::Thirty.next(), FpsChoice::Sixty);
        assert_eq!(FpsChoice::Sixty.next(), FpsChoice::Off);
        assert_eq!(FpsChoice::Off.next(), FpsChoice::Default);
        assert_eq!(FpsChoice::from_limit(None), FpsChoice::Off);
        assert_eq!(FpsChoice::from_limit(Some(30.0)), FpsChoice::Thirty);
        assert_eq!(FpsChoice::from_limit(Some(60.0)), FpsChoice::Sixty);
    }

    #[test]
    fn ram_choices_cycle_through_every_preset() {
        let mut choice = RAM_CHOICES[0];
        let mut seen = vec![choice];
        for _ in 1..RAM_CHOICES.len() {
            choice = next_ram_choice(choice);
            seen.push(choice);
        }
        assert_eq!(seen, RAM_CHOICES.to_vec());
        // ... and wrap back to "Default".
        assert_eq!(next_ram_choice(choice), RAM_CHOICES[0]);
        // A size the list doesn't know starts over from the beginning.
        assert_eq!(next_ram_choice(Some(1000)), Some(32));
        assert_eq!(ram_label(None), "DEFAULT");
        assert_eq!(ram_label(Some(32)), "32 MB");
        assert_eq!(ram_label(Some(1536)), "1536 MB");
        assert_eq!(ram_label(Some(2048)), "2 GB");
    }

    #[test]
    fn unchanged_custom_fps_limit_is_not_replaced_by_a_preset() {
        let mut ui = QuickMenuUi::new();
        ui.fps = FpsChoice::from_limit(Some(120.0));
        ui.fps_limit = Some(120.0);
        ui.begin_editing(true);
        assert_eq!(ui.fps_label(), "120 FPS");
        assert_eq!(ui.applied().fps_limit, Some(120.0));
    }

    #[test]
    fn the_button_sits_in_the_top_left_corner() {
        let ui = QuickMenuUi::new();
        let viewport = (0, 0, 480, 320);
        let layout = compute_layout(&ui, viewport);
        assert!(layout.button.x < 16.0);
        assert!(layout.button.y < 16.0);
        assert!(layout.panel.is_none());
    }

    #[test]
    fn the_open_panel_fits_inside_the_viewport() {
        let mut ui = QuickMenuUi::new();
        ui.open = true;
        for viewport in [(0, 0, 480, 320), (0, 0, 320, 480), (0, 0, 1024, 768)] {
            let layout = compute_layout(&ui, viewport);
            let panel = layout.panel.expect("an open panel has a layout");
            assert!(panel.rect.x >= 0.0);
            assert!(panel.rect.y >= 0.0);
            assert!(panel.rect.x + panel.rect.w <= viewport.2 as f32);
            assert!(panel.rect.y + panel.rect.h <= viewport.3 as f32);
            // Every row is inside the panel and doesn't overlap the header.
            for (_, row) in &panel.rows {
                assert!(row.x >= panel.rect.x);
                assert!(row.y >= panel.rect.y);
                assert!(row.y + row.h <= panel.rect.y + panel.rect.h);
            }
        }
    }

    #[test]
    fn edits_are_only_reported_when_the_panel_closes() {
        let mut ui = QuickMenuUi::new();
        ui.enabled = true;
        ui.open = true;
        ui.perf_display = true;
        ui.fps = FpsChoice::Thirty;
        ui.fps_limit = Some(30.0);
        ui.ram = Some(512);
        assert_eq!(
            ui.applied(),
            Applied {
                perf_display: true,
                fps_limit: Some(30.0),
                memory_limit_mb: Some(512),
                trainer: false,
            }
        );
    }
}
