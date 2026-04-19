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

fn CGImageCreate(
    env: &mut Environment,
    width: GuestUSize,
    height: GuestUSize,
    bits_per_component: GuestUSize,
    bits_per_pixel: GuestUSize,
    bytes_per_row: GuestUSize,
    _color_space: CGColorSpaceRef,
    bitmap_info: CGBitmapInfo,
    provider: CGDataProviderRef,
    decode: ConstPtr<CGFloat>,
    _should_interpolate: bool,
    _intent: i32,
) -> CGImageRef {
    if provider.is_null() { return nil; }

    let bytes = cg_data_provider::borrow_bytes(env, provider);

    // Try to decode as a standard image format first (PNG / JPEG).
    if let Ok(image) = Image::from_bytes(bytes) {
        return from_image(env, image);
    }

    // Fall back: treat as raw RGBA / BGRA bitmap data.
    if bits_per_pixel == 32 && bytes_per_row >= width * 4 {
        let alpha_info = bitmap_info & kCGBitmapAlphaInfoMask;
        let byte_order = bitmap_info & kCGBitmapByteOrderMask;

        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for row in 0..height {
            let row_start = (row * bytes_per_row) as usize;
            for col in 0..width as usize {
                let p = row_start + col * 4;
                if p + 4 > bytes.len() { break; }
                let (r, g, b, a) = match alpha_info {
                    // BGRA / BGRX
                    x if x == kCGImageAlphaNoneSkipFirst
                        || x == kCGImageAlphaPremultipliedFirst
                        || x == kCGImageAlphaFirst => {
                        (bytes[p+1], bytes[p+2], bytes[p+3], bytes[p])
                    }
                    _ => (bytes[p], bytes[p+1], bytes[p+2], bytes[p+3]),
                };
                rgba.extend_from_slice(&[r, g, b, a]);
            }
        }
        let image = Image::from_pixels(width, height, rgba);
        return from_image(env, image);
    }

    // Grayscale 8-bit
    if bits_per_pixel == 8 && bytes_per_row >= width {
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for row in 0..height {
            let row_start = (row * bytes_per_row) as usize;
            for col in 0..width as usize {
                let p = row_start + col;
                if p >= bytes.len() { break; }
                let v = bytes[p];
                rgba.extend_from_slice(&[v, v, v, 255]);
            }
        }
        let image = Image::from_pixels(width, height, rgba);
        return from_image(env, image);
    }

    log!(
        "CGImageCreate: unsupported format bitsPerPixel={} bytesPerRow={}, returning nil",
        bits_per_pixel, bytes_per_row
    );
    nil
}

// MARK: - CGImageMaskCreate

fn CGImageMaskCreate(
    env: &mut Environment,
    width: GuestUSize,
    height: GuestUSize,
    _bits_per_component: GuestUSize,
    _bits_per_pixel: GuestUSize,
    bytes_per_row: GuestUSize,
    provider: CGDataProviderRef,
    _decode: ConstPtr<CGFloat>,
    _should_interpolate: bool,
) -> CGImageRef {
    if provider.is_null() { return nil; }
    let bytes = cg_data_provider::borrow_bytes(env, provider);

    // Build a greyscale→alpha mask image.
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for row in 0..height {
        let row_start = (row * bytes_per_row) as usize;
        for col in 0..width as usize {
            let p = row_start + col;
            let a = if p < bytes.len() { 255 - bytes[p] } else { 0 };
            rgba.extend_from_slice(&[0, 0, 0, a]);
        }
    }
    let image = Image::from_pixels(width, height, rgba);
    from_image(env, image)
}

// MARK: - CGImageCreateWithDataProvider (generic)

fn CGImageCreateWithDataProvider(
    env: &mut Environment,
    provider: CGDataProviderRef,
    decode: ConstPtr<CGFloat>,
    should_interpolate: bool,
    intent: i32,
    _color_space: CGColorSpaceRef,
    bitmap_info: CGBitmapInfo,
    bits_per_component: GuestUSize,
    bits_per_pixel: GuestUSize,
    width: GuestUSize,
    bytes_per_row: GuestUSize,
    height: GuestUSize,
) -> CGImageRef {
    // Delegate to CGImageCreate with the arguments re-ordered.
    CGImageCreate(
        env,
        width, height,
        bits_per_component, bits_per_pixel, bytes_per_row,
        nil, bitmap_info,
        provider, decode,
        should_interpolate, intent,
    )
}

// MARK: - Additional accessors

fn CGImageGetNumberOfComponents(_env: &mut Environment, image: CGImageRef) -> GuestUSize {
    if image.is_null() { return 0; }
    4 // RGBA
}

// Returns a CFString UTType for the image — always PNG for our purposes.
fn CGImageGetUTType(env: &mut Environment, image: CGImageRef) -> crate::objc::id {
    if image.is_null() { return nil; }
    // "public.png" — caller must not release (returns borrowed string).
    let s = ns_string::get_static_str(env, "public.png");
    s
}

// CGImagePixelFormatInfo — 0 = packed (standard).
fn CGImageGetPixelFormatInfo(_env: &mut Environment, image: CGImageRef) -> u32 {
    if image.is_null() { return 0; }
    0 // kCGImagePixelFormatPacked
}

// MARK: - CGImageCreateCopyByMaskingColors

