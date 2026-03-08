/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `NSNull`.

use crate::objc::{id, objc_classes, ClassExports, TrivialHostObject};

#[derive(Default)]
pub struct State {
    null: Option<id>,
}

pub const CLASSES: ClassExports = objc_classes! {
    (env, this, _cmd);
    
    @implementation NSNull: NSObject
    - (id)retain {
        this
    }
    - (())release {}
    - (id)autorelease {
        this
    }
    @end
};
