/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 */

use crate::objc::{
    id, msg_send, nil,
    objc_classes, ClassExports,
};
use crate::frameworks::core_graphics::CGRect;
use crate::msg;

pub const CLASSES: ClassExports = objc_classes! {
    (env, this, _cmd);

    @implementation UITableViewCell : UIView

    - (id)initWithFrame:(CGRect)frame reuseIdentifier:(id)_reuseIdentifier {
        let super_obj: id = msg![env; this initWithFrame:frame];
        super_obj
    }

    - (id)reuseIdentifier {
        nil
    }

    - (())prepareForReuse {
        ()
    }

    - (id)initWithFrame:(CGRect)frame reuseIdentifier:(id)identifier

};
