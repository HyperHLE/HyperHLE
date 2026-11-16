/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Abstraction of window setup, OpenGL context creation and event handling.
//!
//! Implemented using the sdl2 crate (a Rust wrapper for SDL2). All usage of
//! SDL should be confined to this module.
//!
//! There is currently no separation of concerns between a single window and
//! window system interaction in general, because it is assumed only one window
//! will be needed for the runtime of the app.

mod geometry;
mod native_aspect_ratio;

use crate::gles::present::present_frame;
use crate::gles::{
    create_gles1_ctx_no_parent_stack, create_gles2_ctx_no_parent_stack, GLESContext,
    LoggingGLESContext, GLES,
};
use crate::image::Image;
use crate::matrix::Matrix;
use crate::options::{Options, PerformanceOverlayCorner, PerformanceOverlayMode, VsyncMode};
use crate::Environment;
use sdl2::mouse::MouseButton;
use sdl2::pixels::PixelFormatEnum;
use sdl2::surface::Surface;
use sdl2_sys::SDL_PowerState;
use std::collections::{HashMap, VecDeque};
use std::env;
use std::f32::consts::{FRAC_PI_2, PI};
use std::num::NonZeroU32;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

static ANDROID_ANGLE_UNAVAILABLE: AtomicBool = AtomicBool::new(false);

/// Whether Quick Options may offer ANGLE; unknown support stays enabled until a context probe fails.
pub fn angle_backend_available() -> bool {
    !cfg!(target_os = "android") || !ANDROID_ANGLE_UNAVAILABLE.load(Ordering::Relaxed)
}

#[cfg(target_os = "android")]
fn should_attempt_android_angle(gles_native: bool, angle_unavailable: bool) -> bool {
    !gles_native && !angle_unavailable
}

fn swap_interval_for_mode(mode: VsyncMode, is_android: bool) -> Option<sdl2::video::SwapInterval> {
    match mode {
        VsyncMode::On => Some(sdl2::video::SwapInterval::VSync),
        VsyncMode::Off => Some(sdl2::video::SwapInterval::Immediate),
        VsyncMode::Auto if is_android => Some(sdl2::video::SwapInterval::VSync),
        VsyncMode::Auto => None,
    }
}

#[cfg(test)]
mod swap_interval_tests {
    use super::{swap_interval_for_mode, VsyncMode};
    use sdl2::video::SwapInterval;

    #[test]
    fn auto_uses_display_sync_on_android() {
        assert!(matches!(
            swap_interval_for_mode(VsyncMode::Auto, true),
            Some(SwapInterval::VSync)
        ));
    }

    #[test]
    fn auto_keeps_driver_default_on_desktop() {
        assert!(swap_interval_for_mode(VsyncMode::Auto, false).is_none());
    }

    #[test]
    fn explicit_off_remains_available_on_android() {
        assert!(matches!(
            swap_interval_for_mode(VsyncMode::Off, true),
            Some(SwapInterval::Immediate)
        ));
    }
}

#[cfg(any(target_os = "android", test))]
fn angle_backend_unsafe_on_power_vr_bxm_8_256(description: &str) -> bool {
    let description = description.to_ascii_lowercase();
    description.contains("angle")
        && description.contains("vulkan")
        && description.contains("bxm-8-256")
}

#[cfg(test)]
mod android_angle_fallback_tests {
    use super::angle_backend_unsafe_on_power_vr_bxm_8_256;

    #[test]
    fn rejects_power_vr_bxm_8_256_vulkan_angle() {
        let renderer = concat!(
            "OpenGL ES 1.1 (ANGLE) / Google Inc. (Imagination Technologies) / ",
            "ANGLE (Vulkan 1.1.170 (PowerVR BXM-8-256 (0x35010101)))",
        );
        assert!(angle_backend_unsafe_on_power_vr_bxm_8_256(renderer));
    }

    #[test]
    fn accepts_other_angle_renderers() {
        assert!(!angle_backend_unsafe_on_power_vr_bxm_8_256(
            "OpenGL ES 3.2 (ANGLE) / Google Inc. (Qualcomm) / ANGLE (Vulkan 1.3 (Adreno))"
        ));
    }

    #[test]
    fn accepts_other_power_vr_vulkan_renderers() {
        assert!(!angle_backend_unsafe_on_power_vr_bxm_8_256(
            "OpenGL ES 3.2 (ANGLE) / Google Inc. (Imagination Technologies) / ANGLE (Vulkan 1.1 (PowerVR Rogue GE8320))"
        ));
    }

    #[test]
    fn accepts_native_power_vr_bxm_8_256_renderer() {
        assert!(!angle_backend_unsafe_on_power_vr_bxm_8_256(
            "OpenGL ES 3.2 / Imagination Technologies / PowerVR BXM-8-256"
        ));
    }
}

