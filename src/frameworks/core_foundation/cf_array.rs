/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `CFArray` and `CFMutableArray`.
//!
//! These are toll-free bridged to `NSArray` and `NSMutableArray` in Apple's
//! implementation. Here they are the same types.

use super::cf_allocator::{kCFAllocatorDefault, CFAllocatorRef};
use super::{CFIndex, CFRelease, CFRetain, CFTypeRef};
use crate::abi::GuestFunction;
use crate::dyld::{export_c_func, FunctionExports};
use crate::frameworks::foundation::NSUInteger;
use crate::mem::{ConstPtr, ConstVoidPtr, MutPtr, MutVoidPtr};
use crate::objc::{id, msg, msg_class, nil};
use crate::Environment;

pub type CFArrayRef        = CFTypeRef;
pub type CFMutableArrayRef = CFTypeRef;

// CFRange structure
#[repr(C)]
pub struct CFRange {
    pub location: CFIndex,
    pub length: CFIndex,
}

// Constants
pub const K_CF_NOT_FOUND: CFIndex = -1;

// CFArrayCallBacks — we accept the pointer but don't use callbacks.
// A null pointer means "no callbacks" (non-retaining).
// The kCFTypeArrayCallBacks constant has a non-null pointer but we
// ignore the actual callbacks and use NSArray's retain/release instead.

// MARK: - Helper functions for safe range validation

fn validate_range(array_count: CFIndex, location: CFIndex, length: CFIndex) -> bool {
    if location < 0 || length < 0 {
        return false;
    }
    if location > array_count {
        return false;
    }
    if location + length > array_count {
        return false;
    }
    true
}

fn safe_index(idx: CFIndex) -> Result<NSUInteger, ()> {
    if idx < 0 {
        Err(())
    } else {
        idx.try_into().map_err(|_| ())
    }
}

// MARK: - Retain / Release

pub fn CFArrayRetain(env: &mut Environment, arr: CFArrayRef) -> CFArrayRef {
    if !arr.is_null() { 
        CFRetain(env, arr) 
    } else { 
        arr 
    }
}

pub fn CFArrayRelease(env: &mut Environment, arr: CFArrayRef) {
    if !arr.is_null() { 
        CFRelease(env, arr);
    }
}

// MARK: - Immutable constructors

pub fn CFArrayCreate(
    env: &mut Environment,
    _allocator: CFAllocatorRef,
    values: ConstPtr<ConstVoidPtr>,
    num_values: CFIndex,
    _callbacks: ConstVoidPtr,
) -> CFArrayRef {
    if values.is_null() && num_values > 0 {
        return nil;
    }
    
    let arr: id = msg_class![env; NSMutableArray new];
    for i in 0..num_values.max(0) as u32 {
        let val: id = env.mem.read(values + i).cast().cast_mut();
        if !val.is_null() {
            () = msg![env; arr addObject:val];
        }
    }
    
    // Return an immutable copy.
    let immutable: id = msg![env; arr copy];
    crate::objc::release(env, arr);
    immutable
}

pub fn CFArrayCreateCopy(
    env: &mut Environment,
    _allocator: CFAllocatorRef,
    the_array: CFArrayRef,
) -> CFArrayRef {
    if the_array.is_null() { 
        return nil;
    }
    msg![env; the_array copy]
}

// MARK: - Mutable constructors

pub fn CFArrayCreateMutable(
    env: &mut Environment,
    _allocator: CFAllocatorRef,
    capacity: CFIndex,
    callbacks: ConstVoidPtr,
) -> CFMutableArrayRef {
    // capacity hint is ignored — NSMutableArray grows dynamically.
    let _ = capacity;
    
    if callbacks.is_null() {
        msg_class![env; _touchHLE_NSMutableArray_non_retaining new]
    } else {
        // Non-null callbacks → use a retaining mutable array.
        msg_class![env; NSMutableArray new]
    }
}

pub fn CFArrayCreateMutableCopy(
    env: &mut Environment,
    _allocator: CFAllocatorRef,
    capacity: CFIndex,
    the_array: CFArrayRef,
) -> CFMutableArrayRef {
    let _ = capacity; // Capacity hint ignored
    
    if the_array.is_null() {
        return msg_class![env; NSMutableArray new];
    }
    
    let copy: id = msg![env; the_array mutableCopy];
    copy
}

