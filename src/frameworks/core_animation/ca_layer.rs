/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `CALayer`.

use crate::dyld::{ConstantExports, HostConstant};
use crate::frameworks::core_foundation::time::CFTimeInterval;
use crate::frameworks::core_graphics::cg_affine_transform::{
    CGAffineTransform, CGAffineTransformIdentity,
};
use crate::frameworks::core_graphics::cg_bitmap_context::{
    CGBitmapContextCreate, CGBitmapContextGetHeight, CGBitmapContextGetWidth,
};
use crate::frameworks::core_graphics::cg_color::{CGColorHostObject, CGColorRef};
use crate::frameworks::core_graphics::cg_color_space::CGColorSpaceCreateDeviceRGB;
use crate::frameworks::core_graphics::cg_context::{
    CGContextClearRect, CGContextRef, CGContextRelease, CGContextTranslateCTM,
};
use crate::frameworks::core_graphics::cg_image::{
    kCGImageAlphaPremultipliedLast, kCGImageByteOrder32Big,
};
use crate::frameworks::core_graphics::{CGFloat, CGPoint, CGRect, CGSize};
use crate::frameworks::foundation::ns_string::{self, to_rust_string};
use crate::mem::{GuestUSize, Ptr};
use crate::objc::{
    autorelease, id, msg, msg_class, nil, objc_classes, release, retain, todo_objc_setter,
    ClassExports, HostObject, ObjC,
};
use crate::Environment;
use std::collections::{HashMap, HashSet};

#[derive(Clone)]
pub(super) struct CALayerHostObject {
    delegate: id,
    pub(super) sublayers: Vec<id>,
    superlayer: id,
    pub(super) bounds: CGRect,
    pub(super) position: CGPoint,
    pub(super) anchor_point: CGPoint,
    pub(super) affine_transform: CGAffineTransform,
    pub(super) hidden: bool,
    pub(super) opaque: bool,
    pub(super) opacity: f32,
    pub(super) background_color: Option<CGColorHostObject>,
    pub(super) corner_radius: CGFloat,
    pub(super) border_width: CGFloat,
    pub(super) border_color: Option<CGColorHostObject>,
    pub(super) needs_display: bool,
    pub(super) needs_display_on_bounds_change: bool,
    pub(super) contents: id,
    pub(super) drawable_properties: id,
    pub(super) presented_pixels: Option<(Vec<u8>, u32, u32)>,
    pub(super) cg_context: Option<CGContextRef>,
    pub(super) gles_texture: Option<crate::gles::gles11_raw::types::GLuint>,
    pub(super) gles_texture_is_up_to_date: bool,
    pub(super) animations: HashMap<String, id>,
    pub(super) anonymous_animations: HashSet<id>,
    pub(super) name: Option<String>,
    pub(super) mask: id,
}
impl HostObject for CALayerHostObject {}

impl CALayerHostObject {
    pub(super) fn superlayer_to_layer_transform(&self) -> CGAffineTransform {
        CGAffineTransform::make_translation(-self.bounds.origin.x, -self.bounds.origin.y)
            .concat(CGAffineTransform::make_translation(
                -self.bounds.size.width * self.anchor_point.x,
                -self.bounds.size.height * self.anchor_point.y,
            ))
            .concat(self.affine_transform)
            .concat(CGAffineTransform::make_translation(
                self.position.x,
                self.position.y,
            ))
    }
}

pub const kCAFilterLinear: &str = "kCAFilterLinear";
pub const kCAFilterNearest: &str = "kCAFilterNearest";
pub const kCAFilterTrilinear: &str = "kCAFilterTrilinear";

pub const CONSTANTS: ConstantExports = &[
    ("_kCAFilterLinear", HostConstant::NSString(kCAFilterLinear)),
    ("_kCAFilterNearest", HostConstant::NSString(kCAFilterNearest)),
    ("_kCAFilterTrilinear", HostConstant::NSString(kCAFilterTrilinear)),
];

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation CALayer: NSObject

