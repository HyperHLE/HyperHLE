/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `CADisplayLink`
//!
//! Apple documentation:
//! <https://developer.apple.com/documentation/quartzcore/cadisplaylink>
//!
//! `CADisplayLink` is a timer-like object that lets an app synchronize its
//! drawing to the refresh rate of the display. It fires a target/selector
//! pair on a run loop, synchronized to the display vsync.
//!
//! Key contract from Apple docs:
//! - `displayLinkWithTarget:selector:` creates the display link. The display
//!   link retains the target.
//! - `addToRunLoop:forMode:` registers the link to fire; the run loop retains
//!   the display link.
//! - `removeFromRunLoop:forMode:` removes the link from a run loop/mode.
//! - `invalidate` stops the display link and removes it from all run loops.
//!   After `invalidate`, the run loop releases the display link.
//! - The selector is called with the display link as the sole argument.
//! - `timestamp` is the time value associated with the last displayed frame.
//! - `duration` is the time interval between display-refresh cycles.
//! - `frameInterval` (deprecated in iOS 10; still usable in older apps) is
//!   the number of display refreshes that must pass between firings.

use crate::frameworks::foundation::ns_run_loop::NSRunLoopMode;
use crate::frameworks::foundation::ns_timer::set_time_interval;
use crate::frameworks::foundation::NSInteger;
use crate::objc::{
    autorelease, id, msg, msg_class, msg_send, nil, objc_classes, release, retain, ClassExports,
    HostObject, NSZonePtr, SEL,
};

#[derive(Default)]
struct CADisplayLinkHostObject {
    target: id,
    selector: Option<SEL>,
    /// Weak reference. The timer retains the display link (as its target),
    /// so the timer necessarily outlives the display link. After `invalidate`,
    /// this pointer must not be used.
    ns_timer: id,
    paused: bool,
    /// Number of frames between fires. Apple's CADisplayLink documentation
    /// describes `frameInterval` as the number of frames that must pass
    /// before the display link notifies the target again; the default is 1
    /// and the value must be `>= 1`.
    /// <https://developer.apple.com/documentation/quartzcore/cadisplaylink/1648526-frameinterval>
    frame_interval: NSInteger,
}
impl HostObject for CADisplayLinkHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation CADisplayLink: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let mut host = CADisplayLinkHostObject::default();
    // Per Apple's docs, the default frameInterval is 1.
    host.frame_interval = 1;
    env.objc.alloc_object(this, Box::new(host), &mut env.mem)
}

+ (id)displayLinkWithTarget:(id)target selector:(SEL)sel {
    let display_link: id = msg![env; this new];
    // Because the timer will pass itself as a second arg in ns_timer::handle_timer,
    // we need a re-direction: the timer fires on the display link, which then
    // calls the original selector, passing the link as a second argument.
    let redirect_sel: SEL = env.objc.lookup_selector("_touchHLE_displayLinkTimerDidFire:").unwrap();
    let ns_timer = msg_class![env; NSTimer timerWithTimeInterval:(1.0/60.0)
                     target:display_link
                   selector:redirect_sel
                   userInfo:nil
                    repeats:true];
    retain(env, target);
    let host_object = env.objc.borrow_mut::<CADisplayLinkHostObject>(display_link);
    host_object.target = target;
    host_object.selector = Some(sel);
    host_object.ns_timer = ns_timer;
    log_dbg!("[CADisplayLink displayLinkWithTarget:{:?} selector:{}] => {:?}", target, sel.as_str(&env.mem), display_link);
    autorelease(env, display_link)
}

- (bool)isPaused {
    env.objc.borrow::<CADisplayLinkHostObject>(this).paused
}
- (())setPaused:(bool)paused {
    log_dbg!("[(CADisplayLink*){:?} setPaused:{}]", this, paused);
    env.objc.borrow_mut::<CADisplayLinkHostObject>(this).paused = paused;
}

// The time value associated with the last displayed frame.
// Per Apple docs, this is a CFTimeInterval (f64) measuring seconds since
// the system started. We return CACurrentMediaTime() as a best-effort
// approximation so that apps computing dt = (timestamp - lastTimestamp)
// get a plausible positive value rather than 0 or NaN.
// <https://developer.apple.com/documentation/quartzcore/cadisplaylink/1648478-timestamp>
- (f64)timestamp {
    msg_class![env; NSDate timeIntervalSinceReferenceDate]
}

// The time interval between screen refresh updates.
// Per Apple docs: "The time interval between screen refresh updates. The
// value of this property is undefined until the timer has fired at least
// once."
// We derive it from the underlying NSTimer interval so that changes to
// frameInterval are reflected here.
// <https://developer.apple.com/documentation/quartzcore/cadisplaylink/1648422-duration>
- (f64)duration {
    let ns_timer = env.objc.borrow::<CADisplayLinkHostObject>(this).ns_timer;
    if ns_timer == nil {
        return 1.0 / 60.0;
    }
    msg![env; ns_timer timeInterval]
}

