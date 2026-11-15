/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `GameController.framework` — MFi external game controller API.
//!
//! Real iOS exposes the `GCController` class plus a family of profile
//! classes (`GCExtendedGamepad`, `GCMicroGamepad`, …). touchHLE has no
//! HID/Bluetooth backend, so there are never any external controllers
//! attached. The Apple-documented behaviour for that case is well
//! defined:
//!
//!   * `+[GCController controllers]` returns an empty array — see
//!     <https://developer.apple.com/documentation/gamecontroller/gccontroller/1454801-controllers>.
//!   * `+startWirelessControllerDiscoveryWithCompletionHandler:` finishes
//!     immediately and invokes the completion handler with no result.
//!     See <https://developer.apple.com/documentation/gamecontroller/gccontroller/1454822-startwirelesscontrollerdiscovery>.
//!   * `+stopWirelessControllerDiscovery` is a no-op when no discovery is
//!     in progress.
//!
//! Modelling the class itself (rather than leaving it unregistered) is
//! what lets games walk past their "is Game Controller support
//! available?" probe — typically `if (NSClassFromString(@"GCController"))
//! { [GCController respondsToSelector:@selector(controllers)] }`. Before
//! this change touchHLE would print `Class "GCController" is
//! unimplemented` for every such probe and apps would either crash on
//! the next message send or take the wrong code path.

use crate::abi::{CallFromHost, GuestFunction};
use crate::dyld::{ConstantExports, HostConstant, HostDylib};
use crate::frameworks::foundation::ns_array;
use crate::mem::ConstVoidPtr;
use crate::objc::{id, nil, objc_classes, ClassExports};
use crate::Environment;

/// `extern const double GCHapticDurationInfinite`
///
/// CoreHaptics/GameController declare this as `INFINITY`: passing it as a
/// duration means "keep playing until stopped". Apps built with a newer SDK
/// reference it from an iOS 13+ code path, but the relocation is resolved at
/// load time regardless, so the slot must hold a real value.
fn haptic_duration_infinite(env: &mut Environment) -> ConstVoidPtr {
    env.mem.alloc_and_write(f64::INFINITY).cast().cast_const()
}

pub const CONSTANTS: ConstantExports = &[
    (
        "_GCControllerDidConnectNotification",
        HostConstant::NSString("GCControllerDidConnectNotification"),
    ),
    (
        "_GCControllerDidDisconnectNotification",
        HostConstant::NSString("GCControllerDidDisconnectNotification"),
    ),
    // Haptic locality names (`GCHapticsLocality` in GameController.h, iOS 13+).
    // Like the CoreHaptics constants in `core_haptics.rs`, the values are the
    // documented names themselves; they only have to exist for the loader.
    (
        "_GCHapticsLocalityDefault",
        HostConstant::NSString("Default"),
    ),
    ("_GCHapticsLocalityAll", HostConstant::NSString("All")),
    ("_GCHapticsLocalityHandles", HostConstant::NSString("Handles")),
    (
        "_GCHapticsLocalityTriggers",
        HostConstant::NSString("Triggers"),
    ),
    (
        "_GCHapticDurationInfinite",
        HostConstant::Custom(haptic_duration_infinite),
    ),
];

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation GCController: NSObject

// `+ (NSArray<GCController *> *)controllers;`
//
// Per Apple's reference, the returned array is the currently-connected
// MFi controller list. touchHLE has no HID backend, so the array is
// always empty. Returning an autoreleased empty NSArray (rather than
// nil) matches the documented signature; some games iterate the result
// without a nil-check.
+ (id)controllers {
    let empty = ns_array::from_vec(env, Vec::new());
    crate::objc::autorelease(env, empty)
}

// `+ (void)startWirelessControllerDiscoveryWithCompletionHandler:
//                              (void (^)(void))completionHandler;`
//
// Per Apple: "If the user cancels discovery before a controller is
// found, or if no controllers are found, the completion handler is
// still called." With no Bluetooth backend we finish synchronously and
// invoke the handler immediately. Block layout on 32-bit ARM is the
// same as elsewhere in touchHLE: invoke pointer at word 3.
+ (())startWirelessControllerDiscoveryWithCompletionHandler:(id)handler {
    if handler == nil {
        return;
    }
    let invoke_ptr: u32 = env.mem.read(handler.cast::<u32>() + 3u32);
    if invoke_ptr == 0 {
        return;
    }
    let invoke = GuestFunction::from_addr_with_thumb_bit(invoke_ptr);
    let _: () = invoke.call_from_host(env, (handler,));
}

// `+ (void)stopWirelessControllerDiscovery;`
//
// No-op when no discovery is in progress, which is always the case in
// touchHLE.
+ (())stopWirelessControllerDiscovery {
    // Intentionally empty — matches the documented behaviour when no
    // discovery is active.
}

@end

};

pub const DYLIB: HostDylib = HostDylib {
    path: "/System/Library/Frameworks/GameController.framework/GameController",
    aliases: &[],
    class_exports: &[CLASSES],
    constant_exports: &[CONSTANTS],
    function_exports: &[],
};