// Mask out pixels whose components fall within `[min[i], max[i]]`.
// Components are expected as a flat array: `[r_min, r_max, g_min, g_max, b_min, b_max]`.
fn CGImageCreateCopyByMaskingColors(
    env: &mut Environment,
    image: CGImageRef,
    color_components: ConstPtr<CGFloat>,
    _num_components: GuestUSize,
) -> CGImageRef {
    if image.is_null() { return nil; }

    let (w, h) = env.objc.borrow::<CGImageHostObject>(image).image.dimensions();
    let src_pixels = env.objc.borrow::<CGImageHostObject>(image).image.pixels().to_vec();

    // Read min/max pairs — 3 pairs for RGB.
    let r_min = (env.mem.read(color_components)     * 255.0) as u8;
    let r_max = (env.mem.read(color_components + 1) * 255.0) as u8;
    let g_min = (env.mem.read(color_components + 2) * 255.0) as u8;
    let g_max = (env.mem.read(color_components + 3) * 255.0) as u8;
    let b_min = (env.mem.read(color_components + 4) * 255.0) as u8;
    let b_max = (env.mem.read(color_components + 5) * 255.0) as u8;

    let mut dst = src_pixels.clone();
    for i in (0..dst.len()).step_by(4) {
        let r = dst[i]; let g = dst[i+1]; let b = dst[i+2];
        if r >= r_min && r <= r_max
            && g >= g_min && g <= g_max
            && b >= b_min && b <= b_max
        {
            dst[i+3] = 0; // mask out — set alpha to 0
        }
    }

    let new_image = Image::from_pixels(w, h, dst);
    from_image(env, new_image)
}

// MARK: - CGImageFlipVertically / rotation

// Returns a vertically flipped copy of the image.
fn CGImageFlipVertically(env: &mut Environment, image: CGImageRef) -> CGImageRef {
    if image.is_null() { return nil; }
    let (w, h) = env.objc.borrow::<CGImageHostObject>(image).image.dimensions();
    let src = env.objc.borrow::<CGImageHostObject>(image).image.pixels().to_vec();
    let row_bytes = w as usize * 4;
    let mut dst = vec![0u8; src.len()];
    for row in 0..h as usize {
        let src_row = &src[(h as usize - 1 - row) * row_bytes..][..row_bytes];
        let dst_row = &mut dst[row * row_bytes..][..row_bytes];
        dst_row.copy_from_slice(src_row);
    }
    let new_image = Image::from_pixels(w, h, dst);
    from_image(env, new_image)
}

// Rotate image by multiples of 90 degrees.
// `angle_degrees` should be 0, 90, 180, or 270.
fn CGImageCreateWithRotation(
    env: &mut Environment,
    image: CGImageRef,
    angle_degrees: i32,
) -> CGImageRef {
    if image.is_null() { return nil; }
    let normalized = ((angle_degrees % 360) + 360) % 360;
    if normalized == 0 {
        return CGImageCreateCopy(env, image);
    }
    let (w, h) = env.objc.borrow::<CGImageHostObject>(image).image.dimensions();
    let src = env.objc.borrow::<CGImageHostObject>(image).image.pixels().to_vec();

    let (new_w, new_h, dst) = match normalized {
        90 => {
            // 90° clockwise: (x, y) → (h-1-y, x)
            let mut dst = vec![0u8; src.len()];
            for y in 0..h as usize {
                for x in 0..w as usize {
                    let src_off = (y * w as usize + x) * 4;
                    let dst_x = h as usize - 1 - y;
                    let dst_y = x;
                    let dst_off = (dst_y * h as usize + dst_x) * 4;
                    dst[dst_off..dst_off+4].copy_from_slice(&src[src_off..src_off+4]);
                }
            }
            (h, w, dst)
        }
        180 => {
            let mut dst = vec![0u8; src.len()];
            let total = (w * h) as usize;
            for i in 0..total {
                let src_off = i * 4;
                let dst_off = (total - 1 - i) * 4;
                dst[dst_off..dst_off+4].copy_from_slice(&src[src_off..src_off+4]);
            }
            (w, h, dst)
        }
        270 => {
            // 270° clockwise = 90° counter-clockwise: (x, y) → (y, w-1-x)
            let mut dst = vec![0u8; src.len()];
            for y in 0..h as usize {
                for x in 0..w as usize {
                    let src_off = (y * w as usize + x) * 4;
                    let dst_x = y;
                    let dst_y = w as usize - 1 - x;
                    let dst_off = (dst_y * w as usize + dst_x) * 4;
                    dst[dst_off..dst_off+4].copy_from_slice(&src[src_off..src_off+4]);
                }
            }
            (h, w, dst)
        }
        _ => return CGImageCreateCopy(env, image),
    };
    let new_image = Image::from_pixels(new_w, new_h, dst);
    from_image(env, new_image)
}

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
    export_c_func!(CGImageCreate(_, _, _, _, _, _, _, _, _, _, _, _)),
    export_c_func!(CGImageMaskCreate(_, _, _, _, _, _, _)),
    export_c_func!(CGImageCreateWithDataProvider(_, _, _, _, _, _, _, _, _, _)),
    export_c_func!(CGImageGetNumberOfComponents(_)),
    export_c_func!(CGImageGetUTType(_)),
    export_c_func!(CGImageGetPixelFormatInfo(_)),
    export_c_func!(CGImageCreateCopyByMaskingColors(_, _, _)),
    export_c_func!(CGImageFlipVertically(_)),
    export_c_func!(CGImageCreateWithRotation(_, _)),
];
