/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UIAlertView`.

use crate::frameworks::foundation::{ns_string, NSUInteger};
use crate::objc::{id, msg, msg_super, nil, objc_classes, ClassExports};
use std::borrow::Cow;

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UIAlertView: UIView
- (id)initWithTitle:(id)title
                      message:(id)message
                     delegate:(id)delegate
            cancelButtonTitle:(id)cancelButtonTitle
            otherButtonTitles:(id)otherButtonTitles {

    log!("TODO: [(UIAlertView*){:?} initWithTitle:{:?} message:{:?} delegate:{:?} cancelButtonTitle:{:?} otherButtonTitles:{:?}]", this, title, message, delegate, cancelButtonTitle, otherButtonTitles);

    let msg = if message == nil { Cow::from("(nil)") } else { ns_string::to_rust_string(env, message) };
    let title = if title == nil { Cow::from("(nil)") } else { ns_string::to_rust_string(env, title) };
    log!("UIAlertView: title: {:?}, message: {:?}", title, msg);

    msg_super![env; this init]
}

- (id)isVisible {
  nil
}

- (id)addButtonWithTitle:(NSUInteger)title {
    msg![env; this init]
}

- (())show {
    log!("TODO: [(UIAlertView*){:?} show]", this);
}

- (())setCancelButtonIndex:(bool)button {
    log!("TODO: setCancelButtonIndex:{}", button);
}

- (())setDelegate:(bool)message {
    log!("TODO: setDelegate:{}", message);
}

- (())setTitle:(bool)title {
    log!("TODO: setTitle:{}", title);
}

- (())setMessage:(bool)title {
    log!("TODO: setMessage:{}", title);
}

@end

};
