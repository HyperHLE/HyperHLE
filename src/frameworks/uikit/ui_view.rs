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
pub mod ui_collection_view;
pub mod ui_control;
pub mod ui_image_view;
pub mod ui_label;
pub mod ui_page_control;
pub mod ui_picker_view;
pub mod ui_refresh_control;
pub mod ui_scroll_view;
pub mod ui_table_view;
pub mod ui_text_selection_view;
pub mod ui_toolbar;
pub mod ui_web_view;
pub mod ui_window;

use super::ui_graphics::{UIGraphicsPopContext, UIGraphicsPushContext};
use crate::frameworks::core_animation::ca_layer;
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
    pub ui_image_view: ui_image_view::State,
    pub ui_window: ui_window::State,
    pub(super) animation_block: AnimationBlockState,
    pub(super) doodle_jump_popup_dismissed: bool,
}

pub(crate) struct UIViewHostObject {
    pub(crate) layer: id,
    pub(crate) subviews: Vec<id>,
    pub(crate) superview: id,
    view_controller: id,
    /// Only used by UIWindow. Strong reference for the iOS 4
    /// rootViewController property.
    root_view_controller: id,
    tag: NSInteger,
    content_mode: NSInteger,
    autoresizing_mask: NSUInteger,
    autoresizes_subviews: bool,
    clears_context_before_drawing: bool,
    user_interaction_enabled: bool,
    multiple_touch_enabled: bool,
    exclusive_touch: bool,
    content_scale_factor: CGFloat,
    delegate: id,
    animation_interval: f64,
    is_animating: bool,
    clips_to_bounds: bool,
    is_uncontrolled: bool,
    /// Strong refs to attached `UIGestureRecognizer*` instances. Used by
    /// `addGestureRecognizer:` / `removeGestureRecognizer:` /
    /// `gestureRecognizers`. We don't dispatch real gesture recognition;
    /// keeping a list is enough to avoid `Unknown selector` panics in
    /// games that wire pinch/pan/tap recognizers up at startup.
    gesture_recognizers: Vec<id>,
    // ----- UIAccessibility informal protocol (NSObject category in real
    // iOS, but in practice only meaningful for views). All properties
    // default per Apple's documented behaviour for plain `UIView`:
    // <https://developer.apple.com/documentation/objectivec/nsobject/uiaccessibility>
    /// `BOOL isAccessibilityElement` — default `NO` for plain UIView.
    is_accessibility_element: bool,
    /// `UIAccessibilityTraits accessibilityTraits` (uint64_t bitmask).
    /// Default is `UIAccessibilityTraitNone` (0).
    accessibility_traits: u64,
    /// `NSString *accessibilityLabel` — retained; default `nil`.
    accessibility_label: id,
    /// `NSString *accessibilityHint` — retained; default `nil`.
    accessibility_hint: id,
    /// `NSString *accessibilityValue` — retained; default `nil`.
    accessibility_value: id,
    /// `NSString *accessibilityIdentifier` (from UIAccessibilityIdentification).
    accessibility_identifier: id,
    /// `NSString *accessibilityLanguage` — BCP-47 language tag; default `nil`.
    accessibility_language: id,
    /// `BOOL accessibilityElementsHidden` (iOS 5+); default `NO`.
    accessibility_elements_hidden: bool,
    /// `BOOL accessibilityViewIsModal` (iOS 5+); default `NO`.
    accessibility_view_is_modal: bool,
    /// `BOOL shouldGroupAccessibilityChildren` (iOS 6+); default `NO`.
    should_group_accessibility_children: bool,
}
impl HostObject for UIViewHostObject {}
impl Default for UIViewHostObject {
    fn default() -> UIViewHostObject {
        UIViewHostObject {
            layer: nil,
            subviews: Vec::new(),
            superview: nil,
            view_controller: nil,
            root_view_controller: nil,
            tag: 0,
            content_mode: 0,      // UIViewContentModeScaleToFill
            autoresizing_mask: 0, // UIViewAutoresizingNone
            autoresizes_subviews: true,
            clears_context_before_drawing: true,
            user_interaction_enabled: true,
            multiple_touch_enabled: false,
            exclusive_touch: false,
            content_scale_factor: 1.0,
            delegate: nil,
            animation_interval: 1.0 / 60.0,
            is_animating: false,
            clips_to_bounds: false,
            is_uncontrolled: false,
            gesture_recognizers: Vec::new(),
            is_accessibility_element: false,
            accessibility_traits: 0,
            accessibility_label: nil,
            accessibility_hint: nil,
            accessibility_value: nil,
            accessibility_identifier: nil,
            accessibility_language: nil,
            accessibility_elements_hidden: false,
            accessibility_view_is_modal: false,
            should_group_accessibility_children: false,
        }
    }
}

pub fn set_view_controller(env: &mut Environment, view: id, controller: id) {
    let host_obj = env.objc.borrow_mut::<UIViewHostObject>(view);
    host_obj.view_controller = controller;
}

/// Distribute a change of the superview's size along one axis over the
/// flexible parts of a subview (leading margin, size, trailing margin), in
/// proportion to their current lengths, or evenly if they are all zero. This
/// is how UIKit's autoresizing behaves. Returns the new `(origin, size)`.
fn autoresize_axis(
    origin: CGFloat,
    size: CGFloat,
    old_parent: CGFloat,
    new_parent: CGFloat,
    (flex_lead, flex_size, flex_trail): (bool, bool, bool),
) -> (CGFloat, CGFloat) {
    let delta = new_parent - old_parent;
    let trail = old_parent - origin - size;
    let parts = [(flex_lead, origin), (flex_size, size), (flex_trail, trail)];
    let flexible_count = parts.iter().filter(|(flexible, _)| *flexible).count();
    if delta == 0.0 || flexible_count == 0 {
        return (origin, size);
    }
    let flexible_total: CGFloat = parts
        .iter()
        .filter(|(flexible, _)| *flexible)
        .map(|(_, len)| len.max(0.0))
        .sum();
    let share = |flexible: bool, len: CGFloat| match (flexible, flexible_total > 0.0) {
        (false, _) => 0.0,
        (true, true) => delta * len.max(0.0) / flexible_total,
        (true, false) => delta / flexible_count as CGFloat,
    };
    let new_origin = origin + share(flex_lead, origin);
    let new_size = (size + share(flex_size, size)).max(0.0);
    (new_origin, new_size)
}

/// Apply the subviews' autoresizing masks after `view`'s bounds changed from
/// `old_size`, like UIKit does when a view is resized, then lay out each
/// subview whose size changed (e.g. so an EAGLView rebuilds its renderbuffer).
fn resize_subviews_with_old_size(env: &mut Environment, view: id, old_size: CGSize) {
    let autoresizes: bool = msg![env; view autoresizesSubviews];
    if !autoresizes {
        return;
    }
    let new_size = {
        let bounds: CGRect = msg![env; view bounds];
        bounds.size
    };
    // A view that had no size yet (still being set up) has nothing to resize
    // its subviews relative to.
    if new_size == old_size || old_size.width <= 0.0 || old_size.height <= 0.0 {
        return;
    }
    let subviews = env.objc.borrow::<UIViewHostObject>(view).subviews.clone();
    for subview in subviews {
        let mask: NSUInteger = msg![env; subview autoresizingMask];
        if mask == 0 {
            continue;
        }
        let flag = |bit: u32| mask & (1 << bit) != 0;
        let frame: CGRect = msg![env; subview frame];
        let (x, width) = autoresize_axis(
            frame.origin.x,
            frame.size.width,
            old_size.width,
            new_size.width,
            (flag(0), flag(1), flag(2)),
        );
        let (y, height) = autoresize_axis(
            frame.origin.y,
            frame.size.height,
            old_size.height,
            new_size.height,
            (flag(3), flag(4), flag(5)),
        );
        let new_frame = CGRect {
            origin: CGPoint { x, y },
            size: CGSize { width, height },
        };
        if new_frame == frame {
            continue;
        }
        let old_subview_size = {
            let bounds: CGRect = msg![env; subview bounds];
            bounds.size
        };
        // This autoresizes the subview's own subviews in turn.
        () = msg![env; subview setFrame:new_frame];
        let new_subview_size = {
            let bounds: CGRect = msg![env; subview bounds];
            bounds.size
        };
        if new_subview_size != old_subview_size {
            () = msg![env; subview layoutSubviews];
        }
    }
}

