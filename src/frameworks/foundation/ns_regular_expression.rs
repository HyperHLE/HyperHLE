/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use crate::objc::{id, SEL, ClassExports, ClassTemplate};
use crate::selector; 
use crate::log;
use crate::Environment;

pub const CLASSES: ClassExports = &[
    ("NSRegularExpression", ClassTemplate {
        name: "NSRegularExpression",
        superclass: Some("NSObject"),
        class_methods: &[
            // Removed host_imp! for now to see if your build uses a different wrapper
            (selector!(_; alloc), alloc as _),
        ],
        instance_methods: &[
            (selector!(_; initWithPattern, options, error), init_with_pattern as _),
            (selector!(_; matchesInString, options, range), matches_in_string as _),
        ],
    }),
];

extern "C" fn alloc(env: &mut Environment, class: id, _sel: SEL) -> id {
    // Trying the most direct way to create the instance based on common touchHLE patterns
    let instance = crate::objc::alloc_instance(env, class);
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