+ (id)alloc {
    let host_object = Box::new(CALayerHostObject {
        delegate: nil,
        sublayers: Vec::new(),
        superlayer: nil,
        bounds: CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize { width: 0.0, height: 0.0 }
        },
        position: CGPoint { x: 0.0, y: 0.0 },
        anchor_point: CGPoint { x: 0.5, y: 0.5 },
        affine_transform: CGAffineTransformIdentity,
        hidden: false,
        opaque: false,
        opacity: 1.0,
        background_color: None,
        corner_radius: 0.0,
        border_width: 0.0,
        border_color: None,
        needs_display: false,
        needs_display_on_bounds_change: false,
        contents: nil,
        drawable_properties: nil,
        presented_pixels: None,
        cg_context: None,
        gles_texture: None,
        gles_texture_is_up_to_date: false,
        animations: HashMap::new(),
        anonymous_animations: HashSet::new(),
        name: None,
        mask: nil,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

+ (id)layer {
    let new_layer: id = msg![env; this alloc];
    msg![env; new_layer init]
}

- (())dealloc {
    let &mut CALayerHostObject {
        drawable_properties,
        contents,
        superlayer,
        cg_context,
        mask,
        ref mut sublayers,
        ..
    } = env.objc.borrow_mut(this);
    let sublayers = std::mem::take(sublayers);

    if drawable_properties != nil { release(env, drawable_properties); }
    if contents != nil { release(env, contents); }
    if mask != nil { release(env, mask); }
    if let Some(cg_context) = cg_context { CGContextRelease(env, cg_context); }

    assert!(superlayer == nil);
    for sublayer in sublayers {
        env.objc.borrow_mut::<CALayerHostObject>(sublayer).superlayer = nil;
        release(env, sublayer);
    }

    env.objc.dealloc_object(this, &mut env.mem)
}

- (id)delegate { env.objc.borrow::<CALayerHostObject>(this).delegate }
- (())setDelegate:(id)delegate { env.objc.borrow_mut::<CALayerHostObject>(this).delegate = delegate; }

- (id)superlayer { env.objc.borrow::<CALayerHostObject>(this).superlayer }

- (())addSublayer:(id)layer {
    if layer == nil { return; }
    if env.objc.borrow::<CALayerHostObject>(layer).superlayer == this {
        () = msg![env; this bringSublayerToFront:layer];
    } else {
        retain(env, layer);
        () = msg![env; layer removeFromSuperlayer];
        env.objc.borrow_mut::<CALayerHostObject>(layer).superlayer = this;
        env.objc.borrow_mut::<CALayerHostObject>(this).sublayers.push(layer);
    }
}

- (())insertSublayer:(id)layer atIndex:(u32)idx {
    if layer == nil { return; }
    retain(env, layer);
    () = msg![env; layer removeFromSuperlayer];
    env.objc.borrow_mut::<CALayerHostObject>(layer).superlayer = this;
    let CALayerHostObject { ref mut sublayers, .. } = env.objc.borrow_mut(this);
    sublayers.insert(idx.try_into().unwrap(), layer);
}

- (())insertSublayer:(id)layer below:(id)sibling {
    if layer == nil { return; }
    retain(env, layer);
    () = msg![env; layer removeFromSuperlayer];
    env.objc.borrow_mut::<CALayerHostObject>(layer).superlayer = this;
    let CALayerHostObject { ref mut sublayers, .. } = env.objc.borrow_mut(this);
    let idx = sublayers.iter().position(|&sublayer| sublayer == sibling).unwrap();
    sublayers.insert(idx, layer);
}

- (())replaceSublayer:(id)old_layer with:(id)new_layer {
    if old_layer == nil || new_layer == nil || old_layer == new_layer { return; }
    let old_idx = {
        let host = env.objc.borrow::<CALayerHostObject>(this);
        host.sublayers.iter().position(|&x| x == old_layer)
    };
    if old_idx.is_some() {
        retain(env, new_layer);
        () = msg![env; new_layer removeFromSuperlayer];
        let host = env.objc.borrow_mut::<CALayerHostObject>(this);
        if let Some(actual_idx) = host.sublayers.iter().position(|&x| x == old_layer) {
            host.sublayers[actual_idx] = new_layer;
            env.objc.borrow_mut::<CALayerHostObject>(new_layer).superlayer = this;
            env.objc.borrow_mut::<CALayerHostObject>(old_layer).superlayer = nil;
            release(env, old_layer);
        } else {
            release(env, new_layer);
        }
    }
}

- (())removeFromSuperlayer {
    let CALayerHostObject { ref mut superlayer, .. } = env.objc.borrow_mut(this);
    let superlayer = std::mem::take(superlayer);
    if superlayer == nil { return; }
    let CALayerHostObject { ref mut sublayers, .. } = env.objc.borrow_mut(superlayer);
    let idx = sublayers.iter().position(|&sublayer| sublayer == this).unwrap();
    let sublayer = sublayers.remove(idx);
    assert!(sublayer == this);
    release(env, this);
}

- (CGRect)bounds { env.objc.borrow::<CALayerHostObject>(this).bounds }
- (())setBounds:(CGRect)bounds {
    let host_object = env.objc.borrow_mut::<CALayerHostObject>(this);
    host_object.bounds = bounds;
    if host_object.needs_display_on_bounds_change {
        () = msg![env; this setNeedsDisplay];
    }
}
- (CGPoint)position { env.objc.borrow::<CALayerHostObject>(this).position }
- (())setPosition:(CGPoint)position { env.objc.borrow_mut::<CALayerHostObject>(this).position = position; }
- (CGPoint)anchorPoint { env.objc.borrow::<CALayerHostObject>(this).anchor_point }
- (())setAnchorPoint:(CGPoint)anchor_point { env.objc.borrow_mut::<CALayerHostObject>(this).anchor_point = anchor_point; }
- (CGAffineTransform)affineTransform { env.objc.borrow::<CALayerHostObject>(this).affine_transform }
- (())setAffineTransform:(CGAffineTransform)affine_transform { env.objc.borrow_mut::<CALayerHostObject>(this).affine_transform = affine_transform; }

- (CGRect)frame {
    let host_obj @ &CALayerHostObject { bounds, .. } = env.objc.borrow(this);
    host_obj.superlayer_to_layer_transform().apply_to_rect(CGRect {
        origin: CGPoint { x: bounds.origin.x, y: bounds.origin.y },
        size: bounds.size,
    })
}
- (())setFrame:(CGRect)frame {
    let CALayerHostObject { anchor_point, affine_transform, .. } = env.objc.borrow_mut(this);
    let inverse_transform = CGAffineTransform::make_translation(
        -frame.size.width * anchor_point.x,
        -frame.size.height * anchor_point.y,
    ).concat(*affine_transform).invert();

    let transformed_size = inverse_transform.apply_to_rect(CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size: frame.size
    }).size;
    let transformed_offset = inverse_transform.apply_to_point(CGPoint { x: 0.0, y: 0.0 });

    let new_position = CGPoint {
        x: frame.origin.x + transformed_offset.x,
        y: frame.origin.y + transformed_offset.y,
    };
    () = msg![env; this setPosition:new_position];
    let new_bounds = CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size: transformed_size,
    };
    () = msg![env; this setBounds:new_bounds];
}

