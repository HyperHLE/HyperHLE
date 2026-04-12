/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 * If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `CGBitmapContext.h`

use super::cg_affine_transform::{CGAffineTransform, CGAffineTransformIdentity};
use super::cg_color_space::{
    kCGColorSpaceGenericGray, kCGColorSpaceGenericRGB, CGColorSpaceHostObject, CGColorSpaceRef,
};
use super::cg_context::{CGContextHostObject, CGContextRef, CGContextSubclass};
use super::cg_image::{
    self, kCGBitmapAlphaInfoMask, kCGBitmapByteOrderMask, kCGImageAlphaFirst, kCGImageAlphaLast,
    kCGImageAlphaNone, kCGImageAlphaNoneSkipFirst, kCGImageAlphaNoneSkipLast, kCGImageAlphaOnly,
    kCGImageAlphaPremultipliedFirst, kCGImageAlphaPremultipliedLast, kCGImageByteOrder32Big,
    kCGImageByteOrderDefault, CGBitmapInfo, CGImageAlphaInfo, CGImageRef,
};
use super::{CGFloat, CGPoint, CGRect};
use crate::dyld::{export_c_func, FunctionExports};
use crate::image::{gamma_decode, gamma_encode, Image};
use crate::mem::{GuestUSize, Mem, MutVoidPtr};
use crate::objc::ObjC;
use crate::Environment;

#[derive(Copy, Clone)]
pub(super) struct CGBitmapContextData {
    pub(super) data: MutVoidPtr,
    pub(super) data_is_owned: bool,
    width: GuestUSize,
    height: GuestUSize,
    bits_per_component: GuestUSize,
    bytes_per_row: GuestUSize,
    color_space: &'static str,
    alpha_info: CGImageAlphaInfo,
}

pub fn CGBitmapContextCreate(
    env: &mut Environment,
    data: MutVoidPtr,
    width: GuestUSize,
    height: GuestUSize,
    bits_per_component: GuestUSize,
    bytes_per_row: GuestUSize,
    color_space: CGColorSpaceRef,
    bitmap_info: u32,
) -> CGContextRef {
    // assert!(bits_per_component == 8); // TODO: support other bit depths

        // Честная обработка NULL color_space: по умолчанию используем RGB.
    let color_space_name = if color_space.is_null() {
        kCGColorSpaceGenericRGB
    } else {
        env.objc.borrow::<CGColorSpaceHostObject>(color_space).name
    };

    let component_count = match color_space_name {
        kCGColorSpaceGenericRGB => components_for_rgb(bitmap_info).unwrap_or(4), // Fallback на 4 компонента
        kCGColorSpaceGenericGray => components_for_gray(bitmap_info).unwrap_or(1),
        _ => {
            log!("Warning: unknown color space '{}' in CGBitmapContextCreate, falling back to RGB", color_space_name);
            components_for_rgb(bitmap_info).unwrap_or(4)
        }
    };
    
    // Перезаписываем имя для сохранения в структуре
    let color_space = color_space_name;

    let (data, data_is_owned, bytes_per_row) = if data.is_null() {
        let bytes_per_row = if bytes_per_row == 0 {
            width.checked_mul(component_count).unwrap()
        } else {
            bytes_per_row
        };
        let total_size = bytes_per_row.checked_mul(height).unwrap();
        let data = env.mem.alloc(total_size);
        (data, true, bytes_per_row)
    } else {
        // assert!(bytes_per_row != 0);
        (data, false, bytes_per_row)
    };

    let host_object = CGContextHostObject {
        subclass: CGContextSubclass::CGBitmapContext(CGBitmapContextData {
            data,
            data_is_owned,
            width,
            height,
            bits_per_component,
            bytes_per_row,
            color_space,
            alpha_info: bitmap_info & kCGBitmapAlphaInfoMask,
        }),
        transform: CGAffineTransformIdentity, // <--- ИСПРАВЛЕНИЕ: Использована константа вместо Default::default()
        // When creating a CGBitmapContext, initialise:
        rgb_fill_color:   (0.0, 0.0, 0.0, 1.0),
        rgb_stroke_color: (0.0, 0.0, 0.0, 1.0),
        alpha:            1.0,
        line_width:       1.0,
        line_cap:         0,   // kCGLineCapButt
        line_join:        0,   // kCGLineJoinMiter
        miter_limit:      10.0,
        flatness:         0.0,
        blend_mode:       0,   // kCGBlendModeNormal
        state_stack:      Vec::new(),
        path_points:      Vec::new(),
    };

    let isa = env
        .objc
        .get_known_class("_touchHLE_CGContext", &mut env.mem);
    env.objc
        .alloc_object(isa, Box::new(host_object), &mut env.mem)
}

