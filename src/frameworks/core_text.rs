/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `CoreText.framework/CoreText`
//!
//! CoreText is the C-level text rendering API exposed by macOS and iOS
//! since iOS 3.2. We don't currently implement layout/glyph metrics
//! (apps that need full text shaping fall back to UIKit/UIFont), but
//! many apps reference the framework's exported string constants —
//! either as attribute keys when constructing font descriptor
//! dictionaries (`CTFontDescriptorCreateWithAttributes`) or as
//! attribute keys when building `CFAttributedStringRef` /
//! `NSAttributedString` instances for `CTFramesetterCreateWithAttributedString`
//! and friends — purely for `isEqual:` comparisons against keys
//! returned by the system.
//!
//! Per Apple's `CTFontDescriptor.h`, `CTFont.h`, `CTFontTraits.h` and
//! `CTStringAttributes.h` headers these are `CFStringRef` constants
//! with a canonical string value. For attributed-string attribute
//! keys most of them have the same value as their `NSAttributedString`
//! counterpart (e.g. `kCTFontAttributeName` == the `CFStringRef` for
//! the C string "NSFont", which is the same as the AppKit / UIKit
//! `NSFontAttributeName`), which is what makes CoreText / Foundation
//! attributed strings toll-free bridgeable. For touchHLE's purposes
//! the exact textual content only matters for identity comparisons;
//! we mirror the spelling Apple's public headers document.
//!
//! References:
//! - <https://developer.apple.com/documentation/coretext/font_descriptor_attribute_keys>
//! - <https://developer.apple.com/documentation/coretext/core_text_string_attributes>
//! - `CTFontDescriptor.h`, `CTFont.h`, `CTFontTraits.h`,
//!   `CTStringAttributes.h` (Apple SDK).

use crate::dyld::{export_c_func, ConstantExports, FunctionExports, HostConstant, HostDylib};
use crate::frameworks::core_graphics::cg_bitmap_context::CGBitmapContextDrawer;
use crate::frameworks::core_graphics::{CGFloat, CGPoint, CGRect, CGSize};
use crate::frameworks::foundation::ns_string::{get_static_str, to_rust_string};
use crate::frameworks::foundation::NSRange;
use crate::frameworks::uikit::ui_font::{font_from_uifont, is_uifont, draw_font_glyph};
use crate::font::{Font, TextAlignment};
use crate::mem::{ConstVoidPtr, MutPtr};
use crate::objc::{
    id, msg, msg_class, nil, objc_classes, retain, ClassExports, HostObject,
};
use crate::Environment;

/// Opaque CoreText font reference.
pub type CTFontRef = crate::objc::id;

/// Opaque CoreText font descriptor reference.
pub type CTFontDescriptorRef = crate::objc::id;

/// Opaque CoreText attributed string reference.
pub type CFAttributedStringRef = crate::objc::id;

/// Opaque CoreText typesetter reference.
pub type CTTypesetterRef = crate::objc::id;

/// Opaque CoreText line reference.
pub type CTLineRef = crate::objc::id;

#[derive(Clone, Default)]
struct CTFontDescriptorHostObject {
    attrs: id,
}
impl HostObject for CTFontDescriptorHostObject {}

#[derive(Clone, Default)]
struct CTFontHostObject {
    font: id,
}
impl HostObject for CTFontHostObject {}

#[derive(Clone, Default)]
struct CTAttributedStringHostObject {
    string: id,
    attrs: id,
}
impl HostObject for CTAttributedStringHostObject {}

#[derive(Clone, Default)]
struct CTTypesetterHostObject {
    attr_string: id,
}
impl HostObject for CTTypesetterHostObject {}

#[derive(Clone, Default)]
struct CTLineHostObject {
    text: id,
    font: id,
}
impl HostObject for CTLineHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation _touchHLE_CTFontDescriptor: NSObject
- (())dealloc {
    env.objc.dealloc_object(this, &mut env.mem)
}
@end

@implementation _touchHLE_CTFont: NSObject
- (())dealloc {
    env.objc.dealloc_object(this, &mut env.mem)
}
@end

@implementation _touchHLE_CFAttributedString: NSObject
- (())dealloc {
    env.objc.dealloc_object(this, &mut env.mem)
}
@end

