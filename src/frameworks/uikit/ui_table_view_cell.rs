/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 */

use crate::objc::{id};
use crate::{msg};

objc_classes! {

@class UITableViewCell : UIView

@implementation UITableViewCell

- (id)initWithFrame:(CGRect)frame reuseIdentifier:(id)_reuseIdentifier {
    let this: id = msg![env; this initWithFrame:frame];
    this
}

@end

}
