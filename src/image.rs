/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Image decoding.
//!
//! Currently, supports PNG (treated as 8-bit sRGB), JPEG, BMP, GIF, and TGA
//! files.
//!
//! Implemented as a wrapper around the C library stb_image, since it supports
//! "CgBI" PNG files (an Apple proprietary extension used in iPhone OS apps).
//!
//! This module also exposes decompression for Imagination Technologies' PVRTC
//! format, implementing as a wrapper around their decoder from the PowerVR
//! SDK.
//!
//! References:
//! - "Supported Image Formats" in [Loading Images](https://developer.apple.com/library/archive/documentation/2DDrawing/Conceptual/DrawingPrintingiOS/LoadingImages/LoadingImages.html)

use std::collections::hash_map::DefaultHasher;
use std::ffi::{c_int, c_uchar, CStr};
use std::hash::{Hash, Hasher};
use std::io::{Read, Write};

use touchHLE_pvrt_decompress_wrapper::*;
use touchHLE_stb_image_wrapper::*;

#[derive(Default)]
pub struct Image {
    pixels: PixelStore,
    dimensions: (u32, u32),
}

#[derive(Default)]
enum PixelStore {
    #[default]
    Empty,
    StbImage(*mut c_uchar),
    Vec(Vec<u8>),
}

const PNG_MAGIC_NUMBER: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];

impl Image {
    pub fn from_bytes(bytes: &[u8]) -> Result<Image, String> {
        let len: c_int = bytes.len().try_into().unwrap();
        let mut x: c_int = 0;
        let mut y: c_int = 0;
        let mut _channels_in_file: c_int = 0;

        // TODO: we're currently assuming this is sRGB, can we check somehow?
        let pixels = unsafe {
            // stb_image's support for CgBI images is a bit incomplete:
            // - If we don't ask it to "convert to RGB" for us, it will load
            //   CgBI PNGs in BGR and normal PNGs as RGB, with no way to
            //   distinguish them!
            // - If we don't ask it to "unpremultiply" for us, it won't do the
            //   RGB conversion.
            // So this is the only correct way to use it. :(
            stbi_convert_iphone_png_to_rgb(1);
            stbi_set_unpremultiply_on_load(1);
            stbi_load_from_memory(
                bytes.as_ptr(),
                len,
                &mut x,
                &mut y,
                &mut _channels_in_file,
                4,
            )
        };
        if pixels.is_null() {
            let reason = unsafe { CStr::from_ptr(stbi_failure_reason()) };
            return Err(reason.to_str().unwrap().to_string());
        }

        let width: u32 = x.try_into().unwrap();
        let height: u32 = y.try_into().unwrap();

        // (Un-un-)premultiply pixels to match iPhone OS's image loading.
        if bytes.starts_with(&PNG_MAGIC_NUMBER) {
            let len = width as usize * height as usize * 4;
            let pixels = unsafe { std::slice::from_raw_parts_mut(pixels, len) };

            let mut i = 0;
            while i < pixels.len() {
                let a = pixels[i + 3] as f32 / 255.0;
                pixels[i] = (pixels[i] as f32 * a) as u8;
                pixels[i + 1] = (pixels[i + 1] as f32 * a) as u8;
                pixels[i + 2] = (pixels[i + 2] as f32 * a) as u8;
                i += 4;
            }
        }

        Ok(Image {
            pixels: PixelStore::StbImage(pixels),
            dimensions: (width, height),
        })
    }

    /// TODO: This shouldn't really exist, it's a workaround for `CGImage`
    /// relying on this type and should be removed once it can be refactored.
    pub fn from_pixel_vec(pixels: Vec<u8>, dimensions: (u32, u32)) -> Image {
        assert!(dimensions.0 as usize * 4 * dimensions.1 as usize == pixels.len());
        Image {
            pixels: PixelStore::Vec(pixels),
            dimensions,
        }
    }

    pub fn from_pixels(width: u32, height: u32, pixels: Vec<u8>) -> Self {
        assert_eq!(pixels.len(), (width * height * 4) as usize);
        Image {
            pixels: PixelStore::Vec(pixels),
            dimensions: (width, height),
        }
    }

