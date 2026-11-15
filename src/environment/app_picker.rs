//! App picker GUI.

mod access_prompt;
mod archive_store;

use crate::bundle::Bundle;
use crate::frameworks::core_graphics::cg_affine_transform::{
    CGAffineTransform, CGAffineTransformIdentity,
};
use crate::frameworks::core_graphics::cg_bitmap_context::{
    CGBitmapContextCreate, CGBitmapContextCreateImage,
};
use crate::frameworks::core_graphics::cg_color_space::CGColorSpaceCreateDeviceRGB;
use crate::frameworks::core_graphics::cg_context::{
    CGContextFillRect, CGContextRelease, CGContextScaleCTM, CGContextSetRGBFillColor,
    CGContextTranslateCTM,
};
use crate::frameworks::core_graphics::cg_image::{self, kCGImageAlphaPremultipliedLast};
use crate::frameworks::core_graphics::{CGFloat, CGPoint, CGRect, CGSize};
use crate::frameworks::foundation::ns_run_loop::run_run_loop_single_iteration;
use crate::frameworks::foundation::ns_string;
use crate::frameworks::foundation::NSInteger;
use crate::frameworks::uikit::ui_font::{
    UITextAlignmentCenter, UITextAlignmentLeft, UITextAlignmentRight,
};
use crate::frameworks::uikit::ui_gesture_recognizer::{
    UISwipeGestureRecognizerDirectionLeft, UISwipeGestureRecognizerDirectionRight,
};
use crate::frameworks::uikit::ui_graphics::{UIGraphicsPopContext, UIGraphicsPushContext};
use crate::frameworks::uikit::ui_view::ui_control::ui_button::UIButtonTypeCustom;
use crate::frameworks::uikit::ui_view::ui_control::{
    UIControlEventTouchCancel, UIControlEventTouchDown, UIControlEventTouchDragExit,
    UIControlEventTouchDragInside, UIControlEventTouchDragOutside, UIControlEventTouchUpInside,
    UIControlEventTouchUpOutside, UIControlEventValueChanged, UIControlStateNormal,
};
use crate::fs::BundleData;
use crate::image::Image;
use crate::mem::Ptr;
use crate::objc::{id, msg, msg_class, nil, objc_classes, release, ClassExports, HostObject};
use crate::options::{Options, PerformanceOverlayCorner, PerformanceOverlayMode};
use crate::paths;
use crate::window::DeviceOrientation;
use crate::Environment;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, AtomicI8, Ordering};
use std::time::{Duration, Instant};

/// Pending left (-1) or right (+1) arrow key press, 0 if none. Written by the
/// window event handler and consumed by the picker loop.
static PENDING_ARROW_KEY: AtomicI8 = AtomicI8::new(0);
static PENDING_MOUSE_WHEEL: AtomicI32 = AtomicI32::new(0);

/// Records an arrow key press for the app picker to act on.
pub fn post_arrow_key(dir: i8) {
    PENDING_ARROW_KEY.store(dir, Ordering::Relaxed);
}

pub fn post_mouse_wheel(delta: i32) {
    PENDING_MOUSE_WHEEL.fetch_add(delta, Ordering::Relaxed);
}

struct AppInfo {
    path: PathBuf,
    display_name: String,
    /// `CFBundleIdentifier`, which also names the app's sandbox directory.
    bundle_id: String,
    icon: Option<Image>,
    /// `NSString*`
    display_name_ns_string: Option<id>,
    /// `UIImage*`
    icon_ui_image: Option<id>,
}

const LAST_APP_FILE: &str = "touchHLE_last_app.txt";

fn last_app_record_path() -> PathBuf {
    paths::user_data_base_path().join(LAST_APP_FILE)
}

fn read_last_app_path() -> Option<PathBuf> {
    let value = std::fs::read_to_string(last_app_record_path()).ok()?;
    Some(PathBuf::from(value.trim()))
}

fn remember_last_app_path(path: &Path) {
    if let Err(error) = std::fs::write(last_app_record_path(), path.to_string_lossy().as_bytes()) {
        log!("Warning: couldn't save last launched app path: {error}");
    }
}

pub fn app_picker(options: Options, show_lock_screen: bool) -> Result<(PathBuf, Vec<String>), String> {
    let apps_dir = paths::user_data_base_path().join(paths::APPS_DIR);

    let apps: Result<Vec<AppInfo>, String> = if !apps_dir.is_dir() {
        Err(format!("The {} directory couldn't be found. Check you're running touchHLE from the right directory.", apps_dir.display()))
    } else {
        enumerate_apps(&apps_dir).map_err(|err| {
            format!(
                "Couldn't get list of apps in the {} directory: {}.",
                apps_dir.display(),
                err
            )
        })
    };

    show_app_picker_gui(options, apps, show_lock_screen)
}

fn enumerate_apps(apps_dir: &Path) -> Result<Vec<AppInfo>, std::io::Error> {
    let mut apps = Vec::new();
    let mut directories = vec![apps_dir.to_path_buf()];
    while let Some(directory) = directories.pop() {
        for entry in std::fs::read_dir(directory)? {
            let app_path = entry?.path();
            let extension = app_path.extension();
            if extension == Some(OsStr::new("app")) || extension == Some(OsStr::new("ipa")) {
                let (bundle, fs) = match BundleData::open_any(&app_path).and_then(|bundle_data| {
                    Bundle::new_bundle_and_fs_from_host_path(
                        bundle_data,
                        /* read_only_mode: */ true,
                    )
                }) {
                    Ok(ok) => ok,
                    Err(e) => {
                        log!(
                            "Warning: couldn't open app bundle {}: {} (skipping)",
                            app_path.display(),
                            e
                        );
                        continue;
                    }
                };

                let display_name = bundle.display_name().to_owned();
                let bundle_id = bundle.bundle_identifier().to_owned();
                let icon = match bundle.load_icon(&fs) {
                    Ok(icon) => Some(icon),
                    Err(e) => {
                        log!("Warning: couldn't load icon for app bundle {}: {} (displaying placeholder instead)", app_path.display(), e);
                        None
                    }
                };

                apps.push(AppInfo {
                    path: app_path,
                    display_name,
                    bundle_id,
                    icon,
                    display_name_ns_string: None,
                    icon_ui_image: None,
                });
            } else if app_path.is_dir() {
                directories.push(app_path);
            }
        }
    }

    apps.sort_by_key(|app| app.display_name.to_uppercase());
    Ok(apps)
}

fn sqlite_file_belongs_to_bundle(
    file_name: &str,
    bundle_id: &str,
    other_bundle_ids: &[&str],
) -> bool {
    let target_prefix = format!("{}_", sqlite_namespace(bundle_id));
    if !file_name.starts_with(target_prefix.as_str()) {
        return false;
    }
    !other_bundle_ids.iter().any(|other_id| {
        if *other_id == bundle_id {
            return false;
        }
        let other_prefix = format!("{}_", sqlite_namespace(other_id));
        other_prefix.len() > target_prefix.len() && file_name.starts_with(other_prefix.as_str())
    })
}

/// Deletes an app completely, like iOS does when an app and its data are
/// removed: the `.app` folder or `.ipa` file in the apps directory, the
/// sandbox folder (`Documents`, `Library`, ...) and the SQLite databases.
fn delete_app(app: &AppInfo, all_apps: &[AppInfo]) -> Result<(), String> {
    // The app goes first. If that fails, its data is kept.
    let removed = if app.path.is_dir() {
        std::fs::remove_dir_all(&app.path)
    } else {
        std::fs::remove_file(&app.path)
    };
    removed.map_err(|e| format!("couldn't delete {}: {}", app.path.display(), e))?;
    echo!("Deleted app: {}", app.path.display());

    // If another installed bundle/IPA shares the exact same bundle identifier,
    // keep the shared sandbox and SQLite databases intact.
    if all_apps
        .iter()
        .any(|other| other.path != app.path && other.bundle_id == app.bundle_id)
    {
        log!(
            "Keeping shared app data for {:?} because another bundle with the same ID remains installed",
            app.bundle_id
        );
        return Ok(());
    }

    // The app is already gone, so failures from here on are only logged.
    if is_plain_name(&app.bundle_id) {
        let sandbox_dir = paths::user_data_base_path()
            .join(paths::SANDBOX_DIR)
            .join(&app.bundle_id);
        if sandbox_dir.exists() {
            match std::fs::remove_dir_all(&sandbox_dir) {
                Ok(()) => echo!("Deleted app data: {}", sandbox_dir.display()),
                Err(e) => log!("Warning: couldn't delete {}: {}", sandbox_dir.display(), e),
            }
        }
    } else {
        log!(
            "Warning: not deleting the data of {:?}, its bundle identifier is not a plain name",
            app.bundle_id
        );
    }

    // touchHLE names each database `{namespace}_{file name}`. Exclude longer
    // sibling namespaces (`com.foo.game_hd_` when deleting `com.foo.game`).
    let other_bundle_ids: Vec<&str> = all_apps
        .iter()
        .filter(|other| other.path != app.path)
        .map(|other| other.bundle_id.as_str())
        .collect();
    let sqlite_dir = paths::user_data_base_path().join(paths::SQLITE_DIR);
    if let Ok(entries) = std::fs::read_dir(&sqlite_dir) {
        for entry in entries.flatten() {
            let file_name = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path();
            if sqlite_file_belongs_to_bundle(&file_name, &app.bundle_id, &other_bundle_ids)
                && path.is_file()
            {
                if let Err(e) = std::fs::remove_file(&path) {
                    log!("Warning: couldn't delete {}: {}", path.display(), e);
                }
            }
        }
    }
    Ok(())
}

/// The name that libsqlite3.rs uses for an app's databases.
fn sqlite_namespace(bundle_id: &str) -> String {
    bundle_id
        .replace('/', "_")
        .replace('\\', "_")
        .replace(':', "_")
        .replace(' ', "_")
}

/// Whether `name` is one ordinary path component, so that joining it onto a
/// directory can't point somewhere else (e.g. `..` or `a/b`).
fn is_plain_name(name: &str) -> bool {
    let mut components = Path::new(name).components();
    !name.is_empty()
        && !name.contains('/')
        && !name.contains('\\')
        && matches!(
            (components.next(), components.next()),
            (Some(std::path::Component::Normal(_)), None)
        )
}

#[cfg(test)]
mod delete_app_tests {
    use super::*;

    #[test]
    fn sqlite_namespace_matches_libsqlite3() {
        assert_eq!(sqlite_namespace("com.example.game"), "com.example.game");
        assert_eq!(sqlite_namespace("com.example.my game"), "com.example.my_game");
    }

    #[test]
    fn only_plain_names_are_accepted() {
        assert!(is_plain_name("com.example.game"));
        assert!(!is_plain_name(""));
        assert!(!is_plain_name("."));
        assert!(!is_plain_name(".."));
        assert!(!is_plain_name("a/b"));
        assert!(!is_plain_name("a\\b"));
    }

    #[test]
    fn sqlite_prefix_excludes_longer_sibling_bundle_id() {
        let others = ["com.foo.game_hd", "com.bar.other"];
        assert!(sqlite_file_belongs_to_bundle(
            "com.foo.game_save.sqlite",
            "com.foo.game",
            &others
        ));
        assert!(!sqlite_file_belongs_to_bundle(
            "com.foo.game_hd_save.sqlite",
            "com.foo.game",
            &others
        ));
    }
}

/// Watch state for detecting a newly copied-in .ipa file.
struct IpaWatch {
    last_seen: Vec<(String, u64)>,
    dirty: bool,
    last_change: Option<Instant>,
}

/// List the .ipa files directly inside the apps directory, as (name, size).
///
/// The size matters: a file that is still being copied grows, and must not
/// be opened until it is complete. This is cheap enough to poll every
/// run-loop iteration, unlike a full [enumerate_apps], which opens every
/// app bundle.
fn list_top_level_ipa_files(apps_dir: &Path) -> Vec<(String, u64)> {
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(apps_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension() == Some(OsStr::new("ipa")) {
                let name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                files.push((name, size));
            }
        }
    }
    files.sort();
    files
}

/// How long the .ipa listing must stay unchanged before the app grid is
/// refreshed, so that a file that is still being copied is left alone.
const IPA_COPY_SETTLE_TIME: Duration = Duration::from_millis(500);

#[derive(Default)]
struct AppPickerDelegateHostObject {
    icon_tapped: id,
    // Set by the add-app tile; kept separately from icon_tapped because the
    // picker needs to wait for a newly copied IPA to settle before reloading.
    add_ipa: bool,
    recent_app_tapped: bool,
    status_accessory_tapped: bool,
    access_prompt_confirm: bool,
    access_prompt_cancel: bool,
    access_prompt_submit: bool,
    // Actions used by the built-in Internet Archive store screen.
    archive_store_close: bool,
    archive_store_search: bool,
    archive_store_previous: bool,
    archive_store_next: bool,
    archive_store_back: bool,
    archive_store_retry: bool,
    archive_store_cancel: bool,
    archive_store_item: Option<NSInteger>,
    archive_store_download: Option<NSInteger>,
    archive_store_tab: Option<NSInteger>,
    archive_store_sort: bool,
    archive_store_fuzzy: bool,
    archive_store_queue_action: Option<NSInteger>,
    archive_store_tap: Option<CGPoint>,
    settings_show: bool,
    settings_hide: bool,
    scale_hack_default: bool,
    scale_hack1: bool,
    scale_hack2: bool,
    scale_hack3: bool,
    scale_hack4: bool,
    orientation_default: bool,
    orientation_landscape_left: bool,
    orientation_landscape_right: bool,
    orientation_portrait_upside_down: bool,
    analog_stick_tilt_controls: Option<bool>,
    network: Option<bool>,
    game_center_authenticated: Option<bool>,
    show_fps: Option<bool>,
    overlay_mode: Option<PerformanceOverlayMode>,
    overlay_corner: Option<PerformanceOverlayCorner>,
    cheat_engine: Option<bool>,
    /// `Some(..)` only when the user just flipped the switch: `None` means no
    /// change is pending. The saved state is tri-state (see `PickerSettings`).
    rotation_fix: Option<bool>,
    /// `Some(..)` only when the user just flipped the switch.
    fix_movie_player: Option<bool>,
    fps_limit_default: bool,
    fps_limit30: bool,
    fps_limit60: bool,
    fps_limit_off: bool,
    /// `Some(..)` only when the user just picked an entry of the RAM limiter
    /// dropdown.
    memory_limit_tag: Option<i32>,
    memory_limit_toggle: bool,
    memory_limit_scroll_to: Option<CGFloat>,
    preserve_aspect_ratio: Option<bool>,
    show_lock_screen: Option<bool>,
    force_composition: Option<bool>,
    gles_native: Option<bool>,
    fullscreen: Option<bool>,
    device_model_tag: Option<i32>,
    device_model_toggle: bool,
    device_model_scroll_to: Option<CGFloat>,
    language_tag: Option<i32>,
    language_toggle: bool,
    language_scroll_to: Option<CGFloat>,
    /// Set when a finger goes down on an app icon (`UIControlEventTouchDown`).
    icon_touch_down: id,
    /// Set when a touch that started on an icon stops being a tap.
    icon_touch_cancelled: bool,
    delete_cancel: bool,
    delete_confirm: bool,
    /// Set when the horizontal swipe recognizers fire (old-iOS page change).
    swipe_left: bool,
    swipe_right: bool,
    lock_screen_unlocked: bool,
    lock_slider_progress: f32,
}
impl HostObject for AppPickerDelegateHostObject {}

/// The framerate limits offered by the Display section, besides "Default"
/// (which leaves the command line and the options files in charge).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum FpsLimit {
    Fps30,
    Fps60,
    Off,
}

impl FpsLimit {
    /// The value of `--fps-limit=`, and the name used in the settings file.
    fn short_name(self) -> &'static str {
        match self {
            FpsLimit::Fps30 => "30",
            FpsLimit::Fps60 => "60",
            FpsLimit::Off => "off",
        }
    }

    fn from_short_name(value: &str) -> Option<Self> {
        match value {
            "30" => Some(FpsLimit::Fps30),
            "60" => Some(FpsLimit::Fps60),
            "off" => Some(FpsLimit::Off),
            _ => None,
        }
    }

    /// The index of the segment button that shows this choice.
    fn segment_index(self) -> usize {
        match self {
            FpsLimit::Fps30 => 1,
            FpsLimit::Fps60 => 2,
            FpsLimit::Off => 3,
        }
    }
}

/// The choices offered by the "RAM Limiter" dropdown, in display order:
/// `None` leaves the app's heap uncapped, otherwise the cap in MiB.
const MEMORY_LIMIT_CHOICES: &[Option<u32>] = &[
    None,
    Some(128),
    Some(256),
    Some(512),
    Some(1024),
    Some(2048),
];

/// The button title of the "RAM Limiter" dropdown for a choice.
fn memory_limit_label(choice: Option<u32>) -> String {
    match choice {
        None => "Default".to_string(),
        Some(mb) if mb >= 1024 => format!("{} GB", mb / 1024),
        Some(mb) => format!("{mb} MB"),
    }
}

/// The dropdown tag of a RAM limiter choice (`0` is "Default").
fn memory_limit_tag(choice: Option<u32>) -> i32 {
    MEMORY_LIMIT_CHOICES
        .iter()
        .position(|candidate| *candidate == choice)
        .unwrap_or(0) as i32
}

/// The RAM limiter choice of a dropdown tag.
fn memory_limit_choice(tag: Option<i32>) -> Option<u32> {
    tag.and_then(|tag| MEMORY_LIMIT_CHOICES.get(tag as usize).copied().flatten())
}

#[derive(Clone, PartialEq)]
struct PickerSettings {
    scale_hack: Option<NonZeroU32>,
    orientation: Option<DeviceOrientation>,
    fullscreen: bool,
    preserve_aspect_ratio: bool,
    show_lock_screen: bool,
    gles_native: bool,
    force_composition: bool,
    show_fps: bool,
    overlay_mode: PerformanceOverlayMode,
    overlay_corner: PerformanceOverlayCorner,
    /// `None` leaves these compatibility fixes to the command line and the
    /// app-specific options files. `Some(..)` is an explicit override from the
    /// Settings screen, including an explicit "off" that must be able to beat
    /// an options file which enables the fix (e.g. GTA III defaults).
    rotation_fix: Option<bool>,
    fix_movie_player: Option<bool>,
    /// `None` leaves the FPS limit to the command line and the app-specific
    /// options files. `Some(..)` is an explicit choice from the Display
    /// section, which is re-emitted on every later launch.
    fps_limit: Option<FpsLimit>,
    /// `None` leaves the app's heap uncapped (or capped by an options file).
    /// `Some(..)` is an explicit "RAM Limiter" choice, in MiB.
    memory_limit_mb: Option<u32>,
    cheat_engine: bool,
    network: bool,
    game_center_authenticated: bool,
    analog_stick_tilt_controls: bool,
    device_model_tag: Option<i32>,
    language_tag: Option<i32>,
}

impl PickerSettings {
    fn load(options: &Options, angle_backend_available: bool) -> Self {
        let (gles_native, _) = quick_options_gles_native_state(options, angle_backend_available);
        let mut settings = Self {
            scale_hack: None,
            orientation: None,
            fullscreen: options.fullscreen,
            preserve_aspect_ratio: options.preserve_aspect_ratio,
            show_lock_screen: true,
            gles_native,
            force_composition: options.force_composition,
            show_fps: options.performance_overlay_enabled,
            overlay_mode: options.performance_overlay_mode.for_current_platform(),
            overlay_corner: options.performance_overlay_corner,
            // A flag passed on the command line is already an explicit choice;
            // show it as such. Without one, the fixes stay "unset" so the
            // app-specific defaults in the options files still apply.
            rotation_fix: options.rotation_fix.then_some(true),
            fix_movie_player: options.fix_movie_player.then_some(true),
            // Both start as "untouched": the command line already sticks
            // around and the options files still get their say. Only the saved
            // settings file below can make these explicit.
            fps_limit: None,
            memory_limit_mb: None,
            cheat_engine: quick_options_trainer_enabled(options),
            network: options.network_access,
            game_center_authenticated: options.game_center_authenticated,
            analog_stick_tilt_controls: options.analog_stick_tilt_controls,
            device_model_tag: None,
            language_tag: None,
        };
        let path = paths::user_data_base_path().join(paths::PICKER_SETTINGS_FILE);
        let Ok(contents) = std::fs::read_to_string(path) else {
            return settings;
        };
        for line in contents.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim();
            match key.trim() {
                "scale_hack" => {
                    settings.scale_hack = match value {
                        "default" => None,
                        _ => value.parse().ok().and_then(NonZeroU32::new),
                    }
                }
                "orientation" => {
                    settings.orientation = match value {
                        "default" => None,
                        "landscape-left" => Some(DeviceOrientation::LandscapeLeft),
                        "landscape-right" => Some(DeviceOrientation::LandscapeRight),
                        "upside-down" => Some(DeviceOrientation::PortraitUpsideDown),
                        _ => settings.orientation,
                    }
                }
                "fullscreen" => set_bool(value, &mut settings.fullscreen),
                "preserve_aspect_ratio" => set_bool(value, &mut settings.preserve_aspect_ratio),
                "show_lock_screen" => set_bool(value, &mut settings.show_lock_screen),
                "gles_native" => set_bool(value, &mut settings.gles_native),
                "force_composition" => set_bool(value, &mut settings.force_composition),
                "show_fps" => set_bool(value, &mut settings.show_fps),
                "overlay_mode" => {
                    if let Some(mode) = PerformanceOverlayMode::from_short_name(value) {
                        settings.overlay_mode = mode;
                    }
                }
                "overlay_corner" => {
                    if let Some(corner) = PerformanceOverlayCorner::from_short_name(value) {
                        settings.overlay_corner = corner;
                    }
                }
                "rotation_fix" => set_bool_override(value, &mut settings.rotation_fix),
                "fix_movie_player" => set_bool_override(value, &mut settings.fix_movie_player),
                "fps_limit" => {
                    settings.fps_limit = FpsLimit::from_short_name(value);
                }
                "memory_limit_mb" => {
                    settings.memory_limit_mb = match value {
                        "default" => None,
                        _ => value.parse().ok().filter(|mb| *mb > 0),
                    };
                }
                "cheat_engine" => set_bool(value, &mut settings.cheat_engine),
                "network" => set_bool(value, &mut settings.network),
                "game_center_authenticated" => {
                    set_bool(value, &mut settings.game_center_authenticated)
                }
                "analog_stick_tilt_controls" => {
                    set_bool(value, &mut settings.analog_stick_tilt_controls)
                }
                "device_model_tag" => settings.device_model_tag = parse_optional_tag(value),
                "language_tag" => settings.language_tag = parse_optional_tag(value),
                _ => (),
            }
        }
        settings.overlay_mode = settings.overlay_mode.for_current_platform();
        settings
    }

    fn save(&self) {
        let path = paths::user_data_base_path().join(paths::PICKER_SETTINGS_FILE);
        let orientation = match self.orientation {
            None => "default",
            Some(DeviceOrientation::LandscapeLeft) => "landscape-left",
            Some(DeviceOrientation::LandscapeRight) => "landscape-right",
            Some(DeviceOrientation::PortraitUpsideDown) => "upside-down",
            Some(DeviceOrientation::Portrait) => "default",
        };
        let contents = format!(
            "# Saved by the graphical settings screen.\n\
             scale_hack={}\n\
             orientation={orientation}\n\
             fullscreen={}\n\
             preserve_aspect_ratio={}\n\
             show_lock_screen={}\n\
             gles_native={}\n\
             force_composition={}\n\
             show_fps={}\n\
             overlay_mode={}\n\
             overlay_corner={}\n\
             rotation_fix={}\n\
             fix_movie_player={}\n\
             fps_limit={}\n\
             memory_limit_mb={}\n\
             cheat_engine={}\n\
             network={}\n\
             game_center_authenticated={}\n\
             analog_stick_tilt_controls={}\n\
             device_model_tag={}\n\
             language_tag={}\n",
            self.scale_hack
                .map_or_else(|| "default".to_string(), |n| n.to_string()),
            self.fullscreen,
            self.preserve_aspect_ratio,
            self.show_lock_screen,
            self.gles_native,
            self.force_composition,
            self.show_fps,
            self.overlay_mode.short_name(),
            self.overlay_corner.short_name(),
            optional_bool(self.rotation_fix),
            optional_bool(self.fix_movie_player),
            self.fps_limit.map_or("default", |limit| limit.short_name()),
            self.memory_limit_mb
                .map_or_else(|| "default".to_string(), |mb| mb.to_string()),
            self.cheat_engine,
            self.network,
            self.game_center_authenticated,
            self.analog_stick_tilt_controls,
            optional_tag(self.device_model_tag),
            optional_tag(self.language_tag),
        );
        if let Err(error) = std::fs::write(path, contents) {
            log!("Warning: couldn't save app picker settings: {error}");
        }
    }
}

fn set_bool(value: &str, setting: &mut bool) {
    match value {
        "true" => *setting = true,
        "false" => *setting = false,
        _ => (),
    }
}

/// Reads a tri-state setting from the picker settings file. Only explicit
/// `true`/`false` values are overrides; `default` (or an unknown value) leaves
/// the value derived from the command line untouched, so a CLI flag is not
/// silently cancelled by a saved "untouched switch" line.
fn set_bool_override(value: &str, setting: &mut Option<bool>) {
    match value {
        "true" => *setting = Some(true),
        "false" => *setting = Some(false),
        _ => (),
    }
}

fn optional_bool(value: Option<bool>) -> &'static str {
    match value {
        None => "default",
        Some(true) => "true",
        Some(false) => "false",
    }
}

fn parse_optional_tag(value: &str) -> Option<i32> {
    if value == "default" {
        None
    } else {
        value.parse().ok()
    }
}

fn optional_tag(tag: Option<i32>) -> String {
    tag.map_or_else(|| "default".to_string(), |tag| tag.to_string())
}

pub const DYLIB: crate::dyld::HostDylib = crate::dyld::HostDylib {
    // Not a real iOS dylib obviously. This shouldn't really be in the list of
    // dylibs if we can avoid it somehow (TODO?).
    path: "/.touchHLE/AppPickerHelpers.dylib",
    aliases: &[],
    class_exports: &[CLASSES],
    constant_exports: &[],
    function_exports: &[],
};

