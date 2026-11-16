/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
use super::gles11_raw as gles11;
use super::GLES;
use crate::options::{Options, PerformanceOverlayCorner, PerformanceOverlayMode};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

const PANEL_WIDTH: usize = 390;
const PANEL_HEIGHT: usize = 194;
const MINIMAL_WIDTH: usize = 300;
const MINIMAL_HEIGHT: usize = 48;
const FONT_WIDTH: usize = 5;
const FONT_HEIGHT: usize = 7;
const FONT_SCALE: usize = 2;
const CELL_WIDTH: usize = 12;
const LINE_HEIGHT: usize = 18;
const HISTORY_SIZE: usize = 128;
const PIXEL_BYTES: usize = PANEL_WIDTH * PANEL_HEIGHT * 4;

/// The app picker renders through the same present paths as games, but its
/// settings are only meant to control the next game, never the picker itself.
struct OverlayVisibility {
    requested: AtomicBool,
    app_picker: AtomicBool,
}

impl OverlayVisibility {
    const fn new() -> Self {
        Self {
            requested: AtomicBool::new(true),
            app_picker: AtomicBool::new(false),
        }
    }

    fn enabled(&self) -> bool {
        self.requested.load(Ordering::Relaxed) && !self.app_picker.load(Ordering::Relaxed)
    }

    fn set_requested(&self, enabled: bool) -> bool {
        self.requested.swap(enabled, Ordering::Relaxed)
    }

    fn set_app_picker(&self, app_picker: bool) {
        self.app_picker.store(app_picker, Ordering::Relaxed);
    }

    fn is_app_picker(&self) -> bool {
        self.app_picker.load(Ordering::Relaxed)
    }
}

static VISIBILITY: OverlayVisibility = OverlayVisibility::new();
static MODE: AtomicUsize = AtomicUsize::new(2);
static CORNER: AtomicUsize = AtomicUsize::new(1);
static GUEST_FLIPS: AtomicU64 = AtomicU64::new(0);
static HOST_PRESENTS: AtomicU64 = AtomicU64::new(0);
static GUEST_DRAWS: AtomicU64 = AtomicU64::new(0);
static LAST_GUEST_FRAME_NS: AtomicU64 = AtomicU64::new(0);
static SESSION_START_NS: AtomicU64 = AtomicU64::new(0);
static HISTORY_INDEX: AtomicUsize = AtomicUsize::new(0);
static FRAME_MILLISECONDS: [AtomicU32; HISTORY_SIZE] = [const { AtomicU32::new(0) }; HISTORY_SIZE];
static PROCESS_ORIGIN: OnceLock<Instant> = OnceLock::new();
static NEXT_GL_CONTEXT_TOKEN: AtomicUsize = AtomicUsize::new(1);
static PIXEL_VERSION: AtomicU64 = AtomicU64::new(1);
static PIXELS_VERSION: AtomicU64 = AtomicU64::new(0);
static LAST_GRAPH_REFRESH_NS: AtomicU64 = AtomicU64::new(0);
static ES1_TEXTURE: Mutex<Option<CachedTexture>> = Mutex::new(None);
static ES2_TEXTURE: Mutex<Option<CachedTexture>> = Mutex::new(None);
static PIXELS: OnceLock<Mutex<Vec<u8>>> = OnceLock::new();
static STATS: OnceLock<Mutex<Stats>> = OnceLock::new();
static GUEST_RESOURCES: OnceLock<Mutex<GuestResourceTracker>> = OnceLock::new();

#[derive(Clone, Copy)]
struct CachedTexture {
    context_token: usize,
    texture: u32,
    pixel_version: u64,
}

#[derive(Default)]
struct GuestResourceTracker {
    group_references: HashMap<usize, usize>,
    buffers: HashMap<(usize, u32), u64>,
    images: HashMap<(usize, u32, u32, i32), u64>,
    buffer_bytes: u64,
    image_bytes: u64,
    live_allocations: u64,
    peak_allocations: u64,
}

impl GuestResourceTracker {
    fn register_context(&mut self, group: usize) {
        *self.group_references.entry(group).or_default() += 1;
    }

    fn unregister_context(&mut self, group: usize) {
        let remove_group = match self.group_references.get_mut(&group) {
            Some(references) if *references > 1 => {
                *references -= 1;
                false
            }
            Some(_) => true,
            None => false,
        };
        if !remove_group {
            return;
        }
        self.group_references.remove(&group);
        self.buffers.retain(|(owner, _), _| *owner != group);
        self.images.retain(|(owner, _, _, _), _| *owner != group);
        self.buffer_bytes = self.buffers.values().copied().fold(0, u64::saturating_add);
        self.image_bytes = self.images.values().copied().fold(0, u64::saturating_add);
        self.update_live_allocations();
    }

    fn set_buffer_size(&mut self, group: usize, buffer: u32, size: u64) {
        if group == 0 || buffer == 0 {
            return;
        }
        let key = (group, buffer);
        let previous = if size == 0 {
            self.buffers.remove(&key).unwrap_or(0)
        } else {
            self.buffers.insert(key, size).unwrap_or(0)
        };
        self.buffer_bytes = self.buffer_bytes.saturating_sub(previous).saturating_add(size);
        self.update_live_allocations();
    }

    fn delete_buffer(&mut self, group: usize, buffer: u32) {
        let previous = self.buffers.remove(&(group, buffer)).unwrap_or(0);
        self.buffer_bytes = self.buffer_bytes.saturating_sub(previous);
        self.update_live_allocations();
    }

    fn set_image_size(&mut self, group: usize, target: u32, texture: u32, level: i32, size: u64) {
        if group == 0 || texture == 0 || level < 0 || size == 0 {
            return;
        }
        let key = (group, target, texture, level);
        let previous = self.images.insert(key, size).unwrap_or(0);
        self.image_bytes = self.image_bytes.saturating_sub(previous).saturating_add(size);
        self.update_live_allocations();
    }

    fn clear_texture_storage(&mut self, group: usize, texture: u32) {
        let mut removed = 0u64;
        self.images.retain(|(owner, _, name, _), size| {
            if *owner == group && *name == texture {
                removed = removed.saturating_add(*size);
                false
            } else {
                true
            }
        });
        self.image_bytes = self.image_bytes.saturating_sub(removed);
        self.update_live_allocations();
    }

    fn delete_texture(&mut self, group: usize, texture: u32) {
        self.clear_texture_storage(group, texture);
    }

    fn update_live_allocations(&mut self) {
        self.live_allocations = self.buffers.len().saturating_add(self.images.len()) as u64;
        self.peak_allocations = self.peak_allocations.max(self.live_allocations);
    }

    fn totals(&self) -> (u64, u64, u64, u64) {
        (
            self.buffer_bytes,
            self.image_bytes,
            self.live_allocations,
            self.peak_allocations,
        )
    }
}

struct Stats {
    window_start: Instant,
    previous_cpu_seconds: Option<f64>,
    previous_allocated_bytes: u64,
    logical_processors: usize,
    allocated_mib_per_second: f64,
    fps: f64,
    present_fps: f64,
    draws_per_second: f64,
    cpu_percent: Option<f64>,
    resident_memory_mb: Option<u64>,
    frame_label: String,
    elapsed: String,
    full_lines: [String; 6],
    minimal_lines: [String; 2],
    title_summary: String,
}

impl Stats {
    fn new() -> Self {
        Self {
            window_start: Instant::now(),
            previous_cpu_seconds: process_cpu_seconds(),
            previous_allocated_bytes: crate::allocated_bytes_total(),
            logical_processors: logical_processor_count(),
            allocated_mib_per_second: 0.0,
            fps: 0.0,
            present_fps: 0.0,
            draws_per_second: 0.0,
            cpu_percent: None,
            resident_memory_mb: resident_memory_mb(),
            frame_label: "0.0 MS".to_string(),
            elapsed: "00:00:00".to_string(),
            full_lines: [
                "FPS 0.0 PRES 0.0 0.0 MS".to_string(),
                "DRAWS 0/S 0/F Q DIRECT".to_string(),
                "ALLOC 0.0MB/S  BUF 0K  IMG 0K".to_string(),
                "MEM N/A  VMEM 0K".to_string(),
                "CPU N/A GPU N/A".to_string(),
                "TIME 00:00:00 GLALLOC 0/0".to_string(),
            ],
            minimal_lines: [
                "FPS 0.0 CPU N/A".to_string(),
                "GPU N/A TIME 00:00:00".to_string(),
            ],
            title_summary: "FPS 0.0 | CPU N/A | GPU N/A | TIME 00:00:00".to_string(),
        }
    }
}

