/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Parsing and management of user-configurable options, e.g. for input methods.

use crate::gles::GLESImplementation;
use crate::window::{DeviceFamily, DeviceOrientation};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::net::{SocketAddr, ToSocketAddrs};
use std::num::NonZeroU32;
use std::path::PathBuf;

pub const OPTIONS_HELP: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/OPTIONS_HELP.txt"));

/// Game controller button for `--button-to-touch=` option.
#[derive(Copy, Clone, Hash, PartialEq, Eq, Debug)]
pub enum Button {
    DPadLeft,
    DPadUp,
    DPadRight,
    DPadDown,
    Start,
    A,
    B,
    X,
    Y,
    LeftShoulder,
}

/// Highest iOS version currently exposed by the emulator compatibility layer.
pub const LATEST_IOS_VERSION: (i32, i32, i32) = (12, 0, 0);

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PerformanceOverlayMode {
    Full,
    Minimal,
    TitleBar,
}

impl PerformanceOverlayMode {
    pub fn from_short_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "full" => Some(Self::Full),
            "minimal" => Some(Self::Minimal),
            "titlebar" => Some(Self::TitleBar),
            _ => None,
        }
    }

    pub fn short_name(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Minimal => "minimal",
            Self::TitleBar => "titlebar",
        }
    }

    pub fn for_current_platform(self) -> Self {
        self.for_platform(!cfg!(any(target_os = "android", target_os = "ios")))
    }

    fn for_platform(self, title_bar_available: bool) -> Self {
        if !title_bar_available && self == Self::TitleBar {
            Self::Minimal
        } else {
            self
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PerformanceOverlayCorner {
    TopLeft,
    TopRight,
    BottomRight,
    BottomLeft,
}

impl PerformanceOverlayCorner {
    pub fn from_short_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "topleft" => Some(Self::TopLeft),
            "topright" => Some(Self::TopRight),
            "bottomright" => Some(Self::BottomRight),
            "bottomleft" => Some(Self::BottomLeft),
            _ => None,
        }
    }

    pub fn short_name(self) -> &'static str {
        match self {
            Self::TopLeft => "topleft",
            Self::TopRight => "topright",
            Self::BottomRight => "bottomright",
            Self::BottomLeft => "bottomleft",
        }
    }
}

/// How `-[EAGLContext presentRenderbuffer:]` gets a rendered frame onto the
/// host window when the app draws into a fullscreen `CAEAGLLayer`
/// (`--present-mode=`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PresentMode {
    /// Present on the GPU (copy the renderbuffer into a texture and draw a
    /// quad into the window). If the first frames come out black even though
    /// the renderbuffer has content, automatically fall back to `Readback`.
    Auto,
    /// Always present on the GPU, never fall back.
    Direct,
    /// Read the renderbuffer back to system RAM with `glReadPixels()` and
    /// push it through the Core Animation compositor. Slow (a full GPU
    /// pipeline stall plus two full-frame copies per frame), but it avoids
    /// touching the app's GL state and is a useful workaround for broken
    /// vendor OpenGL ES 1.1 drivers.
    Readback,
}

impl PresentMode {
    pub fn from_short_name(name: &str) -> Result<Self, ()> {
        match name {
            "auto" => Ok(Self::Auto),
            "direct" => Ok(Self::Direct),
            "readback" => Ok(Self::Readback),
            _ => Err(()),
        }
    }
}

/// Whether host buffer swaps wait for the display's vertical refresh
/// (`--vsync=`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum VsyncMode {
    /// Android: synchronize swaps to the display. Other platforms: leave the
    /// driver's default alone.
    Auto,
    /// Swap interval 1: every swap waits for the next vertical refresh.
    On,
    /// Swap interval 0: swaps never block.
    Off,
}

impl VsyncMode {
    pub fn from_short_name(name: &str) -> Result<Self, ()> {
        match name {
            "auto" => Ok(Self::Auto),
            "on" | "1" => Ok(Self::On),
            "off" | "0" => Ok(Self::Off),
            _ => Err(()),
        }
    }
}