// MARK: - Queries

pub fn CFArrayGetCount(env: &mut Environment, array: CFArrayRef) -> CFIndex {
    if array.is_null() { 
        return 0;
    }
    
    let count: NSUInteger = msg![env; array count];
    count.try_into().unwrap_or(0)
}

pub fn CFArrayGetValueAtIndex(
    env: &mut Environment,
    array: CFArrayRef,
    idx: CFIndex,
) -> ConstVoidPtr {
    if array.is_null() {
        return ConstVoidPtr::null();
    }
    
    let count = CFArrayGetCount(env, array);
    if idx < 0 || idx >= count {
        return ConstVoidPtr::null();
    }
    
    let idx_u: NSUInteger = idx.try_into().unwrap();
    let value: id = msg![env; array objectAtIndex:idx_u];
    value.cast().cast_const()
}

pub fn CFArrayGetValues(
    env: &mut Environment,
    array: CFArrayRef,
    range_location: CFIndex,
    range_length: CFIndex,
    values: MutPtr<ConstVoidPtr>,
) {
    if array.is_null() || values.is_null() { 
        return;
    }
    
    let count = CFArrayGetCount(env, array);
    if !validate_range(count, range_location, range_length) {
        return;
    }
    
    for i in 0..range_length as u32 {
        let idx = (range_location as u32) + i;
        let val: id = msg![env; array objectAtIndex:idx];
        env.mem.write(values + i, val.cast().cast_const());
    }
}

pub fn CFArrayContainsValue(
    env: &mut Environment,
    array: CFArrayRef,
    range_location: CFIndex,
    range_length: CFIndex,
    value: ConstVoidPtr,
) -> bool {
    if array.is_null() || value.is_null() { 
        return false;
    }
    
    let count = CFArrayGetCount(env, array);
    if !validate_range(count, range_location, range_length) {
        return false;
    }
    
    let val: id = value.cast().cast_mut();
    let end = range_location + range_length;
    for i in range_location..end {
        let item: id = msg![env; array objectAtIndex:(i as NSUInteger)];
        let eq: bool = msg![env; item isEqual:val];
        if eq { 
            return true;
        }
    }
    
    false
}

pub fn CFArrayGetFirstIndexOfValue(
    env: &mut Environment,
    array: CFArrayRef,
    range_location: CFIndex,
    range_length: CFIndex,
    value: ConstVoidPtr,
) -> CFIndex {
    if array.is_null() || value.is_null() { 
        return K_CF_NOT_FOUND;
    }
    
    let count = CFArrayGetCount(env, array);
    if !validate_range(count, range_location, range_length) {
        return K_CF_NOT_FOUND;
    }
    
    let val: id = value.cast().cast_mut();
    let end = range_location + range_length;
    for i in range_location..end {
        let item: id = msg![env; array objectAtIndex:(i as NSUInteger)];
        let eq: bool = msg![env; item isEqual:val];
        if eq { 
            return i;
        }
    }
    
    K_CF_NOT_FOUND
}

pub fn CFArrayGetLastIndexOfValue(
    env: &mut Environment,
    array: CFArrayRef,
    range_location: CFIndex,
    range_length: CFIndex,
    value: ConstVoidPtr,
) -> CFIndex {
    if array.is_null() || value.is_null() { 
        return K_CF_NOT_FOUND;
    }
    
    let count = CFArrayGetCount(env, array);
    if !validate_range(count, range_location, range_length) {
        return K_CF_NOT_FOUND;
    }
    
    let val: id = value.cast().cast_mut();
    let end = range_location + range_length;
    let mut last = K_CF_NOT_FOUND;
    
    for i in range_location..end {
        let item: id = msg![env; array objectAtIndex:(i as NSUInteger)];
        let eq: bool = msg![env; item isEqual:val];
        if eq { 
            last = i;
        }
    }
    
    last
}