fn guest_resources() -> &'static Mutex<GuestResourceTracker> {
    GUEST_RESOURCES.get_or_init(|| Mutex::new(GuestResourceTracker::default()))
}

pub(crate) fn register_gl_resource_context(group: usize) {
    if group != 0 {
        guest_resources()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .register_context(group);
    }
}

pub(crate) fn unregister_gl_resource_context(group: usize) {
    if group != 0 {
        guest_resources()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .unregister_context(group);
    }
}

pub(crate) fn record_gl_buffer_allocation(group: usize, buffer: u32, size: u64) {
    guest_resources()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .set_buffer_size(group, buffer, size);
}

pub(crate) fn delete_gl_buffers(group: usize, buffers: &[u32]) {
    let mut resources = guest_resources()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for buffer in buffers {
        resources.delete_buffer(group, *buffer);
    }
}

pub(crate) fn record_gl_image_allocation(
    group: usize,
    target: u32,
    texture: u32,
    level: i32,
    size: u64,
) {
    guest_resources()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .set_image_size(group, target, texture, level, size);
}

pub(crate) fn delete_gl_texture(group: usize, texture: u32) {
    guest_resources()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .delete_texture(group, texture);
}

pub(crate) fn clear_gl_image_allocations(group: usize, texture: u32) {
    guest_resources()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear_texture_storage(group, texture);
}

fn guest_resource_stats() -> (u64, u64, u64, u64) {
    guest_resources()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .totals()
}

fn resource_memory_label(bytes: u64) -> String {
    if bytes < 1024 * 1024 {
        format!("{}K", bytes.div_ceil(1024))
    } else {
        format!("{}M", bytes / (1024 * 1024))
    }
}

fn origin() -> Instant {
    *PROCESS_ORIGIN.get_or_init(Instant::now)
}
pub fn allocate_context_token() -> usize {
    NEXT_GL_CONTEXT_TOKEN.fetch_add(1, Ordering::Relaxed)
}


fn process_elapsed_ns() -> u64 {
    origin().elapsed().as_nanos().min(u64::MAX as u128) as u64
}

fn session_elapsed_seconds() -> u64 {
    process_elapsed_ns().saturating_sub(SESSION_START_NS.load(Ordering::Relaxed)) / 1_000_000_000
}

fn stats() -> &'static Mutex<Stats> {
    STATS.get_or_init(|| Mutex::new(Stats::new()))
}

fn reset_session() {
    PIXEL_VERSION.fetch_add(1, Ordering::Relaxed);
    LAST_GRAPH_REFRESH_NS.store(0, Ordering::Relaxed);
    let now = process_elapsed_ns();
    SESSION_START_NS.store(now, Ordering::Relaxed);
    GUEST_FLIPS.store(0, Ordering::Relaxed);
    HOST_PRESENTS.store(0, Ordering::Relaxed);
    GUEST_DRAWS.store(0, Ordering::Relaxed);
    LAST_GUEST_FRAME_NS.store(0, Ordering::Relaxed);
    HISTORY_INDEX.store(0, Ordering::Relaxed);
    for sample in &FRAME_MILLISECONDS {
        sample.store(0, Ordering::Relaxed);
    }
    if let Ok(mut current) = stats().lock() {
        *current = Stats::new();
    }
}

fn reset_measurements() {
    PIXEL_VERSION.fetch_add(1, Ordering::Relaxed);
    LAST_GRAPH_REFRESH_NS.store(0, Ordering::Relaxed);
    GUEST_FLIPS.store(0, Ordering::Relaxed);
    HOST_PRESENTS.store(0, Ordering::Relaxed);
    GUEST_DRAWS.store(0, Ordering::Relaxed);
    LAST_GUEST_FRAME_NS.store(0, Ordering::Relaxed);
    HISTORY_INDEX.store(0, Ordering::Relaxed);
    for sample in &FRAME_MILLISECONDS {
        sample.store(0, Ordering::Relaxed);
    }
    if let Ok(mut current) = stats().lock() {
        *current = Stats::new();
    }
}

pub fn hidden_by_environment() -> bool {
    std::env::var("TOUCHHLE_OVERLAY")
        .map(|value| matches!(value.to_ascii_lowercase().as_str(), "0" | "false" | "off"))
        .unwrap_or(false)
}

pub fn configure_from_options(options: &Options, is_app_picker: bool) {
    // Set the session gate before any window or splash can present a frame.
    // Reconfiguring the picker settings cannot lift this gate.
    VISIBILITY.set_app_picker(is_app_picker);
    let enabled = (options.performance_overlay_enabled
        || crate::env_flag_cached!("TOUCHHLE_ONSCREEN_FPS"))
        && !hidden_by_environment();
    configure(
        enabled,
        options.performance_overlay_mode,
        options.performance_overlay_corner,
    );
}

pub fn configure(requested: bool, mode: PerformanceOverlayMode, corner: PerformanceOverlayCorner) {
    VISIBILITY.set_requested(requested);
    MODE.store(mode_index(mode.for_current_platform()), Ordering::Relaxed);
    CORNER.store(corner_index(corner), Ordering::Relaxed);
    crate::set_allocation_meter_enabled(enabled() && mode == PerformanceOverlayMode::Full);
    reset_session();
}

pub fn enabled() -> bool {
    VISIBILITY.enabled()
}

pub fn is_app_picker() -> bool {
    VISIBILITY.is_app_picker()
}

pub fn set_enabled(requested: bool) {
    let was_requested = VISIBILITY.set_requested(requested);
    crate::set_allocation_meter_enabled(enabled() && mode() == PerformanceOverlayMode::Full);
    if was_requested != requested && enabled() {
        reset_measurements();
    }
}

pub fn mode() -> PerformanceOverlayMode {
    match MODE.load(Ordering::Relaxed) {
        0 => PerformanceOverlayMode::Full,
        1 => PerformanceOverlayMode::Minimal,
        _ => PerformanceOverlayMode::TitleBar,
    }
}

fn display_mode() -> PerformanceOverlayMode {
    let selected = mode();
    if cfg!(any(target_os = "android", target_os = "ios"))
        && selected == PerformanceOverlayMode::TitleBar
    {
        PerformanceOverlayMode::Minimal
    } else {
        selected
    }
}

#[derive(Clone, Copy)]
struct OverlayGeometry {
    left: usize,
    top: usize,
    width: usize,
    height: usize,
    uv_width: f32,
    uv_height: f32,
}

fn overlay_geometry(
    viewport: (u32, u32, u32, u32),
    mode: PerformanceOverlayMode,
    corner: PerformanceOverlayCorner,
) -> Option<OverlayGeometry> {
    let (display_width, display_height) = if mode == PerformanceOverlayMode::Full {
        (PANEL_WIDTH, PANEL_HEIGHT)
    } else {
        (MINIMAL_WIDTH, MINIMAL_HEIGHT)
    };
    let (_, _, viewport_width, viewport_height) = viewport;
    let horizontal_margin = 12.min(viewport_width as usize / 2);
    let vertical_margin = 12.min(viewport_height as usize / 2);
    let available_width = (viewport_width as usize).saturating_sub(2 * horizontal_margin);
    let available_height = (viewport_height as usize).saturating_sub(2 * vertical_margin);
    let scale = (available_width as f64 / display_width as f64)
        .min(available_height as f64 / display_height as f64)
        .min(1.0);
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let width = (display_width as f64 * scale).floor() as usize;
    let height = (display_height as f64 * scale).floor() as usize;
    if width == 0 || height == 0 {
        return None;
    }
    let left = if matches!(
        corner,
        PerformanceOverlayCorner::TopRight | PerformanceOverlayCorner::BottomRight
    ) {
        viewport_width as usize - horizontal_margin - width
    } else {
        horizontal_margin
    };
    let top = if matches!(
        corner,
        PerformanceOverlayCorner::BottomLeft | PerformanceOverlayCorner::BottomRight
    ) {
        viewport_height as usize - vertical_margin - height
    } else {
        vertical_margin
    };
    Some(OverlayGeometry {
        left,
        top,
        width,
        height,
        uv_width: display_width as f32 / PANEL_WIDTH as f32,
        uv_height: display_height as f32 / PANEL_HEIGHT as f32,
    })
}

pub fn corner() -> PerformanceOverlayCorner {
    match CORNER.load(Ordering::Relaxed) {
        0 => PerformanceOverlayCorner::TopLeft,
        1 => PerformanceOverlayCorner::TopRight,
        2 => PerformanceOverlayCorner::BottomRight,
        _ => PerformanceOverlayCorner::BottomLeft,
    }
}

pub fn cycle_corner() -> &'static str {
    let corner = CORNER.fetch_add(1, Ordering::Relaxed).wrapping_add(1) % 4;
    match corner {
        0 => "top-left",
        1 => "top-right",
        2 => "bottom-right",
        _ => "bottom-left",
    }
}

