/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 * If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
#![allow(dead_code)]
//! `CFHost` — CFNetwork host resolution stub.

use crate::abi::{CallFromHost, GuestFunction};
use crate::frameworks::core_foundation::cf_allocator::CFAllocatorRef;
use crate::frameworks::core_foundation::{CFRelease, CFRetain, CFTypeRef};
use crate::frameworks::foundation::ns_string;
use crate::dyld::{export_c_func, FunctionExports};
use crate::mem::{MutVoidPtr};
use crate::objc::{nil, objc_classes, ClassExports, HostObject};
use crate::Environment;
use std::net::{SocketAddrV4, ToSocketAddrs};

pub type CFHostRef = CFTypeRef;

type CFHostInfoType = i32;
const kCFHostAddresses:    CFHostInfoType = 0;
const kCFHostNames:        CFHostInfoType = 1;
const kCFHostReachability: CFHostInfoType = 2;

// CFStreamError — simple stub struct (domain + error)
type CFStreamError = u64; // two i32s packed; we never inspect it

pub(crate) struct CFHostHostObject {
    pub(crate) address:   Option<SocketAddrV4>,
    pub(crate) name:      Option<String>,
    pub(crate) resolved:  Vec<SocketAddrV4>,   // populated after resolution
    pub(crate) callout:   Option<GuestFunction>,
    pub(crate) context:   MutVoidPtr,
}

impl HostObject for CFHostHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

// Fix dealloc — no release needed, fields are plain Rust values:
@implementation _touchHLE_CFHost: NSObject
- (())dealloc {
    env.objc.dealloc_object(this, &mut env.mem)
}
@end

};

// MARK: - Internal helpers

fn alloc_cfhost(
    env: &mut Environment,
    name: Option<String>,
    address: Option<SocketAddrV4>,
) -> CFHostRef {
    let class = env.objc.get_known_class("_touchHLE_CFHost", &mut env.mem);
    env.objc.alloc_object(
        class,
        Box::new(CFHostHostObject {
            name,
            address,
            resolved: Vec::new(),
            callout: None,
            context: MutVoidPtr::null(),
        }),
        &mut env.mem,
    )
}

// MARK: - Lifecycle

pub fn CFHostRetain(env: &mut Environment, host: CFHostRef) -> CFHostRef {
    if !host.is_null() { CFRetain(env, host) } else { host }
}

pub fn CFHostRelease(env: &mut Environment, host: CFHostRef) {
    if !host.is_null() { CFRelease(env, host); }
}

// MARK: - Constructors

fn CFHostCreateWithName(
    env: &mut Environment,
    _allocator: CFAllocatorRef,
    hostname: CFTypeRef, // CFStringRef
) -> CFHostRef {
    let name_str = ns_string::to_rust_string(env, hostname).into_owned();
    log_dbg!("CFHostCreateWithName: {}", name_str);
    alloc_cfhost(env, Some(name_str), None)
}

fn CFHostCreateWithAddress(
    env: &mut Environment,
    _allocator: CFAllocatorRef,
    addr: CFTypeRef, // CFDataRef
) -> CFHostRef {
    log_dbg!("CFHostCreateWithAddress: stubbed");
    // We don't parse the sockaddr bytes — store None for address.
    alloc_cfhost(env, None, None)
}

fn CFHostCreateCopy(
    env: &mut Environment,
    _allocator: CFAllocatorRef,
    host: CFHostRef,
) -> CFHostRef {
    if host.is_null() {
        return nil;
    }
    let h = env.objc.borrow::<CFHostHostObject>(host);
    let name    = h.name.clone();
    let address = h.address;
    drop(h);
    alloc_cfhost(env, name, address)
}

