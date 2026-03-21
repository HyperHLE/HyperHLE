/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UISearchBar`.

use crate::frameworks::core_graphics::CGRect;
use crate::objc::{id, msg, msg_super, nil, objc_classes, ClassExports, HostObject, NSZonePtr};

pub struct UISearchBarHostObject {
    pub delegate: id,
    pub text: id,
    pub placeholder: id,
}
impl HostObject for UISearchBarHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UISearchBar: UIView

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(UISearchBarHostObject {
        delegate: nil,
        text: nil,
        placeholder: nil,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)initWithFrame:(CGRect)frame {
    msg_super![env; this initWithFrame:frame]
}

- (id)delegate {
    env.objc.borrow::<UISearchBarHostObject>(this).delegate
}

- (())setDelegate:(id)delegate {
    env.objc.borrow_mut::<UISearchBarHostObject>(this).delegate = delegate;
}

- (id)text {
    env.objc.borrow::<UISearchBarHostObject>(this).text
}

- (())setText:(id)text {
    env.objc.borrow_mut::<UISearchBarHostObject>(this).text = text;
}

- (id)placeholder {
    env.objc.borrow::<UISearchBarHostObject>(this).placeholder
}

- (())setPlaceholder:(id)placeholder {
    env.objc.borrow_mut::<UISearchBarHostObject>(this).placeholder = placeholder;
}

- (())setBarStyle:(i32)_style {
}

- (())setShowsCancelButton:(bool)_shows {
}

- (())setShowsCancelButton:(bool)_shows animated:(bool)_animated {
}

- (())setAutocorrectionType:(i32)_type {
}

- (())setAutocapitalizationType:(i32)_type {
}

- (())setKeyboardType:(i32)_type {
}

- (())setTintColor:(id)_color {
}

- (bool)becomeFirstResponder {
    false
}

- (bool)resignFirstResponder {
    true
}

@end

};
