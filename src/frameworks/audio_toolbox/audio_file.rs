/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 * If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `AudioFile.h` (Audio File Services)

use crate::abi::{CallFromHost, GuestFunction};
use crate::audio; // Keep this module namespaced to avoid confusion
use crate::audio::AudioDescription;
use crate::dyld::{export_c_func, FunctionExports};
use crate::frameworks::carbon_core::{eofErr, paramErr, OSStatus};
use crate::frameworks::core_audio_types::{
    debug_fourcc, fourcc, kAudioFormatFlagIsBigEndian, kAudioFormatFlagIsFloat,
    kAudioFormatFlagIsPacked, kAudioFormatFlagIsSignedInteger, kAudioFormatLinearPCM,
    AudioStreamBasicDescription,
};
use crate::frameworks::core_foundation::cf_url::CFURLRef;
use crate::frameworks::foundation::ns_url::to_rust_path;
use crate::mem::{guest_size_of, GuestUSize, MutPtr, MutVoidPtr, SafeRead};
use crate::Environment;
use std::collections::HashMap;

#[derive(Default)]
pub struct State {
    pub audio_files: HashMap<AudioFileID, AudioFileHostObject>,
}
impl State {
    pub fn get(framework_state: &mut crate::frameworks::State) -> &mut Self {
        &mut framework_state.audio_toolbox.audio_file
    }
}

pub enum AudioFileHostObject {
    Real(audio::AudioFile),
    // 2-секундная заглушка, спасающая эмулятор от OOM (Out Of Memory)
    // если Symphonia или кастомный парсер не осилили файл.
    Dummy {
        format: AudioStreamBasicDescription,
        byte_count: u64,
        packet_count: u64,
    },
}

#[repr(C, packed)]
pub struct OpaqueAudioFileID {
    _filler: u8,
}
unsafe impl SafeRead for OpaqueAudioFileID {}

pub type AudioFileID = MutPtr<OpaqueAudioFileID>;

#[repr(C, packed)]
struct AudioFilePacketTableInfo {
    number_valid_frames: i64,
    priming_frames: i32,
    remainder_frames: i32,
}
unsafe impl SafeRead for AudioFilePacketTableInfo {}

#[allow(dead_code)]
const kAudioFileFileNotFoundError: OSStatus = -43;
const kAudioFileNotOpenError: OSStatus = -38;
const kAudioFileSuccess: OSStatus = 0; // ИСПРАВЛЕНО: Apple success code
const kAudioFileBadPropertySizeError: OSStatus = fourcc(b"!siz") as _;
const kAudioFileUnsupportedPropertyError: OSStatus = fourcc(b"pty?") as _;
const kAudioFileUnsupportedFileTypeError: OSStatus = fourcc(b"typ?") as _;
const kAudioFileUnspecifiedError: OSStatus = fourcc(b"wht?") as _;

type AudioFilePermissions = i8;
pub const kAudioFileReadPermission: AudioFilePermissions = 1;
pub const kAudioFileWritePermission: AudioFilePermissions = 2;
pub const kAudioFileReadWritePermission: AudioFilePermissions = 3;

type AudioFileTypeID = u32;
const kAudioFileCAFType: AudioFileTypeID = fourcc(b"caff");

type AudioFilePropertyID = u32;
pub const kAudioFilePropertyDataFormat: AudioFilePropertyID = fourcc(b"dfmt");
const kAudioFilePropertyAudioDataByteCount: AudioFilePropertyID = fourcc(b"bcnt");
const kAudioFilePropertyAudioDataPacketCount: AudioFilePropertyID = fourcc(b"pcnt");
pub const kAudioFilePropertyPacketSizeUpperBound: AudioFilePropertyID = fourcc(b"pkub");
pub const kAudioFilePropertyMaximumPacketSize: AudioFilePropertyID = fourcc(b"psze");
const kAudioFilePropertyMagicCookieData: AudioFilePropertyID = fourcc(b"mgic");
const kAudioFilePropertyChannelLayout: AudioFilePropertyID = fourcc(b"cmap");
const kAudioFilePropertyEstimatedDuration: AudioFilePropertyID = fourcc(b"edur");
const kAudioFilePropertyPacketTableInfo: AudioFilePropertyID = fourcc(b"pnfo");
const kAudioFilePropertyPacketToFrame: AudioFilePropertyID = fourcc(b"flst");
pub const kAudioFilePropertyFileFormat: AudioFilePropertyID = fourcc(b"ffmt");

