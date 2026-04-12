/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `CGFont`.

use crate::dyld::{export_c_func, FunctionExports};
use crate::frameworks::core_foundation::cf_string::CFStringRef;
use crate::frameworks::core_foundation::CFTypeRef;
use crate::frameworks::foundation::ns_string;
use crate::mem::{MutVoidPtr, Ptr};
use crate::objc::{id, msg, msg_class, nil, retain, release};
use crate::Environment;

// =========================================================================
// MARK: - Type aliases
// =========================================================================

pub type CGFontRef = CFTypeRef;
pub type CGGlyph = u16;
pub type CGFontIndex = u16;
pub type CGFontPostScriptFormat = i32;

// CGFontPostScriptFormat constants
pub const kCGFontPostScriptFormatType1:   CGFontPostScriptFormat = 1;
pub const kCGFontPostScriptFormatType3:   CGFontPostScriptFormat = 3;
pub const kCGFontPostScriptFormatType42:  CGFontPostScriptFormat = 42;

// Special glyph index
pub const kCGFontIndexInvalid: CGFontIndex = 0xFFFF;
pub const kCGFontIndexMax:     CGFontIndex = 0xFFFE;
pub const kCGGlyphMax:         CGGlyph     = kCGFontIndexMax;

// =========================================================================
// MARK: - Internal host object
// We back CGFontRef with an NSFont/UIFont so we can reuse existing font
// infrastructure. The ref is just a retained id.
// =========================================================================

fn font_from_name(env: &mut Environment, name: CFStringRef) -> id {
    if name == nil {
        return nil;
    }
    // UIFont systemFontOfSize: as a fallback for unrecognised names.
    let size: f32 = 12.0;
    let font: id = msg_class![env; UIFont fontWithName:name size:size];
    if font != nil {
        return font;
    }
    msg_class![env; UIFont systemFontOfSize:size]
}

// =========================================================================
// MARK: - Creation
// =========================================================================

/// `CGFontRef CGFontCreateWithFontName(CFStringRef name)`
fn CGFontCreateWithFontName(env: &mut Environment, name: CFStringRef) -> CGFontRef {
    let font = font_from_name(env, name);
    if font == nil {
        log!("CGFontCreateWithFontName: could not create font for {:?}", name);
        return Ptr::null();
    }
    retain(env, font);
    log_dbg!(
        "CGFontCreateWithFontName({}) => {:?}",
        if name != nil { ns_string::to_rust_string(env, name).into_owned() } else { "(null)".into() },
        font
    );
    font
}

/// `CGFontRef CGFontCreateCopyWithVariations(CGFontRef font,
///                                           CFDictionaryRef variations)`
fn CGFontCreateCopyWithVariations(
    env: &mut Environment,
    font: CGFontRef,
    _variations: CFTypeRef,
) -> CGFontRef {
    // Variations (optical size axes, weight axes, etc.) are not supported.
    // Return a retained copy of the original font.
    log_dbg!("CGFontCreateCopyWithVariations — variations ignored, returning copy of original");
    if font.is_null() {
        return Ptr::null();
    }
    retain(env, font);
    font
}

/// `CGFontRef CGFontCreateWithDataProvider(CGDataProviderRef provider)`
fn CGFontCreateWithDataProvider(
    _env: &mut Environment,
    provider: CFTypeRef,
) -> CGFontRef {
    log!("TODO: CGFontCreateWithDataProvider({:?}) — returning NULL", provider);
    Ptr::null()
}

// =========================================================================
// MARK: - Retain / Release
// =========================================================================

fn CGFontRetain(env: &mut Environment, font: CGFontRef) -> CGFontRef {
    if font.is_null() { return Ptr::null(); }
    retain(env, font);
    font
}

fn CGFontRelease(env: &mut Environment, font: CGFontRef) {
    if font.is_null() { return; }
    release(env, font);
}

// =========================================================================
// MARK: - Name queries
// =========================================================================

