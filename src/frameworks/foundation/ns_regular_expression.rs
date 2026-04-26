/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use crate::objc::{Id, Sel, Class};
use crate::log;

/// Registers the NSRegularExpression class methods.
pub fn add_methods(class: &mut Class) {
    // + (id)alloc
    class.add_class_method(sel!(alloc), alloc as _);
    
    // - (id)initWithPattern:(id)pattern options:(unsigned long long)options error:(id *)error
    class.add_method(sel!(initWithPattern:options:error:), init_with_pattern as _);
    
    // - (id)matchesInString:(id)string options:(unsigned long long)options range:(NSRange)range
    class.add_method(sel!(matchesInString:options:range:), matches_in_string as _);
}

extern "C" fn alloc(class: &Class, _sel: Sel) -> Id {
    let instance = class.create_instance();
    log!("NSRegularExpression: Created stub instance {:?}.", instance);
    instance
}

extern "C" fn init_with_pattern(this: Id, _sel: Sel, _pattern: Id, _options: u64, _error: Id) -> Id {
    log!("NSRegularExpression: initWithPattern called (Stubbing success).");
    // In Objective-C, init methods must return 'self' (this)
    this
}

extern "C" fn matches_in_string(_this: Id, _sel: Sel, _string: Id, _options: u64, _range: [usize; 2]) -> Id {
    log!("NSRegularExpression: matchesInString called. Returning empty array.");
    // We return an empty NSArray so the game doesn't crash when it tries to count the matches.
    crate::frameworks::foundation::ns_array::empty_array()
}