// Генерация короткой (2 сек) тишины. Избегает крашей памяти (malloc fails), 
// так как весит всего ~350 КБ, что легко помещается в эмулируемую RAM.
fn create_dummy_audio_file() -> AudioFileHostObject {
    AudioFileHostObject::Dummy {
        format: AudioStreamBasicDescription {
            sample_rate: 44100.0,
            format_id: kAudioFormatLinearPCM,
            format_flags: kAudioFormatFlagIsSignedInteger | kAudioFormatFlagIsPacked,
            bytes_per_packet: 4,
            frames_per_packet: 1,
            bytes_per_frame: 4,
            channels_per_frame: 2,
            bits_per_channel: 16,
            _reserved: 0,
        },
        byte_count: 352800, // 2 секунды
        packet_count: 88200,
    }
}

pub fn AudioFileOpenURL(
    env: &mut Environment,
    in_file_ref: CFURLRef,
    in_permissions: AudioFilePermissions,
    in_file_type_hint: AudioFileTypeID,
    out_audio_file: MutPtr<AudioFileID>,
) -> OSStatus {
    return_if_null!(in_file_ref);

    if in_permissions != kAudioFileReadPermission {
        log!("Warning: AudioFileOpenURL() called with non-read permissions ({})", in_permissions);
    }

    if in_file_type_hint != 0 && in_file_type_hint != kAudioFileCAFType {
        log!("Ignoring unknown file type hint {} for AudioFileOpenURL()", debug_fourcc(in_file_type_hint));
    }

    let path = to_rust_path(env, in_file_ref);
    let host_object = match audio::AudioFile::open_for_reading(path.clone(), &env.fs) {
        Ok(audio_file) => AudioFileHostObject::Real(audio_file),
        Err(error) => {
            log!(
                "Warning: AudioFileOpenURL() for path {:?} failed: {:?}. Substituting 2-sec Dummy AudioFileHostObject.",
                path, error
            );
            create_dummy_audio_file()
        }
    };

    let guest_audio_file = env.mem.alloc_and_write(OpaqueAudioFileID { _filler: 0 });
    
    State::get(&mut env.framework_state)
        .audio_files
        .insert(guest_audio_file, host_object);
        
    if !out_audio_file.is_null() {
        env.mem.write(out_audio_file, guest_audio_file);
    }

    log_dbg!("AudioFileOpenURL() opened, new audio file handle: {:?}", guest_audio_file);
    kAudioFileSuccess
}

pub fn AudioFileOpenWithCallbacks(
    env: &mut Environment,
    client_data: MutVoidPtr,
    read_callback: GuestFunction,
    _write_callback: GuestFunction,
    getsize_callback: GuestFunction,
    _setsize_callback: GuestFunction,
    in_file_type_hint: AudioFileTypeID,
    out_audio_file: MutPtr<AudioFileID>,
) -> OSStatus {
    if _write_callback.to_ptr().is_null() || _setsize_callback.to_ptr().is_null() {
        log_dbg!("AudioFileOpenWithCallbacks() called with write/set_size callbacks");
    }
    
    let size: i64 = getsize_callback.call_from_host(env, (client_data,));
    let size: u32 = size.try_into().unwrap_or(0);
    
    if size == 0 {
        if !out_audio_file.is_null() { env.mem.write(out_audio_file, MutPtr::null()); }
        return kAudioFileUnspecifiedError;
    }
    
    let data_ptr: MutPtr<u8> = env.mem.alloc(size).cast();
    let bytes_read_ptr: MutPtr<u32> = env.mem.alloc(guest_size_of::<u32>()).cast();

    env.mem.write(bytes_read_ptr, 0);
    let status: OSStatus =
        read_callback.call_from_host(env, (client_data, 0_i64, size, data_ptr, bytes_read_ptr));
        
    if status != 0 {
        if !out_audio_file.is_null() { env.mem.write(out_audio_file, MutPtr::null()); }
        return status;
    }

    let actual_bytes_read = env.mem.read(bytes_read_ptr);
    let data_vec = env.mem.bytes_at(data_ptr, actual_bytes_read).to_vec();
        
    let host_object = match audio::AudioFile::read_from_vec(data_vec) {
        Ok(file) => AudioFileHostObject::Real(file),
        Err(_) => {
            log!("Warning: AudioFileOpenWithCallbacks() failed parse. Substituting Dummy AudioFileHostObject.");
            create_dummy_audio_file()
        }
    };
    
    let guest_audio_file = env.mem.alloc_and_write(OpaqueAudioFileID { _filler: 0 });
    State::get(&mut env.framework_state).audio_files.insert(guest_audio_file, host_object);
        
    if !out_audio_file.is_null() {
        env.mem.write(out_audio_file, guest_audio_file);
    }

    kAudioFileSuccess
}