/// Struct containing all user-configurable options.
#[derive(Clone)]
pub struct Options {
    pub fullscreen: bool,
    /// Keep the emulated display's aspect ratio when resizing a desktop window.
    pub preserve_aspect_ratio: bool,
    pub device_family: Option<DeviceFamily>,
    pub auto_device_family: bool,
    /// When set, the guest sees a screen of exactly this size (in points) and
    /// scale 1.0, instead of one of the fixed device profiles. Populated by
    /// `--device-family=auto` (from the host display) or via the explicit
    /// `--screen-size=WxH` override below.
    pub host_screen_size: Option<(u32, u32)>,
    /// Disable the Cheat Engine-style memory trainer overlay. The trainer
    /// is off by default; opt in with `--trainer`.
    pub trainer_disabled: bool,
    pub initial_orientation: DeviceOrientation,
    /// iOS version reported to guest applications. `None` uses the latest compatibility version.
    pub ios_version: Option<(i32, i32, i32)>,
    /// On iOS, present an OpenGL ES 2.0 renderbuffer directly even when its
    /// CAEAGLayer is not the fullscreen layer, instead of going through the
    /// Core Animation composition path.
    ///
    /// This is what makes apps like The Sims Medieval display at all, but it
    /// costs a `glFinish` plus a full renderbuffer readback and re-upload every
    /// frame on the main thread, which starves UIKit of the run-loop time it
    /// needs to dispatch touches. Off by default so apps that worked without it
    /// keep working; enable per app in the options file.
    pub ios_es2_direct_present: bool,
    pub scale_hack: NonZeroU32,
    /// `--ui-scale=N`: resolution multiplier for UIKit/Core Animation UI
    /// (app picker, in-game UIKit HUDs). Layer bitmaps and the compositor
    /// framebuffer are rendered at N times their point size.
    pub ui_scale: NonZeroU32,
    pub deadzone: f32,
    pub analog_stick_tilt_controls: bool,
    pub x_tilt_range: f32,
    pub y_tilt_range: f32,
    pub x_tilt_offset: f32,
    pub y_tilt_offset: f32,
    pub button_to_touch: HashMap<Button, (f32, f32)>,
    pub dpad_to_touch: Option<(f32, f32, f32, f32)>,
    pub stick_to_touch: Option<(f32, f32, f32, f32)>,
    pub stabilize_virtual_cursor: Option<(f32, f32)>,
    pub gles1_implementation: Option<GLESImplementation>,
    /// Allow selected early OpenGL ES 2.0 apps to use the GLES2 subset exposed
    /// through touchHLE's desktop OpenGL 2.1 compatibility backend.
    pub gles2_compat: bool,
    pub direct_memory_access: bool,
    /// CPU affinity policy for the emulator thread on Android
    /// (`--affinity=`): `None` = default (big cores), or one of
    /// `all` / `off` / `big` / an explicit CPU list like `4-7`.
    pub affinity: Option<String>,
    pub gdb_listen_addrs: Option<Vec<SocketAddr>>,
    pub preferred_languages: Option<Vec<String>>,
    pub headless: bool,
    pub print_fps: bool,
    pub performance_overlay_enabled: bool,
    pub performance_overlay_mode: PerformanceOverlayMode,
    pub performance_overlay_corner: PerformanceOverlayCorner,
    pub fps_limit: Option<f64>,
    /// `--memory-limit=...`: cap on the emulated app's heap (its `malloc`
    /// pool), in MiB. `None` is the default and means "let the Device model
    /// decide": half of its RAM, like iOS's jetsam limit (see
    /// `crate::environment::default_guest_heap_cap`). See
    /// `crate::mem::Mem::set_memory_limit`.
    pub memory_limit_mb: Option<u32>,
    pub force_composition: bool,
    /// See [PresentMode]. Can also be set with the `TOUCHHLE_PRESENT_MODE`
    /// environment variable (the option takes precedence).
    pub present_mode: PresentMode,
    /// Issue a `glFinish()` before the presented renderbuffer is copied to
    /// the window. Only needed for drivers that don't order the copy after
    /// the app's draws correctly; costs a GPU pipeline stall per frame.
    /// Can also be enabled with `TOUCHHLE_PRESENT_FINISH=1`.
    pub present_finish: bool,
    /// See [VsyncMode]. Can also be set with the `TOUCHHLE_VSYNC` environment
    /// variable (the option takes precedence).
    pub vsync: VsyncMode,
    /// Android: give the emulator thread a higher scheduling priority and
    /// report its per-frame CPU time to the OS performance hint manager
    /// (ADPF), so the CPU governor keeps the core clocked for the emulated
    /// workload instead of reacting to the idle time between frames. Can be
    /// disabled with `--no-perf-hints` or `TOUCHHLE_PERF_HINTS=0`.
    pub perf_hints: bool,
    /// Force EAGL `initWithAPI:` to create an OpenGL ES 2.0 context even when
    /// the app requested an OpenGL ES 1.1 context.
    ///
    /// This unblocks apps that ask EAGL for an ES 1.1 context but actually
    /// drive rendering with shader entry points (`glUseProgram`,
    /// `glCreateShader`, …). Without this flag those calls fall through to
    /// the GLES 1.1-only backend on Android, get silently stubbed, and the
    /// resulting frames are empty (black screen). Enable it per app via the
    /// per-app default options file or with `--prefer-gles2-context` on the
    /// command line. Apps that legitimately rely on the ES 1.1 fixed-function
    /// pipeline should NOT enable this flag.
    pub prefer_gles2_context: bool,
    /// Break an otherwise-infinite `semaphore_wait()` by returning
    /// KERN_SUCCESS spuriously after this many milliseconds.
    ///
    /// Off by default, because a spurious wake is a lie: the waiter proceeds
    /// as if it had acquired the semaphore while the resource it was waiting
    /// for is not ready. Engines that drive their own synchronisation through
    /// Mach semaphores (Mono/Unity worker threads, in particular) then read
    /// half-initialized state, which surfaces much later as a null
    /// dereference in guest code rather than as a hang.
    ///
    /// It exists for FMOD's worker threads in Minecraft: Story Mode
    /// (com.telltalegames.MC100), which deadlock under our cooperative
    /// scheduler and are written to tolerate a spurious wake. Enable it only
    /// per-app, via the default options file.
    pub mach_semaphore_spurious_wake_ms: Option<u64>,
    /// Override the EAGL present rotation with a fixed clockwise angle in
    /// degrees (0, 90, 180 or 270), replacing the orientation-derived rotation
    /// and the iPad autorotation compensation. Some apps (e.g. the universal
    /// iPhone+iPad binary of Minecraft: Story Mode, com.telltalegames.MC100)
    /// draw their frame already upright for the display, so the heuristic
    /// rotation turns the picture sideways; `--present-rotate=0` presents it
    /// as-is. `None` keeps the automatic behaviour.
    pub present_rotate: Option<u32>,
    /// Use UIKit's actual autorotation transform when deciding whether to add
    /// EAGL's 180-degree presentation compensation. Off by default to retain
    /// the legacy iPad/non-portrait heuristic; opt in with `--rotation-fix`.
    pub rotation_fix: bool,
    /// Force EAGL `initWithAPI:` to create an OpenGL ES 1.1 (fixed-function)
    /// context even when the app requested an OpenGL ES 2.0/3.x context.
    ///
    /// The inverse of `--prefer-gles2-context`. Games built on engines that
    /// support both backends (cocos2d-x 2.x: Geometry Dash, etc.) pick ES 2.0
    /// whenever context creation succeeds, even though they also ship a fully
    /// working ES 1.1 fixed-function path. On hosts where the ES 2.0 path
    /// misrenders, downgrading the context to ES 1.1 makes such engines take
    /// their ES 1.1 code path instead. Enable with `--force-gles1-context`
    /// (per-app via the options file) or `TOUCHHLE_FORCE_GLES1_CONTEXT=1`.
    /// Apps that are ES 2.0-only will fail to create a context with this set.
    pub force_gles1_context: bool,
    pub network_access: bool,
    /// Report a local Game Center player as signed in, without providing
    /// online Game Center services. Some games require this at startup.
    pub game_center_authenticated: bool,
    pub popup_errors: bool,
    pub dumping_options: DumpingOptions,
    pub dumping_file: PathBuf,
    pub ignore_gl_errors: bool,
    /// Wrap every guest GL entry point with a `glGetError()` check after the
    /// call and log the source location (in `gles_guest.rs`) of any non-zero
    /// error. Useful when an app silently misrenders (e.g. a black screen
    /// despite an alive render loop) because earlier calls are emitting
    /// `GL_INVALID_ENUM` / `GL_INVALID_VALUE` etc. that the app never polls
    /// for.
    ///
    /// Note: enabling this changes guest-visible state because the host
    /// `glGetError()` clears the error queue, so guest `glGetError()` calls
    /// will see 0 instead of the real error. Diagnostic only.
    pub trace_gl_errors: bool,
    /// Log every GLES call made by the guest (via the LoggingGLES wrapper).
    /// Much noisier than `trace_gl_errors`. Diagnostic only.
    pub verbose_gles: bool,
    /// Prefer the vendor's native OpenGL ES driver over the bundled ANGLE
    /// libraries on Android. Defaults to `false` ("GLES Native" quick option
    /// OFF): the bundled ANGLE backend is the predictable default, since
    /// vendor drivers differ a lot between devices. Enable (`--gles-native`,
    /// quick option ON) to use the vendor driver instead: it behaves closest
    /// to real iPhone-era hardware (lenient GLSL ES linking etc.) and avoids
    /// ANGLE's stricter validation, which breaks some apps' shaders (e.g.
    /// Gangstar's fragment-only varyings). Only meaningful on Android;
    /// ignored elsewhere.
    pub gles_native: bool,
    /// After a `glTexImage2D(level=0, …)` upload, if the bound texture's
    /// `GL_TEXTURE_MIN_FILTER` is still the ES 1.1 default
    /// `GL_NEAREST_MIPMAP_LINEAR` (which makes the texture incomplete
    /// because no mipmaps have been uploaded), force it to
    /// `GL_LINEAR`. This mirrors the behaviour of lenient drivers like
    /// Mesa and Apple's PowerVR ES 1.1 driver — strict drivers like
    /// Qualcomm Adreno's native ES 1.1 driver instead sample
    /// incomplete textures as opaque black, which produces a black
    /// screen for games that never bother to set
    /// `GL_TEXTURE_MIN_FILTER` themselves. The fix-up only fires for
    /// `level == 0` uploads that find the default mipmap filter still
    /// active; once the guest sets any non-default filter (mipmap or not)
    /// we leave it alone, and any subsequent `glTexParameteri(GL_TEXTURE_MIN_FILTER, …)`
    /// from the guest will override our `GL_LINEAR` write. Multi-level uploads
    /// (`level > 0`) do not trigger the fix-up so games that actually use
    /// mipmaps are unaffected.
    pub fix_texture_min_filter: bool,
    pub zero_stack_after_guest_to_host_call: Option<u32>,
    /// device (FMOD streaming bypass). Opt in with `--fix-music` or
    /// `TOUCHHLE_GD_MUSIC_BYPASS=1`; off by default so the bypass only
    /// affects Geometry Dash sessions where the user asked for it.
    // TODO: flip to opt-out once track-switch/pause/reset handling is
    // validated against a real run of the game.
    pub gd_music_bypass: bool,
    /// Enable the newer `MPMoviePlayerController` lifecycle and view
    /// presentation behaviour with `--fix-movie-player`; the bundled defaults
    /// enable it for GTA III. Off globally because some other games rely on
    /// the legacy eager-load and automatic fullscreen presentation behaviour.
    pub fix_movie_player: bool,
    /// When an OpenGL ES 2.0 app binds no attribute locations itself, give a
    /// linked program's active attributes the locations 0 to N-1 in the
    /// reverse of the order the host driver chose. Turn it on with
    /// `--reverse-attrib-locations`. The bundled defaults turn it on for
    /// Noblemen: 1896. That game sizes a per-program array by
    /// `GL_ACTIVE_ATTRIBUTES` and indexes it by location, so locations must
    /// be 0 to N-1. Its deferred light pass also sets a texture-coordinate
    /// array at location 0 for a program that has no such attribute, which
    /// only works if `position` is not at location 0. The spec leaves
    /// unbound locations to the implementation:
    /// <https://registry.khronos.org/OpenGL-Refpages/es2.0/xhtml/glBindAttribLocation.xml>
    pub reverse_attrib_locations: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            fullscreen: false,
            preserve_aspect_ratio: false,
            trainer_disabled: true,
            device_family: None,
            auto_device_family: false,
            host_screen_size: None,
            initial_orientation: DeviceOrientation::Portrait,
            ios_version: None,
            ios_es2_direct_present: false,
            scale_hack: NonZeroU32::new(1).unwrap(),
            ui_scale: NonZeroU32::new(2).unwrap(),
            analog_stick_tilt_controls: true,
            deadzone: 0.1,
            x_tilt_range: 60.0,
            y_tilt_range: 60.0,
            x_tilt_offset: 0.0,
            y_tilt_offset: 0.0,
            button_to_touch: HashMap::new(),
            dpad_to_touch: None,
            stick_to_touch: None,
            stabilize_virtual_cursor: None,
            gles1_implementation: None,
            gles2_compat: false,
            direct_memory_access: true,
            affinity: None,
            gdb_listen_addrs: None,
            preferred_languages: None,
            headless: false,
            print_fps: false,
            performance_overlay_enabled: true,
            performance_overlay_mode: if cfg!(any(target_os = "android", target_os = "ios")) {
                PerformanceOverlayMode::Minimal
            } else {
                PerformanceOverlayMode::TitleBar
            },
            performance_overlay_corner: PerformanceOverlayCorner::TopRight,
            fps_limit: Some(60.0),
            memory_limit_mb: None,
            force_composition: false,
            present_mode: std::env::var("TOUCHHLE_PRESENT_MODE")
                .ok()
                .and_then(|value| PresentMode::from_short_name(value.trim()).ok())
                .unwrap_or(PresentMode::Auto),
            present_finish: std::env::var_os("TOUCHHLE_PRESENT_FINISH")
                .map(|value| value != "0")
                .unwrap_or(false),
            vsync: std::env::var("TOUCHHLE_VSYNC")
                .ok()
                .and_then(|value| VsyncMode::from_short_name(value.trim()).ok())
                .unwrap_or(VsyncMode::Auto),
            perf_hints: std::env::var_os("TOUCHHLE_PERF_HINTS")
                .map(|value| value != "0")
                .unwrap_or(true),
            prefer_gles2_context: false,
            mach_semaphore_spurious_wake_ms: None,
            present_rotate: None,
            rotation_fix: false,
            force_gles1_context: std::env::var("TOUCHHLE_FORCE_GLES1_CONTEXT")
                .map(|value| {
                    let value = value.trim();
                    value != "0" && !value.is_empty()
                })
                .unwrap_or(false),
            network_access: false,
            game_center_authenticated: false,
            popup_errors: true,
            dumping_options: Default::default(),
            dumping_file: crate::paths::user_data_base_path().join("DUMP.txt"),
            ignore_gl_errors: false,
            trace_gl_errors: false,
            verbose_gles: false,
            gles_native: false,
            // On Android the host GLES driver is essentially always
            // ARM Mali / Qualcomm Adreno / something equally strict,
            // and apps shipped for iOS overwhelmingly upload PVRTC and
            // RGBA textures at level 0 only without ever setting a
            // non-mipmap `GL_TEXTURE_MIN_FILTER`. Real iOS PowerVR
            // drivers were lenient about this; strict Android drivers
            // sample such an "incomplete" texture as opaque black or
            // white, which makes textured geometry render as flat
            // black/white shapes (Temple Run on Mali-G57 is a textbook
            // case). Default the fix-up to ON so games work out of the
            // box on Android — users can still disable it via
            // `--fix-texture-min-filter=false` in
            // `touchHLE_options.txt` if they hit a game that genuinely
            // depends on mipmap minification (rare among iOS 2.x/3.x
            // titles). On desktop hosts (where the user is likely
            // running Mesa / Apple PowerVR / NVIDIA / AMD, all
            // historically lenient) we leave it off so we don't change
            // pixel output for the common case.
            fix_texture_min_filter: cfg!(target_os = "android"),
            zero_stack_after_guest_to_host_call: None,
            gd_music_bypass: std::env::var_os("TOUCHHLE_GD_MUSIC_BYPASS")
                .map(|value| value != "0")
                .unwrap_or(false),
            fix_movie_player: false,
            reverse_attrib_locations: false,
        }
    }
}