pub fn CGBitmapContextGetData(env: &mut Environment, context: CGContextRef) -> MutVoidPtr {
    let host_obj = env.objc.borrow::<CGContextHostObject>(context);
    let CGContextSubclass::CGBitmapContext(bitmap_data) = host_obj.subclass;
    bitmap_data.data
}

pub fn CGBitmapContextGetWidth(env: &mut Environment, context: CGContextRef) -> GuestUSize {
    let host_obj = env.objc.borrow::<CGContextHostObject>(context);
    let CGContextSubclass::CGBitmapContext(bitmap_data) = host_obj.subclass;
    bitmap_data.width
}

pub fn CGBitmapContextGetHeight(env: &mut Environment, context: CGContextRef) -> GuestUSize {
    let host_obj = env.objc.borrow::<CGContextHostObject>(context);
    let CGContextSubclass::CGBitmapContext(bitmap_data) = host_obj.subclass;
    bitmap_data.height
}

fn CGBitmapContextGetBytesPerRow(env: &mut Environment, context: CGContextRef) -> GuestUSize {
    let host_obj = env.objc.borrow::<CGContextHostObject>(context);
    let CGContextSubclass::CGBitmapContext(bitmap_data) = host_obj.subclass;
    bitmap_data.bytes_per_row
}

pub fn CGBitmapContextCreateImage(env: &mut Environment, context: CGContextRef) -> CGImageRef {
    let host_obj = env.objc.borrow::<CGContextHostObject>(context);
    let CGContextSubclass::CGBitmapContext(bitmap_data) = host_obj.subclass;
    assert!(
        bitmap_data.bits_per_component == 8
            && bitmap_data.bytes_per_row == bitmap_data.width * 4
            && bitmap_data.color_space == kCGColorSpaceGenericRGB
            && matches!(
                bitmap_data.alpha_info,
                kCGImageAlphaNoneSkipLast | kCGImageAlphaPremultipliedLast
            )
    );

    let pixels = env
        .mem
        .bytes_at(
            bitmap_data.data.cast(),
            bitmap_data.bytes_per_row * bitmap_data.height,
        )
        .to_vec();

    cg_image::from_image(
        env,
        Image::from_pixel_vec(pixels, (bitmap_data.width, bitmap_data.height)),
    )
}

fn components_for_rgb(bitmap_info: CGBitmapInfo) -> Result<GuestUSize, ()> {
    let byte_order = bitmap_info & kCGBitmapByteOrderMask;
    if byte_order != kCGImageByteOrderDefault && byte_order != kCGImageByteOrder32Big {
        return Err(());
    }

    let alpha_info = bitmap_info & kCGBitmapAlphaInfoMask;
    if (alpha_info | byte_order) != bitmap_info {
        return Err(());
    }
    match alpha_info & kCGBitmapAlphaInfoMask {
        kCGImageAlphaNone => Ok(3), // RGB
        kCGImageAlphaPremultipliedLast
        | kCGImageAlphaPremultipliedFirst
        | kCGImageAlphaLast
        | kCGImageAlphaFirst
        | kCGImageAlphaNoneSkipLast
        | kCGImageAlphaNoneSkipFirst => Ok(4), // RGBA/ARGB/RGBX/XRGB
        kCGImageAlphaOnly => Ok(1), // A
        _ => Err(()),               // unknown values
    }
}

