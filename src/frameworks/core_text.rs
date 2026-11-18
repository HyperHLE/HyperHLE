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
use crate::font::{Font, TextAlignment, WrapMode};
use rusttype::GlyphId;
use crate::frameworks::core_foundation::{cf_array::CFArrayRef, cf_type::CFTypeRef, CFIndex, CFRange};
use crate::frameworks::core_foundation::cf_attributed_string::CFAttributedStringRef;
use crate::frameworks::core_foundation::cf_string::CFStringRef;
use crate::frameworks::core_graphics::cg_affine_transform::CGAffineTransform;
use crate::frameworks::core_graphics::cg_bitmap_context::{
    CGBitmapContextDrawer, CGBitmapContextGetHeight,
};
use crate::frameworks::core_graphics::cg_context::{
    CGContextConcatCTM, CGContextGetCTM, CGContextRestoreGState, CGContextSaveGState,
};
use crate::frameworks::core_graphics::cg_font::{
    cg_font_with_font, data_provider_font, CGFontCreateWithFontName, CGFontRef, CGGlyph,
};
use crate::frameworks::core_graphics::{CGFloat, CGPoint, CGRect, CGSize};
use crate::frameworks::foundation::ns_string::{from_rust_string, get_static_str, to_rust_string};
use crate::frameworks::foundation::{unichar, NSRange, NSUInteger};
use crate::frameworks::uikit::ui_font::{
    custom_font_of_uifont, draw_font_glyph, font_for_text, font_from_uifont, is_uifont,
    new_uifont_with_custom_font, new_uifont_with_name,
};
use crate::mem::{ConstPtr, ConstVoidPtr, MutPtr, Ptr};
use crate::objc::{
    autorelease, id, msg, msg_class, nil, objc_classes, release, retain, ClassExports, HostObject,
};
use crate::Environment;

/// Opaque CoreText font reference.
pub type CTFontRef = crate::objc::id;

/// Opaque CoreText font descriptor reference.
pub type CTFontDescriptorRef = crate::objc::id;

/// Opaque CoreText typesetter reference.
pub type CTTypesetterRef = crate::objc::id;

/// Opaque CoreText line reference.
pub type CTLineRef = crate::objc::id;

/// `CTLineTruncationType`, from `CTLine.h`.
pub type CTLineTruncationType = u32;
pub const kCTLineTruncationStart: CTLineTruncationType = 0;
pub const kCTLineTruncationMiddle: CTLineTruncationType = 1;
pub const kCTLineTruncationEnd: CTLineTruncationType = 2;

/// Opaque CoreText run reference.
pub type CTRunRef = crate::objc::id;

/// Opaque CoreText framesetter reference.
pub type CTFramesetterRef = crate::objc::id;

/// Opaque CoreText frame reference.
pub type CTFrameRef = crate::objc::id;

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
struct CTTypesetterHostObject {
    attr_string: id,
}
impl HostObject for CTTypesetterHostObject {}

#[derive(Clone, Default)]
struct CTLineHostObject {
    text: id,
    font: id,
    attributes: id,
    /// Buffers handed out by `CTRunGetGlyphsPtr` / `CTRunGetPositionsPtr`,
    /// which must stay valid for as long as the run exists.
    glyphs_ptr: MutPtr<CGGlyph>,
    positions_ptr: MutPtr<CGPoint>,
}
impl HostObject for CTLineHostObject {}

#[derive(Clone, Default)]
struct CTFramesetterHostObject {
    attr_string: id,
}
impl HostObject for CTFramesetterHostObject {}

#[derive(Clone, Default)]
struct CTFrameHostObject {
    /// The single CTLine we produce for the (whole) attributed string. Our
    /// layout engine doesn't break a framesetter's string across multiple
    /// lines/columns, so a frame holds exactly one line. This matches what
    /// the apps in the corpus need (they mostly measure / draw short labels).
    line: id,
    /// The string range covered by the frame, mirrored back from the
    /// attributed string for `CTFrameGetStringRange`/`GetVisibleStringRange`.
    range: CFRange,
}
impl HostObject for CTFrameHostObject {}

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

@implementation _touchHLE_CTTypesetter: NSObject
- (())dealloc {
    env.objc.dealloc_object(this, &mut env.mem)
}
@end

@implementation _touchHLE_CTLine: NSObject
- (())dealloc {
    let (text, font, attributes, glyphs_ptr, positions_ptr) = {
        let host = env.objc.borrow::<CTLineHostObject>(this);
        (host.text, host.font, host.attributes, host.glyphs_ptr, host.positions_ptr)
    };
    release(env, text);
    release(env, font);
    release(env, attributes);
    env.mem.free(glyphs_ptr.cast());
    env.mem.free(positions_ptr.cast());
    env.objc.dealloc_object(this, &mut env.mem)
}
@end

@implementation _touchHLE_CTFramesetter: NSObject
- (())dealloc {
    env.objc.dealloc_object(this, &mut env.mem)
}
@end

@implementation _touchHLE_CTFrame: NSObject
- (())dealloc {
    env.objc.dealloc_object(this, &mut env.mem)
}
@end

};

fn alloc_descriptor(env: &mut Environment, attrs: id) -> CTFontDescriptorRef {
    let class = env
        .objc
        .get_known_class("_touchHLE_CTFontDescriptor", &mut env.mem);
    env.objc.alloc_object(
        class,
        Box::new(CTFontDescriptorHostObject { attrs }),
        &mut env.mem,
    )
}

fn alloc_font(env: &mut Environment, font: id) -> CTFontRef {
    let class = env.objc.get_known_class("_touchHLE_CTFont", &mut env.mem);
    env.objc
        .alloc_object(class, Box::new(CTFontHostObject { font }), &mut env.mem)
}

fn alloc_typesetter(env: &mut Environment, attr_string: id) -> CTTypesetterRef {
    let class = env
        .objc
        .get_known_class("_touchHLE_CTTypesetter", &mut env.mem);
    env.objc.alloc_object(
        class,
        Box::new(CTTypesetterHostObject { attr_string }),
        &mut env.mem,
    )
}

fn alloc_line(env: &mut Environment, text: id, font: id, attributes: id) -> CTLineRef {
    let class = env.objc.get_known_class("_touchHLE_CTLine", &mut env.mem);
    env.objc.alloc_object(
        class,
        Box::new(CTLineHostObject {
            text,
            font,
            attributes,
            glyphs_ptr: Ptr::null(),
            positions_ptr: Ptr::null(),
        }),
        &mut env.mem,
    )
}

fn alloc_framesetter(env: &mut Environment, attr_string: id) -> CTFramesetterRef {
    let class = env
        .objc
        .get_known_class("_touchHLE_CTFramesetter", &mut env.mem);
    env.objc.alloc_object(
        class,
        Box::new(CTFramesetterHostObject { attr_string }),
        &mut env.mem,
    )
}

fn alloc_frame(env: &mut Environment, line: id, range: CFRange) -> CTFrameRef {
    let class = env.objc.get_known_class("_touchHLE_CTFrame", &mut env.mem);
    env.objc.alloc_object(
        class,
        Box::new(CTFrameHostObject { line, range }),
        &mut env.mem,
    )
}

