/*
 * MPL 2.0
 */

use crate::objc_classes;
use crate::objc::id;
use crate::frameworks::core_graphics::CGRect;

static CLASSES: &[(&str, crate::objc::ClassTemplate)] = objc_classes! {
    (env, this, _cmd);

    @implementation UITableViewCell : UIView

    - (id)initWithFrame:(CGRect)frame reuseIdentifier:(id)reuseIdentifier {
        let this: id = msg![env; super(this) initWithFrame:frame];
        this
    }

    @end
};
