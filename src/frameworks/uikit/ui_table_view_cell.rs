/*
 * MPL 2.0
 */

use crate::Environment;
use crate::objc::id;
use crate::msg;
use crate::objc_classes;
use crate::frameworks::core_graphics::CGRect;

objc_classes! {
    (env, this, _cmd);

    @implementation UITableViewCell : UIView

        - (id)initWithFrame:(CGRect)frame reuseIdentifier:(id)identifier {
            let this: id = msg![env; super this initWithFrame:frame];
            this
        }

    @end
}