fn descriptor_attrs(env: &mut Environment, descriptor: CTFontDescriptorRef) -> id {
    env.objc
        .borrow::<CTFontDescriptorHostObject>(descriptor)
        .attrs
}

fn font_from_descriptor(
    env: &mut Environment,
    descriptor: CTFontDescriptorRef,
    size: CGFloat,
) -> id {
    let attrs = descriptor_attrs(env, descriptor);
    if attrs == nil {
        return msg_class![env; UIFont systemFontOfSize:size];
    }
    let font_name_key =
        crate::frameworks::foundation::ns_string::get_static_str(env, "NSFontNameAttribute");
    let family_key =
        crate::frameworks::foundation::ns_string::get_static_str(env, "NSFontFamilyAttribute");
    let size_key =
        crate::frameworks::foundation::ns_string::get_static_str(env, "NSFontSizeAttribute");
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

fn attributed_string_parts(env: &mut Environment, attr_string: CFAttributedStringRef) -> (id, id) {
    if attr_string == nil {
        return (nil, nil);
    }
    let string: id = msg![env; attr_string string];
    if string == nil {
        return (nil, nil);
    }
    let length: NSUInteger = msg![env; string length];
    let attrs = if length == 0 {
        nil
    } else {
        let range_ptr: MutPtr<NSRange> = MutPtr::null();
        msg![env; attr_string attributesAtIndex:(0usize) effectiveRange:range_ptr]
    };
    (string, attrs)
}

fn string_and_font_from_attr_string(
    env: &mut Environment,
    attr_string: CFAttributedStringRef,
) -> (id, id, id) {
    let (string, attrs) = attributed_string_parts(env, attr_string);
    let font = if attrs == nil {
        msg_class![env; UIFont systemFontOfSize:12.0]
    } else {
        let key = get_static_str(env, "NSFont");
        let maybe_font = msg![env; attrs objectForKey:key];
        let ui_font = if maybe_font == nil || is_uifont(env, maybe_font) {
            maybe_font
        } else {
            env.objc
                .get_host_object(maybe_font)
                .and_then(|host| host.as_any().downcast_ref::<CTFontHostObject>())
                .map(|host| host.font)
                .unwrap_or(nil)
        };
        if ui_font == nil {
            msg_class![env; UIFont systemFontOfSize:12.0]
        } else {
            ui_font
        }
    };
    (string, font, attrs)
}

fn CTFontDescriptorCreateWithAttributes(
    env: &mut Environment,
    attributes: id,
) -> CTFontDescriptorRef {
    let attrs = if attributes == nil {
        msg_class![env; NSDictionary dictionary]
    } else {
        attributes
    };
    retain(env, attrs);
    alloc_descriptor(env, attrs)
}

/// `CTFontDescriptorRef CTFontDescriptorCreateWithNameAndSize(
///     CFStringRef name, CGFloat size)`
///
/// Convenience constructor for a descriptor carrying the two most common
/// attributes: the PostScript/family name and the point size.
/// <https://developer.apple.com/documentation/coretext/1511145-ctfontdescriptorcreatewithnamea>
fn CTFontDescriptorCreateWithNameAndSize(
    env: &mut Environment,
    name: CFStringRef,
    size: CGFloat,
) -> CTFontDescriptorRef {
    if name.is_null() {
        return nil;
    }
    let attributes: id = msg_class![env; NSMutableDictionary dictionary];
    let name_key = get_static_str(env, "NSFontNameAttribute");
    let size_key = get_static_str(env, "NSFontSizeAttribute");
    let size_f = size as f64;
    let size_number: id = msg_class![env; NSNumber numberWithDouble:size_f];
    () = msg![env; attributes setObject:name forKey:name_key];
    () = msg![env; attributes setObject:size_number forKey:size_key];
    CTFontDescriptorCreateWithAttributes(env, attributes)
}

/// `CTFontDescriptorCreateMatchingFontDescriptors` — returns descriptors
/// matching the given one under `attributes` (a set of mandatory keys).
/// touchHLE's font stack only exposes the built-in system fonts, so no
/// additional descriptors can ever match: return an empty (non-null)
/// CFArray, which is what a real system returns when nothing matches.
/// Returning NULL here makes some engines treat Core Text as broken and
/// abort font fallback entirely.
fn CTFontDescriptorCreateMatchingFontDescriptors(
    env: &mut Environment,
    _descriptor: CTFontDescriptorRef,
    _attributes: CFTypeRef, // CFSetRef of mandatory attribute keys
) -> CFArrayRef {
    let empty: CFArrayRef = msg_class![env; NSArray array];
    retain(env, empty);
    empty
}

/// `CTFontDescriptorCreateMatchingFontDescriptor` — the single best match
/// for a descriptor. Every name falls back to one of the bundled fonts (see
/// [CTFontDescriptorCopyAttribute]), so the descriptor matches itself.
fn CTFontDescriptorCreateMatchingFontDescriptor(
    env: &mut Environment,
    descriptor: CTFontDescriptorRef,
    _mandatory_attributes: CFTypeRef, // CFSetRef of attribute keys
) -> CTFontDescriptorRef {
    if descriptor == nil {
        return nil;
    }
    retain(env, descriptor)
}

/// The file in `/System/Library/Fonts/Cache` (see `fs.rs`) that stands in for
/// a font with this name or family and style. touchHLE ships the metric
/// compatible Liberation fonts, plus Arial and a bold Helvetica.
fn system_font_file(name: &str, style: &str) -> &'static str {
    let name = name.to_ascii_lowercase();
    let style = style.to_ascii_lowercase();
    let bold = name.contains("bold") || style.contains("bold");
    let italic = ["italic", "oblique"]
        .iter()
        .any(|word| name.contains(word) || style.contains(word));
    let family = if name.contains("courier") || name.contains("mono") {
        "LiberationMono"
    } else if ["times", "georgia", "serif"]
        .iter()
        .any(|word| name.contains(word))
        && !name.contains("sans")
    {
        "LiberationSerif"
    } else if name.contains("helvetica") && bold && !italic {
        return "helvetica-bold.ttf";
    } else if name.contains("arial") && !bold && !italic {
        return "Arial.ttf";
    } else {
        "LiberationSans"
    };
    match (family, bold, italic) {
        ("LiberationMono", false, false) => "LiberationMono-Regular.ttf",
        ("LiberationMono", true, false) => "LiberationMono-Bold.ttf",
        ("LiberationMono", false, true) => "LiberationMono-Italic.ttf",
        ("LiberationMono", true, true) => "LiberationMono-BoldItalic.ttf",
        ("LiberationSerif", false, false) => "LiberationSerif-Regular.ttf",
        ("LiberationSerif", true, false) => "LiberationSerif-Bold.ttf",
        ("LiberationSerif", false, true) => "LiberationSerif-Italic.ttf",
        ("LiberationSerif", true, true) => "LiberationSerif-BoldItalic.ttf",
        (_, false, false) => "LiberationSans-Regular.ttf",
        (_, true, false) => "LiberationSans-Bold.ttf",
        (_, false, true) => "LiberationSans-Italic.ttf",
        (_, true, true) => "LiberationSans-BoldItalic.ttf",
    }
}

