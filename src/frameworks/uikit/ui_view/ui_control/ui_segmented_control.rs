/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UISegmentedControl`.

use crate::frameworks::core_graphics::{CGFloat, CGRect, CGSize};
use crate::frameworks::foundation::{NSUInteger, NSInteger};
use crate::objc::{
    id, msg, msg_class, msg_super, nil, objc_classes, release, retain, ClassExports, HostObject,
    NSZonePtr,
};

type UISegmentedControlStyle = NSInteger;
const UISegmentedControlStylePlain:  UISegmentedControlStyle = 0;
const UISegmentedControlStyleBordered: UISegmentedControlStyle = 1;
const UISegmentedControlStyleBar:    UISegmentedControlStyle = 2;
const UISegmentedControlStyleBezeled: UISegmentedControlStyle = 3;

struct UISegmentedControlHostObject {
    /// `NSMutableArray*` of `NSString*` segment titles
    segments: id,
    selected_index: NSInteger,
    momentary: bool,
    enabled: bool,
    style: UISegmentedControlStyle,
}
impl HostObject for UISegmentedControlHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UISegmentedControl: UIControl

+ (id)allocWithZone:(NSZonePtr)_zone {
    let segments = msg_class![env; NSMutableArray new];
    let host_object = Box::new(UISegmentedControlHostObject {
        segments,
        selected_index: 0,
        momentary: false,
        enabled: true,
        style: UISegmentedControlStylePlain,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)initWithFrame:(CGRect)frame {
    log_dbg!("UISegmentedControl initWithFrame: stubbed");
    msg_super![env; this initWithFrame:frame]
}

- (id)initWithItems:(id)items { // NSArray* of NSString* or UIImage*
    let count: u32 = msg![env; items count];
    let segments = msg_class![env; NSMutableArray new];
    for i in 0..count {
        let item: id = msg![env; items objectAtIndex:i];
        retain(env, item);
        msg![env; segments addObject:item]
    }
    env.objc.borrow_mut::<UISegmentedControlHostObject>(this).segments = segments;
    this
}

- (id)initWithCoder:(id)coder {
    log_dbg!("UISegmentedControl initWithCoder: stubbed");
    msg_super![env; this initWithCoder:coder]
}

- (())dealloc {
    let segments = env.objc.borrow::<UISegmentedControlHostObject>(this).segments;
    release(env, segments);
    env.objc.dealloc_object(this, &mut env.mem)
}

// MARK: - Segments

- (NSInteger)numberOfSegments {
    let segments = env.objc.borrow::<UISegmentedControlHostObject>(this).segments;
    let count: u32 = msg![env; segments count];
    count as NSInteger
}

- (())insertSegmentWithTitle:(id)title // NSString*
                    atIndex:(NSInteger)index
                   animated:(bool)_animated {
    let segments = env.objc.borrow::<UISegmentedControlHostObject>(this).segments;
    let count: u32 = msg![env; segments count];
    let insert_idx = (index as u32).min(count);
    retain(env, title);
    msg![env; segments insertObject:title atIndex:insert_idx]
}

- (())insertSegmentWithImage:(id)image // UIImage*
                    atIndex:(NSInteger)index
                   animated:(bool)_animated {
    let segments = env.objc.borrow::<UISegmentedControlHostObject>(this).segments;
    let count: u32 = msg![env; segments count];
    let insert_idx = (index as u32).min(count);
    retain(env, image);
    msg![env; segments insertObject:image atIndex:insert_idx]
}

- (())removeSegmentAtIndex:(NSInteger)index animated:(bool)_animated {
    let segments = env.objc.borrow::<UISegmentedControlHostObject>(this).segments;
    msg![env; segments removeObjectAtIndex:(index as u32)]
}

- (())removeAllSegments {
    let segments = env.objc.borrow::<UISegmentedControlHostObject>(this).segments;
    msg![env; segments removeAllObjects]
}

- (())setTitle:(id)title forSegmentAtIndex:(NSInteger)index {
    let segments = env.objc.borrow::<UISegmentedControlHostObject>(this).segments;
    retain(env, title);
    msg![env; segments replaceObjectAtIndex:(index as u32) withObject:title]
}

- (id)titleForSegmentAtIndex:(NSInteger)index {
    let segments = env.objc.borrow::<UISegmentedControlHostObject>(this).segments;
    let count: u32 = msg![env; segments count];
    if index < 0 || index as u32 >= count { return nil; }
    msg![env; segments objectAtIndex:(index as u32)]
}

- (())setImage:(id)image forSegmentAtIndex:(NSInteger)index {
    let segments = env.objc.borrow::<UISegmentedControlHostObject>(this).segments;
    retain(env, image);
    msg![env; segments replaceObjectAtIndex:(index as u32) withObject:image]
}

- (id)imageForSegmentAtIndex:(NSInteger)index {
    let segments = env.objc.borrow::<UISegmentedControlHostObject>(this).segments;
    let count: u32 = msg![env; segments count];
    if index < 0 || index as u32 >= count { return nil; }
    msg![env; segments objectAtIndex:(index as u32)]
}

// MARK: - Selection

- (NSInteger)selectedSegmentIndex {
    env.objc.borrow::<UISegmentedControlHostObject>(this).selected_index
}

- (())setSelectedSegmentIndex:(NSInteger)index {
    env.objc.borrow_mut::<UISegmentedControlHostObject>(this).selected_index = index;
}

// MARK: - Style

- (UISegmentedControlStyle)segmentedControlStyle {
    env.objc.borrow::<UISegmentedControlHostObject>(this).style
}

- (())setSegmentedControlStyle:(UISegmentedControlStyle)style {
    env.objc.borrow_mut::<UISegmentedControlHostObject>(this).style = style;
}

- (bool)isMomentary {
    env.objc.borrow::<UISegmentedControlHostObject>(this).momentary
}

- (())setMomentary:(bool)value {
    env.objc.borrow_mut::<UISegmentedControlHostObject>(this).momentary = value;
}

// MARK: - Enable / disable individual segments

- (())setEnabled:(bool)enabled forSegmentAtIndex:(NSInteger)_index {
    // We don't track per-segment enabled state; store global.
    env.objc.borrow_mut::<UISegmentedControlHostObject>(this).enabled = enabled;
}

- (bool)isEnabledForSegmentAtIndex:(NSInteger)_index {
    env.objc.borrow::<UISegmentedControlHostObject>(this).enabled
}

// MARK: - Width

- (())setWidth:(CGFloat)_width forSegmentAtIndex:(NSInteger)_index {
    // No rendering — ignore.
}

- (CGFloat)widthForSegmentAtIndex:(NSInteger)_index {
    0.0
}

- (())setContentOffset:(CGSize)_offset forSegmentAtIndex:(NSInteger)_index {
    // No rendering — ignore.
}

// MARK: - Appearance (iOS 5+)

- (())setBackgroundImage:(id)_image
                forState:(NSUInteger)_state
              barMetrics:(NSInteger)_metrics {
    // No rendering — ignore.
}

- (())setDividerImage:(id)_image
  forLeftSegmentState:(NSUInteger)_left
    rightSegmentState:(NSUInteger)_right
           barMetrics:(NSInteger)_metrics {
    // No rendering — ignore.
}

- (())setTitleTextAttributes:(id)_attrs forState:(NSUInteger)_state {
    // No rendering — ignore.
}

- (())setContentPositionAdjustment:(CGSize)_adj
                      forSegmentType:(NSInteger)_type
                          barMetrics:(NSInteger)_metrics {
    // No rendering — ignore.
}

@end

// MARK: - UISegment (undocumented internal class)

@implementation UISegment: UIControl

- (id)initWithCoder:(id)coder {
    log_dbg!("UISegment initWithCoder: stubbed");
    msg_super![env; this initWithCoder:coder]
}

- (id)initWithFrame:(CGRect)frame {
    log_dbg!("UISegment initWithFrame: stubbed");
    msg_super![env; this initWithFrame:frame]
}

- (())setTitle:(id)_title {
    // Stub.
}

- (id)title {
    nil
}

- (())setImage:(id)_image {
    // Stub.
}

- (id)image {
    nil
}

@end

};