/// Be careful! These classes go in the normal class list, just like everything
/// else, so an app could try to instantiate them. Don't give them special
/// powers that could be exploited!
const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation _touchHLE_AppPickerDelegate: NSObject

- (())iconTapped:(id)sender {
    // There is no allocWithZone: that creates AppPickerDelegateHostObject, so
    // this downcast effectively acts as an assertion that this class is being
    // used within the app picker, so it can't be abused. :)
    let host_obj = env.objc.borrow_mut::<AppPickerDelegateHostObject>(this);
    host_obj.icon_tapped = sender;
}

- (())addIpa {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).add_ipa = true;
}

- (())statusAccessoryTapped {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).status_accessory_tapped = true;
}
- (())accessPromptConfirm {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).access_prompt_confirm = true;
}
- (())accessPromptCancel {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).access_prompt_cancel = true;
}

- (())archiveStoreClose {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).archive_store_close = true;
}
- (())archiveStoreSearch {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).archive_store_search = true;
}
- (())archiveStorePrevious {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).archive_store_previous = true;
}
- (())archiveStoreNext {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).archive_store_next = true;
}
- (())archiveStoreBack {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).archive_store_back = true;
}
- (())archiveStoreRetry {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).archive_store_retry = true;
}
- (())archiveStoreCancel {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).archive_store_cancel = true;
}
- (())archiveStoreItem:(id)sender {
    let tag: NSInteger = msg![env; sender tag];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).archive_store_item = Some(tag);
}
- (())archiveStoreDownload:(id)sender {
    let tag: NSInteger = msg![env; sender tag];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).archive_store_download = Some(tag);
}
- (())archiveStoreTab:(id)sender {
    let tag: NSInteger = msg![env; sender tag];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).archive_store_tab = Some(tag);
}
- (())archiveStoreSort {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).archive_store_sort = true;
}
- (())archiveStoreFuzzy {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).archive_store_fuzzy = true;
}
- (())archiveStoreQueueAction:(id)sender {
    let tag: NSInteger = msg![env; sender tag];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).archive_store_queue_action = Some(tag);
}
- (())archiveStoreTap:(id)sender {
    let view: id = msg![env; sender view];
    let point: CGPoint = msg![env; sender locationInView:view];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).archive_store_tap = Some(point);
}
- (())textFieldShouldReturn:(id)sender {
    let tag: NSInteger = msg![env; sender tag];
    let host = env.objc.borrow_mut::<AppPickerDelegateHostObject>(this);
    if tag == access_prompt::FIELD_TAG {
        host.access_prompt_submit = true;
    } else {
        host.archive_store_search = true;
    }
    drop(host);
    let _: bool = msg![env; sender resignFirstResponder];
}

- (())settingsShow {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).settings_show = true;
}
- (())settingsHide {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).settings_hide = true;
}
- (())scaleHackDefault {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).scale_hack_default = true;
}
- (())scaleHack1 {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).scale_hack1 = true;
}
- (())scaleHack2 {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).scale_hack2 = true;
}
- (())scaleHack3 {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).scale_hack3 = true;
}
- (())scaleHack4 {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).scale_hack4 = true;
}
- (())orientationDefault {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).orientation_default = true;
}
- (())orientationLandscapeLeft {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).orientation_landscape_left = true;
}
- (())orientationLandscapeRight {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).orientation_landscape_right = true;
}
- (())orientationPortraitUpsideDown {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).orientation_portrait_upside_down = true;
}
- (())analogStickTiltControls:(id)switch { // UISwitch*
    let switch_state: bool = msg![env; switch isOn];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).analog_stick_tilt_controls = Some(switch_state);
}
- (())network:(id)switch { // UISwitch*
    let switch_state: bool = msg![env; switch isOn];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).network = Some(switch_state);
}
- (())gameCenterAuthenticated:(id)switch { // UISwitch*
    let switch_state: bool = msg![env; switch isOn];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).game_center_authenticated = Some(switch_state);
}
- (())rotationFix:(id)switch { // UISwitch*
    let switch_state: bool = msg![env; switch isOn];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).rotation_fix = Some(switch_state);
}
- (())moviePlayerFix:(id)switch { // UISwitch*
    let switch_state: bool = msg![env; switch isOn];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).fix_movie_player = Some(switch_state);
}
- (())preserveAspectRatio:(id)switch { // UISwitch*
    let switch_state: bool = msg![env; switch isOn];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).preserve_aspect_ratio = Some(switch_state);
}
- (())showLockScreen:(id)switch { // UISwitch*
    let switch_state: bool = msg![env; switch isOn];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).show_lock_screen = Some(switch_state);
}
- (())forceComposition:(id)switch { // UISwitch*
    let switch_state: bool = msg![env; switch isOn];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).force_composition = Some(switch_state);
}
- (())glesNative:(id)switch { // UISwitch*
    let switch_state: bool = msg![env; switch isOn];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).gles_native = Some(switch_state);
}
- (())overlayModeFull {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).overlay_mode = Some(PerformanceOverlayMode::Full);
}
- (())overlayModeMinimal {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).overlay_mode = Some(PerformanceOverlayMode::Minimal);
}
- (())overlayModeTitleBar {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).overlay_mode = Some(PerformanceOverlayMode::TitleBar);
}
- (())overlayCornerTopLeft {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).overlay_corner = Some(PerformanceOverlayCorner::TopLeft);
}
- (())overlayCornerTopRight {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).overlay_corner = Some(PerformanceOverlayCorner::TopRight);
}
- (())overlayCornerBottomRight {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).overlay_corner = Some(PerformanceOverlayCorner::BottomRight);
}
- (())overlayCornerBottomLeft {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).overlay_corner = Some(PerformanceOverlayCorner::BottomLeft);
}
- (())fpsLimitDefault {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).fps_limit_default = true;
}
- (())fpsLimit30 {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).fps_limit30 = true;
}
- (())fpsLimit60 {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).fps_limit60 = true;
}
- (())fpsLimitOff {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).fps_limit_off = true;
}
- (())memoryLimit:(id)sender { // UIButton*
    let tag: NSInteger = msg![env; sender tag];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).memory_limit_tag = Some(tag as i32);
}
- (())memoryLimitToggle {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).memory_limit_toggle = true;
}
- (())memoryLimitScroll:(id)sender forEvent:(id)event {
    if let Some(position) = dropdown_scroll_position(env, sender, event) {
        env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).memory_limit_scroll_to = Some(position);
    }
}
- (())showFPS:(id)switch { // UISwitch*
    let switch_state: bool = msg![env; switch isOn];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).show_fps = Some(switch_state);
    env.window_mut().set_show_fps_counter(switch_state);
}
- (())cheatEngine:(id)switch { // UISwitch*
    let switch_state: bool = msg![env; switch isOn];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).cheat_engine = Some(switch_state);
}
- (())fullscreen:(id)switch { // UISwitch*
    let switch_state: bool = msg![env; switch isOn];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).fullscreen = Some(switch_state);
}
- (())deviceModel:(id)sender { // UIButton*
    let tag: NSInteger = msg![env; sender tag];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).device_model_tag = Some(tag as i32);
}
- (())deviceModelToggle {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).device_model_toggle = true;
}
- (())deviceModelScroll:(id)sender forEvent:(id)event {
    if let Some(position) = dropdown_scroll_position(env, sender, event) {
        env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).device_model_scroll_to = Some(position);
    }
}
- (())language:(id)sender { // UIButton*
    let tag: NSInteger = msg![env; sender tag];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).language_tag = Some(tag as i32);
}
- (())languageToggle {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).language_toggle = true;
}
- (())languageScroll:(id)sender forEvent:(id)event {
    if let Some(position) = dropdown_scroll_position(env, sender, event) {
        env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).language_scroll_to = Some(position);
    }
}
- (())iconTouchDown:(id)sender { // UIButton*
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).icon_touch_down = sender;
}
- (())iconTouchCancelled:(id)_sender { // UIButton*
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).icon_touch_cancelled = true;
}
- (())deleteCancel {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).delete_cancel = true;
}
- (())deleteConfirm {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).delete_confirm = true;
}
- (())swipeLeft {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).swipe_left = true;
}
- (())swipeRight {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).swipe_right = true;
}
- (())lockSliderChanged:(id)sender {
    let progress: f32 = msg![env; sender value]; // UISlider*
    let mut host = env.objc.borrow_mut::<AppPickerDelegateHostObject>(this);
    host.lock_slider_progress = progress;
    if progress >= 0.96 {
        host.lock_screen_unlocked = true;
    }
}
- (())openRecentApp {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).recent_app_tapped = true;
}

@end

};

fn show_app_picker_gui(
    mut options: Options,
    apps: Result<Vec<AppInfo>, String>,
    show_lock_screen: bool,
) -> Result<(PathBuf, Vec<String>), String> {
    if std::env::consts::OS == "ios" && options.scale_hack.get() == 1 {
        options.scale_hack = NonZeroU32::new(3).unwrap();
    }

    let icon = {
        let bytes: &[u8] = match crate::branding() {
            "" | "UNOFFICIAL" => include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/res/icon.png")),
            "PREVIEW" => {
                include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/res/icon_preview.png"))
            }
            _ => panic!(),
        };
        let mut image = Image::from_bytes(bytes).unwrap();
        // Should match Bundle::load_icon(): the corner radius scales with the
        // icon's resolution.
        image.round_icon_corners(/* add_sheen: */ true);
        image
    };
    let environment = Environment::new_without_app(options, icon)?;
    Ok(environment.run_app_picker(move |env| app_picker_inner(env, apps, show_lock_screen)))
}