    pub fn dimensions(&self) -> (u32, u32) {
        self.dimensions
    }

    /// Get image data as bytes (8 bits per channel sRGB RGBA with premultiplied
    /// alpha). Rows are in top-to-bottom order.
    pub fn pixels(&self) -> &[u8] {
        match self.pixels {
            PixelStore::Empty => &[],
            PixelStore::Vec(ref vec) => vec,
            PixelStore::StbImage(ptr) => unsafe {
                std::slice::from_raw_parts(
                    ptr,
                    self.dimensions.0 as usize * self.dimensions.1 as usize * 4,
                )
            },
        }
    }

    fn pixels_mut(&mut self) -> &mut [u8] {
        match self.pixels {
            PixelStore::Empty => &mut [],
            PixelStore::Vec(ref mut vec) => vec,
            PixelStore::StbImage(ptr) => unsafe {
                std::slice::from_raw_parts_mut(
                    ptr,
                    self.dimensions.0 as usize * self.dimensions.1 as usize * 4,
                )
            },
        }
    }

    /// Get value of a pixel as linear RGBA (not sRGB!) with premultiplied
    /// alpha. 0 on the y axis is the top of the image.
    ///
    /// Returns [None] if `at` is out-of-bounds.
    pub fn get_pixel(&self, at: (i32, i32)) -> Option<(f32, f32, f32, f32)> {
        let (x, y) = at;
        let (x_usize, y_usize) = (x as usize, y as usize);
        let (width, height) = self.dimensions;
        let (width, height) = (width as usize, height as usize);

        if x >= 0 && x_usize < width && y >= 0 && y_usize < height {
            let rgba = &self.pixels()[y_usize * width * 4 + x_usize * 4..][..4];
            let [r, g, b, a]: [u8; 4] = rgba.try_into().unwrap();
            Some((
                gamma_decode(r as f32 / 255.0),
                gamma_decode(g as f32 / 255.0),
                gamma_decode(b as f32 / 255.0),
                a as f32 / 255.0, // alpha is linear
            ))
        } else {
            None
        }
    }

    /// Rounds off the corners and adds the gloss of an iOS app icon.
    ///
    /// Unlike [Self::round_corners], the corner radius follows the icon's
    /// resolution: a 57×57 icon (the original iPhone size) gets a 12px
    /// radius, an `@2x` 114×114 icon gets twice that, and a large iPad or
    /// `@3x` icon gets proportionally more. A fixed radius makes
    /// high-resolution icons look almost square.
    pub fn round_icon_corners(&mut self, add_sheen: bool) {
        let (width, height) = self.dimensions();
        let radius = icon_corner_radius(width.min(height));
        self.round_corners(radius, /* four_corners: */ true, add_sheen);
    }

