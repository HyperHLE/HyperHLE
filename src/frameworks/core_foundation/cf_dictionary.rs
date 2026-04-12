/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 * If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//!
//! `CFDictionary` and `CFMutableDictionary`.
//!
//! These are toll-free bridged to `NSDictionary` and `NSMutableDictionary` in
//! Apple's implementation.
//! Here they are the same types.

use super::cf_allocator::{kCFAllocatorDefault, CFAllocatorRef};
use super::{CFHashCode, CFIndex, CFRelease, CFRetain, CFTypeRef};
use crate::abi::GuestFunction;
use crate::dyld::{
    export_c_func, ConstantExports, Dyld, FunctionExports, HostConstant, HostFunction,
};
use crate::frameworks::core_foundation::cf_string::CFStringRef;
use crate::frameworks::core_foundation::cf_type::{CFEqual, CFHash};
use crate::frameworks::foundation::ns_dictionary::{
    CFDictionaryKeyCallBacks, CFDictionaryValueCallBacks,
};
use crate::frameworks::foundation::NSUInteger;
use crate::mem::{ConstPtr, ConstVoidPtr, Mem, MutVoidPtr};
use crate::objc::{id, msg, msg_class, nil};
use crate::Environment;

pub type CFDictionaryRef        = CFTypeRef;
pub type CFMutableDictionaryRef = CFTypeRef;

// MARK: - Retain / Release

pub fn CFDictionaryRetain(env: &mut Environment, dict: CFDictionaryRef) -> CFDictionaryRef {
    if !dict.is_null() { CFRetain(env, dict) } else { dict }
}

pub fn CFDictionaryRelease(env: &mut Environment, dict: CFDictionaryRef) {
    if !dict.is_null() { CFRelease(env, dict); }
}

// MARK: - Constructors

fn CFDictionaryCreate(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    keys:   ConstPtr<ConstVoidPtr>,
    values: ConstPtr<ConstVoidPtr>,
    count: CFIndex,
    key_callbacks:   ConstPtr<CFDictionaryKeyCallBacks>,
    value_callbacks: ConstPtr<CFDictionaryValueCallBacks>,
) -> CFDictionaryRef {
    // Build a mutable dict then return it — immutability not enforced here.
    let dict = CFDictionaryCreateMutable(env, allocator, 0, key_callbacks, value_callbacks);
    for i in 0..count as u32 {
        let key:   ConstVoidPtr = env.mem.read(keys   + i);
        let value: ConstVoidPtr = env.mem.read(values + i);
        CFDictionarySetValue(env, dict, key, value);
    }
    dict
}

fn CFDictionaryCreateMutable(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    capacity: CFIndex,
    key_callbacks:   ConstPtr<CFDictionaryKeyCallBacks>,
    value_callbacks: ConstPtr<CFDictionaryValueCallBacks>,
) -> CFMutableDictionaryRef {
    assert!(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default());
    // capacity hint ignored — NSMutableDictionary grows dynamically.
    let _ = capacity;
    let new = msg_class![env; _touchHLE_NSMutableDictionary_non_retaining alloc];
    msg![env; new initWithKeyCallbacks:key_callbacks andValueCallbacks:value_callbacks]
}

fn CFDictionaryCreateCopy(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    dict: CFDictionaryRef,
) -> CFDictionaryRef {
    assert!(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default());
    let new: id = msg_class![env; NSDictionary alloc];
    msg![env; new initWithDictionary:dict]
}

fn CFDictionaryCreateMutableCopy(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    _capacity: CFIndex,
    dict: CFDictionaryRef,
) -> CFMutableDictionaryRef {
    assert!(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default());
    let new: id = msg_class![env; NSMutableDictionary alloc];
    msg![env; new initWithDictionary:dict]
}

// MARK: - Mutation

fn CFDictionaryAddValue(
    env: &mut Environment,
    dict: CFMutableDictionaryRef,
    key:   ConstVoidPtr,
    value: ConstVoidPtr,
) {
    let key_id: id = key.cast().cast_mut();
    let res: id    = msg![env; dict objectForKey:key_id];
    log_dbg!("CFDictionaryAddValue k {:?} v {:?}; exists={}", key, value, res != nil);
    if res == nil {
        let value_id: id = value.cast().cast_mut();
        msg![env; dict setObject:value_id forKey:key_id]
    }
}

fn CFDictionarySetValue(
    env: &mut Environment,
    dict: CFMutableDictionaryRef,
    key:   ConstVoidPtr,
    value: ConstVoidPtr,
) {
    log_dbg!("CFDictionarySetValue k {:?} v {:?}", key, value);
    let key_id:   id = key.cast().cast_mut();
    let value_id: id = value.cast().cast_mut();
    msg![env; dict setObject:value_id forKey:key_id]
}

fn CFDictionaryReplaceValue(
    env: &mut Environment,
    dict: CFMutableDictionaryRef,
    key:   ConstVoidPtr,
    value: ConstVoidPtr,
) {
    // Only replaces if the key already exists.
    let key_id: id = key.cast().cast_mut();
    let existing: id = msg![env; dict objectForKey:key_id];
    if existing != nil {
        let value_id: id = value.cast().cast_mut();
        msg![env; dict setObject:value_id forKey:key_id]
    }
}