fn app_picker_inner(
    env: &mut Environment,
    mut apps: Result<Vec<AppInfo>, String>,
    show_lock_screen: bool,
) -> (PathBuf, Vec<String>) {
    let mut option_args = Vec::new();
    // Note that objects are generally not released in this code, because they
    // don't need to be: the entire Environment is thrown away at the end.

    // Bypassing UIApplicationMain!
    let ui_application: id = msg_class![env; UIApplication new];
    let delegate = env
        .objc
        .get_known_class("_touchHLE_AppPickerDelegate", &mut env.mem);
    let delegate = env.objc.alloc_object(
        delegate,
        Box::<AppPickerDelegateHostObject>::default(),
        &mut env.mem,
    );
    () = msg![env; ui_application setDelegate:delegate];

    let screen: id = msg_class![env; UIScreen mainScreen];
    let bounds: CGRect = msg![env; screen bounds];

    let window: id = msg_class![env; UIWindow alloc];
    let window: id = msg![env; window initWithFrame:bounds];

    let app_frame: CGRect = msg![env; screen applicationFrame];

    // Wallpaper
    let mut found_wallpaper = false;
    let mut wallpaper_view: Option<id> = None;
    for candidate in paths::WALLPAPER_FILES {
        let candidate = paths::user_data_base_path().join(candidate);
        if !candidate.exists() {
            continue;
        }
        found_wallpaper = true;

        let image = match std::fs::read(&candidate) {
            Ok(image) => image,
            Err(e) => {
                log!("Warning: couldn't read {}: {}", candidate.display(), e);
                break;
            }
        };
        let image = match Image::from_bytes(&image) {
            Ok(image) => image,
            Err(e) => {
                log!("Warning: couldn't decode {}: {}", candidate.display(), e);
                break;
            }
        };

        let image = cg_image::from_image(env, image);
        let image: id = msg_class![env; UIImage imageWithCGImage:image];
        let wallpaper: id = msg_class![env; UIImageView alloc];
        let wallpaper: id = msg![env; wallpaper initWithImage:image];
        // The wallpaper also covers the status bar strip above the
        // application frame, like the old iOS home screen.
        let status_offset = app_frame.origin.y;
        () = msg![env; wallpaper setFrame:(CGRect {
            origin: CGPoint {
                x: 0.0,
                y: 0.0,
            },
            size: CGSize {
                width: app_frame.size.width,
                height: app_frame.size.height + status_offset,
            },
        })];
        () = msg![env; wallpaper setAlpha:(1.0 as CGFloat)];
        () = msg![env; window addSubview:wallpaper];
        wallpaper_view = Some(wallpaper);
        break;
    }
    if !found_wallpaper {
        let CGSize { width, height } = app_frame.size;
        log!(
            "No wallpaper found; filename can be one of: {}; ideal size is {}×{} pixels",
            paths::WALLPAPER_FILES.join(", "),
            width,
            height,
        );
    }

    // Old-iOS status bar, drawn in the strip above the application frame. It
    // sits above the wallpaper, while the (transparent) main view is added
    // after it: the home screen shows the bar through the main view, but the
    // opaque Settings screen covers it when open.
    let (status_bar, status_time_label, status_charge_view) = if app_frame.origin.y >= 12.0 {
        let (bar, time, charge) =
            make_status_bar(env, delegate, bounds.size.width, app_frame.origin.y);
        () = msg![env; window addSubview:bar];
        // The level is cached at window creation, so reading it here is safe
        // even on Android (no JNI on this coroutine stack).
        update_battery_indicator(env, charge, crate::window::cached_battery_status());
        (Some(bar), Some(time), Some(charge))
    } else {
        (None, None, None)
    };

    let main_view: id = msg_class![env; UIView alloc];
    let main_view: id = msg![env; main_view initWithFrame:app_frame];
    () = msg![env; window addSubview:main_view];

    // Old-iOS home screen paging: swipe left/right anywhere on the home screen.
    // The recognizers are attached to the main view, so touches that land on
    // icons or on the dock are seen too.
    let swipe_left_selector = env.objc.lookup_selector("swipeLeft").unwrap();
    let swipe_right_selector = env.objc.lookup_selector("swipeRight").unwrap();
    let swipe_left: id = msg_class![env; UISwipeGestureRecognizer alloc];
    let swipe_left: id = msg![env;
        swipe_left initWithTarget:delegate action:swipe_left_selector];
    () = msg![env; swipe_left setDirection:UISwipeGestureRecognizerDirectionLeft];
    () = msg![env; main_view addGestureRecognizer:swipe_left];
    let swipe_right: id = msg_class![env; UISwipeGestureRecognizer alloc];
    let swipe_right: id = msg![env;
        swipe_right initWithTarget:delegate action:swipe_right_selector];
    () = msg![env; swipe_right setDirection:UISwipeGestureRecognizerDirectionRight];
    () = msg![env; main_view addGestureRecognizer:swipe_right];

    // Resolve the last launched app for the dock shortcut. The record is a
    // path under the apps directory, and is ignored if that app was removed.
    let remembered_app_path = read_last_app_path();
    let recent_app = apps.as_mut().ok().and_then(|apps| {
        let app = apps.iter_mut().find(|app| {
            remembered_app_path
                .as_ref()
                .is_some_and(|path| path == &app.path)
        })?;
        let image = if let Some(icon) = app.icon.as_ref() {
            let cg_image = cg_image::from_image(env, icon.clone());
            msg_class![env; UIImage imageWithCGImage:cg_image]
        } else {
            make_icon_from_text(
                env,
                &app.display_name.chars().take(2).collect::<String>(),
                28.0,
                0.0,
                (0.35, 0.35, 0.35, 1.0),
            )
        };
        Some((app.path.clone(), app.display_name.clone(), image))
    });
    let mut icon_grid_stuff = match &mut apps {
        Ok(ref mut apps) => {
            let mut icon_grid_stuff =
                make_icon_grid(env, delegate, main_view, app_frame, apps.len());
            update_icon_grid(env, &mut icon_grid_stuff, apps, 0, 0);
            Some(icon_grid_stuff)
        }
        Err(e) => {
            let label_frame = CGRect {
                origin: CGPoint { x: 10.0, y: 10.0 },
                size: CGSize {
                    width: app_frame.size.width - 20.0,
                    height: app_picker_grid_bottom(app_frame.size.height) - 20.0,
                },
            };
            let label: id = msg_class![env; UILabel alloc];
            let label: id = msg![env; label initWithFrame:label_frame];
            let text = ns_string::from_rust_string(env, e.clone());
            () = msg![env; label setText:text];
            () = msg![env; label setTextAlignment:UITextAlignmentCenter];
            () = msg![env; label setNumberOfLines:0]; // unlimited
            let text_color: id = msg_class![env; UIColor lightGrayColor];
            () = msg![env; label setTextColor:text_color];
            let bg_color: id = msg_class![env; UIColor clearColor];
            () = msg![env; label setBackgroundColor:bg_color];
            () = msg![env; main_view addSubview:label];
            None
        }
    };

    // Old-iOS page dots, and the dock with the shortcuts that aren't apps.
    let mut page_dots = Vec::new();
    if let Some(ref grid) = icon_grid_stuff {
        page_dots = make_page_dots(env, main_view, app_frame.size, grid.pages.len());
        update_page_dots(env, &page_dots, 0);
    }
    let dock = make_dock(
        env,
        delegate,
        main_view,
        app_frame.size,
        recent_app.as_ref().map(|(_, name, image)| (name.as_str(), *image)),
    );
    // Build stamp on the page-dot row, right-aligned like the one in the
    // reference home screen. It is added after the dock so nothing covers it.
    let version_label = {
        let label_frame = CGRect {
            origin: CGPoint {
                x: 0.0,
                y: app_picker_dots_top(app_frame.size.height) - 2.0,
            },
            size: CGSize {
                width: app_frame.size.width - 8.0,
                height: 12.0,
            },
        };
        let label: id = msg_class![env; UILabel alloc];
        let label: id = msg![env; label initWithFrame:label_frame];
        let text = ns_string::from_rust_string(
            env,
            format!("{} ({})", crate::DISPLAY_NAME, crate::COMMIT_HASH),
        );
        () = msg![env; label setText:text];
        () = msg![env; label setTextAlignment:UITextAlignmentRight];
        let font_size: CGFloat = 10.0;
        let font: id = msg_class![env; UIFont systemFontOfSize:font_size];
        () = msg![env; label setFont:font];
        let text_color: id = msg_class![env; UIColor lightGrayColor];
        () = msg![env; label setTextColor:text_color];
        let bg_color: id = msg_class![env; UIColor clearColor];
        () = msg![env; label setBackgroundColor:bg_color];
        let shadow = ui_color(env, 0.0, 0.0, 0.0, 0.7);
        () = msg![env; label setShadowColor:shadow];
        () = msg![env; label setShadowOffset:(CGSize {
            width: 0.0,
            height: 1.0,
        })];
        () = msg![env; main_view addSubview:label];
        label
    };

    let angle_backend_available = crate::window::angle_backend_available();
    let mut picker_settings = PickerSettings::load(&env.options, angle_backend_available);
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    env.window_mut()
        .set_preserve_aspect_ratio(picker_settings.preserve_aspect_ratio);
    let quick_options_gles_native_switch_enabled = angle_backend_available;
    let show_fullscreen_row = !crate::window::Window::rotatable_fullscreen();
    let overlay_hidden_by_environment = crate::gles::perf_overlay::hidden_by_environment();
    let initial_quick_options_show_fps =
        picker_settings.show_fps && !overlay_hidden_by_environment;
    let settings = setup_settings(
        env,
        delegate,
        main_view,
        app_frame,
        &picker_settings,
        initial_quick_options_show_fps,
        quick_options_gles_native_switch_enabled,
        show_fullscreen_row,
    );
    let mut quick_options_scale_hack = picker_settings.scale_hack;
    let mut quick_options_fullscreen = picker_settings.fullscreen;
    let mut quick_options_orientation = picker_settings.orientation;
    let mut quick_options_analog_stick_tilt_controls = picker_settings.analog_stick_tilt_controls;
    let mut quick_options_network = picker_settings.network;
    let mut quick_options_show_fps = initial_quick_options_show_fps;
    let mut quick_options_overlay_mode = picker_settings.overlay_mode;
    let mut quick_options_overlay_corner = picker_settings.overlay_corner;
    let mut quick_options_rotation_fix = picker_settings.rotation_fix;
    let mut quick_options_fix_movie_player = picker_settings.fix_movie_player;
    let mut quick_options_fps_limit = picker_settings.fps_limit;
    let mut quick_options_memory_limit_mb = picker_settings.memory_limit_mb;
    let mut quick_options_preserve_aspect_ratio = picker_settings.preserve_aspect_ratio;
    let mut quick_options_show_lock_screen = picker_settings.show_lock_screen;
    let mut quick_options_force_composition_override = Some(picker_settings.force_composition);
    let mut quick_options_cheat_engine = picker_settings.cheat_engine;
    let mut quick_options_gles_native = picker_settings.gles_native;
    let mut device_state = DropdownState {
        selected: picker_settings.device_model_tag,
        ..Default::default()
    };
    let mut language_state = DropdownState {
        selected: picker_settings.language_tag,
        ..Default::default()
    };
    let mut memory_limit_state = DropdownState {
        selected: Some(memory_limit_tag(picker_settings.memory_limit_mb)),
        ..Default::default()
    };
    let mut last_saved_picker_settings = picker_settings.clone();

    update_scale_hack_buttons(env, &settings.scale_hack_buttons, quick_options_scale_hack);
    update_orientation_buttons(env, &settings.orientation_buttons, quick_options_orientation);
    update_fps_limit_buttons(env, &settings.fps_limit_buttons, quick_options_fps_limit);
    update_performance_overlay_buttons(
        env,
        &settings.overlay_mode_buttons,
        &settings.overlay_corner_buttons,
        quick_options_overlay_mode,
        quick_options_overlay_corner,
    );
    refresh_dropdown(
        env,
        &settings.device_dropdown,
        &device_model_label_for_tag(device_state.selected),
        &device_state,
    );
    refresh_dropdown(
        env,
        &settings.language_dropdown,
        &language_label_for_tag(language_state.selected),
        &language_state,
    );
    refresh_dropdown(
        env,
        &settings.memory_limit_dropdown,
        &memory_limit_label(quick_options_memory_limit_mb),
        &memory_limit_state,
    );

    // Built after the settings screen, so that it is drawn on top of it.
    let delete_dialog = make_delete_dialog(env, delegate, main_view, app_frame.size);

    () = msg![env; window makeKeyAndVisible];

    let apps_dir = paths::user_data_base_path().join(paths::APPS_DIR);
    let mut current_page: usize = 0;
    // Discard arrow presses made before the picker was shown.
    PENDING_ARROW_KEY.store(0, Ordering::Relaxed);
    PENDING_MOUSE_WHEEL.store(0, Ordering::Relaxed);
    // If the user taps the dock's "Add .ipa" shortcut, this records the .ipa
    // files that existed at that moment; once a new one shows up, the app list
    // is re-enumerated.
    let mut awaited_ipa: Option<IpaWatch> = None;
    // The app icon under a finger that may turn into a long press, and when
    // the finger went down.
    let mut press: Option<(id, Instant)> = None;
    // The icon of the last long press. The release that ends it is not a tap.
    let mut long_pressed_icon: Option<id> = None;
    // Index of the app whose delete dialog is open, if any.
    let mut pending_delete: Option<usize> = None;

    // The status bar clock and battery only need refreshing now and then.
    let mut status_time = status_bar_time_string();
    let mut status_time_updated_at = Instant::now();
    let mut status_battery_updated_at = Instant::now();

    let mut chrome_views: Vec<id> = Vec::with_capacity(3 + page_dots.len());
    if let Some(bar) = status_bar {
        chrome_views.push(bar);
    }
    chrome_views.push(dock);
    chrome_views.push(version_label);
    chrome_views.extend(page_dots.iter().copied());

    let main_run_loop: id = msg_class![env; NSRunLoop mainRunLoop];
    if show_lock_screen && picker_settings.show_lock_screen {
        run_lock_screen(
            env,
            main_run_loop,
            delegate,
            main_view,
            app_frame.size,
            icon_grid_stuff.as_ref(),
            status_bar,
            status_time_label,
            dock,
            version_label,
            &page_dots,
        );
        if let Some(ref grid) = icon_grid_stuff {
            play_home_screen_unlock_animation(
                env,
                main_run_loop,
                delegate,
                grid,
                app_frame.size,
                wallpaper_view,
                &chrome_views,
            );
            PENDING_ARROW_KEY.store(0, Ordering::Relaxed);
        }
    }
    // If an app is picked, this loop returns. If the user quits touchHLE, the
    // process exits.
    let app_path = loop {
        run_run_loop_single_iteration(env, main_run_loop);

        if status_time_updated_at.elapsed() >= Duration::from_secs(10) {
            status_time_updated_at = Instant::now();
            let now = status_bar_time_string();
            if now != status_time {
                status_time = now;
                if let Some(label) = status_time_label {
                    let text = ns_string::from_rust_string(env, status_time.clone());
                    () = msg![env; label setText:text];
                    release(env, text);
                }
            }
        }

        // Keep the battery indicator in sync with the real battery level. The
        // refresh itself must happen on the SDL thread on Android (SDL's
        // battery query uses JNI), and the picker runs on a coroutine stack.
        if let Some(charge) = status_charge_view {
            if status_battery_updated_at.elapsed() >= Duration::from_secs(30) {
                status_battery_updated_at = Instant::now();
                if env.window.is_some() {
                    env.on_parent_stack_in_coroutine(|_, _| crate::window::refresh_battery_cache());
                }
                update_battery_indicator(env, charge, crate::window::cached_battery_status());
            }
        }

        // Left/right arrow keys switch pages, like a swipe does.
        let arrow = PENDING_ARROW_KEY.swap(0, Ordering::Relaxed);
        if arrow != 0 {
            if let Some(grid) = icon_grid_stuff.as_mut() {
                let settings_view = settings.main_view;
                let settings_open: bool = !msg![env; settings_view isHidden];
                let new_page = if arrow > 0 {
                    current_page + 1
                } else {
                    current_page.wrapping_sub(1)
                };
                if !settings_open && pending_delete.is_none() && new_page < grid.pages.len() {
                    slide_to_page(
                        env,
                        main_run_loop,
                        delegate,
                        grid,
                        apps.as_mut().unwrap(),
                        new_page,
                        arrow as CGFloat,
                    );
                    current_page = new_page;
                    update_page_dots(env, &page_dots, current_page);
                }
            }
        }

        // Swipes are recognized when the finger lifts, so they can end on an
        // icon without opening it.
        let (swipe_left, swipe_right) = {
            let host_obj = env.objc.borrow_mut::<AppPickerDelegateHostObject>(delegate);
            (
                std::mem::take(&mut host_obj.swipe_left),
                std::mem::take(&mut host_obj.swipe_right),
            )
        };
        if swipe_left || swipe_right {
            {
                let host_obj = env.objc.borrow_mut::<AppPickerDelegateHostObject>(delegate);
                host_obj.icon_tapped = nil;
                host_obj.icon_touch_down = nil;
                host_obj.icon_touch_cancelled = false;
                host_obj.add_ipa = false;
                host_obj.settings_show = false;
                host_obj.settings_hide = false;
            }
            press = None;
            long_pressed_icon = None;
            if let Some(grid) = icon_grid_stuff.as_mut() {
                let settings_view = settings.main_view;
                let settings_open: bool = !msg![env; settings_view isHidden];
                let new_page = if swipe_left {
                    current_page + 1
                } else {
                    current_page.saturating_sub(1)
                };
                if !settings_open && pending_delete.is_none() && new_page < grid.pages.len() {
                    if new_page != current_page {
                        slide_to_page(
                            env,
                            main_run_loop,
                            delegate,
                            grid,
                            apps.as_mut().unwrap(),
                            new_page,
                            if swipe_left { 1.0 } else { -1.0 },
                        );
                        current_page = new_page;
                        update_page_dots(env, &page_dots, current_page);
                    }
                }
            }
            continue;
        }

        let (touch_down_icon, touch_cancelled, icon_tapped) = {
            let host_obj = env.objc.borrow_mut::<AppPickerDelegateHostObject>(delegate);
            (
                std::mem::take(&mut host_obj.icon_touch_down),
                std::mem::take(&mut host_obj.icon_touch_cancelled),
                std::mem::take(&mut host_obj.icon_tapped),
            )
        };
        if touch_down_icon != nil {
            press = Some((touch_down_icon, Instant::now()));
            long_pressed_icon = None;
        }
        if touch_cancelled {
            press = None;
        }
        // Holding an app icon long enough opens its delete dialog, as in the
        // iOS icon menu. The dialog is shown while the finger is still down.
        if let Some((button, pressed_at)) = press {
            if pressed_at.elapsed() >= APP_LONG_PRESS_DURATION {
                press = None;
                let app_idx = icon_grid_stuff
                    .as_ref()
                    .and_then(|grid| grid.icon_map.get(&button).copied());
                let name = app_idx.and_then(|app_idx| {
                    apps.as_ref()
                        .ok()
                        .and_then(|list| list.get(app_idx))
                        .map(|app| app.display_name.clone())
                });
                if let (Some(app_idx), Some(name)) = (app_idx, name) {
                    long_pressed_icon = Some(button);
                    pending_delete = Some(app_idx);
                    show_delete_dialog(
                        env,
                        main_run_loop,
                        delegate,
                        &delete_dialog,
                        &name,
                    );
                }
            }
        }
        if icon_tapped != nil {
            press = None;
            // The release that ends a long press is not a tap.
            if long_pressed_icon.take() == Some(icon_tapped) {
                continue;
            }
            match icon_grid_stuff
                .as_ref()
                .unwrap()
                .icon_map
                .get(&icon_tapped)
                .copied()
            {
                Some(app_idx) => {
                    let app_path = apps.as_ref().unwrap()[app_idx].path.clone();
                    remember_last_app_path(&app_path);
                    echo!("Picked: {}", app_path.display());
                    // iOS-style launch: the icon travels to the middle of the
                    // screen while the screen fades to black.
                    let icon_image: id = msg![env; icon_tapped currentImage];
                    let icon_frame: CGRect = msg![env; icon_tapped frame];
                    play_app_launch_animation(
                        env,
                        main_run_loop,
                        delegate,
                        main_view,
                        app_frame.size,
                        icon_frame,
                        icon_image,
                    );
                    break app_path;
                }
                None => (), // Tapped on a black space
            }
            continue;
        }
        let status_accessory_tapped = {
            let host = env
                .objc
                .borrow_mut::<AppPickerDelegateHostObject>(delegate);
            std::mem::take(&mut host.status_accessory_tapped)
        };
        if status_accessory_tapped {
            press = None;
            long_pressed_icon = None;
            let downloaded = access_prompt::run_access_prompt(
                env,
                main_run_loop,
                delegate,
                main_view,
                app_frame,
            ) && archive_store::run_archive_store(
                env,
                main_run_loop,
                delegate,
                main_view,
                app_frame,
                &apps_dir,
            );
            // Both screens use nested event loops. Do not let their touches or
            // swipes leak back into the home screen.
            {
                let host = env
                    .objc
                    .borrow_mut::<AppPickerDelegateHostObject>(delegate);
                host.icon_tapped = nil;
                host.icon_touch_down = nil;
                host.icon_touch_cancelled = false;
                host.swipe_left = false;
                host.swipe_right = false;
                host.status_accessory_tapped = false;
            }
            PENDING_ARROW_KEY.store(0, Ordering::Relaxed);
            press = None;
            long_pressed_icon = None;
            if downloaded {
                awaited_ipa = None;
                if let Some(grid) = icon_grid_stuff.as_mut() {
                    let page_count = reload_app_grid(
                        env,
                        grid,
                        &mut apps,
                        &mut current_page,
                        &apps_dir,
                    );
                    rebuild_page_dots(
                        env,
                        main_view,
                        app_frame.size,
                        &mut page_dots,
                        page_count,
                    );
                    update_page_dots(env, &page_dots, current_page);
                }
            }
            continue;
        }

        let host_obj = env.objc.borrow_mut::<AppPickerDelegateHostObject>(delegate);
        if std::mem::take(&mut host_obj.recent_app_tapped) {
            if let Some((path, _, _)) = recent_app.as_ref() {
                echo!("Picked last launched app: {}", path.display());
                break path.clone();
            }
        }
        if std::mem::take(&mut host_obj.add_ipa) {
            // Snapshot the .ipa files that exist right now, so that when the
            // system file picker finishes, we can detect the new file and
            // refresh the app grid.
            awaited_ipa = Some(IpaWatch {
                last_seen: list_top_level_ipa_files(&apps_dir),
                dirty: false,
                last_change: None,
            });
            // MainActivity (Android) opens the system file picker and copies
            // the picked file into the apps directory. On other platforms,
            // the apps directory is opened in the file manager instead.
            if let Err(e) = crate::window::launch_ipa_picker(env) {
                echo!("Couldn't open IPA picker: {}", e);
            }
        } else if std::mem::take(&mut host_obj.settings_show) {
            show_settings_screen(env, main_run_loop, delegate, &settings);
        } else if std::mem::take(&mut host_obj.settings_hide) {
            if device_state.open {
                device_state.open = false;
                refresh_dropdown(
                    env,
                    &settings.device_dropdown,
                    &device_model_label_for_tag(device_state.selected),
                    &device_state,
                );
            }
            if language_state.open {
                language_state.open = false;
                refresh_dropdown(
                    env,
                    &settings.language_dropdown,
                    &language_label_for_tag(language_state.selected),
                    &language_state,
                );
            }
            if memory_limit_state.open {
                memory_limit_state.open = false;
                refresh_dropdown(
                    env,
                    &settings.memory_limit_dropdown,
                    &memory_limit_label(quick_options_memory_limit_mb),
                    &memory_limit_state,
                );
            }
            hide_settings_screen(env, main_run_loop, delegate, &settings);
        } else if std::mem::take(&mut host_obj.scale_hack_default) {
            quick_options_scale_hack = None;
            update_scale_hack_buttons(env, &settings.scale_hack_buttons, quick_options_scale_hack);
        } else if std::mem::take(&mut host_obj.scale_hack1) {
            quick_options_scale_hack = NonZeroU32::new(1);
            update_scale_hack_buttons(env, &settings.scale_hack_buttons, quick_options_scale_hack);
        } else if std::mem::take(&mut host_obj.scale_hack2) {
            quick_options_scale_hack = NonZeroU32::new(2);
            update_scale_hack_buttons(env, &settings.scale_hack_buttons, quick_options_scale_hack);
        } else if std::mem::take(&mut host_obj.scale_hack3) {
            quick_options_scale_hack = NonZeroU32::new(3);
            update_scale_hack_buttons(env, &settings.scale_hack_buttons, quick_options_scale_hack);
        } else if std::mem::take(&mut host_obj.scale_hack4) {
            quick_options_scale_hack = NonZeroU32::new(4);
            update_scale_hack_buttons(env, &settings.scale_hack_buttons, quick_options_scale_hack);
        } else if std::mem::take(&mut host_obj.orientation_default) {
            quick_options_orientation = None;
            update_orientation_buttons(
                env,
                &settings.orientation_buttons,
                quick_options_orientation,
            );
        } else if std::mem::take(&mut host_obj.orientation_landscape_left) {
            quick_options_orientation = Some(DeviceOrientation::LandscapeLeft);
            update_orientation_buttons(
                env,
                &settings.orientation_buttons,
                quick_options_orientation,
            );
        } else if std::mem::take(&mut host_obj.orientation_landscape_right) {
            quick_options_orientation = Some(DeviceOrientation::LandscapeRight);
            update_orientation_buttons(
                env,
                &settings.orientation_buttons,
                quick_options_orientation,
            );
        } else if std::mem::take(&mut host_obj.orientation_portrait_upside_down) {
            quick_options_orientation = Some(DeviceOrientation::PortraitUpsideDown);
            update_orientation_buttons(
                env,
                &settings.orientation_buttons,
                quick_options_orientation,
            );
        } else if std::mem::take(&mut host_obj.fps_limit_default) {
            quick_options_fps_limit = None;
            update_fps_limit_buttons(env, &settings.fps_limit_buttons, quick_options_fps_limit);
        } else if std::mem::take(&mut host_obj.fps_limit30) {
            quick_options_fps_limit = Some(FpsLimit::Fps30);
            update_fps_limit_buttons(env, &settings.fps_limit_buttons, quick_options_fps_limit);
        } else if std::mem::take(&mut host_obj.fps_limit60) {
            quick_options_fps_limit = Some(FpsLimit::Fps60);
            update_fps_limit_buttons(env, &settings.fps_limit_buttons, quick_options_fps_limit);
        } else if std::mem::take(&mut host_obj.fps_limit_off) {
            quick_options_fps_limit = Some(FpsLimit::Off);
            update_fps_limit_buttons(env, &settings.fps_limit_buttons, quick_options_fps_limit);
        } else if let Some(tag) = std::mem::take(&mut host_obj.memory_limit_tag) {
            memory_limit_state.selected = Some(tag);
            memory_limit_state.open = false;
            quick_options_memory_limit_mb = memory_limit_choice(Some(tag));
            refresh_dropdown(
                env,
                &settings.memory_limit_dropdown,
                &memory_limit_label(quick_options_memory_limit_mb),
                &memory_limit_state,
            );
        } else if std::mem::take(&mut host_obj.memory_limit_toggle) {
            memory_limit_state.open = !memory_limit_state.open;
            refresh_dropdown(
                env,
                &settings.memory_limit_dropdown,
                &memory_limit_label(quick_options_memory_limit_mb),
                &memory_limit_state,
            );
        } else if let Some(position) = std::mem::take(&mut host_obj.memory_limit_scroll_to) {
            memory_limit_state.scroll =
                dropdown_scroll_at(position, settings.memory_limit_dropdown.items.len());
            refresh_dropdown(
                env,
                &settings.memory_limit_dropdown,
                &memory_limit_label(quick_options_memory_limit_mb),
                &memory_limit_state,
            );
        } else if let Some(tag) = std::mem::take(&mut host_obj.device_model_tag) {
            device_state.selected = Some(tag);
            device_state.open = false;
            refresh_dropdown(
                env,
                &settings.device_dropdown,
                &device_model_label_for_tag(device_state.selected),
                &device_state,
            );
        } else if std::mem::take(&mut host_obj.device_model_toggle) {
            device_state.open = !device_state.open;
            refresh_dropdown(
                env,
                &settings.device_dropdown,
                &device_model_label_for_tag(device_state.selected),
                &device_state,
            );
        } else if let Some(position) = std::mem::take(&mut host_obj.device_model_scroll_to) {
            device_state.scroll = dropdown_scroll_at(position, settings.device_dropdown.items.len());
            refresh_dropdown(
                env,
                &settings.device_dropdown,
                &device_model_label_for_tag(device_state.selected),
                &device_state,
            );
        } else if let Some(tag) = std::mem::take(&mut host_obj.language_tag) {
            language_state.selected = Some(tag);
            language_state.open = false;
            refresh_dropdown(
                env,
                &settings.language_dropdown,
                &language_label_for_tag(language_state.selected),
                &language_state,
            );
        } else if std::mem::take(&mut host_obj.language_toggle) {
            language_state.open = !language_state.open;
            refresh_dropdown(
                env,
                &settings.language_dropdown,
                &language_label_for_tag(language_state.selected),
                &language_state,
            );
        } else if let Some(position) = std::mem::take(&mut host_obj.language_scroll_to) {
            language_state.scroll = dropdown_scroll_at(position, settings.language_dropdown.items.len());
            refresh_dropdown(
                env,
                &settings.language_dropdown,
                &language_label_for_tag(language_state.selected),
                &language_state,
            );
        } else if std::mem::take(&mut host_obj.delete_cancel) {
            pending_delete = None;
            hide_delete_dialog(env, main_run_loop, delegate, &delete_dialog);
        } else if std::mem::take(&mut host_obj.delete_confirm) {
            if let Some(app_idx) = pending_delete.take() {
                hide_delete_dialog(env, main_run_loop, delegate, &delete_dialog);
                let result = match apps.as_ref().ok().and_then(|list| {
                    list.get(app_idx).map(|app| delete_app(app, list.as_slice()))
                }) {
                    Some(res) => res,
                    None => Err("the app is no longer in the list".to_string()),
                };
                match result {
                    Ok(()) => {
                        if let Some(grid) = icon_grid_stuff.as_mut() {
                            let page_count =
                                reload_app_grid(env, grid, &mut apps, &mut current_page, &apps_dir);
                            rebuild_page_dots(
                                env,
                                main_view,
                                app_frame.size,
                                &mut page_dots,
                                page_count,
                            );
                            update_page_dots(env, &page_dots, current_page);
                        }
                    }
                    Err(e) => echo!("Couldn't delete app: {}", e),
                }
            }
        } else if let Some(enabled) = std::mem::take(&mut host_obj.analog_stick_tilt_controls) {
            quick_options_analog_stick_tilt_controls = enabled;
        } else if let Some(enabled) = std::mem::take(&mut host_obj.network) {
            quick_options_network = enabled;
        } else if let Some(enabled) = std::mem::take(&mut host_obj.game_center_authenticated) {
            picker_settings.game_center_authenticated = enabled;
        } else if let Some(enabled) = std::mem::take(&mut host_obj.cheat_engine) {
            quick_options_cheat_engine = enabled;
        } else if let Some(enabled) = std::mem::take(&mut host_obj.show_fps) {
            quick_options_show_fps = enabled;
            picker_settings.show_fps = enabled;
        } else if let Some(mode) = std::mem::take(&mut host_obj.overlay_mode) {
            quick_options_overlay_mode = mode;
            update_performance_overlay_buttons(
                env,
                &settings.overlay_mode_buttons,
                &settings.overlay_corner_buttons,
                quick_options_overlay_mode,
                quick_options_overlay_corner,
            );
            env.window_mut().set_performance_overlay_settings(
                quick_options_show_fps,
                quick_options_overlay_mode,
                quick_options_overlay_corner,
            );
        } else if let Some(corner) = std::mem::take(&mut host_obj.overlay_corner) {
            quick_options_overlay_corner = corner;
            update_performance_overlay_buttons(
                env,
                &settings.overlay_mode_buttons,
                &settings.overlay_corner_buttons,
                quick_options_overlay_mode,
                quick_options_overlay_corner,
            );
            env.window_mut().set_performance_overlay_settings(
                quick_options_show_fps,
                quick_options_overlay_mode,
                quick_options_overlay_corner,
            );
        } else if let Some(rotation_fix) = std::mem::take(&mut host_obj.rotation_fix) {
            quick_options_rotation_fix = Some(rotation_fix);
        } else if let Some(fix_movie_player) = std::mem::take(&mut host_obj.fix_movie_player) {
            quick_options_fix_movie_player = Some(fix_movie_player);
        } else if let Some(preserve_aspect_ratio) =
            std::mem::take(&mut host_obj.preserve_aspect_ratio)
        {
            quick_options_preserve_aspect_ratio = preserve_aspect_ratio;
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            env.window_mut()
                .set_preserve_aspect_ratio(preserve_aspect_ratio);
        } else if let Some(enabled) = std::mem::take(&mut host_obj.show_lock_screen) {
            quick_options_show_lock_screen = enabled;
        } else if let Some(force_composition) = std::mem::take(&mut host_obj.force_composition) {
            quick_options_force_composition_override = Some(force_composition);
        } else if let Some(gles_native) = std::mem::take(&mut host_obj.gles_native) {
            quick_options_gles_native = gles_native || !crate::window::angle_backend_available();
        } else if let Some(fullscreen) = std::mem::take(&mut host_obj.fullscreen) {
            quick_options_fullscreen = fullscreen;
        }

        let wheel = PENDING_MOUSE_WHEEL.swap(0, Ordering::Relaxed);
        if wheel != 0 {
            let language_open = language_state.open;
            let memory_limit_open = memory_limit_state.open;
            let dropdown = if language_open {
                Some((&mut language_state, &settings.language_dropdown))
            } else if device_state.open {
                Some((&mut device_state, &settings.device_dropdown))
            } else if memory_limit_open {
                Some((&mut memory_limit_state, &settings.memory_limit_dropdown))
            } else {
                None
            };
            if let Some((state, dropdown)) = dropdown {
                state.scroll = (state.scroll - wheel as isize)
                    .clamp(0, dropdown_max_scroll(dropdown.items.len()));
                let label = if language_open {
                    language_label_for_tag(state.selected)
                } else if memory_limit_open {
                    memory_limit_label(memory_limit_choice(state.selected))
                } else {
                    device_model_label_for_tag(state.selected)
                };
                refresh_dropdown(env, dropdown, &label, state);
            } else if !msg![env; (settings.main_view) isHidden] {
                let scroll = settings.scroll_view;
                let offset: CGPoint = msg![env; scroll contentOffset];
                let bounds: CGRect = msg![env; scroll bounds];
                let content_size: CGSize = msg![env; scroll contentSize];
                let max_y = (content_size.height - bounds.size.height).max(0.0);
                let new_y = (offset.y - wheel as CGFloat * 40.0).clamp(0.0, max_y);
                if new_y != offset.y {
                    () = msg![env; scroll setContentOffset:(CGPoint { x: offset.x, y: new_y })];
                }
            }
        }

        picker_settings.scale_hack = quick_options_scale_hack;
        picker_settings.orientation = quick_options_orientation;
        picker_settings.fullscreen = quick_options_fullscreen;
        picker_settings.preserve_aspect_ratio = quick_options_preserve_aspect_ratio;
        picker_settings.show_lock_screen = quick_options_show_lock_screen;
        picker_settings.gles_native = quick_options_gles_native;
        picker_settings.force_composition =
            quick_options_force_composition_override.unwrap_or(env.options.force_composition);
        picker_settings.overlay_mode = quick_options_overlay_mode;
        picker_settings.overlay_corner = quick_options_overlay_corner;
        picker_settings.rotation_fix = quick_options_rotation_fix;
        picker_settings.fix_movie_player = quick_options_fix_movie_player;
        picker_settings.fps_limit = quick_options_fps_limit;
        picker_settings.memory_limit_mb = quick_options_memory_limit_mb;
        picker_settings.cheat_engine = quick_options_cheat_engine;
        picker_settings.network = quick_options_network;
        picker_settings.analog_stick_tilt_controls = quick_options_analog_stick_tilt_controls;
        picker_settings.device_model_tag = device_state.selected;
        picker_settings.language_tag = language_state.selected;
        if picker_settings != last_saved_picker_settings {
            picker_settings.save();
            last_saved_picker_settings = picker_settings.clone();
        }

        // Detect .ipa files copied in by the "+" tile flow and refresh the
        // grid once the new file has finished copying (its size stops
        // changing and has stayed stable for a moment).
        if let Some(watch) = &mut awaited_ipa {
            let new_listing = list_top_level_ipa_files(&apps_dir);
            if new_listing != watch.last_seen {
                watch.last_seen = new_listing;
                watch.dirty = true;
                watch.last_change = Some(Instant::now());
            } else if watch.dirty
                && watch
                    .last_change
                    .is_some_and(|t| t.elapsed() >= IPA_COPY_SETTLE_TIME)
            {
                watch.dirty = false;
                if let Some(grid) = icon_grid_stuff.as_mut() {
                    let page_count =
                        reload_app_grid(env, grid, &mut apps, &mut current_page, &apps_dir);
                    rebuild_page_dots(
                        env,
                        main_view,
                        app_frame.size,
                        &mut page_dots,
                        page_count,
                    );
                    update_page_dots(env, &page_dots, current_page);
                }
            }
        }
    };

    // Apply user-specified overrides
    if let Some(scale_hack) = quick_options_scale_hack {
        option_args.push(format!("--scale-hack={}", scale_hack.get()));
    }
    if let Some(orientation) = quick_options_orientation {
        option_args.push(
            match orientation {
                DeviceOrientation::LandscapeLeft => "--landscape-left",
                DeviceOrientation::LandscapeRight => "--landscape-right",
                DeviceOrientation::PortraitUpsideDown => "--upside-down",
                _ => todo!(),
            }
            .to_string(),
        );
    }
    if quick_options_fullscreen {
        option_args.push("--fullscreen".to_string());
    } else {
        option_args.push("--no-fullscreen".to_string());
    }
    if show_fullscreen_row {
        option_args.push(
            if quick_options_preserve_aspect_ratio {
                "--preserve-aspect-ratio"
            } else {
                "--no-preserve-aspect-ratio"
            }
            .to_string(),
        );
    }
    if !quick_options_analog_stick_tilt_controls {
        option_args.push("--disable-analog-stick-tilt-controls".to_string());
    } else {
        option_args.push("--enable-analog-stick-tilt-controls".to_string());
    }
    if quick_options_network {
        option_args.push("--allow-network-access".to_string());
    } else {
        option_args.push("--no-network-access".to_string());
    }
    option_args.push(quick_options_trainer_argument(quick_options_cheat_engine).to_string());
    option_args.push(
        if picker_settings.game_center_authenticated {
            "--game-center-authenticated"
        } else {
            "--no-game-center-authenticated"
        }
        .to_string(),
    );

    env.window_mut().set_performance_overlay_settings(
        quick_options_show_fps,
        quick_options_overlay_mode,
        quick_options_overlay_corner,
    );
    option_args.push(format!(
        "--overlay={}",
        if quick_options_show_fps { "on" } else { "off" }
    ));
    option_args.push(format!(
        "--overlay-mode={}",
        quick_options_overlay_mode.short_name()
    ));
    option_args.push(format!(
        "--overlay-corner={}",
        quick_options_overlay_corner.short_name()
    ));

    if let Some(enabled) = quick_options_force_composition_override {
        option_args.push(quick_options_force_composition_argument(enabled).to_string());
    }

    option_args.extend(quick_options_limiter_arguments(
        quick_options_fps_limit,
        quick_options_memory_limit_mb,
    ));

    // The two compatibility fixes are only emitted when the Settings screen
    // actually set them (see `quick_options_fix_arguments`).
    option_args.extend(quick_options_fix_arguments(
        quick_options_rotation_fix,
        quick_options_fix_movie_player,
    ));
    // Always passed explicitly (like `--trainer`/`--no-trainer`): the base
    // default is now OFF, so the switch has to be able to turn the native
    // driver back on, and the picker's choice should win over any
    // `--gles-native`/`--no-gles-native` in the options files.
    option_args.push(quick_options_gles_native_argument(quick_options_gles_native).to_string());

    if let Some(tag) = device_state.selected {
        let tag = tag as NSInteger;
        if tag == DEVICE_TAG_DEFAULT {
            // No override — fall back to the app bundle / built-in default.
        } else if tag == DEVICE_TAG_AUTO {
            option_args.push("--device-family=auto".to_string());
        } else if let Some(family) = crate::window::DeviceFamily::for_tag(tag as usize) {
            option_args.push(format!("--device-family={}", family.option_name()));
        }
    }

    if let Some(arg) = language_argument(language_state.selected) {
        option_args.push(arg);
    }

    // Return the environment so some parts of it can be salvaged.
    (app_path, option_args)
}

/// Old-iOS home screen layout: a fixed 4×4 icon grid above a row of page dots,
/// and a translucent dock along the bottom edge.
const APP_PICKER_ICON_COLS: usize = 4;
const APP_PICKER_ICON_ROWS: usize = 4;
const APP_PICKER_GRID_TOP: CGFloat = 6.0;
const APP_PICKER_SIDE_MARGIN: CGFloat = 10.0;
const APP_PICKER_ICON_MAX_SIZE: CGFloat = 60.0;
const APP_PICKER_ICON_MIN_SIZE: CGFloat = 24.0;
const APP_PICKER_ICON_GAP_X_MIN: CGFloat = 10.0;
const APP_PICKER_ICON_GAP_X_MAX: CGFloat = 20.0;
const APP_PICKER_LABEL_HEIGHT: CGFloat = 14.0;
const APP_PICKER_LABEL_TOP_GAP: CGFloat = 2.0;
const APP_PICKER_ROW_GAP_MIN: CGFloat = 2.0;
const APP_PICKER_DOTS_ROW_HEIGHT: CGFloat = 18.0;
const APP_PICKER_DOCK_HEIGHT: CGFloat = 88.0;

const PAGE_DOT_SIZE: CGFloat = 7.0;
const PAGE_DOT_GAP: CGFloat = 9.0;

/// Side length of the bitmap used for app and dock icons. The on-screen size
/// is chosen separately by [app_picker_layout] so that the grid always fits.
const ICON_SIZE: CGSize = CGSize {
    width: 72.0,
    height: 72.0,
};

/// Vertical centre-line of the page-dot row, just above the dock.
fn app_picker_dots_top(app_height: CGFloat) -> CGFloat {
    app_picker_grid_bottom(app_height) + (APP_PICKER_DOTS_ROW_HEIGHT - PAGE_DOT_SIZE) / 2.0
}

/// The bottom edge of the icon grid: the page-dot/version row sits below it.
fn app_picker_grid_bottom(app_height: CGFloat) -> CGFloat {
    app_height - APP_PICKER_DOCK_HEIGHT - APP_PICKER_DOTS_ROW_HEIGHT
}

