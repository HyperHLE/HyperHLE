/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 * If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `ExtAudioFile.h` (Extended Audio File Services)

use crate::audio;
use crate::dyld::{export_c_func, FunctionExports};
use crate::frameworks::audio_toolbox::audio_file::{
    AudioFileHostObject, AudioFileID, State as AudioFileState,
    kAudioFilePropertyDataFormat, kAudioFilePropertyPacketSizeUpperBound,
    AudioFileReadPackets,
};
use crate::frameworks::carbon_core::OSStatus;
use crate::frameworks::core_audio_types::{
    debug_fourcc, fourcc, AudioStreamBasicDescription,
};
use crate::frameworks::core_foundation::cf_url::CFURLRef;
use crate::frameworks::foundation::ns_url::to_rust_path;
use crate::mem::{guest_size_of, GuestUSize, MutPtr, MutVoidPtr, SafeRead};
use crate::Environment;
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct State {
    pub ext_audio_files: HashMap<ExtAudioFileRef, ExtAudioFileHostObject>,
}
impl State {
    pub fn get(framework_state: &mut crate::frameworks::State) -> &mut Self {
        &mut framework_state.audio_toolbox.ext_audio_file
    }
}

pub struct ExtAudioFileHostObject {
    /// The underlying audio file. We always own it (even when created via
    /// `ExtAudioFileWrapAudioFileID`, we keep a reference to the same data).
    pub audio_file: audio::AudioFile,
    /// Client format requested via `kExtAudioFileProperty_ClientDataFormat`.
    /// `None` means "use the file's native format" (no conversion).
    pub client_format: Option<AudioStreamBasicDescription>,
    /// Current read position in *frames* (used for `ExtAudioFileRead`).
    pub frame_position: u64,
    /// When this ExtAudioFile was created by wrapping an existing AudioFileID
    /// we remember that ID so we don't double-free the underlying guest memory.
    pub wrapped_audio_file_id: Option<AudioFileID>,
}

// ---------------------------------------------------------------------------
// Opaque handle type
// ---------------------------------------------------------------------------

#[repr(C, packed)]
pub struct OpaqueExtAudioFileID {
    _filler: u8,
}
unsafe impl SafeRead for OpaqueExtAudioFileID {}

pub type ExtAudioFileRef = MutPtr<OpaqueExtAudioFileID>;

// ---------------------------------------------------------------------------
// Error codes
// ---------------------------------------------------------------------------

const kExtAudioFileError_InvalidProperty: OSStatus = fourcc(b"pty?") as _;
const kExtAudioFileError_InvalidPropertySize: OSStatus = fourcc(b"!siz") as _;
const kExtAudioFileError_NonPCMClientFormat: OSStatus = fourcc(b"!pcm") as _;
const kExtAudioFileError_InvalidOperationOrder: OSStatus = fourcc(b"ord?") as _;
const kExtAudioFileError_InvalidDataFormat: OSStatus = fourcc(b"fmt?") as _;

// ---------------------------------------------------------------------------
// Property IDs
// ---------------------------------------------------------------------------

/// Usually a FourCC.
type ExtAudioFilePropertyID = u32;
const kExtAudioFileProperty_FileDataFormat: ExtAudioFilePropertyID = fourcc(b"ffmt");
const kExtAudioFileProperty_ClientDataFormat: ExtAudioFilePropertyID = fourcc(b"cfmt");
const kExtAudioFileProperty_FileLengthFrames: ExtAudioFilePropertyID = fourcc(b"#frm");
const kExtAudioFileProperty_AudioFile: ExtAudioFilePropertyID = fourcc(b"afil");
// ИСПРАВЛЕНИЕ: Заменено b"aconv" на b"acnv" (ровно 4 байта)
const kExtAudioFileProperty_AudioConverter: ExtAudioFilePropertyID = fourcc(b"acnv");

