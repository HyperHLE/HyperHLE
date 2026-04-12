/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `CGContext.h`

use super::cg_affine_transform::CGAffineTransform;
use super::cg_image::CGImageRef;
use super::{cg_bitmap_context, cg_color, CGFloat, CGPoint, CGRect};
use crate::dyld::{export_c_func, FunctionExports};
use crate::frameworks::core_foundation::{CFRelease, CFRetain, CFTypeRef};
use crate::frameworks::core_graphics::cg_bitmap_context::{
    CGBitmapContextGetHeight, CGBitmapContextGetWidth,
};
use crate::frameworks::core_graphics::cg_color::CGColorRef;
use crate::frameworks::core_graphics::cg_geometry::CGPointZero;
use crate::objc::{objc_classes, ClassExports, HostObject};
use crate::Environment;

type CGInterpolationQuality = i32;

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

// CGContext seems to be a CFType-based type, but in our implementation those
// are just Objective-C types, so we need a class for it, but its name is not
// visible anywhere.
@implementation _touchHLE_CGContext: NSObject

- (())dealloc {
    let host_obj = env.objc.borrow::<CGContextHostObject>(this);
    let CGContextSubclass::CGBitmapContext(bitmap_data) = host_obj.subclass;
    if bitmap_data.data_is_owned {
        env.mem.free(bitmap_data.data);
    }

    env.objc.dealloc_object(this, &mut env.mem)
}

@end

};

pub(super) struct CGContextHostObject {
    pub(super) subclass: CGContextSubclass,
    pub(super) rgb_fill_color:   (CGFloat, CGFloat, CGFloat, CGFloat),
    pub(super) rgb_stroke_color: (CGFloat, CGFloat, CGFloat, CGFloat),
    pub(super) alpha:            CGFloat,
    pub(super) line_width:       CGFloat,
    pub(super) line_cap:         i32,
    pub(super) line_join:        i32,
    pub(super) miter_limit:      CGFloat,
    pub(super) flatness:         CGFloat,
    pub(super) blend_mode:       i32,
    pub(super) transform: CGAffineTransform,
    /// (fill, stroke, alpha, line_width, line_cap, line_join, miter_limit,
    ///  flatness, blend_mode, transform)
    pub(super) state_stack: Vec<CGContextState>,
    // Path accumulator (points only — no real path rendering yet).
    pub(super) path_points: Vec<CGPoint>,
}
impl HostObject for CGContextHostObject {}

#[derive(Clone)]
pub(super) struct CGContextState {
    pub fill_color:   (CGFloat, CGFloat, CGFloat, CGFloat),
    pub stroke_color: (CGFloat, CGFloat, CGFloat, CGFloat),
    pub alpha:        CGFloat,
    pub line_width:   CGFloat,
    pub line_cap:     i32,
    pub line_join:    i32,
    pub miter_limit:  CGFloat,
    pub flatness:     CGFloat,
    pub blend_mode:   i32,
    pub transform:    CGAffineTransform,
}

pub(super) enum CGContextSubclass {
    CGBitmapContext(cg_bitmap_context::CGBitmapContextData),
}

pub type CGContextRef = CFTypeRef;

pub fn CGContextRelease(env: &mut Environment, c: CGContextRef) {
    if !c.is_null() {
        CFRelease(env, c);
    }
}
pub fn CGContextRetain(env: &mut Environment, c: CGContextRef) -> CGContextRef {
    if !c.is_null() {
        CFRetain(env, c)
    } else {
        c
    }
}

fn CGContextSetFillColorWithColor(env: &mut Environment, context: CGContextRef, color: CGColorRef) {
    let (r, g, b, a) = cg_color::to_rgba(&env.objc, color);
    CGContextSetRGBFillColor(env, context, r, g, b, a)
}

pub fn CGContextSetRGBFillColor(
    env: &mut Environment,
    context: CGContextRef,
    red: CGFloat,
    green: CGFloat,
    blue: CGFloat,
    alpha: CGFloat,
) {
    let color = (red, green, blue, alpha);
    env.objc
        .borrow_mut::<CGContextHostObject>(context)
        .rgb_fill_color = color;
}

