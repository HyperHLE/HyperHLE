/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UIWindow`.
//!
//! Useful resources:
//! - [Technical Q&A QA1588: Automatic orientation support for iPhone and iPad apps](https://developer.apple.com/library/archive/qa/qa1588/_index.html)
//! - [Technical Q&A QA1688: Why won't my UIViewController rotate with the device?](https://developer.apple.com/library/archive/qa/qa1688/_index.html)

use super::UIViewHostObject;
use crate::dyld::{ConstantExports, HostConstant};
use crate::frameworks::core_graphics::cg_affine_transform::CGAffineTransform;
use crate::frameworks::core_graphics::{CGPoint, CGRect};
use crate::frameworks::foundation::ns_string;
use crate::frameworks::uikit::ui_application::{
    UIInterfaceOrientationLandscapeLeft, UIInterfaceOrientationLandscapeRight,
};
use crate::frameworks::uikit::ui_device::{
    UIDeviceOrientationLandscapeLeft, UIDeviceOrientationLandscapeRight,
};
use crate::objc::{id, msg, msg_class, msg_super, nil, objc_classes, ClassExports};

#[derive(Default)]
pub struct State {
    /// List of visible windows for internal purposes. Non-retaining!
    ///
    /// This is public because Core Animation also uses it.
    pub windows: Vec<id>,
    /// The most recent window which received `makeKeyAndVisible` message.
    /// Non-retaining!
    pub key_window: Option<id>,
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UIWindow: UIView

// TODO: more?

- (id)initWithFrame:(CGRect)frame {
    let this = msg_super![env; this initWithFrame:frame];
    // Undocumented: windows seem to be hidden by default on iOS, unlike views.
    // Super call to bypass the overriden setter on this class, which would post
    // a notification.
    () = msg_super![env; this setHidden:true];

    let list = &mut env.framework_state.uikit.ui_view.ui_window.windows;
    list.push(this);
    log_dbg!(
        "New window: {:?}. New list of all windows: {:?}",
        this,
        list,
    );

    this
}

// NSCoding implementation
- (id)initWithCoder:(id)coder {
    let this = msg_super![env; this initWithCoder:coder];
    // Undocumented: windows seem to be hidden by default on iOS, unlike views.
    // Super call to bypass the overriden setter on this class, which would post
    // a notification.
    () = msg_super![env; this setHidden:true];

    // Real iOS forces UIWindow.frame to match UIScreen.bounds regardless of
    // whatever frame was encoded in the NIB. Interface Builder by default
    // uses a 320x460 canvas (status bar visible) for iPhone XIBs, so a
    // window loaded from a NIB will normally come out at 320x460 and miss
    // the bottom 20px of the screen. Apps that hide the status bar via
    // Info.plist (e.g. Minecraft PE 0.6.x) then end up with their EAGLView
    // sized to the truncated window, producing a visible offset/shift.
    //
    // Mirror Apple's behaviour by re-setting the frame to the full screen
    // bounds after the super decoder runs.
    let screen: id = msg_class![env; UIScreen mainScreen];
    let screen_bounds: CGRect = msg![env; screen bounds];
    let current_bounds: CGRect = msg![env; this bounds];
    if current_bounds.size != screen_bounds.size {
        log_dbg!(
            "UIWindow {:?}: overriding NIB-encoded size {:?} with UIScreen.bounds size {:?}",
            this,
            current_bounds.size,
            screen_bounds.size,
        );
        () = msg![env; this setFrame:screen_bounds];
    }

    let list = &mut env.framework_state.uikit.ui_view.ui_window.windows;
    list.push(this);
    log_dbg!(
        "New window: {:?}. New list of all windows: {:?}",
        this,
        list,
    );

    this
}

- (())dealloc {
    if let Some(key_window) = env.framework_state.uikit.ui_view.ui_window.key_window {
        if key_window == this {
            env.framework_state.uikit.ui_view.ui_window.key_window = None;
        }
    }
    let list = &mut env.framework_state.uikit.ui_view.ui_window.windows;
    let idx = list.iter().position(|&w| w == this).unwrap();
    list.remove(idx);
    log_dbg!(
        "Deallocating window {:?}. New list of all windows: {:?}",
        this,
        list,
    );
    msg_super![env; this dealloc]
}

- (())layoutIfNeeded {
    log_dbg!("[(UIWindow*){:?} layoutIfNeeded]", this);
    // Честная реализация: немедленно форсируем пересчет layout'а,
    // отправляя сообщение layoutSubviews самому себе (наследуется от UIView)
    () = msg![env; this layoutSubviews];
}

// Permissive hit-testing for the application window.
//
// On real iOS, UIWindow's frame matches UIScreen.bounds and touches always
// land inside it, so the standard UIView hitTest implementation is fine.
// In touchHLE, however, the host window can be larger than the iPhone's
// virtual screen (Android phones, large desktop windows). Even with the
// touch-coordinate clamp in `transform_input_coords`, edge cases such as
// the rootViewController's view being rotated for landscape orientation
// can leave individual modal/overlay subviews positioned in such a way
// that a touch landing on them produces a window-coordinate point just
// outside of UIWindow's own bounds (off-by-one at the bottom row, status
// bar overlap, etc.). The default UIView.hitTest then early-exits via
// pointInside, returning nil for the entire touch — which is what
// produced the `SUPER HACK: Forcing rejected touch ...` log spam and
// stopped the in-game chat / Create World text fields from ever becoming
// first responder.
//
// Override hitTest to ALWAYS recurse into subviews. If any subview claims
// the touch we return it; otherwise we fall back to the window itself so
// touch dispatch is never lost. This matches Apple's documented intent:
// "Windows don't actively participate in event handling. They merely
// pass events on to their subviews."
- (id)hitTest:(CGPoint)point withEvent:(id)event {
    let subviews = env.objc.borrow::<super::UIViewHostObject>(this).subviews.clone();
    for subview in subviews.into_iter().rev() {
        let hidden: bool = msg![env; subview isHidden];
        let alpha: crate::frameworks::core_graphics::CGFloat = msg![env; subview alpha];
        let interactible: bool = msg![env; subview isUserInteractionEnabled];
        if hidden || alpha < 0.01 || !interactible { continue; }
        let sub_point: CGPoint = msg![env; subview convertPoint:point fromView:this];
        let hit: id = msg![env; subview hitTest:sub_point withEvent:event];
        if hit != nil { return hit; }
    }
    this
}

- (())setHidden:(bool)is_hidden {
    () = msg_super![env; this setHidden:is_hidden];

    // TODO: post UIWindowDidBecomeVisibleNotification,
    //            UIWindowDidBecomeHiddenNotification
    log_dbg!("[(UIWindow*){:?} setHidden:{:?}]", this, is_hidden);
}

- (())makeKeyWindow {
    // TODO: post UIWindowDidResignKeyNotification for previous key window
    env.framework_state.uikit.ui_view.ui_window.key_window = Some(this);

    let center: id = msg_class![env; NSNotificationCenter defaultCenter];
    let notif_name = ns_string::get_static_str(env, UIWindowDidBecomeKeyNotification);
    () = msg![env; center postNotificationName:notif_name object:this userInfo:nil];
}

- (bool)isKeyWindow {
    env.framework_state.uikit.ui_view.ui_window.key_window == Some(this)
}

- (())makeKeyAndVisible {
    // TODO: We don't currently have send any non-touch events to windows,
    // so there's no meaning in it yet.

    // FIXME: This should also bump the window to the top of the list.

    () = msg![env; this makeKeyWindow];

    // TODO: post UIWindowDidBecomeVisibleNotification
    () = msg![env; this setHidden:false];
}

// Legacy iOS 2/3 pattern: [window setContentView:someView]
// Semantically equivalent to addSubview: for UIWindow.
- (())setContentView:(id)view {
    log_dbg!("[(UIWindow*){:?} setContentView:{:?}]", this, view);
    () = msg![env; this addSubview:view];
}

// Returns the first subview as the content view (legacy behaviour).
- (id)contentView {
    let subviews = &env.objc.borrow::<UIViewHostObject>(this).subviews;
    subviews.first().copied().unwrap_or(nil)
}

// Support for rootViewController (iOS 4+)
- (())setRootViewController:(id)view_controller {
    log_dbg!("[(UIWindow*){:?} setRootViewController:{:?}]", this, view_controller);

    // The default behavior in iOS is to add the view controller's view as a
    // subview of the window.
    if view_controller != nil {
        let view: id = msg![env; view_controller view];
        () = msg![env; this addSubview:view];
    }
}

- (id)rootViewController {
    log!("TODO: [(UIWindow*){:?} rootViewController] full implementation missing", this);
    nil
}

// UIResponder implementation
// From the Apple UIView docs regarding [UIResponder nextResponder]:
// "UIWindow returns the application object."
- (id)nextResponder {
    msg_class![env; UIApplication sharedApplication]
}

- (())addSubview:(id)view {
    log_dbg!("[(UIWindow*){:?} addSubview:{:?}] => ()", this, view);

    if view == nil || env.objc.borrow::<UIViewHostObject>(view).view_controller == nil {
        () = msg_super![env; this addSubview:view];
        return;
    }

    // Below we treat a special case of adding view controller's view
    // to a window, in order to generate display related notifications

    if env.objc.borrow::<UIViewHostObject>(this).subviews.contains(&view) {
        // For the case of existing view hidden by another view,
        // we need to delay a below sequence up until obstructions are removed
        log!("TODO: case of existing view hidden by another view for sending view[Will,Did]Appear");
    }

    let vc = env.objc.borrow::<UIViewHostObject>(view).view_controller;
    () = msg![env; vc viewWillAppear:false];
    () = msg_super![env; this addSubview:view];
    () = msg![env; vc viewDidAppear:false];

    // Support auto-rotation. This is currently only for apps that request a
    // non-portrait interface orientation via Info.plist, as we do not yet
    // support changes of orientation caused by device rotation (TODO).
    // FIXME: It's unclear when and where this auto-rotation is supposed to
    //        happen. It must have something to do with mounting the view
    //        controller to a window, so we do it here. QA1688 (see top of file)
    //        mentions a breaking behaviour change in iOS 6 that makes
    //        auto-rotation rely on rootViewController (a property only found in
    //        iOS 6), so the current implementation is specific to iOS <= 5.
    // FIXME: Are we supposed to notify the view somehow of the rotation?
    // FIXME: What do we do if shouldAutorotateToInterfaceOrientation:
    //        returns false? The status bar has already been rotated…
    // FIXME: The device orientation stored on env.window can come from one of
    //        three places (user/default options, setStatusBarOrientation: etc,
    //        Info.plist UIInterfaceOrientation etc). It's not clear if these
    //        are really equivalent and should all trigger autorotation.
    if let Some(orientation) = match env.window.as_ref().unwrap().current_rotation() {
        crate::window::DeviceOrientation::LandscapeLeft => Some(UIDeviceOrientationLandscapeLeft),
        crate::window::DeviceOrientation::LandscapeRight => Some(UIDeviceOrientationLandscapeRight),
        // Portrait is the default so we don't do anything here.
        crate::window::DeviceOrientation::Portrait => None,
    } {
        // (UIInterfaceOrientation and UIDeviceOrientation are compatible enums,
        //  here we use whichever is clearer contextually.)
        let should = msg![env; vc shouldAutorotateToInterfaceOrientation:orientation];
        log_dbg!("[{:?} shouldAutorotateToInterfaceOrientation:{:?}] => {:?}", vc, orientation, should);
        if should {
            log_dbg!("App requested autorotation; applying orientation transform to view {:?}.", view);
            let transform = match orientation {
                UIInterfaceOrientationLandscapeLeft => CGAffineTransform::make_rotation(-std::f32::consts::FRAC_PI_2),
                UIInterfaceOrientationLandscapeRight => CGAffineTransform::make_rotation(std::f32::consts::FRAC_PI_2),
                _ => unimplemented!(),
            };

            let window_frame: CGRect = msg![env; this frame];
            log_dbg!("Window frame: {window_frame:?}");
            let view_frame: CGRect = msg![env; view frame];
            log_dbg!("Old view frame: {view_frame:?}");

            () = msg![env; view setTransform:transform];

            // Re-apply the view's old frame to compensate for the rotation
            // effectively offseting its center position and changing the size.
            // FIXME: I have no idea if this is how this should be solved, but
            //        it works for DMC4 Refrain at least.

            let view_frame: CGRect = msg![env; view frame];
            log_dbg!("Old view frame after transform: {view_frame:?}");

            () = msg![env; view setFrame:window_frame];

            let view_frame: CGRect = msg![env; view frame];
            log_dbg!("New view frame after re-applying old view frame: {view_frame:?}");
        }
    }
}

- (CGPoint)convertPoint:(CGPoint)point
             fromWindow:(id)other { // UIWindow*
    let this_layer: id = msg![env; this layer];
    // Resolves to nil if other is nil.
    let other_layer: id = msg![env; other layer];
    msg![env; this_layer convertPoint:point fromLayer:other_layer]
}
- (CGPoint)convertPoint:(CGPoint)point
               toWindow:(id)other { // UIWindow*
    let this_layer: id = msg![env; this layer];
    // Resolves to nil if other is nil.
    let other_layer: id = msg![env; other layer];
    msg![env; this_layer convertPoint:point toLayer:other_layer]
}

@end

};

/// Window life-cycle notifications
/// TODO: more notifications
const UIWindowDidBecomeKeyNotification: &str = "UIWindowDidBecomeKeyNotification";
const UIWindowDidBecomeVisibleNotification: &str = "UIWindowDidBecomeVisibleNotification";
const UIWindowDidBecomeHiddenNotification: &str = "UIWindowDidBecomeHiddenNotification";
const UIWindowDidResignKeyNotification: &str = "UIWindowDidResignKeyNotification";
/// Keyboard notifications
/// TODO: more keyboard notifications
pub const UIKeyboardWillShowNotification: &str = "UIKeyboardWillShowNotification";
pub const UIKeyboardDidShowNotification: &str = "UIKeyboardDidShowNotification";
pub const UIKeyboardWillHideNotification: &str = "UIKeyboardWillHideNotification";
pub const UIKeyboardDidHideNotification: &str = "UIKeyboardDidHideNotification";
pub const UIKeyboardBoundsUserInfoKey: &str = "UIKeyboardBoundsUserInfoKey";

pub const CONSTANTS: ConstantExports = &[
    (
        "_UIWindowDidBecomeKeyNotification",
        HostConstant::NSString(UIWindowDidBecomeKeyNotification),
    ),
    (
        "_UIWindowDidBecomeVisibleNotification",
        HostConstant::NSString(UIWindowDidBecomeVisibleNotification),
    ),
    (
        "_UIWindowDidBecomeHiddenNotification",
        HostConstant::NSString(UIWindowDidBecomeHiddenNotification),
    ),
    (
        "_UIWindowDidResignKeyNotification",
        HostConstant::NSString(UIWindowDidResignKeyNotification),
    ),
    // _UIKeyboardWillShowNotification, _UIKeyboardDidShowNotification,
    // _UIKeyboardWillHideNotification, _UIKeyboardDidHideNotification and
    // _UIKeyboardBoundsUserInfoKey are exported from
    // uikit::ui_keyboard::CONSTANTS; not duplicated here.
];