fn property_size(property_id: AudioFilePropertyID) -> GuestUSize {
    match property_id {
        kAudioFilePropertyDataFormat => guest_size_of::<AudioStreamBasicDescription>(),
        kAudioFilePropertyAudioDataByteCount => guest_size_of::<u64>(),
        kAudioFilePropertyAudioDataPacketCount => guest_size_of::<u64>(),
        kAudioFilePropertyPacketSizeUpperBound => guest_size_of::<u32>(),
        kAudioFilePropertyMaximumPacketSize => guest_size_of::<u32>(),
        kAudioFilePropertyEstimatedDuration => guest_size_of::<f64>(),
        kAudioFilePropertyPacketTableInfo => guest_size_of::<AudioFilePacketTableInfo>(),
        kAudioFilePropertyPacketToFrame => guest_size_of::<f64>(),
        kAudioFilePropertyFileFormat => guest_size_of::<AudioFileTypeID>(),
        _ => 0,
    }
}

fn AudioFileGetPropertyInfo(
    env: &mut Environment,
    in_audio_file: AudioFileID,
    in_property_id: AudioFilePropertyID,
    out_data_size: MutPtr<u32>,
    is_writable: MutPtr<u32>,
) -> OSStatus {
    return_if_null!(in_audio_file);
    
    if in_property_id == kAudioFilePropertyMagicCookieData || in_property_id == kAudioFilePropertyChannelLayout {
        if !out_data_size.is_null() { env.mem.write(out_data_size, 0); }
        if !is_writable.is_null() { env.mem.write(is_writable, 0); }
        return kAudioFileUnsupportedPropertyError;
    }
    
    let req_size = property_size(in_property_id);
    if req_size == 0 {
        if !out_data_size.is_null() { env.mem.write(out_data_size, 0); }
        if !is_writable.is_null() { env.mem.write(is_writable, 0); }
        return kAudioFileUnsupportedPropertyError;
    }
    
    if !out_data_size.is_null() { env.mem.write(out_data_size, req_size); }
    if !is_writable.is_null() { env.mem.write(is_writable, 0); }
    
    kAudioFileSuccess
}