#[allow(non_camel_case_types)]
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum DeviceFamily {
    iPhone,
    iPhone3G,
    iPhone3GS,
    iPhone4,
    iPhone4s,
    iPhone5,
    iPhone5c,
    iPhone6,
    iPad,
    iPad2,
    iPad3,
    iPad4,
    iPad5,
    iPadMini,
    iPadMini2,
    iPadMini3,
    iPodTouch,
    iPodTouch2,
    iPodTouch3,
    iPodTouch4,
    iPodTouch5,
}
impl std::fmt::Display for DeviceFamily {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(self.display_name())
    }
}
impl DeviceFamily {
    pub fn display_name(&self) -> &'static str {
        match self {
            DeviceFamily::iPhone => "iPhone",
            DeviceFamily::iPhone3G => "iPhone 3G",
            DeviceFamily::iPhone3GS => "iPhone 3GS",
            DeviceFamily::iPhone4 => "iPhone 4",
            DeviceFamily::iPhone4s => "iPhone 4s",
            DeviceFamily::iPhone5 => "iPhone 5",
            DeviceFamily::iPhone5c => "iPhone 5c",
            DeviceFamily::iPhone6 => "iPhone 6",
            DeviceFamily::iPad => "iPad",
            DeviceFamily::iPad2 => "iPad 2",
            DeviceFamily::iPad3 => "iPad 3",
            DeviceFamily::iPad4 => "iPad 4",
            DeviceFamily::iPad5 => "iPad 5",
            DeviceFamily::iPadMini => "iPad mini",
            DeviceFamily::iPadMini2 => "iPad mini 2",
            DeviceFamily::iPadMini3 => "iPad mini 3",
            DeviceFamily::iPodTouch => "iPod touch",
            DeviceFamily::iPodTouch2 => "iPod touch 2",
            DeviceFamily::iPodTouch3 => "iPod touch 3",
            DeviceFamily::iPodTouch4 => "iPod touch 4",
            DeviceFamily::iPodTouch5 => "iPod touch 5",
        }
    }

    pub fn is_ipad(&self) -> bool {
        matches!(
            self,
            DeviceFamily::iPad
                | DeviceFamily::iPad2
                | DeviceFamily::iPad3
                | DeviceFamily::iPad4
                | DeviceFamily::iPad5
                | DeviceFamily::iPadMini
                | DeviceFamily::iPadMini2
                | DeviceFamily::iPadMini3
        )
    }

    pub fn is_ipod_touch(&self) -> bool {
        matches!(
            self,
            DeviceFamily::iPodTouch
                | DeviceFamily::iPodTouch2
                | DeviceFamily::iPodTouch3
                | DeviceFamily::iPodTouch4
                | DeviceFamily::iPodTouch5
        )
    }

    pub fn is_phone_568(&self) -> bool {
        matches!(
            self,
            DeviceFamily::iPhone5 | DeviceFamily::iPhone5c | DeviceFamily::iPodTouch5
        )
    }

    /// 4.7-inch phones (375x667 points), the first of which was the iPhone 6.
    /// Their launch image is the `-667h` variant.
    pub fn is_phone_667(&self) -> bool {
        matches!(self, DeviceFamily::iPhone6)
    }

    pub fn is_retina(&self) -> bool {
        matches!(
            self,
            DeviceFamily::iPhone4
                | DeviceFamily::iPhone4s
                | DeviceFamily::iPhone5
                | DeviceFamily::iPhone5c
                | DeviceFamily::iPhone6
                | DeviceFamily::iPodTouch4
                | DeviceFamily::iPodTouch5
                | DeviceFamily::iPad3
                | DeviceFamily::iPad4
                | DeviceFamily::iPad5
                | DeviceFamily::iPadMini2
                | DeviceFamily::iPadMini3
        )
    }

    /// Portrait (width, height) in logical points.
    pub fn portrait_size(&self) -> (u32, u32) {
        if self.is_ipad() {
            (768, 1024)
        } else if self.is_phone_667() {
            (375, 667)
        } else if self.is_phone_568() {
            (320, 568)
        } else {
            (320, 480)
        }
    }

    /// UIScreen.scale — retina multiplier.
    pub fn scale_factor(&self) -> f32 {
        if self.is_retina() {
            2.0
        } else {
            1.0
        }
    }

    /// sysctl hw.machine / uname machine string.
    pub fn machine_name(&self) -> &'static str {
        match self {
            DeviceFamily::iPhone => "iPhone1,1",
            DeviceFamily::iPhone3G => "iPhone1,2",
            DeviceFamily::iPhone3GS => "iPhone2,1",
            DeviceFamily::iPhone4 => "iPhone3,1",
            DeviceFamily::iPhone4s => "iPhone4,1",
            DeviceFamily::iPhone5 => "iPhone5,1",
            DeviceFamily::iPhone5c => "iPhone5,3",
            DeviceFamily::iPhone6 => "iPhone7,2",
            DeviceFamily::iPad => "iPad1,1",
            DeviceFamily::iPad2 => "iPad2,1",
            DeviceFamily::iPad3 => "iPad3,1",
            DeviceFamily::iPad4 => "iPad3,4",
            DeviceFamily::iPad5 => "iPad6,11",
            DeviceFamily::iPadMini => "iPad2,5",
            DeviceFamily::iPadMini2 => "iPad4,4",
            DeviceFamily::iPadMini3 => "iPad4,7",
            DeviceFamily::iPodTouch => "iPod1,1",
            DeviceFamily::iPodTouch2 => "iPod2,1",
            DeviceFamily::iPodTouch3 => "iPod3,1",
            DeviceFamily::iPodTouch4 => "iPod4,1",
            DeviceFamily::iPodTouch5 => "iPod5,1",
        }
    }

    /// Amount of physical RAM (in bytes) the modelled device shipped with.
    ///
    /// Used by `NSProcessInfo.physicalMemory`, the `host_statistics`
    /// VM page counts and the `hw.memsize` / `hw.physmem` sysctls so that
    /// every memory-reporting API agrees with the chosen device family.
    /// Reporting too little (the old code reported ~50 MiB regardless of
    /// device) makes memory-budgeting engines such as Unreal Engine 3
    /// (UDKGame / Batman: Arkham City Lockdown) compute a bogus pool size and
    /// then attempt absurd allocations that fail, while reporting too much on
    /// a 32-bit address space risks overflow.
    /// Values come from Apple's published hardware specifications for each
    /// model.
    pub fn physical_memory(&self) -> u64 {
        const MIB: u64 = 1024 * 1024;
        // Grouped by the RAM Apple shipped them with, not by generation: the
        // generations differ wildly here (the iPhone 4 has the same 512 MiB as
        // the 4S, while the 4S's successor has twice that), and every profile
        // that reports the wrong amount misleads the guest twice over — the
        // memory-budgeting engines size their pools from it, and the Default
        // heap ceiling is derived from it (see `default_guest_heap_cap`).
        // Figures are the ones published for the hardware, e.g.
        // <https://en.wikipedia.org/wiki/List_of_iOS_and_iPadOS_devices>.
        match self {
            // 128 MiB: the first two iPhone and iPod touch generations.
            DeviceFamily::iPhone
            | DeviceFamily::iPhone3G
            | DeviceFamily::iPodTouch
            | DeviceFamily::iPodTouch2 => 128 * MIB,
            // 256 MiB: iPhone 3GS, the first iPad and the 3rd/4th iPod touch.
            DeviceFamily::iPhone3GS
            | DeviceFamily::iPad
            | DeviceFamily::iPodTouch3
            | DeviceFamily::iPodTouch4 => 256 * MIB,
            // 512 MiB: iPhone 4 and 4S, iPad 2, iPad mini, iPod touch 5.
            DeviceFamily::iPhone4
            | DeviceFamily::iPhone4s
            | DeviceFamily::iPad2
            | DeviceFamily::iPadMini
            | DeviceFamily::iPodTouch5 => 512 * MIB,
            // 1 GiB: iPhone 5, 5c and 6, iPad 3 and 4, Retina iPad minis.
            // This is capped at 1 GiB even though the address space is 4 GiB,
            // leaving headroom for the guest heap, stacks and mapped libraries.
            DeviceFamily::iPhone5
            | DeviceFamily::iPhone5c
            | DeviceFamily::iPhone6
            | DeviceFamily::iPad3
            | DeviceFamily::iPad4
            | DeviceFamily::iPadMini2
            | DeviceFamily::iPadMini3 => 1024 * MIB,
            // 2 GiB: the iPad 5 (iPad6,11), a 2017 iPad rather than an iPad
            // Air. Two of the 4 GiB guest address space is still comfortable
            // for the heap; the Default ceiling stays half of this.
            DeviceFamily::iPad5 => 2048 * MIB,
        }
    }

    /// Heuristically pick the emulated device family whose screen most closely
    /// matches an arbitrary host screen of `(width, height)` physical pixels.
    pub fn pick_for_screen(width: u32, height: u32) -> DeviceFamily {
        if width == 0 || height == 0 {
            return DeviceFamily::iPhone3GS;
        }
        let (short, long) = if width <= height {
            (width as f32, height as f32)
        } else {
            (height as f32, width as f32)
        };
        let host_ratio = short / long;
        const CANDIDATES: [DeviceFamily; 5] = [
            DeviceFamily::iPhone3GS,
            DeviceFamily::iPhone4,
            DeviceFamily::iPhone5,
            DeviceFamily::iPad2,
            DeviceFamily::iPad3,
        ];
        let mut best = DeviceFamily::iPhone3GS;
        let mut best_dist = f32::INFINITY;
        for family in CANDIDATES {
            let (w, h) = family.portrait_size();
            let ratio = w as f32 / h as f32;
            let dist = (ratio - host_ratio).abs();
            if dist < best_dist {
                best_dist = dist;
                best = family;
            }
        }
        best
    }

    /// CLI/option canonical name accepted by `--device-family=` and emitted by
    /// the app picker. Round-trips through `TryFrom<&str>`.
    pub fn option_name(&self) -> &'static str {
        match self {
            DeviceFamily::iPhone => "iphone-2g",
            DeviceFamily::iPhone3G => "iphone-3g",
            DeviceFamily::iPhone3GS => "iphone-3gs",
            DeviceFamily::iPhone4 => "iphone-4",
            DeviceFamily::iPhone4s => "iphone-4s",
            DeviceFamily::iPhone5 => "iphone-5",
            DeviceFamily::iPhone5c => "iphone-5c",
            DeviceFamily::iPhone6 => "iphone-6",
            DeviceFamily::iPad => "ipad-1",
            DeviceFamily::iPad2 => "ipad-2",
            DeviceFamily::iPad3 => "ipad-3",
            DeviceFamily::iPad4 => "ipad-4",
            DeviceFamily::iPad5 => "ipad-5",
            DeviceFamily::iPadMini => "ipad-mini",
            DeviceFamily::iPadMini2 => "ipad-mini-2",
            DeviceFamily::iPadMini3 => "ipad-mini-3",
            DeviceFamily::iPodTouch => "ipod-touch",
            DeviceFamily::iPodTouch2 => "ipod-touch-2",
            DeviceFamily::iPodTouch3 => "ipod-touch-3",
            DeviceFamily::iPodTouch4 => "ipod-touch-4",
            DeviceFamily::iPodTouch5 => "ipod-touch-5",
        }
    }

    /// Every device model the user can pick, in menu/display order. Used by the
    /// app picker's Quick Options "Device model" selector and by docs.
    pub const ALL_SELECTABLE: &'static [DeviceFamily] = &[
        DeviceFamily::iPhone,
        DeviceFamily::iPhone3G,
        DeviceFamily::iPhone3GS,
        DeviceFamily::iPhone4,
        DeviceFamily::iPhone4s,
        DeviceFamily::iPhone5,
        DeviceFamily::iPhone5c,
        DeviceFamily::iPhone6,
        DeviceFamily::iPad,
        DeviceFamily::iPad2,
        DeviceFamily::iPad3,
        DeviceFamily::iPad4,
        DeviceFamily::iPad5,
        DeviceFamily::iPadMini,
        DeviceFamily::iPadMini2,
        DeviceFamily::iPadMini3,
        DeviceFamily::iPodTouch,
        DeviceFamily::iPodTouch2,
        DeviceFamily::iPodTouch3,
        DeviceFamily::iPodTouch4,
        DeviceFamily::iPodTouch5,
    ];

    /// The same models numbered the way the app picker tags them, which is also
    /// how it saves the selection. New models are appended here so that a saved
    /// tag keeps pointing at the same device; the display order is
    /// [Self::ALL_SELECTABLE].
    const ALL_BY_TAG: &'static [DeviceFamily] = &[
        DeviceFamily::iPhone,
        DeviceFamily::iPhone3G,
        DeviceFamily::iPhone3GS,
        DeviceFamily::iPhone4,
        DeviceFamily::iPhone4s,
        DeviceFamily::iPhone5,
        DeviceFamily::iPhone5c,
        DeviceFamily::iPad,
        DeviceFamily::iPad2,
        DeviceFamily::iPad3,
        DeviceFamily::iPad4,
        DeviceFamily::iPad5,
        DeviceFamily::iPadMini,
        DeviceFamily::iPadMini2,
        DeviceFamily::iPadMini3,
        DeviceFamily::iPodTouch,
        DeviceFamily::iPodTouch2,
        DeviceFamily::iPodTouch3,
        DeviceFamily::iPodTouch4,
        DeviceFamily::iPodTouch5,
        DeviceFamily::iPhone6,
    ];

    /// The picker tag of this model, or `None` if it is not selectable.
    pub fn tag(&self) -> Option<usize> {
        Self::ALL_BY_TAG.iter().position(|family| family == self)
    }

    /// The model for a picker tag, or `None` if the tag is out of range.
    pub fn for_tag(tag: usize) -> Option<DeviceFamily> {
        Self::ALL_BY_TAG.get(tag).copied()
    }
}
impl TryFrom<u64> for DeviceFamily {
    type Error = ();
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(DeviceFamily::iPhone),
            2 => Ok(DeviceFamily::iPad),
            _ => Err(()),
        }
    }
}
impl TryFrom<&str> for DeviceFamily {
    type Error = ();
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value.to_ascii_lowercase().as_str() {
            "iphone" => Ok(DeviceFamily::iPhone3GS),
            "iphone-2g" | "iphone1,1" => Ok(DeviceFamily::iPhone),
            "iphone-3g" | "iphone1,2" => Ok(DeviceFamily::iPhone3G),
            "iphone-3gs" | "iphone2,1" => Ok(DeviceFamily::iPhone3GS),
            "iphone-4" | "iphone3,1" => Ok(DeviceFamily::iPhone4),
            "iphone-4s" | "iphone4,1" => Ok(DeviceFamily::iPhone4s),
            "iphone-5" | "iphone5,1" => Ok(DeviceFamily::iPhone5),
            "iphone-5c" | "iphone5,3" => Ok(DeviceFamily::iPhone5c),
            "iphone-6" | "iphone7,2" => Ok(DeviceFamily::iPhone6),
            "ipad" => Ok(DeviceFamily::iPad2),
            "ipad-1" | "ipad1,1" => Ok(DeviceFamily::iPad),
            "ipad-2" | "ipad2,1" => Ok(DeviceFamily::iPad2),
            "ipad-3" | "ipad3,1" => Ok(DeviceFamily::iPad3),
            "ipad-4" | "ipad3,4" => Ok(DeviceFamily::iPad4),
            "ipad-5" | "ipad6,11" => Ok(DeviceFamily::iPad5),
            "ipad-mini" | "ipad2,5" => Ok(DeviceFamily::iPadMini),
            "ipad-mini-2" | "ipad4,4" => Ok(DeviceFamily::iPadMini2),
            "ipad-mini-3" | "ipad4,7" => Ok(DeviceFamily::iPadMini3),
            "ipod-touch" | "ipod1,1" => Ok(DeviceFamily::iPodTouch),
            "ipod-touch-2" | "ipod2,1" => Ok(DeviceFamily::iPodTouch2),
            "ipod-touch-3" | "ipod3,1" => Ok(DeviceFamily::iPodTouch3),
            "ipod-touch-4" | "ipod4,1" => Ok(DeviceFamily::iPodTouch4),
            "ipod-touch-5" | "ipod5,1" => Ok(DeviceFamily::iPodTouch5),
            _ => Err(()),
        }
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum DeviceOrientation {
    Portrait,
    PortraitUpsideDown,
    LandscapeLeft,
    LandscapeRight,
}
fn normalize_portrait_size(size: (u32, u32)) -> (u32, u32) {
    if size.0 <= size.1 {
        size
    } else {
        (size.1, size.0)
    }
}

fn size_for_orientation_from_size(
    size: (u32, u32),
    orientation: DeviceOrientation,
    scale_hack: NonZeroU32,
) -> (u32, u32) {
    let (width, height) = size;
    let scale_hack = scale_hack.get();
    match orientation {
        DeviceOrientation::Portrait => (width * scale_hack, height * scale_hack),
        DeviceOrientation::PortraitUpsideDown => (width * scale_hack, height * scale_hack),
        DeviceOrientation::LandscapeLeft => (height * scale_hack, width * scale_hack),
        DeviceOrientation::LandscapeRight => (height * scale_hack, width * scale_hack),
    }
}

fn size_for_orientation(
    family: DeviceFamily,
    orientation: DeviceOrientation,
    scale_hack: NonZeroU32,
) -> (u32, u32) {
    size_for_orientation_from_size(family.portrait_size(), orientation, scale_hack)
}
fn rotate_fullscreen_size(orientation: DeviceOrientation, screen_size: (u32, u32)) -> (u32, u32) {
    let (short_side, long_side) = if screen_size.0 < screen_size.1 {
        (screen_size.0, screen_size.1)
    } else {
        (screen_size.1, screen_size.0)
    };
    match orientation {
        DeviceOrientation::Portrait | DeviceOrientation::PortraitUpsideDown => {
            (short_side, long_side)
        }
        DeviceOrientation::LandscapeLeft | DeviceOrientation::LandscapeRight => {
            (long_side, short_side)
        }
    }
}
/// Tell SDL2 what orientation we want. Only useful on Android and iOS.
fn set_sdl2_orientation(orientation: DeviceOrientation) {
    // On iOS the host UI decides the actual interface orientation, so only
    // tell SDL2 which family of orientations the current game may use.
    if cfg!(target_os = "ios") {
        sdl2::hint::set(
            "SDL_IOS_ORIENTATIONS",
            match orientation {
                DeviceOrientation::Portrait | DeviceOrientation::PortraitUpsideDown => "Portrait",
                DeviceOrientation::LandscapeLeft | DeviceOrientation::LandscapeRight => {
                    "LandscapeLeft LandscapeRight"
                }
            },
        );
        return;
    }
    // Despite the name, this hint works on Android too.
    sdl2::hint::set(
        "SDL_IOS_ORIENTATIONS",
        match orientation {
            DeviceOrientation::Portrait => "Portrait",
            // The inversion is deliberate. These probably correspond to
            // iPhone OS content orientations?
            DeviceOrientation::PortraitUpsideDown => "PortraitUpsideDown",
            DeviceOrientation::LandscapeLeft => "LandscapeRight",
            DeviceOrientation::LandscapeRight => "LandscapeLeft",
        },
    );
}

/// COMPAT: per-game accelerometer axis remap, applied to real-sensor data in
/// the hardware path of [Window::get_acceleration].
///
/// The values delivered to the guest are always in the device's portrait
/// frame (iOS semantics), which is correct for games that honour their
/// declared interface orientation. Some games, however, hard-code their
/// tilt math for one particular way of holding the phone — or the host
/// device reports sensors in a natural-orientation frame some games don't
/// expect (e.g. tablets) — and then steering/camera controls come out
/// mirrored or sideways (seen with e.g. Asphalt 7's tilt camera).
///
/// `TOUCHHLE_ACCELEROMETER_AXES` accepts a comma-separated list of:
/// - "swap": transpose x and y (sideways behaviour on some devices)
/// - "flipx": negate x (left/right inversion)
/// - "flipy": negate y (forward/backward inversion)
/// Both flips together make a 180-degree fix; all three together swap and
/// flip. Example: TOUCHHLE_ACCELEROMETER_AXES=swap,flipy
///
/// The knob is read once and cached. Gyroscope readings are NOT remapped.
fn accelerometer_compat_remap(x: f32, y: f32, z: f32) -> (f32, f32, f32) {
    use std::sync::OnceLock;
    static CACHE: OnceLock<(bool, bool, bool)> = OnceLock::new();
    let (swap, flipx, flipy) = *CACHE.get_or_init(|| {
        let var = std::env::var("TOUCHHLE_ACCELEROMETER_AXES").unwrap_or_default();
        let var = var.to_ascii_lowercase();
        (
            var.contains("swap"),
            var.contains("flipx"),
            var.contains("flipy"),
        )
    });
    let (mut x, mut y) = (x, y);
    if swap {
        std::mem::swap(&mut x, &mut y);
    }
    if flipx {
        x = -x;
    }
    if flipy {
        y = -y;
    }
    (x, y, z)
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum FingerId {
    Mouse,
    Touch(i64),
    VirtualCursor,
    ButtonToTouch(crate::options::Button),
    StickToTouch,
    DpadToTouch,
}
pub type Coords = (f32, f32);

struct DpadState {
    left: bool,
    right: bool,
    up: bool,
    down: bool,
    active: bool,
}

#[derive(Debug)]
pub enum TextInputEvent {
    Text(String),
    Backspace,
    Return,
}

#[derive(Debug)]
pub enum Event {
    /// User requested quit.
    Quit,
    /// User closed the SDL window.
    CloseWindow,
    /// OS has informed touchHLE it will soon become inactive.
    /// (iOS `applicationWillResignActive:`, Android `onPause()`)
    AppWillResignActive,
    /// OS has informed touchHLE it has become inactive and entered the
    /// background. (iOS `applicationDidEnterBackground:`, Android
    /// `onPause()` completing / activity no longer visible)
    AppDidEnterBackground,
    /// OS has informed touchHLE it is about to leave the background and
    /// re-enter the foreground. (iOS `applicationWillEnterForeground:`,
    /// Android `onResume()` starting)
    AppWillEnterForeground,
    /// OS has informed touchHLE it has become active again after being
    /// inactive/backgrounded. (iOS `applicationDidBecomeActive:`)
    AppDidBecomeActive,
    /// OS has informed touchHLE it will soon terminate.
    /// (iOS `applicationWillTerminate:`, Android `onDestroy()`)
    AppWillTerminate,
    TouchesDown(HashMap<FingerId, Coords>),
    TouchesMove(HashMap<FingerId, Coords>),
    TouchesUp(HashMap<FingerId, Coords>),
    /// User pressed F12, requesting that execution be paused and the debugger
    /// take over.
    EnterDebugger,
    TextInput(TextInputEvent),
    /// Left (-1) or right (+1) arrow key was pressed. Used by the app picker
    /// to switch pages.
    ArrowKey(i8),
    /// SDL's vertical mouse-wheel delta (including the host's natural-scroll setting).
    MouseWheel(i32),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BatteryState {
    Unknown,
    OnBattery,
    NoBattery,
    Charging,
    Full,
}

impl BatteryState {
    fn from_sdl(status: SDL_PowerState) -> BatteryState {
        match status {
            SDL_PowerState::SDL_POWERSTATE_UNKNOWN => BatteryState::Unknown,
            SDL_PowerState::SDL_POWERSTATE_ON_BATTERY => BatteryState::OnBattery,
            SDL_PowerState::SDL_POWERSTATE_NO_BATTERY => BatteryState::NoBattery,
            SDL_PowerState::SDL_POWERSTATE_CHARGING => BatteryState::Charging,
            SDL_PowerState::SDL_POWERSTATE_CHARGED => BatteryState::Full,
        }
    }

    fn tag(self) -> u8 {
        match self {
            BatteryState::Unknown => 0,
            BatteryState::OnBattery => 1,
            BatteryState::NoBattery => 2,
            BatteryState::Charging => 3,
            BatteryState::Full => 4,
        }
    }

    fn from_tag(tag: u8) -> BatteryState {
        match tag {
            1 => BatteryState::OnBattery,
            2 => BatteryState::NoBattery,
            3 => BatteryState::Charging,
            4 => BatteryState::Full,
            _ => BatteryState::Unknown,
        }
    }
}

pub enum GLVersion {
    /// OpenGL ES 1.1
    GLES11,
    /// OpenGL ES 2.0
    GLES20,
    /// OpenGL ES 3.0
    GLES30,
    /// OpenGL 2.1 compatibility profile
    GL21Compat,
    /// OpenGL 3.3 Core profile
    GL33Core,
}

pub struct GLContext(sdl2::video::GLContext);

impl GLContext {
    pub fn is_current(&self) -> bool {
        self.0.is_current()
    }
}

fn surface_from_image(image: &Image) -> Surface<'_> {
    let src_pixels = image.pixels();
    let (width, height) = image.dimensions();

    let mut surface = Surface::new(width, height, PixelFormatEnum::RGBA32).unwrap();
    let (width, height) = (width as usize, height as usize);
    let pitch = surface.pitch() as usize;
    surface.with_lock_mut(|dst_pixels| {
        for y in 0..height {
            for x in 0..width {
                for channel in 0..4 {
                    let src_idx = y * width * 4 + x * 4 + channel;
                    let dst_idx = y * pitch + x * 4 + channel;
                    dst_pixels[dst_idx] = src_pixels[src_idx];
                }
            }
        }
    });
    surface
}

/// Leave a transparent margin so guest icons fit the macOS Dock tile similarly
/// to the host application's bundled icon.
#[cfg(target_os = "macos")]
fn pad_icon_for_dock(image: &Image) -> Image {
    let (width, height) = image.dimensions();
    let (canvas_width, canvas_height) = (width * 5 / 4, height * 5 / 4);
    let mut pixels = vec![0; (canvas_width * canvas_height * 4) as usize];
    let x_offset = ((canvas_width - width) / 2) as usize;
    let y_offset = ((canvas_height - height) / 2) as usize;
    let source = image.pixels();
    let source_stride = width as usize * 4;
    let canvas_stride = canvas_width as usize * 4;
    for y in 0..height as usize {
        let source_start = y * source_stride;
        let target_start = (y + y_offset) * canvas_stride + x_offset * 4;
        pixels[target_start..target_start + source_stride]
            .copy_from_slice(&source[source_start..source_start + source_stride]);
    }
    Image::from_pixels(canvas_width, canvas_height, pixels)
}

/// Undo SDL's application-wide Dock icon override when the picker reopens.
/// AppKit restores the icon declared by the app bundle when given nil.
/// See https://developer.apple.com/documentation/appkit/nsapplication/applicationiconimage
#[cfg(target_os = "macos")]
fn restore_application_icon() {
    use std::ffi::{c_char, c_void};

    #[link(name = "objc")]
    unsafe extern "C" {
        fn objc_getClass(name: *const c_char) -> *mut c_void;
        fn sel_registerName(name: *const c_char) -> *mut c_void;
        fn objc_msgSend();
    }

    unsafe {
        let class = objc_getClass(c"NSApplication".as_ptr());
        if class.is_null() {
            return;
        }
        let shared: unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void =
            std::mem::transmute(objc_msgSend as *const ());
        let app = shared(class, sel_registerName(c"sharedApplication".as_ptr()));
        if app.is_null() {
            return;
        }
        let set_icon: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) =
            std::mem::transmute(objc_msgSend as *const ());
        set_icon(
            app,
            sel_registerName(c"setApplicationIconImage:".as_ptr()),
            std::ptr::null_mut(),
        );
    }
}

/// Query the host's primary display size in physical pixels, if possible.
///
/// Used by `--device-family=auto` to pick the closest emulated device before
/// the real [Window] is created. Spins up a throwaway SDL video subsystem; on
/// any failure (e.g. headless host, no display) returns [None] so the caller
/// can fall back to a sensible default.
pub fn host_screen_size() -> Option<(u32, u32)> {
    let sdl_ctx = sdl2::init().ok()?;
    let video_ctx = sdl_ctx.video().ok()?;
    let bounds = video_ctx.display_bounds(0).ok()?;
    let (w, h) = bounds.size();
    if w == 0 || h == 0 {
        None
    } else {
        Some((w, h))
    }
}

/// Configure the host OpenGL ES driver on Android.
///
/// This runs before creating an SDL window/context, because SDL loads EGL and
/// GLES at context-creation time. `false` clears bundled overrides; `true`
/// stages the ANGLE libraries when they can be loaded. A successful GL context
/// is still checked separately to ensure ANGLE actually became active.
#[cfg(target_os = "android")]
fn configure_android_angle_driver(use_angle: bool) -> bool {
    if !use_angle {
        // Do not leave a stale or user-provided override pointing at the
        // bundled ANGLE; SDL will use Android's system OpenGL ES driver.
        env::remove_var("SDL_VIDEO_EGL_DRIVER");
        env::remove_var("SDL_VIDEO_GL_DRIVER");
        log!("GLES Native requested; using the Android system OpenGL ES driver.");
        return false;
    }
    const CANDIDATES: &[(&str, &str, &str)] = &[
        (
            "libEGL_angle.so",
            "libGLESv1_CM_angle.so",
            "libGLESv2_angle.so",
        ),
        (
            "libEGL_angle_in_apk.so",
            "libGLESv1_CM_angle_in_apk.so",
            "libGLESv2_angle_in_apk.so",
        ),
    ];

    fn can_load(name: &str) -> bool {
        use std::ffi::CString;
        let Ok(cname) = CString::new(name) else {
            return false;
        };
        // RTLD_NOW = 2. Load, and immediately unload if it succeeded.
        unsafe {
            let handle = libc::dlopen(cname.as_ptr(), 2);
            if handle.is_null() {
                false
            } else {
                libc::dlclose(handle);
                true
            }
        }
    }

    for &(egl, gles1, gles2) in CANDIDATES {
        let libraries_loadable = can_load(egl) && can_load(gles1) && can_load(gles2);
        if !libraries_loadable {
            continue;
        }

        // Always override SDL's environment-based driver choice when the
        // bundled ANGLE libraries are available.
        env::set_var("SDL_VIDEO_EGL_DRIVER", egl);
        // ANGLE's ES 1.1 front-end also resolves higher-version entry points
        // through EGL's get-proc-address mechanism.
        env::set_var("SDL_VIDEO_GL_DRIVER", gles1);
        log!(
            "Using bundled ANGLE for Android OpenGL ES ({} / {}).",
            egl,
            gles1
        );
        return true;
    }

    // Do not leave a stale or user-provided override pointing at an unavailable
    // driver; SDL will use Android's system OpenGL ES fallback.
    env::remove_var("SDL_VIDEO_EGL_DRIVER");
    env::remove_var("SDL_VIDEO_GL_DRIVER");
    log!("Bundled ANGLE is unavailable; using the Android system OpenGL ES driver.");
    false
}

pub struct Window {
    _sdl_ctx: sdl2::Sdl,
    video_ctx: sdl2::VideoSubsystem,
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    aspect_ratio_constraint: native_aspect_ratio::Constraint,
    window: sdl2::video::Window,
    event_pump: sdl2::EventPump,
    event_queue: VecDeque<Event>,
    last_polled: Instant,
    /// TEST HOOK: injected tap coordinates for the GD music-bypass autoplay
    /// test ( TOUCHHLE_GD_AUTOPLAY ), scheduled in popped-frame counts.
    auto_taps: Vec<(u64, f32, f32)>,
    /// Separate queue for extremely high-priority events (e.g. app about to
    /// terminate).
    high_priority_event: Option<Event>,
    enable_event_polling: bool,
    #[cfg(target_os = "macos")]
    max_height: u32,
    #[cfg(target_os = "macos")]
    viewport_y_offset: u32,
    /// Copy of `fullscreen` on [Options]. Note that this is meaningless when
    /// [Self::rotatable_fullscreen] returns [true].
    fullscreen: bool,
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    preserve_aspect_ratio: bool,
    scale_hack: NonZeroU32,
    host_screen_size: Option<(u32, u32)>,
    internal_gl_ins: Option<Box<dyn GLESContext>>,
    perf_context_token: usize,
    /// Cached `GL_VERSION / GL_VENDOR / GL_RENDERER` string of the internal
    /// OpenGL ES context, captured at window creation. Used to detect the host
    /// GLES driver (e.g. whether we are running through ANGLE or on a vendor's
    /// native driver such as Qualcomm Adreno) so that driver-specific
    /// workarounds can be auto-enabled. See [Window::gl_driver_description].
    gl_driver_description: String,
    host_framebuffer: u32,
    splash_image: Option<Image>,
    /// Whether the selected image already targets the startup orientation.
    splash_image_is_orientation_specific: bool,
    device_family: DeviceFamily,
    device_orientation: DeviceOrientation,
    controller_ctx: sdl2::GameControllerSubsystem,
    controllers: Vec<sdl2::controller::GameController>,
    dpad_state: DpadState,
    stick_active: bool,
    _sensor_ctx: sdl2::SensorSubsystem,
    accelerometer: Option<sdl2::sensor::Sensor>,
    gyroscope: Option<sdl2::sensor::Sensor>,
    virtual_cursor_last: Option<(f32, f32, bool, bool)>,
    virtual_cursor_last_unsticky: Option<(f32, f32, Instant)>,
    virtual_accelerometer_last: Option<(f32, f32, bool)>,
    /// Host scheduling / power-management hints for the emulator thread
    /// (Android). Fed once per presented frame from [Window::swap_window].
    perf_hints: crate::perf_hints::PerfHints,
    /// Whether or not we are on the "main" environment stack (rather than
    /// a coroutine stack). Checked in various functions to make sure that
    /// certain SDL functions (that call JNI functions) are on the main
    /// stack on Android.
    pub(super) on_main_stack: bool,
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn base_window_title() -> String {
    if crate::branding().is_empty() {
        format!("{} {}", crate::DISPLAY_NAME, crate::VERSION)
    } else {
        format!("{} {} {}", crate::DISPLAY_NAME, crate::branding(), crate::VERSION)
    }
}

impl Window {
    /// Returns [true] if touchHLE is running on a device where we should always
    /// display fullscreen, but SDL2 will let us control the orientation, i.e.
    /// Android and iOS devices.
    pub fn rotatable_fullscreen() -> bool {
        matches!(env::consts::OS, "android" | "ios")
    }

