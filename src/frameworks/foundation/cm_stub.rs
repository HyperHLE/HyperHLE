use crate::objc::types::*;

pub const CLASSES: crate::objc::ClassExports = objc_classes! {
    (env, this, _cmd);
    
    @implementation CMMotionManager: NSObject
    - (bool)isGyroAvailable {
        false
    }
    @end
};
