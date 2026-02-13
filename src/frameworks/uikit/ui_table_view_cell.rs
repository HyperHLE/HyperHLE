/*
 * MPL 2.0
 */

use crate::{Environment, objc_classes};
use crate::objc::id;
use crate::frameworks::core_graphics::CGRect;

objc_classes! {
    (env, this, _cmd);

    @implementation UITableViewCell : UIView

    - (id)initWithFrame:(CGRect)frame reuseIdentifier:(id)reuseIdentifier {
        // вызываем super initWithFrame:
        let this: id = msg![env; super(this) initWithFrame:frame];
        this
    }

    @end
}