    fn toggle_fullscreen(&mut self) {
        if Self::rotatable_fullscreen() {
            return;
        }

        let new_fullscreen = !self.fullscreen;
        let mode = if new_fullscreen {
            sdl2::video::FullscreenType::Desktop
        } else {
            sdl2::video::FullscreenType::Off
        };
        match self.window.set_fullscreen(mode) {
            Ok(()) => {
                self.fullscreen = new_fullscreen;
                #[cfg(not(any(target_os = "android", target_os = "ios")))]
                self.aspect_ratio_constraint.set(
                    self.preserve_aspect_ratio && !self.fullscreen,
                    size_for_orientation_from_size(
                        self.screen_size(),
                        self.device_orientation,
                        self.scale_hack,
                    ),
                );
                log!(
                    "Switched to {} mode.",
                    if self.fullscreen {
                        "fullscreen"
                    } else {
                        "windowed"
                    }
                );
            }
            Err(error) => {
                log!("Could not toggle fullscreen mode: {error}");
            }
        }
    }

    /// Create the window, retrying with native GLES if bundled ANGLE is unavailable
    /// or unsafe.
    pub fn new(
        title: &str,
        icon: Option<Image>,
        launch_image: Option<(Image, bool)>,
        options: &Options,
    ) -> Result<Window, String> {
        crate::gles::perf_overlay::configure_from_options(options);
        #[cfg(target_os = "android")]
        {
            let angle_unavailable = ANDROID_ANGLE_UNAVAILABLE.load(Ordering::Relaxed);
            if should_attempt_android_angle(options.gles_native, angle_unavailable) {
                if configure_android_angle_driver(true) {
                    match Self::new_with_configured_driver(
                        title,
                        icon.clone(),
                        launch_image.clone(),
                        options,
                    ) {
                        Ok(window) if window.is_angle_backend() => {
                            ANDROID_ANGLE_UNAVAILABLE.store(false, Ordering::Relaxed);
                            return Ok(window);
                        }
                        Ok(window) => {
                            let driver = window.gl_driver_description().to_owned();
                            ANDROID_ANGLE_UNAVAILABLE.store(true, Ordering::Relaxed);
                            log!(
                                "ANGLE was requested, but SDL activated a non-ANGLE driver ({}). Retrying with native GLES.",
                                driver
                            );
                            drop(window);
                            configure_android_angle_driver(false);
                            return Self::new_with_configured_driver(
                                title,
                                icon,
                                launch_image,
                                options,
                            )
                            .map_err(|native_error| {
                                format!(
                                    "ANGLE activated a non-ANGLE driver ({}); native GLES fallback failed ({})",
                                    driver, native_error
                                )
                            });
                        }
                        Err(angle_error) => {
                            ANDROID_ANGLE_UNAVAILABLE.store(true, Ordering::Relaxed);
                            log!(
                                "Bundled ANGLE could not be used: {}. Retrying with the Android native GLES driver.",
                                angle_error
                            );
                            configure_android_angle_driver(false);
                            return Self::new_with_configured_driver(
                                title,
                                icon,
                                launch_image,
                                options,
                            )
                            .map_err(|native_error| {
                                format!(
                                    "Bundled ANGLE could not be used ({}); native GLES fallback failed ({})",
                                    angle_error, native_error
                                )
                            });
                        }
                    }
                }

                ANDROID_ANGLE_UNAVAILABLE.store(true, Ordering::Relaxed);
                return Self::new_with_configured_driver(title, icon, launch_image, options)
                    .map_err(|native_error| {
                        format!(
                            "Bundled ANGLE is unavailable and native GLES initialization failed: {}",
                            native_error
                        )
                    });
            }

            if !options.gles_native && angle_unavailable {
                log!(
                    "Bundled ANGLE was previously unavailable; using the Android native GLES driver."
                );
            }
            configure_android_angle_driver(false);
        }

        Self::new_with_configured_driver(title, icon, launch_image, options)
    }

