/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `CGImage.h`

use super::cg_color_space::{
    kCGColorSpaceGenericRGB, CGColorSpaceCreateWithName, CGColorSpaceGetModel, CGColorSpaceRef,
};
use super::cg_data_provider::{self, CGDataProviderRef};
use super::CGFloat;
use crate::dyld::{export_c_func, FunctionExports};
use crate::frameworks::core_foundation::{CFRelease, CFRetain, CFTypeRef};
use crate::frameworks::foundation::ns_string;
use crate::image::Image;
use crate::mem::{ConstPtr, GuestUSize, MutPtr};
use crate::objc::{autorelease, id, nil, objc_classes, ClassExports, HostObject, ObjC};
use crate::Environment;

// =========================================================================
// MARK: - CGImageAlphaInfo
// =========================================================================

pub type CGImageAlphaInfo = u32;
pub const kCGImageAlphaNone:             CGImageAlphaInfo = 0;
pub const kCGImageAlphaPremultipliedLast: CGImageAlphaInfo = 1;
pub const kCGImageAlphaPremultipliedFirst:CGImageAlphaInfo = 2;
pub const kCGImageAlphaLast:             CGImageAlphaInfo = 3;
pub const kCGImageAlphaFirst:            CGImageAlphaInfo = 4;
pub const kCGImageAlphaNoneSkipLast:     CGImageAlphaInfo = 5;
pub const kCGImageAlphaNoneSkipFirst:    CGImageAlphaInfo = 6;
pub const kCGImageAlphaOnly:             CGImageAlphaInfo = 7;

// =========================================================================
// MARK: - CGImageByteOrderInfo
// =========================================================================

pub type CGImageByteOrderInfo = u32;
pub const kCGImageByteOrderMask:    CGImageByteOrderInfo = 0x7000;
pub const kCGImageByteOrderDefault: CGImageByteOrderInfo = 0 << 12;
#[allow(dead_code)]
pub const kCGImageByteOrder16Little: CGImageByteOrderInfo = 1 << 12;
#[allow(dead_code)]
pub const kCGImageByteOrder32Little: CGImageByteOrderInfo = 2 << 12;
#[allow(dead_code)]
pub const kCGImageByteOrder16Big: CGImageByteOrderInfo = 3 << 12;
pub const kCGImageByteOrder32Big: CGImageByteOrderInfo = 4 << 12;

// =========================================================================
// MARK: - CGBitmapInfo
// =========================================================================

pub type CGBitmapInfo = u32;
pub const kCGBitmapAlphaInfoMask: CGBitmapInfo = 0x1F;
pub const kCGBitmapByteOrderMask: CGBitmapInfo = kCGImageByteOrderMask;
pub const kCGBitmapFloatComponents: CGBitmapInfo = 1 << 8;
pub const kCGBitmapByteOrder16Host: CGBitmapInfo = kCGImageByteOrder16Little;
pub const kCGBitmapByteOrder32Host: CGBitmapInfo = kCGImageByteOrder32Little;

// =========================================================================
// MARK: - CGImagePixelFormatInfo (iOS 12+)
// =========================================================================

pub type CGImagePixelFormatInfo = u32;
pub const kCGImagePixelFormatPacked:      CGImagePixelFormatInfo = 0;
pub const kCGImagePixelFormatRGB555:      CGImagePixelFormatInfo = 1 << 16;
pub const kCGImagePixelFormatRGB565:      CGImagePixelFormatInfo = 2 << 16;
pub const kCGImagePixelFormatRGB101010:   CGImagePixelFormatInfo = 3 << 16;
pub const kCGImagePixelFormatYCbCr422:    CGImagePixelFormatInfo = 4 << 16;

// =========================================================================
// MARK: - _touchHLE_CGImage ObjC class
// =========================================================================

pub const CLASSES: ClassExports = objc_classes! {
(env, this, _cmd);

@implementation _touchHLE_CGImage: NSObject

- (id)copyWithZone:(id)_zone {
    // CGImage is conceptually immutable — retain on copy.
    crate::objc::retain(env, this);
    this
}

@end
};

// =========================================================================
// MARK: - Host object
// =========================================================================

struct CGImageHostObject {
    image: Image,
}
impl HostObject for CGImageHostObject {}

pub type CGImageRef = CFTypeRef;

// =========================================================================
// MARK: - Retain / Release
// =========================================================================

pub fn CGImageRelease(env: &mut Environment, c: CGImageRef) {
    if !c.is_null() {
        CFRelease(env, c);
    }
}

pub fn CGImageRetain(env: &mut Environment, c: CGImageRef) -> CGImageRef {
    if !c.is_null() {
        CFRetain(env, c)
    } else {
        c
    }
}

// =========================================================================
// MARK: - Internal helpers
// =========================================================================

pub fn from_image(env: &mut Environment, image: Image) -> CGImageRef {
    let host_obj = Box::new(CGImageHostObject { image });
    let class = env.objc.get_known_class("_touchHLE_CGImage", &mut env.mem);
    env.objc.alloc_object(class, host_obj, &mut env.mem)
}