fn CFHostStartInfoResolution(
    env: &mut Environment,
    host: CFHostRef,
    info: CFHostInfoType,
    error: MutVoidPtr, // CFStreamError*
) -> bool {
    if host.is_null() {
        return false;
    }

    let name = env.objc.borrow::<CFHostHostObject>(host).name.clone();
    let Some(name_str) = name else {
        log!("CFHostStartInfoResolution: no hostname stored, returning false");
        return false;
    };

    if info != kCFHostAddresses {
        log!(
            "CFHostStartInfoResolution: info type {} not supported, returning false",
            info
        );
        return false;
    }

    // Perform a synchronous DNS lookup on the host side.
    // Port 0 is used as a placeholder — only the IP addresses matter.
    let lookup_str = format!("{}:0", name_str);
    log_dbg!("CFHostStartInfoResolution: resolving '{}'", lookup_str);

    match lookup_str.to_socket_addrs() {
        Ok(addrs) => {
            let resolved: Vec<SocketAddrV4> = addrs
                .filter_map(|addr| match addr {
                    std::net::SocketAddr::V4(v4) => Some(v4),
                    std::net::SocketAddr::V6(_)  => None, // IPv4-only for now
                })
                .collect();
            log!(
                "CFHostStartInfoResolution: '{}' resolved to {} address(es): {:?}",
                name_str,
                resolved.len(),
                resolved
            );
            env.objc.borrow_mut::<CFHostHostObject>(host).resolved = resolved;

            // Fire the client callout if one was registered via CFHostSetClient.
            let (callout, ctx) = {
                let h = env.objc.borrow::<CFHostHostObject>(host);
                (h.callout, h.context)
            };
            if let Some(cb) = callout {
                log_dbg!(
                    "CFHostStartInfoResolution: firing client callout {:?}",
                    cb
                );
                // Signature: CFHostClientCallBack(CFHostRef, CFHostInfoType,
                //                                 const CFStreamError*, void* info)
                // We pass a null error pointer to signal success.
                let error_ptr = MutVoidPtr::null();
                // ИСПРАВЛЕНИЕ: Явно указываем пустой тип : () 
                let _: () = cb.call_from_host(env, (host, info, error_ptr, ctx));
            }

            true
        }
        Err(e) => {
            log!(
                "CFHostStartInfoResolution: DNS lookup for '{}' failed: {}",
                name_str, e
            );
            // Write a non-zero error if the caller gave us an error pointer.
            if !error.is_null() {
                // CFStreamError is domain (i32) + error (i32).
                // Domain 1 = kCFStreamErrorDomainPOSIX.
                let posix_err: u64 = (1u64) | ((e.raw_os_error().unwrap_or(0) as u64) << 32);
                env.mem.write(error.cast::<u64>(), posix_err);
            }
            false
        }
    }
}


fn CFHostCancelInfoResolution(
    _env: &mut Environment,
    _host: CFHostRef,
    _info: CFHostInfoType,
) {
    log!("CFHostCancelInfoResolution: stubbed");
}

fn CFHostScheduleWithRunLoop(
    _env: &mut Environment,
    _host: CFHostRef,
    _run_loop: CFTypeRef,
    _run_loop_mode: CFTypeRef,
) {
    log!("CFHostScheduleWithRunLoop: stubbed");
}

fn CFHostUnscheduleFromRunLoop(
    _env: &mut Environment,
    _host: CFHostRef,
    _run_loop: CFTypeRef,
    _run_loop_mode: CFTypeRef,
) {
    log!("CFHostUnscheduleFromRunLoop: stubbed");
}

fn CFHostSetClient(
    env: &mut Environment,
    host: CFHostRef,
    client_cb: MutVoidPtr,  // CFHostClientCallBack (guest fn ptr)
    client_ctx: MutVoidPtr, // CFHostClientContext* — we only need the info pointer
) -> bool {
    if host.is_null() {
        return false;
    }

    let (callout, context) = if client_cb.is_null() {
        // Passing NULL clears the client.
        (None, MutVoidPtr::null())
    } else {
        let cb = GuestFunction::from_addr_with_thumb_bit(client_cb.to_bits());
        let ctx_info: MutVoidPtr = if !client_ctx.is_null() {
            let info_ptr: crate::mem::MutPtr<MutVoidPtr> =
                crate::mem::Ptr::from_bits(client_ctx.to_bits() + 4);
            env.mem.read(info_ptr)
        } else {
            MutVoidPtr::null()
        };
        (Some(cb), ctx_info)
    };

    let h = env.objc.borrow_mut::<CFHostHostObject>(host);
    h.callout = callout;
    h.context = context;
    log_dbg!(
        "CFHostSetClient: callout={:?} context={:?}",
        callout, context
    );
    true
}