pub(super) fn gesture_recognizers(env: &Environment, view: id) -> Vec<id> {
    env.objc
        .borrow::<UIViewHostObject>(view)
        .gesture_recognizers
        .clone()
}

fn init_common(env: &mut Environment, this: id) -> id {
    let view_class: Class = msg![env; this class];
    let layer_class: Class = msg![env; view_class layerClass];
    let layer: id = msg![env; layer_class layer];
    () = msg![env; layer setDelegate:this];
    () = msg![env; layer setOpaque:true];
    // A view's backing layer inherits the view's contentScaleFactor, which
    // defaults to the main screen's scale (1.0 on non-retina devices, 2.0 on
    // retina ones). EAGL derives the renderbuffer size from the layer's
    // bounds * contentsScale, so without this retina-aware apps would
    // allocate a half-size framebuffer and render zoomed / cropped.
    let screen_scale: crate::frameworks::core_graphics::CGFloat = {
        let screen: id = msg_class![env; UIScreen mainScreen];
        msg![env; screen scale]
    };
    env.objc
        .borrow_mut::<UIViewHostObject>(this)
        .content_scale_factor = screen_scale;
    env.objc
        .borrow_mut::<crate::frameworks::core_animation::ca_layer::CALayerHostObject>(layer)
        .contents_scale = screen_scale;
    crate::frameworks::core_animation::ca_layer::set_use_implicit_animations(env, layer, false);

    // A view's backing layer is not retained by the view.
    env.objc.borrow_mut::<UIViewHostObject>(this).layer = layer;
    env.framework_state.uikit.ui_view.views.push(this);

    this
}

fn touchhle_cocos_view_class_name(env: &mut Environment, view: id) -> String {
    if view == nil {
        return String::new();
    }
    let cls: crate::objc::Class = msg![env; view class];
    env.objc.get_class_name(cls).to_owned()
}

fn touchhle_cocos_is_gl_or_game_view_name(class_name: &str) -> bool {
    matches!(
        class_name,
        "CCGLView"
            | "CCEAGLView"
            | "EAGLView"
            | "GLKView"
            | "Cocos2dxGLView"
            | "Cocos2dView"
            | "CCUIViewWrapper"
            | "DirectorView"
    ) || class_name.contains("EAGL")
        || class_name.contains("GLView")
        || class_name.contains("Cocos")
        || class_name.contains("CCGL")
        || class_name.contains("Unity")
        || class_name.contains("UnityView")
        || class_name.contains("UnityGLView")
        || class_name.contains("UnityRenderingView")
        || class_name.contains("RenderView")
        || class_name.contains("RootView")
        || class_name.contains("GameView")
}

fn touchhle_cocos_landscape_rect(env: &Environment) -> CGRect {
    // PERF: computed once; both the env lookups and the parse are not free
    // and this is consulted from layout/hit-test paths.
    let bundle_id = env.bundle.bundle_identifier();
    static CACHED: std::sync::OnceLock<(f32, f32)> = std::sync::OnceLock::new();
    let size = *CACHED.get_or_init(|| {
        std::env::var("TOUCHHLE_COCOS_LANDSCAPE_SIZE")
            .or_else(|_| std::env::var("TOUCHHLE_UNITY_LANDSCAPE_SIZE"))
            .or_else(|_| std::env::var("TOUCHHLE_ENGINE_LANDSCAPE_SIZE"))
            .ok()
            .and_then(|v| {
                let mut parts = v.split(|c| c == 'x' || c == 'X' || c == ',');
                let w = parts.next()?.trim().parse::<f32>().ok()?;
                let h = parts.next()?.trim().parse::<f32>().ok()?;
                Some((w, h))
            })
            .unwrap_or_else(|| {
                match bundle_id {
                    // Existing known iPad-ish Cocos clones keep using their old safe size.
                    "com.apprisetec9.minionjump" | "com.risinghighapps.kingdomprincepro" => {
                        (1024.0, 768.0)
                    }
                    _ => (480.0, 320.0),
                }
            })
    });
    CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size: CGSize {
            width: size.0,
            height: size.1,
        },
    }
}

fn touchhle_cocos_should_force_landscape_view(env: &mut Environment, view: id) -> bool {
    if view == nil {
        return false;
    }

    let class_name = touchhle_cocos_view_class_name(env, view);
    if !touchhle_cocos_is_gl_or_game_view_name(&class_name) && class_name != "UIWindow" {
        return false;
    }

    if crate::env_flag_cached!("TOUCHHLE_COCOS_FORCE_LANDSCAPE_VIEW")
        || crate::env_flag_cached!("TOUCHHLE_UNITY_FORCE_LANDSCAPE_VIEW")
        || crate::env_flag_cached!("TOUCHHLE_ENGINE_FORCE_LANDSCAPE_VIEW")
        || crate::env_flag_cached!("TOUCHHLE_FORCE_LANDSCAPE_VIEW_BOUNDS")
    {
        return true;
    }

    matches!(
        env.bundle.bundle_identifier(),
        "com.apprisetec9.minionjump" | "com.risinghighapps.kingdomprincepro"
    ) && class_name == "CCGLView"
}

fn touchhle_cocos_sanitize_rect(rect: CGRect) -> CGRect {
    let mut r = rect;
    if !r.origin.x.is_finite() {
        r.origin.x = 0.0;
    }
    if !r.origin.y.is_finite() {
        r.origin.y = 0.0;
    }
    if !r.size.width.is_finite() || r.size.width < 0.0 {
        r.size.width = 0.0;
    }
    if !r.size.height.is_finite() || r.size.height < 0.0 {
        r.size.height = 0.0;
    }
    r
}

fn touchhle_cocos_should_fuzz_hit_testing(env: &mut Environment, view: id) -> bool {
    if crate::env_flag_cached!("TOUCHHLE_COCOS_STRICT_HITTEST") {
        return false;
    }
    let class_name = touchhle_cocos_view_class_name(env, view);
    touchhle_cocos_is_gl_or_game_view_name(&class_name)
}

fn ultrahle_minionjump_force_landscape_ccglview(env: &mut Environment, this: id) -> bool {
    touchhle_cocos_should_force_landscape_view(env, this)
}

fn ultrahle_minionjump_landscape_rect() -> CGRect {
    CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size: CGSize {
            width: 1024.0,
            height: 768.0,
        },
    }
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);
@implementation UIView: UIResponder

/// `- (void)setNeedsUpdateOfHomeIndicatorAutoHidden` (iOS 11+)
///
/// Asks UIKit to re-evaluate whether the home indicator should be hidden. The
/// emulator draws no home indicator, so there is nothing to re-evaluate; the
/// call is accepted instead of hitting the "does not respond" path.
- (())setNeedsUpdateOfHomeIndicatorAutoHidden {
}

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::<UIViewHostObject>::default();
    let view = env.objc.alloc_object(this, host_object, &mut env.mem);
    crate::frameworks::foundation::ns_object::invoke_cxx_constructors(env, view);
    view
}

