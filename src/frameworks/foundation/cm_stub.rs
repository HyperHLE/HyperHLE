use crate::objc_classes;
use crate::objc::id;

pub const CLASSES: crate::objc::ClassExports = objc_classes! {
    (env, this, _cmd);
    
    @implementation CMMotionManager: NSObject
    
    // Главное для игры
    - (bool)isGyroAvailable {
        false
    }

    // Обязательно для NSSet и коллекций
    - (u32)hash {
        0 // <--- Возвращаем просто 0, никаких кастов!
    }

    - (bool)isEqual:(id)_other {
        this == _other
    }

    // Методы управления памятью (часто причина паник в touchHLE)
    - (id)retain {
        this
    }

    - (())release {}

    - (id)autorelease {
        this
    }

    - (id)init {
        this
    }

    - (id)description {
        this
    }
    
    @end
};