pub fn AudioFileGetProperty(
    env: &mut Environment,
    in_audio_file: AudioFileID,
    in_property_id: AudioFilePropertyID,
    io_data_size: MutPtr<u32>,
    out_property_data: MutVoidPtr,
) -> OSStatus {
    return_if_null!(in_audio_file);
    if io_data_size.is_null() { return paramErr; }

    let required_size = property_size(in_property_id);
    if required_size == 0 {
        return kAudioFileUnsupportedPropertyError;
    }
    
    let provided_size = env.mem.read(io_data_size);
    if provided_size < required_size {
        log!("Warning: AudioFileGetProperty() provided size {} < required {}", provided_size, required_size);
        return kAudioFileBadPropertySizeError;
    }

    env.mem.write(io_data_size, required_size);
    if out_property_data.is_null() { return kAudioFileSuccess; }

    let host_object = match State::get(&mut env.framework_state).audio_files.get_mut(&in_audio_file) {
        Some(obj) => obj,
        None => return kAudioFileNotOpenError,
    };

    match host_object {
        AudioFileHostObject::Real(audio_file) => {
            match in_property_id {
                kAudioFilePropertyDataFormat => {
                    let audio::AudioDescription { sample_rate, format, bytes_per_packet, frames_per_packet, channels_per_frame, bits_per_channel } = audio_file.audio_description();
                    
                    let desc: AudioStreamBasicDescription = match format {
                        audio::AudioFormat::LinearPcm { is_float, is_little_endian } => {
                            let is_packed = (bits_per_channel * channels_per_frame * frames_per_packet) == (bytes_per_packet * 8);
                            let format_flags = (u32::from(is_float) * kAudioFormatFlagIsFloat)
                                | (u32::from((!is_float) && matches!(bits_per_channel, 16 | 24)) * kAudioFormatFlagIsSignedInteger)
                                | (u32::from(is_packed) * kAudioFormatFlagIsPacked)
                                | (u32::from(!is_little_endian) * kAudioFormatFlagIsBigEndian);
                            AudioStreamBasicDescription {
                                sample_rate, format_id: kAudioFormatLinearPCM, format_flags,
                                bytes_per_packet, frames_per_packet, bytes_per_frame: bytes_per_packet / frames_per_packet,
                                channels_per_frame, bits_per_channel, _reserved: 0,
                            }
                        }
                        audio::AudioFormat::Mpeg4Aac => {
                            AudioStreamBasicDescription {
                                sample_rate, format_id: fourcc(b"aac "), format_flags: 0,
                                bytes_per_packet, frames_per_packet, bytes_per_frame: 0,
                                channels_per_frame, bits_per_channel, _reserved: 0,
                            }
                        }
                        audio::AudioFormat::AppleIma4 => {
                            AudioStreamBasicDescription {
                                sample_rate, format_id: fourcc(b"ima4"), format_flags: 0,
                                bytes_per_packet, frames_per_packet, bytes_per_frame: 0,
                                channels_per_frame, bits_per_channel, _reserved: 0,
                            }
                        }
                        // Защита: Если Symphonia парсит новые форматы (MP3, ALAC), и они не описаны выше
                        _ => {
                            AudioStreamBasicDescription {
                                sample_rate, format_id: fourcc(b"fmt?"), format_flags: 0,
                                bytes_per_packet, frames_per_packet, bytes_per_frame: 0,
                                channels_per_frame, bits_per_channel, _reserved: 0,
                            }
                        }
                    };
                    env.mem.write(out_property_data.cast(), desc);
                }
                kAudioFilePropertyAudioDataByteCount => env.mem.write(out_property_data.cast(), audio_file.byte_count()),
                kAudioFilePropertyAudioDataPacketCount => env.mem.write(out_property_data.cast(), audio_file.packet_count()),
                kAudioFilePropertyPacketSizeUpperBound | kAudioFilePropertyMaximumPacketSize => {
                    env.mem.write(out_property_data.cast(), audio_file.packet_size_upper_bound())
                },
                kAudioFilePropertyEstimatedDuration => {
                    let AudioDescription { sample_rate, bytes_per_packet, frames_per_packet, .. } = audio_file.audio_description();
                    let estimated_duration: f64 = audio_file.byte_count() as f64 * frames_per_packet as f64 / (bytes_per_packet as f64 * sample_rate);
                    env.mem.write(out_property_data.cast(), estimated_duration);
                }
                kAudioFilePropertyPacketTableInfo => return kAudioFileUnsupportedPropertyError,
                kAudioFilePropertyPacketToFrame => {
                    let AudioDescription { sample_rate, bytes_per_packet, frames_per_packet, .. } = audio_file.audio_description();
                    let estimated_duration: f64 = audio_file.byte_count() as f64 * frames_per_packet as f64 / (bytes_per_packet as f64 * sample_rate);
                    env.mem.write(out_property_data.cast(), estimated_duration);
                }
                kAudioFilePropertyFileFormat => env.mem.write(out_property_data.cast(), kAudioFileCAFType),
                _ => return kAudioFileUnsupportedPropertyError,
            }
        }
        AudioFileHostObject::Dummy { format, byte_count, packet_count } => {
            match in_property_id {
                kAudioFilePropertyDataFormat => env.mem.write(out_property_data.cast(), *format),
                kAudioFilePropertyAudioDataByteCount => env.mem.write(out_property_data.cast(), *byte_count),
                kAudioFilePropertyAudioDataPacketCount => env.mem.write(out_property_data.cast(), *packet_count),
                kAudioFilePropertyPacketSizeUpperBound | kAudioFilePropertyMaximumPacketSize => {
                    env.mem.write(out_property_data.cast(), format.bytes_per_packet)
                },
                kAudioFilePropertyEstimatedDuration => {
                    let duration = (*packet_count as f64) * (format.frames_per_packet as f64) / format.sample_rate;
                    env.mem.write(out_property_data.cast(), duration);
                }
                kAudioFilePropertyPacketToFrame => env.mem.write(out_property_data.cast(), 1.0f64),
                kAudioFilePropertyFileFormat => env.mem.write(out_property_data.cast(), kAudioFileCAFType),
                _ => return kAudioFileUnsupportedPropertyError,
            }
        }
    }

    kAudioFileSuccess
}