pub fn CGContextSetRGBStrokeColor(
    env: &mut Environment,
    context: CGContextRef,
    red: CGFloat,
    green: CGFloat,
    blue: CGFloat,
    alpha: CGFloat,
) {
    if context.is_null() {
        return;
    }
    // Пишем напрямую в поле структуры через borrow_mut
    env.objc.borrow_mut::<CGContextHostObject>(context).rgb_stroke_color = (red, green, blue, alpha);
}

// MARK: - Stroke colour helpers

fn CGContextSetStrokeColorWithColor(
    env: &mut Environment,
    context: CGContextRef,
    color: CGColorRef,
) {
    if context.is_null() { return; }
    let (r, g, b, a) = cg_color::to_rgba(&env.objc, color);
    CGContextSetRGBStrokeColor(env, context, r, g, b, a);
}

fn CGContextSetGrayStrokeColor(
    env: &mut Environment,
    context: CGContextRef,
    gray: CGFloat,
    alpha: CGFloat,
) {
    CGContextSetRGBStrokeColor(env, context, gray, gray, gray, alpha);
}

// MARK: - Alpha

fn CGContextSetAlpha(env: &mut Environment, context: CGContextRef, alpha: CGFloat) {
    if context.is_null() { return; }
    env.objc.borrow_mut::<CGContextHostObject>(context).alpha = alpha.clamp(0.0, 1.0);
}

// MARK: - Line style

fn CGContextSetLineWidth(env: &mut Environment, context: CGContextRef, width: CGFloat) {
    if context.is_null() { return; }
    env.objc.borrow_mut::<CGContextHostObject>(context).line_width = width;
}

fn CGContextSetLineCap(env: &mut Environment, context: CGContextRef, cap: i32) {
    if context.is_null() { return; }
    env.objc.borrow_mut::<CGContextHostObject>(context).line_cap = cap;
}

fn CGContextSetLineJoin(env: &mut Environment, context: CGContextRef, join: i32) {
    if context.is_null() { return; }
    env.objc.borrow_mut::<CGContextHostObject>(context).line_join = join;
}

fn CGContextSetMiterLimit(env: &mut Environment, context: CGContextRef, limit: CGFloat) {
    if context.is_null() { return; }
    env.objc.borrow_mut::<CGContextHostObject>(context).miter_limit = limit;
}

fn CGContextSetLineDash(
    _env: &mut Environment,
    _context: CGContextRef,
    _phase: CGFloat,
    _lengths: crate::mem::ConstPtr<CGFloat>,
    _count: usize,
) {
    // No dash rendering — stub.
}

fn CGContextSetFlatness(env: &mut Environment, context: CGContextRef, flatness: CGFloat) {
    if context.is_null() { return; }
    env.objc.borrow_mut::<CGContextHostObject>(context).flatness = flatness;
}

// MARK: - Blend mode / shadow

fn CGContextSetBlendMode(env: &mut Environment, context: CGContextRef, mode: i32) {
    if context.is_null() { return; }
    env.objc.borrow_mut::<CGContextHostObject>(context).blend_mode = mode;
}

fn CGContextSetShadow(
    _env: &mut Environment,
    _context: CGContextRef,
    _offset: super::CGSize,
    _blur: CGFloat,
) {
    log!("CGContextSetShadow: stubbed");
}

fn CGContextSetShadowWithColor(
    _env: &mut Environment,
    _context: CGContextRef,
    _offset: super::CGSize,
    _blur: CGFloat,
    _color: CGColorRef,
) {
    log!("CGContextSetShadowWithColor: stubbed");
}

// MARK: - Stroking rects / ellipses

pub fn CGContextStrokeRect(env: &mut Environment, context: CGContextRef, rect: CGRect) {
    if context.is_null() { return; }
    let lw = env.objc.borrow::<CGContextHostObject>(context).line_width;
    CGContextStrokeRectWithWidth(env, context, rect, lw);
}

