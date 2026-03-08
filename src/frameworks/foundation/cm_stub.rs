use crate::objc::macros::*;
use crate::objc::types::*;

pub const CLASSES: ClassExports = objc_classes! {
    (env, this, _cmd);
    
    @implementation CMMotionManager: NSObject
    - (bool)isGyroAvailable {
        false
    }
    @end
};
