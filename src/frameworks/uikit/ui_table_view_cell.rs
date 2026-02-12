/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 */

use crate::objc::{
    id, msg_send, nil,
    objc_classes,
};
use crate::frameworks::uikit::ui_view::CGRect;

objc_classes! {
    (env, this, _cmd);

    @implementation UITableViewCell : UIView

    - (id)initWithFrame:(CGRect)frame reuseIdentifier:(id)_reuse {
        // super.initWithFrame:
        let super_obj: id = msg_send(env, (this, "initWithFrame:", frame));
        super_obj
    }

    - (id)reuseIdentifier {
        nil
    }

    - (())prepareForReuse {
        // intentionally empty
    }

    @end
}
