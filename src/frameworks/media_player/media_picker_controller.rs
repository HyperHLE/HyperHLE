/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `MPMediaPickerController`.

use crate::frameworks::foundation::NSUInteger;
use crate::objc::{id, msg, objc_classes, ClassExports};

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation MPMediaPickerController: UIViewController

- (id)initWithMediaTypes:(NSUInteger)_types {
    msg![env; this init]
}

- (())setDelegate:(bool)delegate {
    log!("TODO: setDelegate:{}", delegate);
}

- (())setAllowsPickingMultipleItems:(bool)items {
    log!("TODO: setAllowsPickingMultipleItems:{}", items);
}

@end

};
