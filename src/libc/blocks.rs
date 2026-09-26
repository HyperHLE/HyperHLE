/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `libBlocksRuntime` — Apple Blocks ABI runtime.
//!
//! `_Block_copy` / `_Block_release` and the `_Block_object_assign` /
//! `_Block_object_dispose` helpers called by compiler-generated copy/dispose
//! helpers, as described in the
//! [Blocks ABI](https://clang.llvm.org/docs/Block-ABI-Apple.html): copying a
//! stack block moves it and the `__block` variables it captures to the heap,
//! where they are reference-counted. Global blocks are never copied.
//!
//! Blocks are not Objective-C objects here: their `isa` is a placeholder (see
//! `_NSConcreteStackBlock` in `dyld.rs`) that a heap copy keeps, and messages
//! to them go nowhere. So `[block copy]`, and `objc_retainBlock` under ARC,
//! don't reach this runtime.

use crate::abi::{CallFromHost, GuestFunction};
use crate::dyld::{export_c_func, FunctionExports};
use crate::mem::{ConstVoidPtr, MutPtr, Ptr};
use crate::objc::{id, release, retain};
use crate::Environment;

// `flags` of `Block_layout` and `Block_byref`. The compiler sets the bits
// from the Blocks ABI; the rest is private to this runtime: bit 24 marks a
// heap copy, and the low 16 bits count its references.
const BLOCK_REFCOUNT_MASK: u32 = 0xffff;
const BLOCK_REFCOUNT_ONE: u32 = 1;
const BLOCK_NEEDS_FREE: u32 = 1 << 24;
const BLOCK_HAS_COPY_DISPOSE: u32 = 1 << 25;
const BLOCK_IS_GLOBAL: u32 = 1 << 28;

// `flags` argument of `_Block_object_assign` / `_Block_object_dispose`.
const BLOCK_FIELD_IS_OBJECT: i32 = 3;
const BLOCK_FIELD_IS_BLOCK: i32 = 7;
const BLOCK_FIELD_IS_BYREF: i32 = 8;
const BLOCK_FIELD_IS_WEAK: i32 = 16;
const BLOCK_BYREF_CALLER: i32 = 128;

// struct Block_layout { isa, flags, reserved, invoke, descriptor, ... }
const BLOCK_FLAGS: u32 = 1;
const BLOCK_DESCRIPTOR: u32 = 4;
// struct Block_descriptor { reserved, size, copy_helper?, dispose_helper? }
const DESCRIPTOR_SIZE: u32 = 1;
const DESCRIPTOR_COPY: u32 = 2;
const DESCRIPTOR_DISPOSE: u32 = 3;
// struct Block_byref { isa, forwarding, flags, size, keep?, destroy?, ... }
const BYREF_FORWARDING: u32 = 1;
const BYREF_FLAGS: u32 = 2;
const BYREF_SIZE: u32 = 3;
const BYREF_KEEP: u32 = 4;
const BYREF_DESTROY: u32 = 5;

/// Increment the reference count in a `flags` word, unless it has saturated.
fn retain_flags(env: &mut Environment, flags: MutPtr<u32>) {
    let value = env.mem.read(flags);
    if value & BLOCK_REFCOUNT_MASK != BLOCK_REFCOUNT_MASK {
        env.mem.write(flags, value + BLOCK_REFCOUNT_ONE);
    }
}

/// Decrement the reference count in a `flags` word. Returns `true` when the
/// last reference is gone. A saturated count is never decremented.
fn release_flags(env: &mut Environment, flags: MutPtr<u32>) -> bool {
    let value = env.mem.read(flags);
    let count = value & BLOCK_REFCOUNT_MASK;
    if count == 0 || count == BLOCK_REFCOUNT_MASK {
        return false;
    }
    env.mem.write(flags, value - BLOCK_REFCOUNT_ONE);
    count == BLOCK_REFCOUNT_ONE
}

