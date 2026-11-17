/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `CFFileDescriptor` — run-loop integration for POSIX file descriptors.
//!
//! Used by frameworks that watch a file descriptor (libnotify's notification
//! pipe, reachability sockets) from a run loop. touchHLE has no live host
//! networking, so callbacks are not delivered, but the object model keeps the
//! native descriptor, the callback and the invalidate/close ownership rules
//! intact so callers don't read uninitialised objects.
//!
//! Reference:
//! <https://developer.apple.com/documentation/corefoundation/cffiledescriptor>

use crate::abi::{CallFromHost, GuestFunction};
use crate::dyld::{export_c_func, FunctionExports};
use crate::frameworks::core_foundation::cf_allocator::{kCFAllocatorDefault, CFAllocatorRef};
use crate::frameworks::core_foundation::cf_run_loop::{
    CFRunLoopSourceHostObject, CFRunLoopSourceRef,
};
use crate::frameworks::core_foundation::{CFIndex, CFTypeRef};
use crate::libc::posix_io::{close, FileDescriptor};
use crate::mem::{MutPtr, MutVoidPtr, SafeRead};
use crate::objc::{nil, objc_classes, ClassExports, HostObject};
use crate::Environment;

pub type CFFileDescriptorRef = CFTypeRef;
pub type CFFileDescriptorNativeDescriptor = i32;
pub type CFOptionFlags = u32;
/// `CFOptionFlags` bits for `CFFileDescriptorEnableCallBacks`.
const kCFFileDescriptorReadCallBack: CFOptionFlags = 1 << 0;
const kCFFileDescriptorWriteCallBack: CFOptionFlags = 1 << 1;

/// `CFFileDescriptorContext` (version 0).
#[repr(C, packed)]
pub struct CFFileDescriptorContext {
    pub version: CFIndex,
    pub info: MutVoidPtr,
    pub retain: GuestFunction,
    pub release: GuestFunction,
    pub copyDescription: GuestFunction,
}
unsafe impl SafeRead for CFFileDescriptorContext {}

#[derive(Default)]
struct CFFileDescriptorHostObject {
    fd: FileDescriptor,
    close_on_invalidate: bool,
    callout: GuestFunction,
    info: MutVoidPtr,
    release: GuestFunction,
    valid: bool,
    enabled_callbacks: CFOptionFlags,
}
impl HostObject for CFFileDescriptorHostObject {}

fn CFFileDescriptorCreate(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    fd: CFFileDescriptorNativeDescriptor,
    close_on_invalidate: bool,
    callout: MutVoidPtr,
    context: MutPtr<CFFileDescriptorContext>,
) -> CFFileDescriptorRef {
    if !(allocator == kCFAllocatorDefault
        || (!allocator.is_null() && env.mem.read(allocator).is_system_default()))
    {
        return nil;
    }
    let (info, retain_cb, release_cb) = if context.is_null() {
        (
            MutVoidPtr::null(),
            GuestFunction::null_ptr(),
            GuestFunction::null_ptr(),
        )
    } else {
        let context = env.mem.read(context);
        // `CFFileDescriptorContext` is packed; copy fields out before use.
        let version = context.version;
        if version != 0 {
            return nil;
        }
        let info = context.info;
        let retain_cb = context.retain;
        let release_cb = context.release;
        (info, retain_cb, release_cb)
    };
    let class = env
        .objc
        .get_known_class("_touchHLE_CFFileDescriptor", &mut env.mem);
    let descriptor = env.objc.alloc_object(
        class,
        Box::new(CFFileDescriptorHostObject {
            fd,
            close_on_invalidate,
            callout: GuestFunction::from_addr_with_thumb_bit(callout.to_bits()),
            info,
            release: release_cb,
            valid: true,
            enabled_callbacks: 0,
        }),
        &mut env.mem,
    );
    if !retain_cb.to_ptr().is_null() {
        let _: MutVoidPtr = retain_cb.call_from_host(env, (info,));
    }
    descriptor
}