/// `CFTypeRef CTFontDescriptorCopyAttribute(CTFontDescriptorRef descriptor,
///     CFStringRef attribute)`. Attributes the descriptor was created with are
/// returned as is. The font file URL is filled in from the bundled fonts:
/// engines such as Unity load the font file themselves through it, and draw
/// no text at all without one.
fn CTFontDescriptorCopyAttribute(
    env: &mut Environment,
    descriptor: CTFontDescriptorRef,
    attribute: id,
) -> CFTypeRef {
    if descriptor == nil || attribute == nil {
        return nil;
    }
    let attrs = descriptor_attrs(env, descriptor);
    if attrs != nil {
        let value: id = msg![env; attrs objectForKey:attribute];
        if value != nil {
            return retain(env, value);
        }
    }
    let url_key = get_static_str(env, "NSCTFontFileURLAttribute");
    let is_url_key: bool = msg![env; attribute isEqualToString:url_key];
    if !is_url_key {
        return nil;
    }
    let mut attr_string = |key: &'static str| {
        if attrs == nil {
            return None;
        }
        let key = get_static_str(env, key);
        let value: id = msg![env; attrs objectForKey:key];
        (value != nil).then(|| to_rust_string(env, value).into_owned())
    };
    let name = attr_string("NSFontNameAttribute")
        .or_else(|| attr_string("NSFontFamilyAttribute"))
        .unwrap_or_default();
    let style = attr_string("NSFontFaceAttribute").unwrap_or_default();
    let path = format!(
        "/System/Library/Fonts/Cache/{}",
        system_font_file(&name, &style)
    );
    let path = from_rust_string(env, path);
    let url: id = msg_class![env; NSURL fileURLWithPath:path];
    release(env, path);
    retain(env, url)
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

fn CTFontCreateWithName(
    env: &mut Environment,
    name: id,
    size: CGFloat,
    _matrix: ConstVoidPtr,
) -> CTFontRef {
    if name.is_null() {
        return nil;
    }
    let size = if size == 0.0 { 12.0 } else { size };
    let name = to_rust_string(env, name).into_owned();
    let font = new_uifont_with_name(env, &name, size);
    retain(env, font);
    alloc_font(env, font)
}

/// Resolve a `CTFontRef` to the `UIFont` backing it. Also accepts a bare
/// `UIFont`, which some apps pass where a `CTFontRef` is expected.
fn ui_font_for_ct_font(env: &mut Environment, font: CTFontRef) -> id {
    if font == nil {
        return nil;
    }
    if is_uifont(env, font) {
        return font;
    }
    env.objc
        .get_host_object(font)
        .and_then(|host| host.as_any().downcast_ref::<CTFontHostObject>())
        .map(|host| host.font)
        .unwrap_or(nil)
}

/// `CTFontRef CTFontCreateCopyWithAttributes(CTFontRef font, CGFloat size,
///     const CGAffineTransform *matrix, CTFontDescriptorRef attributes)`
///
/// Reference: <https://developer.apple.com/documentation/coretext/1509393-ctfontcreatecopywithattributes>
///
/// > Returns a new font with additional attributes based on the original
/// > font. [...] `size`: The point size for the font reference. If 0.0 is
/// > specified, the original font's size is preserved.
///
/// The original font's name is kept unless `attributes` carries a font name
/// or family of its own, in which case those win (descriptor attributes
/// override the original font's, as on iOS). Like the other CTFont
/// constructors here the transform matrix is not applied: touchHLE's text
/// renderer only supports untransformed fonts.
fn CTFontCreateCopyWithAttributes(
    env: &mut Environment,
    font: CTFontRef,
    size: CGFloat,
    _matrix: ConstPtr<CGAffineTransform>,
    attributes: CTFontDescriptorRef,
) -> CTFontRef {
    let original = ui_font_for_ct_font(env, font);
    if original == nil {
        return nil;
    }
    let size: CGFloat = if size > 0.0 {
        size
    } else {
        msg![env; original pointSize]
    };

    let overrides_face = attributes != nil && {
        let attrs = descriptor_attrs(env, attributes);
        attrs != nil && {
            let name_key = get_static_str(env, "NSFontNameAttribute");
            let family_key = get_static_str(env, "NSFontFamilyAttribute");
            let by_name: id = msg![env; attrs objectForKey:name_key];
            let by_family: id = msg![env; attrs objectForKey:family_key];
            by_name != nil || by_family != nil
        }
    };

    let new_font: id = if overrides_face {
        font_from_descriptor(env, attributes, size)
    } else {
        msg![env; original fontWithSize:size]
    };
    let new_font = if new_font == nil { original } else { new_font };
    retain(env, new_font);
    alloc_font(env, new_font)
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

fn CTFontCopyPostScriptName(env: &mut Environment, font: CTFontRef) -> id {
    if font.is_null() {
        return nil;
    }
    let ui_font = env.objc.borrow::<CTFontHostObject>(font).font;
    if ui_font == nil {
        return nil;
    }
    let name: id = msg![env; ui_font fontName];
    if name == nil { nil } else { retain(env, name) }
}

fn CTFontGetAscent(env: &mut Environment, font: CTFontRef) -> CGFloat {
    if font.is_null() {
        return 0.0;
    }
    let ui_font = env.objc.borrow::<CTFontHostObject>(font).font;
    if ui_font == nil {
        return 0.0;
    }
    msg![env; ui_font ascender]
}

fn CTFontGetDescent(env: &mut Environment, font: CTFontRef) -> CGFloat {
    if font.is_null() {
        return 0.0;
    }
    let ui_font = env.objc.borrow::<CTFontHostObject>(font).font;
    if ui_font == nil {
        return 0.0;
    }
    -msg![env; ui_font descender]
}

fn CTFontGetLeading(env: &mut Environment, font: CTFontRef) -> CGFloat {
    if font.is_null() {
        return 0.0;
    }
    let ui_font = env.objc.borrow::<CTFontHostObject>(font).font;
    if ui_font == nil {
        return 0.0;
    }
    msg![env; ui_font leading]
}

/// `uint32_t CTGetCoreTextVersion(void)`
///
/// Encodes the CoreText feature level using the `kCTVersionNumber*`
/// constants from `CTBase.h`. iOS 4.3's CoreText corresponds to the
/// 10.6-level API (0x00030000), which is what apps compare against when
/// deciding whether e.g. framesetter APIs are present.
/// <https://developer.apple.com/documentation/coretext/1499498-ctgetcoretextversion>
fn CTGetCoreTextVersion(_env: &mut Environment) -> u32 {
    0x0003_0000 // kCTVersionNumber10_6
}

fn CTTypesetterCreateWithAttributedString(
    env: &mut Environment,
    string: CFAttributedStringRef,
) -> CTTypesetterRef {
    if string.is_null() {
        return nil;
    }
    retain(env, string);
    alloc_typesetter(env, string)
}

fn CTTypesetterCreateLine(
    env: &mut Environment,
    typesetter: CTTypesetterRef,
    _range: NSRange,
) -> CTLineRef {
    if typesetter.is_null() {
        return nil;
    }
    let attr_string = env
        .objc
        .borrow::<CTTypesetterHostObject>(typesetter)
        .attr_string;
    let (text, font, attributes) = string_and_font_from_attr_string(env, attr_string);
    retain(env, text);
    retain(env, font);
    retain(env, attributes);
    alloc_line(env, text, font, attributes)
}

fn CTLineCreateWithAttributedString(
    env: &mut Environment,
    string: CFAttributedStringRef,
) -> CTLineRef {
    if string.is_null() {
        return nil;
    }
    let (text, font, attributes) = string_and_font_from_attr_string(env, string);
    retain(env, text);
    retain(env, font);
    retain(env, attributes);
    alloc_line(env, text, font, attributes)
}

/// `CFIndex CTTypesetterSuggestClusterBreak(
///     CTTypesetterRef typesetter, CFIndex startIndex, double width)`
///
/// Returns how many characters, counted from `startIndex`, fit into `width`
/// without splitting a cluster. Like real Core Text, we always let at least
/// one cluster through so callers that loop on the result make progress.
/// Metrics come from the same font measurement `CTLineGetTypographicBounds`
/// uses.
/// <https://developer.apple.com/documentation/coretext/1509556-cttypesettersuggestclusterbreak>
fn CTTypesetterSuggestClusterBreak(
    env: &mut Environment,
    typesetter: CTTypesetterRef,
    start_index: CFIndex,
    width: f64,
) -> CFIndex {
    if typesetter.is_null() || start_index < 0 || !(width > 0.0) {
        return 0;
    }
    let attr_string = env
        .objc
        .borrow::<CTTypesetterHostObject>(typesetter)
        .attr_string;
    let (text, font, _attributes) = string_and_font_from_attr_string(env, attr_string);
    if text == nil || font == nil {
        return 0;
    }
    let text_str = to_rust_string(env, text).into_owned();
    let font_size: CGFloat = msg![env; font pointSize];
    let font_obj = font_for_text(env, font, &text_str).unwrap_or_else(Font::sans_regular);
    let mut count = 0;
    for (char_index, (byte_offset, ch)) in text_str.char_indices().enumerate() {
        if char_index < start_index as usize {
            continue;
        }
        let candidate_end = byte_offset + ch.len_utf8();
        let measured = font_obj.calculate_text_size(font_size, &text_str[..candidate_end], None).0;
        if measured > width as f32 && count > 0 {
            break;
        }
        count += 1;
    }
    count as CFIndex
}

/// Longest prefix of `text` whose measured width does not exceed `budget`.
fn fitting_prefix<'a>(measure: &impl Fn(&str) -> f32, text: &'a str, budget: f32) -> &'a str {
    let mut end = 0;
    for (offset, ch) in text.char_indices() {
        let candidate_end = offset + ch.len_utf8();
        if measure(&text[..candidate_end]) > budget {
            break;
        }
        end = candidate_end;
    }
    &text[..end]
}

