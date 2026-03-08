use crate::objc_classes;

pub const CLASSES: crate::objc::ClassExports = objc_classes! {
    (env, this, _cmd);
    
    @implementation CMMotionManager: NSObject
    
    // Тот метод, который нужен игре
    - (bool)isGyroAvailable {
        false
    }

    // Методы для стабильности эмулятора (чтобы NSSet не падал)
    - (u32)hash {
        0
    }

    - (bool)isEqual:(id)_other {
        // Мы просто говорим, что объекты равны сами себе
        true 
    }

    - (id)init {
        this
    }
    @end
};