    // TODO: Eventually this should be in Core Animation instead?
    /// Modify the image to mask it with one to four anti-aliased rounded
    /// corners, and add sheen if desired.
    pub fn round_corners(&mut self, radius: f32, four_corners: bool, add_sheen: bool) {
        let (width, height) = self.dimensions();
        let (w_usize, h_usize) = (width as usize, height as usize);
        let (w, h) = (width as f32, height as f32);

        let (right_corners_begin, bottom_corners_begin) = if four_corners {
            (w - radius - 1.0, h - radius - 1.0)
        } else {
            (f32::INFINITY, f32::INFINITY)
        };

        let (sheen_center_x, sheen_center_y, sheen_radius) = if add_sheen {
            (w / 2.0, -(h / 2.0), h)
        } else {
            (f32::INFINITY, f32::INFINITY, 0.0)
        };

        for y_usize in 0..h_usize {
            for x_usize in 0..w_usize {
                let x = x_usize as f32;
                let y = y_usize as f32;

                let corner_x = (radius - x).max(x - right_corners_begin);
                let corner_y = (radius - y).max(y - bottom_corners_begin);
                let corner_circle_distance = corner_x.hypot(corner_y);

                let x_edge_distance = (x).min(w - x - 1.0);
                let y_edge_distance = (y).min(h - y - 1.0);
                let actual_corner_distance = x_edge_distance.hypot(y_edge_distance);

                let rounded_edge_distance = x_edge_distance
                    .min(y_edge_distance)
                    .min((radius.max(actual_corner_distance) - corner_circle_distance).max(0.0));

                let icon_opacity = if corner_x > 0.0 && corner_y > 0.0 {
                    // Bad approximation of the pixel coverage of a filled arc.
                    let distance = (corner_circle_distance - radius).clamp(0.0, 1.0);
                    let area = distance * distance;
                    1.0 - area
                } else {
                    1.0
                };

                let sheen_opacity = {
                    let distance = (x - sheen_center_x).hypot(y - sheen_center_y);
                    // Bad approximation of the pixel coverage of a filled arc.
                    let distance = (distance - sheen_radius).clamp(0.0, 1.0);
                    let area = distance * distance;
                    // The sheen is stronger at the top and close to edges.
                    let taper = (1.0
                        - ((y / h).sqrt() / 2.0)
                        - (rounded_edge_distance / (h).hypot(w)).sqrt() / 2.0)
                        * 0.75;
                    // There is also extra shinyness around the rim.
                    (1.0 - area) * taper
                };

                let rgba = &mut self.pixels_mut()[y_usize * w_usize * 4 + x_usize * 4..][..4];
                for channel in rgba.iter_mut() {
                    *channel = (*channel as f32 * icon_opacity * (1.0 - sheen_opacity)
                        + 255.0 * icon_opacity * sheen_opacity)
                        as u8;
                }
            }
        }
    }

    /// Writes an RGBA image to a PNG file using stb_image_write_png_to_func
    /// This is used by `UIImageWriteToSavedPhotosAlbum` to save images
    ///
    /// Parameters:
    /// - `ctx_ptr`: Context pointer used by the stb write callback.
    /// - `w`: Image width in pixels.
    /// - `h`: Image height in pixels.
    /// - `rgba`: Raw pixel buffer in RGBA8888 format.
    /// - `stride`: Number of bytes per image row (normally `width * 4`).
    ///
    /// Returns:
    /// The return value from `stbi_write_png`:
    /// - non-zero on success
    /// - zero on failure.
    pub fn write_png_image(
        &self,
        ctx_ptr: *mut std::ffi::c_void,
        w: i32,
        h: i32,
        rgba: &[u8],
        stride: i32,
    ) -> i32 {
        unsafe extern "C" fn write_cb(
            context: *mut std::ffi::c_void,
            data: *mut std::ffi::c_void,
            size: std::ffi::c_int,
        ) {
            if context.is_null() || data.is_null() || size <= 0 {
                return;
            }
            let out: &mut Vec<u8> = &mut *(context as *mut Vec<u8>);
            let bytes = std::slice::from_raw_parts(data as *const u8, size as usize);
            out.extend_from_slice(bytes);
        }

        unsafe {
            stbi_write_png_to_func(
                Some(write_cb),
                ctx_ptr,
                w,
                h,
                4,
                rgba.as_ptr().cast(),
                stride,
            )
        }
    }
}

impl Clone for Image {
    fn clone(&self) -> Image {
        // Note: implicitly converts pixel storage from StbImage to Vec
        // (if needed)
        Image::from_pixel_vec(self.pixels().to_vec(), self.dimensions)
    }
}

impl Drop for Image {
    fn drop(&mut self) {
        match self.pixels {
            PixelStore::StbImage(ptr) => unsafe { stbi_image_free(ptr.cast()) },
            PixelStore::Vec(_) | PixelStore::Empty => (),
        }
    }
}

/// Side length of the original iPhone app icon, which the classic corner
/// radius below is defined for.
const CLASSIC_ICON_SIZE: f32 = 57.0;
/// Corner radius of a classic 57×57 iOS app icon.
const CLASSIC_ICON_CORNER_RADIUS: f32 = 12.0;