/// `CFStringRef CGFontCopyPostScriptName(CGFontRef font)`
fn CGFontCopyPostScriptName(env: &mut Environment, font: CGFontRef) -> CFStringRef {
    if font.is_null() { return nil; }
    // UIFont -fontName returns the PostScript name on iOS.
    let name: id = msg![env; font fontName];
    if name == nil { return nil; }
    msg![env; name copy]
}

/// `CFStringRef CGFontCopyFullName(CGFontRef font)`
fn CGFontCopyFullName(env: &mut Environment, font: CGFontRef) -> CFStringRef {
    // Same as PostScript name in our implementation.
    CGFontCopyPostScriptName(env, font)
}

// =========================================================================
// MARK: - Metrics
// =========================================================================

/// `int CGFontGetUnitsPerEm(CGFontRef font)`
fn CGFontGetUnitsPerEm(_env: &mut Environment, font: CGFontRef) -> i32 {
    if font.is_null() { return 0; }
    // Standard TrueType/OpenType value.
    2048
}

/// `int CGFontGetAscent(CGFontRef font)`
fn CGFontGetAscent(env: &mut Environment, font: CGFontRef) -> i32 {
    if font.is_null() { return 0; }
    let ascender: f32 = msg![env; font ascender];
    let upm = CGFontGetUnitsPerEm(env, font) as f32;
    let point_size: f32 = msg![env; font pointSize];
    if point_size == 0.0 { return 0; }
    (ascender / point_size * upm).round() as i32
}

/// `int CGFontGetDescent(CGFontRef font)`
fn CGFontGetDescent(env: &mut Environment, font: CGFontRef) -> i32 {
    if font.is_null() { return 0; }
    let descender: f32 = msg![env; font descender];
    let upm = CGFontGetUnitsPerEm(env, font) as f32;
    let point_size: f32 = msg![env; font pointSize];
    if point_size == 0.0 { return 0; }
    (descender / point_size * upm).round() as i32
}

/// `int CGFontGetLeading(CGFontRef font)`
fn CGFontGetLeading(env: &mut Environment, font: CGFontRef) -> i32 {
    if font.is_null() { return 0; }
    let leading: f32 = msg![env; font leading];
    let upm = CGFontGetUnitsPerEm(env, font) as f32;
    let point_size: f32 = msg![env; font pointSize];
    if point_size == 0.0 { return 0; }
    (leading / point_size * upm).round() as i32
}

/// `int CGFontGetCapHeight(CGFontRef font)`
fn CGFontGetCapHeight(env: &mut Environment, font: CGFontRef) -> i32 {
    if font.is_null() { return 0; }
    // Approximate cap height as 70% of ascent in design units.
    (CGFontGetAscent(env, font) as f32 * 0.70).round() as i32
}

/// `int CGFontGetXHeight(CGFontRef font)`
fn CGFontGetXHeight(env: &mut Environment, font: CGFontRef) -> i32 {
    if font.is_null() { return 0; }
    // Approximate x-height as 50% of ascent in design units.
    (CGFontGetAscent(env, font) as f32 * 0.50).round() as i32
}

/// `CGRect CGFontGetFontBBox(CGFontRef font)`
///
/// Returns the bounding box in design-unit space as four floats written to a
/// caller-supplied pointer. Since CGRect is a struct return (not natively
/// handled here), we accept a MutVoidPtr output buffer.
fn CGFontGetFontBBox(
    env: &mut Environment,
    font: CGFontRef,
    out: MutVoidPtr, // CGRect* in design units
) {
    use crate::frameworks::core_graphics::{CGPoint, CGRect, CGSize};
    if font.is_null() || out.is_null() { return; }
    let upm = CGFontGetUnitsPerEm(env, font) as f32;
    let asc  =  CGFontGetAscent(env, font)  as f32;
    let desc =  CGFontGetDescent(env, font) as f32; // negative
    let rect = CGRect {
        origin: CGPoint { x: 0.0, y: desc },
        size:   CGSize  { width: upm, height: asc - desc },
    };
    let p: crate::mem::MutPtr<CGRect> = out.cast();
    env.mem.write(p, rect);
}

