/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UISearchBar`.

use crate::frameworks::core_graphics::CGRect;
use crate::frameworks::foundation::NSInteger;
use crate::impl_HostObject_with_superclass;
use crate::objc::{id, msg_super, nil, objc_classes, ClassExports, NSZonePtr};

type UISearchBarStyle = NSInteger;
type UITextAutocapitalizationType = NSInteger;
type UITextAutocorrectionType = NSInteger;
type UIKeyboardType = NSInteger;

struct UISearchBarHostObject {
    superclass: super::UIViewHostObject,
    delegate: id,
}
impl_HostObject_with_superclass!(UISearchBarHostObject);
impl Default for UISearchBarHostObject {
    fn default() -> Self {
        UISearchBarHostObject {
            superclass: Default::default(),
            delegate: nil,
        }
    }
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UISearchBar: UIView

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::<UISearchBarHostObject>::default();
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)initWithFrame:(CGRect)frame {
    msg_super![env; this initWithFrame:frame]
}

- (id)initWithCoder:(id)coder {
    msg_super![env; this initWithCoder:coder]
}

- (())dealloc {
    msg_super![env; this dealloc]
}

- (())setDelegate:(id)delegate {
    log_dbg!("UISearchBar setDelegate:{:?}", delegate);
    env.objc.borrow_mut::<UISearchBarHostObject>(this).delegate = delegate;
}
- (id)delegate {
    env.objc.borrow::<UISearchBarHostObject>(this).delegate
}

- (())setPlaceholder:(id)placeholder {
    log!("TODO: UISearchBar setPlaceholder:");
    let _ = placeholder;
}

- (id)placeholder {
    nil
}

- (())setText:(id)text {
    log!("TODO: UISearchBar setText:");
    let _ = text;
}

- (id)text {
    nil
}

- (())setBarStyle:(UISearchBarStyle)style {
    log!("TODO: UISearchBar setBarStyle:{}", style);
}

- (())setShowsSearchResultsButton:(bool)shows {
    log!("TODO: UISearchBar setShowsSearchResultsButton:{}", shows);
}

- (())setShowsCancelButton:(bool)shows {
    log!("TODO: UISearchBar setShowsCancelButton:{}", shows);
}

- (())setShowsCancelButton:(bool)shows animated:(bool)animated {
    log!("TODO: UISearchBar setShowsCancelButton:{} animated:{}", shows, animated);
}

- (())setShowsBookmarkButton:(bool)shows {
    log!("TODO: UISearchBar setShowsBookmarkButton:{}", shows);
}

- (())setShowsScopeBar:(bool)shows {
    log!("TODO: UISearchBar setShowsScopeBar:{}", shows);
}

- (())setScopeButtonTitles:(id)titles {
    log!("TODO: UISearchBar setScopeButtonTitles:");
    let _ = titles;
}

- (())setSelectedScopeButtonIndex:(NSInteger)index {
    log!("TODO: UISearchBar setSelectedScopeButtonIndex:{}", index);
}

- (())setAutocapitalizationType:(UITextAutocapitalizationType)type_ {
    log!("TODO: UISearchBar setAutocapitalizationType:{}", type_);
}

- (())setAutocorrectionType:(UITextAutocorrectionType)type_ {
    log!("TODO: UISearchBar setAutocorrectionType:{}", type_);
}

- (())setKeyboardType:(UIKeyboardType)type_ {
    log!("TODO: UISearchBar setKeyboardType:{}", type_);
}

- (())setTintColor:(id)color {
    log!("TODO: UISearchBar setTintColor:");
    let _ = color;
}

- (())setBackgroundImage:(id)image {
    log!("TODO: UISearchBar setBackgroundImage:");
    let _ = image;
}

- (bool)becomeFirstResponder {
    log_dbg!("UISearchBar becomeFirstResponder (stub)");
    false
}

- (bool)resignFirstResponder {
    log_dbg!("UISearchBar resignFirstResponder (stub)");
    true
}

@end

};
