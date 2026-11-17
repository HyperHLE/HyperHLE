/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 * If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UITouch`.

use super::ui_event;
use crate::frameworks::core_graphics::{CGFloat, CGPoint, CGRect};
use crate::frameworks::foundation::{NSInteger, NSTimeInterval, NSUInteger};
use crate::mem::GuestUSize;
use crate::objc::{
    autorelease, id, msg, msg_class, msg_send_no_type_checking, nil, objc_classes, release, retain,
    ClassExports, HostObject, NSZonePtr,
};
use crate::window::{Coords, Event, FingerId};
use crate::Environment;
use std::collections::hash_map::{Entry, HashMap};
use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Number of touch-down diagnostics to print at `log!` level (always visible)
/// before going quiet. Helps diagnose "game ignores taps" reports without
/// requiring the user to enable debug logging.

/// Number of touch-end summaries to print at `log!` level. Shows how many
/// Moved events reached the view and the total displacement, which
/// distinguishes "host lost the moves" from "game ignored a complete gesture".

/// Number of warnings for Move/Up events naming an untracked finger (its Down
/// never arrived). Proof of lost Downs in "swipes randomly dead" reports.
static UNTRACKED_TOUCH_WARNS: AtomicUsize = AtomicUsize::new(8);
static TOUCH_BEGAN_DIAGNOSTICS: AtomicUsize = AtomicUsize::new(0);
static TOUCH_ENDED_DIAGNOSTICS: AtomicUsize = AtomicUsize::new(0);

fn log_touch_delivery(marker: &str, counter: &AtomicUsize, count: usize) {
    if !crate::env_flag_cached!("TOUCHHLE_TRACE_TOUCHES") {
        return;
    }
    let sequence = counter.fetch_add(1, Ordering::Relaxed) + 1;
    if sequence <= 12 {
        log!(
            "{} #{} delivered to UIKit ({} touch(es))",
            marker,
            sequence,
            count
        );
    }
}

/// A touch with no event for this long is no longer part of a live gesture;
/// its Up/Cancel was lost. Used to reclaim stale touches safely.
const STALE_TOUCH_SECONDS: f64 = 10.0;

pub type UITouchPhase = NSInteger;
pub const UITouchPhaseBegan: UITouchPhase = 0;
pub const UITouchPhaseMoved: UITouchPhase = 1;
pub const UITouchPhaseStationary: UITouchPhase = 2;
pub const UITouchPhaseEnded: UITouchPhase = 3;

pub type UITouchType = NSInteger;
pub const UITouchTypeDirect: UITouchType = 0;

#[derive(Default)]
pub struct State {
    pub current_touches: HashMap<FingerId, id>,
    cancelled_by_gesture: HashSet<id>,
}

/// Cancel view delivery once a recognizer begins, while continuing to feed
/// the recognizer itself with movement/end events from these touches.
pub(super) fn cancel_for_gesture(env: &mut Environment, touches: &[id]) {
    let mut groups: HashMap<id, Vec<id>> = HashMap::new();
    for &touch in touches {
        if !env.framework_state.uikit.ui_touch.cancelled_by_gesture.insert(touch) { continue; }
        let view: id = msg![env; touch view];
        if view != nil { groups.entry(view).or_default().push(touch); }
    }
    for (view, touches) in groups {
        let set: id = msg_class![env; NSMutableSet new];
        let mut phases = Vec::new();
        for touch in touches {
            () = msg![env; set addObject:touch];
            let old_phase = touch_ivars(env, touch, |v| { let old = v.phase; v.phase = 4; old });
            phases.push((touch, old_phase));
        }
        let event = ui_event::new_event(env, set);
        () = msg![env; view touchesCancelled:set withEvent:event];
        for (touch, phase) in phases { touch_ivars(env, touch, |v| v.phase = phase); }
        release(env, event);
        release(env, set);
    }
}

fn touches_for_view_delivery(env: &mut Environment, touches: id) -> id {
    let filtered: id = msg_class![env; NSMutableSet new];
    let array: id = msg![env; touches allObjects];
    let count: NSUInteger = msg![env; array count];
    for i in 0..count {
        let touch: id = msg![env; array objectAtIndex:i];
        if !env.framework_state.uikit.ui_touch.cancelled_by_gesture.contains(&touch) {
            () = msg![env; filtered addObject:touch];
        }
    }
    filtered
}

/// Guest-memory ivars for `UITouch`.
///
/// Some engines (Gameloft's Cross engine in Scarface, etc.) do a raw
/// `memcpy` of the whole 0x40-byte `UITouch` object into their own
/// structures and later send messages to that *copy*. On real iOS this
/// works because instance methods read their ivars from the object's own
/// guest memory. To emulate that, ALL public state lives in guest memory
/// at self-consistent offsets within the first 0x40 bytes; methods must
/// never read it from the host object (a copy is not registered in the
/// object map, so a host-side lookup would fail and return garbage).
/// The layout is 0x40 bytes to match the stride guest engines use when they
/// bit-copy a UITouch.
#[repr(C)]
struct UITouchIvars {
    /// Written by the runtime in `alloc_object_inner`; never touched by us.
    _isa: u32,
    window: id,                 // 0x04
    view: id,                   // 0x08
    phase: UITouchPhase,        // 0x0C
    timestamp: NSTimeInterval,  // 0x10
    location: CGPoint,          // 0x18
    previous_location: CGPoint, // 0x28
    tap_count: u32,             // 0x38
    touch_type: UITouchType,    // 0x3C
}

/// Run `f` with the guest-memory ivars of the (possibly unregistered —
/// i.e. a guest-made bit-copy) UITouch at `this`. The borrow is not held
/// across message sends: copy fields out first, then act on them.
fn touch_ivars<R>(env: &mut Environment, this: id, f: impl FnOnce(&mut UITouchIvars) -> R) -> R {
    let size = std::mem::size_of::<UITouchIvars>();
    let bytes = env.mem.bytes_at_mut(this.cast(), size as GuestUSize);
    let ptr = bytes.as_mut_ptr() as *mut UITouchIvars;
    // `this` is a guest address and may not be 8-byte aligned, but
    // `UITouchIvars` contains f64 fields (timestamp/CGPoint) that need
    // 8-byte alignment. Forming a `&mut UITouchIvars` from a misaligned host
    // pointer is undefined behaviour and aborts in debug builds ("misaligned
    // pointer dereference") — which crashed Where's My Water on the first
    // touch. Round-trip the struct through unaligned reads/writes instead so
    // alignment of the backing guest allocation doesn't matter.
    let mut ivars = unsafe { ptr.read_unaligned() };
    let r = f(&mut ivars);
    unsafe { ptr.write_unaligned(ivars) };
    r
}

#[derive(Default)]
pub(super) struct UITouchHostObject {
    /// Where this touch first landed (in window/screen coordinates). Used to
    /// compute the total displacement for swipe gesture recognition.
    /// Host-side only: guests never need it, and swipe recognition always
    /// operates on registered (non-copied) touch objects.
    start_location: CGPoint,
    /// How many Moved updates this touch has received. Diagnostics only.
    move_count: u32,
}
impl HostObject for UITouchHostObject {}

/// Cancel a touch whose Up/Cancel was lost, exactly like a real cancel, and
/// drop it from the tracking table. `reason` is shown in the log.
fn stale_cancel_and_release(env: &mut Environment, finger_id: FingerId, touch: id, reason: &str) {
    static HEALS: AtomicUsize = AtomicUsize::new(0);
    let n = HEALS.fetch_add(1, Ordering::Relaxed) + 1;
    log!(
        "Warning: touch {:?} {} [heal {}]; delivering touchesCancelled and dropping it.",
        finger_id,
        reason,
        n
    );
    let stale_view: id = touch_ivars(env, touch, |v| v.view);
    if stale_view != nil {
        // Phase 4 (`UITouchPhaseCancelled`) mirrors `cancel_for_gesture`.
        touch_ivars(env, touch, |v| v.phase = 4);
        let stale_set: id = msg_class![env; NSMutableSet new];
        () = msg![env; stale_set addObject:touch];
        let cancel_event = ui_event::new_event(env, stale_set);
        () = msg![env; stale_view touchesCancelled:stale_set withEvent:cancel_event];
        release(env, cancel_event);
        release(env, stale_set);
    }
    env.framework_state
        .uikit
        .ui_touch
        .cancelled_by_gesture
        .remove(&touch);
    env.framework_state
        .uikit
        .ui_touch
        .current_touches
        .remove(&finger_id);
    release(env, touch);
}

fn touchhle_cocos_view_class_name(env: &mut Environment, view: id) -> String {
    if view == nil {
        return String::new();
    }
    let view_class: crate::objc::Class = msg![env; view class];
    env.objc.get_class_name(view_class).to_owned()
}

fn touchhle_cocos_is_gl_or_game_view_name(class_name: &str) -> bool {
    matches!(
        class_name,
        "CCGLView"
            | "EAGLView"
            | "CCEAGLView"
            | "GLKView"
            | "Cocos2dxGLView"
            | "Cocos2dView"
            | "DirectorView"
            | "CCUIViewWrapper"
    ) || class_name.contains("EAGL")
        || class_name.contains("GLView")
        || class_name.contains("Cocos")
        || class_name.contains("CCGL")
        || class_name.contains("Unity")
        || class_name.contains("UnityView")
        || class_name.contains("UnityGLView")
        || class_name.contains("UnityRenderingView")
        || class_name.contains("RenderView")
        || class_name.contains("GameView")
        || class_name.contains("RootView")
}

fn touchhle_manual_landscape_touch_remap_enabled() -> bool {
    crate::env_flag_cached!("TOUCHHLE_TOUCH_LOCATION_PORTRAIT_TO_LANDSCAPE")
        || crate::env_flag_cached!("TOUCHHLE_COCOS_TOUCH_REMAP")
        || crate::env_flag_cached!("TOUCHHLE_UNITY_TOUCH_REMAP")
        || crate::env_flag_cached!("TOUCHHLE_ENGINE_TOUCH_REMAP")
}

fn touchhle_should_use_landscape_touch_remap_for_bundle(
    bundle_id: &str,
    manual_override: bool,
) -> bool {
    match bundle_id {
        "at.source.veggie1" | "at.source.potato3D" | "at.source.potpan" => true,
        "at.source.tomzom" => false,
        // UIKit already returns the landscape coordinates used by the game's 480x320 view.
        "com.saban.powerrangersbash" => manual_override,
        _ => manual_override,
    }
}

fn touchhle_should_use_landscape_touch_remap(env: &Environment) -> bool {
    touchhle_should_use_landscape_touch_remap_for_bundle(
        env.bundle.bundle_identifier(),
        touchhle_manual_landscape_touch_remap_enabled(),
    )
}

fn should_remap_touch_location_for_view(env: &mut Environment, view: id) -> bool {
    if view == nil || !touchhle_should_use_landscape_touch_remap(env) {
        return false;
    }
    let class_name = touchhle_cocos_view_class_name(env, view);
    touchhle_cocos_is_gl_or_game_view_name(&class_name)
}

#[cfg(test)]
mod landscape_touch_remap_tests {
    use super::touchhle_should_use_landscape_touch_remap_for_bundle;

    #[test]
    fn power_rangers_keeps_its_native_landscape_touch_coordinates() {
        assert!(!touchhle_should_use_landscape_touch_remap_for_bundle(
            "com.saban.powerrangersbash",
            false
        ));
        assert!(touchhle_should_use_landscape_touch_remap_for_bundle(
            "com.saban.powerrangersbash",
            true
        ));
        assert!(touchhle_should_use_landscape_touch_remap_for_bundle(
            "at.source.veggie1",
            false
        ));
        assert!(!touchhle_should_use_landscape_touch_remap_for_bundle(
            "at.source.tomzom",
            true
        ));
    }
}

fn touchhle_cocos_target_size() -> (f32, f32) {
    // PERF: computed once; this runs for every remapped touch point.
    static CACHED: std::sync::OnceLock<(f32, f32)> = std::sync::OnceLock::new();
    *CACHED.get_or_init(|| {
        std::env::var("TOUCHHLE_COCOS_TOUCH_SIZE")
            .or_else(|_| std::env::var("TOUCHHLE_UNITY_TOUCH_SIZE"))
            .or_else(|_| std::env::var("TOUCHHLE_ENGINE_TOUCH_SIZE"))
            .ok()
            .and_then(|v| {
                let mut parts = v.split(|c| c == 'x' || c == 'X' || c == ',');
                let w = parts.next()?.trim().parse::<f32>().ok()?;
                let h = parts.next()?.trim().parse::<f32>().ok()?;
                Some((w, h))
            })
            .unwrap_or((480.0, 320.0))
    })
}

fn touchhle_cocos_remap_point(env: &mut Environment, view: id, point: CGPoint) -> CGPoint {
    let old_x = point.x;
    let old_y = point.y;
    let (target_w, target_h) = touchhle_cocos_target_size();
    // PERF: cached read-once lookups; runs for every remapped touch point.
    let mode = crate::env_var_cached!("TOUCHHLE_TOUCH_MODE")
        .map(str::to_owned)
        .unwrap_or_else(|| {
            let bundle_id = env.bundle.bundle_identifier();
            let custom_mode = crate::env_var_cached!("TOUCHHLE_COCOS_TOUCH_MODE")
                .or(crate::env_var_cached!("TOUCHHLE_UNITY_TOUCH_MODE"))
                .or(crate::env_var_cached!("TOUCHHLE_ENGINE_TOUCH_MODE"));
            match bundle_id {
                "at.source.veggie1" | "at.source.potato3D" | "at.source.potpan" => {
                    "scale".to_string()
                }
                _ => custom_mode
                    .map(str::to_owned)
                    .unwrap_or_else(|| "scale".to_string()),
            }
        });

    let source_bounds: CGRect = if view != nil {
        msg![env; view bounds]
    } else {
        CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: crate::frameworks::core_graphics::CGSize {
                width: 320.0,
                height: 480.0,
            },
        }
    };
    let source_w = if source_bounds.size.width.abs() > 0.001 {
        source_bounds.size.width.abs()
    } else {
        320.0
    };
    let source_h = if source_bounds.size.height.abs() > 0.001 {
        source_bounds.size.height.abs()
    } else {
        480.0
    };

    let (mut new_x, mut new_y) = match mode.as_str() {
        "identity" | "none" => (old_x, old_y),
        "right" => (
            old_y * (target_w / source_h),
            target_h - old_x * (target_h / source_w),
        ),
        "right-flip-x" => (
            target_w - old_y * (target_w / source_h),
            target_h - old_x * (target_h / source_w),
        ),
        "left" => (
            target_w - old_y * (target_w / source_h),
            old_x * (target_h / source_w),
        ),
        "left-flip-x" => (old_y * (target_w / source_h), old_x * (target_h / source_w)),
        "flip-x" => (
            target_w - old_x * (target_w / source_w),
            old_y * (target_h / source_h),
        ),
        "flip-y" => (
            old_x * (target_w / source_w),
            target_h - old_y * (target_h / source_h),
        ),
        "scale-1024x768" => (old_x * (1024.0 / source_w), old_y * (768.0 / source_h)),
        "scale-480x320" | "scale" | _ => {
            (old_x * (target_w / source_w), old_y * (target_h / source_h))
        }
    };

    if let Some(offset) = crate::env_var_cached!("TOUCHHLE_TOUCH_LOCATION_X_OFFSET") {
        if let Ok(offset) = offset.parse::<f32>() {
            new_x += offset;
        }
    }
    if let Some(offset) = crate::env_var_cached!("TOUCHHLE_TOUCH_LOCATION_Y_OFFSET") {
        if let Ok(offset) = offset.parse::<f32>() {
            new_y += offset;
        }
    }

    if !crate::env_flag_cached!("TOUCHHLE_COCOS_NO_TOUCH_CLAMP") {
        new_x = new_x.clamp(0.0, (target_w - 1.0).max(0.0));
        new_y = new_y.clamp(0.0, (target_h - 1.0).max(0.0));
    }

    log_dbg!(
        "UITouch Cocos remap mode={} source=({:.1}x{:.1}) target=({:.1}x{:.1}): ({:.1}, {:.1}) -> ({:.1}, {:.1})",
        mode, source_w, source_h, target_w, target_h, old_x, old_y, new_x, new_y
    );
    CGPoint { x: new_x, y: new_y }
}

fn touchhle_cocos_should_allow_multitouch(env: &mut Environment, view: id) -> bool {
    if crate::env_flag_cached!("TOUCHHLE_COCOS_FORCE_SINGLE_TOUCH")
        || crate::env_flag_cached!("TOUCHHLE_UNITY_FORCE_SINGLE_TOUCH")
        || crate::env_flag_cached!("TOUCHHLE_ENGINE_FORCE_SINGLE_TOUCH")
    {
        return false;
    }
    if crate::env_flag_cached!("TOUCHHLE_COCOS_FORCE_MULTITOUCH")
        || crate::env_flag_cached!("TOUCHHLE_UNITY_FORCE_MULTITOUCH")
        || crate::env_flag_cached!("TOUCHHLE_ENGINE_FORCE_MULTITOUCH")
    {
        return true;
    }
    let class_name = touchhle_cocos_view_class_name(env, view);
    touchhle_cocos_is_gl_or_game_view_name(&class_name)
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UITouch: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(UITouchHostObject {
        start_location: CGPoint { x: 0.0, y: 0.0 },
        move_count: 0,
    });
    // The guest allocation must cover the ivars (see `UITouchIvars`): guest
    // engines bit-copy the whole 0x40-byte object and message the copy.
    let new_touch = env.objc.alloc_object_sized(
        this,
        std::mem::size_of::<UITouchIvars>() as GuestUSize,
        host_object,
        &mut env.mem,
    );
    touch_ivars(env, new_touch, |v| v.touch_type = UITouchTypeDirect);
    new_touch
}

- (())dealloc {
    // UITouch objects are owned by the system on real iOS: apps only ever
    // hold weak, unowned references to them and UIKit keeps them alive for
    // the app's lifetime. Some 3D games (Gameloft's Scarface, Asphalt etc.)
    // keep messaging a UITouch pointer many frames after the touch ended.
    // If we actually deallocate here, those late messages hit a dead object
    // ("SUPER HACK! Faking borrow for missing object" spam) and return
    // garbage, which makes the games ignore input. So -dealloc is a no-op:
    // the refcount machinery still works (releases below zero are no-ops
    // once the entry's refcount is gone), and each touch leaks a few dozen
    // bytes for the app's lifetime — negligible.
    log_dbg!("[(UITouch) dealloc suppressed: touches live for the app's lifetime, like on real iOS]");
}

- (CGPoint)locationInView:(id)that_view {
    let (location, window, view) = touch_ivars(env, this, |v| (v.location, v.window, v.view));
    let location_in_window: CGPoint = if window != nil {
        msg![env; window convertPoint:location fromWindow:nil]
    } else {
        location
    };
    let mut result: CGPoint = if that_view == nil {
        location_in_window
    } else {
        msg![env; that_view convertPoint:location_in_window fromView:window]
    };

    let remap_view = if that_view != nil { that_view } else { view };
    let should_remap = should_remap_touch_location_for_view(env, remap_view);

    if crate::env_flag_cached!("TOUCHHLE_TRACE_TOUCH_REMAP") {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        if COUNTER.fetch_add(1, Ordering::Relaxed) < 24 {
            let location_x = location.x;
            let location_y = location.y;
            let location_in_window_x = location_in_window.x;
            let location_in_window_y = location_in_window.y;
            let result_x = result.x;
            let result_y = result.y;
            let vclass = if remap_view != nil {
                let c: crate::objc::Class = msg![env; remap_view class];
                env.objc.get_class_name(c).to_owned()
            } else {
                "(nil)".to_string()
            };
            let vbounds: CGRect = if remap_view != nil {
                msg![env; remap_view bounds]
            } else {
                CGRect {
                    origin: CGPoint { x: 0.0, y: 0.0 },
                    size: crate::frameworks::core_graphics::CGSize { width: 0.0, height: 0.0 },
                }
            };
            // Copy packed CGRect fields into aligned locals before formatting
            // (taking a reference to a packed field is a hard error).
            let vb_w = vbounds.size.width;
            let vb_h = vbounds.size.height;
            log!(
                "TOUCHHLE_TRACE_TOUCH_REMAP: view={} bounds=({:.0}x{:.0}) location=({:.1},{:.1}) location_in_window=({:.1},{:.1}) result=({:.1},{:.1}) should_remap={}",
                vclass, vb_w, vb_h,
                location_x, location_y, location_in_window_x, location_in_window_y, result_x, result_y,
                should_remap
            );
        }
    }

    if should_remap {
        // Important: this happens AFTER UIKit hit-testing. The touch can still
        // hit a portrait-sized EAGL/CCGL view, but the game can receive the
        // Cocos/OpenGL coordinate system it expects.
        result = touchhle_cocos_remap_point(env, remap_view, result);
    }

    result
}

- (CGPoint)previousLocationInView:(id)that_view {
    let (previous_location, window, view) =
        touch_ivars(env, this, |v| (v.previous_location, v.window, v.view));
    let location_in_window: CGPoint = if window != nil {
        msg![env; window convertPoint:previous_location fromWindow:nil]
    } else {
        previous_location
    };
    let mut result: CGPoint = if that_view == nil {
        location_in_window
    } else {
        msg![env; that_view convertPoint:location_in_window fromView:window]
    };

    let remap_view = if that_view != nil { that_view } else { view };
    if should_remap_touch_location_for_view(env, remap_view) {
        result = touchhle_cocos_remap_point(env, remap_view, result);
    }

    result
}

- (id)view {
    touch_ivars(env, this, |v| v.view)
}

- (id)window {
    touch_ivars(env, this, |v| v.window)
}

- (NSTimeInterval)timestamp {
    touch_ivars(env, this, |v| v.timestamp)
}

- (NSUInteger)tapCount {
    touch_ivars(env, this, |v| v.tap_count as NSUInteger)
}

- (UITouchPhase)phase {
    touch_ivars(env, this, |v| v.phase)
}

- (UITouchType)type {
    touch_ivars(env, this, |v| v.touch_type)
}

// Event::Touches* carries point coordinates but no hardware contact-area sample.
- (CGFloat)majorRadius {
    0.0
}

- (CGFloat)majorRadiusTolerance {
    0.0
}

// UITouch conforms to NSCoding. A touch is only meaningful for the live event
// stream it belongs to, so archiving it stores no restorable state.
- (())encodeWithCoder:(id)_coder {}

@end

};

pub fn handle_event(env: &mut Environment, event: Event) {
    let touch_ids: Vec<id> = env
        .framework_state
        .uikit
        .ui_touch
        .current_touches
        .values()
        .cloned()
        .collect();
    for touch in touch_ids {
        touch_ivars(env, touch, |v| v.phase = UITouchPhaseStationary);
    }
    match event {
        Event::TouchesDown(map) => handle_touches_down(env, map),
        Event::TouchesMove(map) => handle_touches_move(env, map),
        Event::TouchesUp(map) => handle_touches_up(env, map),
        other => {
            // ui_touch::handle_event only ever wants touch events; non-touch
            // events are filtered out before getting here. Log instead of
            // panicking the host if that contract is ever violated.
            log!(
                "Warning: ui_touch::handle_event: unsupported event {:?}; ignored.",
                other
            );
        }
    }
}

fn touchhle_cocos_touch_aliases_enabled(env: &mut Environment, view: id) -> bool {
    if crate::env_flag_cached!("TOUCHHLE_DISABLE_COCOS_TOUCH_ALIASES") {
        return false;
    }
    if crate::env_flag_cached!("TOUCHHLE_COCOS_TOUCH_ALIASES") {
        return true;
    }
    let class_name = touchhle_cocos_view_class_name(env, view);
    touchhle_cocos_is_gl_or_game_view_name(&class_name)
        || class_name.starts_with("CC")
        || class_name.contains("Layer")
        || class_name.contains("Scene")
        || class_name.contains("Menu")
}

fn touchhle_send_cocos_touch_aliases(
    env: &mut Environment,
    view: id,
    phase: &str,
    touches: id,
    event: id,
) {
    if view == nil || !touchhle_cocos_touch_aliases_enabled(env, view) {
        return;
    }

    let all_sel_name = match phase {
        "began" => "ccTouchesBegan:withEvent:",
        "moved" => "ccTouchesMoved:withEvent:",
        "ended" => "ccTouchesEnded:withEvent:",
        "cancelled" => "ccTouchesCancelled:withEvent:",
        _ => return,
    };
    let one_sel_name = match phase {
        "began" => "ccTouchBegan:withEvent:",
        "moved" => "ccTouchMoved:withEvent:",
        "ended" => "ccTouchEnded:withEvent:",
        "cancelled" => "ccTouchCancelled:withEvent:",
        _ => return,
    };

    let all_sel = env
        .objc
        .lookup_selector(all_sel_name)
        .unwrap_or_else(|| {
            env.objc
                .register_host_selector(all_sel_name.to_string(), &mut env.mem)
        });
    let responds_all: bool = msg![env; view respondsToSelector:all_sel];
    if responds_all {
        let _: () = msg_send_no_type_checking(env, (view, all_sel, touches, event));
    }

    let one_sel = env
        .objc
        .lookup_selector(one_sel_name)
        .unwrap_or_else(|| {
            env.objc
                .register_host_selector(one_sel_name.to_string(), &mut env.mem)
        });
    let responds_one: bool = msg![env; view respondsToSelector:one_sel];
    if responds_one {
        let arr: id = msg![env; touches allObjects];
        let count: NSUInteger = msg![env; arr count];
        if count > 0 {
            let touch: id = msg![env; arr objectAtIndex:0];
            if phase == "began" {
                let _: u32 = msg_send_no_type_checking(env, (view, one_sel, touch, event));
            } else {
                let _: () = msg_send_no_type_checking(env, (view, one_sel, touch, event));
            }
        }
    }
}

fn touchhle_send_cocos_touch_aliases_to_chain(
    env: &mut Environment,
    view: id,
    phase: &str,
    touches: id,
    event: id,
) {
    // XaView A8 fix: touchHLE's responder chain has no view->viewController
    // links, so game views owned by view controllers (Asphalt 8's CCEAGLView
    // subclass) never receive Cocos touch aliases when a tap lands on a
    // sibling/overlay UIView. Walk the ENTIRE view hierarchy under the
    // touched view's top-level ancestor instead - the superview chain is a
    // subset of it.
    let mut current = view;
    let mut depth = 0;
    while current != nil && depth < 32 {
        let superview: id = msg![env; current superview];
        if superview == nil {
            break;
        }
        current = superview;
        depth += 1;
    }
    touchhle_send_cocos_touch_aliases_to_hierarchy(env, current, phase, touches, event);
}

/// Send Cocos touch aliases (`ccTouchesBegan:` etc.) to every view in the
/// view hierarchy rooted at `root` whose class responds to them. This makes
/// up for touchHLE not modelling UIViewController responder-chain links.
fn touchhle_send_cocos_touch_aliases_to_hierarchy(
    env: &mut Environment,
    root: id,
    phase: &str,
    touches: id,
    event: id,
) {
    if root == nil {
        return;
    }
    let mut stack: Vec<id> = vec![root];
    let mut visited: HashSet<id> = HashSet::new();
    while let Some(current) = stack.pop() {
        if current == nil || !visited.insert(current) {
            continue;
        }
        touchhle_send_cocos_touch_aliases(env, current, phase, touches, event);
        let subviews: id = msg![env; current subviews];
        if subviews != nil {
            let count: NSUInteger = msg![env; subviews count];
            for i in 0..count {
                let child: id = msg![env; subviews objectAtIndex:i];
                stack.push(child);
            }
        }
    }
}


/// Whether a window contains any real content. Empty helper windows left
/// behind by completed modal flows (age gate, ATT prompt, ...) have only a
/// bare, childless plain `UIView` root and must not intercept touches meant
/// for the window underneath, whereas a game window whose root view is itself
/// a `CAEAGLLayer`-backed view (`EAGLView` / `GLKView`), custom `UIView`
/// subclass, or rendered layer is real content even with zero subviews.
fn touchhle_window_has_content(env: &mut Environment, window: id) -> bool {
    let subviews: id = msg![env; window subviews];
    let count: NSUInteger = msg![env; subviews count];
    for i in 0..count {
        let root: id = msg![env; subviews objectAtIndex:i];
        let hidden: bool = msg![env; root isHidden];
        if hidden {
            continue;
        }
        let root_subviews: id = msg![env; root subviews];
        let root_count: NSUInteger = msg![env; root_subviews count];
        if root_count > 0 {
            return true;
        }
        let root_class: crate::objc::Class = msg![env; root class];
        if env.objc.get_class_name(root_class) != "UIView" {
            return true;
        }
        let layer: id = msg![env; root layer];
        if layer != nil {
            let layer_class: crate::objc::Class = msg![env; layer class];
            if env.objc.get_class_name(layer_class) == "CAEAGLLayer" {
                return true;
            }
            let contents: id = msg![env; layer contents];
            if contents != nil {
                return true;
            }
        }
    }
    false
}

/// Returns true for a view that cannot itself be a meaningful touch target:
/// a plain `UIView` (not a `UIControl` or custom subclass) with no subviews,
/// no gesture recognizers, no layer contents, and a clear background color.
fn touchhle_is_empty_overlay(env: &mut Environment, view: id) -> bool {
    let class: crate::objc::Class = msg![env; view class];
    if env.objc.get_class_name(class) != "UIView" {
        return false;
    }
    let subviews: id = msg![env; view subviews];
    let count: NSUInteger = msg![env; subviews count];
    if count != 0 {
        return false;
    }
    let recognizers: id = msg![env; view gestureRecognizers];
    if recognizers != nil {
        let rec_count: NSUInteger = msg![env; recognizers count];
        if rec_count > 0 {
            return false;
        }
    }
    let layer: id = msg![env; view layer];
    if layer != nil {
        let contents: id = msg![env; layer contents];
        if contents != nil {
            return false;
        }
    }
    let bg_color: id = msg![env; view backgroundColor];
    if bg_color != nil {
        if super::ui_color::get_pattern_image(&env.objc, bg_color) != nil {
            return false;
        }
        let cg: crate::frameworks::core_graphics::cg_color::CGColorRef = msg![env; bg_color CGColor];
        if !cg.is_null()
            && crate::frameworks::core_graphics::cg_color::to_rgba(&env.objc, cg).3 > 0.01
        {
            return false;
        }
    }
    true
}

/// Depth-first search for the frontmost interactive descendant of `root` that
/// contains `point` (expressed in `root`'s coordinate space), ignoring empty
/// transparent overlay views (see [touchhle_is_empty_overlay]). Used as a
/// fallback when the standard `hitTest:` lands on such an overlay so touches
/// still reach the real content underneath.
fn touchhle_hit_test_skipping_overlays(env: &mut Environment, root: id, point: CGPoint) -> id {
    let hidden: bool = msg![env; root isHidden];
    let uie: bool = msg![env; root isUserInteractionEnabled];
    let alpha: f32 = msg![env; root alpha];
    if hidden || !uie || alpha < 0.01 {
        return nil;
    }
    if !msg![env; root pointInside:point withEvent:(nil)] {
        return nil;
    }
    let subviews: id = msg![env; root subviews];
    let count: NSUInteger = msg![env; subviews count];
    for i in (0..count).rev() {
        let child: id = msg![env; subviews objectAtIndex:i];
        let child_point: CGPoint = msg![env; root convertPoint:point toView:child];
        let hit = touchhle_hit_test_skipping_overlays(env, child, child_point);
        if hit != nil {
            return hit;
        }
    }
    // No descendant matched. Only claim the touch if this view is itself a
    // meaningful target; empty transparent overlays fall through.
    if touchhle_is_empty_overlay(env, root) {
        return nil;
    }
    root
}

fn touchhle_find_cocos_touch_target(env: &mut Environment, root: id) -> id {
    if root == nil {
        return nil;
    }
    let subviews: id = msg![env; root subviews];
    if subviews != nil {
        let count: NSUInteger = msg![env; subviews count];
        for i in (0..count).rev() {
            let child: id = msg![env; subviews objectAtIndex:i];
            let found = touchhle_find_cocos_touch_target(env, child);
            if found != nil {
                return found;
            }
        }
    }
    let class_name = touchhle_cocos_view_class_name(env, root);
    if touchhle_cocos_is_gl_or_game_view_name(&class_name) {
        root
    } else {
        nil
    }
}

fn handle_touches_down(env: &mut Environment, map: HashMap<FingerId, Coords>) {
    let pool: id = msg_class![env;
        NSAutoreleasePool new];

    let timestamp: NSTimeInterval = {
        let process_info = msg_class![env; NSProcessInfo processInfo];
        msg![env; process_info systemUptime]
    };

    let touches: id = msg_class![env; NSMutableSet new];
    autorelease(env, touches);

    for (finger_id, coords) in map {
        // A Down for a finger that is still tracked means an earlier Up or
        // Cancel was lost somewhere before it reached us (overlay ate the Up,
        // app went to the background mid-touch, host quirk, ...). The old
        // behaviour converted this Down into a Move, so the game never saw
        // touchesBegan: for this finger and silently ignored the whole
        // swipe/tap — until the host recycled the finger id. That is the
        // classic "swipes randomly dead" report (e.g. Subway Surfers). Heal
        // like real iOS: cancel the stale touch, then deliver a fresh Began.
        if let Some(stale_touch) = env
            .framework_state
            .uikit
            .ui_touch
            .current_touches
            .remove(&finger_id)
        {
            stale_cancel_and_release(env, finger_id, stale_touch, "restarted without ending");
        }

        let location = CGPoint {
            x: coords.0,
            y: coords.1,
        };
        let new_touch: id = msg_class![env; UITouch alloc];
        touch_ivars(env, new_touch, |v| {
            v.window = nil;
            v.view = nil;
            v.location = location;
            v.previous_location = location;
            v.timestamp = timestamp;
            v.phase = UITouchPhaseBegan;
            v.tap_count = 1;
        });
        env.objc.borrow_mut::<UITouchHostObject>(new_touch).start_location = location;
        autorelease(env, new_touch);

        let _: () = msg![env; touches addObject:new_touch];
        retain(env, new_touch);
        env.framework_state
            .uikit
            .ui_touch
            .current_touches
            .insert(finger_id, new_touch);
    }

    let all_touches_set: id = msg_class![env; NSMutableSet new];
    autorelease(env, all_touches_set);
    let existing_touches: Vec<id> = env
        .framework_state
        .uikit
        .ui_touch
        .current_touches
        .values()
        .cloned()
        .collect();
    for touch in existing_touches {
        let _: () = msg![env; all_touches_set addObject:touch];
    }

    let event = ui_event::new_event(env, all_touches_set);
    autorelease(env, event);
    let current_touch_ids: Vec<id> = env
        .framework_state
        .uikit
        .ui_touch
        .current_touches
        .values()
        .cloned()
        .collect();
    let views_with_existing_touches: HashSet<id> = current_touch_ids
        .into_iter()
        .map(|touch| touch_ivars(env, touch, |v| v.view))
        .collect();
    let mut view_touches: HashMap<id, id> = HashMap::new();
    let touches_arr: id = msg![env; touches allObjects];
    let touches_count: NSUInteger = msg![env;
        touches_arr count];

    for i in 0..touches_count {
        let touch: id = msg![env;
            touches_arr objectAtIndex:i];
        let location = touch_ivars(env, touch, |v| v.location);

        let windows = env.framework_state.uikit.ui_view.ui_window.windows.clone();
        let found_window = windows.iter().rev().find_map(|&window| {
            // A hidden window must never receive touches, even if it is the
            // topmost window containing the point. (iOS routes touches only to
            // visible windows.) This matters when an app leaves an empty
            // helper window — e.g. the dedicated UIWindow Doodle Jump creates
            // to host its age gate — on screen: once hidden it must stop
            // swallowing touches meant for the game window underneath.
            let hidden: bool = msg![env; window isHidden];
            if hidden {
                return None;
            }
            // Skip empty overlay windows that carry no content. Doodle Jump
            // (and other apps) present launch popups — age gate, ATT tracking
            // prompt, etc. — each in its own dedicated UIWindow. When those
            // flows are completed synchronously the app can leave the now-empty
            // helper window on screen as the key/top window; without this it
            // would swallow every touch meant for the game window beneath it.
            // A window whose view tree bottoms out in contentless root views is
            // never a legitimate touch target, so fall through to the window
            // below (the primary search below still falls back to the topmost
            // window if every candidate is empty).
            if !touchhle_window_has_content(env, window) {
                return None;
            }
            let location_in_window: CGPoint = msg![env; window
                convertPoint:location fromWindow:nil];
            if msg![env; window pointInside:location_in_window withEvent:event] {
                Some((window, location_in_window))
            } else {
                None
            }
        });
        // SUPER HACK: Если окно отвергло касание, силой отправляем его в
        // главное окно!
        let Some((window, location_in_window)) = found_window.or_else(|| {
            windows.last().map(|&window| {
                let lx = location.x;
                let ly = location.y;
                log_dbg!(
                    "SUPER HACK: Forcing rejected touch at ({}, {}) into window",
                    lx,
                    ly
                );
                let loc: CGPoint = msg![env; window convertPoint:location fromWindow:nil];
                (window, loc)
            })
        }) else {
            let lx = location.x;
            let ly = location.y;
            log!(
                "Couldn't find ANY window for touch at ({}, {}), discarding",
                lx,
                ly
            );
            continue;
        };
        let mut view: id = msg![env; window hitTest:location_in_window withEvent:event];

        // If the standard hitTest landed on an empty, transparent overlay view
        // (no subviews, not a control), retry while skipping such overlays so
        // the touch reaches the real interactive content underneath. This is
        // what lets Doodle Jump's launch cross-promo close button — hidden
        // behind an empty full-screen sibling view — actually receive taps.
        if view != nil && touchhle_is_empty_overlay(env, view) {
            let deep = touchhle_hit_test_skipping_overlays(env, window, location_in_window);
            if deep != nil {
                view = deep;
            }
        }

        if view != nil {
            let view_class: crate::objc::Class = msg![env; view class];
            let class_name = env.objc.get_class_name(view_class).to_owned();

            if class_name == "MBProgressHUD" {
                log!("Touch hit MBProgressHUD; keeping HUD as touch target");
            }
        }

        if view == nil {
            log_dbg!("SUPER HACK: hitTest failed, looking for Cocos/GL target before using window");
            let cocos_target = touchhle_find_cocos_touch_target(env, window);
            view = if cocos_target != nil {
                cocos_target
            } else {
                window
            };
        } else if view == window {
            let cocos_target = touchhle_find_cocos_touch_target(env, window);
            if cocos_target != nil {
                view = cocos_target;
            }
        } else {
            let f: CGRect = msg![env;
                view frame];
            let view_class: crate::objc::Class = msg![env; view class];
            let class_name = env.objc.get_class_name(view_class).to_owned();
            let lx = location_in_window.x;
            let ly = location_in_window.y;
            log_dbg!(
                "Touch at ({}, {}) hit {} {:?} with frame {:?}",
                lx,
                ly,
                class_name,
                view,
                f,
            );
        }

        let is_multi_touch_enabled: bool = msg![env; view isMultipleTouchEnabled];
        if !is_multi_touch_enabled
            && !touchhle_cocos_should_allow_multitouch(env, view)
            && (view_touches.contains_key(&view) || views_with_existing_touches.contains(&view))
        {
            // The view already has a finger and refuses multi-touch. Real iOS
            // delivers only the first touch of a sequence and KEEPS it alive;
            // the old code silently deleted the tracked touch instead, so the
            // first finger turned into a ghost (no more Moved/Ended, and the
            // view was never told it ended). Unity-style input state machines
            // then stalled and later swipes were randomly ignored.
            let active: Vec<(FingerId, id)> = env
                .framework_state
                .uikit
                .ui_touch
                .current_touches
                .iter()
                .map(|(&fid, &t)| (fid, t))
                .collect();
            let stale: Vec<(FingerId, id)> = active
                .into_iter()
                .filter(|&(_, t)| {
                    touch_ivars(env, t, |v| {
                        v.view == view
                            && t != touch
                            && (timestamp - v.timestamp) > STALE_TOUCH_SECONDS
                    })
                })
                .collect();
            if !stale.is_empty() {
                // Very old touches cannot be part of a live gesture; their
                // Up/Cancel was lost long ago. Cancel them visibly, then let
                // the newcomer through.
                for (fid, t) in stale {
                    stale_cancel_and_release(env, fid, t, "stale on single-touch view");
                }
            } else {
                // A live second finger on a single-touch view: like real iOS,
                // keep the tracked touch intact and drop the newcomer.
                log_dbg!("Second finger on single-touch view; dropping the new touch.");
                let new_finger = env
                    .framework_state
                    .uikit
                    .ui_touch
                    .current_touches
                    .iter()
                    .find(|(_, &v)| v == touch)
                    .map(|(&k, _)| k);
                if let Some(new_finger) = new_finger {
                    env.framework_state
                        .uikit
                        .ui_touch
                        .current_touches
                        .remove(&new_finger);
                }
                release(env, touch);
                continue;
            }
        }

        if let Entry::Vacant(e) = view_touches.entry(view) {
            let s: id = msg_class![env; NSMutableSet new];
            autorelease(env, s);
            e.insert(s);
        }
        let v_set: id = *view_touches.get(&view).unwrap();
        let _: () = msg![env; v_set addObject:touch];
        retain(env, view);
        retain(env, window);
        touch_ivars(env, touch, |v| {
            v.view = view;
            v.window = window;
            v.location = location;
        });
    }

    for (view, v_set) in view_touches {
        let _: () = msg![env;
            view touchesBegan:v_set withEvent:event];
        super::ui_gesture_recognizer::touches_began(env, view, v_set);
        touchhle_send_cocos_touch_aliases_to_chain(env, view, "began", v_set, event);
        let count: NSUInteger = msg![env; v_set count];
        log_touch_delivery("TOUCH-DIAG", &TOUCH_BEGAN_DIAGNOSTICS, count as usize);
    }
    release(env, pool);
}

fn handle_touches_move(env: &mut Environment, map: HashMap<FingerId, Coords>) {
    let pool: id = msg_class![env;
        NSAutoreleasePool new];
    let timestamp: NSTimeInterval = {
        let pi = msg_class![env; NSProcessInfo processInfo];
        msg![env; pi systemUptime]
    };

    let mut view_touches: HashMap<id, id> = HashMap::new();
    for (finger_id, coords) in map {
        let Some(&touch) = env
            .framework_state
            .uikit
            .ui_touch
            .current_touches
            .get(&finger_id)
        else {
            let n = UNTRACKED_TOUCH_WARNS.load(Ordering::Relaxed);
            if n > 0 {
                UNTRACKED_TOUCH_WARNS.fetch_sub(1, Ordering::Relaxed);
                log!(
                    "Warning: TouchesMove for untracked finger {:?}; its Down never arrived. [{} reports left]",
                    finger_id,
                    n - 1
                );
            }
            continue;
        };
        let location = CGPoint {
            x: coords.0,
            y: coords.1,
        };
        let view = touch_ivars(env, touch, |v| v.view);
        let moved = touch_ivars(env, touch, |v| {
            if v.location == location {
                false
            } else {
                v.previous_location = v.location;
                v.location = location;
                v.timestamp = timestamp;
                v.phase = UITouchPhaseMoved;
                true
            }
        });
        if !moved {
            continue;
        }
        env.objc.borrow_mut::<UITouchHostObject>(touch).move_count += 1;

        if let Entry::Vacant(e) = view_touches.entry(view) {
            let s: id = msg_class![env; NSMutableSet new];
            autorelease(env, s);
            e.insert(s);
        }
        let v_set: id = *view_touches.get(&view).unwrap();
        let _: () = msg![env; v_set addObject:touch];
    }

    let all_touches_set: id = msg_class![env; NSMutableSet new];
    autorelease(env, all_touches_set);
    let existing: Vec<id> = env
        .framework_state
        .uikit
        .ui_touch
        .current_touches
        .values()
        .cloned()
        .collect();
    for t in existing {
        let _: () = msg![env; all_touches_set addObject:t];
    }

    let event = ui_event::new_event(env, all_touches_set);
    autorelease(env, event);
    for (view, v_set) in view_touches {
        super::ui_gesture_recognizer::touches_moved(env, view, v_set);
        let deliver = touches_for_view_delivery(env, v_set);
        let count: NSUInteger = msg![env; deliver count];
        if count != 0 {
            let _: () = msg![env; view touchesMoved:deliver withEvent:event];
            touchhle_send_cocos_touch_aliases_to_chain(env, view, "moved", deliver, event);
        }
        release(env, deliver);
    }
    release(env, pool);
}

fn ultrahle_minionjump_drain_pending_callback(env: &mut Environment, select_only: bool) {
    if !matches!(
        env.bundle.bundle_identifier(),
        "com.apprisetec9.minionjump" | "com.risinghighapps.kingdomprincepro"
    ) {
        return;
    }

    let Some(target_raw) = std::env::var("ULTRAHLE_MINIONJUMP_PENDING_TARGET")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
    else {
        return;
    };

    let Some(sel_raw) = std::env::var("ULTRAHLE_MINIONJUMP_PENDING_SEL")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
    else {
        return;
    };

    let sender_raw = std::env::var("ULTRAHLE_MINIONJUMP_PENDING_SENDER")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(0);

    let callback_name = std::env::var("ULTRAHLE_MINIONJUMP_PENDING_CALLBACK")
        .unwrap_or_else(|_| "<unknown>".to_string());

    let stage =
        std::env::var("ULTRAHLE_MINIONJUMP_PENDING_STAGE").unwrap_or_else(|_| "0".to_string());

    let is_level_select = callback_name == "selectLVAction:";
    if select_only != is_level_select {
        return;
    }

    std::env::remove_var("ULTRAHLE_MINIONJUMP_PENDING_TARGET");
    std::env::remove_var("ULTRAHLE_MINIONJUMP_PENDING_SEL");
    std::env::remove_var("ULTRAHLE_MINIONJUMP_PENDING_SENDER");
    std::env::remove_var("ULTRAHLE_MINIONJUMP_PENDING_CALLBACK");
    std::env::remove_var("ULTRAHLE_MINIONJUMP_PENDING_STAGE");

    if target_raw == 0 || sel_raw == 0 {
        return;
    }

    let target_id = id::from_bits(target_raw);
    let sender = id::from_bits(sender_raw);
    let callback_sel_ptr = crate::mem::ConstPtr::<u8>::from_bits(sel_raw);
    let callback_sel: crate::objc::SEL = unsafe { std::mem::transmute(callback_sel_ptr) };

    log!(
        "UltraHLE MinionJump: draining pending callback selector={} target={:?} sender={:?} stage={} select_only={}",
        callback_name,
        target_id,
        sender,
        stage,
        select_only
    );

    if callback_name.ends_with(':') {
        let _: () = msg_send_no_type_checking(env, (target_id, callback_sel, sender));
    } else {
        let _: () = msg_send_no_type_checking(env, (target_id, callback_sel));
    }
}

fn handle_touches_up(env: &mut Environment, map: HashMap<FingerId, Coords>) {
    let pool: id = msg_class![env;
        NSAutoreleasePool new];
    let timestamp: NSTimeInterval = {
        let pi = msg_class![env; NSProcessInfo processInfo];
        msg![env; pi systemUptime]
    };

    let touches: id = msg_class![env; NSMutableSet new];
    autorelease(env, touches);
    let all_touches_set: id = msg_class![env; NSMutableSet new];
    autorelease(env, all_touches_set);
    let existing: Vec<id> = env
        .framework_state
        .uikit
        .ui_touch
        .current_touches
        .values()
        .cloned()
        .collect();
    for t in existing {
        let _: () = msg![env; all_touches_set addObject:t];
    }

    let mut view_touches: HashMap<id, id> = HashMap::new();
    // Collect finger_ids to remove from current_touches AFTER touchesEnded is
    // delivered.  Some apps (e.g. Scarface) retain a UITouch pointer inside
    // touchesEnded: and then access it on the next run-loop turn.  Releasing
    // the object before the callback returns causes "Faking borrow for missing
    // object" warnings because the retain from current_touches has already
    // been undone and the autorelease pool may have drained.  By deferring the
    // remove+release until after all touchesEnded: callbacks we guarantee the
    // objects stay alive throughout the handler.
    let mut touches_to_remove: Vec<(FingerId, id)> = Vec::new();
    for (finger_id, coords) in map {
        let Some(&touch) = env
            .framework_state
            .uikit
            .ui_touch
            .current_touches
            .get(&finger_id)
        else {
            let n = UNTRACKED_TOUCH_WARNS.load(Ordering::Relaxed);
            if n > 0 {
                UNTRACKED_TOUCH_WARNS.fetch_sub(1, Ordering::Relaxed);
                log!(
                    "Warning: TouchesUp for untracked finger {:?}; its Down never arrived. [{} reports left]",
                    finger_id,
                    n - 1
                );
            }
            continue;
        };
        let location = CGPoint {
            x: coords.0,
            y: coords.1,
        };
        let view = touch_ivars(env, touch, |v| v.view);
        touch_ivars(env, touch, |v| {
            v.previous_location = v.location;
            v.location = location;
            v.timestamp = timestamp;
            v.phase = UITouchPhaseEnded;
        });

        let _: () = msg![env;
            touches addObject:touch];

        if let Entry::Vacant(e) = view_touches.entry(view) {
            let s: id = msg_class![env; NSMutableSet new];
            autorelease(env, s);
            e.insert(s);
        }
        let v_set: id = *view_touches.get(&view).unwrap();
        let _: () = msg![env; v_set addObject:touch];
        // Defer the remove+release so the touch object is still alive when
        // touchesEnded:withEvent: runs (and for any cross-run-loop references
        // the app may hold inside that callback).
        touches_to_remove.push((finger_id, touch));
    }

    let event = ui_event::new_event(env, all_touches_set);
    autorelease(env, event);
    for (view, v_set) in view_touches {
        super::ui_gesture_recognizer::touches_ended(env, view, v_set);
        let deliver = touches_for_view_delivery(env, v_set);
        let count: NSUInteger = msg![env; deliver count];
        if count != 0 {
            let _: () = msg![env; view touchesEnded:deliver withEvent:event];
            touchhle_send_cocos_touch_aliases_to_chain(env, view, "ended", deliver, event);
            log_touch_delivery("TOUCH-END", &TOUCH_ENDED_DIAGNOSTICS, count as usize);
        }
        release(env, deliver);
    }

    // Now that all touchesEnded: callbacks have returned, remove the touches
    // from current_touches and release our retain.  The touch objects are
    // still in the NSMutableSets held by the per-view v_set locals (via
    // addObject:, which retains), so they remain alive until those sets are
    // released when the autorelease pool drains.
    for (finger_id, touch) in touches_to_remove {
        env.framework_state.uikit.ui_touch.cancelled_by_gesture.remove(&touch);
        if let Some(current_touch) = env
            .framework_state
            .uikit
            .ui_touch
            .current_touches
            .remove(&finger_id)
        {
            release(env, current_touch);
        }
    }

    // ULTRAHLE_MINIONJUMP_DRAIN_SELECT_BEGIN
    ultrahle_minionjump_drain_pending_callback(env, true);
    // ULTRAHLE_MINIONJUMP_DRAIN_SELECT_END
    // ULTRAHLE_MINIONJUMP_DRAIN_POST_BEGIN
    ultrahle_minionjump_drain_pending_callback(env, false);
    // ULTRAHLE_MINIONJUMP_DRAIN_POST_END

    release(env, pool);
}