/// Longest suffix of `text` whose measured width does not exceed `budget`.
fn fitting_suffix<'a>(measure: &impl Fn(&str) -> f32, text: &'a str, budget: f32) -> &'a str {
    let mut start = text.len();
    for (offset, _ch) in text.char_indices().rev() {
        if measure(&text[offset..]) > budget {
            break;
        }
        start = offset;
    }
    &text[start..]
}

/// `CTLineRef CTLineCreateTruncatedLine(
///     CTLineRef line, double width, CTLineTruncationType truncationType,
///     CTLineRef truncationToken)`
///
/// The truncated line keeps the original line's font and attributes and
/// carries the token text at the start, middle or end as requested. Per Apple,
/// a `NULL` token means no token is displayed (the line is simply clipped).
/// <https://developer.apple.com/documentation/coretext/1509487-ctlinecreatetruncatedline>
fn CTLineCreateTruncatedLine(
    env: &mut Environment,
    line: CTLineRef,
    width: f64,
    truncation_type: CTLineTruncationType,
    truncation_token: CTLineRef,
) -> CTLineRef {
    if line.is_null() || !(width > 0.0) {
        return nil;
    }
    let text = line_text(env, line);
    let font = line_font(env, line);
    if text == nil || font == nil {
        return nil;
    }
    let attributes = env.objc.borrow::<CTLineHostObject>(line).attributes;
    let text_str = to_rust_string(env, text).into_owned();
    let font_size: CGFloat = msg![env; font pointSize];
    let font_obj = font_for_text(env, font, &text_str).unwrap_or_else(Font::sans_regular);
    let measure = |s: &str| font_obj.calculate_text_size(font_size, s, None).0;
    let new_text = if measure(&text_str) as f64 <= width {
        text_str
    } else {
        let token_text = if truncation_token.is_null() {
            String::new()
        } else {
            let token = line_text(env, truncation_token);
            if token == nil {
                String::new()
            } else {
                to_rust_string(env, token).into_owned()
            }
        };
        let budget = (width as f32 - measure(&token_text)).max(0.0);
        match truncation_type {
            kCTLineTruncationStart => {
                let tail = fitting_suffix(&measure, &text_str, budget);
                format!("{token_text}{tail}")
            }
            kCTLineTruncationMiddle => {
                let head = fitting_prefix(&measure, &text_str, budget / 2.0);
                let rest = &text_str[head.len()..];
                let tail = fitting_suffix(&measure, rest, budget / 2.0);
                format!("{head}{token_text}{tail}")
            }
            _ => {
                let head = fitting_prefix(&measure, &text_str, budget);
                format!("{head}{token_text}")
            }
        }
    };
    let new_text = from_rust_string(env, new_text);
    retain(env, font);
    retain(env, attributes);
    alloc_line(env, new_text, font, attributes)
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
    let font_obj = font_for_text(env, font, &text_str).unwrap_or_else(Font::sans_regular);
    let width = font_obj.calculate_text_size(size, &text_str, None).0;
    if !ascent.is_null() {
        env.mem.write(ascent, ascent_val);
    }
    if !descent.is_null() {
        env.mem.write(descent, -descender_val);
    }
    if !leading.is_null() {
        env.mem.write(leading, leading_val);
    }
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
    let font_obj = font_for_text(env, font, &text_str).unwrap_or_else(Font::sans_regular);
    let (w, h) = font_obj.calculate_text_size(size, &text_str, None);
    CGRect {
        origin: CGPoint {
            x: 0.0,
            y: -ascent_val,
        },
        size: CGSize {
            width: w,
            height: h,
        },
    }
}