pub fn borrow_image(objc: &ObjC, image: CGImageRef) -> &Image {
    &objc.borrow::<CGImageHostObject>(image).image
}

pub fn borrow_image_mut(objc: &mut ObjC, image: CGImageRef) -> &mut Image {
    &mut objc.borrow_mut::<CGImageHostObject>(image).image
}

// =========================================================================
// MARK: - Creation functions
// =========================================================================

fn CGImageCreateCopyWithColorSpace(
    env: &mut Environment,
    image: CGImageRef,
    color_space: CGColorSpaceRef,
) -> CGImageRef {
    if image.is_null() { return nil; }
    let image_cs = CGImageGetColorSpace(env, image);
    if image_cs.is_null() { return nil; }
    // Only assert model match if both color spaces are non-null.
    if !color_space.is_null() {
        assert_eq!(
            CGColorSpaceGetModel(env, image_cs),
            CGColorSpaceGetModel(env, color_space)
        );
    }
    let new_image = env.objc.borrow::<CGImageHostObject>(image).image.clone();
    from_image(env, new_image)
}

fn CGImageCreateWithPNGDataProvider(
    env: &mut Environment,
    source: CGDataProviderRef,
    decode: ConstPtr<CGFloat>,
    _should_interpolate: bool,
    _intent: i32,
) -> CGImageRef {
    if source.is_null() { return nil; }
    assert!(decode.is_null());
    let bytes = cg_data_provider::borrow_bytes(env, source);
    match Image::from_bytes(bytes) {
        Ok(image) => from_image(env, image),
        Err(_) => {
            log!("CGImageCreateWithPNGDataProvider: failed to decode image");
            nil
        }
    }
}

fn CGImageCreateWithJPEGDataProvider(
    env: &mut Environment,
    source: CGDataProviderRef,
    decode: ConstPtr<CGFloat>,
    _should_interpolate: bool,
    _intent: i32,
) -> CGImageRef {
    if source.is_null() { return nil; }
    assert!(decode.is_null());
    let bytes = cg_data_provider::borrow_bytes(env, source);
    match Image::from_bytes(bytes) {
        Ok(image) => from_image(env, image),
        Err(_) => {
            log!("CGImageCreateWithJPEGDataProvider: failed to decode image");
            nil
        }
    }
}

fn CGImageCreateCopy(env: &mut Environment, image: CGImageRef) -> CGImageRef {
    if image.is_null() { return nil; }
    let new_image = env.objc.borrow::<CGImageHostObject>(image).image.clone();
    from_image(env, new_image)
}

fn CGImageCreateWithImageInRect(
    env: &mut Environment,
    image: CGImageRef,
    rect: super::CGRect,
) -> CGImageRef {
    if image.is_null() { return nil; }

    let (img_w, img_h) = env
        .objc
        .borrow::<CGImageHostObject>(image)
        .image
        .dimensions();

    // Clamp rect to image bounds.
    let x      = (rect.origin.x as u32).min(img_w);
    let y      = (rect.origin.y as u32).min(img_h);
    let width  = (rect.size.width  as u32).min(img_w.saturating_sub(x));
    let height = (rect.size.height as u32).min(img_h.saturating_sub(y));

    if width == 0 || height == 0 {
        log_dbg!("CGImageCreateWithImageInRect: empty rect, returning nil");
        return nil;
    }

    let src_pixels = env
        .objc
        .borrow::<CGImageHostObject>(image)
        .image
        .pixels();

    // Copy sub-region row by row (RGBA — 4 bytes per pixel).
    let mut dst = vec![0u8; (width * height * 4) as usize];
    for row in 0..height {
        let src_start = ((y + row) * img_w + x) as usize * 4;
        let dst_start = (row * width) as usize * 4;
        dst[dst_start..dst_start + width as usize * 4]
            .copy_from_slice(&src_pixels[src_start..src_start + width as usize * 4]);
    }

    let new_image = Image::from_pixels(width, height, dst);
    from_image(env, new_image)
}

fn CGImageCreateWithMask(
    env: &mut Environment,
    image: CGImageRef,
    _mask: CGImageRef,
) -> CGImageRef {
    log_dbg!("CGImageCreateWithMask: mask not applied (stub) — returning copy");
    CGImageCreateCopy(env, image)
}

fn CGImageCreateMaskWithImageMask(env: &mut Environment, mask_image: CGImageRef) -> CGImageRef {
    log_dbg!("CGImageCreateMaskWithImageMask: stub — returning copy");
    CGImageCreateCopy(env, mask_image)
}

/// `CGImageCreateWithBitmapContext` — create a CGImage from raw RGBA bytes.
pub fn CGImageCreateWithRawPixels(
    env: &mut Environment,
    width: GuestUSize,
    height: GuestUSize,
    pixels: Vec<u8>,
) -> CGImageRef {
    let image = Image::from_pixels(width, height, pixels);
    from_image(env, image)
}

