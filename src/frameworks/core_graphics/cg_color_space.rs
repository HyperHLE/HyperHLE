/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `CGColorSpace.h`

use crate::dyld::{export_c_func, ConstantExports, FunctionExports, HostConstant};
use crate::frameworks::core_foundation::cf_string::CFStringRef;
use crate::frameworks::core_foundation::{CFRelease, CFRetain, CFTypeRef};
use crate::frameworks::foundation::{ns_string, NSUInteger};
use crate::objc::{msg, nil, objc_classes, ClassExports, HostObject};
use crate::Environment;

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation _touchHLE_CGColorSpace: NSObject
@end

};

pub type CGColorSpaceModel = i32;
#[allow(dead_code)]
pub const kCGColorSpaceModelUnknown:    CGColorSpaceModel = -1;
pub const kCGColorSpaceModelMonochrome: CGColorSpaceModel =  0;
pub const kCGColorSpaceModelRGB:        CGColorSpaceModel =  1;
#[allow(dead_code)]
pub const kCGColorSpaceModelCMYK:       CGColorSpaceModel =  2;
#[allow(dead_code)]
pub const kCGColorSpaceModelLab:        CGColorSpaceModel =  3;
#[allow(dead_code)]
pub const kCGColorSpaceModelDeviceN:    CGColorSpaceModel =  4;
#[allow(dead_code)]
pub const kCGColorSpaceModelIndexed:    CGColorSpaceModel =  5;
#[allow(dead_code)]
pub const kCGColorSpaceModelPattern:    CGColorSpaceModel =  6;

pub(super) struct CGColorSpaceHostObject {
    pub(super) name: &'static str,
}
impl HostObject for CGColorSpaceHostObject {}

pub type CGColorSpaceRef = CFTypeRef;

// MARK: - Internal alloc helper

fn alloc_color_space(env: &mut Environment, name: &'static str) -> CGColorSpaceRef {
    let isa = env
        .objc
        .get_known_class("_touchHLE_CGColorSpace", &mut env.mem);
    env.objc.alloc_object(
        isa,
        Box::new(CGColorSpaceHostObject { name }),
        &mut env.mem,
    )
}

// MARK: - Constructors

pub fn CGColorSpaceCreateWithName(env: &mut Environment, name: CFStringRef) -> CGColorSpaceRef {
    let generic_rgb  = ns_string::get_static_str(env, kCGColorSpaceGenericRGB);
    let generic_gray = ns_string::get_static_str(env, kCGColorSpaceGenericGray);
    let srgb         = ns_string::get_static_str(env, kCGColorSpaceSRGB);
    let device_cmyk  = ns_string::get_static_str(env, kCGColorSpaceGenericCMYK);
    let linear_gray  = ns_string::get_static_str(env, kCGColorSpaceLinearGray);
    let linear_srgb  = ns_string::get_static_str(env, kCGColorSpaceLinearSRGB);

    if msg![env; name isEqualToString:generic_rgb]
        || msg![env; name isEqualToString:srgb]
        || msg![env; name isEqualToString:linear_srgb]
    {
        alloc_color_space(env, kCGColorSpaceGenericRGB)
    } else if msg![env; name isEqualToString:generic_gray]
        || msg![env; name isEqualToString:linear_gray]
    {
        alloc_color_space(env, kCGColorSpaceGenericGray)
    } else if msg![env; name isEqualToString:device_cmyk] {
        alloc_color_space(env, kCGColorSpaceGenericCMYK)
    } else {
        log!(
            "Warning: CGColorSpaceCreateWithName: unknown color space, \
             falling back to GenericRGB"
        );
        alloc_color_space(env, kCGColorSpaceGenericRGB)
    }
}

pub fn CGColorSpaceCreateDeviceRGB(env: &mut Environment) -> CGColorSpaceRef {
    alloc_color_space(env, kCGColorSpaceGenericRGB)
}

pub fn CGColorSpaceCreateDeviceGray(env: &mut Environment) -> CGColorSpaceRef {
    alloc_color_space(env, kCGColorSpaceGenericGray)
}

fn CGColorSpaceCreateDeviceCMYK(env: &mut Environment) -> CGColorSpaceRef {
    alloc_color_space(env, kCGColorSpaceGenericCMYK)
}

fn CGColorSpaceCreateWithICCProfile(
    env: &mut Environment,
    _data: crate::objc::id, // CFDataRef
) -> CGColorSpaceRef {
    log!("Warning: CGColorSpaceCreateWithICCProfile: ICC profiles not supported, \
          falling back to GenericRGB");
    alloc_color_space(env, kCGColorSpaceGenericRGB)
}

fn CGColorSpaceCreateICCBased(
    env: &mut Environment,
    n_components: NSUInteger,
    _range: crate::mem::ConstPtr<crate::frameworks::core_graphics::CGFloat>,
    _profile: CFTypeRef, // CGDataProviderRef
    _alternate: CGColorSpaceRef,
) -> CGColorSpaceRef {
    log!(
        "Warning: CGColorSpaceCreateICCBased: ICC profiles not supported \
         (nComponents={}), falling back",
        n_components
    );
    match n_components {
        1 => alloc_color_space(env, kCGColorSpaceGenericGray),
        4 => alloc_color_space(env, kCGColorSpaceGenericCMYK),
        _ => alloc_color_space(env, kCGColorSpaceGenericRGB),
    }
}

fn CGColorSpaceCreateIndexed(
    env: &mut Environment,
    _base_space: CGColorSpaceRef,
    _last_index: NSUInteger,
    _color_table: crate::mem::ConstPtr<u8>,
) -> CGColorSpaceRef {
    log!("Warning: CGColorSpaceCreateIndexed: indexed color spaces not supported, \
          falling back to GenericRGB");
    alloc_color_space(env, kCGColorSpaceGenericRGB)
}

