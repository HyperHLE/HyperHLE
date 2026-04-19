/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UITextSelectionView` — private UIKit class used internally by
//! `UITextField`, `UITextView`, and `UIWebView` for rendering the text
//! selection handles and highlight rects.
//!
//! Apps rarely reference this class directly, but some older iOS games
//! (and third-party text libraries) do instantiate or message it.
//! We provide a minimal stub that is a well-behaved `UIView` subclass.

use crate::frameworks::core_graphics::{CGFloat, CGPoint, CGRect};
use crate::objc::{
    id, msg, msg_class, nil, objc_classes, release, retain,
    ClassExports, HostObject, NSZonePtr,
};

// =========================================================================
// MARK: - Host object
// =========================================================================

struct UITextSelectionViewHostObject {
    /// The colour used to draw the selection highlight.  UIColor* — retained.
    selection_color: id,
    /// Whether selection handles (drag handles) are visible.
    handles_visible: bool,
    /// The selection rects — NSArray* of UITextSelectionRect* (we store NSValue
    /// wrapped CGRects since UITextSelectionRect is also a stub).
    selection_rects: id,
    /// Caret rect — the blink-cursor position.
    caret_rect: CGRect,
    /// Whether the caret is currently visible.
    caret_visible: bool,
}
impl HostObject for UITextSelectionViewHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UITextSelectionView: UIView

// =========================================================================
// MARK: - Allocation
// =========================================================================

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(UITextSelectionViewHostObject {
        selection_color: nil,
        handles_visible: false,
        selection_rects: nil,
        caret_rect: CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: crate::frameworks::core_graphics::CGSize { width: 2.0, height: 16.0 },
        },
        caret_visible: false,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

// =========================================================================
// MARK: - Initializers
// =========================================================================

- (id)init {
    this
}

- (id)initWithFrame:(CGRect)frame {
    let _: () = msg![env; this setFrame:frame];
    this
}

// =========================================================================
// MARK: - Dealloc
// =========================================================================

- (())dealloc {
    let (color, rects) = {
        let h = env.objc.borrow::<UITextSelectionViewHostObject>(this);
        (h.selection_color, h.selection_rects)
    };
    release(env, color);
    release(env, rects);
    env.objc.dealloc_object(this, &mut env.mem)
}

// =========================================================================
// MARK: - Selection colour
// =========================================================================

- (id)selectionColor { // UIColor*
    env.objc.borrow::<UITextSelectionViewHostObject>(this).selection_color
}

- (())setSelectionColor:(id)color { // UIColor*
    let old = env.objc.borrow::<UITextSelectionViewHostObject>(this).selection_color;
    release(env, old);
    retain(env, color);
    env.objc.borrow_mut::<UITextSelectionViewHostObject>(this).selection_color = color;
}

// Tint colour alias — UIKit internally calls setTintColor: on selection views.
- (())setTintColor:(id)color {
    let _: () = msg![env; this setSelectionColor:color];
}

- (id)tintColor {
    msg![env; this selectionColor]
}

// =========================================================================
// MARK: - Selection rects
// =========================================================================

- (id)selectionRects { // NSArray*
    let rects = env.objc.borrow::<UITextSelectionViewHostObject>(this).selection_rects;
    if rects != nil { rects } else { msg_class![env; NSArray array] }
}

- (())setSelectionRects:(id)rects { // NSArray*
    let old = env.objc.borrow::<UITextSelectionViewHostObject>(this).selection_rects;
    release(env, old);
    retain(env, rects);
    env.objc.borrow_mut::<UITextSelectionViewHostObject>(this).selection_rects = rects;
    // Trigger a redraw whenever the selection changes.
    let _: () = msg![env; this setNeedsDisplay];
}

// =========================================================================
// MARK: - Handles
// =========================================================================

- (bool)handlesAreVisible {
    env.objc.borrow::<UITextSelectionViewHostObject>(this).handles_visible
}

- (())setHandlesAreVisible:(bool)visible {
    env.objc.borrow_mut::<UITextSelectionViewHostObject>(this).handles_visible = visible;
}

// iOS 9+ variant.
- (())setGrabberVisible:(bool)visible {
    env.objc.borrow_mut::<UITextSelectionViewHostObject>(this).handles_visible = visible;
}

- (bool)isGrabberVisible {
    env.objc.borrow::<UITextSelectionViewHostObject>(this).handles_visible
}

// =========================================================================
// MARK: - Caret
// =========================================================================

- (CGRect)caretRect {
    env.objc.borrow::<UITextSelectionViewHostObject>(this).caret_rect
}

- (())setCaretRect:(CGRect)rect {
    env.objc.borrow_mut::<UITextSelectionViewHostObject>(this).caret_rect = rect;
    let _: () = msg![env; this setNeedsDisplay];
}

- (bool)isCaretVisible {
    env.objc.borrow::<UITextSelectionViewHostObject>(this).caret_visible
}

- (())setCaretVisible:(bool)visible {
    env.objc.borrow_mut::<UITextSelectionViewHostObject>(this).caret_visible = visible;
    let _: () = msg![env; this setNeedsDisplay];
}

// =========================================================================
// MARK: - Blink control
// =========================================================================

// Apps and UIKit internals call these to start/stop the caret blink
// animation. We simply show/hide the caret since we don't animate.
- (())startCaretBlink {
    log_dbg!("UITextSelectionView startCaretBlink");
    env.objc.borrow_mut::<UITextSelectionViewHostObject>(this).caret_visible = true;
    let _: () = msg![env; this setNeedsDisplay];
}

- (())stopCaretBlink {
    log_dbg!("UITextSelectionView stopCaretBlink");
    env.objc.borrow_mut::<UITextSelectionViewHostObject>(this).caret_visible = false;
    let _: () = msg![env; this setNeedsDisplay];
}

- (())beginSelectionChange {
    log_dbg!("UITextSelectionView beginSelectionChange — no-op");
}

- (())endSelectionChange {
    log_dbg!("UITextSelectionView endSelectionChange — no-op");
    let _: () = msg![env; this setNeedsDisplay];
}

// =========================================================================
// MARK: - Hit testing / touch handling
// =========================================================================

// Selection views generally don't participate in hit testing so that taps
// pass through to the underlying text view.
- (id)hitTest:(CGPoint)point withEvent:(id)_event {
    log_dbg!("UITextSelectionView hitTest:{:?} — returning nil (pass-through)", point);
    nil
}

- (bool)pointInside:(CGPoint)_point withEvent:(id)_event {
    false
}

// =========================================================================
// MARK: - Drawing (no-op — no real text rendering in touchHLE)
// =========================================================================

- (())drawRect:(CGRect)_rect {
    // No-op — touchHLE does not render actual text selection UI.
}

// =========================================================================
// MARK: - Layout
// =========================================================================

- (())layoutSubviews {
    // Nothing to lay out — selection handles would go here in a real impl.
}

// =========================================================================
// MARK: - Visibility helpers
// =========================================================================

- (())show {
    let _: () = msg![env; this setHidden:false];
}

- (())hide {
    let _: () = msg![env; this setHidden:true];
}

- (bool)isShowing {
    let hidden: bool = msg![env; this isHidden];
    !hidden
}

// =========================================================================
// MARK: - Description
// =========================================================================

- (id)description {
    let (caret_visible, handles_visible) = {
        let h = env.objc.borrow::<UITextSelectionViewHostObject>(this);
        (h.caret_visible, h.handles_visible)
    };
    let s = format!(
        "<UITextSelectionView: {:?}; caretVisible={}; handlesVisible={}>",
        this, caret_visible, handles_visible
    );
    let cstr = env.mem.alloc_and_write_cstr(s.as_bytes());
    msg_class![env; NSString stringWithUTF8String:cstr]
}

@end

};
