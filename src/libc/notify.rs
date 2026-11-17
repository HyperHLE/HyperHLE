/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `libnotify` (`<notify.h>`).
//!
//! Apps use Darwin notifications to observe system/framework state changes.
//! touchHLE has no such notifications, but the registration functions must
//! return sane handles: callers (notably Google Analytics' reachability code)
//! pass the returned file descriptor to `CFFileDescriptorCreate` and add it to
//! a run loop. Returning a real, never-readable pipe end keeps that path valid
//! without ever firing a callback.
//!
//! Reference: `<notify.h>` from libnotify (Apple's `Libc` project).

use std::sync::atomic::{AtomicI32, Ordering};

use crate::dyld::{export_c_func, FunctionExports};
use crate::fs::PipeBuffer;
use crate::libc::posix_io::{find_or_create_pipe_read_fd, find_or_create_pipe_write_fd};
use crate::mem::{ConstPtr, MutPtr};
use crate::Environment;

/// `NOTIFY_STATUS_OK`.
const NOTIFY_STATUS_OK: u32 = 0;
/// `NOTIFY_STATUS_FAILED`.
const NOTIFY_STATUS_FAILED: u32 = 1000000;

static NEXT_TOKEN: AtomicI32 = AtomicI32::new(1);

fn allocate_token() -> i32 {
    NEXT_TOKEN.fetch_add(1, Ordering::Relaxed)
}

/// `uint32_t notify_register_file_descriptor(const char *name, int *out_fd,
///     uint32_t flags, int *out_token)`
///
/// The returned descriptor is the read end of a pipe whose write end stays
/// open in the guest descriptor table, so `poll()`/`read()` on it block
/// instead of reporting EOF forever.
#[allow(non_snake_case)]
pub fn notify_register_file_descriptor(
    env: &mut Environment,
    name: ConstPtr<u8>,
    out_fd: MutPtr<i32>,
    _flags: u32,
    out_token: MutPtr<i32>,
) -> u32 {
    let _ = env.mem.cstr_at_utf8(name);
    let buffer = std::rc::Rc::new(std::cell::RefCell::new(PipeBuffer::new()));
    let read_fd = find_or_create_pipe_read_fd(env, buffer.clone());
    // Keep the write end open for the lifetime of the process; closing it
    // would make the read end permanently readable (EOF).
    let _write_fd = find_or_create_pipe_write_fd(env, buffer);
    if !out_fd.is_null() {
        env.mem.write(out_fd, read_fd);
    }
    if !out_token.is_null() {
        env.mem.write(out_token, allocate_token());
    }
    NOTIFY_STATUS_OK
}

/// `uint32_t notify_register_check(const char *name, int *out_token)`
#[allow(non_snake_case)]
pub fn notify_register_check(
    env: &mut Environment,
    name: ConstPtr<u8>,
    out_token: MutPtr<i32>,
) -> u32 {
    let _ = env.mem.cstr_at_utf8(name);
    if !out_token.is_null() {
        env.mem.write(out_token, allocate_token());
    }
    NOTIFY_STATUS_OK
}

/// `uint32_t notify_cancel(int token)`
#[allow(non_snake_case)]
pub fn notify_cancel(_env: &mut Environment, _token: i32) -> u32 {
    NOTIFY_STATUS_OK
}

/// `uint32_t notify_post(const char *name)`
#[allow(non_snake_case)]
pub fn notify_post(_env: &mut Environment, _name: ConstPtr<u8>) -> u32 {
    NOTIFY_STATUS_OK
}

/// `uint32_t notify_get_state(int token, uint64_t *state64)`
#[allow(non_snake_case)]
pub fn notify_get_state(env: &mut Environment, _token: i32, state: MutPtr<u64>) -> u32 {
    if state.is_null() {
        return NOTIFY_STATUS_FAILED;
    }
    env.mem.write(state, 0);
    NOTIFY_STATUS_OK
}

/// `uint32_t notify_set_state(int token, uint64_t state64)`
#[allow(non_snake_case)]
pub fn notify_set_state(_env: &mut Environment, _token: i32, _state: u64) -> u32 {
    NOTIFY_STATUS_OK
}

/// `uint32_t notify_suspend(int token)` / `uint32_t notify_resume(int token)`
#[allow(non_snake_case)]
pub fn notify_suspend(_env: &mut Environment, _token: i32) -> u32 {
    NOTIFY_STATUS_OK
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(notify_register_file_descriptor(_, _, _, _)),
    export_c_func!(notify_register_check(_, _)),
    export_c_func!(notify_cancel(_)),
    export_c_func!(notify_post(_)),
    export_c_func!(notify_get_state(_, _)),
    export_c_func!(notify_set_state(_, _)),
    export_c_func!(notify_suspend(_)),
];
