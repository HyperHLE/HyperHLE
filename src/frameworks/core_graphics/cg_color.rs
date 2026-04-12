/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `CGColor.h`

use std::ops::{Add, Mul, Sub};

use crate::dyld::{export_c_func, FunctionExports};
use crate::frameworks::core_foundation::{CFRelease, CFRetain, CFTypeRef};
use crate::frameworks::core_graphics::cg_color_space::{
    kCGColorSpaceGenericRGB, CGColorSpaceHostObject, CGColorSpaceRef,
};
use crate::frameworks::core_graphics::CGFloat;
use crate::mem::{ConstPtr, MutPtr};
use crate::objc::{objc_classes, ClassExports, HostObject, ObjC};
use crate::Environment;

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

// CGColor seems to be a CFType-based type, but in our implementation
// those are just Objective-C types, so we need a class for it, but its name is
// not visible anywhere.
@implementation _touchHLE_CGColor: NSObject
@end

};

#[derive(Copy, Clone)]
pub struct CGColorHostObject {
    pub color_space_name: &'static str,
    // this assumes usage of CGColorSpaceGenericRGB
    // TODO: support other color spaces
    pub r: CGFloat,
    pub g: CGFloat,
    pub b: CGFloat,
    pub a: CGFloat,
}
impl HostObject for CGColorHostObject {}
// Implemented to aid animation code.
// Theres are the operations needed for the interpolation.
impl Mul<f32> for CGColorHostObject {
    type Output = CGColorHostObject;

    fn mul(self, rhs: f32) -> Self::Output {
        CGColorHostObject {
            color_space_name: self.color_space_name,
            r: self.r * rhs,
            g: self.g * rhs,
            b: self.b * rhs,
            a: self.a * rhs,
        }
    }
}
impl Add<CGColorHostObject> for CGColorHostObject {
    type Output = CGColorHostObject;

    fn add(self, rhs: CGColorHostObject) -> Self::Output {
        CGColorHostObject {
            color_space_name: self.color_space_name,
            r: self.r + rhs.r,
            g: self.g + rhs.g,
            b: self.b + rhs.b,
            a: self.a + rhs.a,
        }
    }
}
impl Sub<CGColorHostObject> for CGColorHostObject {
    type Output = CGColorHostObject;

    fn sub(self, rhs: CGColorHostObject) -> Self::Output {
        CGColorHostObject {
            color_space_name: self.color_space_name,
            r: self.r - rhs.r,
            g: self.g - rhs.g,
            b: self.b - rhs.b,
            a: self.a - rhs.a,
        }
    }
}

pub type CGColorRef = CFTypeRef;
pub fn CGColorRelease(env: &mut Environment, c: CGColorRef) {
    if !c.is_null() {
        CFRelease(env, c);
    }
}
pub fn CGColorRetain(env: &mut Environment, c: CGColorRef) -> CGColorRef {
    if !c.is_null() {
        CFRetain(env, c)
    } else {
        c
    }
}

fn CGColorCreate(
    env: &mut Environment,
    space: CGColorSpaceRef,
    components: MutPtr<CGFloat>,
) -> CGColorRef {
    let color_space = env.objc.borrow::<CGColorSpaceHostObject>(space).name;
    assert_eq!(color_space, kCGColorSpaceGenericRGB);
    let r = env.mem.read(components);
    let g = env.mem.read(components + 1);
    let b = env.mem.read(components + 2);
    let a = env.mem.read(components + 3);
    from_rgba(env, (r, g, b, a))
}

fn CGColorCreateGenericRGB(
    env: &mut Environment,
    r: CGFloat,
    g: CGFloat,
    b: CGFloat,
    a: CGFloat,
) -> CGColorRef {
    from_rgba(env, (r, g, b, a))
}

fn CGColorEqualToColor(env: &mut Environment, a: CGColorRef, b: CGColorRef) -> bool {
    to_rgba(&env.objc, a) == to_rgba(&env.objc, b)
}

fn CGColorCreateGenericGray(
    env: &mut Environment,
    gray: CGFloat,
    alpha: CGFloat,
) -> CGColorRef {
    // Expand grey to RGB.
    from_rgba(env, (gray, gray, gray, alpha))
}

fn CGColorCreateGenericCMYK(
    env: &mut Environment,
    cyan: CGFloat,
    magenta: CGFloat,
    yellow: CGFloat,
    black: CGFloat,
    alpha: CGFloat,
) -> CGColorRef {
    // Convert CMYK → RGB.
    let r = (1.0 - cyan)    * (1.0 - black);
    let g = (1.0 - magenta) * (1.0 - black);
    let b = (1.0 - yellow)  * (1.0 - black);
    from_rgba(env, (r, g, b, alpha))
}

