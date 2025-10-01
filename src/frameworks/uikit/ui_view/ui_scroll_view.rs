/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UIScrollView`.

pub mod ui_text_view;
use crate::frameworks::core_graphics::{CGFloat, CGPoint, CGRect, CGSize};
use crate::frameworks::foundation::{NSUInteger, NSInteger};
use crate::objc::{
    id, impl_HostObject_with_superclass, msg, nil, objc_classes, ClassExports, NSZonePtr, SEL,
};

type UIScrollViewIndicatorStyle = NSInteger;

pub struct UIScrollViewHostObject {
    superclass: super::UIViewHostObject,
    /// UIScrollViewDelegate, weak reference
    delegate: id,
    scroll_enabled: bool,
    content_offset: CGPoint,
    content_size: CGSize,
}
impl_HostObject_with_superclass!(UIScrollViewHostObject);
impl Default for UIScrollViewHostObject {
    fn default() -> Self {
        UIScrollViewHostObject {
            superclass: Default::default(),
            delegate: nil,
            scroll_enabled: true,
            content_offset: CGPoint { x: 0.0, y: 0.0 },
            content_size: CGSize {
                width: 0.0,
                height: 0.0,
            },
        }
    }
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UIScrollView: UIView

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::<UIScrollViewHostObject>::default();
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)delegate {
    env.objc.borrow::<UIScrollViewHostObject>(this).delegate
}
- (())setDelegate:(id)delegate {
    env.objc.borrow_mut::<UIScrollViewHostObject>(this).delegate = delegate;
}

- (())setDelaysContentTouches:(id)_delay_content_touches{
    // TODO
}
- (())setBounces:(id)_bounces {
    // TODO
}

- (bool)scrollEnabled {
    env.objc.borrow::<UIScrollViewHostObject>(this).scroll_enabled
}
- (())setScrollEnabled:(bool)scroll_enabled {
    env.objc.borrow_mut::<UIScrollViewHostObject>(this).scroll_enabled = scroll_enabled;
}

- (CGPoint)contentOffset {
    env.objc.borrow::<UIScrollViewHostObject>(this).content_offset
}
- (())setContentOffset:(CGPoint)offset {
    env.objc.borrow_mut::<UIScrollViewHostObject>(this).content_offset = offset;
    // Bounds origin should be equals to the content offset
    let mut bounds: CGRect = msg![env; this bounds];
    bounds.origin = offset;
    () = msg![env; this setBounds:bounds];
    () = msg![env; this setNeedsDisplay];
}

- (CGSize)contentSize {
    env.objc.borrow::<UIScrollViewHostObject>(this).content_size
}
- (())setContentSize:(CGSize)size {
    env.objc.borrow_mut::<UIScrollViewHostObject>(this).content_size = size;
}

- (())setIndicatorStyle:(UIScrollViewIndicatorStyle)style {
    log!("TODO: [(UIScrollView*) {:?} setIndicatorStyle:{:?}]", this, style);
}

- (())setPagingEnabled:(bool)paging {
    log!("TODO: setPagingEnabled:{}", paging);
}

- (())setAlwaysBounceHorizontal:(bool)bounce {
    log!("TODO: setAlwaysBounceHorizontal:{}", bounce);
}

- (())setShowsHorizontalScrollIndicator:(bool)scroll {
    log!("TODO: setShowsHorizontalScrollIndicator:{}", scroll);
}

- (())setAlwaysBounceVertical:(bool)bounce {
    log!("TODO: setAlwaysBounceVertical:{}", bounce);
}

- (())setShowsVerticalScrollIndicator:(bool)scroll {
    log!("TODO: setShowsVerticalScrollIndicator:{}", scroll);
}

- (())setScrollsToTop:(bool)scrolls {
    log!("TODO: setScrollsToTop:{}", scrolls);
}

- (())setMaximumZoomScale:(bool)maximum {
    log!("TODO: setMaximumZoomScale:{}", maximum);
}

- (())setMinimumZoomScale:(bool)minimum {
    log!("TODO: setMinimumZoomScale:{}", minimum);
}

- (())setCanCancelContentTouches:(bool)touches {
    log!("TODO: setCanCancelContentTouches:{}", touches);
}

- (())setDirectionalLockEnabled:(bool)enabled {
    log!("TODO: setDirectionalLockEnabled:{}", enabled);
}

- (())touchesMoved:(id)touches // NSSet* of UITouch*
         withEvent:(id)_event { // UIEvent*
    let scroll_enabled: bool = msg![env; this scrollEnabled];
    if !scroll_enabled {
        return;
    }

    let touch_arr: id = msg![env; touches allObjects];
    // Assume single finger touches for now
    let touch: id = msg![env; touch_arr objectAtIndex:0u32];
    let bounds: CGRect = msg![env; this bounds];

    let prev_location: CGPoint = msg![env; touch previousLocationInView:this];
    let prev_x = prev_location.x;
    let prev_y = prev_location.y;

    let new_location: CGPoint = msg![env; touch locationInView:this];
    let y = new_location.y;
    let x = new_location.x;

    let delta_y = y - prev_y;
    let delta_x = x - prev_x;

    let offset: CGPoint = msg![env; this contentOffset];
    let content_size: CGSize = msg![env; this contentSize];

    // Very rudimentary scrolling.
    // We emulate sliding up to scroll down like on the real iPhone.
    let mut new_content_offset: CGPoint = CGPoint { x: offset.x - delta_x, y: offset.y - delta_y };

    // Update content offset within bounds
    new_content_offset.y = new_content_offset.y.min(content_size.height - bounds.size.height).max(0.0);
    new_content_offset.x = new_content_offset.x.min(content_size.width - bounds.size.width).max(0.0);

    // Trigger rerender only if required.
    log_dbg!("content offset: old {:?}, new {:?}", offset, new_content_offset);
    if new_content_offset != offset {
        () = msg![env; this setContentOffset:new_content_offset];

        let delegate: id = msg![env; this delegate];
        let sel: SEL = env
            .objc
            .register_host_selector("scrollViewDidScroll:".to_string(), &mut env.mem);
        let responds: bool = msg![env; delegate respondsToSelector:sel];
        if responds {
            () = msg![env; delegate scrollViewDidScroll:this];
        }
    }
}

@end

@implementation UITableView: UIScrollView

- (id)initWithFrame:(CGRect)frame style:(NSInteger)_style {
    // TODO: proper init
    msg![env; this init]
}

- (id)reloadData {
    nil
}

- (id)view {
    nil
}

- (id)indexPathForSelectedRow {
    nil
}

- (id)cellForRowAtIndexPath:(NSUInteger)_path {
    msg![env; this init]
}

- (())deselectRowAtIndexPath:(NSInteger)path animated:(bool)_animated {
  // TODO
}

- (())scrollToRowAtIndexPath:(NSInteger)path atScrollPosition:(bool)_position animated:(bool)_animated {
  // TODO
}

- (())setRowHeight:(CGFloat)_height {
    // TODO
}

- (())setDelegate:(id)_delegate {
    // TODO
}

- (())setDataSource:(id)_source {
    // TODO
}

- (())setShowsVerticalScrollIndicator:(bool)_show {
    // TODO
}
- (())setShowsHorizontalScrollIndicator:(bool)_show {
    // TODO
}

- (())setAllowsSelection:(bool)selection {
    log!("TODO: setAllowsSelection:{}", selection);
}

- (())setAllowsSelectionDuringEditing:(bool)editing {
    log!("TODO: setAllowsSelectionDuringEditing:{}", editing);
}

- (())setAccessoryType:(bool)accessory {
    log!("TODO: setAccessoryType:{}", accessory);
}

- (())setSeparatorColor:(bool)color {
    log!("TODO: setSeparatorColor:{}", color);
}

- (())setSeparatorStyle:(bool)style {
    log!("TODO: setSeparatorStyle:{}", style);
}

- (())setSectionHeaderHeight:(bool)height {
    log!("TODO: setSectionHeaderHeight:{}", height);
}

- (())setSectionFooterHeight:(bool)height {
    log!("TODO: setSectionFooterHeight:{}", height);
}

- (())setSectionIndexMinimumDisplayRowCount:(bool)count {
    log!("TODO: setSectionIndexMinimumDisplayRowCount:{}", count);
}

- (())setEditing:(bool)editing {
    log!("TODO: setEditing:{}", editing);
}

- (())setTableHeaderView:(bool)view {
    log!("TODO: setTableHeaderView:{}", view);
}

@end

@implementation UITableViewController: UIScrollView
@end

};
