/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 * If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//!
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
use crate::frameworks::foundation::ns_dictionary::dict_from_keys_and_objects;
use crate::frameworks::foundation::ns_string::{from_rust_string, get_static_str, to_rust_string};
use crate::frameworks::foundation::{ns_array, NSInteger, NSUInteger};
use crate::mem::MutPtr;
use crate::objc::{
    autorelease, id, msg, msg_class, msg_send_no_type_checking, nil, objc_classes, release, retain,
    Class, ClassExports, HostObject, NSZonePtr, ObjC, SEL,
};
use crate::Environment;

/// State maintained for UIView's class-level animation block API
/// (`+beginAnimations:context:` ... `+commitAnimations`). At most one block
/// may be open at a time on the main thread; if a new `beginAnimations:` is
/// received before the previous block was committed, the in-progress state is
/// discarded.
pub(super) struct AnimationBlockState {
    pub(super) in_block: bool,
    pub(super) animation_id: id,
    pub(super) context: MutPtr<()>,
    pub(super) duration: f64,
    pub(super) delay: f64,
    /// Per Apple's docs the delegate is **not** retained while inside an
    /// animation block. We do retain it temporarily here so that we can hold
    /// onto it until the (asynchronous) completion fires.
    pub(super) delegate: id,
    pub(super) did_stop_selector: Option<SEL>,
    pub(super) will_start_selector: Option<SEL>,
}
impl Default for AnimationBlockState {
    fn default() -> AnimationBlockState {
        AnimationBlockState {
            in_block: false,
            animation_id: nil,
            context: MutPtr::from_bits(0),
            duration: 0.2, // UIKit default
            delay: 0.0,
            delegate: nil,
            did_stop_selector: None,
            will_start_selector: None,
        }
    }
}

#[derive(Default)]
pub struct State {
    pub(super) views: Vec<id>,
    pub ui_window: ui_window::State,
    pub(super) animation_block: AnimationBlockState,
}

pub(super) struct UIViewHostObject {
    layer: id,
    subviews: Vec<id>,
    superview: id,
    view_controller: id,
    tag: NSInteger,
    content_mode: NSInteger,
    autoresizing_mask: NSUInteger,
    autoresizes_subviews: bool,
    clears_context_before_drawing: bool,
    user_interaction_enabled: bool,
    multiple_touch_enabled: bool,
    exclusive_touch: bool,
    delegate: id,
    animation_interval: f64,
    is_animating: bool,
    clips_to_bounds: bool,
    is_uncontrolled: bool,
}
impl HostObject for UIViewHostObject {}
impl Default for UIViewHostObject {
    fn default() -> UIViewHostObject {
        UIViewHostObject {
            layer: nil,
            subviews: Vec::new(),
            superview: nil,
            view_controller: nil,
            tag: 0,
            content_mode: 0,      // UIViewContentModeScaleToFill
            autoresizing_mask: 0, // UIViewAutoresizingNone
            autoresizes_subviews: true,
            clears_context_before_drawing: true,
            user_interaction_enabled: true,
            multiple_touch_enabled: false,
            exclusive_touch: false,
            delegate: nil,
            animation_interval: 1.0 / 60.0,
            is_animating: false,
            clips_to_bounds: false,
            is_uncontrolled: false,
        }
    }
}

pub fn set_view_controller(env: &mut Environment, view: id, controller: id) {
    let mut host_obj = env.objc.borrow_mut::<UIViewHostObject>(view);
    host_obj.view_controller = controller;
}