pub fn CFArrayGetCountOfValue(
    env: &mut Environment,
    array: CFArrayRef,
    range_location: CFIndex,
    range_length: CFIndex,
    value: ConstVoidPtr,
) -> CFIndex {
    if array.is_null() || value.is_null() { 
        return 0;
    }
    
    let count = CFArrayGetCount(env, array);
    if !validate_range(count, range_location, range_length) {
        return 0;
    }
    
    let val: id = value.cast().cast_mut();
    let end = range_location + range_length;
    let mut found_count = 0;
    
    for i in range_location..end {
        let item: id = msg![env; array objectAtIndex:(i as NSUInteger)];
        let eq: bool = msg![env; item isEqual:val];
        if eq { 
            found_count += 1;
        }
    }
    
    found_count
}

// MARK: - Binary search

pub fn CFArrayBSearchValues(
    env: &mut Environment,
    array: CFArrayRef,
    range_location: CFIndex,
    range_length: CFIndex,
    value: ConstVoidPtr,
    comparator: GuestFunction, // CFComparatorFunction
    context: MutVoidPtr,
) -> CFIndex {
    use crate::abi::CallFromHost;
    if array.is_null() || value.is_null() {
        return K_CF_NOT_FOUND;
    }
    
    let count = CFArrayGetCount(env, array);
    if !validate_range(count, range_location, range_length) {
        return K_CF_NOT_FOUND;
    }
    
    if range_length == 0 {
        return range_location;
    }
    
    // Binary search implementation
    let mut low = range_location;
    let mut high = range_location + range_length - 1;
    
    while low <= high {
        let mid = low + (high - low) / 2;
        let item: id = msg![env; array objectAtIndex:(mid as NSUInteger)];
        
        let cmp: i32 = comparator.call_from_host(
            env,
            (value, item.cast::<std::ffi::c_void>().cast_const(), context),
        );
        if cmp == 0 {
            return mid; // Found exact match
        } else if cmp < 0 {
            high = mid - 1;
        } else {
            low = mid + 1;
        }
    }
    
    // Return insertion point
    low
}

// MARK: - Mutation

pub fn CFArrayAppendValue(
    env: &mut Environment, 
    array: CFMutableArrayRef, 
    value: ConstVoidPtr,
) {
    if array.is_null() || value.is_null() {
        return;
    }
    
    let value: id = value.cast().cast_mut();
    () = msg![env; array addObject:value];
}

pub fn CFArrayInsertValueAtIndex(
    env: &mut Environment,
    array: CFMutableArrayRef,
    idx: CFIndex,
    value: ConstVoidPtr,
) {
    if array.is_null() || value.is_null() {
        return;
    }
    
    let count = CFArrayGetCount(env, array);
    if idx < 0 || idx > count {
        return;
    }
    
    let idx_u: NSUInteger = idx.try_into().unwrap();
    let val: id = value.cast().cast_mut();
    () = msg![env; array insertObject:val atIndex:idx_u];
}

pub fn CFArraySetValueAtIndex(
    env: &mut Environment,
    array: CFMutableArrayRef,
    idx: CFIndex,
    value: ConstVoidPtr,
) {
    if array.is_null() || value.is_null() {
        return;
    }
    
    let count = CFArrayGetCount(env, array);
    if idx < 0 || idx >= count {
        return;
    }
    
    let idx_u: NSUInteger = idx.try_into().unwrap();
    let val: id = value.cast().cast_mut();
    () = msg![env; array replaceObjectAtIndex:idx_u withObject:val];
}

pub fn CFArrayRemoveValueAtIndex(
    env: &mut Environment,
    array: CFMutableArrayRef,
    idx: CFIndex,
) {
    if array.is_null() {
        return;
    }
    
    let count = CFArrayGetCount(env, array);
    if idx < 0 || idx >= count {
        return;
    }
    
    let idx_u: NSUInteger = idx.try_into().unwrap();
    () = msg![env; array removeObjectAtIndex:idx_u];
}

pub fn CFArrayRemoveAllValues(env: &mut Environment, array: CFMutableArrayRef) {
    if array.is_null() {
        return;
    }
    
    () = msg![env; array removeAllObjects];
}