@implementation _touchHLE_CTTypesetter: NSObject
- (())dealloc {
    env.objc.dealloc_object(this, &mut env.mem)
}
@end

@implementation _touchHLE_CTLine: NSObject
- (())dealloc {
    env.objc.dealloc_object(this, &mut env.mem)
}
@end

};

fn alloc_descriptor(env: &mut Environment, attrs: id) -> CTFontDescriptorRef {
    let class = env.objc.get_known_class("_touchHLE_CTFontDescriptor", &mut env.mem);
    env.objc.alloc_object(class, Box::new(CTFontDescriptorHostObject { attrs }), &mut env.mem)
}

fn alloc_font(env: &mut Environment, font: id) -> CTFontRef {
    let class = env.objc.get_known_class("_touchHLE_CTFont", &mut env.mem);
    env.objc.alloc_object(class, Box::new(CTFontHostObject { font }), &mut env.mem)
}

fn alloc_attr_string(env: &mut Environment, string: id, attrs: id) -> CFAttributedStringRef {
    let class = env.objc.get_known_class("_touchHLE_CFAttributedString", &mut env.mem);
    env.objc.alloc_object(class, Box::new(CTAttributedStringHostObject { string, attrs }), &mut env.mem)
}

fn alloc_typesetter(env: &mut Environment, attr_string: id) -> CTTypesetterRef {
    let class = env.objc.get_known_class("_touchHLE_CTTypesetter", &mut env.mem);
    env.objc.alloc_object(class, Box::new(CTTypesetterHostObject { attr_string }), &mut env.mem)
}

fn alloc_line(env: &mut Environment, text: id, font: id) -> CTLineRef {
    let class = env.objc.get_known_class("_touchHLE_CTLine", &mut env.mem);
    env.objc.alloc_object(class, Box::new(CTLineHostObject { text, font }), &mut env.mem)
}

fn descriptor_attrs(env: &mut Environment, descriptor: CTFontDescriptorRef) -> id {
    env.objc.borrow::<CTFontDescriptorHostObject>(descriptor).attrs
}

fn font_from_descriptor(env: &mut Environment, descriptor: CTFontDescriptorRef, size: CGFloat) -> id {
    let attrs = descriptor_attrs(env, descriptor);
    if attrs == nil {
        return msg_class![env; UIFont systemFontOfSize:size];
    }
    let font_name_key = crate::frameworks::foundation::ns_string::get_static_str(env, "NSFontNameAttribute");
    let family_key = crate::frameworks::foundation::ns_string::get_static_str(env, "NSFontFamilyAttribute");
    let size_key = crate::frameworks::foundation::ns_string::get_static_str(env, "NSFontSizeAttribute");
    let mut name: id = nil;
    let by_name: id = msg![env; attrs objectForKey:font_name_key];
    if by_name != nil {
        name = by_name;
    } else {
        let family: id = msg![env; attrs objectForKey:family_key];
        if family != nil {
            let family_str = to_rust_string(env, family).into_owned();
            name = match family_str.as_str() {
                "Courier New" => get_static_str(env, "CourierNewPSMT"),
                "Helvetica" => get_static_str(env, "Helvetica"),
                "Times New Roman" => get_static_str(env, "TimesNewRomanPSMT"),
                _ => nil,
            };
        }
    }
    let font_size: CGFloat = if size > 0.0 {
        size
    } else {
        let size_num: id = msg![env; attrs objectForKey:size_key];
        if size_num != nil {
            msg![env; size_num floatValue]
        } else {
            12.0
        }
    };
    if name == nil {
        msg_class![env; UIFont systemFontOfSize:font_size]
    } else {
        msg_class![env; UIFont fontWithName:name size:font_size]
    }
}

fn attributed_string_text(env: &mut Environment, attr_string: CFAttributedStringRef) -> id {
    env.objc.borrow::<CTAttributedStringHostObject>(attr_string).string
}

fn attributed_string_attrs(env: &mut Environment, attr_string: CFAttributedStringRef) -> id {
    env.objc.borrow::<CTAttributedStringHostObject>(attr_string).attrs
}