fn components_for_gray(bitmap_info: CGBitmapInfo) -> Result<GuestUSize, ()> {
    let byte_order = bitmap_info & kCGBitmapByteOrderMask;
    if byte_order != kCGImageByteOrderDefault && byte_order != kCGImageByteOrder32Big {
        return Err(());
    }

    let alpha_info = bitmap_info & kCGBitmapAlphaInfoMask;
    if (alpha_info | byte_order) != bitmap_info {
        return Err(());
    }
    match alpha_info & kCGBitmapAlphaInfoMask {
        kCGImageAlphaNone => Ok(1), // gray
        kCGImageAlphaPremultipliedLast
        | kCGImageAlphaPremultipliedFirst
        | kCGImageAlphaLast
        | kCGImageAlphaFirst
        | kCGImageAlphaNoneSkipLast
        | kCGImageAlphaNoneSkipFirst => Ok(2), // gray + alpha
        kCGImageAlphaOnly => Ok(1), // A
        _ => Err(()),               // unknown values
    }
}

fn bytes_per_pixel(data: &CGBitmapContextData) -> GuestUSize {
    let &CGBitmapContextData {
        bits_per_component,
        color_space,
        alpha_info,
        ..
    } = data;
    assert!(bits_per_component == 8);
    match color_space {
        kCGColorSpaceGenericRGB => components_for_rgb(alpha_info).unwrap_or(4),
        kCGColorSpaceGenericGray => components_for_gray(alpha_info).unwrap_or(1),
        _ => components_for_rgb(alpha_info).unwrap_or(4), // Fallback
    }
}

fn get_pixels<'a>(data: &CGBitmapContextData, mem: &'a mut Mem) -> &'a mut [u8] {
    let pixel_data_size = data.height.checked_mul(data.bytes_per_row).unwrap();
    mem.bytes_at_mut(data.data.cast(), pixel_data_size)
}

fn blend_alpha(bg: f32, fg: f32) -> f32 {
    fg + bg * (1.0 - fg)
}

fn blend_straight(bg: (f32, f32, f32, f32), fg: (f32, f32, f32, f32)) -> (f32, f32, f32, f32) {
    if fg.3 == 0.0 {
        bg
    } else {
        let new_a = blend_alpha(bg.3, fg.3);
        (
            (fg.0 * fg.3 + bg.0 * bg.3 * (1.0 - fg.3)) / new_a,
            (fg.1 * fg.3 + bg.1 * bg.3 * (1.0 - fg.3)) / new_a,
            (fg.2 * fg.3 + bg.2 * bg.3 * (1.0 - fg.3)) / new_a,
            new_a,
        )
    }
}

fn blend_premultiplied(bg: (f32, f32, f32, f32), fg: (f32, f32, f32, f32)) -> (f32, f32, f32, f32) {
    (
        fg.0 + bg.0 * (1.0 - fg.3),
        fg.1 + bg.1 * (1.0 - fg.3),
        fg.2 + bg.2 * (1.0 - fg.3),
        blend_alpha(bg.3, fg.3),
    )
}

fn pixel_offsets(data: &CGBitmapContextData) -> (usize, usize, usize, Option<usize>) {
    match data.color_space {
        kCGColorSpaceGenericRGB => {
            match data.alpha_info {
                kCGImageAlphaNone => (0, 1, 2, None),
                kCGImageAlphaPremultipliedLast | kCGImageAlphaLast => (0, 1, 2, Some(3)),
                kCGImageAlphaPremultipliedFirst | kCGImageAlphaFirst => (1, 2, 3, Some(0)),
                kCGImageAlphaNoneSkipLast => (0, 1, 2, None),
                kCGImageAlphaNoneSkipFirst => (1, 2, 3, None),
                kCGImageAlphaOnly => (0, 0, 0, Some(0)),
                _ => unreachable!(),
            }
        }
        kCGColorSpaceGenericGray => {
            match data.alpha_info {
                kCGImageAlphaNone => (0, 0, 0, None),
                kCGImageAlphaPremultipliedLast | kCGImageAlphaLast => (0, 0, 0, Some(1)),
                kCGImageAlphaPremultipliedFirst | kCGImageAlphaFirst => (1, 1, 1, Some(0)),
                kCGImageAlphaNoneSkipLast => (0, 0, 0, None),
                kCGImageAlphaNoneSkipFirst => (1, 1, 1, None),
                kCGImageAlphaOnly => (0, 0, 0, Some(0)),
                _ => unreachable!(),
            }
        }
        _ => unimplemented!(),
    }
}