// MARK: - Info accessors (always return nil / false)

fn CFHostGetAddressing(
    env: &mut Environment,
    host: CFHostRef,
    has_been_resolved: MutVoidPtr, // Boolean* (u8*)
) -> CFTypeRef {
    if host.is_null() {
        return nil;
    }

    let resolved = env.objc.borrow::<CFHostHostObject>(host).resolved.clone();
    let did_resolve = !resolved.is_empty();
    if !has_been_resolved.is_null() {
        env.mem.write(has_been_resolved.cast::<u8>(), did_resolve as u8);
    }

    if !did_resolve {
        return nil;
    }

    // Build a CFArray of CFData objects, each holding a struct sockaddr_in
    // (16 bytes, matching the guest sockaddr layout).
    use crate::frameworks::core_foundation::cf_array::CFArrayCreateMutable;
    use crate::frameworks::core_foundation::cf_data::CFDataCreate;
    use crate::libc::sys::socket::sockaddr;

    let array = CFArrayCreateMutable(
        env, 
        crate::frameworks::core_foundation::cf_allocator::kCFAllocatorDefault, 
        0, 
        crate::mem::ConstVoidPtr::null()
    );
    for addr in &resolved {
        let sa = sockaddr::from_ipv4_parts(addr.ip().octets(), addr.port());
        let sa_size = std::mem::size_of::<sockaddr>() as u32;
        let buf: crate::mem::MutPtr<u8> = env.mem.alloc(sa_size).cast();
        env.mem.write(buf.cast::<sockaddr>(), sa);
        
        // ИСПРАВЛЕНИЕ: добавлены правильные аргументы и касты
        let data = CFDataCreate(
            env, 
            crate::frameworks::core_foundation::cf_allocator::kCFAllocatorDefault, 
            buf.cast_const().cast(), 
            sa_size as i32
        );
        crate::frameworks::core_foundation::cf_array::CFArrayAppendValue(env, array, data.cast().cast_const());
        crate::frameworks::core_foundation::CFRelease(env, data);
        env.mem.free(buf.cast());
    }

    array
}

fn CFHostGetNames(
    _env: &mut Environment,
    _host: CFHostRef,
    _has_been_resolved: MutVoidPtr, // Boolean*
) -> CFTypeRef {
    // Returns CFArrayRef of CFStringRef names.
    nil
}

fn CFHostGetReachability(
    _env: &mut Environment,
    _host: CFHostRef,
    _has_been_resolved: MutVoidPtr, // Boolean*
) -> CFTypeRef {
    // Returns CFDataRef reachability flags.
    nil
}

fn CFHostIsInfoResolved(
    env: &mut Environment,
    host: CFHostRef,
    info: CFHostInfoType,
) -> bool {
    if host.is_null() {
        return false;
    }
    if info != kCFHostAddresses {
        return false;
    }
    !env.objc.borrow::<CFHostHostObject>(host).resolved.is_empty()
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(CFHostRetain(_)),
    export_c_func!(CFHostRelease(_)),
    export_c_func!(CFHostCreateWithName(_, _)),
    export_c_func!(CFHostCreateWithAddress(_, _)),
    export_c_func!(CFHostCreateCopy(_, _)),
    export_c_func!(CFHostStartInfoResolution(_, _, _)),
    export_c_func!(CFHostCancelInfoResolution(_, _)),
    export_c_func!(CFHostScheduleWithRunLoop(_, _, _)),
    export_c_func!(CFHostUnscheduleFromRunLoop(_, _, _)),
    export_c_func!(CFHostSetClient(_, _, _)),
    export_c_func!(CFHostGetAddressing(_, _)),
    export_c_func!(CFHostGetNames(_, _)),
    export_c_func!(CFHostGetReachability(_, _)),
    export_c_func!(CFHostIsInfoResolved(_, _)),
];

