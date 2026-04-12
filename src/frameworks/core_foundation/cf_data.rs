/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 * If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//!
//! `CFData` and `CFMutableData`.
//!
//! These are toll-free bridged to `NSData` and `NSMutableData` in Apple's
//! implementation.
//! Here they are the same types.

use super::cf_allocator::{kCFAllocatorDefault, CFAllocatorRef};
use super::{CFIndex, CFRange, CFRelease, CFRetain, CFTypeRef};
use crate::dyld::FunctionExports;
use crate::export_c_func;
use crate::frameworks::foundation::{NSRange, NSUInteger};
use crate::mem::{ConstPtr, ConstVoidPtr, MutPtr, MutVoidPtr};
use crate::objc::{id, msg, msg_class, nil};
use crate::Environment;

pub type CFDataRef        = CFTypeRef;
pub type CFMutableDataRef = CFTypeRef;

// MARK: - CFData (immutable)

pub fn CFDataCreate(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    bytes: ConstPtr<u8>,
    length: CFIndex,
) -> CFDataRef {
    assert!(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default());
    let bytes: ConstVoidPtr = bytes.cast();
    let length: NSUInteger  = length.try_into().unwrap();
    let new: id = msg_class![env; NSData alloc];
    msg![env; new initWithBytes:bytes length:length]
}

pub fn CFDataCreateCopy(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    data: CFDataRef,
) -> CFDataRef {
    assert!(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default());
    let bytes: ConstVoidPtr = msg![env; data bytes];
    let length: NSUInteger  = msg![env; data length];
    let new: id = msg_class![env; NSData alloc];
    msg![env; new initWithBytes:bytes length:length]
}

fn CFDataCreateWithBytesNoCopy(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    bytes: ConstPtr<u8>,
    length: CFIndex,
    _bytes_deallocator: CFAllocatorRef, // ignored — we copy anyway
) -> CFDataRef {
    assert!(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default());
    // We don't support custom deallocators; just copy the bytes.
    CFDataCreate(env, kCFAllocatorDefault, bytes, length)
}

pub fn CFDataGetLength(env: &mut Environment, data: CFDataRef) -> CFIndex {
    let len: NSUInteger = msg![env; data length];
    len.try_into().unwrap()
}

pub fn CFDataGetBytePtr(env: &mut Environment, data: CFDataRef) -> ConstPtr<u8> {
    let ptr: ConstVoidPtr = msg![env; data bytes];
    ptr.cast()
}

fn CFDataGetBytes(
    env: &mut Environment,
    data: CFDataRef,
    range: CFRange,
    buffer: MutPtr<u8>,
) {
    let range = NSRange {
        location: range.location.try_into().unwrap(),
        length:   range.length.try_into().unwrap(),
    };
    msg![env; data getBytes:buffer range:range]
}

fn CFDataGetMutableBytePtr(env: &mut Environment, data: CFMutableDataRef) -> MutPtr<u8> {
    let ptr: MutVoidPtr = msg![env; data mutableBytes];
    ptr.cast()
}

// MARK: - CFData retain / release

pub fn CFDataRetain(env: &mut Environment, data: CFDataRef) -> CFDataRef {
    if !data.is_null() { CFRetain(env, data) } else { data }
}

pub fn CFDataRelease(env: &mut Environment, data: CFDataRef) {
    if !data.is_null() { CFRelease(env, data); }
}

fn CFDataIsEqual(env: &mut Environment, a: CFDataRef, b: CFDataRef) -> bool {
    if a == b { return true; }
    if a.is_null() || b.is_null() { return false; }
    let len_a: NSUInteger = msg![env; a length];
    let len_b: NSUInteger = msg![env; b length];
    if len_a != len_b { return false; }
    msg![env; a isEqualToData:b]
}

// MARK: - CFMutableData

fn CFDataCreateMutable(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    capacity: CFIndex,
) -> CFMutableDataRef {
    assert!(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default());
    let new: id = msg_class![env; NSMutableData alloc];
    if capacity > 0 {
        let cap: NSUInteger = capacity.try_into().unwrap();
        msg![env; new initWithCapacity:cap]
    } else {
        msg![env; new init]
    }
}

fn CFDataCreateMutableCopy(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    _capacity: CFIndex,
    data: CFDataRef,
) -> CFMutableDataRef {
    assert!(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default());
    let new: id = msg_class![env; NSMutableData alloc];
    msg![env; new initWithData:data]
}

fn CFDataSetLength(
    env: &mut Environment,
    data: CFMutableDataRef,
    length: CFIndex,
) {
    let len: NSUInteger = length.try_into().unwrap();
    msg![env; data setLength:len]
}

fn CFDataIncreaseLength(
    env: &mut Environment,
    data: CFMutableDataRef,
    extra_length: CFIndex,
) {
    let current: NSUInteger = msg![env; data length];
    let extra: NSUInteger   = extra_length.try_into().unwrap();
    let new_len             = current + extra;
    msg![env; data setLength:new_len]
}

fn CFDataAppendBytes(
    env: &mut Environment,
    data: CFMutableDataRef,
    bytes: ConstPtr<u8>,
    length: CFIndex,
) {
    let len: NSUInteger = length.try_into().unwrap();
    msg![env; data appendBytes:(bytes.cast::<std::ffi::c_void>()) length:len]
}

fn CFDataDeleteBytes(
    env: &mut Environment,
    data: CFMutableDataRef,
    range: CFRange,
) {
    let ns_range = NSRange {
        location: range.location.try_into().unwrap(),
        length:   range.length.try_into().unwrap(),
    };
    msg![env; data replaceBytesInRange:ns_range withBytes:(nil) length:0u32]
}

fn CFDataReplaceBytes(
    env: &mut Environment,
    data: CFMutableDataRef,
    range: CFRange,
    new_bytes: ConstPtr<u8>,
    new_length: CFIndex,
) {
    let ns_range = NSRange {
        location: range.location.try_into().unwrap(),
        length:   range.length.try_into().unwrap(),
    };
    let new_len: NSUInteger = new_length.try_into().unwrap();
    msg![env; data replaceBytesInRange:ns_range
                             withBytes:(new_bytes.cast::<std::ffi::c_void>())
                                length:new_len]
}

pub const FUNCTIONS: FunctionExports = &[
    // Immutable
    export_c_func!(CFDataCreate(_, _, _)),
    export_c_func!(CFDataCreateCopy(_, _)),
    export_c_func!(CFDataCreateWithBytesNoCopy(_, _, _, _)),
    export_c_func!(CFDataGetLength(_)),
    export_c_func!(CFDataGetBytePtr(_)),
    export_c_func!(CFDataGetMutableBytePtr(_)),
    export_c_func!(CFDataGetBytes(_, _, _)),
    export_c_func!(CFDataRetain(_)),
    export_c_func!(CFDataRelease(_)),
    export_c_func!(CFDataIsEqual(_, _)),
    // Mutable
    export_c_func!(CFDataCreateMutable(_, _)),
    export_c_func!(CFDataCreateMutableCopy(_, _, _)),
    export_c_func!(CFDataSetLength(_, _)),
    export_c_func!(CFDataIncreaseLength(_, _)),
    export_c_func!(CFDataAppendBytes(_, _, _)),
    export_c_func!(CFDataDeleteBytes(_, _)),
    export_c_func!(CFDataReplaceBytes(_, _, _, _)),
];