fn get_pixel(
    data: &CGBitmapContextData,
    pixels: &mut [u8],
    first_component_idx: usize,
) -> (f32, f32, f32, f32) {
    let pixel_offset = pixel_offsets(data);
    let pixel = (
        pixels[first_component_idx + pixel_offset.0] as f32 / 255.0,
        pixels[first_component_idx + pixel_offset.1] as f32 / 255.0,
        pixels[first_component_idx + pixel_offset.2] as f32 / 255.0,
        if let Some(alpha_offest) = pixel_offset.3 {
            pixels[first_component_idx + alpha_offest] as f32 / 255.0
        } else {
            1.0
        },
    );
    (
        gamma_decode(pixel.0),
        gamma_decode(pixel.1),
        gamma_decode(pixel.2),
        pixel.3,
    )
}

fn put_pixel(
    data: &CGBitmapContextData,
    pixels: &mut [u8],
    coords: (i32, i32),
    pixel: (CGFloat, CGFloat, CGFloat, CGFloat),
    blend: bool,
) {
    let (x, y) = coords;
    if x < 0 || y < 0 {
        return;
    }
    let (x, y) = (x as GuestUSize, y as GuestUSize);
    if x >= data.width || y >= data.height {
        return;
    }

    let y = data.height - 1 - y;
    let pixel_size = bytes_per_pixel(data);
    let first_component_idx = (y * data.bytes_per_row + x * pixel_size) as usize;

    let bg_pixel = get_pixel(data, pixels, first_component_idx);
    let (r, g, b, a) = if blend {
        match data.alpha_info {
            kCGImageAlphaLast | kCGImageAlphaFirst => blend_straight(bg_pixel, pixel),
            kCGImageAlphaPremultipliedLast | kCGImageAlphaPremultipliedFirst => {
                blend_premultiplied(bg_pixel, pixel)
            }
            kCGImageAlphaOnly => (pixel.0, pixel.1, pixel.2, blend_alpha(bg_pixel.3, pixel.3)),
            _ => pixel,
        }
    } else {
        pixel
    };
    let (r, g, b) = (gamma_encode(r), gamma_encode(g), gamma_encode(b));
    let pixel_offset = pixel_offsets(data);
    match data.alpha_info {
        kCGImageAlphaOnly => {
            pixels[first_component_idx] = (a * 255.0) as u8;
        }
        _ => {
            pixels[first_component_idx + pixel_offset.0] = (r * 255.0) as u8;
            pixels[first_component_idx + pixel_offset.1] = (g * 255.0) as u8;
            pixels[first_component_idx + pixel_offset.2] = (b * 255.0) as u8;
            if let Some(alpha_offset) = pixel_offset.3 {
                pixels[first_component_idx + alpha_offset] = (a * 255.0) as u8;
            }
        }
    }
}

