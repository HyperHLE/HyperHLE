/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `libkern/OSAtomic.h`
//!
//! Atomic operations.
//!
//! touchHLE is a single-host-thread application, so host functions cannot be
//! interrupted by other threads. All operations are therefore trivially atomic.

use crate::dyld::FunctionExports;
use crate::export_c_func;
use crate::mem::{MutPtr, MutVoidPtr};
use crate::Environment;

// =========================================================================
// MARK: - OSAtomicAdd
// =========================================================================

fn OSAtomicAdd32(env: &mut Environment, amount: i32, value_ptr: MutPtr<i32>) -> i32 {
    OSAtomicAdd32Barrier(env, amount, value_ptr)
}

fn OSAtomicAdd32Barrier(env: &mut Environment, amount: i32, value_ptr: MutPtr<i32>) -> i32 {
    let new = env.mem.read(value_ptr).wrapping_add(amount);
    env.mem.write(value_ptr, new);
    new
}

fn OSAtomicAdd64(env: &mut Environment, amount: i64, value_ptr: MutPtr<i64>) -> i64 {
    OSAtomicAdd64Barrier(env, amount, value_ptr)
}

fn OSAtomicAdd64Barrier(env: &mut Environment, amount: i64, value_ptr: MutPtr<i64>) -> i64 {
    let new = env.mem.read(value_ptr).wrapping_add(amount);
    env.mem.write(value_ptr, new);
    new
}

// =========================================================================
// MARK: - OSAtomicIncrement / OSAtomicDecrement
// =========================================================================

fn OSAtomicIncrement32(env: &mut Environment, value_ptr: MutPtr<i32>) -> i32 {
    OSAtomicAdd32Barrier(env, 1, value_ptr)
}

fn OSAtomicIncrement32Barrier(env: &mut Environment, value_ptr: MutPtr<i32>) -> i32 {
    OSAtomicAdd32Barrier(env, 1, value_ptr)
}

fn OSAtomicDecrement32(env: &mut Environment, value_ptr: MutPtr<i32>) -> i32 {
    OSAtomicAdd32Barrier(env, -1, value_ptr)
}

fn OSAtomicDecrement32Barrier(env: &mut Environment, value_ptr: MutPtr<i32>) -> i32 {
    OSAtomicAdd32Barrier(env, -1, value_ptr)
}

fn OSAtomicIncrement64(env: &mut Environment, value_ptr: MutPtr<i64>) -> i64 {
    OSAtomicAdd64Barrier(env, 1, value_ptr)
}

fn OSAtomicIncrement64Barrier(env: &mut Environment, value_ptr: MutPtr<i64>) -> i64 {
    OSAtomicAdd64Barrier(env, 1, value_ptr)
}

fn OSAtomicDecrement64(env: &mut Environment, value_ptr: MutPtr<i64>) -> i64 {
    OSAtomicAdd64Barrier(env, -1, value_ptr)
}

fn OSAtomicDecrement64Barrier(env: &mut Environment, value_ptr: MutPtr<i64>) -> i64 {
    OSAtomicAdd64Barrier(env, -1, value_ptr)
}

// =========================================================================
// MARK: - OSAtomicCompareAndSwap32
// =========================================================================

fn OSAtomicCompareAndSwap32(
    env: &mut Environment,
    old_value: i32,
    new_value: i32,
    value_ptr: MutPtr<i32>,
) -> bool {
    OSAtomicCompareAndSwap32Barrier(env, old_value, new_value, value_ptr)
}

fn OSAtomicCompareAndSwap32Barrier(
    env: &mut Environment,
    old_value: i32,
    new_value: i32,
    value_ptr: MutPtr<i32>,
) -> bool {
    if env.mem.read(value_ptr) == old_value {
        env.mem.write(value_ptr, new_value);
        true
    } else {
        false
    }
}

fn OSAtomicCompareAndSwapInt(
    env: &mut Environment,
    old_value: i32,
    new_value: i32,
    value_ptr: MutPtr<i32>,
) -> bool {
    OSAtomicCompareAndSwap32Barrier(env, old_value, new_value, value_ptr)
}

fn OSAtomicCompareAndSwapIntBarrier(
    env: &mut Environment,
    old_value: i32,
    new_value: i32,
    value_ptr: MutPtr<i32>,
) -> bool {
    OSAtomicCompareAndSwap32Barrier(env, old_value, new_value, value_ptr)
}

fn OSAtomicCompareAndSwapLong(
    env: &mut Environment,
    old_value: i32,
    new_value: i32,
    value_ptr: MutPtr<i32>,
) -> bool {
    // On 32-bit platforms `long` == `int`.
    OSAtomicCompareAndSwap32Barrier(env, old_value, new_value, value_ptr)
}

fn OSAtomicCompareAndSwapLongBarrier(
    env: &mut Environment,
    old_value: i32,
    new_value: i32,
    value_ptr: MutPtr<i32>,
) -> bool {
    OSAtomicCompareAndSwap32Barrier(env, old_value, new_value, value_ptr)
}

// =========================================================================
// MARK: - OSAtomicCompareAndSwap64
// =========================================================================