fn CTLineDraw(env: &mut Environment, line: CTLineRef, context: id) {
    if line.is_null() || context.is_null() {
        return;
    }
    let text = line_text(env, line);
    let font = line_font(env, line);
    let size: CGFloat = msg![env; font pointSize];
    let text_str = to_rust_string(env, text).into_owned();
    let font_obj = font_for_text(env, font, &text_str).unwrap_or_else(Font::sans_regular);
    let needs_y_flip = CGContextGetCTM(env, context).d > 0.0;
    if needs_y_flip {
        let height = CGBitmapContextGetHeight(env, context) as CGFloat;
        CGContextSaveGState(env, context);
        CGContextConcatCTM(
            env,
            context,
            CGAffineTransform {
                a: 1.0,
                b: 0.0,
                c: 0.0,
                d: -1.0,
                tx: 0.0,
                ty: height,
            },
        );
    }
    let mut drawer = CGBitmapContextDrawer::new(&env.objc, &mut env.mem, context);
    let fill_color = drawer.rgb_fill_color();
    font_obj.draw(
        size,
        &text_str,
        (0.0, 0.0),
        None,
        TextAlignment::Left,
        |glyph| draw_font_glyph(&mut drawer, glyph, fill_color, None, None),
    );
    drop(drawer);
    if needs_y_flip {
        CGContextRestoreGState(env, context);
    }
}

/// `CFIndex CTLineGetGlyphCount(CTLineRef line)`
///
/// Returns the total glyph count for the line. We map the line's text to
/// glyphs through the backing font's cmap (one glyph per UTF-16 code unit,
/// which matches CoreText for the simple Latin/Cyrillic text these apps lay
/// out), so callers that size buffers off this count get a usable value.
///
/// Reference: <https://developer.apple.com/documentation/coretext/1509609-ctlinegetglyphcount>
fn CTLineGetGlyphCount(env: &mut Environment, line: CTLineRef) -> i32 {
    if line.is_null() {
        return 0;
    }
    let text = line_text(env, line);
    if text == nil {
        return 0;
    }
    let len: NSUInteger = msg![env; text length];
    len as i32
}

/// `CFArrayRef CTLineGetGlyphRuns(CTLineRef line)`
///
/// Returns the array of glyph runs that make up the line. touchHLE lays text
/// out as a single run, so we return a one-element array wrapping the line
/// itself (our `_touchHLE_CTLine` host object also stands in for a run — the
/// run-level getters used by the apps in the corpus, e.g. glyph count, work
/// the same way). Returning a real (empty-safe) `CFArray` rather than NULL
/// prevents callers from dereferencing a NULL array.
///
/// Reference: <https://developer.apple.com/documentation/coretext/1509596-ctlinegetglyphruns>
fn CTLineGetGlyphRuns(env: &mut Environment, line: CTLineRef) -> id {
    if line.is_null() {
        return nil;
    }
    let arr: id = msg_class![env; NSMutableArray arrayWithCapacity:1u32];
    () = msg![env; arr addObject:line];
    let immutable: id = msg![env; arr copy];
    autorelease(env, immutable)
}

/// `CFIndex CTRunGetGlyphCount(CTRunRef run)`
fn CTRunGetGlyphCount(env: &mut Environment, run: CTRunRef) -> CFIndex {
    CTLineGetGlyphCount(env, run)
}

/// `CFDictionaryRef CTRunGetAttributes(CTRunRef run)`
fn CTRunGetAttributes(env: &mut Environment, run: CTRunRef) -> id {
    if run.is_null() {
        return nil;
    }
    let attributes = env.objc.borrow::<CTLineHostObject>(run).attributes;
    if attributes == nil {
        msg_class![env; NSDictionary dictionary]
    } else {
        attributes
    }
}

fn glyph_range_for_run(range: CFRange, glyph_count: CFIndex) -> Option<(u32, u32)> {
    let total = u32::try_from(glyph_count).ok()?;
    let start = u32::try_from(range.location).ok()?;
    if start > total {
        return None;
    }
    let length = if range.location == 0 && range.length == 0 {
        total
    } else {
        u32::try_from(range.length).ok()?
    };
    Some((start, start.saturating_add(length).min(total)))
}

fn glyph_for_code_unit(font: &Font, code_unit: unichar) -> CGGlyph {
    let character = char::from_u32(code_unit as u32).unwrap_or(char::REPLACEMENT_CHARACTER);
    font.glyph_id_for_char(character as u16).0
}

/// `void CTRunGetGlyphs(CTRunRef run, CFRange range, CGGlyph buffer[])`
fn CTRunGetGlyphs(env: &mut Environment, run: CTRunRef, range: CFRange, buffer: MutPtr<CGGlyph>) {
    if run.is_null() || buffer.is_null() {
        return;
    }
    let (text, ui_font) = {
        let host = env.objc.borrow::<CTLineHostObject>(run);
        (host.text, host.font)
    };
    if text == nil {
        return;
    }
    let count = CTLineGetGlyphCount(env, run);
    let Some((start, end)) = glyph_range_for_run(range, count) else {
        return;
    };
    let text_str = to_rust_string(env, text).into_owned();
    let font = font_for_text(env, ui_font, &text_str).unwrap_or_else(Font::sans_regular);
    for (output_index, text_index) in (start..end).enumerate() {
        let character: unichar = msg![env; text characterAtIndex:(text_index as NSUInteger)];
        env.mem.write(buffer + output_index as u32, glyph_for_code_unit(&font, character));
    }
}

/// `const CGGlyph *CTRunGetGlyphsPtr(CTRunRef run)`
///
/// CoreText may return NULL here, telling the caller to copy the glyphs out
/// with `CTRunGetGlyphs` instead, but apps often skip that fallback: Real
/// Racing 3 only imports this function and read every glyph as 0 (.notdef),
/// so its text came out as a row of boxes. Hand out a buffer that lives as
/// long as the run, like CoreText's own storage.
fn CTRunGetGlyphsPtr(env: &mut Environment, run: CTRunRef) -> ConstPtr<CGGlyph> {
    if run.is_null() {
        return Ptr::null();
    }
    let existing = env.objc.borrow::<CTLineHostObject>(run).glyphs_ptr;
    if !existing.is_null() {
        return existing.cast_const();
    }
    let count = CTRunGetGlyphCount(env, run).max(0) as u32;
    if count == 0 {
        return Ptr::null();
    }
    let buffer: MutPtr<CGGlyph> = env.mem.calloc(count * 2).cast();
    CTRunGetGlyphs(
        env,
        run,
        CFRange {
            location: 0,
            length: 0,
        },
        buffer,
    );
    env.objc.borrow_mut::<CTLineHostObject>(run).glyphs_ptr = buffer;
    buffer.cast_const()
}

/// `const CGPoint *CTRunGetPositionsPtr(CTRunRef run)`
///
/// See [CTRunGetGlyphsPtr].
fn CTRunGetPositionsPtr(env: &mut Environment, run: CTRunRef) -> ConstPtr<CGPoint> {
    if run.is_null() {
        return Ptr::null();
    }
    let existing = env.objc.borrow::<CTLineHostObject>(run).positions_ptr;
    if !existing.is_null() {
        return existing.cast_const();
    }
    let count = CTRunGetGlyphCount(env, run).max(0) as u32;
    if count == 0 {
        return Ptr::null();
    }
    let size = count * crate::mem::guest_size_of::<CGPoint>();
    let buffer: MutPtr<CGPoint> = env.mem.calloc(size).cast();
    CTRunGetPositions(
        env,
        run,
        CFRange {
            location: 0,
            length: 0,
        },
        buffer,
    );
    env.objc.borrow_mut::<CTLineHostObject>(run).positions_ptr = buffer;
    buffer.cast_const()
}

