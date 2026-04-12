/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `AudioConverter.h` — Audio format conversion.
//!
//! PvZ 1.1 calls AudioConverterNew() when setting up its audio pipeline.
//! We provide a stub implementation that returns a dummy converter handle
//! and no-ops most operations, since touchHLE handles audio via OpenAL/
//! AudioQueue and doesn't need real PCM format conversion.

use crate::dyld::FunctionExports;
use crate::export_c_func;
use crate::frameworks::carbon_core::OSStatus;
use crate::frameworks::core_audio_types::AudioStreamBasicDescription;
use crate::mem::{ConstPtr, MutPtr, SafeRead};
use crate::Environment;

// OSStatus error codes
const kAudioConverterErr_InvalidInputSize: OSStatus = -50;

#[repr(C, packed)]
struct OpaqueAudioConverter {
    _pad: u8,
}
unsafe impl SafeRead for OpaqueAudioConverter {}

type AudioConverterRef = MutPtr<OpaqueAudioConverter>;

fn AudioConverterNew(
    env: &mut Environment,
    in_source_format: ConstPtr<AudioStreamBasicDescription>,
    in_destination_format: ConstPtr<AudioStreamBasicDescription>,
    out_audio_converter: MutPtr<AudioConverterRef>,
) -> OSStatus {
    // Allocate a dummy converter object so the caller gets a non-null handle.
    let converter: AudioConverterRef = env
        .mem
        .alloc_and_write(OpaqueAudioConverter { _pad: 0 });
    env.mem.write(out_audio_converter, converter);

    log!(
        "TODO: AudioConverterNew({:?}, {:?}) -> 0 (stubbed)",
        in_source_format,
        in_destination_format,
    );
    0 // noErr
}

fn AudioConverterDispose(env: &mut Environment, in_audio_converter: AudioConverterRef) -> OSStatus {
    if in_audio_converter.is_null() {
        log!("AudioConverterDispose: null converter, returning error");
        return kAudioConverterErr_InvalidInputSize;
    }
    env.mem.free(in_audio_converter.cast());
    log_dbg!("AudioConverterDispose({:?}) -> 0", in_audio_converter);
    0
}

fn AudioConverterReset(env: &mut Environment, in_audio_converter: AudioConverterRef) -> OSStatus {
    log_dbg!("AudioConverterReset({:?}) -> 0 (stubbed)", in_audio_converter);
    0
}

fn AudioConverterGetProperty(
    _env: &mut Environment,
    in_audio_converter: AudioConverterRef,
    in_property_id: u32,
    _io_data_size: MutPtr<u32>,
    _out_property_data: MutPtr<u8>,
) -> OSStatus {
    log!(
        "TODO: AudioConverterGetProperty({:?}, {:#010x}) -> unimplemented (-1)",
        in_audio_converter,
        in_property_id,
    );
    -1
}

fn AudioConverterSetProperty(
    _env: &mut Environment,
    in_audio_converter: AudioConverterRef,
    in_property_id: u32,
    in_data_size: u32,
    _in_property_data: ConstPtr<u8>,
) -> OSStatus {
    log!(
        "TODO: AudioConverterSetProperty({:?}, {:#010x}, size={}) -> 0 (stubbed)",
        in_audio_converter,
        in_property_id,
        in_data_size,
    );
    0
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(AudioConverterNew(_, _, _)),
    export_c_func!(AudioConverterDispose(_)),
    export_c_func!(AudioConverterReset(_)),
    export_c_func!(AudioConverterGetProperty(_, _, _, _)),
    export_c_func!(AudioConverterSetProperty(_, _, _, _)),
];