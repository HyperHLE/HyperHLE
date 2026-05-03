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

pub fn AudioFileWriteBytes(
    _env: &mut Environment,
    _in_audio_file: AudioFileID,
    _in_use_cache: bool,
    _in_starting_byte: i64,
    _io_num_bytes: MutPtr<u32>,
    _in_buffer: ConstVoidPtr,
) -> OSStatus {
    log!("TODO: AudioFileWriteBytes stubbed");
    kAudioFileOperationNotSupportedError
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
    in_use_cache: bool,
    out_num_bytes: MutPtr<u32>,
    out_packet_descriptions: MutVoidPtr,
    in_starting_packet: i64,
    io_num_packets: MutPtr<u32>,
    out_buffer: MutVoidPtr,
) -> OSStatus {
    return_if_null!(in_audio_file);

    if io_num_packets.is_null() {
        log!("AudioFileReadPackets: io_num_packets is null");
        return paramErr;
    }

    // PvM Fix #1: Validate out_buffer early
    let packets_to_read = env.mem.read(io_num_packets);
    if packets_to_read > 0 && out_buffer.is_null() {
        log!("AudioFileReadPackets: out_buffer is null but packets_to_read > 0");
        env.mem.write(io_num_packets, 0);
        if !out_num_bytes.is_null() {
            env.mem.write(out_num_bytes, 0);
        }
        return paramErr;
    }

    if !out_packet_descriptions.is_null() {
        log_dbg!("AudioFileReadPackets: ignoring non-null out_packet_descriptions");
    }

    let host_object = match State::get(&mut env.framework_state)
        .audio_files
        .get_mut(&in_audio_file)
    {
        Some(obj) => obj,
        None => {
            log!("AudioFileReadPackets: unknown AudioFileID {:?}", in_audio_file);
            return kAudioFileNotOpenError;
        }
    };

    let packet_size = match host_object {
        AudioFileHostObject::Real(audio_file) => audio_file.packet_size_fixed(),
        AudioFileHostObject::Dummy { format, .. } => format.bytes_per_packet,
    };

    // PvM Fix #2: Validate packet size
    if packet_size == 0 {
        log_dbg!("AudioFileReadPackets: packet_size is 0, returning success with 0 packets");
        env.mem.write(io_num_packets, 0);
        if !out_num_bytes.is_null() {
            env.mem.write(out_num_bytes, 0);
        }
        return kAudioFileSuccess;
    }

    // PvM Fix #3: Clamp unreasonably large packet requests
    // Some apps may request millions of packets accidentally
    const MAX_REASONABLE_PACKETS: u32 = 1_000_000;
    let packets_requested = if packets_to_read > MAX_REASONABLE_PACKETS {
        log!("AudioFileReadPackets: clamping excessive packet request {} to {}", 
             packets_to_read, MAX_REASONABLE_PACKETS);
        MAX_REASONABLE_PACKETS
    } else {
        packets_to_read
    };

    // Early-out: nothing to read
    if packets_requested == 0 {
        env.mem.write(io_num_packets, 0);
        if !out_num_bytes.is_null() {
            env.mem.write(out_num_bytes, 0);
        }
        return kAudioFileSuccess;
    }

    // PvM Fix #4: Handle negative starting packet gracefully
    if in_starting_packet < 0 {
        log_dbg!("AudioFileReadPackets: negative starting packet {}, returning EOF", 
                 in_starting_packet);
        env.mem.write(io_num_packets, 0);
        if !out_num_bytes.is_null() {
            env.mem.write(out_num_bytes, 0);
        }
        return eofErr;
    }

    // PvM Fix #5: Prevent integer overflow in byte calculations
    let starting_byte = match i64::from(packet_size).checked_mul(in_starting_packet) {
        Some(v) if v >= 0 => v,
        Some(v) => {
            log!("AudioFileReadPackets: starting_byte calculation resulted in negative value: {}", v);
            return kAudioFileBadPropertySizeError;
        }
        None => {
            log!("AudioFileReadPackets: starting_byte calculation overflow");
            return kAudioFileBadPropertySizeError;
        }
    };

    let bytes_to_read = match packets_requested.checked_mul(packet_size) {
        Some(v) => v,
        None => {
            log!("AudioFileReadPackets: bytes_to_read calculation overflow");
            return kAudioFileBadPropertySizeError;
        }
    };

    if bytes_to_read == 0 {
        env.mem.write(io_num_packets, 0);
        if !out_num_bytes.is_null() {
            env.mem.write(out_num_bytes, 0);
        }
        return kAudioFileSuccess;
    }

    // PvM Fix #6: Check if starting position is beyond file size
    let file_byte_count = match host_object {
        AudioFileHostObject::Real(ref audio_file) => {
            audio_file.byte_count().unwrap_or(u64::MAX)
        }
        AudioFileHostObject::Dummy { byte_count, .. } => *byte_count,
    };

    if starting_byte >= file_byte_count as i64 {
        log_dbg!("AudioFileReadPackets: starting_byte {} >= file size {}, returning EOF",
                 starting_byte, file_byte_count);
        env.mem.write(io_num_packets, 0);
        if !out_num_bytes.is_null() {
            env.mem.write(out_num_bytes, 0);
        }
        return eofErr;
    }

    // PvM Fix #7: Calculate actual available bytes and clamp read request
    let available_bytes = file_byte_count.saturating_sub(starting_byte as u64);
    let bytes_to_read_clamped = std::cmp::min(bytes_to_read as u64, available_bytes) as u32;
    let packets_can_read = bytes_to_read_clamped / packet_size;

    log_dbg!(
        "AudioFileReadPackets: file={:?}, start_pkt={}, req_pkts={}, pkt_sz={}, \
         start_byte={}, bytes_req={}, bytes_avail={}, pkts_avail={}, use_cache={}",
        in_audio_file,
        in_starting_packet,
        packets_requested,
        packet_size,
        starting_byte,
        bytes_to_read,
        available_bytes,
        packets_can_read,
        in_use_cache
    );

    // PvM Fix #8: Resolve the read BEFORE taking a mutable slice of guest memory
    // This avoids borrow-checker conflicts between host_object and env.mem
    let bytes_read: usize = match host_object {
        AudioFileHostObject::Real(ref mut audio_file) => {
            // PvM Fix #9: Use clamped size to prevent over-read
            let actual_bytes_to_read = bytes_to_read_clamped as usize;
            
            // Read into temporary host buffer first
            let mut tmp = vec![0u8; actual_bytes_to_read];
            
            let n = match audio_file.read_bytes(
                starting_byte.try_into().unwrap_or(0),
                &mut tmp
            ) {
                Ok(bytes_read) => bytes_read,
                Err(e) => {
                    log!("AudioFileReadPackets: read_bytes failed: {:?}", e);
                    0
                }
            };

            if n > 0 {
                // PvM Fix #10: Validate buffer size before copy
                let dst_slice = env.mem.bytes_at_mut(out_buffer.cast(), n as u32);
                if dst_slice.len() >= n {
                    dst_slice[..n].copy_from_slice(&tmp[..n]);
                } else {
                    log!("AudioFileReadPackets: destination buffer too small");
                    return kAudioFileBadPropertySizeError;
                }
            }
            
            n
        }
        AudioFileHostObject::Dummy { byte_count, .. } => {
            // Dummy file: zero-fill the buffer
            let max_read = byte_count.saturating_sub(starting_byte as u64);
            let n = std::cmp::min(bytes_to_read_clamped as u64, max_read) as usize;
            
            if n > 0 {
                let dst = env.mem.bytes_at_mut(out_buffer.cast(), n as u32);
                for b in dst.iter_mut() {
                    *b = 0;
                }
            }
            
            n
        }
    };

    // PvM Fix #11: Ensure packets_read aligns with packet boundaries
    let packets_read = if bytes_read > 0 {
        (bytes_read as u32) / packet_size
    } else {
        0
    };

    let actual_bytes_read = packets_read * packet_size;

    // Write results
    if !out_num_bytes.is_null() {
        env.mem.write(out_num_bytes, actual_bytes_read);
    }
    env.mem.write(io_num_packets, packets_read);

    log_dbg!(
        "AudioFileReadPackets: read {} packets ({} bytes), requested {} packets",
        packets_read,
        actual_bytes_read,
        packets_requested
    );

    // PvM Fix #12: CRITICAL - Only return eofErr when NO packets could be read
    // Returning eofErr on a PARTIAL read causes Plants vs. Monsters and other
    // apps to treat it as a hard error and freeze their audio decode loop.
    // 
    // The proper behavior:
    // - If we read SOME packets but fewer than requested: return SUCCESS
    //   The caller checks io_num_packets < requested to detect short read
    // - Only return eofErr when we're at EOF AND read zero packets
    //
    // This matches real iOS AudioFile behavior where partial reads are SUCCESS
    // and only a true zero-read at EOF returns eofErr.
    if packets_read == 0 {
        // PvM Fix #13: Only return EOF if we're actually past the start
        // Reading from packet 0 with 0 result is different from reading
        // from packet N with 0 result
        if in_starting_packet > 0 {
            log_dbg!("AudioFileReadPackets: EOF - read 0 packets at position {}", 
                     in_starting_packet);
            eofErr
        } else {
            // Starting at beginning but got nothing - could be empty file
            log_dbg!("AudioFileReadPackets: Empty file or zero-length read at start");
            kAudioFileSuccess
        }
    } else {
        // Successfully read at least one packet
        kAudioFileSuccess
    }
}

