/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `cxxabi.h`
//!
//! Resources:
//! - [Itanium C++ ABI specification](https://itanium-cxx-abi.github.io/cxx-abi/abi.html#dso-dtor-runtime-api)

use crate::abi::GuestFunction;
use crate::dyld::{export_c_func, FunctionExports};
use crate::mem::MutVoidPtr;
use crate::Environment;

use lazy_static::lazy_static;
use std::sync::Mutex;

lazy_static::lazy_static! {
    static ref EXIT_FUNCS: Mutex<Vec<GuestFunction>> = Mutex::new(Vec::new());
}

pub fn __cxa_atexit(
    _env: &mut Environment,
    func: GuestFunction,
    _p: MutVoidPtr,
) -> i32 {
    EXIT_FUNCS.lock().unwrap().push(func);
    0 // success
}

pub fn __cxa_finalize(
    _env: &mut Environment,
    _p: MutVoidPtr,
) -> i32 {
    // по стандарту допускается no-op
    0
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(__cxa_atexit(GuestFunction, MutVoidPtr)),
    export_c_func!(__cxa_finalize(MutVoidPtr)),
];