fn mode_index(mode: PerformanceOverlayMode) -> usize {
    match mode {
        PerformanceOverlayMode::Full => 0,
        PerformanceOverlayMode::Minimal => 1,
        PerformanceOverlayMode::TitleBar => 2,
    }
}

fn corner_index(corner: PerformanceOverlayCorner) -> usize {
    match corner {
        PerformanceOverlayCorner::TopLeft => 0,
        PerformanceOverlayCorner::TopRight => 1,
        PerformanceOverlayCorner::BottomRight => 2,
        PerformanceOverlayCorner::BottomLeft => 3,
    }
}

pub fn record_guest_flip() {
    if !enabled() {
        return;
    }
    let now = process_elapsed_ns();
    let session_start = SESSION_START_NS.load(Ordering::Relaxed);
    if session_start == 0 {
        SESSION_START_NS.store(now, Ordering::Relaxed);
    }
    let last = LAST_GUEST_FRAME_NS.swap(now, Ordering::Relaxed);
    if last != 0 && now > last {
        let index = HISTORY_INDEX.fetch_add(1, Ordering::Relaxed) % HISTORY_SIZE;
        let milliseconds = (now - last) as f32 / 1_000_000.0;
        FRAME_MILLISECONDS[index].store(milliseconds.to_bits(), Ordering::Relaxed);
    }
    GUEST_FLIPS.fetch_add(1, Ordering::Relaxed);
    let last_refresh = LAST_GRAPH_REFRESH_NS.load(Ordering::Relaxed);
    if now.saturating_sub(last_refresh) >= 100_000_000
        && LAST_GRAPH_REFRESH_NS
            .compare_exchange(last_refresh, now, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
    {
        PIXEL_VERSION.fetch_add(1, Ordering::Relaxed);
    }
}

pub fn record_host_present() {
    if enabled() {
        HOST_PRESENTS.fetch_add(1, Ordering::Relaxed);
    }
}

pub fn record_draw() {
    if enabled() {
        GUEST_DRAWS.fetch_add(1, Ordering::Relaxed);
    }
}

fn refresh_stats(stats: &mut Stats) -> bool {
    let now = Instant::now();
    let elapsed = now.duration_since(stats.window_start).as_secs_f64();
    if elapsed < 1.0 {
        return false;
    }
    stats.window_start = now;
    stats.fps = GUEST_FLIPS.swap(0, Ordering::Relaxed) as f64 / elapsed;
    stats.present_fps = HOST_PRESENTS.swap(0, Ordering::Relaxed) as f64 / elapsed;
    stats.draws_per_second = GUEST_DRAWS.swap(0, Ordering::Relaxed) as f64 / elapsed;

    let allocated_now = crate::allocated_bytes_total();
    stats.allocated_mib_per_second = allocated_now.saturating_sub(stats.previous_allocated_bytes)
        as f64
        / elapsed
        / (1024.0 * 1024.0);
    stats.previous_allocated_bytes = allocated_now;

    let cpu_now = process_cpu_seconds();
    stats.cpu_percent = match (cpu_now, stats.previous_cpu_seconds) {
        (Some(now), Some(previous)) => cpu_percent(
            now - previous,
            elapsed,
            stats.logical_processors,
        ),
        _ => None,
    };
    stats.previous_cpu_seconds = cpu_now;
    stats.resident_memory_mb = resident_memory_mb();
    request_gpu_sample();
    let gpu = gpu_percent_label();

    let history_index = HISTORY_INDEX.load(Ordering::Relaxed);
    let last_frame_ms = if history_index == 0 {
        0.0
    } else {
        f32::from_bits(
            FRAME_MILLISECONDS[(history_index - 1) % HISTORY_SIZE].load(Ordering::Relaxed),
        ) as f64
    };
    if stats.fps > 0.0 {
        stats.frame_label = format!(
            "{:.1} MS",
            if last_frame_ms > 0.0 {
                last_frame_ms
            } else {
                1000.0 / stats.fps
            }
        );
    } else {
        let last_guest = LAST_GUEST_FRAME_NS.load(Ordering::Relaxed);
        let now_ns = process_elapsed_ns();
        if last_guest != 0 && now_ns > last_guest {
            stats.frame_label = format!(
                "STALL {:.1}S",
                (now_ns - last_guest) as f64 / 1_000_000_000.0
            );
        } else if stats.present_fps > 0.0 {
            stats.frame_label = format!("{:.1} MS", 1000.0 / stats.present_fps);
        } else {
            stats.frame_label = "0.0 MS".to_string();
        }
    }

    let seconds = session_elapsed_seconds();
    stats.elapsed = format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        (seconds / 60) % 60,
        seconds % 60
    );
    let cpu = stats
        .cpu_percent
        .map_or_else(|| "N/A".to_string(), |value| format!("{value:.0}%"));
    let memory = stats
        .resident_memory_mb
        .map_or_else(|| "N/A".to_string(), |value| format!("{value}M"));
    let (buffer_bytes, image_bytes, live_allocations, peak_allocations) = guest_resource_stats();
    let buffer_memory = resource_memory_label(buffer_bytes);
    let image_memory = resource_memory_label(image_bytes);
    // `VMEM` reads `used/limit` while the RAM limiter is active, so it is
    // obvious at a glance whether the cap is set and how close the app is to
    // it.
    let heap_bytes = crate::mem::guest_heap_bytes();
    let heap_limit = crate::mem::guest_heap_limit_bytes();
    let guest_memory = if heap_limit > 0 {
        format!(
            "{}/{}",
            resource_memory_label(heap_bytes),
            resource_memory_label(heap_limit)
        )
    } else {
        resource_memory_label(heap_bytes)
    };
    let draws_per_frame = if stats.fps > 0.0 {
        stats.draws_per_second / stats.fps
    } else {
        0.0
    };
    stats.full_lines = [
        format!(
            "FPS {:.1}  PRES {:.1}  {}",
            stats.fps, stats.present_fps, stats.frame_label
        ),
        format!(
            "DRAWS {:.0}/S  {:.0}/F  Q DIRECT",
            stats.draws_per_second, draws_per_frame
        ),
        // The old `GC 0/0/0` counter was dead weight: the Rust runtime has no
        // garbage collector, so it could never be anything but zero. Its room
        // now holds the GL buffer/texture totals, which frees the memory row
        // for VMEM.
        format!(
            "ALLOC {:.1}MB/S  BUF {}  IMG {}",
            stats.allocated_mib_per_second, buffer_memory, image_memory
        ),
        // MEM is the host process's resident set; VMEM is what the emulated
        // app itself has allocated through its own malloc.
        format!("MEM {}  VMEM {}", memory, guest_memory),
        format!("CPU {}  GPU {}", cpu, gpu),
        format!(
            "TIME {}  GLALLOC {}/{}",
            stats.elapsed, live_allocations, peak_allocations
        ),
    ];
    stats.minimal_lines = [
        format!("FPS {:.1}  CPU {}", stats.fps, cpu),
        format!("GPU {}  TIME {}", gpu, stats.elapsed),
    ];
    stats.title_summary = format!(
        "FPS {:.1} | CPU {} | GPU {} | TIME {}",
        stats.fps, cpu, gpu, stats.elapsed
    );
    PIXEL_VERSION.fetch_add(1, Ordering::Relaxed);
    true
}

pub fn take_title_bar_summary() -> Option<String> {
    if cfg!(any(target_os = "android", target_os = "ios"))
        || !enabled()
        || mode() != PerformanceOverlayMode::TitleBar
    {
        return None;
    }
    let mut stats = stats().lock().ok()?;
    refresh_stats(&mut stats).then(|| stats.title_summary.clone())
}