// PvM Fix #14: Helper function for safe packet-to-byte conversion
#[inline]
fn safe_packet_to_byte_offset(packet_index: i64, packet_size: u32) -> Option<i64> {
    i64::from(packet_size)
        .checked_mul(packet_index)
        .filter(|&v| v >= 0)
}

// PvM Fix #15: Helper function to validate audio file state
#[inline]
fn validate_audio_file_params(
    packet_size: u32,
    packets_requested: u32,
    starting_packet: i64,
) -> Result<(), OSStatus> {
    if packet_size == 0 {
        return Err(kAudioFileBadPropertySizeError);
    }
    
    if starting_packet < 0 {
        return Err(paramErr);
    }
    
    // Check for overflow in total bytes calculation
    if let Some(total_bytes) = packets_requested.checked_mul(packet_size) {
        // Reasonable limit: 100 MB per read
        const MAX_READ_SIZE: u32 = 100 * 1024 * 1024;
        if total_bytes > MAX_READ_SIZE {
            return Err(kAudioFileBadPropertySizeError);
        }
        Ok(())
    } else {
        Err(kAudioFileBadPropertySizeError)
    }
}

// PvM Fix #16: Enhanced logging for debugging PvM audio issues
#[cfg(debug_assertions)]
fn log_audio_read_state(
    file_id: AudioFileID,
    starting_packet: i64,
    requested: u32,
    read: u32,
    bytes: u32,
    status: OSStatus,
) {
    if read < requested || status != kAudioFileSuccess {
        log!(
            "AudioFileReadPackets [{:?}]: start_pkt={}, req={}, got={}, bytes={}, status={}",
            file_id,
            starting_packet,
            requested,
            read,
            bytes,
            status
        );
    }
}

