/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Stub for `CoreAudio.framework/CoreAudio`.
//!
//! On iOS, CoreAudio exposes no public C API that games call directly.
//! The audio session and audio queue APIs live in AudioToolbox, not here.
//! This stub exists solely so that apps whose Mach-O dependency list
//! includes CoreAudio (e.g. those built with older SDKs or certain
//! third-party libraries) do not trigger "unimplemented dylib" warnings.

use crate::dyld::{export_c_func, FunctionExports};
use crate::mem::MutVoidPtr;
use crate::Environment;

/// `void CAShow(void *inObject)`
///
/// CoreAudio's debugging dump helper. It only prints a human-readable
/// description of an audio object to stderr, so logging the pointer is a
/// faithful no-op for the guest.
/// <https://developer.apple.com/documentation/coreaudio/1422621-cashow>
fn CAShow(_env: &mut Environment, object: MutVoidPtr) {
    log_dbg!("CAShow({:?})", object);
}

/// `void CAShowFile(void *inObject, FILE *inFile)`
fn CAShowFile(_env: &mut Environment, object: MutVoidPtr, file: MutVoidPtr) {
    log_dbg!("CAShowFile({:?}, {:?})", object, file);
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(CAShow(_)),
    export_c_func!(CAShowFile(_, _)),
];