impl Options {
    /// Parse the command-line argument syntax for an option. Returns `Ok(true)`
    /// if the option was valid and has been applied, or `Ok(false)` if the
    /// option was not recognized.
    pub fn parse_argument(&mut self, arg: &str) -> Result<bool, String> {
        fn parse_degrees(arg: &str, name: &str) -> Result<f32, String> {
            let arg: f32 = arg
                .parse()
                .map_err(|_| format!("Value for {name} is invalid"))?;
            if !arg.is_finite() || !(-360.0..=360.0).contains(&arg) {
                return Err(format!("Value for {name} is out of range"));
            }
            Ok(arg)
        }

        if arg == "--fullscreen" {
            self.fullscreen = true;
        } else if arg == "--no-fullscreen" {
            self.fullscreen = false;
        } else if arg == "--preserve-aspect-ratio" {
            self.preserve_aspect_ratio = true;
        } else if arg == "--no-preserve-aspect-ratio" {
            self.preserve_aspect_ratio = false;
        } else if arg == "--fix-music" {
            self.gd_music_bypass = true;
        } else if arg == "--fix-movie-player" {
            self.fix_movie_player = true;
        } else if arg == "--no-fix-movie-player" {
            self.fix_movie_player = false;
        } else if arg == "--reverse-attrib-locations" {
            self.reverse_attrib_locations = true;
        } else if arg == "--no-reverse-attrib-locations" {
            self.reverse_attrib_locations = false;
        } else if arg == "--landscape-left" {
            self.initial_orientation = DeviceOrientation::LandscapeLeft;
        } else if arg == "--landscape-right" {
            self.initial_orientation = DeviceOrientation::LandscapeRight;
        } else if arg == "--upside-down" {
            self.initial_orientation = DeviceOrientation::PortraitUpsideDown;
        } else if arg == "--ios-es2-direct-present" {
            self.ios_es2_direct_present = true;
        } else if arg == "--no-ios-es2-direct-present" {
            self.ios_es2_direct_present = false;
        } else if let Some(value) = arg.strip_prefix("--device-family=") {
            if value == "auto" {
                self.auto_device_family = true;
                self.device_family = None;
            } else {
                let parsed = DeviceFamily::try_from(value)
                    .map_err(|_| "Invalid device family".to_string())?;
                self.auto_device_family = false;
                self.device_family = Some(parsed);
            }
        } else if let Some(value) = arg.strip_prefix("--ios-version=") {
            let mut parts = value.split('.');
            let major: i32 = parts
                .next()
                .ok_or_else(|| "--ios-version= requires MAJOR.MINOR[.PATCH]".to_string())?
                .parse()
                .map_err(|_| "Invalid major version for --ios-version=".to_string())?;
            let minor: i32 = parts
                .next()
                .ok_or_else(|| "--ios-version= requires MAJOR.MINOR[.PATCH]".to_string())?
                .parse()
                .map_err(|_| "Invalid minor version for --ios-version=".to_string())?;
            let patch: i32 = parts
                .next()
                .unwrap_or("0")
                .parse()
                .map_err(|_| "Invalid patch version for --ios-version=".to_string())?;
            if parts.next().is_some() || major < 1 || minor < 0 || patch < 0 {
                return Err("Invalid value for --ios-version=".to_string());
            }
            self.ios_version = Some((major, minor, patch));
        } else if let Some(value) = arg.strip_prefix("--screen-size=") {
            let (w, h) = value
                .split_once(|c| c == 'x' || c == 'X' || c == ',')
                .ok_or_else(|| "--screen-size= requires WIDTHxHEIGHT".to_string())?;
            let w: u32 = w
                .trim()
                .parse()
                .map_err(|_| "Invalid width for --screen-size=".to_string())?;
            let h: u32 = h
                .trim()
                .parse()
                .map_err(|_| "Invalid height for --screen-size=".to_string())?;
            if w == 0 || h == 0 {
                return Err("--screen-size= dimensions must be non-zero".to_string());
            }
            self.host_screen_size = Some((w, h));
        } else if let Some(value) = arg.strip_prefix("--scale-hack=") {
            self.scale_hack = value
                .parse()
                .map_err(|_| "Invalid scale hack factor".to_string())?;
        } else if let Some(value) = arg.strip_prefix("--ui-scale=") {
            self.ui_scale = value
                .parse()
                .map_err(|_| "Invalid UI scale factor".to_string())?;
        } else if arg == "--disable-analog-stick-tilt-controls" {
            self.analog_stick_tilt_controls = false;
        } else if arg == "--enable-analog-stick-tilt-controls" {
            self.analog_stick_tilt_controls = true;
        } else if let Some(value) = arg.strip_prefix("--deadzone=") {
            self.deadzone = parse_degrees(value, "deadzone")?;
        } else if let Some(value) = arg.strip_prefix("--x-tilt-range=") {
            self.x_tilt_range = parse_degrees(value, "X tilt range")?;
        } else if let Some(value) = arg.strip_prefix("--y-tilt-range=") {
            self.y_tilt_range = parse_degrees(value, "Y tilt range")?;
        } else if let Some(value) = arg.strip_prefix("--x-tilt-offset=") {
            self.x_tilt_offset = parse_degrees(value, "X tilt offset")?;
        } else if let Some(value) = arg.strip_prefix("--y-tilt-offset=") {
            self.y_tilt_offset = parse_degrees(value, "Y tilt offset")?;
        } else if let Some(values) = arg.strip_prefix("--button-to-touch=") {
            let (button, coords) = values
                .split_once(',')
                .ok_or_else(|| "--button-to-touch= requires three values".to_string())?;
            let (x, y) = coords
                .split_once(',')
                .ok_or_else(|| "--button-to-touch= requires three values".to_string())?;
            let button = match button {
                "DPadLeft" => Ok(Button::DPadLeft),
                "DPadUp" => Ok(Button::DPadUp),
                "DPadRight" => Ok(Button::DPadRight),
                "DPadDown" => Ok(Button::DPadDown),
                "Start" => Ok(Button::Start),
                "A" => Ok(Button::A),
                "B" => Ok(Button::B),
                "X" => Ok(Button::X),
                "Y" => Ok(Button::Y),
                "LeftShoulder" => Ok(Button::LeftShoulder),
                _ => Err("Invalid button for --button-to-touch=".to_string()),
            }?;
            let x: f32 = x
                .parse()
                .map_err(|_| "Invalid X co-ordinate for --button-to-touch=".to_string())?;
            let y: f32 = y
                .parse()
                .map_err(|_| "Invalid Y co-ordinate for --button-to-touch=".to_string())?;
            self.button_to_touch.insert(button, (x, y));
        } else if let Some(values) = arg.strip_prefix("--stick-to-touch=") {
            let nums: [f32; 4] = values
                .split(',')
                .map(|s| s.parse::<f32>())
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| "invalid --stick-to-touch".to_string())?
                .try_into()
                .map_err(|_| "--stick-to-touch= requires four values".to_string())?;

            self.stick_to_touch = Some((nums[0], nums[1], nums[2], nums[3]));
        } else if let Some(values) = arg.strip_prefix("--dpad-to-touch=") {
            let nums: [f32; 4] = values
                .split(',')
                .map(|s| s.parse::<f32>())
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| "invalid --dpad-to-touch".to_string())?
                .try_into()
                .map_err(|_| "--dpad-to-touch= requires four values".to_string())?;