#[cfg(not(debug_assertions))]
#[inline]
fn log_audio_read_state(
    _file_id: AudioFileID,
    _starting_packet: i64,
    _requested: u32,
    _read: u32,
    _bytes: u32,
    _status: OSStatus,
) {
    // No-op in release builds
}

pub fn AudioFileWritePackets(
    _env: &mut Environment,
    _in_audio_file: AudioFileID,
    _in_use_cache: bool,
    _in_num_bytes: u32,
    _in_packet_descriptions: ConstVoidPtr,
    _in_starting_packet: i64,
    _io_num_packets: MutPtr<u32>,
    _in_buffer: ConstVoidPtr,
) -> OSStatus {
    log!("TODO: AudioFileWritePackets stubbed");
    kAudioFileOperationNotSupportedError
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

pub fn AudioFileSetProperty(
    _env: &mut Environment,
    _in_audio_file: AudioFileID,
    _in_property_id: AudioFilePropertyID,
    _in_data_size: u32,
    _in_property_data: ConstVoidPtr,
) -> OSStatus {
    log!("TODO: AudioFileSetProperty stubbed");
    kAudioFileUnsupportedPropertyError
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
    _env: &mut Environment,
    _property_id: AudioFilePropertyID,
    _specifier_size: u32,
    _specifier: crate::mem::ConstPtr<u8>,
    _out_property_data_size: MutPtr<u32>,
) -> OSStatus {
    -50 // paramErr
}

// =========================================================================
// MARK: - Offline Rendering (AudioQueue)
// =========================================================================

/// Устанавливает режим offline-рендеринга и формат для очереди воспроизведения.
/// inFormat == NULL — возврат в обычный режим (вывод на устройство).
/// inLayout == NULL — не используется при отключении offline-режима.
/// Реализация-заглушка: offline-рендеринг в touchHLE не поддерживается,
/// функция всегда возвращает успех, чтобы не блокировать инициализацию.
fn AudioQueueSetOfflineRenderFormat(
    _env: &mut Environment,
    _in_aq: MutVoidPtr,
    _in_format: ConstVoidPtr,
    _in_layout: ConstVoidPtr,
) -> OSStatus {
    log!("TODO: AudioQueueSetOfflineRenderFormat stubbed");
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
    export_c_func!(AudioQueueSetOfflineRenderFormat(_, _, _)),
];