fn OSAtomicCompareAndSwap64(
    env: &mut Environment,
    old_value: i64,
    new_value: i64,
    value_ptr: MutPtr<i64>,
) -> bool {
    OSAtomicCompareAndSwap64Barrier(env, old_value, new_value, value_ptr)
}

fn OSAtomicCompareAndSwap64Barrier(
    env: &mut Environment,
    old_value: i64,
    new_value: i64,
    value_ptr: MutPtr<i64>,
) -> bool {
    if env.mem.read(value_ptr) == old_value {
        env.mem.write(value_ptr, new_value);
        true
    } else {
        false
    }
}

// =========================================================================
// MARK: - OSAtomicCompareAndSwapPtr
// =========================================================================

fn OSAtomicCompareAndSwapPtr(
    env: &mut Environment,
    old_value: MutVoidPtr,
    new_value: MutVoidPtr,
    value_ptr: MutPtr<MutVoidPtr>,
) -> bool {
    OSAtomicCompareAndSwapPtrBarrier(env, old_value, new_value, value_ptr)
}

fn OSAtomicCompareAndSwapPtrBarrier(
    env: &mut Environment,
    old_value: MutVoidPtr,
    new_value: MutVoidPtr,
    value_ptr: MutPtr<MutVoidPtr>,
) -> bool {
    if env.mem.read(value_ptr) == old_value {
        env.mem.write(value_ptr, new_value);
        true
    } else {
        false
    }
}

// =========================================================================
// MARK: - OSAtomicOr / OSAtomicAnd / OSAtomicXor
// =========================================================================

fn OSAtomicOr32(env: &mut Environment, mask: u32, value_ptr: MutPtr<u32>) -> u32 {
    OSAtomicOr32Barrier(env, mask, value_ptr)
}

fn OSAtomicOr32Barrier(env: &mut Environment, mask: u32, value_ptr: MutPtr<u32>) -> u32 {
    let new = env.mem.read(value_ptr) | mask;
    env.mem.write(value_ptr, new);
    new
}

fn OSAtomicOr32Orig(env: &mut Environment, mask: u32, value_ptr: MutPtr<u32>) -> u32 {
    let old = env.mem.read(value_ptr);
    env.mem.write(value_ptr, old | mask);
    old
}

fn OSAtomicOr32OrigBarrier(env: &mut Environment, mask: u32, value_ptr: MutPtr<u32>) -> u32 {
    OSAtomicOr32Orig(env, mask, value_ptr)
}

fn OSAtomicAnd32(env: &mut Environment, mask: u32, value_ptr: MutPtr<u32>) -> u32 {
    OSAtomicAnd32Barrier(env, mask, value_ptr)
}

fn OSAtomicAnd32Barrier(env: &mut Environment, mask: u32, value_ptr: MutPtr<u32>) -> u32 {
    let new = env.mem.read(value_ptr) & mask;
    env.mem.write(value_ptr, new);
    new
}

fn OSAtomicAnd32Orig(env: &mut Environment, mask: u32, value_ptr: MutPtr<u32>) -> u32 {
    let old = env.mem.read(value_ptr);
    env.mem.write(value_ptr, old & mask);
    old
}

fn OSAtomicAnd32OrigBarrier(env: &mut Environment, mask: u32, value_ptr: MutPtr<u32>) -> u32 {
    OSAtomicAnd32Orig(env, mask, value_ptr)
}

fn OSAtomicXor32(env: &mut Environment, mask: u32, value_ptr: MutPtr<u32>) -> u32 {
    OSAtomicXor32Barrier(env, mask, value_ptr)
}

fn OSAtomicXor32Barrier(env: &mut Environment, mask: u32, value_ptr: MutPtr<u32>) -> u32 {
    let new = env.mem.read(value_ptr) ^ mask;
    env.mem.write(value_ptr, new);
    new
}

fn OSAtomicXor32Orig(env: &mut Environment, mask: u32, value_ptr: MutPtr<u32>) -> u32 {
    let old = env.mem.read(value_ptr);
    env.mem.write(value_ptr, old ^ mask);
    old
}

fn OSAtomicXor32OrigBarrier(env: &mut Environment, mask: u32, value_ptr: MutPtr<u32>) -> u32 {
    OSAtomicXor32Orig(env, mask, value_ptr)
}

// =========================================================================
// MARK: - OSAtomicTestAndSet / OSAtomicTestAndClear
// =========================================================================

// n is a bit number; bit 0 of the *byte at* (addr + n/8), specifically the
// bit (7 - n%8) within that byte (big-endian bit numbering per Darwin docs).

fn OSAtomicTestAndSet(env: &mut Environment, n: u32, addr: MutPtr<u8>) -> bool {
    OSAtomicTestAndSetBarrier(env, n, addr)
}

fn OSAtomicTestAndSetBarrier(env: &mut Environment, n: u32, addr: MutPtr<u8>) -> bool {
    let byte_ptr = addr + (n / 8);
    let bit = 7 - (n % 8);
    let old: u8 = env.mem.read(byte_ptr);
    env.mem.write(byte_ptr, old | (1 << bit));
    (old >> bit) & 1 != 0
}