            self.dpad_to_touch = Some((nums[0], nums[1], nums[2], nums[3]));
        } else if let Some(value) = arg.strip_prefix("--stabilize-virtual-cursor=") {
            let (smoothing_strength, sticky_radius) = value
                .split_once(',')
                .ok_or_else(|| "--stabilize-virtual-cursor= requires two values".to_string())?;
            let smoothing_strength: f32 = smoothing_strength
                .parse()
                .ok()
                .and_then(|s| if s < 0.0 { None } else { Some(s) })
                .ok_or_else(|| {
                    "Invalid smoothing strength for --stabilize-virtual-cursor=".to_string()
                })?;
            let sticky_radius: f32 = sticky_radius
                .parse()
                .ok()
                .and_then(|s| if s < 0.0 { None } else { Some(s) })
                .ok_or_else(|| {
                    "Invalid sticky radius for --stabilize-virtual-cursor=".to_string()
                })?;
            self.stabilize_virtual_cursor = Some((smoothing_strength, sticky_radius));
        } else if let Some(value) = arg.strip_prefix("--gles1=") {
            self.gles1_implementation = Some(
                GLESImplementation::from_short_name(value)
                    .map_err(|_| "Unrecognized --gles1= value".to_string())?,
            );
        } else if arg == "--gles2-compat" {
            self.gles2_compat = true;
        } else if let Some(value) = arg.strip_prefix("--affinity=") {
            self.affinity = Some(value.to_string());
        } else if arg == "--disable-direct-memory-access" {
            self.direct_memory_access = false;
        } else if let Some(address) = arg.strip_prefix("--gdb=") {
            let addrs = address
                .to_socket_addrs()
                .map_err(|e| format!("Could not resolve GDB server listen address: {e}"))?
                .collect();
            self.gdb_listen_addrs = Some(addrs);
        } else if let Some(value) = arg.strip_prefix("--preferred-languages=") {
            self.preferred_languages = Some(value.split(',').map(ToOwned::to_owned).collect());
        } else if arg == "--headless" {
            self.headless = true;
            // Can't show the dialog box when headless!
            self.popup_errors = false;
        } else if arg == "--print-fps" {
            self.print_fps = true;
        } else if arg == "--no-print-fps" {
            self.print_fps = false;
        } else if let Some(value) = arg.strip_prefix("--overlay=") {
            self.performance_overlay_enabled = match value {
                "on" => true,
                "off" => false,
                _ => return Err("Invalid value for --overlay=".to_string()),
            };
        } else if let Some(value) = arg.strip_prefix("--overlay-mode=") {
            self.performance_overlay_mode = PerformanceOverlayMode::from_short_name(value)
                .ok_or_else(|| "Invalid value for --overlay-mode=".to_string())?;
        } else if let Some(value) = arg.strip_prefix("--overlay-corner=") {
            self.performance_overlay_corner = PerformanceOverlayCorner::from_short_name(value)
                .ok_or_else(|| "Invalid value for --overlay-corner=".to_string())?;
        } else if let Some(value) = arg.strip_prefix("--fps-limit=") {
            if value == "off" {
                self.fps_limit = None;
            } else {
                let limit: f64 = value
                    .parse()
                    .ok()
                    .and_then(|v| if v <= 0.0 { None } else { Some(v) })
                    .ok_or_else(|| "Invalid value for --fps-limit=".to_string())?;
                self.fps_limit = Some(limit);
            }
        } else if let Some(value) = arg.strip_prefix("--memory-limit=") {
            self.memory_limit_mb = parse_memory_limit_mb(value)?;
        } else if arg == "--force-composition" {
            self.force_composition = true;
        } else if arg == "--no-force-composition" {
            self.force_composition = false;
        } else if let Some(value) = arg.strip_prefix("--present-mode=") {
            self.present_mode = PresentMode::from_short_name(value).map_err(|_| {
                "Invalid value for --present-mode= (expected auto, direct or readback)"
                    .to_string()
            })?;
        } else if arg == "--present-finish" {
            self.present_finish = true;
        } else if arg == "--no-present-finish" {
            self.present_finish = false;
        } else if let Some(value) = arg.strip_prefix("--vsync=") {
            self.vsync = VsyncMode::from_short_name(value).map_err(|_| {
                "Invalid value for --vsync= (expected auto, on or off)".to_string()
            })?;
        } else if arg == "--vsync" {
            self.vsync = VsyncMode::On;
        } else if arg == "--no-vsync" {
            self.vsync = VsyncMode::Off;
        } else if arg == "--landscape-uiscreen-bounds" {
            // Report UIScreen.bounds (and therefore the EAGL layer / renderbuffer
            // size) as landscape instead of the iOS-accurate portrait-native
            // bounds. Apps that lay their render target out directly from
            // UIScreen.bounds without applying the interface-orientation
            // rotation (e.g. Minecraft: Story Mode) otherwise render a portrait
            // frame that gets stretched into the landscape window. The GLES
            // layer reads this via the env var, so set it here.
            std::env::set_var("TOUCHHLE_LANDSCAPE_UISCREEN_BOUNDS", "1");
        } else if let Some(value) = arg.strip_prefix("--present-rotate=") {
            let deg: u32 = value
                .parse()
                .map_err(|_| "Invalid value for --present-rotate= (expected 0, 90, 180 or 270)".to_string())?;
            if !matches!(deg, 0 | 90 | 180 | 270) {
                return Err("Invalid value for --present-rotate= (expected 0, 90, 180 or 270)".to_string());
            }
            self.present_rotate = Some(deg);
        } else if arg == "--rotation-fix" {
            self.rotation_fix = true;
        } else if arg == "--no-rotation-fix" {
            self.rotation_fix = false;
        } else if arg == "--perf-hints" {
            self.perf_hints = true;
        } else if arg == "--no-perf-hints" {
            self.perf_hints = false;
        } else if let Some(value) = arg.strip_prefix("--mach-semaphore-spurious-wake=") {
            let ms: u64 = value.parse().map_err(|_| {
                "Invalid value for --mach-semaphore-spurious-wake= (expected \
                 milliseconds, or 0 to disable)"
                    .to_string()
            })?;
            self.mach_semaphore_spurious_wake_ms = (ms != 0).then_some(ms);
        } else if arg == "--prefer-gles2-context" {
            self.prefer_gles2_context = true;
        } else if arg == "--force-gles1-context" {
            self.force_gles1_context = true;
            // GLES-native backend selection and EAGL both read this env var
            // (the GLES backend layer has no `Options` access).
            std::env::set_var("TOUCHHLE_FORCE_GLES1_CONTEXT", "1");
        } else if arg == "--allow-network-access" {
            self.network_access = true;
        } else if arg == "--no-network-access" {
            self.network_access = false;
        } else if arg == "--game-center-authenticated" {
            self.game_center_authenticated = true;
        } else if arg == "--no-game-center-authenticated" {
            self.game_center_authenticated = false;
        } else if arg == "--no-error-popup" {
            self.popup_errors = false;
        } else if let Some(values) = arg.strip_prefix("--dump=") {
            self.dumping_options = parse_dump_options(values)?;
        } else if let Some(path) = arg.strip_prefix("--dump-file=") {
            self.dumping_file = crate::paths::user_data_base_path().join(path);
        } else if arg == "--ignore-gl-errors" {
            self.ignore_gl_errors = true;
        } else if arg == "--trace-gl-errors" {
            self.trace_gl_errors = true;
        } else if arg == "--no-trace-gl-errors" {
            self.trace_gl_errors = false;
        } else if arg == "--verbose-gles" {
            self.verbose_gles = true;
        } else if arg == "--gles-native" {
            self.gles_native = true;
        } else if arg == "--no-gles-native" {
            self.gles_native = false;
        } else if arg == "--fix-texture-min-filter" {
            self.fix_texture_min_filter = true;
            // GLES1Native reads this as its source of truth (it has no
            // `Options` access from inside the GL call path).
            std::env::set_var("TOUCHHLE_FIX_TEXTURE_MIN_FILTER", "1");
        } else if arg == "--no-fix-texture-min-filter" {
            // Off-switch for the Android default. Useful when an iOS
            // title actually relies on mipmap minification and the
            // forced `GL_LINEAR` would visibly degrade quality.
            self.fix_texture_min_filter = false;
            std::env::set_var("TOUCHHLE_FIX_TEXTURE_MIN_FILTER", "0");
        } else if let Some(value) = arg.strip_prefix("--zero-stack-after-guest-to-host-call=") {
            self.zero_stack_after_guest_to_host_call = Some(value.parse().map_err(|_| {
                "Invalid value for --zero-stack-after-guest-to-host-call=".to_string()
            })?);
        } else if arg == "--no-trainer" {
            self.trainer_disabled = true;
        } else if arg == "--trainer" {
            self.trainer_disabled = false;
        } else {
            return Ok(false);
        };
        Ok(true)
    }
}