    fn new_with_configured_driver(
        title: &str,
        icon: Option<Image>,
        launch_image: Option<(Image, bool)>,
        options: &Options,
    ) -> Result<Window, String> {
        let sdl_ctx = sdl2::init().map_err(|err| format!("SDL initialization failed: {}", err))?;
        let video_ctx = sdl_ctx
            .video()
            .map_err(|err| format!("SDL video initialization failed: {}", err))?;

        // The "hidapi" feature of rust-sdl2 is enabled so that sdl2::sensor
        // is available, but we don't want to enable SDL's HIDAPI controller
        // drivers because they cause duplicated controllers on macOS
        // (https://github.com/libsdl-org/SDL/issues/7479). Once that's fixed,
        // remove this (https://github.com/touchHLE/touchHLE/issues/85).
        sdl2::hint::set("SDL_JOYSTICK_HIDAPI", "0");

        if env::consts::OS == "android" {
            // It's important to set context version BEFORE window creation
            // ref. https://wiki.libsdl.org/SDL2/SDL_GLattr
            let attr = video_ctx.gl_attr();
            attr.set_context_version(1, 1);
            attr.set_context_profile(sdl2::video::GLProfile::GLES);

            // Disable blocking of event loop when app is paused.
            sdl2::hint::set("SDL_ANDROID_BLOCK_ON_PAUSE", "0");
        }

        // Separate mouse and touch events in both SDL synthesis directions.
        // SDL_TOUCH_MOUSE_EVENTS=0 stops touch input from also generating mouse input.
        // SDL_MOUSE_TOUCH_EVENTS=0 stops mouse input from also generating touch input.
        // Without both, one physical press can become two UIKit touches on some hosts.
        sdl2::hint::set("SDL_TOUCH_MOUSE_EVENTS", "0");
        sdl2::hint::set("SDL_MOUSE_TOUCH_EVENTS", "0");

        // SDL2 disables the screen saver by default, but iPhone OS enables
        // the idle timer that triggers sleep by default, so we turn it back on
        // here, and then the app can disable it if it wants to.
        video_ctx.enable_screen_saver();

        // PERF: never request depth or stencil buffers for the window's own
        // framebuffer. The only things ever drawn to it are flat,
        // depth-untested textured quads (app presentation, splash screen, app
        // picker); the guest app itself renders into offscreen renderbuffers
        // with their own attachments. On tile-based mobile GPUs, skipping the
        // window depth/stencil buffers saves memory bandwidth on every swap
        // chain resolution. Note: must be set *before* window creation.
        {
            let attr = video_ctx.gl_attr();
            attr.set_depth_size(0);
            attr.set_stencil_size(0);
        }

        let scale_hack = options.scale_hack;
        let host_screen_size = options.host_screen_size.map(normalize_portrait_size);
        // TODO: some apps specify their orientation in Info.plist, we could use
        // that here.
        let device_family = options.device_family.unwrap_or(DeviceFamily::iPhone);
        let device_orientation = options.initial_orientation;
        let fullscreen = options.fullscreen;
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        let preserve_aspect_ratio = options.preserve_aspect_ratio;
        let portrait_screen_size =
            host_screen_size.unwrap_or_else(|| device_family.portrait_size());

        let mut window = if Self::rotatable_fullscreen() {
            // Without this, SDL will force fullscreen mode to be portrait.
            set_sdl2_orientation(device_orientation);
            let screen_size = video_ctx
                .display_bounds(0)
                .map_err(|err| format!("Could not query the display bounds: {}", err))?
                .size();
            let (width, height) = rotate_fullscreen_size(device_orientation, screen_size);
            let mut window_builder = video_ctx.window(title, width, height);
            window_builder.fullscreen().opengl();
            // Render at the device's native (Retina) resolution.
            #[cfg(target_os = "ios")]
            window_builder.allow_highdpi();
            let window = window_builder
                .build()
                .map_err(|err| format!("Could not create the SDL OpenGL window: {}", err))?;
            window
        } else if fullscreen {
            let (width, height) = video_ctx
                .display_bounds(0)
                .map_err(|err| format!("Could not query the display bounds: {}", err))?
                .size();
            let mut window_builder = video_ctx.window(title, width, height);
            window_builder.fullscreen_desktop().opengl();
            #[cfg(target_os = "macos")]
            window_builder.allow_highdpi();
            let window = window_builder
                .build()
                .map_err(|err| format!("Could not create the SDL OpenGL window: {}", err))?;
            window
        } else {
            let (width, height) = size_for_orientation_from_size(
                portrait_screen_size,
                device_orientation,
                scale_hack,
            );
            let mut window_builder = video_ctx.window(title, width, height);
            window_builder.position_centered().resizable().opengl();
            #[cfg(target_os = "macos")]
            window_builder.allow_highdpi();
            let window = window_builder
                .build()
                .map_err(|err| format!("Could not create the SDL OpenGL window: {}", err))?;
            window
        };

        if env::consts::OS == "android" {
            // Sanity check
            let gl_attr = video_ctx.gl_attr();
            debug_assert_eq!(gl_attr.context_profile(), sdl2::video::GLProfile::GLES);
            debug_assert_eq!(gl_attr.context_version(), (1, 1));
        }

        if let Some(icon) = icon {
            #[cfg(target_os = "macos")]
            let icon = pad_icon_for_dock(&icon);
            #[cfg(not(target_os = "macos"))]
            let icon = icon;
            window.set_icon(surface_from_image(&icon));
        } else {
            #[cfg(target_os = "macos")]
            restore_application_icon();
        }

        let event_pump = sdl_ctx
            .event_pump()
            .map_err(|err| format!("Could not initialize the SDL event pump: {}", err))?;

        let controller_ctx = sdl_ctx
            .game_controller()
            .map_err(|err| format!("Could not initialize SDL game controllers: {}", err))?;

        let sensor_ctx = sdl_ctx
            .sensor()
            .map_err(|err| format!("Could not initialize SDL sensors: {}", err))?;
        let mut accelerometer: Option<sdl2::sensor::Sensor> = None;
        let mut gyroscope: Option<sdl2::sensor::Sensor> = None;
        if let Ok(num_sensors) = sensor_ctx.num_sensors() {
            for sensor_idx in 0..num_sensors {
                if accelerometer.is_some() && gyroscope.is_some() {
                    break;
                }
                if let Ok(sensor) = sensor_ctx.open(sensor_idx) {
                    match sensor.sensor_type() {
                        sdl2::sensor::SensorType::Accelerometer => {
                            if accelerometer.is_none() {
                                log!("Accelerometer detected: {}.", sensor.name());
                                accelerometer = Some(sensor);
                            }
                        }
                        sdl2::sensor::SensorType::Gyroscope
                        | sdl2::sensor::SensorType::LeftGyroscope
                        | sdl2::sensor::SensorType::RightGyroscope => {
                            // Gyroscope was previously disabled here under the
                            // suspicion of a native abort; the real culprit was
                            // batteryLevel -> SDL_GetPowerInfo -> JNI (see
                            // get_battery_status cache below), so the sensor is
                            // back.
                            if gyroscope.is_none() {
                                log!("Gyroscope detected: {}.", sensor.name());
                                gyroscope = Some(sensor);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        // Populate the battery cache here, on the real SDLThread stack: guest
        // code later calls [UIDevice batteryLevel] from a coroutine stack, and
        // the JNI calls inside SDL_GetPowerInfo (Android_JNI_GetPowerInfo)
        // abort ART when made from that context (pending StackOverflowError).
        populate_battery_cache();

        // Likewise, resolve the WebView overlay bridge's JNI class and
        // method IDs here: FindClass needs the app class loader, which is
        // only reachable from this stack before guest code starts running
        // on a coroutine stack.
        crate::android_web_view::populate_jni_cache();

        // And for the camera/microphone bridge, whose HostMedia lookups need
        // the app class loader just as much as the WebView ones do.
        crate::android_media::populate_jni_cache();

        #[cfg(target_os = "macos")]
        let max_height = window.size().1;
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        let mut aspect_ratio_constraint = native_aspect_ratio::Constraint::new(&window);
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        aspect_ratio_constraint.set(
            preserve_aspect_ratio && !fullscreen,
            size_for_orientation_from_size(portrait_screen_size, device_orientation, scale_hack),
        );

        let (splash_image, splash_image_is_orientation_specific) = launch_image
            .map(|(image, orientation_specific)| (Some(image), orientation_specific))
            .unwrap_or((None, false));

        let mut window = Window {
            _sdl_ctx: sdl_ctx,
            video_ctx,
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            aspect_ratio_constraint,
            window,
            event_pump,
            event_queue: VecDeque::new(),
            auto_taps: {
                let mut taps = Vec::new();
                if std::env::var_os("TOUCHHLE_GD_AUTOPLAY").is_some() {
                    // (frame, x, y) in window-drawable coordinates. GD flow:
                    // main menu centre = play button; level select centre =
                    // level 1 card / Normal Mode.
                    taps = vec![
                        (600, -1.0, -1.0),
                        (660, -1.0, -1.0),
                        (760, -1.0, -1.0),
                        // Level select: tap the level card to start the level.
                        (1100, 240.0, 150.0),
                        (1250, 240.0, 150.0),
                    ];
                }
                taps
            },
            last_polled: Instant::now() - Duration::from_secs(1),
            high_priority_event: None,
            enable_event_polling: true,
            #[cfg(target_os = "macos")]
            max_height,
            #[cfg(target_os = "macos")]
            viewport_y_offset: 0,
            fullscreen,
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            preserve_aspect_ratio,
            scale_hack,
            host_screen_size,
            internal_gl_ins: None,
            perf_context_token: crate::gles::perf_overlay::allocate_context_token(),
            gl_driver_description: String::new(),
            host_framebuffer: 0,
            splash_image,
            splash_image_is_orientation_specific,
            device_family,
            device_orientation,
            controller_ctx,
            controllers: Vec::new(),
            dpad_state: DpadState {
                left: false,
                right: false,
                up: false,
                down: false,
                active: false,
            },
            stick_active: false,
            _sensor_ctx: sensor_ctx,
            accelerometer,
            gyroscope,
            virtual_cursor_last: None,
            virtual_cursor_last_unsticky: None,
            virtual_accelerometer_last: None,
            perf_hints: crate::perf_hints::PerfHints::new(
                options.perf_hints,
                options
                    .fps_limit
                    .map(|fps| Duration::from_secs_f64(1.0 / fps))
                    .unwrap_or(Duration::from_micros(16_667)),
                options.affinity.as_deref(),
            ),
            on_main_stack: true,
        };

        // Set up OpenGL ES context used for splash screen and app UI rendering
        // (see src/frameworks/core_animation/composition.rs). OpenGL ES is used
        // because SDL2 won't let us use more than one graphics API in the same
        // window, and we also need OpenGL ES for the app's own rendering.
        let mut gl_ins = if options.prefer_gles2_context {
            create_gles2_ctx_no_parent_stack(&mut window)
        } else {
            create_gles1_ctx_no_parent_stack(&mut window, options)
        }
        .map_err(|err| format!("Could not initialize an OpenGL ES context: {}", err))?;
        if options.trace_gl_errors {
            gl_ins = Box::new(LoggingGLESContext {
                inner: gl_ins,
                verbose: options.trace_gl_errors || options.verbose_gles,
            });
        }
        let (gl_driver_description, host_framebuffer) = {
            #[allow(unused_mut)]
            let mut gl_ctx = gl_ins.make_current(&mut window);
            #[allow(unused_mut)]
            let mut framebuffer = 0;
            // On iOS, SDL2 draws the window through a framebuffer object of
            // its own rather than framebuffer 0.
            #[cfg(target_os = "ios")]
            unsafe {
                gl_ctx.GetIntegerv(
                    crate::gles::gles11_raw::FRAMEBUFFER_BINDING_OES,
                    &mut framebuffer,
                );
            }
            (unsafe { gl_ctx.driver_description() }, framebuffer as u32)
        };
        log!("Driver info: {}", gl_driver_description);
        window.host_framebuffer = host_framebuffer;
        if env::consts::OS == "ios" {
            let (window_width, window_height) = window.window.size();
            let (drawable_width, drawable_height) = window.window.drawable_size();
            log!(
                "iOS host framebuffer: {}, window: {}×{}, Retina drawable: {}×{}",
                host_framebuffer,
                window_width,
                window_height,
                drawable_width,
                drawable_height,
            );
        }
        window.gl_driver_description = gl_driver_description;
        window.internal_gl_ins = Some(gl_ins);

        #[cfg(target_os = "android")]
        if angle_backend_unsafe_on_power_vr_bxm_8_256(&window.gl_driver_description) {
            return Err(format!(
                "ANGLE's Vulkan backend is disabled for PowerVR BXM-8-256 after its startup crash: {}",
                window.gl_driver_description
            ));
        }

        // The Android surface requests a 60 Hz mode, matching the default
        // frame limit. Synchronize swaps to that display cadence; explicit
        // --vsync=off remains available for drivers where interval 1 regresses.
        let swap_interval = swap_interval_for_mode(options.vsync, env::consts::OS == "android");
        if let Some(swap_interval) = swap_interval {
            let vsync_on = matches!(swap_interval, sdl2::video::SwapInterval::VSync);
            match window.video_ctx.gl_set_swap_interval(swap_interval) {
                Ok(()) => log!(
                    "Vsync {} (swap interval {}).",
                    if vsync_on { "enabled" } else { "disabled" },
                    if vsync_on { 1 } else { 0 }
                ),
                Err(e) => log!(
                    "Warning: could not {} vsync, leaving the driver's default swap interval: {}",
                    if vsync_on { "enable" } else { "disable" },
                    e
                ),
            }
        }

        // Detect the host GL stack once, up front, so we can auto-apply the
        // known Adreno black-screen workarounds. Android uses bundled ANGLE
        // only when its active renderer passes the startup compatibility check;
        // otherwise Window::new retries with the system OpenGL ES driver.
        window.log_gpu_backend_hints();

        if window.splash_image.is_some() {
            window.display_splash();
        }

        Ok(window)
    }

    /// Whether the host OpenGL stack is Google's ANGLE (OpenGL ES translated to
    /// Vulkan/Direct3D/Metal) rather than a vendor-native driver. Detected from
    /// the `GL_VERSION` / `GL_RENDERER` strings captured at context creation.
    pub fn is_angle_backend(&self) -> bool {
        self.gl_driver_description.contains("ANGLE")
    }

    /// Whether the host GPU is a Qualcomm Adreno part. Used to decide whether
    /// the Adreno-specific rendering workarounds should be auto-enabled.
    pub fn is_adreno_gpu(&self) -> bool {
        let d = &self.gl_driver_description;
        d.contains("Adreno") || d.contains("Qualcomm")
    }

    /// The cached `GL_VERSION / GL_VENDOR / GL_RENDERER` string for the internal
    /// OpenGL ES context.
    #[allow(dead_code)]
    pub fn gl_driver_description(&self) -> &str {
        &self.gl_driver_description
    }

    /// Log a one-line summary of the detected GPU backend and, on Adreno
    /// hardware, whether ANGLE is active. This makes the "black screen on
    /// Adreno" situation diagnosable straight from the log the user shares.
    fn log_gpu_backend_hints(&self) {
        if self.is_adreno_gpu() {
            if self.is_angle_backend() {
                log!(
                    "GPU backend: Qualcomm Adreno via ANGLE (recommended). \
                     ANGLE's lenient ES emulation avoids the native Adreno \
                     driver's black-screen issues."
                );
            } else {
                log!(
                    "GPU backend: Qualcomm Adreno native OpenGL ES driver. \
                     Bundled ANGLE could not be loaded; the native ES 1.1 path \
                     can render some early iPhone OS games as a black screen. \
                     Adreno rendering workarounds (--fix-texture-min-filter) \
                     are auto-enabled to mitigate this."
                );
            }
        }
    }

    /// Poll for events from the OS. This needs to be done reasonably often
    /// (60Hz is probably fine) so that the host OS doesn't consider touchHLE
    /// to be unresponsive. Note that events are not returned by this function,
    /// since we often need to defer actually handling them.
    ///
    /// Since polling can be quite expensive, this function will skip it if it
    /// was called too recently.
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    pub fn set_preserve_aspect_ratio(&mut self, enabled: bool) {
        self.preserve_aspect_ratio = enabled;
        self.aspect_ratio_constraint.set(
            enabled && !self.fullscreen,
            size_for_orientation_from_size(
                self.screen_size(),
                self.device_orientation,
                self.scale_hack,
            ),
        );
    }

    pub fn poll_for_events(&mut self, options: &Options) {
        if !self.on_main_stack {
            log!("Warning: poll_for_events called off main stack, skipping");
            return;
        }
        let now = Instant::now();
        // poll roughly twice per frame to try to avoid missing frames sometimes
        if now.duration_since(self.last_polled) < Duration::from_secs_f64(1.0 / 120.0) {
            return;
        }
        self.last_polled = now;

        fn transform_input_coords(
            window: &Window,
            (in_x, in_y): (f32, f32),
            independent_of_viewport: bool,
        ) -> (f32, f32) {
            let (vx, vy, vw, vh) = if independent_of_viewport {
                let (width, height) = size_for_orientation(
                    window.device_family,
                    window.device_orientation,
                    NonZeroU32::new(1).unwrap(),
                );
                (0, 0, width, height)
            } else {
                window.viewport()
            };
            // Clamp touches in letterbox bars to the nearest visible edge.
            let (x, y) = geometry::normalize_viewport_coords((in_x, in_y), (vx, vy, vw, vh));
            // `TOUCHHLE_LANDSCAPE_UISCREEN_BOUNDS=1` (e.g. Asphalt Xtreme) makes
            // UIScreen.bounds report the interface-oriented size, and the window
            // it is forced to match is landscape too. The presenter's device
            // rotation is cancelled by the autorotation transform the window
            // applies to the view, so the presented image is already in
            // interface space: the touch must be mapped into that same space,
            // without the portrait-space rotation below, and scaled by the
            // interface size rather than the portrait one.
            let interface_space = crate::env_flag_cached!("TOUCHHLE_LANDSCAPE_UISCREEN_BOUNDS");
            if interface_space {
                log_once!("TOUCHHLE_LANDSCAPE_UISCREEN_BOUNDS=1: mapping touches in interface space");
            }
            // rotate
            //
            // If the final EAGL presentation is not being rotated, the touch
            // path should not rotate either. Keep hit-testing in the normal
            // 320x480 UIKit space so EAGLView still receives the event; a
            // separate UITouch locationInView compatibility path can remap
            // the coordinates returned to the game.
            // PERF: cache the read-once debug toggles; this runs per SDL
            // touch event, and each std::env::var_os is a global-lock environ
            // scan with allocation.
            let [x, y] = if interface_space {
                // Logged above; the presented frame is already in interface space.
                [x, y]
            } else if crate::env_flag_cached!("TOUCHHLE_DISABLE_PRESENT_ROTATION")
                || crate::env_flag_cached!("TOUCHHLE_DISABLE_TOUCH_ROTATION")
            {
                log_once!("TOUCHHLE_DISABLE_TOUCH_ROTATION: not rotating touch hit-test coordinates");
                [x, y]
            } else {
                let matrix = window.rotation_matrix().inverse().unwrap();
                matrix.transform([x, y])
            };

            // back to pixels
            let (out_w, out_h) = if interface_space {
                size_for_orientation_from_size(
                    window.screen_size(),
                    window.device_orientation,
                    NonZeroU32::new(1).unwrap(),
                )
            } else {
                window.size_unrotated_unscaled()
            };
            let mut out_x = (x + 0.5) * out_w as f32;
            let mut out_y = (y + 0.5) * out_h as f32;

            // Optional hit-test tuning only. Do not use these unless you are
            // deliberately testing the UIKit hit-test position.
            if let Some(offset) = crate::env_var_cached!("TOUCHHLE_HITTEST_X_OFFSET") {
                if let Ok(offset) = offset.parse::<f32>() {
                    out_x += offset;
                }
            }
            if let Some(offset) = crate::env_var_cached!("TOUCHHLE_HITTEST_Y_OFFSET") {
                if let Ok(offset) = offset.parse::<f32>() {
                    out_y += offset;
                }
            }

            // Keep the result strictly *inside* the iOS window. CGRect
            // containment is half-open on the high edge (a point with
            // y == bounds.size.height is *outside*), so clamping to the
            // exclusive size of the window — e.g. y == 480 on an iPhone
            // 480-pt landscape screen — would still cause -[UIWindow
            // pointInside:] to return false. Subtract a single point.
            let max_x = (out_w.saturating_sub(1)) as f32;
            let max_y = (out_h.saturating_sub(1)) as f32;
            let out_x = out_x.clamp(0.0, max_x);
            let out_y = out_y.clamp(0.0, max_y);
            // Round to match touch precision of official devices.
            (out_x.round(), out_y.round())
        }
        fn transform_virt_accel_coords(window: &Window, (in_x, in_y): (i32, i32)) -> (f32, f32) {
            let coords = window.mouse_drawable_coords((in_x, in_y));
            let (x, y) = geometry::normalize_viewport_coords(coords, window.viewport());
            (x * 2.0, y * 2.0)
        }
        fn translate_button(button: sdl2::controller::Button) -> Option<crate::options::Button> {
            match button {
                sdl2::controller::Button::DPadLeft => Some(crate::options::Button::DPadLeft),
                sdl2::controller::Button::DPadUp => Some(crate::options::Button::DPadUp),
                sdl2::controller::Button::DPadRight => Some(crate::options::Button::DPadRight),
                sdl2::controller::Button::DPadDown => Some(crate::options::Button::DPadDown),
                sdl2::controller::Button::Start => Some(crate::options::Button::Start),
                sdl2::controller::Button::A => Some(crate::options::Button::A),
                sdl2::controller::Button::B => Some(crate::options::Button::B),
                sdl2::controller::Button::X => Some(crate::options::Button::X),
                sdl2::controller::Button::Y => Some(crate::options::Button::Y),
                sdl2::controller::Button::LeftShoulder => {
                    Some(crate::options::Button::LeftShoulder)
                }
                _ => None,
            }
        }
        fn finger_absolute_coords(window: &Window, (x, y): (f32, f32)) -> (f32, f32) {
            let (screen_width, screen_height) = window.window.drawable_size();
            (screen_width as f32 * x, screen_height as f32 * y)
        }

        let mut controller_updated = false;
        // event_pump doesn't have a method to peek on events
        // so, we keep track of an unconsumed one from a previous loop iteration
        // FIXME: use peek_event() from even_subsystem
        let mut previous_event: Option<sdl2::event::Event> = None;
        while self.enable_event_polling {
            use sdl2::event::Event as E;
            let event = if let Some(e) = previous_event.take() {
                match e {
                    E::Unknown { .. } => (),
                    _ => log_dbg!("Consuming previous event: {:?}", e),
                }
                e
            } else if let Some(e) = self.event_pump.poll_event() {
                match e {
                    E::Unknown { .. } => (),
                    _ => log_dbg!("Consuming new event: {:?}", e),
                }
                e
            } else {
                break;
            };

            // Virtual accelerometer
            match event {
                E::MouseButtonDown {
                    x,
                    y,
                    mouse_btn: MouseButton::Right,
                    ..
                } => {
                    let (x, y) = transform_virt_accel_coords(self, (x, y));
                    self.virtual_accelerometer_last = Some((x, y, true));
                }
                E::MouseMotion {
                    x, y, mousestate, ..
                } => {
                    if mousestate.right() {
                        let (x, y) = transform_virt_accel_coords(self, (x, y));
                        self.virtual_accelerometer_last = Some((x, y, true));
                    }
                }
                E::MouseButtonUp {
                    x,
                    y,
                    mouse_btn: MouseButton::Right,
                    ..
                } => {
                    let (x, y) = transform_virt_accel_coords(self, (x, y));
                    self.virtual_accelerometer_last = Some((x, y, false));
                }
                _ => {}
            }

            self.event_queue.push_back(match event {
                E::Quit { .. } => Event::Quit,
                E::Window {
                    win_event: sdl2::event::WindowEvent::Close,
                    ..
                } => Event::CloseWindow,
                E::MouseButtonDown {
                    x,
                    y,
                    mouse_btn: MouseButton::Left,
                    ..
                } => {
                    let abs_coords = self.mouse_drawable_coords((x, y));
                    if crate::quick_menu::touch_down(abs_coords, self.viewport()) {
                        continue;
                    }
                    if crate::trainer_ui::touch_down(abs_coords, self.viewport()) {
                        continue;
                    }
                    let coords = transform_input_coords(self, abs_coords, false);
                    log_dbg!("MouseButtonDown x {}, y {}, coords {:?}", x, y, coords);
                    if crate::env_flag_cached!("TOUCHHLE_TRACE_TOUCHES") {
                        log!("SDL mouse-down delivered at ({}, {}) -> {:?}", x, y, coords);
                    }
                    Event::TouchesDown(HashMap::from([(FingerId::Mouse, coords)]))
                }
                E::MouseMotion {
                    x, y, mousestate, ..
                } if mousestate.left() => {
                    let abs_coords = self.mouse_drawable_coords((x, y));
                    if crate::quick_menu::touch_motion(abs_coords, self.viewport()) {
                        continue;
                    }
                    if crate::trainer_ui::touch_motion(abs_coords, self.viewport()) {
                        continue;
                    }
                    let coords = transform_input_coords(self, abs_coords, false);
                    log_dbg!("MouseMotion x {}, y {}, coords {:?}", x, y, coords);
                    Event::TouchesMove(HashMap::from([(FingerId::Mouse, coords)]))
                }
                E::MouseButtonUp {
                    x,
                    y,
                    mouse_btn: MouseButton::Left,
                    ..
                } => {
                    let abs_coords = self.mouse_drawable_coords((x, y));
                    if crate::quick_menu::touch_up(abs_coords, self.viewport()) {
                        continue;
                    }
                    if crate::trainer_ui::touch_up(abs_coords, self.viewport()) {
                        continue;
                    }
                    let coords = transform_input_coords(self, abs_coords, false);
                    log_dbg!("MouseButtonUp x {}, y {}, coords {:?}", x, y, coords);
                    Event::TouchesUp(HashMap::from([(FingerId::Mouse, coords)]))
                }
                E::ControllerDeviceAdded { which, .. } => {
                    self.controller_added(which);
                    continue;
                }
                E::ControllerDeviceRemoved { which, .. } => {
                    self.controller_removed(which);
                    continue;
                }
                // Note that accelerometer simulation with analog sticks is
                // handled with polling, rather than being event-based.
                E::ControllerButtonUp { button, .. } | E::ControllerButtonDown { button, .. } => {
                    controller_updated = true;
                    let Some(button) = translate_button(button) else {
                        continue;
                    };
                    // Called whenever a DPad direction is pressed or released
                    if (button == crate::options::Button::DPadLeft
                        || button == crate::options::Button::DPadUp
                        || button == crate::options::Button::DPadRight
                        || button == crate::options::Button::DPadDown)
                        && options.dpad_to_touch.is_some()
                    {
                        let Some((x, y, w, h)) = options.dpad_to_touch else {
                            // We already checked dpad_to_touch.is_some() in
                            // the surrounding if-condition, but be defensive
                            // against future refactors and bail out instead
                            // of panicking the host.
                            log!(
                                "Warning: dpad_to_touch became None between outer check and inner destructure; ignoring D-pad event."
                            );
                            continue;
                        };

                        // Update held state
                        let pressed = matches!(event, E::ControllerButtonDown { .. });
                        match button {
                            crate::options::Button::DPadLeft => self.dpad_state.left = pressed,
                            crate::options::Button::DPadRight => self.dpad_state.right = pressed,
                            crate::options::Button::DPadUp => self.dpad_state.up = pressed,
                            crate::options::Button::DPadDown => self.dpad_state.down = pressed,
                            _ => {
                                // The outer `if` already restricts us to the
                                // four D-pad buttons; if a new variant slips
                                // through after a refactor, just ignore it
                                // instead of crashing.
                                log!(
                                    "Warning: unexpected button {:?} reached D-pad arm; ignoring.",
                                    button
                                );
                                continue;
                            }
                        }

                        // Compute center
                        let cx = x + w * 0.5;
                        let cy = y + h * 0.5;

                        // Compute combined delta
                        let mut dx = 0.0;
                        let mut dy = 0.0;

                        if self.dpad_state.left {
                            dx -= 0.5 * w;
                        }
                        if self.dpad_state.right {
                            dx += 0.5 * w;
                        }
                        if self.dpad_state.up {
                            dy -= 0.5 * h;
                        }
                        if self.dpad_state.down {
                            dy += 0.5 * h;
                        }

                        // Final coords: center + movement
                        let coords = transform_input_coords(self, (cx + dx, cy + dy), true);

                        // Send TouchDown if any dpad is held, TouchUp if none
                        let any_held = self.dpad_state.left
                            || self.dpad_state.right
                            || self.dpad_state.up
                            || self.dpad_state.down;

                        if !self.dpad_state.active && any_held {
                            // New touch
                            self.dpad_state.active = true;
                            Event::TouchesDown(HashMap::from([(FingerId::DpadToTouch, coords)]))
                        } else if self.dpad_state.active && any_held {
                            // Move existing touch
                            Event::TouchesMove(HashMap::from([(FingerId::DpadToTouch, coords)]))
                        } else if self.dpad_state.active && !any_held {
                            // Release touch
                            self.dpad_state.active = false;
                            Event::TouchesUp(HashMap::from([(FingerId::DpadToTouch, coords)]))
                        } else {
                            continue;
                        }
                    } else {
                        let Some(&(x, y)) = options.button_to_touch.get(&button) else {
                            continue;
                        };
                        match event {
                            E::ControllerButtonUp { .. } => {
                                let coords = transform_input_coords(self, (x, y), true);
                                Event::TouchesUp(HashMap::from([(
                                    FingerId::ButtonToTouch(button),
                                    coords,
                                )]))
                            }
                            E::ControllerButtonDown { .. } => {
                                let coords = transform_input_coords(self, (x, y), true);
                                Event::TouchesDown(HashMap::from([(
                                    FingerId::ButtonToTouch(button),
                                    coords,
                                )]))
                            }
                            _ => {
                                // Outer arm only matches ControllerButton{Up,Down}.
                                log!(
                                    "Warning: unexpected event {:?} in button-to-touch arm; ignoring.",
                                    event
                                );
                                continue;
                            }
                        }
                    }
                }
                E::ControllerAxisMotion { axis, .. } => {
                    controller_updated = true;
                    let Some((x, y, w, h)) = options.stick_to_touch else {
                        continue;
                    };
                    if axis == sdl2::controller::Axis::LeftX
                        || axis == sdl2::controller::Axis::LeftY
                    {
                        let (stick_x, stick_y, _) = self.get_controller_stick(options, true);
                        let coords = transform_input_coords(
                            self,
                            (
                                x + ((stick_x + 1.0) / 2.0) * w,
                                y + ((stick_y + 1.0) / 2.0) * h,
                            ),
                            true,
                        );
                        if stick_x.abs() < options.deadzone && stick_y.abs() < options.deadzone {
                            if !self.stick_active {
                                // Ignore deadzone events when stick is inactive
                                continue;
                            } else {
                                // Release touch when stick returns to deadzone
                                self.stick_active = false;
                                Event::TouchesUp(HashMap::from([(FingerId::StickToTouch, coords)]))
                            }
                        } else if !self.stick_active {
                            // New touch
                            self.stick_active = true;
                            Event::TouchesDown(HashMap::from([(FingerId::StickToTouch, coords)]))
                        } else {
                            // Move existing touch
                            Event::TouchesMove(HashMap::from([(FingerId::StickToTouch, coords)]))
                        }
                    } else {
                        continue;
                    }
                }
                E::AppWillEnterBackground { .. } => {
                    log_dbg!("Received app-will-resign-active event.");
                    Event::AppWillResignActive
                }
                E::AppDidEnterBackground { .. } => {
                    log_dbg!("Received app-did-enter-background event.");
                    Event::AppDidEnterBackground
                }
                E::AppWillEnterForeground { .. } => {
                    log_dbg!("Received app-will-enter-foreground event.");
                    Event::AppWillEnterForeground
                }
                E::AppDidEnterForeground { .. } => {
                    log_dbg!("Received app-did-become-active event.");
                    Event::AppDidBecomeActive
                }
                E::AppTerminating { .. } => {
                    log_dbg!("Received app-will-terminate event.");
                    assert!(self.high_priority_event.is_none());
                    self.high_priority_event = Some(Event::AppWillTerminate);
                    // App is about to be killed by the OS. Stop polling so we
                    // can return to the run loop and dispatch the high
                    // priority event quickly, before the process is
                    // terminated.
                    self.enable_event_polling = false;
                    continue;
                }
                E::FingerUp {
                    timestamp,
                    finger_id,
                    x,
                    y,
                    ..
                }
                | E::FingerMotion {
                    timestamp,
                    finger_id,
                    x,
                    y,
                    ..
                }
                | E::FingerDown {
                    timestamp,
                    finger_id,
                    x,
                    y,
                    ..
                } => {
                    log_dbg!("Starting multi-touch for {:?}", event);
                    // To implement multi-touch we accumulate here same touch
                    // events at the same timestamp. This is consistent with
                    // UIKit, but could be broken if events come out of order.
                    // (in worst case we separate multi-touches in several ones)
                    // TODO: handle out of order touches
                    let curr_timestamp = timestamp;
                    let abs_coords = finger_absolute_coords(self, (x, y));
                    // The quick menu's button and panel get first dibs,
                    // then the trainer overlay (Cheat Engine-style).
                    let quick_menu_consumed = match event {
                        E::FingerDown { .. } => {
                            crate::quick_menu::touch_down(abs_coords, self.viewport())
                        }
                        E::FingerUp { .. } => {
                            crate::quick_menu::touch_up(abs_coords, self.viewport())
                        }
                        _ => crate::quick_menu::touch_motion(abs_coords, self.viewport()),
                    };
                    let trainer_consumed = match event {
                        E::FingerDown { .. } => {
                            crate::trainer_ui::touch_down(abs_coords, self.viewport())
                        }
                        E::FingerUp { .. } => {
                            crate::trainer_ui::touch_up(abs_coords, self.viewport())
                        }
                        _ => crate::trainer_ui::touch_motion(abs_coords, self.viewport()),
                    };
                    let coords = transform_input_coords(self, abs_coords, false);
                    log_dbg!("Finger event x {}, y {}, coords {:?}", x, y, coords);
                    let mut map = if quick_menu_consumed || trainer_consumed {
                        HashMap::new()
                    } else {
                        HashMap::from([(FingerId::Touch(finger_id), coords)])
                    };
                    while let Some(next) = self.event_pump.poll_event() {
                        match next {
                            E::Unknown { .. } => (),
                            _ => log_dbg!("Next possible multi-touch event: {:?}", next),
                        }
                        match next {
                            E::FingerUp {
                                timestamp,
                                finger_id,
                                x,
                                y,
                                ..
                            }
                            | E::FingerMotion {
                                timestamp,
                                finger_id,
                                x,
                                y,
                                ..
                            }
                            | E::FingerDown {
                                timestamp,
                                finger_id,
                                x,
                                y,
                                ..
                            } if timestamp == curr_timestamp && next.is_same_kind_as(&event) => {
                                let abs_coords = finger_absolute_coords(self, (x, y));
                                let trainer_consumed = match next {
                                    E::FingerDown { .. } => {
                                        crate::trainer_ui::touch_down(abs_coords, self.viewport())
                                    }
                                    E::FingerUp { .. } => {
                                        crate::trainer_ui::touch_up(abs_coords, self.viewport())
                                    }
                                    _ => {
                                        crate::trainer_ui::touch_motion(abs_coords, self.viewport())
                                    }
                                };
                                let coords = transform_input_coords(self, abs_coords, false);
                                if !trainer_consumed {
                                    map.insert(FingerId::Touch(finger_id), coords);
                                }
                            }
                            E::MultiGesture { timestamp, .. } if timestamp == curr_timestamp => {
                                // TODO: handle gestures
                                continue;
                            }
                            _ => {
                                // event_pump doesn't have a method to peek on
                                // events, so we keep track of an unconsumed
                                // one from a previous loop iteration
                                assert!(previous_event.is_none());
                                previous_event = Some(next);
                                break;
                            }
                        }
                    }
                    log_dbg!("Finishing multi-touch for {:?} with {:?}", event, map);
                    match event {
                        E::FingerUp { .. } => Event::TouchesUp(map),
                        E::FingerMotion { .. } => Event::TouchesMove(map),
                        E::FingerDown { .. } => Event::TouchesDown(map),
                        _ => {
                            // Outer arm matches FingerUp/Motion/Down only.
                            log!(
                                "Warning: unexpected event {:?} in multi-touch arm; treating as TouchesMove.",
                                event
                            );
                            Event::TouchesMove(map)
                        }
                    }
                }
                E::KeyDown {
                    keycode: Some(sdl2::keyboard::Keycode::F11),
                    repeat: false,
                    ..
                } => {
                    self.toggle_fullscreen();
                    continue;
                }
                E::KeyDown {
                    keycode: Some(sdl2::keyboard::Keycode::Return),
                    keymod,
                    repeat: false,
                    ..
                } if keymod
                    .intersects(sdl2::keyboard::Mod::LALTMOD | sdl2::keyboard::Mod::RALTMOD) =>
                {
                    self.toggle_fullscreen();
                    continue;
                }
                E::KeyDown {
                    keycode: Some(sdl2::keyboard::Keycode::F1),
                    repeat: false,
                    ..
                } => {
                    let enabled = !crate::gles::present::onscreen_fps_enabled();
                    self.set_show_fps_counter(enabled);
                    echo!("Performance display {}", if enabled { "enabled" } else { "disabled" });
                    continue;
                }
                E::KeyDown {
                    keycode: Some(sdl2::keyboard::Keycode::F2),
                    repeat: false,
                    ..
                } => {
                    let corner = crate::gles::present::cycle_onscreen_fps_corner();
                    echo!("Performance display moved to {corner}");
                    continue;
                }
                E::KeyDown {
                    keycode: Some(sdl2::keyboard::Keycode::F12),
                    ..
                } => {
                    // Log this so you can tell when touchHLE has received
                    // the event but it's stuck in the queue.
                    echo!("F12 pressed, EnterDebugger event queued.");
                    Event::EnterDebugger
                }
                E::KeyDown {
                    keycode: Some(sdl2::keyboard::Keycode::Backspace),
                    ..
                } => {
                    log_dbg!("SDL TextInput Backspace");
                    Event::TextInput(TextInputEvent::Backspace)
                }
                E::KeyDown {
                    keycode: Some(sdl2::keyboard::Keycode::Left),
                    repeat: false,
                    ..
                } => Event::ArrowKey(-1),
                E::KeyDown {
                    keycode: Some(sdl2::keyboard::Keycode::Right),
                    repeat: false,
                    ..
                } => Event::ArrowKey(1),
                E::MouseWheel { y, .. } => Event::MouseWheel(y),
                E::KeyDown {
                    keycode: Some(sdl2::keyboard::Keycode::Return),
                    ..
                } => {
                    log_dbg!("SDL TextInput Return");
                    Event::TextInput(TextInputEvent::Return)
                }
                E::TextInput { text, .. } => {
                    log_dbg!("SDL TextInput {}", text);
                    Event::TextInput(TextInputEvent::Text(text))
                }
                _ => continue,
            })
        }

        if controller_updated {
            let (new_x, new_y, pressed, pressed_changed, moved) =
                self.update_virtual_cursor(options);
            self.event_queue
                .push_back(match (pressed, pressed_changed, moved) {
                    (true, true, _) => {
                        let coords = transform_input_coords(self, (new_x, new_y), false);
                        Event::TouchesDown(HashMap::from([(FingerId::VirtualCursor, coords)]))
                    }
                    (false, true, _) => {
                        let coords = transform_input_coords(self, (new_x, new_y), false);
                        Event::TouchesUp(HashMap::from([(FingerId::VirtualCursor, coords)]))
                    }
                    (true, _, true) => {
                        let coords = transform_input_coords(self, (new_x, new_y), false);
                        Event::TouchesMove(HashMap::from([(FingerId::VirtualCursor, coords)]))
                    }
                    _ => return,
                });
        }
    }

    /// Pop an event from the queue (in FIFO order, except for high priority
    /// events)
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    pub fn close_requested(&self) -> bool {
        matches!(self.high_priority_event.as_ref(), Some(Event::CloseWindow))
            || self.event_queue.iter().any(|event| matches!(event, Event::CloseWindow))
    }

    pub fn pop_event(&mut self) -> Option<Event> {
        // TEST HOOK: GD autoplay taps for verifying level music headlessly.
        if !self.auto_taps.is_empty() {
            static POP_FRAMES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let frame = POP_FRAMES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let due: Vec<(f32, f32)> = self
                .auto_taps
                .iter()
                .filter(|(tap_frame, _, _)| *tap_frame == frame)
                .map(|(_, x, y)| (*x, *y))
                .collect();
            for (x, y) in due {
                let (w, h) = self.window.drawable_size();
                let x = if x < 0.0 { w as f32 / 2.0 } else { x };
                let y = if y < 0.0 { h as f32 / 2.0 } else { y };
                let coords = (x, y);
                log!(
                    "GD autoplay tap at frame {} -> drawable ({:.0},{:.0}) coords {:?}",
                    frame,
                    x,
                    y,
                    coords
                );
                self.event_queue
                    .push_back(Event::TouchesDown(HashMap::from([(
                        FingerId::Mouse,
                        coords,
                    )])));
                self.event_queue
                    .push_back(Event::TouchesUp(HashMap::from([(FingerId::Mouse, coords)])));
            }
            self.auto_taps
                .retain(|(tap_frame, _, _)| *tap_frame > frame);
        }
        self.high_priority_event
            .take()
            .or_else(|| self.event_queue.pop_front())
    }

    fn controller_added(&mut self, joystick_idx: u32) {
        let Ok(controller) = self.controller_ctx.open(joystick_idx) else {
            log!("Warning: A new controller was connected, but it couldn't be accessed!");
            return;
        };

        let controller_name = controller.name();
        if env::consts::OS == "android" && controller_name.starts_with("uinput-") {
            log!("ignoring fingerprint device: {}", controller_name);
            return;
        }
        log!(
            "New controller connected: {}. Left stick = device tilt. Right stick = touch input (press the stick or shoulder button to tap/hold).",
            controller_name
        );
        self.controllers.push(controller);
    }
    fn controller_removed(&mut self, instance_id: u32) {
        let Some(idx) = self
            .controllers
            .iter()
            .position(|controller| controller.instance_id() == instance_id)
        else {
            return;
        };
        let controller = self.controllers.remove(idx);
        log!("Warning: Controller disconnected: {}", controller.name());
    }
    pub fn print_accelerometer_notice(&self, options: &Options) {
        log!("This app uses the accelerometer.");

        if !self.controllers.is_empty() && options.analog_stick_tilt_controls {
            log!("Your connected controller's left analog stick will be used for accelerometer simulation.");
            if self.accelerometer.is_some() {
                log!("Disconnect the controller if you want to use your device's accelerometer.");
            }
        } else if self.accelerometer.is_some() {
            log!("Your device's accelerometer will be used for accelerometer simulation.");
            if options.analog_stick_tilt_controls {
                log!("Connect a controller if you would prefer to use an analog stick.");
            }
        } else if self.controllers.is_empty() && options.analog_stick_tilt_controls {
            log!("Connect a controller to get accelerometer simulation.");
        }

        if self.accelerometer.is_none() {
            log!(
                "You can {}hold right click and move the cursor to simulate the accelerometer.",
                if options.analog_stick_tilt_controls {
                    "also "
                } else {
                    ""
                }
            );
        }
    }

    /// Get the real or simulated accelerometer output.
    /// See also [crate::frameworks::uikit::ui_accelerometer].
    pub fn get_acceleration(&self, options: &Options) -> (f32, f32, f32) {
        if self.controllers.is_empty() || !options.analog_stick_tilt_controls {
            if let Some(ref accelerometer) = self.accelerometer {
                let data = accelerometer.get_data().unwrap();
                let sdl2::sensor::SensorData::Accel(data) = data else {
                    // We asked SDL for the accelerometer sensor explicitly
                    // earlier; if SDL handed us a different sensor variant
                    // (driver bug, future SDL version, etc.), keep the host
                    // running by reporting the device flat-on-its-back
                    // (UIAcceleration neutral position).
                    log!(
                        "Warning: accelerometer sensor returned non-Accel data ({:?}); reporting neutral acceleration.",
                        data
                    );
                    return (0.0, 0.0, -1.0);
                };
                let [x, y, z] = data;
                // UIAcceleration reports acceleration towards gravity, but SDL2
                // reports acceleration away from gravity.
                let (x, y, z) = (-x, -y, -z);
                // UIAcceleration reports acceleration in units of g-force, but
                // SDL2 reports acceleration in units of m/s^2.
                let gravity: f32 = 9.80665; // SDL_STANDARD_GRAVITY
                let (x, y, z) = (x / gravity, y / gravity, z / gravity);
                return accelerometer_compat_remap(x, y, z);
            }
        }

        let (x, y) = if self
            .virtual_accelerometer_last
            .is_some_and(|(_x, _y, right_click_hold)| right_click_hold)
        {
            self.virtual_accelerometer_last
                .map(|(x, y, _right_click_hold)| (x, y))
                .unwrap()
        } else {
            // Get left analog stick input. The range is [-1, 1] on each axis.
            let (x, y, _) = self.get_controller_stick(options, true);
            (x, y)
        };

        // Correct for window rotation
        let [x, y] = self.rotation_matrix().inverse().unwrap().transform([x, y]);
        let (x, y) = (x.clamp(-1.0, 1.0), y.clamp(-1.0, 1.0)); // just in case

        // Let's simulate tilting the device based on the analog stick inputs.
        //
        // If an iPhone is lying flat on its back, level with the ground, and it
        // is on Earth, the accelerometer will report approximately (0, 0, -1).
        // The acceleration x and y axes are aligned with the screen's x and y
        // axes. +x points to the right of the screen, +y points to the top of
        // the screen, and +z points away from the screen. In the example
        // scenario, the z axis is parallel to gravity.

        let gravity: [f32; 3] = [0.0, 0.0, -1.0];

        let neutral_x = options.x_tilt_offset.to_radians();
        let neutral_y = options.y_tilt_offset.to_radians();
        let x_rotation_range = options.x_tilt_range.to_radians() / 2.0;
        let y_rotation_range = options.y_tilt_range.to_radians() / 2.0;
        // (x, y) are swapped because the controller Y axis usually corresponds
        // to forward/backward movement, but rotating about the Y axis means
        // tilting the device left/right.
        let x_rotation = neutral_x - x_rotation_range * y;
        let y_rotation = neutral_y - y_rotation_range * x;
        let matrix =
            Matrix::<3>::y_rotation(y_rotation).multiply(&Matrix::<3>::x_rotation(x_rotation));
        let [x, y, z] = matrix.transform(gravity);

        (x, y, z)
    }

    /// Returns [true] if the host device provides a gyroscope sensor.
    pub fn has_gyroscope(&self) -> bool {
        self.gyroscope.is_some()
    }

    /// Get the host gyroscope rotation rate, in radians per second, in the
    /// device coordinate frame (+x right of the screen, +y top of the screen,
    /// +z away from the screen), matching the axes of CMRotationRate.
    /// SDL reports gyroscope data in radians per second relative to the
    /// device axes, so no unit or axis conversion is needed.
    /// See also [crate::frameworks::core_motion].
    pub fn get_rotation_rate(&self) -> Option<(f32, f32, f32)> {
        let gyroscope = self.gyroscope.as_ref()?;
        let data = gyroscope.get_data().ok()?;
        let sdl2::sensor::SensorData::Gyro(data) = data else {
            // We asked SDL for the gyroscope sensor explicitly earlier; if
            // SDL handed us a different sensor variant (driver bug, future
            // SDL version, etc.), treat the sensor as unavailable.
            log!(
                "Warning: gyroscope sensor returned non-Gyro data ({:?}); ignoring.",
                data
            );
            return None;
        };
        let [x, y, z] = data;
        Some((x, y, z))
    }

    /// For use when redrawing the screen: Get the cached on-screen position and
    /// press state of the analog stick-controlled virtual cursor, if it is
    /// visible.
    pub fn virtual_cursor_visible_at(&self) -> Option<(f32, f32, bool)> {
        let (x, y, pressed, visible) = self.virtual_cursor_last?;
        if visible {
            // When stickyness is in use, the visual cursor movement appears
            // uncomfortably choppy. Showing the un-sticky position is a bit
            // misleading but it *feels* better, and it is documented.
            if let Some((x_unsticky, y_unsticky, _time)) = self.virtual_cursor_last_unsticky {
                Some((x_unsticky, y_unsticky, pressed))
            } else {
                Some((x, y, pressed))
            }
        } else {
            None
        }
    }

    /// Update the virtual cursor's position, click state and visibility, then
    /// return the new position, pressed state, whether the press state changed
    /// and whether the cursor moved.
    fn update_virtual_cursor(&mut self, options: &Options) -> (f32, f32, bool, bool, bool) {
        // Get right analog stick input. The range is [-1, 1] on each axis.
        let (x, y, pressed) = self.get_controller_stick(options, false);

        // The cursor is intended to only show up once you move the analog stick
        // out of its deadzone, or while the button is held.
        let visible = pressed || x != 0.0 || y != 0.0;

        // Though the analog stick output fits within a square, its actual range
        // is usually a circle enclosed by the square. So we need to cut out the
        // rectangular shape of the screen from that circle within the square.
        let (vx, vy, vw, vh) = self.viewport();
        let (vx, vy, vw, vh) = (vx as f32, vy as f32, vw as f32, vh as f32);

        let (x, y) = {
            // Use Pythagoras's theorem to find the largest size the rectangle
            // can have within the circle.
            let ratio = vw / vh;
            let rect_height = (ratio * ratio + 1.0).powf(-0.5);
            let rect_width = ratio * rect_height;

            let x_abs = x.abs().min(rect_width) / rect_width;
            let y_abs = y.abs().min(rect_height) / rect_height;
            (x_abs.copysign(x), y_abs.copysign(y))
        };

        // Convert to on-screen window co-ordinates
        let x = (x / 2.0 + 0.5) * vw + vx;
        let y = (y / 2.0 + 0.5) * vh + vy;

        let (old_x, old_y, old_pressed, _old_visible) =
            self.virtual_cursor_last.unwrap_or_default();

        let (x, y) = if let Some((smoothing_strength, sticky_radius)) =
            options.stabilize_virtual_cursor
        {
            let new_time = Instant::now();

            let (old_x_unsticky, old_y_unsticky, old_time) = self
                .virtual_cursor_last_unsticky
                .unwrap_or((0.0, 0.0, new_time));

            let delta_t = new_time.saturating_duration_since(old_time).as_secs_f32();

            // Apply a feedback-based smoothing with exponential decay, to try
            // to dampen shakiness in the stick movement.

            let smooth = |old: f32, new: f32| -> f32 {
                if smoothing_strength != 0.0 {
                    let lerp_factor = 1.0 - (0.5_f32).powf(delta_t * (1.0 / smoothing_strength));
                    old + (new - old) * lerp_factor
                } else {
                    new
                }
            };

            let new_x_unsticky = smooth(old_x_unsticky, x);
            let new_y_unsticky = smooth(old_y_unsticky, y);

            self.virtual_cursor_last_unsticky = Some((new_x_unsticky, new_y_unsticky, new_time));

            // Make the reported position "sticky" within a certain radius, i.e.
            // if the new position's distance from the old one is within the
            // radius, report no change in position.

            if (new_x_unsticky - old_x).hypot(new_y_unsticky - old_y) < sticky_radius {
                (old_x, old_y)
            } else {
                (new_x_unsticky, new_y_unsticky)
            }
        } else {
            (x, y)
        };

        self.virtual_cursor_last = Some((x, y, pressed, visible));

        (
            x,
            y,
            pressed,
            pressed != old_pressed,
            x != old_x || y != old_y,
        )
    }

    /// Get the summed X and Y positions and button state of the left or right
    /// analog stick of the game controllers. Each axis value is in the range
    /// [-1, 1].
    fn get_controller_stick(&self, options: &Options, left: bool) -> (f32, f32, bool) {
        fn convert_axis(axis: i16, deadzone: f32) -> f32 {
            assert!(deadzone >= 0.0);
            let axis = ((axis as f32) / (i16::MAX as f32)).clamp(-1.0, 1.0);
            let abs_axis = (axis.abs().max(deadzone) - deadzone) / (1.0 - deadzone);
            abs_axis.copysign(axis)
        }

        let (mut x, mut y) = (0.0, 0.0);
        let mut pressed = false;
        for controller in &self.controllers {
            use sdl2::controller::{Axis, Button};
            let (x_axis, y_axis, button1, button2) = if left {
                (
                    Axis::LeftX,
                    Axis::LeftY,
                    Button::LeftStick,
                    Button::LeftShoulder,
                )
            } else {
                (
                    Axis::RightX,
                    Axis::RightY,
                    Button::RightStick,
                    Button::RightShoulder,
                )
            };
            x += convert_axis(controller.axis(x_axis), options.deadzone);
            y += convert_axis(controller.axis(y_axis), options.deadzone);
            pressed |= controller.button(button1);
            pressed |= controller.button(button2);
        }
        let (x, y) = (x.clamp(-1.0, 1.0), y.clamp(-1.0, 1.0));

        (x, y, pressed)
    }

    pub fn create_gl_context(&self, version: GLVersion) -> Result<GLContext, String> {
        let attr = self.video_ctx.gl_attr();
        match version {
            GLVersion::GLES11 => {
                attr.set_context_version(1, 1);
                attr.set_context_profile(sdl2::video::GLProfile::GLES);
            }
            GLVersion::GLES20 => {
                attr.set_context_version(2, 0);
                attr.set_context_profile(sdl2::video::GLProfile::GLES);
            }
            GLVersion::GLES30 => {
                attr.set_context_version(3, 0);
                attr.set_context_profile(sdl2::video::GLProfile::GLES);
            }
            GLVersion::GL21Compat => {
                attr.set_context_version(2, 1);
                attr.set_context_profile(sdl2::video::GLProfile::Compatibility);
            }
            GLVersion::GL33Core => {
                attr.set_context_version(3, 3);
                attr.set_context_profile(sdl2::video::GLProfile::Core);
            }
        }

        let gl_ctx = self.window.gl_create_context()?;

        Ok(GLContext(gl_ctx))
    }

    pub fn gl_get_proc_address(&self, procname: &str) -> *const std::ffi::c_void {
        // For some reason, rust-sdl2 uses *const (), but () is not meant to be
        // used for void pointees (just void results), so let's fix that.
        self.video_ctx.gl_get_proc_address(procname) as *const _
    }

    pub fn set_share_with_current_context(&self, value: bool) {
        self.video_ctx
            .gl_attr()
            .set_share_with_current_context(value)
    }

    pub unsafe fn make_gl_context_current(&self, gl_ctx: &GLContext) {
        self.window.gl_make_current(&gl_ctx.0).unwrap();
    }

    /// Make the internal OpenGL ES context (for splash screen and UI rendering)
    /// current.
    #[must_use]
    pub fn make_internal_gl_ctx_current<'win>(&'win mut self) -> Box<dyn GLES + 'win> {
        // The invariant is held up here - since the instance we return is
        // bound to the lifetime of window, it can't outlive the internal GL
        // context and can't outlive the window.
        let gl_ins = unsafe {
            self.internal_gl_ins
                .as_mut()
                .unwrap()
                .make_current_unchecked_for_window(
                    &mut |gl_ctx| self.window.gl_make_current(&gl_ctx.0).unwrap(),
                    &mut |s| self.video_ctx.gl_get_proc_address(s) as *const _,
                )
        };
        gl_ins
    }

    fn display_splash(&mut self) {
        assert!(self.splash_image.is_some());

        let image = self.splash_image.as_ref().unwrap();
        let (image_width, image_height) = image.dimensions();

        // Legacy iPhone landscape launch images are stored in a portrait-sized
        // Default.png with their content already rotated. Applying the normal
        // framebuffer rotation to such an image turns it upside down, so use
        // the inverse rotation for that case. Native landscape-sized launch
        // images and portrait launch images use the regular transform.
        let is_landscape = matches!(
            self.device_orientation,
            DeviceOrientation::LandscapeLeft | DeviceOrientation::LandscapeRight
        );
        let rotation = if self.splash_image_is_orientation_specific {
            Matrix::identity()
        } else if is_landscape && image_height > image_width {
            self.rotation_matrix().inverse().unwrap()
        } else {
            self.rotation_matrix()
        };

        // OpenGL ES expects bottom-to-top row order for image data, but our
        // image data will be top-to-bottom. A reflection transform compensates.
        let matrix = rotation.multiply(&Matrix::y_flip());
        let (vx, vy, vw, vh) = self.viewport();
        let viewport = (vx, vy + self.viewport_y_offset(), vw, vh);

        unsafe {
            let mut gl_ctx = self
                .internal_gl_ins
                .as_mut()
                .unwrap()
                .make_current_unchecked_for_window(
                    &mut |gl_ctx| self.window.gl_make_current(&gl_ctx.0).unwrap(),
                    &mut |s| self.video_ctx.gl_get_proc_address(s) as *const _,
                );

            use crate::gles::gles11_raw as gles11; // constants only

            let mut texture = 0;
            gl_ctx.GenTextures(1, &mut texture);
            gl_ctx.BindTexture(gles11::TEXTURE_2D, texture);
            gl_ctx.TexImage2D(
                gles11::TEXTURE_2D,
                0,
                gles11::RGBA as _,
                image_width as _,
                image_height as _,
                0,
                gles11::RGBA,
                gles11::UNSIGNED_BYTE,
                image.pixels().as_ptr() as *const _,
            );
            gl_ctx.TexParameteri(
                gles11::TEXTURE_2D,
                gles11::TEXTURE_MIN_FILTER,
                gles11::LINEAR as _,
            );
            gl_ctx.TexParameteri(
                gles11::TEXTURE_2D,
                gles11::TEXTURE_MAG_FILTER,
                gles11::LINEAR as _,
            );

            present_frame(
                gl_ctx.as_mut(),
                viewport,
                matrix,
                /* virtual_cursor_visible_at: */ None,
                self.perf_context_token,
            );

            gl_ctx.DeleteTextures(1, &texture);
        };

        self.window.gl_swap_window();

        // hold onto GL context so the image doesn't disappear, and hold
        // onto image so we can rotate later if necessary
    }

    /// Swap front-buffer and back-buffer so the result of OpenGL rendering is
    /// presented.
    pub fn swap_window(&mut self) {
        self.window.gl_swap_window();
        self.perf_hints.frame_presented();
        crate::gles::perf_overlay::record_host_present();

        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        if let Some(summary) = crate::gles::perf_overlay::take_title_bar_summary() {
            let title = format!("{} - {}", base_window_title(), summary);
            let _ = self.window.set_title(&title);
        }
    }

    /// Consider the emulated device to be rotated to a particular orientation.
    ///
    /// On a PC or laptop, this will make the window be rotated so the app
    /// content appears upright. On a mobile device, this might do something
    /// else, because the user can physically rotate the screen.
    pub fn rotate_device(&mut self, new_orientation: DeviceOrientation) {
        if !self.on_main_stack {
            log!("Warning: rotate_device called off main stack, skipping");
            return;
        }
        if new_orientation == self.device_orientation {
            return;
        }

        if !self.fullscreen && !Self::rotatable_fullscreen() {
            let (width, height) = if Self::rotatable_fullscreen() {
                set_sdl2_orientation(new_orientation);
                rotate_fullscreen_size(new_orientation, self.window.size())
            } else {
                size_for_orientation(self.device_family, new_orientation, self.scale_hack)
            };

            // macOS quirk: when resizing the window, the new framebuffer's size
            // is apparently max(new_size, old_size) in each dimension, but the
            // viewport is positioned wrong on the y axis for some reason, so we
            // need to apply an offset.
            // Recreating the OpenGL context was an alternative workaround, but
            // that apparently stops other OpenGL contexts drawing to the
            // framebuffer!
            #[cfg(target_os = "macos")]
            {
                let (_old_width, old_height) = self.window.size();
                self.max_height = self.max_height.max(old_height).max(height);
                self.viewport_y_offset = self.max_height - height;
            }

            self.window.set_size(width, height).unwrap();
        }

        if Self::rotatable_fullscreen() {
            set_sdl2_orientation(new_orientation);
            // Hack: from reading SDL2's source code, it seems that SDL2 will
            // only re-do the orientation when changing whether a window is
            // "resizeable" (can be rotated). You can't set the resizeable state
            // on a fullscreen window, so it must be temporarily stop being
            // fulscreen.
            // Apparently, doing this does result in resizing the window.
            // None of this is allowed to be fatal. On iOS the host UI decides
            // whether the interface may rotate, and if it has not rotated yet,
            // SDL2 rejects the new display mode with "Screen orientation does
            // not match display mode size". That used to panic and take the
            // whole game down mid-play; a rotation that does not happen is far
            // better than that, and the next attempt can still succeed.
            if let Err(err) = self.window.set_fullscreen(sdl2::video::FullscreenType::Off) {
                log!("Warning: couldn't leave fullscreen to rotate: {}", err);
            }
            unsafe {
                let window_raw = self.window.raw();
                sdl2_sys::SDL_SetWindowResizable(window_raw, sdl2_sys::SDL_bool::SDL_FALSE);
                sdl2_sys::SDL_SetWindowResizable(window_raw, sdl2_sys::SDL_bool::SDL_TRUE);
            }
            if let Err(err) = self
                .window
                .set_fullscreen(sdl2::video::FullscreenType::True)
            {
                log!(
                    "Warning: couldn't return to fullscreen after rotating to {:?}: {}. \
                     The host UI may not permit this orientation.",
                    new_orientation,
                    err
                );
            }
        }

        self.device_orientation = new_orientation;
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        self.aspect_ratio_constraint.set(
            self.preserve_aspect_ratio && !self.fullscreen,
            size_for_orientation_from_size(
                self.screen_size(),
                self.device_orientation,
                self.scale_hack,
            ),
        );

        if self.splash_image.is_some() {
            self.display_splash();
        }
    }

    pub fn device_family(&self) -> DeviceFamily {
        self.device_family
    }

    pub fn screen_size(&self) -> (u32, u32) {
        self.host_screen_size
            .unwrap_or_else(|| self.device_family.portrait_size())
    }

    pub fn screen_scale(&self) -> f32 {
        if self.host_screen_size.is_some() {
            1.0
        } else {
            self.device_family.scale_factor()
        }
    }

    /// Returns the current device orientation
    pub fn current_rotation(&self) -> DeviceOrientation {
        self.device_orientation
    }

    /// Get the size in pixels of the window without rotation or scaling.
    ///
    /// The aspect ratio, scale and orientation reflect the guest app's view of
    /// the world.
    pub fn size_unrotated_unscaled(&self) -> (u32, u32) {
        size_for_orientation_from_size(
            self.screen_size(),
            DeviceOrientation::Portrait,
            NonZeroU32::new(1).unwrap(),
        )
    }

    /// Get the region of the on-screen window (x, y, width, height) used to
    /// display the app content.
    ///
    /// The scale and aspect ratio of this region depend on the host window and
    /// the desktop aspect-ratio setting; its orientation follows the guest app.
    pub fn viewport(&self) -> (u32, u32, u32, u32) {
        let (app_width, app_height) = size_for_orientation_from_size(
            self.screen_size(),
            self.device_orientation,
            self.scale_hack,
        );
        // Use the full drawable when the native constraint is active and the
        // current client size still matches it. Programmatic resizes, Snap,
        // maximize and window managers that ignore hints fall back to fitting
        // the guest image. Check the drawable on macOS too: its title bar and
        // backing-pixel rounding can leave the content a pixel off the ratio.
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        {
            let drawable_size = self.window.drawable_size();
            let native_ratio_matches = {
                let lhs = drawable_size.0 as u64 * app_height as u64;
                let rhs = drawable_size.1 as u64 * app_width as u64;
                lhs.abs_diff(rhs) <= app_width.max(app_height) as u64
            };
            return if self.preserve_aspect_ratio {
                if self.aspect_ratio_constraint.is_native() && native_ratio_matches {
                    (0, 0, drawable_size.0, drawable_size.1)
                } else {
                    geometry::fit_viewport((app_width, app_height), drawable_size)
                }
            } else {
                (0, 0, drawable_size.0, drawable_size.1)
            };
        }

        // Fit the actual drawable in windowed mode too: the compositor may
        // resize the window or give it more pixels than its logical size.
        geometry::fit_viewport((app_width, app_height), self.window.drawable_size())
    }

    /// SDL mouse coordinates must use the same pixel space as the viewport.
    fn mouse_drawable_coords(&self, (x, y): (i32, i32)) -> (f32, f32) {
        geometry::window_to_drawable(
            (x as f32, y as f32),
            self.window.size(),
            self.window.drawable_size(),
        )
    }

    /// Map a guest-space `CGRect` (points, possibly rotated/letterboxed) to
    /// host window pixels `(x, y, w, h)` for host-side overlays (native
    /// Android webviews etc.). Returns `w`/`h` of 0 when nothing is visible.
    pub fn guest_frame_to_window_px(
        &self,
        frame: crate::frameworks::core_graphics::CGRect,
    ) -> (i32, i32, i32, i32) {
        let (vp_x, vp_y, vp_w, vp_h) = self.viewport();
        let (app_w, app_h) = self.size_unrotated_unscaled();
        let (app_w, app_h) = (app_w as f32, app_h as f32);
        if app_w <= 0.0 || app_h <= 0.0 || vp_w == 0 || vp_h == 0 {
            return (0, 0, 0, 0);
        }
        let sx = vp_w as f32 / app_w;
        let sy = vp_h as f32 / app_h;
        // Rotate the guest rect into the device orientation, then scale to
        // viewport pixels (mirrors the composition rotation).
        let (x, y, w, h) = match self.device_orientation {
            crate::window::DeviceOrientation::Portrait => (
                frame.origin.x as f32,
                frame.origin.y as f32,
                frame.size.width as f32,
                frame.size.height as f32,
            ),
            crate::window::DeviceOrientation::PortraitUpsideDown => (
                app_w - frame.origin.x as f32 - frame.size.width as f32,
                app_h - frame.origin.y as f32 - frame.size.height as f32,
                frame.size.width as f32,
                frame.size.height as f32,
            ),
            crate::window::DeviceOrientation::LandscapeLeft => (
                frame.origin.y as f32,
                app_w - frame.origin.x as f32 - frame.size.width as f32,
                frame.size.height as f32,
                frame.size.width as f32,
            ),
            crate::window::DeviceOrientation::LandscapeRight => (
                app_h - frame.origin.y as f32 - frame.size.height as f32,
                frame.origin.x as f32,
                frame.size.height as f32,
                frame.size.width as f32,
            ),
        };
        let px = (vp_x as f32 + x * sx).round() as i32;
        let py = (vp_y as f32 + y * sy).round() as i32;
        let pw = (w * sx).round() as i32;
        let ph = (h * sy).round() as i32;
        (px, py, pw.max(0), ph.max(0))
    }

    pub fn host_framebuffer(&self) -> u32 {
        self.host_framebuffer
    }

    pub(crate) fn perf_context_token(&self) -> usize {
        self.perf_context_token
    }

    /// Special offset to add to y co-ordinates, only when drawing to screen.
    pub fn viewport_y_offset(&self) -> u32 {
        #[cfg(target_os = "macos")]
        return if self.fullscreen {
            self.viewport_y_offset
        } else {
            0
        };
        #[cfg(not(target_os = "macos"))]
        return 0;
    }

    /// Transformation matrix for transforming between the window's co-ordinate
    /// space and the app's original co-ordinate space when rotation is in use
    /// (see [Self::rotate_device]). This returns a matrix appropriate for
    /// rotating texture co-ordinates to display the image in the window; when
    /// rotating input co-ordinates, invert the matrix.
    pub fn rotation_matrix(&self) -> Matrix<2> {
        match self.device_orientation {
            DeviceOrientation::Portrait => Matrix::identity(),
            DeviceOrientation::PortraitUpsideDown => Matrix::z_rotation(PI),
            DeviceOrientation::LandscapeLeft => Matrix::z_rotation(-FRAC_PI_2),
            DeviceOrientation::LandscapeRight => Matrix::z_rotation(FRAC_PI_2),
        }
    }

    pub fn is_screen_saver_enabled(&self) -> bool {
        self.video_ctx.is_screen_saver_enabled()
    }
    pub fn set_screen_saver_enabled(&mut self, enabled: bool) {
        if !self.on_main_stack {
            log!("Warning: set_screen_saver_enabled called off main stack, skipping");
            return;
        }
        match enabled {
            true => self.video_ctx.enable_screen_saver(),
            false => self.video_ctx.disable_screen_saver(),
        }
    }

    pub fn start_text_input(&self) {
        if !self.on_main_stack {
            log!("Warning: start_text_input called off main stack, skipping");
            return;
        }
        unsafe {
            sdl2_sys::SDL_StartTextInput();
        }
    }
    pub fn stop_text_input(&self) {
        if !self.on_main_stack {
            log!("Warning: stop_text_input called off main stack, skipping");
            return;
        }
        unsafe {
            sdl2_sys::SDL_StopTextInput();
        }
    }

    pub fn is_text_input_active(&self) -> bool {
        if !self.on_main_stack {
            log!("Warning: is_text_input_active called off main stack, returning false");
            return false;
        }
        unsafe { sdl2_sys::SDL_IsTextInputActive() == sdl2_sys::SDL_bool::SDL_TRUE }
    }

    pub fn is_screen_keyboard_shown(&self) -> bool {
        if !self.on_main_stack {
            log!("Warning: is_screen_keyboard_shown called off main stack, returning false");
            return false;
        }
        unsafe {
            sdl2_sys::SDL_IsScreenKeyboardShown(self.window.raw()) == sdl2_sys::SDL_bool::SDL_TRUE
        }
    }

    pub fn on_main_stack(&self) -> bool {
        self.on_main_stack
    }

    pub fn set_show_fps_counter(&mut self, enabled: bool) {
        crate::gles::perf_overlay::set_enabled(enabled);
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        if !enabled || crate::gles::perf_overlay::mode() != crate::options::PerformanceOverlayMode::TitleBar {
            let _ = self.window.set_title(&base_window_title());
        }
    }

    pub fn set_performance_overlay_settings(
        &mut self,
        enabled: bool,
        mode: PerformanceOverlayMode,
        corner: PerformanceOverlayCorner,
    ) {
        crate::gles::perf_overlay::configure(enabled, mode, corner);
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        if !enabled || mode != PerformanceOverlayMode::TitleBar {
            let _ = self.window.set_title(&base_window_title());
        }
    }
}

pub fn open_url(env: &mut Environment, url: &str) -> Result<(), String> {
    // On Android prefer our own JNI path to the system browser: SDL's
    // SDL_OpenURL goes through SDLActivity.openURL, which works, but doing
    // it directly keeps the intent flags predictable and avoids issues when
    // the app process is paused on return. Non-Android platforms keep using
    // SDL_OpenURL (desktop shell handlers).
    #[cfg(target_os = "android")]
    {
        // For content:// URIs from our DocumentsProvider, use the dedicated
        // file-manager path that grants URI permissions and sets the correct
        // MIME type (vnd.android.document/directory). The generic browser
        // path doesn't grant permissions, so the system file manager would be
        // denied access and the intent would silently fail.
        if url.starts_with("content://") {
            if crate::android_web_view::open_content_uri(url) {
                return Ok(());
            }
            // Fall through to try the generic path as well before giving up.
            if crate::android_web_view::open_url_external(url) {
                return Ok(());
            }
        } else if crate::android_web_view::open_url_external(url) {
            return Ok(());
        }
    }
    env.on_parent_stack_in_coroutine(|_, _| sdl2::url::open_url(url).map_err(|e| e.to_string()))
}

/// Open a content:// or file:// URI in the system file manager / browser.
/// On Android this ensures URI permissions are granted for DocumentsProvider
/// URIs. On other platforms it is identical to `open_url`.
pub fn open_file_manager_url(env: &mut Environment, url: &str) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        if url.starts_with("content://") {
            if crate::android_web_view::open_content_uri(url) {
                return Ok(());
            }
        }
    }
    open_url(env, url)
}

/// SDL COMMAND_USER value used to ask MainActivity to open the system file
/// picker for adding an .ipa file. Must be >= 0x8000 (see SDLActivity.java).
#[cfg(target_os = "android")]
const ADD_IPA_COMMAND: u32 = 0x8000;

// SDL_AndroidSendMessage is only available in SDL builds for Android.
#[cfg(target_os = "android")]
extern "C" {
    fn SDL_AndroidSendMessage(command: u32, param: i32) -> i32;
}

/// Ask the Java side to open the system file picker for adding an .ipa file
/// (see MainActivity.onUnhandledMessage). On other platforms, fall back to
/// opening the apps directory in the file manager.
///
/// Unlike [open_url], this never opens a URL, so it is safe to call from the
/// app picker without risking a failed system intent.
pub fn launch_ipa_picker(env: &mut Environment) -> Result<(), String> {
    // Like other SDL calls from the app picker, this must run on the main
    // stack (see on_parent_stack_in_coroutine), otherwise Android can
    // misbehave or crash.
    let result = env.on_parent_stack_in_coroutine(|_, _| {
        #[cfg(target_os = "android")]
        {
            unsafe { SDL_AndroidSendMessage(ADD_IPA_COMMAND, 0) }
        }
        #[cfg(not(target_os = "android"))]
        {
            -1
        }
    });
    if result == 0 {
        return Ok(());
    }
    // Fall back to opening the apps directory in the file manager, so a
    // .ipa file can still be added manually.
    match crate::paths::url_for_opening_apps_dir() {
        Ok(url) => open_url(env, &url),
        Err(e) => Err(format!(
            "SDL_AndroidSendMessage returned {result}, and opening the \
             apps directory failed too: {e}"
        )),
    }
}

/// Show an SDL messagebox for an error (typically after a panic).
///
/// The window argument allows for passing in the parent window for the
/// messagebox, which is not required but should be done if possible.
pub fn show_error_messagebox(window: Option<&Window>, error_message: &str) {
    if window.is_some_and(|win| !win.on_main_stack) {
        log!("Warning: show_error_messagebox called off main stack, skipping");
        return;
    }
    use sdl2::messagebox;
    let mbox = [
        messagebox::ButtonData {
            flags: messagebox::MessageBoxButtonFlag::NOTHING,
            button_id: 0,
            text: "Open touchHLE directory",
        },
        messagebox::ButtonData {
            flags: messagebox::MessageBoxButtonFlag::NOTHING,
            button_id: 1,
            text: "Close",
        },
    ];

    let Ok(clicked_button) = messagebox::show_message_box(
        messagebox::MessageBoxFlag::ERROR,
        &mbox,
        "touchHLE crashed!",
        &format!("touchHLE crashed with the following error: {error_message}"),
        window.map(|win| &win.window),
        None,
    ) else {
        log!("Warning: Failed to show error message box; falling back to stderr only.");
        eprintln!("touchHLE crashed: {}", error_message);
        return;
    };

    match clicked_button {
        messagebox::ClickedButton::CloseButton => {}
        messagebox::ClickedButton::CustomButton(button) => {
            match button.button_id {
                // Open data directory (contains log file on android)
                0 => match crate::paths::url_for_opening_user_data_dir() {
                    Ok(url) => {
                        let open_res = {
                            #[cfg(target_os = "android")]
                            {
                                if url.starts_with("content://")
                                    && crate::android_web_view::open_content_uri(&url)
                                {
                                    Ok(())
                                } else {
                                    sdl2::url::open_url(&url).map_err(|e| e.to_string())
                                }
                            }
                            #[cfg(not(target_os = "android"))]
                            {
                                sdl2::url::open_url(&url).map_err(|e| e.to_string())
                            }
                        };
                        if let Err(e) = open_res {
                            echo!("Couldn't open file manager at {:?}: {}", url, e);
                        } else {
                            echo!("Opened file manager at {:?}, exiting.", url);
                        }
                    }
                    Err(e) => echo!("Couldn't open file manager: {}", e),
                },
                // Close
                1 => {}
                _ => {
                    // SDL_ShowMessageBox should never return an unknown id
                    // for the buttons we configured, but be defensive.
                    log!("Warning: unexpected message-box button id; ignoring.");
                }
            }
        }
    }
}

/// Get current battery state from SDL2.
///
/// Battery state is cached at window creation and refreshed by
/// [refresh_battery_cache]. Guest code runs on a coroutine stack where JNI
/// calls abort on Android (see [populate_battery_cache]), so
/// [UIDevice batteryLevel] must never reach SDL_GetPowerInfo directly.
static BATTERY_CACHED: AtomicBool = AtomicBool::new(false);
static BATTERY_PERCENT: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);
static BATTERY_STATE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// Query SDL for the battery. Only call this where JNI calls are safe (the
/// real SDLThread stack), see [populate_battery_cache].
fn read_battery_status() -> (i32, BatteryState) {
    let mut pct = 0;
    // Unfortunately, Rust-SDL2 does not expose this function yet.
    // iPhoneOS does not measure the battery in seconds remaining,
    // so we discard this argument.
    let status = unsafe { sdl2_sys::SDL_GetPowerInfo(null_mut(), &mut pct) };
    (pct, BatteryState::from_sdl(status))
}

/// Read the battery once, from the real SDLThread stack. JNI (Android) is only
/// safe here — this runs before guest emulation starts on a coroutine stack.
pub fn populate_battery_cache() {
    refresh_battery_cache();
}

/// Re-read the battery and update the cache.
///
/// Like [populate_battery_cache], this must be called from a stack where JNI
/// calls are safe on Android (the SDL thread), e.g. through
/// [crate::Environment::on_parent_stack_in_coroutine]. The app picker uses it
/// to keep its status bar battery indicator up to date.
pub fn refresh_battery_cache() {
    let (pct, state) = read_battery_status();
    BATTERY_PERCENT.store(pct, Ordering::Relaxed);
    BATTERY_STATE.store(state.tag(), Ordering::Relaxed);
    BATTERY_CACHED.store(true, Ordering::Release);
}

/// The cached battery reading, if one has been taken yet.
///
/// This never calls into SDL, so it is safe from any thread or coroutine. The
/// value is a percentage (0 to 100, or -1 if unknown) and the state.
pub fn cached_battery_status() -> Option<(i32, BatteryState)> {
    BATTERY_CACHED.load(Ordering::Acquire).then(|| {
        (
            BATTERY_PERCENT.load(Ordering::Relaxed),
            BatteryState::from_tag(BATTERY_STATE.load(Ordering::Relaxed)),
        )
    })
}

pub fn get_battery_status() -> (i32, BatteryState) {
    if let Some(status) = cached_battery_status() {
        return status;
    }
    // Not cached yet (headless/window-less use): query directly.
    read_battery_status()
}

pub fn get_preferred_language_codes(env: &mut Environment) -> Vec<String> {
    // In headless mode there is no window, and the parent-stack machinery
    // [Environment::on_parent_stack_in_coroutine] relies on requires one. The
    // closure below doesn't actually use the window, but routing through it
    // would still unwrap the absent window and panic. There is no meaningful
    // user locale to report without a session anyway, so report no preference
    // and let the caller fall back to its default (English).
    if env.window.is_none() {
        return Vec::new();
    }
    env.on_parent_stack_in_coroutine(|_, _| {
        sdl2::locale::get_preferred_locales()
            .map(|loc| loc.lang)
            .collect()
    })
}

pub fn get_preferred_country_codes(env: &mut Environment) -> Vec<String> {
    // See the note in `get_preferred_language_codes` about headless mode.
    if env.window.is_none() {
        return Vec::new();
    }
    env.on_parent_stack_in_coroutine(|_, _| {
        sdl2::locale::get_preferred_locales()
            .filter_map(|loc| loc.country)
            .collect()
    })
}

/// Show a UIAlertView-style dialog using SDL2 message box.
/// Returns the index of the clicked button, or 0 if closed.
pub fn show_alert_dialog(
    env: &mut Environment,
    title: &str,
    message: &str,
    buttons: &[&str],
) -> i32 {
    let title = title.to_string();
    let message = message.to_string();
    let buttons: Vec<String> = buttons.iter().map(|s| s.to_string()).collect();

    env.on_parent_stack_in_coroutine(move |window, _options| {
        use sdl2::messagebox;

        let button_data: Vec<messagebox::ButtonData> = buttons
            .iter()
            .enumerate()
            .map(|(i, text)| messagebox::ButtonData {
                flags: messagebox::MessageBoxButtonFlag::NOTHING,
                button_id: i as i32,
                text: text.as_str(),
            })
            .collect();

        match messagebox::show_message_box(
            messagebox::MessageBoxFlag::INFORMATION,
            &button_data,
            &title,
            &message,
            Some(&window.window),
            None,
        ) {
            Ok(messagebox::ClickedButton::CustomButton(btn)) => btn.button_id,
            _ => 0,
        }
    })
}


#[cfg(test)]
mod device_family_tests {
    use super::DeviceFamily;