// =========================================================================
// MARK: - Accessors
// =========================================================================

fn CGImageGetAlphaInfo(_env: &mut Environment, image: CGImageRef) -> CGImageAlphaInfo {
    if image.is_null() { return kCGImageAlphaNone; }
    kCGImageAlphaPremultipliedLast
}

fn CGImageGetBitmapInfo(_env: &mut Environment, image: CGImageRef) -> CGBitmapInfo {
    if image.is_null() { return 0; }
    kCGImageAlphaPremultipliedLast | kCGImageByteOrder32Big
}

fn CGImageGetPixelFormatInfo(_env: &mut Environment, image: CGImageRef) -> CGImagePixelFormatInfo {
    if image.is_null() { return 0; }
    kCGImagePixelFormatPacked
}

fn CGImageGetColorSpace(env: &mut Environment, image: CGImageRef) -> CGColorSpaceRef {
    if image.is_null() { return nil; }
    let srgb_name = ns_string::get_static_str(env, kCGColorSpaceGenericRGB);
    CGColorSpaceCreateWithName(env, srgb_name)
}

pub fn CGImageGetWidth(env: &mut Environment, image: CGImageRef) -> GuestUSize {
    if image.is_null() { return 0; }
    env.objc.borrow::<CGImageHostObject>(image).image.dimensions().0
}

pub fn CGImageGetHeight(env: &mut Environment, image: CGImageRef) -> GuestUSize {
    if image.is_null() { return 0; }
    env.objc.borrow::<CGImageHostObject>(image).image.dimensions().1
}

fn CGImageGetBitsPerPixel(_env: &mut Environment, image: CGImageRef) -> GuestUSize {
    if image.is_null() { return 0; }
    32
}

fn CGImageGetBitsPerComponent(_env: &mut Environment, image: CGImageRef) -> GuestUSize {
    if image.is_null() { return 0; }
    8
}

fn CGImageGetBytesPerRow(env: &mut Environment, image: CGImageRef) -> GuestUSize {
    if image.is_null() { return 0; }
    env.objc.borrow::<CGImageHostObject>(image).image.dimensions().0 * 4
}

fn CGImageGetDataProvider(env: &mut Environment, image: CGImageRef) -> CGDataProviderRef {
    if image.is_null() { return nil; }
    let provider = cg_data_provider::from_cg_image(env, image);
    autorelease(env, provider)
}

fn CGImageGetDecode(_env: &mut Environment, _image: CGImageRef) -> ConstPtr<CGFloat> {
    // Always returns NULL — we use the default identity decode array.
    ConstPtr::null()
}

fn CGImageGetShouldInterpolate(_env: &mut Environment, image: CGImageRef) -> bool {
    if image.is_null() { return false; }
    true
}

fn CGImageGetRenderingIntent(_env: &mut Environment, _image: CGImageRef) -> i32 {
    // kCGRenderingIntentDefault = 0
    0
}

fn CGImageIsMask(_env: &mut Environment, _image: CGImageRef) -> bool {
    false
}

fn CGImageGetUTType(_env: &mut Environment, _image: CGImageRef) -> CFTypeRef {
    // Return nil — we don't track the original file type.
    nil
}

// =========================================================================
// MARK: - FUNCTIONS
// =========================================================================

pub const FUNCTIONS: FunctionExports = &[
    // Retain / release
    export_c_func!(CGImageRelease(_)),
    export_c_func!(CGImageRetain(_)),
    // Creation
    export_c_func!(CGImageCreateCopyWithColorSpace(_, _)),
    export_c_func!(CGImageCreateWithPNGDataProvider(_, _, _, _)),
    export_c_func!(CGImageCreateWithJPEGDataProvider(_, _, _, _)),
    export_c_func!(CGImageCreateWithImageInRect(_, _)),
    export_c_func!(CGImageCreateWithMask(_, _)),
    export_c_func!(CGImageCreateMaskWithImageMask(_)),
    export_c_func!(CGImageCreateCopy(_)),
    // Accessors
    export_c_func!(CGImageGetAlphaInfo(_)),
    export_c_func!(CGImageGetBitmapInfo(_)),
    export_c_func!(CGImageGetPixelFormatInfo(_)),
    export_c_func!(CGImageGetColorSpace(_)),
    export_c_func!(CGImageGetWidth(_)),
    export_c_func!(CGImageGetHeight(_)),
    export_c_func!(CGImageGetBitsPerPixel(_)),
    export_c_func!(CGImageGetBitsPerComponent(_)),
    export_c_func!(CGImageGetBytesPerRow(_)),
    export_c_func!(CGImageGetDataProvider(_)),
    export_c_func!(CGImageGetDecode(_)),
    export_c_func!(CGImageGetShouldInterpolate(_)),
    export_c_func!(CGImageGetRenderingIntent(_)),
    export_c_func!(CGImageIsMask(_)),
    export_c_func!(CGImageGetUTType(_)),
];