pub fn CGContextStrokeRectWithWidth(
    env: &mut Environment,
    context: CGContextRef,
    rect: CGRect,
    width: CGFloat,
) {
    if context.is_null() { return; }
    let (r, g, b, a) = env.objc.borrow::<CGContextHostObject>(context).rgb_stroke_color;
    // Draw four filled thin rects forming the border.
    let hw = width / 2.0;
    let CGRect { origin, size } = rect;

    // Top, bottom, left, right bands.
    let top    = CGRect { origin: CGPoint { x: origin.x, y: origin.y },
                          size: super::CGSize { width: size.width, height: width } };
    let bottom = CGRect { origin: CGPoint { x: origin.x, y: origin.y + size.height - width },
                          size: super::CGSize { width: size.width, height: width } };
    let left   = CGRect { origin: CGPoint { x: origin.x, y: origin.y },
                          size: super::CGSize { width, height: size.height } };
    let right  = CGRect { origin: CGPoint { x: origin.x + size.width - width, y: origin.y },
                          size: super::CGSize { width, height: size.height } };

    // Temporarily set fill to stroke colour.
    let saved_fill = env.objc.borrow::<CGContextHostObject>(context).rgb_fill_color;
    CGContextSetRGBFillColor(env, context, r, g, b, a);
    for band in [top, bottom, left, right] {
        cg_bitmap_context::fill_rect(env, context, band, false);
    }
    env.objc.borrow_mut::<CGContextHostObject>(context).rgb_fill_color = saved_fill;
}

fn CGContextStrokeEllipseInRect(env: &mut Environment, context: CGContextRef, rect: CGRect) {
    // Approximate with stroking the bounding rect for now.
    log_dbg!("CGContextStrokeEllipseInRect: approximated as stroke rect");
    CGContextStrokeRect(env, context, rect);
}

pub fn CGContextFillEllipseInRect(env: &mut Environment, context: CGContextRef, rect: CGRect) {
    if context.is_null() { return; }
    log_dbg!("CGContextFillEllipseInRect: approximated as fill rect");
    cg_bitmap_context::fill_rect(env, context, rect, false);
}

// MARK: - Path construction (accumulator only — no real rasterisation)

fn CGContextBeginPath(env: &mut Environment, context: CGContextRef) {
    if context.is_null() { return; }
    env.objc.borrow_mut::<CGContextHostObject>(context).path_points.clear();
}

fn CGContextMoveToPoint(env: &mut Environment, context: CGContextRef, x: CGFloat, y: CGFloat) {
    if context.is_null() { return; }
    env.objc.borrow_mut::<CGContextHostObject>(context)
        .path_points.push(CGPoint { x, y });
}

fn CGContextAddLineToPoint(env: &mut Environment, context: CGContextRef, x: CGFloat, y: CGFloat) {
    if context.is_null() { return; }
    env.objc.borrow_mut::<CGContextHostObject>(context)
        .path_points.push(CGPoint { x, y });
}

fn CGContextAddLines(
    env: &mut Environment,
    context: CGContextRef,
    points: crate::mem::ConstPtr<CGPoint>,
    count: usize,
) {
    if context.is_null() { return; }
    for i in 0..count as u32 {
        let p = env.mem.read(points + i);
        env.objc.borrow_mut::<CGContextHostObject>(context).path_points.push(p);
    }
}

fn CGContextAddRect(env: &mut Environment, context: CGContextRef, rect: CGRect) {
    if context.is_null() { return; }
    let o = rect.origin;
    let s = rect.size;
    let pts = [
        CGPoint { x: o.x,          y: o.y },
        CGPoint { x: o.x + s.width, y: o.y },
        CGPoint { x: o.x + s.width, y: o.y + s.height },
        CGPoint { x: o.x,          y: o.y + s.height },
    ];
    for p in pts {
        env.objc.borrow_mut::<CGContextHostObject>(context).path_points.push(p);
    }
}