    /// Apple iPhone 6 Tech Specs: 4.7-inch 1334x750 display (375x667 points at
    /// scale 2), 1 GiB of RAM, hw.machine iPhone7,2.
    /// <https://support.apple.com/kb/SP705>
    #[test]
    fn iphone_6_reports_iphone_6_specs() {
        let family = DeviceFamily::try_from("iphone7,2").unwrap();
        assert_eq!(family, DeviceFamily::iPhone6);
        assert_eq!(family.display_name(), "iPhone 6");
        assert_eq!(family.option_name(), "iphone-6");
        assert_eq!(family.machine_name(), "iPhone7,2");
        assert_eq!(family.portrait_size(), (375, 667));
        assert_eq!(family.scale_factor(), 2.0);
        assert_eq!(family.physical_memory(), 1024 * 1024 * 1024u64);
        assert!(family.is_retina());
        assert!(family.is_phone_667());
        assert!(!family.is_phone_568());
        assert!(!family.is_ipad());
        assert!(!family.is_ipod_touch());
    }

    /// The RAM the hardware actually shipped with, which both the guest's
    /// memory-reporting APIs and the Default heap ceiling are derived from.
    /// <https://en.wikipedia.org/wiki/List_of_iOS_and_iPadOS_devices>
    #[test]
    fn every_model_reports_the_ram_it_shipped_with() {
        const MIB: u64 = 1024 * 1024;
        for (family, mib) in [
            (DeviceFamily::iPhone, 128),
            (DeviceFamily::iPhone3G, 128),
            (DeviceFamily::iPhone3GS, 256),
            (DeviceFamily::iPhone4, 512),
            (DeviceFamily::iPhone4s, 512),
            (DeviceFamily::iPhone5, 1024),
            (DeviceFamily::iPhone5c, 1024),
            (DeviceFamily::iPhone6, 1024),
            (DeviceFamily::iPad, 256),
            (DeviceFamily::iPad2, 512),
            (DeviceFamily::iPad3, 1024),
            (DeviceFamily::iPad4, 1024),
            (DeviceFamily::iPad5, 2048),
            (DeviceFamily::iPadMini, 512),
            (DeviceFamily::iPadMini2, 1024),
            (DeviceFamily::iPadMini3, 1024),
            (DeviceFamily::iPodTouch, 128),
            (DeviceFamily::iPodTouch2, 128),
            (DeviceFamily::iPodTouch3, 256),
            (DeviceFamily::iPodTouch4, 256),
            (DeviceFamily::iPodTouch5, 512),
        ] {
            assert_eq!(
                family.physical_memory(),
                mib * MIB,
                "{} should report {mib} MiB",
                family.option_name()
            );
        }
    }