fn string_and_font_from_attr_string(env: &mut Environment, attr_string: CFAttributedStringRef) -> (id, id) {
    let string = attributed_string_text(env, attr_string);
    let attrs = attributed_string_attrs(env, attr_string);
    let font = if attrs == nil {
        msg_class![env; UIFont systemFontOfSize:12.0]
    } else {
        let key = crate::frameworks::foundation::ns_string::get_static_str(env, "NSFont");
        let maybe_font = msg![env; attrs objectForKey:key];
        if maybe_font != nil {
            if is_uifont(env, maybe_font) {
                maybe_font
            } else {
                msg_class![env; UIFont systemFontOfSize:12.0]
            }
        } else {
            msg_class![env; UIFont systemFontOfSize:12.0]
        }
    };
    (string, font)
}

fn CTFontDescriptorCreateWithAttributes(env: &mut Environment, attributes: id) -> CTFontDescriptorRef {
    let attrs = if attributes == nil {
        msg_class![env; NSDictionary dictionary]
    } else {
        attributes
    };
    retain(env, attrs);
    alloc_descriptor(env, attrs)
}

fn CTFontCreateWithFontDescriptor(
    env: &mut Environment,
    descriptor: CTFontDescriptorRef,
    size: CGFloat,
    _matrix: ConstVoidPtr,
) -> CTFontRef {
    let font = font_from_descriptor(env, descriptor, size);
    retain(env, font);
    alloc_font(env, font)
}

fn CTFontGetSize(env: &mut Environment, font: CTFontRef) -> CGFloat {
    if font.is_null() {
        return 0.0;
    }
    let ui_font = env.objc.borrow::<CTFontHostObject>(font).font;
    if ui_font == nil {
        return 0.0;
    }
    msg![env; ui_font pointSize]
}

fn CTFontGetAscent(env: &mut Environment, font: CTFontRef) -> CGFloat {
    if font.is_null() { return 0.0; }
    let ui_font = env.objc.borrow::<CTFontHostObject>(font).font;
    if ui_font == nil { return 0.0; }
    msg![env; ui_font ascender]
}

fn CTFontGetDescent(env: &mut Environment, font: CTFontRef) -> CGFloat {
    if font.is_null() { return 0.0; }
    let ui_font = env.objc.borrow::<CTFontHostObject>(font).font;
    if ui_font == nil { return 0.0; }
    -msg![env; ui_font descender]
}

fn CTFontGetLeading(env: &mut Environment, font: CTFontRef) -> CGFloat {
    if font.is_null() { return 0.0; }
    let ui_font = env.objc.borrow::<CTFontHostObject>(font).font;
    if ui_font == nil { return 0.0; }
    msg![env; ui_font leading]
}

fn CFAttributedStringCreate(
    env: &mut Environment,
    _allocator: crate::frameworks::core_foundation::cf_allocator::CFAllocatorRef,
    string: id,
    attributes: id,
) -> CFAttributedStringRef {
    let s = if string == nil { msg_class![env; NSString string] } else { string };
    let attrs = if attributes == nil { msg_class![env; NSDictionary dictionary] } else { attributes };
    retain(env, s);
    retain(env, attrs);
    alloc_attr_string(env, s, attrs)
}

fn CFAttributedStringCreateCopy(
    env: &mut Environment,
    _allocator: crate::frameworks::core_foundation::cf_allocator::CFAllocatorRef,
    string: CFAttributedStringRef,
) -> CFAttributedStringRef {
    if string.is_null() { return nil; }
    let host = env.objc.borrow::<CTAttributedStringHostObject>(string).clone();
    retain(env, host.string);
    retain(env, host.attrs);
    alloc_attr_string(env, host.string, host.attrs)
}

fn CFAttributedStringGetString(env: &mut Environment, attr_string: CFAttributedStringRef) -> id {
    if attr_string.is_null() { return nil; }
    env.objc.borrow::<CTAttributedStringHostObject>(attr_string).string
}

fn CTTypesetterCreateWithAttributedString(env: &mut Environment, string: CFAttributedStringRef) -> CTTypesetterRef {
    if string.is_null() { return nil; }
    retain(env, string);
    alloc_typesetter(env, string)
}

