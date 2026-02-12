/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 */

use crate::frameworks::foundation::ns_object::NSObject;
use crate::frameworks::uikit::ui_view::UIView;
use crate::objc::{
    autorelease, id, msg, nil, objc_classes, ClassExports, MutVoidPtr,
};

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UITableViewCell : UIView

- (id)initWithFrame:(crate::frameworks::core_graphics::cg_geometry::CGRect)_frame
  reuseIdentifier:(id)_reuseIdentifier
{
    // iOS 2.x behavior:
    // UITableViewCell existed but was very minimal

    let this: id = msg![env; this init];
    this
}

- (id)reuseIdentifier {
    nil
}

@end

};
