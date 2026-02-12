use crate::Environment;
use crate::objc::{id, objc_classes};
use crate::frameworks::core_graphics::CGRect;
use crate::msg;

pub fn register(env: &mut Environment) {
    // Вызываем макрос, который напрямую регистрирует классы в env
    objc_classes! {
        (env, this, _cmd);

        @implementation UITableViewCell : UIView
            - (id)initWithFrame:(CGRect)frame reuseIdentifier:(id)identifier {
                let this: id = msg![env; super(this) "initWithFrame:reuseIdentifier:" frame identifier];
                this
            }
        @end
    };
}