fn AudioFileReadBytes(
    env: &mut Environment,
    in_audio_file: AudioFileID,
    _in_use_cache: bool,
    in_starting_byte: i64,
    io_num_bytes: MutPtr<u32>,
    out_buffer: MutVoidPtr,
) -> OSStatus {
    return_if_null!(in_audio_file);
    if io_num_bytes.is_null() { return paramErr; }

    let host_object = match State::get(&mut env.framework_state).audio_files.get_mut(&in_audio_file) {
        Some(obj) => obj,
        None => return kAudioFileNotOpenError,
    };
        
    let bytes_to_read = env.mem.read(io_num_bytes);
    if bytes_to_read == 0 || out_buffer.is_null() {
        return kAudioFileSuccess;
    }

    let buffer_slice = env.mem.bytes_at_mut(out_buffer.cast(), bytes_to_read);
    
    let bytes_read = match host_object {
        AudioFileHostObject::Real(ref mut audio_file) => {
            audio_file.read_bytes(in_starting_byte.try_into().unwrap_or(0), buffer_slice).unwrap_or(0)
        }
        AudioFileHostObject::Dummy { byte_count, .. } => {
            for b in buffer_slice.iter_mut() { *b = 0; }
            let max_read = byte_count.saturating_sub(in_starting_byte as u64);
            std::cmp::min(bytes_to_read as u64, max_read) as usize
        }
    };
    
    env.mem.write(io_num_bytes, bytes_read.try_into().unwrap_or(0));

    if bytes_read < bytes_to_read as usize { eofErr } else { kAudioFileSuccess }
}

fn AudioFileReadPacketData(
    env: &mut Environment,
    in_audio_file: AudioFileID,
    in_use_cache: bool,
    out_num_bytes: MutPtr<u32>,
    out_packet_descriptions: MutVoidPtr,
    in_starting_packet: i64,
    io_num_packets: MutPtr<u32>,
    out_buffer: MutVoidPtr,
) -> OSStatus {
    AudioFileReadPackets(
        env, in_audio_file, in_use_cache, out_num_bytes,
        out_packet_descriptions, in_starting_packet, io_num_packets, out_buffer,
    )
}