- (bool)isHidden { env.objc.borrow::<CALayerHostObject>(this).hidden }
- (())setHidden:(bool)hidden { env.objc.borrow_mut::<CALayerHostObject>(this).hidden = hidden; }

- (bool)isOpaque { env.objc.borrow::<CALayerHostObject>(this).opaque }
- (())setOpaque:(bool)opaque { env.objc.borrow_mut::<CALayerHostObject>(this).opaque = opaque; }

- (f32)opacity { env.objc.borrow::<CALayerHostObject>(this).opacity }
- (())setOpacity:(f32)opacity { env.objc.borrow_mut::<CALayerHostObject>(this).opacity = opacity; }

- (CGColorRef)backgroundColor {
    if let Some(bg_color) = env.objc.borrow::<CALayerHostObject>(this).background_color {
        let class = env.objc.get_known_class("_touchHLE_CGColor", &mut env.mem);
        let obj = env.objc.alloc_object(class, Box::new(bg_color), &mut env.mem);
        autorelease(env, obj)
    } else { nil }
}
- (())setBackgroundColor:(CGColorRef)new_color {
    let new_color = if new_color == nil { None } else { Some(*env.objc.borrow::<CGColorHostObject>(new_color)) };
    env.objc.borrow_mut::<CALayerHostObject>(this).background_color = new_color;
}

- (CGFloat)cornerRadius { env.objc.borrow::<CALayerHostObject>(this).corner_radius }
- (())setCornerRadius:(CGFloat)corner_radius { env.objc.borrow_mut::<CALayerHostObject>(this).corner_radius = corner_radius; }

- (CGFloat)borderWidth { env.objc.borrow::<CALayerHostObject>(this).border_width }
- (())setBorderWidth:(CGFloat)border_width { env.objc.borrow_mut::<CALayerHostObject>(this).border_width = border_width; }