fn property_size(property_id: ExtAudioFilePropertyID) -> Option<GuestUSize> {
    match property_id {
        kExtAudioFileProperty_FileDataFormat => {
            Some(guest_size_of::<AudioStreamBasicDescription>())
        }
        kExtAudioFileProperty_ClientDataFormat => {
            Some(guest_size_of::<AudioStreamBasicDescription>())
        }
        kExtAudioFileProperty_FileLengthFrames => Some(guest_size_of::<i64>()),
        kExtAudioFileProperty_AudioFile => Some(guest_size_of::<AudioFileID>()),
        kExtAudioFileProperty_AudioConverter => Some(guest_size_of::<u32>()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build an `ExtAudioFileHostObject` from an already-opened `audio::AudioFile`
/// and insert it into state, returning the new opaque handle written to
/// `out_ext_audio_file`.
fn register_ext_audio_file(
    env: &mut Environment,
    audio_file: audio::AudioFile,
    wrapped_id: Option<AudioFileID>,
    out_ext_audio_file: MutPtr<ExtAudioFileRef>,
) -> OSStatus {
    let host_object = ExtAudioFileHostObject {
        audio_file,
        client_format: None,
        frame_position: 0,
        wrapped_audio_file_id: wrapped_id,
    };
    let guest_ref = env
        .mem
        .alloc_and_write(OpaqueExtAudioFileID { _filler: 0 });
    State::get(&mut env.framework_state)
        .ext_audio_files
        .insert(guest_ref, host_object);
    env.mem.write(out_ext_audio_file, guest_ref);
    log_dbg!(
        "ExtAudioFile registered, new handle: {:?}",
        guest_ref
    );
    0 // success
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn ExtAudioFileOpenURL(
    env: &mut Environment,
    in_url: CFURLRef,
    out_ext_audio_file: MutPtr<ExtAudioFileRef>,
) -> OSStatus {
    return_if_null!(in_url);
    let path = to_rust_path(env, in_url);
    let audio_file = match audio::AudioFile::open_for_reading(path, &env.fs) {
        Ok(af) => af,
        Err(e) => {
            log!(
                "Warning: ExtAudioFileOpenURL() failed for {:?}: {:?}",
                in_url, e
            );
            return kExtAudioFileError_InvalidDataFormat;
        }
    };

    log_dbg!("ExtAudioFileOpenURL() opened {:?}", in_url);
    register_ext_audio_file(env, audio_file, None, out_ext_audio_file)
}

/// Wrap an existing `AudioFileID` in an `ExtAudioFileRef`.
/// The caller retains ownership of the `AudioFileID`; disposing the
/// `ExtAudioFileRef` does **not** close the underlying `AudioFileID`.
pub fn ExtAudioFileWrapAudioFileID(
    env: &mut Environment,
    in_audio_file: AudioFileID,
    _in_for_writing: bool,
    out_ext_audio_file: MutPtr<ExtAudioFileRef>,
) -> OSStatus {
    return_if_null!(in_audio_file);
    // We clone the audio data out of the existing host object so we have an
    // independent read cursor, but mark the original ID so Dispose skips the
    // memory free.
    let audio_file = {
        let host_obj = AudioFileState::get(&mut env.framework_state)
            .audio_files
            .get(&in_audio_file)
            .expect("ExtAudioFileWrapAudioFileID: unknown AudioFileID");
        // AudioFile must implement Clone (or we expose a dup helper).
        // If it doesn't yet, add `#[derive(Clone)]` to `audio::AudioFile`.
        host_obj.audio_file.clone()
    };

    log_dbg!(
        "ExtAudioFileWrapAudioFileID() wrapping AudioFileID {:?}",
        in_audio_file
    );
    register_ext_audio_file(
        env,
        audio_file,
        Some(in_audio_file),
        out_ext_audio_file,
    )
}

pub fn ExtAudioFileDispose(
    env: &mut Environment,
    in_ext_audio_file: ExtAudioFileRef,
) -> OSStatus {
    return_if_null!(in_ext_audio_file);
    let Some(host_object) = State::get(&mut env.framework_state)
        .ext_audio_files
        .remove(&in_ext_audio_file)
    else {
        log!(
            "Bad ExtAudioFileDispose for {:?} (likely double-dispose), ignoring!",
            in_ext_audio_file
        );
        return kExtAudioFileError_InvalidOperationOrder;
    };

    // Only free the guest allocation we own (the opaque handle itself).
    // If this was a wrapped AudioFileID the guest still owns that memory.
    if host_object.wrapped_audio_file_id.is_some() {
        log_dbg!(
            "ExtAudioFileDispose {:?}: wrapped AudioFileID retained by caller",
            in_ext_audio_file
        );
    }
    env.mem.free(in_ext_audio_file.cast());
    log_dbg!(
        "ExtAudioFileDispose() destroyed handle {:?}",
        in_ext_audio_file
    );
    0 // success
}

pub fn ExtAudioFileGetPropertyInfo(
    env: &mut Environment,
    in_ext_audio_file: ExtAudioFileRef,
    in_property_id: ExtAudioFilePropertyID,
    out_size: MutPtr<u32>,
    out_writable: MutPtr<u32>,
) -> OSStatus {
    return_if_null!(in_ext_audio_file);
    let Some(size) = property_size(in_property_id) else {
        log!(
            "Warning: ExtAudioFileGetPropertyInfo() unknown property {}",
            debug_fourcc(in_property_id)
        );
        return kExtAudioFileError_InvalidProperty;
    };

    // kExtAudioFileProperty_AudioConverter is not supported; signal that by
    // reporting a size of 0 and returning the unsupported error so callers
    // can gracefully skip converter-specific setup.
    if in_property_id == kExtAudioFileProperty_AudioConverter {
        if !out_size.is_null() {
            env.mem.write(out_size, 0);
        }
        if !out_writable.is_null() {
            env.mem.write(out_writable, 0);
        }
        return kExtAudioFileError_InvalidProperty;
    }

    if !out_size.is_null() {
        env.mem.write(out_size, size);
    }
    if !out_writable.is_null() {
        let writable: u32 =
            (in_property_id == kExtAudioFileProperty_ClientDataFormat) as u32;
        env.mem.write(out_writable, writable);
    }
    0 // success
}

pub fn ExtAudioFileGetProperty(
    env: &mut Environment,
    in_ext_audio_file: ExtAudioFileRef,
    in_property_id: ExtAudioFilePropertyID,
    io_data_size: MutPtr<u32>,
    out_property_data: MutVoidPtr,
) -> OSStatus {
    return_if_null!(in_ext_audio_file);
    let Some(required_size) = property_size(in_property_id) else {
        log!(
            "Warning: ExtAudioFileGetProperty() unknown property {}",
            debug_fourcc(in_property_id)
        );
        return kExtAudioFileError_InvalidProperty;
    };
    if env.mem.read(io_data_size) < required_size {
        log!("Warning: ExtAudioFileGetProperty() bad property size");
        return kExtAudioFileError_InvalidPropertySize;
    }

    let host_object = State::get(&mut env.framework_state)
        .ext_audio_files
        .get(&in_ext_audio_file)
        .expect("ExtAudioFileGetProperty: unknown ExtAudioFileRef");
    match in_property_id {
        kExtAudioFileProperty_FileDataFormat => {
            // Delegate to the underlying AudioFile property machinery by
            // re-using the same ASBD construction we do for AudioFileGetProperty.
            let desc = build_asbd(&host_object.audio_file);
            env.mem.write(out_property_data.cast(), desc);
        }
        kExtAudioFileProperty_ClientDataFormat => {
            // Return the client format if set, otherwise the file format.
            let desc = host_object
                .client_format
                .unwrap_or_else(|| build_asbd(&host_object.audio_file));
            env.mem.write(out_property_data.cast(), desc);
        }
        kExtAudioFileProperty_FileLengthFrames => {
            let desc = host_object.audio_file.audio_description();
            let total_frames: i64 = if desc.bytes_per_packet != 0 {
                (host_object.audio_file.byte_count() as i64
                    * desc.frames_per_packet as i64)
                    / desc.bytes_per_packet as i64
            } else {
                0
            };
            env.mem.write(out_property_data.cast(), total_frames);
        }
        kExtAudioFileProperty_AudioFile => {
            // We don't expose an AudioFileID for wrapped files (the original
            // AudioFileID is owned by the caller), so we return null here.
            // Callers should use the AudioFileID they already have.
            let null_id: u32 = 0;
            env.mem.write(out_property_data.cast(), null_id);
        }
        kExtAudioFileProperty_AudioConverter => {
            return kExtAudioFileError_InvalidProperty;
        }
        _ => unreachable!(),
    }

    0 // success
}

pub fn ExtAudioFileSetProperty(
    env: &mut Environment,
    in_ext_audio_file: ExtAudioFileRef,
    in_property_id: ExtAudioFilePropertyID,
    in_data_size: u32,
    in_property_data: MutVoidPtr,
) -> OSStatus {
    return_if_null!(in_ext_audio_file);
    match in_property_id {
        kExtAudioFileProperty_ClientDataFormat => {
            let required = guest_size_of::<AudioStreamBasicDescription>();
            if in_data_size < required {
                log!("Warning: ExtAudioFileSetProperty(ClientDataFormat) bad size");
                return kExtAudioFileError_InvalidPropertySize;
            }
            let new_format: AudioStreamBasicDescription =
                env.mem.read(in_property_data.cast());
            log_dbg!(
                "ExtAudioFileSetProperty(ClientDataFormat): {:?}",
                new_format
            );
            State::get(&mut env.framework_state)
                .ext_audio_files
                .get_mut(&in_ext_audio_file)
                .expect("ExtAudioFileSetProperty: unknown ExtAudioFileRef")
                .client_format = Some(new_format);
            0 // success
        }
        kExtAudioFileProperty_FileDataFormat
        | kExtAudioFileProperty_FileLengthFrames
        | kExtAudioFileProperty_AudioFile
        | kExtAudioFileProperty_AudioConverter => {
            log!(
                "Warning: ExtAudioFileSetProperty() read-only property {}",
                debug_fourcc(in_property_id)
            );
            kExtAudioFileError_InvalidProperty
        }
        _ => {
            log!(
                "Warning: ExtAudioFileSetProperty() unknown property {}",
                debug_fourcc(in_property_id)
            );
            kExtAudioFileError_InvalidProperty
        }
    }
}

/// `AudioBufferList` layout (single-buffer case):
///
/// ```c
/// struct AudioBufferList {
///     UInt32 mNumberBuffers;
///     AudioBuffer mBuffers[1];   // variable length
/// };
/// struct AudioBuffer {
///     UInt32 mNumberChannels;
///     UInt32 mDataByteSize;
///     void  *mData;
/// };
/// ```
#[repr(C, packed)]
#[derive(Clone, Copy)]
struct AudioBuffer {
    number_channels: u32,
    data_byte_size: u32,
    data: MutVoidPtr,
}
unsafe impl SafeRead for AudioBuffer {}

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct AudioBufferList {
    number_buffers: u32,
    // Followed in memory by `number_buffers` AudioBuffer entries.
    // We only ever deal with the first one.
    first_buffer: AudioBuffer,
}
unsafe impl SafeRead for AudioBufferList {}

// Inside ExtAudioFile.h

pub fn ExtAudioFileRead(
    env: &mut Environment,
    in_ext_audio_file: ExtAudioFileRef,
    io_num_frames: MutPtr<u32>,
    io_data: MutPtr<AudioBufferList>,
) -> OSStatus {
    return_if_null!(in_ext_audio_file);

    let frames_requested = env.mem.read(io_num_frames);
    if frames_requested == 0 {
        return 0; 
    }

    let (desc, frame_pos) = {
        let state = State::get(&mut env.framework_state);
        let host = state.ext_audio_files.get(&in_ext_audio_file).expect("Invalid Ref");
        (host.audio_file.audio_description(), host.frame_position)
    };

    // 1. Calculate how many packets we need to satisfy the frame request
    // For VBR (like AAC), frames_per_packet is > 1. For PCM, it's usually 1.
    let starting_packet = (frame_pos / desc.frames_per_packet as u64) as i64;
    let packets_to_read = frames_requested.div_ceil(desc.frames_per_packet);

    // 2. Prepare Guest Memory for the underlying AudioFileReadPackets call
    let io_num_packets_ptr: MutPtr<u32> = env.mem.alloc_and_write(packets_to_read);
    let out_num_bytes_ptr: MutPtr<u32> = env.mem.alloc_and_write(0u32);
    
    let abl: AudioBufferList = env.mem.read(io_data);
    let out_buffer = abl.first_buffer.data;

    // 3. Perform the actual read from the virtual filesystem/buffer
    // We transmute the ExtAudioFileRef to an AudioFileID for the shim call
    let status = AudioFileReadPackets(
        env,
        unsafe { std::mem::transmute(in_ext_audio_file) },
        false,
        out_num_bytes_ptr,
        MutVoidPtr::null(), // We don't support PacketDescriptions yet
        starting_packet,
        io_num_packets_ptr,
        out_buffer,
    );

    if status == 0 {
        let packets_actually_read = env.mem.read(io_num_packets_ptr);
        let bytes_actually_read = env.mem.read(out_num_bytes_ptr);
        let frames_actually_read = packets_actually_read * desc.frames_per_packet;

        // 4. Update the Host Object's playback cursor
        let host = State::get(&mut env.framework_state)
            .ext_audio_files
            .get_mut(&in_ext_audio_file)
            .unwrap();
        
        host.frame_position += frames_actually_read as u64;

        // 5. Update the Guest's io_num_frames and AudioBufferList
        env.mem.write(io_num_frames, frames_actually_read);
        
        let mut abl_updated = abl;
        abl_updated.first_buffer.data_byte_size = bytes_actually_read;
        env.mem.write(io_data, abl_updated);
    }

    env.mem.free(io_num_packets_ptr.cast());
    env.mem.free(out_num_bytes_ptr.cast());

    status
}

pub fn ExtAudioFileSeek(
    env: &mut Environment,
    in_ext_audio_file: ExtAudioFileRef,
    in_frame_offset: i64,
) -> OSStatus {
    return_if_null!(in_ext_audio_file);
    let Some(host_object) = State::get(&mut env.framework_state)
        .ext_audio_files
        .get_mut(&in_ext_audio_file)
    else {
        log!(
            "Warning: ExtAudioFileSeek() unknown handle {:?}",
            in_ext_audio_file
        );
        return kExtAudioFileError_InvalidOperationOrder;
    };

    if in_frame_offset < 0 {
        log!("Warning: ExtAudioFileSeek() negative offset not supported");
        return kExtAudioFileError_InvalidOperationOrder;
    }
    host_object.frame_position = in_frame_offset as u64;
    log_dbg!(
        "ExtAudioFileSeek() {:?} -> frame {}",
        in_ext_audio_file,
        in_frame_offset
    );
    0 // success
}

pub fn ExtAudioFileTell(
    env: &mut Environment,
    in_ext_audio_file: ExtAudioFileRef,
    out_frame_offset: MutPtr<i64>,
) -> OSStatus {
    return_if_null!(in_ext_audio_file);
    let host_object = State::get(&mut env.framework_state)
        .ext_audio_files
        .get(&in_ext_audio_file)
        .expect("ExtAudioFileTell: unknown ExtAudioFileRef");
    let pos = host_object.frame_position as i64;
    env.mem.write(out_frame_offset, pos);
    log_dbg!(
        "ExtAudioFileTell() {:?} -> frame {}",
        in_ext_audio_file,
        pos
    );
    0 // success
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Build an `AudioStreamBasicDescription` from an `audio::AudioFile`, mirroring
/// the logic in `AudioFileGetProperty` for `kAudioFilePropertyDataFormat`.
fn build_asbd(audio_file: &audio::AudioFile) -> AudioStreamBasicDescription {
    use crate::frameworks::core_audio_types::{
        kAudioFormatAppleIMA4, kAudioFormatFlagIsBigEndian, kAudioFormatFlagIsFloat,
        kAudioFormatFlagIsPacked, kAudioFormatFlagIsSignedInteger, kAudioFormatLinearPCM,
    };
    let audio::AudioDescription {
        sample_rate,
        format,
        bytes_per_packet,
        frames_per_packet,
        channels_per_frame,
        bits_per_channel,
    } = audio_file.audio_description();
    match format {
        audio::AudioFormat::LinearPcm {
            is_float,
            is_little_endian,
        } => {
            let is_packed = (bits_per_channel * channels_per_frame * frames_per_packet)
                == (bytes_per_packet * 8);
            let format_flags = (u32::from(is_float) * kAudioFormatFlagIsFloat)
                | (u32::from((!is_float) && matches!(bits_per_channel, 16 | 24))
                    * kAudioFormatFlagIsSignedInteger)
                | (u32::from(is_packed) * kAudioFormatFlagIsPacked)
                | (u32::from(!is_little_endian) * kAudioFormatFlagIsBigEndian);
            AudioStreamBasicDescription {
                sample_rate,
                format_id: kAudioFormatLinearPCM,
                format_flags,
                bytes_per_packet,
                frames_per_packet,
                bytes_per_frame: bytes_per_packet / frames_per_packet,
                channels_per_frame,
                bits_per_channel,
                _reserved: 0,
            }
        }
        audio::AudioFormat::AppleIma4 => AudioStreamBasicDescription {
            sample_rate,
            format_id: kAudioFormatAppleIMA4,
            format_flags: 0,
            bytes_per_packet,
            frames_per_packet,
            bytes_per_frame: 0,
            channels_per_frame,
            bits_per_channel,
            _reserved: 0,
        },
        // ИСПРАВЛЕНИЕ: Добавлен отсутствующий вариант Mpeg4Aac
        audio::AudioFormat::Mpeg4Aac => AudioStreamBasicDescription {
            sample_rate,
            format_id: fourcc(b"aac "), // Формат AAC
            format_flags: 0,
            bytes_per_packet,
            frames_per_packet,
            bytes_per_frame: 0,
            channels_per_frame,
            bits_per_channel,
            _reserved: 0,
        },
    }
}

// ---------------------------------------------------------------------------
// Function export table
// ---------------------------------------------------------------------------

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(ExtAudioFileOpenURL(_, _)),
    export_c_func!(ExtAudioFileWrapAudioFileID(_, _, _)),
    export_c_func!(ExtAudioFileDispose(_)),
    export_c_func!(ExtAudioFileGetPropertyInfo(_, _, _, _)),
    export_c_func!(ExtAudioFileGetProperty(_, _, _, _)),
    export_c_func!(ExtAudioFileSetProperty(_, _, _, _)),
    export_c_func!(ExtAudioFileRead(_, _, _)),
    export_c_func!(ExtAudioFileSeek(_, _)),
    export_c_func!(ExtAudioFileTell(_, _)),
];

