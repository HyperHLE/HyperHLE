/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 */

use crate::objc::{id, objc_classes};
use crate::frameworks::core_graphics::CGRect;
use crate::msg;

objc_classes! {
    (env, this, _cmd);

    @implementation UITableViewCell : UIView

    - (id)initWithFrame:(CGRect)frame reuseIdentifier:(id)identifier {
        let obj: id = msg![env; this initWithFrame:frame];
        obj
    }

    @end
}
