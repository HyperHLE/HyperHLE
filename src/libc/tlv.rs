/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Thread-local variables (`__tlv_bootstrap`).
//!
//! A dylib built with `-fthread-local-variables` (or any code using `__thread`
//! / C++ `thread_local` in a library rather than the main executable) reaches
//! its variables through `__tlv_bootstrap`, which Darwin's dyld supplies.
//! Without it the loader cannot bind the symbol, so every access goes through
//! the generic "unimplemented function" return-0 stub and the guest
//! dereferences NULL.
//!
//! The contract, from `<mach-o/thread_local_variables.h>` and dyld's
//! `TLVGetAddressOfVariable`:
//!
//! ```c
//! struct tlv_descriptor {
//!     void *(*thunk)(struct tlv_descriptor *, size_t); // initializer image
//!     unsigned long key;    // index in this thread's TLV array
//!     unsigned long offset; // size of one instance, in bytes
//! };
//! void *__tlv_bootstrap(struct tlv_descriptor *descriptor);
//! ```
//!
//! The thunk returns the address of a template image for the variable; the
//! runtime copies `offset` bytes of it into per-thread storage the first time
//! the thread touches the variable. That is exactly what is done here: one
//! zeroed guest buffer per (thread, key), filled from the template image.

use crate::abi::{CallFromHost, GuestFunction};
use crate::dyld::{export_c_func, FunctionExports};
use crate::mem::{ConstVoidPtr, MutVoidPtr, SafeRead};
use crate::Environment;
use std::collections::HashMap;
use std::sync::Mutex;

/// `struct tlv_descriptor`, as laid out by the toolchain.
#[repr(C)]
struct TlvDescriptor {
    /// Returns the address of the variable's initializer image, or NULL when
    /// the variable has no dynamic initializer.
    thunk: ConstVoidPtr,
    /// Index of the variable in the per-thread TLV array.
    key: u32,
    /// Size in bytes of one instance of the variable.
    size: u32,
}
unsafe impl SafeRead for TlvDescriptor {}

/// Per-thread copies, keyed by `(thread index, descriptor key)`.
///
/// Guest addresses are process-global, so a static table is enough; the key
/// includes the thread so each thread gets its own instance, as required.
static TLV_STORAGE: Mutex<Option<HashMap<(usize, u32), u32>>> = Mutex::new(None);

/// `void *__tlv_bootstrap(struct tlv_descriptor *descriptor)`
fn _tlv_bootstrap(env: &mut Environment, descriptor: ConstVoidPtr) -> MutVoidPtr {
    if descriptor.is_null() {
        return MutVoidPtr::null();
    }
    let descriptor_value: TlvDescriptor = env.mem.read(descriptor.cast());
    if descriptor_value.size == 0 {
        return MutVoidPtr::null();
    }

    let thread = env.current_thread;
    if let Ok(mut guard) = TLV_STORAGE.lock() {
        let storage = guard.get_or_insert_with(HashMap::new);
        if let Some(&address) = storage.get(&(thread, descriptor_value.key)) {
            return MutVoidPtr::from_bits(address);
        }
    }

    // A NULL thunk means the variable has no initializer image; there is
    // nothing to copy, so report failure rather than handing out a block with
    // uninitialised (guest-visible) contents.
    if descriptor_value.thunk.is_null() {
        return MutVoidPtr::null();
    }
    let thunk = GuestFunction::from_addr_with_thumb_bit(descriptor_value.thunk.to_bits());
    let image: ConstVoidPtr = thunk.call_from_host(env, (descriptor, descriptor_value.size));
    if image.is_null() {
        return MutVoidPtr::null();
    }

    let template = env
        .mem
        .bytes_at(image.cast(), descriptor_value.size)
        .to_vec();
    let block = env.mem.alloc(descriptor_value.size);
    env.mem
        .bytes_at_mut(block.cast(), descriptor_value.size)
        .copy_from_slice(&template);

    if let Ok(mut guard) = TLV_STORAGE.lock() {
        guard
            .get_or_insert_with(HashMap::new)
            .insert((thread, descriptor_value.key), block.to_bits());
    }
    block
}

pub const FUNCTIONS: FunctionExports = &[export_c_func!(_tlv_bootstrap(_))];
