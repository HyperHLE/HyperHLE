/*
 * Эта лицензия Source Code Form подпадает под условия Mozilla Public
 * License, v. 2.0.
 * Если копия MPL не распространялась вместе с этим
 * файлом, вы можете получить ее на https://mozilla.org/MPL/2.0/.
 */
//! `AudioFile.h` (Audio File Services)

use crate::abi::{CallFromHost, GuestFunction};
use crate::audio; // Избегаем путаницы имен
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
use crate::mem::{guest_size_of, ConstPtr, ConstVoidPtr, GuestUSize, MutPtr, MutVoidPtr, SafeRead};
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
    // если парсер не осилил файл.
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

// --- Официальные коды ошибок Audio File Services ---
const kAudioFileSuccess: OSStatus = 0;
const kAudioFileUnspecifiedError: OSStatus = fourcc(b"wht?") as _;
const kAudioFileUnsupportedFileTypeError: OSStatus = fourcc(b"typ?") as _;
const kAudioFileUnsupportedDataFormatError: OSStatus = fourcc(b"fmt?") as _;
// pub: используется в audio_queue.rs и других модулях
pub const kAudioFileUnsupportedPropertyError: OSStatus = fourcc(b"pty?") as _;
pub const kAudioFileBadPropertySizeError: OSStatus = fourcc(b"!siz") as _;
const kAudioFilePermissionsError: OSStatus = fourcc(b"prm?") as _;
const kAudioFileNotOptimizedError: OSStatus = fourcc(b"optm") as _;
const kAudioFileInvalidChunkError: OSStatus = fourcc(b"chk?") as _;
const kAudioFileDoesNotAllow64BitDataSizeError: OSStatus = fourcc(b"off?") as _;
const kAudioFileInvalidPacketOffsetError: OSStatus = fourcc(b"pck?") as _;
const kAudioFileInvalidFileError: OSStatus = fourcc(b"dta?") as _;
const kAudioFileOperationNotSupportedError: OSStatus = fourcc(b"op??") as _;
const kAudioFileNotOpenError: OSStatus = -38;
const kAudioFileEndOfFileError: OSStatus = eofErr;
const kAudioFilePositionError: OSStatus = -40;
#[allow(dead_code)]
const kAudioFileFileNotFoundError: OSStatus = -43;

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

const MAX_PACKET_SIZE_UPPER_BOUND: u32 = 65536;

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

// =========================================================================
// MARK: - Creating and Initializing Audio Files
// =========================================================================

pub fn AudioFileCreateWithURL(
    _env: &mut Environment,
    _in_file_ref: CFURLRef,
    _in_file_type: AudioFileTypeID,
    _in_format: ConstPtr<AudioStreamBasicDescription>,
    _in_flags: u32,
    _out_audio_file: MutPtr<AudioFileID>,
) -> OSStatus {
    log!("TODO: AudioFileCreateWithURL stubbed");
    kAudioFileOperationNotSupportedError
}

pub fn AudioFileInitializeWithCallbacks(
    _env: &mut Environment,
    _in_client_data: MutVoidPtr,
    _in_read_func: GuestFunction,
    _in_write_func: GuestFunction,
    _in_get_size_func: GuestFunction,
    _in_set_size_func: GuestFunction,
    _in_file_type: AudioFileTypeID,
    _in_format: ConstPtr<AudioStreamBasicDescription>,
    _in_flags: u32,
    _out_audio_file: MutPtr<AudioFileID>,
) -> OSStatus {
    log!("TODO: AudioFileInitializeWithCallbacks stubbed");
    kAudioFileOperationNotSupportedError
}

// =========================================================================
// MARK: - Opening and Closing Audio Files
// =========================================================================

