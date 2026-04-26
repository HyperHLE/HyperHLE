use crate::objc::{id, SEL};
use crate::selector;
use crate::log;
use crate::Environment;

pub const CLASSES: &[(&str, fn(&mut Environment, id))] = &[
    ("NSRegularExpression", register_class),
];

fn register_class(env: &mut Environment, class: id) {
    env.objc.add_class_method(class, selector!(env; alloc), alloc as _);
    env.objc.add_method(class, selector!(env; initWithPattern, options, error), init_with_pattern as _);
    env.objc.add_method(class, selector!(env; matchesInString, options, range), matches_in_string as _);
}

extern "C" fn alloc(env: &mut Environment, class: id, _sel: SEL) -> id {
    let instance = env.objc.alloc_instance(class);
    log!("NSRegularExpression: Created instance {:?}.", instance);
    instance
}

extern "C" fn init_with_pattern(_env: &mut Environment, this: id, _sel: SEL, _pattern: id, _options: u64, _error: id) -> id {
    log!("NSRegularExpression: initWithPattern stubbed.");
    this
}

// Changed NSRange to _loc: u64, _len: u64 to match how your build handles ranges
extern "C" fn matches_in_string(_env: &mut Environment, _this: id, _sel: SEL, _string: id, _options: u64, _loc: u64, _len: u64) -> id {
    log!("NSRegularExpression: matchesInString returning nil.");
    crate::objc::id::null()
}