// =========================================================================
// MARK: - Glyph advances (stub)
// =========================================================================

/// `bool CGFontGetGlyphAdvances(font, glyphs, count, advances)`
///
/// Writes `count` integer advance widths (in design units) for the given
/// glyph array. We write a fixed advance proportional to the em size as a
/// stub — proper per-glyph metrics would require a full font renderer.
fn CGFontGetGlyphAdvances(
    env: &mut Environment,
    font: CGFontRef,
    glyphs: crate::mem::ConstPtr<CGGlyph>,
    count: u32,
    advances: crate::mem::MutPtr<i32>,
) -> bool {
    if font.is_null() { return false; }
    let upm = CGFontGetUnitsPerEm(env, font);
    let avg_advance = (upm as f32 * 0.60).round() as i32;
    for i in 0..count {
        let _glyph: CGGlyph = env.mem.read(glyphs + i);
        env.mem.write(advances + i, avg_advance);
    }
    log_dbg!("CGFontGetGlyphAdvances: wrote {} stub advances", count);
    true
}

/// `bool CGFontGetGlyphBBoxes(font, glyphs, count, bboxes)`
///
/// Writes `count` bounding boxes (CGRect, in design units). Stub: writes the
/// full font bounding box for every glyph.
fn CGFontGetGlyphBBoxes(
    env: &mut Environment,
    font: CGFontRef,
    _glyphs: crate::mem::ConstPtr<CGGlyph>,
    count: u32,
    bboxes: crate::mem::MutVoidPtr,
) -> bool {
    use crate::frameworks::core_graphics::{CGPoint, CGRect, CGSize};
    if font.is_null() { return false; }
    let upm = CGFontGetUnitsPerEm(env, font) as f32;
    let asc  = CGFontGetAscent(env, font)  as f32;
    let desc = CGFontGetDescent(env, font) as f32;
    let rect = CGRect {
        origin: CGPoint { x: 0.0, y: desc },
        size:   CGSize  { width: upm * 0.60, height: asc - desc },
    };
    let p: crate::mem::MutPtr<CGRect> = bboxes.cast();
    for i in 0..count {
        env.mem.write(p + i, rect);
    }
    true
}

// =========================================================================
// MARK: - Glyph name lookup (stub)
// =========================================================================

/// `CGGlyph CGFontGetGlyphWithGlyphName(font, glyphName)`
fn CGFontGetGlyphWithGlyphName(
    env: &mut Environment,
    font: CGFontRef,
    glyph_name: CFStringRef,
) -> CGGlyph {
    log_dbg!(
        "CGFontGetGlyphWithGlyphName({:?}, {:?}) — returning kCGFontIndexInvalid",
        font, glyph_name
    );
    kCGFontIndexInvalid
}

/// `CFStringRef CGFontCopyGlyphNameForGlyph(font, glyph)`
fn CGFontCopyGlyphNameForGlyph(
    _env: &mut Environment,
    font: CGFontRef,
    glyph: CGGlyph,
) -> CFStringRef {
    log_dbg!(
        "CGFontCopyGlyphNameForGlyph({:?}, {}) — returning nil",
        font, glyph
    );
    nil
}

// =========================================================================
// MARK: - Table access (stub)
// =========================================================================

/// `CFArrayRef CGFontCopyTableTags(CGFontRef font)`
fn CGFontCopyTableTags(_env: &mut Environment, font: CGFontRef) -> CFTypeRef {
    log_dbg!("CGFontCopyTableTags({:?}) — returning nil", font);
    nil
}