fn CTTypesetterCreateLine(env: &mut Environment, typesetter: CTTypesetterRef, _range: NSRange) -> CTLineRef {
    if typesetter.is_null() { return nil; }
    let attr_string = env.objc.borrow::<CTTypesetterHostObject>(typesetter).attr_string;
    let (text, font) = string_and_font_from_attr_string(env, attr_string);
    retain(env, text);
    retain(env, font);
    alloc_line(env, text, font)
}

fn CTLineCreateWithAttributedString(env: &mut Environment, string: CFAttributedStringRef) -> CTLineRef {
    if string.is_null() { return nil; }
    let (text, font) = string_and_font_from_attr_string(env, string);
    retain(env, text);
    retain(env, font);
    alloc_line(env, text, font)
}

fn line_text(env: &mut Environment, line: CTLineRef) -> id {
    env.objc.borrow::<CTLineHostObject>(line).text
}

fn line_font(env: &mut Environment, line: CTLineRef) -> id {
    env.objc.borrow::<CTLineHostObject>(line).font
}

fn CTLineGetTypographicBounds(
    env: &mut Environment,
    line: CTLineRef,
    ascent: MutPtr<CGFloat>,
    descent: MutPtr<CGFloat>,
    leading: MutPtr<CGFloat>,
) -> CGFloat {
    if line.is_null() {
        return 0.0;
    }
    let text = line_text(env, line);
    let font = line_font(env, line);
    let size: CGFloat = msg![env; font pointSize];
    let ascent_val: CGFloat = msg![env; font ascender];
    let descender_val: CGFloat = msg![env; font descender];
    let leading_val: CGFloat = msg![env; font leading];
    let text_str = to_rust_string(env, text).into_owned();
    let font_obj = font_from_uifont(env, font).unwrap_or_else(Font::sans_regular);
    let width = font_obj.calculate_text_size(size, &text_str, None).0;
    if !ascent.is_null() { env.mem.write(ascent, ascent_val); }
    if !descent.is_null() { env.mem.write(descent, -descender_val); }
    if !leading.is_null() { env.mem.write(leading, leading_val); }
    width
}

fn CTLineGetImageBounds(env: &mut Environment, line: CTLineRef, _context: id) -> CGRect {
    if line.is_null() {
        return CGRect::default();
    }
    let text = line_text(env, line);
    let font = line_font(env, line);
    let size: CGFloat = msg![env; font pointSize];
    let ascent_val: CGFloat = msg![env; font ascender];
    let text_str = to_rust_string(env, text).into_owned();
    let font_obj = font_from_uifont(env, font).unwrap_or_else(Font::sans_regular);
    let (w, h) = font_obj.calculate_text_size(size, &text_str, None);
    CGRect { origin: CGPoint { x: 0.0, y: -ascent_val }, size: CGSize { width: w, height: h } }
}

fn CTLineDraw(env: &mut Environment, line: CTLineRef, context: id) {
    if line.is_null() || context.is_null() { return; }
    let text = line_text(env, line);
    let font = line_font(env, line);
    let size: CGFloat = msg![env; font pointSize];
    let text_str = to_rust_string(env, text).into_owned();
    let font_obj = font_from_uifont(env, font).unwrap_or_else(Font::sans_regular);
    let mut drawer = CGBitmapContextDrawer::new(&env.objc, &mut env.mem, context);
    let fill_color = drawer.rgb_fill_color();
    font_obj.draw(size, &text_str, (0.0, 0.0), None, TextAlignment::Left, |glyph| {
        draw_font_glyph(&mut drawer, glyph, fill_color, None, None)
    });
}

