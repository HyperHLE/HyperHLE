/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

//! Cheat Engine-style on-screen trainer overlay: a small floating button in
//! the top-right corner of the viewport opens a touch panel for searching and
//! editing guest memory while a game is running.
//!
//! The overlay is drawn host-side on top of the presented frame (see
//! `crate::gles::present::present_frame`) using the GLES 1.x fixed-function
//! API, and receives raw window-space touch coordinates from
//! `crate::window` before they are forwarded to the guest. Commands produced
//! by the panel are executed by the trainer engine (`crate::trainer`) on the
//! main loop thread, where `&mut Mem` is available.

use crate::font::{Font, TextAlignment};
use crate::guest_clock::Speed;
use crate::gles::gles11_raw as gles11;
use crate::gles::{GLES, GLint, GLuint};
use crate::trainer::{SearchResult, VType};
use crate::trainer::watch::{Change, WatchFilter};
use std::collections::VecDeque;
use crate::trainer::classify::{Category, ResultFilter};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

// ---------------------------------------------------------------------------
// Commands sent from the overlay to the trainer engine.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub enum TrainerCmd {
    Search { vtype: VType, text: String },
    Refine { vtype: VType, text: String },
    Reset,
    Set { vtype: VType, text: String },
    WatchSet { addr: u32, vtype: VType, text: String },
    SetAll { vtype: VType, text: String, confirm: bool, safe_mode: bool, filter: ResultFilter },
    CancelBulk,
    RefreshView,
    InspectSelection,
    Mark,
    Compare(WatchFilter),
    ClearActivity,
    Freeze { vtype: VType, text: String },
    UnfreezeAll,
    Dump,
    SaveHack { vtype: VType },
    ApplyHack { addr: u32, vtype: VType, bits: u64 },
}

static COMMANDS: Mutex<Vec<TrainerCmd>> = Mutex::new(Vec::new());

// ---------------------------------------------------------------------------
// Overlay state, shared between the input path (window thread), the draw
// path (present callback) and the engine (main loop tick).
// ---------------------------------------------------------------------------

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum Focus {
    Search,
    Set,
}

struct TrainerUi {
    enabled: bool,
    app_id: Option<String>,
    open: bool,
    focus: Focus,
    search_text: String,
    set_text: String,
    vtype: VType,
    results: Vec<SearchResult>,
    total_results: usize,
    filtered_results: usize,
    category_counts: [usize; 6],
    filter: ResultFilter,
    selected_type: Option<VType>,
    selected: Option<u32>,
    scroll: usize,
    status: String,
    frozen_count: usize,
    bulk_preview: bool,
    /// Latched preference for this session, not the current touch state.
    safe_mode: bool,
    /// Latched StoreKit IAP emulation (Lucky Patcher-style free in-app buys).
    iap: bool,
    speed: Speed,
    speed_dirty: bool,
    /// Widget id pressed but not yet released (pending activation).
    pending: Option<u16>,
    watch_open: bool,
    watch_paused: bool,
    activity: Vec<Change>,
    activity_changed: usize,
    activity_tracked: usize,
    activity_scroll: usize,
    watch_target: Option<(u32, VType)>,
    watch_current: Option<u64>,
    watch_text: String,
    watch_status: String,
}

impl TrainerUi {
    /// Cache only the current page, but count/filter the complete result set.
    /// This avoids both a huge UI copy and the old first-200 paging cutoff.
    fn update_results(&mut self, results: &[SearchResult], reset: bool) {
        if reset {
            self.watch_target = None;
            self.watch_current = None;
            self.watch_text.clear();
            self.watch_status.clear();
            self.activity_tracked = results.len();
            self.scroll = 0;
            self.selected = None;
            self.selected_type = None;
        }
        self.category_counts = [0; 6];
        for result in results {
            self.category_counts[result.analysis.category.index()] += 1;
        }
        self.total_results = results.len();
        self.filtered_results = match self.filter {
            ResultFilter::All => results.len(),
            ResultFilter::Category(c) => self.category_counts[c.index()],
        };
        self.scroll = self.scroll.min(self.filtered_results.saturating_sub(RESULT_ROWS));
        self.results = results.iter()
            .filter(|r| self.filter.matches(r.analysis.category))
            .skip(self.scroll).take(RESULT_ROWS).copied().collect();
        if self.selected.is_some() && !self.results.iter().any(|r| {
            Some(r.addr) == self.selected && Some(r.vtype) == self.selected_type
        }) {
            self.status = "SELECTION LEFT CURRENT PAGE".to_string();
            self.selected = None;
            self.selected_type = None;
        }
    }

    fn take_speed_request(&mut self) -> Option<Speed> {
        std::mem::take(&mut self.speed_dirty).then_some(self.speed)
    }

    const fn new() -> TrainerUi {
        TrainerUi {
            enabled: true,
            app_id: None,
            open: false,
            focus: Focus::Search,
            search_text: String::new(),
            set_text: String::new(),
            vtype: VType::Auto,
            results: Vec::new(),
            total_results: 0,
            filtered_results: 0,
            category_counts: [0; 6],
            filter: ResultFilter::All,
            selected_type: None,
            selected: None,
            scroll: 0,
            status: String::new(),
            frozen_count: 0,
            bulk_preview: false,
            safe_mode: true,
            // Cannot read the environment in this const context; the env
            // default is picked up lazily by the activation toggle.
            iap: false,
            speed: Speed::Normal,
            speed_dirty: false,
            pending: None,
            watch_open: false,
            watch_paused: false,
            activity: Vec::new(),
            activity_changed: 0,
            activity_tracked: 0,
            activity_scroll: 0,
            watch_target: None,
            watch_current: None,
            watch_text: String::new(),
            watch_status: String::new(),
        }
    }
}

static UI: Mutex<TrainerUi> = Mutex::new(TrainerUi::new());
static HARDWARE_ENABLED: AtomicBool = AtomicBool::new(true);

/// Master switch. The trainer is disabled by default; `--trainer` opts in
/// and `--no-trainer` forces it off again.
pub fn set_hardware_enabled(enabled: bool) {
    HARDWARE_ENABLED.store(enabled, Ordering::SeqCst);
}

/// Called by the trainer engine whenever the running app changes.
pub fn reset_for_app(app_id: Option<&str>) {
    // Each app gets a fresh GL context, so the cached glyph atlas texture
    // (created under the previous context) is stale — sampling it in the new
    // context returns garbage/white. Rebuild it for the new context.
    invalidate_atlas();
    let mut ui = UI.lock().unwrap();
    ui.app_id = app_id.map(String::from);
    ui.open = false;
    ui.focus = Focus::Search;
    ui.search_text.clear();
    ui.set_text.clear();
    ui.results.clear();
    ui.total_results = 0;
    ui.filtered_results = 0;
    ui.category_counts = [0; 6];
    ui.filter = ResultFilter::All;
    ui.selected = None;
    ui.selected_type = None;
    ui.scroll = 0;
    ui.status.clear();
    ui.frozen_count = 0;
    ui.bulk_preview = false;
    ui.speed = Speed::Normal;
    ui.speed_dirty = false;
    ui.pending = None;
    ui.watch_open = false;
    ui.watch_target = None;
    ui.watch_current = None;
    ui.watch_text.clear();
    ui.watch_status.clear();
    ui.watch_paused = false;
    ui.activity.clear();
    ui.activity_changed = 0;
    ui.activity_tracked = 0;
    ui.activity_scroll = 0;
}

/// Refresh the currently selected category/page without resetting its scroll.
pub fn publish_live_values(results: &[SearchResult]) {
    UI.lock().unwrap().update_results(results, false);
}

pub fn publish_activity(changes: &VecDeque<Change>, changed: usize, tracked: usize) {
    let mut ui = UI.lock().unwrap();
    // Do not let a live reorder replace the row under the user's finger.
    let pressing_row = ui.pending.is_some_and(|id| (W_CHANGE_BASE..W_CHANGE_BASE + RESULT_ROWS as u16).contains(&id));
    if !ui.watch_paused && !pressing_row {
        ui.activity_scroll = 0;
        ui.activity = changes.iter().copied().collect();
        ui.activity_changed = changed;
        ui.activity_tracked = tracked;
    }
}

pub fn clear_activity() {
    let mut ui = UI.lock().unwrap();
    ui.activity.clear();
    ui.activity_scroll = 0;
    ui.activity_changed = 0;
    ui.activity_tracked = ui.total_results;
}

pub fn watch_target() -> Option<(u32, VType)> {
    UI.lock().unwrap().watch_target
}

pub fn publish_watch_value(target: (u32, VType), value: Option<u64>) {
    let mut ui = UI.lock().unwrap();
    if ui.watch_target == Some(target) { ui.watch_current = value; }
}

pub fn publish_watch_status(target: (u32, VType), status: String) {
    let mut ui = UI.lock().unwrap();
    if ui.watch_target == Some(target) { ui.watch_status = status; }
}

/// Consumed by Environment, which owns the per-app guest clock.
pub fn take_speed_request() -> Option<Speed> {
    UI.lock().unwrap().take_speed_request()
}

pub fn take_commands() -> Vec<TrainerCmd> {
    std::mem::take(&mut COMMANDS.lock().unwrap())
}

pub fn publish_results(results: &[SearchResult], _total: usize) {
    UI.lock().unwrap().update_results(results, true);
}

pub fn selected_result_type(addr: u32) -> Option<VType> {
    let ui = UI.lock().unwrap();
    if ui.selected == Some(addr) { ui.selected_type } else { None }
}

pub fn publish_status(status: String) {
    UI.lock().unwrap().status = status;
}

pub fn publish_bulk_preview(ready: bool) {
    UI.lock().unwrap().bulk_preview = ready;
}

pub fn publish_frozen(count: usize) {
    UI.lock().unwrap().frozen_count = count;
}

pub fn selected_address() -> Option<u32> {
    UI.lock().unwrap().selected
}

// ---------------------------------------------------------------------------
// Layout & hit testing.
// ---------------------------------------------------------------------------