fn CFDictionaryRemoveValue(
    env: &mut Environment,
    dict: CFMutableDictionaryRef,
    key: ConstVoidPtr,
) {
    let key_id: id = key.cast().cast_mut();
    log_dbg!("CFDictionaryRemoveValue key {:?}", key);
    () = msg![env; dict removeObjectForKey:key_id];
}

fn CFDictionaryRemoveAllValues(env: &mut Environment, dict: CFMutableDictionaryRef) {
    let keys_arr: id = msg![env; dict allKeys];
    let enumerator: id = msg![env; keys_arr objectEnumerator];
    loop {
        let key: id = msg![env; enumerator nextObject];
        if key == nil { break; }
        CFDictionaryRemoveValue(env, dict, key.cast().cast_const());
    }
}

// MARK: - Queries

fn CFDictionaryGetValue(
    env: &mut Environment,
    dict: CFDictionaryRef,
    key: ConstVoidPtr,
) -> ConstVoidPtr {
    let key_id: id = key.cast().cast_mut();
    let res: id    = msg![env; dict objectForKey:key_id];
    res.cast().cast_const()
}

fn CFDictionaryGetValueIfPresent(
    env: &mut Environment,
    dict: CFDictionaryRef,
    key:   ConstVoidPtr,
    value: MutVoidPtr, // void** — out param
) -> bool {
    let key_id: id = key.cast().cast_mut();
    let res: id    = msg![env; dict objectForKey:key_id];
    if res != nil {
        if !value.is_null() {
            env.mem.write(value.cast::<ConstVoidPtr>(), res.cast().cast_const());
        }
        true
    } else {
        false
    }
}

fn CFDictionaryContainsKey(
    env: &mut Environment,
    dict: CFDictionaryRef,
    key: ConstVoidPtr,
) -> bool {
    let key_id: id = key.cast().cast_mut();
    let res: id    = msg![env; dict objectForKey:key_id];
    res != nil
}

fn CFDictionaryContainsValue(
    env: &mut Environment,
    dict: CFDictionaryRef,
    value: ConstVoidPtr,
) -> bool {
    let value_id: id   = value.cast().cast_mut();
    let values_arr: id = msg![env; dict allValues];
    let count: NSUInteger = msg![env; values_arr count];
    for i in 0..count {
        let v: id = msg![env; values_arr objectAtIndex:i];
        let eq: bool = msg![env; v isEqual:value_id];
        if eq { return true; }
    }
    false
}

fn CFDictionaryGetCount(env: &mut Environment, dict: CFDictionaryRef) -> CFIndex {
    let count: NSUInteger = msg![env; dict count];
    log_dbg!("CFDictionaryGetCount -> {}", count);
    count.try_into().unwrap()
}

fn CFDictionaryGetKeysAndValues(
    env: &mut Environment,
    dict: CFDictionaryRef,
    keys:   ConstPtr<MutVoidPtr>,
    values: ConstPtr<MutVoidPtr>,
) {
    let mut key_ptr = keys.cast_mut();
    let mut val_ptr = values.cast_mut();
    let keys_arr: id   = msg![env; dict allKeys];
    let enumerator: id = msg![env; keys_arr objectEnumerator];
    loop {
        let key: id = msg![env; enumerator nextObject];
        if key == nil { break; }
        if !key_ptr.is_null() {
            env.mem.write(key_ptr, key.cast());
            key_ptr += 1;
        }
        if !val_ptr.is_null() {
            let val: id = msg![env; dict objectForKey:key];
            env.mem.write(val_ptr, val.cast());
            val_ptr += 1;
        }
    }
}

fn CFDictionaryApplyFunction(
    env: &mut Environment,
    dict: CFDictionaryRef,
    applier: GuestFunction, // CFDictionaryApplierFunction: (key, value, context) -> void
    context: MutVoidPtr,
) {
    use crate::abi::CallFromHost;
    let keys_arr: id   = msg![env; dict allKeys];
    let count: NSUInteger = msg![env; keys_arr count];
    for i in 0..count {
        let key: id = msg![env; keys_arr objectAtIndex:i];
        let val: id = msg![env; dict objectForKey:key];
        let args: (ConstVoidPtr, ConstVoidPtr, MutVoidPtr) = (
            key.cast().cast_const(),
            val.cast().cast_const(),
            context,
        );
        () = applier.call_from_host(env, args);
    }
}

fn CFDictionaryCreateWithCopy(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    dict: CFDictionaryRef,
) -> CFDictionaryRef {
    CFDictionaryCreateCopy(env, allocator, dict)
}

// MARK: - Default callbacks (unchanged from original)