pub unsafe fn draw(gles: &mut dyn GLES, viewport: (u32, u32, u32, u32), context_token: usize) {
    use gles11::types::*;

    if !enabled() {
        return;
    }
    let selected_mode = display_mode();
    if selected_mode == PerformanceOverlayMode::TitleBar {
        return;
    }
    let display_width = if selected_mode == PerformanceOverlayMode::Full {
        PANEL_WIDTH
    } else {
        MINIMAL_WIDTH
    };
    let Some(geometry) = overlay_geometry(viewport, selected_mode, corner()) else {
        return;
    };
    let OverlayGeometry {
        left,
        top,
        width,
        height,
        uv_width,
        uv_height,
    } = geometry;

    let (_, _, viewport_width, viewport_height) = viewport;

    let Ok(mut stats) = stats().lock() else {
        return;
    };
    refresh_stats(&mut stats);
    let Ok(mut pixels) = PIXELS
        .get_or_init(|| Mutex::new(vec![0; PIXEL_BYTES]))
        .lock()
    else {
        return;
    };
    let requested_pixel_version = PIXEL_VERSION.load(Ordering::Relaxed);
    if PIXELS_VERSION.load(Ordering::Relaxed) != requested_pixel_version {
        fill_pixels(&mut pixels, &stats, selected_mode, display_width);
        PIXELS_VERSION.store(requested_pixel_version, Ordering::Relaxed);
    }
    let pixel_version = PIXELS_VERSION.load(Ordering::Relaxed);

    let mut old_active_texture: GLint = 0;
    gles.GetIntegerv(gles11::ACTIVE_TEXTURE, &mut old_active_texture);
    gles.ActiveTexture(gles11::TEXTURE0);
    let mut old_texture: GLint = 0;
    gles.GetIntegerv(gles11::TEXTURE_BINDING_2D, &mut old_texture);
    let mut old_matrix_mode: GLint = 0;
    gles.GetIntegerv(gles11::MATRIX_MODE, &mut old_matrix_mode);
    let mut old_client_active_texture = 0;
    gles.GetIntegerv(gles11::CLIENT_ACTIVE_TEXTURE, &mut old_client_active_texture);
    gles.ClientActiveTexture(gles11::TEXTURE0);
    let mut old_vertex_array: GLboolean = gles11::FALSE;
    let mut old_texture_array: GLboolean = gles11::FALSE;
    gles.GetBooleanv(gles11::VERTEX_ARRAY, &mut old_vertex_array);
    gles.GetBooleanv(gles11::TEXTURE_COORD_ARRAY, &mut old_texture_array);
    let old_blend = gles.IsEnabled(gles11::BLEND);
    let old_texture_2d = gles.IsEnabled(gles11::TEXTURE_2D);
    let mut old_color_mask = [gles11::TRUE; 4];
    gles.GetBooleanv(gles11::COLOR_WRITEMASK, old_color_mask.as_mut_ptr());
    let mut old_texture_env_mode = gles11::MODULATE as GLint;
    gles.GetTexEnviv(
        gles11::TEXTURE_ENV,
        gles11::TEXTURE_ENV_MODE,
        &mut old_texture_env_mode,
    );
    let old_overlay_caps = [
        (gles11::SCISSOR_TEST, gles.IsEnabled(gles11::SCISSOR_TEST)),
        (gles11::DEPTH_TEST, gles.IsEnabled(gles11::DEPTH_TEST)),
        (gles11::CULL_FACE, gles.IsEnabled(gles11::CULL_FACE)),
        (gles11::STENCIL_TEST, gles.IsEnabled(gles11::STENCIL_TEST)),
        (gles11::ALPHA_TEST, gles.IsEnabled(gles11::ALPHA_TEST)),
        (gles11::LIGHTING, gles.IsEnabled(gles11::LIGHTING)),
        (gles11::FOG, gles.IsEnabled(gles11::FOG)),
    ];
    let mut old_color = [0.0; 4];
    gles.GetFloatv(gles11::CURRENT_COLOR, old_color.as_mut_ptr());
    let mut old_array_buffer: GLint = 0;
    let mut old_vertex_array_buffer: GLint = 0;
    let mut old_texcoord_array_buffer: GLint = 0;
    gles.GetIntegerv(gles11::ARRAY_BUFFER_BINDING, &mut old_array_buffer);
    gles.GetIntegerv(
        gles11::VERTEX_ARRAY_BUFFER_BINDING,
        &mut old_vertex_array_buffer,
    );
    gles.GetIntegerv(
        gles11::TEXTURE_COORD_ARRAY_BUFFER_BINDING,
        &mut old_texcoord_array_buffer,
    );
    let mut old_vertex_size: GLint = 0;
    let mut old_vertex_type: GLint = 0;
    let mut old_vertex_stride: GLint = 0;
    let mut old_vertex_pointer: *const GLvoid = std::ptr::null();
    gles.GetIntegerv(gles11::VERTEX_ARRAY_SIZE, &mut old_vertex_size);
    gles.GetIntegerv(gles11::VERTEX_ARRAY_TYPE, &mut old_vertex_type);
    gles.GetIntegerv(gles11::VERTEX_ARRAY_STRIDE, &mut old_vertex_stride);
    gles.GetPointerv(gles11::VERTEX_ARRAY_POINTER, &mut old_vertex_pointer);
    let mut old_texcoord_size: GLint = 0;
    let mut old_texcoord_type: GLint = 0;
    let mut old_texcoord_stride: GLint = 0;
    let mut old_texcoord_pointer: *const GLvoid = std::ptr::null();
    gles.GetIntegerv(gles11::TEXTURE_COORD_ARRAY_SIZE, &mut old_texcoord_size);
    gles.GetIntegerv(gles11::TEXTURE_COORD_ARRAY_TYPE, &mut old_texcoord_type);
    gles.GetIntegerv(gles11::TEXTURE_COORD_ARRAY_STRIDE, &mut old_texcoord_stride);
    gles.GetPointerv(
        gles11::TEXTURE_COORD_ARRAY_POINTER,
        &mut old_texcoord_pointer,
    );
    let mut old_projection_matrix = [0.0; 16];
    let mut old_modelview_matrix = [0.0; 16];
    gles.MatrixMode(gles11::PROJECTION);
    gles.GetFloatv(gles11::PROJECTION_MATRIX, old_projection_matrix.as_mut_ptr());
    gles.MatrixMode(gles11::MODELVIEW);
    gles.GetFloatv(gles11::MODELVIEW_MATRIX, old_modelview_matrix.as_mut_ptr());
    gles.MatrixMode(old_matrix_mode as _);

    {
        let mut texture_slot = ES1_TEXTURE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut cached = match *texture_slot {
            Some(cached) if cached.context_token == context_token => cached,
            _ => {
                let mut texture = 0;
                gles.GenTextures(1, &mut texture);
                gles.BindTexture(gles11::TEXTURE_2D, texture);
                gles.TexImage2D(
                    gles11::TEXTURE_2D,
                    0,
                    gles11::RGBA as _,
                    PANEL_WIDTH as _,
                    PANEL_HEIGHT as _,
                    0,
                    gles11::RGBA,
                    gles11::UNSIGNED_BYTE,
                    std::ptr::null(),
                );
                gles.TexParameteri(
                    gles11::TEXTURE_2D,
                    gles11::TEXTURE_MIN_FILTER,
                    gles11::NEAREST as _,
                );
                gles.TexParameteri(
                    gles11::TEXTURE_2D,
                    gles11::TEXTURE_MAG_FILTER,
                    gles11::NEAREST as _,
                );
                gles.TexParameteri(
                    gles11::TEXTURE_2D,
                    gles11::TEXTURE_WRAP_S,
                    gles11::CLAMP_TO_EDGE as _,
                );
                gles.TexParameteri(
                    gles11::TEXTURE_2D,
                    gles11::TEXTURE_WRAP_T,
                    gles11::CLAMP_TO_EDGE as _,
                );
                CachedTexture {
                    context_token,
                    texture,
                    pixel_version: 0,
                }
            }
        };
        gles.BindTexture(gles11::TEXTURE_2D, cached.texture);
        if cached.pixel_version != pixel_version {
            gles.TexSubImage2D(
                gles11::TEXTURE_2D,
                0,
                0,
                0,
                PANEL_WIDTH as _,
                PANEL_HEIGHT as _,
                gles11::RGBA,
                gles11::UNSIGNED_BYTE,
                pixels.as_ptr() as *const GLvoid,
            );
            cached.pixel_version = pixel_version;
        }
        *texture_slot = Some(cached);
        cached.texture
    };

    gles.MatrixMode(gles11::PROJECTION);
    gles.LoadIdentity();
    gles.Orthof(
        0.0,
        viewport_width as _,
        viewport_height as _,
        0.0,
        -1.0,
        1.0,
    );
    gles.MatrixMode(gles11::MODELVIEW);
    gles.LoadIdentity();
    gles.EnableClientState(gles11::VERTEX_ARRAY);
    gles.EnableClientState(gles11::TEXTURE_COORD_ARRAY);
    gles.Enable(gles11::TEXTURE_2D);
    gles.Disable(gles11::BLEND);
    gles.Color4f(1.0, 1.0, 1.0, 1.0);
    gles.ColorMask(gles11::TRUE, gles11::TRUE, gles11::TRUE, gles11::TRUE);
    gles.TexEnvi(
        gles11::TEXTURE_ENV,
        gles11::TEXTURE_ENV_MODE,
        gles11::REPLACE as GLint,
    );
    for (capability, _) in &old_overlay_caps {
        gles.Disable(*capability);
    }
    let x0 = left as f32;
    let y0 = top as f32;
    let x1 = x0 + width as f32;
    let y1 = y0 + height as f32;
    let vertices = [x0, y0, x1, y0, x0, y1, x1, y1];
    let texcoords = [
        0.0, 0.0, uv_width, 0.0, 0.0, uv_height, uv_width, uv_height,
    ];
    gles.BindBuffer(gles11::ARRAY_BUFFER, 0);

    gles.VertexPointer(2, gles11::FLOAT, 0, vertices.as_ptr() as *const GLvoid);
    gles.TexCoordPointer(2, gles11::FLOAT, 0, texcoords.as_ptr() as *const GLvoid);
    gles.DrawArrays(gles11::TRIANGLE_STRIP, 0, 4);

    gles.BindBuffer(gles11::ARRAY_BUFFER, old_vertex_array_buffer as _);
    gles.VertexPointer(
        old_vertex_size,
        old_vertex_type as _,
        old_vertex_stride,
        old_vertex_pointer,
    );
    gles.BindBuffer(gles11::ARRAY_BUFFER, old_texcoord_array_buffer as _);
    gles.TexCoordPointer(
        old_texcoord_size,
        old_texcoord_type as _,
        old_texcoord_stride,
        old_texcoord_pointer,
    );
    gles.BindBuffer(gles11::ARRAY_BUFFER, old_array_buffer as _);
    gles.Color4f(old_color[0], old_color[1], old_color[2], old_color[3]);

    for (capability, was_enabled) in old_overlay_caps {
        if was_enabled == gles11::TRUE {
            gles.Enable(capability);
        }
    }

    gles.BindTexture(gles11::TEXTURE_2D, old_texture as _);
    if old_blend == gles11::TRUE {
        gles.Enable(gles11::BLEND);
    } else {
        gles.Disable(gles11::BLEND);
    }
    if old_texture_2d != gles11::TRUE {
        gles.Disable(gles11::TEXTURE_2D);
    }
    if old_vertex_array != gles11::TRUE {
        gles.DisableClientState(gles11::VERTEX_ARRAY);
    }
    if old_texture_array != gles11::TRUE {
        gles.DisableClientState(gles11::TEXTURE_COORD_ARRAY);
    }
    gles.ClientActiveTexture(old_client_active_texture as _);
    gles.ColorMask(
        old_color_mask[0],
        old_color_mask[1],
        old_color_mask[2],
        old_color_mask[3],
    );
    let old_texture_env_mode = [old_texture_env_mode];
    gles.TexEnviv(
        gles11::TEXTURE_ENV,
        gles11::TEXTURE_ENV_MODE,
        old_texture_env_mode.as_ptr(),
    );
    gles.MatrixMode(gles11::MODELVIEW);
    gles.LoadMatrixf(old_modelview_matrix.as_ptr());
    gles.MatrixMode(gles11::PROJECTION);
    gles.LoadMatrixf(old_projection_matrix.as_ptr());
    gles.MatrixMode(old_matrix_mode as _);
    gles.ActiveTexture(old_active_texture as _);
}