const W_BUTTON: u16 = 1;
const W_CLOSE: u16 = 2;
const W_TYPE: u16 = 3;
const W_FIELD_SEARCH: u16 = 4;
const W_FIELD_SET: u16 = 5;
const W_SEARCH: u16 = 6;
const W_REFINE: u16 = 7;
const W_RESET: u16 = 8;
const W_SET: u16 = 9;
const W_FREEZE: u16 = 10;
const W_UNFREEZE: u16 = 11;
const W_SET_ALL: u16 = 12;
const W_DUMP: u16 = 16;
const W_SAFE_MODE: u16 = 17;
const W_SPEED_DOWN: u16 = 18;
const W_SPEED_RESET: u16 = 19;
const W_SPEED_UP: u16 = 20;
const W_CATEGORY: u16 = 21;
const W_WATCH: u16 = 22;
const W_MARK: u16 = 23;
/// Sentinel for presses that hit the overlay but no widget: the whole gesture
/// is swallowed, so no ghost Moved/Ended without Began leaks into the game.
const W_SWALLOW: u16 = u16::MAX;
const W_CHANGED: u16 = 24;
const W_SAME: u16 = 25;
const W_INCREASED: u16 = 26;
const W_DECREASED: u16 = 27;
const W_WATCH_PAUSE: u16 = 28;
const W_WATCH_CLEAR: u16 = 29;
const W_WATCH_CLOSE: u16 = 30;
const W_IAP: u16 = 31;
const W_WATCH_NEWER: u16 = 45;
const W_WATCH_OLDER: u16 = 46;
const W_WATCH_FIELD: u16 = 47;
const W_WATCH_SET: u16 = 48;
const W_WATCH_DONE: u16 = 49;
const W_WATCH_KEY_BASE: u16 = 80; // + keypad index
const W_CHANGE_BASE: u16 = 40; // + row index; below keypad IDs
const W_SAVE: u16 = 13;
const W_SCROLL_UP: u16 = 14;
const W_SCROLL_DOWN: u16 = 15;
const W_RESULT_BASE: u16 = 32; // + row index
const W_KEY_BASE: u16 = 64; // + key index

#[derive(Copy, Clone, Debug)]
struct Rect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

impl Rect {
    fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
}

/// Rows of the on-screen keypad: digits, sign and editing keys.
const KEYPAD: [[Option<char>; 4]; 4] = [
    [Some('1'), Some('2'), Some('3'), Some('\u{8}')],
    [Some('4'), Some('5'), Some('6'), Some('\u{4}')],
    [Some('7'), Some('8'), Some('9'), Some('.')],
    [Some('-'), Some('0'), None, None],
];

struct Layout {
    scale: f32,
    button: Rect,
    panel: Option<PanelLayout>,
    monitor: Option<PanelLayout>,
}

impl Layout {
    fn panels(&self) -> impl Iterator<Item = &PanelLayout> {
        self.panel.iter().chain(self.monitor.iter())
    }
}

struct PanelLayout {
    rect: Rect,
    widgets: Vec<(u16, Rect)>,
    results_header_y: f32,
}

const RESULT_ROWS: usize = 5;

fn compute_layout(ui: &TrainerUi, viewport: (u32, u32, u32, u32)) -> Layout {
    // Coordinates are VIEWPORT-RELATIVE: the GL viewport already offsets
    // drawing by (vx, vy), and the touch handlers subtract it before
    // hit-testing. Adding the origin here would double it (panel shifted
    // by the letterbox offset, tap targets misaligned with drawn keys).
    let (_vx, _vy, vw, vh) = viewport;
    let (vx, vy, vw, vh) = (0.0_f32, 0.0_f32, vw as f32, vh as f32);
    // Fit the complete panel, including speed controls, in both orientations.
    let s = (vh / 545.0).min(vw / if ui.watch_open && ui.open { 600.0 } else { 320.0 }).clamp(0.1, 4.0);
    let btn = 30.0 * s;
    let button = Rect {
        x: vx + vw - btn - 6.0 * s,
        y: vy + 6.0 * s,
        w: btn,
        h: btn,
    };
    let panel: Option<PanelLayout> = if ui.open {
        let pw = (vw - 12.0 * s).min(300.0 * s);
        let px = vx + vw - pw - 6.0 * s;
        // Leave room for the button row above the panel.
        let py = button.y + button.h + 3.0 * s;
        let row_h = 22.0 * s;
        let small_h = 18.0 * s;
        let key_h = 24.0 * s;
        let mut widgets = Vec::new();
        let mut y = py + 3.0 * s;
        let mut push_widget = |id: u16, wx: f32, wy: f32, ww: f32, wh: f32, widgets: &mut Vec<(u16, Rect)>| {
            widgets.push((id, Rect { x: wx, y: wy, w: ww, h: wh }));
        };
        // Header row (title + close button).
        push_widget(W_CLOSE, px + pw - small_h - 4.0 * s, y, small_h, small_h, &mut widgets);
        y += small_h + 3.0 * s;
        // Type and latched safe-mode toggle share a row.
        push_widget(W_TYPE, px + 6.0 * s, y, 110.0 * s, row_h, &mut widgets);
        push_widget(W_SAFE_MODE, px + 120.0 * s, y, pw - 126.0 * s, row_h, &mut widgets);
        y += row_h + 3.0 * s;
        // Game speed: minus, current value (tap to reset), plus.
        let speed_side = 38.0 * s;
        push_widget(W_SPEED_DOWN, px + 6.0 * s, y, speed_side, row_h, &mut widgets);
        push_widget(W_SPEED_RESET, px + 48.0 * s, y, pw - 96.0 * s, row_h, &mut widgets);
        push_widget(W_SPEED_UP, px + pw - 44.0 * s, y, speed_side, row_h, &mut widgets);
        y += row_h + 3.0 * s;
        // Latched IAP emulation toggle (StoreKit free in-app purchases).
        push_widget(W_IAP, px + 6.0 * s, y, pw - 12.0 * s, row_h, &mut widgets);
        y += row_h + 3.0 * s;
        // Search value field.
        push_widget(W_FIELD_SEARCH, px + 6.0 * s, y, pw - 12.0 * s, row_h, &mut widgets);
        y += row_h + 3.0 * s;
        // Keypad: 4 columns x 4 rows.
        let key_w = (pw - 12.0 * s) / 4.0;
        for (row_idx, row) in KEYPAD.iter().enumerate() {
            for (col_idx, _) in row.iter().enumerate() {
                if row[col_idx].is_none() {
                    continue;
                }
                let kx = px + 6.0 * s + key_w * col_idx as f32;
                let ky = y + key_h * row_idx as f32;
                push_widget(
                    W_KEY_BASE + (row_idx * 4 + col_idx) as u16,
                    kx,
                    ky,
                    key_w,
                    key_h,
                    &mut widgets,
                );
            }
        }
        y += key_h * 4.0 + 3.0 * s;
        // Actions row.
        let third = (pw - 12.0 * s - 8.0 * s) / 3.0;
        for (i, id) in [W_SEARCH, W_REFINE, W_RESET].iter().enumerate() {
            push_widget(*id, px + 6.0 * s + (third + 4.0 * s) * i as f32, y, third, row_h, &mut widgets);
        }
        y += row_h + 3.0 * s;
        // Explicit before/after comparison: independent of live refresh.
        let fifth = (pw - 12.0 * s - 8.0 * s) / 5.0;
        for (i, id) in [W_MARK, W_CHANGED, W_SAME, W_INCREASED, W_DECREASED].iter().enumerate() {
            push_widget(*id, px + 6.0 * s + (fifth + 2.0 * s) * i as f32, y, fifth, row_h, &mut widgets);
        }
        y += row_h + 3.0 * s;
        // Category selector: guesses always keep a question mark.
        push_widget(W_CATEGORY, px + 6.0 * s, y, pw - 12.0 * s, row_h, &mut widgets);
        y += row_h + 3.0 * s;
        // Results header + scroll buttons.
        let results_header_y = y;
        let res_h = 16.0 * s;
        push_widget(W_SCROLL_UP, px + pw - 2.0 * (res_h + 3.0 * s), y, res_h, res_h, &mut widgets);
        push_widget(W_SCROLL_DOWN, px + pw - res_h - 3.0 * s, y, res_h, res_h, &mut widgets);
        y += res_h + 2.0 * s;
        // Result rows.
        for i in 0..RESULT_ROWS {
            push_widget(W_RESULT_BASE + i as u16, px + 6.0 * s, y, pw - 12.0 * s, res_h, &mut widgets);
            y += res_h;
        }
        y += 3.0 * s;
        // Set value field.
        push_widget(W_FIELD_SET, px + 6.0 * s, y, pw - 12.0 * s, row_h, &mut widgets);
        y += row_h + 3.0 * s;
        // Set / set-all / freeze row.
        let quarter = (pw - 12.0 * s - 12.0 * s) / 4.0;
        for (i, id) in [W_SET, W_SET_ALL, W_FREEZE, W_UNFREEZE].iter().enumerate() {
            push_widget(*id, px + 6.0 * s + (quarter + 4.0 * s) * i as f32, y, quarter, row_h, &mut widgets);
        }
        y += row_h + 3.0 * s;
        // Activity window with inline editing, plus dump/save.
        let third = (pw - 12.0 * s - 8.0 * s) / 3.0;
        for (i, id) in [W_DUMP, W_SAVE, W_WATCH].iter().enumerate() {
            push_widget(*id, px + 6.0 * s + (third + 4.0 * s) * i as f32, y, third, row_h, &mut widgets);
        }
        y += row_h + 3.0 * s;
        // Status line (not interactive).
        let ph = y + small_h + 3.0 * s - py;
        Some(PanelLayout {
            rect: Rect { x: px, y: py, w: pw, h: ph },
            widgets,
            results_header_y,
        })
    } else {
        None
    };
    let monitor = if ui.watch_open {
        // A compact independent window leaves the rest of the game touchable.
        // Beside the editor when it is open; larger when watching the game.
        let ms = if ui.open { s } else { (vw / 300.0).min(vh / 480.0).clamp(0.1, 2.0) };
        let height = if ui.watch_target.is_some() { 428.0 } else { 218.0 };
        let rect = Rect { x: 6.0 * ms, y: 42.0 * ms, w: 280.0 * ms, h: height * ms };
        let mut widgets = Vec::new();
        for (i, id) in [W_WATCH_PAUSE, W_WATCH_CLEAR, W_WATCH_CLOSE].iter().enumerate() {
            widgets.push((*id, Rect { x: rect.x + (128.0 + i as f32 * 48.0) * ms,
                y: rect.y + 3.0 * ms, w: 46.0 * ms, h: 23.0 * ms }));
        }
        for row in 0..RESULT_ROWS {
            widgets.push((W_CHANGE_BASE + row as u16, Rect { x: rect.x + 4.0 * ms,
                y: rect.y + (42.0 + row as f32 * 29.0) * ms,
                w: rect.w - 8.0 * ms, h: 28.0 * ms }));
        }
        for (i, id) in [W_WATCH_NEWER, W_WATCH_OLDER].iter().enumerate() {
            widgets.push((*id, Rect { x: rect.x + (4.0 + i as f32 * 138.0) * ms,
                y: rect.y + 190.0 * ms, w: 134.0 * ms, h: 23.0 * ms }));
        }
        if ui.watch_target.is_some() {
            for (id, x, width) in [(W_WATCH_FIELD, 4.0, 198.0), (W_WATCH_SET, 206.0, 70.0)] {
                widgets.push((id, Rect { x: rect.x + x * ms, y: rect.y + 242.0 * ms,
                    w: width * ms, h: 24.0 * ms }));
            }
            for (row, keys) in KEYPAD.iter().enumerate() {
                for (col, key) in keys.iter().enumerate() {
                    if key.is_some() {
                        widgets.push((W_WATCH_KEY_BASE + (row * 4 + col) as u16,
                            Rect { x: rect.x + (4.0 + col as f32 * 68.0) * ms,
                                y: rect.y + (270.0 + row as f32 * 26.0) * ms,
                                w: 66.0 * ms, h: 24.0 * ms }));
                    }
                }
            }
            widgets.push((W_WATCH_DONE, Rect { x: rect.x + 4.0 * ms,
                y: rect.y + 378.0 * ms, w: 272.0 * ms, h: 23.0 * ms }));
        }
        Some(PanelLayout { rect, widgets, results_header_y: rect.y + 28.0 * ms })
    } else { None };
    Layout { scale: s, button, panel, monitor }
}