/// Geometry of the 4×4 icon grid.
#[derive(Clone, Copy, Debug)]
struct AppPickerLayout {
    /// Side length of one (square) icon cell.
    icon_size: CGFloat,
    /// Horizontal distance between icon cells.
    icon_gap_x: CGFloat,
    /// Vertical distance between icon rows.
    icon_gap_y: CGFloat,
    /// Top-left corner of the first icon cell.
    grid_origin: CGPoint,
    /// Size of an icon label, also used to space the rows.
    label_size: CGSize,
}

fn app_picker_layout(screen_width: CGFloat, screen_height: CGFloat) -> AppPickerLayout {
    let cols = APP_PICKER_ICON_COLS as CGFloat;
    let rows = APP_PICKER_ICON_ROWS as CGFloat;
    let inner_width = (screen_width - APP_PICKER_SIDE_MARGIN * 2.0).max(1.0);
    let row_chrome = APP_PICKER_LABEL_TOP_GAP + APP_PICKER_LABEL_HEIGHT;
    let available_height = (app_picker_grid_bottom(screen_height) - APP_PICKER_GRID_TOP).max(1.0);

    // The icons must fit both across (four columns) and down (four rows plus
    // labels). Small screens get smaller icons rather than fewer cells.
    let icon_by_width = (inner_width - APP_PICKER_ICON_GAP_X_MIN * (cols - 1.0)) / cols;
    let icon_by_height =
        (available_height - rows * row_chrome - APP_PICKER_ROW_GAP_MIN * (rows - 1.0)) / rows;
    let icon_size = icon_by_width
        .min(icon_by_height)
        .clamp(APP_PICKER_ICON_MIN_SIZE, APP_PICKER_ICON_MAX_SIZE)
        .floor();
    let icon_gap_x = ((inner_width - icon_size * cols) / (cols - 1.0))
        .clamp(0.0, APP_PICKER_ICON_GAP_X_MAX);
    let grid_width = icon_size * cols + icon_gap_x * (cols - 1.0);
    let cell_height = icon_size + row_chrome;
    let icon_gap_y = ((available_height - rows * cell_height) / (rows - 1.0))
        .max(APP_PICKER_ROW_GAP_MIN);

    AppPickerLayout {
        icon_size,
        icon_gap_x,
        icon_gap_y,
        grid_origin: CGPoint {
            x: (screen_width - grid_width) / 2.0,
            y: APP_PICKER_GRID_TOP,
        },
        label_size: CGSize {
            width: icon_size + icon_gap_x,
            height: APP_PICKER_LABEL_HEIGHT,
        },
    }
}

#[cfg(test)]
mod layout_tests {
    use super::*;

    #[test]
    fn classic_phone_picker_has_four_by_four_grid() {
        // A visible status bar leaves `UIScreen.applicationFrame` at 320x460.
        let layout = app_picker_layout(320.0, 460.0);
        assert_eq!(APP_PICKER_ICON_COLS, 4);
        assert_eq!(APP_PICKER_ICON_ROWS, 4);
        assert_eq!(layout.icon_size, 60.0);

        let last_row_bottom = layout.grid_origin.y
            + 4.0 * layout.icon_size
            + 3.0 * layout.icon_gap_y
            + 4.0 * (APP_PICKER_LABEL_TOP_GAP + APP_PICKER_LABEL_HEIGHT);
        assert!(last_row_bottom <= app_picker_grid_bottom(460.0) + 0.5);
    }

    #[test]
    fn pages_hold_one_gridful_of_apps() {
        assert_eq!(compute_pages(16, 0), vec![0..0]);
        assert_eq!(compute_pages(16, 16), vec![0..16]);
        assert_eq!(compute_pages(16, 17), vec![0..16, 16..17]);
    }
}

/// One full set of icon-grid cells on its own container view. Two of these
/// exist so that one page can slide out while the next slides in.
struct PageContainer {
    view: id,
    /// (icon button, name label) pairs.
    cells: Vec<(id, id)>,
}

struct IconGridStuff {
    containers: Vec<PageContainer>,
    /// Index into `containers` of the container currently on screen.
    visible: usize,
    /// Width of the picker screen, used by the page slide animation.
    screen_width: CGFloat,
    placeholder_icon: Option<id>,
    pages: Vec<std::ops::Range<usize>>,
    /// Maps an on-screen icon cell's button to the app it shows.
    icon_map: HashMap<id, usize>,
}

fn make_icon_grid(
    env: &mut Environment,
    delegate: id,
    main_view: id,
    app_frame: CGRect,
    total_app_count: usize,
) -> IconGridStuff {
    let layout = app_picker_layout(app_frame.size.width, app_frame.size.height);
    let num_cols = APP_PICKER_ICON_COLS;
    let num_rows = APP_PICKER_ICON_ROWS;
    let cell_step_x = layout.icon_size + layout.icon_gap_x;
    let cell_step_y =
        layout.icon_size + APP_PICKER_LABEL_TOP_GAP + layout.label_size.height + layout.icon_gap_y;

    // The icon image view is inset slightly so the rounded corners aren't
    // clipped.
    let icon_inset = (layout.icon_size * 0.03).max(0.0);

    let icon_tapped_sel = env.objc.lookup_selector("iconTapped:").unwrap();
    let icon_touch_down_sel = env.objc.lookup_selector("iconTouchDown:").unwrap();
    let icon_touch_cancelled_sel = env.objc.lookup_selector("iconTouchCancelled:").unwrap();

    let mut containers = Vec::new();
    for container_idx in 0..2 {
        // The container is a full-screen view, so moving it slides all its cells.
        let clear: id = msg_class![env; UIColor clearColor];
        let container = new_view(
            env,
            CGRect {
                origin: CGPoint { x: 0.0, y: 0.0 },
                size: app_frame.size,
            },
            clear,
        );
        if container_idx != 0 {
            () = msg![env; container setHidden:true];
        }
        () = msg![env; main_view addSubview:container];

        let mut cells = Vec::new();
        for i in 0..(num_cols * num_rows) {
            let col = i % num_cols;
            let row = i / num_cols;

            // Rounding is needed here to avoid a blurry or offset image.
            let icon_frame = CGRect {
                origin: CGPoint {
                    x: (layout.grid_origin.x + (col as CGFloat) * cell_step_x).round(),
                    y: (layout.grid_origin.y + (row as CGFloat) * cell_step_y).round(),
                },
                size: CGSize {
                    width: layout.icon_size,
                    height: layout.icon_size,
                },
            };
            let icon_button: id = msg_class![env; UIButton buttonWithType:UIButtonTypeCustom];
            () = msg![env; icon_button setFrame:icon_frame];
            let image_view: id = msg![env; icon_button imageView];
            let bounds: CGRect = msg![env; icon_button bounds];
            () = msg![env; image_view setFrame:(CGRect {
                origin: CGPoint {
                    x: icon_inset,
                    y: icon_inset,
                },
                size: CGSize {
                    width: (bounds.size.width - icon_inset * 2.0).max(1.0),
                    height: (bounds.size.height - icon_inset * 2.0).max(1.0),
                },
            })];
            let layer: id = msg![env; image_view layer];
            let gravity = ns_string::get_static_str(env, "resizeAspect");
            () = msg![env; layer setContentsGravity:gravity];
            () = msg![env; icon_button addTarget:delegate
                                          action:icon_tapped_sel
                                forControlEvents:UIControlEventTouchUpInside];
            // Finger-down starts a possible long press; leaving the icon or
            // lifting the finger outside it cancels that.
            () = msg![env; icon_button addTarget:delegate
                                          action:icon_touch_down_sel
                                forControlEvents:UIControlEventTouchDown];
            () = msg![env; icon_button addTarget:delegate
                                          action:icon_touch_cancelled_sel
                                forControlEvents:(UIControlEventTouchDragExit
                                    | UIControlEventTouchUpOutside
                                    | UIControlEventTouchCancel)];
            () = msg![env; container addSubview:icon_button];

            // Rounding is needed here to avoid blurry text.
            let label_frame = CGRect {
                origin: CGPoint {
                    x: (icon_frame.origin.x
                        - (layout.label_size.width - layout.icon_size) / 2.0)
                        .round(),
                    y: (icon_frame.origin.y + layout.icon_size + APP_PICKER_LABEL_TOP_GAP)
                        .round(),
                },
                size: layout.label_size,
            };
            let label: id = msg_class![env; UILabel alloc];
            let label: id = msg![env; label initWithFrame:label_frame];
            () = msg![env; label setTextAlignment:UITextAlignmentCenter];
            let font_size: CGFloat = layout.label_size.height - 3.0;
            let font: id = msg_class![env; UIFont boldSystemFontOfSize:font_size];
            () = msg![env; label setFont:font];
            // White with a soft shadow, like the old iOS home screen.
            let text_color: id = msg_class![env; UIColor whiteColor];
            () = msg![env; label setTextColor:text_color];
            let shadow_color = ui_color(env, 0.0, 0.0, 0.0, 0.7);
            () = msg![env; label setShadowColor:shadow_color];
            () = msg![env; label setShadowOffset:(CGSize {
                width: 0.0,
                height: 1.0,
            })];
            let bg_color: id = msg_class![env; UIColor clearColor];
            () = msg![env; label setBackgroundColor:bg_color];
            () = msg![env; container addSubview:label];

            cells.push((icon_button, label));
        }
        containers.push(PageContainer {
            view: container,
            cells,
        });
    }

    let total_slots = containers[0].cells.len();
    let pages = compute_pages(total_slots, total_app_count);

    IconGridStuff {
        containers,
        visible: 0,
        screen_width: app_frame.size.width,
        placeholder_icon: None,
        pages,
        icon_map: HashMap::new(),
    }
}

/// Work out which apps go on each page of the icon grid. Every page is a full
/// 4×4 grid; the "+" tile and Settings live in the dock instead of taking
/// grid slots.
fn compute_pages(total_slots: usize, total_app_count: usize) -> Vec<std::ops::Range<usize>> {
    let total_slots = total_slots.max(1);
    let page_count = ((total_app_count + total_slots - 1) / total_slots).max(1);
    (0..page_count)
        .map(|page_idx| {
            let start = page_idx * total_slots;
            start..(start + total_slots).min(total_app_count)
        })
        .collect()
}

/// Renders a rounded, glossed app icon with the given background colour.
/// `draw` paints on top of the background at bitmap resolution, in
/// top-left-origin coordinates (y grows downwards).
fn make_icon(
    env: &mut Environment,
    bg_color: (CGFloat, CGFloat, CGFloat, CGFloat),
    draw: impl FnOnce(&mut Environment, id),
) -> id {
    let ui_scale = env.options.ui_scale.get() as CGFloat;
    let color_space = CGColorSpaceCreateDeviceRGB(env);
    let context = CGBitmapContextCreate(
        env,
        Ptr::null(),
        (ICON_SIZE.width as u32).saturating_mul(env.options.ui_scale.get()),
        (ICON_SIZE.height as u32).saturating_mul(env.options.ui_scale.get()),
        8,
        4 * (ICON_SIZE.width as u32).saturating_mul(env.options.ui_scale.get()),
        color_space,
        kCGImageAlphaPremultipliedLast,
    );
    UIGraphicsPushContext(env, context);

    let scaled_width = ICON_SIZE.width * ui_scale;
    let scaled_height = ICON_SIZE.height * ui_scale;

    // The bitmap's rows are stored bottom-up; flip the transform so the
    // background and `draw` can use screen-like top-down coordinates.
    CGContextTranslateCTM(env, context, 0.0, scaled_height);
    CGContextScaleCTM(env, context, 1.0, -1.0);

    let (r, g, b, a) = bg_color;
    CGContextSetRGBFillColor(env, context, r, g, b, a);
    CGContextFillRect(
        env,
        context,
        CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize {
                width: scaled_width,
                height: scaled_height,
            },
        },
    );

    draw(env, context);

    UIGraphicsPopContext(env);

    let cg_image = CGBitmapContextCreateImage(env, context);
    // Same gloss and (resolution-scaled) corner radius as `Bundle::load_icon()`.
    cg_image::borrow_image_mut(&mut env.objc, cg_image)
        .round_icon_corners(/* add_sheen: */ true);
    CGContextRelease(env, context);

    let ui_image: id = msg_class![env; UIImage imageWithCGImage:cg_image];
    release(env, cg_image);

    ui_image
}

/// Builds an icon with white text in the middle, e.g. "+" or "WWW".
fn make_icon_from_text(
    env: &mut Environment,
    text: &str,
    font_size: CGFloat,
    baseline_offset: CGFloat,
    bg_color: (CGFloat, CGFloat, CGFloat, CGFloat),
) -> id {
    make_icon(env, bg_color, |env, context| {
        let ui_scale = env.options.ui_scale.get() as CGFloat;
        let scaled_width = ICON_SIZE.width * ui_scale;
        let scaled_height = ICON_SIZE.height * ui_scale;
        let font: id = msg_class![env; UIFont systemFontOfSize:(font_size * ui_scale)];
        let string: id = ns_string::from_rust_string(env, text.to_string());
        let size: CGSize = msg![env; string sizeWithFont:font];
        CGContextSetRGBFillColor(env, context, 1.0, 1.0, 1.0, 1.0); // white
        let origin = CGPoint {
            x: scaled_width / 2.0 - size.width / 2.0,
            y: scaled_height / 2.0 - size.height / 2.0 + baseline_offset * ui_scale,
        };
        let _: CGSize = msg![env; string drawAtPoint:origin withFont:font];
        release(env, string);
    })
}

/// Fills one page container (`container_idx`) of the icon grid with the apps
/// of `page_idx`. The other container is left alone, so it can keep showing
/// the previous page while a slide animation runs.
fn update_icon_grid(
    env: &mut Environment,
    icon_grid_stuff: &mut IconGridStuff,
    apps: &mut [AppInfo],
    page_idx: usize,
    container_idx: usize,
) {
    let cells = icon_grid_stuff.containers[container_idx].cells.clone();
    for &(icon_button, _) in &cells {
        icon_grid_stuff.icon_map.remove(&icon_button);
    }

    let app_idx_range = icon_grid_stuff.pages[page_idx].clone();
    let mut icon_iter = cells.iter();

    for app_idx in app_idx_range {
        let app = &mut apps[app_idx];

        let &(icon_button, label) = icon_iter.next().unwrap();

        if let Some(icon) = app.icon.take() {
            let image = cg_image::from_image(env, icon);
            let image: id = msg_class![env; UIImage imageWithCGImage:image];
            app.icon_ui_image = Some(image);
        }

        let image = app.icon_ui_image.unwrap_or_else(|| {
            *icon_grid_stuff.placeholder_icon.get_or_insert_with(|| {
                make_icon_from_text(env, "?", 40.0, 0.0, (0.5, 0.5, 0.5, 1.0))
            })
        });
        () = msg![env; icon_button setImage:image forState:UIControlStateNormal];

        let text = *app
            .display_name_ns_string
            .get_or_insert_with(|| ns_string::from_rust_string(env, app.display_name.clone()));
        () = msg![env; label setText:text];

        icon_grid_stuff.icon_map.insert(icon_button, app_idx);
    }

    // Blank the unused cells at the end of the last page.
    for &(icon_button, label) in icon_iter {
        () = msg![env; icon_button setImage:nil forState:UIControlStateNormal];
        () = msg![env; label setText:(ns_string::get_static_str(env, ""))];
    }
}

/// Construct and display the iPhone OS 4 lock screen over the picker wallpaper.
/// A rightward swipe dismisses the lock screen; a pulsing label recreates the
/// animated “slide to unlock” treatment.
fn run_lock_screen(
    env: &mut Environment,
    run_loop: id,
    delegate: id,
    parent: id,
    size: CGSize,
    grid: Option<&IconGridStuff>,
    status_bar: Option<id>,
    status_time_label: Option<id>,
    dock: id,
    version_label: id,
    page_dots: &[id],
) {
    if let Some(grid) = grid {
        for container in &grid.containers {
            let container_view = container.view;
            () = msg![env; container_view setHidden:true];
        }
    }
    () = msg![env; dock setHidden:true];
    () = msg![env; version_label setHidden:true];
    for &dot in page_dots {
        () = msg![env; dot setHidden:true];
    }
    let clear: id = msg_class![env; UIColor clearColor];
    let lock = new_view(env, rect(0.0, 0.0, size.width, size.height), clear);
    if let Some(bar) = status_bar {
        let status_glass = ui_color(env, 0.02, 0.025, 0.04, 0.36);
        () = msg![env; bar setBackgroundColor:status_glass];
    }

    // Layered translucent bands give the clock panel the soft glass gradient
    // of the iOS 4 lock screen while keeping the wallpaper visible beneath it.
    let top = new_view(env, rect(0.0, 0.0, size.width, 138.0), clear);
    const GLASS_STEPS: usize = 48;
    for band in 0..GLASS_STEPS {
        let t = band as CGFloat / (GLASS_STEPS - 1) as CGFloat;
        let y = 138.0 * t;
        let band_height = 138.0 / GLASS_STEPS as CGFloat + 0.5;
        let tint = ui_color(env, 0.025, 0.035, 0.06, 0.30 - 0.18 * t);
        let strip = new_view(env, rect(0.0, y, size.width, band_height), tint);
        () = msg![env; top addSubview:strip];
    }
    () = msg![env; lock addSubview:top];
    let header_edge_color = ui_color(env, 1.0, 1.0, 1.0, 0.18);
    let header_edge = new_view(env, rect(0.0, 137.0, size.width, 1.0), header_edge_color);
    () = msg![env; lock addSubview:header_edge];
    let white: id = msg_class![env; UIColor whiteColor];
    let clock = new_label(env, rect(0.0, 12.0, size.width, 82.0), &lock_screen_time_string(), 68.0, false, white);
    () = msg![env; clock setTextAlignment:UITextAlignmentCenter];
    let shadow = ui_color(env, 0.0, 0.0, 0.0, 0.7);
    () = msg![env; clock setShadowColor:shadow];
    () = msg![env; clock setShadowOffset:(CGSize { width: 0.0, height: 2.0 })];
    () = msg![env; lock addSubview:clock];
    let date = new_label(env, rect(8.0, 91.0, size.width - 16.0, 32.0), &lock_screen_date_string(), 22.0, false, white);
    () = msg![env; date setTextAlignment:UITextAlignmentCenter];
    () = msg![env; date setShadowColor:shadow];
    () = msg![env; date setShadowOffset:(CGSize { width: 0.0, height: 1.0 })];
    () = msg![env; lock addSubview:date];

    let bottom = size.height - 116.0;
    let shade = new_view(env, rect(0.0, bottom - 24.0, size.width, 140.0), clear);
    for band in 0..GLASS_STEPS {
        let t = band as CGFloat / (GLASS_STEPS - 1) as CGFloat;
        let y = 140.0 * t;
        let band_height = 140.0 / GLASS_STEPS as CGFloat + 0.5;
        let tint = ui_color(env, 0.015, 0.02, 0.035, 0.035 + 0.20 * t);
        let strip = new_view(env, rect(0.0, y, size.width, band_height), tint);
        () = msg![env; shade addSubview:strip];
    }
    () = msg![env; lock addSubview:shade];
    // Raised metal bezel around the black glass slider, like the inset 3D
    // capsule on the reference iPhone OS lock screen.
    let bezel_color = ui_color(env, 0.25, 0.26, 0.28, 1.0);
    let bezel = new_view(env, rect(17.0, bottom + 10.0, size.width - 34.0, 68.0), bezel_color);
    let bezel_layer: id = msg![env; bezel layer];
    () = msg![env; bezel_layer setCornerRadius:(20.0 as CGFloat)];
    let bezel_edge = ui_color(env, 0.62, 0.64, 0.67, 1.0);
    let bezel_edge_cg: id = msg![env; bezel_edge CGColor];
    () = msg![env; bezel_layer setBorderColor:bezel_edge_cg];
    () = msg![env; bezel_layer setBorderWidth:(1.5 as CGFloat)];
    () = msg![env; lock addSubview:bezel];

    let track_color = ui_color(env, 0.008, 0.012, 0.02, 0.90);
    let track = new_view(env, rect(21.0, bottom + 13.0, size.width - 42.0, 62.0), track_color);
    let track_layer: id = msg![env; track layer];
    () = msg![env; track_layer setCornerRadius:(18.0 as CGFloat)];
    () = msg![env; track setClipsToBounds:true];
    let track_edge = ui_color(env, 0.0, 0.0, 0.0, 0.85);
    let track_edge_cg: id = msg![env; track_edge CGColor];
    () = msg![env; track_layer setBorderColor:track_edge_cg];
    () = msg![env; track_layer setBorderWidth:(1.0 as CGFloat)];
    () = msg![env; lock addSubview:track];
    // A single restrained top glint keeps the capsule glossy without visible
    // striping on renderers that do not composite translucent bands smoothly.
    let track_glint_color = ui_color(env, 1.0, 1.0, 1.0, 0.11);
    let track_glint = new_view(env, rect(14.0, 2.0, size.width - 70.0, 1.0), track_glint_color);
    () = msg![env; track addSubview:track_glint];
    let track_width = size.width - 42.0;
    let thumb_width = (track_width * 0.245).clamp(76.0, 100.0);
    let label_x = thumb_width + 24.0;
    let label_width = (track_width - label_x - 12.0).max(0.0);
    // Render one label per glyph. The custom renderer does not reliably clip
    // duplicate full-string labels, so animating individual glyph alpha gives
    // a clean left-to-right sheen without any overprinted text.
    let shimmer_text = "slide to unlock";
    let font: id = msg_class![env; UIFont systemFontOfSize:(25.0 as CGFloat)];
    let whole_text = ns_string::from_rust_string(env, shimmer_text.to_string());
    let whole_size: CGSize = msg![env; whole_text sizeWithFont:font];
    release(env, whole_text);
    let mut shimmer_letters = Vec::new();
    let mut prefix = String::new();
    let mut prefix_width = 0.0 as CGFloat;
    let text_start = label_x + (label_width - whole_size.width) * 0.5;
    for glyph in shimmer_text.chars() {
        prefix.push(glyph);
        let prefix_string = ns_string::from_rust_string(env, prefix.clone());
        let size: CGSize = msg![env; prefix_string sizeWithFont:font];
        release(env, prefix_string);
        let glyph_width = (size.width - prefix_width).max(1.0);
        let glyph_text = glyph.to_string();
        let color = ui_color(env, 1.0, 1.0, 1.0, 0.86);
        let letter = new_label(
            env,
            rect(text_start + prefix_width, 0.0, glyph_width + 0.5, 62.0),
            &glyph_text,
            25.0,
            false,
            color,
        );
        () = msg![env; letter setTextAlignment:UITextAlignmentCenter];
        () = msg![env; track addSubview:letter];
        shimmer_letters.push(letter);
        prefix_width = size.width;
    }
    // The classic iOS thumb is a wide silver pill, not a square tile.
    // Keep its proportion tied to the track so it scales with device width.
    let thumb_height = 54.0;
    let thumb_image = make_lock_thumb_image(env);
    let knob: id = msg_class![env; UIImageView alloc];
    let knob: id = msg![env; knob initWithImage:thumb_image];
    () = msg![env; knob setFrame:(rect(4.0, 4.0, thumb_width, thumb_height))];
    () = msg![env; track addSubview:knob];
    let arrow_image = make_lock_arrow_image(env);
    let arrow_view: id = msg_class![env; UIImageView alloc];
    let arrow_view: id = msg![env; arrow_view initWithImage:arrow_image];
    let arrow_width = 52.0;
    let arrow_height = 40.0;
    () = msg![env; arrow_view setFrame:(rect(
        (thumb_width - arrow_width) * 0.5,
        (thumb_height - arrow_height) * 0.5,
        arrow_width,
        arrow_height,
    ))];
    () = msg![env; knob addSubview:arrow_view];

    // A nearly transparent UISlider is the actual touch surface. Its value
    // drives the visible thumb continuously, so the arrow follows the finger
    // instead of only reacting after a swipe has already ended.
    let slider: id = msg_class![env; UISlider alloc];
    // Align UISlider's touch-value endpoints with the centers of the wide
    // custom thumb so the visual knob stays directly under the finger.
    let slider_start = 4.0 + thumb_width * 0.5 - 11.5;
    let slider_width = track_width - slider_start * 2.0;
    let slider: id = msg![env; slider initWithFrame:(rect(
        slider_start,
        0.0,
        slider_width,
        62.0,
    ))];
    () = msg![env; slider setValue:0.0f32];
    // Keep the control itself fully hit-testable, but hide its stock blue/gray
    // track and thumb so only our custom glass slider is visible.
    let slider_parts: id = msg![env; slider subviews];
    let slider_part_count: usize = msg![env; slider_parts count];
    for index in 0..slider_part_count {
        let part: id = msg![env; slider_parts objectAtIndex:(index as usize)];
        () = msg![env; part setAlpha:(0.0 as CGFloat)];
    }
    let slider_selector = env.objc.lookup_selector("lockSliderChanged:").unwrap();
    () = msg![env; slider addTarget:delegate
                              action:slider_selector
                    forControlEvents:UIControlEventValueChanged];
    () = msg![env; track addSubview:slider];
    () = msg![env; parent addSubview:lock];

    {
        let mut host = env.objc.borrow_mut::<AppPickerDelegateHostObject>(delegate);
        host.lock_screen_unlocked = false;
        host.lock_slider_progress = 0.0;
    }
    let started = Instant::now();
    let mut clock_updated = Instant::now();
    let mut shimmer_updated = Instant::now() - Duration::from_millis(34);
    let mut last_knob_progress = -1.0f32;
    loop {
        run_run_loop_single_iteration(env, run_loop);
        let (unlocked, mut progress) = {
            let host = env.objc.borrow::<AppPickerDelegateHostObject>(delegate);
            (host.lock_screen_unlocked, host.lock_slider_progress)
        };
        if unlocked { break; }
        let tracking: bool = msg![env; slider tracking];
        if !tracking && progress > 0.0 {
            // A partial swipe eases back to the start; a completed one is
            // handled above by the unlock transition.
            progress = (progress - 0.055).max(0.0);
            () = msg![env; slider setValue:progress];
            env.objc.borrow_mut::<AppPickerDelegateHostObject>(delegate).lock_slider_progress = progress;
        }
        if shimmer_updated.elapsed() >= Duration::from_millis(33) {
            shimmer_updated = Instant::now();
            let elapsed = started.elapsed().as_secs_f64();
            let sweep = (elapsed % 2.4) / 2.4 * (shimmer_letters.len() as f64 + 4.0) - 2.0;
            for (index, &letter) in shimmer_letters.iter().enumerate() {
                let distance = (index as f64 - sweep).abs();
                let glow = (1.0 - distance / 2.2).clamp(0.0, 1.0) as CGFloat;
                () = msg![env; letter setAlpha:(0.58 + glow * 0.42)];
            }
        }
        if progress != last_knob_progress {
            last_knob_progress = progress;
            let mut knob_frame: CGRect = msg![env; knob frame];
            let travel = (track_width - thumb_width - 8.0).max(0.0);
            knob_frame.origin.x = 4.0 + travel * progress as CGFloat;
            () = msg![env; knob setFrame:knob_frame];
        }
        if clock_updated.elapsed() >= Duration::from_secs(1) {
            clock_updated = Instant::now();
            // Keep both clocks on the same timestamp while the lock screen is
            // modal; the outer picker event loop is paused until unlock.
            let text = ns_string::from_rust_string(env, lock_screen_time_string());
            () = msg![env; clock setText:text];
            if let Some(status_time_label) = status_time_label {
                () = msg![env; status_time_label setText:text];
            }
            release(env, text);
        }
    }
    {
        let mut host = env.objc.borrow_mut::<AppPickerDelegateHostObject>(delegate);
        host.lock_screen_unlocked = false;
        host.lock_slider_progress = 0.0;
        host.swipe_right = false;
        host.swipe_left = false;
    }
    animate_for(env, run_loop, delegate, Duration::from_millis(480), |env, t| {
        let p = ease_in_out_cubic(t) as CGFloat;
        let mut frame: CGRect = msg![env; lock frame];
        frame.origin.y = -size.height * p;
        () = msg![env; lock setFrame:frame];
    });
    () = msg![env; lock setHidden:true];
    if let Some(bar) = status_bar {
        () = msg![env; bar setBackgroundColor:clear];
    }
    if let Some(grid) = grid {
        let active_container = grid.containers[grid.visible].view;
        () = msg![env; active_container setHidden:false];
    }
    () = msg![env; dock setHidden:false];
    () = msg![env; version_label setHidden:false];
    for &dot in page_dots {
        () = msg![env; dot setHidden:false];
    }
}

