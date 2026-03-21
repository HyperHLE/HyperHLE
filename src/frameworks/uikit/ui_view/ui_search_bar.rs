/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UISearchBar`.

use crate::frameworks::core_graphics::CGRect;
use crate::objc::{id, msg_super, nil, objc_classes, ClassExports};

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UISearchBar: UIView

- (id)initWithFrame:(CGRect)frame {
    msg_super![env; this initWithFrame:frame]
}

- (id)delegate { nil }
- (())setDelegate:(id)_delegate {}
- (id)text { nil }
- (())setText:(id)_text {}
- (id)placeholder { nil }
- (())setPlaceholder:(id)_placeholder {}
- (())setBarStyle:(i32)_style {}
- (())setShowsCancelButton:(bool)_shows {}
- (())setShowsCancelButton:(bool)_shows animated:(bool)_animated {}
- (())setAutocorrectionType:(i32)_type {}
- (())setAutocapitalizationType:(i32)_type {}
- (())setKeyboardType:(i32)_type {}
- (())setTintColor:(id)_color {}
- (bool)becomeFirstResponder { false }
- (bool)resignFirstResponder { true }

@end

};