fn init_common(env: &mut Environment, this: id) -> id {
    let view_class: Class = msg![env; this class];
    let layer_class: Class = msg![env; view_class layerClass];
    let layer: id = msg![env; layer_class layer];
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

+ (Class)layerClass { env.objc.get_known_class("CALayer", &mut env.mem) }

// MARK: - Class-level animation block API
//
// touchHLE does not currently animate the visual side of these UIView
// animation blocks (positions/opacity/transforms snap immediately to their
// final value). However, a correct implementation **must** still call the
// configured `setAnimationDidStopSelector:` on the configured
// `setAnimationDelegate:` once the would-be animation finishes, otherwise
// games that drive their state machine off animation completion callbacks
// (very common — e.g. fade-in/fade-out transitions, splash → menu hand-offs)
// hang forever waiting for the callback. We therefore record the parameters
// of each block and schedule a one-shot NSTimer at `commitAnimations` that
// fires the callback after `delay + duration` seconds.

+ (())beginAnimations:(id)animationID context:(MutPtr<()>)context {
    let block = std::mem::take(&mut env.framework_state.uikit.ui_view.animation_block);
    // If a previous block was opened but never committed, drop it. This
    // matches what apps typically expect: starting a new block resets state.
    if block.in_block {
        log_dbg!(
            "Warning: nested/uncommitted UIView animation block discarded \
             (animationID={:?}, delegate={:?})",
            block.animation_id,
            block.delegate
        );
        if block.delegate != nil { release(env, block.delegate); }
        if block.animation_id != nil { release(env, block.animation_id); }
    }
    if animationID != nil { let _: id = msg![env; animationID retain]; }
    env.framework_state.uikit.ui_view.animation_block = AnimationBlockState {
        in_block: true,
        animation_id: animationID,
        context,
        ..Default::default()
    };
}

+ (())setAnimationDuration:(f64)duration {
    let block = &mut env.framework_state.uikit.ui_view.animation_block;
    if block.in_block { block.duration = duration.max(0.0); }
}

+ (())setAnimationDelay:(f32)delay {
    let block = &mut env.framework_state.uikit.ui_view.animation_block;
    if block.in_block { block.delay = (delay as f64).max(0.0); }
}

+ (())setAnimationDelegate:(id)delegate {
    if !env.framework_state.uikit.ui_view.animation_block.in_block { return; }
    let prev = env.framework_state.uikit.ui_view.animation_block.delegate;
    if delegate != nil { let _: id = msg![env; delegate retain]; }
    env.framework_state.uikit.ui_view.animation_block.delegate = delegate;
    if prev != nil { release(env, prev); }
}

+ (())setAnimationDidStopSelector:(SEL)selector {
    let block = &mut env.framework_state.uikit.ui_view.animation_block;
    if !block.in_block { return; }
    block.did_stop_selector = if selector.is_null() { None } else { Some(selector) };
}

+ (())setAnimationWillStartSelector:(SEL)selector {
    let block = &mut env.framework_state.uikit.ui_view.animation_block;
    if !block.in_block { return; }
    block.will_start_selector = if selector.is_null() { None } else { Some(selector) };
}

+ (())commitAnimations {
    let block = std::mem::take(&mut env.framework_state.uikit.ui_view.animation_block);
    if !block.in_block { return; }

    // Fire `setAnimationWillStartSelector:` synchronously. This is good enough
    // for the apps that touchHLE supports; iOS would normally fire it at the
    // start of the next display frame.
    if block.delegate != nil {
        if let Some(sel) = block.will_start_selector {
            let _: () = msg_send_no_type_checking(
                env,
                (block.delegate, sel, block.animation_id, block.context),
            );
        }
    }

    // Schedule the `setAnimationDidStopSelector:` callback. Even when the
    // delegate is nil we still need to release the retained animation_id, so
    // the early-return paths below take care of that.
    let total_delay = (block.delay + block.duration).max(0.0);

    if block.delegate == nil || block.did_stop_selector.is_none() {
        if block.delegate != nil { release(env, block.delegate); }
        if block.animation_id != nil { release(env, block.animation_id); }
        return;
    }

    let did_stop_selector = block.did_stop_selector.unwrap();
    let sel_name = did_stop_selector.as_str(&env.mem).to_string();
    let sel_str: id = from_rust_string(env, sel_name);

    // Pack the raw context pointer in an NSNumber so it can survive a trip
    // through `userInfo`.
    let context_bits = block.context.to_bits();
    let context_num: id = msg_class![env; NSNumber numberWithUnsignedInt:context_bits];

    let key_delegate: id = get_static_str(env, "_touchHLE_uiview_anim_delegate");
    let key_sel: id = get_static_str(env, "_touchHLE_uiview_anim_sel");
    let key_anim_id: id = get_static_str(env, "_touchHLE_uiview_anim_id");
    let key_context: id = get_static_str(env, "_touchHLE_uiview_anim_context");

    // NSDictionary cannot store nil values; substitute NSNull for a missing
    // animationID.
    let anim_id_obj: id = if block.animation_id == nil {
        msg_class![env; NSNull null]
    } else {
        block.animation_id
    };

    let dict: id = dict_from_keys_and_objects(
        env,
        &[
            (key_delegate, block.delegate),
            (key_sel, sel_str),
            (key_anim_id, anim_id_obj),
            (key_context, context_num),
        ],
    );

    let fire_sel = env.objc.lookup_selector("_touchHLE_animationDidStopFireMethod:")
        .expect("UIView _touchHLE_animationDidStopFireMethod: not registered");
    let ui_view_class: Class = env.objc.get_known_class("UIView", &mut env.mem);
    let _: id = msg_class![env;
        NSTimer scheduledTimerWithTimeInterval:total_delay
                                       target:ui_view_class
                                     selector:fire_sel
                                     userInfo:dict
                                      repeats:false
    ];

    // The dictionary retains `delegate` and `animation_id`, so release the
    // retains we held in our state struct.
    release(env, block.delegate);
    if block.animation_id != nil { release(env, block.animation_id); }
}

+ (())_touchHLE_animationDidStopFireMethod:(id)which_timer {
    let dict: id = msg![env; which_timer userInfo];
    let key_delegate: id = get_static_str(env, "_touchHLE_uiview_anim_delegate");
    let key_sel: id = get_static_str(env, "_touchHLE_uiview_anim_sel");
    let key_anim_id: id = get_static_str(env, "_touchHLE_uiview_anim_id");
    let key_context: id = get_static_str(env, "_touchHLE_uiview_anim_context");

    let delegate: id = msg![env; dict objectForKey:key_delegate];
    let sel_str_id: id = msg![env; dict objectForKey:key_sel];
    let mut animation_id: id = msg![env; dict objectForKey:key_anim_id];
    let context_num: id = msg![env; dict objectForKey:key_context];

    let null_class: Class = env.objc.get_known_class("NSNull", &mut env.mem);
    if !animation_id.is_null() {
        let id_class: Class = msg![env; animation_id class];
        if id_class == null_class { animation_id = nil; }
    }

    let sel_str = to_rust_string(env, sel_str_id).to_string();
    let Some(sel) = env.objc.lookup_selector(&sel_str) else {
        log!(
            "Warning: animation didStopSelector \"{}\" no longer exists; skipping callback.",
            sel_str
        );
        return;
    };

    let context_bits: u32 = msg![env; context_num unsignedIntValue];
    let context: MutPtr<()> = MutPtr::from_bits(context_bits);
    let finished: bool = true;

    if delegate == nil {
        return;
    }
    let _: () = msg_send_no_type_checking(env, (delegate, sel, animation_id, finished, context));
}

// Visual properties of animation blocks that touchHLE does not animate.
// These are intentionally no-ops, but they must remain present so that the
// app's calls don't fall through to the dynamic dispatcher's "unimplemented
// selector" path.
+ (())setAnimationCurve:(NSInteger)_curve { }
+ (())setAnimationBeginsFromCurrentState:(bool)_from { }
+ (())setAnimationRepeatAutoreverses:(bool)_autoreverses { }
+ (())setAnimationRepeatCount:(f32)_count { }
+ (())setAnimationsEnabled:(f32)_enabled { }
+ (())setAnimationTransition:(NSInteger)_transition forView:(id)_view cache:(bool)_cache { }

- (())setIsUncontrolled:(bool)uncontrolled {
    env.objc.borrow_mut::<UIViewHostObject>(this).is_uncontrolled = uncontrolled;
}

- (id)init {
    msg![env; this initWithFrame:(<CGRect as Default>::default())]
}

- (id)initWithFrame:(CGRect)frame {
    let this = init_common(env, this);
    () = msg![env; this setFrame:frame];
    this
}

- (id)initWithCoder:(id)coder {
    let this = init_common(env, this);

    let key_bounds = get_static_str(env, "UIBounds");
    let key_center = get_static_str(env, "UICenter");
    let key_frame = get_static_str(env, "UIFrame");

    let has_bounds = msg![env; coder containsValueForKey:key_bounds];
    let has_center = msg![env; coder containsValueForKey:key_center];
    let has_frame = msg![env; coder containsValueForKey:key_frame];

    let mut bounds: CGRect = if has_bounds { msg![env; coder decodeCGRectForKey:key_bounds] } else { CGRect::default() };
    let mut center: CGPoint = if has_center { msg![env; coder decodeCGPointForKey:key_center] } else { CGPoint::default() };

    if has_frame {
        let frame: CGRect = msg![env; coder decodeCGRectForKey:key_frame];
        if !has_bounds {
            bounds = CGRect { origin: CGPoint::default(), size: frame.size };
        }
        if !has_center {
            center = CGPoint {
                x: frame.origin.x + frame.size.width / 2.0,
                y: frame.origin.y + frame.size.height / 2.0,
            };
        }
    }

    let key_hidden = get_static_str(env, "UIHidden");
    let hidden: bool = if msg![env; coder containsValueForKey:key_hidden] { msg![env; coder decodeBoolForKey:key_hidden] } else { false };

    let key_opaque = get_static_str(env, "UIOpaque");
    let opaque: bool = if msg![env; coder containsValueForKey:key_opaque] { msg![env; coder decodeBoolForKey:key_opaque] } else { false };

    let key_bg = get_static_str(env, "UIBackgroundColor");
    let bg_color: id = if msg![env; coder containsValueForKey:key_bg] { msg![env; coder decodeObjectForKey:key_bg] } else { nil };

    let key_tag = get_static_str(env, "UITag");
    let tag: NSInteger = if msg![env; coder containsValueForKey:key_tag] { msg![env; coder decodeIntegerForKey:key_tag] } else { 0 };

    let key_content_mode = get_static_str(env, "UIContentMode");
    let content_mode: NSInteger = if msg![env; coder containsValueForKey:key_content_mode] { msg![env; coder decodeIntegerForKey:key_content_mode] } else { 0 };

    let key_autoresizing_mask = get_static_str(env, "UIAutoresizingMask");
    let autoresizing_mask: NSUInteger = if msg![env; coder containsValueForKey:key_autoresizing_mask] {
        let mask: NSInteger = msg![env; coder decodeIntegerForKey:key_autoresizing_mask];
        mask as NSUInteger
    } else {
        0
    };

    let key_autoresizes_subviews = get_static_str(env, "UIAutoresizesSubviews");
    let autoresizes_subviews: bool = if msg![env; coder containsValueForKey:key_autoresizes_subviews] { msg![env; coder decodeBoolForKey:key_autoresizes_subviews] } else { true };

    let key_multi_touch = get_static_str(env, "UIMultipleTouchEnabled");
    let multi_touch_enabled: bool = if msg![env; coder containsValueForKey:key_multi_touch] { msg![env; coder decodeBoolForKey:key_multi_touch] } else { false };

    let key_subviews = get_static_str(env, "UISubviews");
    let subviews: id = if msg![env; coder containsValueForKey:key_subviews] { msg![env; coder decodeObjectForKey:key_subviews] } else { nil };
    let subview_count: NSUInteger = if subviews != nil { msg![env; subviews count] } else { 0 };

    if !has_bounds && !has_frame {
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
    if bg_color != nil { () = msg![env; this setBackgroundColor:bg_color]; }

    () = msg![env; this setTag:tag];
    () = msg![env; this setContentMode:content_mode];
    () = msg![env; this setAutoresizingMask:autoresizing_mask];
    () = msg![env; this setAutoresizesSubviews:autoresizes_subviews];
    () = msg![env; this setMultipleTouchEnabled:multi_touch_enabled];

    for i in 0..subview_count {
        let subview: id = msg![env; subviews objectAtIndex:i];
        () = msg![env; this addSubview:subview];
    }

    this
}

- (NSInteger)tag { env.objc.borrow::<UIViewHostObject>(this).tag }
- (())setTag:(NSInteger)tag { env.objc.borrow_mut::<UIViewHostObject>(this).tag = tag; }

- (NSInteger)contentMode { env.objc.borrow::<UIViewHostObject>(this).content_mode }
- (())setContentMode:(NSInteger)content_mode { env.objc.borrow_mut::<UIViewHostObject>(this).content_mode = content_mode; }

- (NSUInteger)autoresizingMask { env.objc.borrow::<UIViewHostObject>(this).autoresizing_mask }
- (())setAutoresizingMask:(NSUInteger)mask { env.objc.borrow_mut::<UIViewHostObject>(this).autoresizing_mask = mask; }

- (bool)autoresizesSubviews { env.objc.borrow::<UIViewHostObject>(this).autoresizes_subviews }
- (())setAutoresizesSubviews:(bool)enabled { env.objc.borrow_mut::<UIViewHostObject>(this).autoresizes_subviews = enabled; }

- (f64)animationInterval { env.objc.borrow::<UIViewHostObject>(this).animation_interval }
- (())setAnimationInterval:(f64)interval { env.objc.borrow_mut::<UIViewHostObject>(this).animation_interval = interval; }

- (id)delegate { env.objc.borrow::<UIViewHostObject>(this).delegate }
- (())setDelegate:(id)delegate { env.objc.borrow_mut::<UIViewHostObject>(this).delegate = delegate; }

- (id)viewWithTag:(NSInteger)tag {
    let &UIViewHostObject { ref subviews, tag: view_tag, .. } = env.objc.borrow(this);
    if view_tag == tag { return this; }
    for view in subviews {
        if env.objc.borrow::<UIViewHostObject>(*view).tag == tag { return *view; }
    }
    nil
}

- (bool)isUserInteractionEnabled { env.objc.borrow::<UIViewHostObject>(this).user_interaction_enabled }
- (())setUserInteractionEnabled:(bool)enabled { env.objc.borrow_mut::<UIViewHostObject>(this).user_interaction_enabled = enabled; }

- (bool)isAnimating { env.objc.borrow::<UIViewHostObject>(this).is_animating }
- (())startAnimation {
    let mut host = env.objc.borrow_mut::<UIViewHostObject>(this);
    if !host.is_animating { host.is_animating = true; }
}
- (())stopAnimation {
    let mut host = env.objc.borrow_mut::<UIViewHostObject>(this);
    if host.is_animating { host.is_animating = false; }
}

- (bool)isMultipleTouchEnabled { env.objc.borrow::<UIViewHostObject>(this).multiple_touch_enabled }
- (())setMultipleTouchEnabled:(bool)enabled { env.objc.borrow_mut::<UIViewHostObject>(this).multiple_touch_enabled = enabled; }

- (bool)isExclusiveTouch { env.objc.borrow::<UIViewHostObject>(this).exclusive_touch }
- (())setExclusiveTouch:(bool)exclusive { env.objc.borrow_mut::<UIViewHostObject>(this).exclusive_touch = exclusive; }

- (())layoutSubviews { }

- (())layoutIfNeeded {
    () = msg![env; this layoutSubviews];
}

- (id)superview { env.objc.borrow::<UIViewHostObject>(this).superview }

- (id)window {
    let mut window: id = env.objc.borrow::<UIViewHostObject>(this).superview;
    let window_class = env.objc.get_known_class("UIWindow", &mut env.mem);
    while window != nil {
        let current_class: Class = msg![env; window class];
        if env.objc.class_is_subclass_of(current_class, window_class) { break; }
        window = env.objc.borrow::<UIViewHostObject>(window).superview;
    }
    window
}

- (id)subviews {
    let views = env.objc.borrow::<UIViewHostObject>(this).subviews.clone();
    for view in &views { retain(env, *view); }
    let subs = ns_array::from_vec(env, views);
    autorelease(env, subs)
}

- (())addSubview:(id)view {
    if view == nil { return; }
    if env.objc.borrow::<UIViewHostObject>(view).superview == this {
        () = msg![env; this bringSubviewToFront:view];
    } else {
        retain(env, view);
        () = msg![env; view removeFromSuperview];
        let mut subview_obj = env.objc.borrow_mut::<UIViewHostObject>(view);
        subview_obj.superview = this;
        let subview_layer = subview_obj.layer;
        let mut this_obj = env.objc.borrow_mut::<UIViewHostObject>(this);
        this_obj.subviews.push(view);
        let this_layer = this_obj.layer;
        () = msg![env; this_layer addSublayer:subview_layer];
    }
}

- (())insertSubview:(id)view atIndex:(NSInteger)index {
    assert!(view != nil);
    retain(env, view);
    () = msg![env; view removeFromSuperview];

    let mut subview_obj = env.objc.borrow_mut::<UIViewHostObject>(view);
    subview_obj.superview = this;
    let subview_layer = subview_obj.layer;

    let &mut UIViewHostObject { ref mut subviews, layer: this_layer, .. } = env.objc.borrow_mut(this);
    subviews.insert(index as usize, view);

    assert!(index >= 0);
    () = msg![env; this_layer insertSublayer:subview_layer atIndex:(index as u32)];
}

- (())insertSubview:(id)view belowSubview:(id)sibling {
    retain(env, view);
    () = msg![env; view removeFromSuperview];

    let mut subview_obj = env.objc.borrow_mut::<UIViewHostObject>(view);
    subview_obj.superview = this;
    let subview_layer = subview_obj.layer;

    let sibling_layer = env.objc.borrow_mut::<UIViewHostObject>(sibling).layer;

    let &mut UIViewHostObject { ref mut subviews, layer: this_layer, .. } = env.objc.borrow_mut(this);
    let idx = subviews.iter().position(|&subview2| subview2 == sibling).unwrap();
    subviews.insert(idx, view);

    () = msg![env; this_layer insertSublayer:subview_layer below:sibling_layer];
}

- (())insertSubview:(id)view aboveSubview:(id)sibling {
    if view == nil { return; }
    retain(env, view);
    let _: () = msg![env; view removeFromSuperview];

    let subview_layer = env.objc.borrow::<UIViewHostObject>(view).layer;
    let sibling_idx = env.objc.borrow::<UIViewHostObject>(this).subviews.iter().position(|&s| s == sibling);

    let insert_idx = match sibling_idx {
        Some(idx) => idx + 1,
        None => env.objc.borrow::<UIViewHostObject>(this).subviews.len()
    };

    env.objc.borrow_mut::<UIViewHostObject>(view).superview = this;
    env.objc.borrow_mut::<UIViewHostObject>(this).subviews.insert(insert_idx, view);

    let this_layer = env.objc.borrow::<UIViewHostObject>(this).layer;
    let sibling_layer = if sibling != nil { env.objc.borrow::<UIViewHostObject>(sibling).layer } else { crate::objc::nil };

    if sibling_layer != crate::objc::nil {
        let _: () = msg![env; this_layer insertSublayer:subview_layer above:sibling_layer];
    } else {
        let _: () = msg![env; this_layer addSublayer:subview_layer];
    }
}

- (())bringSubviewToFront:(id)subview {
    if subview == nil { return; }
    let &mut UIViewHostObject { ref mut subviews, layer, .. } = env.objc.borrow_mut(this);
    let Some(idx) = subviews.iter().position(|&subview2| subview2 == subview) else { return; };
    let subview2 = subviews.remove(idx);
    assert!(subview2 == subview);
    subviews.push(subview);

    let subview_layer = env.objc.borrow::<UIViewHostObject>(subview).layer;
    () = msg![env; subview_layer removeFromSuperlayer];
    () = msg![env; layer addSublayer:subview_layer];
}

- (())sendSubviewToBack:(id)subview {
    if subview == nil { return; }
    let &mut UIViewHostObject { ref mut subviews, layer, .. } = env.objc.borrow_mut(this);
    let Some(idx) = subviews.iter().position(|&subview2| subview2 == subview) else { return; };
    let subview2 = subviews.remove(idx);
    assert!(subview2 == subview);
    subviews.insert(0, subview);

    let subview_layer = env.objc.borrow::<UIViewHostObject>(subview).layer;
    () = msg![env; subview_layer removeFromSuperlayer];
    () = msg![env; layer insertSublayer:subview_layer atIndex:0u32];
}

- (())removeFromSuperview {
    let &mut UIViewHostObject { ref mut superview, layer: this_layer, .. } = env.objc.borrow_mut(this);
    let superview = std::mem::take(superview);
    if superview == nil { return; }
    let _: () = msg![env; this_layer removeFromSuperlayer];

    let mut superview_obj = env.objc.borrow_mut::<UIViewHostObject>(superview);
    let subviews = &mut superview_obj.subviews;

    if let Some(idx) = subviews.iter().position(|&subview| subview == this) {
        let subview = subviews.remove(idx);
        assert!(subview == this);
        release(env, this);
    }
}

- (())dealloc {
    let UIViewHostObject { layer, subviews, .. } = std::mem::take(env.objc.borrow_mut(this));
    release(env, layer);

    for subview in subviews {
        env.objc.borrow_mut::<UIViewHostObject>(subview).superview = nil;
        release(env, subview);
    }

    let state = &mut env.framework_state.uikit.ui_view.views;
    if let Some(pos) = state.iter().position(|&v| v == this) {
        state.swap_remove(pos);
    }

    env.objc.dealloc_object(this, &mut env.mem);
}

- (id)layer { env.objc.borrow_mut::<UIViewHostObject>(this).layer }

- (bool)isHidden {
    let layer = env.objc.borrow::<UIViewHostObject>(this).layer;
    msg![env; layer isHidden]
}
- (())setHidden:(bool)hidden {
    let layer = env.objc.borrow::<UIViewHostObject>(this).layer;
    msg![env; layer setHidden:hidden]
}

- (bool)clipsToBounds { env.objc.borrow::<UIViewHostObject>(this).clips_to_bounds }
- (())setClipsToBounds:(bool)clips { env.objc.borrow_mut::<UIViewHostObject>(this).clips_to_bounds = clips; }

- (())setStyle:(u32)_style { }
- (id)context { nil }
- (())setContext:(id)_context { }
- (())resume {
    let mut host = env.objc.borrow_mut::<UIViewHostObject>(this);
    host.is_animating = true;
}

- (())flushBuffer {
    let layer = env.objc.borrow::<UIViewHostObject>(this).layer;
    let _: () = msg![env; layer display];
}

- (())setupView { }
- (())endDrawing { }

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

- (CGFloat)contentScaleFactor { 1.0 }
- (())setContentScaleFactor:(CGFloat)_scale { }

- (id)backgroundColor {
    let layer = env.objc.borrow::<UIViewHostObject>(this).layer;
    let cg_color: CGColorRef = msg![env; layer backgroundColor];
    msg_class![env; UIColor colorWithCGColor:cg_color]
}
- (())setBackgroundColor:(id)color {
    let layer = env.objc.borrow::<UIViewHostObject>(this).layer;

    if color != nil {
        let pattern_image = super::ui_color::get_pattern_image(&env.objc, color);
        if pattern_image != nil {
            let cg_image: id = msg![env; pattern_image CGImage];
            crate::frameworks::core_animation::ca_layer::set_background_pattern_cg_image(
                env, layer, cg_image,
            );
            let clear: CGColorRef = nil;
            () = msg![env; layer setBackgroundColor:clear];
            return;
        }
    }

    let cg_color: CGColorRef = if color != nil { msg![env; color CGColor] } else { nil };
    () = msg![env; layer setBackgroundColor:cg_color];
}

// Some apps (notably Google Mobile 0.1.337) call -setLineBreakMode: on plain
// UIView subclasses that contain a UILabel, expecting the view to proxy the
// call. UIKit itself silently ignored this on iOS 2.x, so we mirror that by
// making UIView accept (and discard) the call.
- (())setLineBreakMode:(i32)_mode { }
- (i32)lineBreakMode { 0 }
// Same treatment for a couple of other label-style setters that iOS 2.x apps
// sometimes invoke on container views.
- (())setTextAlignment:(i32)_align { }
- (i32)textAlignment { 0 }

- (())setNeedsDisplay {
    let this_class = ObjC::read_isa(this, &env.mem);
    let ui_view_class = env.objc.get_known_class("UIView", &mut env.mem);

    let draw_layer_sel = env.objc.lookup_selector("drawLayer:inContext:").unwrap();
    let draw_rect_sel = env.objc.lookup_selector("drawRect:").unwrap();

    if env.objc.class_overrides_method_of_superclass(this_class, draw_rect_sel, ui_view_class) ||
       env.objc.class_overrides_method_of_superclass(this_class, draw_layer_sel, ui_view_class) {
        let layer = env.objc.borrow::<UIViewHostObject>(this).layer;
        msg![env; layer setNeedsDisplay]
    }
}

- (())setNeedsLayout { }

- (CGRect)bounds {
    let layer = env.objc.borrow::<UIViewHostObject>(this).layer;
    msg![env; layer bounds]
}
- (())setBounds:(CGRect)bounds {
    let layer = env.objc.borrow::<UIViewHostObject>(this).layer;
    msg![env; layer setBounds:bounds]
}
- (CGPoint)center {
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

- (bool)clearsContextBeforeDrawing { env.objc.borrow::<UIViewHostObject>(this).clears_context_before_drawing }
- (())setClearsContextBeforeDrawing:(bool)v { env.objc.borrow_mut::<UIViewHostObject>(this).clears_context_before_drawing = v; }

- (())drawRect:(CGRect)_rect { }

- (())drawLayer:(id)layer inContext:(CGContextRef)context {
    let mut bounds: CGRect = msg![env; layer bounds];
    bounds.origin = CGPoint { x: 0.0, y: 0.0 };

    if env.objc.borrow::<UIViewHostObject>(this).clears_context_before_drawing {
        CGContextClearRect(env, context, bounds);
    }
    UIGraphicsPushContext(env, context);
    () = msg![env; this drawRect:bounds];
    UIGraphicsPopContext(env);
}

- (bool)pointInside:(CGPoint)point withEvent:(id)_event {
    let layer = env.objc.borrow::<UIViewHostObject>(this).layer;
    msg![env; layer containsPoint:point]
}

- (bool)isUncontrolled {
    env.objc.borrow::<UIViewHostObject>(this).is_uncontrolled
}

- (id)hitTest:(CGPoint)point withEvent:(id)event {
    let is_inside: bool = msg![env; this pointInside:point withEvent:event];
    let subviews = env.objc.borrow::<UIViewHostObject>(this).subviews.clone();

    for subview in subviews.into_iter().rev() {
        let hidden: bool = msg![env; subview isHidden];
        let alpha: CGFloat = msg![env; subview alpha];
        let interactible: bool = msg![env; subview isUserInteractionEnabled];
        if hidden || alpha < 0.01 || !interactible { continue; }

        let sub_point: CGPoint = msg![env; subview convertPoint:point fromView:this];
        let subview_hit: id = msg![env; subview hitTest:sub_point withEvent:event];
        if subview_hit != nil { return subview_hit; }
    }

    if is_inside { this } else { nil }
}

- (bool)endEditing:(bool)force {
    assert!(force);
    let responder: id = env.framework_state.uikit.ui_responder.first_responder;
    let class = msg![env; responder class];
    let ui_text_field_class = env.objc.get_known_class("UITextField", &mut env.mem);

    if responder != nil && env.objc.class_is_subclass_of(class, ui_text_field_class) {
        let mut to_find = responder;
        while to_find != nil {
            if to_find == this { return msg![env; responder resignFirstResponder]; }
            to_find = msg![env; to_find superview];
        }
    }
    false
}

- (id)nextResponder {
    let host_object = env.objc.borrow::<UIViewHostObject>(this);
    if host_object.view_controller != nil { host_object.view_controller } else { host_object.superview }
}

- (CGPoint)convertPoint:(CGPoint)point fromView:(id)other {
    if other == nil {
        let window: id = msg![env; this window];
        if window == nil { return point; }
        return msg![env; this convertPoint:point fromView:window]
    }

    let view_class: id = msg_class![env; UIView class];
    let is_view: bool = msg![env; other isKindOfClass:view_class];
    let actual_other = if is_view { other } else {
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

- (CGPoint)convertPoint:(CGPoint)point toView:(id)other {
    if other == nil {
        let window: id = msg![env; this window];
        if window == nil { return point; }
        return msg![env; this convertPoint:point toView:window]
    }

    let view_class: id = msg_class![env; UIView class];
    let is_view: bool = msg![env; other isKindOfClass:view_class];
    let actual_other = if is_view { other } else {
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
    msg![env; this_layer convertPoint:point toLayer:other_layer]
}

- (CGRect)convertRect:(CGRect)rect fromView:(id)other {
    if other == nil {
        let window: id = msg![env; this window];
        if window == nil { return rect; }
        return msg![env; this convertRect:rect fromView:window]
    }

    let view_class: id = msg_class![env; UIView class];
    let is_view: bool = msg![env; other isKindOfClass:view_class];
    let actual_other = if is_view { other } else {
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

- (CGRect)convertRect:(CGRect)rect toView:(id)other {
    if other == nil {
        let window: id = msg![env; this window];
        if window == nil { return rect; }
        return msg![env; this convertRect:rect toView:window]
    }

    let view_class: id = msg_class![env; UIView class];
    let is_view: bool = msg![env; other isKindOfClass:view_class];
    let actual_other = if is_view { other } else {
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

- (CGSize)sizeThatFits:(CGSize)size { size }

- (())sizeToFit {
    let bounds: CGRect = msg![env; this bounds];
    let size: CGSize = bounds.size;
    let new_size: CGSize = msg![env; this sizeThatFits:size];
    () = msg![env; this setBounds:(CGRect { origin: CGPoint::default(), size: new_size })];
}

@end

};
