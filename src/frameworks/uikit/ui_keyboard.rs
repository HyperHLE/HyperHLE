/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UIKeyboard` — keyboard appearance/dismissal stubs and notification
//! constants.

use crate::dyld::{ConstantExports, HostConstant};
use crate::frameworks::core_graphics::{CGPoint, CGRect, CGSize};
use crate::frameworks::foundation::{ns_string, NSInteger};
use crate::frameworks::uikit::ui_view::ui_control::ui_text_field::{
    handle_backspace, handle_return, handle_text,
};
use crate::frameworks::uikit::ui_view::ui_control::{
    UIControlEventTouchUpInside, UIControlStateNormal,
};
use crate::frameworks::uikit::ui_view::ui_control::ui_button::UIButtonTypeCustom;
use crate::objc::{
    id, msg, msg_class, nil, objc_classes, release, ClassExports, HostObject, NSZonePtr,
    TrivialHostObject,
};
use crate::Environment;

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
    /// The lazily-created on-screen keyboard controller (`_touchHLE_Keyboard*`).
    onscreen: Option<id>,
    /// Whether the on-screen keyboard is currently attached to a window.
    onscreen_visible: bool,
}

// MARK: - On-screen keyboard (drawn iOS 5-style keyboard)

/// The logical action performed by tapping a key.
#[derive(Clone)]
enum KeyAction {
    /// Insert this (lowercase, for letters) string. Letters are upper-cased
    /// when shift is engaged.
    Char(String),
    /// Insert a space.
    Space,
    /// Delete backwards.
    Backspace,
    /// Send the return/done action.
    Return,
    /// Toggle shift (letters page only).
    Shift,
    /// Switch to the numbers page.
    ToNumbers,
    /// Switch to the symbols page.
    ToSymbols,
    /// Switch (back) to the letters page.
    ToLetters,
}

/// Which page of keys is currently shown.
#[derive(Clone, Copy, PartialEq, Default)]
enum Page {
    #[default]
    Letters,
    Numbers,
    Symbols,
}

/// Host object backing the on-screen keyboard controller. It owns a plain
/// `UIView` container and a set of `UIButton` keys; it is the target of every
/// key button's action.
struct KeyboardCtrlHostObject {
    /// `UIView*` holding all the key buttons.
    container: id,
    /// The key buttons currently laid out. The button `tag` is its index here.
    buttons: Vec<id>,
    /// The action for each button (parallel to `buttons`).
    keys: Vec<KeyAction>,
    page: Page,
    shifted: bool,
}
impl HostObject for KeyboardCtrlHostObject {}

/// Canonical portrait keyboard height in points (iPhone, iOS 5).
const KB_HEIGHT: f32 = 216.0;
const SIDE_MARGIN: f32 = 3.0;
const KEY_GAP: f32 = 6.0;
const TOP_MARGIN: f32 = 8.0;
const ROW_GAP: f32 = 8.0;

/// Build the rows of keys for the given page/shift state. Each key carries a
/// relative width weight used to distribute the row's horizontal space.
fn layout_rows(page: Page, _shifted: bool) -> Vec<Vec<(KeyAction, f32)>> {
    fn chars(s: &[&str]) -> Vec<(KeyAction, f32)> {
        s.iter()
            .map(|c| (KeyAction::Char((*c).to_string()), 1.0))
            .collect()
    }
    let bottom = |first: KeyAction| {
        vec![
            (first, 2.2),
            (KeyAction::Space, 5.6),
            (KeyAction::Return, 3.0),
        ]
    };
    match page {
        Page::Letters => vec![
            chars(&["q", "w", "e", "r", "t", "y", "u", "i", "o", "p"]),
            chars(&["a", "s", "d", "f", "g", "h", "j", "k", "l"]),
            {
                let mut row = vec![(KeyAction::Shift, 1.6)];
                row.extend(chars(&["z", "x", "c", "v", "b", "n", "m"]));
                row.push((KeyAction::Backspace, 1.6));
                row
            },
            bottom(KeyAction::ToNumbers),
        ],
        Page::Numbers => vec![
            chars(&["1", "2", "3", "4", "5", "6", "7", "8", "9", "0"]),
            chars(&["-", "/", ":", ";", "(", ")", "$", "&", "@", "\""]),
            {
                let mut row = vec![(KeyAction::ToSymbols, 1.6)];
                row.extend(chars(&[".", ",", "?", "!", "'"]));
                row.push((KeyAction::Backspace, 1.6));
                row
            },
            bottom(KeyAction::ToLetters),
        ],
        Page::Symbols => vec![
            chars(&["[", "]", "{", "}", "#", "%", "^", "*", "+", "="]),
            chars(&["_", "\\", "|", "~", "<", ">", "€", "£", "¥", "•"]),
            {
                let mut row = vec![(KeyAction::ToNumbers, 1.6)];
                row.extend(chars(&[".", ",", "?", "!", "'"]));
                row.push((KeyAction::Backspace, 1.6));
                row
            },
            bottom(KeyAction::ToLetters),
        ],
    }
}

/// The label shown on a key, given the current shift state.
fn key_label(action: &KeyAction, shifted: bool) -> String {
    match action {
        KeyAction::Char(c) => {
            if shifted {
                c.to_uppercase()
            } else {
                c.clone()
            }
        }
        KeyAction::Space => "space".to_string(),
        KeyAction::Backspace => "del".to_string(),
        KeyAction::Return => "return".to_string(),
        KeyAction::Shift => "shift".to_string(),
        KeyAction::ToNumbers => ".?123".to_string(),
        KeyAction::ToSymbols => "#+=".to_string(),
        KeyAction::ToLetters => "ABC".to_string(),
    }
}

/// Whether a key is a "function" key (drawn darker, with light text).
fn is_function_key(action: &KeyAction) -> bool {
    matches!(
        action,
        KeyAction::Backspace
            | KeyAction::Return
            | KeyAction::Shift
            | KeyAction::ToNumbers
            | KeyAction::ToSymbols
            | KeyAction::ToLetters
    )
}

/// Tear down and rebuild all the key buttons for the current page/shift state.
fn rebuild_keys(env: &mut Environment, this: id) {
    let (container, old_buttons) = {
        let host = env.objc.borrow_mut::<KeyboardCtrlHostObject>(this);
        let old = std::mem::take(&mut host.buttons);
        host.keys.clear();
        (host.container, old)
    };
    // Removing from the superview releases the button (the container held the
    // only strong reference), so we must not release again here.
    for button in old_buttons {
        () = msg![env; button removeFromSuperview];
    }

    let bounds: CGRect = msg![env; container bounds];
    let width = bounds.size.width;
    if width <= 0.0 {
        return;
    }
    let (page, shifted) = {
        let host = env.objc.borrow::<KeyboardCtrlHostObject>(this);
        (host.page, host.shifted)
    };
    let rows = layout_rows(page, shifted);

    let key_font: id = msg_class![env; UIFont systemFontOfSize:18.0f32];
    let fn_font: id = msg_class![env; UIFont systemFontOfSize:15.0f32];
    let black: id = msg_class![env; UIColor blackColor];
    let white: id = msg_class![env; UIColor whiteColor];
    // Letter keys: near-white light gray. Function keys: mid gray. Shift when
    // engaged: bright white to signal the active state.
    let char_bg: id = msg_class![env; UIColor colorWithRed:0.99f32 green:0.99f32 blue:1.0f32 alpha:1.0f32];
    let fn_bg: id = msg_class![env; UIColor colorWithRed:0.60f32 green:0.63f32 blue:0.68f32 alpha:1.0f32];

    let selector = env.objc.lookup_selector("_touchHLE_keyPressed:").unwrap();

    let row_count = rows.len();
    let key_h = (KB_HEIGHT - 2.0 * TOP_MARGIN - (row_count as f32 - 1.0) * ROW_GAP)
        / row_count as f32;

    let mut new_buttons: Vec<id> = Vec::new();
    let mut new_keys: Vec<KeyAction> = Vec::new();

    for (row_index, row) in rows.into_iter().enumerate() {
        let total_weight: f32 = row.iter().map(|(_, w)| *w).sum();
        let n = row.len();
        let avail = width - 2.0 * SIDE_MARGIN - (n as f32 - 1.0) * KEY_GAP;
        let unit = avail / total_weight;
        let y = TOP_MARGIN + row_index as f32 * (key_h + ROW_GAP);

        let mut x = SIDE_MARGIN;
        for (action, weight) in row {
            let w = weight * unit;
            let frame = CGRect {
                origin: CGPoint { x, y },
                size: CGSize {
                    width: w,
                    height: key_h,
                },
            };
            let is_fn = is_function_key(&action);
            let button: id = msg_class![env; UIButton buttonWithType:UIButtonTypeCustom];
            () = msg![env; button setFrame:frame];

            let label = key_label(&action, shifted);
            let title = ns_string::from_rust_string(env, label);
            () = msg![env; button setTitle:title forState:UIControlStateNormal];
            release(env, title);

            let shift_active = shifted && matches!(action, KeyAction::Shift);
            let (bg, fg, font) = if is_fn {
                let bg = if shift_active { white } else { fn_bg };
                (bg, white, fn_font)
            } else {
                (char_bg, black, key_font)
            };
            () = msg![env; button setBackgroundColor:bg];
            () = msg![env; button setTitleColor:fg forState:UIControlStateNormal];
            let title_label: id = msg![env; button titleLabel];
            () = msg![env; title_label setFont:font];

            // Rounded corners for the classic key look.
            let layer: id = msg![env; button layer];
            () = msg![env; layer setCornerRadius:5.0f32];

            let tag = new_buttons.len() as NSInteger;
            () = msg![env; button setTag:tag];
            () = msg![env; button addTarget:this
                                     action:selector
                           forControlEvents:UIControlEventTouchUpInside];
            () = msg![env; container addSubview:button];

            new_buttons.push(button);
            new_keys.push(action);
            x += w + KEY_GAP;
        }
    }

    let host = env.objc.borrow_mut::<KeyboardCtrlHostObject>(this);
    host.buttons = new_buttons;
    host.keys = new_keys;
}

/// Show the on-screen keyboard, creating it on first use and attaching it to
/// the bottom of the key window.
pub fn show_keyboard(env: &mut Environment, _first_responder: id) {
    // Get or create the controller.
    let controller = match env.framework_state.uikit.ui_keyboard.onscreen {
        Some(c) => c,
        None => {
            let c: id = msg_class![env; _touchHLE_Keyboard new];
            env.framework_state.uikit.ui_keyboard.onscreen = Some(c);
            c
        }
    };

    let app: id = msg_class![env; UIApplication sharedApplication];
    let window: id = msg![env; app keyWindow];
    if window == nil {
        return;
    }

    let screen: id = msg_class![env; UIScreen mainScreen];
    let screen_bounds: CGRect = msg![env; screen bounds];
    let frame = CGRect {
        origin: CGPoint {
            x: 0.0,
            y: screen_bounds.size.height - KB_HEIGHT,
        },
        size: CGSize {
            width: screen_bounds.size.width,
            height: KB_HEIGHT,
        },
    };

    let container = env
        .objc
        .borrow::<KeyboardCtrlHostObject>(controller)
        .container;
    () = msg![env; container setFrame:frame];
    () = msg![env; window addSubview:container];
    () = msg![env; window bringSubviewToFront:container];

    rebuild_keys(env, controller);
    env.framework_state.uikit.ui_keyboard.onscreen_visible = true;
}

/// Hide the on-screen keyboard (detach it from its window).
pub fn hide_keyboard(env: &mut Environment) {
    let Some(controller) = env.framework_state.uikit.ui_keyboard.onscreen else {
        return;
    };
    if !env.framework_state.uikit.ui_keyboard.onscreen_visible {
        return;
    }
    let container = env
        .objc
        .borrow::<KeyboardCtrlHostObject>(controller)
        .container;
    () = msg![env; container removeFromSuperview];
    env.framework_state.uikit.ui_keyboard.onscreen_visible = false;
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

// MARK: - On-screen keyboard controller

@implementation _touchHLE_Keyboard: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(KeyboardCtrlHostObject {
        container: nil,
        buttons: Vec::new(),
        keys: Vec::new(),
        page: Page::Letters,
        shifted: false,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)init {
    // Container view that holds the keys and paints the keyboard backdrop.
    let container: id = msg_class![env; UIView new];
    let bg: id = msg_class![env; UIColor colorWithRed:0.63f32 green:0.65f32 blue:0.69f32 alpha:1.0f32];
    () = msg![env; container setBackgroundColor:bg];
    env.objc.borrow_mut::<KeyboardCtrlHostObject>(this).container = container;
    this
}

- (())_touchHLE_keyPressed:(id)sender { // UIButton*
    let tag: NSInteger = msg![env; sender tag];
    let (action, page, shifted) = {
        let host = env.objc.borrow::<KeyboardCtrlHostObject>(this);
        (
            host.keys.get(tag as usize).cloned(),
            host.page,
            host.shifted,
        )
    };
    let Some(action) = action else { return; };

    // Text-affecting keys only apply to a UITextField first responder, matching
    // the physical-keyboard routing in `uikit::handle_events`.
    let responder = env.framework_state.uikit.ui_responder.first_responder;
    let is_text_field = if responder != nil {
        let class = msg![env; responder class];
        let tf_class = env.objc.get_known_class("UITextField", &mut env.mem);
        env.objc.class_is_subclass_of(class, tf_class)
    } else {
        false
    };

    match action {
        KeyAction::Char(c) => {
            if is_text_field {
                let text = if shifted { c.to_uppercase() } else { c };
                handle_text(env, responder, text);
            }
            // Auto-release a one-shot shift after a letter.
            if shifted && page == Page::Letters {
                env.objc.borrow_mut::<KeyboardCtrlHostObject>(this).shifted = false;
                rebuild_keys(env, this);
            }
        }
        KeyAction::Space => {
            if is_text_field {
                handle_text(env, responder, " ".to_string());
            }
        }
        KeyAction::Backspace => {
            if is_text_field {
                handle_backspace(env, responder);
            }
        }
        KeyAction::Return => {
            if is_text_field {
                handle_return(env, responder);
            }
        }
        KeyAction::Shift => {
            let host = env.objc.borrow_mut::<KeyboardCtrlHostObject>(this);
            host.shifted = !host.shifted;
            rebuild_keys(env, this);
        }
        KeyAction::ToNumbers => {
            let host = env.objc.borrow_mut::<KeyboardCtrlHostObject>(this);
            host.page = Page::Numbers;
            host.shifted = false;
            rebuild_keys(env, this);
        }
        KeyAction::ToSymbols => {
            env.objc.borrow_mut::<KeyboardCtrlHostObject>(this).page = Page::Symbols;
            rebuild_keys(env, this);
        }
        KeyAction::ToLetters => {
            env.objc.borrow_mut::<KeyboardCtrlHostObject>(this).page = Page::Letters;
            rebuild_keys(env, this);
        }
    }
}

- (())dealloc {
    let container = env.objc.borrow::<KeyboardCtrlHostObject>(this).container;
    if container != nil {
        () = msg![env; container removeFromSuperview];
        release(env, container);
    }
    env.objc.dealloc_object(this, &mut env.mem)
}

@end

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
    false
}

+ (CGSize)defaultSizeForOrientation:(NSInteger)_orientation {
    // Standard iPhone keyboard size in portrait.
    CGSize { width: 320.0, height: 216.0 }
}

+ (CGSize)defaultSizeForInterfaceOrientation:(NSInteger)_orientation {
    CGSize { width: 320.0, height: 216.0 }
}

- (())orderInWithAnimation:(bool)_animated {
    log!("UIKeyboard orderInWithAnimation: stubbed (no keyboard shown)");
}

- (())orderOutWithAnimation:(bool)_animated {
    log!("UIKeyboard orderOutWithAnimation: stubbed");
}

- (())activate {
    log!("UIKeyboard activate: stubbed");
}

- (())deactivate {
    log!("UIKeyboard deactivate: stubbed");
}

- (bool)isVisible {
    false
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
    nil
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
/// In touchHLE we never actually show a keyboard, but apps that observe these
/// notifications (to scroll their content) need them to fire.
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

    for name in [will_name, did_name] {
        let ns_name = get_static_str(env, name);
        let _: () = msg![env; center postNotificationName:ns_name
                                                   object:app
                                                 userInfo:user_info];
    }

    crate::objc::release(env, user_info);
}