/// Try to get app-specific options from a file.
///
/// Returns [Ok] if there is no error when reading the file, otherwise [Err].
/// The [Ok] value is a [Some] with the options if they could be found, or
/// [None] if no options were found for this app.
pub fn get_options_from_file<F: Read>(file: F, app_id: &str) -> Result<Option<String>, String> {
    let file = BufReader::new(file);
    for (line_no, line) in BufRead::lines(file).enumerate() {
        // Line numbering usually starts from 1
        let line_no = line_no + 1;

        let line = line.map_err(|e| format!("Error while reading line {line_no}: {e}"))?;

        // # for single-line comments
        let line = if let Some((rest, _)) = line.split_once('#') {
            rest
        } else {
            &line
        };

        // Empty/all-comment lines ignored
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let (line_app_id, line_options) = line.split_once(':').ok_or_else(|| format!("Line {line_no} is not a comment and is missing a colon (:) to separate the app ID from the options"))?;
        let line_app_id = line_app_id.trim();

        if line_app_id != app_id {
            continue;
        }

        let line_options = line_options.trim();
        if line_options.is_empty() {
            return Ok(None);
        } else {
            return Ok(Some(line_options.to_string()));
        }
    }
    Ok(None)
}

/// Parses the value of `--memory-limit=` into a heap cap in MiB.
///
/// Accepts a positive whole number of mebibytes, or `off`/`default` for the
/// ceiling derived from the Device model. Zero is rejected: it would make the
/// very first guest allocation fail, so the emulator could never start an app.
fn parse_memory_limit_mb(value: &str) -> Result<Option<u32>, String> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("off") || value.eq_ignore_ascii_case("default") {
        return Ok(None);
    }
    let mb: u32 = value.parse().map_err(|_| {
        "Invalid value for --memory-limit= (expected a whole number of MiB, or off)".to_string()
    })?;
    if mb == 0 {
        return Err(
            "Invalid value for --memory-limit= (expected a positive number of MiB, or off)"
                .to_string(),
        );
    }
    Ok(Some(mb))
}

