/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UISegmentedControl`.

use crate::frameworks::core_graphics::{CGFloat, CGPoint, CGRect, CGSize};
use crate::frameworks::foundation::{NSInteger, NSUInteger};
use crate::frameworks::uikit::ui_view::UIViewHostObject;
use crate::objc::{
    id, impl_HostObject_with_superclass, msg, msg_class, msg_super, nil, objc_classes, release,
    retain, ClassExports, NSZonePtr,
};

struct UISegmentedControlHostObject {
    superclass: super::UIControlHostObject,
    segments: Vec<id>,   // NSString* titles (retained)
    selected_index: i32,
    segment_labels: Vec<id>, // UILabel* subviews (retained)
}
impl_HostObject_with_superclass!(UISegmentedControlHostObject);
impl Default for UISegmentedControlHostObject {
    fn default() -> Self {
        UISegmentedControlHostObject {
            superclass: Default::default(),
            segments: Vec::new(),
            selected_index: 0,
            segment_labels: Vec::new(),
        }
    }
}

fn rebuild_segments(env: &mut crate::Environment, this: id) {
    // Remove old labels
    let old_labels = env
        .objc
        .borrow::<UISegmentedControlHostObject>(this)
        .segment_labels
        .clone();
    for label in &old_labels {
        let _: () = msg![env; *label removeFromSuperview];
        release(env, *label);
    }
    env.objc
        .borrow_mut::<UISegmentedControlHostObject>(this)
        .segment_labels
        .clear();

    let bounds: CGRect = msg![env; this bounds];
    let n = env
        .objc
        .borrow::<UISegmentedControlHostObject>(this)
        .segments
        .len();
    if n == 0 {
        return;
    }

    let seg_width = bounds.size.width / n as CGFloat;
    let selected = env
        .objc
        .borrow::<UISegmentedControlHostObject>(this)
        .selected_index;

    let titles: Vec<id> = env
        .objc
        .borrow::<UISegmentedControlHostObject>(this)
        .segments
        .clone();

    for (i, title) in titles.iter().enumerate() {
        let frame = CGRect {
            origin: CGPoint {
                x: seg_width * i as CGFloat,
                y: 0.0,
            },
            size: CGSize {
                width: seg_width,
                height: bounds.size.height,
            },
        };

        let label: id = msg_class![env; UILabel alloc];
        let label: id = msg![env; label initWithFrame:frame];
        () = msg![env; label setText:*title];
        () = msg![env; label setTextAlignment:1i32]; // NSTextAlignmentCenter

        let font: id = msg_class![env; UIFont systemFontOfSize:12_f32];
        () = msg![env; label setFont:font];

        // Selected = white text on dark bg, unselected = dark text on light bg
        if i as i32 == selected {
            let bg: id = msg_class![env; UIColor darkGrayColor];
            let fg: id = msg_class![env; UIColor whiteColor];
            () = msg![env; label setBackgroundColor:bg];
            () = msg![env; label setTextColor:fg];
        } else {
            let bg: id = msg_class![env; UIColor lightGrayColor];
            let fg: id = msg_class![env; UIColor blackColor];
            () = msg![env; label setBackgroundColor:bg];
            () = msg![env; label setTextColor:fg];
        }

        () = msg![env; this addSubview:label];
        retain(env, label);
        env.objc
            .borrow_mut::<UISegmentedControlHostObject>(this)
            .segment_labels
            .push(label);
    }
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UISegmentedControl: UIControl

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::<UISegmentedControlHostObject>::default();
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)initWithFrame:(CGRect)frame {
    let this: id = msg_super![env; this initWithFrame:frame];
    this
}

- (id)initWithCoder:(id)coder {
    let this: id = msg_super![env; this initWithCoder:coder];
    this
}

- (id)initWithItems:(id)items {
    let this: id = msg![env; this init];
    if items != nil {
        let count: NSUInteger = msg![env; items count];
        for i in 0..count {
            let item: id = msg![env; items objectAtIndex:i];
            let n = env.objc.borrow::<UISegmentedControlHostObject>(this).segments.len();
            retain(env, item);
            env.objc.borrow_mut::<UISegmentedControlHostObject>(this).segments.push(item);
            let _ = n;
        }
    }
    this
}

- (NSInteger)selectedSegmentIndex {
    env.objc.borrow::<UISegmentedControlHostObject>(this).selected_index as NSInteger
}

- (())setSelectedSegmentIndex:(NSInteger)index {
    env.objc.borrow_mut::<UISegmentedControlHostObject>(this).selected_index = index as i32;
    rebuild_segments(env, this);
}

- (NSUInteger)numberOfSegments {
    env.objc.borrow::<UISegmentedControlHostObject>(this).segments.len() as NSUInteger
}

- (())insertSegmentWithTitle:(id)title atIndex:(NSUInteger)index animated:(bool)_animated {
    retain(env, title);
    let len = env.objc.borrow::<UISegmentedControlHostObject>(this).segments.len();
    let idx = (index as usize).min(len);
    env.objc.borrow_mut::<UISegmentedControlHostObject>(this).segments.insert(idx, title);
    rebuild_segments(env, this);
}

- (())insertSegmentWithImage:(id)_image atIndex:(NSUInteger)index animated:(bool)_animated {
    // Use empty string as placeholder for image segments
    let placeholder: id = msg_class![env; NSString string];
    retain(env, placeholder);
    let len = env.objc.borrow::<UISegmentedControlHostObject>(this).segments.len();
    let idx = (index as usize).min(len);
    env.objc.borrow_mut::<UISegmentedControlHostObject>(this).segments.insert(idx, placeholder);
    rebuild_segments(env, this);
}

- (())removeSegmentAtIndex:(NSUInteger)index animated:(bool)_animated {
    let len = env.objc.borrow::<UISegmentedControlHostObject>(this).segments.len();
    if (index as usize) < len {
        let title = env.objc.borrow::<UISegmentedControlHostObject>(this).segments[index as usize];
        release(env, title);
        env.objc.borrow_mut::<UISegmentedControlHostObject>(this).segments.remove(index as usize);
        rebuild_segments(env, this);
    }
}

- (())removeAllSegments {
    let titles: Vec<id> = env.objc.borrow::<UISegmentedControlHostObject>(this).segments.clone();
    for t in titles { release(env, t); }
    env.objc.borrow_mut::<UISegmentedControlHostObject>(this).segments.clear();
    rebuild_segments(env, this);
}

- (())setTitle:(id)title forSegmentAtIndex:(NSUInteger)index {
    let len = env.objc.borrow::<UISegmentedControlHostObject>(this).segments.len();
    if (index as usize) < len {
        let old = env.objc.borrow::<UISegmentedControlHostObject>(this).segments[index as usize];
        release(env, old);
        retain(env, title);
        env.objc.borrow_mut::<UISegmentedControlHostObject>(this).segments[index as usize] = title;
        rebuild_segments(env, this);
    }
}

- (id)titleForSegmentAtIndex:(NSUInteger)index {
    let segs = env.objc.borrow::<UISegmentedControlHostObject>(this).segments.clone();
    if (index as usize) < segs.len() {
        segs[index as usize]
    } else {
        nil
    }
}

- (())setEnabled:(bool)_enabled forSegmentAtIndex:(NSUInteger)_index {
}

- (bool)isEnabledForSegmentAtIndex:(NSUInteger)_index {
    true
}

- (())setWidth:(CGFloat)_width forSegmentAtIndex:(NSUInteger)_index {
}

- (())setMomentary:(bool)_momentary {
}

- (bool)isMomentary {
    false
}

- (())setSegmentedControlStyle:(NSInteger)_style {
}

- (NSInteger)segmentedControlStyle {
    0
}

- (())setTintColor:(id)_color {
}

- (())setApportionsSegmentWidthsByContent:(bool)_val {
}

- (())sizeToFit {
}

- (())setImage:(id)_image forSegmentAtIndex:(NSUInteger)_index {
}

- (())setContentOffset:(CGSize)_offset forSegmentAtIndex:(NSUInteger)_index {
}

- (())setDividerImage:(id)_image forLeftSegmentState:(NSUInteger)_left rightSegmentState:(NSUInteger)_right barMetrics:(NSInteger)_metrics {
}

- (())setBackgroundImage:(id)_image forState:(NSUInteger)_state barMetrics:(NSInteger)_metrics {
}

- (())setTitleTextAttributes:(id)_attrs forState:(NSUInteger)_state {
}

- (())dealloc {
    let titles: Vec<id> = env.objc.borrow::<UISegmentedControlHostObject>(this).segments.clone();
    for t in titles { release(env, t); }
    let labels: Vec<id> = env.objc.borrow::<UISegmentedControlHostObject>(this).segment_labels.clone();
    for l in labels { release(env, l); }
    msg_super![env; this dealloc]
}

@end

// Undocumented class used internally by UISegmentedControl
@implementation UISegment: UIControl

- (id)initWithCoder:(id)coder {
    msg_super![env; this initWithCoder:coder]
}

- (id)initWithFrame:(CGRect)frame {
    msg_super![env; this initWithFrame:frame]
}

@end

};