fn CFFileDescriptorGetNativeDescriptor(
    env: &mut Environment,
    descriptor: CFFileDescriptorRef,
) -> CFFileDescriptorNativeDescriptor {
    if descriptor.is_null() {
        return -1;
    }
    env.objc
        .borrow::<CFFileDescriptorHostObject>(descriptor)
        .fd
}

fn CFFileDescriptorIsValid(env: &mut Environment, descriptor: CFFileDescriptorRef) -> bool {
    if descriptor.is_null() {
        return false;
    }
    env.objc
        .borrow::<CFFileDescriptorHostObject>(descriptor)
        .valid
}

fn CFFileDescriptorInvalidate(env: &mut Environment, descriptor: CFFileDescriptorRef) {
    if descriptor.is_null() {
        return;
    }
    let (valid, fd, close_on_invalidate) = {
        let host = env.objc.borrow::<CFFileDescriptorHostObject>(descriptor);
        (host.valid, host.fd, host.close_on_invalidate)
    };
    if !valid {
        return;
    }
    env.objc
        .borrow_mut::<CFFileDescriptorHostObject>(descriptor)
        .valid = false;
    if close_on_invalidate && fd >= 0 {
        close(env, fd);
    }
}

fn CFFileDescriptorEnableCallBacks(
    env: &mut Environment,
    descriptor: CFFileDescriptorRef,
    callback_types: CFOptionFlags,
) {
    if descriptor.is_null() {
        return;
    }
    env.objc
        .borrow_mut::<CFFileDescriptorHostObject>(descriptor)
        .enabled_callbacks |= callback_types
        & (kCFFileDescriptorReadCallBack | kCFFileDescriptorWriteCallBack);
}

fn CFFileDescriptorDisableCallBacks(
    env: &mut Environment,
    descriptor: CFFileDescriptorRef,
    callback_types: CFOptionFlags,
) {
    if descriptor.is_null() {
        return;
    }
    env.objc
        .borrow_mut::<CFFileDescriptorHostObject>(descriptor)
        .enabled_callbacks &= !(callback_types
        & (kCFFileDescriptorReadCallBack | kCFFileDescriptorWriteCallBack));
}

/// Creates a run-loop source backed by the file descriptor. There is no live
/// event delivery here, so the source never signals; it still participates in
/// CFRunLoopAddSource/RemoveSource and retain/release correctly.
fn CFFileDescriptorCreateRunLoopSource(
    env: &mut Environment,
    _allocator: CFAllocatorRef,
    descriptor: CFFileDescriptorRef,
    order: CFIndex,
) -> CFRunLoopSourceRef {
    if descriptor.is_null() {
        return nil;
    }
    let class = env
        .objc
        .get_known_class("_touchHLE_CFRunLoopSource", &mut env.mem);
    env.objc.alloc_object(
        class,
        Box::new(CFRunLoopSourceHostObject {
            order: order as i32,
            valid: true,
            ..Default::default()
        }),
        &mut env.mem,
    )
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(CFFileDescriptorCreate(_, _, _, _, _)),
    export_c_func!(CFFileDescriptorGetNativeDescriptor(_)),
    export_c_func!(CFFileDescriptorIsValid(_)),
    export_c_func!(CFFileDescriptorInvalidate(_)),
    export_c_func!(CFFileDescriptorEnableCallBacks(_, _)),
    export_c_func!(CFFileDescriptorDisableCallBacks(_, _)),
    export_c_func!(CFFileDescriptorCreateRunLoopSource(_, _, _)),
];

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation _touchHLE_CFFileDescriptor: NSObject

- (())dealloc {
    let (valid, fd, close_on_invalidate, release_cb, info) = {
        let host = env.objc.borrow::<CFFileDescriptorHostObject>(this);
        (
            host.valid,
            host.fd,
            host.close_on_invalidate,
            host.release,
            host.info,
        )
    };
    if valid && close_on_invalidate && fd >= 0 {
        close(env, fd);
    }
    if !release_cb.to_ptr().is_null() {
        let _: () = release_cb.call_from_host(env, (info,));
    }
    env.objc.dealloc_object(this, &mut env.mem)
}

@end

};