/// Corner radius, in pixels, for a square app icon of the given side length.
///
/// iOS always rounds app icon corners by a fixed fraction of the icon size;
/// touchHLE only used a fixed radius, which under-rounds high-resolution
/// (`@2x`, `@3x`, iPad) icons.
pub fn icon_corner_radius(icon_side: u32) -> f32 {
    (CLASSIC_ICON_CORNER_RADIUS * icon_side as f32 / CLASSIC_ICON_SIZE).max(1.0)
}

/// Approximate implementation of sRGB gamma encoding.
pub fn gamma_encode(intensity: f32) -> f32 {
    // TODO: This doesn't implement the linear section near zero.
    intensity.powf(1.0 / 2.2)
}
/// Approximate implementation of sRGB gamma decoding.
pub fn gamma_decode(intensity: f32) -> f32 {
    // TODO: This doesn't implement the linear section near zero.
    intensity.powf(2.2)
}

/// Decodes Imagination Technologies' PVRTC texture compression format to
/// RGBA (8 bits per channel).
///
/// `is_opaque` must be `true` for the `COMPRESSED_RGB_PVRTC_*` (no-alpha)
/// internal formats and `false` for the `COMPRESSED_RGBA_PVRTC_*` variants.
/// Per the `IMG_texture_compression_pvrtc` extension spec, the RGB variants
/// have a base internal format of `RGB`, so sampling them must yield an alpha
/// of 1.0 — exactly as real PowerVR / Apple hardware does. The underlying
/// PVRTDecompress routine always produces a per-texel alpha value (PVRTC
/// blocks can individually opt into a "transparent" colour mode regardless of
/// the requested internal format), so for the opaque formats we overwrite the
/// decoded alpha with 0xFF. Without this, opaque textures uploaded as GL_RGBA
/// keep the decoder's stray sub-255 alpha and, when the app has GL_BLEND
/// enabled with GL_SRC_ALPHA, blend away to nothing — producing a black screen
/// while audio and input keep working.
pub fn decode_pvrtc(pvrtc_data: &[u8], is_2bit: bool, width: u32, height: u32) -> Vec<u32> {
    decode_pvrtc_with_alpha(pvrtc_data, is_2bit, width, height, false)
}

/// Like [decode_pvrtc], but lets the caller declare whether the source format
/// is one of the opaque `COMPRESSED_RGB_PVRTC_*` variants (`is_opaque = true`).
pub fn decode_pvrtc_with_alpha(
    pvrtc_data: &[u8],
    is_2bit: bool,
    width: u32,
    height: u32,
    is_opaque: bool,
) -> Vec<u32> {
    // This formula is from the IMG_texture_compression_pvrtc extension spec.
    let expected_size = if is_2bit {
        (width.max(16) as usize * height.max(8) as usize * 2).div_ceil(8)
    } else {
        (width.max(8) as usize * height.max(8) as usize * 4).div_ceil(8)
    };
    assert!(pvrtc_data.len() == expected_size);

    let rgba8_word_count = width as usize * height as usize;
    let mut rgba8_data = Vec::with_capacity(rgba8_word_count);
    unsafe {
        let consumed_size = touchHLE_decompress_pvrtc(
            pvrtc_data.as_ptr() as *const _,
            is_2bit,
            width,
            height,
            // The interface says `uint8_t *` but the source seems to work with
            // 32-bit words, so using Vec<u32> seems more appropriate.
            rgba8_data.as_mut_ptr() as *mut u8,
        );
        assert_eq!(consumed_size as usize, expected_size);
        rgba8_data.set_len(rgba8_word_count);
    };

    if is_opaque {
        // Force alpha to fully opaque (the high byte of each little-endian
        // RGBA8 word). The RGB-PVRTC base internal format is RGB, so the
        // sampled alpha must be 1.0 on real hardware.
        for word in rgba8_data.iter_mut() {
            *word |= 0xFF00_0000;
        }
    }

    rgba8_data
}

#[cfg(test)]
mod tests {
    use super::{decode_pvrtc_with_alpha, icon_corner_radius};

