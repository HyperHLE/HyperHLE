use crate::objc::{id, SEL, ClassExports, objc_classes, NSZonePtr};
use crate::log;
use crate::Environment;

pub const CLASSES: ClassExports = objc_classes! {
    (env, this, _cmd);

    @implementation NSRegularExpression: NSObject

            + (id)allocWithZone:(NSZonePtr)_zone {
        // Use alloc_object with a dummy host object ()
        let instance = (*env.objc).alloc_object(this, (), &mut env.mem);
        log!("NSRegularExpression: Created instance {:?}.", instance);
        instance
    }

    - (id)initWithPattern:(id)_pattern options:(u64)_options error:(id)_error {
        // If init_instance doesn't exist, it might be init_object or 
        // simply not required if alloc_object handles registration.
        // Try removing the init_instance call if it keeps failing.
        log!("NSRegularExpression: initWithPattern initialized {:?}.", this);
        this
    }
    
    - (id)matchesInString:(id)_string options:(u64)_options range:(crate::frameworks::foundation::NSRange)_range {
        log!("NSRegularExpression: matchesInString returning nil.");
        crate::objc::nil
    }

    @end
};