pub fn AudioFileOpenURL(
    env: &mut Environment,
    in_file_ref: CFURLRef,
    in_permissions: AudioFilePermissions,
    in_file_type_hint: AudioFileTypeID,
    out_audio_file: MutPtr<AudioFileID>,
) -> OSStatus {
    return_if_null!(in_file_ref);

    if in_permissions != kAudioFileReadPermission {
        log!(
            "Внимание: AudioFileOpenURL() вызван с правами, отличными от чтения ({})",
            in_permissions
        );
    }

    if in_file_type_hint != 0 && in_file_type_hint != kAudioFileCAFType {
        log!(
            "Игнорируем неизвестный тип файла {} для AudioFileOpenURL()",
            debug_fourcc(in_file_type_hint)
        );
    }

    let path = to_rust_path(env, in_file_ref);
    let host_object = match audio::AudioFile::open_for_reading(path.clone(), &env.fs) {
        Ok(audio_file) => AudioFileHostObject::Real(audio_file),
        Err(error) => {
            log!(
                "Внимание: AudioFileOpenURL() для пути {:?} завершился ошибкой: \
                 {:?}. Подставляем Dummy AudioFile.",
                path,
                error
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

    kAudioFileSuccess
}

pub fn AudioFileOpenWithCallbacks(
    env: &mut Environment,
    client_data: MutVoidPtr,
    read_callback: GuestFunction,
    _write_callback: GuestFunction,
    getsize_callback: GuestFunction,
    _setsize_callback: GuestFunction,
    _in_file_type_hint: AudioFileTypeID,
    out_audio_file: MutPtr<AudioFileID>,
) -> OSStatus {
    if !_write_callback.to_ptr().is_null() || !_setsize_callback.to_ptr().is_null() {
        log_dbg!(
            "AudioFileOpenWithCallbacks() вызван с write/set_size \
             коллбэками (не поддерживается)"
        );
    }

    let size: i64 = getsize_callback.call_from_host(env, (client_data,));
    let size: u32 = size.try_into().unwrap_or(0);

    if size == 0 {
        if !out_audio_file.is_null() {
            env.mem.write(out_audio_file, MutPtr::null());
        }
        return kAudioFileUnspecifiedError;
    }

    // Цикл полного чтения файла
    let mut data_vec = Vec::with_capacity(size as usize);
    let chunk_size: u32 = 65536; // 64 КБ на один запрос
    let data_ptr: MutPtr<u8> = env.mem.alloc(chunk_size).cast();
    let bytes_read_ptr: MutPtr<u32> = env.mem.alloc(guest_size_of::<u32>()).cast();

    let mut current_offset: i64 = 0;
    let mut remaining = size;
    let mut final_status = 0;

    while remaining > 0 {
        let to_read = std::cmp::min(remaining, chunk_size);
        env.mem.write(bytes_read_ptr, 0);

        let status: OSStatus = read_callback.call_from_host(
            env,
            (
                client_data,
                current_offset,
                to_read,
                data_ptr,
                bytes_read_ptr,
            ),
        );

        if status != 0 {
            final_status = status;
            break;
        }

        let actual_read = env.mem.read(bytes_read_ptr);
        if actual_read == 0 {
            break; // Конец файла
        }

        let chunk = env.mem.bytes_at(data_ptr, actual_read);
        data_vec.extend_from_slice(chunk);

        current_offset += actual_read as i64;
        remaining -= actual_read;
    }

    env.mem.free(data_ptr.cast());
    env.mem.free(bytes_read_ptr.cast());

    if final_status != 0 && data_vec.is_empty() {
        if !out_audio_file.is_null() {
            env.mem.write(out_audio_file, MutPtr::null());
        }
        return final_status;
    }

    let host_object = match audio::AudioFile::read_from_vec(data_vec) {
        Ok(file) => AudioFileHostObject::Real(file),
        Err(e) => {
            log!(
                "Внимание: Ошибка парсинга в AudioFileOpenWithCallbacks(): \
                 {:?}. Dummy AudioFile.",
                e
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

    kAudioFileSuccess
}

pub fn AudioFileClose(env: &mut Environment, in_audio_file: AudioFileID) -> OSStatus {
    return_if_null!(in_audio_file);

    let Some(_host_object) = State::get(&mut env.framework_state)
        .audio_files
        .remove(&in_audio_file)
    else {
        log!(
            "Внимание: AudioFileClose для {:?} (повторное закрытие), игнорируем.",
            in_audio_file
        );
        return kAudioFileSuccess;
    };
    env.mem.free(in_audio_file.cast());
    kAudioFileSuccess
}

// =========================================================================
// MARK: - Reading and Writing Audio Files
// =========================================================================

pub fn AudioFileReadBytes(
    env: &mut Environment,
    in_audio_file: AudioFileID,
    _in_use_cache: bool,
    in_starting_byte: i64,
    io_num_bytes: MutPtr<u32>,
    out_buffer: MutVoidPtr,
) -> OSStatus {
    return_if_null!(in_audio_file);
    if io_num_bytes.is_null() {
        return paramErr;
    }

    if in_starting_byte < 0 {
        env.mem.write(io_num_bytes, 0);
        return eofErr;
    }

    let host_object = match State::get(&mut env.framework_state)
        .audio_files
        .get_mut(&in_audio_file)
    {
        Some(obj) => obj,
        None => return kAudioFileNotOpenError,
    };

    let bytes_to_read = env.mem.read(io_num_bytes);
    if bytes_to_read == 0 || out_buffer.is_null() {
        return kAudioFileSuccess;
    }

    let buffer_slice = env.mem.bytes_at_mut(out_buffer.cast(), bytes_to_read);

    let bytes_read = match host_object {
        AudioFileHostObject::Real(ref mut audio_file) => audio_file
            .read_bytes(in_starting_byte.try_into().unwrap_or(0), buffer_slice)
            .unwrap_or(0),
        AudioFileHostObject::Dummy { byte_count, .. } => {
            for b in buffer_slice.iter_mut() {
                *b = 0;
            }
            let max_read = byte_count.saturating_sub(in_starting_byte as u64);
            std::cmp::min(bytes_to_read as u64, max_read) as usize
        }
    };

    env.mem
        .write(io_num_bytes, bytes_read.try_into().unwrap_or(0));
    if bytes_read < bytes_to_read as usize {
        eofErr
    } else {
        kAudioFileSuccess
    }
}

/// Per Apple Audio File Services Reference:
/// AudioFileWriteBytes writes raw audio data to an audio file at the specified
/// byte offset. Parameters:
///   inAudioFile: The audio file to write to
///   inUseCache: Whether to use the write cache
///   inStartingByte: Byte offset to begin writing
///   ioNumBytes: On input, number of bytes to write; on output, actual written
///   inBuffer: Pointer to the data to write
///
/// Since HyperHLE's audio files are opened read-only from .ipa bundles,
/// write operations are only meaningful for files created via
/// AudioFileCreateWithURL (not yet implemented for filesystem writes).
/// We accept the data but discard it, returning success so apps that
/// write audio (recording, caching) don't crash.
pub fn AudioFileWriteBytes(
    env: &mut Environment,
    in_audio_file: AudioFileID,
    _in_use_cache: bool,
    _in_starting_byte: i64,
    io_num_bytes: MutPtr<u32>,
    _in_buffer: ConstVoidPtr,
) -> OSStatus {
    if in_audio_file.is_null() {
        return paramErr;
    }
    // For now we accept the write silently. The bytes_written output
    // equals the requested count (pretend all bytes were written).
    // This allows recording-capable apps to proceed without error.
    if !io_num_bytes.is_null() {
        // Leave io_num_bytes unchanged (= all bytes "written")
        let _ = env.mem.read(io_num_bytes);
    }
    log_dbg!("AudioFileWriteBytes: accepted {} bytes (discarded — file is virtual)",
        if io_num_bytes.is_null() { 0 } else { env.mem.read(io_num_bytes) });
    kAudioFileSuccess
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
        env,
        in_audio_file,
        in_use_cache,
        out_num_bytes,
        out_packet_descriptions,
        in_starting_packet,
        io_num_packets,
        out_buffer,
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
    if io_num_packets.is_null() {
        return paramErr;
    }

    if !out_packet_descriptions.is_null() {
        log!(
            "Внимание: игнорирование не-null out_packet_descriptions \
             в AudioFileReadPackets()"
        );
    }

    let host_object = match State::get(&mut env.framework_state)
        .audio_files
        .get_mut(&in_audio_file)
    {
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
        if !out_num_bytes.is_null() {
            env.mem.write(out_num_bytes, 0);
        }
        return kAudioFileSuccess;
    }

    if in_starting_packet < 0 {
        env.mem.write(io_num_packets, 0);
        if !out_num_bytes.is_null() {
            env.mem.write(out_num_bytes, 0);
        }
        return eofErr;
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
        if !out_num_bytes.is_null() {
            env.mem.write(out_num_bytes, 0);
        }
        return kAudioFileSuccess;
    }

    let buffer_slice = env.mem.bytes_at_mut(out_buffer.cast(), bytes_to_read);

    let bytes_read = match host_object {
        AudioFileHostObject::Real(ref mut audio_file) => audio_file
            .read_bytes(starting_byte.try_into().unwrap_or(0), buffer_slice)
            .unwrap_or(0),
        AudioFileHostObject::Dummy { byte_count, .. } => {
            for b in buffer_slice.iter_mut() {
                *b = 0;
            }
            let max_read = byte_count.saturating_sub(starting_byte as u64);
            std::cmp::min(bytes_to_read as u64, max_read) as usize
        }
    };

    if !out_num_bytes.is_null() {
        env.mem
            .write(out_num_bytes, bytes_read.try_into().unwrap_or(0));
    }

    let packets_read = (bytes_read as u32) / packet_size;
    env.mem.write(io_num_packets, packets_read);

    if (bytes_read as u32) < bytes_to_read {
        eofErr
    } else {
        kAudioFileSuccess
    }
}

/// Per Apple Audio File Services Reference:
/// AudioFileWritePackets writes packets of audio data to an audio file.
/// Parameters mirror AudioFileReadPackets but for writing.
///
/// Same approach as AudioFileWriteBytes: accept silently, report success.
pub fn AudioFileWritePackets(
    env: &mut Environment,
    in_audio_file: AudioFileID,
    _in_use_cache: bool,
    _in_num_bytes: u32,
    _in_packet_descriptions: ConstVoidPtr,
    _in_starting_packet: i64,
    io_num_packets: MutPtr<u32>,
    _in_buffer: ConstVoidPtr,
) -> OSStatus {
    if in_audio_file.is_null() {
        return paramErr;
    }
    let packets = if io_num_packets.is_null() { 0 } else { env.mem.read(io_num_packets) };
    log_dbg!("AudioFileWritePackets: accepted {} packets (discarded — file is virtual)", packets);
    kAudioFileSuccess
}

// =========================================================================
// MARK: - Getting and Setting Audio File Properties
// =========================================================================

pub(super) fn property_size(property_id: AudioFilePropertyID) -> GuestUSize {
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

    if in_property_id == kAudioFilePropertyMagicCookieData
        || in_property_id == kAudioFilePropertyChannelLayout
    {
        if !out_data_size.is_null() {
            env.mem.write(out_data_size, 0);
        }
        if !is_writable.is_null() {
            env.mem.write(is_writable, 0);
        }
        return kAudioFileUnsupportedPropertyError;
    }

    let req_size = property_size(in_property_id);

    if req_size == 0 {
        if !out_data_size.is_null() {
            env.mem.write(out_data_size, 0);
        }
        if !is_writable.is_null() {
            env.mem.write(is_writable, 0);
        }
        return kAudioFileUnsupportedPropertyError;
    }

    if !out_data_size.is_null() {
        env.mem.write(out_data_size, req_size);
    }
    if !is_writable.is_null() {
        env.mem.write(is_writable, 0);
    }

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
    if io_data_size.is_null() {
        return paramErr;
    }

    let required_size = property_size(in_property_id);
    if required_size == 0 {
        return kAudioFileUnsupportedPropertyError;
    }

    let provided_size = env.mem.read(io_data_size);
    if provided_size < required_size {
        return kAudioFileBadPropertySizeError;
    }

    env.mem.write(io_data_size, required_size);
    if out_property_data.is_null() {
        return kAudioFileSuccess;
    }

    let host_object = match State::get(&mut env.framework_state)
        .audio_files
        .get_mut(&in_audio_file)
    {
        Some(obj) => obj,
        None => return kAudioFileNotOpenError,
    };

    match host_object {
        AudioFileHostObject::Real(audio_file) => {
            match in_property_id {
                kAudioFilePropertyDataFormat => {
                    let AudioDescription {
                        sample_rate,
                        format,
                        bytes_per_packet,
                        frames_per_packet,
                        channels_per_frame,
                        bits_per_channel,
                    } = audio_file.audio_description();

                    let desc: AudioStreamBasicDescription = match format {
                        audio::AudioFormat::LinearPcm {
                            is_float,
                            is_little_endian,
                        } => {
                            let is_packed =
                                (bits_per_channel * channels_per_frame * frames_per_packet)
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
                        audio::AudioFormat::Mpeg4Aac => AudioStreamBasicDescription {
                            sample_rate,
                            format_id: fourcc(b"aac "),
                            format_flags: 0,
                            bytes_per_packet,
                            frames_per_packet,
                            bytes_per_frame: 0,
                            channels_per_frame,
                            bits_per_channel,
                            _reserved: 0,
                        },
                        audio::AudioFormat::AppleIma4 => AudioStreamBasicDescription {
                            sample_rate,
                            format_id: fourcc(b"ima4"),
                            format_flags: 0,
                            bytes_per_packet,
                            frames_per_packet,
                            bytes_per_frame: 0,
                            channels_per_frame,
                            bits_per_channel,
                            _reserved: 0,
                        },
                        _ => AudioStreamBasicDescription {
                            sample_rate,
                            format_id: fourcc(b"fmt?"),
                            format_flags: 0,
                            bytes_per_packet,
                            frames_per_packet,
                            bytes_per_frame: 0,
                            channels_per_frame,
                            bits_per_channel,
                            _reserved: 0,
                        },
                    };

                    env.mem.write(out_property_data.cast(), desc);
                }
                kAudioFilePropertyAudioDataByteCount => env
                    .mem
                    .write(out_property_data.cast(), audio_file.byte_count()),
                kAudioFilePropertyAudioDataPacketCount => env
                    .mem
                    .write(out_property_data.cast(), audio_file.packet_count()),
                kAudioFilePropertyPacketSizeUpperBound | kAudioFilePropertyMaximumPacketSize => {
                    let raw = audio_file.packet_size_upper_bound();
                    let capped = std::cmp::min(raw, MAX_PACKET_SIZE_UPPER_BOUND);
                    env.mem.write(out_property_data.cast(), capped)
                }
                kAudioFilePropertyEstimatedDuration => {
                    let AudioDescription {
                        sample_rate,
                        bytes_per_packet,
                        frames_per_packet,
                        ..
                    } = audio_file.audio_description();
                    let estimated_duration: f64 = if bytes_per_packet == 0 || sample_rate == 0.0 {
                        let pc = audio_file.packet_count() as f64;
                        let fpp = frames_per_packet as f64;
                        if sample_rate > 0.0 {
                            pc * fpp / sample_rate
                        } else {
                            0.0
                        }
                    } else {
                        audio_file.byte_count() as f64 * frames_per_packet as f64
                            / (bytes_per_packet as f64 * sample_rate)
                    };
                    env.mem.write(out_property_data.cast(), estimated_duration);
                }
                // kAudioFilePropertyPacketTableInfo
                // Возвращает AudioFilePacketTableInfo:
                //   mNumberValidFrames = packet_count * frames_per_packet
                //   mPrimingFrames     = 0  (нет данных об encoder delay)
                //   mRemainderFrames   = 0  (нет данных о хвостовом паддинге)
                // Сумма трёх полей == total frames, что соответствует
                // требованию Apple: sum == total frames in all packets.
                kAudioFilePropertyPacketTableInfo => {
                    let AudioDescription {
                        frames_per_packet, ..
                    } = audio_file.audio_description();
                    let valid_frames =
                        (audio_file.packet_count() as i64).saturating_mul(frames_per_packet as i64);
                    let info = AudioFilePacketTableInfo {
                        number_valid_frames: valid_frames,
                        priming_frames: 0,
                        remainder_frames: 0,
                    };
                    env.mem.write(out_property_data.cast(), info);
                }
                kAudioFilePropertyPacketToFrame => {
                    let AudioDescription {
                        frames_per_packet, ..
                    } = audio_file.audio_description();
                    env.mem
                        .write(out_property_data.cast(), frames_per_packet as f64);
                }
                kAudioFilePropertyFileFormat => {
                    env.mem.write(out_property_data.cast(), kAudioFileCAFType)
                }
                _ => return kAudioFileUnsupportedPropertyError,
            }
        }
        AudioFileHostObject::Dummy {
            format,
            byte_count,
            packet_count,
        } => {
            match in_property_id {
                kAudioFilePropertyDataFormat => env.mem.write(out_property_data.cast(), *format),
                kAudioFilePropertyAudioDataByteCount => {
                    env.mem.write(out_property_data.cast(), *byte_count)
                }
                kAudioFilePropertyAudioDataPacketCount => {
                    env.mem.write(out_property_data.cast(), *packet_count)
                }
                kAudioFilePropertyPacketSizeUpperBound | kAudioFilePropertyMaximumPacketSize => env
                    .mem
                    .write(out_property_data.cast(), format.bytes_per_packet),
                kAudioFilePropertyEstimatedDuration => {
                    let duration = (*packet_count as f64) * (format.frames_per_packet as f64)
                        / format.sample_rate;
                    env.mem.write(out_property_data.cast(), duration);
                }
                // Для Dummy: все фреймы считаются валидными, padding = 0.
                kAudioFilePropertyPacketTableInfo => {
                    let valid_frames =
                        (*packet_count as i64).saturating_mul(format.frames_per_packet as i64);
                    let info = AudioFilePacketTableInfo {
                        number_valid_frames: valid_frames,
                        priming_frames: 0,
                        remainder_frames: 0,
                    };
                    env.mem.write(out_property_data.cast(), info);
                }
                kAudioFilePropertyPacketToFrame => env
                    .mem
                    .write(out_property_data.cast(), format.frames_per_packet as f64),
                kAudioFilePropertyFileFormat => {
                    env.mem.write(out_property_data.cast(), kAudioFileCAFType)
                }
                _ => return kAudioFileUnsupportedPropertyError,
            }
        }
    }

    kAudioFileSuccess
}

/// Per Apple Audio File Services Reference:
/// AudioFileSetProperty sets the value of an audio file property.
/// Properties that can be set include kAudioFilePropertyMagicCookieData,
/// kAudioFilePropertyDataFormat (before writing), etc.
///
/// Since our audio files are virtual/read-only during emulation,
/// we accept the property silently and return success. This allows
/// apps that configure audio file properties before writing to proceed.
pub fn AudioFileSetProperty(
    env: &mut Environment,
    in_audio_file: AudioFileID,
    in_property_id: AudioFilePropertyID,
    in_data_size: u32,
    _in_property_data: ConstVoidPtr,
) -> OSStatus {
    if in_audio_file.is_null() {
        return paramErr;
    }
    log_dbg!(
        "AudioFileSetProperty({:?}, {}, size={}): accepted (no-op for virtual files)",
        in_audio_file,
        debug_fourcc(in_property_id),
        in_data_size
    );
    let _ = env; // suppress unused warning
    kAudioFileSuccess
}

// =========================================================================
// MARK: - Working with User Data
// =========================================================================

pub fn AudioFileCountUserData(
    _env: &mut Environment,
    _in_audio_file: AudioFileID,
    _in_user_data_id: u32,
    _out_number_items: MutPtr<u32>,
) -> OSStatus {
    log!("TODO: AudioFileCountUserData stubbed");
    kAudioFileUnsupportedPropertyError
}

pub fn AudioFileGetUserDataSize(
    _env: &mut Environment,
    _in_audio_file: AudioFileID,
    _in_user_data_id: u32,
    _in_index: u32,
    _out_user_data_size: MutPtr<u32>,
) -> OSStatus {
    log!("TODO: AudioFileGetUserDataSize stubbed");
    kAudioFileUnsupportedPropertyError
}

pub fn AudioFileGetUserDataSize64(
    _env: &mut Environment,
    _in_audio_file: AudioFileID,
    _in_user_data_id: u32,
    _in_index: u32,
    _out_user_data_size: MutPtr<u64>,
) -> OSStatus {
    log!("TODO: AudioFileGetUserDataSize64 stubbed");
    kAudioFileUnsupportedPropertyError
}

pub fn AudioFileGetUserData(
    _env: &mut Environment,
    _in_audio_file: AudioFileID,
    _in_user_data_id: u32,
    _in_index: u32,
    _io_user_data_size: MutPtr<u32>,
    _out_user_data: MutVoidPtr,
) -> OSStatus {
    log!("TODO: AudioFileGetUserData stubbed");
    kAudioFileUnsupportedPropertyError
}

pub fn AudioFileGetUserDataAtOffset(
    _env: &mut Environment,
    _in_audio_file: AudioFileID,
    _in_user_data_id: u32,
    _in_index: u32,
    _in_offset: i64,
    _io_user_data_size: MutPtr<u32>,
    _out_user_data: MutVoidPtr,
) -> OSStatus {
    log!("TODO: AudioFileGetUserDataAtOffset stubbed");
    kAudioFileUnsupportedPropertyError
}

pub fn AudioFileSetUserData(
    _env: &mut Environment,
    _in_audio_file: AudioFileID,
    _in_user_data_id: u32,
    _in_index: u32,
    _in_user_data_size: u32,
    _in_user_data: ConstVoidPtr,
) -> OSStatus {
    log!("TODO: AudioFileSetUserData stubbed");
    kAudioFileUnsupportedPropertyError
}

pub fn AudioFileRemoveUserData(
    _env: &mut Environment,
    _in_audio_file: AudioFileID,
    _in_user_data_id: u32,
    _in_index: u32,
) -> OSStatus {
    log!("TODO: AudioFileRemoveUserData stubbed");
    kAudioFileUnsupportedPropertyError
}

// =========================================================================
// MARK: - Working with Global Information
// =========================================================================

pub fn AudioFileGetGlobalInfoSize(
    _env: &mut Environment,
    _in_property_id: AudioFilePropertyID,
    _in_specifier_size: u32,
    _in_specifier: MutVoidPtr,
    _out_data_size: MutPtr<u32>,
) -> OSStatus {
    log!("TODO: AudioFileGetGlobalInfoSize stubbed");
    kAudioFileUnsupportedPropertyError
}

pub fn AudioFileGetGlobalInfo(
    _env: &mut Environment,
    _in_property_id: AudioFilePropertyID,
    _in_specifier_size: u32,
    _in_specifier: MutVoidPtr,
    _io_data_size: MutPtr<u32>,
    _out_property_data: MutVoidPtr,
) -> OSStatus {
    log!("TODO: AudioFileGetGlobalInfo stubbed");
    kAudioFileUnsupportedPropertyError
}

// =========================================================================
// MARK: - Optimizing Audio Files
// =========================================================================

pub fn AudioFileOptimize(_env: &mut Environment, _in_audio_file: AudioFileID) -> OSStatus {
    log!("TODO: AudioFileOptimize stubbed");
    kAudioFileOperationNotSupportedError
}

// =========================================================================
// MARK: - AudioFileStreamOpen (Устаревшее / Streaming)
// =========================================================================

fn AudioFileStreamOpen(
    _env: &mut Environment,
    _in_client_data: MutVoidPtr,
    _in_property_listener_proc: MutVoidPtr,
    _in_packets_proc: MutVoidPtr,
    _in_file_type_hint: AudioFileTypeID,
    _out_audio_file_stream: MutVoidPtr,
) -> OSStatus {
    log!("TODO: AudioFileStreamOpen stubbed");
    kAudioFileUnspecifiedError
}

pub fn AudioFormatGetPropertyInfo(
    env: &mut Environment,
    property_id: AudioFilePropertyID,
    _specifier_size: u32,
    _specifier: crate::mem::ConstPtr<u8>,
    out_property_data_size: MutPtr<u32>,
) -> OSStatus {
    // kAudioFormatProperty_Encoders = 'aenc' (0x61656E63)
    // kAudioFormatProperty_Decoders = 'adec' (0x61646563)
    // kAudioFormatProperty_FormatList = 'flst'
    // kAudioFormatProperty_FormatInfo = 'fmti'
    //
    // Apple docs: AudioFormatGetPropertyInfo returns the size in bytes of
    // the data for the given property. When the property is a list of items,
    // the number of items = size / sizeof(one_item).
    //
    // For Encoders/Decoders we return size=0 indicating no encoders/decoders
    // are available on this (emulated) device. This is a valid response that
    // apps handle gracefully — they simply skip encoding or fall back to
    // raw PCM.
    let prop_name = crate::frameworks::core_audio_types::debug_fourcc(property_id);
    log_dbg!(
        "AudioFormatGetPropertyInfo(property='{}') => size=0 (no codecs available)",
        prop_name
    );

    if !out_property_data_size.is_null() {
        env.mem.write(out_property_data_size, 0u32);
    }
    kAudioFileSuccess
}

/// `AudioFormatGetProperty` — retrieve audio format property data.
///
/// Apple docs: Gets the value of an audio format property.
/// Since we report size=0 for most properties in GetPropertyInfo, callers
/// typically won't call this with a non-zero buffer. If they do, we return
/// noErr with an empty result.
pub fn AudioFormatGetProperty(
    env: &mut Environment,
    property_id: AudioFilePropertyID,
    _specifier_size: u32,
    _specifier: crate::mem::ConstPtr<u8>,
    io_property_data_size: MutPtr<u32>,
    _out_property_data: MutVoidPtr,
) -> OSStatus {
    let prop_name = crate::frameworks::core_audio_types::debug_fourcc(property_id);
    log_dbg!(
        "AudioFormatGetProperty(property='{}') => returning empty",
        prop_name
    );
    // Set output size to 0 — no data written
    if !io_property_data_size.is_null() {
        env.mem.write(io_property_data_size, 0u32);
    }
    kAudioFileSuccess
}

// =========================================================================
// MARK: - Exports
// =========================================================================

// Число _ = число параметров функции минус 1 (env не считается)
pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(AudioFileCreateWithURL(_, _, _, _, _)),
    export_c_func!(AudioFileInitializeWithCallbacks(_, _, _, _, _, _, _, _, _)),
    export_c_func!(AudioFileOpenURL(_, _, _, _)),
    export_c_func!(AudioFileOpenWithCallbacks(_, _, _, _, _, _, _)),
    export_c_func!(AudioFileClose(_)),
    export_c_func!(AudioFileReadBytes(_, _, _, _, _)),
    export_c_func!(AudioFileWriteBytes(_, _, _, _, _)),
    export_c_func!(AudioFileReadPackets(_, _, _, _, _, _, _)),
    export_c_func!(AudioFileReadPacketData(_, _, _, _, _, _, _)),
    export_c_func!(AudioFileWritePackets(_, _, _, _, _, _, _)),
    export_c_func!(AudioFileGetPropertyInfo(_, _, _, _)),
    export_c_func!(AudioFileGetProperty(_, _, _, _)),
    export_c_func!(AudioFileSetProperty(_, _, _, _)),
    export_c_func!(AudioFileCountUserData(_, _, _)),
    export_c_func!(AudioFileGetUserDataSize(_, _, _, _)),
    export_c_func!(AudioFileGetUserDataSize64(_, _, _, _)),
    export_c_func!(AudioFileGetUserData(_, _, _, _, _)),
    export_c_func!(AudioFileGetUserDataAtOffset(_, _, _, _, _, _)),
    export_c_func!(AudioFileSetUserData(_, _, _, _, _)),
    export_c_func!(AudioFileRemoveUserData(_, _, _)),
    export_c_func!(AudioFileGetGlobalInfoSize(_, _, _, _)),
    export_c_func!(AudioFileGetGlobalInfo(_, _, _, _, _)),
    export_c_func!(AudioFileOptimize(_)),
    export_c_func!(AudioFileStreamOpen(_, _, _, _, _)),
    export_c_func!(AudioFormatGetPropertyInfo(_, _, _, _)),
    export_c_func!(AudioFormatGetProperty(_, _, _, _, _)),
];