// ---------------------------------------------------------------------------
// Touch input, called from crate::window with raw window-space coordinates.
// Returns true if the touch was consumed by the overlay.
// ---------------------------------------------------------------------------

fn overlay_active() -> bool {
    HARDWARE_ENABLED.load(Ordering::SeqCst) && {
        let ui = UI.lock().unwrap();
        ui.enabled && ui.app_id.is_some()
    }
}

pub fn touch_down(abs: (f32, f32), viewport: (u32, u32, u32, u32)) -> bool {
    if !overlay_active() {
        return false;
    }
    let mut ui = UI.lock().unwrap();
    let layout = compute_layout(&ui, viewport);
    let (vx, vy, _, _) = viewport;
    let (x, y) = (abs.0 - vx as f32, abs.1 - vy as f32);
    if layout.button.contains(x, y) {
        ui.pending = Some(W_BUTTON);
        return true;
    }
    for panel in layout.panels() {
        if !panel.rect.contains(x, y) { continue; }
        for (id, rect) in &panel.widgets {
            if rect.contains(x, y) {
                ui.pending = Some(*id);
                return true;
            }
        }
        // Inside the panel but not on a widget: swallow the WHOLE gesture so
        // the game never sees a Moved/Ended without the matching Began.
        ui.pending = Some(W_SWALLOW);
        return true;
    }
    false
}

pub fn touch_motion(abs: (f32, f32), _viewport: (u32, u32, u32, u32)) -> bool {
    if !overlay_active() {
        return false;
    }
    let ui = UI.lock().unwrap();
    // Swallow motion while a press is pending so the game doesn't see drags
    // that started on the overlay.
    ui.pending.is_some()
}

pub fn touch_up(abs: (f32, f32), viewport: (u32, u32, u32, u32)) -> bool {
    if !overlay_active() {
        return false;
    }
    let mut ui = UI.lock().unwrap();
    let layout = compute_layout(&ui, viewport);
    let (vx, vy, _, _) = viewport;
    let (x, y) = (abs.0 - vx as f32, abs.1 - vy as f32);
    let Some(pending) = ui.pending.take() else {
        // The Down went to the game, so the Up belongs to the game too.
        // Eating it here used to leave a stuck touch in the game: its next
        // swipe/tap then had no touchesBegan and was ignored until the host
        // recycled the finger id (the Subway Surfers "swipes randomly dead"
        // bug). A stray release over the panel activates nothing.
        return false;
    };
    // Activate if the finger is released over the same widget.
    let hit = pending == W_BUTTON && layout.button.contains(x, y)
        || layout.panels().any(|p| p.widgets.iter().any(|(id, r)| *id == pending && r.contains(x, y)));
    if hit {
        activate_widget(&mut ui, pending);
    }
    true
}