- (CGColorRef)borderColor {
    if let Some(border_color) = env.objc.borrow::<CALayerHostObject>(this).border_color {
        let class = env.objc.get_known_class("_touchHLE_CGColor", &mut env.mem);
        let obj = env.objc.alloc_object(class, Box::new(border_color), &mut env.mem);
        autorelease(env, obj)
    } else { nil }
}
- (())setBorderColor:(CGColorRef)new_color {
    let new_color = if new_color == nil { None } else { Some(*env.objc.borrow::<CGColorHostObject>(new_color)) };
    env.objc.borrow_mut::<CALayerHostObject>(this).border_color = new_color;
}

- (bool)needsDisplay { env.objc.borrow::<CALayerHostObject>(this).needs_display }
- (())setNeedsDisplay { env.objc.borrow_mut::<CALayerHostObject>(this).needs_display = true; }

- (bool)needsDisplayOnBoundsChange { env.objc.borrow::<CALayerHostObject>(this).needs_display_on_bounds_change }
- (())setNeedsDisplayOnBoundsChange:(bool)value { env.objc.borrow_mut::<CALayerHostObject>(this).needs_display_on_bounds_change = value; }

- (())displayIfNeeded {
    let &mut CALayerHostObject {
        ref mut needs_display,
        delegate,
        ..
    } = env.objc.borrow_mut(this);
    if !std::mem::take(needs_display) { return; }
    if delegate == nil { return; }

    let delegate_class = ObjC::read_isa(delegate, &env.mem);
    if env.objc.class_has_method_named(delegate_class, "displayLayer:") {
        () = msg![env; delegate displayLayer:this];
        return;
    }

    let &mut CALayerHostObject {
        cg_context,
        ref mut gles_texture_is_up_to_date,
        bounds: CGRect { origin, size },
        ..
    } = env.objc.borrow_mut(this);

    *gles_texture_is_up_to_date = false;

    let int_width = size.width.round() as GuestUSize;
    let int_height = size.height.round() as GuestUSize;

    // --- ФИКС КРАША 0x0 ---
    if int_width == 0 || int_height == 0 {
        return;
    }

    let need_new_context = cg_context.is_none_or(|existing|
            CGBitmapContextGetWidth(env, existing) != int_width ||
            CGBitmapContextGetHeight(env, existing) != int_height
    );
    let cg_context = if need_new_context {
        if let Some(old_context) = cg_context { CGContextRelease(env, old_context); }
        let color_space = CGColorSpaceCreateDeviceRGB(env);
        let cg_context = CGBitmapContextCreate(
            env, Ptr::null(), int_width, int_height, 8,
            int_width.checked_mul(4).unwrap(), color_space,
            kCGImageByteOrder32Big | kCGImageAlphaPremultipliedLast
        );
        env.objc.borrow_mut::<CALayerHostObject>(this).cg_context = Some(cg_context);
        cg_context
    } else {
        cg_context.unwrap()
    };

    CGContextTranslateCTM(env, cg_context, -origin.x, -origin.y);
    CGContextClearRect(env, cg_context, CGRect { origin, size });
    () = msg![env; delegate drawLayer:this inContext:cg_context];
    CGContextTranslateCTM(env, cg_context, origin.x, origin.y);
}

- (id)contents { env.objc.borrow::<CALayerHostObject>(this).contents }
- (())setContents:(id)new_contents {
    let host_obj = env.objc.borrow_mut::<CALayerHostObject>(this);
    host_obj.gles_texture_is_up_to_date = false;
    let old_contents = std::mem::replace(&mut host_obj.contents, new_contents);
    retain(env, new_contents);
    release(env, old_contents);
}

- (id)name {
    if let Some(ref name) = env.objc.borrow::<CALayerHostObject>(this).name {
        let string_id = ns_string::from_rust_string(env, name.clone());
        autorelease(env, string_id)
    } else { nil }
}

- (())setName:(id)name {
    let name_str = if name != nil { Some(ns_string::to_rust_string(env, name).into_owned()) } else { None };
    env.objc.borrow_mut::<CALayerHostObject>(this).name = name_str;
}

- (id)mask { env.objc.borrow::<CALayerHostObject>(this).mask }

- (())setMask:(id)mask {
    let old_mask = env.objc.borrow::<CALayerHostObject>(this).mask;
    if mask != old_mask {
        if mask != nil { retain(env, mask); }
        env.objc.borrow_mut::<CALayerHostObject>(this).mask = mask;
        if old_mask != nil { release(env, old_mask); }
    }
}

- (())setEdgeAntialiasingMask:(u32)mask { todo_objc_setter!(this, mask); }
- (())setMagnificationFilter:(id)filter { todo_objc_setter!(this, ns_string::to_rust_string(env, filter)); }
- (())setMinificationFilter:(id)filter { todo_objc_setter!(this, ns_string::to_rust_string(env, filter)); }

