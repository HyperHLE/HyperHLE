/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UITabBarController` and `UITabBar`.

use crate::frameworks::foundation::NSUInteger;
use crate::objc::{
    id, msg, msg_class, nil, objc_classes, release, retain, ClassExports, HostObject, NSZonePtr,
};

// MARK: - UITabBar host object

struct UITabBarHostObject {
    /// `NSArray*` of `UITabBarItem*`
    items: id,
    /// Currently selected `UITabBarItem*` (weak — owned by `items`)
    selected_item: id,
    delegate: id,
    bar_tint_color: id,  // UIColor*
    tint_color: id,      // UIColor*
    translucent: bool,
}
impl HostObject for UITabBarHostObject {}

// MARK: - UITabBarController host object

struct UITabBarControllerHostObject {
    /// `NSArray*` of `UIViewController*`
    view_controllers: id,
    selected_index: NSUInteger,
    delegate: id,
    /// The managed `UITabBar*`
    tab_bar: id,
}
impl HostObject for UITabBarControllerHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

// =========================================================================
// MARK: - UITabBar
// =========================================================================

@implementation UITabBar: UIView

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(UITabBarHostObject {
        items: nil,
        selected_item: nil,
        delegate: nil,
        bar_tint_color: nil,
        tint_color: nil,
        translucent: true,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)init {
    let items = msg_class![env; NSArray new];
    env.objc.borrow_mut::<UITabBarHostObject>(this).items = items;
    this
}

- (())dealloc {
    let host = env.objc.borrow::<UITabBarHostObject>(this);
    let (items, delegate, bar_tint_color, tint_color) =
        (host.items, host.delegate, host.bar_tint_color, host.tint_color);
    release(env, items);
    release(env, delegate);
    release(env, bar_tint_color);
    release(env, tint_color);
    env.objc.dealloc_object(this, &mut env.mem)
}

// MARK: Items

- (id)items { // NSArray* of UITabBarItem*
    env.objc.borrow::<UITabBarHostObject>(this).items
}

- (())setItems:(id)items { // NSArray*
    let old = env.objc.borrow::<UITabBarHostObject>(this).items;
    release(env, old);
    retain(env, items);
    env.objc.borrow_mut::<UITabBarHostObject>(this).items = items;
    // Clear selected item — caller must set it again if desired.
    env.objc.borrow_mut::<UITabBarHostObject>(this).selected_item = nil;
}

- (())setItems:(id)items animated:(bool)_animated {
    let _: () = msg![env; this setItems:items];
}

// MARK: Selection

- (id)selectedItem { // UITabBarItem*
    env.objc.borrow::<UITabBarHostObject>(this).selected_item
}

- (())setSelectedItem:(id)item { // UITabBarItem*
    // Verify the item is actually in our items array (or nil to deselect).
    if item != nil {
        let items = env.objc.borrow::<UITabBarHostObject>(this).items;
        let count: NSUInteger = msg![env; items count];
        let mut found = false;
        let mut i: NSUInteger = 0;
        while i < count {
            let candidate: id = msg![env; items objectAtIndex:i];
            if candidate == item {
                found = true;
                break;
            }
            i += 1;
        }
        if !found {
            log!("Warning: [UITabBar setSelectedItem:] item not in items array");
            return;
        }
    }
    env.objc.borrow_mut::<UITabBarHostObject>(this).selected_item = item;

    let delegate = env.objc.borrow::<UITabBarHostObject>(this).delegate;
    if delegate != nil {
        let _: () = msg![env; delegate tabBar:this didSelectItem:item];
    }
}

// MARK: Delegate

- (id)delegate {
    env.objc.borrow::<UITabBarHostObject>(this).delegate
}

- (())setDelegate:(id)delegate {
    let old = env.objc.borrow::<UITabBarHostObject>(this).delegate;
    release(env, old);
    retain(env, delegate);
    env.objc.borrow_mut::<UITabBarHostObject>(this).delegate = delegate;
}

// MARK: Appearance

- (id)barTintColor { // UIColor*
    env.objc.borrow::<UITabBarHostObject>(this).bar_tint_color
}

- (())setBarTintColor:(id)color { // UIColor*
    let old = env.objc.borrow::<UITabBarHostObject>(this).bar_tint_color;
    release(env, old);
    retain(env, color);
    env.objc.borrow_mut::<UITabBarHostObject>(this).bar_tint_color = color;
}

- (id)tintColor { // UIColor*
    env.objc.borrow::<UITabBarHostObject>(this).tint_color
}

- (())setTintColor:(id)color { // UIColor*
    let old = env.objc.borrow::<UITabBarHostObject>(this).tint_color;
    release(env, old);
    retain(env, color);
    env.objc.borrow_mut::<UITabBarHostObject>(this).tint_color = color;
}

- (bool)isTranslucent {
    env.objc.borrow::<UITabBarHostObject>(this).translucent
}

- (())setTranslucent:(bool)translucent {
    env.objc.borrow_mut::<UITabBarHostObject>(this).translucent = translucent;
}

// MARK: Background / shadow image stubs

- (id)backgroundImage { // UIImage*
    log!("TODO: [UITabBar backgroundImage] — returning nil");
    nil
}

- (())setBackgroundImage:(id)_image {
    log!("TODO: [UITabBar setBackgroundImage:] — ignored");
}

- (id)shadowImage { // UIImage*
    log!("TODO: [UITabBar shadowImage] — returning nil");
    nil
}

- (())setShadowImage:(id)_image {
    log!("TODO: [UITabBar setShadowImage:] — ignored");
}

- (id)selectionIndicatorImage { // UIImage*
    log!("TODO: [UITabBar selectionIndicatorImage] — returning nil");
    nil
}

- (())setSelectionIndicatorImage:(id)_image {
    log!("TODO: [UITabBar setSelectionIndicatorImage:] — ignored");
}

// MARK: Custom items editing

- (())beginCustomizingItems:(id)_items {
    log!("TODO: [UITabBar beginCustomizingItems:] — ignored");
}

- (bool)endCustomizingAnimated:(bool)_animated {
    log!("TODO: [UITabBar endCustomizingAnimated:] — returning NO");
    false
}

- (bool)isCustomizing {
    false
}

@end

// =========================================================================
// MARK: - UITabBarController
// =========================================================================

@implementation UITabBarController: UIViewController

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(UITabBarControllerHostObject {
        view_controllers: nil,
        selected_index: 0,
        delegate: nil,
        tab_bar: nil,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)init {
    let view_controllers = msg_class![env; NSArray new];
    // Create the owned UITabBar instance.
    let tab_bar: id = msg_class![env; UITabBar new];
    {
        let host = env.objc.borrow_mut::<UITabBarControllerHostObject>(this);
        host.view_controllers = view_controllers;
        host.tab_bar = tab_bar;
    }
    this
}

- (())dealloc {
    let host = env.objc.borrow::<UITabBarControllerHostObject>(this);
    let (view_controllers, delegate, tab_bar) =
        (host.view_controllers, host.delegate, host.tab_bar);
    release(env, view_controllers);
    release(env, delegate);
    release(env, tab_bar);
    env.objc.dealloc_object(this, &mut env.mem)
}

// MARK: - View controllers

- (id)viewControllers {
    env.objc.borrow::<UITabBarControllerHostObject>(this).view_controllers
}

- (())setViewControllers:(id)view_controllers { // NSArray*
    let old = env.objc.borrow::<UITabBarControllerHostObject>(this).view_controllers;
    release(env, old);
    retain(env, view_controllers);
    env.objc.borrow_mut::<UITabBarControllerHostObject>(this).view_controllers = view_controllers;

    // Sync tab bar items from each view controller's tabBarItem.
    let count: NSUInteger = msg![env; view_controllers count];

    // Build an NSMutableArray of tab bar items.
    let items: id = msg_class![env; NSMutableArray new];
    let mut i: NSUInteger = 0;
    while i < count {
        let vc: id = msg![env; view_controllers objectAtIndex:i];
        let item: id = msg![env; vc tabBarItem];
        let _: () = msg![env; items addObject:item];
        i += 1;
    }
    let tab_bar = env.objc.borrow::<UITabBarControllerHostObject>(this).tab_bar;
    let _: () = msg![env; tab_bar setItems:items];
    release(env, items);

    // Reset / clamp selected index.
    let idx = env.objc.borrow::<UITabBarControllerHostObject>(this).selected_index;
    if count == 0 {
        env.objc.borrow_mut::<UITabBarControllerHostObject>(this).selected_index = 0;
        let _: () = msg![env; tab_bar setSelectedItem:nil];
    } else {
        let clamped = if idx >= count { 0 } else { idx };
        env.objc.borrow_mut::<UITabBarControllerHostObject>(this).selected_index = clamped;
        let vc: id = msg![env; view_controllers objectAtIndex:clamped];
        let item: id = msg![env; vc tabBarItem];
        let _: () = msg![env; tab_bar setSelectedItem:item];
    }
}

- (())setViewControllers:(id)view_controllers animated:(bool)_animated {
    let _: () = msg![env; this setViewControllers:view_controllers];
}

// MARK: - Selected index / controller

- (NSUInteger)selectedIndex {
    env.objc.borrow::<UITabBarControllerHostObject>(this).selected_index
}

- (())setSelectedIndex:(NSUInteger)index {
    let vcs = env.objc.borrow::<UITabBarControllerHostObject>(this).view_controllers;
    let count: NSUInteger = msg![env; vcs count];
    if index >= count {
        log!(
            "Warning: [UITabBarController setSelectedIndex:{}] out of bounds (count {})",
            index, count
        );
        return;
    }
    env.objc.borrow_mut::<UITabBarControllerHostObject>(this).selected_index = index;

    // Keep tab bar in sync — write directly to avoid double-firing delegate.
    let vc: id = msg![env; vcs objectAtIndex:index];
    let item: id = msg![env; vc tabBarItem];
    let tab_bar = env.objc.borrow::<UITabBarControllerHostObject>(this).tab_bar;
    env.objc.borrow_mut::<UITabBarHostObject>(tab_bar).selected_item = item;

    let delegate = env.objc.borrow::<UITabBarControllerHostObject>(this).delegate;
    if delegate != nil {
        let _: () = msg![env; delegate tabBarController:this didSelectViewController:vc];
    }
}

- (id)selectedViewController {
    let vcs = env.objc.borrow::<UITabBarControllerHostObject>(this).view_controllers;
    let count: NSUInteger = msg![env; vcs count];
    if count == 0 {
        return nil;
    }
    let idx = env.objc.borrow::<UITabBarControllerHostObject>(this).selected_index;
    msg![env; vcs objectAtIndex:idx]
}

- (())setSelectedViewController:(id)view_controller {
    let vcs = env.objc.borrow::<UITabBarControllerHostObject>(this).view_controllers;
    let count: NSUInteger = msg![env; vcs count];
    let mut found_index: Option<NSUInteger> = None;
    let mut i: NSUInteger = 0;
    while i < count {
        let vc: id = msg![env; vcs objectAtIndex:i];
        if vc == view_controller {
            found_index = Some(i);
            break;
        }
        i += 1;
    }
    if let Some(index) = found_index {
        let _: () = msg![env; this setSelectedIndex:index];
    } else {
        log!("Warning: [UITabBarController setSelectedViewController:] view controller not found in list");
    }
}

// MARK: - Delegate

- (id)delegate {
    env.objc.borrow::<UITabBarControllerHostObject>(this).delegate
}

- (())setDelegate:(id)delegate {
    let old = env.objc.borrow::<UITabBarControllerHostObject>(this).delegate;
    release(env, old);
    retain(env, delegate);
    env.objc.borrow_mut::<UITabBarControllerHostObject>(this).delegate = delegate;
}

// MARK: - Tab bar accessor

- (id)tabBar {
    env.objc.borrow::<UITabBarControllerHostObject>(this).tab_bar
}

// MARK: - UIViewController overrides

- (id)view {
    let vc: id = msg![env; this selectedViewController];
    if vc == nil { return nil; }
    msg![env; vc view]
}

- (())viewDidLoad {
    let vc: id = msg![env; this selectedViewController];
    if vc != nil {
        let _: () = msg![env; vc viewDidLoad];
    }
}

- (())viewWillAppear:(bool)animated {
    let vc: id = msg![env; this selectedViewController];
    if vc != nil {
        let _: () = msg![env; vc viewWillAppear:animated];
    }
}

- (())viewDidAppear:(bool)animated {
    let vc: id = msg![env; this selectedViewController];
    if vc != nil {
        let _: () = msg![env; vc viewDidAppear:animated];
    }
}

- (())viewWillDisappear:(bool)animated {
    let vc: id = msg![env; this selectedViewController];
    if vc != nil {
        let _: () = msg![env; vc viewWillDisappear:animated];
    }
}

- (())viewDidDisappear:(bool)animated {
    let vc: id = msg![env; this selectedViewController];
    if vc != nil {
        let _: () = msg![env; vc viewDidDisappear:animated];
    }
}

@end

};