pub unsafe fn draw_es2(gles: &mut dyn GLES, viewport: (u32, u32, u32, u32), context_token: usize) {
    use super::gles2_raw as gles2;
    use gles2::types::*;

    if !enabled() {
        return;
    }
    let selected_mode = display_mode();
    if selected_mode == PerformanceOverlayMode::TitleBar {
        return;
    }
    let display_width = if selected_mode == PerformanceOverlayMode::Full {
        PANEL_WIDTH
    } else {
        MINIMAL_WIDTH
    };
    let Some(geometry) = overlay_geometry(viewport, selected_mode, corner()) else {
        return;
    };
    let OverlayGeometry {
        left,
        top,
        width,
        height,
        uv_width,
        uv_height,
    } = geometry;

    let Ok(mut stats) = stats().lock() else {
        return;
    };
    refresh_stats(&mut stats);
    let Ok(mut pixels) = PIXELS
        .get_or_init(|| Mutex::new(vec![0; PIXEL_BYTES]))
        .lock()
    else {
        return;
    };
    let requested_pixel_version = PIXEL_VERSION.load(Ordering::Relaxed);
    if PIXELS_VERSION.load(Ordering::Relaxed) != requested_pixel_version {
        fill_pixels(&mut pixels, &stats, selected_mode, display_width);
        PIXELS_VERSION.store(requested_pixel_version, Ordering::Relaxed);
    }
    let pixel_version = PIXELS_VERSION.load(Ordering::Relaxed);

    let mut old_active_texture: GLint = 0;
    gles.GetIntegerv(gles2::ACTIVE_TEXTURE, &mut old_active_texture);
    gles.ActiveTexture(gles2::TEXTURE0);
    let mut old_texture: GLint = 0;
    gles.GetIntegerv(gles2::TEXTURE_BINDING_2D, &mut old_texture);
    let texture = {
        let mut texture_slot = ES2_TEXTURE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut cached = match *texture_slot {
            Some(cached) if cached.context_token == context_token => cached,
            _ => {
                let mut texture = 0;
                gles.GenTextures(1, &mut texture);
                gles.BindTexture(gles2::TEXTURE_2D, texture);
                gles.TexParameteri(
                    gles2::TEXTURE_2D,
                    gles2::TEXTURE_MIN_FILTER,
                    gles2::NEAREST as _,
                );
                gles.TexParameteri(
                    gles2::TEXTURE_2D,
                    gles2::TEXTURE_MAG_FILTER,
                    gles2::NEAREST as _,
                );
                gles.TexParameteri(
                    gles2::TEXTURE_2D,
                    gles2::TEXTURE_WRAP_S,
                    gles2::CLAMP_TO_EDGE as _,
                );
                gles.TexParameteri(
                    gles2::TEXTURE_2D,
                    gles2::TEXTURE_WRAP_T,
                    gles2::CLAMP_TO_EDGE as _,
                );
                gles.TexImage2D(
                    gles2::TEXTURE_2D,
                    0,
                    gles2::RGBA as _,
                    PANEL_WIDTH as _,
                    PANEL_HEIGHT as _,
                    0,
                    gles2::RGBA,
                    gles2::UNSIGNED_BYTE,
                    std::ptr::null(),
                );
                CachedTexture {
                    context_token,
                    texture,
                    pixel_version: 0,
                }
            }
        };
        gles.BindTexture(gles2::TEXTURE_2D, cached.texture);
        if cached.pixel_version != pixel_version {
            gles.TexSubImage2D(
                gles2::TEXTURE_2D,
                0,
                0,
                0,
                PANEL_WIDTH as _,
                PANEL_HEIGHT as _,
                gles2::RGBA,
                gles2::UNSIGNED_BYTE,
                pixels.as_ptr() as *const GLvoid,
            );
            cached.pixel_version = pixel_version;
        }
        *texture_slot = Some(cached);
        cached.texture
    };

    crate::trainer_ui::draw_texture_es2(
        gles,
        viewport,
        context_token,
        texture,
        (left as f32, top as f32, width as f32, height as f32),
        (uv_width, uv_height),
    );
    gles.BindTexture(gles2::TEXTURE_2D, old_texture as _);
    gles.ActiveTexture(old_active_texture as _);
}

fn fill_pixels(
    buffer: &mut [u8],
    stats: &Stats,
    mode: PerformanceOverlayMode,
    display_width: usize,
) {
    for pixel in buffer.chunks_exact_mut(4) {
        pixel.copy_from_slice(&[0x14, 0x18, 0x20, 0xFF]);
    }
    if mode == PerformanceOverlayMode::Full {
        let colors = [
            [0x60, 0xFF, 0x60],
            [0xFF, 0xFF, 0xFF],
            [0xB0, 0xD0, 0xFF],
            [0xB0, 0xB0, 0xB0],
            [0xB0, 0xB0, 0xB0],
            [0xFF, 0xD0, 0x80],
        ];
        for (index, line) in stats.full_lines.iter().enumerate() {
            draw_string(
                buffer,
                8,
                6 + index * LINE_HEIGHT,
                line,
                colors[index],
                display_width,
            );
        }
        draw_frame_graph(buffer, 8, 118, PANEL_WIDTH - 16, PANEL_HEIGHT - 124);
    } else {
        draw_string(
            buffer,
            8,
            6,
            &stats.minimal_lines[0],
            [0x60, 0xFF, 0x60],
            display_width,
        );
        draw_string(
            buffer,
            8,
            6 + LINE_HEIGHT,
            &stats.minimal_lines[1],
            [0xFF, 0xD0, 0x80],
            display_width,
        );
    }
}

