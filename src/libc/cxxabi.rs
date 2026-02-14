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

#[macro_use]
extern crate lazy_static;

use std::sync::Mutex;

lazy_static::lazy_static! {
    static ref EXIT_FUNCS: Mutex<Vec<GuestFunction>> = Mutex::new(Vec::new());
}

fn __cxa_atexit(
    _env: &mut Environment,
    func: GuestFunction,
    _p: MutVoidPtr,
) -> i32 {
    EXIT_FUNCS.lock().unwrap().push(func);
    0 // success
}

fn __cxa_finalize(_f: MutVoidPtr) {
    let funcs = EXIT_FUNCS.lock().unwrap();
    for f in funcs.iter() {
        f.call(); // вызов GuestFunction в эмуляторе
    }
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(__cxa_atexit(_, _, _)),
    export_c_func!(__cxa_finalize(_)),
];