+ (Class)layerClass {
    // Game engines that ship their own GL view classes (Gameloft's EAGLView,
    // Cocos2d's CCEAGLView, etc.) override +layerClass to return CAEAGLLayer.
    // Mirror that here so their backing layer is a real CAEAGLLayer: the EAGL
    // fast-path presentation and find_fullscreen_eagl_layer() both depend on
    // the layer being an EAGL layer, otherwise frames go through the RAM
    // readback slow path (or never reach the screen at all - see the
    // Asphalt 8 black-screen report).
    env.objc.get_known_class("CAEAGLLayer", &mut env.mem)
}

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
+ (())setAnimationStartDate:(id)_date { }
+ (())setAnimationPosition:(CGPoint)_position { }
+ (bool)areAnimationsEnabled { true }

// MARK: - Block-based animation API (iOS 4+)
//
// touchHLE doesn't actually animate property changes — assignments inside
// the `animations` block snap to their final value immediately. The
// important contract that *must* be honoured is:
//
//   1. Run the `animations` block synchronously, with view-animation
//      semantics, so the app's mutations to view properties take effect.
//   2. Schedule a one-shot timer that fires `completion(YES)` after
//      `delay + duration` seconds (or immediately, if both are zero, on
//      the next run-loop tick — we approximate this with a 0-second
//      NSTimer).
//
// This matches Apple's documentation for
// `+[UIView animateWithDuration:delay:options:animations:completion:]`,
// which guarantees the completion handler is invoked exactly once on the
// main thread, with `finished:YES` if the animation ran to completion
// (we never cancel them, so this is always YES).
//
// On the wire, `animations` and `completion` are Objective-C blocks. A
// block is a guest pointer to a `Block_layout` struct whose `invoke`
// field (offset 12) is the function pointer to call. We fish the
// `invoke` out with `mem.read` and call it via `GuestFunction`'s
// `call_from_host`.

+ (())animateWithDuration:(f64)duration
                animations:(MutPtr<()>)animations {
    let zero_completion: MutPtr<()> = MutPtr::null();
    () = msg![env; this
        animateWithDuration:duration
                      delay:(0.0_f64)
                    options:(0u32)
                 animations:animations
                 completion:zero_completion
    ];
}

+ (())animateWithDuration:(f64)duration
                animations:(MutPtr<()>)animations
                completion:(MutPtr<()>)completion {
    () = msg![env; this
        animateWithDuration:duration
                      delay:(0.0_f64)
                    options:(0u32)
                 animations:animations
                 completion:completion
    ];
}

+ (())animateWithDuration:(f64)duration
                     delay:(f64)delay
                   options:(u32)_options
                animations:(MutPtr<()>)animations
                completion:(MutPtr<()>)completion {
    // 1. Invoke the animations block synchronously. Animation blocks on
    //    iOS take no arguments and return void, so we just need
    //    `invoke(block_ptr)`.
    if !animations.is_null() {
        invoke_void_block(env, animations);
    }

    // Copy for asynchronous use: guest code may reuse stack storage
    // once this method returns. A plain ObjC retain cannot promote a block.
    if completion.is_null() {
        return;
    }
    let total_delay = (delay + duration).max(0.0);
    let completion = crate::libc::blocks::_Block_copy(env, completion.cast_void().cast_const());

    // Pack the block pointer into an NSNumber so it survives userInfo.
    let bits = completion.to_bits();
    let context_num: id = msg_class![env; NSNumber numberWithUnsignedInt:bits];

    let key_block: id = get_static_str(env, "_touchHLE_uiview_block_anim_block");
    let dict: id = dict_from_keys_and_objects(env, &[(key_block, context_num)]);

    let fire_sel = env
        .objc
        .lookup_selector("_touchHLE_blockAnimationDidFinish:")
        .expect("UIView _touchHLE_blockAnimationDidFinish: not registered");
    let ui_view_class: Class = env.objc.get_known_class("UIView", &mut env.mem);
    let _: id = msg_class![env;
        NSTimer scheduledTimerWithTimeInterval:total_delay
                                       target:ui_view_class
                                     selector:fire_sel
                                     userInfo:dict
                                      repeats:false
    ];
}

+ (())_touchHLE_blockAnimationDidFinish:(id)which_timer {
    let dict: id = msg![env; which_timer userInfo];
    let key_block: id = get_static_str(env, "_touchHLE_uiview_block_anim_block");
    let context_num: id = msg![env; dict objectForKey:key_block];
    if context_num == nil { return; }

    let bits: u32 = msg![env; context_num unsignedIntValue];
    let block: MutPtr<()> = MutPtr::from_bits(bits);
    if !block.is_null() {
        invoke_bool_block(env, block, true);
        // Balance the heap copy held by this timer.
        crate::libc::blocks::_Block_release(env, block.cast_void().cast_const());
    }
}

// `+transitionWithView:duration:options:animations:completion:` and
// `+transitionFromView:toView:duration:options:completion:` ship in
// iOS 4 too. Real iOS swaps the view hierarchy with a flip / cross-
// fade transition; touchHLE has no animator, so we do the swap
// instantaneously and still fire the completion block — that keeps
// games whose state machine waits on the completion (e.g. Bubble
// Witch's level transition) from deadlocking.

+ (())transitionWithView:(id)_view
                duration:(f64)duration
                 options:(u32)options
              animations:(MutPtr<()>)animations
              completion:(MutPtr<()>)completion {
    () = msg![env; this
        animateWithDuration:duration
                      delay:(0.0_f64)
                    options:options
                 animations:animations
                 completion:completion
    ];
}

+ (())transitionFromView:(id)from_view
                  toView:(id)to_view
                duration:(f64)duration
                 options:(u32)options
              completion:(MutPtr<()>)completion {
    // Apple docs: removes `from_view` from its superview and inserts
    // `to_view` into the same place (unless the
    // UIViewAnimationOptionShowHideTransitionViews option is set, in
    // which case both views remain in their hierarchy and only their
    // hidden state is toggled).
    const SHOW_HIDE_OPTION: u32 = 1 << 19;
    if options & SHOW_HIDE_OPTION != 0 {
        if from_view != nil { let _: () = msg![env; from_view setHidden:true]; }
        if to_view != nil { let _: () = msg![env; to_view setHidden:false]; }
    } else if from_view != nil {
        let parent: id = msg![env; from_view superview];
        if parent != nil && to_view != nil {
            let _: () = msg![env; parent addSubview:to_view];
        }
        let _: () = msg![env; from_view removeFromSuperview];
    }
    let zero_animations: MutPtr<()> = MutPtr::null();
    () = msg![env; this
        animateWithDuration:duration
                      delay:(0.0_f64)
                    options:options
                 animations:zero_animations
                 completion:completion
    ];
}

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

    // Interface Builder archives the Alpha and "Clip Subviews" attributes
    // only when they differ from the defaults (1.0 and NO). A view with
    // UIAlpha 0 is invisible until the app fades it in.
    let key_alpha = get_static_str(env, "UIAlpha");
    let alpha: CGFloat = if msg![env; coder containsValueForKey:key_alpha] { msg![env; coder decodeFloatForKey:key_alpha] } else { 1.0 };

    let key_clips = get_static_str(env, "UIClipsToBounds");
    let clips_to_bounds: bool = if msg![env; coder containsValueForKey:key_clips] { msg![env; coder decodeBoolForKey:key_clips] } else { false };

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

    // Interface Builder archives this for views whose "User Interaction
    // Enabled" box is unchecked, which is the default for labels and image
    // views. Without it, a label laid over a button swallows its taps.
    let key_interaction_disabled = get_static_str(env, "UIUserInteractionDisabled");
    let user_interaction_disabled: bool = if msg![env; coder containsValueForKey:key_interaction_disabled] { msg![env; coder decodeBoolForKey:key_interaction_disabled] } else { false };

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
    () = msg![env; this setAlpha:alpha];
    () = msg![env; this setClipsToBounds:clips_to_bounds];

    () = msg![env; this setTag:tag];
    () = msg![env; this setContentMode:content_mode];
    () = msg![env; this setAutoresizingMask:autoresizing_mask];
    () = msg![env; this setAutoresizesSubviews:autoresizes_subviews];
    () = msg![env; this setMultipleTouchEnabled:multi_touch_enabled];
    if user_interaction_disabled {
        () = msg![env; this setUserInteractionEnabled:false];
    }

    for i in 0..subview_count {
        let subview: id = msg![env; subviews objectAtIndex:i];
        () = msg![env; this addSubview:subview];
    }

    {
        let view_class: Class = msg![env; this class];
        let class_name = env.objc.get_class_name(view_class).to_owned();

        if crate::env_flag_cached!("TOUCHHLE_FORCE_LANDSCAPE_VIEW_BOUNDS")
            && (class_name == "UIWindow" || class_name.contains("EAGLView"))
        {
            let forced_bounds = CGRect {
                origin: CGPoint { x: 0.0, y: 0.0 },
                size: CGSize {
                    width: 480.0,
                    height: 320.0,
                },
            };
            let forced_center = CGPoint { x: 240.0, y: 160.0 };

            log!(
                "TOUCHHLE_FORCE_LANDSCAPE_VIEW_BOUNDS=1: forcing {} {:?} frame/bounds to 480x320",
                class_name,
                this
            );

            () = msg![env; this setBounds:forced_bounds];
            () = msg![env; this setFrame:forced_bounds];
            () = msg![env; this setCenter:forced_center];
        }

        let final_frame: CGRect = msg![env; this frame];
        let user_int: bool = msg![env; this isUserInteractionEnabled];
        let hidden: bool = msg![env; this isHidden];
        log!(
            "UIView initWithCoder finished: {} {:?} frame={:?} userInteraction={} hidden={} subviews={}",
            class_name,
            this,
            final_frame,
            user_int,
            hidden,
            subview_count,
        );
    }

    this
}

