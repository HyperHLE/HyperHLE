/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 * If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
#![allow(dead_code)]
//! `CGPath.h`

use crate::dyld::FunctionExports;
use crate::export_c_func;
use crate::frameworks::core_foundation::{CFRelease, CFRetain, CFTypeRef};
use crate::frameworks::core_graphics::{CGFloat, CGPoint, CGRect};
use crate::mem::MutVoidPtr;
use crate::objc::{objc_classes, ClassExports, HostObject};
use crate::Environment;

pub type CGPathRef = CFTypeRef;
pub type CGMutablePathRef = CFTypeRef;

type CGPathElementType = i32;
const kCGPathElementMoveToPoint: CGPathElementType = 0;
const kCGPathElementAddLineToPoint: CGPathElementType = 1;
const kCGPathElementAddQuadCurveToPoint: CGPathElementType = 2;
const kCGPathElementAddCurveToPoint: CGPathElementType = 3;
const kCGPathElementCloseSubpath: CGPathElementType = 4;

#[derive(Clone, Debug)]
enum PathElement {
    MoveTo(CGPoint),
    LineTo(CGPoint),
    QuadCurveTo { control: CGPoint, to: CGPoint },
    CurveTo { c1: CGPoint, c2: CGPoint, to: CGPoint },
    Close,
}

struct CGPathHostObject {
    elements: Vec<PathElement>,
    mutable: bool,
}
impl HostObject for CGPathHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation _touchHLE_CGPath: NSObject

- (())dealloc {
    env.objc.dealloc_object(this, &mut env.mem)
}

@end

};

// MARK: - Helpers

fn alloc_path(env: &mut Environment, mutable: bool) -> CGPathRef {
    let class = env.objc.get_known_class("_touchHLE_CGPath", &mut env.mem);
    env.objc.alloc_object(
        class,
        Box::new(CGPathHostObject {
            elements: Vec::new(),
            mutable,
        }),
        &mut env.mem,
    )
}

// MARK: - Lifecycle

pub fn CGPathRetain(env: &mut Environment, path: CGPathRef) -> CGPathRef {
    if !path.is_null() {
        CFRetain(env, path)
    } else {
        path
    }
}

pub fn CGPathRelease(env: &mut Environment, path: CGPathRef) {
    if !path.is_null() {
        CFRelease(env, path);
    }
}

fn CGPathCreateMutable(env: &mut Environment) -> CGMutablePathRef {
    alloc_path(env, true)
}

fn CGPathCreateCopy(env: &mut Environment, path: CGPathRef) -> CGPathRef {
    if path.is_null() {
        return path;
    }
    let elements = env.objc.borrow::<CGPathHostObject>(path).elements.clone();
    let class = env.objc.get_known_class("_touchHLE_CGPath", &mut env.mem);
    env.objc.alloc_object(
        class,
        Box::new(CGPathHostObject {
            elements,
            mutable: false,
        }),
        &mut env.mem,
    )
}

fn CGPathCreateMutableCopy(env: &mut Environment, path: CGPathRef) -> CGMutablePathRef {
    if path.is_null() {
        return alloc_path(env, true);
    }
    let elements = env.objc.borrow::<CGPathHostObject>(path).elements.clone();
    let class = env.objc.get_known_class("_touchHLE_CGPath", &mut env.mem);
    env.objc.alloc_object(
        class,
        Box::new(CGPathHostObject {
            elements,
            mutable: true,
        }),
        &mut env.mem,
    )
}

fn CGPathCreateWithRect(
    env: &mut Environment,
    rect: CGRect,
    _transform: MutVoidPtr, // const CGAffineTransform* — ignored for now
) -> CGPathRef {
    let path = alloc_path(env, false);
    let CGRect { origin, size } = rect;
    let tl = origin;
    let tr = CGPoint { x: origin.x + size.width, y: origin.y };
    let br = CGPoint { x: origin.x + size.width, y: origin.y + size.height };
    let bl = CGPoint { x: origin.x,              y: origin.y + size.height };

    let elems = &mut env.objc.borrow_mut::<CGPathHostObject>(path).elements;
    elems.push(PathElement::MoveTo(tl));
    elems.push(PathElement::LineTo(tr));
    elems.push(PathElement::LineTo(br));
    elems.push(PathElement::LineTo(bl));
    elems.push(PathElement::Close);

    path
}