fn make_lock_thumb_image(env: &mut Environment) -> id {
    let bytes: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/res/lock_thumb.png"));
    let image = Image::from_bytes(bytes).unwrap();
    let cg_image = cg_image::from_image(env, image);
    let ui_image: id = msg_class![env; UIImage imageWithCGImage:cg_image];
    release(env, cg_image);
    ui_image
}

fn make_lock_arrow_image(env: &mut Environment) -> id {
    let bytes: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/res/lock_arrow.png"));
    let image = Image::from_bytes(bytes).unwrap();
    let cg_image = cg_image::from_image(env, image);
    let ui_image: id = msg_class![env; UIImage imageWithCGImage:cg_image];
    release(env, cg_image);
    ui_image
}

#[cfg(unix)]
fn lock_screen_calendar_parts() -> (i32, i32, i32, i32, i32, i32) {
    unsafe {
        let mut now: libc::time_t = 0;
        libc::time(&mut now);
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut tm).is_null() { return (14, 50, 3, 0, 21, 2026); }
        (tm.tm_hour, tm.tm_min, tm.tm_wday, tm.tm_mon, tm.tm_mday, tm.tm_year + 1900)
    }
}
#[cfg(not(unix))]
fn lock_screen_calendar_parts() -> (i32, i32, i32, i32, i32, i32) { (14, 50, 3, 0, 21, 2026) }
fn lock_screen_time_string() -> String {
    let (hour, minute, ..) = lock_screen_calendar_parts();
    format!("{:02}:{:02}", hour, minute)
}
fn lock_screen_date_string() -> String {
    let (_, _, weekday, month, day, _) = lock_screen_calendar_parts();
    let weekdays = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
    let months = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    format!("{}, {} {}", weekdays[weekday.clamp(0, 6) as usize], months[month.clamp(0, 11) as usize], day)
}

/// Builds a fake old-iOS status bar: signal bars, carrier, clock and battery.
/// Returns the bar itself, the clock label and the battery charge bar, which
/// the event loop updates.
fn make_status_bar(
    env: &mut Environment,
    delegate: id,
    width: CGFloat,
    height: CGFloat,
) -> (id, id, id) {
    let clear: id = msg_class![env; UIColor clearColor];
    let bar = new_view(env, rect(0.0, 0.0, width, height), clear);
    let white: id = msg_class![env; UIColor whiteColor];
    let shadow = ui_color(env, 0.0, 0.0, 0.0, 0.7);

    // Signal strength: four white bars of increasing height.
    let signal_width: CGFloat = 3.0;
    let signal_gap: CGFloat = 1.5;
    let signal_bottom = height - 4.0;
    for (idx, signal_height) in [4.0 as CGFloat, 6.0, 8.0, 10.0].iter().enumerate() {
        let signal = new_view(
            env,
            rect(
                5.0 + idx as CGFloat * (signal_width + signal_gap),
                signal_bottom - *signal_height,
                signal_width,
                *signal_height,
            ),
            white,
        );
        () = msg![env; bar addSubview:signal];
    }

    let carrier = new_label(
        env,
        rect(27.0, 1.0, width * 0.35, height - 2.0),
        "HyperHLE",
        12.0,
        true,
        white,
    );
    () = msg![env; carrier setShadowColor:shadow];
    () = msg![env; carrier setShadowOffset:(CGSize {
        width: 0.0,
        height: 1.0,
    })];
    () = msg![env; bar addSubview:carrier];

    let time = new_label(
        env,
        rect(0.0, 1.0, width, height - 2.0),
        &status_bar_time_string(),
        12.0,
        true,
        white,
    );
    () = msg![env; time setTextAlignment:UITextAlignmentCenter];
    () = msg![env; time setShadowColor:shadow];
    () = msg![env; time setShadowOffset:(CGSize {
        width: 0.0,
        height: 1.0,
    })];
    () = msg![env; bar addSubview:time];

    // Battery: a white rounded shell with a black inside and a charge bar.
    let battery = new_view(env, rect(width - BATTERY_RIGHT_INSET, 5.0, 24.0, 11.0), white);
    let battery_layer: id = msg![env; battery layer];
    () = msg![env; battery_layer setCornerRadius:(2.5 as CGFloat)];
    () = msg![env; bar addSubview:battery];

    let black: id = msg_class![env; UIColor blackColor];
    let battery_inner = new_view(
        env,
        rect(width - BATTERY_RIGHT_INSET + 1.5, 6.5, 21.0, 8.0),
        black,
    );
    () = msg![env; bar addSubview:battery_inner];
    let charge = new_view(
        env,
        rect(width - BATTERY_CHARGE_LEFT, 7.5, BATTERY_CHARGE_MAX_WIDTH, 6.0),
        white,
    );
    () = msg![env; bar addSubview:charge];
    let nub = new_view(env, rect(width - BATTERY_RIGHT_INSET + 24.5, 8.5, 2.0, 4.0), white);
    () = msg![env; bar addSubview:nub];
    access_prompt::add_status_target(env, delegate, bar, width);

    (bar, time, charge)
}

/// Left edge of the battery shell, measured from the right of the screen.
const BATTERY_RIGHT_INSET: CGFloat = 31.0;
/// Left edge of the charge bar, measured from the right of the screen.
const BATTERY_CHARGE_LEFT: CGFloat = 28.5;
/// Width of the charge bar when the battery is full.
const BATTERY_CHARGE_MAX_WIDTH: CGFloat = 15.0;

/// Updates the status bar's battery charge bar for the given percentage and
/// charging state, like the old iOS status bar: the fill grows with the level
/// and turns red when the battery is low.
fn update_battery_indicator(
    env: &mut Environment,
    charge_view: id,
    level: Option<(i32, crate::window::BatteryState)>,
) {
    let (percent, charging) = match level {
        Some((percent, state)) if percent >= 0 => {
            let charging = matches!(
                state,
                crate::window::BatteryState::Charging | crate::window::BatteryState::Full
            );
            (percent.clamp(0, 100) as CGFloat, charging)
        }
        // No battery information (a desktop, or SDL couldn't read the level):
        // show a full battery, as the picker always did before the level was
        // wired up. SDL reports -1 when the level is unknown.
        _ => (100.0, false),
    };
    let full: id = msg_class![env; UIColor whiteColor];
    let color: id = if charging {
        ui_color(env, 0.20, 0.85, 0.30, 1.0)
    } else if percent <= 20.0 {
        ui_color(env, 0.90, 0.15, 0.15, 1.0)
    } else {
        full
    };
    () = msg![env; charge_view setBackgroundColor:color];
    let width = if percent <= 0.0 {
        0.0
    } else {
        (BATTERY_CHARGE_MAX_WIDTH * percent / 100.0).max(1.5)
    };
    let frame: CGRect = msg![env; charge_view frame];
    () = msg![env; charge_view setFrame:(rect(
        frame.origin.x,
        7.5,
        width,
        6.0
    ))];
}

#[cfg(unix)]
fn status_bar_time_string() -> String {
    // SAFETY: `localtime_r` only writes into the provided `tm`.
    let tm = unsafe {
        let mut now: libc::time_t = 0;
        libc::time(&mut now);
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut tm).is_null() {
            return "9:41".to_string();
        }
        tm
    };
    format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
}

#[cfg(not(unix))]
fn status_bar_time_string() -> String {
    "9:41".to_string()
}

/// Creates the page-dot row. The caller fills in the current page with
/// [update_page_dots].
fn make_page_dots(
    env: &mut Environment,
    main_view: id,
    screen_size: CGSize,
    page_count: usize,
) -> Vec<id> {
    let mut dots = Vec::with_capacity(page_count);
    // Old iOS only shows the indicator when there is more than one page.
    if page_count <= 1 {
        return dots;
    }
    let count = page_count as CGFloat;
    let total_width = PAGE_DOT_SIZE * count + PAGE_DOT_GAP * (count - 1.0);
    let first_x = (screen_size.width - total_width) / 2.0;
    let y = app_picker_dots_top(screen_size.height);
    let inactive = ui_color(env, 0.75, 0.75, 0.75, 0.45);
    for idx in 0..page_count {
        let dot = new_view(
            env,
            rect(
                first_x + idx as CGFloat * (PAGE_DOT_SIZE + PAGE_DOT_GAP),
                y,
                PAGE_DOT_SIZE,
                PAGE_DOT_SIZE,
            ),
            inactive,
        );
        let layer: id = msg![env; dot layer];
        () = msg![env; layer setCornerRadius:(PAGE_DOT_SIZE / 2.0)];
        () = msg![env; main_view addSubview:dot];
        dots.push(dot);
    }
    dots
}

/// Highlights the dot of the current page.
fn update_page_dots(env: &mut Environment, dots: &[id], current_page: usize) {
    let active: id = msg_class![env; UIColor whiteColor];
    let inactive = ui_color(env, 0.75, 0.75, 0.75, 0.45);
    for (idx, &dot) in dots.iter().enumerate() {
        let color = if idx == current_page { active } else { inactive };
        () = msg![env; dot setBackgroundColor:color];
    }
}

/// Re-creates the page dots after the number of pages changes.
fn rebuild_page_dots(
    env: &mut Environment,
    main_view: id,
    screen_size: CGSize,
    dots: &mut Vec<id>,
    page_count: usize,
) {
    for &dot in dots.iter() {
        () = msg![env; dot removeFromSuperview];
    }
    dots.clear();
    *dots = make_page_dots(env, main_view, screen_size, page_count);
}

const DOCK_ICON_SIZE: CGFloat = 60.0;
const DOCK_ICON_TOP: CGFloat = 5.0;
const DOCK_LABEL_HEIGHT: CGFloat = 12.0;

/// Builds the old-iOS dock at the bottom of the home screen. The shortcuts
/// live outside the paged grid, so they stay in place while pages slide.
fn make_dock(
    env: &mut Environment,
    delegate: id,
    main_view: id,
    screen_size: CGSize,
    recent_app: Option<(&str, id)>,
) -> id {
    let width = screen_size.width;
    let dock_inset = 6.0;
    let dock_width = width - dock_inset * 2.0;
    let top = screen_size.height - APP_PICKER_DOCK_HEIGHT + 2.0;
    let background = ui_color(env, 0.035, 0.045, 0.065, 0.62);
    let dock = new_view(env, rect(dock_inset, top, dock_width, APP_PICKER_DOCK_HEIGHT - 4.0), background);
    let dock_layer: id = msg![env; dock layer];
    () = msg![env; dock_layer setCornerRadius:(12.0 as CGFloat)];
    let hairline_color = ui_color(env, 1.0, 1.0, 1.0, 0.38);
    let hairline = new_view(env, rect(12.0, 0.0, dock_width - 24.0, 1.0), hairline_color);
    () = msg![env; dock addSubview:hairline];

    let count: CGFloat = 4.0;
    let side_inset: CGFloat = 8.0;
    let gap = ((dock_width - side_inset * 2.0 - DOCK_ICON_SIZE * count) / (count - 1.0))
        .clamp(2.0, 18.0);
    let first_x = (dock_width - (DOCK_ICON_SIZE * count + gap * (count - 1.0))) / 2.0;

    let plus_image = make_icon_from_text(env, "+", 50.0, -6.0, (0.25, 0.25, 0.25, 1.0));
    let settings_image = make_settings_app_icon(env);
    let recent_placeholder = make_icon_from_text(env, "↻", 34.0, 0.0, (0.25, 0.25, 0.25, 1.0));
    let browse_image = make_browse_app_icon(env);
    let step = DOCK_ICON_SIZE + gap;
    make_dock_item(env, delegate, dock, first_x, plus_image, "Add .ipa", "addIpa");
    make_dock_item(
        env,
        delegate,
        dock,
        first_x + step,
        settings_image,
        "Settings",
        "settingsShow",
    );
    let (recent_label, recent_image) = recent_app
        .map(|(name, image)| (name, image))
        .unwrap_or(("Recent app", recent_placeholder));
    make_dock_item(
        env,
        delegate,
        dock,
        first_x + step * 3.0,
        recent_image,
        recent_label,
        "openRecentApp",
    );
    make_dock_item(env, delegate, dock, first_x + step * 2.0, browse_image, "Browse", "addIpa");
    () = msg![env; main_view addSubview:dock];
    dock
}

/// Adds one icon + label shortcut to the dock.
fn make_dock_item(
    env: &mut Environment,
    delegate: id,
    dock: id,
    x: CGFloat,
    image: id,
    label_text: &str,
    action: &str,
) {
    let icon_button: id = msg_class![env; UIButton buttonWithType:UIButtonTypeCustom];
    () = msg![env; icon_button setFrame:(rect(
        x,
        DOCK_ICON_TOP,
        DOCK_ICON_SIZE,
        DOCK_ICON_SIZE
    ))];
    () = msg![env; icon_button setImage:image forState:UIControlStateNormal];
    let image_view: id = msg![env; icon_button imageView];
    () = msg![env; image_view setFrame:(rect(0.0, 0.0, DOCK_ICON_SIZE, DOCK_ICON_SIZE))];
    let layer: id = msg![env; image_view layer];
    let gravity = ns_string::get_static_str(env, "resizeAspect");
    () = msg![env; layer setContentsGravity:gravity];
    let selector = env.objc.lookup_selector(action).unwrap();
    () = msg![env; icon_button addTarget:delegate
                                  action:selector
                        forControlEvents:UIControlEventTouchUpInside];
    () = msg![env; dock addSubview:icon_button];

    let label_width = DOCK_ICON_SIZE + 30.0;
    let white: id = msg_class![env; UIColor whiteColor];
    let label = new_label(
        env,
        rect(
            x - (label_width - DOCK_ICON_SIZE) / 2.0,
            DOCK_ICON_TOP + DOCK_ICON_SIZE + 3.0,
            label_width,
            DOCK_LABEL_HEIGHT,
        ),
        label_text,
        10.5,
        true,
        white,
    );
    () = msg![env; label setTextAlignment:UITextAlignmentCenter];
    let shadow = ui_color(env, 0.0, 0.0, 0.0, 0.65);
    () = msg![env; label setShadowColor:shadow];
    () = msg![env; label setShadowOffset:(CGSize {
        width: 0.0,
        height: 1.0,
    })];
    () = msg![env; dock addSubview:label];
}

/// Helper to build a `CGRect` from its components.
fn rect(x: CGFloat, y: CGFloat, width: CGFloat, height: CGFloat) -> CGRect {
    CGRect {
        origin: CGPoint { x, y },
        size: CGSize { width, height },
    }
}

fn ui_color(env: &mut Environment, r: CGFloat, g: CGFloat, b: CGFloat, a: CGFloat) -> id {
    let color: id = msg_class![env; UIColor colorWithRed:r green:g blue:b alpha:a];
    color
}

/// Creates a plain `UIView` with the given frame and background colour.
fn new_view(env: &mut Environment, frame: CGRect, bg: id) -> id {
    let view: id = msg_class![env; UIView alloc];
    let view: id = msg![env; view initWithFrame:frame];
    () = msg![env; view setBackgroundColor:bg];
    view
}

/// Creates a transparent `UILabel` with the given text and style.
fn new_label(
    env: &mut Environment,
    frame: CGRect,
    text: &str,
    font_size: CGFloat,
    bold: bool,
    text_color: id,
) -> id {
    let label: id = msg_class![env; UILabel alloc];
    let label: id = msg![env; label initWithFrame:frame];
    let ns_text = ns_string::from_rust_string(env, text.to_string());
    () = msg![env; label setText:ns_text];
    let font: id = if bold {
        msg_class![env; UIFont boldSystemFontOfSize:font_size]
    } else {
        msg_class![env; UIFont systemFontOfSize:font_size]
    };
    () = msg![env; label setFont:font];
    () = msg![env; label setTextColor:text_color];
    let clear: id = msg_class![env; UIColor clearColor];
    () = msg![env; label setBackgroundColor:clear];
    label
}

fn set_view_x(env: &mut Environment, view: id, x: CGFloat) {
    let mut frame: CGRect = msg![env; view frame];
    frame.origin.x = x;
    () = msg![env; view setFrame:frame];
}

fn lerp_f(a: CGFloat, b: CGFloat, t: CGFloat) -> CGFloat {
    a + (b - a) * t
}

fn ease_out_cubic(t: f64) -> f64 {
    1.0 - (1.0 - t).powi(3)
}

fn ease_in_out_cubic(t: f64) -> f64 {
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}

/// Duration of the iOS 4-style radial icon fly-in when the picker opens.
const HOME_UNLOCK_DURATION: Duration = Duration::from_millis(600);
/// Initial scale of each icon and label at the start of the unlock fly-in.
const HOME_UNLOCK_START_SCALE: CGFloat = 1.65;
/// Fraction of the unlock duration by which the outermost corner icons are
/// staggered behind the innermost icons.
const HOME_UNLOCK_MAX_STAGGER: f64 = 0.22;
/// Base off-screen push distance as a fraction of the longer screen dimension,
/// ensuring even the innermost cells start outside the screen bounds.
const HOME_UNLOCK_BASE_PUSH_FACTOR: CGFloat = 0.55;
/// Extra radial distance multiplier applied to each cell's offset from the
/// grid center so outer rows/columns stay separated from inner ones in flight.
const HOME_UNLOCK_RADIAL_MULT: CGFloat = 1.85;
/// Duration of the page slide after a swipe or arrow-key press.
const PAGE_SLIDE_DURATION: Duration = Duration::from_millis(320);
/// Duration of the "app icon zooms to the middle, screen fades to black"
/// animation played when an app is picked.
/// Duration of the icon's move to the middle of the screen.
const APP_LAUNCH_MOVE_DURATION: Duration = Duration::from_millis(600);
/// How long the icon rests in the middle before it dissolves.
const APP_LAUNCH_CENTER_HOLD_DURATION: Duration = Duration::from_millis(400);
/// Duration of the dissolve of the icon into the black screen.
const APP_LAUNCH_DISSOLVE_DURATION: Duration = Duration::from_millis(500);
/// How long the black screen stays up before the app takes over.
const APP_LAUNCH_BLACK_HOLD_DURATION: Duration = Duration::from_millis(250);
/// Side length of the app icon at the centre of the launch animation.
const APP_LAUNCH_ICON_SIZE: CGFloat = 120.0;
/// How long an app icon must be held down before its delete dialog opens.
const APP_LONG_PRESS_DURATION: Duration = Duration::from_millis(500);

/// Runs the main run loop for `duration`, calling `step` with the linear
/// progress (0.0 to 1.0) before every iteration. Taps delivered while the
/// animation runs are dropped, because they would land on moving views.
fn animate_for(
    env: &mut Environment,
    run_loop: id,
    delegate: id,
    duration: Duration,
    mut step: impl FnMut(&mut Environment, f64),
) {
    let start = Instant::now();
    loop {
        let t = (start.elapsed().as_secs_f64() / duration.as_secs_f64()).min(1.0);
        step(env, t);
        run_run_loop_single_iteration(env, run_loop);
        let host_obj = env.objc.borrow_mut::<AppPickerDelegateHostObject>(delegate);
        host_obj.icon_tapped = nil;
        host_obj.icon_touch_down = nil;
        if t >= 1.0 {
            break;
        }
    }
}

/// Precomputed trajectory of one icon-grid cell for the iOS 4-style home
/// screen unlock animation.
#[derive(Clone, Copy, Debug)]
struct UnlockCellTarget {
    /// Initial X displacement (off-screen) relative to the resting slot.
    start_dx: CGFloat,
    /// Initial Y displacement (off-screen) relative to the resting slot.
    start_dy: CGFloat,
    /// Resting vertical distance from the icon center to its label center,
    /// so the label stays attached below the scaled icon during flight.
    label_offset_y: CGFloat,
    /// Normalized radial distance from the grid center (`0.0..=1.0`), used
    /// to stagger outer icons slightly behind inner ones.
    norm_dist: f64,
}

fn compute_unlock_cell_targets(
    icon_centers: &[CGPoint],
    label_offsets_y: &[CGFloat],
    screen_size: CGSize,
) -> Vec<UnlockCellTarget> {
    if icon_centers.is_empty() {
        return Vec::new();
    }
    let mut min_x = icon_centers[0].x;
    let mut max_x = icon_centers[0].x;
    let mut min_y = icon_centers[0].y;
    let mut max_y = icon_centers[0].y;
    for c in icon_centers {
        min_x = min_x.min(c.x);
        max_x = max_x.max(c.x);
        min_y = min_y.min(c.y);
        max_y = max_y.max(c.y);
    }
    let grid_center = CGPoint {
        x: (min_x + max_x) / 2.0,
        y: (min_y + max_y) / 2.0,
    };

    let mut max_dist: CGFloat = 0.0;
    let mut raw: Vec<(CGFloat, CGFloat, CGFloat)> = Vec::with_capacity(icon_centers.len());
    for c in icon_centers {
        let vx = c.x - grid_center.x;
        let vy = c.y - grid_center.y;
        let dist = (vx * vx + vy * vy).sqrt();
        max_dist = max_dist.max(dist);
        raw.push((vx, vy, dist));
    }

    let base_push = screen_size.width.max(screen_size.height) * HOME_UNLOCK_BASE_PUSH_FACTOR;
    raw.into_iter()
        .zip(label_offsets_y.iter().copied())
        .map(|((vx, vy, dist), label_offset_y)| {
            let (ux, uy) = if dist > 1e-3 {
                (vx / dist, vy / dist)
            } else {
                (0.0, -1.0)
            };
            let start_dx = ux * base_push + vx * HOME_UNLOCK_RADIAL_MULT;
            let start_dy = uy * base_push + vy * HOME_UNLOCK_RADIAL_MULT;
            let norm_dist = if max_dist > 1e-3 {
                (dist / max_dist) as f64
            } else {
                0.0
            };
            UnlockCellTarget {
                start_dx,
                start_dy,
                label_offset_y,
                norm_dist,
            }
        })
        .collect()
}

/// Returns `(dx, dy, label_dy, scale, alpha)` for `target` at global
/// animation progress `t` (`0.0..=1.0`).
fn unlock_cell_state_at(
    target: &UnlockCellTarget,
    t: f64,
) -> (CGFloat, CGFloat, CGFloat, CGFloat, CGFloat) {
    let t = t.clamp(0.0, 1.0);
    let delay = target.norm_dist * HOME_UNLOCK_MAX_STAGGER;
    let span = (1.0 - HOME_UNLOCK_MAX_STAGGER).max(1e-6);
    let local_t = ((t - delay) / span).clamp(0.0, 1.0);
    let p = ease_out_cubic(local_t) as CGFloat;
    let rem = 1.0 - p;
    let dx = target.start_dx * rem;
    let dy = target.start_dy * rem;
    let scale = lerp_f(HOME_UNLOCK_START_SCALE, 1.0, p);
    let label_dy = dy + target.label_offset_y * (scale - 1.0);
    let alpha = (local_t * 2.0).min(1.0) as CGFloat;
    (dx, dy, label_dy, scale, alpha)
}

fn apply_unlock_step(
    env: &mut Environment,
    cells: &[(id, id)],
    targets: &[UnlockCellTarget],
    wallpaper: Option<id>,
    chrome_views: &[id],
    t: f64,
) {
    let chrome_alpha = ease_out_cubic(t.clamp(0.0, 1.0)) as CGFloat;
    if let Some(wallpaper) = wallpaper {
        () = msg![env; wallpaper setAlpha:chrome_alpha];
    }
    for &view in chrome_views {
        () = msg![env; view setAlpha:chrome_alpha];
    }

    for (&(icon_button, label), target) in cells.iter().zip(targets.iter()) {
        let (dx, dy, label_dy, scale, alpha) = unlock_cell_state_at(target, t);
        let icon_transform = CGAffineTransform::make_scale(scale, scale)
            .concat(CGAffineTransform::make_translation(dx, dy));
        let label_transform = CGAffineTransform::make_scale(scale, scale)
            .concat(CGAffineTransform::make_translation(dx, label_dy));
        () = msg![env; icon_button setTransform:icon_transform];
        () = msg![env; icon_button setAlpha:alpha];
        () = msg![env; label setTransform:label_transform];
        () = msg![env; label setAlpha:alpha];
    }
}

