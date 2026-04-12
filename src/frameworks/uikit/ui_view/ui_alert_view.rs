/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UIAlertView`.

use crate::frameworks::foundation::{ns_string, NSInteger, NSUInteger};
use crate::objc::{
    id, msg, msg_class, nil, objc_classes, release, retain, ClassExports, HostObject, NSZonePtr,
};

// UIAlertViewStyle constants
pub type UIAlertViewStyle = NSInteger;
pub const UIAlertViewStyleDefault:             UIAlertViewStyle = 0;
pub const UIAlertViewStyleSecureTextInput:     UIAlertViewStyle = 1;
pub const UIAlertViewStylePlainTextInput:      UIAlertViewStyle = 2;
pub const UIAlertViewStyleLoginAndPasswordInput: UIAlertViewStyle = 3;

struct UIAlertViewHostObject {
    title: id,
    message: id,
    delegate: id,
    /// NSMutableArray* of NSString* button titles
    button_titles: id,
    cancel_button_index: NSInteger,
    visible: bool,
    alert_view_style: UIAlertViewStyle,
    tag: NSInteger,
}
impl HostObject for UIAlertViewHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UIAlertView: UIView

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(UIAlertViewHostObject {
        title: nil,
        message: nil,
        delegate: nil,
        button_titles: nil,
        cancel_button_index: -1,
        visible: false,
        alert_view_style: UIAlertViewStyleDefault,
        tag: 0,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

// =========================================================================
// MARK: - Designated initializer
// =========================================================================

- (id)initWithTitle:(id)title
            message:(id)message
           delegate:(id)delegate
  cancelButtonTitle:(id)cancel_title
  otherButtonTitles:(id)other_titles {
    let buttons: id = msg_class![env; NSMutableArray new];
    env.objc.borrow_mut::<UIAlertViewHostObject>(this).button_titles = buttons;

    retain(env, title);
    retain(env, message);
    retain(env, delegate);
    {
        let host = env.objc.borrow_mut::<UIAlertViewHostObject>(this);
        host.title    = title;
        host.message  = message;
        host.delegate = delegate;
    }

    // Add cancel button first (index 0 when present).
    if cancel_title != nil {
        let idx: NSUInteger = msg![env; buttons count];
        let _: () = msg![env; buttons addObject:cancel_title];
        env.objc.borrow_mut::<UIAlertViewHostObject>(this).cancel_button_index =
            idx as NSInteger;
    }

    // Add first "other" button (varargs not supported — one arg only).
    if other_titles != nil {
        let _: () = msg![env; buttons addObject:other_titles];
    }

    let title_str = if title != nil {
        ns_string::to_rust_string(env, title).into_owned()
    } else { "(nil)".into() };
    let msg_str = if message != nil {
        ns_string::to_rust_string(env, message).into_owned()
    } else { "(nil)".into() };
    log!("UIAlertView init title={:?} message={:?}", title_str, msg_str);

    this
}

// =========================================================================
// MARK: - Dealloc
// =========================================================================

- (())dealloc {
    let host = env.objc.borrow::<UIAlertViewHostObject>(this);
    let (title, message, delegate, buttons) =
        (host.title, host.message, host.delegate, host.button_titles);
    release(env, title);
    release(env, message);
    release(env, delegate);
    release(env, buttons);
    env.objc.dealloc_object(this, &mut env.mem)
}

// =========================================================================
// MARK: - Title / message
// =========================================================================

- (id)title { // NSString*
    env.objc.borrow::<UIAlertViewHostObject>(this).title
}

- (())setTitle:(id)title {
    let old = env.objc.borrow::<UIAlertViewHostObject>(this).title;
    release(env, old);
    retain(env, title);
    env.objc.borrow_mut::<UIAlertViewHostObject>(this).title = title;
}

- (id)message { // NSString*
    env.objc.borrow::<UIAlertViewHostObject>(this).message
}

- (())setMessage:(id)message {
    let old = env.objc.borrow::<UIAlertViewHostObject>(this).message;
    release(env, old);
    retain(env, message);
    env.objc.borrow_mut::<UIAlertViewHostObject>(this).message = message;
}

// =========================================================================
// MARK: - Delegate
// =========================================================================

- (id)delegate {
    env.objc.borrow::<UIAlertViewHostObject>(this).delegate
}

- (())setDelegate:(id)delegate {
    let old = env.objc.borrow::<UIAlertViewHostObject>(this).delegate;
    release(env, old);
    retain(env, delegate);
    env.objc.borrow_mut::<UIAlertViewHostObject>(this).delegate = delegate;
}

// =========================================================================
// MARK: - Tag
// =========================================================================

- (NSInteger)tag {
    env.objc.borrow::<UIAlertViewHostObject>(this).tag
}

- (())setTag:(NSInteger)tag {
    env.objc.borrow_mut::<UIAlertViewHostObject>(this).tag = tag;
}

// =========================================================================
// MARK: - Style
// =========================================================================

- (UIAlertViewStyle)alertViewStyle {
    env.objc.borrow::<UIAlertViewHostObject>(this).alert_view_style
}

- (())setAlertViewStyle:(UIAlertViewStyle)style {
    env.objc.borrow_mut::<UIAlertViewHostObject>(this).alert_view_style = style;
}

// =========================================================================
// MARK: - Buttons
// =========================================================================

- (NSInteger)addButtonWithTitle:(id)title { // NSString* -> button index
    let buttons = env.objc.borrow::<UIAlertViewHostObject>(this).button_titles;
    let idx: NSUInteger = msg![env; buttons count];
    let _: () = msg![env; buttons addObject:title];
    log_dbg!("UIAlertView addButtonWithTitle:{:?} => {}",
        if title != nil { ns_string::to_rust_string(env, title).into_owned() } else { "(nil)".into() },
        idx);
    idx as NSInteger
}

- (NSUInteger)numberOfButtons {
    let buttons = env.objc.borrow::<UIAlertViewHostObject>(this).button_titles;
    msg![env; buttons count]
}

- (id)buttonTitleAtIndex:(NSInteger)index { // NSString*
    let buttons = env.objc.borrow::<UIAlertViewHostObject>(this).button_titles;
    let count: NSUInteger = msg![env; buttons count];
    if index < 0 || index as NSUInteger >= count {
        return nil;
    }
    msg![env; buttons objectAtIndex:(index as NSUInteger)]
}

- (NSInteger)cancelButtonIndex {
    env.objc.borrow::<UIAlertViewHostObject>(this).cancel_button_index
}

- (())setCancelButtonIndex:(NSInteger)index {
    env.objc.borrow_mut::<UIAlertViewHostObject>(this).cancel_button_index = index;
}

- (NSInteger)firstOtherButtonIndex {
    let host = env.objc.borrow::<UIAlertViewHostObject>(this);
    let buttons = host.button_titles;
    let cancel = host.cancel_button_index;
    let count: NSUInteger = msg![env; buttons count];
    for i in 0..count {
        if i as NSInteger != cancel {
            return i as NSInteger;
        }
    }
    -1
}

// =========================================================================
// MARK: - Text fields (stub — no real input UI)
// =========================================================================

- (id)textFieldAtIndex:(NSInteger)_index { // UITextField*
    log_dbg!("UIAlertView textFieldAtIndex: — returning nil (no input UI)");
    nil
}

// =========================================================================
// MARK: - Visibility
// =========================================================================

- (bool)isVisible {
    env.objc.borrow::<UIAlertViewHostObject>(this).visible
}

// =========================================================================
// MARK: - Show / dismiss
// touchHLE has no alert UI. We immediately fire the cancel callback so
// the app's delegate can clean up.
// =========================================================================

- (())show {
    log!("UIAlertView show — no UI, dismissing immediately via cancel button");
    env.objc.borrow_mut::<UIAlertViewHostObject>(this).visible = true;
    let cancel = env.objc.borrow::<UIAlertViewHostObject>(this).cancel_button_index;
    let dismiss_index = if cancel >= 0 { cancel } else { 0 };
    let _: () = msg![env; this dismissWithClickedButtonIndex:dismiss_index animated:false];
}

- (())dismissWithClickedButtonIndex:(NSInteger)button_index
                           animated:(bool)_animated {
    env.objc.borrow_mut::<UIAlertViewHostObject>(this).visible = false;

    let delegate = env.objc.borrow::<UIAlertViewHostObject>(this).delegate;
    if delegate == nil { return; }

    // alertView:clickedButtonAtIndex:
    if let Some(sel_clicked) = env.objc
        .lookup_selector("alertView:clickedButtonAtIndex:") {
        let responds: bool = msg![env; delegate respondsToSelector:sel_clicked];
        if responds {
            let _: () = msg![env; delegate alertView:this
                                 clickedButtonAtIndex:button_index];
        }
    }

    // alertView:willDismissWithButtonIndex:
    if let Some(sel_will) = env.objc
        .lookup_selector("alertView:willDismissWithButtonIndex:") {
        let responds: bool = msg![env; delegate respondsToSelector:sel_will];
        if responds {
            let _: () = msg![env; delegate alertView:this
                            willDismissWithButtonIndex:button_index];
        }
    }

    // alertView:didDismissWithButtonIndex:
    if let Some(sel_did) = env.objc
        .lookup_selector("alertView:didDismissWithButtonIndex:") {
        let responds: bool = msg![env; delegate respondsToSelector:sel_did];
        if responds {
            let _: () = msg![env; delegate alertView:this
                             didDismissWithButtonIndex:button_index];
        }
    }
}

// =========================================================================
// MARK: - Description
// =========================================================================

- (id)description {
    let (title, visible) = {
        let h = env.objc.borrow::<UIAlertViewHostObject>(this);
        (h.title, h.visible)
    };
    let title_str = if title != nil {
        ns_string::to_rust_string(env, title).into_owned()
    } else { "(nil)".into() };
    let s = format!(
        "<UIAlertView: {:?}; title={:?}; visible={}>",
        this, title_str, visible
    );
    let cstr = env.mem.alloc_and_write_cstr(s.as_bytes());
    msg_class![env; NSString stringWithUTF8String:cstr]
}

@end

};