- (NSInteger)tag { env.objc.borrow::<UIViewHostObject>(this).tag }
- (())setTag:(NSInteger)tag { env.objc.borrow_mut::<UIViewHostObject>(this).tag = tag; }

- (NSInteger)contentMode { env.objc.borrow::<UIViewHostObject>(this).content_mode }
// A view's contentMode positions its layer's contents, the way UIKit sets the
// layer's contentsGravity. Gravity names follow a Y-up axis, so the top of a
// view is the layer's kCAGravityBottom edge.
// https://developer.apple.com/documentation/uikit/uiview/1622619-contentmode
// https://developer.apple.com/documentation/quartzcore/calayer/1410933-contentsgravity
- (())setContentMode:(NSInteger)content_mode {
    env.objc.borrow_mut::<UIViewHostObject>(this).content_mode = content_mode;
    let gravity = match content_mode {
        1 => ca_layer::kCAGravityResizeAspect,      // ScaleAspectFit
        2 => ca_layer::kCAGravityResizeAspectFill,  // ScaleAspectFill
        4 => ca_layer::kCAGravityCenter,            // Center
        5 => ca_layer::kCAGravityBottom,            // Top
        6 => ca_layer::kCAGravityTop,               // Bottom
        7 => ca_layer::kCAGravityLeft,              // Left
        8 => ca_layer::kCAGravityRight,             // Right
        9 => ca_layer::kCAGravityBottomLeft,        // TopLeft
        10 => ca_layer::kCAGravityBottomRight,      // TopRight
        11 => ca_layer::kCAGravityTopLeft,          // BottomLeft
        12 => ca_layer::kCAGravityTopRight,         // BottomRight
        _ => ca_layer::kCAGravityResize,            // ScaleToFill, Redraw
    };
    let gravity = get_static_str(env, gravity);
    let layer = env.objc.borrow::<UIViewHostObject>(this).layer;
    () = msg![env; layer setContentsGravity:gravity];
}

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
    let subviews = subviews.clone();
    for view in subviews {
        let found: id = msg![env; view viewWithTag:tag];
        if found != nil { return found; }
    }
    nil
}

- (bool)isUserInteractionEnabled { env.objc.borrow::<UIViewHostObject>(this).user_interaction_enabled }
- (())setUserInteractionEnabled:(bool)enabled { env.objc.borrow_mut::<UIViewHostObject>(this).user_interaction_enabled = enabled; }

- (bool)isAnimating { env.objc.borrow::<UIViewHostObject>(this).is_animating }
- (())startAnimation {
    let host = env.objc.borrow_mut::<UIViewHostObject>(this);
    if !host.is_animating { host.is_animating = true; }
}
- (())stopAnimation {
    let host = env.objc.borrow_mut::<UIViewHostObject>(this);
    if host.is_animating { host.is_animating = false; }
}

- (bool)isMultipleTouchEnabled { env.objc.borrow::<UIViewHostObject>(this).multiple_touch_enabled }
- (())setMultipleTouchEnabled:(bool)enabled { env.objc.borrow_mut::<UIViewHostObject>(this).multiple_touch_enabled = enabled; }

- (bool)isExclusiveTouch { env.objc.borrow::<UIViewHostObject>(this).exclusive_touch }
- (())setExclusiveTouch:(bool)exclusive { env.objc.borrow_mut::<UIViewHostObject>(this).exclusive_touch = exclusive; }

// MARK: - UIAccessibility informal protocol
//
// In Apple's framework UIAccessibility is declared as an `NSObject`
// category, so every `NSObject` answers these selectors. In practice
// only views set them — Unity in particular calls
// `setIsAccessibilityElement:` / `setAccessibilityTraits:` on its
// `UnityView` at start-up. We back the state on `UIViewHostObject`
// and follow Apple's documented "retain (copy)" / "assign" semantics:
// <https://developer.apple.com/documentation/objectivec/nsobject/uiaccessibility>

- (bool)isAccessibilityElement {
    env.objc.borrow::<UIViewHostObject>(this).is_accessibility_element
}
- (())setIsAccessibilityElement:(bool)flag {
    env.objc.borrow_mut::<UIViewHostObject>(this).is_accessibility_element = flag;
}

- (u64)accessibilityTraits {
    env.objc.borrow::<UIViewHostObject>(this).accessibility_traits
}
- (())setAccessibilityTraits:(u64)traits {
    env.objc.borrow_mut::<UIViewHostObject>(this).accessibility_traits = traits;
}

- (id)accessibilityLabel {
    env.objc.borrow::<UIViewHostObject>(this).accessibility_label
}
- (())setAccessibilityLabel:(id)label {
    // Apple's docs declare this as `copy` (since iOS 5+), so deep-copy
    // the string rather than just retaining it. `copy` on an immutable
    // NSString just retains; on NSMutableString it makes a snapshot.
    let new_label: id = if label == nil { nil } else { msg![env; label copy] };
    let old = std::mem::replace(
        &mut env.objc.borrow_mut::<UIViewHostObject>(this).accessibility_label,
        new_label,
    );
    release(env, old);
}

- (id)accessibilityHint {
    env.objc.borrow::<UIViewHostObject>(this).accessibility_hint
}
- (())setAccessibilityHint:(id)hint {
    let new_hint: id = if hint == nil { nil } else { msg![env; hint copy] };
    let old = std::mem::replace(
        &mut env.objc.borrow_mut::<UIViewHostObject>(this).accessibility_hint,
        new_hint,
    );
    release(env, old);
}

