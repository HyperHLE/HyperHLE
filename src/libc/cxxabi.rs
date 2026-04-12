/*
* `cxxabi.h`
*/
use crate::abi::GuestFunction;
use crate::dyld::{export_c_func, FunctionExports};
use crate::mem::MutVoidPtr;
use crate::Environment;

fn __cxa_atexit(
    _env: &mut Environment,
    _func: GuestFunction,
    _p: MutVoidPtr,
    _d: MutVoidPtr,
) -> i32 {
    // Return 0 to indicate success. 
    // Logging removed to prevent console spam.
    0 
}

fn __cxa_finalize(_env: &mut Environment, _d: MutVoidPtr) {
    // Empty
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(__cxa_atexit(_, _, _)),
    export_c_func!(__cxa_finalize(_)),
];
