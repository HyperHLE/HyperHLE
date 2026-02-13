/*
 * MPL 2.0
 */

use crate::{Environment};
use crate::objc::id; // тип id
use crate::msg;       // макрос msg! нужен, если будут селекторы

objc_classes! {
    (env, this, _cmd);

    @implementation UITableViewCell : UIView

    // Инициализация для touchHLE
    - (id)initWithFrame:(CGRect)frame reuseIdentifier:(id)reuseIdentifier {
        this
    }

    @end
}