- (id)accessibilityValue {
    env.objc.borrow::<UIViewHostObject>(this).accessibility_value
}
- (())setAccessibilityValue:(id)value {
    let new_value: id = if value == nil { nil } else { msg![env; value copy] };
    let old = std::mem::replace(
        &mut env.objc.borrow_mut::<UIViewHostObject>(this).accessibility_value,
        new_value,
    );
    release(env, old);
}

- (id)accessibilityIdentifier {
    env.objc.borrow::<UIViewHostObject>(this).accessibility_identifier
}
- (())setAccessibilityIdentifier:(id)identifier {
    let new_id: id = if identifier == nil { nil } else { msg![env; identifier copy] };
    let old = std::mem::replace(
        &mut env.objc.borrow_mut::<UIViewHostObject>(this).accessibility_identifier,
        new_id,
    );
    release(env, old);
}

- (id)accessibilityLanguage {
    env.objc.borrow::<UIViewHostObject>(this).accessibility_language
}
- (())setAccessibilityLanguage:(id)language {
    let new_lang: id = if language == nil { nil } else { msg![env; language copy] };
    let old = std::mem::replace(
        &mut env.objc.borrow_mut::<UIViewHostObject>(this).accessibility_language,
        new_lang,
    );
    release(env, old);
}

- (bool)accessibilityElementsHidden {
    env.objc.borrow::<UIViewHostObject>(this).accessibility_elements_hidden
}
- (())setAccessibilityElementsHidden:(bool)hidden {
    env.objc.borrow_mut::<UIViewHostObject>(this).accessibility_elements_hidden = hidden;
}

- (bool)accessibilityViewIsModal {
    env.objc.borrow::<UIViewHostObject>(this).accessibility_view_is_modal
}
- (())setAccessibilityViewIsModal:(bool)modal {
    env.objc.borrow_mut::<UIViewHostObject>(this).accessibility_view_is_modal = modal;
}

- (bool)shouldGroupAccessibilityChildren {
    env.objc.borrow::<UIViewHostObject>(this).should_group_accessibility_children
}
- (())setShouldGroupAccessibilityChildren:(bool)should {
    env.objc.borrow_mut::<UIViewHostObject>(this).should_group_accessibility_children = should;
}

- (())setTranslatesAutoresizingMaskIntoConstraints:(bool)_translates { }
- (bool)translatesAutoresizingMaskIntoConstraints { true }
- (())setNeedsLayout { }
// Apple's
// <https://developer.apple.com/documentation/uikit/uiview/1622450-setneedsupdateconstraints>:
// "Sets whether the view needs to update its constraints." Auto Layout is not
// emulated, so marking the view dirty and the follow-up passes are no-ops;
// implementing them keeps subclasses that drive them from warning.
- (())setNeedsUpdateConstraints { }
- (())updateConstraintsIfNeeded { }
- (())updateConstraints { }
- (())addConstraint:(id)_constraint { }
- (())addConstraints:(id)_constraints { }
- (())removeConstraint:(id)_constraint { }
- (())removeConstraints:(id)_constraints { }
- (id)constraints { msg_class![env; NSArray array] }

// Broad UIKit/Cocos compatibility. A lot of Cocos2D-era games call
// these optional UIView hooks directly on their GL view subclasses. The default
// UIView behavior is effectively no-op, but having the selectors prevents the
// dynamic dispatcher from treating harmless lifecycle notifications as missing
// methods.
- (())willMoveToSuperview:(id)_newSuperview { }
- (())didMoveToSuperview { }
- (())willMoveToWindow:(id)_newWindow { }
- (())didMoveToWindow { }
- (())didAddSubview:(id)_subview { }
- (())willRemoveSubview:(id)_subview { }
- (())setNeedsDisplayOnBoundsChange:(bool)_flag { }
- (bool)needsDisplayOnBoundsChange { false }
- (())setAutoresizesLayer:(bool)_flag { }
- (bool)autoresizesLayer { true }
- (())setLayerContentsPlacement:(NSInteger)_placement { }
- (NSInteger)layerContentsPlacement { 0 }
- (())setLayerContentsRedrawPolicy:(NSInteger)_policy { }
- (NSInteger)layerContentsRedrawPolicy { 0 }

// `UIAccessibilityContainer` informal protocol — `UIView` returns
// these no-op defaults in real iOS when nothing has been customised.
- (id)accessibilityElements { nil }
- (())setAccessibilityElements:(id)_elements {
    // Apple's UIView ignores the setter unless a subclass overrides
    // -accessibilityElements; we follow suit.
}
- (crate::frameworks::foundation::NSInteger)accessibilityElementCount { 0 }
- (id)accessibilityElementAtIndex:(crate::frameworks::foundation::NSInteger)_index { nil }
- (crate::frameworks::foundation::NSInteger)indexOfAccessibilityElement:(id)_element {
    // NSNotFound on iOS 32-bit == NSIntegerMax.
    crate::frameworks::foundation::NSInteger::MAX
}

- (())layoutSubviews {
    // Apple docs: "The default implementation uses any constraints you have
    // set to determine the size and position of any subviews." For legacy
    // autoresizing mask-based layout (iOS ≤ 5 era), the default implementation
    // adjusts subviews based on their autoresizingMask relative to changes
    // in the receiver's bounds.
    //
    // Autoresizing mask bits:
    //   UIViewAutoresizingFlexibleLeftMargin   = 1 << 0
    //   UIViewAutoresizingFlexibleWidth        = 1 << 1
    //   UIViewAutoresizingFlexibleRightMargin  = 1 << 2
    //   UIViewAutoresizingFlexibleTopMargin    = 1 << 3
    //   UIViewAutoresizingFlexibleHeight       = 1 << 4
    //   UIViewAutoresizingFlexibleBottomMargin = 1 << 5
    //
    // We only apply autoresizing if `autoresizesSubviews` is YES (default).
    let autoresizes: bool = msg![env; this autoresizesSubviews];
    if !autoresizes {
        return;
    }

    let bounds: CGRect = msg![env; this bounds];
    let subviews = env.objc.borrow::<UIViewHostObject>(this).subviews.clone();

    for subview in subviews {
        let mask: NSUInteger = msg![env; subview autoresizingMask];
        if mask == 0 {
            continue;
        }

        let frame: CGRect = msg![env; subview frame];
        let parent_w = bounds.size.width;
        let parent_h = bounds.size.height;

        // Determine how to distribute extra space horizontally.
        let flex_left = (mask & (1 << 0)) != 0;
        let flex_width = (mask & (1 << 1)) != 0;
        let flex_right = (mask & (1 << 2)) != 0;

        let mut new_x = frame.origin.x;
        let mut new_w = frame.size.width;

        let right_margin = parent_w - (frame.origin.x + frame.size.width);
        let h_flex_count =
            flex_left as i32 + flex_width as i32 + flex_right as i32;
        if h_flex_count > 0 {
            // For flexible width, expand to fill; for flexible margins, center.
            if flex_width && !flex_left && !flex_right {
                new_w = parent_w - frame.origin.x - right_margin;
            } else if flex_width && flex_left && flex_right {
                // All three flexible: distribute proportionally based on
                // original ratios. Simplified: just stretch width to fill.
                let total = frame.origin.x + frame.size.width + right_margin;
                if total > 0.0 {
                    let ratio_x = frame.origin.x / total;
                    let ratio_w = frame.size.width / total;
                    new_x = parent_w * ratio_x;
                    new_w = parent_w * ratio_w;
                }
            } else if flex_left && flex_right && !flex_width {
                // Flexible margins, fixed width: center.
                new_x = (parent_w - frame.size.width) / 2.0;
            } else if flex_width && flex_left {
                new_x = parent_w - new_w - right_margin;
                new_w = parent_w - new_x - right_margin;
            } else if flex_width && flex_right {
                new_w = parent_w - frame.origin.x - right_margin;
            } else if flex_left && !flex_width {
                new_x = parent_w - frame.size.width - right_margin;
            }
            // flex_right only: x and width stay the same
        }

        // Determine how to distribute extra space vertically.
        let flex_top = (mask & (1 << 3)) != 0;
        let flex_height = (mask & (1 << 4)) != 0;
        let flex_bottom = (mask & (1 << 5)) != 0;

        let mut new_y = frame.origin.y;
        let mut new_h = frame.size.height;

        let bottom_margin = parent_h - (frame.origin.y + frame.size.height);
        let v_flex_count =
            flex_top as i32 + flex_height as i32 + flex_bottom as i32;
        if v_flex_count > 0 {
            if flex_height && !flex_top && !flex_bottom {
                new_h = parent_h - frame.origin.y - bottom_margin;
            } else if flex_height && flex_top && flex_bottom {
                let total = frame.origin.y + frame.size.height + bottom_margin;
                if total > 0.0 {
                    let ratio_y = frame.origin.y / total;
                    let ratio_h = frame.size.height / total;
                    new_y = parent_h * ratio_y;
                    new_h = parent_h * ratio_h;
                }
            } else if flex_top && flex_bottom && !flex_height {
                new_y = (parent_h - frame.size.height) / 2.0;
            } else if flex_height && flex_top {
                new_y = parent_h - new_h - bottom_margin;
                new_h = parent_h - new_y - bottom_margin;
            } else if flex_height && flex_bottom {
                new_h = parent_h - frame.origin.y - bottom_margin;
            } else if flex_top && !flex_height {
                new_y = parent_h - frame.size.height - bottom_margin;
            }
        }

        let new_frame = CGRect {
            origin: CGPoint { x: new_x, y: new_y },
            size: CGSize { width: new_w.max(0.0), height: new_h.max(0.0) },
        };

        if new_frame != frame {
            () = msg![env; subview setFrame:new_frame];
        }
    }
}