/// Plays the iOS 4-style home screen unlock animation when the picker opens:
/// app icons and their labels fly in radially from outside the screen into
/// their grid slots, with inner icons leading and outer corners settling
/// right after.
#[allow(clippy::too_many_arguments)]
fn play_home_screen_unlock_animation(
    env: &mut Environment,
    run_loop: id,
    delegate: id,
    grid: &IconGridStuff,
    screen_size: CGSize,
    wallpaper: Option<id>,
    chrome_views: &[id],
) {
    let cells = grid.containers[grid.visible].cells.clone();
    let mut icon_centers = Vec::with_capacity(cells.len());
    let mut label_offsets_y = Vec::with_capacity(cells.len());
    for &(icon_button, label) in &cells {
        let icon_frame: CGRect = msg![env; icon_button frame];
        let label_frame: CGRect = msg![env; label frame];
        let icon_center = CGPoint {
            x: icon_frame.origin.x + icon_frame.size.width / 2.0,
            y: icon_frame.origin.y + icon_frame.size.height / 2.0,
        };
        let label_center_y = label_frame.origin.y + label_frame.size.height / 2.0;
        icon_centers.push(icon_center);
        label_offsets_y.push(label_center_y - icon_center.y);
    }
    let targets = compute_unlock_cell_targets(&icon_centers, &label_offsets_y, screen_size);

    // Render the initial off-screen state once before starting the timer so
    // first-frame texture creation doesn't eat into the animation duration.
    apply_unlock_step(env, &cells, &targets, wallpaper, chrome_views, 0.0);
    run_run_loop_single_iteration(env, run_loop);
    {
        let host_obj = env.objc.borrow_mut::<AppPickerDelegateHostObject>(delegate);
        host_obj.icon_tapped = nil;
        host_obj.icon_touch_down = nil;
    }

    animate_for(
        env,
        run_loop,
        delegate,
        HOME_UNLOCK_DURATION,
        |env, t| {
            apply_unlock_step(env, &cells, &targets, wallpaper, chrome_views, t);
        },
    );

    if let Some(wallpaper) = wallpaper {
        () = msg![env; wallpaper setAlpha:(1.0 as CGFloat)];
    }
    for &view in chrome_views {
        () = msg![env; view setAlpha:(1.0 as CGFloat)];
    }
    for &(icon_button, label) in &cells {
        () = msg![env; icon_button setTransform:CGAffineTransformIdentity];
        () = msg![env; icon_button setAlpha:(1.0 as CGFloat)];
        () = msg![env; label setTransform:CGAffineTransformIdentity];
        () = msg![env; label setAlpha:(1.0 as CGFloat)];
    }
}

/// Plays the iOS-style launch animation: the tapped icon travels to the middle
/// of the screen while the screen fades to black. The app then starts on top.
fn play_app_launch_animation(
    env: &mut Environment,
    run_loop: id,
    delegate: id,
    main_view: id,
    screen_size: CGSize,
    icon_frame: CGRect,
    icon: id,
) {
    let black: id = msg_class![env; UIColor blackColor];
    let overlay = new_view(
        env,
        rect(0.0, 0.0, screen_size.width, screen_size.height),
        black,
    );
    () = msg![env; overlay setAlpha:(0.0 as CGFloat)];
    () = msg![env; main_view addSubview:overlay];

    let icon_view: id = msg_class![env; UIImageView alloc];
    let icon_view: id = msg![env; icon_view initWithImage:icon];
    () = msg![env; icon_view setFrame:icon_frame];
    () = msg![env; main_view addSubview:icon_view];

    let target_x = (screen_size.width - APP_LAUNCH_ICON_SIZE) / 2.0;
    let target_y = (screen_size.height - APP_LAUNCH_ICON_SIZE) / 2.0;
    // 1. The icon travels to the middle of the screen and grows.
    animate_for(
        env,
        run_loop,
        delegate,
        APP_LAUNCH_MOVE_DURATION,
        |env, t| {
            let p = ease_in_out_cubic(t.min(1.0)) as CGFloat;
            let frame = rect(
                lerp_f(icon_frame.origin.x, target_x, p),
                lerp_f(icon_frame.origin.y, target_y, p),
                lerp_f(icon_frame.size.width, APP_LAUNCH_ICON_SIZE, p),
                lerp_f(icon_frame.size.height, APP_LAUNCH_ICON_SIZE, p),
            );
            () = msg![env; icon_view setFrame:frame];
            // The black background fades in behind the moving icon.
            () = msg![env; overlay setAlpha:(p as CGFloat)];
        },
    );
    // 2. The icon rests in the middle for a moment.
    animate_for(
        env,
        run_loop,
        delegate,
        APP_LAUNCH_CENTER_HOLD_DURATION,
        |_env, _t| {},
    );
    // 3. The icon dissolves into the black background.
    animate_for(
        env,
        run_loop,
        delegate,
        APP_LAUNCH_DISSOLVE_DURATION,
        |env, t| {
            let p = ease_in_out_cubic(t.min(1.0));
            () = msg![env; icon_view setAlpha:((1.0 - p) as CGFloat)];
        },
    );
    // 4. Hold on the black screen before the app takes over.
    animate_for(
        env,
        run_loop,
        delegate,
        APP_LAUNCH_BLACK_HOLD_DURATION,
        |_env, _t| {},
    );
}

/// Slides the icon grid to `new_page`. `direction` is +1.0 when moving to a
/// later page (the new page comes in from the right) and -1.0 for an earlier
/// one. The two page containers swap roles at the end.
fn slide_to_page(
    env: &mut Environment,
    run_loop: id,
    delegate: id,
    grid: &mut IconGridStuff,
    apps: &mut [AppInfo],
    new_page: usize,
    direction: CGFloat,
) {
    let front = grid.visible;
    let back = front ^ 1;
    let width = grid.screen_width;
    update_icon_grid(env, grid, apps, new_page, back);

    let front_view = grid.containers[front].view;
    let back_view = grid.containers[back].view;
    set_view_x(env, back_view, direction * width);
    () = msg![env; back_view setHidden:false];

    animate_for(env, run_loop, delegate, PAGE_SLIDE_DURATION, |env, t| {
        let p = ease_out_cubic(t) as CGFloat;
        set_view_x(env, front_view, -direction * width * p);
        set_view_x(env, back_view, direction * width * (1.0 - p));
    });

    set_view_x(env, front_view, 0.0);
    () = msg![env; front_view setHidden:true];
    grid.visible = back;
}

/// Returns `(scale, alpha)` at progress `t` (`0.0..=1.0`) for the centered
/// pop-in (`open == true`) or pop-out (`open == false`) animation shared by
/// the Delete dialog and the Settings screen.
fn pop_dialog_state_at(t: f64, open: bool) -> (CGFloat, CGFloat) {
    let t = t.clamp(0.0, 1.0);
    let visible = if open {
        ease_out_cubic(t)
    } else {
        1.0 - ease_out_cubic(t)
    };
    let scale = DELETE_DIALOG_START_SCALE + (1.0 - DELETE_DIALOG_START_SCALE) * visible;
    (scale as CGFloat, visible as CGFloat)
}

/// Hides or restores Settings scroll rows that sit outside `main_view` while
/// `main_view` is scaled below full size, since the compositor does not clip
/// `UIScrollView` children to its bounds.
fn set_offscreen_scroll_items_hidden(
    env: &mut Environment,
    settings: &SettingsStuff,
    hide_offscreen: bool,
) {
    if !hide_offscreen {
        for &(view, _, _) in &settings.scroll_items {
            () = msg![env; view setHidden:false];
        }
        return;
    }
    let scroll_view = settings.scroll_view;
    let offset: CGPoint = msg![env; scroll_view contentOffset];
    let offset_y = offset.y;
    let top_limit = settings.scroll_origin_y;
    let bottom_limit = settings.full_height - 20.0;
    for &(view, top_y, bottom_y) in &settings.scroll_items {
        let top_in_settings = settings.scroll_origin_y + top_y - offset_y;
        let bottom_in_settings = settings.scroll_origin_y + bottom_y - offset_y;
        let offscreen = bottom_in_settings <= top_limit || top_in_settings >= bottom_limit;
        () = msg![env; view setHidden:offscreen];
    }
}

/// Opens the Settings screen with the same centered pop-in animation as
/// [show_delete_dialog]: the screen dims while the view grows from the center
/// and fades in over [DELETE_DIALOG_APPEAR_DURATION].
fn show_settings_screen(
    env: &mut Environment,
    run_loop: id,
    delegate: id,
    settings: &SettingsStuff,
) {
    let dimmer = settings.dimmer;
    let settings_view = settings.main_view;
    let (start_scale, _) = pop_dialog_state_at(0.0, true);
    let initial_transform = CGAffineTransform::make_scale(start_scale, start_scale);
    () = msg![env; dimmer setAlpha:(0.0 as CGFloat)];
    () = msg![env; dimmer setHidden:false];
    () = msg![env; settings_view setTransform:initial_transform];
    () = msg![env; settings_view setAlpha:(0.0 as CGFloat)];
    () = msg![env; settings_view setHidden:false];

    // Render one invisible frame before starting the timer so first-time
    // texture creation for Settings rows doesn't eat into the pop-in duration.
    run_run_loop_single_iteration(env, run_loop);
    {
        let host_obj = env.objc.borrow_mut::<AppPickerDelegateHostObject>(delegate);
        host_obj.icon_tapped = nil;
        host_obj.icon_touch_down = nil;
        host_obj.settings_show = false;
        host_obj.settings_hide = false;
    }

    set_offscreen_scroll_items_hidden(env, settings, true);
    animate_for(
        env,
        run_loop,
        delegate,
        DELETE_DIALOG_APPEAR_DURATION,
        |env, t| {
            let (scale, alpha) = pop_dialog_state_at(t, true);
            let transform = CGAffineTransform::make_scale(scale, scale);
            () = msg![env; settings_view setTransform:transform];
            () = msg![env; settings_view setAlpha:alpha];
            () = msg![env; dimmer setAlpha:alpha];
        },
    );
    () = msg![env; settings_view setTransform:CGAffineTransformIdentity];
    () = msg![env; settings_view setAlpha:(1.0 as CGFloat)];
    () = msg![env; dimmer setHidden:true];
    set_offscreen_scroll_items_hidden(env, settings, false);
    {
        let host_obj = env.objc.borrow_mut::<AppPickerDelegateHostObject>(delegate);
        host_obj.settings_show = false;
        host_obj.settings_hide = false;
    }
}

/// Closes the Settings screen with the same centered pop-out animation as
/// [hide_delete_dialog].
fn hide_settings_screen(
    env: &mut Environment,
    run_loop: id,
    delegate: id,
    settings: &SettingsStuff,
) {
    let dimmer = settings.dimmer;
    let settings_view = settings.main_view;
    () = msg![env; dimmer setAlpha:(1.0 as CGFloat)];
    () = msg![env; dimmer setHidden:false];
    set_offscreen_scroll_items_hidden(env, settings, true);

    animate_for(
        env,
        run_loop,
        delegate,
        DELETE_DIALOG_DISAPPEAR_DURATION,
        |env, t| {
            let (scale, alpha) = pop_dialog_state_at(t, false);
            let transform = CGAffineTransform::make_scale(scale, scale);
            () = msg![env; settings_view setTransform:transform];
            () = msg![env; settings_view setAlpha:alpha];
            () = msg![env; dimmer setAlpha:alpha];
        },
    );
    () = msg![env; dimmer setHidden:true];
    () = msg![env; settings_view setHidden:true];
    () = msg![env; settings_view setTransform:CGAffineTransformIdentity];
    () = msg![env; settings_view setAlpha:(1.0 as CGFloat)];
    set_offscreen_scroll_items_hidden(env, settings, false);
    {
        let host_obj = env.objc.borrow_mut::<AppPickerDelegateHostObject>(delegate);
        host_obj.settings_show = false;
        host_obj.settings_hide = false;
    }
}

/// Loads a flat monochrome dock icon, applying the same rounded mask and
/// subtle gloss as the original gray picker icons.
fn make_dock_asset_icon(env: &mut Environment, bytes: &[u8]) -> id {
    let mut image = Image::from_bytes(bytes).unwrap();
    image.round_icon_corners(/* add_sheen: */ true);
    let cg_image = cg_image::from_image(env, image);
    let ui_image: id = msg_class![env; UIImage imageWithCGImage:cg_image];
    release(env, cg_image);
    ui_image
}

fn make_settings_app_icon(env: &mut Environment) -> id {
    let bytes: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/res/settings_icon.png"));
    make_dock_asset_icon(env, bytes)
}

fn make_browse_app_icon(env: &mut Environment) -> id {
    let bytes: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/res/browse_icon.png"));
    make_dock_asset_icon(env, bytes)
}

/// Blue for the selected segment, light grey for the others (iOS style).
fn update_segment_buttons(env: &mut Environment, buttons: &[id], selected_idx: usize) {
    for (idx, &button) in buttons.iter().enumerate() {
        let selected = idx == selected_idx;
        let bg = if selected {
            ui_color(env, 0.0, 0.478, 1.0, 1.0)
        } else {
            ui_color(env, 0.87, 0.87, 0.89, 1.0)
        };
        let title_color: id = if selected {
            msg_class![env; UIColor whiteColor]
        } else {
            msg_class![env; UIColor blackColor]
        };
        () = msg![env; button setBackgroundColor:bg];
        () = msg![env; button setTitleColor:title_color forState:UIControlStateNormal];
    }
}

fn update_scale_hack_buttons(env: &mut Environment, buttons: &[id], value: Option<NonZeroU32>) {
    update_segment_buttons(env, buttons, value.map_or(0, |v| v.get() as usize));
}

fn update_orientation_buttons(
    env: &mut Environment,
    buttons: &[id],
    value: Option<DeviceOrientation>,
) {
    update_segment_buttons(
        env,
        buttons,
        value.map_or(0, |v| match v {
            DeviceOrientation::LandscapeLeft => 1,
            DeviceOrientation::LandscapeRight => 2,
            DeviceOrientation::PortraitUpsideDown => 3,
            _ => panic!(),
        }),
    );
}

fn update_fps_limit_buttons(env: &mut Environment, buttons: &[id], value: Option<FpsLimit>) {
    update_segment_buttons(env, buttons, value.map_or(0, |v| v.segment_index()));
}

fn update_performance_overlay_buttons(
    env: &mut Environment,
    mode_buttons: &[id],
    corner_buttons: &[id],
    mode: PerformanceOverlayMode,
    corner: PerformanceOverlayCorner,
) {
    update_segment_buttons(
        env,
        mode_buttons,
        match mode {
            PerformanceOverlayMode::Full => 0,
            PerformanceOverlayMode::Minimal => 1,
            PerformanceOverlayMode::TitleBar => 2,
        },
    );
    update_segment_buttons(
        env,
        corner_buttons,
        match corner {
            PerformanceOverlayCorner::TopLeft => 0,
            PerformanceOverlayCorner::TopRight => 1,
            PerformanceOverlayCorner::BottomRight => 2,
            PerformanceOverlayCorner::BottomLeft => 3,
        },
    );
}

/// Creates a horizontal row of segment-style buttons inside `parent`.
fn make_segment_buttons(
    env: &mut Environment,
    delegate: id,
    parent: id,
    frame: CGRect,
    buttons: &[(&'static str, &'static str)],
) -> Vec<id> {
    let gap: CGFloat = 4.0;
    let count = buttons.len() as CGFloat;
    let button_width = (frame.size.width - gap * (count - 1.0)) / count;
    let mut result = Vec::new();
    for (i, &(title_text, selector)) in buttons.iter().enumerate() {
        let button_frame = rect(
            frame.origin.x + (i as CGFloat) * (button_width + gap),
            frame.origin.y,
            button_width,
            frame.size.height,
        );
        let button: id = msg_class![env; UIButton buttonWithType:UIButtonTypeCustom];
        () = msg![env; button setFrame:button_frame];
        let text = ns_string::get_static_str(env, title_text);
        () = msg![env; button setTitle:text forState:UIControlStateNormal];
        // FIXME: manually calling layoutSubviews shouldn't be needed?
        () = msg![env; button layoutSubviews];
        let label: id = msg![env; button titleLabel];
        let font: id = msg_class![env; UIFont systemFontOfSize:(13.0 as CGFloat)];
        () = msg![env; label setFont:font];
        let layer: id = msg![env; button layer];
        () = msg![env; layer setCornerRadius:(5.0 as CGFloat)];
        let selector = env.objc.lookup_selector(selector).unwrap();
        () = msg![env; button addTarget:delegate
                                 action:selector
                       forControlEvents:UIControlEventTouchUpInside];
        () = msg![env; parent addSubview:button];
        result.push(button);
    }
    result
}

/// One row of the settings screen.
#[derive(Clone, Copy)]
enum SettingsRow {
    /// A label with a row of segment buttons below it.
    Segmented(&'static str, &'static [(&'static str, &'static str)]),
    /// The "Device model" row (its dropdown is a root-level overlay).
    DeviceDropdown,
    /// The "Language" row (its dropdown is a root-level overlay).
    LanguageDropdown,
    /// The "RAM Limiter" row (its dropdown is a root-level overlay).
    MemoryLimitDropdown,
    /// Label and switch: (label, selector, initial state, enabled).
    Toggle(&'static str, &'static str, bool, bool),
}

const SETTINGS_NAV_BAR_HEIGHT: CGFloat = 44.0;
const SETTINGS_TOP_PADDING: CGFloat = 12.0;
const SETTINGS_GROUP_INSET: CGFloat = 16.0;
const SETTINGS_ROW_SIDE_INSET: CGFloat = 16.0;
const SETTINGS_ROW_HEIGHT: CGFloat = 44.0;
const SETTINGS_SEGMENT_ROW_HEIGHT: CGFloat = 58.0;
const SETTINGS_HEADER_HEIGHT: CGFloat = 28.0;
const SETTINGS_SECTION_GAP: CGFloat = 10.0;
/// Width of the dropdown lists (list plus scrollbar).
const SETTINGS_DROPDOWN_MENU_WIDTH: CGFloat = 280.0;

fn settings_row_height(row: &SettingsRow) -> CGFloat {
    match row {
        SettingsRow::Segmented(..) => SETTINGS_SEGMENT_ROW_HEIGHT,
        _ => SETTINGS_ROW_HEIGHT,
    }
}

/// The views of the Settings screen that the event loop needs to update.
struct SettingsStuff {
    /// Dims the picker behind the Settings screen while it pops in or out.
    dimmer: id,
    /// Full-screen settings view (hidden until the Settings tile is tapped).
    main_view: id,
    /// Scrollable list of grouped rows below the navigation bar.
    scroll_view: id,
    /// `(view, top_y, bottom_y)` for each child added directly to
    /// `scroll_view`, used to hide off-screen rows while scaled.
    scroll_items: Vec<(id, CGFloat, CGFloat)>,
    /// Total height of `main_view`, including the status bar strip.
    full_height: CGFloat,
    /// Vertical origin of `scroll_view` inside `main_view`.
    scroll_origin_y: CGFloat,
    scale_hack_buttons: [id; 5],
    orientation_buttons: [id; 4],
    fps_limit_buttons: [id; 4],
    overlay_mode_buttons: Vec<id>,
    overlay_corner_buttons: Vec<id>,
    /// The "Device model" dropdown.
    device_dropdown: SettingsDropdown,
    /// The "Language" dropdown.
    language_dropdown: SettingsDropdown,
    /// The "RAM Limiter" dropdown.
    memory_limit_dropdown: SettingsDropdown,
}

/// The views of one settings dropdown: a button in a settings row, and a list
/// that opens over the whole Settings screen when the button is tapped.
struct SettingsDropdown {
    /// The button in the settings row. Its title shows the selection.
    button: id,
    /// The list (hidden until the button is tapped).
    menu: id,
    /// Dims the screen behind the open list; tapping it closes the list.
    dimmer: id,
    /// One button per choice, in the order of the spec's entries.
    items: Vec<id>,
    /// The scrollbar thumb shown alongside the list, or `nil` when all the
    /// entries fit and the list needs no scrollbar.
    thumb: id,
    /// Width of the list itself, excluding the scrollbar (if there is one).
    list_width: CGFloat,
}

/// What a settings dropdown offers, and which delegate selectors its buttons
/// call (the `&'static str` names are looked up when the views are built).
struct DropdownSpec {
    /// `(title, tag)` for each choice, in display order.
    entries: Vec<(String, NSInteger)>,
    toggle_selector: &'static str,
    item_selector: &'static str,
    scroll_selector: &'static str,
}

/// The state that the picker keeps for one settings dropdown.
#[derive(Default)]
struct DropdownState {
    /// The tag of the chosen entry, or `None` for the default.
    selected: Option<i32>,
    /// Whether the list is currently open.
    open: bool,
    /// Index of the first visible entry in the list.
    scroll: isize,
}

fn set_dropdown_open(env: &mut Environment, menu: id, dimmer: id, open: bool) {
    () = msg![env; menu setHidden:(!open)];
    () = msg![env; dimmer setHidden:(!open)];
}

/// Makes the dropdown match `state`: the list open or closed, the selection
/// highlighted, the list scrolled, and the button titled with `label`.
fn refresh_dropdown(
    env: &mut Environment,
    dropdown: &SettingsDropdown,
    label: &str,
    state: &DropdownState,
) {
    set_dropdown_open(env, dropdown.menu, dropdown.dimmer, state.open);
    update_dropdown_menu(
        env,
        &dropdown.items,
        dropdown.thumb,
        state.selected,
        state.scroll,
        dropdown.list_width,
    );
    let arrow = if state.open { "▲" } else { "▼" };
    let title = ns_string::from_rust_string(env, format!("{label} {arrow}"));
    () = msg![env; (dropdown.button) setTitle:title forState:UIControlStateNormal];
    release(env, title);
}

/// The largest scroll offset for a list of `item_count` entries.
fn dropdown_max_scroll(item_count: usize) -> isize {
    (item_count as isize - DEVICE_MENU_VISIBLE_ITEMS as isize).max(0)
}

fn dropdown_scroll_position(env: &mut Environment, sender: id, event: id) -> Option<CGFloat> {
    let touches: id = msg![env; event touchesForView:sender];
    let touch: id = msg![env; touches anyObject];
    (touch != nil).then(|| {
        let point: CGPoint = msg![env; touch locationInView:sender];
        point.y
    })
}

fn dropdown_scroll_at(position: CGFloat, item_count: usize) -> isize {
    let travel = (DEVICE_MENU_VISIBLE_ITEMS as CGFloat) * DEVICE_MENU_ITEM_HEIGHT - 54.0;
    (((position - 27.0) / travel) * dropdown_max_scroll(item_count) as CGFloat)
        .round()
        .clamp(0.0, dropdown_max_scroll(item_count) as CGFloat) as isize
}

