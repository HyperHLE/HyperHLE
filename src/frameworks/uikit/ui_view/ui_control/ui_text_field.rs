/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UITextField`.

use crate::dyld::{ConstantExports, HostConstant};
use crate::frameworks::core_graphics::{CGPoint, CGRect, CGSize};
use crate::frameworks::foundation::ns_string::get_static_str;
use crate::frameworks::foundation::{ns_string, NSInteger, NSRange, NSUInteger};
use crate::frameworks::uikit::ui_font::{UITextAlignment, UITextAlignmentLeft};
use crate::frameworks::uikit::ui_view::ui_window::{
    UIKeyboardDidHideNotification, UIKeyboardDidShowNotification, UIKeyboardWillHideNotification,
    UIKeyboardWillShowNotification,
};
use crate::impl_HostObject_with_superclass;
use crate::objc::{
    id, msg, msg_class, msg_super, nil, objc_classes, release, retain, todo_objc_setter,
    ClassExports, NSZonePtr, SEL,
};
use crate::Environment;

type UIKeyboardAppearance = NSInteger;
type UIKeyboardType = NSInteger;
pub type UIReturnKeyType = NSInteger;
type UITextAutocapitalizationType = NSInteger;
type UITextAutocorrectionType = NSInteger;
type UITextBorderStyle = NSInteger;
type UITextFieldViewMode = NSInteger;

const UITextFieldTextDidChangeNotification: &str = "UITextFieldTextDidChangeNotification";
const UITextFieldTextDidBeginEditingNotification: &str =
    "UITextFieldTextDidBeginEditingNotification";
const UITextFieldTextDidEndEditingNotification: &str = "UITextFieldTextDidEndEditingNotification";

pub const CONSTANTS: ConstantExports = &[
    (
        "_UITextFieldTextDidChangeNotification",
        HostConstant::NSString(UITextFieldTextDidChangeNotification),
    ),
    (
        "_UITextFieldTextDidBeginEditingNotification",
        HostConstant::NSString(UITextFieldTextDidBeginEditingNotification),
    ),
    (
        "_UITextFieldTextDidEndEditingNotification",
        HostConstant::NSString(UITextFieldTextDidEndEditingNotification),
    ),
];

struct UITextFieldHostObject {
    superclass: super::UIControlHostObject,
    delegate: id,
    editing: bool,
    text_label: id,
    placeholder: id,
    attributed_placeholder: id,
    font: id,
    text_color: id,
    text_alignment: UITextAlignment,
    border_style: UITextBorderStyle,
    background: id,
    disabled_background: id,
    clear_button_mode: UITextFieldViewMode,
    left_view: id,
    left_view_mode: UITextFieldViewMode,
    right_view: id,
    right_view_mode: UITextFieldViewMode,
    input_view: id,
    input_accessory_view: id,
    clears_on_begin_editing: bool,
    adjusts_font_size_to_fit_width: bool,
    minimum_font_size: f32,
    secure_text_entry: bool,
    autocapitalization_type: UITextAutocapitalizationType,
    autocorrection_type: UITextAutocorrectionType,
    return_key_type: UIReturnKeyType,
    keyboard_type: UIKeyboardType,
    keyboard_appearance: UIKeyboardAppearance,
    enables_return_key_automatically: bool,
    spell_checking_type: NSInteger,
    content_vertical_alignment: NSInteger,
}
impl_HostObject_with_superclass!(UITextFieldHostObject);
impl Default for UITextFieldHostObject {
    fn default() -> Self {
        UITextFieldHostObject {
            superclass: Default::default(),
            delegate: nil,
            editing: false,
            text_label: nil,
            placeholder: nil,
            attributed_placeholder: nil,
            font: nil,
            text_color: nil,
            text_alignment: UITextAlignmentLeft,
            border_style: 0,
            background: nil,
            disabled_background: nil,
            clear_button_mode: 0,
            left_view: nil,
            left_view_mode: 0,
            right_view: nil,
            right_view_mode: 0,
            input_view: nil,
            input_accessory_view: nil,
            clears_on_begin_editing: false,
            adjusts_font_size_to_fit_width: false,
            minimum_font_size: 0.0,
            secure_text_entry: false,
            autocapitalization_type: 0,
            autocorrection_type: 0,
            return_key_type: 0,
            keyboard_type: 0,
            keyboard_appearance: 0,
            enables_return_key_automatically: false,
            spell_checking_type: 0,
            content_vertical_alignment: 0,
        }
    }
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UITextField: UIControl

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::<UITextFieldHostObject>::default();
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)initWithFrame:(CGRect)frame {
    let this: id = msg_super![env; this initWithFrame:frame];

    let _: () = msg![env; this setOpaque:true];
    let bg_color: id = msg_class![env; UIColor whiteColor];
    let _: () = msg![env; this setBackgroundColor:bg_color];

    let text_label: id = msg_class![env; UILabel new];

    // ВЫНЕСЕННАЯ ПЕРЕМЕННАЯ ДЛЯ ИЗБЕЖАНИЯ ОШИБОК КОМПИЛЯЦИИ E0283
    let clear_color: id = msg_class![env; UIColor clearColor];
    let _: () = msg![env; text_label setBackgroundColor:clear_color];

    let _: () = msg![env; text_label setTextAlignment:UITextAlignmentLeft];
    let text_color: id = msg_class![env; UIColor blackColor];
    let _: () = msg![env; text_label setTextColor:text_color];

    env.objc.borrow_mut::<UITextFieldHostObject>(this).text_label = text_label;
    let _: () = msg![env; this addSubview:text_label];

    this
}

- (id)initWithCoder:(id)coder {
    let this: id = msg_super![env; this initWithCoder:coder];

    let text_label: id = msg_class![env; UILabel new];

    // ВЫНЕСЕННАЯ ПЕРЕМЕННАЯ ДЛЯ ИЗБЕЖАНИЯ ОШИБОК КОМПИЛЯЦИИ E0283
    let clear_color: id = msg_class![env; UIColor clearColor];
    let _: () = msg![env; text_label setBackgroundColor:clear_color];

    env.objc.borrow_mut::<UITextFieldHostObject>(this).text_label = text_label;
    let _: () = msg![env; this addSubview:text_label];

    let key_text = get_static_str(env, "UIText");
    if msg![env; coder containsValueForKey:key_text] {
        let text: id = msg![env; coder decodeObjectForKey:key_text];
        () = msg![env; this setText:text];
    }

    let key_font = get_static_str(env, "UIFont");
    if msg![env; coder containsValueForKey:key_font] {
        let font: id = msg![env; coder decodeObjectForKey:key_font];
        () = msg![env; this setFont:font];
    } else {
        () = msg![env; this setFont:nil];
    }

    let key_color = get_static_str(env, "UITextColor");
    if msg![env; coder containsValueForKey:key_color] {
        let text_color: id = msg![env; coder decodeObjectForKey:key_color];
        () = msg![env; this setTextColor:text_color];
    } else {
        () = msg![env; this setTextColor:nil];
    }

    let key_align = get_static_str(env, "UITextAlignment");
    if msg![env; coder containsValueForKey:key_align] {
        let align: UITextAlignment = msg![env; coder decodeIntegerForKey:key_align];
        () = msg![env; this setTextAlignment:align];
    }

    this
}

- (())dealloc {
    let UITextFieldHostObject {
        text_label, placeholder, attributed_placeholder, font, text_color,
        background, disabled_background, left_view, right_view, input_view, input_accessory_view, ..
    } = std::mem::take(env.objc.borrow_mut(this));

    release(env, text_label);
    release(env, placeholder);
    release(env, attributed_placeholder);
    release(env, font);
    release(env, text_color);
    release(env, background);
    release(env, disabled_background);
    release(env, left_view);
    release(env, right_view);
    release(env, input_view);
    release(env, input_accessory_view);
    msg_super![env; this dealloc]
}

- (())layoutSubviews {
    let text_label = env.objc.borrow::<UITextFieldHostObject>(this).text_label;
    let bounds: CGRect = msg![env; this bounds];
    let _: () = msg![env; text_label setFrame:bounds];
}

- (id)text {
    let text_label = env.objc.borrow::<UITextFieldHostObject>(this).text_label;
    msg![env; text_label text]
}

- (())setText:(id)text {
    let text_label = env.objc.borrow::<UITextFieldHostObject>(this).text_label;
    let _: () = msg![env; text_label setText:text];
    let center: id = msg_class![env; NSNotificationCenter defaultCenter];
    let name = ns_string::get_static_str(env, UITextFieldTextDidChangeNotification);
    let _: () = msg![env; center postNotificationName:name object:this userInfo:nil];
}

- (id)attributedText {
    let text: id = msg![env; this text];
    if text == nil { return nil; }
    msg_class![env; NSAttributedString alloc]
}

- (())setAttributedText:(id)attributed_text {
    if attributed_text == nil {
        let _: () = msg![env; this setText:nil];
        return;
    }
    let plain: id = msg![env; attributed_text string];
    let _: () = msg![env; this setText:plain];
}

- (id)placeholder { env.objc.borrow::<UITextFieldHostObject>(this).placeholder }

- (())setPlaceholder:(id)placeholder {
    let old = env.objc.borrow::<UITextFieldHostObject>(this).placeholder;
    release(env, old);
    retain(env, placeholder);
    env.objc.borrow_mut::<UITextFieldHostObject>(this).placeholder = placeholder;
}

- (id)attributedPlaceholder { env.objc.borrow::<UITextFieldHostObject>(this).attributed_placeholder }

- (())setAttributedPlaceholder:(id)attr_placeholder {
    let old = env.objc.borrow::<UITextFieldHostObject>(this).attributed_placeholder;
    release(env, old);
    retain(env, attr_placeholder);
    env.objc.borrow_mut::<UITextFieldHostObject>(this).attributed_placeholder = attr_placeholder;
    if attr_placeholder != nil {
        let plain: id = msg![env; attr_placeholder string];
        let _: () = msg![env; this setPlaceholder:plain];
    }
}

- (id)font { env.objc.borrow::<UITextFieldHostObject>(this).font }

- (())setFont:(id)new_font {
    let old = env.objc.borrow::<UITextFieldHostObject>(this).font;
    release(env, old);
    retain(env, new_font);
    env.objc.borrow_mut::<UITextFieldHostObject>(this).font = new_font;
    let text_label = env.objc.borrow::<UITextFieldHostObject>(this).text_label;
    let _: () = msg![env; text_label setFont:new_font];
}

- (id)textColor { env.objc.borrow::<UITextFieldHostObject>(this).text_color }

- (())setTextColor:(id)color {
    let old = env.objc.borrow::<UITextFieldHostObject>(this).text_color;
    release(env, old);
    retain(env, color);
    env.objc.borrow_mut::<UITextFieldHostObject>(this).text_color = color;
    let text_label = env.objc.borrow::<UITextFieldHostObject>(this).text_label;
    let _: () = msg![env; text_label setTextColor:color];
}

- (UITextAlignment)textAlignment { env.objc.borrow::<UITextFieldHostObject>(this).text_alignment }

- (())setTextAlignment:(UITextAlignment)text_alignment {
    env.objc.borrow_mut::<UITextFieldHostObject>(this).text_alignment = text_alignment;
    let text_label = env.objc.borrow::<UITextFieldHostObject>(this).text_label;
    let _: () = msg![env; text_label setTextAlignment:text_alignment];
}

- (UITextBorderStyle)borderStyle { env.objc.borrow::<UITextFieldHostObject>(this).border_style }
- (())setBorderStyle:(UITextBorderStyle)style { env.objc.borrow_mut::<UITextFieldHostObject>(this).border_style = style; }

- (id)background { env.objc.borrow::<UITextFieldHostObject>(this).background }
- (())setBackground:(id)background {
    let old = env.objc.borrow::<UITextFieldHostObject>(this).background;
    release(env, old);
    retain(env, background);
    env.objc.borrow_mut::<UITextFieldHostObject>(this).background = background;
}

- (id)disabledBackground { env.objc.borrow::<UITextFieldHostObject>(this).disabled_background }
- (())setDisabledBackground:(id)background {
    let old = env.objc.borrow::<UITextFieldHostObject>(this).disabled_background;
    release(env, old);
    retain(env, background);
    env.objc.borrow_mut::<UITextFieldHostObject>(this).disabled_background = background;
}

- (UITextFieldViewMode)clearButtonMode { env.objc.borrow::<UITextFieldHostObject>(this).clear_button_mode }
- (())setClearButtonMode:(UITextFieldViewMode)mode { env.objc.borrow_mut::<UITextFieldHostObject>(this).clear_button_mode = mode; }
- (())setClearsOnBeginEditing:(bool)clear { env.objc.borrow_mut::<UITextFieldHostObject>(this).clears_on_begin_editing = clear; }
- (bool)clearsOnBeginEditing { env.objc.borrow::<UITextFieldHostObject>(this).clears_on_begin_editing }

- (id)leftView { env.objc.borrow::<UITextFieldHostObject>(this).left_view }
- (())setLeftView:(id)view {
    let old = env.objc.borrow::<UITextFieldHostObject>(this).left_view;
    release(env, old);
    retain(env, view);
    env.objc.borrow_mut::<UITextFieldHostObject>(this).left_view = view;
}

- (UITextFieldViewMode)leftViewMode { env.objc.borrow::<UITextFieldHostObject>(this).left_view_mode }
- (())setLeftViewMode:(UITextFieldViewMode)mode { env.objc.borrow_mut::<UITextFieldHostObject>(this).left_view_mode = mode; }

- (id)rightView { env.objc.borrow::<UITextFieldHostObject>(this).right_view }
- (())setRightView:(id)view {
    let old = env.objc.borrow::<UITextFieldHostObject>(this).right_view;
    release(env, old);
    retain(env, view);
    env.objc.borrow_mut::<UITextFieldHostObject>(this).right_view = view;
}
- (UITextFieldViewMode)rightViewMode { env.objc.borrow::<UITextFieldHostObject>(this).right_view_mode }
- (())setRightViewMode:(UITextFieldViewMode)mode { env.objc.borrow_mut::<UITextFieldHostObject>(this).right_view_mode = mode; }

- (id)inputView { env.objc.borrow::<UITextFieldHostObject>(this).input_view }
- (())setInputView:(id)view {
    let old = env.objc.borrow::<UITextFieldHostObject>(this).input_view;
    release(env, old);
    retain(env, view);
    env.objc.borrow_mut::<UITextFieldHostObject>(this).input_view = view;
}

- (id)inputAccessoryView { env.objc.borrow::<UITextFieldHostObject>(this).input_accessory_view }
- (())setInputAccessoryView:(id)view {
    let old = env.objc.borrow::<UITextFieldHostObject>(this).input_accessory_view;
    release(env, old);
    retain(env, view);
    env.objc.borrow_mut::<UITextFieldHostObject>(this).input_accessory_view = view;
}

- (bool)adjustsFontSizeToFitWidth { env.objc.borrow::<UITextFieldHostObject>(this).adjusts_font_size_to_fit_width }
- (())setAdjustsFontSizeToFitWidth:(bool)adjusts { env.objc.borrow_mut::<UITextFieldHostObject>(this).adjusts_font_size_to_fit_width = adjusts; }
- (f32)minimumFontSize { env.objc.borrow::<UITextFieldHostObject>(this).minimum_font_size }
- (())setMinimumFontSize:(f32)size { env.objc.borrow_mut::<UITextFieldHostObject>(this).minimum_font_size = size; }

- (bool)isSecureTextEntry { env.objc.borrow::<UITextFieldHostObject>(this).secure_text_entry }
- (())setSecureTextEntry:(bool)secure { env.objc.borrow_mut::<UITextFieldHostObject>(this).secure_text_entry = secure; }
- (UITextAutocapitalizationType)autocapitalizationType { env.objc.borrow::<UITextFieldHostObject>(this).autocapitalization_type }
- (())setAutocapitalizationType:(UITextAutocapitalizationType)type_ { env.objc.borrow_mut::<UITextFieldHostObject>(this).autocapitalization_type = type_; }
- (UITextAutocorrectionType)autocorrectionType { env.objc.borrow::<UITextFieldHostObject>(this).autocorrection_type }
- (())setAutocorrectionType:(UITextAutocorrectionType)type_ { env.objc.borrow_mut::<UITextFieldHostObject>(this).autocorrection_type = type_; }
- (UIReturnKeyType)returnKeyType { env.objc.borrow::<UITextFieldHostObject>(this).return_key_type }
- (())setReturnKeyType:(UIReturnKeyType)type_ { env.objc.borrow_mut::<UITextFieldHostObject>(this).return_key_type = type_; }
- (UIKeyboardType)keyboardType { env.objc.borrow::<UITextFieldHostObject>(this).keyboard_type }
- (())setKeyboardType:(UIKeyboardType)type_ { env.objc.borrow_mut::<UITextFieldHostObject>(this).keyboard_type = type_; }
- (UIKeyboardAppearance)keyboardAppearance { env.objc.borrow::<UITextFieldHostObject>(this).keyboard_appearance }
- (())setKeyboardAppearance:(UIKeyboardAppearance)appearance { env.objc.borrow_mut::<UITextFieldHostObject>(this).keyboard_appearance = appearance; }
- (bool)enablesReturnKeyAutomatically { env.objc.borrow::<UITextFieldHostObject>(this).enables_return_key_automatically }
- (())setEnablesReturnKeyAutomatically:(bool)enables { env.objc.borrow_mut::<UITextFieldHostObject>(this).enables_return_key_automatically = enables; }
- (NSInteger)spellCheckingType { env.objc.borrow::<UITextFieldHostObject>(this).spell_checking_type }
- (())setSpellCheckingType:(NSInteger)type_ { env.objc.borrow_mut::<UITextFieldHostObject>(this).spell_checking_type = type_; }
- (NSInteger)contentVerticalAlignment { env.objc.borrow::<UITextFieldHostObject>(this).content_vertical_alignment }
- (())setContentVerticalAlignment:(NSInteger)alignment { env.objc.borrow_mut::<UITextFieldHostObject>(this).content_vertical_alignment = alignment; }

- (id)delegate { env.objc.borrow::<UITextFieldHostObject>(this).delegate }
- (())setDelegate:(id)delegate { env.objc.borrow_mut::<UITextFieldHostObject>(this).delegate = delegate; }

- (bool)isEditing { env.objc.borrow::<UITextFieldHostObject>(this).editing }
- (bool)hasText {
    let text: id = msg![env; this text];
    if text == nil { return false; }
    let len: NSUInteger = msg![env; text length];
    len > 0
}

- (())touchesBegan:(id)_touches withEvent:(id)_event { let _: bool = msg![env; this becomeFirstResponder]; }
- (bool)canBecomeFirstResponder { true }
- (bool)canResignFirstResponder { true }

- (bool)becomeFirstResponder {
    if env.objc.borrow::<UITextFieldHostObject>(this).editing { return true; }

    let delegate: id = env.objc.borrow::<UITextFieldHostObject>(this).delegate;
    let mut delegate_alive = false;
    if delegate != nil {
        let isa: u32 = env.mem.read(delegate.cast());
        delegate_alive = isa != 0;
    }

    if delegate_alive {
        let sel: SEL = env.objc.register_host_selector("textFieldShouldBeginEditing:".to_string(), &mut env.mem);
        if msg![env; delegate respondsToSelector:sel] && !msg![env; delegate textFieldShouldBeginEditing:this] { return false; }
    }

    if env.objc.borrow::<UITextFieldHostObject>(this).clears_on_begin_editing {
        let empty = ns_string::get_static_str(env, "");
        let _: () = msg![env; this setText:empty];
    }

    let text_label = env.objc.borrow::<UITextFieldHostObject>(this).text_label;
    let curr_text: id = msg![env; text_label text];
    if curr_text == nil {
        let empty = ns_string::get_static_str(env, "");
        let _: () = msg![env; text_label setText:empty];
    }

    let center: id = msg_class![env; NSNotificationCenter defaultCenter];
    let name = ns_string::get_static_str(env, UIKeyboardWillShowNotification);
    let _: () = msg![env; center postNotificationName:name object:this userInfo:nil];

    env.framework_state.uikit.ui_responder.first_responder = this;
    env.on_parent_stack_in_coroutine(|window, _| window.start_text_input());

    let name = ns_string::get_static_str(env, UIKeyboardDidShowNotification);
    let _: () = msg![env; center postNotificationName:name object:this userInfo:nil];

    env.objc.borrow_mut::<UITextFieldHostObject>(this).editing = true;

    // Show the drawn on-screen iOS 5 keyboard for this field.
    crate::frameworks::uikit::ui_keyboard::show_keyboard(env, this);

    let name = ns_string::get_static_str(env, UITextFieldTextDidBeginEditingNotification);
    let _: () = msg![env; center postNotificationName:name object:this userInfo:nil];

    if delegate_alive {
        let sel: SEL = env.objc.register_host_selector("textFieldDidBeginEditing:".to_string(), &mut env.mem);
        if msg![env; delegate respondsToSelector:sel] { let _: () = msg![env; delegate textFieldDidBeginEditing:this]; }
    }
    true
}

- (bool)resignFirstResponder {
    if !env.objc.borrow::<UITextFieldHostObject>(this).editing { return true; }

    let delegate: id = env.objc.borrow::<UITextFieldHostObject>(this).delegate;
    let mut delegate_alive = false;
    if delegate != nil {
        let isa: u32 = env.mem.read(delegate.cast());
        delegate_alive = isa != 0;
    }

    if delegate_alive {
        let sel: SEL = env.objc.register_host_selector("textFieldShouldEndEditing:".to_string(), &mut env.mem);
        if msg![env; delegate respondsToSelector:sel] && !msg![env; delegate textFieldShouldEndEditing:this] { return false; }
    }

    let center: id = msg_class![env; NSNotificationCenter defaultCenter];
    let name = ns_string::get_static_str(env, UIKeyboardWillHideNotification);
    let _: () = msg![env; center postNotificationName:name object:this userInfo:nil];

    env.framework_state.uikit.ui_responder.first_responder = nil;
    // Hide the drawn on-screen iOS 5 keyboard.
    crate::frameworks::uikit::ui_keyboard::hide_keyboard(env);
    env.on_parent_stack_in_coroutine(|window, _| window.stop_text_input());

    let name = ns_string::get_static_str(env, UIKeyboardDidHideNotification);
    let _: () = msg![env; center postNotificationName:name object:this userInfo:nil];

    env.objc.borrow_mut::<UITextFieldHostObject>(this).editing = false;

    let name = ns_string::get_static_str(env, UITextFieldTextDidEndEditingNotification);
    let _: () = msg![env; center postNotificationName:name object:this userInfo:nil];

    if delegate_alive {
        let sel: SEL = env.objc.register_host_selector("textFieldDidEndEditing:".to_string(), &mut env.mem);
        if msg![env; delegate respondsToSelector:sel] { let _: () = msg![env; delegate textFieldDidEndEditing:this]; }
    }
    true
}

- (CGRect)textRectForBounds:(CGRect)bounds { bounds }
- (CGRect)editingRectForBounds:(CGRect)bounds { bounds }
- (CGRect)placeholderRectForBounds:(CGRect)bounds { bounds }
- (CGRect)borderRectForBounds:(CGRect)bounds { bounds }

- (CGRect)clearButtonRectForBounds:(CGRect)bounds {
    let sz: f32 = 19.0;
    CGRect {
        origin: CGPoint {
            x: bounds.origin.x + bounds.size.width - sz - 4.0,
            y: bounds.origin.y + (bounds.size.height - sz) * 0.5,
        },
        size: CGSize { width: sz, height: sz },
    }
}

- (CGRect)leftViewRectForBounds:(CGRect)bounds {
    CGRect { origin: bounds.origin, size: CGSize { width: 0.0, height: bounds.size.height } }
}

- (CGRect)rightViewRectForBounds:(CGRect)bounds {
    CGRect {
        origin: CGPoint { x: bounds.origin.x + bounds.size.width, y: bounds.origin.y },
        size: CGSize { width: 0.0, height: bounds.size.height },
    }
}

- (())setPosition:(CGPoint)position { todo_objc_setter!(this, position); }

@end

};

pub fn handle_text(env: &mut Environment, text_field: id, text: String) {
    let txt = ns_string::from_rust_string(env, text);
    let text_label = env
        .objc
        .borrow::<UITextFieldHostObject>(text_field)
        .text_label;
    let mut curr_text: id = msg![env; text_label text];
    if curr_text == nil {
        curr_text = ns_string::get_static_str(env, "");
    }

    let len: NSUInteger = msg![env; curr_text length];
    let range = NSRange {
        location: len,
        length: 0,
    };
    let delegate: id = env
        .objc
        .borrow::<UITextFieldHostObject>(text_field)
        .delegate;
    let mut should = true;

    if delegate != nil {
        let isa: u32 = env.mem.read(delegate.cast());
        if isa != 0 {
            let sel: SEL = env.objc.register_host_selector(
                "textField:shouldChangeCharactersInRange:replacementString:".to_string(),
                &mut env.mem,
            );
            if msg![env; delegate respondsToSelector:sel] {
                should = msg![env; delegate textField:text_field shouldChangeCharactersInRange:range replacementString:txt];
            }
        }
    }

    if should {
        let new_text: id = msg![env; curr_text stringByAppendingString:txt];
        // Route through `setText:` on the text field (not directly on the
        // text label) so that `UITextFieldTextDidChangeNotification` is
        // posted. Some apps (e.g. the Minecraft PE 0.6.1 iOS port's
        // `ShowKeyboardView`) drive their input loop entirely from this
        // notification, so skipping it makes typing on the soft keyboard
        // appear to do nothing.
        let _: () = msg![env; text_field setText:new_text];
        let _: () = msg![env; text_field setNeedsDisplay];
        release(env, new_text);
    }
    release(env, txt);
}

pub fn handle_backspace(env: &mut Environment, text_field: id) {
    let text_label = env
        .objc
        .borrow::<UITextFieldHostObject>(text_field)
        .text_label;
    let curr_text: id = msg![env; text_label text];
    let len: NSUInteger = msg![env; curr_text length];
    if len == 0 {
        return;
    }

    let range = NSRange {
        location: len - 1,
        length: 1,
    };
    let empty = ns_string::get_static_str(env, "");
    let delegate: id = env
        .objc
        .borrow::<UITextFieldHostObject>(text_field)
        .delegate;
    let mut should = true;

    if delegate != nil {
        let isa: u32 = env.mem.read(delegate.cast());
        if isa != 0 {
            let sel: SEL = env.objc.register_host_selector(
                "textField:shouldChangeCharactersInRange:replacementString:".to_string(),
                &mut env.mem,
            );
            if msg![env; delegate respondsToSelector:sel] {
                should = msg![env; delegate textField:text_field shouldChangeCharactersInRange:range replacementString:empty];
            }
        }
    }

    if should {
        let new_text: id = msg![env; curr_text substringToIndex:(len - 1)];
        // See `handle_text` above: go through `setText:` so that the
        // `UITextFieldTextDidChangeNotification` observer is notified.
        let _: () = msg![env; text_field setText:new_text];
        let _: () = msg![env; text_field setNeedsDisplay];
    }
}

pub fn handle_return(env: &mut Environment, text_field: id) {
    let delegate: id = env
        .objc
        .borrow::<UITextFieldHostObject>(text_field)
        .delegate;
    if delegate != nil {
        let isa: u32 = env.mem.read(delegate.cast());
        if isa != 0 {
            let sel: SEL = env
                .objc
                .register_host_selector("textFieldShouldReturn:".to_string(), &mut env.mem);
            if msg![env; delegate respondsToSelector:sel] {
                let _: () = msg![env; delegate textFieldShouldReturn:text_field];
            }
        }
    }
}
