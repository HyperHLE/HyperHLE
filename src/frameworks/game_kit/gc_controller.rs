/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `GameController.framework` — `GCController` and related classes.
//!
//! touchHLE has no physical controller support, so all controllers report
//! as disconnected and all button/axis values are zero.  Apps that
//! optionally use GCController (checking `[GCController controllers]` before
//! doing anything) will fall back to their touch controls gracefully.

use crate::dyld::{ConstantExports, HostConstant};
use crate::frameworks::foundation::ns_string;
use crate::frameworks::foundation::NSInteger;
use crate::objc::{
    autorelease, id, msg, msg_class, nil, objc_classes, release, retain,
    ClassExports, HostObject, NSZonePtr,
};

// MARK: - Notification name constants

pub const GCControllerDidConnectNotification:    &str = "GCControllerDidConnectNotification";
pub const GCControllerDidDisconnectNotification: &str = "GCControllerDidDisconnectNotification";

pub const CONSTANTS: ConstantExports = &[
    (
        "_GCControllerDidConnectNotification",
        HostConstant::NSString(GCControllerDidConnectNotification),
    ),
    (
        "_GCControllerDidDisconnectNotification",
        HostConstant::NSString(GCControllerDidDisconnectNotification),
    ),
];

// MARK: - Player index

type GCControllerPlayerIndex = NSInteger;
const GCControllerPlayerIndexUnset: GCControllerPlayerIndex = -1;
const GCControllerPlayerIndex1:     GCControllerPlayerIndex = 0;
const GCControllerPlayerIndex2:     GCControllerPlayerIndex = 1;
const GCControllerPlayerIndex3:     GCControllerPlayerIndex = 2;
const GCControllerPlayerIndex4:     GCControllerPlayerIndex = 3;

// MARK: - Host objects

/// GCController — no physical controller connected.
struct GCControllerHostObject {
    player_index:    GCControllerPlayerIndex,
    /// `GCGamepad*` — retained
    gamepad:         id,
    /// `GCExtendedGamepad*` — retained
    extended_gamepad: id,
    /// `GCMicroGamepad*` — retained
    micro_gamepad:   id,
    /// `NSString*` — retained
    vendor_name:     id,
    attached_to_device: bool,
    handler_queue:   id, // dispatch_queue_t, weak
}
impl HostObject for GCControllerHostObject {}

/// GCGamepad (standard gamepad profile)
struct GCGamepadHostObject {
    /// weak back-reference to owning GCController
    controller: id,
}
impl HostObject for GCGamepadHostObject {}

/// GCExtendedGamepad (extended gamepad profile)
struct GCExtendedGamepadHostObject {
    controller: id,
}
impl HostObject for GCExtendedGamepadHostObject {}

/// GCMicroGamepad (Siri Remote / micro profile)
struct GCMicroGamepadHostObject {
    controller: id,
}
impl HostObject for GCMicroGamepadHostObject {}

/// GCButtonInput — a single digital or analog button
struct GCButtonInputHostObject {
    value:    f32,
    pressed:  bool,
}
impl HostObject for GCButtonInputHostObject {}

/// GCAxisInput — a single axis
struct GCAxisInputHostObject {
    value: f32,
}
impl HostObject for GCAxisInputHostObject {}

/// GCDirectionPad — a d-pad or thumbstick with x/y axes and 4 buttons
struct GCDirectionPadHostObject {
    x_axis: id, // GCAxisInput*
    y_axis: id,
    up:     id, // GCButtonInput*
    down:   id,
    left:   id,
    right:  id,
}
impl HostObject for GCDirectionPadHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

// =========================================================================
// GCController
// =========================================================================

@implementation GCController: NSObject

// MARK: Class methods

+ (id)controllers {
    // No controllers connected — return empty array.
    let arr: id = msg_class![env; NSArray new];
    autorelease(env, arr)
}

+ (bool)isWirelessControllerConnectionSupported {
    false
}

+ (())startWirelessControllerDiscoveryWithCompletionHandler:(id)_handler {
    log_dbg!("GCController startWirelessControllerDiscoveryWithCompletionHandler: stubbed");
    // Immediately call completion handler if provided (no controllers found).
    // handler is a `void (^)(void)` block — we can't easily invoke it here,
    // so we leave it. Apps check [GCController controllers] anyway.
}

+ (())stopWirelessControllerDiscovery {
    log_dbg!("GCController stopWirelessControllerDiscovery: stubbed");
}