fn CGContextAddRects(
    env: &mut Environment,
    context: CGContextRef,
    rects: crate::mem::ConstPtr<CGRect>,
    count: usize,
) {
    for i in 0..count as u32 {
        let r = env.mem.read(rects + i);
        CGContextAddRect(env, context, r);
    }
}

fn CGContextAddEllipseInRect(env: &mut Environment, context: CGContextRef, rect: CGRect) {
    // Approximate with 4 points on the ellipse boundary.
    let cx = rect.origin.x + rect.size.width  * 0.5;
    let cy = rect.origin.y + rect.size.height * 0.5;
    let rx = rect.size.width  * 0.5;
    let ry = rect.size.height * 0.5;
    if context.is_null() { return; }
    let pts = [
        CGPoint { x: cx + rx, y: cy },
        CGPoint { x: cx,      y: cy + ry },
        CGPoint { x: cx - rx, y: cy },
        CGPoint { x: cx,      y: cy - ry },
    ];
    for p in pts {
        env.objc.borrow_mut::<CGContextHostObject>(context).path_points.push(p);
    }
}

fn CGContextAddArc(
    env: &mut Environment,
    context: CGContextRef,
    x: CGFloat, y: CGFloat,
    radius: CGFloat,
    start_angle: CGFloat,
    end_angle: CGFloat,
    clockwise: i32,
) {
    // Store start/end points only.
    if context.is_null() { return; }
    let p0 = CGPoint { x: x + radius * start_angle.cos(), y: y + radius * start_angle.sin() };
    let p1 = CGPoint { x: x + radius * end_angle.cos(),   y: y + radius * end_angle.sin() };
    let host = env.objc.borrow_mut::<CGContextHostObject>(context);
    host.path_points.push(p0);
    host.path_points.push(p1);
}

fn CGContextAddArcToPoint(
    env: &mut Environment,
    context: CGContextRef,
    x1: CGFloat, y1: CGFloat,
    x2: CGFloat, y2: CGFloat,
    _radius: CGFloat,
) {
    if context.is_null() { return; }
    let host = env.objc.borrow_mut::<CGContextHostObject>(context);
    host.path_points.push(CGPoint { x: x1, y: y1 });
    host.path_points.push(CGPoint { x: x2, y: y2 });
}

fn CGContextAddCurveToPoint(
    env: &mut Environment,
    context: CGContextRef,
    _cp1x: CGFloat, _cp1y: CGFloat,
    _cp2x: CGFloat, _cp2y: CGFloat,
    x: CGFloat, y: CGFloat,
) {
    if context.is_null() { return; }
    env.objc.borrow_mut::<CGContextHostObject>(context)
        .path_points.push(CGPoint { x, y });
}

fn CGContextAddQuadCurveToPoint(
    env: &mut Environment,
    context: CGContextRef,
    _cpx: CGFloat, _cpy: CGFloat,
    x: CGFloat, y: CGFloat,
) {
    if context.is_null() { return; }
    env.objc.borrow_mut::<CGContextHostObject>(context)
        .path_points.push(CGPoint { x, y });
}

fn CGContextClosePath(env: &mut Environment, context: CGContextRef) {
    if context.is_null() { return; }
    // Close by adding the first point again.
    let first = env.objc.borrow::<CGContextHostObject>(context).path_points.first().copied();
    if let Some(p) = first {
        env.objc.borrow_mut::<CGContextHostObject>(context).path_points.push(p);
    }
}

// MARK: - Path drawing

fn CGContextDrawPath(env: &mut Environment, context: CGContextRef, mode: i32) {
    // mode: 0=fill, 1=eof-fill, 2=stroke, 3=fill+stroke, 4=eof-fill+stroke
    let do_fill   = matches!(mode, 0 | 1 | 3 | 4);
    let do_stroke = matches!(mode, 2 | 3 | 4);
    if context.is_null() { return; }
    if do_fill   { CGContextFillPath(env, context); }
    if do_stroke { CGContextStrokePath(env, context); }
}

