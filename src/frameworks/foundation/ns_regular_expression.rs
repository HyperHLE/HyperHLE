/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use crate::objc::{id, SEL, ClassExports};
use crate::selector; // Importing it directly as suggested by the compiler
use crate::log;
use crate::Environment;

pub const CLASSES: ClassExports = &[
    crate::objc::Class {
        class_name: "NSRegularExpression",
        parent_class_name: "NSObject",
        add_methods,
    },
];

fn add_methods(env: &mut Environment, class: id) {
    env.objc.add_class_method(class, selector!("alloc"), alloc as _);
    env.objc.add_method(class, selector!("initWithPattern:options:error:"), init_with_pattern as _);
    env.objc.add_method(class, selector!("matchesInString:options:range:"), matches_in_string as _);
}

extern "C" fn alloc(env: &mut Environment, class: id, _sel: SEL) -> id {
    let instance = env.objc.alloc_instance(class);
    log!("NSRegularExpression: Created stub instance {:?}.", instance);
    instance
}

extern "C" fn init_with_pattern(_env: &mut Environment, this: id, _sel: SEL, _pattern: id, _options: u64, _error: id) -> id {
    log!("NSRegularExpression: initWithPattern called.");
    this
}

extern "C" fn matches_in_string(_env: &mut Environment, _this: id, _sel: SEL, _string: id, _options: u64, _range: [u32; 2]) -> id {
    log!("NSRegularExpression: matchesInString returning nil.");
    // Returning 0 (nil) is the universal "safe" way to return an empty object/array 
    // in Objective-C if the specific helper isn't found.
    0
}