pub fn CFArrayAppendArray(
    env: &mut Environment,
    array: CFMutableArrayRef,
    other: CFArrayRef,
    range_location: CFIndex,
    range_length: CFIndex,
) {
    if array.is_null() || other.is_null() { 
        return;
    }
    
    let count = CFArrayGetCount(env, other);
    if !validate_range(count, range_location, range_length) {
        return;
    }
    
    let end = range_location + range_length;
    for i in range_location..end {
        let val: id = msg![env; other objectAtIndex:(i as NSUInteger)];
        () = msg![env; array addObject:val];
    }
}

pub fn CFArrayReplaceValues(
    env: &mut Environment,
    array: CFMutableArrayRef,
    range_location: CFIndex,
    range_length: CFIndex,
    new_values: ConstPtr<ConstVoidPtr>,
    new_count: CFIndex,
) {
    if array.is_null() {
        return;
    }
    
    let count = CFArrayGetCount(env, array);
    if !validate_range(count, range_location, range_length) {
        return;
    }
    
    if new_count > 0 && new_values.is_null() {
        return;
    }
    
    // Remove old range (in reverse order to maintain indices).
    for i in (range_location..range_location + range_length).rev() {
        () = msg![env; array removeObjectAtIndex:(i as NSUInteger)];
    }
    
    // Insert new values at range_location.
    for i in 0..new_count.max(0) as u32 {
        let val: id = env.mem.read(new_values + i).cast().cast_mut();
        if !val.is_null() {
            let idx = (range_location as u32 + i) as NSUInteger;
            () = msg![env; array insertObject:val atIndex:idx];
        }
    }
}

pub fn CFArrayExchangeValuesAtIndices(
    env: &mut Environment,
    array: CFMutableArrayRef,
    idx1: CFIndex,
    idx2: CFIndex,
) {
    if array.is_null() {
        return;
    }
    
    let count = CFArrayGetCount(env, array);
    if idx1 < 0 || idx1 >= count || idx2 < 0 || idx2 >= count {
        return;
    }
    
    if idx1 == idx2 {
        return; // Nothing to swap
    }
    
    let i1 = idx1 as NSUInteger;
    let i2 = idx2 as NSUInteger;
    
    let v1: id = msg![env; array objectAtIndex:i1];
    let v2: id = msg![env; array objectAtIndex:i2];
    // Retain temporarily to prevent premature deallocation
    crate::objc::retain(env, v1);
    crate::objc::retain(env, v2);
    
    () = msg![env; array replaceObjectAtIndex:i1 withObject:v2];
    () = msg![env; array replaceObjectAtIndex:i2 withObject:v1];
    
    crate::objc::release(env, v1);
    crate::objc::release(env, v2);
}

// MARK: - Sorting

pub fn CFArraySortValues(
    env: &mut Environment,
    array: CFMutableArrayRef,
    range_location: CFIndex,
    range_length: CFIndex,
    comparator: GuestFunction, // CFComparatorFunction
    context: MutVoidPtr,
) {
    use crate::abi::CallFromHost;
    if array.is_null() {
        return;
    }
    
    let count = CFArrayGetCount(env, array);
    if !validate_range(count, range_location, range_length) {
        return;
    }
    
    if range_length <= 1 {
        return; // Already sorted
    }
    
    // Extract the range into a temporary vec and sort.
    let end = range_location + range_length;
    let mut items: Vec<id> = (range_location..end)
        .map(|i| {
            let val: id = msg![env; array objectAtIndex:(i as NSUInteger)];
            crate::objc::retain(env, val);
            val
        })
        .collect();
    // Simple insertion sort — avoids the borrow-checker complexity of in-place
    // sort with a closure that borrows env.
    let n = items.len();
    for i in 1..n {
        let mut j = i;
        while j > 0 {
            let a = items[j - 1];
            let b = items[j];
            let res: i32 = comparator.call_from_host(
                env,
                (a.cast::<std::ffi::c_void>().cast_const(), b.cast::<std::ffi::c_void>().cast_const(), context),
            );
            if res > 0 {
                items.swap(j - 1, j);
                j -= 1;
            } else {
                break;
            }
        }
    }

    // Write sorted items back.
    for (k, val) in items.iter().enumerate() {
        let idx = (range_location as usize + k) as NSUInteger;
        () = msg![env; array replaceObjectAtIndex:idx withObject:(*val)];
    }
    
    // Release retained items
    for item in items {
        crate::objc::release(env, item);
    }
}