- (bool)containsPoint:(CGPoint)point {
    let bounds: CGRect = msg![env; this bounds];
    let x_range = bounds.origin.x..(bounds.origin.x + bounds.size.width);
    let y_range = bounds.origin.y..(bounds.origin.y + bounds.size.height);
    let CGPoint {x, y} = point;
    x_range.contains(&x) && y_range.contains(&y)
}

- (CGPoint)convertPoint:(CGPoint)point fromLayer:(id)other {
    if this == other { return point; }
    transform_for_conversion(env, this, other).apply_to_point(point)
}
- (CGPoint)convertPoint:(CGPoint)point toLayer:(id)other {
    if this == other { return point; }
    transform_for_conversion(env, other, this).apply_to_point(point)
}
- (CGRect)convertRect:(CGRect)rect fromLayer:(id)other {
    if this == other { return rect; }
    transform_for_conversion(env, this, other).apply_to_rect(rect)
}
- (CGRect)convertRect:(CGRect)rect toLayer:(id)other {
    if this == other { return rect; }
    transform_for_conversion(env, other, this).apply_to_rect(rect)
}

- (())addAnimation:(id)anim forKey:(id)key {
    let duration: CFTimeInterval = msg![env; anim duration];
    if duration == 0.0 {
        let duration: CFTimeInterval = msg_class![env; CATransaction animationDuration];
        () = msg![env; anim setDuration:duration];
    }
    if key == nil {
        let inserted = env.objc.borrow_mut::<CALayerHostObject>(this).anonymous_animations.insert(anim);
        assert!(inserted);
    } else {
        let key_string = to_rust_string(env, key);
        env.objc.borrow_mut::<CALayerHostObject>(this).animations.insert(key_string.to_string(), anim);
    }
    retain(env, anim);
}

- (())removeAnimationForKey:(id)key {
    let key_string = to_rust_string(env, key);
    if let Some(anim) = env.objc.borrow_mut::<CALayerHostObject>(this).animations.remove(&*key_string) {
        release(env, anim);
    };
}

@end

};

pub fn remove_anonymous_animation(env: &mut Environment, layer: id, animation: id) {
    let removed = env.objc.borrow_mut::<CALayerHostObject>(layer).anonymous_animations.remove(&animation);
    assert!(removed);
    release(env, animation);
}

fn transform_for_conversion(env: &mut Environment, this: id, other: id) -> CGAffineTransform {
    let need_common_ancestor = this != nil && other != nil;
    assert!(!(this == nil && other == nil));

    let mut this_map = HashMap::from([(this, CGAffineTransformIdentity)]);
    let mut other_map = HashMap::from([(other, CGAffineTransformIdentity)]);
    let mut this_superlayer = this;
    let mut this_transform = CGAffineTransformIdentity;
    let mut other_superlayer = other;
    let mut other_transform = CGAffineTransformIdentity;
    
    let (common_ancestor, this_transform, other_transform) = loop {
        if this_superlayer != nil {
            let this_hostobj: &CALayerHostObject = env.objc.borrow(this_superlayer);
            let next = this_hostobj.superlayer;
            let next_transform = this_transform.concat(this_hostobj.superlayer_to_layer_transform());
            if need_common_ancestor && next != nil {
                if let Some(&other_transform) = other_map.get(&next) { break (next, next_transform, other_transform); }
                this_map.insert(next, next_transform);
            }
            this_superlayer = next;
            this_transform = next_transform;
        }

        if other_superlayer != nil {
            let other_hostobj: &CALayerHostObject = env.objc.borrow(other_superlayer);
            let next = other_hostobj.superlayer;
            let next_transform = other_transform.concat(other_hostobj.superlayer_to_layer_transform());
            if need_common_ancestor && next != nil {
                if let Some(&this_transform) = this_map.get(&next) { break (next, this_transform, next_transform); }
                other_map.insert(next, next_transform);
            }
            other_superlayer = next;
            other_transform = next_transform;
        }

        if this_superlayer == nil && other_superlayer == nil {
            if need_common_ancestor { panic!("Layers {this:?} and {other:?} have no common ancestor!"); } 
            else { break (nil, this_transform, other_transform); }
        }
    };

    assert!((common_ancestor == nil) != need_common_ancestor);
    other_transform.concat(this_transform.invert())
}