/// `void CTRunGetPositions(CTRunRef run, CFRange range, CGPoint buffer[])`
fn CTRunGetPositions(env: &mut Environment, run: CTRunRef, range: CFRange, buffer: MutPtr<CGPoint>) {
    if run.is_null() || buffer.is_null() {
        return;
    }
    let (text, ui_font) = {
        let host = env.objc.borrow::<CTLineHostObject>(run);
        (host.text, host.font)
    };
    if text == nil {
        return;
    }
    let count = CTLineGetGlyphCount(env, run);
    let Some((start, end)) = glyph_range_for_run(range, count) else {
        return;
    };
    let text_str = to_rust_string(env, text).into_owned();
    let font = font_for_text(env, ui_font, &text_str).unwrap_or_else(Font::sans_regular);
    let size: CGFloat = msg![env; ui_font pointSize];
    let glyphs: Vec<GlyphId> = (0..end)
        .map(|text_index| {
            let character: unichar = msg![env; text characterAtIndex:(text_index as NSUInteger)];
            GlyphId(glyph_for_code_unit(&font, character))
        })
        .collect();
    let positions = font.glyph_positions_at_size(size, &glyphs);
    for text_index in start..end {
        env.mem.write(
            buffer + (text_index - start),
            CGPoint {
                x: positions[text_index as usize],
                y: 0.0,
            },
        );
    }
}

/// `double CTFontGetAdvancesForGlyphs(CTFontRef font, CTFontOrientation orientation,
///     const CGGlyph glyphs[], CGSize advances[], CFIndex count)`
///
/// Returns the total advance width for the given glyphs and fills the
/// caller's array with per-glyph advances (width only; height 0, matching
/// horizontal orientation). The `orientation` parameter (0 = default /
/// horizontal, 1 = vertical) is accepted but only horizontal metrics are
/// produced — vertical layout is not used by apps in the corpus.
///
/// Reference: <https://developer.apple.com/documentation/coretext/ctfontgetadvancesforglyphs(_:_:_:_:_:)>
fn CTFontGetAdvancesForGlyphs(
    env: &mut Environment,
    font: CTFontRef,
    _orientation: u32,
    glyphs: ConstPtr<CGGlyph>,
    advances: MutPtr<CGSize>,
    count: i32,
) -> f64 {
    if font.is_null() || glyphs.is_null() || count <= 0 {
        return 0.0;
    }
    let ui_font = env.objc.borrow::<CTFontHostObject>(font).font;
    let font_obj = font_from_uifont(env, ui_font).unwrap_or_else(Font::sans_regular);
    let size = CTFontGetSize(env, font);
    let mut total = 0.0f64;
    for i in 0..count as u32 {
        let glyph_id: CGGlyph = env.mem.read(glyphs + i);
        let advance_pt = font_obj.glyph_advance_at_size(GlyphId(glyph_id), size) as f64;
        if !advances.is_null() {
            env.mem.write(advances + i, CGSize { width: advance_pt as CGFloat, height: 0.0 });
        }
        total += advance_pt;
    }
    total
}

/// `CGRect CTFontGetBoundingRectsForGlyphs(CTFontRef font, CTFontOrientation orientation,
///     const CGGlyph glyphs[], CGRect boundingRects[], CFIndex count)`
fn CTFontGetBoundingRectsForGlyphs(
    env: &mut Environment,
    font: CTFontRef,
    _orientation: u32,
    glyphs: ConstPtr<CGGlyph>,
    bounding_rects: MutPtr<CGRect>,
    count: CFIndex,
) -> CGRect {
    let empty = CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size: CGSize { width: 0.0, height: 0.0 },
    };
    if font.is_null() || glyphs.is_null() || count <= 0 {
        return empty;
    }
    let ui_font = env.objc.borrow::<CTFontHostObject>(font).font;
    let font_obj = font_from_uifont(env, ui_font).unwrap_or_else(Font::sans_regular);
    let size = CTFontGetSize(env, font);
    let mut union: Option<CGRect> = None;
    for index in 0..count as u32 {
        let glyph = env.mem.read(glyphs + index);
        let (x, y, width, height) = font_obj.glyph_bbox_at_size(GlyphId(glyph), size);
        let rect = CGRect {
            origin: CGPoint { x, y },
            size: CGSize { width, height },
        };
        if !bounding_rects.is_null() {
            env.mem.write(bounding_rects + index, rect);
        }
        union = Some(if let Some(current) = union {
            let min_x = current.origin.x.min(rect.origin.x);
            let min_y = current.origin.y.min(rect.origin.y);
            let max_x = (current.origin.x + current.size.width).max(rect.origin.x + rect.size.width);
            let max_y = (current.origin.y + current.size.height).max(rect.origin.y + rect.size.height);
            CGRect {
                origin: CGPoint { x: min_x, y: min_y },
                size: CGSize { width: max_x - min_x, height: max_y - min_y },
            }
        } else {
            rect
        });
    }
    union.unwrap_or(empty)
}

/// `bool CTFontGetGlyphsForCharacters(CTFontRef font, const UniChar characters[],
///     CGGlyph glyphs[], CFIndex count)`
///
/// Provides basic Unicode-to-glyph mapping for a font, writing one `CGGlyph`
/// per input `UniChar` via the backing font's cmap. Returns `true` only if
/// every character mapped to a non-zero (non-`.notdef`) glyph, exactly as
/// Apple documents.
///
/// Reference: <https://developer.apple.com/documentation/coretext/ctfontgetglyphsforcharacters(_:_:_:_:)>
fn CTFontGetGlyphsForCharacters(
    env: &mut Environment,
    font: CTFontRef,
    characters: ConstPtr<unichar>,
    glyphs: MutPtr<CGGlyph>,
    count: i32,
) -> bool {
    if font.is_null() || characters.is_null() || glyphs.is_null() || count <= 0 {
        return false;
    }
    // Resolve the backing UIFont, then its rasterizable Font.
    let ui_font = env.objc.borrow::<CTFontHostObject>(font).font;
    let mut text = String::new();
    for index in 0..count as u32 {
        let code_unit: unichar = env.mem.read(characters + index);
        if let Some(character) = char::from_u32(code_unit as u32) {
            text.push(character);
        }
    }
    let font_obj = font_for_text(env, ui_font, &text).unwrap_or_else(Font::sans_regular);
    let mut all_mapped = true;
    for i in 0..count as u32 {
        let c: unichar = env.mem.read(characters + i);
        let glyph_id = font_obj.glyph_id_for_char(c);
        let g = glyph_id.0;
        if g == 0 {
            all_mapped = false;
        }
        env.mem.write(glyphs + i, g);
    }
    all_mapped
}

