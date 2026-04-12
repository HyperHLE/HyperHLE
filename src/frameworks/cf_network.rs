/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Stub for `CFNetwork.framework/CFNetwork`.
//!
//! Purpose:
//! - satisfy symbol lookup so apps that depend on CFNetwork don't crash
//! - avoid crashes on simple create/open checks
//! - behave safely when the game polls streams or hosts

use crate::dyld::{export_c_func, FunctionExports};
use crate::mem::MutPtr;
use crate::Environment;

const DUMMY_STREAM: u32 = 0xC0F0_0001;
const DUMMY_HOST: u32 = 0xC0F0_0002;

fn CFReadStreamCreateForHTTPRequest(
    _env: &mut Environment,
    _alloc: u32,
    _request: u32,
) -> u32 {
    // Return a non-null dummy handle so callers that only check for null
    // continue past the nil check.
    DUMMY_STREAM
}

fn CFReadStreamOpen(_env: &mut Environment, _stream: u32) -> bool {
    true
}

fn CFReadStreamHasBytesAvailable(_env: &mut Environment, _stream: u32) -> bool {
    false
}

fn CFReadStreamRead(
    _env: &mut Environment,
    _stream: u32,
    _buffer: MutPtr<u8>,
    _buffer_length: i32,
) -> i32 {
    0
}

fn CFReadStreamClose(_env: &mut Environment, _stream: u32) {}

fn CFReadStreamSetProperty(
    _env: &mut Environment,
    _stream: u32,
    _property: u32,
    _value: u32,
) -> bool {
    true
}

fn CFReadStreamCopyProperty(
    _env: &mut Environment,
    _stream: u32,
    _property: u32,
) -> u32 {
    0
}

fn CFReadStreamScheduleWithRunLoop(
    _env: &mut Environment,
    _stream: u32,
    _run_loop: u32,
    _run_loop_mode: u32,
) {
}

fn CFReadStreamUnscheduleFromRunLoop(
    _env: &mut Environment,
    _stream: u32,
    _run_loop: u32,
    _run_loop_mode: u32,
) {
}

fn CFReadStreamSetClient(
    _env: &mut Environment,
    _stream: u32,
    _callback_types: u32,
    _client_cb: u32,
    _client_context: u32,
) -> bool {
    true
}

fn CFReadStreamGetStatus(_env: &mut Environment, _stream: u32) -> u32 {
    // kCFStreamStatusOpen
    2
}

fn CFReadStreamCopyError(_env: &mut Environment, _stream: u32) -> u32 {
    0
}

fn CFHostCreateWithAddress(_env: &mut Environment, _allocator: u32, _address: u32) -> u32 {
    DUMMY_HOST
}

fn CFHostStartInfoResolution(
    _env: &mut Environment,
    _the_host: u32,
    _info: u32,
    _error: u32,
) -> bool {
    true
}

fn CFHostCancelInfoResolution(_env: &mut Environment, _the_host: u32) {}

fn CFHostGetAddresses(_env: &mut Environment, _the_host: u32, _resolved: u32) -> u32 {
    0
}

fn CFHostSetClient(
    _env: &mut Environment,
    _the_host: u32,
    _client_cb: u32,
    _client_context: u32,
) -> bool {
    true
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(CFReadStreamCreateForHTTPRequest(_, _)),
    export_c_func!(CFReadStreamOpen(_)),
    export_c_func!(CFReadStreamHasBytesAvailable(_)),
    export_c_func!(CFReadStreamRead(_, _, _)),
    export_c_func!(CFReadStreamClose(_)),
    export_c_func!(CFReadStreamSetProperty(_, _, _)),
    export_c_func!(CFReadStreamCopyProperty(_, _)),
    export_c_func!(CFReadStreamScheduleWithRunLoop(_, _, _)),
    export_c_func!(CFReadStreamUnscheduleFromRunLoop(_, _, _)),
    export_c_func!(CFReadStreamSetClient(_, _, _, _)),
    export_c_func!(CFReadStreamGetStatus(_)),
    export_c_func!(CFReadStreamCopyError(_)),
    export_c_func!(CFHostCreateWithAddress(_, _)),
    export_c_func!(CFHostStartInfoResolution(_, _, _)),
    export_c_func!(CFHostCancelInfoResolution(_)),
    export_c_func!(CFHostGetAddresses(_, _)),
    export_c_func!(CFHostSetClient(_, _, _)),
];

