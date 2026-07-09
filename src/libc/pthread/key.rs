/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Thread-specific data keys.

use crate::abi::GuestFunction;
use crate::dyld::{export_c_func, FunctionExports};
use crate::mem::{ConstVoidPtr, MutPtr, MutVoidPtr, Ptr};
use crate::{Environment, ThreadId};
use std::collections::HashMap;

#[derive(Default)]
pub struct State {
    /// The `pthread_key_t` value, with 1 subtracted, is the index into this
    /// vector. The tuple contains the map of thread-specific data pointers plus
    /// the destructor pointer.
    keys: Vec<(HashMap<ThreadId, MutVoidPtr>, GuestFunction)>,
}

fn get_state(env: &mut Environment) -> &mut State {
    &mut env.libc_state.pthread.key
}

type pthread_key_t = u32;

fn pthread_key_create(
    env: &mut Environment,
    key_ptr: MutPtr<pthread_key_t>,
    destructor: GuestFunction, // void (*destructor)(void *), may be NULL
) -> i32 {
    let idx = get_state(env).keys.len();
    let key: pthread_key_t = (idx + 1).try_into().unwrap();
    get_state(env).keys.push((HashMap::new(), destructor));
    env.mem.write(key_ptr, key);
    0 // success
}

fn pthread_getspecific(env: &mut Environment, key: pthread_key_t) -> MutVoidPtr {
    // Use of invalid key is undefined, panicking is fine.
    let idx: usize = key.checked_sub(1).unwrap().try_into().unwrap();
    let current_thread = env.current_thread;
    get_state(env).keys[idx]
        .0
        .get(&current_thread)
        .copied()
        .unwrap_or(Ptr::null())
}

fn pthread_setspecific(env: &mut Environment, key: pthread_key_t, value: ConstVoidPtr) -> i32 {
    // TODO: return error instead of panicking if key is invalid?
    let idx: usize = key.checked_sub(1).unwrap().try_into().unwrap();
    let current_thread = env.current_thread;
    get_state(env).keys[idx]
        .0
        .insert(current_thread, value.cast_mut());
    0 // success
}

fn pthread_key_delete(env: &mut Environment, key: pthread_key_t) -> i32 {
    // POSIX `int pthread_key_delete(pthread_key_t key)`: returns 0 on
    // success, EINVAL if `key` is not a valid key. We don't currently
    // free the slot in the keys table — Mono only deletes keys at
    // teardown so the leaked slot is bounded — but we do clear out
    // any per-thread values stored under it so the next
    // `pthread_getspecific` after a fresh `pthread_key_create` reuse
    // doesn't see a stale pointer.
    let Some(idx) = key.checked_sub(1).and_then(|i| usize::try_from(i).ok()) else {
        return 22; // EINVAL
    };
    let state = get_state(env);
    if let Some((data, _)) = state.keys.get_mut(idx) {
        data.clear();
        log_dbg!("pthread_key_delete({}) cleared per-thread values", key);
        0
    } else {
        log_dbg!("pthread_key_delete({}) on unknown key, returning EINVAL", key);
        22
    }
}

// --- ОБНОВЛЕННЫЙ СПИСОК ЭКСПОРТА ---
pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(pthread_key_create(_, _)),
    export_c_func!(pthread_getspecific(_)),
    export_c_func!(pthread_setspecific(_, _)),
    export_c_func!(pthread_key_delete(_)), // Регистрируем новую функцию
];