/// `CGFontRef CTFontCopyGraphicsFont(CTFontRef font, CTFontDescriptorRef *attributes)`
///
/// Returns a `CGFont` for the given `CTFont`. We bridge through the backing
/// `UIFont` and create a `CGFont` from its name, so subsequent CGFont glyph
/// queries operate on the same typeface. The caller owns the returned font
/// (the "Copy" rule), so it is returned retained.
///
/// Reference: <https://developer.apple.com/documentation/coretext/1509342-ctfontcopygraphicsfont>
fn CTFontCopyGraphicsFont(
    env: &mut Environment,
    font: CTFontRef,
    attributes: MutPtr<CTFontDescriptorRef>,
) -> CGFontRef {
    if !attributes.is_null() {
        env.mem.write(attributes, nil);
    }
    if font.is_null() {
        return Ptr::null();
    }
    let ui_font = env.objc.borrow::<CTFontHostObject>(font).font;
    if let Some(custom) = custom_font_of_uifont(env, ui_font) {
        return cg_font_with_font(env, custom);
    }
    // Derive a font name from the UIFont and build a CGFont from it. This
    // matches what CoreText does (a CTFont and its CGFont share a typeface).
    let name: id = if ui_font != nil {
        msg![env; ui_font fontName]
    } else {
        nil
    };
    let cg_font = if name != nil {
        CGFontCreateWithFontName(env, name)
    } else {
        Ptr::null()
    };
    cg_font
}

/// `CTFramesetterRef CTFramesetterCreateWithAttributedString(CFAttributedStringRef string)`
///
/// Creates a framesetter that manages laying the attributed string into
/// frames. We retain the attributed string and wrap it in a host object so
/// frame creation and size suggestion can re-derive the text and font.
///
/// Reference: <https://developer.apple.com/documentation/coretext/1509611-ctframesettercreatewithattribute>
fn CTFramesetterCreateWithAttributedString(
    env: &mut Environment,
    string: CFAttributedStringRef,
) -> CTFramesetterRef {
    if string.is_null() {
        return nil;
    }
    retain(env, string);
    alloc_framesetter(env, string)
}

/// `CGSize CTFramesetterSuggestFrameSizeWithConstraints(CTFramesetterRef framesetter,
///     CFRange stringRange, CFDictionaryRef frameAttributes, CGSize constraints,
///     CFRange *fitRange)`
///
/// Determines the frame size needed for a string range. We measure the text
/// with the backing font, wrapping at the constraint width when one is given
/// (a width <= 0 or non-finite means "unconstrained"), and report the fit
/// range as the whole string. This produces correct sizing for the
/// single-/multi-line labels the apps in the corpus lay out.
///
/// Reference: <https://developer.apple.com/documentation/coretext/ctframesettersuggestframesizewithconstraints(_:_:_:_:_:)>
fn CTFramesetterSuggestFrameSizeWithConstraints(
    env: &mut Environment,
    framesetter: CTFramesetterRef,
    string_range: CFRange,
    _frame_attributes: id,
    constraints: CGSize,
    fit_range: MutPtr<CFRange>,
) -> CGSize {
    if framesetter.is_null() {
        if !fit_range.is_null() {
            env.mem.write(
                fit_range,
                CFRange {
                    location: 0,
                    length: 0,
                },
            );
        }
        return CGSize {
            width: 0.0,
            height: 0.0,
        };
    }
    let attr_string = env
        .objc
        .borrow::<CTFramesetterHostObject>(framesetter)
        .attr_string;
    let (text, font, _) = string_and_font_from_attr_string(env, attr_string);
    let size: CGFloat = msg![env; font pointSize];
    let text_str = to_rust_string(env, text).into_owned();
    let font_obj = font_for_text(env, font, &text_str).unwrap_or_else(Font::sans_regular);

    let wrap = if constraints.width.is_finite() && constraints.width > 0.0 {
        Some((constraints.width, WrapMode::Word))
    } else {
        None
    };
    let (w, h) = font_obj.calculate_text_size(size, &text_str, wrap);

    let full_len = {
        let len: NSUInteger = msg![env; text length];
        len as i32
    };
    if !fit_range.is_null() {
        // Report the entire requested range as fitting. A length of 0 in the
        // requested range means "to the end of the string" per Apple's docs.
        let length = if string_range.length == 0 {
            (full_len - string_range.location).max(0)
        } else {
            string_range.length
        };
        env.mem.write(
            fit_range,
            CFRange {
                location: string_range.location,
                length,
            },
        );
    }
    CGSize {
        width: w,
        height: h,
    }
}

/// `CTFrameRef CTFramesetterCreateFrame(CTFramesetterRef framesetter,
///     CFRange stringRange, CGPathRef path, CFDictionaryRef frameAttributes)`
///
/// Creates a frame laying the framesetter's attributed string into the
/// supplied path's bounding region. touchHLE represents the laid-out frame as
/// a single `CTLine` over the whole text; `CTFrameGetLines` then returns it.
///
/// Reference: <https://developer.apple.com/documentation/coretext/1509702-ctframesettercreateframe>
fn CTFramesetterCreateFrame(
    env: &mut Environment,
    framesetter: CTFramesetterRef,
    string_range: CFRange,
    _path: id,
    _frame_attributes: id,
) -> CTFrameRef {
    if framesetter.is_null() {
        return nil;
    }
    let attr_string = env
        .objc
        .borrow::<CTFramesetterHostObject>(framesetter)
        .attr_string;
    let (text, font, attributes) = string_and_font_from_attr_string(env, attr_string);
    retain(env, text);
    retain(env, font);
    retain(env, attributes);
    let line = alloc_line(env, text, font, attributes);
    // alloc_line took the retains above; the frame owns the line.
    let full_len = {
        let len: NSUInteger = msg![env; text length];
        len as i32
    };
    let range = CFRange {
        location: string_range.location,
        length: if string_range.length == 0 {
            (full_len - string_range.location).max(0)
        } else {
            string_range.length
        },
    };
    alloc_frame(env, line, range)
}

/// `CFArrayRef CTFrameGetLines(CTFrameRef frame)`
///
/// Returns the array of `CTLine`s that make up the frame. touchHLE lays the
/// frame out as a single line, so this returns a one-element array.
///
/// Reference: <https://developer.apple.com/documentation/coretext/1509601-ctframegetlines>
fn CTFrameGetLines(env: &mut Environment, frame: CTFrameRef) -> id {
    if frame.is_null() {
        return nil;
    }
    let line = env.objc.borrow::<CTFrameHostObject>(frame).line;
    let arr: id = msg_class![env; NSMutableArray arrayWithCapacity:1u32];
    if line != nil {
        () = msg![env; arr addObject:line];
    }
    let immutable: id = msg![env; arr copy];
    autorelease(env, immutable)
}

/// `void CTFrameGetLineOrigins(CTFrameRef frame, CFRange range, CGPoint origins[])`
///
/// Writes the origin of each line into `origins`. Our frame is a single line
/// whose origin sits at the top-left of the frame's coordinate space.
///
/// Reference: <https://developer.apple.com/documentation/coretext/1509559-ctframegetlineorigins>
fn CTFrameGetLineOrigins(
    env: &mut Environment,
    frame: CTFrameRef,
    range: CFRange,
    origins: MutPtr<CGPoint>,
) {
    if frame.is_null() || origins.is_null() {
        return;
    }
    let line = env.objc.borrow::<CTFrameHostObject>(frame).line;
    let line_count: i32 = if line != nil { 1 } else { 0 };
    let count = if range.length == 0 {
        line_count
    } else {
        range.length.min(line_count)
    };
    for i in 0..count as u32 {
        env.mem.write(origins + i, CGPoint { x: 0.0, y: 0.0 });
    }
}

