/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use crate::objc::{Id, Sel, ClassExports, ClassMethod};
use crate::log;
use crate::Environment;

pub const CLASSES: ClassExports = &[
    ClassMethod {
        class_name: "NSRegularExpression",
        parent_class_name: "NSObject",
        add_methods,
    },
];

/// The manual registration style for your specific touchHLE build
fn add_methods(env: &mut Environment, class: Id) {
    crate::objc::add_class_method(env, class, sel!(alloc), alloc as _);
    crate::objc::add_method(env, class, sel!(initWithPattern:options:error:), init_with_pattern as _);
    crate::objc::add_method(env, class, sel!(matchesInString:options:range:), matches_in_string as _);
}

extern "C" fn alloc(env: &mut Environment, class: Id, _sel: Sel) -> Id {
    let instance = crate::objc::alloc_instance(env, class);
    log!("NSRegularExpression: Created stub instance {:?}.", instance);
    instance
}

extern "C" fn init_with_pattern(_env: &mut Environment, this: Id, _sel: Sel, _pattern: Id, _options: u64, _error: Id) -> Id {
    log!("NSRegularExpression: initWithPattern called.");
    this
}

extern "C" fn matches_in_string(env: &mut Environment, _this: Id, _sel: Sel, _string: Id, _options: u64, _range: [usize; 2]) -> Id {
    log!("NSRegularExpression: matchesInString returning empty array.");
    crate::frameworks::foundation::ns_array::empty_array(env)
}
