use crate::objc::{id, SEL, ClassExports, objc_classes, NSZonePtr};
use crate::log;
use crate::Environment;

pub const CLASSES: ClassExports = objc_classes! {
    (env, this, _cmd);

    @implementation NSRegularExpression: NSObject

        + (id)allocWithZone:(NSZonePtr)_zone {
        // Use (*env.objc) to explicitly trigger DerefMut and find alloc_instance
        let instance = (*env.objc).alloc_instance(this, &mut env.mem);
        log!("NSRegularExpression: Created instance {:?}.", instance);
        instance
        }
    
    - (id)initWithPattern:(id)_pattern options:(u64)_options error:(id)_error {
        // CRITICAL: We must notify the engine that this instance is being initialized
        (*env.objc).init_instance(this);
        log!("NSRegularExpression: initWithPattern initialized {:?}.", this);
        this
    }

    - (id)matchesInString:(id)_string options:(u64)_options range:(crate::frameworks::foundation::NSRange)_range {
        log!("NSRegularExpression: matchesInString returning nil.");
        crate::objc::nil
    }

    @end
};