/// `CFRange CTFrameGetStringRange(CTFrameRef frame)`
///
/// Reference: <https://developer.apple.com/documentation/coretext/1509593-ctframegetstringrange>
fn CTFrameGetStringRange(env: &mut Environment, frame: CTFrameRef) -> CFRange {
    if frame.is_null() {
        return CFRange {
            location: 0,
            length: 0,
        };
    }
    env.objc.borrow::<CTFrameHostObject>(frame).range
}

/// `CFRange CTFrameGetVisibleStringRange(CTFrameRef frame)`
///
/// We lay the entire string into the (single-line) frame, so the visible
/// range equals the full string range.
///
/// Reference: <https://developer.apple.com/documentation/coretext/1509563-ctframegetvisiblestringrange>
fn CTFrameGetVisibleStringRange(env: &mut Environment, frame: CTFrameRef) -> CFRange {
    if frame.is_null() {
        return CFRange {
            location: 0,
            length: 0,
        };
    }
    env.objc.borrow::<CTFrameHostObject>(frame).range
}

/// `void CTFrameDraw(CTFrameRef frame, CGContextRef context)`
///
/// Draws the frame's lines into the context. We draw our single line at the
/// origin, matching `CTLineDraw`.
///
/// Reference: <https://developer.apple.com/documentation/coretext/1509601-ctframedraw>
fn CTFrameDraw(env: &mut Environment, frame: CTFrameRef, context: id) {
    if frame.is_null() || context.is_null() {
        return;
    }
    let line = env.objc.borrow::<CTFrameHostObject>(frame).line;
    if line != nil {
        CTLineDraw(env, line, context);
    }
}

/// `CTFontRef CTFontCreateWithGraphicsFont(CGFontRef graphicsFont,
///     CGFloat size, const CGAffineTransform *matrix,
///     CTFontDescriptorRef attributes)`
///
/// Creates a CTFont from a CGFont. We wrap a system UIFont at the requested
/// size in a real `_touchHLE_CTFont` host object so subsequent metric queries
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
    let font_size = if size > 0.0 { size } else { 12.0 };
    // Keep the typeface the CGFont was made from. Apps load their own font
    // files this way (Real Racing 3 draws all its text and its icon glyphs
    // with bundled fonts); substituting the system font turned icons into
    // plain letters. A CGFont made from a font name is a UIFont here.
    let ui_font: id = if let Some(font) = data_provider_font(env, graphics_font) {
        new_uifont_with_custom_font(env, font, font_size)
    } else if is_uifont(env, graphics_font) {
        msg![env; graphics_font fontWithSize:font_size]
    } else {
        msg_class![env; UIFont systemFontOfSize:font_size]
    };
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
    export_c_func!(CTGetCoreTextVersion()),
    export_c_func!(CTFontCreateWithGraphicsFont(_, _, _, _)),
    export_c_func!(CTFontCreateWithName(_, _, _)),
    export_c_func!(CTFontCreateWithFontDescriptor(_, _, _)),
    export_c_func!(CTFontCreateCopyWithAttributes(_, _, _, _)),
    export_c_func!(CTFontDescriptorCreateWithAttributes(_)),
    export_c_func!(CTFontDescriptorCreateWithNameAndSize(_, _)),
    export_c_func!(CTFontDescriptorCreateMatchingFontDescriptors(_, _)),
    export_c_func!(CTFontDescriptorCreateMatchingFontDescriptor(_, _)),
    export_c_func!(CTFontDescriptorCopyAttribute(_, _)),
    export_c_func!(CTFontManagerRegisterGraphicsFont(_, _)),
    export_c_func!(CTFontGetAscent(_)),
    export_c_func!(CTFontGetDescent(_)),
    export_c_func!(CTFontGetLeading(_)),
    export_c_func!(CTFontGetSize(_)),
    export_c_func!(CTFontCopyPostScriptName(_)),
    export_c_func!(CTFontGetAdvancesForGlyphs(_, _, _, _, _)),
    export_c_func!(CTFontGetBoundingRectsForGlyphs(_, _, _, _, _)),
    export_c_func!(CTRunGetGlyphCount(_)),
    export_c_func!(CTRunGetAttributes(_)),
    export_c_func!(CTRunGetGlyphs(_, _, _)),
    export_c_func!(CTRunGetGlyphsPtr(_)),
    export_c_func!(CTRunGetPositionsPtr(_)),
    export_c_func!(CTRunGetPositions(_, _, _)),
    export_c_func!(CTParagraphStyleCreate(_, _)),
    export_c_func!(CTParagraphStyleCreateCopy(_)),
    export_c_func!(CTParagraphStyleGetValueForSpecifier(_, _, _, _)),
    export_c_func!(CTTypesetterCreateWithAttributedString(_)),
    export_c_func!(CTTypesetterCreateLine(_, _)),
    export_c_func!(CTTypesetterSuggestClusterBreak(_, _, _)),
    export_c_func!(CTLineCreateWithAttributedString(_)),
    export_c_func!(CTLineCreateTruncatedLine(_, _, _, _)),
    export_c_func!(CTLineGetTypographicBounds(_, _, _, _)),
    export_c_func!(CTLineGetImageBounds(_, _)),
    export_c_func!(CTLineDraw(_, _)),
    export_c_func!(CTLineGetGlyphCount(_)),
    export_c_func!(CTLineGetGlyphRuns(_)),
    export_c_func!(CTFontGetGlyphsForCharacters(_, _, _, _)),
    export_c_func!(CTFontCopyGraphicsFont(_, _)),
    export_c_func!(CTFramesetterCreateWithAttributedString(_)),
    export_c_func!(CTFramesetterSuggestFrameSizeWithConstraints(_, _, _, _, _)),
    export_c_func!(CTFramesetterCreateFrame(_, _, _, _)),
    export_c_func!(CTFrameGetLines(_)),
    export_c_func!(CTFrameGetLineOrigins(_, _, _)),
    export_c_func!(CTFrameGetStringRange(_)),
    export_c_func!(CTFrameGetVisibleStringRange(_)),
    export_c_func!(CTFrameDraw(_, _)),
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

#[cfg(test)]
mod tests {
    use super::system_font_file;

    #[test]
    fn font_names_map_to_bundled_files() {
        assert_eq!(system_font_file("Arial", ""), "Arial.ttf");
        assert_eq!(system_font_file("ArialMT", "Bold"), "LiberationSans-Bold.ttf");
        assert_eq!(system_font_file("Helvetica-Bold", ""), "helvetica-bold.ttf");
        assert_eq!(system_font_file("Helvetica", ""), "LiberationSans-Regular.ttf");
        assert_eq!(
            system_font_file("TimesNewRomanPS-ItalicMT", ""),
            "LiberationSerif-Italic.ttf"
        );
        assert_eq!(system_font_file("Courier New", "Bold Italic"), "LiberationMono-BoldItalic.ttf");
        assert_eq!(system_font_file("", ""), "LiberationSans-Regular.ttf");
    }
}