// MARK: Alloc

+ (id)allocWithZone:(NSZonePtr)_zone {
    let vendor: id = ns_string::from_rust_string(env, "touchHLE Virtual Controller".to_string());
    let host_object = Box::new(GCControllerHostObject {
        player_index:    GCControllerPlayerIndexUnset,
        gamepad:         nil,
        extended_gamepad: nil,
        micro_gamepad:   nil,
        vendor_name:     vendor,
        attached_to_device: false,
        handler_queue:   nil,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (())dealloc {
    let host = env.objc.borrow::<GCControllerHostObject>(this);
    let (gp, egp, mgp, vn) = (
        host.gamepad, host.extended_gamepad,
        host.micro_gamepad, host.vendor_name,
    );
    release(env, gp);
    release(env, egp);
    release(env, mgp);
    release(env, vn);
    env.objc.dealloc_object(this, &mut env.mem)
}

// MARK: Properties

- (id)vendorName {
    env.objc.borrow::<GCControllerHostObject>(this).vendor_name
}

- (bool)isAttachedToDevice {
    env.objc.borrow::<GCControllerHostObject>(this).attached_to_device
}

- (GCControllerPlayerIndex)playerIndex {
    env.objc.borrow::<GCControllerHostObject>(this).player_index
}

- (())setPlayerIndex:(GCControllerPlayerIndex)idx {
    env.objc.borrow_mut::<GCControllerHostObject>(this).player_index = idx;
}

// MARK: Profiles

- (id)gamepad {
    let gp = env.objc.borrow::<GCControllerHostObject>(this).gamepad;
    if gp != nil { return gp; }
    // Lazy-create a GCGamepad.
    let new_gp: id = msg_class![env; GCGamepad alloc];
    env.objc.borrow_mut::<GCGamepadHostObject>(new_gp).controller = this;
    retain(env, new_gp);
    env.objc.borrow_mut::<GCControllerHostObject>(this).gamepad = new_gp;
    new_gp
}

- (id)extendedGamepad {
    let egp = env.objc.borrow::<GCControllerHostObject>(this).extended_gamepad;
    if egp != nil { return egp; }
    let new_egp: id = msg_class![env; GCExtendedGamepad alloc];
    env.objc.borrow_mut::<GCExtendedGamepadHostObject>(new_egp).controller = this;
    retain(env, new_egp);
    env.objc.borrow_mut::<GCControllerHostObject>(this).extended_gamepad = new_egp;
    new_egp
}

- (id)microGamepad {
    let mgp = env.objc.borrow::<GCControllerHostObject>(this).micro_gamepad;
    if mgp != nil { return mgp; }
    let new_mgp: id = msg_class![env; GCMicroGamepad alloc];
    env.objc.borrow_mut::<GCMicroGamepadHostObject>(new_mgp).controller = this;
    retain(env, new_mgp);
    env.objc.borrow_mut::<GCControllerHostObject>(this).micro_gamepad = new_mgp;
    new_mgp
}

// MARK: Handlers / queue

- (())setControllerPausedHandler:(id)_handler {
    log_dbg!("GCController setControllerPausedHandler: stubbed");
}

- (id)controllerPausedHandler {
    nil
}

- (())setHandlerQueue:(id)queue {
    // Weak — no retain.
    env.objc.borrow_mut::<GCControllerHostObject>(this).handler_queue = queue;
}

- (id)handlerQueue {
    env.objc.borrow::<GCControllerHostObject>(this).handler_queue
}

- (())capture {
    // Returns a snapshot — we return self since all values are already frozen.
    log_dbg!("GCController capture: stubbed (returning self)");
}

@end

// =========================================================================
// GCGamepad
// =========================================================================

@implementation GCGamepad: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(GCGamepadHostObject { controller: nil });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (())dealloc {
    env.objc.dealloc_object(this, &mut env.mem)
}

- (id)controller {
    env.objc.borrow::<GCGamepadHostObject>(this).controller
}

// All buttons / d-pad return zero-value inputs.
- (id)buttonA      { make_button(env, 0.0, false) }
- (id)buttonB      { make_button(env, 0.0, false) }
- (id)buttonX      { make_button(env, 0.0, false) }
- (id)buttonY      { make_button(env, 0.0, false) }
- (id)leftShoulder { make_button(env, 0.0, false) }
- (id)rightShoulder { make_button(env, 0.0, false) }
- (id)dpad         { make_dpad(env) }

- (())setValueChangedHandler:(id)_handler {}
- (id)valueChangedHandler { nil }

- (id)capture {
    retain(env, this)
}

- (id)saveSnapshot {
    retain(env, this)
}

@end

// =========================================================================
// GCExtendedGamepad
// =========================================================================

@implementation GCExtendedGamepad: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(GCExtendedGamepadHostObject { controller: nil });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (())dealloc {
    env.objc.dealloc_object(this, &mut env.mem)
}

- (id)controller {
    env.objc.borrow::<GCExtendedGamepadHostObject>(this).controller
}

- (id)buttonA       { make_button(env, 0.0, false) }
- (id)buttonB       { make_button(env, 0.0, false) }
- (id)buttonX       { make_button(env, 0.0, false) }
- (id)buttonY       { make_button(env, 0.0, false) }
- (id)buttonMenu    { make_button(env, 0.0, false) }
- (id)buttonOptions { make_button(env, 0.0, false) }
- (id)buttonHome    { make_button(env, 0.0, false) }
- (id)leftShoulder  { make_button(env, 0.0, false) }
- (id)rightShoulder { make_button(env, 0.0, false) }
- (id)leftTrigger   { make_button(env, 0.0, false) }
- (id)rightTrigger  { make_button(env, 0.0, false) }
- (id)leftThumbstickButton  { make_button(env, 0.0, false) }
- (id)rightThumbstickButton { make_button(env, 0.0, false) }
- (id)dpad              { make_dpad(env) }
- (id)leftThumbstick    { make_dpad(env) }
- (id)rightThumbstick   { make_dpad(env) }

- (())setValueChangedHandler:(id)_handler {}
- (id)valueChangedHandler { nil }

- (id)capture {
    retain(env, this)
}

- (id)saveSnapshot {
    retain(env, this)
}

@end

// =========================================================================
// GCMicroGamepad
// =========================================================================

@implementation GCMicroGamepad: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(GCMicroGamepadHostObject { controller: nil });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (())dealloc {
    env.objc.dealloc_object(this, &mut env.mem)
}

- (id)controller {
    env.objc.borrow::<GCMicroGamepadHostObject>(this).controller
}

- (id)buttonA      { make_button(env, 0.0, false) }
- (id)buttonX      { make_button(env, 0.0, false) }
- (id)buttonMenu   { make_button(env, 0.0, false) }
- (id)dpad         { make_dpad(env) }

- (bool)reportsAbsoluteDpadValues { false }
- (())setReportsAbsoluteDpadValues:(bool)_v {}

- (bool)allowsRotation { false }
- (())setAllowsRotation:(bool)_v {}

- (())setValueChangedHandler:(id)_handler {}
- (id)valueChangedHandler { nil }

- (id)capture {
    retain(env, this)
}

@end

// =========================================================================
// GCButtonInput
// =========================================================================

@implementation GCButtonInput: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(GCButtonInputHostObject { value: 0.0, pressed: false });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (())dealloc {
    env.objc.dealloc_object(this, &mut env.mem)
}

- (f32)value {
    env.objc.borrow::<GCButtonInputHostObject>(this).value
}

- (bool)isPressed {
    env.objc.borrow::<GCButtonInputHostObject>(this).pressed
}

- (bool)isTouched {
    env.objc.borrow::<GCButtonInputHostObject>(this).pressed
}

- (())setValueChangedHandler:(id)_handler {}
- (id)valueChangedHandler { nil }

- (())setPressedChangedHandler:(id)_handler {}
- (id)pressedChangedHandler { nil }

- (id)collection { nil }
- (bool)isAnalog { false }
- (f32)analogThreshold { 0.5 }
- (())setAnalogThreshold:(f32)_v {}

@end

// =========================================================================
// GCAxisInput
// =========================================================================

@implementation GCAxisInput: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(GCAxisInputHostObject { value: 0.0 });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (())dealloc {
    env.objc.dealloc_object(this, &mut env.mem)
}

- (f32)value {
    env.objc.borrow::<GCAxisInputHostObject>(this).value
}

- (())setValueChangedHandler:(id)_handler {}
- (id)valueChangedHandler { nil }

@end

// =========================================================================
// GCDirectionPad
// =========================================================================

@implementation GCDirectionPad: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(GCDirectionPadHostObject {
        x_axis: nil, y_axis: nil,
        up: nil, down: nil, left: nil, right: nil,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (())dealloc {
    let host = env.objc.borrow::<GCDirectionPadHostObject>(this);
    let (xa, ya, u, d, l, r) = (host.x_axis, host.y_axis,
                                  host.up, host.down, host.left, host.right);
    release(env, xa); release(env, ya);
    release(env, u);  release(env, d);
    release(env, l);  release(env, r);
    env.objc.dealloc_object(this, &mut env.mem)
}

- (id)xAxis {
    let xa = env.objc.borrow::<GCDirectionPadHostObject>(this).x_axis;
    if xa != nil { return xa; }
    let new_xa = make_axis(env, 0.0);
    retain(env, new_xa);
    env.objc.borrow_mut::<GCDirectionPadHostObject>(this).x_axis = new_xa;
    new_xa
}

- (id)yAxis {
    let ya = env.objc.borrow::<GCDirectionPadHostObject>(this).y_axis;
    if ya != nil { return ya; }
    let new_ya = make_axis(env, 0.0);
    retain(env, new_ya);
    env.objc.borrow_mut::<GCDirectionPadHostObject>(this).y_axis = new_ya;
    new_ya
}

- (id)up    { lazy_button(env, this, 0) }
- (id)down  { lazy_button(env, this, 1) }
- (id)left  { lazy_button(env, this, 2) }
- (id)right { lazy_button(env, this, 3) }

- (())setValueChangedHandler:(id)_handler {}
- (id)valueChangedHandler { nil }

- (id)collection { nil }

@end

// =========================================================================
// GCGamepadSnapshot / GCExtendedGamepadSnapshot (iOS 7–12 compatibility)
// =========================================================================

@implementation GCGamepadSnapshot: GCGamepad
@end

@implementation GCExtendedGamepadSnapshot: GCExtendedGamepad
@end

};

// MARK: - Helpers (outside macro)

/// Create an autoreleased `GCButtonInput` with given value/pressed state.
fn make_button(env: &mut crate::Environment, value: f32, pressed: bool) -> crate::objc::id {
    let class = env.objc.get_known_class("GCButtonInput", &mut env.mem);
    let obj = env.objc.alloc_object(
        class,
        Box::new(GCButtonInputHostObject { value, pressed }),
        &mut env.mem,
    );
    crate::objc::autorelease(env, obj)
}

/// Create an autoreleased `GCAxisInput` with given value.
fn make_axis(env: &mut crate::Environment, value: f32) -> crate::objc::id {
    let class = env.objc.get_known_class("GCAxisInput", &mut env.mem);
    env.objc.alloc_object(
        class,
        Box::new(GCAxisInputHostObject { value }),
        &mut env.mem,
    )
}

/// Create an autoreleased `GCDirectionPad` with all axes/buttons at zero.
fn make_dpad(env: &mut crate::Environment) -> crate::objc::id {
    let class = env.objc.get_known_class("GCDirectionPad", &mut env.mem);
    let obj = env.objc.alloc_object(
        class,
        Box::new(GCDirectionPadHostObject {
            x_axis: crate::objc::nil, y_axis: crate::objc::nil,
            up:     crate::objc::nil, down:   crate::objc::nil,
            left:   crate::objc::nil, right:  crate::objc::nil,
        }),
        &mut env.mem,
    );
    crate::objc::autorelease(env, obj)
}

/// Lazy-initialise one of the four direction buttons on a `GCDirectionPad`.
/// `slot`: 0=up 1=down 2=left 3=right
fn lazy_button(env: &mut crate::Environment, dpad: crate::objc::id, slot: u8) -> crate::objc::id {
    let existing = {
        let host = env.objc.borrow::<GCDirectionPadHostObject>(dpad);
        match slot {
            0 => host.up,
            1 => host.down,
            2 => host.left,
            _ => host.right,
        }
    };
    if existing != crate::objc::nil { return existing; }

    let class = env.objc.get_known_class("GCButtonInput", &mut env.mem);
    let btn = env.objc.alloc_object(
        class,
        Box::new(GCButtonInputHostObject { value: 0.0, pressed: false }),
        &mut env.mem,
    );
    crate::objc::retain(env, btn);
    {
        let host = env.objc.borrow_mut::<GCDirectionPadHostObject>(dpad);
        match slot {
            0 => host.up    = btn,
            1 => host.down  = btn,
            2 => host.left  = btn,
            _ => host.right = btn,
        }
    }
    btn
}