/// Builds the Settings screen: a navigation bar with a "Done" button, and a
/// scrollable list of grouped sections in the style of the iOS Settings app.
fn setup_settings(
    env: &mut Environment,
    delegate: id,
    super_view: id,
    app_frame: CGRect,
    picker_settings: &PickerSettings,
    show_fps: bool,
    gles_native_switch_enabled: bool,
    show_fullscreen_row: bool,
) -> SettingsStuff {
    let width = app_frame.size.width;
    let height = app_frame.size.height;
    // The Settings screen covers the whole window, including the status bar
    // strip above the application frame. Nothing is clipped by the emulator,
    // so scrolled content must not be able to show up in that strip.
    let status_offset = app_frame.origin.y;
    let full_height = height + status_offset;
    let full_frame = rect(0.0, -status_offset, width, full_height);

    let dim_color = ui_color(env, 0.0, 0.0, 0.0, 0.4);
    let dimmer = new_view(env, full_frame, dim_color);
    () = msg![env; dimmer setHidden:true];
    () = msg![env; super_view addSubview:dimmer];

    let grouped_bg = ui_color(env, 0.79, 0.82, 0.85, 1.0);
    let settings_view = new_view(env, full_frame, grouped_bg);
    () = msg![env; settings_view setHidden:true];
    () = msg![env; super_view addSubview:settings_view];
    let stripe = ui_color(env, 0.55, 0.58, 0.62, 0.10);
    let mut stripe_x: CGFloat = 2.0;
    while stripe_x < width {
        let stripe_view = new_view(env, rect(stripe_x, 0.0, 1.0, height + status_offset), stripe);
        () = msg![env; settings_view addSubview:stripe_view];
        stripe_x += 5.0;
    }

    let clear: id = msg_class![env; UIColor clearColor];
    let black: id = msg_class![env; UIColor blackColor];
    let white: id = msg_class![env; UIColor whiteColor];
    let gray_text = ui_color(env, 0.43, 0.43, 0.45, 1.0);
    // Navigation bar with the title and a "Done" button.
    let nav_bar_bg = ui_color(env, 0.20, 0.34, 0.52, 1.0);
    let nav_bar = new_view(
        env,
        rect(0.0, 0.0, width, SETTINGS_NAV_BAR_HEIGHT + status_offset),
        nav_bar_bg,
    );
    () = msg![env; settings_view addSubview:nav_bar];
    let nav_height = SETTINGS_NAV_BAR_HEIGHT + status_offset;
    const NAV_GRADIENT_STEPS: usize = 48;
    for step in 0..NAV_GRADIENT_STEPS {
        let t = step as CGFloat / (NAV_GRADIENT_STEPS - 1) as CGFloat;
        let y = nav_height * t;
        let band_height = nav_height / NAV_GRADIENT_STEPS as CGFloat + 0.5;
        let color = ui_color(
            env,
            0.39 - 0.18 * t,
            0.56 - 0.25 * t,
            0.75 - 0.23 * t,
            1.0,
        );
        let band = new_view(env, rect(0.0, y, width, band_height), color);
        () = msg![env; nav_bar addSubview:band];
    }
    let nav_separator_color = ui_color(env, 0.12, 0.22, 0.36, 1.0);
    let nav_separator = new_view(
        env,
        rect(0.0, SETTINGS_NAV_BAR_HEIGHT + status_offset - 1.0, width, 1.0),
        nav_separator_color,
    );
    () = msg![env; settings_view addSubview:nav_separator];

    let title_color = ui_color(env, 1.0, 1.0, 1.0, 1.0);
    let title = new_label(
        env,
        rect(60.0, status_offset, width - 120.0, SETTINGS_NAV_BAR_HEIGHT),
        "Settings",
        17.0,
        true,
        title_color,
    );
    () = msg![env; title setTextAlignment:UITextAlignmentCenter];
    () = msg![env; settings_view addSubview:title];

    let done: id = msg_class![env; UIButton buttonWithType:UIButtonTypeCustom];
    () = msg![env; done setFrame:(rect(width - 72.0, status_offset, 64.0, SETTINGS_NAV_BAR_HEIGHT))];
    let done_text = ns_string::get_static_str(env, "Done");
    () = msg![env; done setTitle:done_text forState:UIControlStateNormal];
    // FIXME: manually calling layoutSubviews shouldn't be needed?
    () = msg![env; done layoutSubviews];
    () = msg![env; done setTitleColor:white forState:UIControlStateNormal];
    let done_label: id = msg![env; done titleLabel];
    let done_font: id = msg_class![env; UIFont boldSystemFontOfSize:(17.0 as CGFloat)];
    () = msg![env; done_label setFont:done_font];
    let hide_selector = env.objc.lookup_selector("settingsHide").unwrap();
    () = msg![env; done addTarget:delegate
                           action:hide_selector
                 forControlEvents:UIControlEventTouchUpInside];
    () = msg![env; settings_view addSubview:done];

    // Scrollable content below the navigation bar.
    let scroll: id = msg_class![env; UIScrollView alloc];
    let scroll: id = msg![env; scroll initWithFrame:(rect(
        0.0,
        SETTINGS_NAV_BAR_HEIGHT + status_offset,
        width,
        height - SETTINGS_NAV_BAR_HEIGHT,
    ))];
    () = msg![env; scroll setBackgroundColor:clear];
    // Clip the scrolled content to the scroll view, so it never slides up
    // over the navigation bar.
    () = msg![env; scroll setClipsToBounds:true];
    () = msg![env; settings_view addSubview:scroll];
    // The navigation bar must stay on top of the scrolled content.
    () = msg![env; settings_view bringSubviewToFront:nav_bar];
    () = msg![env; settings_view bringSubviewToFront:nav_separator];
    () = msg![env; settings_view bringSubviewToFront:title];
    () = msg![env; settings_view bringSubviewToFront:done];

    let mut sections: Vec<(&'static str, Vec<SettingsRow>)> = Vec::new();
    let mut display_rows = vec![
        SettingsRow::Segmented(
            "Scale hack",
            &[
                ("Default", "scaleHackDefault"),
                ("Off", "scaleHack1"),
                ("2×", "scaleHack2"),
                ("3×", "scaleHack3"),
                ("4×", "scaleHack4"),
            ],
        ),
        SettingsRow::Segmented(
            "Orientation",
            &[
                ("Default", "orientationDefault"),
                ("←", "orientationLandscapeLeft"),
                ("→", "orientationLandscapeRight"),
                ("↓", "orientationPortraitUpsideDown"),
            ],
        ),
        // The FPS limiter belongs with the rest of the presentation options.
        // "Default" emits nothing, so the command line and the app-specific
        // options files keep their say.
        SettingsRow::Segmented(
            "FPS Limiter",
            &[
                ("Default", "fpsLimitDefault"),
                ("30", "fpsLimit30"),
                ("60", "fpsLimit60"),
                ("Off", "fpsLimitOff"),
            ],
        ),
    ];
    if show_fullscreen_row {
        display_rows.push(SettingsRow::Toggle(
            "Fullscreen (override)",
            "fullscreen:",
            picker_settings.fullscreen,
            true,
        ));
    }
    // Both compatibility fixes are opt-in and may be enabled per-app in the
    // options files. Leaving a switch alone keeps those defaults; flipping it
    // (either way) is an explicit override for every later launch.
    display_rows.push(SettingsRow::Toggle(
        "Rotation fix",
        "rotationFix:",
        picker_settings.rotation_fix.unwrap_or(false),
        true,
    ));
    sections.push(("DISPLAY", display_rows));
    let overlay_mode_options: &'static [(&'static str, &'static str)] =
        if cfg!(any(target_os = "android", target_os = "ios")) {
            &[("Full", "overlayModeFull"), ("Minimal", "overlayModeMinimal")]
        } else {
            &[
                ("Full", "overlayModeFull"),
                ("Minimal", "overlayModeMinimal"),
                ("Title bar", "overlayModeTitleBar"),
            ]
        };
    let mut graphics_rows = vec![
            SettingsRow::Toggle(
                "GLES Native",
                "glesNative:",
            picker_settings.gles_native,
                gles_native_switch_enabled,
            ),
            SettingsRow::Toggle(
                "May fix graphics issues",
                "forceComposition:",
            picker_settings.force_composition,
                true,
            ),
        SettingsRow::Toggle("Performance display", "showFPS:", show_fps, true),
        SettingsRow::Segmented("Display mode", overlay_mode_options),
        SettingsRow::Segmented(
            "Screen corner",
            &[
                ("Top left", "overlayCornerTopLeft"),
                ("Top right", "overlayCornerTopRight"),
                ("Bottom right", "overlayCornerBottomRight"),
                ("Bottom left", "overlayCornerBottomLeft"),
            ],
        ),
        SettingsRow::Toggle(
            "Movie player fix",
            "moviePlayerFix:",
            picker_settings.fix_movie_player.unwrap_or(false),
            true,
        ),
    ];
    graphics_rows.push(SettingsRow::Toggle(
        "Show slide to unlock",
        "showLockScreen:",
        picker_settings.show_lock_screen,
        true,
    ));
    if show_fullscreen_row {
        graphics_rows.push(SettingsRow::Toggle(
            "Keep aspect ratio on resize",
            "preserveAspectRatio:",
            picker_settings.preserve_aspect_ratio,
            true,
    ));
    }
    sections.push(("GRAPHICS", graphics_rows));
    sections.push((
        "DEVICE",
        vec![
            SettingsRow::DeviceDropdown,
            SettingsRow::LanguageDropdown,
        ],
    ));
    sections.push((
        "EMULATION",
        vec![
            SettingsRow::MemoryLimitDropdown,
            SettingsRow::Toggle("Cheat Engine", "cheatEngine:", picker_settings.cheat_engine, true),
            SettingsRow::Toggle("Network access", "network:", picker_settings.network, true),
            SettingsRow::Toggle(
                "Local Game Center sign-in",
                "gameCenterAuthenticated:",
                picker_settings.game_center_authenticated,
                true,
            ),
            SettingsRow::Toggle(
                "Use analog sticks for tilt controls",
                "analogStickTiltControls:",
                picker_settings.analog_stick_tilt_controls,
                true,
            ),
        ],
    ));

    let inner_width = width - 2.0 * SETTINGS_GROUP_INSET;
    let side = SETTINGS_ROW_SIDE_INSET;
    let mut scale_hack_buttons: Vec<id> = Vec::new();
    let mut orientation_buttons: Vec<id> = Vec::new();
    let mut fps_limit_buttons: Vec<id> = Vec::new();
    let mut device_dropdown: Option<SettingsDropdown> = None;
    let mut language_dropdown: Option<SettingsDropdown> = None;
    let mut memory_limit_dropdown: Option<SettingsDropdown> = None;
    let mut overlay_mode_buttons = Vec::new();
    let mut overlay_corner_buttons = Vec::new();
    let mut scroll_items: Vec<(id, CGFloat, CGFloat)> = Vec::new();
    // Where the lists of both dropdowns open: just under the navigation bar.
    let menu_origin = CGPoint {
        x: (width - SETTINGS_DROPDOWN_MENU_WIDTH) / 2.0,
        y: SETTINGS_NAV_BAR_HEIGHT + status_offset + 12.0,
    };

    let mut y: CGFloat = SETTINGS_TOP_PADDING;
    for (header, rows) in sections {
        let header_label = new_label(
            env,
            rect(SETTINGS_GROUP_INSET + 12.0, y, inner_width - 12.0, 18.0),
            header,
            13.0,
            false,
            gray_text,
        );
        () = msg![env; scroll addSubview:header_label];
        scroll_items.push((header_label, y, y + 18.0));
        y += SETTINGS_HEADER_HEIGHT;

        let group_height: CGFloat = rows.iter().map(settings_row_height).sum();
        let outline_color = ui_color(env, 0.66, 0.67, 0.69, 1.0);
        let outline = new_view(env, rect(SETTINGS_GROUP_INSET - 1.0, y - 1.0, inner_width + 2.0, group_height + 2.0), outline_color);
        let outline_layer: id = msg![env; outline layer];
        () = msg![env; outline_layer setCornerRadius:(11.0 as CGFloat)];
        () = msg![env; scroll addSubview:outline];
        scroll_items.push((outline, y, y + group_height));
        let group = new_view(
            env,
            rect(SETTINGS_GROUP_INSET, y, inner_width, group_height),
            white,
        );
        let group_layer: id = msg![env; group layer];
        () = msg![env; group_layer setCornerRadius:(10.0 as CGFloat)];
        () = msg![env; scroll addSubview:group];
        scroll_items.push((group, y, y + group_height));

        let row_count = rows.len();
        let mut row_y: CGFloat = 0.0;
        for (index, row) in rows.iter().enumerate() {
            match *row {
                SettingsRow::Segmented(label, buttons) => {
                    let label_view = new_label(
                        env,
                        rect(side, row_y + 6.0, inner_width - 2.0 * side, 18.0),
                        label,
                        15.0,
                        false,
                        black,
                    );
                    () = msg![env; group addSubview:label_view];
                    let seg_buttons = make_segment_buttons(
                        env,
                        delegate,
                        group,
                        rect(side, row_y + 28.0, inner_width - 2.0 * side, 24.0),
                        buttons,
                    );
                    match buttons[0].1 {
                        "scaleHackDefault" => scale_hack_buttons = seg_buttons,
                        "orientationDefault" => orientation_buttons = seg_buttons,
                        "fpsLimitDefault" => fps_limit_buttons = seg_buttons,
                        "overlayModeFull" => overlay_mode_buttons = seg_buttons,
                        "overlayCornerTopLeft" => overlay_corner_buttons = seg_buttons,
                        _ => unreachable!(),
                    }
                }
                SettingsRow::DeviceDropdown => {
                    let label_view = new_label(
                        env,
                        rect(side, row_y + 11.0, 150.0, 22.0),
                        "Device model",
                        16.0,
                        false,
                        black,
                    );
                    () = msg![env; group addSubview:label_view];
                    device_dropdown = Some(make_settings_dropdown(
                        env,
                        delegate,
                        group,
                        settings_view,
                        rect(inner_width - side - 170.0, row_y + 7.0, 170.0, 30.0),
                        menu_origin,
                        device_model_dropdown_spec(),
                    ));
                }
                SettingsRow::MemoryLimitDropdown => {
                    let label_view = new_label(
                        env,
                        rect(side, row_y + 11.0, 150.0, 22.0),
                        "RAM Limiter",
                        16.0,
                        false,
                        black,
                    );
                    () = msg![env; group addSubview:label_view];
                    memory_limit_dropdown = Some(make_settings_dropdown(
                        env,
                        delegate,
                        group,
                        settings_view,
                        rect(inner_width - side - 170.0, row_y + 7.0, 170.0, 30.0),
                        menu_origin,
                        memory_limit_dropdown_spec(),
                    ));
                }
                SettingsRow::LanguageDropdown => {
                    let label_view = new_label(
                        env,
                        rect(side, row_y + 11.0, 150.0, 22.0),
                        "Language",
                        16.0,
                        false,
                        black,
                    );
                    () = msg![env; group addSubview:label_view];
                    language_dropdown = Some(make_settings_dropdown(
                        env,
                        delegate,
                        group,
                        settings_view,
                        rect(inner_width - side - 170.0, row_y + 7.0, 170.0, 30.0),
                        menu_origin,
                        language_dropdown_spec(),
                    ));
                }
                SettingsRow::Toggle(label_text, selector, default_state, enabled) => {
                    let switch_width: CGFloat = 94.0;
                    let label_view = new_label(
                        env,
                        rect(
                            side,
                            row_y + 4.0,
                            inner_width - 2.0 * side - switch_width - 8.0,
                            36.0,
                        ),
                        label_text,
                        16.0,
                        false,
                        black,
                    );
                    () = msg![env; label_view setTextAlignment:UITextAlignmentLeft];
                    // Long labels wrap onto a second line, as in iOS Settings.
                    () = msg![env; label_view setNumberOfLines:2];
                    () = msg![env; group addSubview:label_view];

                    let switch: id = msg_class![env; UISwitch alloc];
                    let switch: id = msg![env; switch initWithFrame:(rect(
                        inner_width - side - switch_width,
                        row_y + 8.5,
                        switch_width,
                        27.0,
                    ))];
                    () = msg![env; switch setOn:default_state];
                    () = msg![env; switch setEnabled:enabled];
                    let selector = env.objc.lookup_selector(selector).unwrap();
                    () = msg![env; switch addTarget:delegate
                                             action:selector
                                   forControlEvents:UIControlEventValueChanged];
                    () = msg![env; group addSubview:switch];
                }
            }
            row_y += settings_row_height(row);
            // Hairline separator between rows, inset from the left like iOS.
            if index + 1 < row_count {
                let separator_color = ui_color(env, 0.85, 0.85, 0.87, 1.0);
                let separator = new_view(
                    env,
                    rect(side, row_y - 1.0, inner_width - side, 1.0),
                    separator_color,
                );
                () = msg![env; group addSubview:separator];
            }
        }
        y += group_height + SETTINGS_SECTION_GAP;
    }

    // Footer with the build identifier.
    let footer = new_label(
        env,
        rect(0.0, y, width, 18.0),
        &format!("{} ({})", crate::DISPLAY_NAME, crate::COMMIT_HASH),
        12.0,
        false,
        gray_text,
    );
    () = msg![env; footer setTextAlignment:UITextAlignmentCenter];
    () = msg![env; scroll addSubview:footer];
    scroll_items.push((footer, y, y + 18.0));
    y += 30.0;

    () = msg![env; scroll setContentSize:(CGSize { width, height: y + 24.0 })];

    SettingsStuff {
        dimmer,
        main_view: settings_view,
        scroll_view: scroll,
        scroll_items,
        full_height,
        scroll_origin_y: SETTINGS_NAV_BAR_HEIGHT + status_offset,
        scale_hack_buttons: scale_hack_buttons.try_into().unwrap(),
        orientation_buttons: orientation_buttons.try_into().unwrap(),
        fps_limit_buttons: fps_limit_buttons.try_into().unwrap(),
        overlay_mode_buttons,
        overlay_corner_buttons,
        device_dropdown: device_dropdown
            .expect("the settings screen always has a device model row"),
        language_dropdown: language_dropdown
            .expect("the settings screen always has a language row"),
        memory_limit_dropdown: memory_limit_dropdown
            .expect("the settings screen always has a RAM limiter row"),
    }
}

/// Sentinel button tags for the device-model dropdown. Model buttons use their
/// picker tag (`DeviceFamily::tag`, at most 20) as their tag, so the sentinels
/// are placed well above that range.
const DEVICE_TAG_DEFAULT: NSInteger = 1000;
const DEVICE_TAG_AUTO: NSInteger = 1001;

/// How many rows of the device-model dropdown are visible at once before the
/// list has to be scrolled.
const DEVICE_MENU_VISIBLE_ITEMS: usize = 6;
/// Height of a single row in the device-model dropdown.
const DEVICE_MENU_ITEM_HEIGHT: CGFloat = 30.0;

/// The choices shown in the device-model dropdown, in display order, as
/// `(title, tag)` pairs: "Default" (no override), "Auto" (match host screen),
/// then one entry per [crate::window::DeviceFamily] in `ALL_SELECTABLE` order,
/// tagged with the model's own picker tag rather than its position, so that
/// saved settings survive changes to the display order.
fn device_model_entries() -> Vec<(String, NSInteger)> {
    use crate::window::DeviceFamily;
    let mut entries: Vec<(String, NSInteger)> = Vec::new();
    entries.push(("Default".to_string(), DEVICE_TAG_DEFAULT));
    entries.push(("Auto".to_string(), DEVICE_TAG_AUTO));
    for family in DeviceFamily::ALL_SELECTABLE {
        let Some(tag) = family.tag() else {
            continue;
        };
        entries.push((family.display_name().to_string(), tag as NSInteger));
    }
    entries
}

/// The spec of the "Device model" dropdown.
fn device_model_dropdown_spec() -> DropdownSpec {
    DropdownSpec {
        entries: device_model_entries(),
        toggle_selector: "deviceModelToggle",
        item_selector: "deviceModel:",
        scroll_selector: "deviceModelScroll:forEvent:",
    }
}

/// The languages offered by the "Language" dropdown, as `(title, value)`.
/// `value` is what `--preferred-languages=` gets. `None` means "Auto": no
/// override, so the game sees the host's own language settings. Only codes
/// that ns_bundle.rs maps to `.lproj` folders are listed, with English as a
/// fallback.
const LANGUAGE_CHOICES: &[(&str, Option<&str>)] = &[
    ("Auto", None),
    ("English", Some("en")),
    ("Русский", Some("ru,en")),
    ("Deutsch", Some("de,en")),
    ("Français", Some("fr,en")),
    ("Español", Some("es,en")),
    ("Italiano", Some("it,en")),
    ("Português", Some("pt,en")),
    ("Nederlands", Some("nl,en")),
    ("Svenska", Some("sv,en")),
    ("Dansk", Some("da,en")),
    ("Norsk", Some("no,en")),
    ("Suomi", Some("fi,en")),
    ("Türkçe", Some("tr,en")),
    ("日本語", Some("ja,en")),
    ("简体中文", Some("zh-Hans,zh,en")),
];

/// The spec of the "Language" dropdown.
fn language_dropdown_spec() -> DropdownSpec {
    DropdownSpec {
        entries: LANGUAGE_CHOICES
            .iter()
            .enumerate()
            .map(|(idx, (title, _))| (title.to_string(), idx as NSInteger))
            .collect(),
        toggle_selector: "languageToggle",
        item_selector: "language:",
        scroll_selector: "languageScroll:forEvent:",
    }
}

/// The spec of the "RAM Limiter" dropdown.
fn memory_limit_dropdown_spec() -> DropdownSpec {
    DropdownSpec {
        entries: MEMORY_LIMIT_CHOICES
            .iter()
            .enumerate()
            .map(|(idx, choice)| (memory_limit_label(*choice), idx as NSInteger))
            .collect(),
        toggle_selector: "memoryLimitToggle",
        item_selector: "memoryLimit:",
        scroll_selector: "memoryLimitScroll:forEvent:",
    }
}

/// The button title of the language dropdown for the chosen tag.
fn language_label_for_tag(tag: Option<i32>) -> String {
    let idx = tag.unwrap_or(0) as usize;
    LANGUAGE_CHOICES
        .get(idx)
        .map_or("Auto", |choice| choice.0)
        .to_string()
}

/// The launch argument for the chosen language, or `None` for "Auto".
fn language_argument(tag: Option<i32>) -> Option<String> {
    let idx = tag?;
    let &(_, codes) = LANGUAGE_CHOICES.get(idx as usize)?;
    codes.map(|codes| format!("--preferred-languages={codes}"))
}

/// Human-readable label for a device-model choice tag, used as the dropdown
/// button title.
fn device_model_label_for_tag(tag: Option<i32>) -> String {
    use crate::window::DeviceFamily;
    match tag.map(|t| t as NSInteger) {
        None | Some(DEVICE_TAG_DEFAULT) => "Default".to_string(),
        Some(DEVICE_TAG_AUTO) => "Auto".to_string(),
        Some(idx) => DeviceFamily::for_tag(idx as usize)
            .map(|f| f.display_name().to_string())
            .unwrap_or_else(|| "Default".to_string()),
    }
}

/// Re-lay-out and re-style the device-model dropdown list for the given scroll
/// offset and current selection. Items are positioned relative to `scroll`
/// (each row is `DEVICE_MENU_ITEM_HEIGHT` tall); rows outside the visible
/// window are hidden. The currently-selected item is highlighted in magenta,
/// the rest in dark gray. The scrollbar thumb is moved to reflect `scroll`.
fn update_dropdown_menu(
    env: &mut Environment,
    items: &[id],
    thumb: id,
    selected: Option<i32>,
    scroll: isize,
    list_width: CGFloat,
) {
    let visible_menu_height = (DEVICE_MENU_VISIBLE_ITEMS as CGFloat) * DEVICE_MENU_ITEM_HEIGHT;
    let max_scroll = (items.len() as isize).saturating_sub(DEVICE_MENU_VISIBLE_ITEMS as isize);

    for (j, &item) in items.iter().enumerate() {
        let y_pos = ((j as isize - scroll) as CGFloat) * DEVICE_MENU_ITEM_HEIGHT;
        let is_visible = y_pos >= 0.0 && y_pos < visible_menu_height;
        () = msg![env; item setHidden:(!is_visible)];
        if is_visible {
            let item_frame = CGRect {
                origin: CGPoint { x: 0.0, y: y_pos },
                size: CGSize {
                    width: list_width,
                    height: DEVICE_MENU_ITEM_HEIGHT,
                },
            };
            () = msg![env; item setFrame:item_frame];
        }
        let tag: NSInteger = msg![env; item tag];
        let is_selected = selected.is_some_and(|v| v as NSInteger == tag);
        let color = if is_selected { ui_color(env, 0.08, 0.40, 0.78, 1.0) } else { ui_color(env, 1.0, 1.0, 1.0, 1.0) };
        let text_color: id = if is_selected { msg_class![env; UIColor whiteColor] } else { msg_class![env; UIColor blackColor] };
        () = msg![env; item setBackgroundColor:color];
        () = msg![env; item setTitleColor:text_color forState:UIControlStateNormal];
    }

    // Position the scrollbar thumb proportionally to the scroll offset. A list
    // that fits has no thumb (and no track).
    if thumb == nil {
        return;
    }
    let thumb_height: CGFloat = 54.0;
    let travel = (visible_menu_height - thumb_height).max(0.0);
    let thumb_y = if max_scroll > 0 {
        (scroll as CGFloat / max_scroll as CGFloat) * travel
    } else {
        0.0
    };
    let thumb_frame = CGRect {
        origin: CGPoint {
            x: list_width,
            y: thumb_y,
        },
        size: CGSize {
            width: 24.0,
            height: thumb_height,
        },
    };
    () = msg![env; thumb setFrame:thumb_frame];
}

/// Builds a settings dropdown: a button in a settings row, whose title shows
/// the selection, plus a list that is shown as an overlay on the whole Settings
/// screen, with a dimmer behind it. The list holds one button per entry of
/// `spec`, a scrollbar track and thumb, and a draggable control over them.
/// The list starts out hidden, and its titles are set by
/// [refresh_dropdown].
fn make_settings_dropdown(
    env: &mut Environment,
    delegate: id,
    group_view: id,
    root_view: id,
    button_frame: CGRect,
    menu_origin: CGPoint,
    spec: DropdownSpec,
) -> SettingsDropdown {
    let DropdownSpec {
        entries,
        toggle_selector,
        item_selector,
        scroll_selector,
    } = spec;
    // Lists whose entries all fit need no scrollbar, so they use the whole
    // menu width instead of leaving a bare track next to the rows.
    let scrollable = entries.len() > DEVICE_MENU_VISIBLE_ITEMS;
    let scrollbar_width: CGFloat = 24.0;
    let list_width: CGFloat = if scrollable {
        SETTINGS_DROPDOWN_MENU_WIDTH - scrollbar_width
    } else {
        SETTINGS_DROPDOWN_MENU_WIDTH
    };
    let dark_gray: id = msg_class![env; UIColor darkGrayColor];
    let root_size: CGSize = {
        let bounds: CGRect = msg![env; root_view bounds];
        bounds.size
    };

    // Dimmer covering the whole Settings screen while the list is open.
    let dimmer: id = msg_class![env; UIButton buttonWithType:UIButtonTypeCustom];
    () = msg![env; dimmer setFrame:(rect(0.0, 0.0, root_size.width, root_size.height))];
    let dim_color = ui_color(env, 0.0, 0.0, 0.0, 0.35);
    () = msg![env; dimmer setBackgroundColor:dim_color];
    () = msg![env; dimmer addTarget:delegate
                             action:(env.objc.lookup_selector(toggle_selector).unwrap())
                   forControlEvents:UIControlEventTouchUpInside];
    () = msg![env; dimmer setHidden:true];
    () = msg![env; root_view addSubview:dimmer];

    // The toggle button, shown in the settings row. Its title is set by
    // `refresh_dropdown`.
    let button: id = msg_class![env; UIButton buttonWithType:UIButtonTypeCustom];
    let blue = ui_color(env, 0.0, 0.478, 1.0, 1.0);
    () = msg![env; button setTitleColor:blue forState:UIControlStateNormal];
    () = msg![env; button setFrame:button_frame];
    () = msg![env; button layoutSubviews];
    let button_label: id = msg![env; button titleLabel];
    let button_font: id = msg_class![env; UIFont systemFontOfSize:(16.0 as CGFloat)];
    () = msg![env; button_label setFont:button_font];
    () = msg![env; button addTarget:delegate
                             action:(env.objc.lookup_selector(toggle_selector).unwrap())
                   forControlEvents:UIControlEventTouchUpInside];
    () = msg![env; group_view addSubview:button];

    // The dropdown list, placed on the Settings screen (not inside the
    // scroll view, so it is never clipped). Clipped to its own bounds.
    let visible_menu_height = (DEVICE_MENU_VISIBLE_ITEMS as CGFloat) * DEVICE_MENU_ITEM_HEIGHT;
    let menu_frame = CGRect {
        origin: menu_origin,
        size: CGSize {
            width: SETTINGS_DROPDOWN_MENU_WIDTH,
            height: visible_menu_height,
        },
    };
    let menu_view: id = msg_class![env; UIView alloc];
    let menu_view: id = msg![env; menu_view initWithFrame:menu_frame];
    () = msg![env; menu_view setBackgroundColor:dark_gray];
    () = msg![env; menu_view setClipsToBounds:true];
    () = msg![env; menu_view setHidden:true];
    () = msg![env; root_view addSubview:menu_view];

    // List items: one button per choice. Items that fall outside the initially
    // visible window are hidden; scrolling reveals them (see
    // `update_dropdown_menu`).
    let item_selector = env.objc.lookup_selector(item_selector).unwrap();
    let white: id = msg_class![env; UIColor whiteColor];
    let mut items: Vec<id> = Vec::new();
    for (j, (title, tag)) in entries.into_iter().enumerate() {
        let y_pos = (j as CGFloat) * DEVICE_MENU_ITEM_HEIGHT;
        let item_frame = CGRect {
            origin: CGPoint { x: 0.0, y: y_pos },
            size: CGSize {
                width: list_width,
                height: DEVICE_MENU_ITEM_HEIGHT,
            },
        };
        let item_btn: id = msg_class![env; UIButton buttonWithType:UIButtonTypeCustom];
        let text = ns_string::from_rust_string(env, title);
        () = msg![env; item_btn setTitle:text forState:UIControlStateNormal];
        release(env, text);
        () = msg![env; item_btn setTitleColor:white forState:UIControlStateNormal];
        () = msg![env; item_btn setFrame:item_frame];
        () = msg![env; item_btn layoutSubviews];
        () = msg![env; item_btn setTag:tag];
        if y_pos >= visible_menu_height {
            () = msg![env; item_btn setHidden:true];
        }
        () = msg![env; item_btn addTarget:delegate
                                   action:item_selector
                         forControlEvents:UIControlEventTouchUpInside];
        () = msg![env; menu_view addSubview:item_btn];
        items.push(item_btn);
    }

    // Scrollbar track (full height), thumb, and the control that supports
    // clicking the track and dragging the thumb. A list that fits gets none of
    // them: there is nothing to scroll.
    let thumb_view: id = if scrollable {
        let track_view: id = msg_class![env; UIView alloc];
        let track_frame = CGRect {
            origin: CGPoint {
                x: list_width,
                y: 0.0,
            },
            size: CGSize {
                width: scrollbar_width,
                height: visible_menu_height,
            },
        };
        let track_view: id = msg![env; track_view initWithFrame:track_frame];
        let track_color = ui_color(env, 0.87, 0.87, 0.89, 1.0);
        () = msg![env; track_view setBackgroundColor:track_color];
        () = msg![env; menu_view addSubview:track_view];

        let thumb_view: id = msg_class![env; UIView alloc];
        let thumb_frame = CGRect {
            origin: CGPoint {
                x: list_width,
                y: 0.0,
            },
            size: CGSize {
                width: scrollbar_width,
                height: 54.0,
            },
        };
        let thumb_view: id = msg![env; thumb_view initWithFrame:thumb_frame];
        let thumb_color = ui_color(env, 0.55, 0.55, 0.58, 1.0);
        () = msg![env; thumb_view setBackgroundColor:thumb_color];
        () = msg![env; menu_view addSubview:thumb_view];

        let clear: id = msg_class![env; UIColor clearColor];
        let scroll_btn: id = msg_class![env; UIButton buttonWithType:UIButtonTypeCustom];
        () = msg![env; scroll_btn setFrame:track_frame];
        () = msg![env; scroll_btn setBackgroundColor:clear];
        () = msg![env; scroll_btn addTarget:delegate
                                   action:(env.objc.lookup_selector(scroll_selector).unwrap())
                         forControlEvents:(UIControlEventTouchDown | UIControlEventTouchDragInside | UIControlEventTouchDragExit | UIControlEventTouchDragOutside)];
        () = msg![env; menu_view addSubview:scroll_btn];

        thumb_view
    } else {
        nil
    };

    SettingsDropdown {
        button,
        menu: menu_view,
        dimmer,
        items,
        thumb: thumb_view,
        list_width,
    }
}

/// Re-reads the apps directory and redraws the icon grid to match. The current
/// page is kept if it still exists. If the directory can't be read, nothing
/// changes. Returns the new number of pages, for the page dots.
fn reload_app_grid(
    env: &mut Environment,
    grid: &mut IconGridStuff,
    apps: &mut Result<Vec<AppInfo>, String>,
    current_page: &mut usize,
    apps_dir: &Path,
) -> usize {
    let mut new_apps = match enumerate_apps(apps_dir) {
        Ok(new_apps) => new_apps,
        Err(e) => {
            log!("Warning: couldn't reload the apps directory: {}", e);
            return grid.pages.len();
        }
    };
    grid.pages = compute_pages(grid.containers[0].cells.len(), new_apps.len());
    if *current_page >= grid.pages.len() {
        *current_page = grid.pages.len() - 1;
    }
    let visible = grid.visible;
    update_icon_grid(env, grid, &mut new_apps, *current_page, visible);
    *apps = Ok(new_apps);
    grid.pages.len()
}

/// Width and height of the delete dialog's box, and the height of its row of
/// buttons at the bottom.
const DELETE_DIALOG_WIDTH: CGFloat = 270.0;
const DELETE_DIALOG_HEIGHT: CGFloat = 166.0;
const DELETE_DIALOG_BUTTON_HEIGHT: CGFloat = 44.0;

/// The confirmation shown after a long press on an app icon.
struct DeleteDialog {
    /// Covers the picker. Tapping it cancels.
    dimmer: id,
    /// The alert box, with its title, message and buttons.
    panel: id,
    /// Names the app being deleted.
    title: id,
}

/// Builds one button of an alert box.
fn make_alert_button(
    env: &mut Environment,
    delegate: id,
    frame: CGRect,
    title: &str,
    color: id,
    bold: bool,
    selector: &str,
) -> id {
    let button: id = msg_class![env; UIButton buttonWithType:UIButtonTypeCustom];
    () = msg![env; button setFrame:frame];
    let clear: id = msg_class![env; UIColor clearColor];
    () = msg![env; button setBackgroundColor:clear];
    let text = ns_string::from_rust_string(env, title.to_string());
    () = msg![env; button setTitle:text forState:UIControlStateNormal];
    release(env, text);
    () = msg![env; button setTitleColor:color forState:UIControlStateNormal];
    // FIXME: manually calling layoutSubviews shouldn't be needed?
    () = msg![env; button layoutSubviews];
    let label: id = msg![env; button titleLabel];
    let font: id = if bold {
        msg_class![env; UIFont boldSystemFontOfSize:(17.0 as CGFloat)]
    } else {
        msg_class![env; UIFont systemFontOfSize:(17.0 as CGFloat)]
    };
    () = msg![env; label setFont:font];
    let sel = env.objc.lookup_selector(selector).unwrap();
    () = msg![env; button addTarget:delegate
                             action:sel
                   forControlEvents:UIControlEventTouchUpInside];
    button
}

/// Builds the delete dialog to match the old-iOS Settings screen: a white
/// grouped card with a soft gray outline, separators, and blue/red text actions.
/// It is hidden until [show_delete_dialog].
fn make_delete_dialog(
    env: &mut Environment,
    delegate: id,
    main_view: id,
    screen_size: CGSize,
) -> DeleteDialog {
    let dimmer: id = msg_class![env; UIButton buttonWithType:UIButtonTypeCustom];
    () = msg![env; dimmer setFrame:(rect(0.0, 0.0, screen_size.width, screen_size.height))];
    let dim_color = ui_color(env, 0.0, 0.0, 0.0, 0.4);
    () = msg![env; dimmer setBackgroundColor:dim_color];
    let cancel_sel = env.objc.lookup_selector("deleteCancel").unwrap();
    () = msg![env; dimmer addTarget:delegate
                             action:cancel_sel
                   forControlEvents:UIControlEventTouchUpInside];
    () = msg![env; dimmer setHidden:true];
    () = msg![env; main_view addSubview:dimmer];

    let panel_frame = rect(
        (screen_size.width - DELETE_DIALOG_WIDTH) / 2.0,
        (screen_size.height - DELETE_DIALOG_HEIGHT) / 2.0,
        DELETE_DIALOG_WIDTH,
        DELETE_DIALOG_HEIGHT,
    );
    let white: id = msg_class![env; UIColor whiteColor];
    let panel = new_view(env, panel_frame, white);
    let panel_layer: id = msg![env; panel layer];
    () = msg![env; panel_layer setCornerRadius:(12.0 as CGFloat)];
    let border = ui_color(env, 0.70, 0.72, 0.75, 1.0);
    let border_cg: id = msg![env; border CGColor];
    () = msg![env; panel_layer setBorderColor:border_cg];
    () = msg![env; panel_layer setBorderWidth:(1.0 as CGFloat)];
    () = msg![env; panel setHidden:true];
    () = msg![env; main_view addSubview:panel];

    let black: id = msg_class![env; UIColor blackColor];
    let title = new_label(
        env,
        rect(12.0, 14.0, DELETE_DIALOG_WIDTH - 24.0, 44.0),
        "",
        17.0,
        true,
        black,
    );
    () = msg![env; title setTextAlignment:UITextAlignmentCenter];
    () = msg![env; title setNumberOfLines:2];
    () = msg![env; panel addSubview:title];

    let message_color = ui_color(env, 0.25, 0.25, 0.25, 1.0);
    let message = new_label(
        env,
        rect(12.0, 58.0, DELETE_DIALOG_WIDTH - 24.0, 52.0),
        "This will delete the app and all of its saved data.",
        14.0,
        false,
        message_color,
    );
    () = msg![env; message setTextAlignment:UITextAlignmentCenter];
    () = msg![env; message setNumberOfLines:3];
    () = msg![env; panel addSubview:message];

    let separator_color = ui_color(env, 0.82, 0.83, 0.85, 1.0);
    let button_y = DELETE_DIALOG_HEIGHT - DELETE_DIALOG_BUTTON_HEIGHT;
    let half = DELETE_DIALOG_WIDTH / 2.0;
    let separator = new_view(
        env,
        rect(0.0, button_y, DELETE_DIALOG_WIDTH, 1.0),
        separator_color,
    );
    () = msg![env; panel addSubview:separator];
    let v_separator = new_view(
        env,
        rect(half, button_y, 1.0, DELETE_DIALOG_BUTTON_HEIGHT),
        separator_color,
    );
    () = msg![env; panel addSubview:v_separator];

    let blue = ui_color(env, 0.0, 0.478, 1.0, 1.0);
    let red = ui_color(env, 0.88, 0.18, 0.20, 1.0);
    let cancel = make_alert_button(
        env,
        delegate,
        rect(0.0, button_y, half, DELETE_DIALOG_BUTTON_HEIGHT),
        "Cancel",
        blue,
        false,
        "deleteCancel",
    );
    () = msg![env; panel addSubview:cancel];
    let delete = make_alert_button(
        env,
        delegate,
        rect(half, button_y, half, DELETE_DIALOG_BUTTON_HEIGHT),
        "Delete",
        red,
        true,
        "deleteConfirm",
    );
    () = msg![env; panel addSubview:delete];

    DeleteDialog {
        dimmer,
        panel,
        title,
    }
}

/// How long the delete dialog takes to appear, and the scale its box starts
/// from (it grows to full size while it fades in).
const DELETE_DIALOG_APPEAR_DURATION: Duration = Duration::from_millis(200);
const DELETE_DIALOG_START_SCALE: f64 = 0.8;

/// Opens the delete dialog for the app called `app_name`, with the same
/// pop-in as an old iOS alert: the screen dims and the box grows and fades in.
fn show_delete_dialog(
    env: &mut Environment,
    run_loop: id,
    delegate: id,
    dialog: &DeleteDialog,
    app_name: &str,
) {
    let title = ns_string::from_rust_string(env, format!("Delete “{app_name}”?"));
    () = msg![env; (dialog.title) setText:title];
    release(env, title);
    () = msg![env; (dialog.dimmer) setAlpha:(0.0 as CGFloat)];
    () = msg![env; (dialog.dimmer) setHidden:false];
    () = msg![env; (dialog.panel) setAlpha:(0.0 as CGFloat)];
    () = msg![env; (dialog.panel) setHidden:false];

    animate_for(
        env,
        run_loop,
        delegate,
        DELETE_DIALOG_APPEAR_DURATION,
        |env, t| {
            let (scale, alpha) = pop_dialog_state_at(t, true);
            let transform = CGAffineTransform::make_scale(scale, scale);
            () = msg![env; (dialog.panel) setTransform:transform];
            () = msg![env; (dialog.panel) setAlpha:alpha];
            () = msg![env; (dialog.dimmer) setAlpha:alpha];
        },
    );
}

/// How long the delete dialog takes to disappear. It is the reverse of
/// [show_delete_dialog]: the box shrinks and fades out, and the screen clears.
const DELETE_DIALOG_DISAPPEAR_DURATION: Duration = Duration::from_millis(150);

/// Closes the delete dialog with the reverse of its pop-in animation. This
/// returns once the dialog is hidden.
fn hide_delete_dialog(
    env: &mut Environment,
    run_loop: id,
    delegate: id,
    dialog: &DeleteDialog,
) {
    animate_for(
        env,
        run_loop,
        delegate,
        DELETE_DIALOG_DISAPPEAR_DURATION,
        |env, t| {
            let (scale, alpha) = pop_dialog_state_at(t, false);
            let transform = CGAffineTransform::make_scale(scale, scale);
            () = msg![env; (dialog.panel) setTransform:transform];
            () = msg![env; (dialog.panel) setAlpha:alpha];
            () = msg![env; (dialog.dimmer) setAlpha:alpha];
        },
    );
    () = msg![env; (dialog.dimmer) setHidden:true];
    () = msg![env; (dialog.panel) setHidden:true];
}

fn quick_options_trainer_enabled(options: &Options) -> bool {
    !options.trainer_disabled
}

fn quick_options_trainer_argument(enabled: bool) -> &'static str {
    if enabled {
        "--trainer"
    } else {
        "--no-trainer"
    }
}