fn CGContextFillPath(env: &mut Environment, context: CGContextRef) {
    if context.is_null() { return; }
    // Compute axis-aligned bounding box of path and fill it.
    let points = env.objc.borrow::<CGContextHostObject>(context).path_points.clone();
    if points.is_empty() { return; }
    let min_x = points.iter().map(|p| p.x).fold(f32::MAX, f32::min);
    let min_y = points.iter().map(|p| p.y).fold(f32::MAX, f32::min);
    let max_x = points.iter().map(|p| p.x).fold(f32::MIN, f32::max);
    let max_y = points.iter().map(|p| p.y).fold(f32::MIN, f32::max);
    let rect = CGRect {
        origin: CGPoint { x: min_x, y: min_y },
        size:   super::CGSize { width: max_x - min_x, height: max_y - min_y },
    };
    cg_bitmap_context::fill_rect(env, context, rect, false);
    env.objc.borrow_mut::<CGContextHostObject>(context).path_points.clear();
}

fn CGContextEOFillPath(env: &mut Environment, context: CGContextRef) {
    // Even-odd fill — treat same as winding fill for now.
    CGContextFillPath(env, context);
}

fn CGContextStrokePath(env: &mut Environment, context: CGContextRef) {
    if context.is_null() { return; }
    let lw = env.objc.borrow::<CGContextHostObject>(context).line_width;
    let (r, g, b, a) = env.objc.borrow::<CGContextHostObject>(context).rgb_stroke_color;
    let points = env.objc.borrow::<CGContextHostObject>(context).path_points.clone();
    let saved_fill = env.objc.borrow::<CGContextHostObject>(context).rgb_fill_color;
    CGContextSetRGBFillColor(env, context, r, g, b, a);
    // Draw a thin rect along each segment.
    for pair in points.windows(2) {
        let (p0, p1) = (pair[0], pair[1]);
        let dx = p1.x - p0.x;
        let dy = p1.y - p0.y;
        let len = (dx * dx + dy * dy).sqrt();
        if len < 0.001 { continue; }
        // Axis-aligned approximation — draw bounding box of the segment.
        let min_x = p0.x.min(p1.x) - lw * 0.5;
        let min_y = p0.y.min(p1.y) - lw * 0.5;
        let w = (p0.x - p1.x).abs().max(lw);
        let h = (p0.y - p1.y).abs().max(lw);
        let seg_rect = CGRect {
            origin: CGPoint { x: min_x, y: min_y },
            size:   super::CGSize { width: w, height: h },
        };
        cg_bitmap_context::fill_rect(env, context, seg_rect, false);
    }
    env.objc.borrow_mut::<CGContextHostObject>(context).rgb_fill_color = saved_fill;
    env.objc.borrow_mut::<CGContextHostObject>(context).path_points.clear();
}

// MARK: - Antialiasing / quality hints

fn CGContextSetShouldAntialias(_env: &mut Environment, _context: CGContextRef, _value: bool) {}
fn CGContextSetAllowsAntialiasing(_env: &mut Environment, _context: CGContextRef, _value: bool) {}
fn CGContextSetShouldSmoothFonts(_env: &mut Environment, _context: CGContextRef, _value: bool) {}

// MARK: - Flush / sync

fn CGContextFlush(_env: &mut Environment, _context: CGContextRef) {}
fn CGContextSynchronize(_env: &mut Environment, _context: CGContextRef) {}

// MARK: - Clipping

pub fn CGContextGetClipBoundingBox(env: &mut Environment, context: CGContextRef) -> CGRect {
    let w = CGBitmapContextGetWidth(env, context) as CGFloat;
    let h = CGBitmapContextGetHeight(env, context) as CGFloat;
    CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size: super::CGSize { width: w, height: h },
    }
}