#[derive(Default, Clone)]
pub struct DumpingOptions {
    pub linking_info: bool,
    pub symbols: bool,
}

impl DumpingOptions {
    /// Check if any of the dumping options are active.
    pub fn any(&self) -> bool {
        self.linking_info || self.symbols
    }
}

fn parse_dump_options(options: &str) -> Result<DumpingOptions, String> {
    let mut dumping_options = DumpingOptions::default();
    for opt in options.split(",") {
        if opt == "linking-info" {
            // Dumps linked symbols, classes and selectors for the given app
            dumping_options.linking_info = true;
        } else if opt == "symbols" {
            // Dumps touchHLE provided symbols and exits
            dumping_options.symbols = true;
        } else {
            return Err(format!("Unrecognized option {opt} for --dump=..."));
        }
    }
    Ok(dumping_options)
}

#[cfg(test)]
mod option_parsing_tests {
    use super::*;

    #[test]
    fn force_composition_can_be_enabled_and_disabled() {
        let mut options = Options::default();
        assert!(!options.force_composition);

        assert!(options.parse_argument("--force-composition").unwrap());
        assert!(options.force_composition);

        assert!(options.parse_argument("--no-force-composition").unwrap());
        assert!(!options.force_composition);
    }

    #[test]
    fn fix_movie_player_is_opt_in_and_can_be_disabled() {
        let mut options = Options::default();
        assert!(!options.fix_movie_player);

        assert!(options.parse_argument("--fix-movie-player").unwrap());
        assert!(options.fix_movie_player);

        assert!(options.parse_argument("--no-fix-movie-player").unwrap());
        assert!(!options.fix_movie_player);
    }