fn CGPathCreateWithEllipseInRect(
    env: &mut Environment,
    rect: CGRect,
    _transform: MutVoidPtr,
) -> CGPathRef {
    // Approximate ellipse with four cubic Bézier curves (standard magic number).
    let path = alloc_path(env, false);
    let k: CGFloat = 0.5522848;
    let cx = rect.origin.x + rect.size.width  * 0.5;
    let cy = rect.origin.y + rect.size.height * 0.5;
    let rx = rect.size.width  * 0.5;
    let ry = rect.size.height * 0.5;

    let elems = &mut env.objc.borrow_mut::<CGPathHostObject>(path).elements;
    elems.push(PathElement::MoveTo(CGPoint { x: cx + rx, y: cy }));
    elems.push(PathElement::CurveTo {
        c1: CGPoint { x: cx + rx,      y: cy + ry * k },
        c2: CGPoint { x: cx + rx * k,  y: cy + ry },
        to: CGPoint { x: cx,           y: cy + ry },
    });
    elems.push(PathElement::CurveTo {
        c1: CGPoint { x: cx - rx * k,  y: cy + ry },
        c2: CGPoint { x: cx - rx,      y: cy + ry * k },
        to: CGPoint { x: cx - rx,      y: cy },
    });
    elems.push(PathElement::CurveTo {
        c1: CGPoint { x: cx - rx,      y: cy - ry * k },
        c2: CGPoint { x: cx - rx * k,  y: cy - ry },
        to: CGPoint { x: cx,           y: cy - ry },
    });
    elems.push(PathElement::CurveTo {
        c1: CGPoint { x: cx + rx * k,  y: cy - ry },
        c2: CGPoint { x: cx + rx,      y: cy - ry * k },
        to: CGPoint { x: cx + rx,      y: cy },
    });
    elems.push(PathElement::Close);

    path
}

// MARK: - Mutable path operations

fn CGPathMoveToPoint(
    env: &mut Environment,
    path: CGMutablePathRef,
    _transform: MutVoidPtr,
    x: CGFloat,
    y: CGFloat,
) {
    env.objc
        .borrow_mut::<CGPathHostObject>(path)
        .elements
        .push(PathElement::MoveTo(CGPoint { x, y }));
}

fn CGPathAddLineToPoint(
    env: &mut Environment,
    path: CGMutablePathRef,
    _transform: MutVoidPtr,
    x: CGFloat,
    y: CGFloat,
) {
    env.objc
        .borrow_mut::<CGPathHostObject>(path)
        .elements
        .push(PathElement::LineTo(CGPoint { x, y }));
}

fn CGPathAddQuadCurveToPoint(
    env: &mut Environment,
    path: CGMutablePathRef,
    _transform: MutVoidPtr,
    cpx: CGFloat,
    cpy: CGFloat,
    x: CGFloat,
    y: CGFloat,
) {
    env.objc
        .borrow_mut::<CGPathHostObject>(path)
        .elements
        .push(PathElement::QuadCurveTo {
            control: CGPoint { x: cpx, y: cpy },
            to:      CGPoint { x,      y      },
        });
}

fn CGPathAddCurveToPoint(
    env: &mut Environment,
    path: CGMutablePathRef,
    _transform: MutVoidPtr,
    cp1x: CGFloat,
    cp1y: CGFloat,
    cp2x: CGFloat,
    cp2y: CGFloat,
    x: CGFloat,
    y: CGFloat,
) {
    env.objc
        .borrow_mut::<CGPathHostObject>(path)
        .elements
        .push(PathElement::CurveTo {
            c1: CGPoint { x: cp1x, y: cp1y },
            c2: CGPoint { x: cp2x, y: cp2y },
            to: CGPoint { x,       y       },
        });
}

fn CGPathCloseSubpath(env: &mut Environment, path: CGMutablePathRef) {
    env.objc
        .borrow_mut::<CGPathHostObject>(path)
        .elements
        .push(PathElement::Close);
}

fn CGPathAddRect(
    env: &mut Environment,
    path: CGMutablePathRef,
    _transform: MutVoidPtr,
    rect: CGRect,
) {
    let CGRect { origin, size } = rect;
    let tl = origin;
    let tr = CGPoint { x: origin.x + size.width, y: origin.y };
    let br = CGPoint { x: origin.x + size.width, y: origin.y + size.height };
    let bl = CGPoint { x: origin.x,              y: origin.y + size.height };

    let elems = &mut env.objc.borrow_mut::<CGPathHostObject>(path).elements;
    elems.push(PathElement::MoveTo(tl));
    elems.push(PathElement::LineTo(tr));
    elems.push(PathElement::LineTo(br));
    elems.push(PathElement::LineTo(bl));
    elems.push(PathElement::Close);
}

fn CGPathAddPath(
    env: &mut Environment,
    path: CGMutablePathRef,
    _transform: MutVoidPtr,
    other: CGPathRef,
) {
    if other.is_null() {
        return;
    }
    let other_elems = env.objc.borrow::<CGPathHostObject>(other).elements.clone();
    env.objc
        .borrow_mut::<CGPathHostObject>(path)
        .elements
        .extend(other_elems);
}