fn CGContextResetClip(_env: &mut Environment, _context: CGContextRef) {
    log_dbg!("CGContextResetClip: stubbed");
}

fn CGContextClipToMask(
    _env: &mut Environment,
    _context: CGContextRef,
    _rect: CGRect,
    _mask: CGImageRef,
) {
    log!("CGContextClipToMask: stubbed");
}

fn CGContextSetGrayFillColor(
    env: &mut Environment,
    context: CGContextRef,
    gray: CGFloat,
    alpha: CGFloat,
) {
    let color = (gray, gray, gray, alpha);
    env.objc
        .borrow_mut::<CGContextHostObject>(context)
        .rgb_fill_color = color;
}

pub fn CGContextFillRect(
    env: &mut Environment,
    context: CGContextRef,
    rect: CGRect,
) {                                    // ← opens function
    if context.is_null() {             // ← opens if
        log!(
            "Warning: CGContextFillRect called with null context, skipping"
        );
        return;
    }                                  // ← closes if
    cg_bitmap_context::fill_rect(env, context, rect, /* clear: */ false);
}                      

pub fn CGContextClearRect(env: &mut Environment, context: CGContextRef, rect: CGRect) {
    cg_bitmap_context::fill_rect(env, context, rect, /* clear: */ true);
}

pub fn CGContextClipToRect(env: &mut Environment, context: CGContextRef, rect: CGRect) {
    if rect.origin == CGPointZero
        && rect.size.height == CGBitmapContextGetHeight(env, context) as f32
        && rect.size.width == CGBitmapContextGetWidth(env, context) as f32
    {
        assert!(env
            .objc
            .borrow_mut::<CGContextHostObject>(context)
            .transform
            .is_identity());
        // All good, clipping is not needed!
        return;
    }
    todo!();
}

pub fn CGContextConcatCTM(
    env: &mut Environment,
    context: CGContextRef,
    transform: CGAffineTransform,
) {
    log_dbg!("CGContextConcatCTM({:?})", transform);
    let host_obj = env.objc.borrow_mut::<CGContextHostObject>(context);
    host_obj.transform = transform.concat(host_obj.transform);
}
pub fn CGContextGetCTM(env: &mut Environment, context: CGContextRef) -> CGAffineTransform {
    let res = env.objc.borrow::<CGContextHostObject>(context).transform;
    log_dbg!("CGContextGetCTM() => {:?}", res);
    res
}
pub fn CGContextRotateCTM(env: &mut Environment, context: CGContextRef, angle: CGFloat) {
    log_dbg!("CGContextRotateCTM({:?})", angle);
    let host_obj = env.objc.borrow_mut::<CGContextHostObject>(context);
    host_obj.transform = host_obj.transform.rotate(angle);
}
pub fn CGContextScaleCTM(env: &mut Environment, context: CGContextRef, x: CGFloat, y: CGFloat) {
    log_dbg!("CGContextScaleCTM({:?})", (x, y));
    let host_obj = env.objc.borrow_mut::<CGContextHostObject>(context);
    host_obj.transform = host_obj.transform.scale(x, y);
}
pub fn CGContextTranslateCTM(
    env: &mut Environment,
    context: CGContextRef,
    tx: CGFloat,
    ty: CGFloat,
) {
    log_dbg!("CGContextTranslateCTM({:?})", (tx, ty));
    let host_obj = env.objc.borrow_mut::<CGContextHostObject>(context);
    host_obj.transform = host_obj.transform.translate(tx, ty);
}

pub fn CGContextDrawImage(
    env: &mut Environment,
    context: CGContextRef,
    rect: CGRect,
    image: CGImageRef,
) {                                    // ← opens function
    if context.is_null() {             // ← opens if
        log!(
            "Warning: CGContextDrawImage called with null context, skipping"
        );
        return;
    }                                  // ← closes if
    cg_bitmap_context::draw_image(env, context, rect, image);
}                                      // ← closes function ← THIS IS MISSING OR WRONG

