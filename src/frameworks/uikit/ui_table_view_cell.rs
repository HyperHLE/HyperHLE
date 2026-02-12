use crate::{Environment, msg_send, id};
use crate::frameworks::core_graphics::CGRect;
use crate::frameworks::foundation::NSString;
use crate::objc::classes::objc_classes;

pub fn register(env: &mut Environment) {
    // Создаём класс UITableViewCell, наследуемый от UIView
    objc_classes! {
        (env, this, _cmd);

        @implementation UITableViewCell : UIView
            // Реализация initWithFrame:reuseIdentifier:
            - (id)initWithFrame:(CGRect)frame reuseIdentifier:(id)identifier) {
                // Вызываем super initWithFrame:
                let super_obj: id = msg_send(env, (super(this), "initWithFrame:", frame));

                // Тут можно сохранить identifier, если нужно
                this
            }
        @end
    };
}