// MARK: - Query

fn CGPathIsEmpty(env: &mut Environment, path: CGPathRef) -> bool {
    if path.is_null() {
        return true;
    }
    env.objc.borrow::<CGPathHostObject>(path).elements.is_empty()
}

fn CGPathGetCurrentPoint(env: &mut Environment, path: CGPathRef) -> CGPoint {
    if path.is_null() {
        return CGPoint { x: 0.0, y: 0.0 };
    }
    for elem in env.objc.borrow::<CGPathHostObject>(path).elements.iter().rev() {
        match *elem {
            PathElement::MoveTo(p)        => return p,
            PathElement::LineTo(p)        => return p,
            PathElement::QuadCurveTo { to, .. } => return to,
            PathElement::CurveTo   { to, .. }   => return to,
            PathElement::Close => {}
        }
    }
    CGPoint { x: 0.0, y: 0.0 }
}

fn CGPathGetBoundingBox(env: &mut Environment, path: CGPathRef) -> CGRect {
    if path.is_null() {
        return CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: crate::frameworks::core_graphics::CGSize { width: 0.0, height: 0.0 },
        };
    }

    let mut min_x = CGFloat::MAX;
    let mut min_y = CGFloat::MAX;
    let mut max_x = CGFloat::MIN;
    let mut max_y = CGFloat::MIN;

    let mut update = |p: CGPoint| {
        if p.x < min_x { min_x = p.x; }
        if p.y < min_y { min_y = p.y; }
        if p.x > max_x { max_x = p.x; }
        if p.y > max_y { max_y = p.y; }
    };

    for elem in env.objc.borrow::<CGPathHostObject>(path).elements.iter() {
        match *elem {
            PathElement::MoveTo(p) |
            PathElement::LineTo(p) => update(p),
            PathElement::QuadCurveTo { control, to } => { update(control); update(to); }
            PathElement::CurveTo { c1, c2, to } => { update(c1); update(c2); update(to); }
            PathElement::Close => {}
        }
    }

    if min_x > max_x {
        return CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: crate::frameworks::core_graphics::CGSize { width: 0.0, height: 0.0 },
        };
    }

    CGRect {
        origin: CGPoint { x: min_x, y: min_y },
        size: crate::frameworks::core_graphics::CGSize {
            width:  max_x - min_x,
            height: max_y - min_y,
        },
    }
}

fn CGPathContainsPoint(
    env: &mut Environment,
    path: CGPathRef,
    _transform: MutVoidPtr,
    point: CGPoint,
    _eo_fill_rule: bool,
) -> bool {
    // Simple bounding-box hit test — good enough for most game use-cases.
    let bbox = CGPathGetBoundingBox(env, path);
    point.x >= bbox.origin.x
        && point.x <= bbox.origin.x + bbox.size.width
        && point.y >= bbox.origin.y
        && point.y <= bbox.origin.y + bbox.size.height
}

fn CGPathEqualToPath(
    env: &mut Environment,
    path1: CGPathRef,
    path2: CGPathRef,
) -> bool {
    if path1 == path2 {
        return true;
    }
    if path1.is_null() || path2.is_null() {
        return false;
    }
    // Compare element counts as a quick proxy; deep equality not needed for stubs.
    env.objc.borrow::<CGPathHostObject>(path1).elements.len()
        == env.objc.borrow::<CGPathHostObject>(path2).elements.len()
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(CGPathRetain(_)),
    export_c_func!(CGPathRelease(_)),
    export_c_func!(CGPathCreateMutable()),
    export_c_func!(CGPathCreateCopy(_)),
    export_c_func!(CGPathCreateMutableCopy(_)),
    export_c_func!(CGPathCreateWithRect(_, _)),
    export_c_func!(CGPathCreateWithEllipseInRect(_, _)),
    export_c_func!(CGPathMoveToPoint(_, _, _, _)),
    export_c_func!(CGPathAddLineToPoint(_, _, _, _)),
    export_c_func!(CGPathAddQuadCurveToPoint(_, _, _, _, _, _)),
    export_c_func!(CGPathAddCurveToPoint(_, _, _, _, _, _, _, _)),
    export_c_func!(CGPathCloseSubpath(_)),
    export_c_func!(CGPathAddRect(_, _, _)),
    export_c_func!(CGPathAddPath(_, _, _)),
    export_c_func!(CGPathIsEmpty(_)),
    export_c_func!(CGPathGetCurrentPoint(_)),
    export_c_func!(CGPathGetBoundingBox(_)),
    export_c_func!(CGPathContainsPoint(_, _, _, _)),
    export_c_func!(CGPathEqualToPath(_, _)),
];

