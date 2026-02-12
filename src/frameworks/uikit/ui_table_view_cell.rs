use crate::{Environment, msg_send, id};
use crate::frameworks::core_graphics::CGRect;
use crate::frameworks::foundation::NSString;
use crate::objc::classes::objc_classes;

pub fn register(env: &mut Environment) {
    objc_classes! {
        (env, this, _cmd);

        @implementation UITableViewCell : UIView
            // Реализация initWithFrame:reuseIdentifier:
            - (id)initWithFrame:(CGRect)frame reuseIdentifier:(id)identifier {
                // Вызываем super initWithFrame:
                let super_obj: id = msg_send(env, (super(this), "initWithFrame:", frame));

                // Здесь можно сохранить identifier в поле, если нужно
                this
            }
        @end
    };
}
