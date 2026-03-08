use crate::objc_classes;
use crate::objc::id; // Импортируем тип 'id' напрямую

pub const CLASSES: crate::objc::ClassExports = objc_classes! {
    (env, this, _cmd);
    
    @implementation CMMotionManager: NSObject
    
    - (bool)isGyroAvailable {
        false
    }

    - (u32)hash {
        0
    }

    - (bool)isEqual:(id)_other {
        // Сравниваем объект сам с собой
        this == _other
    }

    - (id)init {
        this
    }
    @end
};