    #[test]
    fn reverse_attrib_locations_is_opt_in_and_can_be_disabled() {
        let mut options = Options::default();
        assert!(!options.reverse_attrib_locations);

        assert!(options
            .parse_argument("--reverse-attrib-locations")
            .unwrap());
        assert!(options.reverse_attrib_locations);

        assert!(options
            .parse_argument("--no-reverse-attrib-locations")
            .unwrap());
        assert!(!options.reverse_attrib_locations);
    }

    #[test]
    fn rotation_fix_is_opt_in_and_can_be_disabled() {
        let mut options = Options::default();
        assert!(!options.rotation_fix);

        assert!(options.parse_argument("--rotation-fix").unwrap());
        assert!(options.rotation_fix);

        assert!(options.parse_argument("--no-rotation-fix").unwrap());
        assert!(!options.rotation_fix);
    }

    #[test]
    fn local_game_center_sign_in_is_opt_in_and_can_be_disabled() {
        let mut options = Options::default();
        assert!(!options.game_center_authenticated);
        assert!(options.parse_argument("--game-center-authenticated").unwrap());
        assert!(options.game_center_authenticated);
        assert!(options.parse_argument("--no-game-center-authenticated").unwrap());
        assert!(!options.game_center_authenticated);
    }

    #[test]
    fn title_bar_mode_falls_back_when_unavailable() {
        assert_eq!(
            PerformanceOverlayMode::TitleBar.for_platform(false),
            PerformanceOverlayMode::Minimal
        );
        assert_eq!(
            PerformanceOverlayMode::TitleBar.for_platform(true),
            PerformanceOverlayMode::TitleBar
        );
        assert_eq!(
            PerformanceOverlayMode::Full.for_platform(false),
            PerformanceOverlayMode::Full
        );
    }