fn draw_frame_graph(buffer: &mut [u8], x: usize, y: usize, width: usize, height: usize) {
    if height < 8 {
        return;
    }
    draw_horizontal_line(
        buffer,
        x,
        y + height - (16.7 / 40.0 * height as f64) as usize,
        width,
        [0x30, 0x60, 0x30],
    );
    draw_horizontal_line(
        buffer,
        x,
        y + height - (33.3 / 40.0 * height as f64) as usize,
        width,
        [0x30, 0x30, 0x60],
    );
    let bar_width = (width / HISTORY_SIZE).max(1);
    let history_index = HISTORY_INDEX.load(Ordering::Relaxed);
    for column in 0..HISTORY_SIZE {
        let milliseconds = f32::from_bits(
            FRAME_MILLISECONDS[(history_index + column) % HISTORY_SIZE].load(Ordering::Relaxed),
        ) as f64;
        if milliseconds <= 0.0 {
            continue;
        }
        let bar_height = (milliseconds / 40.0 * height as f64).min(height as f64) as usize;
        let bar_x = x + column * bar_width;
        if bar_x + bar_width > x + width {
            break;
        }
        let color = if milliseconds <= 17.5 {
            [0x40, 0xE0, 0x40]
        } else if milliseconds <= 34.0 {
            [0xE0, 0xC0, 0x30]
        } else {
            [0xE8, 0x40, 0x40]
        };
        for row in 0..bar_height {
            for pixel_x in 0..bar_width {
                set_pixel(buffer, bar_x + pixel_x, y + height - 1 - row, color);
            }
        }
    }
}

fn draw_horizontal_line(buffer: &mut [u8], x: usize, y: usize, width: usize, color: [u8; 3]) {
    for pixel_x in 0..width {
        set_pixel(buffer, x + pixel_x, y, color);
    }
}

fn draw_string(
    buffer: &mut [u8],
    x: usize,
    y: usize,
    text: &str,
    color: [u8; 3],
    display_width: usize,
) {
    let mut pen_x = x;
    for character in text.bytes() {
        let rows = glyph_rows(character.to_ascii_uppercase());
        for (row_index, bits) in rows.iter().enumerate() {
            for column in 0..FONT_WIDTH {
                if bits & (1 << (FONT_WIDTH - 1 - column)) == 0 {
                    continue;
                }
                for scale_y in 0..FONT_SCALE {
                    for scale_x in 0..FONT_SCALE {
                        set_pixel(
                            buffer,
                            pen_x + column * FONT_SCALE + scale_x,
                            y + row_index * FONT_SCALE + scale_y,
                            color,
                        );
                    }
                }
            }
        }
        pen_x += CELL_WIDTH;
        if pen_x + CELL_WIDTH > display_width {
            break;
        }
    }
}

fn glyph_rows(character: u8) -> [u8; FONT_HEIGHT] {
    match character {
        b'A' => [14, 17, 17, 31, 17, 17, 17],
        b'B' => [30, 17, 17, 30, 17, 17, 30],
        b'C' => [14, 17, 16, 16, 16, 17, 14],
        b'D' => [30, 17, 17, 17, 17, 17, 30],
        b'E' => [31, 16, 16, 30, 16, 16, 31],
        b'F' => [31, 16, 16, 30, 16, 16, 16],
        b'G' => [14, 17, 16, 23, 17, 17, 15],
        b'I' => [14, 4, 4, 4, 4, 4, 14],
        b'K' => [17, 18, 20, 24, 20, 18, 17],
        b'L' => [16, 16, 16, 16, 16, 16, 31],
        b'M' => [17, 27, 21, 21, 17, 17, 17],
        b'N' => [17, 25, 21, 19, 17, 17, 17],
        b'O' => [14, 17, 17, 17, 17, 17, 14],
        b'P' => [30, 17, 17, 30, 16, 16, 16],
        b'Q' => [14, 17, 17, 17, 21, 18, 13],
        b'R' => [30, 17, 17, 30, 20, 18, 17],
        b'S' => [15, 16, 16, 14, 1, 1, 30],
        b'T' => [31, 4, 4, 4, 4, 4, 4],
        b'U' => [17, 17, 17, 17, 17, 17, 14],
        b'V' => [17, 17, 17, 17, 17, 10, 4],
        b'W' => [17, 17, 17, 21, 21, 21, 10],
        b'0' => [14, 17, 19, 21, 25, 17, 14],
        b'1' => [4, 12, 4, 4, 4, 4, 14],
        b'2' => [14, 17, 1, 2, 4, 8, 31],
        b'3' => [30, 1, 1, 14, 1, 1, 30],
        b'4' => [2, 6, 10, 18, 31, 2, 2],
        b'5' => [31, 16, 16, 30, 1, 1, 30],
        b'6' => [14, 16, 16, 30, 17, 17, 14],
        b'7' => [31, 1, 2, 4, 8, 8, 8],
        b'8' => [14, 17, 17, 14, 17, 17, 14],
        b'9' => [14, 17, 17, 15, 1, 1, 14],
        b':' => [0, 4, 4, 0, 4, 4, 0],
        b'.' => [0, 0, 0, 0, 0, 4, 4],
        b'/' => [1, 2, 4, 8, 16, 0, 0],
        b'%' => [17, 2, 4, 8, 17, 0, 0],
        b'|' => [4, 4, 4, 4, 4, 4, 4],
        b'+' => [0, 4, 4, 31, 4, 4, 0],
        _ => [0; FONT_HEIGHT],
    }
}

fn set_pixel(buffer: &mut [u8], x: usize, y: usize, color: [u8; 3]) {
    if x >= PANEL_WIDTH || y >= PANEL_HEIGHT {
        return;
    }
    let offset = (y * PANEL_WIDTH + x) * 4;
    buffer[offset] = color[0];
    buffer[offset + 1] = color[1];
    buffer[offset + 2] = color[2];
    buffer[offset + 3] = 0xFF;
}

fn logical_processor_count() -> usize {
    #[cfg(target_os = "android")]
    if let Some(count) = std::fs::read_to_string("/sys/devices/system/cpu/online")
        .ok()
        .and_then(|online| parse_cpu_list(&online))
    {
        return count;
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        let count = unsafe { libc::sysconf(libc::_SC_NPROCESSORS_ONLN) };
        if count > 0 {
            return count as usize;
        }
    }
    std::thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(1)
}

#[cfg(any(target_os = "android", test))]
fn parse_cpu_list(contents: &str) -> Option<usize> {
    let mut count = 0usize;
    for range in contents.trim().split(',') {
        if range.is_empty() {
            return None;
        }
        let (first, last) = match range.split_once('-') {
            Some((first, last)) => (first.parse::<usize>().ok()?, last.parse::<usize>().ok()?),
            None => {
                let cpu = range.parse::<usize>().ok()?;
                (cpu, cpu)
            }
        };
        if last < first {
            return None;
        }
        count = count.checked_add(last.checked_sub(first)?.checked_add(1)?)?;
    }
    (count > 0).then_some(count)
}

/// CPU usage of the whole emulator process, as a percentage of one core.
///
/// This is what `top` and Android's own tools report as %CPU: a process that
/// keeps one core busy shows 100%, and one that keeps two busy shows 200%.
/// Dividing by the number of logical processors instead (the old behaviour)
/// made a saturated core read as 13% on an 8-core phone, which looks like the
/// emulator is idle while it is in fact pegging a core; the processor count is
/// only used to cap the result, since the process cannot use more than the
/// whole machine.
fn cpu_percent(cpu_seconds: f64, wall_seconds: f64, processors: usize) -> Option<f64> {
    if !cpu_seconds.is_finite()
        || !wall_seconds.is_finite()
        || wall_seconds <= 0.0
        || processors == 0
    {
        return None;
    }
    Some((cpu_seconds.max(0.0) / wall_seconds * 100.0).clamp(0.0, 100.0 * processors as f64))
}

#[cfg(unix)]
fn process_cpu_seconds() -> Option<f64> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } != 0 {
        return None;
    }
    let usage = unsafe { usage.assume_init() };
    let time = |value: libc::timeval| value.tv_sec as f64 + value.tv_usec as f64 / 1_000_000.0;
    Some(time(usage.ru_utime) + time(usage.ru_stime))
}

#[cfg(target_os = "windows")]
fn process_cpu_seconds() -> Option<f64> {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    if unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
    } == 0
    {
        return None;
    }
    let ticks = |time: FILETIME| ((time.dwHighDateTime as u64) << 32) | time.dwLowDateTime as u64;
    Some((ticks(kernel) + ticks(user)) as f64 / 10_000_000.0)
}