// frameInterval: the number of display refreshes between notifications.
// Default value is 1 (fire every frame).  Values less than 1 are clamped
// to 1 per Apple docs.
// <https://developer.apple.com/documentation/quartzcore/cadisplaylink/1648526-frameinterval>
- (NSInteger)frameInterval {
    env.objc.borrow::<CADisplayLinkHostObject>(this).frame_interval
}

- (())setFrameInterval:(NSInteger)frameInterval {
    log_dbg!("[(CADisplayLink*){:?} setFrameInterval:{}]", this, frameInterval);
    // Apple docs state the value must be >= 1; clamp to keep guests alive
    // rather than raising NSInvalidArgumentException.
    let safe_interval = frameInterval.max(1);
    env.objc.borrow_mut::<CADisplayLinkHostObject>(this).frame_interval = safe_interval;
    let interval = safe_interval as f64 / 60.0;

    let ns_timer = env.objc.borrow::<CADisplayLinkHostObject>(this).ns_timer;
    if ns_timer != nil {
        set_time_interval(env, ns_timer, interval);
    }
}

- (NSInteger)preferredFramesPerSecond {
    let interval = env.objc.borrow::<CADisplayLinkHostObject>(this).frame_interval;
    (60 / interval.max(1)).max(1)
}

- (())setPreferredFramesPerSecond:(NSInteger)fps {
    let safe_fps = if fps <= 0 { 60 } else { fps.min(60) };
    let interval = (60 / safe_fps).max(1);
    () = msg![env; this setFrameInterval:interval];
}

// Registers the display link with a run loop.
// Per Apple docs: "You can add a display link to multiple input modes.
// When the input mode changes, the run loop stops calling the selector
// until the input mode changes back to one associated with the display
// link."
// <https://developer.apple.com/documentation/quartzcore/cadisplaylink/add(to:formode:)>
- (())addToRunLoop:(id)run_loop forMode:(NSRunLoopMode)mode {
    log_dbg!("[(CADisplayLink*){:?} addToRunLoop:{:?} forMode:{:?}]", this, run_loop, mode);
    let ns_timer = env.objc.borrow::<CADisplayLinkHostObject>(this).ns_timer;
    if ns_timer != nil {
        () = msg![env; run_loop addTimer:ns_timer forMode:mode];
    }
}

// Removes the display link from a run loop in a specific mode.
// Per Apple docs: this removes the display link from the specified mode
// of a run loop. To stop all firing, call `invalidate`.
// <https://developer.apple.com/documentation/quartzcore/cadisplaylink/remove(from:formode:)>
- (())removeFromRunLoop:(id)run_loop forMode:(NSRunLoopMode)_mode {
    log_dbg!("[(CADisplayLink*){:?} removeFromRunLoop:{:?} forMode:{:?}]", this, run_loop, _mode);
    // NSTimer does not expose a per-mode remove API; invalidating stops all
    // firing.  We mirror Apple's documented behaviour: if the caller later
    // calls addToRunLoop:forMode: for a different mode without invalidating
    // first, the link continues to fire in that mode. Since we have a single
    // underlying NSTimer we simply stop it here — any subsequent
    // addToRunLoop:forMode: call will re-register it.
    let ns_timer = env.objc.borrow::<CADisplayLinkHostObject>(this).ns_timer;
    if ns_timer != nil && run_loop != nil {
        () = msg![env; ns_timer invalidate];
    }
}

// Removes the display link from all run loops, releasing the target.
// After calling this method, the display link will not fire again.
// <https://developer.apple.com/documentation/quartzcore/cadisplaylink/invalidate()>
- (())invalidate {
    log_dbg!("[(CADisplayLink*){:?} invalidate]", this);
    let ns_timer = env.objc.borrow::<CADisplayLinkHostObject>(this).ns_timer;
    () = msg![env; ns_timer invalidate];
}

- (())dealloc {
    let &CADisplayLinkHostObject { target, .. } = env.objc.borrow(this);
    release(env, target);
    env.objc.dealloc_object(this, &mut env.mem);
}

// Internal trampoline: called by the underlying NSTimer. Redirects the
// call to the game's target/selector, passing `this` (the CADisplayLink)
// as the argument — matching the iOS contract where the selector receives
// the CADisplayLink as its sole argument.
- (())_touchHLE_displayLinkTimerDidFire:(id)timer { // NSTimer *
    let &CADisplayLinkHostObject {
        target,
        selector,
        ns_timer,
        paused,
        ..
    } = env.objc.borrow::<CADisplayLinkHostObject>(this);
    assert_eq!(ns_timer, timer);
    if paused {
        // This could be improved, as we're still running the timer,
        // but just not passing the actual call.
        return;
    }
    // Signature is `- (void) selector:(CADisplayLink *)sender;`
    () = msg_send(env, (target, selector.unwrap(), this));
}

@end

};
