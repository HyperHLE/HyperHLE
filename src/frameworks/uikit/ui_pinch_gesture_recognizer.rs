/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UIGestureRecognizer` and concrete subclasses.
//!
//! touchHLE processes touches directly via `UITouch` / `UIEvent`, so gesture
//! recognisers are implemented as a lightweight layer on top: they track the
//! raw touch sequence and fire the registered target/action pair when the
//! gesture is detected.

use crate::frameworks::core_graphics::{CGFloat, CGPoint};
use crate::frameworks::foundation::NSInteger;
use crate::objc::{
    autorelease, id, msg, msg_class, msg_send, nil, objc_classes, release, retain,
    ClassExports, HostObject, NSZonePtr, SEL,
};

// MARK: - UIGestureRecognizerState

type UIGestureRecognizerState = NSInteger;
const UIGestureRecognizerStatePossible:   UIGestureRecognizerState = 0;
const UIGestureRecognizerStateBegan:      UIGestureRecognizerState = 1;
const UIGestureRecognizerStateChanged:    UIGestureRecognizerState = 2;
const UIGestureRecognizerStateEnded:      UIGestureRecognizerState = 3;
const UIGestureRecognizerStateCancelled:  UIGestureRecognizerState = 4;
const UIGestureRecognizerStateFailed:     UIGestureRecognizerState = 5;
// Aliases
const UIGestureRecognizerStateRecognized: UIGestureRecognizerState = UIGestureRecognizerStateEnded;

// MARK: - Host objects

/// Shared fields for all gesture recognisers.
struct UIGestureRecognizerHostObject {
    /// `id` — weak (delegate pattern)
    delegate: id,
    /// Target/action pairs: `Vec<(target: id, action: SEL)>`
    /// Targets are *retained*.
    targets: Vec<(id, SEL)>,
    state: UIGestureRecognizerState,
    /// `UIView*` — weak
    view: id,
    enabled: bool,
    cancels_touches_in_view: bool,
    delays_touches_began: bool,
    delays_touches_ended: bool,
    requires_exclusive_touch: bool,
    /// Gesture recognisers that must fail before this one can fire.
    /// `NSMutableArray<UIGestureRecognizer*>*` — retained
    dependencies: id,
    /// Current touch location (updated on each touch event).
    location: CGPoint,
    /// Previous touch location (for pan translation).
    prev_location: CGPoint,
}
impl HostObject for UIGestureRecognizerHostObject {}

struct UIPanGestureRecognizerHostObject {
    super_: UIGestureRecognizerHostObject,
    minimum_number_of_touches: u32,
    maximum_number_of_touches: u32,
    /// Accumulated translation since gesture began.
    translation: CGPoint,
    /// Current velocity estimate (pixels/second).
    velocity: CGPoint,
}
impl HostObject for UIPanGestureRecognizerHostObject {}

struct UITapGestureRecognizerHostObject {
    super_: UIGestureRecognizerHostObject,
    number_of_taps_required:    u32,
    number_of_touches_required: u32,
    /// How many taps have been counted in the current sequence.
    tap_count: u32,
}
impl HostObject for UITapGestureRecognizerHostObject {}

struct UILongPressGestureRecognizerHostObject {
    super_: UIGestureRecognizerHostObject,
    number_of_taps_required:     u32,
    number_of_touches_required:  u32,
    minimum_press_duration:      f64,
    allowable_movement:          CGFloat,
}
impl HostObject for UILongPressGestureRecognizerHostObject {}

struct UIPinchGestureRecognizerHostObject {
    super_: UIGestureRecognizerHostObject,
    scale:    CGFloat,
    velocity: CGFloat,
}
impl HostObject for UIPinchGestureRecognizerHostObject {}

struct UIRotationGestureRecognizerHostObject {
    super_: UIGestureRecognizerHostObject,
    rotation: CGFloat,
    velocity: CGFloat,
}
impl HostObject for UIRotationGestureRecognizerHostObject {}

struct UISwipeGestureRecognizerHostObject {
    super_: UIGestureRecognizerHostObject,
    direction:                  u32,
    number_of_touches_required: u32,
}
impl HostObject for UISwipeGestureRecognizerHostObject {}

struct UIScreenEdgePanGestureRecognizerHostObject {
    super_: UIPanGestureRecognizerHostObject,
    edges: u32,
}
impl HostObject for UIScreenEdgePanGestureRecognizerHostObject {}

// MARK: - Helper

fn make_recognizer_base(env: &mut crate::Environment) -> UIGestureRecognizerHostObject {
    let deps: id = msg_class![env; NSMutableArray new];
    UIGestureRecognizerHostObject {
        delegate:                 nil,
        targets:                  Vec::new(),
        state:                    UIGestureRecognizerStatePossible,
        view:                     nil,
        enabled:                  true,
        cancels_touches_in_view:  true,
        delays_touches_began:     false,
        delays_touches_ended:     true,
        requires_exclusive_touch: false,
        dependencies:             deps,
        location:                 CGPoint { x: 0.0, y: 0.0 },
        prev_location:            CGPoint { x: 0.0, y: 0.0 },
    }
}

/// Fire all registered target/action pairs.
fn fire_actions(env: &mut crate::Environment, this: id) {
    // Copy the targets vec to avoid borrow conflicts.
    let pairs: Vec<(id, SEL)> = env.objc
        .borrow::<UIGestureRecognizerHostObject>(this)
        .targets.clone();
    for (target, action) in pairs {
        if target != nil && !action.is_null() {
            let sel_str = action.as_str(&env.mem).to_string();
            if sel_str.ends_with(':') {
                let _: () = msg_send(env, (target, action, this));
            } else {
                let _: () = msg_send(env, (target, action));
            }
        }
    }
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

// =========================================================================
// UIGestureRecognizer (abstract base)
// =========================================================================

@implementation UIGestureRecognizer: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let base = make_recognizer_base(env);
    let host_object = Box::new(base);
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

// MARK: - Init

- (id)initWithTarget:(id)target action:(SEL)action {
    if target != nil && !action.is_null() {
        retain(env, target);
        env.objc.borrow_mut::<UIGestureRecognizerHostObject>(this)
            .targets.push((target, action));
    }
    this
}

- (())dealloc {
    let host = env.objc.borrow::<UIGestureRecognizerHostObject>(this);
    let deps = host.dependencies;
    for (t, _) in &host.targets { release(env, *t); }
    release(env, deps);
    env.objc.dealloc_object(this, &mut env.mem)
}

// MARK: - Targets

- (())addTarget:(id)target action:(SEL)action {
    if target == nil || action.is_null() { return; }
    retain(env, target);
    env.objc.borrow_mut::<UIGestureRecognizerHostObject>(this)
        .targets.push((target, action));
}

- (())removeTarget:(id)target action:(SEL)action {
    if target == nil || action.is_null() { return; }
    retain(env, target);
    env.objc.borrow_mut::<UIGestureRecognizerHostObject>(this)
        .targets.push((target, action));
}

// MARK: - State

- (UIGestureRecognizerState)state {
    env.objc.borrow::<UIGestureRecognizerHostObject>(this).state
}

- (())setState:(UIGestureRecognizerState)state {
    env.objc.borrow_mut::<UIGestureRecognizerHostObject>(this).state = state;
}

// MARK: - View

- (id)view {
    env.objc.borrow::<UIGestureRecognizerHostObject>(this).view
}

- (())setView:(id)view {
    // Weak — no retain.
    env.objc.borrow_mut::<UIGestureRecognizerHostObject>(this).view = view;
}

// MARK: - Enabled

- (bool)isEnabled {
    env.objc.borrow::<UIGestureRecognizerHostObject>(this).enabled
}

- (())setEnabled:(bool)enabled {
    env.objc.borrow_mut::<UIGestureRecognizerHostObject>(this).enabled = enabled;
}

// MARK: - Delegate

- (id)delegate {
    env.objc.borrow::<UIGestureRecognizerHostObject>(this).delegate
}

- (())setDelegate:(id)delegate {
    // Weak — no retain.
    env.objc.borrow_mut::<UIGestureRecognizerHostObject>(this).delegate = delegate;
}

// MARK: - Behaviour flags

- (bool)cancelsTouchesInView {
    env.objc.borrow::<UIGestureRecognizerHostObject>(this).cancels_touches_in_view
}
- (())setCancelsTouchesInView:(bool)v {
    env.objc.borrow_mut::<UIGestureRecognizerHostObject>(this).cancels_touches_in_view = v;
}

- (bool)delaysTouchesBegan {
    env.objc.borrow::<UIGestureRecognizerHostObject>(this).delays_touches_began
}
- (())setDelaysTouchesBegan:(bool)v {
    env.objc.borrow_mut::<UIGestureRecognizerHostObject>(this).delays_touches_began = v;
}

- (bool)delaysTouchesEnded {
    env.objc.borrow::<UIGestureRecognizerHostObject>(this).delays_touches_ended
}
- (())setDelaysTouchesEnded:(bool)v {
    env.objc.borrow_mut::<UIGestureRecognizerHostObject>(this).delays_touches_ended = v;
}

- (bool)requiresExclusiveTouchType {
    env.objc.borrow::<UIGestureRecognizerHostObject>(this).requires_exclusive_touch
}
- (())setRequiresExclusiveTouchType:(bool)v {
    env.objc.borrow_mut::<UIGestureRecognizerHostObject>(this).requires_exclusive_touch = v;
}

// MARK: - Dependencies

- (())requireGestureRecognizerToFail:(id)other {
    let deps = env.objc.borrow::<UIGestureRecognizerHostObject>(this).dependencies;
    () = msg![env; deps addObject:other];
}

// MARK: - Location

- (CGPoint)locationInView:(id)_view {
    env.objc.borrow::<UIGestureRecognizerHostObject>(this).location
}

- (CGPoint)locationOfTouch:(u32)_touch_index inView:(id)_view {
    env.objc.borrow::<UIGestureRecognizerHostObject>(this).location
}

- (u32)numberOfTouches {
    1
}

// MARK: - Touch event hooks (called by the view's touch handling)

- (())touchesBegan:(id)_touches withEvent:(id)_event {
    let host = env.objc.borrow_mut::<UIGestureRecognizerHostObject>(this);
    host.state = UIGestureRecognizerStatePossible;
}

- (())touchesMoved:(id)_touches withEvent:(id)_event {}

- (())touchesEnded:(id)_touches withEvent:(id)_event {
    env.objc.borrow_mut::<UIGestureRecognizerHostObject>(this).state =
        UIGestureRecognizerStateFailed;
}

- (())touchesCancelled:(id)_touches withEvent:(id)_event {
    env.objc.borrow_mut::<UIGestureRecognizerHostObject>(this).state =
        UIGestureRecognizerStateCancelled;
}

- (())reset {
    env.objc.borrow_mut::<UIGestureRecognizerHostObject>(this).state =
        UIGestureRecognizerStatePossible;
}

// MARK: - Ignore

- (())ignoreTouch:(id)_touch forEvent:(id)_event {}

@end

// =========================================================================
// UIPanGestureRecognizer
// =========================================================================

@implementation UIPanGestureRecognizer: UIGestureRecognizer

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(UIPanGestureRecognizerHostObject {
        super_: make_recognizer_base(env),
        minimum_number_of_touches: 1,
        maximum_number_of_touches: u32::MAX,
        translation: CGPoint { x: 0.0, y: 0.0 },
        velocity:    CGPoint { x: 0.0, y: 0.0 },
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)initWithTarget:(id)target action:(SEL)action {
    if target != nil && !action.is_null() {
        retain(env, target);
        env.objc.borrow_mut::<UIPanGestureRecognizerHostObject>(this)
            .super_.targets.push((target, action));
    }
    this
}

- (())dealloc {
    let host = env.objc.borrow::<UIPanGestureRecognizerHostObject>(this);
    let deps = host.super_.dependencies;
    for (t, _) in &host.super_.targets { release(env, *t); }
    release(env, deps);
    env.objc.dealloc_object(this, &mut env.mem)
}

// MARK: Properties

- (u32)minimumNumberOfTouches {
    env.objc.borrow::<UIPanGestureRecognizerHostObject>(this).minimum_number_of_touches
}
- (())setMinimumNumberOfTouches:(u32)n {
    env.objc.borrow_mut::<UIPanGestureRecognizerHostObject>(this)
        .minimum_number_of_touches = n.max(1);
}

- (u32)maximumNumberOfTouches {
    env.objc.borrow::<UIPanGestureRecognizerHostObject>(this).maximum_number_of_touches
}
- (())setMaximumNumberOfTouches:(u32)n {
    env.objc.borrow_mut::<UIPanGestureRecognizerHostObject>(this)
        .maximum_number_of_touches = n;
}

// MARK: Translation / velocity

- (CGPoint)translationInView:(id)_view {
    env.objc.borrow::<UIPanGestureRecognizerHostObject>(this).translation
}

- (())setTranslation:(CGPoint)translation inView:(id)_view {
    env.objc.borrow_mut::<UIPanGestureRecognizerHostObject>(this).translation = translation;
}

- (CGPoint)velocityInView:(id)_view {
    env.objc.borrow::<UIPanGestureRecognizerHostObject>(this).velocity
}

// MARK: Location

- (CGPoint)locationInView:(id)_view {
    env.objc.borrow::<UIPanGestureRecognizerHostObject>(this).super_.location
}

- (UIGestureRecognizerState)state {
    env.objc.borrow::<UIPanGestureRecognizerHostObject>(this).super_.state
}

// MARK: Touch events

- (())touchesBegan:(id)touches withEvent:(id)_event {
    let touch: id = msg![env; touches anyObject];
    if touch == nil { return; }
    let loc: CGPoint = msg![env; touch locationInView:nil];
    let host = env.objc.borrow_mut::<UIPanGestureRecognizerHostObject>(this);
    host.super_.location      = loc;
    host.super_.prev_location = loc;
    host.super_.state         = UIGestureRecognizerStatePossible;
    host.translation          = CGPoint { x: 0.0, y: 0.0 };
    host.velocity             = CGPoint { x: 0.0, y: 0.0 };
}

- (())touchesMoved:(id)touches withEvent:(id)_event {
    let touch: id = msg![env; touches anyObject];
    if touch == nil { return; }
    let loc: CGPoint = msg![env; touch locationInView:nil];
    {
        let host = env.objc.borrow_mut::<UIPanGestureRecognizerHostObject>(this);
        let prev = host.super_.prev_location;
        let dx = loc.x - prev.x;
        let dy = loc.y - prev.y;
        host.translation.x += dx;
        host.translation.y += dy;
        // Simple velocity estimate (pixels per ~16ms frame → ×60 ≈ px/s).
        host.velocity = CGPoint { x: dx * 60.0, y: dy * 60.0 };
        host.super_.prev_location = loc;
        host.super_.location      = loc;

        let old_state = host.super_.state;
        host.super_.state = if old_state == UIGestureRecognizerStatePossible {
            UIGestureRecognizerStateBegan
        } else {
            UIGestureRecognizerStateChanged
        };
    }
    if env.objc.borrow::<UIPanGestureRecognizerHostObject>(this).super_.enabled {
        fire_actions(env, this);
    }
}

- (())touchesEnded:(id)_touches withEvent:(id)_event {
    let old = env.objc.borrow::<UIPanGestureRecognizerHostObject>(this).super_.state;
    if old == UIGestureRecognizerStateBegan || old == UIGestureRecognizerStateChanged {
        env.objc.borrow_mut::<UIPanGestureRecognizerHostObject>(this).super_.state =
            UIGestureRecognizerStateEnded;
        if env.objc.borrow::<UIPanGestureRecognizerHostObject>(this).super_.enabled {
            fire_actions(env, this);
        }
    }
    env.objc.borrow_mut::<UIPanGestureRecognizerHostObject>(this).super_.state =
        UIGestureRecognizerStatePossible;
    env.objc.borrow_mut::<UIPanGestureRecognizerHostObject>(this).translation =
        CGPoint { x: 0.0, y: 0.0 };
}

- (())touchesCancelled:(id)_touches withEvent:(id)_event {
    env.objc.borrow_mut::<UIPanGestureRecognizerHostObject>(this).super_.state =
        UIGestureRecognizerStateCancelled;
    if env.objc.borrow::<UIPanGestureRecognizerHostObject>(this).super_.enabled {
        fire_actions(env, this);
    }
    env.objc.borrow_mut::<UIPanGestureRecognizerHostObject>(this).super_.state =
        UIGestureRecognizerStatePossible;
}

- (())reset {
    let host = env.objc.borrow_mut::<UIPanGestureRecognizerHostObject>(this);
    host.super_.state = UIGestureRecognizerStatePossible;
    host.translation  = CGPoint { x: 0.0, y: 0.0 };
    host.velocity     = CGPoint { x: 0.0, y: 0.0 };
}

// MARK: Delegate / misc forwarded from super

- (id)delegate { env.objc.borrow::<UIPanGestureRecognizerHostObject>(this).super_.delegate }
- (())setDelegate:(id)d { env.objc.borrow_mut::<UIPanGestureRecognizerHostObject>(this).super_.delegate = d; }
- (bool)isEnabled { env.objc.borrow::<UIPanGestureRecognizerHostObject>(this).super_.enabled }
- (())setEnabled:(bool)v { env.objc.borrow_mut::<UIPanGestureRecognizerHostObject>(this).super_.enabled = v; }
- (bool)cancelsTouchesInView { env.objc.borrow::<UIPanGestureRecognizerHostObject>(this).super_.cancels_touches_in_view }
- (())setCancelsTouchesInView:(bool)v { env.objc.borrow_mut::<UIPanGestureRecognizerHostObject>(this).super_.cancels_touches_in_view = v; }
- (())addTarget:(id)target action:(SEL)action {
    if target == nil || action.is_null() { return; }
    retain(env, target);
    env.objc.borrow_mut::<UIPanGestureRecognizerHostObject>(this).super_.targets.push((target, action));
}
- (())requireGestureRecognizerToFail:(id)other {
    let deps = env.objc.borrow::<UIPanGestureRecognizerHostObject>(this).super_.dependencies;
    () = msg![env; deps addObject:other];
}

@end

// =========================================================================
// UITapGestureRecognizer
// =========================================================================

@implementation UITapGestureRecognizer: UIGestureRecognizer

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(UITapGestureRecognizerHostObject {
        super_: make_recognizer_base(env),
        number_of_taps_required:    1,
        number_of_touches_required: 1,
        tap_count: 0,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)initWithTarget:(id)target action:(SEL)action {
    if target != nil && !action.is_null() {
        retain(env, target);
        env.objc.borrow_mut::<UITapGestureRecognizerHostObject>(this)
            .super_.targets.push((target, action));
    }
    this
}

- (())dealloc {
    let host = env.objc.borrow::<UITapGestureRecognizerHostObject>(this);
    let deps = host.super_.dependencies;
    for (t, _) in &host.super_.targets { release(env, *t); }
    release(env, deps);
    env.objc.dealloc_object(this, &mut env.mem)
}

// MARK: Properties

- (u32)numberOfTapsRequired {
    env.objc.borrow::<UITapGestureRecognizerHostObject>(this).number_of_taps_required
}
- (())setNumberOfTapsRequired:(u32)n {
    env.objc.borrow_mut::<UITapGestureRecognizerHostObject>(this).number_of_taps_required = n.max(1);
}

- (u32)numberOfTouchesRequired {
    env.objc.borrow::<UITapGestureRecognizerHostObject>(this).number_of_touches_required
}
- (())setNumberOfTouchesRequired:(u32)n {
    env.objc.borrow_mut::<UITapGestureRecognizerHostObject>(this).number_of_touches_required = n.max(1);
}

// MARK: State / location

- (UIGestureRecognizerState)state {
    env.objc.borrow::<UITapGestureRecognizerHostObject>(this).super_.state
}

- (CGPoint)locationInView:(id)_view {
    env.objc.borrow::<UITapGestureRecognizerHostObject>(this).super_.location
}

// MARK: Touch events

- (())touchesBegan:(id)touches withEvent:(id)_event {
    let touch: id = msg![env; touches anyObject];
    if touch == nil { return; }
    let loc: CGPoint = msg![env; touch locationInView:nil];
    let host = env.objc.borrow_mut::<UITapGestureRecognizerHostObject>(this);
    host.super_.location = loc;
    host.super_.state    = UIGestureRecognizerStatePossible;
    host.tap_count       = 0;
}

- (())touchesMoved:(id)touches withEvent:(id)_event {
    // Movement cancels a tap.
    let touch: id = msg![env; touches anyObject];
    if touch == nil { return; }
    let loc: CGPoint  = msg![env; touch locationInView:nil];
    let prev = env.objc.borrow::<UITapGestureRecognizerHostObject>(this).super_.location;
    let dx = (loc.x - prev.x).abs();
    let dy = (loc.y - prev.y).abs();
    if dx > 10.0 || dy > 10.0 {
        env.objc.borrow_mut::<UITapGestureRecognizerHostObject>(this).super_.state =
            UIGestureRecognizerStateFailed;
    }
}

- (())touchesEnded:(id)touches withEvent:(id)_event {
    let state = env.objc.borrow::<UITapGestureRecognizerHostObject>(this).super_.state;
    if state == UIGestureRecognizerStateFailed { return; }

    let touch: id = msg![env; touches anyObject];
    if touch != nil {
        let tc: u32 = msg![env; touch tapCount];
        env.objc.borrow_mut::<UITapGestureRecognizerHostObject>(this).tap_count = tc;
    }

    let (required, actual) = {
        let host = env.objc.borrow::<UITapGestureRecognizerHostObject>(this);
        (host.number_of_taps_required, host.tap_count)
    };

    if actual >= required {
        env.objc.borrow_mut::<UITapGestureRecognizerHostObject>(this).super_.state =
            UIGestureRecognizerStateRecognized;
        if env.objc.borrow::<UITapGestureRecognizerHostObject>(this).super_.enabled {
            fire_actions(env, this);
        }
    } else {
        env.objc.borrow_mut::<UITapGestureRecognizerHostObject>(this).super_.state =
            UIGestureRecognizerStateFailed;
    }
    env.objc.borrow_mut::<UITapGestureRecognizerHostObject>(this).super_.state =
        UIGestureRecognizerStatePossible;
}

- (())touchesCancelled:(id)_touches withEvent:(id)_event {
    env.objc.borrow_mut::<UITapGestureRecognizerHostObject>(this).super_.state =
        UIGestureRecognizerStateCancelled;
    env.objc.borrow_mut::<UITapGestureRecognizerHostObject>(this).super_.state =
        UIGestureRecognizerStatePossible;
}

- (())reset {
    let host = env.objc.borrow_mut::<UITapGestureRecognizerHostObject>(this);
    host.super_.state = UIGestureRecognizerStatePossible;
    host.tap_count    = 0;
}

// Forwarded properties
- (id)delegate { env.objc.borrow::<UITapGestureRecognizerHostObject>(this).super_.delegate }
- (())setDelegate:(id)d { env.objc.borrow_mut::<UITapGestureRecognizerHostObject>(this).super_.delegate = d; }
- (bool)isEnabled { env.objc.borrow::<UITapGestureRecognizerHostObject>(this).super_.enabled }
- (())setEnabled:(bool)v { env.objc.borrow_mut::<UITapGestureRecognizerHostObject>(this).super_.enabled = v; }
- (bool)cancelsTouchesInView { env.objc.borrow::<UITapGestureRecognizerHostObject>(this).super_.cancels_touches_in_view }
- (())setCancelsTouchesInView:(bool)v { env.objc.borrow_mut::<UITapGestureRecognizerHostObject>(this).super_.cancels_touches_in_view = v; }
- (())addTarget:(id)target action:(SEL)action {
    if target == nil || action.is_null() { return; }
    retain(env, target);
    env.objc.borrow_mut::<UITapGestureRecognizerHostObject>(this).super_.targets.push((target, action));
}
- (())requireGestureRecognizerToFail:(id)other {
    let deps = env.objc.borrow::<UITapGestureRecognizerHostObject>(this).super_.dependencies;
    () = msg![env; deps addObject:other];
}

@end

// =========================================================================
// UILongPressGestureRecognizer
// =========================================================================

@implementation UILongPressGestureRecognizer: UIGestureRecognizer

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(UILongPressGestureRecognizerHostObject {
        super_: make_recognizer_base(env),
        number_of_taps_required:    0,
        number_of_touches_required: 1,
        minimum_press_duration:     0.5,
        allowable_movement:         10.0,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)initWithTarget:(id)target action:(SEL)action {
    if target != nil && !action.is_null() {
        retain(env, target);
        env.objc.borrow_mut::<UILongPressGestureRecognizerHostObject>(this)
            .super_.targets.push((target, action));
    }
    this
}

- (())dealloc {
    let host = env.objc.borrow::<UILongPressGestureRecognizerHostObject>(this);
    let deps = host.super_.dependencies;
    for (t, _) in &host.super_.targets { release(env, *t); }
    release(env, deps);
    env.objc.dealloc_object(this, &mut env.mem)
}

- (f64)minimumPressDuration {
    env.objc.borrow::<UILongPressGestureRecognizerHostObject>(this).minimum_press_duration
}
- (())setMinimumPressDuration:(f64)d {
    env.objc.borrow_mut::<UILongPressGestureRecognizerHostObject>(this).minimum_press_duration = d;
}
- (CGFloat)allowableMovement {
    env.objc.borrow::<UILongPressGestureRecognizerHostObject>(this).allowable_movement
}
- (())setAllowableMovement:(CGFloat)m {
    env.objc.borrow_mut::<UILongPressGestureRecognizerHostObject>(this).allowable_movement = m;
}
- (u32)numberOfTapsRequired {
    env.objc.borrow::<UILongPressGestureRecognizerHostObject>(this).number_of_taps_required
}
- (())setNumberOfTapsRequired:(u32)n {
    env.objc.borrow_mut::<UILongPressGestureRecognizerHostObject>(this).number_of_taps_required = n;
}
- (u32)numberOfTouchesRequired {
    env.objc.borrow::<UILongPressGestureRecognizerHostObject>(this).number_of_touches_required
}
- (())setNumberOfTouchesRequired:(u32)n {
    env.objc.borrow_mut::<UILongPressGestureRecognizerHostObject>(this).number_of_touches_required = n;
}

- (UIGestureRecognizerState)state {
    env.objc.borrow::<UILongPressGestureRecognizerHostObject>(this).super_.state
}
- (CGPoint)locationInView:(id)_view {
    env.objc.borrow::<UILongPressGestureRecognizerHostObject>(this).super_.location
}
- (id)delegate { env.objc.borrow::<UILongPressGestureRecognizerHostObject>(this).super_.delegate }
- (())setDelegate:(id)d { env.objc.borrow_mut::<UILongPressGestureRecognizerHostObject>(this).super_.delegate = d; }
- (bool)isEnabled { env.objc.borrow::<UILongPressGestureRecognizerHostObject>(this).super_.enabled }
- (())setEnabled:(bool)v { env.objc.borrow_mut::<UILongPressGestureRecognizerHostObject>(this).super_.enabled = v; }
- (bool)cancelsTouchesInView { env.objc.borrow::<UILongPressGestureRecognizerHostObject>(this).super_.cancels_touches_in_view }
- (())setCancelsTouchesInView:(bool)v { env.objc.borrow_mut::<UILongPressGestureRecognizerHostObject>(this).super_.cancels_touches_in_view = v; }
- (())addTarget:(id)target action:(SEL)action {
    if target == nil || action.is_null() { return; }
    retain(env, target);
    env.objc.borrow_mut::<UILongPressGestureRecognizerHostObject>(this).super_.targets.push((target, action));
}

// Long press is not triggered automatically in our model — just track location.
- (())touchesBegan:(id)touches withEvent:(id)_event {
    let touch: id = msg![env; touches anyObject];
    if touch == nil { return; }
    let loc: CGPoint = msg![env; touch locationInView:nil];
    let host = env.objc.borrow_mut::<UILongPressGestureRecognizerHostObject>(this);
    host.super_.location = loc;
    host.super_.state    = UIGestureRecognizerStatePossible;
}
- (())touchesMoved:(id)_touches withEvent:(id)_event {}
- (())touchesEnded:(id)_touches withEvent:(id)_event {
    env.objc.borrow_mut::<UILongPressGestureRecognizerHostObject>(this).super_.state =
        UIGestureRecognizerStatePossible;
}
- (())touchesCancelled:(id)_touches withEvent:(id)_event {
    env.objc.borrow_mut::<UILongPressGestureRecognizerHostObject>(this).super_.state =
        UIGestureRecognizerStateCancelled;
}
- (())reset {
    env.objc.borrow_mut::<UILongPressGestureRecognizerHostObject>(this).super_.state =
        UIGestureRecognizerStatePossible;
}

@end

// =========================================================================
// UIPinchGestureRecognizer
// =========================================================================

@implementation UIPinchGestureRecognizer: UIGestureRecognizer

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(UIPinchGestureRecognizerHostObject {
        super_: make_recognizer_base(env),
        scale: 1.0, velocity: 0.0,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)initWithTarget:(id)target action:(SEL)action {
    if target != nil && !action.is_null() {
        retain(env, target);
        env.objc.borrow_mut::<UIPinchGestureRecognizerHostObject>(this)
            .super_.targets.push((target, action));
    }
    this
}

- (())dealloc {
    let host = env.objc.borrow::<UIPinchGestureRecognizerHostObject>(this);
    let deps = host.super_.dependencies;
    for (t, _) in &host.super_.targets { release(env, *t); }
    release(env, deps);
    env.objc.dealloc_object(this, &mut env.mem)
}

- (CGFloat)scale { env.objc.borrow::<UIPinchGestureRecognizerHostObject>(this).scale }
- (())setScale:(CGFloat)s { env.objc.borrow_mut::<UIPinchGestureRecognizerHostObject>(this).scale = s; }
- (CGFloat)velocity { env.objc.borrow::<UIPinchGestureRecognizerHostObject>(this).velocity }
- (UIGestureRecognizerState)state { env.objc.borrow::<UIPinchGestureRecognizerHostObject>(this).super_.state }
- (CGPoint)locationInView:(id)_view { env.objc.borrow::<UIPinchGestureRecognizerHostObject>(this).super_.location }
- (id)delegate { env.objc.borrow::<UIPinchGestureRecognizerHostObject>(this).super_.delegate }
- (())setDelegate:(id)d { env.objc.borrow_mut::<UIPinchGestureRecognizerHostObject>(this).super_.delegate = d; }
- (bool)isEnabled { env.objc.borrow::<UIPinchGestureRecognizerHostObject>(this).super_.enabled }
- (())setEnabled:(bool)v { env.objc.borrow_mut::<UIPinchGestureRecognizerHostObject>(this).super_.enabled = v; }
- (())addTarget:(id)target action:(SEL)action {
    if target == nil || action.is_null() { return; }
    retain(env, target);
    env.objc.borrow_mut::<UIPinchGestureRecognizerHostObject>(this).super_.targets.push((target, action));
}
- (())touchesBegan:(id)_t withEvent:(id)_e {
    env.objc.borrow_mut::<UIPinchGestureRecognizerHostObject>(this).super_.state = UIGestureRecognizerStatePossible;
}
- (())touchesMoved:(id)_t withEvent:(id)_e {}
- (())touchesEnded:(id)_t withEvent:(id)_e {
    env.objc.borrow_mut::<UIPinchGestureRecognizerHostObject>(this).super_.state = UIGestureRecognizerStatePossible;
}
- (())touchesCancelled:(id)_t withEvent:(id)_e {
    env.objc.borrow_mut::<UIPinchGestureRecognizerHostObject>(this).super_.state = UIGestureRecognizerStateCancelled;
}
- (())reset {
    env.objc.borrow_mut::<UIPinchGestureRecognizerHostObject>(this).super_.state = UIGestureRecognizerStatePossible;
    env.objc.borrow_mut::<UIPinchGestureRecognizerHostObject>(this).scale = 1.0;
}

@end

// =========================================================================
// UIRotationGestureRecognizer
// =========================================================================

@implementation UIRotationGestureRecognizer: UIGestureRecognizer

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(UIRotationGestureRecognizerHostObject {
        super_: make_recognizer_base(env),
        rotation: 0.0, velocity: 0.0,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)initWithTarget:(id)target action:(SEL)action {
    if target != nil && !action.is_null() {
        retain(env, target);
        env.objc.borrow_mut::<UIRotationGestureRecognizerHostObject>(this)
            .super_.targets.push((target, action));
    }
    this
}

- (())dealloc {
    let host = env.objc.borrow::<UIRotationGestureRecognizerHostObject>(this);
    let deps = host.super_.dependencies;
    for (t, _) in &host.super_.targets { release(env, *t); }
    release(env, deps);
    env.objc.dealloc_object(this, &mut env.mem)
}

- (CGFloat)rotation { env.objc.borrow::<UIRotationGestureRecognizerHostObject>(this).rotation }
- (())setRotation:(CGFloat)r { env.objc.borrow_mut::<UIRotationGestureRecognizerHostObject>(this).rotation = r; }
- (CGFloat)velocity { env.objc.borrow::<UIRotationGestureRecognizerHostObject>(this).velocity }
- (UIGestureRecognizerState)state { env.objc.borrow::<UIRotationGestureRecognizerHostObject>(this).super_.state }
- (id)delegate { env.objc.borrow::<UIRotationGestureRecognizerHostObject>(this).super_.delegate }
- (())setDelegate:(id)d { env.objc.borrow_mut::<UIRotationGestureRecognizerHostObject>(this).super_.delegate = d; }
- (bool)isEnabled { env.objc.borrow::<UIRotationGestureRecognizerHostObject>(this).super_.enabled }
- (())setEnabled:(bool)v { env.objc.borrow_mut::<UIRotationGestureRecognizerHostObject>(this).super_.enabled = v; }
- (())addTarget:(id)target action:(SEL)action {
    if target == nil || action.is_null() { return; }
    retain(env, target);
    env.objc.borrow_mut::<UIRotationGestureRecognizerHostObject>(this).super_.targets.push((target, action));
}
- (())touchesBegan:(id)_t withEvent:(id)_e {}
- (())touchesMoved:(id)_t withEvent:(id)_e {}
- (())touchesEnded:(id)_t withEvent:(id)_e {
    env.objc.borrow_mut::<UIRotationGestureRecognizerHostObject>(this).super_.state = UIGestureRecognizerStatePossible;
}
- (())touchesCancelled:(id)_t withEvent:(id)_e {
    env.objc.borrow_mut::<UIRotationGestureRecognizerHostObject>(this).super_.state = UIGestureRecognizerStateCancelled;
}
- (())reset {
    env.objc.borrow_mut::<UIRotationGestureRecognizerHostObject>(this).rotation = 0.0;
    env.objc.borrow_mut::<UIRotationGestureRecognizerHostObject>(this).super_.state = UIGestureRecognizerStatePossible;
}

@end

// =========================================================================
// UISwipeGestureRecognizer
// =========================================================================

@implementation UISwipeGestureRecognizer: UIGestureRecognizer

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(UISwipeGestureRecognizerHostObject {
        super_: make_recognizer_base(env),
        direction: 1, // UISwipeGestureRecognizerDirectionRight
        number_of_touches_required: 1,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)initWithTarget:(id)target action:(SEL)action {
    if target != nil && !action.is_null() {
        retain(env, target);
        env.objc.borrow_mut::<UISwipeGestureRecognizerHostObject>(this)
            .super_.targets.push((target, action));
    }
    this
}

- (())dealloc {
    let host = env.objc.borrow::<UISwipeGestureRecognizerHostObject>(this);
    let deps = host.super_.dependencies;
    for (t, _) in &host.super_.targets { release(env, *t); }
    release(env, deps);
    env.objc.dealloc_object(this, &mut env.mem)
}

- (u32)direction { env.objc.borrow::<UISwipeGestureRecognizerHostObject>(this).direction }
- (())setDirection:(u32)d { env.objc.borrow_mut::<UISwipeGestureRecognizerHostObject>(this).direction = d; }
- (u32)numberOfTouchesRequired { env.objc.borrow::<UISwipeGestureRecognizerHostObject>(this).number_of_touches_required }
- (())setNumberOfTouchesRequired:(u32)n { env.objc.borrow_mut::<UISwipeGestureRecognizerHostObject>(this).number_of_touches_required = n; }
- (UIGestureRecognizerState)state { env.objc.borrow::<UISwipeGestureRecognizerHostObject>(this).super_.state }
- (CGPoint)locationInView:(id)_view { env.objc.borrow::<UISwipeGestureRecognizerHostObject>(this).super_.location }
- (id)delegate { env.objc.borrow::<UISwipeGestureRecognizerHostObject>(this).super_.delegate }
- (())setDelegate:(id)d { env.objc.borrow_mut::<UISwipeGestureRecognizerHostObject>(this).super_.delegate = d; }
- (bool)isEnabled { env.objc.borrow::<UISwipeGestureRecognizerHostObject>(this).super_.enabled }
- (())setEnabled:(bool)v { env.objc.borrow_mut::<UISwipeGestureRecognizerHostObject>(this).super_.enabled = v; }
- (())addTarget:(id)target action:(SEL)action {
    if target == nil || action.is_null() { return; }
    retain(env, target);
    env.objc.borrow_mut::<UISwipeGestureRecognizerHostObject>(this).super_.targets.push((target, action));
}
- (())touchesBegan:(id)touches withEvent:(id)_event {
    let touch: id = msg![env; touches anyObject];
    if touch == nil { return; }
    let loc: CGPoint = msg![env; touch locationInView:nil];
    let host = env.objc.borrow_mut::<UISwipeGestureRecognizerHostObject>(this);
    host.super_.location = loc;
    host.super_.prev_location = loc;
    host.super_.state = UIGestureRecognizerStatePossible;
}
- (())touchesMoved:(id)touches withEvent:(id)_event {
    let touch: id = msg![env; touches anyObject];
    if touch == nil { return; }
    let loc: CGPoint = msg![env; touch locationInView:nil];
    env.objc.borrow_mut::<UISwipeGestureRecognizerHostObject>(this).super_.location = loc;
}
- (())touchesEnded:(id)_touches withEvent:(id)_event {
    let host = env.objc.borrow::<UISwipeGestureRecognizerHostObject>(this);
    let start = host.super_.prev_location;
    let end   = host.super_.location;
    let dx = end.x - start.x;
    let dy = end.y - start.y;
    let enabled = host.super_.enabled;
    drop(host);
    // Detect swipe direction (30px threshold).
    let detected = if dx.abs() > dy.abs() && dx.abs() > 30.0 {
        let dir = if dx > 0.0 { 1u32 } else { 2u32 };
        dir & env.objc.borrow::<UISwipeGestureRecognizerHostObject>(this).direction != 0
    } else if dy.abs() > 30.0 {
        let dir = if dy > 0.0 { 8u32 } else { 4u32 };
        dir & env.objc.borrow::<UISwipeGestureRecognizerHostObject>(this).direction != 0
    } else {
        false
    };
    if detected {
        env.objc.borrow_mut::<UISwipeGestureRecognizerHostObject>(this).super_.state =
            UIGestureRecognizerStateRecognized;
        if enabled { fire_actions(env, this); }
    }
    env.objc.borrow_mut::<UISwipeGestureRecognizerHostObject>(this).super_.state =
        UIGestureRecognizerStatePossible;
}
- (())touchesCancelled:(id)_t withEvent:(id)_e {
    env.objc.borrow_mut::<UISwipeGestureRecognizerHostObject>(this).super_.state = UIGestureRecognizerStateCancelled;
}
- (())reset {
    env.objc.borrow_mut::<UISwipeGestureRecognizerHostObject>(this).super_.state = UIGestureRecognizerStatePossible;
}

@end

// =========================================================================
// UIScreenEdgePanGestureRecognizer
// =========================================================================

@implementation UIScreenEdgePanGestureRecognizer: UIPanGestureRecognizer

+ (id)allocWithZone:(NSZonePtr)_zone {
    let pan = UIPanGestureRecognizerHostObject {
        super_: make_recognizer_base(env),
        minimum_number_of_touches: 1,
        maximum_number_of_touches: u32::MAX,
        translation: CGPoint { x: 0.0, y: 0.0 },
        velocity:    CGPoint { x: 0.0, y: 0.0 },
    };
    let host_object = Box::new(UIScreenEdgePanGestureRecognizerHostObject {
        super_: pan,
        edges: 0,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (u32)edges { env.objc.borrow::<UIScreenEdgePanGestureRecognizerHostObject>(this).edges }
- (())setEdges:(u32)e { env.objc.borrow_mut::<UIScreenEdgePanGestureRecognizerHostObject>(this).edges = e; }

@end

};