// Per Apple docs: "Use this method to force the view to update its layout
// immediately. [...] This method acts on the root view of the receiver's
// subtree, laying out the entire subtree starting from that root."
// In our simplified implementation, we just call layoutSubviews.
- (())layoutIfNeeded {
    () = msg![env; this layoutSubviews];
}

// MARK: - Gesture recognizers
//
// These methods just track recognizers in a `Vec<id>`. Gesture recognition is
// not dispatched; this is enough to keep games from crashing on startup when
// they wire up `UIPinchGestureRecognizer` / `UITapGestureRecognizer` etc.

- (())addGestureRecognizer:(id)recognizer {
    if recognizer == nil { return; }
    retain(env, recognizer);
    env.objc.borrow_mut::<UIViewHostObject>(this).gesture_recognizers.push(recognizer);
    // Apple docs: -addGestureRecognizer: sets the recognizer's `view` to
    // the receiver. The recognizer holds this back-pointer weakly.
    // Use msg_send to set the view property so it goes through the ObjC
    // dispatch and handles subclass host objects correctly.
    let _: () = crate::objc::msg![env; recognizer setView:this];
}

- (())removeGestureRecognizer:(id)recognizer {
    if recognizer == nil { return; }
    let host = env.objc.borrow_mut::<UIViewHostObject>(this);
    if let Some(pos) = host.gesture_recognizers.iter().position(|&r| r == recognizer) {
        host.gesture_recognizers.remove(pos);
        // Apple docs: recognizer's view back-pointer is cleared on detach.
        let _: () = crate::objc::msg![env; recognizer setView:nil];
        release(env, recognizer);
    }
}

- (id)gestureRecognizers {
    let recognizers = env
        .objc
        .borrow::<UIViewHostObject>(this)
        .gesture_recognizers
        .clone();
    for r in &recognizers {
        retain(env, *r);
    }
    let array = ns_array::from_vec(env, recognizers);
    autorelease(env, array)
}

- (())setGestureRecognizers:(id)recognizers { // NSArray*
    // Per Apple docs: replaces the current set of recognizers. Iterate the
    // existing list, release each, replace, retain new ones.
    let old: Vec<id> =
        env.objc.borrow::<UIViewHostObject>(this).gesture_recognizers.clone();
    for r in &old {
        // Clear the recognizer's view back-pointer before releasing.
        super::ui_gesture_recognizer::set_view(env, *r, nil);
    }
    for r in old { release(env, r); }
    let mut new_list: Vec<id> = Vec::new();
    if recognizers != nil {
        let count: NSUInteger = msg![env; recognizers count];
        for i in 0..count {
            let r: id = msg![env; recognizers objectAtIndex:i];
            if r != nil { retain(env, r); new_list.push(r); }
        }
    }
    env.objc.borrow_mut::<UIViewHostObject>(this).gesture_recognizers = new_list.clone();
    for r in &new_list {
        super::ui_gesture_recognizer::set_view(env, *r, this);
    }
}

- (id)superview { env.objc.borrow::<UIViewHostObject>(this).superview }

- (bool)isDescendantOfView:(id)ancestor {
    if ancestor == nil {
        return false;
    }
    let mut current = this;
    for _ in 0..256 {
        if current == nil {
            return false;
        }
        if current == ancestor {
            return true;
        }
        current = msg![env; current superview];
    }
    false
}

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

    // Doodle Jump launch cross-promo workaround.
    //
    // At startup Doodle Jump runs a queue of popups (age gate, ATT tracking,
    // "what's new"). Only after the last one closes does it call
    // `continueInitializationAfterPopups`, which creates the cocos2d game
    // scene and starts the render loop. The "what's new" popup is an
    // `ImagePopUpView` whose close button only un-hides once its embedded
    // `UIWebView` finishes loading remote content. With no web backend (and
    // offline) that content never arrives, so the close button stays hidden,
    // the popup can never be dismissed, and the game is left on a black menu
    // with the cocos2d scene never created.
    //
    // Mirror the `AgeGateView` auto-complete: the first time the popup is
    // added, schedule its removal and the initialization continuation on the
    // run loop so the game proceeds to its real (fully rendered) menu.
    if env.bundle.bundle_identifier() == "com.yourcompany.DoodleJump" {
        let view_class: Class = msg![env; view class];
        if env.objc.get_class_name(view_class) == "ImagePopUpView"
            && !env.framework_state.uikit.ui_view.doodle_jump_popup_dismissed
        {
            env.framework_state.uikit.ui_view.doodle_jump_popup_dismissed = true;
            let app: id = msg_class![env; UIApplication sharedApplication];
            let delegate: id = msg![env; app delegate];
            if delegate != nil
                && env.objc.object_has_method_named(
                    &env.mem,
                    delegate,
                    "continueInitializationAfterPopups",
                )
            {
                if let Some(cont) = env.objc.lookup_selector("continueInitializationAfterPopups") {
                    log!(
                        "Doodle Jump: auto-dismissing offline cross-promo popup and \
                         continuing initialization"
                    );
                    if let Some(remove) = env.objc.lookup_selector("removeFromSuperview") {
                        () = msg![env; view performSelector:remove
                                                   withObject:nil
                                                    afterDelay:2.5f64];
                    }
                    () = msg![env; delegate performSelector:cont
                                                   withObject:nil
                                                    afterDelay:2.6f64];
                }
            }
        }
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
    // Apple's UIView silently ignores -insertSubview:atIndex: when `view` is
    // nil, so do the same instead of asserting. Ancient War (and likely
    // other games) call this with a nil placeholder during interface
    // construction.
    if view == nil { return; }
    retain(env, view);
    () = msg![env; view removeFromSuperview];

    let subview_obj = env.objc.borrow_mut::<UIViewHostObject>(view);
    subview_obj.superview = this;
    let subview_layer = subview_obj.layer;

    let &mut UIViewHostObject { ref mut subviews, layer: this_layer, .. } = env.objc.borrow_mut(this);
    let clamped_index = if index < 0 {
        0
    } else {
        (index as usize).min(subviews.len())
    };
    subviews.insert(clamped_index, view);

    () = msg![env; this_layer insertSublayer:subview_layer atIndex:(clamped_index as u32)];
}

