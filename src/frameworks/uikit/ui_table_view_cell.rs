/*
 * MPL 2.0
 */

use crate::objc_classes; // это публичный re-export макроса
use crate::{Environment, id, msg};
use crate::objc::id; // тип id

objc_classes! {
    (env, this, _cmd);

    @implementation UITableViewCell : UIView

    - (id)initWithFrame:(CGRect)frame reuseIdentifier:(id)reuseIdentifier {
        this
    }

    @end
}