pub fn AudioFileReadPackets(
    env: &mut Environment,
    in_audio_file: AudioFileID,
    _in_use_cache: bool,
    out_num_bytes: MutPtr<u32>,
    out_packet_descriptions: MutVoidPtr,
    in_starting_packet: i64,
    io_num_packets: MutPtr<u32>,
    out_buffer: MutVoidPtr,
) -> OSStatus {
    return_if_null!(in_audio_file);
    if io_num_packets.is_null() { return paramErr; }

    // Логирование из оригинала:
    // Variable-size packets are not implemented currently. When they are,
    // this parameter should be a `MutPtr<AudioStreamPacketDescription>`.
    if !out_packet_descriptions.is_null() {
        log!("Warning: ignoring non-null out_packet_descriptions in AudioFileReadPackets()");
    }

    let host_object = match State::get(&mut env.framework_state).audio_files.get_mut(&in_audio_file) {
        Some(obj) => obj,
        None => return kAudioFileNotOpenError,
    };

    let packet_size = match host_object {
        AudioFileHostObject::Real(audio_file) => audio_file.packet_size_fixed(),
        AudioFileHostObject::Dummy { format, .. } => format.bytes_per_packet,
    };

    let packets_to_read = env.mem.read(io_num_packets);
    
    if packet_size == 0 || packets_to_read == 0 {
        env.mem.write(io_num_packets, 0);
        if !out_num_bytes.is_null() { env.mem.write(out_num_bytes, 0); }
        return kAudioFileSuccess;
    }

    let starting_byte = match i64::from(packet_size).checked_mul(in_starting_packet) {
        Some(v) => v,
        None => return kAudioFileBadPropertySizeError,
    };
    
    let bytes_to_read = match packets_to_read.checked_mul(packet_size) {
        Some(v) => v,
        None => return kAudioFileBadPropertySizeError,
    };

    if bytes_to_read == 0 || out_buffer.is_null() {
        env.mem.write(io_num_packets, 0);
        if !out_num_bytes.is_null() { env.mem.write(out_num_bytes, 0); }
        return kAudioFileSuccess;
    }

    let buffer_slice = env.mem.bytes_at_mut(out_buffer.cast(), bytes_to_read);

    let bytes_read = match host_object {
        AudioFileHostObject::Real(ref mut audio_file) => {
            audio_file.read_bytes(starting_byte.try_into().unwrap_or(0), buffer_slice).unwrap_or(0)
        }
        AudioFileHostObject::Dummy { byte_count, .. } => {
            for b in buffer_slice.iter_mut() { *b = 0; }
            let max_read = byte_count.saturating_sub(starting_byte as u64);
            std::cmp::min(bytes_to_read as u64, max_read) as usize
        }
    };
    
    if !out_num_bytes.is_null() {
        env.mem.write(out_num_bytes, bytes_read.try_into().unwrap_or(0));
    }

    let packets_read = (bytes_read as u32) / packet_size;
    env.mem.write(io_num_packets, packets_read);

    if (bytes_read as u32) < bytes_to_read { eofErr } else { kAudioFileSuccess }
}

pub fn AudioFileClose(env: &mut Environment, in_audio_file: AudioFileID) -> OSStatus {
    return_if_null!(in_audio_file);
    let Some(_host_object) = State::get(&mut env.framework_state).audio_files.remove(&in_audio_file) else {
        log!("Bad AudioFileClose for {:?} (likely double close), ignoring!", in_audio_file);
        return kAudioFileUnspecifiedError;
    };
    env.mem.free(in_audio_file.cast());
    
    // Логирование из оригинала
    log_dbg!("AudioFileClose() destroyed audio file handle: {:?}", in_audio_file);
    
    kAudioFileSuccess
}

fn AudioFileStreamOpen(
    _env: &mut Environment,
    _in_client_data: MutVoidPtr,
    _in_property_listener_proc: MutVoidPtr,
    _in_packets_proc: MutVoidPtr,
    _in_file_type_hint: AudioFileTypeID,
    _out_audio_file_stream: MutVoidPtr,
) -> OSStatus {
    log!("TODO: AudioFileStreamOpen(), returning kAudioFileUnspecifiedError!");
    kAudioFileUnspecifiedError
}

pub fn AudioFormatGetPropertyInfo(
    _env: &mut Environment,
    property_id: AudioFilePropertyID,
    _specifier_size: u32,
    _specifier: crate::mem::ConstPtr<u8>,
    _out_property_data_size: MutPtr<u32>,
) -> OSStatus {
    -50 // paramErr
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(AudioFileOpenURL(_, _, _, _)),
    export_c_func!(AudioFileGetPropertyInfo(_, _, _, _)),
    export_c_func!(AudioFileGetProperty(_, _, _, _)),
    export_c_func!(AudioFileReadBytes(_, _, _, _, _)),
    export_c_func!(AudioFileReadPackets(_, _, _, _, _, _, _)),
    export_c_func!(AudioFileReadPacketData(_, _, _, _, _, _, _)),
    export_c_func!(AudioFileOpenWithCallbacks(_, _, _, _, _, _, _)),
    export_c_func!(AudioFileClose(_)),
    export_c_func!(AudioFileStreamOpen(_, _, _, _, _)),
    export_c_func!(AudioFormatGetPropertyInfo(_, _, _, _)),
];