    #[test]
    fn performance_overlay_options_parse_and_validate() {
        let mut options = Options::default();
        assert!(options.performance_overlay_enabled);
        let default_mode = if cfg!(any(target_os = "android", target_os = "ios")) {
            PerformanceOverlayMode::Minimal
        } else {
            PerformanceOverlayMode::TitleBar
        };
        assert_eq!(options.performance_overlay_mode, default_mode);
        assert_eq!(options.performance_overlay_corner, PerformanceOverlayCorner::TopRight);

        assert!(options.parse_argument("--overlay=off").unwrap());
        assert!(!options.performance_overlay_enabled);
        assert!(options.parse_argument("--overlay=on").unwrap());
        assert!(options.performance_overlay_enabled);
        assert!(options.parse_argument("--overlay-mode=full").unwrap());
        assert_eq!(options.performance_overlay_mode, PerformanceOverlayMode::Full);
        assert!(options.parse_argument("--overlay-corner=bottomleft").unwrap());
        assert_eq!(options.performance_overlay_corner, PerformanceOverlayCorner::BottomLeft);
        assert!(options.parse_argument("--overlay-mode=invalid").is_err());
        assert!(options.parse_argument("--overlay-corner=invalid").is_err());
    }

    #[test]
    fn memory_limit_parses_mib_and_off() {
        let mut options = Options::default();
        assert_eq!(options.memory_limit_mb, None);

        assert!(options.parse_argument("--memory-limit=128").unwrap());
        assert_eq!(options.memory_limit_mb, Some(128));

        assert!(options.parse_argument("--memory-limit=off").unwrap());
        assert_eq!(options.memory_limit_mb, None);

        assert!(options.parse_argument("--memory-limit=0").is_err());
        assert!(options.parse_argument("--memory-limit=lots").is_err());
        // A rejected value must leave the previous setting alone.
        assert!(options.parse_argument("--memory-limit=64").unwrap());
        assert!(options.parse_argument("--memory-limit=-1").is_err());
        assert_eq!(options.memory_limit_mb, Some(64));
    }

    #[test]
    fn xml_dom_mirror_is_not_a_user_option() {
        let mut options = Options::default();
        assert_eq!(options.parse_argument("--xml-dom-mirror"), Ok(false));
    }
}
