/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UIImage`.

use crate::dyld::{export_c_func, FunctionExports};
use crate::frameworks::core_graphics::cg_context::CGContextDrawImage;
use crate::frameworks::core_graphics::cg_image::{
    self, CGImageGetHeight, CGImageGetWidth, CGImageRef, CGImageRelease, CGImageRetain,
};
use crate::frameworks::core_graphics::{CGFloat, CGPoint, CGRect, CGSize};
use crate::frameworks::foundation::ns_string::get_static_str;
use crate::frameworks::foundation::{ns_data, ns_string, NSInteger};
use crate::frameworks::uikit::ui_graphics::UIGraphicsGetCurrentContext;
use crate::fs::GuestPath;
use crate::image::Image;
use crate::objc::{
    autorelease, id, msg, msg_class, msg_send, nil, objc_classes, release, retain, ClassExports,
    HostObject, NSZonePtr, SEL,
};
use crate::Environment;
use std::collections::HashMap;

const CACHE_SIZE: usize = 10;

#[derive(Default)]
pub struct State {
    /// Cache of images for `[UIImage imageNamed:]` method.
    /// Images are explicitly retained.
    cached_images: HashMap<String, id>,
}
impl State {
    fn get(env: &Environment) -> &Self {
        &env.framework_state.uikit.ui_image
    }
    fn get_mut(env: &mut Environment) -> &mut Self {
        &mut env.framework_state.uikit.ui_image
    }
}

struct UIImageHostObject {
    cg_image:    CGImageRef,
    orientation: NSInteger, // UIImageOrientation
}
impl HostObject for UIImageHostObject {}

pub const CLASSES: ClassExports = objc_classes!
{

(env, this, _cmd);

@implementation UIImage: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(UIImageHostObject {
        cg_image:    nil,
        orientation: 0, // UIImageOrientationUp
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}


+ (id)imageWithCGImage:(CGImageRef)cg_image {
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithCGImage:cg_image];
    autorelease(env, new)
}

+ (id)imageNamed:(id)name { // NSString*
    // TODO: figure out whether this is actually correct in all cases
    let bundle: id = msg_class![env; NSBundle mainBundle];
    let path: id = msg![env; bundle pathForResource:name ofType:nil];
    let name_str = ns_string::to_rust_string(env, name).to_string();
    if path == nil {
        log!("Warning: [UIImage imageNamed:{:?}] => nil", name_str);
        return nil;
    }
    // TODO: find a better eviction policy
    if State::get(env).cached_images.len() > CACHE_SIZE {
        let cache = std::mem::take(&mut State::get_mut(env).cached_images);
        log_dbg!("Evicting {} images from UIImage cache.", cache.len());
        for (_, img) in cache {
            release(env, img);
        }
    }
    if !State::get(env).cached_images.contains_key(&name_str) {
        let img = msg![env; this imageWithContentsOfFile:path];
        retain(env, img);
        State::get_mut(env).cached_images.insert(name_str.clone(), img);
    }
    *State::get(env).cached_images.get(&name_str).unwrap()
}

+ (id)imageWithContentsOfFile:(id)path { // NSString*
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithContentsOfFile:path];
    autorelease(env, new)
}

+ (id)imageWithData:(id)data { // NSData*
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithData:data];
    autorelease(env, new)
}

+ (id)imageWithCGImage:(CGImageRef)cg_image
                 scale:(CGFloat)_scale
           orientation:(NSInteger)orientation {
    // We ignore scale for now (stored as 1.0).
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithCGImage:cg_image];
    // Store orientation.
    env.objc.borrow_mut::<UIImageHostObject>(new).orientation = orientation;
    autorelease(env, new)
}

+ (id)animatedImageNamed:(id)name
               duration:(f64)_duration { // NSString*
    // No animation support — return static image.
    msg_class![env; UIImage imageNamed:name]
}

+ (id)animatedImageWithImages:(id)images
                     duration:(f64)_duration { // NSArray<UIImage*>*
    // Return first frame.
    let count: u32 = msg![env; images count];
    if count == 0 { return nil; }
    msg![env; images objectAtIndex:0u32]
}

+ (id)animatedResizableImageNamed:(id)name
                        capInsets:(CGRect)_insets
                         duration:(f64)_duration {
    msg_class![env; UIImage imageNamed:name]
}

// MARK: - Init with scale / orientation

- (id)initWithCGImage:(CGImageRef)cg_image
                scale:(CGFloat)_scale
          orientation:(NSInteger)orientation {
    CGImageRetain(env, cg_image);
    let host = env.objc.borrow_mut::<UIImageHostObject>(this);
    host.cg_image    = cg_image;
    host.orientation = orientation;
    this
}

- (id)initWithData:(id)data
             scale:(CGFloat)_scale { // NSData*
    msg![env; this initWithData:data]
}

// MARK: - Resizable images

- (id)resizableImageWithCapInsets:(CGRect)_insets {
    log!("UIImage resizableImageWithCapInsets: stubbed (returning self)");
    retain(env, this)
}

- (id)resizableImageWithCapInsets:(CGRect)_insets
                    resizingMode:(NSInteger)_mode {
    log!("UIImage resizableImageWithCapInsets:resizingMode: stubbed (returning self)");
    retain(env, this)
}

- (CGRect)capInsets {
    CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { width: 0.0, height: 0.0 } }
}

- (NSInteger)resizingMode {
    0 // UIImageResizingModeTile
}

// MARK: - Rendering mode

- (id)imageWithRenderingMode:(NSInteger)_rendering_mode {
    log_dbg!("UIImage imageWithRenderingMode: stubbed (returning self)");
    retain(env, this)
}

- (NSInteger)renderingMode {
    0 // UIImageRenderingModeAutomatic
}

// MARK: - Flipped image

- (id)imageFlippedForRightToLeftLayoutDirection {
    retain(env, this)
}

- (bool)flipsForRightToLeftLayoutDirection {
    false
}

// MARK: - Image with tint (iOS 13+)

- (id)imageWithTintColor:(id)_color { // UIColor*
    retain(env, this)
}

- (id)imageWithTintColor:(id)_color
        renderingMode:(NSInteger)_mode {
    retain(env, this)
}

// MARK: - Accessors

- (NSInteger)imageOrientation {
    env.objc.borrow::<UIImageHostObject>(this).orientation
}

- (CGFloat)scale {
    1.0
}

- (bool)isSymbolImage {
    false
}

- (id)imageAsset {
    nil
}

- (id)traitCollection {
    nil
}

- (id)configuration {
    nil
}

- (id)baselineOffsetFromBottom {
    nil
}

- (CGFloat)leftCapWidth {
    0.0
}

- (CGFloat)topCapHeight {
    0.0
}

- (id)images {
    // Non-animated — return nil (single frame).
    nil
}

- (f64)duration {
    0.0
}

- (bool)isHighDynamicRange {
    false
}

// MARK: - Drawing variants

- (())drawAtPoint:(CGPoint)point
        blendMode:(NSInteger)_blend_mode
            alpha:(CGFloat)alpha {
    let context = UIGraphicsGetCurrentContext(env);
    if context == nil { return; }
    
    // TODO: apply blend mode and alpha properly.
    // (Точно так же, как и в drawInRect:blendMode:alpha:)
    let image = env.objc.borrow::<UIImageHostObject>(this).cg_image;
    
    // Высчитываем CGRect на основе переданной CGPoint и размеров картинки,
    // как это сделано в обычном drawAtPoint:
    let rect = CGRect {
        origin: point,
        size: CGSize {
            width: CGImageGetWidth(env, image) as CGFloat,
            height: CGImageGetHeight(env, image) as CGFloat,
        }
    };
    
    // Отрисовываем
    CGContextDrawImage(env, context, rect, image);
            }
    
- (())drawInRect:(CGRect)rect
       blendMode:(NSInteger)_blend_mode
           alpha:(CGFloat)alpha {
    let context = UIGraphicsGetCurrentContext(env);
    if context == nil { return; }
    // TODO: apply blend mode and alpha properly.
    let image = env.objc.borrow::<UIImageHostObject>(this).cg_image;
    CGContextDrawImage(env, context, rect, image);
}

- (())drawAsPatternInRect:(CGRect)_rect {
    log!("UIImage drawAsPatternInRect: stubbed (not rendered)");
}

// MARK: - NSCoding

- (id)initWithCoder:(id)coder {
    // Some NIB archives embed UIImage data.
    let key = get_static_str(env, "UIImageData");
    let data: id = msg![env; coder decodeObjectForKey:key];
    if data != nil {
        return msg![env; this initWithData:data];
    }
    let key = get_static_str(env, "UIResourceName");
    let name: id = msg![env; coder decodeObjectForKey:key];
    if name != nil {
        release(env, this);
        let img = msg_class![env; UIImage imageNamed:name];
        return retain(env, img);
    }
    this
}

// MARK: - Equality / description

- (bool)isEqual:(id)other {
    if this == other { return true; }
    if other == nil  { return false; }
    let a = env.objc.borrow::<UIImageHostObject>(this).cg_image;
    let b = env.objc.borrow::<UIImageHostObject>(other).cg_image;
    a == b
}

- (id)description {
    let host  = env.objc.borrow::<UIImageHostObject>(this);
    let image = host.cg_image;
    drop(host);
    let (w, h) = if image != nil {
        cg_image::borrow_image(&env.objc, image).dimensions()
    } else {
        (0, 0)
    };
    let s = format!("<UIImage: {:?} size={{{}×{}}}>", this, w, h);
    let ns = crate::frameworks::foundation::ns_string::from_rust_string(env, s);
    autorelease(env, ns)
}

// MARK: - NSCopying

- (id)copyWithZone:(NSZonePtr)_zone {
    retain(env, this)
}

- (())dealloc {
    let &UIImageHostObject { cg_image, .. } = env.objc.borrow(this);
    CGImageRelease(env, cg_image);

    env.objc.dealloc_object(this, &mut env.mem)
}

- (id)initWithCGImage:(CGImageRef)cg_image {
    CGImageRetain(env, cg_image);
    env.objc.borrow_mut::<UIImageHostObject>(this).cg_image = cg_image;
    this
}

- (id)initWithContentsOfFile:(id)path { // NSString*
    if path == nil {
        return nil;
    }
    let path = ns_string::to_rust_string(env, path); // TODO: avoid copy
    let Ok(bytes) = env.fs.read(GuestPath::new(&path)) else {
        log!("Warning: couldn't read image file at {:?}, returning nil", path);
        release(env, this);
        return nil;
    };
    let Ok(image) = Image::from_bytes(&bytes) else {
        log!("Warning: couldn't decode image file at {:?}, returning nil", path);
        release(env, this);
        return nil;
    };
    let cg_image = cg_image::from_image(env, image);
    env.objc.borrow_mut::<UIImageHostObject>(this).cg_image = cg_image;
    this
}

- (id)initWithData:(id)data { // NSData*
    let slice = ns_data::to_rust_slice(env, data);
    let Ok(image) = Image::from_bytes(slice) else {
        log!("Warning: couldn't decode image from NSData, returning nil");
        release(env, this);
        return nil;
    };
    let cg_image = cg_image::from_image(env, image);
    env.objc.borrow_mut::<UIImageHostObject>(this).cg_image = cg_image;
    this
}

- (id)stretchableImageWithLeftCapWidth:(NSInteger)_leftCapWidth
                          topCapHeight:(NSInteger)_topCapHeight {
    log!("TODO: properly support stretchableImageWithLeftCapWidth:topCapHeight:");
    retain(env, this)
}

// TODO: more init methods
// TODO: more accessors

- (CGImageRef)CGImage {
    env.objc.borrow::<UIImageHostObject>(this).cg_image
}

// TODO: should have UIImageOrientation type
- (NSInteger)imageOrientation {
    // FIXME: load image orientation info from file?
    0 // UIImageOrientationUp
}

- (CGSize)size {
    let image = env.objc.borrow::<UIImageHostObject>(this).cg_image;
    let (width, height) = cg_image::borrow_image(&env.objc, image).dimensions();
    CGSize {
        width: width as _,
        height: height as _,
    }
}

- (CGFloat)scale {
    // TODO: support other scales, such as @2x
    1.0
}

- (())drawInRect:(CGRect)rect {
    let context = UIGraphicsGetCurrentContext(env);
    let image = env.objc.borrow::<UIImageHostObject>(this).cg_image;
    CGContextDrawImage(env, context, rect, image);
}

- (())drawAtPoint:(CGPoint)point {
    let context = UIGraphicsGetCurrentContext(env);
    let image = env.objc.borrow::<UIImageHostObject>(this).cg_image;
    let rect = CGRect {
        origin: point,
        size: CGSize {
            width: CGImageGetWidth(env, image) as CGFloat,
            height: CGImageGetHeight(env, image) as CGFloat,
        }
    };
    CGContextDrawImage(env, context, rect, image);
}

@end

// Undocumented class used in NIBs
// TODO: It's not clear _why_ placeholder is needed?
@implementation UIImageNibPlaceholder: UIImage

// NSCoding implementation
- (id)initWithCoder:(id)coder {
    release(env, this);
    // TODO: decode other attributes
    let key_ns_string = get_static_str(env, "UIResourceName");
    let resource_name: id = msg![env; coder decodeObjectForKey:key_ns_string];
    let res = msg_class![env; UIImage imageNamed:resource_name];
    // TODO: It is not clear if we need to additionally retain here?
    retain(env, res)
}

@end

};
fn UIImageWriteToSavedPhotosAlbum(
    env: &mut Environment,
    image: id,
    completionTarget: id,
    completionSelector: SEL,
    contextInfo: id,
) {
    log_dbg!(
        "UIImageWriteToSavedPhotosAlbum image:{:?} completionTarget:{:?} completionSelector:{:?}",
        image,
        completionTarget,
        completionSelector,
    );
    let cg_image = if image != nil {
        msg![env; image CGImage]
    } else {
        nil
    };
    if cg_image != nil {
        write_to_saved_photos_album_inner(env, cg_image);
    } else {
        log!("UIImageWriteToSavedPhotosAlbum: image has no CGImage, skipping save");
    }

    // Call completion handler
    if completionTarget != nil && !completionSelector.is_null() {
        let _: () = msg_send(
            env,
            (
                completionTarget,
                completionSelector,
                image,
                nil,
                contextInfo,
            ),
        );
    }
}

/// Helper function to simplify UIImageWriteToSavedPhotosAlbum
/// Allows several failure points to do an early return
fn write_to_saved_photos_album_inner(env: &mut Environment, cg_image: CGImageRef) {
    let img = cg_image::borrow_image(&env.objc, cg_image);
    let (w_u32, h_u32) = img.dimensions();

    let w = w_u32 as i32;
    let h = h_u32 as i32;
    let rgba: &[u8] = img.pixels();
    let stride = (w_u32 as usize) * 4;

    let mut png_data: Vec<u8> = Vec::new();
    let ctx_ptr: *mut std::ffi::c_void = (&mut png_data as *mut Vec<u8>).cast();

    let ok = img.write_png_image(ctx_ptr, w, h, rgba, stride as i32);
    if ok == 0 {
        log!("Warning: UIImageWriteToSavedPhotosAlbum: stb_image_write failed to encode PNG");
        return;
    }
    let base = crate::paths::user_data_base_path();
    let album_dir = base.join(crate::paths::PHOTO_ALBUM_DIR);
    if let Err(e) = std::fs::create_dir_all(&album_dir) {
        log!(
            "Warning: UIImageWriteToSavedPhotosAlbum failed to create {:?}: {:?}",
            album_dir,
            e
        );
        return;
    }
    // Find next IMG_####.PNG
    let mut max_index: u32 = 0;
    if let Ok(entries) = std::fs::read_dir(&album_dir) {
        for entry_res in entries {
            let Ok(entry) = entry_res else { continue };
            let name_os = entry.file_name();
            let Some(name) = name_os.to_str() else {
                continue;
            };

            // Accept IMG_0001.PNG / IMG_0001.png etc
            if name.starts_with("IMG_") {
                if let Some(dot_idx) = name.rfind('.') {
                    let num_str = &name[4..dot_idx];
                    if let Ok(n) = num_str.parse::<u32>() {
                        max_index = max_index.max(n);
                    }
                }
            }
        }
    }

    let next_index = max_index + 1;
    let file_name = format!("IMG_{:04}.PNG", next_index);
    let file_path = album_dir.join(file_name);

    if let Err(e) = std::fs::write(&file_path, &png_data) {
        log!(
            "Warning: UIImageWriteToSavedPhotosAlbum failed to write {:?}: {:?}",
            file_path,
            e
        );
        return;
    }
    log_dbg!(
        "UIImageWriteToSavedPhotosAlbum: wrote {:?} ({}×{})",
        file_path,
        w_u32,
        h_u32
    );
}

fn UIImagePNGRepresentation(env: &mut Environment, image: id) -> id {
    if image == nil { return nil; }
    let cg_image: CGImageRef = msg![env; image CGImage];
    if cg_image.is_null() { return nil; }
    let img = cg_image::borrow_image(&env.objc, cg_image);
    let (w, h) = img.dimensions();
    let rgba = img.pixels();
    let stride = w as usize * 4;
    let mut png_data: Vec<u8> = Vec::new();
    let ctx_ptr: *mut std::ffi::c_void = (&mut png_data as *mut Vec<u8>).cast();
    let ok = img.write_png_image(ctx_ptr, w as i32, h as i32, rgba, stride as i32);
    if ok == 0 {
        log!("UIImagePNGRepresentation: encoding failed");
        return nil;
    }
    let len = png_data.len() as crate::frameworks::foundation::NSUInteger;
    let buf: crate::mem::MutVoidPtr = env.mem.alloc(len as u32).cast();
    env.mem.bytes_at_mut(buf.cast(), len as u32).copy_from_slice(&png_data);
    msg_class![env; NSData dataWithBytesNoCopy:buf length:len]
}

fn UIImageJPEGRepresentation(
    env: &mut Environment,
    image: id,
    _compression_quality: CGFloat,
) -> id {
    // We don't have a JPEG encoder — fall back to PNG.
    log!("UIImageJPEGRepresentation: stubbed, returning PNG data");
    UIImagePNGRepresentation(env, image)
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(UIImageWriteToSavedPhotosAlbum(_, _, _, _)),
    export_c_func!(UIImagePNGRepresentation(_)),
    export_c_func!(UIImageJPEGRepresentation(_, _)),
];

