/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UIGestureRecognizer` and the tap/swipe recognizers available in iOS 3.2.

use std::collections::HashMap;
use std::time::{Duration, Instant};
use crate::frameworks::core_graphics::{CGPoint, CGFloat};
use crate::frameworks::foundation::{NSInteger, NSUInteger};
use crate::objc::{id, msg, msg_class, msg_send, nil, release, retain, objc_classes, ClassExports, HostObject, NSZonePtr, SEL};
use crate::Environment;

pub type UIGestureRecognizerState = NSInteger;
pub const UIGestureRecognizerStatePossible: UIGestureRecognizerState = 0;
pub const UIGestureRecognizerStateRecognized: UIGestureRecognizerState = 3;
pub const UIGestureRecognizerStateFailed: UIGestureRecognizerState = 5;

pub type UISwipeGestureRecognizerDirection = NSUInteger;
pub const UISwipeGestureRecognizerDirectionRight: UISwipeGestureRecognizerDirection = 1 << 0;
pub const UISwipeGestureRecognizerDirectionLeft: UISwipeGestureRecognizerDirection = 1 << 1;
pub const UISwipeGestureRecognizerDirectionUp: UISwipeGestureRecognizerDirection = 1 << 2;
pub const UISwipeGestureRecognizerDirectionDown: UISwipeGestureRecognizerDirection = 1 << 3;

#[derive(Clone, Copy, PartialEq, Eq)]
enum GestureKind {
    Generic,
    Tap,
    Swipe,
    LongPress,
}

pub(super) struct UIGestureRecognizerHostObject {
    // UIKit does not retain targets, delegates or the associated view.
    target: id,
    action: Option<SEL>,
    delegate: id,
    pub(super) view: id,
    kind: GestureKind,
    state: UIGestureRecognizerState,
    enabled: bool,
    cancels_touches_in_view: bool,
    delays_touches_began: bool,
    delays_touches_ended: bool,
    number_of_taps_required: NSUInteger,
    number_of_touches_required: NSUInteger,
    direction: UISwipeGestureRecognizerDirection,
    initial_location: CGPoint,
    current_location: CGPoint,
    tracking: bool,
    minimum_press_duration: f64,
    allowable_movement: CGFloat,
    press_timer: id,
    completed_taps: NSUInteger,
    previous_tap: Option<(Instant, CGPoint)>,
    press_started: Option<Instant>,
    active_touches: HashMap<id, (CGPoint, CGPoint)>,
    /// Recognizers that must fail before this one may recognize, in the
    /// order `-requireGestureRecognizerToFail:` was called. UIKit does not
    /// retain them (same as targets and the delegate).
    required_to_fail: Vec<id>,
}
impl HostObject for UIGestureRecognizerHostObject {}

impl UIGestureRecognizerHostObject {
    fn new(kind: GestureKind) -> Self {
        Self {
            target: nil,
            action: None,
            delegate: nil,
            view: nil,
            kind,
            state: UIGestureRecognizerStatePossible,
            enabled: true,
            cancels_touches_in_view: true,
            delays_touches_began: false,
            delays_touches_ended: true,
            number_of_taps_required: if kind == GestureKind::LongPress { 0 } else { 1 },
            number_of_touches_required: 1,
            direction: UISwipeGestureRecognizerDirectionRight,
            initial_location: CGPoint { x: 0.0, y: 0.0 },
            current_location: CGPoint { x: 0.0, y: 0.0 },
            tracking: false,
            minimum_press_duration: 0.5,
            allowable_movement: 10.0,
            press_timer: nil,
            completed_taps: 0,
            previous_tap: None,
            press_started: None,
            active_touches: HashMap::new(),
            required_to_fail: Vec::new(),
        }
    }
}

impl Default for UIGestureRecognizerHostObject {
    fn default() -> Self {
        Self::new(GestureKind::Generic)
    }
}

fn init_with_target(env: &mut Environment, this: id, target: id, action: SEL) -> id {
    let recognizer = env.objc.borrow_mut::<UIGestureRecognizerHostObject>(this);
    recognizer.target = target;
    recognizer.action = if action.is_null() { None } else { Some(action) };
    this
}

fn send_action(env: &mut Environment, recognizer: id) {
    let (target, action) = {
        let host = env.objc.borrow::<UIGestureRecognizerHostObject>(recognizer);
        (host.target, host.action)
    };
    let Some(action) = action else { return };
    if target == nil {
        log!("TODO: UIGestureRecognizer target:nil responder-chain dispatch is unsupported");
        return;
    }

    let colon_count = action
        .as_str(&env.mem)
        .bytes()
        .filter(|&b| b == b':')
        .count();
    match colon_count {
        0 => () = msg_send(env, (target, action)),
        1 => () = msg_send(env, (target, action, recognizer)),
        // The selector comes straight from guest code; a malformed selector
        // must not take down the host. Log and skip, like UIControl does.
        _ => log!(
            "Warning: gesture recognizer action {:?} has unsupported \
             argument count {}; skipping.",
            action.as_str(&env.mem),
            colon_count
        ),
    }
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UIGestureRecognizer: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(UIGestureRecognizerHostObject::new(GestureKind::Generic));
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)initWithTarget:(id)target action:(SEL)action {
    init_with_target(env, this, target, action)
}

- (id)delegate { env.objc.borrow::<UIGestureRecognizerHostObject>(this).delegate }
- (())setDelegate:(id)delegate {
    env.objc.borrow_mut::<UIGestureRecognizerHostObject>(this).delegate = delegate;
}
- (id)view { env.objc.borrow::<UIGestureRecognizerHostObject>(this).view }
- (())setView:(id)view {
    set_view(env, this, view);
}
- (UIGestureRecognizerState)state {
    env.objc.borrow::<UIGestureRecognizerHostObject>(this).state
}
// Apple: "Creates a dependency relationship between the receiver and
// another gesture recognizer ... the receiver will not recognize until the
// other recognizer fails." Recognition is driven per recognizer here, so we
// record the dependency and consult it when the receiver would recognize.
// <https://developer.apple.com/documentation/uikit/uigesturerecognizer/1624410-requiregesturerecognizertofail>
- (())requireGestureRecognizerToFail:(id)other {
    if other == nil || other == this { return; }
    let mut host = env.objc.borrow_mut::<UIGestureRecognizerHostObject>(this);
    if !host.required_to_fail.contains(&other) {
        host.required_to_fail.push(other);
    }
}
- (bool)isEnabled { env.objc.borrow::<UIGestureRecognizerHostObject>(this).enabled }
- (())setEnabled:(bool)enabled {
    if !enabled { cancel_long_press(env, this); }
    let host = env.objc.borrow_mut::<UIGestureRecognizerHostObject>(this);
    host.enabled = enabled;
    host.tracking = false;
    host.state = UIGestureRecognizerStatePossible;
}
- (bool)cancelsTouchesInView {
    env.objc.borrow::<UIGestureRecognizerHostObject>(this).cancels_touches_in_view
}
- (())setCancelsTouchesInView:(bool)value {
    env.objc.borrow_mut::<UIGestureRecognizerHostObject>(this).cancels_touches_in_view = value;
}
- (bool)delaysTouchesBegan {
    env.objc.borrow::<UIGestureRecognizerHostObject>(this).delays_touches_began
}
- (())setDelaysTouchesBegan:(bool)value {
    env.objc.borrow_mut::<UIGestureRecognizerHostObject>(this).delays_touches_began = value;
}
- (bool)delaysTouchesEnded {
    env.objc.borrow::<UIGestureRecognizerHostObject>(this).delays_touches_ended
}
- (())setDelaysTouchesEnded:(bool)value {
    env.objc.borrow_mut::<UIGestureRecognizerHostObject>(this).delays_touches_ended = value;
}
- (CGPoint)locationInView:(id)view {
    let (current_location, own_view) = {
        let host = env.objc.borrow::<UIGestureRecognizerHostObject>(this);
        (host.current_location, host.view)
    };
    if view == nil || view == own_view {
        current_location
    } else {
        msg![env; view convertPoint:current_location fromView:own_view]
    }
}

@end

@implementation UILongPressGestureRecognizer: UIGestureRecognizer
+ (id)allocWithZone:(NSZonePtr)_zone {
    let host = UIGestureRecognizerHostObject::new(GestureKind::LongPress);
    env.objc.alloc_object(this, Box::new(host), &mut env.mem)
}
- (f64)minimumPressDuration { env.objc.borrow::<UIGestureRecognizerHostObject>(this).minimum_press_duration }
- (())setMinimumPressDuration:(f64)value {
    if value.is_finite() && value >= 0.0 {
        env.objc.borrow_mut::<UIGestureRecognizerHostObject>(this).minimum_press_duration = value;
    }
}
- (CGFloat)allowableMovement { env.objc.borrow::<UIGestureRecognizerHostObject>(this).allowable_movement }
- (())setAllowableMovement:(CGFloat)value {
    if value.is_finite() && value >= 0.0 {
        env.objc.borrow_mut::<UIGestureRecognizerHostObject>(this).allowable_movement = value;
    }
}
- (NSUInteger)numberOfTapsRequired { env.objc.borrow::<UIGestureRecognizerHostObject>(this).number_of_taps_required }
- (())setNumberOfTapsRequired:(NSUInteger)value {
    cancel_long_press(env, this);
    env.objc.borrow_mut::<UIGestureRecognizerHostObject>(this).number_of_taps_required = value;
}
- (NSUInteger)numberOfTouchesRequired { env.objc.borrow::<UIGestureRecognizerHostObject>(this).number_of_touches_required }
- (())setNumberOfTouchesRequired:(NSUInteger)value {
    cancel_long_press(env, this);
    env.objc.borrow_mut::<UIGestureRecognizerHostObject>(this).number_of_touches_required = value.max(1);
}
- (())_touchHLE_longPressTimer:(id)timer { long_press_timer(env, this, timer); }
- (())reset {
    cancel_long_press(env, this);
    env.objc.borrow_mut::<UIGestureRecognizerHostObject>(this).state = UIGestureRecognizerStatePossible;
}
- (())dealloc {
    stop_press_timer(env, this);
    env.objc.dealloc_object(this, &mut env.mem)
}
@end

@implementation UIPanGestureRecognizer: UIGestureRecognizer

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(UIGestureRecognizerHostObject::new(GestureKind::Generic));
    env.objc.alloc_object(this, host_object, &mut env.mem)
}
// Chrome's gesture-driven UI creates pan recognizers from code; the base
// class already covers target/action, state and touch plumbing.

@end

@implementation UITapGestureRecognizer: UIGestureRecognizer

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(UIGestureRecognizerHostObject::new(GestureKind::Tap));
    env.objc.alloc_object(this, host_object, &mut env.mem)
}
- (NSUInteger)numberOfTapsRequired {
    env.objc.borrow::<UIGestureRecognizerHostObject>(this).number_of_taps_required
}
- (())setNumberOfTapsRequired:(NSUInteger)value {
    env.objc.borrow_mut::<UIGestureRecognizerHostObject>(this).number_of_taps_required = value;
}
- (NSUInteger)numberOfTouchesRequired {
    env.objc.borrow::<UIGestureRecognizerHostObject>(this).number_of_touches_required
}
- (())setNumberOfTouchesRequired:(NSUInteger)value {
    env.objc.borrow_mut::<UIGestureRecognizerHostObject>(this).number_of_touches_required = value;
}

@end


@implementation UISwipeGestureRecognizer: UIGestureRecognizer

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(UIGestureRecognizerHostObject::new(GestureKind::Swipe));
    env.objc.alloc_object(this, host_object, &mut env.mem)
}
- (UISwipeGestureRecognizerDirection)direction {
    env.objc.borrow::<UIGestureRecognizerHostObject>(this).direction
}
- (())setDirection:(UISwipeGestureRecognizerDirection)value {
    env.objc.borrow_mut::<UIGestureRecognizerHostObject>(this).direction = value;
}
- (NSUInteger)numberOfTouchesRequired {
    env.objc.borrow::<UIGestureRecognizerHostObject>(this).number_of_touches_required
}
- (())setNumberOfTouchesRequired:(NSUInteger)value {
    env.objc.borrow_mut::<UIGestureRecognizerHostObject>(this).number_of_touches_required = value;
}

@end

};

pub(super) fn set_view(env: &mut Environment, recognizer: id, view: id) {
    retain(env, recognizer);
    if env.objc.borrow::<UIGestureRecognizerHostObject>(recognizer).view != view {
        cancel_long_press(env, recognizer);
    }
    env.objc
        .borrow_mut::<UIGestureRecognizerHostObject>(recognizer)
        .view = view;
    release(env, recognizer);
}

fn delegate_allows_touch(env: &mut Environment, recognizer: id, touch: id) -> bool {
    let delegate = env
        .objc
        .borrow::<UIGestureRecognizerHostObject>(recognizer)
        .delegate;
    if delegate == nil {
        return true;
    }
    let Some(selector) = env
        .objc
        .lookup_selector("gestureRecognizer:shouldReceiveTouch:")
    else {
        return true;
    };
    if msg![env; delegate respondsToSelector:selector] {
        msg_send(env, (delegate, selector, recognizer, touch))
    } else {
        true
    }
}

fn delegate_allows_begin(env: &mut Environment, recognizer: id) -> bool {
    let delegate = env
        .objc
        .borrow::<UIGestureRecognizerHostObject>(recognizer)
        .delegate;
    if delegate == nil {
        return true;
    }
    let Some(selector) = env.objc.lookup_selector("gestureRecognizerShouldBegin:") else {
        return true;
    };
    if msg![env; delegate respondsToSelector:selector] {
        msg_send(env, (delegate, selector, recognizer))
    } else {
        true
    }
}

pub(super) fn touches_began(env: &mut Environment, view: id, touches: id) {
    if view == nil {
        return;
    }
    let touch: id = msg![env; touches anyObject];
    if touch == nil {
        return;
    }
    let recognizers = recognizers_in_chain(env, view);
    for &recognizer in &recognizers { retain(env, recognizer); }
    for &recognizer in &recognizers {
        if env.objc.borrow::<UIGestureRecognizerHostObject>(recognizer).kind == GestureKind::LongPress {
            long_press_touches(env, recognizer, touches, 0);
            continue;
        }
        let (enabled, recognizer_view) = {
            let host = env.objc.borrow::<UIGestureRecognizerHostObject>(recognizer);
            (host.enabled, host.view)
        };
        if !enabled || !delegate_allows_touch(env, recognizer, touch) {
            continue;
        }
        let location: CGPoint = msg![env; touch locationInView:recognizer_view];
        let host = env
            .objc
            .borrow_mut::<UIGestureRecognizerHostObject>(recognizer);
        host.initial_location = location;
        host.current_location = location;
        host.state = UIGestureRecognizerStatePossible;
        host.tracking = true;
    }
    for recognizer in recognizers { release(env, recognizer); }
}

pub(super) fn touches_moved(env: &mut Environment, view: id, touches: id) {
    if view == nil {
        return;
    }
    let touch: id = msg![env; touches anyObject];
    if touch == nil {
        return;
    }
    let recognizers = recognizers_in_chain(env, view);
    for &recognizer in &recognizers { retain(env, recognizer); }
    for &recognizer in &recognizers {
        if env.objc.borrow::<UIGestureRecognizerHostObject>(recognizer).kind == GestureKind::LongPress {
            long_press_touches(env, recognizer, touches, 1);
            continue;
        }
        let (tracking, recognizer_view) = {
            let host = env.objc.borrow::<UIGestureRecognizerHostObject>(recognizer);
            (host.tracking, host.view)
        };
        if tracking {
            let location: CGPoint = msg![env; touch locationInView:recognizer_view];
            env.objc
                .borrow_mut::<UIGestureRecognizerHostObject>(recognizer)
                .current_location = location;
        }
    }
    for recognizer in recognizers { release(env, recognizer); }
}

pub(super) fn touches_ended(env: &mut Environment, view: id, touches: id) {
    if view == nil {
        return;
    }
    let touch: id = msg![env; touches anyObject];
    if touch == nil {
        return;
    }
    let recognizers = recognizers_in_chain(env, view);
    for &recognizer in &recognizers { retain(env, recognizer); }
    for &recognizer in &recognizers {
        if env.objc.borrow::<UIGestureRecognizerHostObject>(recognizer).kind == GestureKind::LongPress {
            long_press_touches(env, recognizer, touches, 2);
            continue;
        }
        let (tracking, recognizer_view) = {
            let host = env.objc.borrow::<UIGestureRecognizerHostObject>(recognizer);
            (host.tracking, host.view)
        };
        if !tracking {
            continue;
        }
        let location: CGPoint = msg![env; touch locationInView:recognizer_view];
        {
            let host = env
                .objc
                .borrow_mut::<UIGestureRecognizerHostObject>(recognizer);
            host.current_location = location;
            host.tracking = false;
        }

        let recognized = {
            let host = env.objc.borrow::<UIGestureRecognizerHostObject>(recognizer);
            let dx = host.current_location.x - host.initial_location.x;
            let dy = host.current_location.y - host.initial_location.y;
            match host.kind {
                GestureKind::Tap => host.number_of_taps_required == 1 && dx * dx + dy * dy <= 100.0,
                GestureKind::Swipe => {
                    let (direction, distance, cross_distance) = if dx.abs() >= dy.abs() {
                        (
                            if dx >= 0.0 {
                                UISwipeGestureRecognizerDirectionRight
                            } else {
                                UISwipeGestureRecognizerDirectionLeft
                            },
                            dx.abs(),
                            dy.abs(),
                        )
                    } else {
                        (
                            if dy >= 0.0 {
                                UISwipeGestureRecognizerDirectionDown
                            } else {
                                UISwipeGestureRecognizerDirectionUp
                            },
                            dy.abs(),
                            dx.abs(),
                        )
                    };
                    distance >= 24.0
                        && cross_distance <= distance
                        && host.direction & direction != 0
                }
                GestureKind::Generic | GestureKind::LongPress => false,
            }
        };

        // A recognizer gated by `-requireGestureRecognizerToFail:` may only
        // recognize while none of its dependencies has begun or recognized.
        let superseded = {
            let required = env
                .objc
                .borrow::<UIGestureRecognizerHostObject>(recognizer)
                .required_to_fail
                .clone();
            required.iter().any(|&other| {
                env.objc
                    .get_host_object(other)
                    .and_then(|host| host.as_any().downcast_ref::<UIGestureRecognizerHostObject>())
                    .map(|host| matches!(host.state, 1 | 2 | 3))
                    .unwrap_or(false)
            })
        };

        if recognized && !superseded && delegate_allows_begin(env, recognizer) {
            env.objc
                .borrow_mut::<UIGestureRecognizerHostObject>(recognizer)
                .state = UIGestureRecognizerStateRecognized;
            send_action(env, recognizer);
        } else {
            env.objc
                .borrow_mut::<UIGestureRecognizerHostObject>(recognizer)
                .state = UIGestureRecognizerStateFailed;
        }
    }
    for recognizer in recognizers { release(env, recognizer); }
}

// A recognizer on a container observes touches hitting any descendant.
fn recognizers_in_chain(env: &mut Environment, mut view: id) -> Vec<id> {
    let mut result = Vec::new();
    let mut seen = std::collections::HashSet::new();
    while view != nil && seen.insert(view) {
        for r in super::ui_view::gesture_recognizers(env, view) {
            if !result.contains(&r) { result.push(r); }
        }
        view = msg![env; view superview];
    }
    result
}

fn stop_press_timer(env: &mut Environment, recognizer: id) {
    let timer = std::mem::replace(&mut env.objc.borrow_mut::<UIGestureRecognizerHostObject>(recognizer).press_timer, nil);
    if timer != nil {
        () = msg![env; timer invalidate];
        release(env, timer);
    }
}

fn cancel_long_press(env: &mut Environment, recognizer: id) {
    let h = env.objc.borrow::<UIGestureRecognizerHostObject>(recognizer);
    if h.kind != GestureKind::LongPress { return; }
    retain(env, recognizer);
    stop_press_timer(env, recognizer);
    let h = env.objc.borrow_mut::<UIGestureRecognizerHostObject>(recognizer);
    let active = matches!(h.state, 1 | 2);
    h.state = if active { 4 } else { UIGestureRecognizerStateFailed };
    h.active_touches.clear();
    h.completed_taps = 0;
    h.previous_tap = None;
    h.press_started = None;
    h.tracking = false;
    if active { send_action(env, recognizer); }
    release(env, recognizer);
}

fn movement_allowed(a: CGPoint, b: CGPoint, limit: CGFloat) -> bool {
    let dx = a.x - b.x;
    let dy = a.y - b.y;
    dx * dx + dy * dy <= limit * limit
}

fn long_press_timer(env: &mut Environment, recognizer: id, timer: id) {
    let h = env.objc.borrow::<UIGestureRecognizerHostObject>(recognizer);
    if h.press_timer != timer { return; }
    let ready = h.enabled && h.view != nil && h.tracking && h.state == 0
        && h.active_touches.len() == h.number_of_touches_required as usize;
    stop_press_timer(env, recognizer);
    if !ready || !delegate_allows_begin(env, recognizer) {
        env.objc.borrow_mut::<UIGestureRecognizerHostObject>(recognizer).state = UIGestureRecognizerStateFailed;
        return;
    }
    // Delegate code may have detached or disabled the recognizer.
    let h = env.objc.borrow_mut::<UIGestureRecognizerHostObject>(recognizer);
    if !h.enabled || h.view == nil || !h.tracking { return; }
    h.state = 1; // Began, while the finger is still down, not at release.
    h.completed_taps = 0;
    h.previous_tap = None;
    let cancel = h.cancels_touches_in_view;
    let touches: Vec<id> = h.active_touches.keys().copied().collect();
    if cancel { super::ui_touch::cancel_for_gesture(env, &touches); }
    send_action(env, recognizer);
}

fn long_press_touches(env: &mut Environment, recognizer: id, touches: id, phase: u8) {
    retain(env, recognizer);
    long_press_touches_inner(env, recognizer, touches, phase);
    release(env, recognizer);
}

fn long_press_touches_inner(env: &mut Environment, recognizer: id, touches: id, phase: u8) {
    let h = env.objc.borrow::<UIGestureRecognizerHostObject>(recognizer);
    if !h.enabled { return; }
    let view = h.view;
    let array: id = msg![env; touches allObjects];
    let count: NSUInteger = msg![env; array count];
    let mut failed = false;
    for i in 0..count {
        let touch: id = msg![env; array objectAtIndex:i];
        if phase == 0 && !delegate_allows_touch(env, recognizer, touch) { continue; }
        let point: CGPoint = msg![env; touch locationInView:view];
        let h = env.objc.borrow_mut::<UIGestureRecognizerHostObject>(recognizer);
        if phase == 0 {
            if h.active_touches.is_empty() {
                let follows_tap = h.previous_tap.map(|(when, at)|
                    when.elapsed() <= Duration::from_millis(300) && movement_allowed(at, point, h.allowable_movement)
                ).unwrap_or(false);
                if !follows_tap { h.completed_taps = 0; }
                h.press_started = Some(Instant::now());
                h.state = 0;
                h.tracking = true;
            }
            h.active_touches.insert(touch, (point, point));
        } else if let Some((start, current)) = h.active_touches.get_mut(&touch) {
            *current = point;
            // Once recognised, motion reports Changed rather than failing.
            if h.state == 0 && !movement_allowed(*start, point, h.allowable_movement) { failed = true; }
        }
        if phase == 2 { h.active_touches.remove(&touch); }
    }
    let h = env.objc.borrow_mut::<UIGestureRecognizerHostObject>(recognizer);
    let n = h.active_touches.len();
    if n > 0 {
        let (x, y) = h.active_touches.values().fold((0.0, 0.0), |(x, y), (_, p)| (x + p.x, y + p.y));
        h.current_location = CGPoint { x: x / n as f32, y: y / n as f32 };
    }
    if n > h.number_of_touches_required as usize { failed = true; }
    let active = matches!(h.state, 1 | 2);
    if failed {
        cancel_long_press(env, recognizer);
    } else if phase == 2 && n < h.number_of_touches_required as usize {
        if !active && h.tracking && h.number_of_taps_required > 0
            && h.press_started.map(|at| at.elapsed() <= Duration::from_millis(300)).unwrap_or(false) {
            h.completed_taps = (h.completed_taps + 1).min(h.number_of_taps_required);
            h.previous_tap = Some((Instant::now(), h.current_location));
        }
        h.tracking = false;
        h.state = if active { 3 } else { 5 };
        stop_press_timer(env, recognizer);
        if active { send_action(env, recognizer); }
    } else if active && phase == 1 {
        h.state = 2; // Changed
        send_action(env, recognizer);
    } else if h.state == 0 && h.tracking && h.press_timer == nil && n == h.number_of_touches_required as usize
        && h.completed_taps == h.number_of_taps_required {
        let duration = h.minimum_press_duration;
        let selector = env.objc.lookup_selector("_touchHLE_longPressTimer:").unwrap();
        let timer: id = msg_class![env; NSTimer scheduledTimerWithTimeInterval:duration target:recognizer selector:selector userInfo:nil repeats:false];
        retain(env, timer);
        env.objc.borrow_mut::<UIGestureRecognizerHostObject>(recognizer).press_timer = timer;
    }
}

#[cfg(test)]
mod long_press_tests {
    use super::*;
    #[test]
    fn defaults_and_motion_threshold() {
        let h = UIGestureRecognizerHostObject::new(GestureKind::LongPress);
        assert_eq!(h.minimum_press_duration, 0.5);
        assert_eq!(h.number_of_taps_required, 0);
        assert_eq!(h.number_of_touches_required, 1);
        assert!(h.press_timer == nil);
        assert!(movement_allowed(CGPoint { x: 0.0, y: 0.0 }, CGPoint { x: 6.0, y: 8.0 }, h.allowable_movement));
        assert!(!movement_allowed(CGPoint { x: 0.0, y: 0.0 }, CGPoint { x: 6.0, y: 8.1 }, h.allowable_movement));
    }
}