fn CGColorCreateCopy(env: &mut Environment, color: CGColorRef) -> CGColorRef {
    if color.is_null() {
        return color;
    }
    let &CGColorHostObject { r, g, b, a, .. } = env.objc.borrow(color);
    from_rgba(env, (r, g, b, a))
}

fn CGColorCreateCopyWithAlpha(
    env: &mut Environment,
    color: CGColorRef,
    alpha: CGFloat,
) -> CGColorRef {
    if color.is_null() {
        return color;
    }
    let &CGColorHostObject { r, g, b, .. } = env.objc.borrow(color);
    from_rgba(env, (r, g, b, alpha))
}

// MARK: - Accessors

fn CGColorGetAlpha(env: &mut Environment, color: CGColorRef) -> CGFloat {
    if color.is_null() {
        return 0.0;
    }
    env.objc.borrow::<CGColorHostObject>(color).a
}

fn CGColorGetColorSpace(env: &mut Environment, color: CGColorRef) -> CGColorSpaceRef {
    if color.is_null() {
        return crate::objc::nil;
    }
    // Return a retained colour space matching the stored name.
    // We only support GenericRGB right now.
    crate::frameworks::core_graphics::cg_color_space::CGColorSpaceCreateDeviceRGB(env)
}

/// Returns a pointer directly into guest memory where the four RGBA floats
/// of this colour live. Real CG stores these inline; we allocate a small
/// guest buffer on demand and write current values into it.
pub fn CGColorGetComponents(env: &mut Environment, color: CGColorRef) -> ConstPtr<CGFloat> {
    if color.is_null() {
        return ConstPtr::null();
    }
    let &CGColorHostObject { r, g, b, a, .. } = env.objc.borrow(color);
    // Allocate a 4-float guest buffer each call (simple, no lifetime issues).
    let buf: MutPtr<CGFloat> = env.mem.alloc(4 * 4).cast(); // 4 × sizeof(f32)
    env.mem.write(buf,     r);
    env.mem.write(buf + 1, g);
    env.mem.write(buf + 2, b);
    env.mem.write(buf + 3, a);
    buf.cast_const()
}

fn CGColorGetNumberOfComponents(_env: &mut Environment, color: CGColorRef) -> crate::frameworks::foundation::NSUInteger {
    if color.is_null() {
        return 0;
    }
    // GenericRGB always has 4 components (r, g, b, alpha).
    4
}

fn CGColorIsValid(_env: &mut Environment, color: CGColorRef) -> bool {
    !color.is_null()
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(CGColorRetain(_)),
    export_c_func!(CGColorRelease(_)),
    export_c_func!(CGColorCreate(_, _)),
    export_c_func!(CGColorCreateGenericRGB(_, _, _, _)),
    export_c_func!(CGColorCreateGenericGray(_, _)),
    export_c_func!(CGColorCreateGenericCMYK(_, _, _, _, _)),
    export_c_func!(CGColorCreateCopyWithAlpha(_, _)),
    export_c_func!(CGColorCreateCopy(_)),
    export_c_func!(CGColorEqualToColor(_, _)),
    export_c_func!(CGColorGetAlpha(_)),
    export_c_func!(CGColorGetColorSpace(_)),
    export_c_func!(CGColorGetComponents(_)),
    export_c_func!(CGColorGetNumberOfComponents(_)),
    export_c_func!(CGColorIsValid(_)),
];

/// Shortcut for use by `UIColor`: directly construct a `CGColor` instance from
/// an rgba tuple of CGFloats.
pub fn from_rgba(env: &mut Environment, rgba: (CGFloat, CGFloat, CGFloat, CGFloat)) -> CGColorRef {
    let (r, g, b, a) = rgba;
    let host_obj = Box::new(CGColorHostObject {
        color_space_name: kCGColorSpaceGenericRGB,
        r,
        g,
        b,
        a,
    });
    let class = env.objc.get_known_class("_touchHLE_CGColor", &mut env.mem);
    env.objc.alloc_object(class, host_obj, &mut env.mem)
}

/// Shortcut for use by `UIColor`
pub fn to_rgba(objc: &ObjC, color: CGColorRef) -> (CGFloat, CGFloat, CGFloat, CGFloat) {
    let &CGColorHostObject {
        color_space_name,
        r,
        g,
        b,
        a,
        ..
    } = objc.borrow(color);
    assert_eq!(color_space_name, kCGColorSpaceGenericRGB);
    (r, g, b, a)
}

