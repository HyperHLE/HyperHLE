/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use crate::objc::{id, SEL, ClassExports, ClassTemplate};
use crate::selector; // Explicitly imported as requested
use crate::log;
use crate::Environment;
use crate::dyld::host_imp; // Helper for the method casting error

pub const CLASSES: ClassExports = &[
    ("NSRegularExpression", ClassTemplate {
        name: "NSRegularExpression",
        superclass: Some("NSObject"), // Wrap in Some() to fix mismatched types
        class_methods: &[
            (selector!("alloc"), host_imp!(alloc)),
        ],
        instance_methods: &[
            (selector!("initWithPattern:options:error:"), host_imp!(init_with_pattern)),
            (selector!("matchesInString:options:range:"), host_imp!(matches_in_string)),
        ],
    }),
];

extern "C" fn alloc(env: &mut Environment, class: id, _sel: SEL) -> id {
    // In your build, as_mut() returns the engine directly, no unwrap needed
    let instance = env.objc.as_mut().alloc_instance(class);
    log!("NSRegularExpression: Created stub instance {:?}.", instance);
    instance
}

extern "C" fn init_with_pattern(_env: &mut Environment, this: id, _sel: SEL, _pattern: id, _options: u64, _error: id) -> id {
    log!("NSRegularExpression: initWithPattern called.");
    this
}

extern "C" fn matches_in_string(_env: &mut Environment, _this: id, _sel: SEL, _string: id, _options: u64, _range: crate::frameworks::foundation::NSRange) -> id {
    log!("NSRegularExpression: matchesInString returning nil.");
    id::null()
}