fn CGContextSaveGState(env: &mut Environment, context: CGContextRef) {
    if context.is_null() { return; }
    let h = env.objc.borrow::<CGContextHostObject>(context);
    let state = CGContextState {
        fill_color:   h.rgb_fill_color,
        stroke_color: h.rgb_stroke_color,
        alpha:        h.alpha,
        line_width:   h.line_width,
        line_cap:     h.line_cap,
        line_join:    h.line_join,
        miter_limit:  h.miter_limit,
        flatness:     h.flatness,
        blend_mode:   h.blend_mode,
        transform:    h.transform,
    };
    env.objc.borrow_mut::<CGContextHostObject>(context).state_stack.push(state);
}

fn CGContextRestoreGState(env: &mut Environment, context: CGContextRef) {
    if context.is_null() { return; }
    let host = env.objc.borrow_mut::<CGContextHostObject>(context);
    if let Some(state) = host.state_stack.pop() {
        host.rgb_fill_color   = state.fill_color;
        host.rgb_stroke_color = state.stroke_color;
        host.alpha            = state.alpha;
        host.line_width       = state.line_width;
        host.line_cap         = state.line_cap;
        host.line_join        = state.line_join;
        host.miter_limit      = state.miter_limit;
        host.flatness         = state.flatness;
        host.blend_mode       = state.blend_mode;
        host.transform        = state.transform;
    } else {
        log!("Warning: CGContextRestoreGState: stack underflow");
    }
}


fn CGContextSetInterpolationQuality(
    _env: &mut Environment,
    context: CGContextRef,
    quality: CGInterpolationQuality,
) {
    log!(
        "TODO: CGContextSetInterpolationQuality({:?}, {:?})",
        context,
        quality
    );
}

fn CGContextGetTextPosition(
    _env: &mut Environment,
    _context: CGContextRef,
) -> CGPoint {
    CGPoint { x: 0.0, y: 0.0 }
}

fn CGContextSetTextPosition(
    _env: &mut Environment,
    _context: CGContextRef,
    _x: CGFloat,
    _y: CGFloat,
) {
}

fn CGContextSetTextDrawingMode(
    _env: &mut Environment,
    _context: CGContextRef,
    _mode: i32,
) {
}

fn CGContextSetCharacterSpacing(
    _env: &mut Environment,
    _context: CGContextRef,
    _spacing: CGFloat,
) {
}

fn CGContextSetTextMatrix(
    _env: &mut Environment,
    _context: CGContextRef,
    _t: CGAffineTransform,
) {
}

fn CGContextSelectFont(
    _env: &mut Environment,
    _context: CGContextRef,
    _name: crate::mem::ConstPtr<u8>,
    _size: CGFloat,
    _encoding: i32,
) {
}

fn CGContextShowTextAtPoint(
    _env: &mut Environment,
    _context: CGContextRef,
    _x: CGFloat,
    _y: CGFloat,
    _string: crate::mem::ConstPtr<u8>,
    _length: u32,
) {
}

fn CGContextShowText(
    _env: &mut Environment,
    _context: CGContextRef,
    _string: crate::mem::ConstPtr<u8>,
    _length: u32,
) {
}

fn CGContextSetFontSize(
    _env: &mut Environment,
    _context: CGContextRef,
    _size: CGFloat,
) {
}

