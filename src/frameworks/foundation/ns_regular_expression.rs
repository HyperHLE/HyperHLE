use crate::objc::{id, SEL, ClassExports, objc_classes, NSZonePtr};
use crate::log;
use crate::Environment;

pub const CLASSES: ClassExports = objc_classes! {
    (env, this, _cmd);

    @implementation NSRegularExpression: NSObject

        + (id)allocWithZone:(NSZonePtr)_zone {
        // Use as_deref_mut() to reach through the NullableBox
        let instance = env.objc.as_deref_mut().unwrap().alloc_instance(this, &mut env.mem);
        log!("NSRegularExpression: Created instance {:?}.", instance);
        instance
        }
    
    - (id)initWithPattern:(id)_pattern options:(u64)_options error:(id)_error {
        log!("NSRegularExpression: initWithPattern stubbed.");
        this
    }

    - (id)matchesInString:(id)_string options:(u64)_options range:(crate::frameworks::foundation::NSRange)_range {
        log!("NSRegularExpression: matchesInString returning nil.");
        crate::objc::nil
    }

    @end
};