- (())insertSubview:(id)view belowSubview:(id)sibling {
    if view == nil { return; }
    retain(env, view);
    () = msg![env; view removeFromSuperview];

    let subview_obj = env.objc.borrow_mut::<UIViewHostObject>(view);
    subview_obj.superview = this;
    let subview_layer = subview_obj.layer;

    let sibling_layer = if sibling != nil {
        env.objc.borrow_mut::<UIViewHostObject>(sibling).layer
    } else {
        crate::objc::nil
    };

    let &mut UIViewHostObject { ref mut subviews, layer: this_layer, .. } = env.objc.borrow_mut(this);
    let idx = subviews
        .iter()
        .position(|&subview2| subview2 == sibling)
        .unwrap_or(subviews.len());
    subviews.insert(idx, view);

    if sibling_layer != crate::objc::nil {
        () = msg![env; this_layer insertSublayer:subview_layer below:sibling_layer];
    } else {
        () = msg![env; this_layer addSublayer:subview_layer];
    }
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

- (())exchangeSubviewAtIndex:(NSInteger)index1 withSubviewAtIndex:(NSInteger)index2 {
    let &mut UIViewHostObject { ref mut subviews, layer, .. } = env.objc.borrow_mut(this);
    if index1 < 0 || index2 < 0 { return; }
    let i1 = index1 as usize;
    let i2 = index2 as usize;
    if i1 >= subviews.len() || i2 >= subviews.len() || i1 == i2 { return; }
    subviews.swap(i1, i2);

    // Keep CALayer order roughly in sync. The exact UIKit implementation is
    // more subtle, but rebuilding the sublayer order is enough for old Cocos
    // menu stacks and avoids crashes from missing exchangeSubviewAtIndex:.
    let ordered = subviews.clone();
    for subview in &ordered {
        let subview_layer = env.objc.borrow::<UIViewHostObject>(*subview).layer;
        let _: () = msg![env; subview_layer removeFromSuperlayer];
    }
    for subview in &ordered {
        let subview_layer = env.objc.borrow::<UIViewHostObject>(*subview).layer;
        let _: () = msg![env; layer addSublayer:subview_layer];
    }
}

- (())removeFromSuperview {
    let &mut UIViewHostObject { ref mut superview, layer: this_layer, .. } = env.objc.borrow_mut(this);
    let superview = std::mem::take(superview);
    if superview == nil { return; }
    let _: () = msg![env; this_layer removeFromSuperlayer];

    let superview_obj = env.objc.borrow_mut::<UIViewHostObject>(superview);
    let subviews = &mut superview_obj.subviews;

    if let Some(idx) = subviews.iter().position(|&subview| subview == this) {
        let subview = subviews.remove(idx);
        assert!(subview == this);
        release(env, this);
    }
}

- (())dealloc {
    let superview = env.objc.borrow::<UIViewHostObject>(this).superview;
    if superview != nil {
        log!("Warning: {:?} was deallocated while in superview {:?}", this, superview);
        {
            let siblings = &mut env.objc.borrow_mut::<UIViewHostObject>(superview).subviews;
            siblings.retain(|&sibling| sibling != this);
        }
        let layer = env.objc.borrow::<UIViewHostObject>(this).layer;
        () = msg![env; layer removeFromSuperlayer];
    }

    let UIViewHostObject {
        layer, subviews, gesture_recognizers,
        accessibility_label, accessibility_hint, accessibility_value,
        accessibility_identifier, accessibility_language,
        ..
    } = std::mem::take(env.objc.borrow_mut(this));
    release(env, layer);

    for subview in subviews {
        env.objc.borrow_mut::<UIViewHostObject>(subview).superview = nil;
        release(env, subview);
    }
    for recognizer in gesture_recognizers {
        super::ui_gesture_recognizer::set_view(env, recognizer, nil);
        release(env, recognizer);
    }

    // UIAccessibility informal protocol: properties are documented as
    // "copy" / "retain" — release them on teardown to match
    // <https://developer.apple.com/documentation/objectivec/nsobject/uiaccessibility>.
    release(env, accessibility_label);
    release(env, accessibility_hint);
    release(env, accessibility_value);
    release(env, accessibility_identifier);
    release(env, accessibility_language);

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
    let host = env.objc.borrow_mut::<UIViewHostObject>(this);
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

- (CGFloat)contentScaleFactor { env.objc.borrow::<UIViewHostObject>(this).content_scale_factor }
- (())setContentScaleFactor:(CGFloat)scale {
    let safe_scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    env.objc.borrow_mut::<UIViewHostObject>(this).content_scale_factor = safe_scale;
    let layer = env.objc.borrow::<UIViewHostObject>(this).layer;
    if let Some(sel) = env.objc.lookup_selector("setContentsScale:") {
        let _: () = msg_send_no_type_checking(env, (layer, sel, safe_scale));
    }
}

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

- (())setNeedsDisplayInRect:(CGRect)invalid_rect {
    // Apple docs (UIView Reference, "Drawing and Updating the View"):
    //   "Marks the specified rectangle of the receiver as needing to be
    //    redrawn. invalidRect: The rectangular region of the receiver to
    //    mark as invalid; it should be specified in the coordinate system
    //    of the receiver."
    //
    // The view delegates the dirty rectangle to its backing CALayer; the
    // next display cycle will only repaint the union of pending invalid
    // rects (CALayer.setNeedsDisplayInRect: is documented to coalesce).
    // We keep the same fast-out as -setNeedsDisplay: if neither -drawRect:
    // nor -drawLayer:inContext: is overridden by this view's class, there
    // is nothing to repaint and we can save the layer round-trip.
    let this_class = ObjC::read_isa(this, &env.mem);
    let ui_view_class = env.objc.get_known_class("UIView", &mut env.mem);

    let draw_layer_sel = env.objc.lookup_selector("drawLayer:inContext:").unwrap();
    let draw_rect_sel = env.objc.lookup_selector("drawRect:").unwrap();

    if !(env.objc.class_overrides_method_of_superclass(this_class, draw_rect_sel, ui_view_class)
        || env
            .objc
            .class_overrides_method_of_superclass(this_class, draw_layer_sel, ui_view_class))
    {
        return;
    }

    let layer = env.objc.borrow::<UIViewHostObject>(this).layer;
    () = msg![env; layer setNeedsDisplayInRect:invalid_rect]
}

- (CGRect)bounds {
    let layer = env.objc.borrow::<UIViewHostObject>(this).layer;
    msg![env; layer bounds]
}
- (())setBounds:(CGRect)bounds {
    let mut bounds = touchhle_cocos_sanitize_rect(bounds);

    if crate::env_flag_cached!("TOUCHHLE_FORCE_LANDSCAPE_VIEW_BOUNDS") {
        let view_class: Class = msg![env; this class];
        let class_name = env.objc.get_class_name(view_class).to_owned();
        if class_name == "UIWindow" || class_name.contains("EAGLView") {
            let w = bounds.size.width.round() as i32;
            let h = bounds.size.height.round() as i32;
            if (w == 320 && (h == 460 || h == 480)) || (w == 0 && h == 0) {
                log!(
                    "TOUCHHLE_FORCE_LANDSCAPE_VIEW_BOUNDS=1: coercing setBounds for {} {:?} from {:?} to 480x320",
                    class_name,
                    this,
                    bounds
                );
                bounds = CGRect {
                    origin: CGPoint { x: 0.0, y: 0.0 },
                    size: CGSize {
                        width: 480.0,
                        height: 320.0,
                    },
                };
            }
        }
    }

    let layer = env.objc.borrow::<UIViewHostObject>(this).layer;
    let old_bounds: CGRect = msg![env; layer bounds];
    () = msg![env; layer setBounds:bounds];
    resize_subviews_with_old_size(env, this, old_bounds.size);
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
    // ULTRAHLE_MINIONJUMP_FRAME_BEGIN
    if ultrahle_minionjump_force_landscape_ccglview(env, this) {
        return touchhle_cocos_landscape_rect(env);
    }
    // ULTRAHLE_MINIONJUMP_FRAME_END

    let layer = env.objc.borrow::<UIViewHostObject>(this).layer;
    msg![env; layer frame]
}
- (())setFrame:(CGRect)frame {
    // ULTRAHLE_MINIONJUMP_SETFRAME_BEGIN
    let frame = if ultrahle_minionjump_force_landscape_ccglview(env, this) {
        touchhle_cocos_landscape_rect(env)
    } else {
        frame
    };
    // ULTRAHLE_MINIONJUMP_SETFRAME_END

    let mut frame = touchhle_cocos_sanitize_rect(frame);

    if crate::env_flag_cached!("TOUCHHLE_FORCE_LANDSCAPE_VIEW_BOUNDS") {
        let view_class: Class = msg![env; this class];
        let class_name = env.objc.get_class_name(view_class).to_owned();
        if class_name == "UIWindow" || class_name.contains("EAGLView") {
            let w = frame.size.width.round() as i32;
            let h = frame.size.height.round() as i32;
            if (w == 320 && (h == 460 || h == 480)) || (w == 0 && h == 0) {
                log!(
                    "TOUCHHLE_FORCE_LANDSCAPE_VIEW_BOUNDS=1: coercing setFrame for {} {:?} from {:?} to 480x320",
                    class_name,
                    this,
                    frame
                );
                frame = CGRect {
                    origin: CGPoint { x: 0.0, y: 0.0 },
                    size: CGSize {
                        width: 480.0,
                        height: 320.0,
                    },
                };
            }
        }
    }

    let layer = env.objc.borrow::<UIViewHostObject>(this).layer;
    let old_bounds: CGRect = msg![env; layer bounds];
    () = msg![env; layer setFrame:frame];
    resize_subviews_with_old_size(env, this, old_bounds.size);
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
    let inside: bool = msg![env; layer containsPoint:point];
    if inside { return true; }

    if touchhle_cocos_should_fuzz_hit_testing(env, this) {
        let bounds: CGRect = msg![env; this bounds];
        // PERF: parsed once; this runs for every fuzzy hit test.
        static SLOP: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
        let inset = *SLOP.get_or_init(|| {
            std::env::var("TOUCHHLE_COCOS_HITTEST_SLOP")
                .ok()
                .and_then(|v| v.parse::<f32>().ok())
                .unwrap_or(12.0)
        });
        return point.x >= bounds.origin.x - inset
            && point.y >= bounds.origin.y - inset
            && point.x <= bounds.origin.x + bounds.size.width + inset
            && point.y <= bounds.origin.y + bounds.size.height + inset;
    }

    false
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
    if !force { return false; }
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

- (id)traitCollection {
    let window: id = msg![env; this window];
    let screen: id = if window == nil {
        msg_class![env; UIScreen mainScreen]
    } else {
        msg![env; window screen]
    };
    msg![env; screen traitCollection]
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

// MARK: - Block-invocation helpers
//
// Apple Blocks ABI: a block is a guest pointer to a `Block_layout`
// struct whose `invoke` field lives at byte offset 12 on 32-bit ARM
// (after `isa` at +0, `flags` at +4, `reserved` at +8). `invoke` is a
// function whose first argument is the block pointer itself; any
// additional arguments come after.
//
// References:
// - Apple [Block Implementation Specification](https://clang.llvm.org/docs/Block-ABI-Apple.html)

const BLOCK_INVOKE_OFFSET: u32 = 12;

/// Invoke a block whose underlying function signature is `void (^)(void)`.
fn invoke_void_block(env: &mut Environment, block: MutPtr<()>) {
    if block.is_null() {
        return;
    }
    let invoke_ptr_addr: MutPtr<u32> =
        crate::mem::Ptr::from_bits(block.to_bits() + BLOCK_INVOKE_OFFSET);
    let invoke_addr: u32 = env.mem.read(invoke_ptr_addr);
    if invoke_addr == 0 {
        log!(
            "Warning: invoke_void_block: block at {:?} has NULL invoke pointer; skipping.",
            block
        );
        return;
    }
    let func = crate::abi::GuestFunction::from_addr_with_thumb_bit(invoke_addr);
    let block_arg: crate::mem::ConstVoidPtr =
        crate::mem::Ptr::from_bits(block.to_bits()).cast_const();
    use crate::abi::CallFromHost;
    <crate::abi::GuestFunction as CallFromHost<(), (crate::mem::ConstVoidPtr,)>>::call_from_host(
        &func,
        env,
        (block_arg,),
    );
}

/// Invoke a block whose signature is `void (^)(BOOL finished)`.
fn invoke_bool_block(env: &mut Environment, block: MutPtr<()>, arg: bool) {
    if block.is_null() {
        return;
    }
    let invoke_ptr_addr: MutPtr<u32> =
        crate::mem::Ptr::from_bits(block.to_bits() + BLOCK_INVOKE_OFFSET);
    let invoke_addr: u32 = env.mem.read(invoke_ptr_addr);
    if invoke_addr == 0 {
        log!(
            "Warning: invoke_bool_block: block at {:?} has NULL invoke pointer; skipping.",
            block
        );
        return;
    }
    let func = crate::abi::GuestFunction::from_addr_with_thumb_bit(invoke_addr);
    let block_arg: crate::mem::ConstVoidPtr =
        crate::mem::Ptr::from_bits(block.to_bits()).cast_const();
    use crate::abi::CallFromHost;
    <crate::abi::GuestFunction as CallFromHost<(), (crate::mem::ConstVoidPtr, bool)>>::call_from_host(
        &func, env, (block_arg, arg),
    );
}

#[cfg(test)]
mod autoresize_tests {
    use super::autoresize_axis;

    #[test]
    fn flexible_size_absorbs_the_whole_change() {
        // A 320x460 view filling its portrait superview, rotated to landscape.
        assert_eq!(
            autoresize_axis(0.0, 320.0, 320.0, 480.0, (false, true, false)),
            (0.0, 480.0)
        );
        assert_eq!(
            autoresize_axis(0.0, 460.0, 460.0, 320.0, (false, true, false)),
            (0.0, 320.0)
        );
    }

    #[test]
    fn change_is_split_in_proportion_to_flexible_parts() {
        // Flexible margins keep a 100-wide view centred.
        assert_eq!(
            autoresize_axis(110.0, 100.0, 320.0, 480.0, (true, false, true)),
            (190.0, 100.0)
        );
        // A fixed view with a flexible leading margin stays pinned right.
        assert_eq!(
            autoresize_axis(220.0, 100.0, 320.0, 480.0, (true, false, false)),
            (380.0, 100.0)
        );
        // Zero-length flexible parts share the change evenly.
        assert_eq!(
            autoresize_axis(0.0, 0.0, 0.0, 100.0, (true, true, true)),
            (100.0 / 3.0, 100.0 / 3.0)
        );
        assert_eq!(
            autoresize_axis(5.0, 10.0, 20.0, 40.0, (false, false, false)),
            (5.0, 10.0)
        );
    }
}