fn activate_widget(ui: &mut TrainerUi, id: u16) {
    // Editing inputs, closing the panel or taking any other action cancels
    // confirmation. A later click must preview again, never apply silently.
    if id != W_SET_ALL {
        ui.bulk_preview = false;
        COMMANDS.lock().unwrap().push(TrainerCmd::CancelBulk);
    }
    match id {
        W_BUTTON => ui.open = !ui.open,
        W_WATCH => ui.watch_open = !ui.watch_open,
        W_WATCH_CLOSE => ui.watch_open = false,
        W_WATCH_PAUSE => {
            ui.watch_paused = !ui.watch_paused;
            ui.activity_scroll = 0;
        }
        W_WATCH_NEWER | W_WATCH_OLDER => {
            ui.watch_paused = true;
            ui.activity_scroll = if id == W_WATCH_NEWER {
                ui.activity_scroll.saturating_sub(RESULT_ROWS)
            } else {
                (ui.activity_scroll + RESULT_ROWS).min(ui.activity.len().saturating_sub(RESULT_ROWS))
            };
        }
        W_WATCH_CLEAR => {
            ui.activity.clear();
            ui.activity_scroll = 0;
            ui.activity_changed = 0;
            COMMANDS.lock().unwrap().push(TrainerCmd::ClearActivity);
        }
        W_MARK => COMMANDS.lock().unwrap().push(TrainerCmd::Mark),
        W_CHANGED | W_SAME | W_INCREASED | W_DECREASED => {
            let filter = match id {
                W_CHANGED => WatchFilter::Changed, W_SAME => WatchFilter::Same,
                W_INCREASED => WatchFilter::Increased, _ => WatchFilter::Decreased,
            };
            COMMANDS.lock().unwrap().push(TrainerCmd::Compare(filter));
        }
        id if (W_CHANGE_BASE..W_CHANGE_BASE + RESULT_ROWS as u16).contains(&id) => {
            if let Some(change) = ui.activity.get(ui.activity_scroll + (id - W_CHANGE_BASE) as usize) {
                // Pin identity, not row index: subsequent live events cannot
                // redirect the edit to a different address or AUTO alias.
                ui.watch_target = Some((change.addr, change.vtype));
                ui.watch_current = None;
                ui.watch_text = change.vtype.format(change.after);
                ui.watch_status = "SET edits this address only; still risky".to_string();
                COMMANDS.lock().unwrap().push(TrainerCmd::RefreshView);
            }
        }
        W_WATCH_FIELD => {}
        W_WATCH_DONE => {
            ui.watch_target = None;
            ui.watch_current = None;
            ui.watch_text.clear();
            ui.watch_status.clear();
        }
        W_WATCH_SET => {
            if let Some((addr, vtype)) = ui.watch_target {
                if ui.watch_current.is_some() {
                    COMMANDS.lock().unwrap().push(TrainerCmd::WatchSet {
                        addr, vtype, text: ui.watch_text.trim().to_string(),
                    });
                } else {
                    ui.watch_status = "ADDRESS UNAVAILABLE: WAIT OR SEARCH AGAIN".to_string();
                }
            }
        }
        id if (W_WATCH_KEY_BASE..W_WATCH_KEY_BASE + 16).contains(&id) => {
            let index = (id - W_WATCH_KEY_BASE) as usize;
            if let Some(ch) = KEYPAD[index / 4][index % 4] {
                match ch {
                    '\u{8}' => { ui.watch_text.pop(); }
                    '\u{4}' => ui.watch_text.clear(),
                    _ if ui.watch_text.len() < 24 => ui.watch_text.push(ch),
                    _ => {}
                }
            }
        }
        W_CLOSE => ui.open = false,
        W_TYPE => ui.vtype = ui.vtype.next(),
        W_SAFE_MODE => {
            ui.safe_mode = !ui.safe_mode;
            ui.status = if ui.safe_mode {
                "SAFE MODE ON: FILTER BULK WRITES"
            } else {
                "SAFE MODE OFF: EXTRA CRASH RISK"
            }.to_string();
        }
        W_IAP => {
            ui.iap = !ui.iap;
            crate::frameworks::store_kit::set_emulation_enabled(ui.iap);
            ui.status = if ui.iap {
                "IN-APP ON: PURCHASES FREE"
            } else {
                "IN-APP OFF: STOCK STORE STUBS"
            }
            .to_string();
        }
        W_SPEED_DOWN | W_SPEED_RESET | W_SPEED_UP => {
            ui.speed = match id {
                W_SPEED_DOWN => ui.speed.step(false),
                W_SPEED_UP => ui.speed.step(true),
                _ => Speed::Normal,
            };
            ui.speed_dirty = true;
        }
        W_CATEGORY => {
            ui.filter = ui.filter.next();
            ui.scroll = 0;
            ui.selected = None;
            ui.selected_type = None;
            ui.results.clear();
            ui.filtered_results = match ui.filter {
                ResultFilter::All => ui.total_results,
                ResultFilter::Category(c) => ui.category_counts[c.index()],
            };
            ui.status = "Guesses only; not verified".to_string();
            COMMANDS.lock().unwrap().push(TrainerCmd::RefreshView);
        }
        W_FIELD_SEARCH => ui.focus = Focus::Search,
        W_FIELD_SET => ui.focus = Focus::Set,
        W_SEARCH => {
            let text = ui.search_text.trim().to_string();
            COMMANDS
                .lock()
                .unwrap()
                .push(TrainerCmd::Search { vtype: ui.vtype, text });
        }
        W_REFINE => {
            let text = ui.search_text.trim().to_string();
            COMMANDS
                .lock()
                .unwrap()
                .push(TrainerCmd::Refine { vtype: ui.vtype, text });
        }
        W_RESET => {
            COMMANDS.lock().unwrap().push(TrainerCmd::Reset);
        }
        W_SET => {
            let text = ui.set_text.trim().to_string();
            COMMANDS.lock().unwrap().push(TrainerCmd::Set { vtype: ui.vtype, text });
        }
        W_SET_ALL => {
            let text = ui.set_text.trim().to_string();
            let confirm = ui.bulk_preview;
            ui.bulk_preview = false;
            COMMANDS.lock().unwrap().push(TrainerCmd::SetAll {
                vtype: ui.vtype, text, confirm, safe_mode: ui.safe_mode, filter: ui.filter,
            });
        }
        W_FREEZE => {
            let text = ui.set_text.trim().to_string();
            COMMANDS
                .lock()
                .unwrap()
                .push(TrainerCmd::Freeze { vtype: ui.vtype, text });
        }
        W_UNFREEZE => {
            COMMANDS.lock().unwrap().push(TrainerCmd::UnfreezeAll);
        }
        W_DUMP => {
            COMMANDS.lock().unwrap().push(TrainerCmd::Dump);
        }
        W_SAVE => {
            COMMANDS.lock().unwrap().push(TrainerCmd::SaveHack { vtype: ui.vtype });
        }
        W_SCROLL_UP | W_SCROLL_DOWN => {
            let last = ui.filtered_results.saturating_sub(RESULT_ROWS);
            ui.scroll = if id == W_SCROLL_UP {
                ui.scroll.saturating_sub(RESULT_ROWS)
            } else {
                (ui.scroll + RESULT_ROWS).min(last)
            };
            ui.results.clear();
            ui.selected = None;
            ui.selected_type = None;
            COMMANDS.lock().unwrap().push(TrainerCmd::RefreshView);
        }
        id if (W_RESULT_BASE..W_RESULT_BASE + RESULT_ROWS as u16).contains(&id) => {
            let idx = (id - W_RESULT_BASE) as usize;
            if let Some(result) = ui.results.get(idx) {
                ui.selected = Some(result.addr);
                ui.selected_type = Some(result.vtype);
                ui.status = format!("{}: {}", result.analysis.category.label(), result.analysis.description());
                COMMANDS.lock().unwrap().push(TrainerCmd::InspectSelection);
            }
        }
        id if (W_KEY_BASE..W_KEY_BASE + 16).contains(&id) => {
            let key_idx = (id - W_KEY_BASE) as usize;
            let row = key_idx / 4;
            let col = key_idx % 4;
            if let Some(Some(ch)) = KEYPAD.get(row).and_then(|r| r.get(col)) {
                let field = match ui.focus {
                    Focus::Search => &mut ui.search_text,
                    Focus::Set => &mut ui.set_text,
                };
                match ch {
                    '\u{8}' => {
                        field.pop();
                    }
                    '\u{4}' => field.clear(),
                    _ => {
                        if field.len() < 15 {
                            field.push(*ch);
                        }
                    }
                }
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Drawing. Only the GLES 1.x fixed-function API is used, matching
// present_frame's own overlay, so this works on every backend.
// ---------------------------------------------------------------------------

const FONT_PX: f32 = 14.0;
const ATLAS_CHARS: &str = " !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~";

struct GlyphCell {
    /// Atlas-space position and size of the glyph bitmap.
    u0: f32,
    v0: f32,
    u1: f32,
    v1: f32,
    /// Per-character screen metrics (at font size units).
    advance: f32,
    draw_dx: f32,
    draw_dy: f32,
    draw_w: f32,
    draw_h: f32,
}

struct Atlas {
    tex: GLuint,
    height: f32,
    glyphs: Vec<Option<GlyphCell>>,
    atlas_w: usize,
    atlas_h: usize,
    bitmap: Vec<u8>,
}

static ATLAS: OnceLock<Mutex<Option<Atlas>>> = OnceLock::new();

/// Drop the cached glyph atlas (e.g. because the GL context changed).
pub fn invalidate_atlas() {
    if let Some(lock) = ATLAS.get() {
        *lock.lock().unwrap() = None;
    }
}

fn char_index(ch: char) -> Option<usize> {
    ATLAS_CHARS.chars().position(|c| c == ch)
}

unsafe fn build_atlas(gles: &mut dyn GLES) -> Option<Atlas> {
    let font = Font::mono_regular();
    let units_per_em = font.units_per_em() as f32;
    let mut raw: Vec<(char, f32, (f32, f32), (i32, i32), Vec<f32>)> = Vec::new();
    let mut min_y = f32::MAX;
    let mut max_y = f32::MIN;
    for ch in ATLAS_CHARS.chars() {
        let glyph_id = font.glyph_id_for_char(ch as u16);
        let advance = font.glyph_advance(glyph_id) as f32 * FONT_PX / units_per_em;
        let mut captured: Option<((f32, f32), (i32, i32), Vec<f32>)> = None;
        font.draw(
            FONT_PX,
            &ch.to_string(),
            (0.0, 0.0),
            None,
            TextAlignment::Left,
            |glyph| {
                let (origin, dims) = (glyph.origin(), glyph.dimensions());
                if dims.0 <= 0 || dims.1 <= 0 {
                    return;
                }
                let mut pixels = Vec::with_capacity((dims.0 * dims.1) as usize);
                for y in 0..dims.1 {
                    for x in 0..dims.0 {
                        pixels.push(glyph.pixel_at((x, y)));
                    }
                }
                captured = Some((origin, dims, pixels));
            },
        );
        if let Some((origin, dims, pixels)) = captured {
            min_y = min_y.min(origin.1);
            max_y = max_y.max(origin.1 + dims.1 as f32);
            raw.push((ch, advance, origin, dims, pixels));
        } else {
            raw.push((ch, advance, (0.0, 0.0), (0, 0), Vec::new()));
        }
    }
    if min_y > max_y {
        return None;
    }
    let cell_h = max_y - min_y;
    let mut atlas_w = 0.0f32;
    // Pixel-space U range of each glyph inside the atlas row. Converted to
    // 0..1 UVs once the final atlas width is known (dividing by the running
    // width here AND by the final width below would corrupt the coordinates).
    let mut u_ranges_px: Vec<(f32, f32)> = Vec::with_capacity(ATLAS_CHARS.len());
    let mut cells: Vec<Option<GlyphCell>> = Vec::with_capacity(ATLAS_CHARS.len());
    for (_ch, advance, _origin, dims, _pixels) in &raw {
        let gw = if dims.0 > 0 { dims.0 as f32 } else { 0.0 };
        let u0 = atlas_w + 1.0; // 1px padding to avoid bleeding
        let u1 = u0 + gw;
        u_ranges_px.push((u0, u1));
        cells.push(Some(GlyphCell {
            u0: 0.0,
            v0: 0.0,
            u1: 0.0,
            v1: 0.0,
            advance: *advance,
            draw_dx: 0.0,
            draw_dy: 0.0,
            draw_w: gw,
            draw_h: dims.1 as f32,
        }));
        atlas_w = u1 + 1.0;
    }
    // Second pass to fix UVs now that atlas_w is known, and to blit pixels.
    let atlas_w = atlas_w.ceil() as usize;
    let atlas_h = cell_h.ceil() as usize;
    let mut bitmap = vec![0u8; atlas_w * atlas_h * 4];
    for (i, (cell, (_ch, _adv, origin, dims, pixels))) in
        cells.iter_mut().zip(raw.iter()).enumerate()
    {
        let Some(cell) = cell else { continue };
        if dims.0 <= 0 || dims.1 <= 0 {
            continue;
        }
        let (u0_px, u1_px) = u_ranges_px[i];
        let bx = u0_px.round() as usize;
        // Vertical placement of this glyph's bitmap inside the atlas.
        let by = ((origin.1 - min_y).round() as usize).min(atlas_h.saturating_sub(1));
        for y in 0..dims.1 as usize {
            for x in 0..dims.0 as usize {
                let coverage = pixels[y * dims.0 as usize + x];
                let idx = ((by + y) * atlas_w + bx + x) * 4;
                if idx + 3 < bitmap.len() {
                    bitmap[idx] = 255;
                    bitmap[idx + 1] = 255;
                    bitmap[idx + 2] = 255;
                    bitmap[idx + 3] = (coverage * 255.0).clamp(0.0, 255.0) as u8;
                }
            }
        }
        // Texture coordinates. GL textures have their origin at the BOTTOM
        // left (the first byte of the uploaded data is v=0), while our bitmap
        // has row 0 at the top — so v grows downwards through the bitmap as
        // the coordinate DECREASES from 1.
        let atlas_h_f = atlas_h as f32;
        let atlas_w_f = atlas_w as f32;
        cell.u0 = u0_px / atlas_w_f;
        cell.u1 = u1_px / atlas_w_f;
        // v0 = top of the glyph band, v1 = bottom (matches the quad corners
        // in push_text, where v0 is used at the top edge).
        // The first row uploaded (bitmap row 0, the glyph band's top) is
        // sampled at v = 0, so v simply grows downward through the bitmap:
        // no `1 -` inversion here — that would flip every glyph upside down.
        cell.v0 = by as f32 / atlas_h_f;
        cell.v1 = (by as f32 + dims.1 as f32) / atlas_h_f;
        cell.draw_dx = 0.0;
        cell.draw_dy = origin.1 - min_y;
        cell.draw_w = dims.0 as f32;
        cell.draw_h = dims.1 as f32;
    }
    let mut tex: GLuint = 0;
    gles.GenTextures(1, &mut tex);
    gles.BindTexture(gles11::TEXTURE_2D, tex);
    gles.TexImage2D(
        gles11::TEXTURE_2D,
        0,
        gles11::RGBA as _,
        atlas_w as _,
        atlas_h as _,
        0,
        gles11::RGBA,
        gles11::UNSIGNED_BYTE,
        bitmap.as_ptr() as *const _,
    );
    gles.TexParameteri(gles11::TEXTURE_2D, gles11::TEXTURE_MIN_FILTER, gles11::LINEAR as _);
    gles.TexParameteri(gles11::TEXTURE_2D, gles11::TEXTURE_MAG_FILTER, gles11::LINEAR as _);
    gles.TexParameteri(gles11::TEXTURE_2D, gles11::TEXTURE_WRAP_S, gles11::CLAMP_TO_EDGE as _);
    gles.TexParameteri(gles11::TEXTURE_2D, gles11::TEXTURE_WRAP_T, gles11::CLAMP_TO_EDGE as _);
    Some(Atlas {
        tex,
        height: cell_h,
        glyphs: cells,
        atlas_w,
        atlas_h,
        bitmap,
    })
}

unsafe fn ensure_atlas(gles: &mut dyn GLES) -> Option<()> {
    {
        let lock = ATLAS.get_or_init(|| Mutex::new(None)).lock().unwrap();
        if let Some(atlas) = lock.as_ref() {
            if gles.IsTexture(atlas.tex) != 0 {
                return Some(());
            }
        }
    }
    let mut guard = ATLAS.get_or_init(|| Mutex::new(None)).lock().unwrap();
    match guard.as_mut() {
        // Bitmap/cells are still valid — only re-create the GL texture.
        Some(atlas) if gles.IsTexture(atlas.tex) == 0 => {
            atlas.tex = upload_atlas_texture(gles, &atlas.bitmap, atlas.atlas_w, atlas.atlas_h);
            Some(())
        }
        Some(_) => Some(()),
        None => {
            *guard = build_atlas(gles);
            guard.is_some().then_some(())
        }
    }
}

/// Create and fill a texture from raw RGBA atlas pixels.
unsafe fn upload_atlas_texture(
    gles: &mut dyn GLES,
    bitmap: &[u8],
    atlas_w: usize,
    atlas_h: usize,
) -> GLuint {
    let mut tex: GLuint = 0;
    gles.GenTextures(1, &mut tex);
    gles.BindTexture(gles11::TEXTURE_2D, tex);
    gles.TexImage2D(
        gles11::TEXTURE_2D,
        0,
        gles11::RGBA as _,
        atlas_w as _,
        atlas_h as _,
        0,
        gles11::RGBA,
        gles11::UNSIGNED_BYTE,
        bitmap.as_ptr() as *const _,
    );
    gles.TexParameteri(gles11::TEXTURE_2D, gles11::TEXTURE_MIN_FILTER, gles11::LINEAR as _);
    gles.TexParameteri(gles11::TEXTURE_2D, gles11::TEXTURE_MAG_FILTER, gles11::LINEAR as _);
    gles.TexParameteri(gles11::TEXTURE_2D, gles11::TEXTURE_WRAP_S, gles11::CLAMP_TO_EDGE as _);
    gles.TexParameteri(gles11::TEXTURE_2D, gles11::TEXTURE_WRAP_T, gles11::CLAMP_TO_EDGE as _);
    tex
}

/// Draw a solid rectangle.
/// One drawable rectangle, positioned in viewport pixel space. `tex == 0`
/// means a solid quad (no texture); otherwise the atlas texture is sampled.
#[derive(Clone, Copy)]
struct Quad {
    tex: GLuint,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    u0: f32,
    v0: f32,
    u1: f32,
    v1: f32,
    col: (f32, f32, f32, f32),
}

fn quad_vertices(q: &Quad) -> [f32; 16] {
    let (x, y, w, h) = (q.x, q.y, q.w, q.h);
    let (u0, v0, u1, v1) = (q.u0, q.v0, q.u1, q.v1);
    // Interleaved pos(2) + uv(2), triangle strip: TL, BL, TR, BR.
    [
        x, y, u0, v0,
        x, y + h, u0, v1,
        x + w, y, u1, v0,
        x + w, y + h, u1, v1,
    ]
}

/// Push a solid rectangle onto the scene.
fn push_rect(quads: &mut Vec<Quad>, rect: Rect, color: (f32, f32, f32, f32)) {
    quads.push(Quad {
        tex: 0,
        x: rect.x,
        y: rect.y,
        w: rect.w,
        h: rect.h,
        u0: 0.0,
        v0: 0.0,
        u1: 1.0,
        v1: 1.0,
        col: color,
    });
}

/// Draw text with the cached atlas, top-left at (x, y), pixel height
/// `px_size`. Returns the width of the drawn text.
/// Push text (as atlas glyph quads) onto the scene, top-left at (x, y),
/// pixel height `px_size`. Returns the width of the text.
fn push_text(
    quads: &mut Vec<Quad>,
    atlas: &Atlas,
    text: &str,
    x: f32,
    y: f32,
    px_size: f32,
    color: (f32, f32, f32, f32),
) -> f32 {
    let scale = px_size / FONT_PX;
    let mut cursor = x;
    for ch in text.chars() {
        let Some(idx) = char_index(ch) else {
            cursor += px_size * 0.3;
            continue;
        };
        let Some(cell) = &atlas.glyphs[idx] else { continue; };
        let gx = cursor + cell.draw_dx * scale;
        let gy = y + cell.draw_dy * scale;
        let gw = cell.draw_w * scale;
        let gh = cell.draw_h * scale;
        if gw > 0.0 && gh > 0.0 {
            quads.push(Quad {
                tex: atlas.tex,
                x: gx,
                y: gy,
                w: gw,
                h: gh,
                u0: cell.u0,
                v0: cell.v0,
                u1: cell.u1,
                v1: cell.v1,
                col: color,
            });
        }
        cursor += cell.advance * scale;
    }
    cursor - x
}

const COL_PANEL: (f32, f32, f32, f32) = (0.03, 0.03, 0.04, 1.0);
const COL_WIDGET: (f32, f32, f32, f32) = (0.22, 0.22, 0.24, 1.0);
const COL_WIDGET_LIT: (f32, f32, f32, f32) = (0.0, 0.5, 0.25, 1.0);
const COL_FIELD: (f32, f32, f32, f32) = (0.12, 0.12, 0.14, 1.0);
const COL_TEXT: (f32, f32, f32, f32) = (0.95, 0.95, 0.95, 1.0);
const COL_TEXT_DIM: (f32, f32, f32, f32) = (0.6, 0.6, 0.62, 1.0);
const COL_ACCENT: (f32, f32, f32, f32) = (0.0, 0.75, 0.35, 0.92);
const COL_SELECTED: (f32, f32, f32, f32) = (0.15, 0.3, 0.6, 1.0);
const COL_CHANGED: (f32, f32, f32, f32) = (0.75, 0.55, 0.0, 0.95);
const COL_SAFE_MODE_ON: (f32, f32, f32, f32) = (1.0, 0.8, 0.1, 1.0);
const COL_SAFE_MODE_TEXT: (f32, f32, f32, f32) = (0.08, 0.07, 0.02, 1.0);

fn safe_mode_colors(enabled: bool) -> ((f32, f32, f32, f32), (f32, f32, f32, f32)) {
    if enabled {
        (COL_SAFE_MODE_ON, COL_SAFE_MODE_TEXT)
    } else {
        (COL_WIDGET, COL_TEXT)
    }
}

/// Entry point called from present_frame (CA composition and native ES 1.1
/// present paths), in viewport pixel space. Renders with GLES 1.x
/// fixed-function calls.
pub unsafe fn draw(gles: &mut dyn GLES, viewport: (u32, u32, u32, u32)) {
    let Some(quads) = build_scene(gles, viewport) else {
        return;
    };
    render_gles1(gles, viewport, &quads);
}

/// Entry point for native OpenGL ES 2.0 present paths (see
/// present_renderbuffer_es2), which lack the fixed-function pipeline.
pub(crate) fn prepare_overlay_context(context_token: usize) {
    let mut stored = OVERLAY_CONTEXT_TOKEN.lock().unwrap();
    if *stored != Some(context_token) {
        *stored = Some(context_token);
        drop(stored);
        *OVERLAY_PROGRAM.lock().unwrap() = None;
        *OVERLAY_VBO.lock().unwrap() = None;
        *OVERLAY_WHITE_TEX.lock().unwrap() = None;
        invalidate_atlas();
    }
}

pub unsafe fn draw_es2(
    gles: &mut dyn GLES,
    viewport: (u32, u32, u32, u32),
    context_token: usize,
) {
    prepare_overlay_context(context_token);
    let Some(quads) = build_scene(gles, viewport) else {
        return;
    };
    render_es2(gles, viewport, &quads);
}

pub unsafe fn draw_texture_es2(
    gles: &mut dyn GLES,
    viewport: (u32, u32, u32, u32),
    context_token: usize,
    texture: GLuint,
    rect: (f32, f32, f32, f32),
    uv: (f32, f32),
) {
    prepare_overlay_context(context_token);
    let quad = Quad {
        tex: texture,
        x: rect.0,
        y: rect.1,
        w: rect.2,
        h: rect.3,
        u0: 0.0,
        v0: 0.0,
        u1: uv.0,
        v1: uv.1,
        col: (1.0, 1.0, 1.0, 1.0),
    };
    render_es2(gles, viewport, &[quad]);
}

/// A scene of rectangles and text that another on-screen overlay can build and
/// hand to `draw_scene` or `draw_scene_es2`.
///
/// Other overlays (the quick menu, for one) need the same glyph atlas and the
/// same two renderers, and duplicating the font machinery for each of them
/// would be silly, so it is exposed here instead.
pub struct OverlayScene {
    quads: Vec<Quad>,
}

impl Default for OverlayScene {
    fn default() -> Self {
        Self::new()
    }
}

impl OverlayScene {
    pub fn new() -> Self {
        Self { quads: Vec::new() }
    }

    /// Adds a solid rectangle, in viewport pixel space.
    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: (f32, f32, f32, f32)) {
        push_rect(&mut self.quads, Rect { x, y, w, h }, color);
    }

    /// Adds `text` with its top-left at `(x, y)` and glyphs `px_size` pixels
    /// high, and returns the width it took. `gles` is needed because the glyph
    /// atlas is built lazily, on first use.
    pub unsafe fn text(
        &mut self,
        gles: &mut dyn GLES,
        text: &str,
        x: f32,
        y: f32,
        px_size: f32,
        color: (f32, f32, f32, f32),
    ) -> f32 {
        if ensure_atlas(gles).is_none() {
            return 0.0;
        }
        let atlas_guard = ATLAS.get().unwrap().lock().unwrap();
        let Some(atlas) = atlas_guard.as_ref() else {
            return 0.0;
        };
        push_text(&mut self.quads, atlas, text, x, y, px_size, color)
    }
}

/// Draws a scene built with `OverlayScene` on a fixed-function (GLES 1.x)
/// present path.
pub unsafe fn draw_scene(
    gles: &mut dyn GLES,
    viewport: (u32, u32, u32, u32),
    scene: &OverlayScene,
) {
    if scene.quads.is_empty() {
        return;
    }
    render_gles1(gles, viewport, &scene.quads);
}

/// Draws a scene built with `OverlayScene` on a native OpenGL ES 2.0 present
/// path.
pub unsafe fn draw_scene_es2(
    gles: &mut dyn GLES,
    viewport: (u32, u32, u32, u32),
    context_token: usize,
    scene: &OverlayScene,
) {
    prepare_overlay_context(context_token);
    if scene.quads.is_empty() {
        return;
    }
    render_es2(gles, viewport, &scene.quads);
}

/// Build the overlay scene (shared by both renderers). Returns None when the
/// overlay is disabled or the glyph atlas is unavailable.
unsafe fn build_scene(
    gles: &mut dyn GLES,
    viewport: (u32, u32, u32, u32),
) -> Option<Vec<Quad>> {
    if !HARDWARE_ENABLED.load(Ordering::SeqCst) {
        return None;
    }
    let mut ui_state = UI.lock().unwrap();
    if !ui_state.enabled || ui_state.app_id.is_none() {
        return None;
    }
    if ensure_atlas(gles).is_none() {
        return None;
    }
    let atlas_guard = ATLAS.get().unwrap().lock().unwrap();
    let Some(atlas) = atlas_guard.as_ref() else { return None; };

    let mut quads: Vec<Quad> = Vec::new();

    let layout = compute_layout(&ui_state, viewport);

    // Floating button.
    let lit = ui_state.pending == Some(W_BUTTON) || ui_state.open;
    push_rect(&mut quads, layout.button, if lit { COL_WIDGET_LIT } else { COL_ACCENT });
    let bs = layout.scale;
    let label = if ui_state.open { "X" } else { "CE" };
    let tw = text_width(atlas, label, 12.0 * bs);
    push_text(&mut quads,
        atlas,
        label,
        layout.button.x + (layout.button.w - tw) / 2.0,
        layout.button.y + (layout.button.h - atlas.height * (12.0 * bs / FONT_PX)) / 2.0,
        12.0 * bs,
        COL_TEXT,
    );

    if let Some(monitor) = &layout.monitor {
        let ms = monitor.rect.w / 280.0;
        push_rect(&mut quads, monitor.rect, COL_PANEL);
        push_text(&mut quads, atlas, if ui_state.watch_paused { "CHANGES PAUSED" } else { "LIVE CHANGES" },
            monitor.rect.x + 5.0 * ms, monitor.rect.y + 9.0 * ms, 10.0 * ms, COL_ACCENT);
        let counts = if ui_state.activity_tracked == 0 {
            "Search a value first; then play".to_string()
        } else {
            format!("{} changed / {} tracked{}", ui_state.activity_changed, ui_state.activity_tracked,
                if ui_state.activity_changed > 256 { " (sampled)" } else { "" })
        };
        let size = 9.0 * ms;
        let size = size * ((monitor.rect.w - 10.0 * ms) / text_width(atlas, &counts, size).max(1.0)).min(1.0);
        push_text(&mut quads, atlas, &counts, monitor.rect.x + 5.0 * ms,
            monitor.results_header_y + 2.0 * ms, size, COL_TEXT_DIM);
        for &(id, rect) in &monitor.widgets {
            push_rect(&mut quads, rect, COL_WIDGET);
            let label = match id {
                W_WATCH_PAUSE => Some(if ui_state.watch_paused { "RESUME" } else { "PAUSE" }),
                W_WATCH_CLEAR => Some("CLEAR"), W_WATCH_CLOSE => Some("X"),
                W_WATCH_NEWER => Some("< NEWER"), W_WATCH_OLDER => Some("OLDER >"), _ => None,
            };
            if let Some(label) = label {
                push_text(&mut quads, atlas, label, rect.x + 3.0 * ms, rect.y + 7.0 * ms, 9.0 * ms, COL_TEXT);
            } else if (W_CHANGE_BASE..W_CHANGE_BASE + RESULT_ROWS as u16).contains(&id) {
                let Some(change) = ui_state.activity.get(ui_state.activity_scroll + (id - W_CHANGE_BASE) as usize) else { continue; };
                if ui_state.watch_target == Some((change.addr, change.vtype)) {
                    push_rect(&mut quads, rect, COL_SELECTED);
                }
                let fresh = !ui_state.watch_paused && change.at.elapsed().as_secs_f32() < 0.75;
                let title = format!("0x{:08X} {}  {:.1}s ago", change.addr, change.vtype.name(), change.at.elapsed().as_secs_f32());
                let values = format!("{} -> {}", change.vtype.format(change.before), change.vtype.format(change.after));
                for (line, text) in [title, values].iter().enumerate() {
                    let size = 10.0 * ms;
                    let size = size * ((rect.w - 10.0 * ms) / text_width(atlas, text, size).max(1.0)).min(1.0);
                    push_text(&mut quads, atlas, text, rect.x + 4.0 * ms,
                        rect.y + (2.0 + line as f32 * 13.0) * ms, size,
                        if fresh { COL_ACCENT } else { COL_TEXT });
                }
            } else {
                let label = match id {
                    W_WATCH_FIELD => format!("NEW: {}_", ui_state.watch_text),
                    W_WATCH_SET => "SET".to_string(),
                    W_WATCH_DONE => "DONE (keep watching)".to_string(),
                    id if (W_WATCH_KEY_BASE..W_WATCH_KEY_BASE + 16).contains(&id) => {
                        let index = (id - W_WATCH_KEY_BASE) as usize;
                        match KEYPAD[index / 4][index % 4] {
                            Some('\u{8}') => "DEL".to_string(),
                            Some('\u{4}') => "CLR".to_string(),
                            Some(ch) => ch.to_string(), None => String::new(),
                        }
                    }
                    _ => String::new(),
                };
                let size = 11.0 * ms;
                let size = size * ((rect.w - 8.0 * ms) / text_width(atlas, &label, size).max(1.0)).min(1.0);
                push_text(&mut quads, atlas, &label, rect.x + 4.0 * ms, rect.y + 6.0 * ms, size, COL_TEXT);
            }
        }
        if let Some((addr, vtype)) = ui_state.watch_target {
            let current = ui_state.watch_current.map(|v| vtype.format(v)).unwrap_or_else(|| "unavailable".to_string());
            let target = format!("0x{:08X} {} NOW: {}", addr, vtype.name(), current);
            for (y, text) in [(222.0, target.as_str()), (407.0, ui_state.watch_status.as_str())] {
                let size = 10.0 * ms;
                let size = size * ((monitor.rect.w - 10.0 * ms) / text_width(atlas, text, size).max(1.0)).min(1.0);
                push_text(&mut quads, atlas, text, monitor.rect.x + 5.0 * ms,
                    monitor.rect.y + y * ms, size, COL_ACCENT);
            }
        }
    }

    if let Some(panel) = &layout.panel {
        push_rect(&mut quads, panel.rect, COL_PANEL);

        // Header.
        let title = "Cheat Engine";
        push_text(&mut quads,
            atlas,
            title,
            panel.rect.x + 8.0 * bs,
            panel.rect.y + 7.0 * bs,
            12.0 * bs,
            COL_ACCENT,
        );

        // Widget pass: draw every widget by id.
        for (id, rect) in &panel.widgets {
            match *id {
                W_CLOSE => {
                    push_rect(&mut quads, *rect, COL_WIDGET);
                    let tw = text_width(atlas, "X", 12.0 * bs);
                    push_text(&mut quads,
                        atlas,
                        "X",
                        rect.x + (rect.w - tw) / 2.0,
                        rect.y + (rect.h - atlas.height * (12.0 * bs / FONT_PX)) / 2.0,
                        12.0 * bs,
                        COL_TEXT,
                    );
                }
                W_TYPE => {
                    push_rect(&mut quads, *rect, COL_WIDGET);
                    let label = format!("TYPE: {}", ui_state.vtype.name());
                    push_text(&mut quads, atlas, &label, rect.x + 6.0 * bs, rect.y + 5.0 * bs, 12.0 * bs, COL_TEXT);
                }
                W_SAFE_MODE => {
                    let (background, foreground) = safe_mode_colors(ui_state.safe_mode);
                    push_rect(&mut quads, *rect, background);
                    let label = "SAFE MODE";
                    let size = 11.0 * bs;
                    let tw = text_width(atlas, label, size);
                    push_text(&mut quads, atlas, label,
                        rect.x + (rect.w - tw) / 2.0,
                        rect.y + (rect.h - atlas.height * (size / FONT_PX)) / 2.0,
                        size, foreground,
                    );
                }
                W_IAP => {
                    let (background, foreground) = safe_mode_colors(ui_state.iap);
                    push_rect(&mut quads, *rect, background);
                    let label = "IN-APP: FREE BUYS";
                    let size = 11.0 * bs;
                    let tw = text_width(atlas, label, size);
                    push_text(&mut quads, atlas, label,
                        rect.x + (rect.w - tw) / 2.0,
                        rect.y + (rect.h - atlas.height * (size / FONT_PX)) / 2.0,
                        size, foreground,
                    );
                }
                W_SPEED_DOWN | W_SPEED_RESET | W_SPEED_UP => {
                    push_rect(&mut quads, *rect, if *id == W_SPEED_RESET && ui_state.speed != Speed::Normal {
                        COL_WIDGET_LIT
                    } else { COL_WIDGET });
                    let label = match *id {
                        W_SPEED_DOWN => "-".to_string(),
                        W_SPEED_UP => "+".to_string(),
                        _ => format!("SPEED {}", ui_state.speed.label()),
                    };
                    let size = 12.0 * bs;
                    let tw = text_width(atlas, &label, size);
                    push_text(&mut quads, atlas, &label,
                        rect.x + (rect.w - tw) / 2.0,
                        rect.y + 5.0 * bs, size, COL_TEXT,
                    );
                }
                W_CATEGORY => {
                    push_rect(&mut quads, *rect, COL_WIDGET);
                    let label = format!("GROUP: {} ({})", ui_state.filter.label(), ui_state.filtered_results);
                    push_text(&mut quads, atlas, &label,
                        rect.x + 6.0 * bs, rect.y + 5.0 * bs, 11.0 * bs, COL_ACCENT);
                }
                W_FIELD_SEARCH | W_FIELD_SET => {
                    let focus_here = (*id == W_FIELD_SEARCH && ui_state.focus == Focus::Search)
                        || (*id == W_FIELD_SET && ui_state.focus == Focus::Set);
                    push_rect(&mut quads, *rect, if focus_here { COL_SELECTED } else { COL_FIELD });
                    let (label, content) = if *id == W_FIELD_SEARCH {
                        ("VAL", &ui_state.search_text)
                    } else {
                        ("SET", &ui_state.set_text)
                    };
                    let text = format!("{}: {}_", label, content);
                    push_text(&mut quads, atlas, &text, rect.x + 6.0 * bs, rect.y + 5.0 * bs, 12.0 * bs, COL_TEXT);
                }
                id if (W_KEY_BASE..W_KEY_BASE + 16).contains(&id) => {
                    let key_idx = (id - W_KEY_BASE) as usize;
                    let row = key_idx / 4;
                    let col = key_idx % 4;
                    let ch = KEYPAD[row][col];
                    let label = match ch {
                        Some('\u{8}') => "DEL".to_string(),
                        Some('\u{4}') => "CLR".to_string(),
                        Some(c) => c.to_string(),
                        None => String::new(),
                    };
                    push_rect(&mut quads, *rect, COL_WIDGET);
                    if !label.is_empty() {
                        let size = 12.0 * bs;
                        let tw = text_width(atlas, &label, size);
                        push_text(&mut quads,
                            atlas,
                            &label,
                            rect.x + (rect.w - tw) / 2.0,
                            rect.y + (rect.h - atlas.height * (size / FONT_PX)) / 2.0,
                            size,
                            COL_TEXT,
                        );
                    }
                }
                W_SEARCH | W_REFINE | W_RESET | W_SET | W_SET_ALL | W_FREEZE | W_UNFREEZE | W_DUMP | W_SAVE | W_WATCH | W_MARK | W_CHANGED | W_SAME | W_INCREASED | W_DECREASED => {
                    push_rect(&mut quads, *rect, COL_WIDGET);
                    let label: &str = match *id {
                        W_SEARCH => "SEARCH",
                        W_REFINE => "REFINE",
                        W_RESET => "RESET",
                        W_SET => "SET",
                        W_SET_ALL => if ui_state.bulk_preview { "CONFIRM" } else { "SET ALL" },
                        W_FREEZE => "FREEZE",
                        W_UNFREEZE => "UNFRZ",
                        W_DUMP => "DUMP",
                        W_SAVE => "SAVE",
                        W_WATCH => "WATCH",
                        W_MARK => "MARK",
                        W_CHANGED => "CHANGED",
                        W_SAME => "SAME",
                        W_INCREASED => "UP",
                        W_DECREASED => "DOWN",
                        _ => "",
                    };
                    let size = 12.0 * bs;
                    let size = size * ((rect.w - 6.0 * bs) / text_width(atlas, label, size).max(1.0)).min(1.0);
                    let tw = text_width(atlas, label, size);
                    push_text(&mut quads,
                        atlas,
                        label,
                        rect.x + (rect.w - tw) / 2.0,
                        rect.y + (rect.h - atlas.height * (size / FONT_PX)) / 2.0,
                        size,
                        COL_TEXT,
                    );
                }
                W_SCROLL_UP | W_SCROLL_DOWN => {
                    push_rect(&mut quads, *rect, COL_WIDGET);
                    let label: &str = if *id == W_SCROLL_UP { "^" } else { "v" };
                    let size = 12.0 * bs;
                    let size = size * ((rect.w - 6.0 * bs) / text_width(atlas, label, size).max(1.0)).min(1.0);
                    let tw = text_width(atlas, label, size);
                    push_text(&mut quads,
                        atlas,
                        label,
                        rect.x + (rect.w - tw) / 2.0,
                        rect.y + (rect.h - atlas.height * (size / FONT_PX)) / 2.0,
                        size,
                        COL_TEXT,
                    );
                }
                id if (W_RESULT_BASE..W_RESULT_BASE + RESULT_ROWS as u16).contains(&id) => {
                    let row_idx = (id - W_RESULT_BASE) as usize;
                    if let Some(result) = ui_state.results.get(row_idx) {
                        let selected = ui_state.selected == Some(result.addr)
                            && ui_state.selected_type == Some(result.vtype);
                        push_rect(
                            &mut quads,
                            *rect,
                            if selected {
                                COL_SELECTED
                            } else if result.changed {
                                COL_CHANGED
                            } else {
                                COL_FIELD
                            },
                        );
                        let text = format!(
                            "0x{:08X} {} {} {}",
                            result.addr,
                            result.vtype.name(),
                            result.vtype.format(result.bits),
                            result.analysis.category.label(),
                        );
                        let size = 10.0 * bs;
                        let width = text_width(atlas, &text, size).max(1.0);
                        let size = size * ((rect.w - 12.0 * bs) / width).min(1.0);
                        push_text(&mut quads, atlas, &text, rect.x + 6.0 * bs, rect.y + 3.0 * bs, size, COL_TEXT);
                    }
                }
                _ => {}
            }
        }

        // Results header + counts, drawn between the header and the result rows.
        let res_label = format!(
            "HITS {}/{} FROZEN {}",
            ui_state.filtered_results, ui_state.total_results, ui_state.frozen_count
        );
        push_text(&mut quads,
            atlas,
            &res_label,
            panel.rect.x + 8.0 * bs,
            panel.results_header_y + 3.0 * bs,
            9.0 * bs,
            COL_TEXT_DIM,
        );

        // Status line at the bottom of the panel.
        if !ui_state.status.is_empty() {
            let status_y = panel.rect.y + panel.rect.h - 18.0 * bs;
            let size = 11.0 * bs;
            let width = text_width(atlas, &ui_state.status, size).max(1.0);
            let size = size * ((panel.rect.w - 16.0 * bs) / width).min(1.0);
            push_text(&mut quads,
                atlas,
                &ui_state.status,
                panel.rect.x + 8.0 * bs,
                status_y,
                size,
                COL_ACCENT,
            );
        }
    }

    Some(quads)
}

/// Render the scene with GLES 1.x fixed-function calls.
unsafe fn render_gles1(gles: &mut dyn GLES, viewport: (u32, u32, u32, u32), quads: &[Quad]) {
    // Coordinates are VIEWPORT-RELATIVE: the GL viewport already offsets
    // drawing by (vx, vy), and the touch handlers subtract it before
    // hit-testing. Adding the origin here would double it (panel shifted
    // by the letterbox offset, tap targets misaligned with drawn keys).
    let (_vx, _vy, vw, vh) = viewport;
    let (vx, vy, vw, vh) = (0.0_f32, 0.0_f32, vw as f32, vh as f32);

    // Save state (mirrors draw_onscreen_text).
    let mut old_active_texture: GLint = 0;
    gles.GetIntegerv(gles11::ACTIVE_TEXTURE, &mut old_active_texture);
    let mut old_texture: GLint = 0;
    gles.GetIntegerv(gles11::TEXTURE_BINDING_2D, &mut old_texture);
    let mut old_tex_env_mode: GLint = 0;
    gles.GetTexEnviv(gles11::TEXTURE_ENV, gles11::TEXTURE_ENV_MODE, &mut old_tex_env_mode);
    // The presenter uses REPLACE for the game frame; glyphs instead need
    // their atlas coverage multiplied by the overlay's text colour.
    let tex_env_mode = gles11::MODULATE as GLint;
    gles.TexEnviv(gles11::TEXTURE_ENV, gles11::TEXTURE_ENV_MODE, &tex_env_mode);

    gles.MatrixMode(gles11::PROJECTION);
    gles.PushMatrix();
    gles.LoadIdentity();
    gles.Orthof(0.0, vw, vh, 0.0, -1.0, 1.0);
    gles.MatrixMode(gles11::MODELVIEW);
    gles.PushMatrix();
    gles.LoadIdentity();

    gles.EnableClientState(gles11::VERTEX_ARRAY);
    gles.EnableClientState(gles11::TEXTURE_COORD_ARRAY);
    gles.Enable(gles11::BLEND);
    gles.BlendFunc(gles11::SRC_ALPHA, gles11::ONE_MINUS_SRC_ALPHA);

    // present_frame leaves TEXTURE_2D enabled with the game frame bound
    // (unless the cursor/FPS overlay happened to disable it). Establish the
    // untextured state before using 0 as our solid-quad cache sentinel, or
    // the first rectangle — the CE button — samples a miniature game frame.
    gles.Disable(gles11::TEXTURE_2D);
    let mut bound_tex: GLuint = 0;
    for q in quads {
        let (r, g, b, a) = q.col;
        gles.Color4f(r, g, b, a);
        if q.tex == 0 {
            if bound_tex != 0 {
                gles.Disable(gles11::TEXTURE_2D);
                bound_tex = 0;
            }
        } else {
            if bound_tex == 0 {
                gles.Enable(gles11::TEXTURE_2D);
            }
            if bound_tex != q.tex {
                gles.BindTexture(gles11::TEXTURE_2D, q.tex);
                bound_tex = q.tex;
            }
        }
        let verts = quad_vertices(q);
        gles.VertexPointer(2, gles11::FLOAT, 16, verts.as_ptr() as *const _);
        gles.TexCoordPointer(
            2,
            gles11::FLOAT,
            16,
            verts.as_ptr().add(2) as *const _,
        );
        gles.DrawArrays(gles11::TRIANGLE_STRIP, 0, 4);
    }

    // Restore state.
    gles.TexEnviv(gles11::TEXTURE_ENV, gles11::TEXTURE_ENV_MODE, &old_tex_env_mode);
    gles.BindTexture(gles11::TEXTURE_2D, old_texture as _);
    gles.ActiveTexture(old_active_texture as _);
    gles.Disable(gles11::BLEND);
    gles.Disable(gles11::TEXTURE_2D);
    gles.DisableClientState(gles11::TEXTURE_COORD_ARRAY);
    gles.DisableClientState(gles11::VERTEX_ARRAY);

    gles.MatrixMode(gles11::MODELVIEW);
    gles.PopMatrix();
    gles.MatrixMode(gles11::PROJECTION);
    gles.PopMatrix();
    gles.MatrixMode(gles11::TEXTURE);
    gles.LoadIdentity();
}

// ---------------------------------------------------------------------------
// Native OpenGL ES 2.0 rendering. Native ES 2.0 drivers (Android etc.) have
// no fixed-function pipeline, so present_renderbuffer_es2 calls draw_es2()
// and the scene is drawn with a small dedicated shader program instead.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct OverlayProgram {
    program: GLuint,
    a_pos: GLint,
    a_uv: GLint,
    a_col: GLint,
    u_viewport: GLint,
    u_tex: GLint,
}

static OVERLAY_CONTEXT_TOKEN: Mutex<Option<usize>> = Mutex::new(None);
static OVERLAY_PROGRAM: Mutex<Option<OverlayProgram>> = Mutex::new(None);
static OVERLAY_VBO: Mutex<Option<GLuint>> = Mutex::new(None);
static OVERLAY_WHITE_TEX: Mutex<Option<GLuint>> = Mutex::new(None);

const OVERLAY_VS_SRC: &[u8] = b"\
    attribute vec2 aPos;\n\
    attribute vec2 aUV;\n\
    attribute vec4 aCol;\n\
    uniform vec2 uViewport;\n\
    varying vec2 vUV;\n\
    varying vec4 vCol;\n\
    void main() {\n\
        vec2 ndc = vec2(aPos.x / uViewport.x * 2.0 - 1.0, 1.0 - aPos.y / uViewport.y * 2.0);\n\
        gl_Position = vec4(ndc, 0.0, 1.0);\n\
        vUV = aUV;\n\
        vCol = aCol;\n\
    }\n\0";

const OVERLAY_FS_SRC: &[u8] = b"\
    precision mediump float;\n\
    varying vec2 vUV;\n\
    varying vec4 vCol;\n\
    uniform sampler2D uTex;\n\
    void main() {\n\
        gl_FragColor = texture2D(uTex, vUV) * vCol;\n\
    }\n\0";

unsafe fn ensure_overlay_program(gles: &mut dyn GLES) -> Option<OverlayProgram> {
    use crate::gles::gles2_raw as gles2;
    {
        let guard = OVERLAY_PROGRAM.lock().unwrap();
        if let Some(p) = *guard {
            return Some(p);
        }
    }
    let mut guard = OVERLAY_PROGRAM.lock().unwrap();
    if let Some(p) = guard.as_ref() {
        return Some(OverlayProgram { ..*p });
    }

    let vs_src = OVERLAY_VS_SRC;
    let fs_src = OVERLAY_FS_SRC;

    let vs = gles.CreateShader(gles2::VERTEX_SHADER);
    let vs_ptr = vs_src.as_ptr() as *const _;
    let vs_len = (vs_src.len() - 1) as GLint;
    gles.ShaderSource(vs, 1, &vs_ptr, &vs_len);
    gles.CompileShader(vs);
    let mut ok: GLint = 0;
    gles.GetShaderiv(vs, gles2::COMPILE_STATUS, &mut ok);
    if ok == 0 {
        log!("Warning: trainer overlay: vertex shader failed to compile.");
        gles.DeleteShader(vs);
        return None;
    }

    let fs = gles.CreateShader(gles2::FRAGMENT_SHADER);
    let fs_ptr = fs_src.as_ptr() as *const _;
    let fs_len = (fs_src.len() - 1) as GLint;
    gles.ShaderSource(fs, 1, &fs_ptr, &fs_len);
    gles.CompileShader(fs);
    let mut ok: GLint = 0;
    gles.GetShaderiv(fs, gles2::COMPILE_STATUS, &mut ok);
    if ok == 0 {
        log!("Warning: trainer overlay: fragment shader failed to compile.");
        gles.DeleteShader(vs);
        gles.DeleteShader(fs);
        return None;
    }

    let program = gles.CreateProgram();
    gles.AttachShader(program, vs);
    gles.AttachShader(program, fs);
    gles.LinkProgram(program);
    gles.DeleteShader(vs);
    gles.DeleteShader(fs);
    let mut ok: GLint = 0;
    gles.GetProgramiv(program, gles2::LINK_STATUS, &mut ok);
    if ok == 0 {
        log!("Warning: trainer overlay: program failed to link.");
        return None;
    }

    let p = OverlayProgram {
        program,
        a_pos: gles.GetAttribLocation(program, b"aPos\0".as_ptr() as *const _),
        a_uv: gles.GetAttribLocation(program, b"aUV\0".as_ptr() as *const _),
        a_col: gles.GetAttribLocation(program, b"aCol\0".as_ptr() as *const _),
        u_viewport: gles.GetUniformLocation(program, b"uViewport\0".as_ptr() as *const _),
        u_tex: gles.GetUniformLocation(program, b"uTex\0".as_ptr() as *const _),
    };
    *guard = Some(p);
    Some(p)
}

unsafe fn ensure_overlay_vbo(gles: &mut dyn GLES) -> GLuint {
    use crate::gles::gles2_raw as gles2;
    let mut guard = OVERLAY_VBO.lock().unwrap();
    if let Some(vbo) = *guard {
        return vbo;
    }
    let mut vbo: GLuint = 0;
    gles.GenBuffers(1, &mut vbo);
    *guard = Some(vbo);
    vbo
}

unsafe fn ensure_overlay_white_tex(gles: &mut dyn GLES) -> GLuint {
    use crate::gles::gles2_raw as gles2;
    {
        let guard = OVERLAY_WHITE_TEX.lock().unwrap();
        if let Some(tex) = *guard {
            return tex;
        }
    }
    let mut guard = OVERLAY_WHITE_TEX.lock().unwrap();
    if let Some(tex) = *guard {
        return tex;
    }
    let mut tex: GLuint = 0;
    gles.GenTextures(1, &mut tex);
    gles.BindTexture(gles2::TEXTURE_2D, tex);
    let white: [u8; 4] = [255, 255, 255, 255];
    gles.TexImage2D(
        gles2::TEXTURE_2D,
        0,
        gles2::RGBA as GLint,
        1,
        1,
        0,
        gles2::RGBA,
        gles2::UNSIGNED_BYTE,
        white.as_ptr() as *const _,
    );
    gles.TexParameteri(gles2::TEXTURE_2D, gles2::TEXTURE_MIN_FILTER, gles2::NEAREST as _);
    gles.TexParameteri(gles2::TEXTURE_2D, gles2::TEXTURE_MAG_FILTER, gles2::NEAREST as _);
    gles.TexParameteri(gles2::TEXTURE_2D, gles2::TEXTURE_WRAP_S, gles2::CLAMP_TO_EDGE as _);
    gles.TexParameteri(gles2::TEXTURE_2D, gles2::TEXTURE_WRAP_T, gles2::CLAMP_TO_EDGE as _);
    *guard = Some(tex);
    tex
}

/// Render the scene with a small ES 2.0 shader program. Saves and restores
/// the state it touches; the caller (present_renderbuffer_es2) restores the
/// rest of the presenter state afterwards.
unsafe fn render_es2(gles: &mut dyn GLES, viewport: (u32, u32, u32, u32), quads: &[Quad]) {
    use crate::gles::gles2_raw as gles2;
    use gles2::types::*;

    if quads.is_empty() {
        return;
    }
    let Some(program) = ensure_overlay_program(gles) else {
        return;
    };
    if program.a_pos < 0 || program.a_uv < 0 || program.a_col < 0 {
        return;
    }
    let mut old_program: GLint = 0;
    gles.GetIntegerv(gles2::CURRENT_PROGRAM, &mut old_program);
    let mut old_array_buffer: GLint = 0;
    gles.GetIntegerv(gles2::ARRAY_BUFFER_BINDING, &mut old_array_buffer);
    let mut old_active_texture: GLint = 0;
    gles.GetIntegerv(gles2::ACTIVE_TEXTURE, &mut old_active_texture);
    gles.ActiveTexture(gles2::TEXTURE0);
    let mut old_tex0: GLint = 0;
    gles.GetIntegerv(gles2::TEXTURE_BINDING_2D, &mut old_tex0);
    let blend_was_on = gles.IsEnabled(gles2::BLEND) != 0;
    let mut old_color_mask = [gles2::TRUE; 4];
    gles.GetBooleanv(gles2::COLOR_WRITEMASK, old_color_mask.as_mut_ptr());
    let overlay_caps = [
        gles2::SCISSOR_TEST,
        gles2::DEPTH_TEST,
        gles2::CULL_FACE,
        gles2::STENCIL_TEST,
        gles2::SAMPLE_ALPHA_TO_COVERAGE,
    ];
    let old_overlay_caps = overlay_caps.map(|cap| (cap, gles.IsEnabled(cap) != 0));
    let mut old_blend_src_rgb: GLint = 0;
    let mut old_blend_dst_rgb: GLint = 0;
    let mut old_blend_src_alpha: GLint = 0;
    let mut old_blend_dst_alpha: GLint = 0;
    gles.GetIntegerv(0x80C9, &mut old_blend_src_rgb);
    gles.GetIntegerv(0x80C8, &mut old_blend_dst_rgb);
    gles.GetIntegerv(0x80CB, &mut old_blend_src_alpha);
    gles.GetIntegerv(0x80CA, &mut old_blend_dst_alpha);
    let attribs = [program.a_pos as GLuint, program.a_uv as GLuint, program.a_col as GLuint];
    const VERTEX_ATTRIB_ARRAY_ENABLED: GLenum = 0x8622;
    const VERTEX_ATTRIB_ARRAY_SIZE: GLenum = 0x8623;
    const VERTEX_ATTRIB_ARRAY_STRIDE: GLenum = 0x8624;
    const VERTEX_ATTRIB_ARRAY_TYPE: GLenum = 0x8625;
    const VERTEX_ATTRIB_ARRAY_POINTER: GLenum = 0x8645;
    const VERTEX_ATTRIB_ARRAY_NORMALIZED: GLenum = 0x886A;
    const VERTEX_ATTRIB_ARRAY_BUFFER_BINDING: GLenum = 0x889F;
    struct SavedAttrib {
        index: GLuint,
        enabled: GLint,
        buffer: GLint,
        size: GLint,
        stride: GLint,
        type_: GLint,
        normalized: GLint,
        pointer: *mut GLvoid,
    }
    let mut attrib_states = attribs.map(|index| SavedAttrib {
        index,
        enabled: 0,
        buffer: 0,
        size: 0,
        stride: 0,
        type_: 0,
        normalized: 0,
        pointer: std::ptr::null_mut(),
    });
    for state in &mut attrib_states {
        gles.GetVertexAttribiv(state.index, VERTEX_ATTRIB_ARRAY_ENABLED, &mut state.enabled);
        gles.GetVertexAttribiv(state.index, VERTEX_ATTRIB_ARRAY_BUFFER_BINDING, &mut state.buffer);
        gles.GetVertexAttribiv(state.index, VERTEX_ATTRIB_ARRAY_SIZE, &mut state.size);
        gles.GetVertexAttribiv(state.index, VERTEX_ATTRIB_ARRAY_STRIDE, &mut state.stride);
        gles.GetVertexAttribiv(state.index, VERTEX_ATTRIB_ARRAY_TYPE, &mut state.type_);
        gles.GetVertexAttribiv(state.index, VERTEX_ATTRIB_ARRAY_NORMALIZED, &mut state.normalized);
        gles.GetVertexAttribPointerv(state.index, VERTEX_ATTRIB_ARRAY_POINTER, &mut state.pointer);
    }
    let vbo = ensure_overlay_vbo(gles);
    let white = ensure_overlay_white_tex(gles);

    gles.ColorMask(gles2::TRUE, gles2::TRUE, gles2::TRUE, gles2::TRUE);
    for (cap, _) in old_overlay_caps {
        gles.Disable(cap);
    }
    gles.UseProgram(program.program);
    gles.Uniform2f(program.u_viewport, viewport.2 as f32, viewport.3 as f32);
    gles.Uniform1i(program.u_tex, 0);
    gles.Enable(gles2::BLEND);
    gles.BlendFunc(gles2::SRC_ALPHA, gles2::ONE_MINUS_SRC_ALPHA);

    gles.BindBuffer(gles2::ARRAY_BUFFER, vbo);
    for &attr in &attribs {
        gles.EnableVertexAttribArray(attr as _);
    }
    let stride = 8 * 4;
    gles.VertexAttribPointer(
        program.a_pos as _,
        2,
        gles2::FLOAT,
        gles2::FALSE,
        stride,
        0usize as *const _,
    );
    gles.VertexAttribPointer(
        program.a_uv as _,
        2,
        gles2::FLOAT,
        gles2::FALSE,
        stride,
        8usize as *const _,
    );
    gles.VertexAttribPointer(
        program.a_col as _,
        4,
        gles2::FLOAT,
        gles2::FALSE,
        stride,
        16usize as *const _,
    );

    for q in quads {
        let (r, g, b, a) = q.col;
        // Interleaved pos(2) uv(2) col(4), triangle strip TL, BL, TR, BR.
        let mut data = [0.0f32; 4 * 8];
        let corners = [
            (q.x, q.y, q.u0, q.v0),
            (q.x, q.y + q.h, q.u0, q.v1),
            (q.x + q.w, q.y, q.u1, q.v0),
            (q.x + q.w, q.y + q.h, q.u1, q.v1),
        ];
        for (i, (px, py, u, v)) in corners.iter().enumerate() {
            data[i * 8] = *px;
            data[i * 8 + 1] = *py;
            data[i * 8 + 2] = *u;
            data[i * 8 + 3] = *v;
            data[i * 8 + 4] = r;
            data[i * 8 + 5] = g;
            data[i * 8 + 6] = b;
            data[i * 8 + 7] = a;
        }
        let tex = if q.tex == 0 { white } else { q.tex };
        gles.BindTexture(gles2::TEXTURE_2D, tex);
        gles.BufferData(
            gles2::ARRAY_BUFFER,
            (data.len() * 4) as _,
            data.as_ptr() as *const _,
            gles2::DYNAMIC_DRAW,
        );
        gles.DrawArrays(gles2::TRIANGLE_STRIP, 0, 4);
    }

    for state in &attrib_states {
        gles.BindBuffer(gles2::ARRAY_BUFFER, state.buffer as _);
        gles.VertexAttribPointer(
            state.index,
            state.size,
            state.type_ as _,
            state.normalized as _,
            state.stride,
            state.pointer as *const GLvoid,
        );
        if state.enabled != 0 {
            gles.EnableVertexAttribArray(state.index);
        } else {
            gles.DisableVertexAttribArray(state.index);
        }
    }
    gles.BlendFuncSeparate(
        old_blend_src_rgb as _,
        old_blend_dst_rgb as _,
        old_blend_src_alpha as _,
        old_blend_dst_alpha as _,
    );
    if blend_was_on {
        gles.Enable(gles2::BLEND);
    } else {
        gles.Disable(gles2::BLEND);
    }
    for (cap, was_enabled) in old_overlay_caps {
        if was_enabled {
            gles.Enable(cap);
        }
    }
    gles.ColorMask(
        old_color_mask[0],
        old_color_mask[1],
        old_color_mask[2],
        old_color_mask[3],
    );
    gles.BindTexture(gles2::TEXTURE_2D, old_tex0 as _);
    gles.ActiveTexture(old_active_texture as _);
    gles.BindBuffer(gles2::ARRAY_BUFFER, old_array_buffer as _);
    gles.UseProgram(old_program as _);
}

fn text_width(atlas: &Atlas, text: &str, px_size: f32) -> f32 {
    let scale = px_size / FONT_PX;
    text.chars()
        .map(|ch| {
            char_index(ch)
                .and_then(|idx| atlas.glyphs[idx].as_ref())
                .map_or(px_size * 0.3, |cell| cell.advance * scale)
        })
        .sum()
}

#[cfg(test)]
mod tests;