fn OSAtomicTestAndClear(env: &mut Environment, n: u32, addr: MutPtr<u8>) -> bool {
    OSAtomicTestAndClearBarrier(env, n, addr)
}

fn OSAtomicTestAndClearBarrier(env: &mut Environment, n: u32, addr: MutPtr<u8>) -> bool {
    let byte_ptr = addr + (n / 8);
    let bit = 7 - (n % 8);
    let old: u8 = env.mem.read(byte_ptr);
    env.mem.write(byte_ptr, old & !(1 << bit));
    (old >> bit) & 1 != 0
}

// =========================================================================
// MARK: - OSMemoryBarrier
// =========================================================================

fn OSMemoryBarrier(_env: &mut Environment) {
    // No-op — single host thread; memory ordering is trivially satisfied.
}

// =========================================================================
// MARK: - OSSpinLock
// =========================================================================

// OS_SPINLOCK_INIT = 0 (unlocked); any non-zero value = locked.

fn OSSpinLockLock(env: &mut Environment, lock: MutPtr<i32>) {
    // Single-threaded guest: just set to locked.
    // If already locked this would spin forever in a real implementation;
    // log a warning instead of hanging.
    if env.mem.read(lock) != 0 {
        log!("Warning: OSSpinLockLock called on already-locked spinlock {:?}", lock);
    }
    env.mem.write(lock, 1);
}

fn OSSpinLockUnlock(env: &mut Environment, lock: MutPtr<i32>) {
    env.mem.write(lock, 0);
}

fn OSSpinLockTry(env: &mut Environment, lock: MutPtr<i32>) -> bool {
    if env.mem.read(lock) == 0 {
        env.mem.write(lock, 1);
        true
    } else {
        false
    }
}

pub const FUNCTIONS: FunctionExports = &[
    // Add32
    export_c_func!(OSAtomicAdd32(_, _)),
    export_c_func!(OSAtomicAdd32Barrier(_, _)),
    // Add64
    export_c_func!(OSAtomicAdd64(_, _)),
    export_c_func!(OSAtomicAdd64Barrier(_, _)),
    // Increment32
    export_c_func!(OSAtomicIncrement32(_)),
    export_c_func!(OSAtomicIncrement32Barrier(_)),
    // Decrement32
    export_c_func!(OSAtomicDecrement32(_)),
    export_c_func!(OSAtomicDecrement32Barrier(_)),
    // Increment64
    export_c_func!(OSAtomicIncrement64(_)),
    export_c_func!(OSAtomicIncrement64Barrier(_)),
    // Decrement64
    export_c_func!(OSAtomicDecrement64(_)),
    export_c_func!(OSAtomicDecrement64Barrier(_)),
    // CompareAndSwap32
    export_c_func!(OSAtomicCompareAndSwap32(_, _, _)),
    export_c_func!(OSAtomicCompareAndSwap32Barrier(_, _, _)),
    export_c_func!(OSAtomicCompareAndSwapInt(_, _, _)),
    export_c_func!(OSAtomicCompareAndSwapIntBarrier(_, _, _)),
    export_c_func!(OSAtomicCompareAndSwapLong(_, _, _)),
    export_c_func!(OSAtomicCompareAndSwapLongBarrier(_, _, _)),
    // CompareAndSwap64
    export_c_func!(OSAtomicCompareAndSwap64(_, _, _)),
    export_c_func!(OSAtomicCompareAndSwap64Barrier(_, _, _)),
    // CompareAndSwapPtr
    export_c_func!(OSAtomicCompareAndSwapPtr(_, _, _)),
    export_c_func!(OSAtomicCompareAndSwapPtrBarrier(_, _, _)),
    // Or32
    export_c_func!(OSAtomicOr32(_, _)),
    export_c_func!(OSAtomicOr32Barrier(_, _)),
    export_c_func!(OSAtomicOr32Orig(_, _)),
    export_c_func!(OSAtomicOr32OrigBarrier(_, _)),
    // And32
    export_c_func!(OSAtomicAnd32(_, _)),
    export_c_func!(OSAtomicAnd32Barrier(_, _)),
    export_c_func!(OSAtomicAnd32Orig(_, _)),
    export_c_func!(OSAtomicAnd32OrigBarrier(_, _)),
    // Xor32
    export_c_func!(OSAtomicXor32(_, _)),
    export_c_func!(OSAtomicXor32Barrier(_, _)),
    export_c_func!(OSAtomicXor32Orig(_, _)),
    export_c_func!(OSAtomicXor32OrigBarrier(_, _)),
    // TestAndSet / TestAndClear
    export_c_func!(OSAtomicTestAndSet(_, _)),
    export_c_func!(OSAtomicTestAndSetBarrier(_, _)),
    export_c_func!(OSAtomicTestAndClear(_, _)),
    export_c_func!(OSAtomicTestAndClearBarrier(_, _)),
    // Memory barrier
    export_c_func!(OSMemoryBarrier()),
    // SpinLock
    export_c_func!(OSSpinLockLock(_)),
    export_c_func!(OSSpinLockUnlock(_)),
    export_c_func!(OSSpinLockTry(_)),
];
