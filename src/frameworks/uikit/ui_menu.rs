/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UIMenuController` and `UIMenuItem`.
//!
//! Editing menus (Copy/Paste, and custom items added by the app). Chrome and
//! other text-heavy apps construct menu items during launch; touchHLE does
//! not draw the menu, but the object model has to exist so that those calls
//! don't fail.
//!
//! References:
//! - <https://developer.apple.com/documentation/uikit/uimenucontroller>
//! - <https://developer.apple.com/documentation/uikit/uimenuitem>

use crate::frameworks::core_graphics::CGRect;
use crate::frameworks::foundation::NSInteger;
use crate::objc::{
    id, msg, nil, objc_classes, release, retain, ClassExports, HostObject, NSZonePtr, SEL,
};

#[derive(Default)]
struct UIMenuItemHostObject {
    /// `NSString *`
    title: id,
    action: SEL,
}
impl HostObject for UIMenuItemHostObject {}

#[derive(Default)]
struct UIMenuControllerHostObject {
    /// `NSArray *` of `UIMenuItem *`, retained.
    menu_items: id,
    /// The target rectangle, in `target_view`'s coordinate space.
    #[allow(dead_code)] // Only the (unrendered) menu consumes it.
    target_rect: CGRect,
    /// Weak (UIKit does not retain the view).
    #[allow(dead_code)]
    target_view: id,
    visible: bool,
    /// `UIMenuControllerArrowDirection`
    arrow_direction: NSInteger,
}
impl HostObject for UIMenuControllerHostObject {}

#[derive(Default)]
pub struct State {
    /// The shared menu controller (lazily created).
    shared_menu_controller: id,
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UIMenuItem: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    env.objc.alloc_object(this, Box::<UIMenuItemHostObject>::default(), &mut env.mem)
}

- (id)initWithTitle:(id)title action:(SEL)action {
    retain(env, title);
    let old = {
        let mut host = env.objc.borrow_mut::<UIMenuItemHostObject>(this);
        let old = std::mem::replace(&mut host.title, title);
        host.action = action;
        old
    };
    release(env, old);
    this
}

- (id)title { env.objc.borrow::<UIMenuItemHostObject>(this).title }
- (())setTitle:(id)title {
    retain(env, title);
    let old = std::mem::replace(
        &mut env.objc.borrow_mut::<UIMenuItemHostObject>(this).title,
        title,
    );
    release(env, old);
}

- (SEL)action { env.objc.borrow::<UIMenuItemHostObject>(this).action }
- (())setAction:(SEL)action {
    env.objc.borrow_mut::<UIMenuItemHostObject>(this).action = action;
}

- (())dealloc {
    let title = std::mem::take(&mut env.objc.borrow_mut::<UIMenuItemHostObject>(this).title);
    release(env, title);
    env.objc.dealloc_object(this, &mut env.mem)
}

@end

@implementation UIMenuController: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    env.objc.alloc_object(this, Box::<UIMenuControllerHostObject>::default(), &mut env.mem)
}

+ (id)sharedMenuController {
    let existing = env.framework_state.uikit.ui_menu.shared_menu_controller;
    if existing != nil {
        return existing;
    }
    let new_instance: id = msg![env; this alloc];
    let new_instance: id = msg![env; new_instance init];
    env.framework_state.uikit.ui_menu.shared_menu_controller = new_instance;
    new_instance
}

- (bool)isMenuVisible {
    env.objc.borrow::<UIMenuControllerHostObject>(this).visible
}
- (())setMenuVisible:(bool)visible {
    env.objc.borrow_mut::<UIMenuControllerHostObject>(this).visible = visible;
}
- (())setMenuVisible:(bool)visible animated:(bool)_animated {
    env.objc.borrow_mut::<UIMenuControllerHostObject>(this).visible = visible;
}

- (CGRect)menuFrame { CGRect::default() }

- (id)menuItems {
    env.objc.borrow::<UIMenuControllerHostObject>(this).menu_items
}
- (())setMenuItems:(id)items {
    retain(env, items);
    let old = std::mem::replace(
        &mut env.objc.borrow_mut::<UIMenuControllerHostObject>(this).menu_items,
        items,
    );
    release(env, old);
}

// Per Apple, the rectangle is in the view's coordinate space. It is only a
// positioning hint for the (unrendered) menu, so storing it is enough.
- (())setTargetRect:(CGRect)rect inView:(id)view {
    let host = env.objc.borrow_mut::<UIMenuControllerHostObject>(this);
    host.target_rect = rect;
    host.target_view = view;
}

- (NSInteger)arrowDirection {
    env.objc.borrow::<UIMenuControllerHostObject>(this).arrow_direction
}
- (())setArrowDirection:(NSInteger)direction {
    env.objc.borrow_mut::<UIMenuControllerHostObject>(this).arrow_direction = direction;
}

// Recomputes the menu's contents; there is nothing to redraw here.
- (())update {}

- (())dealloc {
    let items = std::mem::take(&mut env.objc.borrow_mut::<UIMenuControllerHostObject>(this).menu_items);
    release(env, items);
    env.objc.dealloc_object(this, &mut env.mem)
}

@end

};