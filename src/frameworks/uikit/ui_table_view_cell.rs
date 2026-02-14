/*
 * MPL 2.0
 */

use crate::objc_classes; // это публичный re-export макроса
use crate::{Environment, id, msg};
use crate::objc::id; // тип id

pub fn register(env: &mut Environment) {
    objc_classes! {
        (env, this, _cmd);

        @implementation UITableViewCell : UIView
            - (id)initWithFrame:(CGRect)frame reuseIdentifier:(id)identifier {
                // Получаем указатель на суперкласс
                let super_cls: id = msg![env; this superclass];

                // Вызываем инициализатор суперкласса
                let this: id = msg![env; super_cls initWithFrame:frame];

                this
            }
        @end
    }
}