fn call_helper<A>(env: &mut Environment, helper: u32, args: A)
where
    GuestFunction: CallFromHost<(), A>,
{
    let helper = GuestFunction::from_addr_with_thumb_bit(helper);
    helper.call_from_host(env, args)
}

/// `Block_copy()`: moves a stack block to the heap (running its copy helper),
/// or adds a reference to a heap block. Global blocks are returned as-is.
pub fn copy_block(env: &mut Environment, block: ConstVoidPtr) -> ConstVoidPtr {
    if block.is_null() {
        return block;
    }
    let layout: MutPtr<u32> = block.cast_mut().cast();
    let flags = env.mem.read(layout + BLOCK_FLAGS);
    if flags & BLOCK_NEEDS_FREE != 0 {
        retain_flags(env, layout + BLOCK_FLAGS);
        return block;
    }
    if flags & BLOCK_IS_GLOBAL != 0 {
        return block;
    }

    let descriptor: MutPtr<u32> = Ptr::from_bits(env.mem.read(layout + BLOCK_DESCRIPTOR));
    let size = env.mem.read(descriptor + DESCRIPTOR_SIZE);
    let copy: MutPtr<u32> = env.mem.alloc(size).cast();
    env.mem.memmove(copy.cast(), block, size);
    let copy_flags = (flags & !BLOCK_REFCOUNT_MASK) | BLOCK_NEEDS_FREE | BLOCK_REFCOUNT_ONE;
    env.mem.write(copy + BLOCK_FLAGS, copy_flags);
    if flags & BLOCK_HAS_COPY_DISPOSE != 0 {
        let helper = env.mem.read(descriptor + DESCRIPTOR_COPY);
        call_helper(env, helper, (copy.cast_void(), block));
    }
    copy.cast_void().cast_const()
}

/// `Block_release()`: drops a reference to a heap block, running its dispose
/// helper and freeing it when it was the last one. No-op for other blocks.
pub fn release_block(env: &mut Environment, block: ConstVoidPtr) {
    if block.is_null() {
        return;
    }
    let layout: MutPtr<u32> = block.cast_mut().cast();
    let flags = env.mem.read(layout + BLOCK_FLAGS);
    if flags & BLOCK_NEEDS_FREE == 0 || !release_flags(env, layout + BLOCK_FLAGS) {
        return;
    }
    if flags & BLOCK_HAS_COPY_DISPOSE != 0 {
        let descriptor: MutPtr<u32> = Ptr::from_bits(env.mem.read(layout + BLOCK_DESCRIPTOR));
        let helper = env.mem.read(descriptor + DESCRIPTOR_DISPOSE);
        call_helper(env, helper, (block,));
    }
    env.mem.free(block.cast_mut());
}

/// Moves a `__block` variable to the heap the first time a block capturing
/// it is copied, and points the stack copy's `forwarding` at it. Later
/// copies add a reference to the heap copy.
fn copy_byref(env: &mut Environment, byref: ConstVoidPtr) -> ConstVoidPtr {
    let src: MutPtr<u32> = byref.cast_mut().cast();
    let forwarding: MutPtr<u32> = Ptr::from_bits(env.mem.read(src + BYREF_FORWARDING));
    if env.mem.read(forwarding + BYREF_FLAGS) & BLOCK_NEEDS_FREE != 0 {
        retain_flags(env, forwarding + BYREF_FLAGS);
        return forwarding.cast_void().cast_const();
    }

    let flags = env.mem.read(src + BYREF_FLAGS);
    let size = env.mem.read(src + BYREF_SIZE);
    let copy: MutPtr<u32> = env.mem.alloc(size).cast();
    env.mem.memmove(copy.cast(), byref, size);
    env.mem.write(copy, 0); // isa
    env.mem.write(copy + BYREF_FORWARDING, copy.to_bits());
    // One reference for the copied block, one for the stack frame: the
    // compiler releases the variable when it goes out of scope.
    let copy_flags = (flags & !BLOCK_REFCOUNT_MASK) | BLOCK_NEEDS_FREE | (2 * BLOCK_REFCOUNT_ONE);
    env.mem.write(copy + BYREF_FLAGS, copy_flags);
    env.mem.write(src + BYREF_FORWARDING, copy.to_bits());
    if flags & BLOCK_HAS_COPY_DISPOSE != 0 {
        let keep = env.mem.read(src + BYREF_KEEP);
        call_helper(env, keep, (copy.cast_void(), byref));
    }
    copy.cast_void().cast_const()
}