#[cfg(not(any(unix, target_os = "windows")))]
fn process_cpu_seconds() -> Option<f64> {
    None
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn resident_memory_mb() -> Option<u64> {
    let resident_pages = std::fs::read_to_string("/proc/self/statm")
        .ok()?
        .split_whitespace()
        .nth(1)?
        .parse::<u64>()
        .ok()?;
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    (page_size > 0).then(|| resident_pages * page_size as u64 / (1024 * 1024))
}

#[cfg(target_os = "windows")]
fn resident_memory_mb() -> Option<u64> {
    use windows_sys::Win32::System::ProcessStatus::{
        K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let mut counters = PROCESS_MEMORY_COUNTERS::default();
    counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    if unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) } == 0 {
        return None;
    }
    Some(counters.WorkingSetSize as u64 / (1024 * 1024))
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_os = "windows")))]
fn resident_memory_mb() -> Option<u64> {
    None
}

fn gpu_percent_label() -> String {
    gpu_percent().map_or_else(|| "N/A".to_string(), |percent| format!("{percent:.0}%"))
}

#[cfg(any(target_os = "windows", target_os = "android"))]
static GPU_PERCENT_BITS: AtomicU64 = AtomicU64::new(0x7ff8_0000_0000_0000);
#[cfg(any(target_os = "windows", target_os = "android"))]
static GPU_SAMPLE_PENDING: AtomicBool = AtomicBool::new(false);
#[cfg(target_os = "windows")]
static GPU_QUERY: AtomicUsize = AtomicUsize::new(0);
#[cfg(target_os = "windows")]
static GPU_COUNTER: AtomicUsize = AtomicUsize::new(0);

#[cfg(target_os = "windows")]
fn gpu_percent() -> Option<f64> {
    let value = f64::from_bits(GPU_PERCENT_BITS.load(Ordering::Relaxed));
    value.is_finite().then_some(value)
}

#[cfg(target_os = "android")]
fn gpu_percent() -> Option<f64> {
    let value = f64::from_bits(GPU_PERCENT_BITS.load(Ordering::Relaxed));
    value.is_finite().then_some(value)
}

#[cfg(target_os = "android")]
fn read_gpu_percent_from_path(path: &std::path::Path) -> Option<f64> {
    let contents = std::fs::read_to_string(path).ok()?;
    let value = parse_gpu_percent(&contents)?;
    static GPU_SOURCE_LOGGED: AtomicBool = AtomicBool::new(false);
    if !GPU_SOURCE_LOGGED.swap(true, Ordering::Relaxed) {
        log!("GPU utilization is being read from {}.", path.display());
    }
    Some(value)
}

#[cfg(target_os = "android")]
fn collect_gpu_sample() -> Option<f64> {
    const KNOWN_PATHS: &[&str] = &[
        "/sys/module/ged/parameters/gpu_loading",
        "/sys/kernel/ged/gpu_loading",
        "/sys/kernel/ged/gpu_busy",
        "/sys/kernel/ged/hal/gpu_utilization",
        "/sys/kernel/ged/hal/gpu_loading",
        "/sys/kernel/ged/hal/current/gpu_loading",
        "/sys/kernel/ged/hal/current/gpu_utilization",
        "/sys/kernel/debug/ged/hal/gpu_utilization",
        "/d/ged/hal/gpu_utilization",
        "/sys/kernel/gpu/gpu_loading",
        "/sys/kernel/gpu/gpu_load",
        "/sys/kernel/gpu/gpu_busy",
        "/sys/devices/platform/soc/13000000.mali/gpu_utilization",
        "/sys/devices/platform/soc/13000000.mali/devfreq/load",
        "/sys/class/misc/mali0/device/gpu_utilization",
        "/sys/class/misc/mali0/device/utilization",
        "/sys/class/misc/mali0/device/devfreq/load",
    ];
    for path in KNOWN_PATHS {
        if let Some(value) = read_gpu_percent_from_path(std::path::Path::new(path)) {
            return Some(value);
        }
    }
    for root in ["/sys/class/devfreq", "/sys/class/misc", "/sys/devices/platform/soc"] {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if !name.contains("gpu") && !name.contains("mali") {
                continue;
            }
            let base = entry.path();
            let paths = if root == "/sys/class/devfreq" {
                vec![
                    base.join("gpu_loading"),
                    base.join("gpu_utilization"),
                    base.join("utilization"),
                    base.join("load"),
                ]
            } else if root == "/sys/class/misc" {
                vec![
                    base.join("device/gpu_utilization"),
                    base.join("device/gpu_loading"),
                    base.join("device/utilization"),
                    base.join("device/devfreq/load"),
                ]
            } else {
                vec![
                    base.join("gpu_utilization"),
                    base.join("gpu_loading"),
                    base.join("utilization"),
                    base.join("devfreq/load"),
                ]
            };
            for path in paths {
                if let Some(value) = read_gpu_percent_from_path(&path) {
                    return Some(value);
                }
            }
        }
    }
    static GPU_UNAVAILABLE_LOGGED: AtomicBool = AtomicBool::new(false);
    if !GPU_UNAVAILABLE_LOGGED.swap(true, Ordering::Relaxed) {
        log!("GPU utilization is N/A: Android did not expose a readable supported GPU-load counter.");
    }
    None
}

#[cfg(not(any(target_os = "windows", target_os = "android")))]
fn gpu_percent() -> Option<f64> {
    None
}

#[cfg(any(target_os = "android", test))]
fn parse_gpu_percent(contents: &str) -> Option<f64> {
    let contents = contents.trim();
    let value = match contents.split_once('@') {
        Some((value, frequency)) if frequency.trim().parse::<u64>().is_ok() => value.trim(),
        Some(_) => return None,
        None => contents,
    };
    let parse_percent = |value: &str| {
        let value = value.strip_suffix('%').unwrap_or(value);
        let value = value.parse::<f64>().ok()?;
        (value.is_finite() && (0.0..=100.0).contains(&value)).then_some(value)
    };
    let mut values = value.split_whitespace();
    let load = parse_percent(values.next()?)?;
    match (values.next(), values.next(), values.next()) {
        (None, None, None) => Some(load),
        (Some(blocking), Some(idle), None)
            if parse_percent(blocking).is_some() && parse_percent(idle).is_some() =>
        {
            Some(load)
        }
        _ => None,
    }
}

#[cfg(target_os = "windows")]
fn request_gpu_sample() {
    if GPU_SAMPLE_PENDING
        .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
        .is_err()
    {
        return;
    }
    if std::thread::Builder::new()
        .name("touchHLE-GPU-meter".to_string())
        .spawn(|| {
            let value = collect_gpu_sample().unwrap_or(f64::NAN);
            GPU_PERCENT_BITS.store(value.to_bits(), Ordering::Relaxed);
            GPU_SAMPLE_PENDING.store(false, Ordering::Relaxed);
        })
        .is_err()
    {
        GPU_SAMPLE_PENDING.store(false, Ordering::Relaxed);
    }
}

#[cfg(target_os = "android")]
fn request_gpu_sample() {
    if GPU_SAMPLE_PENDING
        .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
        .is_err()
    {
        return;
    }
    if std::thread::Builder::new()
        .name("touchHLE-GPU-meter".to_string())
        .spawn(|| {
            let value = collect_gpu_sample().unwrap_or(f64::NAN);
            GPU_PERCENT_BITS.store(value.to_bits(), Ordering::Relaxed);
            GPU_SAMPLE_PENDING.store(false, Ordering::Relaxed);
        })
        .is_err()
    {
        GPU_SAMPLE_PENDING.store(false, Ordering::Relaxed);
    }
}

#[cfg(not(any(target_os = "windows", target_os = "android")))]
fn request_gpu_sample() {}

