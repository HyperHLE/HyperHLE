/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UIView`.
//!
//! Useful resources:
//! - Apple's [View Programming Guide for iOS](https://developer.apple.com/library/archive/documentation/WindowsViews/Conceptual/ViewPG_iPhoneOS/Introduction/Introduction.html)

pub mod ui_alert_view;
pub mod ui_control;
pub mod ui_image_view;
pub mod ui_label;
pub mod ui_page_control;
pub mod ui_picker_view;
pub mod ui_scroll_view;
pub mod ui_table_view;
pub mod ui_toolbar;
pub mod ui_web_view;
pub mod ui_window;

use super::ui_graphics::{UIGraphicsPopContext, UIGraphicsPushContext};
use crate::frameworks::core_graphics::cg_affine_transform::CGAffineTransform;
use crate::frameworks::core_graphics::cg_color::CGColorRef;
use crate::frameworks::core_graphics::cg_context::{CGContextClearRect, CGContextRef};
use crate::frameworks::core_graphics::{CGFloat, CGPoint, CGRect, CGSize};
use crate::frameworks::foundation::ns_string::get_static_str;
use crate::frameworks::foundation::{ns_array, NSInteger, NSUInteger};
use crate::mem::MutPtr;
use crate::objc::{
    autorelease, id, msg, msg_class, nil, objc_classes, release, retain, todo_objc_setter, Class,
    ClassExports, HostObject, NSZonePtr, ObjC, SEL,
};
use crate::Environment;

#[derive(Default)]
pub struct State {
    /// List of views for internal purposes. Non-retaining!
    pub(super) views: Vec<id>,
    pub ui_window: ui_window::State,
}

pub(super) struct UIViewHostObject {
    /// CALayer or subclass.
    layer: id,
    /// Subviews in back-to-front order. These are strong references.
    subviews: Vec<id>,
    /// The superview. This is a weak reference.
    superview: id,
    /// The view controller that controls this view. This is a weak reference
    view_controller: id,
    tag: NSInteger,
    clears_context_before_drawing: bool,
    user_interaction_enabled: bool,
    multiple_touch_enabled: bool,
    exclusive_touch: bool,
    delegate: id,
    animation_interval: f64,
    is_animating: bool,
    clips_to_bounds: bool,
}
impl HostObject for UIViewHostObject {}
impl Default for UIViewHostObject {
    fn default() -> UIViewHostObject {
        // The Default trait is implemented so subclasses will get the same
        // defaults.
        UIViewHostObject {
            layer: nil,
            subviews: Vec::new(),
            superview: nil,
            view_controller: nil,
            tag: 0,
            clears_context_before_drawing: true,
            user_interaction_enabled: true,
            multiple_touch_enabled: false,
            exclusive_touch: false,
            delegate: nil,
            animation_interval: 1.0 / 60.0,
            is_animating: false,
            clips_to_bounds: false,
        }
    }
}

pub fn set_view_controller(env: &mut Environment, view: id, controller: id) {
    let host_obj = env.objc.borrow_mut::<UIViewHostObject>(view);
    host_obj.view_controller = controller;
}

/// Shared parts of `initWithCoder:` and `initWithFrame:`. These can't call
/// `init`: the subclass may have overridden `init` and will not expect to be
/// called here.
///
/// Do not call this in subclasses of `UIView`.
fn init_common(env: &mut Environment, this: id) -> id {
    let view_class: Class = msg![env; this class];
    let layer_class: Class = msg![env; view_class layerClass];
    let layer: id = msg![env; layer_class layer];

    // CALayer is not opaque by default, but UIView is
    () = msg![env; layer setDelegate:this];
    () = msg![env; layer setOpaque:true];

    env.objc.borrow_mut::<UIViewHostObject>(this).layer = layer;

    env.framework_state.uikit.ui_view.views.push(this);

    this
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UIView: UIResponder

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::<UIViewHostObject>::default();
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

+ (Class)layerClass {
    env.objc.get_known_class("CALayer", &mut env.mem)
}

// --- Animation Methods ---

+ (())beginAnimations:(id)animationID context:(MutPtr<()>)context {
    log!("TODO: [UIView beginAnimations:{:?} context:{:?}]", animationID, context);
}

+ (())commitAnimations {
    log!("TODO: [UIView commitAnimations]");
}

+ (())setAnimationDuration:(f64)duration {
    log!("TODO: [UIView setAnimationDuration:{}]", duration);
}

+ (())setAnimationCurve:(NSInteger)curve {
    log!("TODO: [UIView setAnimationCurve:{}]", curve);
}

+ (())setAnimationDelegate:(id)delegate {
    log!("TODO: [UIView setAnimationDelegate:{:?}]", delegate);
}

+ (())setAnimationDidStopSelector:(SEL)selector {
    log!("TODO: [UIView setAnimationDidStopSelector:{:?}]", selector);
}

+ (())setAnimationWillStartSelector:(SEL)selector {
    log!("TODO: [UIView setAnimationWillStartSelector:{:?}]", selector);
}

+ (())setAnimationBeginsFromCurrentState:(bool)from {
    log!("TODO: [UIView setAnimationBeginsFromCurrentState:{}]", from);
}

+ (())setAnimationRepeatAutoreverses:(bool)repeatAutoreverses {
    log!("TODO: [UIView setAnimationRepeatAutoreverses:{}]", repeatAutoreverses);
}

+ (())setAnimationRepeatCount:(f32)repeatCount {
    log!("TODO: [UIView setAnimationRepeatCount:{}]", repeatCount);
}

+ (())setAnimationDelay:(f32)delay {
    log!("TODO: [UIView setAnimationDelay:{}]", delay);
}

+ (())setAnimationsEnabled:(f32)enabled {
    log!("TODO: [UIView setAnimationsEnabled:{}]", enabled);
}

+ (())setAnimationTransition:(NSInteger)transition forView:(id)view cache:(bool)cache {
    log!("TODO: [UIView setAnimationTransition:{} forView:{:?} cache:{}]", transition, view, cache);
}

// -------------------------

// initWithCoder: and initWithFrame: are basically UIView's designated
// initializers. init is not, it's a shortcut for the latter.
// Subclasses need to override both.

- (id)init {
    msg![env; this initWithFrame:(<CGRect as Default>::default())]
}

- (id)initWithFrame:(CGRect)frame {
    let this = init_common(env, this);

    () = msg![env; this setFrame:frame];

    log_dbg!(
        "[(UIView*){:?} initWithFrame:{:?}] => bounds {:?}, center {:?}",
        this,
        frame,
        { let bounds: CGRect = msg![env; this bounds]; bounds },
        { let center: CGPoint = msg![env; this center]; center },
    );

    this
}

// NSCoding implementation
- (id)initWithCoder:(id)coder {
    let this = init_common(env, this);

    // TODO: decode the various other UIView properties

    let key_ns_string = get_static_str(env, "UIBounds");
    let bounds: CGRect = msg![env; coder decodeCGRectForKey:key_ns_string];

    let key_ns_string = get_static_str(env, "UICenter");
    let center: CGPoint = msg![env; coder decodeCGPointForKey:key_ns_string];

    let key_ns_string = get_static_str(env, "UIHidden");
    let hidden: bool = msg![env; coder decodeBoolForKey:key_ns_string];

    let key_ns_string = get_static_str(env, "UIOpaque");
    let opaque: bool = msg![env; coder decodeBoolForKey:key_ns_string];

    let key_ns_string = get_static_str(env, "UIBackgroundColor");
    let bg_color: id = msg![env; coder decodeObjectForKey:key_ns_string];

    let key_ns_string = get_static_str(env, "UITag");
    let tag: NSInteger = msg![env; coder decodeIntegerForKey:key_ns_string];

    let key_ns_string = get_static_str(env, "UIMultipleTouchEnabled");
    let multi_touch_enabled: bool = msg![env; coder decodeBoolForKey:key_ns_string];

    let key_ns_string = get_static_str(env, "UISubviews");
    let subviews: id = msg![env; coder decodeObjectForKey:key_ns_string];
    let subview_count: NSUInteger = msg![env; subviews count];

    log_dbg!(
        "[(UIView*){:?} initWithCoder:{:?}] => bounds {}, center {}, hidden {}, bg color {:?}, tag {}, opaque {}, multi touch enabled {}, {} subviews",
        this,
        coder,
        bounds,
        center,
        hidden,
        bg_color,
        tag,
        opaque,
        multi_touch_enabled,
        subview_count,
    );

    // ФИКС ДЛЯ MINECRAFT: Если фрейм нулевой, берем экран
    if bounds.size.width == 0.0 || bounds.size.height == 0.0 {
        let screen: id = msg_class![env; UIScreen mainScreen];
        let screen_bounds: CGRect = msg![env; screen bounds];
        () = msg![env; this setBounds:screen_bounds];
        
        let new_center = CGPoint { 
            x: screen_bounds.size.width / 2.0, 
            y: screen_bounds.size.height / 2.0 
        };
        () = msg![env; this setCenter:new_center];
    } else {
        () = msg![env; this setBounds:bounds];
        () = msg![env; this setCenter:center];
    }

    () = msg![env; this setHidden:hidden];
    () = msg![env; this setOpaque:opaque];
    () = msg![env; this setBackgroundColor:bg_color];
    () = msg![env; this setTag:tag];
    () = msg![env; this setMultipleTouchEnabled:multi_touch_enabled];

    for i in 0..subview_count {
        let subview: id = msg![env; subviews objectAtIndex:i];
        () = msg![env; this addSubview:subview];
    }

    this
}

- (NSInteger)tag {
    env.objc.borrow::<UIViewHostObject>(this).tag
}
- (())setTag:(NSInteger)tag {
    env.objc.borrow_mut::<UIViewHostObject>(this).tag = tag;
}

- (f64)animationInterval {
    env.objc.borrow::<UIViewHostObject>(this).animation_interval
}

- (())setAnimationInterval:(f64)interval {
    env.objc.borrow_mut::<UIViewHostObject>(this).animation_interval = interval;
}
    
- (id)delegate {
    env.objc.borrow::<UIViewHostObject>(this).delegate
}
- (())setDelegate:(id)delegate {
    env.objc.borrow_mut::<UIViewHostObject>(this).delegate = delegate;
}

- (id)viewWithTag:(NSInteger)tag {
    let &UIViewHostObject {
        ref subviews,
        tag: view_tag,
        ..
    } = env.objc.borrow(this);
    if view_tag == tag {
        return this;
    }
    for view in subviews {
        if env.objc.borrow::<UIViewHostObject>(*view).tag == tag {
            return *view;
        }
    }
    nil
}

- (bool)isUserInteractionEnabled {
    env.objc.borrow::<UIViewHostObject>(this).user_interaction_enabled
}
- (())setUserInteractionEnabled:(bool)enabled {
    env.objc.borrow_mut::<UIViewHostObject>(this).user_interaction_enabled = enabled;
}

- (bool)isAnimating {
    env.objc.borrow::<UIViewHostObject>(this).is_animating
}

- (())startAnimation {
    let mut host = env.objc.borrow_mut::<UIViewHostObject>(this);
    if !host.is_animating {
        host.is_animating = true;
        // Примечание: В оригинальном коде iOS здесь создается NSTimer, который 
        // дергает метод drawView. В эмуляторе цикл рендеринга OpenGL часто 
        // работает на уровне самого эмулятора, поэтому честного переключения 
        // внутреннего state (is_animating) достаточно для корректной работы логики игры.
    }
}

- (())stopAnimation {
    let mut host = env.objc.borrow_mut::<UIViewHostObject>(this);
    if host.is_animating {
        host.is_animating = false;
    }
}

- (bool)isMultipleTouchEnabled {
    env.objc.borrow::<UIViewHostObject>(this).multiple_touch_enabled
}
- (())setMultipleTouchEnabled:(bool)enabled {
    env.objc.borrow_mut::<UIViewHostObject>(this).multiple_touch_enabled = enabled;
}

- (bool)isExclusiveTouch {
    env.objc.borrow::<UIViewHostObject>(this).exclusive_touch
}

- (())setExclusiveTouch:(bool)exclusive {
    env.objc.borrow_mut::<UIViewHostObject>(this).exclusive_touch = exclusive;
}

- (())layoutSubviews {
    // On iOS 5.1 and earlier, the default implementation of this method does
    // nothing.
}

- (id)superview {
    env.objc.borrow::<UIViewHostObject>(this).superview
}

- (id)window {
    // Looks up window in the superview hierarchy
    let mut window: id = env.objc.borrow::<UIViewHostObject>(this).superview;
    let window_class = env.objc.get_known_class("UIWindow", &mut env.mem);
    while window != nil {
        let current_class: Class = msg![env; window class];
        if env.objc.class_is_subclass_of(current_class, window_class) {
            break;
        }
        window = env.objc.borrow::<UIViewHostObject>(window).superview;
    }
    window
}

- (id)subviews {
    let views = env.objc.borrow::<UIViewHostObject>(this).subviews.clone();
    for view in &views {
        retain(env, *view);
    }
    let subs = ns_array::from_vec(env, views);
    autorelease(env, subs)
}

- (())addSubview:(id)view {
    if view == nil {
        log_dbg!("Tolerating [(UIView*){:?} addSubview:nil]", this);
        return;
    }

    if env.objc.borrow::<UIViewHostObject>(view).superview == this {
        () = msg![env; this bringSubviewToFront:view];
    } else {
        retain(env, view);
        () = msg![env; view removeFromSuperview];
        let subview_obj = env.objc.borrow_mut::<UIViewHostObject>(view);
        subview_obj.superview = this;
        let subview_layer = subview_obj.layer;
        let this_obj = env.objc.borrow_mut::<UIViewHostObject>(this);
        this_obj.subviews.push(view);
        let this_layer = this_obj.layer;
        () = msg![env; this_layer addSublayer:subview_layer];
    }
}

- (())insertSubview:(id)view atIndex:(NSInteger)index {
    assert!(view != nil);
    retain(env, view);
    () = msg![env; view removeFromSuperview];

    let subview_obj = env.objc.borrow_mut::<UIViewHostObject>(view);
    subview_obj.superview = this;
    let subview_layer = subview_obj.layer;

    let &mut UIViewHostObject {
        ref mut subviews,
        layer: this_layer,
        ..
    } = env.objc.borrow_mut(this);

    subviews.insert(index as usize, view);

    assert!(index >= 0);
    () = msg![env; this_layer insertSublayer:subview_layer atIndex:(index as u32)];
}

- (())insertSubview:(id)view belowSubview:(id)sibling {
    retain(env, view);
    () = msg![env; view removeFromSuperview];

    let subview_obj = env.objc.borrow_mut::<UIViewHostObject>(view);
    subview_obj.superview = this;
    let subview_layer = subview_obj.layer;

    let sibling_layer = env.objc.borrow_mut::<UIViewHostObject>(sibling).layer;

    let &mut UIViewHostObject {
        ref mut subviews,
        layer: this_layer,
        ..
    } = env.objc.borrow_mut(this);

    let idx = subviews.iter().position(|&subview2| subview2 == sibling).unwrap();
    subviews.insert(idx, view);

    () = msg![env; this_layer insertSublayer:subview_layer below:sibling_layer];
}

- (())insertSubview:(id)view aboveSubview:(id)sibling {
    if view == nil { return; }
    retain(env, view);
    let _: () = msg![env; view removeFromSuperview];

    let subview_layer = env.objc.borrow::<UIViewHostObject>(view).layer;

    let sibling_idx = env.objc
        .borrow::<UIViewHostObject>(this)
        .subviews
        .iter()
        .position(|&s| s == sibling);

    let insert_idx = match sibling_idx {
        Some(idx) => idx + 1, // one above the sibling
        None => {
            // Sibling not found — insert at top (same as addSubview).
            env.objc.borrow::<UIViewHostObject>(this).subviews.len()
        }
    };

    env.objc.borrow_mut::<UIViewHostObject>(view).superview = this;
    env.objc
        .borrow_mut::<UIViewHostObject>(this)
        .subviews
        .insert(insert_idx, view);

    let this_layer = env.objc.borrow::<UIViewHostObject>(this).layer;
    let sibling_layer = if sibling != nil {
        env.objc.borrow::<UIViewHostObject>(sibling).layer
    } else {
        crate::objc::nil
    };

    if sibling_layer != crate::objc::nil {
        let _: () = msg![env; this_layer insertSublayer:subview_layer above:sibling_layer];
    } else {
        let _: () = msg![env; this_layer addSublayer:subview_layer];
    }
}

- (())bringSubviewToFront:(id)subview {
    if subview == nil {
        // This happens in Touch & Go LITE. It's probably due to the ad classes
        // being replaced with fakes.
        log_dbg!("Tolerating [{:?} bringSubviewToFront:nil]", this);
        return;
    }

    let &mut UIViewHostObject {
        ref mut subviews,
        layer,
        ..
    } = env.objc.borrow_mut(this);

    let Some(idx) = subviews.iter().position(|&subview2| subview2 == subview) else {
        log_dbg!("Warning: Unable to find the subview {:?} in subviews of {:?}", subview, this);
        return;
    };
    let subview2 = subviews.remove(idx);
    assert!(subview2 == subview);
    subviews.push(subview);

    let subview_layer = env.objc.borrow::<UIViewHostObject>(subview).layer;
    () = msg![env; subview_layer removeFromSuperlayer];
    () = msg![env; layer addSublayer:subview_layer];
}

- (())sendSubviewToBack:(id)subview {
    if subview == nil {
        log_dbg!("Tolerating [{:?} sendSubviewToBack:nil]", this);
        return;
    }

    let &mut UIViewHostObject {
        ref mut subviews,
        layer,
        ..
    } = env.objc.borrow_mut(this);

    let Some(idx) = subviews.iter().position(|&subview2| subview2 == subview) else {
        log_dbg!("Warning: Unable to find the subview {:?} in subviews of {:?}", subview, this);
        return;
    };
    let subview2 = subviews.remove(idx);
    assert!(subview2 == subview);
    subviews.insert(0, subview);

    let subview_layer = env.objc.borrow::<UIViewHostObject>(subview).layer;
    () = msg![env; subview_layer removeFromSuperlayer];
    () = msg![env; layer insertSublayer:subview_layer atIndex:0u32];
}

- (())removeFromSuperview {
    let &mut UIViewHostObject {
        ref mut superview,
        layer: this_layer,
        ..
    } = env.objc.borrow_mut(this);
    let superview = std::mem::take(superview);
    if superview == nil {
        return;
    }
    let _: () = msg![env; this_layer removeFromSuperlayer];

    let UIViewHostObject { ref mut subviews, .. } = env.objc.borrow_mut(superview);
    if let Some(idx) = subviews.iter().position(|&subview| subview == this) {
        let subview = subviews.remove(idx);
        assert!(subview == this);
        release(env, this);
    } else {
        log_dbg!(
            "Warning: [UIView removeFromSuperview] {:?} not found in superview's subviews — already removed?",
            this
        );
    }
}

- (())dealloc {
    let UIViewHostObject {
        layer,
        superview: _,
        subviews,
        view_controller,
        ..
    } = std::mem::take(env.objc.borrow_mut(this));

    release(env, layer);
    assert!(view_controller == nil);
    for subview in subviews {
        env.objc.borrow_mut::<UIViewHostObject>(subview).superview = nil;
        release(env, subview);
    }

    let state = &mut env.framework_state.uikit.ui_view.views;
    state.swap_remove(
        state.iter().position(|&v| v == this).unwrap()
    );

    env.objc.dealloc_object(this, &mut env.mem);
}

- (id)layer {
    env.objc.borrow_mut::<UIViewHostObject>(this).layer
}

- (bool)isHidden {
    let layer = env.objc.borrow::<UIViewHostObject>(this).layer;
    msg![env; layer isHidden]
}
- (())setHidden:(bool)hidden {
    let layer = env.objc.borrow::<UIViewHostObject>(this).layer;
    msg![env; layer setHidden:hidden]
}

- (bool)clipsToBounds {
    env.objc.borrow::<UIViewHostObject>(this).clips_to_bounds
}
- (())setClipsToBounds:(bool)clips {
    env.objc.borrow_mut::<UIViewHostObject>(this).clips_to_bounds = clips;
}

// --- ДОБАВЛЕННЫЙ ХАК ДЛЯ FBLoginButton ---
- (())setStyle:(u32)_style {
    // Заглушка, чтобы эмулятор не падал при настройке фейковых элементов (например, FBLoginButton)
}
// -----------------------------------------

// --- ДОБАВЛЕННЫЙ ХАК ДЛЯ EAGLView ---
- (id)context {
    nil // Возвращаем пустоту, так как настоящего контекста у обычного UIView нет
}

- (())setContext:(id)_context {
    // Ничего не делаем, просто игнорируем попытку игры передать нам контекст
}
// ------------------------------------

// =========================================================================
// MARK: - OpenGL ES / EAGLView lifecycle stubs
// These are called by apps that subclass UIView as an EAGLView.
// =========================================================================

- (())resume {
    log_dbg!("UIView resume {:?}", this);
    let mut host = env.objc.borrow_mut::<UIViewHostObject>(this);
    host.is_animating = true;
}

- (())flushBuffer {
    // Called by some EAGLView implementations after rendering a frame to
    // present the renderbuffer. The actual present is handled by EAGLContext
    // presentRenderBuffer: — this is just a hook some apps call before that.
    log_dbg!("UIView flushBuffer {:?}", this);
    let layer = env.objc.borrow::<UIViewHostObject>(this).layer;
    // Forward to the layer's display if it has content to present.
    let _: () = msg![env; layer display];
}

- (())setupView {
    // Called by EAGLView subclasses to set up the OpenGL ES state
    // (viewport, projection matrix, etc.) before rendering begins.
    // The actual GL setup is done by the app's own override; the base
    // UIView implementation is a no-op.
    log_dbg!("UIView setupView {:?}", this);
}

- (())endDrawing {
    // Called by some EAGLView implementations at the end of a render pass.
    // No-op at the UIView level — the app's override does the real work.
    log_dbg!("UIView endDrawing {:?}", this);
}

- (bool)isOpaque {
    let layer = env.objc.borrow::<UIViewHostObject>(this).layer;
    msg![env; layer isOpaque]
}
- (())setOpaque:(bool)opaque {
    let layer = env.objc.borrow::<UIViewHostObject>(this).layer;
    msg![env; layer setOpaque:opaque]
}

- (CGFloat)alpha {
    let layer = env.objc.borrow::<UIViewHostObject>(this).layer;
    msg![env; layer opacity]
}
- (())setAlpha:(CGFloat)alpha {
    let layer = env.objc.borrow::<UIViewHostObject>(this).layer;
    msg![env; layer setOpacity:alpha]
}

- (CGFloat)contentScaleFactor {
    1.0
}

- (())setContentScaleFactor:(CGFloat)_scale {
    // Заглушка, чтобы не крашилось
}

- (id)backgroundColor {
    let layer = env.objc.borrow::<UIViewHostObject>(this).layer;
    let cg_color: CGColorRef = msg![env; layer backgroundColor];
    msg_class![env; UIColor colorWithCGColor:cg_color]
}
- (())setBackgroundColor:(id)color { // UIColor*
    let color: CGColorRef = msg![env; color CGColor];
    let layer = env.objc.borrow::<UIViewHostObject>(this).layer;
    msg![env; layer setBackgroundColor:color]
}

// TODO: support setNeedsDisplayInRect:
- (())setNeedsDisplay {
    // UIView has a method called drawRect: that subclasses override if they
    // need custom drawing. touchHLE's UIView (a CALayerDelegate) provides
    // an implementation of drawLayer:inContext: that calls drawRect:.
    // This maintains a clean separation of UIView and CALayer.
    //
    // To avoid wasting space and time on unnecessary bitmaps and drawing,
    // let's optimize here by only marking the layer as needing display if
    // the UIView's subclass overrides drawRect: or drawLayer:inContext:.
    let this_class = ObjC::read_isa(this, &env.mem);

    let ui_view_class = env.objc.get_known_class("UIView", &mut env.mem);

    let draw_layer_sel = env.objc.lookup_selector("drawLayer:inContext:").unwrap();
    let draw_rect_sel = env.objc.lookup_selector("drawRect:").unwrap();

    if env
        .objc
        .class_overrides_method_of_superclass(this_class, draw_rect_sel, ui_view_class)
        || env
            .objc
            .class_overrides_method_of_superclass(this_class, draw_layer_sel, ui_view_class)
    {
        let layer = env.objc.borrow::<UIViewHostObject>(this).layer;
        msg![env; layer setNeedsDisplay]
    }
}

- (())setNeedsLayout {

}

- (CGRect)bounds {
    let layer = env.objc.borrow::<UIViewHostObject>(this).layer;
    msg![env; layer bounds]
}
- (())setBounds:(CGRect)bounds {
    let layer = env.objc.borrow::<UIViewHostObject>(this).layer;
    msg![env; layer setBounds:bounds]
}
- (CGPoint)center {
    // FIXME: what happens if [layer anchorPoint] isn't (0.5, 0.5)?
    let layer = env.objc.borrow::<UIViewHostObject>(this).layer;
    msg![env; layer position]
}
- (())setCenter:(CGPoint)center {
    let layer = env.objc.borrow::<UIViewHostObject>(this).layer;
    msg![env; layer setPosition:center]
}
- (CGRect)frame {
    let layer = env.objc.borrow::<UIViewHostObject>(this).layer;
    msg![env; layer frame]
}
- (())setFrame:(CGRect)frame {
    let layer = env.objc.borrow::<UIViewHostObject>(this).layer;
    msg![env; layer setFrame:frame]
}
- (CGAffineTransform)transform {
    let layer = env.objc.borrow::<UIViewHostObject>(this).layer;
    msg![env; layer affineTransform]
}
- (())setTransform:(CGAffineTransform)transform {
    let layer = env.objc.borrow::<UIViewHostObject>(this).layer;
    msg![env; layer setAffineTransform:transform]
}

- (())setContentMode:(NSInteger)content_mode { // should be UIViewContentMode
    todo_objc_setter!(this, content_mode);
}

- (bool)clearsContextBeforeDrawing {
    env.objc.borrow::<UIViewHostObject>(this).clears_context_before_drawing
}
- (())setClearsContextBeforeDrawing:(bool)v {
    env.objc.borrow_mut::<UIViewHostObject>(this).clears_context_before_drawing = v;
}

// Drawing stuff that views should override
- (())drawRect:(CGRect)_rect {
    // default implementation does nothing
}

// CALayerDelegate implementation
- (())drawLayer:(id)layer // CALayer*
      inContext:(CGContextRef)context {
    let mut bounds: CGRect = msg![env; layer bounds];
    bounds.origin = CGPoint { x: 0.0, y: 0.0 }; // FIXME: not tested
    if env.objc.borrow::<UIViewHostObject>(this).clears_context_before_drawing {
        CGContextClearRect(env, context, bounds);
    }
    UIGraphicsPushContext(env, context);
    () = msg![env; this drawRect:bounds];
    UIGraphicsPopContext(env);
}

// Event handling

- (bool)pointInside:(CGPoint)point
          withEvent:(id)_event { // UIEvent* (possibly nil)
    let layer = env.objc.borrow::<UIViewHostObject>(this).layer;
    msg![env; layer containsPoint:point]
}

- (id)hitTest:(CGPoint)point
    withEvent:(id)event { // UIEvent* (possibly nil)
    
    // ХАК: Запоминаем, попал ли тап в саму вьюшку, но НЕ ВЫХОДИМ, если нет!
    // В портированных играх рамки (bounds) часто кривые, из-за чего клики теряются.
    let is_inside: bool = msg![env; this pointInside:point withEvent:event];

    // TODO: avoid copy somehow?
    let subviews = env.objc.borrow::<UIViewHostObject>(this).subviews.clone();
    
    // Сначала принудительно опрашиваем всех детей (кнопки, картинки)
    for subview in subviews.into_iter().rev() { // later views are on top
        let hidden: bool = msg![env; subview isHidden];
        let alpha: CGFloat = msg![env; subview alpha];
        let interactible: bool = msg![env; subview isUserInteractionEnabled];
        if hidden || alpha < 0.01 || !interactible {
           continue;
        }
        let sub_point: CGPoint = msg![env; subview convertPoint:point fromView:this];
        let subview_hit: id = msg![env; subview hitTest:sub_point withEvent:event];
        
        // Если ребенок поймал клик — отдаем его ему, игнорируя кривые рамки родителя
        if subview_hit != nil {
            return subview_hit;
        }
    }
    
    // Если ни один ребенок не поймал тап, проверяем честный pointInside
    if is_inside {
        this
    } else {
        nil
    }
}

// Ending a view-editing session

- (bool)endEditing:(bool)force {
    assert!(force);
    let responder: id = env.framework_state.uikit.ui_responder.first_responder;
    let class = msg![env; responder class];
    let ui_text_field_class = env.objc.get_known_class("UITextField", &mut env.mem);
    if responder != nil && env.objc.class_is_subclass_of(class, ui_text_field_class) {
        // we need to check if text field is in the current view hierarchy
        let mut to_find = responder;
        while to_find != nil {
            if to_find == this {
                return msg![env; responder resignFirstResponder];
            }
            to_find = msg![env; to_find superview];
        }
    }
    false
}

// UIResponder implementation
// From the Apple UIView docs regarding [UIResponder nextResponder]:
// "UIView implements this method and returns the UIViewController object that
//  manages it (if it has one) or its superview (if it doesn’t)."
- (id)nextResponder {
    let host_object = env.objc.borrow::<UIViewHostObject>(this);
    if host_object.view_controller != nil {
        host_object.view_controller
    } else {
        host_object.superview
    }
}

// Co-ordinate space conversion

- (CGPoint)convertPoint:(CGPoint)point
               fromView:(id)other { // UIView*
    if other == nil {
        let window: id = msg![env; this window];
        if window == nil { return point; }
        return msg![env; this convertPoint:point fromView:window]
    }
    
    let view_class: id = msg_class![env; UIView class];
    let is_view: bool = msg![env; other isKindOfClass:view_class];
    let actual_other = if is_view {
        other
    } else {
        let mut found_view = nil;
        if let Some(sel_view) = env.objc.lookup_selector("view") {
            let responds: bool = msg![env; other respondsToSelector:sel_view];
            if responds { found_view = msg![env; other view]; }
        }
        found_view
    };
    
    if actual_other == nil { return point; }

    let this_layer = env.objc.borrow::<UIViewHostObject>(this).layer;
    let other_layer = env.objc.borrow::<UIViewHostObject>(actual_other).layer;
    msg![env; this_layer convertPoint:point fromLayer:other_layer]
}
- (CGPoint)convertPoint:(CGPoint)point
                 toView:(id)other { // UIView*
    if other == nil {
        let window: id = msg![env; this window];
        if window == nil { return point; }
        return msg![env; this convertPoint:point toView:window]
    }
    
    // Честная проверка: является ли other UIView
    let view_class: id = msg_class![env; UIView class];
    let is_view: bool = msg![env; other isKindOfClass:view_class];
    let actual_other = if is_view {
        other
    } else {
        // Если передали не UIView (например, UIViewController), пробуем достать его view
        let mut found_view = nil;
        if let Some(sel_view) = env.objc.lookup_selector("view") {
            let responds: bool = msg![env; other respondsToSelector:sel_view];
            if responds { found_view = msg![env; other view]; }
        }
        found_view
    };
    
    // Если объект абсолютно несовместим (например, UIAlertView), отдаем точку как есть
    if actual_other == nil { return point; }

    let this_layer = env.objc.borrow::<UIViewHostObject>(this).layer;
    let other_layer = env.objc.borrow::<UIViewHostObject>(actual_other).layer;
    msg![env; this_layer convertPoint:point toLayer:other_layer]
}
- (CGRect)convertRect:(CGRect)rect
             fromView:(id)other { // UIView*
    if other == nil {
        let window: id = msg![env; this window];
        if window == nil { return rect; }
        return msg![env; this convertRect:rect fromView:window]
    }
    
    let view_class: id = msg_class![env; UIView class];
    let is_view: bool = msg![env; other isKindOfClass:view_class];
    let actual_other = if is_view {
        other
    } else {
        let mut found_view = nil;
        if let Some(sel_view) = env.objc.lookup_selector("view") {
            let responds: bool = msg![env; other respondsToSelector:sel_view];
            if responds { found_view = msg![env; other view]; }
        }
        found_view
    };
    
    if actual_other == nil { return rect; }

    let this_layer = env.objc.borrow::<UIViewHostObject>(this).layer;
    let other_layer = env.objc.borrow::<UIViewHostObject>(actual_other).layer;
    msg![env; this_layer convertRect:rect fromLayer:other_layer]
}
- (CGRect)convertRect:(CGRect)rect
               toView:(id)other { // UIView*
    if other == nil {
        let window: id = msg![env; this window];
        if window == nil { return rect; }
        return msg![env; this convertRect:rect toView:window]
    }
    
    let view_class: id = msg_class![env; UIView class];
    let is_view: bool = msg![env; other isKindOfClass:view_class];
    let actual_other = if is_view {
        other
    } else {
        let mut found_view = nil;
        if let Some(sel_view) = env.objc.lookup_selector("view") {
            let responds: bool = msg![env; other respondsToSelector:sel_view];
            if responds { found_view = msg![env; other view]; }
        }
        found_view
    };
    
    if actual_other == nil { return rect; }

    let this_layer = env.objc.borrow::<UIViewHostObject>(this).layer;
    let other_layer = env.objc.borrow::<UIViewHostObject>(actual_other).layer;
    msg![env; this_layer convertRect:rect toLayer:other_layer]
}

- (())setAutoresizingMask:(NSUInteger)mask {
    todo_objc_setter!(this, mask);
}
- (())setAutoresizesSubviews:(bool)enabled {
    todo_objc_setter!(this, enabled);
}

- (CGSize)sizeThatFits:(CGSize)size {
    // default implementation, subclasses can override
    size
}
- (())sizeToFit {
    let bounds: CGRect = msg![env; this bounds];
    let size: CGSize = bounds.size;
    let new_size: CGSize = msg![env; this sizeThatFits:size];
    () = msg![env; this setBounds:(CGRect { origin: CGPoint::default(), size: new_size })];
}

@end

};

}
