/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use crate::objc::{id as Id, SEL as Sel, ClassExports}; // Matches your project's naming
use crate::log;
use crate::frameworks::foundation::NSRange;

// If your project uses the manual "add_methods" style, keep this.
// If it uses the macro style, this file needs to be listed in foundation/mod.rs
pub fn add_methods(class: &mut crate::objc::Class) {
    class.add_class_method(sel!(alloc), alloc as _);
    class.add_method(sel!(initWithPattern:options:error:), init_with_pattern as _);
    class.add_method(sel!(matchesInString:options:range:), matches_in_string as _);
}

extern "C" fn alloc(class: &crate::objc::Class, _sel: Sel) -> Id {
    let instance = class.create_instance();
    log!("NSRegularExpression: Created stub instance {:?}.", instance);
    instance
}

extern "C" fn init_with_pattern(this: Id, _sel: Sel, _pattern: Id, _options: u64, _error: Id) -> Id {
    log!("NSRegularExpression: initWithPattern called.");
    this
}

extern "C" fn matches_in_string(_this: Id, _sel: Sel, _string: Id, _options: u64, _range: NSRange) -> Id {
    log!("NSRegularExpression: matchesInString called. Returning empty array.");
    // This assumes ns_array has a helper. If this fails, use crate::objc::nil
    crate::objc::nil 
}