fn CGColorSpaceCreatePattern(
    env: &mut Environment,
    _base_space: CGColorSpaceRef,
) -> CGColorSpaceRef {
    log!("Warning: CGColorSpaceCreatePattern: pattern color spaces not supported, \
          falling back to GenericRGB");
    alloc_color_space(env, kCGColorSpaceGenericRGB)
}

// MARK: - Retain / Release

pub fn CGColorSpaceRelease(env: &mut Environment, cs: CGColorSpaceRef) {
    if !cs.is_null() {
        CFRelease(env, cs);
    }
}

pub fn CGColorSpaceRetain(env: &mut Environment, cs: CGColorSpaceRef) -> CGColorSpaceRef {
    if !cs.is_null() {
        CFRetain(env, cs)
    } else {
        cs
    }
}

// MARK: - Accessors

pub fn CGColorSpaceGetModel(env: &mut Environment, cs: CGColorSpaceRef) -> CGColorSpaceModel {
    if cs.is_null() {
        return kCGColorSpaceModelUnknown;
    }
    match env.objc.borrow::<CGColorSpaceHostObject>(cs).name {
        kCGColorSpaceGenericGray => kCGColorSpaceModelMonochrome,
        kCGColorSpaceGenericRGB  => kCGColorSpaceModelRGB,
        kCGColorSpaceGenericCMYK => kCGColorSpaceModelCMYK,
        _                        => kCGColorSpaceModelUnknown,
    }
}

/// Returns the number of colour components *excluding* alpha.
pub fn CGColorSpaceGetNumberOfComponents(
    env: &mut Environment,
    cs: CGColorSpaceRef,
) -> NSUInteger {
    if cs.is_null() {
        return 0;
    }
    match env.objc.borrow::<CGColorSpaceHostObject>(cs).name {
        kCGColorSpaceGenericGray => 1,
        kCGColorSpaceGenericRGB  => 3,
        kCGColorSpaceGenericCMYK => 4,
        _                        => 3,
    }
}

fn CGColorSpaceCopyName(env: &mut Environment, cs: CGColorSpaceRef) -> CFStringRef {
    if cs.is_null() {
        return nil;
    }
    let name = env.objc.borrow::<CGColorSpaceHostObject>(cs).name;
    let ns = ns_string::from_rust_string(env, name.to_string());
    // CFStringRef is toll-free bridged with NSString; autorelease so caller
    // gets a +0 reference (matching real CG behaviour of "Copy" returning +1,
    // but apps usually don't release it — autorelease is the safe middle ground).
    crate::objc::autorelease(env, ns)
}

fn CGColorSpaceIsWideGamutRGB(_env: &mut Environment, _cs: CGColorSpaceRef) -> bool {
    // touchHLE only models sRGB-equivalent spaces — never wide gamut.
    false
}

fn CGColorSpaceSupportsOutput(_env: &mut Environment, cs: CGColorSpaceRef) -> bool {
    !cs.is_null()
}

// MARK: - Constants

pub const kCGColorSpaceGenericRGB:  &str = "kCGColorSpaceGenericRGB";
pub const kCGColorSpaceGenericGray: &str = "kCGColorSpaceGenericGray";
pub const kCGColorSpaceGenericCMYK: &str = "kCGColorSpaceGenericCMYK";
pub const kCGColorSpaceSRGB:        &str = "kCGColorSpaceSRGB";
pub const kCGColorSpaceLinearSRGB:  &str = "kCGColorSpaceLinearSRGB";
pub const kCGColorSpaceLinearGray:  &str = "kCGColorSpaceLinearGray";

pub const CONSTANTS: ConstantExports = &[
    (
        "_kCGColorSpaceGenericRGB",
        HostConstant::NSString(kCGColorSpaceGenericRGB),
    ),
    (
        "_kCGColorSpaceGenericGray",
        HostConstant::NSString(kCGColorSpaceGenericGray),
    ),
    (
        "_kCGColorSpaceGenericCMYK",
        HostConstant::NSString(kCGColorSpaceGenericCMYK),
    ),
    (
        "_kCGColorSpaceSRGB",
        HostConstant::NSString(kCGColorSpaceSRGB),
    ),
    (
        "_kCGColorSpaceLinearSRGB",
        HostConstant::NSString(kCGColorSpaceLinearSRGB),
    ),
    (
        "_kCGColorSpaceLinearGray",
        HostConstant::NSString(kCGColorSpaceLinearGray),
    ),
];

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(CGColorSpaceCreateWithName(_)),
    export_c_func!(CGColorSpaceCreateDeviceRGB()),
    export_c_func!(CGColorSpaceCreateDeviceGray()),
    export_c_func!(CGColorSpaceCreateDeviceCMYK()),
    export_c_func!(CGColorSpaceCreateWithICCProfile(_)),
    export_c_func!(CGColorSpaceCreateICCBased(_, _, _, _)),
    export_c_func!(CGColorSpaceCreateIndexed(_, _, _)),
    export_c_func!(CGColorSpaceCreatePattern(_)),
    export_c_func!(CGColorSpaceRetain(_)),
    export_c_func!(CGColorSpaceRelease(_)),
    export_c_func!(CGColorSpaceGetModel(_)),
    export_c_func!(CGColorSpaceGetNumberOfComponents(_)),
    export_c_func!(CGColorSpaceCopyName(_)),
    export_c_func!(CGColorSpaceIsWideGamutRGB(_)),
    export_c_func!(CGColorSpaceSupportsOutput(_)),
];