    #[test]
    fn every_selectable_model_round_trips_through_its_option_name() {
        for family in DeviceFamily::ALL_SELECTABLE {
            assert_eq!(DeviceFamily::try_from(family.option_name()).unwrap(), *family);
        }
    }

    #[test]
    fn iphone_6_is_listed_right_after_iphone_5c() {
        let all = DeviceFamily::ALL_SELECTABLE;
        assert_eq!(all.len(), 21);
        assert_eq!(all[6], DeviceFamily::iPhone5c);
        assert_eq!(all[7], DeviceFamily::iPhone6);
        assert_eq!(all[8], DeviceFamily::iPad);
    }

    /// The app picker saves the chosen model as its tag, so tags must not
    /// renumber when the display order changes.
    #[test]
    fn selectable_tags_stay_stable() {
        assert_eq!(DeviceFamily::for_tag(0), Some(DeviceFamily::iPhone));
        assert_eq!(DeviceFamily::for_tag(5), Some(DeviceFamily::iPhone5));
        assert_eq!(DeviceFamily::for_tag(6), Some(DeviceFamily::iPhone5c));
        assert_eq!(DeviceFamily::for_tag(7), Some(DeviceFamily::iPad));
        assert_eq!(
            DeviceFamily::for_tag(19),
            Some(DeviceFamily::iPodTouch5)
        );
        assert_eq!(DeviceFamily::for_tag(20), Some(DeviceFamily::iPhone6));
        assert_eq!(DeviceFamily::iPhone6.tag(), Some(20));
        assert_eq!(DeviceFamily::for_tag(21), None);
        for family in DeviceFamily::ALL_SELECTABLE {
            assert_eq!(DeviceFamily::for_tag(family.tag().unwrap()), Some(*family));
        }
    }
}