fn _touchHLE_CFDictionary_retain(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    value: ConstVoidPtr,
) -> ConstVoidPtr {
    assert!(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default());
    CFRetain(env, value.cast_mut().cast()).cast_const().cast()
}
fn _touchHLE_CFDictionary_release(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    value: ConstVoidPtr,
) {
    assert!(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default());
    CFRelease(env, value.cast_mut().cast());
}
fn _touchHLE_CFDictionary_copyDescription(
    _env: &mut Environment,
    _value: ConstVoidPtr,
) -> CFStringRef {
    todo!()
}
fn _touchHLE_CFDictionary_equal(
    env: &mut Environment,
    value1: ConstVoidPtr,
    value2: ConstVoidPtr,
) -> bool {
    CFEqual(env, value1.cast_mut().cast(), value2.cast_mut().cast())
}
fn _touchHLE_CFDictionary_hash(env: &mut Environment, value: ConstVoidPtr) -> CFHashCode {
    CFHash(env, value.cast_mut().cast())
}

struct DefaultCallbackFunctions {
    retain:    GuestFunction,
    release:   GuestFunction,
    copy_desc: GuestFunction,
    equal:     GuestFunction,
    hash:      GuestFunction,
}
fn create_default_callback_functions(mem: &mut Mem, dyld: &mut Dyld) -> DefaultCallbackFunctions {
    macro_rules! make_gf {
        ($sym:expr, $f:expr, $t:ty) => {{
            let hf: HostFunction = &($f as $t);
            dyld.create_guest_function(mem, $sym, hf)
        }};
    }
    DefaultCallbackFunctions {
        retain:    make_gf!("__touchHLE_CFDictionary_retain",
                            _touchHLE_CFDictionary_retain,
                            fn(&mut Environment, _, _) -> _),
        release:   make_gf!("__touchHLE_CFDictionary_release",
                            _touchHLE_CFDictionary_release,
                            fn(&mut Environment, _, _)),
        copy_desc: make_gf!("__touchHLE_CFDictionary_copyDescription",
                            _touchHLE_CFDictionary_copyDescription,
                            fn(&mut Environment, _) -> _),
        equal:     make_gf!("__touchHLE_CFDictionary_equal",
                            _touchHLE_CFDictionary_equal,
                            fn(&mut Environment, _, _) -> _),
        hash:      make_gf!("__touchHLE_CFDictionary_hash",
                            _touchHLE_CFDictionary_hash,
                            fn(&mut Environment, _) -> _),
    }
}

pub const CONSTANTS: ConstantExports = &[
    (
        "_kCFTypeDictionaryKeyCallBacks",
        HostConstant::Custom(|env| {
            let common = create_default_callback_functions(&mut env.mem, &mut env.dyld);
            let callbacks = CFDictionaryKeyCallBacks {
                version:   0,
                retain:    common.retain,
                release:   common.release,
                copy_desc: common.copy_desc,
                equal:     common.equal,
                hash:      common.hash,
            };
            env.mem.alloc_and_write(callbacks).cast_void().cast_const()
        }),
    ),
    (
        "_kCFTypeDictionaryValueCallBacks",
        HostConstant::Custom(|env| {
            let common = create_default_callback_functions(&mut env.mem, &mut env.dyld);
            let callbacks = CFDictionaryValueCallBacks {
                version:   0,
                retain:    common.retain,
                release:   common.release,
                copy_desc: common.copy_desc,
                equal:     common.equal,
            };
            env.mem.alloc_and_write(callbacks).cast_void().cast_const()
        }),
    ),
    (
        "_kCFCopyStringDictionaryKeyCallBacks",
        HostConstant::Custom(|env| {
            // Same as kCFTypeDictionaryKeyCallBacks for our purposes.
            let common = create_default_callback_functions(&mut env.mem, &mut env.dyld);
            let callbacks = CFDictionaryKeyCallBacks {
                version:   0,
                retain:    common.retain,
                release:   common.release,
                copy_desc: common.copy_desc,
                equal:     common.equal,
                hash:      common.hash,
            };
            env.mem.alloc_and_write(callbacks).cast_void().cast_const()
        }),
    ),
];

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(CFDictionaryRetain(_)),
    export_c_func!(CFDictionaryRelease(_)),
    export_c_func!(CFDictionaryCreate(_, _, _, _, _, _)),
    export_c_func!(CFDictionaryCreateMutable(_, _, _, _)),
    export_c_func!(CFDictionaryCreateCopy(_, _)),
    export_c_func!(CFDictionaryCreateMutableCopy(_, _, _)),
    export_c_func!(CFDictionaryCreateWithCopy(_, _)),
    export_c_func!(CFDictionaryAddValue(_, _, _)),
    export_c_func!(CFDictionarySetValue(_, _, _)),
    export_c_func!(CFDictionaryReplaceValue(_, _, _)),
    export_c_func!(CFDictionaryRemoveValue(_, _)),
    export_c_func!(CFDictionaryRemoveAllValues(_)),
    export_c_func!(CFDictionaryGetValue(_, _)),
    export_c_func!(CFDictionaryGetValueIfPresent(_, _, _)),
    export_c_func!(CFDictionaryContainsKey(_, _)),
    export_c_func!(CFDictionaryContainsValue(_, _)),
    export_c_func!(CFDictionaryGetCount(_)),
    export_c_func!(CFDictionaryGetKeysAndValues(_, _, _)),
    export_c_func!(CFDictionaryApplyFunction(_, _, _)),
];

