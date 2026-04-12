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
use crate::objc::{autorelease, nil, objc_classes, ClassExports, HostObject, ObjC};
use crate::Environment;

pub type CGImageAlphaInfo = u32;
pub const kCGImageAlphaNone: CGImageAlphaInfo = 0;
pub const kCGImageAlphaPremultipliedLast: CGImageAlphaInfo = 1;
pub const kCGImageAlphaPremultipliedFirst: CGImageAlphaInfo = 2;
pub const kCGImageAlphaLast: CGImageAlphaInfo = 3;
pub const kCGImageAlphaFirst: CGImageAlphaInfo = 4;
pub const kCGImageAlphaNoneSkipLast: CGImageAlphaInfo = 5;
pub const kCGImageAlphaNoneSkipFirst: CGImageAlphaInfo = 6;
pub const kCGImageAlphaOnly: CGImageAlphaInfo = 7;

pub type CGImageByteOrderInfo = u32;
pub const kCGImageByteOrderMask: CGImageByteOrderInfo = 0x7000;
pub const kCGImageByteOrderDefault: CGImageByteOrderInfo = 0 << 12;
#[allow(dead_code)]
pub const kCGImageByteOrder16Little: CGImageByteOrderInfo = 1 << 12;
#[allow(dead_code)]
pub const kCGImageByteOrder32Little: CGImageByteOrderInfo = 2 << 12;
#[allow(dead_code)]
pub const kCGImageByteOrder16Big: CGImageByteOrderInfo = 3 << 12;
pub const kCGImageByteOrder32Big: CGImageByteOrderInfo = 4 << 12;

pub type CGBitmapInfo = u32;
pub const kCGBitmapAlphaInfoMask: CGBitmapInfo = 0x1F;
pub const kCGBitmapByteOrderMask: CGBitmapInfo = kCGImageByteOrderMask;

pub const CLASSES: ClassExports = objc_classes! {
(env, this, _cmd);

@implementation _touchHLE_CGImage: NSObject
@end
};

struct CGImageHostObject {
    image: Image,
}
impl HostObject for CGImageHostObject {}

pub type CGImageRef = CFTypeRef;

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

pub fn from_image(env: &mut Environment, image: Image) -> CGImageRef {
    let host_obj = Box::new(CGImageHostObject { image });
    let class = env.objc.get_known_class("_touchHLE_CGImage", &mut env.mem);
    env.objc.alloc_object(class, host_obj, &mut env.mem)
}

pub fn borrow_image(objc: &ObjC, image: CGImageRef) -> &Image {
    // ВНИМАНИЕ: Если здесь передан null, эмулятор упадет. 
    // Но CoreGraphics функции ниже теперь защищены.
    &objc.borrow::<CGImageHostObject>(image).image
}

pub fn borrow_image_mut(objc: &mut ObjC, image: CGImageRef) -> &mut Image {
    &mut objc.borrow_mut::<CGImageHostObject>(image).image
}

fn CGImageCreateCopyWithColorSpace(
    env: &mut Environment,
    image: CGImageRef,
    color_space: CGColorSpaceRef,
) -> CGImageRef {
    if image.is_null() { return nil; }
    
    let image_color_space = CGImageGetColorSpace(env, image);
    if image_color_space.is_null() { return nil; }

    assert_eq!(
        CGColorSpaceGetModel(env, image_color_space),
        CGColorSpaceGetModel(env, color_space)
    );
    
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
    let Ok(image) = Image::from_bytes(bytes) else {
        return nil;
    };

    from_image(env, image)
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
    let Ok(image) = Image::from_bytes(bytes) else {
        return nil;
    };

    from_image(env, image)
}

fn CGImageGetAlphaInfo(_env: &mut Environment, image: CGImageRef) -> CGImageAlphaInfo {
    if image.is_null() { return kCGImageAlphaNone; }
    kCGImageAlphaPremultipliedLast
}

fn CGImageGetColorSpace(env: &mut Environment, image: CGImageRef) -> CGColorSpaceRef {
    if image.is_null() { return nil; }
    let srgb_name = ns_string::get_static_str(env, kCGColorSpaceGenericRGB);
    CGColorSpaceCreateWithName(env, srgb_name)
}

pub fn CGImageGetWidth(env: &mut Environment, image: CGImageRef) -> GuestUSize {
    if image.is_null() { return 0; }
    let (width, _height) = env
        .objc
        .borrow::<CGImageHostObject>(image)
        .image
        .dimensions();
    width
}