/// Drops a reference to a `__block` variable (through its `forwarding`),
/// running its destroy helper and freeing it when it was the last one.
fn release_byref(env: &mut Environment, byref: ConstVoidPtr) {
    let src: MutPtr<u32> = byref.cast_mut().cast();
    let byref: MutPtr<u32> = Ptr::from_bits(env.mem.read(src + BYREF_FORWARDING));
    let flags = env.mem.read(byref + BYREF_FLAGS);
    if flags & BLOCK_NEEDS_FREE == 0 || !release_flags(env, byref + BYREF_FLAGS) {
        return;
    }
    if flags & BLOCK_HAS_COPY_DISPOSE != 0 {
        let destroy = env.mem.read(byref + BYREF_DESTROY);
        call_helper(env, destroy, (byref.cast_void(),));
    }
    env.mem.free(byref.cast_void());
}

fn _Block_copy(env: &mut Environment, block: ConstVoidPtr) -> ConstVoidPtr {
    copy_block(env, block)
}

fn _Block_release(env: &mut Environment, block: ConstVoidPtr) {
    release_block(env, block)
}

/// `_Block_object_assign(destAddr, object, flags)`. Called by the copy helper
/// of a block for each captured object, block or `__block` variable, and by
/// the keep helper of a `__block` variable (`BLOCK_BYREF_CALLER`) for its
/// contents, which aren't retained.
fn _Block_object_assign(
    env: &mut Environment,
    dest_addr: MutPtr<ConstVoidPtr>,
    object: ConstVoidPtr,
    flags: i32,
) {
    // Compilers set this flag only in the helpers of a `__block` variable,
    // never together with `BLOCK_FIELD_IS_BYREF` (one example in the Blocks
    // ABI does that, but it doesn't match any compiler).
    if flags & BLOCK_BYREF_CALLER != 0 {
        env.mem.write(dest_addr, object);
        return;
    }
    let value = match flags {
        BLOCK_FIELD_IS_OBJECT => {
            let obj: id = Ptr::from_bits(object.to_bits());
            retain(env, obj);
            object
        }
        BLOCK_FIELD_IS_BLOCK => copy_block(env, object),
        // A `__weak __block` variable is a byref too.
        _ if flags & !BLOCK_FIELD_IS_WEAK == BLOCK_FIELD_IS_BYREF => copy_byref(env, object),
        _ => object,
    };
    env.mem.write(dest_addr, value);
}

/// `_Block_object_dispose(object, flags)`. Called by the dispose helper to
/// release what `_Block_object_assign` retained or copied, and at the end of
/// a `__block` variable's scope.
fn _Block_object_dispose(env: &mut Environment, object: ConstVoidPtr, flags: i32) {
    if flags & BLOCK_BYREF_CALLER != 0 {
        return;
    }
    match flags {
        BLOCK_FIELD_IS_OBJECT => {
            let obj: id = Ptr::from_bits(object.to_bits());
            release(env, obj);
        }
        BLOCK_FIELD_IS_BLOCK => release_block(env, object),
        _ if flags & !BLOCK_FIELD_IS_WEAK == BLOCK_FIELD_IS_BYREF => release_byref(env, object),
        _ => (),
    }
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(_Block_copy(_)),
    export_c_func!(_Block_release(_)),
    export_c_func!(_Block_object_assign(_, _, _)),
    export_c_func!(_Block_object_dispose(_, _)),
];