/// `CFDataRef CGFontCopyTableForTag(CGFontRef font, uint32_t tag)`
fn CGFontCopyTableForTag(
    _env: &mut Environment,
    font: CGFontRef,
    tag: u32,
) -> CFTypeRef {
    log_dbg!("CGFontCopyTableForTag({:?}, {:#010x}) — returning nil", font, tag);
    nil
}

// =========================================================================
// MARK: - PostScript data (stub)
// =========================================================================

fn CGFontCanCreatePostScriptSubset(
    _env: &mut Environment,
    font: CGFontRef,
    format: CGFontPostScriptFormat,
) -> bool {
    log_dbg!(
        "CGFontCanCreatePostScriptSubset({:?}, {}) — returning false",
        font, format
    );
    false
}

fn CGFontCreatePostScriptSubset(
    _env: &mut Environment,
    font: CGFontRef,
    subset_name: CFStringRef,
    format: CGFontPostScriptFormat,
    _glyphs: crate::mem::ConstPtr<CGGlyph>,
    _count: u32,
    _encoding: crate::mem::ConstPtr<CGGlyph>,
) -> CFTypeRef {
    log!(
        "TODO: CGFontCreatePostScriptSubset({:?}, {:?}, {}) — returning nil",
        font, subset_name, format
    );
    nil
}

fn CGFontCreatePostScriptEncoding(
    _env: &mut Environment,
    font: CGFontRef,
    _encoding: crate::mem::ConstPtr<CGGlyph>,
) -> CFTypeRef {
    log!("TODO: CGFontCreatePostScriptEncoding({:?}) — returning nil", font);
    nil
}

// =========================================================================
// MARK: - Variations (stub)
// =========================================================================

fn CGFontCopyVariationAxes(_env: &mut Environment, font: CGFontRef) -> CFTypeRef {
    log_dbg!("CGFontCopyVariationAxes({:?}) — returning nil", font);
    nil
}

fn CGFontCopyVariations(_env: &mut Environment, font: CGFontRef) -> CFTypeRef {
    log_dbg!("CGFontCopyVariations({:?}) — returning nil", font);
    nil
}

pub const FUNCTIONS: FunctionExports = &[
    // Creation
    export_c_func!(CGFontCreateWithFontName(_)),
    export_c_func!(CGFontCreateCopyWithVariations(_, _)),
    export_c_func!(CGFontCreateWithDataProvider(_)),
    // Retain / Release
    export_c_func!(CGFontRetain(_)),
    export_c_func!(CGFontRelease(_)),
    // Names
    export_c_func!(CGFontCopyPostScriptName(_)),
    export_c_func!(CGFontCopyFullName(_)),
    // Metrics
    export_c_func!(CGFontGetUnitsPerEm(_)),
    export_c_func!(CGFontGetAscent(_)),
    export_c_func!(CGFontGetDescent(_)),
    export_c_func!(CGFontGetLeading(_)),
    export_c_func!(CGFontGetCapHeight(_)),
    export_c_func!(CGFontGetXHeight(_)),
    export_c_func!(CGFontGetFontBBox(_, _)),
    // Glyph advances / bboxes
    export_c_func!(CGFontGetGlyphAdvances(_, _, _, _)),
    export_c_func!(CGFontGetGlyphBBoxes(_, _, _, _)),
    // Glyph name lookup
    export_c_func!(CGFontGetGlyphWithGlyphName(_, _)),
    export_c_func!(CGFontCopyGlyphNameForGlyph(_, _)),
    // Table access
    export_c_func!(CGFontCopyTableTags(_)),
    export_c_func!(CGFontCopyTableForTag(_, _)),
    // PostScript
    export_c_func!(CGFontCanCreatePostScriptSubset(_, _)),
    export_c_func!(CGFontCreatePostScriptSubset(_, _, _, _, _, _)),
    export_c_func!(CGFontCreatePostScriptEncoding(_, _)),
    // Variations
    export_c_func!(CGFontCopyVariationAxes(_)),
    export_c_func!(CGFontCopyVariations(_)),
];
