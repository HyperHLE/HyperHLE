/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UISlider`.

use crate::frameworks::core_graphics::CGRect;
use crate::frameworks::uikit::ui_view::NSInteger;
use crate::objc::{id, msg_super, nil, objc_classes, ClassExports};

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UISlider: UIControl

- (id)initWithFrame:(CGRect)frame {
    log!("[(UISlider*){:?} initWithFrame:{:?}] TODO: Implement UISlider. The control won't be rendered.", this, frame);
    msg_super![env; this initWithFrame:frame]
}

// NSCoding implementation
- (id)initWithCoder:(id)coder {
    log!("[(UISlider*){:?} initWithCoder:{:?}] TODO: Implement UISlider. The control won't be rendered.", this, coder);
    msg_super![env; this initWithCoder:coder]
}
- (())setContinuous:(bool)continuous {
    log!("TODO: setContinuous:{}", continuous);
}

- (())setMinimumValue:(bool)minimum {
    log!("TODO: setMinimumValue:{}", minimum);
}

- (())setMaximumValue:(bool)maximum {
    log!("TODO: setMaximumValue:{}", maximum);
}

- (())setValue:(bool)value {
    log!("TODO: setValue:{}", value);
}

- (())setMinimumValueImage:(bool)image {
    log!("TODO: setMinimumValueImage:{}", image);
}

- (())setMaximumValueImage:(bool)image {
    log!("TODO: setMaximumValueImage:{}", image);
}

- (())setThumbImage:(NSInteger)_image forState:(bool)_state {
    // TODO
}

- (())setMinimumTrackImage:(NSInteger)_image forState:(bool)_state {
    // TODO
}

- (())setMaximumTrackImage:(NSInteger)_image forState:(bool)_state {
    // TODO
}

- (())setValue:(NSInteger)_value animated:(bool)_animated {
    // TODO
}

- (id)value {
    nil
}

- (id)isDragging {
    nil
}
// TODO: all of it

@end

@implementation Slider: UISlider
- (id)isDragging {
    nil
}
@end

};