fn CGContextSetFont(
    _env: &mut Environment,
    _context: CGContextRef,
    _font: crate::mem::ConstVoidPtr,
) {
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(CGContextRetain(_)),
    export_c_func!(CGContextRelease(_)),
    export_c_func!(CGContextSetFillColorWithColor(_, _)),
    export_c_func!(CGContextSetRGBFillColor(_, _, _, _, _)),
    export_c_func!(CGContextSetRGBStrokeColor(_, _, _, _, _)),
    export_c_func!(CGContextSetGrayFillColor(_, _, _)),
    export_c_func!(CGContextFillRect(_, _)),
    export_c_func!(CGContextClearRect(_, _)),
    export_c_func!(CGContextClipToRect(_, _)),
    export_c_func!(CGContextConcatCTM(_, _)),
    export_c_func!(CGContextGetCTM(_)),
    export_c_func!(CGContextRotateCTM(_, _)),
    export_c_func!(CGContextScaleCTM(_, _, _)),
    export_c_func!(CGContextTranslateCTM(_, _, _)),
    export_c_func!(CGContextDrawImage(_, _, _)),
    export_c_func!(CGContextSaveGState(_)),
    export_c_func!(CGContextRestoreGState(_)),
    export_c_func!(CGContextSetInterpolationQuality(_, _)),
    export_c_func!(CGContextGetTextPosition(_)),
    export_c_func!(CGContextSetTextPosition(_, _, _)),
    export_c_func!(CGContextSetTextDrawingMode(_, _)),
    export_c_func!(CGContextSetCharacterSpacing(_, _)),
    export_c_func!(CGContextSetTextMatrix(_, _)),
    export_c_func!(CGContextSelectFont(_, _, _, _)),
    export_c_func!(CGContextShowTextAtPoint(_, _, _, _, _)),
    export_c_func!(CGContextShowText(_, _, _)),
    export_c_func!(CGContextSetFontSize(_, _)),
    export_c_func!(CGContextSetFont(_, _)),
    // Add to FUNCTIONS:
export_c_func!(CGContextSetStrokeColorWithColor(_, _)),
export_c_func!(CGContextSetGrayStrokeColor(_, _, _)),
export_c_func!(CGContextSetAlpha(_, _)),
export_c_func!(CGContextSetLineWidth(_, _)),
export_c_func!(CGContextSetLineCap(_, _)),
export_c_func!(CGContextSetLineJoin(_, _)),
export_c_func!(CGContextSetMiterLimit(_, _)),
export_c_func!(CGContextSetLineDash(_, _, _, _)),
export_c_func!(CGContextSetFlatness(_, _)),
export_c_func!(CGContextStrokeRect(_, _)),
export_c_func!(CGContextStrokeRectWithWidth(_, _, _)),
export_c_func!(CGContextStrokeEllipseInRect(_, _)),
export_c_func!(CGContextFillEllipseInRect(_, _)),
export_c_func!(CGContextAddRect(_, _)),
export_c_func!(CGContextAddRects(_, _, _)),
export_c_func!(CGContextAddEllipseInRect(_, _)),
export_c_func!(CGContextAddArc(_, _, _, _, _, _, _)),
export_c_func!(CGContextAddArcToPoint(_, _, _, _, _, _)),
export_c_func!(CGContextAddLineToPoint(_, _, _)),
export_c_func!(CGContextAddLines(_, _, _)),
export_c_func!(CGContextMoveToPoint(_, _, _)),
export_c_func!(CGContextAddCurveToPoint(_, _, _, _, _, _, _)),
export_c_func!(CGContextAddQuadCurveToPoint(_, _, _, _, _)),
export_c_func!(CGContextClosePath(_)),
export_c_func!(CGContextBeginPath(_)),
export_c_func!(CGContextDrawPath(_, _)),
export_c_func!(CGContextFillPath(_)),
export_c_func!(CGContextEOFillPath(_)),
export_c_func!(CGContextStrokePath(_)),
export_c_func!(CGContextSetShouldAntialias(_, _)),
export_c_func!(CGContextSetAllowsAntialiasing(_, _)),
export_c_func!(CGContextSetShouldSmoothFonts(_, _)),
export_c_func!(CGContextFlush(_)),
export_c_func!(CGContextSynchronize(_)),
export_c_func!(CGContextGetClipBoundingBox(_)),
export_c_func!(CGContextResetClip(_)),
export_c_func!(CGContextClipToMask(_, _, _)),
export_c_func!(CGContextSetBlendMode(_, _)),
export_c_func!(CGContextSetShadow(_, _, _)),
export_c_func!(CGContextSetShadowWithColor(_, _, _, _)),

];