fn quick_options_force_composition_argument(enabled: bool) -> &'static str {
    if enabled {
        "--force-composition"
    } else {
        "--no-force-composition"
    }
}

fn quick_options_rotation_fix_argument(enabled: bool) -> &'static str {
    if enabled {
        "--rotation-fix"
    } else {
        "--no-rotation-fix"
    }
}

fn quick_options_fix_movie_player_argument(enabled: bool) -> &'static str {
    if enabled {
        "--fix-movie-player"
    } else {
        "--no-fix-movie-player"
    }
}

/// Launch arguments for the two compatibility fix switches. An untouched
/// switch (`None`) emits nothing: `--no-rotation-fix` / `--no-fix-movie-player`
/// are explicit overrides that would cancel the app-specific defaults in
/// touchHLE_default_options.txt (GTA III needs both fixes).
fn quick_options_fix_arguments(
    rotation_fix: Option<bool>,
    fix_movie_player: Option<bool>,
) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(enabled) = rotation_fix {
        args.push(quick_options_rotation_fix_argument(enabled).to_string());
    }
    if let Some(enabled) = fix_movie_player {
        args.push(quick_options_fix_movie_player_argument(enabled).to_string());
    }
    args
}

/// Launch arguments for the Display section's FPS limiter and the Emulation
/// section's RAM limiter. `None` means "Default": nothing is emitted, so the
/// command line and the app-specific options files stay in charge.
fn quick_options_limiter_arguments(
    fps_limit: Option<FpsLimit>,
    memory_limit_mb: Option<u32>,
) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(limit) = fps_limit {
        args.push(format!("--fps-limit={}", limit.short_name()));
    }
    if let Some(mb) = memory_limit_mb {
        args.push(format!("--memory-limit={mb}"));
    }
    args
}

/// Returns `(native_enabled, switch_enabled)` for the probed ANGLE state.
fn quick_options_gles_native_state(options: &Options, angle_available: bool) -> (bool, bool) {
    (options.gles_native || !angle_available, angle_available)
}

/// Launch argument matching the "GLES Native" switch. Always emitted so that
/// the switch's choice overrides the base default and the options files.
fn quick_options_gles_native_argument(enabled: bool) -> &'static str {
    if enabled {
        "--gles-native"
    } else {
        "--no-gles-native"
    }
}

#[cfg(test)]
mod quick_options_trainer_tests {
    use super::*;

    #[test]
    fn trainer_toggle_defaults_off_and_emits_explicit_launch_option() {
        let mut options = Options::default();

        let mut enabled = quick_options_trainer_enabled(&options);
        assert!(!enabled);
        assert_eq!(quick_options_trainer_argument(enabled), "--no-trainer");

        options.parse_argument("--trainer").unwrap();
        enabled = quick_options_trainer_enabled(&options);
        assert!(enabled);
        assert_eq!(quick_options_trainer_argument(enabled), "--trainer");

        options.parse_argument("--no-trainer").unwrap();
        enabled = quick_options_trainer_enabled(&options);
        assert!(!enabled);
        assert_eq!(quick_options_trainer_argument(enabled), "--no-trainer");
    }
}

#[cfg(test)]
mod quick_options_gles_native_tests {
    use super::*;

    #[test]
    fn gles_native_switch_follows_backend_availability_and_option_state() {
        let mut options = Options::default();

        let (mut enabled, mut switch_enabled) = quick_options_gles_native_state(&options, true);
        assert!(!enabled);
        assert!(switch_enabled);
        assert_eq!(
            quick_options_gles_native_argument(enabled),
            "--no-gles-native"
        );

        (enabled, switch_enabled) = quick_options_gles_native_state(&options, false);
        assert!(enabled);
        assert!(!switch_enabled);
        assert_eq!(quick_options_gles_native_argument(enabled), "--gles-native");

        options.parse_argument("--gles-native").unwrap();
        (enabled, switch_enabled) = quick_options_gles_native_state(&options, true);
        assert!(enabled);
        assert!(switch_enabled);
        assert_eq!(quick_options_gles_native_argument(enabled), "--gles-native");

        (enabled, switch_enabled) = quick_options_gles_native_state(&options, false);
        assert!(enabled);
        assert!(!switch_enabled);

        options.parse_argument("--no-gles-native").unwrap();
        (enabled, switch_enabled) = quick_options_gles_native_state(&options, true);
        assert!(!enabled);
        assert!(switch_enabled);
        assert_eq!(
            quick_options_gles_native_argument(enabled),
            "--no-gles-native"
        );
    }
}

#[cfg(test)]
mod quick_options_force_composition_tests {
    use super::*;

    #[test]
    fn switch_argument_explicitly_sets_or_clears_the_option() {
        assert_eq!(
            quick_options_force_composition_argument(true),
            "--force-composition"
        );
        assert_eq!(
            quick_options_force_composition_argument(false),
            "--no-force-composition"
        );
    }
}

#[cfg(test)]
mod quick_options_fix_tests {
    use super::*;

    #[test]
    fn switches_emit_explicit_arguments_both_ways() {
        assert_eq!(quick_options_rotation_fix_argument(true), "--rotation-fix");
        assert_eq!(
            quick_options_rotation_fix_argument(false),
            "--no-rotation-fix"
        );
        assert_eq!(
            quick_options_fix_movie_player_argument(true),
            "--fix-movie-player"
        );
        assert_eq!(
            quick_options_fix_movie_player_argument(false),
            "--no-fix-movie-player"
        );

        assert!(quick_options_fix_arguments(None, None).is_empty());
        assert_eq!(
            quick_options_fix_arguments(Some(true), None),
            vec!["--rotation-fix"]
        );
        assert_eq!(
            quick_options_fix_arguments(None, Some(false)),
            vec!["--no-fix-movie-player"]
        );
        assert_eq!(
            quick_options_fix_arguments(Some(false), Some(true)),
            vec!["--no-rotation-fix", "--fix-movie-player"]
        );
    }

    #[test]
    fn saved_overrides_round_trip_without_cancelling_cli_flags() {
        // An untouched switch is saved as "default" and must not emit an
        // argument, so app-specific defaults (e.g. GTA III) still apply.
        let mut setting = None;
        assert_eq!(optional_bool(setting), "default");
        set_bool_override(optional_bool(setting), &mut setting);
        assert_eq!(setting, None);

        set_bool_override("true", &mut setting);
        assert_eq!(setting, Some(true));
        set_bool_override("false", &mut setting);
        assert_eq!(setting, Some(false));

        // A flag passed on the command line is an explicit choice that a
        // saved "default" line must not erase.
        let mut setting = Some(true);
        set_bool_override("default", &mut setting);
        assert_eq!(setting, Some(true));

        // Unknown values must not turn an explicit override into a default
        // (or the other way round).
        set_bool_override("garbage", &mut setting);
        assert_eq!(setting, Some(true));
    }
}

#[cfg(test)]
mod limiter_option_tests {
    use super::*;

    #[test]
    fn fps_limit_choices_map_to_arguments_and_back() {
        assert_eq!(FpsLimit::Fps30.short_name(), "30");
        assert_eq!(FpsLimit::Fps60.short_name(), "60");
        assert_eq!(FpsLimit::Off.short_name(), "off");
        for choice in [FpsLimit::Fps30, FpsLimit::Fps60, FpsLimit::Off] {
            assert_eq!(FpsLimit::from_short_name(choice.short_name()), Some(choice));
        }
        assert_eq!(FpsLimit::from_short_name("default"), None);
        assert_eq!(FpsLimit::from_short_name("45"), None);
        // The segment buttons are Default, 30, 60, Off.
        assert_eq!(FpsLimit::Fps30.segment_index(), 1);
        assert_eq!(FpsLimit::Fps60.segment_index(), 2);
        assert_eq!(FpsLimit::Off.segment_index(), 3);
    }

    #[test]
    fn ram_limiter_choices_map_to_labels_and_tags() {
        assert_eq!(memory_limit_label(None), "Default");
        assert_eq!(memory_limit_label(Some(128)), "128 MB");
        assert_eq!(memory_limit_label(Some(512)), "512 MB");
        assert_eq!(memory_limit_label(Some(1024)), "1 GB");
        assert_eq!(memory_limit_label(Some(2048)), "2 GB");

        for (idx, choice) in MEMORY_LIMIT_CHOICES.iter().enumerate() {
            assert_eq!(memory_limit_tag(*choice), idx as i32);
            assert_eq!(memory_limit_choice(Some(idx as i32)), *choice);
        }
        // "Default" is the fallback for a tag that isn't offered.
        assert_eq!(memory_limit_tag(Some(64)), 0);
        assert_eq!(memory_limit_choice(Some(i32::MAX)), None);
    }

    #[test]
    fn limiter_arguments_are_only_emitted_for_explicit_choices() {
        assert!(quick_options_limiter_arguments(None, None).is_empty());
        assert_eq!(
            quick_options_limiter_arguments(Some(FpsLimit::Fps30), None),
            vec!["--fps-limit=30"]
        );
        assert_eq!(
            quick_options_limiter_arguments(None, Some(1024)),
            vec!["--memory-limit=1024"]
        );
        assert_eq!(
            quick_options_limiter_arguments(Some(FpsLimit::Off), Some(256)),
            vec!["--fps-limit=off", "--memory-limit=256"]
        );
    }
}

#[cfg(test)]
mod unlock_animation_tests {
    use super::*;

    #[test]
    fn ios4_unlock_flies_in_radially_from_off_screen_to_grid_slots() {
        let screen = CGSize {
            width: 320.0,
            height: 460.0,
        };
        let mut centers = Vec::new();
        let mut label_offsets = Vec::new();
        for row in 0..4 {
            for col in 0..3 {
                centers.push(CGPoint {
                    x: 69.0 + (col as CGFloat) * 91.0,
                    y: 80.0 + (row as CGFloat) * 88.0,
                });
                label_offsets.push(44.0);
            }
        }
        let targets = compute_unlock_cell_targets(&centers, &label_offsets, screen);
        assert_eq!(targets.len(), 12);

        // Every cell starts completely off-screen at t = 0.0 and lands at its
        // resting slot with identity scale and full opacity at t = 1.0.
        for (center, target) in centers.iter().zip(targets.iter()) {
            let (dx0, dy0, _, scale0, alpha0) = unlock_cell_state_at(target, 0.0);
            let start_x = center.x + dx0;
            let start_y = center.y + dy0;
            let off_screen = start_x < -ICON_SIZE.width / 2.0
                || start_x > screen.width + ICON_SIZE.width / 2.0
                || start_y < -ICON_SIZE.height / 2.0
                || start_y > screen.height + ICON_SIZE.height / 2.0;
            assert!(off_screen);
            assert!((scale0 - HOME_UNLOCK_START_SCALE).abs() < 1e-4);
            assert_eq!(alpha0, 0.0);

            let (dx1, dy1, label_dy1, scale1, alpha1) = unlock_cell_state_at(target, 1.0);
            assert!(dx1.abs() < 1e-4);
            assert!(dy1.abs() < 1e-4);
            assert!(label_dy1.abs() < 1e-4);
            assert!((scale1 - 1.0).abs() < 1e-4);
            assert!((alpha1 - 1.0).abs() < 1e-4);
        }

        // Corner cells are staggered behind inner cells.
        assert!(targets[0].norm_dist > targets[4].norm_dist);
    }
}

#[cfg(test)]
mod pop_dialog_animation_tests {
    use super::*;

    #[test]
    fn settings_and_delete_share_centered_pop_profile_and_timing() {
        assert_eq!(DELETE_DIALOG_APPEAR_DURATION, Duration::from_millis(200));
        assert_eq!(DELETE_DIALOG_DISAPPEAR_DURATION, Duration::from_millis(150));

        let (open_scale_0, open_alpha_0) = pop_dialog_state_at(0.0, true);
        assert!((open_scale_0 - DELETE_DIALOG_START_SCALE as CGFloat).abs() < 1e-5);
        assert_eq!(open_alpha_0, 0.0);

        let (open_scale_1, open_alpha_1) = pop_dialog_state_at(1.0, true);
        assert!((open_scale_1 - 1.0).abs() < 1e-5);
        assert!((open_alpha_1 - 1.0).abs() < 1e-5);

        let (close_scale_0, close_alpha_0) = pop_dialog_state_at(0.0, false);
        assert!((close_scale_0 - 1.0).abs() < 1e-5);
        assert!((close_alpha_0 - 1.0).abs() < 1e-5);

        let (close_scale_1, close_alpha_1) = pop_dialog_state_at(1.0, false);
        assert!((close_scale_1 - DELETE_DIALOG_START_SCALE as CGFloat).abs() < 1e-5);
        assert_eq!(close_alpha_1, 0.0);

        let (mid_open_scale, mid_open_alpha) = pop_dialog_state_at(0.5, true);
        let (mid_close_scale, mid_close_alpha) = pop_dialog_state_at(0.5, false);
        assert!(mid_open_scale > 0.95 && mid_open_alpha > 0.85);
        assert!((mid_open_alpha + mid_close_alpha - 1.0).abs() < 1e-5);
        assert!(mid_close_scale < 0.85);
    }
}
