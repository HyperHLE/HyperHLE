/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use crate::objc::{Id, Sel, Class, ClassExports, ClassMethod};
use crate::log;

/// This is what foundation.rs needs to see to actually "load" the class.
pub const CLASSES: ClassExports = &[
    ClassMethod {
        class_name: "NSRegularExpression",
        parent_class_name: "NSObject",
        add_methods,
    },
];

fn add_methods(class: &mut Class) {
    // + (id)alloc
    class.add_class_method(sel!(alloc), alloc as _);
    
    // - (id)initWithPattern:options:error:
    class.add_method(sel!(initWithPattern:options:error:), init_with_pattern as _);
    
    // - (id)matchesInString:options:range:
    class.add_method(sel!(matchesInString:options:range:), matches_in_string as _);
}

extern "C" fn alloc(class: &Class, _sel: Sel) -> Id {
    let instance = class.create_instance();
    log!("NSRegularExpression: Created stub instance {:?}.", instance);
    instance
}

extern "C" fn init_with_pattern(this: Id, _sel: Sel, _pattern: Id, _options: u64, _error: Id) -> Id {
    log!("NSRegularExpression: initWithPattern called (Stubbing success).");
    this
}

extern "C" fn matches_in_string(_this: Id, _sel: Sel, _string: Id, _options: u64, _range: [usize; 2]) -> Id {
    log!("NSRegularExpression: matchesInString called. Returning empty array.");
    crate::frameworks::foundation::ns_array::empty_array()
}
