/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UIKeyboard` — host soft keyboard bridge and notification constants.

use crate::dyld::{ConstantExports, HostConstant};
use crate::frameworks::core_graphics::CGSize;
use crate::frameworks::foundation::NSInteger;
use crate::objc::{id, msg_class, nil, objc_classes, ClassExports, TrivialHostObject};

// MARK: - Notification names

pub const UIKeyboardWillShowNotification: &str = "UIKeyboardWillShowNotification";
pub const UIKeyboardDidShowNotification: &str = "UIKeyboardDidShowNotification";
pub const UIKeyboardWillHideNotification: &str = "UIKeyboardWillHideNotification";
pub const UIKeyboardDidHideNotification: &str = "UIKeyboardDidHideNotification";
pub const UIKeyboardWillChangeFrameNotification: &str = "UIKeyboardWillChangeFrameNotification";
pub const UIKeyboardDidChangeFrameNotification: &str = "UIKeyboardDidChangeFrameNotification";

// MARK: - UserInfo keys (iOS 3.2+)

pub const UIKeyboardFrameBeginUserInfoKey: &str = "UIKeyboardFrameBeginUserInfoKey";
pub const UIKeyboardFrameEndUserInfoKey: &str = "UIKeyboardFrameEndUserInfoKey";
pub const UIKeyboardAnimationDurationUserInfoKey: &str = "UIKeyboardAnimationDurationUserInfoKey";
pub const UIKeyboardAnimationCurveUserInfoKey: &str = "UIKeyboardAnimationCurveUserInfoKey";
pub const UIKeyboardIsLocalUserInfoKey: &str = "UIKeyboardIsLocalUserInfoKey";

// MARK: - Legacy userInfo keys (pre-iOS 3.2, still seen in old apps)

pub const UIKeyboardBoundsUserInfoKey: &str = "UIKeyboardBoundsUserInfoKey";
pub const UIKeyboardCenterBeginUserInfoKey: &str = "UIKeyboardCenterBeginUserInfoKey";
pub const UIKeyboardCenterEndUserInfoKey: &str = "UIKeyboardCenterEndUserInfoKey";

// MARK: - Keyboard type / appearance / return key

type UIKeyboardType = NSInteger;
const UIKeyboardTypeDefault: UIKeyboardType = 0;
const UIKeyboardTypeASCIICapable: UIKeyboardType = 1;
const UIKeyboardTypeNumbersAndPunctuation: UIKeyboardType = 2;
const UIKeyboardTypeURL: UIKeyboardType = 3;
const UIKeyboardTypeNumberPad: UIKeyboardType = 4;
const UIKeyboardTypePhonePad: UIKeyboardType = 5;
const UIKeyboardTypeNamePhonePad: UIKeyboardType = 6;
const UIKeyboardTypeEmailAddress: UIKeyboardType = 7;
const UIKeyboardTypeDecimalPad: UIKeyboardType = 8;
const UIKeyboardTypeTwitter: UIKeyboardType = 9;
const UIKeyboardTypeWebSearch: UIKeyboardType = 10;

type UIKeyboardAppearance = NSInteger;
const UIKeyboardAppearanceDefault: UIKeyboardAppearance = 0;
const UIKeyboardAppearanceDark: UIKeyboardAppearance = 1;
const UIKeyboardAppearanceLight: UIKeyboardAppearance = 2;
const UIKeyboardAppearanceAlert: UIKeyboardAppearance = UIKeyboardAppearanceDark;

type UIReturnKeyType = NSInteger;
const UIReturnKeyDefault: UIReturnKeyType = 0;
const UIReturnKeyGo: UIReturnKeyType = 1;
const UIReturnKeyGoogle: UIReturnKeyType = 2;
const UIReturnKeyJoin: UIReturnKeyType = 3;
const UIReturnKeyNext: UIReturnKeyType = 4;
const UIReturnKeyRoute: UIReturnKeyType = 5;
const UIReturnKeySearch: UIReturnKeyType = 6;
const UIReturnKeySend: UIReturnKeyType = 7;
const UIReturnKeyYahoo: UIReturnKeyType = 8;
const UIReturnKeyDone: UIReturnKeyType = 9;
const UIReturnKeyEmergencyCall: UIReturnKeyType = 10;
const UIReturnKeyContinue: UIReturnKeyType = 11;

type UITextAutocapitalizationType = NSInteger;
const UITextAutocapitalizationTypeNone: UITextAutocapitalizationType = 0;
const UITextAutocapitalizationTypeWords: UITextAutocapitalizationType = 1;
const UITextAutocapitalizationTypeSentences: UITextAutocapitalizationType = 2;
const UITextAutocapitalizationTypeAllCharacters: UITextAutocapitalizationType = 3;

type UITextAutocorrectionType = NSInteger;
const UITextAutocorrectionTypeDefault: UITextAutocorrectionType = 0;
const UITextAutocorrectionTypeNo: UITextAutocorrectionType = 1;
const UITextAutocorrectionTypeYes: UITextAutocorrectionType = 2;

type UITextSpellCheckingType = NSInteger;
const UITextSpellCheckingTypeDefault: UITextSpellCheckingType = 0;
const UITextSpellCheckingTypeNo: UITextSpellCheckingType = 1;
const UITextSpellCheckingTypeYes: UITextSpellCheckingType = 2;

pub const CONSTANTS: ConstantExports = &[
    // Notification names
    (
        "_UIKeyboardWillShowNotification",
        HostConstant::NSString(UIKeyboardWillShowNotification),
    ),
    (
        "_UIKeyboardDidShowNotification",
        HostConstant::NSString(UIKeyboardDidShowNotification),
    ),
    (
        "_UIKeyboardWillHideNotification",
        HostConstant::NSString(UIKeyboardWillHideNotification),
    ),
    (
        "_UIKeyboardDidHideNotification",
        HostConstant::NSString(UIKeyboardDidHideNotification),
    ),
    (
        "_UIKeyboardWillChangeFrameNotification",
        HostConstant::NSString(UIKeyboardWillChangeFrameNotification),
    ),
    (
        "_UIKeyboardDidChangeFrameNotification",
        HostConstant::NSString(UIKeyboardDidChangeFrameNotification),
    ),
    // UserInfo keys
    (
        "_UIKeyboardFrameBeginUserInfoKey",
        HostConstant::NSString(UIKeyboardFrameBeginUserInfoKey),
    ),
    (
        "_UIKeyboardFrameEndUserInfoKey",
        HostConstant::NSString(UIKeyboardFrameEndUserInfoKey),
    ),
    (
        "_UIKeyboardAnimationDurationUserInfoKey",
        HostConstant::NSString(UIKeyboardAnimationDurationUserInfoKey),
    ),
    (
        "_UIKeyboardAnimationCurveUserInfoKey",
        HostConstant::NSString(UIKeyboardAnimationCurveUserInfoKey),
    ),
    (
        "_UIKeyboardIsLocalUserInfoKey",
        HostConstant::NSString(UIKeyboardIsLocalUserInfoKey),
    ),
    // Legacy keys
    (
        "_UIKeyboardBoundsUserInfoKey",
        HostConstant::NSString(UIKeyboardBoundsUserInfoKey),
    ),
    (
        "_UIKeyboardCenterBeginUserInfoKey",
        HostConstant::NSString(UIKeyboardCenterBeginUserInfoKey),
    ),
    (
        "_UIKeyboardCenterEndUserInfoKey",
        HostConstant::NSString(UIKeyboardCenterEndUserInfoKey),
    ),
];

/// State for the private keyboard classes. These expose `+sharedInstance`,
/// which (like all Cocoa singletons) must return the *same* object on every
/// call. The previous implementation allocated a brand-new object each time,
/// so old apps that call `[UIKeyboard sharedInstance]` (or
/// `[UIKeyboardImpl sharedInstance]`) repeatedly — sometimes recursively, via
/// `+initialize`/`retain` chains — kept manufacturing fresh objects. That
/// churn is what tripped the `objc_msgSend` recursion guard (bailing out with
/// a nil return) and left the app messaging a half-built/freed object.
#[derive(Default)]
pub struct State {
    ui_keyboard_shared_instance: Option<id>,
    ui_keyboard_impl_shared_instance: Option<id>,
}

pub fn start_text_input(env: &mut crate::Environment) {
    if env.window.is_some() {
        env.on_parent_stack_in_coroutine(|window, _| window.start_text_input());
    }
}

pub fn stop_text_input(env: &mut crate::Environment) {
    if env.window.is_some() {
        env.on_parent_stack_in_coroutine(|window, _| window.stop_text_input());
    }
}

pub fn is_text_input_active(env: &mut crate::Environment) -> bool {
    if env.window.is_none() {
        return false;
    }
    let mut active = false;
    env.on_parent_stack_in_coroutine(|window, _| active = window.is_text_input_active());
    active
}

pub fn is_screen_keyboard_shown(env: &mut crate::Environment) -> bool {
    if env.window.is_none() {
        return false;
    }
    let mut shown = false;
    env.on_parent_stack_in_coroutine(|window, _| shown = window.is_screen_keyboard_shown());
    shown
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

// MARK: - UIKeyboard (private internal class, seen in old apps via NIBs)

@implementation UIKeyboard: UIView

+ (id)sharedInstance {
    if let Some(existing) = env.framework_state.uikit.ui_keyboard.ui_keyboard_shared_instance {
        return existing;
    }
    let instance = env.objc.alloc_static_object(this, Box::new(TrivialHostObject), &mut env.mem);
    env.framework_state.uikit.ui_keyboard.ui_keyboard_shared_instance = Some(instance);
    instance
}

+ (bool)isInHardwareKeyboardMode {
    false
}

+ (bool)isOnScreen {
    is_screen_keyboard_shown(env)
}

+ (CGSize)defaultSizeForOrientation:(NSInteger)_orientation {
    // Standard iPhone keyboard size in portrait.
    CGSize { width: 320.0, height: 216.0 }
}

+ (CGSize)defaultSizeForInterfaceOrientation:(NSInteger)_orientation {
    CGSize { width: 320.0, height: 216.0 }
}

- (())orderInWithAnimation:(bool)_animated {
    start_text_input(env);
}

- (())orderOutWithAnimation:(bool)_animated {
    stop_text_input(env);
}

- (())activate {
    start_text_input(env);
}

- (())deactivate {
    stop_text_input(env);
}

- (bool)isVisible {
    is_screen_keyboard_shown(env)
}

@end

// MARK: - UIKeyboardImpl (private, accessed by some apps)

@implementation UIKeyboardImpl: NSObject

+ (id)sharedInstance {
    if let Some(existing) = env.framework_state.uikit.ui_keyboard.ui_keyboard_impl_shared_instance {
        return existing;
    }
    let instance = env.objc.alloc_static_object(this, Box::new(TrivialHostObject), &mut env.mem);
    env.framework_state.uikit.ui_keyboard.ui_keyboard_impl_shared_instance = Some(instance);
    instance
}

+ (id)activeInstance {
    if is_text_input_active(env) {
        msg_class![env; this sharedInstance]
    } else {
        nil
    }
}

- (())orderInWithAnimation:(bool)_animated {
    start_text_input(env);
}

- (())orderOutWithAnimation:(bool)_animated {
    stop_text_input(env);
}

- (())activate {
    start_text_input(env);
}

- (())deactivate {
    stop_text_input(env);
}

- (bool)isVisible {
    is_screen_keyboard_shown(env)
}

- (())setDelegate:(id)_delegate {
    log!("UIKeyboardImpl setDelegate: stubbed");
}

- (id)delegate {
    nil
}

- (())setReturnKeyType:(UIReturnKeyType)_type {
    // Stub.
}

- (UIReturnKeyType)returnKeyType {
    UIReturnKeyDefault
}

- (())setKeyboardType:(UIKeyboardType)_type {
    // Stub.
}

- (UIKeyboardType)keyboardType {
    UIKeyboardTypeDefault
}

- (())setKeyboardAppearance:(UIKeyboardAppearance)_appearance {
    // Stub.
}

- (UIKeyboardAppearance)keyboardAppearance {
    UIKeyboardAppearanceDefault
}

- (())setAutocorrectionType:(UITextAutocorrectionType)_type {
    // Stub.
}

- (UITextAutocorrectionType)autocorrectionType {
    UITextAutocorrectionTypeDefault
}

- (())setAutocapitalizationType:(UITextAutocapitalizationType)_type {
    // Stub.
}

- (UITextAutocapitalizationType)autocapitalizationType {
    UITextAutocapitalizationTypeNone
}

- (())setSpellCheckingType:(UITextSpellCheckingType)_type {
    // Stub.
}

- (UITextSpellCheckingType)spellCheckingType {
    UITextSpellCheckingTypeDefault
}

- (())setSecureTextEntry:(bool)_secure {
    // Stub.
}

- (bool)isSecureTextEntry {
    false
}

- (())updateForChangedSelection {
    // Stub.
}

- (())clearAutocorrection {
    // Stub.
}

@end

// MARK: - UITextInputTraits category stub (implemented as a standalone class
//          so other classes can reference these property names)

@implementation UITextInputTraits: NSObject
@end

};

/// Helper called by text-input views (UITextField, UITextView) to post the
/// standard keyboard-will/did-show notifications with an empty frame userInfo.
/// The host soft keyboard itself is shown through SDL text input.
pub fn post_keyboard_notifications(env: &mut crate::Environment, will_show: bool) {
    use crate::frameworks::foundation::ns_string::get_static_str;
    use crate::objc::msg;

    let (will_name, did_name) = if will_show {
        (
            UIKeyboardWillShowNotification,
            UIKeyboardDidShowNotification,
        )
    } else {
        (
            UIKeyboardWillHideNotification,
            UIKeyboardDidHideNotification,
        )
    };

    let app: crate::objc::id = msg_class![env; UIApplication sharedApplication];
    let center: crate::objc::id = msg_class![env; NSNotificationCenter defaultCenter];

    let user_info: crate::objc::id = msg_class![env; NSMutableDictionary new];

    // iOS puts the keyboard's geometry in the notification's userInfo, and
    // apps use it to move their own content out of the way:
    //
    //   UIKeyboardFrameBeginUserInfoKey  — NSValue(CGRect), starting frame
    //   UIKeyboardFrameEndUserInfoKey    — NSValue(CGRect), final frame
    //   UIKeyboardAnimationDurationUserInfoKey — NSNumber, seconds
    //   UIKeyboardAnimationCurveUserInfoKey    — NSNumber, UIViewAnimationCurve
    //
    // Both frames are in *screen* coordinates and describe the keyboard's
    // rectangle at the bottom of the display. Posting an empty dictionary, as
    // this used to do, leaves apps reading a nil frame: Minecraft PE 0.16.2
    // asks for `UIKeyboardFrameBeginUserInfoKey` and, with no frame, never
    // considers the keyboard visible — so every typed character it receives is
    // discarded (issue reported for 0.16.2).
    //
    // https://developer.apple.com/documentation/uikit/uikeyboardframeenduserinfokey
    let screen: crate::objc::id = msg_class![env; UIScreen mainScreen];
    let bounds: crate::frameworks::core_graphics::CGRect = msg![env; screen bounds];
    let (screen_width, screen_height) = (bounds.size.width, bounds.size.height);
    // The simulated keyboard is the iPhone one: it takes the bottom of the
    // display. Apple's on-screen keyboard is ~216 points tall in portrait and
    // ~162 in landscape on this device class.
    let keyboard_height = if screen_width > screen_height {
        162.0
    } else {
        216.0
    };
    let visible_frame = crate::frameworks::core_graphics::CGRect {
        origin: crate::frameworks::core_graphics::CGPoint {
            x: 0.0,
            y: screen_height - keyboard_height,
        },
        size: crate::frameworks::core_graphics::CGSize {
            width: screen_width,
            height: keyboard_height,
        },
    };
    // Hidden: same size, pushed just below the display.
    let hidden_frame = crate::frameworks::core_graphics::CGRect {
        origin: crate::frameworks::core_graphics::CGPoint {
            x: 0.0,
            y: screen_height,
        },
        size: visible_frame.size,
    };
    let (begin_frame, end_frame) = if will_show {
        (hidden_frame, visible_frame)
    } else {
        (visible_frame, hidden_frame)
    };
    let begin_value: crate::objc::id = msg_class![env; NSValue valueWithCGRect:begin_frame];
    let end_value: crate::objc::id = msg_class![env; NSValue valueWithCGRect:end_frame];
    let duration_value: crate::objc::id = msg_class![env; NSNumber numberWithDouble:0.25];
    let curve_value: crate::objc::id = msg_class![env; NSNumber numberWithInteger:7];
    let begin_key = get_static_str(env, UIKeyboardFrameBeginUserInfoKey);
    let end_key = get_static_str(env, UIKeyboardFrameEndUserInfoKey);
    let duration_key = get_static_str(env, UIKeyboardAnimationDurationUserInfoKey);
    let curve_key = get_static_str(env, UIKeyboardAnimationCurveUserInfoKey);
    let _: () = msg![env; user_info setObject:begin_value forKey:begin_key];
    let _: () = msg![env; user_info setObject:end_value forKey:end_key];
    let _: () = msg![env; user_info setObject:duration_value forKey:duration_key];
    let _: () = msg![env; user_info setObject:curve_value forKey:curve_key];

    for name in [will_name, did_name] {
        let ns_name = get_static_str(env, name);
        let _: () = msg![env; center postNotificationName:ns_name
                                                   object:app
                                                 userInfo:user_info];
    }

    crate::objc::release(env, user_info);
}