pub fn CGImageGetHeight(env: &mut Environment, image: CGImageRef) -> GuestUSize {
    if image.is_null() { return 0; }
    let (_width, height) = env
        .objc
        .borrow::<CGImageHostObject>(image)
        .image
        .dimensions();
    height
}

fn CGImageGetBitsPerPixel(_env: &mut Environment, image: CGImageRef) -> GuestUSize {
    if image.is_null() { return 0; }
    32
}

fn CGImageGetBytesPerRow(env: &mut Environment, image: CGImageRef) -> GuestUSize {
    if image.is_null() { return 0; }
    let (width, _height) = env
        .objc
        .borrow::<CGImageHostObject>(image)
        .image
        .dimensions();
    width * 4
}

fn CGImageGetDataProvider(env: &mut Environment, image: CGImageRef) -> CGDataProviderRef {
    if image.is_null() { return nil; }
    let cg_data_provider = cg_data_provider::from_cg_image(env, image);
    autorelease(env, cg_data_provider)
}

fn CGImageGetBitsPerComponent(_: &mut Environment, image: CGImageRef) -> GuestUSize {
    if image.is_null() { return 0; }
    8
}

/// Copy of an existing CGImage — just clone the underlying Image.
fn CGImageCreateCopy(env: &mut Environment, image: CGImageRef) -> CGImageRef {
    if image.is_null() { return nil; }
    let new_image = env.objc.borrow::<CGImageHostObject>(image).image.clone();
    from_image(env, new_image)
}

/// Crop to a sub-rectangle. CGRect is in pixels (no coordinate transform here).
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

    if width == 0 || height == 0 { return nil; }

    let src_pixels = env
        .objc
        .borrow::<CGImageHostObject>(image)
        .image
        .pixels();

    // Copy the sub-region row by row (RGBA — 4 bytes per pixel).
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

/// Create an image masked by another image. We don't apply the mask —
/// return a copy of the source so apps that read back pixels get something.
fn CGImageCreateWithMask(
    env: &mut Environment,
    image: CGImageRef,
    _mask: CGImageRef,
) -> CGImageRef {
    log!("CGImageCreateWithMask: mask not applied (stubbed)");
    CGImageCreateCopy(env, image)
}

/// Invert-mask stub — just copy.
fn CGImageCreateMaskWithImageMask(env: &mut Environment, mask_image: CGImageRef) -> CGImageRef {
    log!("CGImageCreateMaskWithImageMask: stubbed");
    CGImageCreateCopy(env, mask_image)
}

// MARK: - Additional accessors

fn CGImageGetBitmapInfo(_env: &mut Environment, image: CGImageRef) -> CGBitmapInfo {
    if image.is_null() { return 0; }
    // Report premultiplied-last RGBA, big-endian 32-bit — matches our Image format.
    kCGImageAlphaPremultipliedLast | kCGImageByteOrder32Big
}

/// Decode array — we always use the default (nil / identity mapping).
fn CGImageGetDecode(_env: &mut Environment, image: CGImageRef) -> ConstPtr<CGFloat> {
    ConstPtr::null()
}

fn CGImageGetShouldInterpolate(_env: &mut Environment, image: CGImageRef) -> bool {
    if image.is_null() { return false; }
    true
}

/// CGColorRenderingIntent — 0 = kCGRenderingIntentDefault.
fn CGImageGetRenderingIntent(_env: &mut Environment, image: CGImageRef) -> i32 {
    0
}

fn CGImageIsMask(_env: &mut Environment, _image: CGImageRef) -> bool {
    false
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(CGImageRelease(_)),
    export_c_func!(CGImageRetain(_)),
    export_c_func!(CGImageCreateCopyWithColorSpace(_, _)),
    export_c_func!(CGImageCreateWithPNGDataProvider(_, _, _, _)),
    export_c_func!(CGImageCreateWithJPEGDataProvider(_, _, _, _)),
    export_c_func!(CGImageCreateWithImageInRect(_, _)),
    export_c_func!(CGImageCreateWithMask(_, _)),
    export_c_func!(CGImageCreateMaskWithImageMask(_)),
    export_c_func!(CGImageCreateCopy(_)),
    export_c_func!(CGImageGetAlphaInfo(_)),
    export_c_func!(CGImageGetBitmapInfo(_)),
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
    export_c_func!(CGImageCreateWithImageInRect(_, _)),
];

