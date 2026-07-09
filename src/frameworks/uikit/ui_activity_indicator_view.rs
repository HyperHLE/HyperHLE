/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UIActivityIndicatorView`.
//!
//! The indicator is drawn entirely in `-drawRect:` using the shared iOS 5
//! theme helper (`ios5_theme::draw_activity_indicator`). While animating, a
//! repeating `NSTimer` marks the view as needing display so the spinner
//! actually rotates on screen; the rotation phase itself is derived from the
//! emulator's wall clock so it stays smooth regardless of the exact timer
//! cadence.

use crate::frameworks::core_graphics::cg_context::CGContextRef;
use crate::frameworks::core_graphics::{CGFloat, CGPoint, CGRect, CGSize};
use crate::frameworks::foundation::NSInteger;
use crate::frameworks::uikit::ui_graphics::UIGraphicsGetCurrentContext;
use crate::frameworks::uikit::ui_view::ios5_theme;
use crate::objc::{
    id, impl_HostObject_with_superclass, msg, msg_class, msg_super, nil, objc_classes, release,
    retain, ClassExports, NSZonePtr,
};
use crate::Environment;

type UIActivityIndicatorViewStyle = NSInteger;
/// Large white spinner (37×37). The iOS default.
const UIActivityIndicatorViewStyleWhiteLarge: UIActivityIndicatorViewStyle = 0;
/// Small white spinner (20×20).
const UIActivityIndicatorViewStyleWhite: UIActivityIndicatorViewStyle = 1;
/// Small grey spinner (20×20), for use on light backgrounds.
const UIActivityIndicatorViewStyleGray: UIActivityIndicatorViewStyle = 2;

/// How many full rotations per second the spinner performs.
const ROTATIONS_PER_SECOND: CGFloat = 1.0;

#[derive(Default)]
pub struct UIActivityIndicatorViewHostObject {
    superclass: super::ui_view::UIViewHostObject,
    animating: bool,
    hides_when_stopped: bool,
    style: UIActivityIndicatorViewStyle,
    /// Repeating timer that drives the animation (weak-ish: retained by the
    /// run loop, invalidated + released by us). `nil` when not animating.
    spin_timer: id,
}
impl_HostObject_with_superclass!(UIActivityIndicatorViewHostObject);

/// Canonical on-screen size for a given style.
fn default_size_for_style(style: UIActivityIndicatorViewStyle) -> CGSize {
    match style {
        UIActivityIndicatorViewStyleWhiteLarge => CGSize {
            width: 37.0,
            height: 37.0,
        },
        _ => CGSize {
            width: 20.0,
            height: 20.0,
        },
    }
}

/// Spoke colour for a given style.
fn color_for_style(style: UIActivityIndicatorViewStyle) -> ios5_theme::Rgba {
    match style {
        UIActivityIndicatorViewStyleGray => (0.55, 0.55, 0.55, 1.0),
        // Both white styles.
        _ => (1.0, 1.0, 1.0, 1.0),
    }
}

/// If the view currently has an empty frame, give it the canonical size for
/// its style so there is something to draw and tap.
fn apply_default_size_if_needed(env: &mut Environment, this: id) {
    let bounds: CGRect = msg![env; this bounds];
    if bounds.size.width > 0.0 && bounds.size.height > 0.0 {
        return;
    }
    let style = env
        .objc
        .borrow::<UIActivityIndicatorViewHostObject>(this)
        .style;
    let size = default_size_for_style(style);
    let frame: CGRect = msg![env; this frame];
    let new_frame = CGRect {
        origin: frame.origin,
        size,
    };
    () = msg![env; this setFrame:new_frame];
}

/// Start the repeating redraw timer (idempotent).
fn start_spin_timer(env: &mut Environment, this: id) {
    let existing = env
        .objc
        .borrow::<UIActivityIndicatorViewHostObject>(this)
        .spin_timer;
    if existing != nil {
        return;
    }
    let sel = env
        .objc
        .lookup_selector("_touchHLE_activityIndicatorTick:")
        .unwrap();
    // ~12 fps is plenty to read as smooth rotation for a 12-spoke wheel and
    // keeps the redraw cost negligible.
    let interval: f64 = 1.0 / 12.0;
    let timer: id = msg_class![env; NSTimer scheduledTimerWithTimeInterval:interval
                                                                   target:this
                                                                 selector:sel
                                                                 userInfo:nil
                                                                  repeats:true];
    let timer = retain(env, timer);
    env.objc
        .borrow_mut::<UIActivityIndicatorViewHostObject>(this)
        .spin_timer = timer;
}

/// Stop and release the repeating redraw timer (idempotent).
fn stop_spin_timer(env: &mut Environment, this: id) {
    let timer = env
        .objc
        .borrow_mut::<UIActivityIndicatorViewHostObject>(this)
        .spin_timer;
    if timer == nil {
        return;
    }
    () = msg![env; timer invalidate];
    release(env, timer);
    env.objc
        .borrow_mut::<UIActivityIndicatorViewHostObject>(this)
        .spin_timer = nil;
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UIActivityIndicatorView: UIView

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(UIActivityIndicatorViewHostObject {
        superclass: Default::default(),
        animating: false,
        hides_when_stopped: true, // По умолчанию в iOS это свойство равно YES
        style: UIActivityIndicatorViewStyleWhiteLarge,
        spin_timer: nil,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)initWithFrame:(CGRect)frame {
    let this: id = msg_super![env; this initWithFrame:frame];
    // The indicator draws its own content; keep the backing layer clear.
    let clear: id = msg_class![env; UIColor clearColor];
    () = msg![env; this setBackgroundColor:clear];
    () = msg![env; this setOpaque:false];
    apply_default_size_if_needed(env, this);
    this
}

- (id)initWithActivityIndicatorStyle:(UIActivityIndicatorViewStyle)style {
    // UIView's designated initialiser is -initWithFrame:; use the canonical
    // size for the requested style.
    let size = default_size_for_style(style);
    let frame = CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size };
    let this: id = msg![env; this initWithFrame:frame];
    () = msg![env; this setActivityIndicatorViewStyle:style];
    this
}

- (())dealloc {
    stop_spin_timer(env, this);
    msg_super![env; this dealloc]
}

- (())setActivityIndicatorViewStyle:(UIActivityIndicatorViewStyle)style {
    env.objc.borrow_mut::<UIActivityIndicatorViewHostObject>(this).style = style;
    () = msg![env; this setNeedsDisplay];
}
- (UIActivityIndicatorViewStyle)activityIndicatorViewStyle {
    env.objc.borrow::<UIActivityIndicatorViewHostObject>(this).style
}

- (())startAnimating {
    env.objc.borrow_mut::<UIActivityIndicatorViewHostObject>(this).animating = true;

    // The indicator becomes visible when it starts animating.
    () = msg![env; this setHidden:false];
    apply_default_size_if_needed(env, this);
    start_spin_timer(env, this);
    () = msg![env; this setNeedsDisplay];

    log!("UIActivityIndicatorView: started animating [{:?}]", this);
}

- (())stopAnimating {
    let hides = {
        let host = env.objc.borrow_mut::<UIActivityIndicatorViewHostObject>(this);
        host.animating = false;
        host.hides_when_stopped
    };

    stop_spin_timer(env, this);
    log!("UIActivityIndicatorView: stopped animating [{:?}]", this);

    // If hidesWhenStopped is set, hide the view; otherwise repaint the static
    // (dimmed) spokes.
    if hides {
        () = msg![env; this setHidden:true];
    } else {
        () = msg![env; this setNeedsDisplay];
    }
}

- (bool)isAnimating {
    env.objc.borrow::<UIActivityIndicatorViewHostObject>(this).animating
}

- (())setHidesWhenStopped:(bool)hides {
    env.objc.borrow_mut::<UIActivityIndicatorViewHostObject>(this).hides_when_stopped = hides;
}

- (bool)hidesWhenStopped {
    env.objc.borrow::<UIActivityIndicatorViewHostObject>(this).hides_when_stopped
}

// Repeating-timer callback: just request a redraw. The actual rotation phase
// is computed from the wall clock in -drawRect:, so a missed/late tick only
// costs a frame rather than desyncing the animation.
- (())_touchHLE_activityIndicatorTick:(id)_timer {
    () = msg![env; this setNeedsDisplay];
}

- (())drawRect:(CGRect)_rect {
    let ctx: CGContextRef = UIGraphicsGetCurrentContext(env);
    if ctx.is_null() {
        return;
    }
    let (style, animating) = {
        let host = env.objc.borrow::<UIActivityIndicatorViewHostObject>(this);
        (host.style, host.animating)
    };
    let bounds: CGRect = msg![env; this bounds];

    // Derive the rotation phase from the emulator's wall clock so the spinner
    // turns at a steady rate independent of the redraw cadence. When stopped,
    // freeze at phase 0 and dim so the control still reads as "idle".
    let elapsed = env.startup_time.elapsed().as_secs_f32();
    let phase = if animating {
        (elapsed * ROTATIONS_PER_SECOND).fract()
    } else {
        0.0
    };
    let mut color = color_for_style(style);
    if !animating {
        color.3 *= 0.4;
    }

    ios5_theme::draw_activity_indicator(env, ctx, bounds, color, phase);
}

@end

};
