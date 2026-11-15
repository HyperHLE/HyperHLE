/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `CoreHaptics.framework` — the iOS 13 haptic engine.
//!
//! The framework does not exist on the iPhone OS versions this emulator
//! models, but apps that were built against a newer SDK (RetroArch's armv7
//! slice is one) still reference its exported string constants from code
//! paths that only run on iOS 13+. Those references are non-lazy data
//! relocations, so without host definitions the loader leaves the slots
//! pointing at garbage and prints `unhandled external relocation` /
//! `unhandled non-lazy symbol` for each one at startup.
//!
//! Only the constants are provided. The classes (`CHHapticEngine` and
//! friends) are deliberately left out: on an emulated iOS 9 device the app's
//! `NSClassFromString(@"CHHapticEngine")` probe answers nil, which is exactly
//! what the app sees on real hardware from that era, and it then skips the
//! haptic code path entirely.
//!
//! Values are the documented ones from Apple's `CHHapticEngine.h`:
//! each string constant's value is its own name, e.g.
//! <https://developer.apple.com/documentation/corehaptics/chhapticevent/eventtype/haptictransient>.

use crate::dyld::{ConstantExports, HostConstant, HostDylib};

pub const CONSTANTS: ConstantExports = &[
    (
        "_CHHapticEventTypeHapticTransient",
        HostConstant::NSString("HapticTransient"),
    ),
    (
        "_CHHapticEventTypeHapticContinuous",
        HostConstant::NSString("HapticContinuous"),
    ),
    (
        "_CHHapticEventParameterIDHapticIntensity",
        HostConstant::NSString("HapticIntensity"),
    ),
    (
        "_CHHapticEventParameterIDHapticSharpness",
        HostConstant::NSString("HapticSharpness"),
    ),
    (
        "_CHHapticDynamicParameterIDHapticIntensityControl",
        HostConstant::NSString("HapticIntensityControl"),
    ),
];

pub const DYLIB: HostDylib = HostDylib {
    path: "/System/Library/Frameworks/CoreHaptics.framework/CoreHaptics",
    aliases: &[],
    class_exports: &[],
    constant_exports: &[CONSTANTS],
    function_exports: &[],
};