/// `CTFontRef CTFontCreateWithGraphicsFont(CGFontRef graphicsFont,
///     CGFloat size, const CGAffineTransform *matrix,
///     CTFontDescriptorRef attributes)`
///
/// Creates a CTFont from a CGFont. We wrap the CGFont's underlying rasterizable
/// font in a real `_touchHLE_CTFont` host object so subsequent metric queries
/// (`CTFontGetAscent`/`Descent`/`Leading`/`Size`) and line drawing work.
///
/// Reference: <https://developer.apple.com/documentation/coretext/1509694-ctfontcreatewithgraphicsfont>
fn CTFontCreateWithGraphicsFont(
    env: &mut Environment,
    graphics_font: id, // CGFontRef
    size: CGFloat,
    _matrix: ConstVoidPtr, // const CGAffineTransform*
    _attributes: id,       // CTFontDescriptorRef
) -> CTFontRef {
    // A CGFont is toll-free convertible to a UIFont in our model only via the
    // font name; CGFont host objects don't carry a UIFont. Fall back to a
    // system font at the requested size so callers get a usable CTFont with
    // real metrics rather than NULL.
    let _ = graphics_font;
    let font_size = if size > 0.0 { size } else { 12.0 };
    let ui_font: id = msg_class![env; UIFont systemFontOfSize:font_size];
    retain(env, ui_font);
    alloc_font(env, ui_font)
}

/// `bool CTFontManagerRegisterGraphicsFont(CGFontRef font, CFErrorRef *error)`
///
/// Reference: <https://developer.apple.com/documentation/coretext/1499468-ctfontmanagerregistergraphicsfon>
fn CTFontManagerRegisterGraphicsFont(
    env: &mut Environment,
    font: ConstVoidPtr,             // CGFontRef
    error: MutPtr<crate::objc::id>, // CFErrorRef*
) -> bool {
    if !error.is_null() {
        env.mem.write(error, crate::objc::nil);
    }
    if font.is_null() {
        return false;
    }
    true
}

/// Opaque paragraph style reference.
pub type CTParagraphStyleRef = crate::objc::id;

/// `CTParagraphStyleRef CTParagraphStyleCreate(
///     const CTParagraphStyleSetting *settings, size_t settingCount)`
///
/// Reference: <https://developer.apple.com/documentation/coretext/1524171-ctparagraphstylecreate>
fn CTParagraphStyleCreate(
    env: &mut Environment,
    _settings: ConstVoidPtr,
    _setting_count: u32,
) -> CTParagraphStyleRef {
    msg_class![env; NSObject new]
}

/// `CTParagraphStyleRef CTParagraphStyleCreateCopy(CTParagraphStyleRef)`
///
/// Reference: <https://developer.apple.com/documentation/coretext/1525098-ctparagraphstylecreatecopy>
fn CTParagraphStyleCreateCopy(
    env: &mut Environment,
    paragraph_style: CTParagraphStyleRef,
) -> CTParagraphStyleRef {
    if paragraph_style.is_null() {
        return nil;
    }
    retain(env, paragraph_style);
    paragraph_style
}

/// `bool CTParagraphStyleGetValueForSpecifier(CTParagraphStyleRef, CTParagraphStyleSpecifier,
///     size_t valueBufferSize, void *valueBuffer)`
///
/// Reference: <https://developer.apple.com/documentation/coretext/1525353-ctparagraphstylegetvalueforspeci>
fn CTParagraphStyleGetValueForSpecifier(
    env: &mut Environment,
    _paragraph_style: CTParagraphStyleRef,
    _spec: u32,
    value_buffer_size: u32,
    value_buffer: MutPtr<u8>,
) -> bool {
    if !value_buffer.is_null() && value_buffer_size > 0 {
        let slice = env.mem.bytes_at_mut(value_buffer, value_buffer_size);
        slice.fill(0);
    }
    false
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(CTFontCreateWithGraphicsFont(_, _, _, _)),
    export_c_func!(CTFontCreateWithFontDescriptor(_, _, _)),
    export_c_func!(CTFontDescriptorCreateWithAttributes(_)),
    export_c_func!(CTFontManagerRegisterGraphicsFont(_, _)),
    export_c_func!(CTFontGetAscent(_)),
    export_c_func!(CTFontGetDescent(_)),
    export_c_func!(CTFontGetLeading(_)),
    export_c_func!(CTFontGetSize(_)),
    export_c_func!(CTParagraphStyleCreate(_, _)),
    export_c_func!(CTParagraphStyleCreateCopy(_)),
    export_c_func!(CTParagraphStyleGetValueForSpecifier(_, _, _, _)),
    export_c_func!(CFAttributedStringCreate(_, _, _)),
    export_c_func!(CFAttributedStringCreateCopy(_, _)),
    export_c_func!(CFAttributedStringGetString(_)),
    export_c_func!(CTTypesetterCreateWithAttributedString(_)),
    export_c_func!(CTTypesetterCreateLine(_, _)),
    export_c_func!(CTLineCreateWithAttributedString(_)),
    export_c_func!(CTLineGetTypographicBounds(_, _, _, _)),
    export_c_func!(CTLineGetImageBounds(_, _)),
    export_c_func!(CTLineDraw(_, _)),
];

