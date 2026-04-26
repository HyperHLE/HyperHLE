/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use crate::objc::{id, SEL, ClassExports, ClassTemplate};
use crate::selector; 
use crate::log;
use crate::Environment;
use crate::dyld::host_imp;

pub const CLASSES: ClassExports = &[
    ("NSRegularExpression", ClassTemplate {
        name: "NSRegularExpression",
        superclass: Some("NSObject"),
        class_methods: &[
            // The macro expects: (dummy_token; identifier)
            (selector!(_; alloc), host_imp!(alloc)),
        ],
        instance_methods: &[
            (selector!(_; initWithPattern, options, error), host_imp!(init_with_pattern)),
            (selector!(_; matchesInString, options, range), host_imp!(matches_in_string)),
        ],
    }),
];

extern "C" fn alloc(env: &mut Environment, class: id, _sel: SEL) -> id {
    let instance = env.objc.as_mut().alloc_instance(class);
    log!("NSRegularExpression: Created stub instance {:?}.", instance);
    instance
}

extern "C" fn init_with_pattern(_env: &mut Environment, this: id, _sel: SEL, _pattern: id, _options: u64, _error: id) -> id {
    log!("NSRegularExpression: initWithPattern stub called.");
    this
}

extern "C" fn matches_in_string(_env: &mut Environment, _this: id, _sel: SEL, _string: id, _options: u64, _range: crate::frameworks::foundation::NSRange) -> id {
    log!("NSRegularExpression: matchesInString returning nil.");
    id::null()
}