// MARK: - Apply / callbacks

pub fn CFArrayApplyFunction(
    env: &mut Environment,
    array: CFArrayRef,
    range_location: CFIndex,
    range_length: CFIndex,
    applier: GuestFunction, // CFArrayApplierFunction: (value, context) -> void
    context: MutVoidPtr,
) {
    use crate::abi::CallFromHost;
    if array.is_null() {
        return;
    }
    
    let count = CFArrayGetCount(env, array);
    if !validate_range(count, range_location, range_length) {
        return;
    }
    
    let end = range_location + range_length;
    for i in range_location..end {
        let val: id = msg![env; array objectAtIndex:(i as NSUInteger)];
        let _: () = applier.call_from_host(
            env,
            (val.cast::<std::ffi::c_void>().cast_const(), context),
        );
    }
}

// MARK: - Description

pub fn CFArrayCreateDescription(
    env: &mut Environment,
    array: CFArrayRef,
) -> CFTypeRef {
    if array.is_null() { 
        return nil;
    }
    
    msg![env; array description]
}

// MARK: - Additional utility functions

pub fn CFArrayGetTypeID(_env: &mut Environment) -> u32 {
    // Return a fake CFTypeID for CFArray
    // In real implementation, this would be a unique type identifier
    0x43464172 // 'CFAr' in hex
}

pub fn CFMakeCollectable(
    _env: &mut Environment,
    cf: CFTypeRef,
) -> CFTypeRef {
    // In garbage-collected environments, this makes the object collectable
    // In ARC/manual retain-release, this is a no-op
    cf
}

// MARK: - Exports

pub const FUNCTIONS: FunctionExports = &[
    // Lifecycle
    export_c_func!(CFArrayRetain(_)),
    export_c_func!(CFArrayRelease(_)),
    
    // Immutable constructors
    export_c_func!(CFArrayCreate(_, _, _, _)),
    export_c_func!(CFArrayCreateCopy(_, _)),
    
    // Mutable constructors
    export_c_func!(CFArrayCreateMutable(_, _, _)),
    export_c_func!(CFArrayCreateMutableCopy(_, _, _)),
    
    // Queries
    export_c_func!(CFArrayGetCount(_)),
    export_c_func!(CFArrayGetValueAtIndex(_, _)),
    export_c_func!(CFArrayGetValues(_, _, _, _)),
    export_c_func!(CFArrayContainsValue(_, _, _, _)),
    export_c_func!(CFArrayGetFirstIndexOfValue(_, _, _, _)),
    export_c_func!(CFArrayGetLastIndexOfValue(_, _, _, _)),
    export_c_func!(CFArrayGetCountOfValue(_, _, _, _)),
    
    // Binary search
    export_c_func!(CFArrayBSearchValues(_, _, _, _, _, _)),
    
    // Mutation
    export_c_func!(CFArrayAppendValue(_, _)),
    export_c_func!(CFArrayInsertValueAtIndex(_, _, _)),
    export_c_func!(CFArraySetValueAtIndex(_, _, _)),
    export_c_func!(CFArrayRemoveValueAtIndex(_, _)),
    export_c_func!(CFArrayRemoveAllValues(_)),
    export_c_func!(CFArrayAppendArray(_, _, _, _)),
    export_c_func!(CFArrayReplaceValues(_, _, _, _, _)),
    export_c_func!(CFArrayExchangeValuesAtIndices(_, _, _)),
    
    // Sorting / applying
    export_c_func!(CFArraySortValues(_, _, _, _, _)),
    export_c_func!(CFArrayApplyFunction(_, _, _, _, _)),
    
    // Description
    export_c_func!(CFArrayCreateDescription(_)),
    
    // Type info and utilities
    export_c_func!(CFArrayGetTypeID()),
    export_c_func!(CFMakeCollectable(_)),
];