#[cfg(target_os = "windows")]
fn collect_gpu_sample() -> Option<f64> {
    use windows_sys::Win32::System::Performance::{
        PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
        PdhOpenQueryW, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY,
        PDH_MORE_DATA,
    };

    let query_handle = GPU_QUERY.load(Ordering::Relaxed);
    let counter_handle = GPU_COUNTER.load(Ordering::Relaxed);
    let (query, counter) = if query_handle == 0 {
        let mut query: PDH_HQUERY = std::ptr::null_mut();
        if unsafe { PdhOpenQueryW(std::ptr::null(), 0, &mut query) } != 0 {
            return None;
        }
        let path = format!(
            "\\GPU Engine(pid_{}_*)\\Utilization Percentage",
            std::process::id()
        );
        let path: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        let mut counter: PDH_HCOUNTER = std::ptr::null_mut();
        if unsafe { PdhAddEnglishCounterW(query, path.as_ptr(), 0, &mut counter) } != 0 {
            unsafe { PdhCloseQuery(query) };
            return None;
        }
        if unsafe { PdhCollectQueryData(query) } != 0 {
            unsafe { PdhCloseQuery(query) };
            return None;
        }
        GPU_COUNTER.store(counter as usize, Ordering::Relaxed);
        GPU_QUERY.store(query as usize, Ordering::Relaxed);
        return None;
    } else {
        (query_handle as PDH_HQUERY, counter_handle as PDH_HCOUNTER)
    };

    if unsafe { PdhCollectQueryData(query) } != 0 {
        return None;
    }
    let mut buffer_bytes = 0u32;
    let mut item_count = 0u32;
    let status = unsafe {
        PdhGetFormattedCounterArrayW(
            counter,
            PDH_FMT_DOUBLE,
            &mut buffer_bytes,
            &mut item_count,
            std::ptr::null_mut(),
        )
    };
    if status != PDH_MORE_DATA || buffer_bytes == 0 {
        return None;
    }
    let item_size = std::mem::size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>();
    if buffer_bytes as usize % item_size != 0 {
        return None;
    }
    let capacity = buffer_bytes as usize / item_size;
    let mut items = vec![PDH_FMT_COUNTERVALUE_ITEM_W::default(); capacity];
    let status = unsafe {
        PdhGetFormattedCounterArrayW(
            counter,
            PDH_FMT_DOUBLE,
            &mut buffer_bytes,
            &mut item_count,
            items.as_mut_ptr(),
        )
    };
    if status != 0 || item_count as usize > items.len() {
        return None;
    }
    let mut maximum: Option<f64> = None;
    for item in &items[..item_count as usize] {
        if item.FmtValue.CStatus > 1 {
            continue;
        }
        let value = unsafe { item.FmtValue.Anonymous.doubleValue };
        if value.is_finite() && value >= 0.0 {
            maximum = Some(
                maximum
                    .map_or(value, |current| current.max(value))
                    .min(100.0),
            );
        }
    }
    maximum
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_picker_never_shows_the_overlay_and_games_honour_the_setting() {
        let visibility = OverlayVisibility::new();
        assert!(visibility.enabled());
        visibility.set_app_picker(true);
        assert!(!visibility.enabled());
        visibility.set_requested(false);
        assert!(!visibility.enabled());
        visibility.set_requested(true);
        assert!(!visibility.enabled());

        visibility.set_app_picker(false);
        assert!(visibility.enabled());
        visibility.set_requested(false);
        assert!(!visibility.enabled());
        visibility.set_app_picker(true);
        visibility.set_app_picker(false);
        assert!(!visibility.enabled());
    }

    #[test]
    fn gpu_percent_parser_accepts_scalar_and_mtk_ged_values() {
        assert_eq!(parse_gpu_percent("42\n"), Some(42.0));
        assert_eq!(parse_gpu_percent("42%\n"), Some(42.0));
        assert_eq!(parse_gpu_percent("42@800000\n"), Some(42.0));
        assert_eq!(parse_gpu_percent("42@unknown"), None);
        assert_eq!(parse_gpu_percent("42 13 45\n"), Some(42.0));
        assert_eq!(parse_gpu_percent("42% 13% 45%\n"), Some(42.0));
        assert_eq!(parse_gpu_percent("42 13 45 1\n"), None);
        assert_eq!(parse_gpu_percent("0"), Some(0.0));
        assert_eq!(parse_gpu_percent("100"), Some(100.0));
        assert_eq!(parse_gpu_percent("42 800000"), None);
        assert_eq!(parse_gpu_percent("42 13 101"), None);
        assert_eq!(parse_gpu_percent("101"), None);
        assert_eq!(parse_gpu_percent("-1"), None);
        assert_eq!(parse_gpu_percent("N/A"), None);
    }

    #[test]
    fn resource_memory_labels_preserve_small_allocations() {
        assert_eq!(resource_memory_label(0), "0K");
        assert_eq!(resource_memory_label(1), "1K");
        assert_eq!(resource_memory_label(512 * 1024), "512K");
        assert_eq!(resource_memory_label(1024 * 1024), "1M");
    }

    #[test]
    fn guest_resource_tracker_replaces_and_deletes_allocations() {
        let mut tracker = GuestResourceTracker::default();
        tracker.register_context(7);
        tracker.set_buffer_size(7, 2, 4096);
        tracker.set_buffer_size(7, 2, 1024);
        tracker.set_image_size(7, gles11::TEXTURE_2D, 3, 0, 2048);
        tracker.set_image_size(7, gles11::TEXTURE_2D, 3, 0, 512);
        assert_eq!(tracker.totals(), (1024, 512, 2, 2));

        tracker.delete_buffer(7, 2);
        tracker.delete_texture(7, 3);
        assert_eq!(tracker.totals(), (0, 0, 0, 2));
    }

    #[test]
    fn shared_context_resources_are_removed_after_last_context() {
        let mut tracker = GuestResourceTracker::default();
        tracker.register_context(11);
        tracker.register_context(11);
        tracker.set_buffer_size(11, 5, 4096);
        tracker.set_image_size(11, gles11::TEXTURE_2D, 9, 0, 8192);

        tracker.unregister_context(11);
        assert_eq!(tracker.totals(), (4096, 8192, 2, 2));

        tracker.unregister_context(11);
        assert_eq!(tracker.totals(), (0, 0, 0, 2));
    }

    #[test]
    fn full_overlay_scales_to_fit_mobile_viewports_without_cropping() {
        let geometry = overlay_geometry(
            (0, 0, 320, 460),
            PerformanceOverlayMode::Full,
            PerformanceOverlayCorner::BottomRight,
        )
        .unwrap();
        assert_eq!((geometry.width, geometry.height), (296, 147));
        assert_eq!((geometry.uv_width, geometry.uv_height), (1.0, 1.0));
        assert_eq!(geometry.left + geometry.width, 308);
        assert_eq!(geometry.top + geometry.height, 448);
    }

    #[test]
    fn minimal_overlay_scales_texture_crop_to_the_viewport() {
        let geometry = overlay_geometry(
            (0, 0, 320, 460),
            PerformanceOverlayMode::Minimal,
            PerformanceOverlayCorner::TopRight,
        )
        .unwrap();
        assert_eq!(geometry.width, 296);
        assert_eq!(geometry.uv_width, MINIMAL_WIDTH as f32 / PANEL_WIDTH as f32);
        assert_eq!(geometry.uv_height, MINIMAL_HEIGHT as f32 / PANEL_HEIGHT as f32);
    }

    #[test]
    fn corner_cycle_matches_sharpemu_clockwise_order() {
        let saved = CORNER.swap(0, Ordering::Relaxed);
        assert_eq!(cycle_corner(), "top-right");
        assert_eq!(cycle_corner(), "bottom-right");
        assert_eq!(cycle_corner(), "bottom-left");
        assert_eq!(cycle_corner(), "top-left");
        CORNER.store(saved, Ordering::Relaxed);
    }

    #[test]
    fn cpu_list_parser_counts_online_cpus() {
        assert_eq!(parse_cpu_list("0-7\n"), Some(8));
        assert_eq!(parse_cpu_list("0-3,5,7-8"), Some(7));
        assert_eq!(parse_cpu_list(""), None);
        assert_eq!(parse_cpu_list("3-1"), None);
        assert_eq!(parse_cpu_list("0,broken"), None);
    }

    #[test]
    fn cpu_usage_is_a_percentage_of_one_core() {
        // One busy core reads as 100%, like top's %CPU, not as 1/8th of the
        // machine.
        assert_eq!(cpu_percent(1.0, 1.0, 8), Some(100.0));
        assert_eq!(cpu_percent(0.5, 1.0, 8), Some(50.0));
        assert_eq!(cpu_percent(1.5, 1.0, 8), Some(150.0));
        // The whole machine is the ceiling.
        assert_eq!(cpu_percent(100.0, 1.0, 8), Some(800.0));
        assert_eq!(cpu_percent(1.0, 0.0, 8), None);
        assert_eq!(cpu_percent(-1.0, 1.0, 8), Some(0.0));
    }

    #[test]
    fn resource_memory_labels_do_not_hide_sub_megabyte_allocations() {
        assert_eq!(resource_memory_label(0), "0K");
        assert_eq!(resource_memory_label(512 * 1024), "512K");
        assert_eq!(resource_memory_label(1024 * 1024), "1M");
    }

    #[test]
    fn text_font_covers_all_overlay_labels() {
        let text = "FPS | PRES MS DRAWS S F Q DIRECT ALLOC MB VMEM MEM BUF IMG CPU GPU TIME GLALLOC STALL 0123456789 /";
        assert!(text
            .bytes()
            .all(|character| glyph_rows(character.to_ascii_uppercase())
                .iter()
                .any(|row| *row != 0)
                || character == b' '));
    }
}