pub struct CGBitmapContextDrawer<'a> {
    bitmap_info: CGBitmapContextData,
    rgb_fill_color: (CGFloat, CGFloat, CGFloat, CGFloat),
    transform: CGAffineTransform,
    pixels: &'a mut [u8],
}
impl CGBitmapContextDrawer<'_> {
    pub fn new<'a>(
        objc: &ObjC,
        mem: &'a mut Mem,
        context: CGContextRef,
    ) -> CGBitmapContextDrawer<'a> {
        let &CGContextHostObject {
            subclass: CGContextSubclass::CGBitmapContext(bitmap_info),
            rgb_fill_color,
            transform,
            ..
        } = objc.borrow(context);
        let pixels = get_pixels(&bitmap_info, mem);

        CGBitmapContextDrawer {
            bitmap_info,
            rgb_fill_color,
            transform,
            pixels,
        }
    }

    pub fn width(&self) -> GuestUSize {
        self.bitmap_info.width
    }
    pub fn height(&self) -> GuestUSize {
        self.bitmap_info.height
    }
    pub fn rgb_fill_color(&self) -> (CGFloat, CGFloat, CGFloat, CGFloat) {
        let multiply_by = match self.bitmap_info.alpha_info {
            kCGImageAlphaPremultipliedLast | kCGImageAlphaPremultipliedFirst => {
                self.rgb_fill_color.3
            }
            _ => 1.0,
        };
        (
            gamma_decode(self.rgb_fill_color.0 * multiply_by),
            gamma_decode(self.rgb_fill_color.1 * multiply_by),
            gamma_decode(self.rgb_fill_color.2 * multiply_by),
            self.rgb_fill_color.3, // alpha is always linear
        )
    }
    pub fn put_pixel(
        &mut self,
        coords: (i32, i32),
        color: (CGFloat, CGFloat, CGFloat, CGFloat),
        blend: bool,
    ) {
        put_pixel(&self.bitmap_info, self.pixels, coords, color, blend)
    }

    pub fn iter_transformed_pixels(
        &self,
        untransformed_rect: CGRect,
    ) -> impl Iterator<Item = ((i32, i32), (f32, f32))> {
        let bounding_rect = self.transform.apply_to_rect(untransformed_rect);
        let x_start = bounding_rect.origin.x.round().max(0.0) as GuestUSize;
        let y_start = bounding_rect.origin.y.round().max(0.0) as GuestUSize;
        let x_end = (bounding_rect.origin.x + bounding_rect.size.width)
            .round()
            .min(self.width() as f32) as GuestUSize;
        let y_end = (bounding_rect.origin.y + bounding_rect.size.height)
            .round()
            .min(self.height() as f32) as GuestUSize;
        let inverse_transform = self.transform.invert();

        (y_start..y_end).flat_map(move |y| {
            (x_start..x_end).flat_map(move |x| {
                let untransformed = inverse_transform.apply_to_point(CGPoint {
                    x: x as f32 + 0.5,
                    y: y as f32 + 0.5,
                });
                let x_within =
                    (untransformed.x - untransformed_rect.origin.x) / untransformed_rect.size.width;
                let y_within = (untransformed.y - untransformed_rect.origin.y)
                    / untransformed_rect.size.height;
                if !(0.0..1.0).contains(&x_within) || !(0.0..1.0).contains(&y_within) {
                    None
                } else {
                    Some(((x as i32, y as i32), (x_within, y_within)))
                }
            })
        })
    }
}

pub(super) fn fill_rect(env: &mut Environment, context: CGContextRef, rect: CGRect, clear: bool) {
    let mut drawer = CGBitmapContextDrawer::new(&env.objc, &mut env.mem, context);
    let color = if clear {
        (0.0, 0.0, 0.0, 0.0)
    } else {
        drawer.rgb_fill_color()
    };
    for ((x, y), _) in drawer.iter_transformed_pixels(rect) {
        drawer.put_pixel((x, y), color, /* blend: */ !clear)
    }
}

pub(super) fn draw_image(
    env: &mut Environment,
    context: CGContextRef,
    rect: CGRect,
    image: CGImageRef,
) {
    let image = cg_image::borrow_image(&env.objc, image);
    let mut drawer = CGBitmapContextDrawer::new(&env.objc, &mut env.mem, context);

    let (image_width, image_height) = image.dimensions();
    for ((x, y), (texel_x, texel_y)) in drawer.iter_transformed_pixels(rect) {
        let texel_x = (image_width as f32 * texel_x) as i32;
        let texel_y = (image_height as f32 * (1.0 - texel_y)) as i32;
        if let Some(color) = image.get_pixel((texel_x, texel_y)) {
            drawer.put_pixel((x, y), color, /* blend: */ true)
        }
    }
}

#[allow(rustdoc::broken_intra_doc_links)] // https://github.com/rust-lang/rust/issues/83049
pub fn get_data(objc: &ObjC, context: CGContextRef) -> (GuestUSize, GuestUSize, MutVoidPtr) {
    let host_obj = objc.borrow::<CGContextHostObject>(context);
    let CGContextSubclass::CGBitmapContext(bitmap_data) = host_obj.subclass;
    (bitmap_data.width, bitmap_data.height, bitmap_data.data)
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(CGBitmapContextCreate(_, _, _, _, _, _, _)),
    export_c_func!(CGBitmapContextCreateImage(_)),
    export_c_func!(CGBitmapContextGetData(_)),
    export_c_func!(CGBitmapContextGetWidth(_)),
    export_c_func!(CGBitmapContextGetHeight(_)),
    export_c_func!(CGBitmapContextGetBytesPerRow(_)),
];

