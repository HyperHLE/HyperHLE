use crate::objc::{id, SEL, ClassExports, ClassTemplate};
use crate::selector;
use crate::log;
use crate::Environment;

pub const CLASSES: ClassExports = &[
    ("NSRegularExpression", ClassTemplate {
        name: "NSRegularExpression",
        superclass: Some("NSObject"),
        class_methods: &[
            (selector!(_; alloc), alloc as _),
        ],
        instance_methods: &[
            (selector!(_; initWithPattern, options, error), init_with_pattern as _),
            (selector!(_; matchesInString, options, range), matches_in_string as _),
        ],
    }),
];

extern "C" fn alloc(env: &mut Environment, class: id, _sel: SEL) -> id {
    // We use .as_mut().unwrap() to get past the NullableBox
    let instance = env.objc.as_mut().unwrap().alloc_instance(class);
    log!("NSRegularExpression: Created instance {:?}.", instance);
    instance
}

extern "C" fn init_with_pattern(_env: &mut Environment, this: id, _sel: SEL, _pattern: id, _options: u64, _error: id) -> id {
    log!("NSRegularExpression: initWithPattern stubbed.");
    this
}

extern "C" fn matches_in_string(_env: &mut Environment, _this: id, _sel: SEL, _string: id, _options: u64, _loc: u64, _len: u64) -> id {
    log!("NSRegularExpression: matchesInString returning nil.");
    crate::objc::id::null()
}