    #[test]
    fn icon_corner_radius_scales_with_resolution() {
        // The classic 57×57 icon keeps the radius touchHLE always used.
        assert_eq!(icon_corner_radius(57), 12.0);
        // @2x and @3x icons get proportionally larger corners.
        assert_eq!(icon_corner_radius(114), 24.0);
        assert_eq!(icon_corner_radius(171), 36.0);
        // Tiny placeholder icons still get a visible, non-zero radius.
        assert!(icon_corner_radius(1) >= 1.0);
    }

    fn decode_checksum(width: u32, height: u32, is_2bit: bool) -> u64 {
        let compressed_size = width as usize * height as usize / if is_2bit { 4 } else { 2 };
        let mut state = 0x1234_5678u32;
        let compressed: Vec<u8> = (0..compressed_size)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state as u8
            })
            .collect();
        let pixels = decode_pvrtc_with_alpha(&compressed, is_2bit, width, height, false);

        let mut checksum = 14_695_981_039_346_656_037u64;
        for pixel in pixels {
            for byte in pixel.to_le_bytes() {
                checksum ^= u64::from(byte);
                checksum = checksum.wrapping_mul(1_099_511_628_211);
            }
        }
        checksum
    }

    #[test]
    fn pvrtc_parallel_decoding_matches_reference_output() {
        assert_eq!(decode_checksum(64, 64, false), 0x3d39_b466_115e_383c);
        assert_eq!(decode_checksum(512, 512, false), 0xabfa_9dae_71e5_095d);
        assert_eq!(decode_checksum(512, 512, true), 0xd44a_a11d_f63a_0446);
        assert_eq!(decode_checksum(1024, 512, true), 0x99ac_697d_389a_6a2a);
    }
}

/// Directory for cached decoded images, inside the user data directory.
/// Avoids re-decoding PNGs and re-running the premultiply pass on every
/// launch: once an image has been decoded, its raw RGBA is stored here and
/// loaded directly next time.
fn image_cache_dir() -> std::path::PathBuf {
    crate::paths::user_data_base_path().join("image_cache")
}

/// Hash image bytes into a cache-file name. Different images get different
/// files; the same image always maps to the same file (within one Rust
/// toolchain release — a hash change just means a cache miss, not an error).
fn image_cache_key(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

/// Try to load a decoded image from the disk cache. Returns `None` when the
/// cache file is missing, truncated, or unreadable.
fn read_cached_image(key: u64) -> Option<Image> {
    let path = image_cache_dir().join(format!("{key}.rgba"));
    let mut file = std::fs::File::open(&path).ok()?;
    let mut header = [0u8; 8];
    file.read_exact(&mut header).ok()?;
    let width = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
    let height = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
    let expected_len = width.checked_mul(height)? as usize * 4;
    let mut pixels = vec![0u8; expected_len];
    file.read_exact(&mut pixels).ok()?;
    Some(Image::from_pixels(width, height, pixels))
}

/// Write a decoded image to the disk cache. Errors are ignored: the cache is
/// purely an optimization, and a failed write just means decoding next time.
fn write_cached_image(key: u64, image: &Image) {
    let _ = std::fs::create_dir_all(&image_cache_dir());
    let path = image_cache_dir().join(format!("{key}.rgba"));
    let Ok(mut file) = std::fs::File::create(&path) else { return };
    let (width, height) = image.dimensions();
    let _ = file.write_all(&width.to_le_bytes());
    let _ = file.write_all(&height.to_le_bytes());
    let _ = file.write_all(image.pixels());
}

/// Decode an image, using the disk cache when possible.
/// The first launch pays the full PNG decode + premultiply cost; every later
/// launch loads the raw RGBA from disk instead.
pub fn load_image_cached(bytes: &[u8]) -> Result<Image, String> {
    let key = image_cache_key(bytes);
    if let Some(cached) = read_cached_image(key) {
        return Ok(cached);
    }
    let image = Image::from_bytes(bytes)?;
    write_cached_image(key, &image);
    Ok(image)
}