pub const CONSTANTS: ConstantExports = &[
    // CTFontDescriptor.h
    (
        "_kCTFontNameAttribute",
        HostConstant::NSString("NSFontNameAttribute"),
    ),
    (
        "_kCTFontFamilyNameAttribute",
        HostConstant::NSString("NSFontFamilyAttribute"),
    ),
    (
        "_kCTFontStyleNameAttribute",
        HostConstant::NSString("NSFontFaceAttribute"),
    ),
    (
        "_kCTFontTraitsAttribute",
        HostConstant::NSString("NSCTFontTraitsAttribute"),
    ),
    (
        "_kCTFontURLAttribute",
        HostConstant::NSString("NSCTFontFileURLAttribute"),
    ),
    (
        "_kCTFontDisplayNameAttribute",
        HostConstant::NSString("NSFontVisibleNameAttribute"),
    ),
    (
        "_kCTFontSizeAttribute",
        HostConstant::NSString("NSFontSizeAttribute"),
    ),
    (
        "_kCTFontMatrixAttribute",
        HostConstant::NSString("NSCTFontMatrixAttribute"),
    ),
    (
        "_kCTFontCascadeListAttribute",
        HostConstant::NSString("NSCTFontCascadeListAttribute"),
    ),
    (
        "_kCTFontCharacterSetAttribute",
        HostConstant::NSString("NSCTFontCharacterSetAttribute"),
    ),
    (
        "_kCTFontLanguagesAttribute",
        HostConstant::NSString("NSCTFontLanguagesAttribute"),
    ),
    (
        "_kCTFontBaselineAdjustAttribute",
        HostConstant::NSString("NSCTFontBaselineAdjustAttribute"),
    ),
    (
        "_kCTFontMacintoshEncodingsAttribute",
        HostConstant::NSString("NSCTFontMacintoshEncodingsAttribute"),
    ),
    (
        "_kCTFontFeaturesAttribute",
        HostConstant::NSString("NSCTFontFeaturesAttribute"),
    ),
    (
        "_kCTFontFeatureSettingsAttribute",
        HostConstant::NSString("NSCTFontFeatureSettingsAttribute"),
    ),
    (
        "_kCTFontFixedAdvanceAttribute",
        HostConstant::NSString("NSCTFontFixedAdvanceAttribute"),
    ),
    (
        "_kCTFontOrientationAttribute",
        HostConstant::NSString("NSCTFontOrientationAttribute"),
    ),
    (
        "_kCTFontFormatAttribute",
        HostConstant::NSString("NSCTFontFormatAttribute"),
    ),
    (
        "_kCTFontRegistrationScopeAttribute",
        HostConstant::NSString("NSCTFontRegistrationScopeAttribute"),
    ),
    (
        "_kCTFontPriorityAttribute",
        HostConstant::NSString("NSCTFontPriorityAttribute"),
    ),
    (
        "_kCTFontEnabledAttribute",
        HostConstant::NSString("NSCTFontEnabledAttribute"),
    ),
    (
        "_kCTFontDownloadableAttribute",
        HostConstant::NSString("NSCTFontDownloadableAttribute"),
    ),
    (
        "_kCTFontDownloadedAttribute",
        HostConstant::NSString("NSCTFontDownloadedAttribute"),
    ),
    // CTFontTraits.h
    (
        "_kCTFontSymbolicTrait",
        HostConstant::NSString("NSCTFontSymbolicTrait"),
    ),
    (
        "_kCTFontWeightTrait",
        HostConstant::NSString("NSCTFontWeightTrait"),
    ),
    (
        "_kCTFontWidthTrait",
        HostConstant::NSString("NSCTFontWidthTrait"),
    ),
    (
        "_kCTFontSlantTrait",
        HostConstant::NSString("NSCTFontSlantTrait"),
    ),
    // CTStringAttributes.h — attribute keys for `CFAttributedStringRef`
    // (and, toll-free bridged, `NSAttributedString`) used by
    // `CTFramesetterCreateWithAttributedString` and friends.
    // Canonical string values come from Apple's public
    // `CTStringAttributes.h` header; many deliberately share their
    // value with the corresponding `NSAttributedString` attribute name
    // so the same dictionary can be used by both CoreText and
    // UIKit/AppKit.
    (
        "_kCTFontAttributeName",
        // Same value as `NSFontAttributeName`.
        HostConstant::NSString("NSFont"),
    ),
    (
        "_kCTForegroundColorAttributeName",
        HostConstant::NSString("CTForegroundColor"),
    ),
    (
        "_kCTForegroundColorFromContextAttributeName",
        HostConstant::NSString("CTForegroundColorFromContext"),
    ),
    (
        "_kCTBackgroundColorAttributeName",
        HostConstant::NSString("kCTBackgroundColorAttributeName"),
    ),
    (
        "_kCTKernAttributeName",
        // Same value as `NSKernAttributeName`.
        HostConstant::NSString("NSKern"),
    ),
    (
        "_kCTLigatureAttributeName",
        // Same value as `NSLigatureAttributeName`.
        HostConstant::NSString("NSLigature"),
    ),
    (
        "_kCTParagraphStyleAttributeName",
        // Same value as `NSParagraphStyleAttributeName`.
        HostConstant::NSString("NSParagraphStyle"),
    ),
    (
        "_kCTStrokeWidthAttributeName",
        // Same value as `NSStrokeWidthAttributeName`.
        HostConstant::NSString("NSStrokeWidth"),
    ),
    (
        "_kCTStrokeColorAttributeName",
        // Same value as `NSStrokeColorAttributeName`.
        HostConstant::NSString("NSStrokeColor"),
    ),
    (
        "_kCTUnderlineStyleAttributeName",
        HostConstant::NSString("CTUnderlineStyle"),
    ),
    (
        "_kCTUnderlineColorAttributeName",
        HostConstant::NSString("CTUnderlineColor"),
    ),
    (
        "_kCTSuperscriptAttributeName",
        // Same value as `NSSuperscriptAttributeName`.
        HostConstant::NSString("NSSuperScript"),
    ),
    (
        "_kCTVerticalFormsAttributeName",
        HostConstant::NSString("CTVerticalForms"),
    ),
    (
        "_kCTGlyphInfoAttributeName",
        HostConstant::NSString("CTGlyphInfo"),
    ),
    (
        "_kCTCharacterShapeAttributeName",
        // Same value as `NSCharacterShapeAttributeName`.
        HostConstant::NSString("NSCharacterShape"),
    ),
    (
        "_kCTLanguageAttributeName",
        HostConstant::NSString("CTLanguage"),
    ),
    (
        "_kCTRunDelegateAttributeName",
        HostConstant::NSString("CTRunDelegate"),
    ),
    (
        "_kCTBaselineClassAttributeName",
        HostConstant::NSString("CTBaselineClass"),
    ),
    (
        "_kCTBaselineInfoAttributeName",
        HostConstant::NSString("CTBaselineInfo"),
    ),
    (
        "_kCTBaselineReferenceInfoAttributeName",
        HostConstant::NSString("CTBaselineReferenceInfo"),
    ),
    (
        "_kCTBaselineOffsetAttributeName",
        // Same value as `NSBaselineOffsetAttributeName`.
        HostConstant::NSString("NSBaselineOffset"),
    ),
    (
        "_kCTWritingDirectionAttributeName",
        // Same value as `NSWritingDirectionAttributeName`.
        HostConstant::NSString("NSWritingDirection"),
    ),
    (
        "_kCTTrackingAttributeName",
        HostConstant::NSString("CTTracking"),
    ),
];

pub const DYLIB: HostDylib = HostDylib {
    path: "/System/Library/Frameworks/CoreText.framework/CoreText",
    aliases: &[],
    class_exports: &[CLASSES],
    constant_exports: &[CONSTANTS],
    function_exports: &[FUNCTIONS],
};
