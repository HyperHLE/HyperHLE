/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `AudioQueue.h` (Audio Queue Services)
//!
//! The audio playback here is mapped onto OpenAL Soft for convenience.
//! Apple's implementation probably uses Core Audio instead.

use crate::abi::{CallFromHost, GuestFunction};
use crate::audio::decode_ima4;
use crate::audio::openal as al;
use crate::audio::openal::al_types::*;
use crate::audio::openal::{OpenAL, OpenALManager};
use crate::dyld::{export_c_func, FunctionExports};
use crate::frameworks::carbon_core::OSStatus;
use crate::frameworks::core_audio_types::{
    debug_fourcc, fourcc, kAudioFormatAppleIMA4, kAudioFormatFlagIsBigEndian,
    kAudioFormatFlagIsFloat, kAudioFormatFlagIsPacked,
    kAudioFormatLinearPCM, AudioStreamBasicDescription,
};
use crate::frameworks::core_foundation::cf_run_loop::{
    kCFRunLoopCommonModes, CFRunLoopGetMain, CFRunLoopMode, CFRunLoopRef,
};
use crate::frameworks::foundation::ns_run_loop;
use crate::frameworks::foundation::ns_string::get_static_str;
use crate::mem::{
    guest_size_of, ConstPtr, ConstVoidPtr, GuestUSize, Mem, MutPtr, MutVoidPtr, Ptr, SafeRead,
};
use crate::objc::msg;
use crate::Environment;
use std::collections::{HashMap, VecDeque};

#[derive(Default)]
pub struct State {
    audio_queues: HashMap<AudioQueueRef, AudioQueueHostObject>,
}

impl State {
    fn get(framework_state: &mut crate::frameworks::State) -> &mut Self {
        &mut framework_state.audio_toolbox.audio_queue
    }
    fn get_with_context<'s, 'm: 's>(
        framework_state: &'s mut crate::frameworks::State,
        manager: &'m mut OpenALManager,
    ) -> (&'s mut Self, OpenAL<'s>) {
        (
            &mut framework_state.audio_toolbox.audio_queue,
            framework_state
                .audio_toolbox
                .al_context
                .make_al_context_current(manager),
        )
    }
}

struct AudioQueueHostObject {
    format: AudioStreamBasicDescription,
    callback_proc: AudioQueueOutputCallback,
    callback_user_data: MutVoidPtr,
    /// Weak reference
    run_loop: CFRunLoopRef,
    volume: f32,
    buffers: Vec<AudioQueueBufferRef>,
    buffer_queue: VecDeque<AudioQueueBufferRef>,
    is_running: AudioQueueIsRunning,
    al_source: Option<ALuint>,
    al_unused_buffers: Vec<ALuint>,
    aq_is_running_proc: Option<AudioQueuePropertyListenerProc>,
    aq_is_running_user_data: Option<MutVoidPtr>,
    is_running_handler: bool,
    is_input: bool,
    input_delay: u32,
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum AudioQueueIsRunning {
    Running,
    Stopping,
    Stopped,
}

#[repr(C, packed)]
pub struct OpaqueAudioQueue {
    _filler: u8,
}
unsafe impl SafeRead for OpaqueAudioQueue {}

pub type AudioQueueRef = MutPtr<OpaqueAudioQueue>;

#[repr(C, packed)]
pub struct AudioQueueBuffer {
    audio_data_bytes_capacity: u32,
    pub audio_data: MutVoidPtr,
    pub audio_data_byte_size: u32,
    user_data: MutVoidPtr,
    packet_description_capacity: u32,
    _packet_descriptions: MutVoidPtr,
    _packet_description_count: u32,
}
unsafe impl SafeRead for AudioQueueBuffer {}

pub type AudioQueueBufferRef = MutPtr<AudioQueueBuffer>;

pub type AudioQueueOutputCallback = GuestFunction;

type AudioQueueParameterID = u32;
pub const kAudioQueueParam_Volume: AudioQueueParameterID = 1;

type AudioQueueParameterValue = f32;

pub type AudioQueuePropertyID = u32;
pub const kAudioQueueProperty_IsRunning: AudioQueuePropertyID = fourcc(b"aqrn");
const kAudioQueueProperty_MagicCookie: AudioQueuePropertyID = fourcc(b"aqmc");
const kAudioQueueProperty_StreamDescription: AudioQueuePropertyID = fourcc(b"aqft");

type AudioQueuePropertyListenerProc = GuestFunction;

const kAudioQueueErr_InvalidBuffer: OSStatus = -66687;
const kAudioQueueErr_InvalidPropertySize: OSStatus = -66683;
const kAudioQueueErr_BufferInQueue: OSStatus = -66679;
const kAudioQueueErr_InvalidProperty: OSStatus = -66684;

pub fn AudioQueueNewOutput(
    env: &mut Environment,
    in_format: ConstPtr<AudioStreamBasicDescription>,
    in_callback_proc: AudioQueueOutputCallback,
    in_user_data: MutVoidPtr,
    in_callback_run_loop: CFRunLoopRef,
    in_callback_run_loop_mode: CFRunLoopMode,
    in_flags: u32,
    out_aq: MutPtr<AudioQueueRef>,
) -> OSStatus {
    assert!(in_flags == 0);
    assert!(
        in_callback_run_loop_mode.is_null() || {
            let common_modes = get_static_str(env, kCFRunLoopCommonModes);
            msg![env; in_callback_run_loop_mode isEqual:common_modes]
        }
    );

    let in_callback_run_loop = if in_callback_run_loop.is_null() {
        CFRunLoopGetMain(env)
    } else {
        in_callback_run_loop
    };

    let mut format = env.mem.read(in_format);
    if env.bundle.bundle_identifier().starts_with("com.ea.candcra")
        && format.format_id == fourcc(b".mp3")
    {
        log!("Applying hack for C&C Red Alert: Fixing audio format to PCM.");
        format = AudioStreamBasicDescription {
            sample_rate: 44100.0,
            format_id: kAudioFormatLinearPCM,
            format_flags: 12,
            bytes_per_packet: 4,
            frames_per_packet: 1,
            bytes_per_frame: 4,
            channels_per_frame: 2,
            bits_per_channel: 16,
            _reserved: 0,
        }
    }

    let host_object = AudioQueueHostObject {
        format,
        callback_proc: in_callback_proc,
        callback_user_data: in_user_data,
        run_loop: in_callback_run_loop,
        volume: 1.0,
        buffers: Vec::new(),
        buffer_queue: VecDeque::new(),
        is_running: AudioQueueIsRunning::Stopped,
        al_source: None,
        al_unused_buffers: Vec::new(),
        aq_is_running_proc: None,
        aq_is_running_user_data: None,
        is_running_handler: false,
        is_input: false,
        input_delay: 0,
    };

    let aq_ref = env.mem.alloc_and_write(OpaqueAudioQueue { _filler: 0 });
    State::get(&mut env.framework_state)
        .audio_queues
        .insert(aq_ref, host_object);
    env.mem.write(out_aq, aq_ref);

    ns_run_loop::add_audio_queue(env, in_callback_run_loop, aq_ref);

    log_if_broken_audio_format(&format);

    if !is_supported_audio_format(&format) {
        log_dbg!("Warning: Format not supported yet: {:#?}", format);
    }

    0 
}

pub fn AudioQueueNewInput(
    env: &mut Environment,
    in_format: ConstPtr<AudioStreamBasicDescription>,
    in_callback_proc: AudioQueueOutputCallback,
    in_user_data: MutVoidPtr,
    in_callback_run_loop: CFRunLoopRef,
    in_callback_run_loop_mode: CFRunLoopMode,
    in_flags: u32,
    out_aq: MutPtr<AudioQueueRef>,
) -> OSStatus {
    log!("TODO: AudioQueueNewInput(...) stubbed to prevent crash");
    assert!(in_flags == 0);

    let in_callback_run_loop = if in_callback_run_loop.is_null() {
        CFRunLoopGetMain(env)
    } else {
        in_callback_run_loop
    };

    let format = env.mem.read(in_format);

    let host_object = AudioQueueHostObject {
        format,
        callback_proc: in_callback_proc,
        callback_user_data: in_user_data,
        run_loop: in_callback_run_loop,
        volume: 1.0,
        buffers: Vec::new(),
        buffer_queue: VecDeque::new(),
        is_running: AudioQueueIsRunning::Stopped,
        al_source: None,
        al_unused_buffers: Vec::new(),
        aq_is_running_proc: None,
        aq_is_running_user_data: None,
        is_running_handler: false,
        is_input: true,
        input_delay: 0,
    };

    let aq_ref = env.mem.alloc_and_write(OpaqueAudioQueue { _filler: 0 });
    State::get(&mut env.framework_state)
        .audio_queues
        .insert(aq_ref, host_object);

    if !out_aq.is_null() {
        env.mem.write(out_aq, aq_ref);
    }

    ns_run_loop::add_audio_queue(env, in_callback_run_loop, aq_ref);
    0
}

pub fn AudioQueueGetParameter(
    env: &mut Environment,
    in_aq: AudioQueueRef,
    in_param_id: AudioQueueParameterID,
    out_value: MutPtr<AudioQueueParameterValue>,
) -> OSStatus {
    return_if_null!(in_aq);
    assert!(in_param_id == kAudioQueueParam_Volume);

    let state = State::get(&mut env.framework_state);
    let host_object = match state.audio_queues.get_mut(&in_aq) {
        Some(obj) => obj,
        None => return 0,
    };
    env.mem.write(out_value, host_object.volume);
    0
}

pub fn AudioQueueSetParameter(
    env: &mut Environment,
    in_aq: AudioQueueRef,
    in_param_id: AudioQueueParameterID,
    in_value: AudioQueueParameterValue,
) -> OSStatus {
    return_if_null!(in_aq);
    assert!(in_param_id == kAudioQueueParam_Volume);

    let state = State::get(&mut env.framework_state);
    let host_object = match state.audio_queues.get_mut(&in_aq) {
        Some(obj) => obj,
        None => return 0,
    };

    host_object.volume = in_value;

    if let Some(al_source) = host_object.al_source {
        let context = env.framework_state.audio_toolbox
            .make_al_context_current(&mut env.openal_manager);
        let in_value = in_value.clamp(0.0, 1.0);
        unsafe {
            context.Sourcef(al_source, al::AL_MAX_GAIN, in_value);
        }
    }
    0
}

fn AudioQueueAllocateBufferWithPacketDescriptions(
    env: &mut Environment,
    in_aq: AudioQueueRef,
    in_buffer_byte_size: GuestUSize,
    _in_number_packet_desc: GuestUSize,
    out_buffer: MutPtr<AudioQueueBufferRef>,
) -> OSStatus {
    AudioQueueAllocateBuffer(env, in_aq, in_buffer_byte_size, out_buffer)
}

pub fn AudioQueueAllocateBuffer(
    env: &mut Environment,
    in_aq: AudioQueueRef,
    in_buffer_byte_size: GuestUSize,
    out_buffer: MutPtr<AudioQueueBufferRef>,
) -> OSStatus {
    return_if_null!(in_aq);

    let mut final_size = in_buffer_byte_size;
    if final_size > 16 * 1024 * 1024 {
        log!("Warning: AudioQueueAllocateBuffer requested {:#x}. Capping.", 
             final_size);
        final_size = 64 * 1024;
    }

    let host_object = match State::get(&mut env.framework_state)
        .audio_queues
        .get_mut(&in_aq) {
            Some(obj) => obj,
            None => return 0,
        };

    let packet_description_capacity = if env.bundle.bundle_identifier()
        .starts_with("com.ea.candcra") {
        1024
    } else {
        0
    };

    let audio_data = env.mem.alloc(final_size);
    let buffer_ptr = env.mem.alloc_and_write(AudioQueueBuffer {
        audio_data_bytes_capacity: final_size,
        audio_data,
        audio_data_byte_size: 0,
        user_data: Ptr::null(),
        packet_description_capacity,
        _packet_descriptions: Ptr::null(),
        _packet_description_count: 0,
    });

    host_object.buffers.push(buffer_ptr);
    env.mem.write(out_buffer, buffer_ptr);

    0
}

pub fn AudioQueueEnqueueBuffer(
    env: &mut Environment,
    in_aq: AudioQueueRef,
    in_buffer: AudioQueueBufferRef,
    _in_num_packet_descs: u32,
    _in_packet_descs: MutVoidPtr,
) -> OSStatus {
    return_if_null!(in_aq);

    let host_object = match State::get(&mut env.framework_state).audio_queues
        .get_mut(&in_aq) {
            Some(obj) => obj,
            None => return 0,
        };

    if !host_object.buffers.contains(&in_buffer) {
        return kAudioQueueErr_InvalidBuffer;
    }

    host_object.buffer_queue.push_back(in_buffer);
    0
}

fn AudioQueueEnqueueBufferWithParameters(
    env: &mut Environment,
    in_aq: AudioQueueRef,
    in_buffer: AudioQueueBufferRef,
    in_num_packet_descs: u32,
    in_packet_descs: MutVoidPtr,
) -> OSStatus {
    AudioQueueEnqueueBuffer(env, in_aq, in_buffer, in_num_packet_descs, in_packet_descs)
}

fn AudioQueueAddPropertyListener(
    env: &mut Environment,
    in_aq: AudioQueueRef,
    in_id: AudioQueuePropertyID,
    in_proc: AudioQueuePropertyListenerProc,
    in_user_data: MutVoidPtr,
) -> OSStatus {
    return_if_null!(in_aq);

    if in_id == kAudioQueueProperty_IsRunning {
        let host_object = match State::get(&mut env.framework_state).audio_queues
            .get_mut(&in_aq) {
                Some(obj) => obj,
                None => return 0,
            };

        host_object.aq_is_running_proc = Some(in_proc);
        host_object.aq_is_running_user_data = Some(in_user_data);
    }
    0
}

fn AudioQueueRemovePropertyListener(
    env: &mut Environment,
    in_aq: AudioQueueRef,
    in_id: AudioQueuePropertyID,
    _in_proc: AudioQueuePropertyListenerProc,
    _in_user_data: MutVoidPtr,
) -> OSStatus {
    return_if_null!(in_aq);

    if in_id == kAudioQueueProperty_IsRunning {
        let host_object = match State::get(&mut env.framework_state).audio_queues
            .get_mut(&in_aq) {
                Some(obj) => obj,
                None => return 0,
            };

        host_object.aq_is_running_proc = None;
        host_object.aq_is_running_user_data = None;
    }
    0
}

fn property_size(property_id: AudioQueuePropertyID) -> Option<GuestUSize> {
    match property_id {
        kAudioQueueProperty_IsRunning => Some(guest_size_of::<u32>()),
        kAudioQueueProperty_MagicCookie => Some(0),
        kAudioQueueProperty_StreamDescription => Some(guest_size_of::<AudioStreamBasicDescription>()),
        _ => None,
    }
}

fn AudioQueueGetPropertySize(
    env: &mut Environment,
    in_aq: AudioQueueRef,
    in_property_id: AudioQueuePropertyID,
    out_data_size: MutPtr<u32>,
) -> OSStatus {
    return_if_null!(in_aq);
    match property_size(in_property_id) {
        Some(size) => {
            env.mem.write(out_data_size, size);
            0
        }
        None => {
            log!("Warning: AudioQueueGetPropertySize unknown property {}, returning fake size 4", debug_fourcc(in_property_id));
            env.mem.write(out_data_size, 4); // Имитируем размер
            0 // Возвращаем 0 (Успех) вместо ошибки
        }
    }
}

fn AudioQueueGetProperty(
    env: &mut Environment,
    in_aq: AudioQueueRef,
    in_property_id: AudioQueuePropertyID,
    out_property_data: MutVoidPtr,
    io_data_size: MutPtr<u32>,
) -> OSStatus {
    return_if_null!(in_aq);
    let required_size = match property_size(in_property_id) {
        Some(size) => size,
        None => {
            log!("Warning: AudioQueueGetProperty unknown property {}, pretending success", debug_fourcc(in_property_id));
            let provided_size = env.mem.read(io_data_size);
            if provided_size >= 4 && !out_property_data.is_null() {
                env.mem.write(out_property_data.cast::<u32>(), 0u32);
            }
            return 0; // Возвращаем 0 (Успех) вместо kAudioQueueErr_InvalidProperty
        }
    };

    let provided_size = env.mem.read(io_data_size);
    if required_size != 0 && provided_size < required_size {
        return kAudioQueueErr_InvalidPropertySize;
    }

    let host_object = match State::get(&mut env.framework_state).audio_queues
        .get_mut(&in_aq) {
            Some(obj) => obj,
            None => return 0,
        };

    match in_property_id {
        kAudioQueueProperty_IsRunning => {
            let is_running: u32 = match host_object.is_running {
                AudioQueueIsRunning::Running |
                AudioQueueIsRunning::Stopping => 1,
                AudioQueueIsRunning::Stopped => 0,
            };
            env.mem.write(out_property_data.cast(), is_running);
        }
        kAudioQueueProperty_MagicCookie => { log_dbg!("MagicCookie stubbed."); }
        kAudioQueueProperty_StreamDescription => {
            env.mem.write(out_property_data.cast(), host_object.format);
        }
        _ => unreachable!(),
    }
    0
}

fn AudioQueueSetProperty(
    _env: &mut Environment,
    in_aq: AudioQueueRef,
    in_property_id: AudioQueuePropertyID,
    _in_property_data: ConstVoidPtr,
    _in_data_size: u32,
) -> OSStatus {
    return_if_null!(in_aq);

    if in_property_id == kAudioQueueProperty_MagicCookie {
        log_dbg!("AudioQueueSetProperty: Ignoring Magic Cookie for {:?}", in_aq);
        return 0; 
    }
    0
}

pub fn log_if_broken_audio_format(format: &AudioStreamBasicDescription) {
    let bytes_per_channel = format.bits_per_channel / 8;

    let expected_bytes_per_packet = format.bytes_per_frame * format.frames_per_packet;
    let expected_bytes_per_frame = format.channels_per_frame * bytes_per_channel;
    if format.bytes_per_packet < expected_bytes_per_packet ||
        format.bytes_per_frame < expected_bytes_per_frame {
        log!("Warning: Stream format has non-sensical values: {:?}", format);
    }
}

pub fn is_supported_audio_format(format: &AudioStreamBasicDescription) -> bool {
    let &AudioStreamBasicDescription { 
        format_id, format_flags, channels_per_frame, 
        bits_per_channel, bytes_per_frame, .. 
    } = format;

    match format_id {
        kAudioFormatAppleIMA4 => (channels_per_frame == 1) ||
            (channels_per_frame == 2),
        kAudioFormatLinearPCM => {
            (channels_per_frame == 1 || channels_per_frame == 2)
                && (bits_per_channel == 8 || bits_per_channel == 16 || bits_per_channel == 32)
                && ((format_flags & kAudioFormatFlagIsPacked) != 0 || 
                    ((bits_per_channel / 8) * channels_per_frame) == bytes_per_frame)
                && (format_flags & kAudioFormatFlagIsBigEndian) == 0
                && (format_flags & kAudioFormatFlagIsFloat) == 0
        }
        _ => false,
    }
}

pub fn decode_buffer(
    mem: &Mem,
    format: &AudioStreamBasicDescription,
    audio_data: MutPtr<u8>,
    audio_data_byte_size: GuestUSize,
) -> (ALenum, ALsizei, Vec<u8>) {
   
    let data_slice = mem.bytes_at(audio_data, audio_data_byte_size);

    assert!(is_supported_audio_format(format));

    match format.format_id {
        kAudioFormatAppleIMA4 => {
            assert!(data_slice.len().is_multiple_of(34));

            let mut out_pcm = Vec::<u8>::with_capacity((data_slice.len() / 34) * 64 * 2);
            let packets = data_slice.chunks(34);

            if format.channels_per_frame == 1 {
                for packet in packets {
                    let packet_arr: &[u8; 34] = match packet.try_into() {
                        Ok(p) => p,
                        Err(_) => continue,
                    };
                    let pcm_packet: [i16; 64] = decode_ima4(packet_arr);
                    let pcm_bytes: &[u8] = unsafe { 
                        std::slice::from_raw_parts(pcm_packet.as_ptr() as *const u8, 128) 
                    };

                    out_pcm.extend_from_slice(pcm_bytes);
                }
                (al::AL_FORMAT_MONO16, format.sample_rate as ALsizei, out_pcm)
            } else {
                let mut peekable_packets = packets.peekable();

                while peekable_packets.peek().is_some() {
                    let left = peekable_packets.next().unwrap();
                    let left_arr: &[u8; 34] = match left.try_into() {
                        Ok(p) => p,
                        Err(_) => continue,
                    };
                    let left_pcm: [i16; 64] = decode_ima4(left_arr);
                    
                    let right = peekable_packets.next().unwrap_or(left);
                    let right_arr: &[u8; 34] = match right.try_into() {
                        Ok(p) => p,
                        Err(_) => continue,
                    };
                    let right_pcm: [i16; 64] = decode_ima4(right_arr);

                    for (l, r) in left_pcm.iter().zip(right_pcm.iter()) {
                        out_pcm.extend_from_slice(&l.to_le_bytes());

                        out_pcm.extend_from_slice(&r.to_le_bytes());
                    }
                }
                (al::AL_FORMAT_STEREO16, format.sample_rate as ALsizei, out_pcm)
            }
        }
        kAudioFormatLinearPCM => {
            let misaligned = data_slice.len() % (format.bytes_per_frame as usize);

            let data_slice = if misaligned != 0 { 
                &data_slice[..data_slice.len() - misaligned] 
            } else { data_slice };

            let bytes_per_channel = format.bits_per_channel / 8;
            let actual_bytes_per_frame = format.channels_per_frame * bytes_per_channel;
            let actual_channels_per_frame = format.bytes_per_frame / bytes_per_channel;

            let processed_data: Vec<u8> = if actual_bytes_per_frame == format.bytes_per_frame {
                data_slice.to_owned()
            } else {
                let actual_frame_count = data_slice.len() / actual_bytes_per_frame as usize;

                let mut processed = Vec::<u8>::with_capacity(
                    format.bytes_per_frame as usize * actual_frame_count);

                for frame in data_slice.chunks(actual_bytes_per_frame as usize) {
                    let frame_bytes = &frame[frame.len() - format.bytes_per_frame as usize..];

                    match format.bytes_per_frame {
                        1 => processed.extend(&u8::from_be_bytes(
                            frame_bytes.try_into().unwrap_or([0; 1])).to_le_bytes()),
                        2 => processed.extend_from_slice(&u16::from_be_bytes(
                            frame_bytes.try_into().unwrap_or([0; 2])).to_le_bytes()),
                        4 => processed.extend_from_slice(&u32::from_be_bytes(
                            frame_bytes.try_into().unwrap_or([0; 4])).to_le_bytes()),
                        _ => unimplemented!(),
                    };

                }
                processed
            };

            let f = match (actual_channels_per_frame, format.bits_per_channel) {
                (1, 8) => al::AL_FORMAT_MONO8,
                (1, 16) => al::AL_FORMAT_MONO16,
                (2, 8) => al::AL_FORMAT_STEREO8,
                (2, 16) => al::AL_FORMAT_STEREO16,
                (2, 32) 
                => {
                    let mut new_data = Vec::<u8>::with_capacity(
                        (processed_data.len() / 4) * 2);

                    for chunk in processed_data.chunks(4) {
                        if chunk.len() < 4 { continue; }
                        let val: i32 = i32::from_le_bytes(chunk.try_into().unwrap());
                        new_data.extend(((val >> 16) as i16).to_le_bytes());
                    }
                    return (al::AL_FORMAT_STEREO16, format.sample_rate as ALsizei, new_data);

                }
                _ => unreachable!(),
            };

            (f, format.sample_rate as ALsizei, processed_data)
        }
        _ => unreachable!(),
    }
}

fn prime_audio_queue(env: &mut Environment, in_aq: AudioQueueRef) {
    let (state, context) = State::get_with_context(
        &mut env.framework_state, &mut env.openal_manager);

    let host_object = match state.audio_queues.get_mut(&in_aq) {
        Some(obj) => obj,
        None => return,
    };
    
    if host_object.is_input {
        return; 
    }
    
    if !is_supported_audio_format(&host_object.format) { return; }

    if host_object.al_source.is_none() {
        let volume = host_object.volume.clamp(0.0, 1.0);

        let mut al_source = 0;
        unsafe {
            context.GenSources(1, &mut al_source);
            context.Sourcef(al_source, al::AL_MAX_GAIN, volume);
        };
        host_object.al_source = Some(al_source);
    }
    let al_source = host_object.al_source.unwrap();

    loop {
        let (mut al_queued, mut al_processed) = (0, 0);

        unsafe {
            context.GetSourcei(al_source, al::AL_BUFFERS_QUEUED, &mut al_queued);
            context.GetSourcei(al_source, al::AL_BUFFERS_PROCESSED, &mut al_processed);
        }
        if (al_queued - al_processed) > 1 ||
            al_queued as usize == host_object.buffer_queue.len() { break; }

        let next_buffer_ref = host_object.buffer_queue[al_queued as usize];

        let next_buffer = env.mem.read(next_buffer_ref);
        let next_al_buffer = host_object.al_unused_buffers.pop().unwrap_or_else(|| {
            let mut b = 0; unsafe { context.GenBuffers(1, &mut b) }; b
        });

        let (al_format, al_freq, data) = decode_buffer(
            &env.mem, &host_object.format, 
            next_buffer.audio_data.cast(), next_buffer.audio_data_byte_size);

        unsafe {
            context.BufferData(next_al_buffer, al_format, 
                               data.as_ptr() as *const ALvoid, 
                               data.len() as ALsizei, al_freq);

            context.SourceQueueBuffers(al_source, 1, &next_al_buffer);
        }
    }
}

fn unqueue_buffers<F: FnMut(ALuint)>(al_source: ALuint, context: &OpenAL<'_>, mut callback: F) {
    loop {
        let mut processed = 0;

        unsafe { context.GetSourcei(al_source, al::AL_BUFFERS_PROCESSED, &mut processed); }
        if processed == 0 { break; }
        
        let mut b = 0;
        unsafe { context.SourceUnqueueBuffers(al_source, 1, &mut b); }
        callback(b);
    }
}

pub fn handle_audio_queue(env: &mut Environment, in_aq: AudioQueueRef) {
    let mut to_reuse = Vec::new();
    let mut callback_info: Option<(AudioQueueOutputCallback, MutVoidPtr)> = None;
    let mut al_source_id: Option<ALuint> = None;
    let mut is_input_queue = false;

    // Шаг 1: Работаем с состоянием
    {
        let (state, context) = State::get_with_context(
            &mut env.framework_state, &mut env.openal_manager);

        let host_object = match state.audio_queues.get_mut(&in_aq) {
            Some(obj) => obj,
            None => return,
        };
        is_input_queue = host_object.is_input;
        
        if is_input_queue {
            // Очередь ввода (Микрофон)
            if host_object.is_running == AudioQueueIsRunning::Running {
                // Искусственная задержка: не отправляем звук мгновенно каждый тик, чтобы не убить игру
                host_object.input_delay += 1;
                if host_object.input_delay >= 3 {
                    host_object.input_delay = 0;
                    if !host_object.is_running_handler {
                        host_object.is_running_handler = true;
                        
                        if let Some(buf) = host_object.buffer_queue.pop_front() {
                            to_reuse.push(buf);
                        }
                        
                        callback_info = Some((host_object.callback_proc, host_object.callback_user_data));
                    }
                }
            }
        } else {
            // Очередь вывода (Динамик)
            if let Some(al_source) = host_object.al_source {
                if !host_object.is_running_handler {
                    host_object.is_running_handler = true;
                    al_source_id = Some(al_source);

                    unqueue_buffers(al_source, &context, |b| {
                        host_object.al_unused_buffers.push(b);
                        if let Some(buf) = host_object.buffer_queue.pop_front() {
                            to_reuse.push(buf);
                        }
                    });

                    callback_info = Some((host_object.callback_proc, host_object.callback_user_data));
                }
            }
        }
    }

    // Шаг 2: Вызовы колбэков
    if let Some((callback, user_data)) = callback_info {
        for buf in to_reuse {
            if is_input_queue {
                // Читаем структуру буфера
                let mut buffer_struct = env.mem.read(buf);
                buffer_struct.audio_data_byte_size = buffer_struct.audio_data_bytes_capacity;

                // ФЕЙКОВЫЙ ШУМ: пишем немного единичек вместо абсолютного нуля
                // Делаем это ДО сохранения структуры, пока она принадлежит нам
                if buffer_struct.audio_data_bytes_capacity >= 4 {
                    env.mem.write(buffer_struct.audio_data.cast::<u32>(), 0x01010101u32);
                }

                // Безопасно отдаем обновленную структуру обратно в память
                env.mem.write(buf, buffer_struct);

                // Вызываем коллбэк ввода (сигнатура на 6 аргументов)
                <GuestFunction as CallFromHost<(), (MutVoidPtr, AudioQueueRef, AudioQueueBufferRef, ConstVoidPtr, u32, ConstVoidPtr)>>::
                call_from_host(&callback, env, (user_data, in_aq, buf, ConstVoidPtr::null(), 0, ConstVoidPtr::null()));
            } else {
                // Вызываем обычный коллбэк вывода (сигнатура на 3 аргумента)
                let _: () = callback.call_from_host(env, (user_data, in_aq, buf));
            }
        }
    }

    prime_audio_queue(env, in_aq);

    // Шаг 3: Финальные операции OpenAL (только для вывода)
    if let Some(al_source) = al_source_id {
        let mut needs_play = false;
        let mut is_stopping = false;
        let mut al_state = 0;

        {
            let (state_ref, context) = State::get_with_context(
                &mut env.framework_state, &mut env.openal_manager);

            if let Some(host_object) = state_ref.audio_queues.get_mut(&in_aq) {
                unsafe { context.GetSourcei(al_source, al::AL_SOURCE_STATE, &mut al_state); }
                
                if host_object.is_running != AudioQueueIsRunning::Stopped && al_state == al::AL_STOPPED {
                    needs_play = true;
                }
                if host_object.is_running == AudioQueueIsRunning::Stopping {
                    is_stopping = true;
                }
            }
        }

        if needs_play || (is_stopping && al_state == al::AL_STOPPED) {
            let context = env.framework_state.audio_toolbox.make_al_context_current(&mut env.openal_manager);

            if needs_play { unsafe { context.SourcePlay(al_source); } }
            if is_stopping && al_state == al::AL_STOPPED {
                finish_stopping_audio_queue(env, in_aq);
            }
        }
    }

    // Сбрасываем флаг обработчика
    if let Some(host_object) = State::get(&mut env.framework_state).audio_queues.get_mut(&in_aq) {
        host_object.is_running_handler = false;
    }
}

fn AudioQueuePrime(env: &mut Environment, in_aq: AudioQueueRef, _: u32, _: MutPtr<u32>) -> OSStatus {
    return_if_null!(in_aq);
    prime_audio_queue(env, in_aq);
    0
}

fn notify_aq_is_running(env: &mut Environment, in_aq: AudioQueueRef) {
    let host_object = match State::get(&mut env.framework_state).audio_queues
        .get_mut(&in_aq) {
            Some(obj) => obj,
            None => return,
        };

    if let (Some(proc), Some(data)) = (host_object.aq_is_running_proc, 
                                       host_object.aq_is_running_user_data) {
        <GuestFunction as CallFromHost<(), (MutVoidPtr, Ptr<OpaqueAudioQueue, true>, u32)>>::
        call_from_host(&proc, env, (data, in_aq, kAudioQueueProperty_IsRunning));
    }
}

pub fn AudioQueueStart(env: &mut Environment, in_aq: AudioQueueRef, _: ConstVoidPtr) -> OSStatus {
    return_if_null!(in_aq);
    prime_audio_queue(env, in_aq);

    let (state, context) = State::get_with_context(
        &mut env.framework_state, &mut env.openal_manager);
    let host_object = match state.audio_queues.get_mut(&in_aq) {
        Some(obj) => obj,
        None => return 0,
    };

    host_object.is_running = AudioQueueIsRunning::Running;

    if let Some(src) = host_object.al_source { unsafe { context.SourcePlay(src); } }
    notify_aq_is_running(env, in_aq);

    0
}

pub fn AudioQueuePause(env: &mut Environment, in_aq: AudioQueueRef) -> OSStatus {
    return_if_null!(in_aq);

    let (state, context) = State::get_with_context(
        &mut env.framework_state, &mut env.openal_manager);
    let host_object = match state.audio_queues.get_mut(&in_aq) {
        Some(obj) => obj,
        None => return 0,
    };

    host_object.is_running = AudioQueueIsRunning::Stopped;
    if let Some(src) = host_object.al_source { unsafe { context.SourcePause(src); } }
    0
}

fn finish_stopping_audio_queue(env: &mut Environment, in_aq: AudioQueueRef) {
    AudioQueueReset(env, in_aq);

    if let Some(obj) = State::get(&mut env.framework_state).audio_queues.get_mut(&in_aq) {
        obj.is_running = AudioQueueIsRunning::Stopped;
    }
    notify_aq_is_running(env, in_aq);
}

pub fn AudioQueueStop(env: &mut Environment, in_aq: AudioQueueRef, immediate: bool) -> OSStatus {
    return_if_null!(in_aq);

    if immediate {
        let (state, context) = State::get_with_context(
            &mut env.framework_state, &mut env.openal_manager);

        let host_object = match state.audio_queues.get_mut(&in_aq) {
            Some(obj) => obj,
            None => return 0,
        };
        if let Some(src) = host_object.al_source { unsafe { context.SourceStop(src); } }
        finish_stopping_audio_queue(env, in_aq);

    } else {
        let state = State::get(&mut env.framework_state);
        if let Some(obj) = state.audio_queues.get_mut(&in_aq) {
            obj.is_running = AudioQueueIsRunning::Stopping;
        }
    }
    0
}

fn AudioQueueReset(env: &mut Environment, in_aq: AudioQueueRef) -> OSStatus {
    return_if_null!(in_aq);

    let (state, context) = State::get_with_context(
        &mut env.framework_state, &mut env.openal_manager);
    let host_object = match state.audio_queues.get_mut(&in_aq) {
        Some(obj) => obj,
        None => return 0,
    };

    if let Some(src) = host_object.al_source {
        unsafe { context.SourceStop(src); }
        unqueue_buffers(src, &context, |b| {
            host_object.al_unused_buffers.push(b);
            host_object.buffer_queue.pop_front();
        });
    }
    host_object.buffer_queue.clear();
    0
}

fn AudioQueueFlush(_env: &mut Environment, _in_aq: AudioQueueRef) -> OSStatus { 0 }

fn AudioQueueFreeBuffer(env: &mut Environment, in_aq: AudioQueueRef, 
                        in_buffer: AudioQueueBufferRef) -> OSStatus {
    return_if_null!(in_aq);

    let host_object = match State::get(&mut env.framework_state).audio_queues
        .get_mut(&in_aq) {
            Some(obj) => obj,
            None => return 0,
        };
    if host_object.buffer_queue.contains(&in_buffer) { return kAudioQueueErr_BufferInQueue; }

    if let Some(idx) = host_object.buffers.iter().position(|x| x == &in_buffer) {
        host_object.buffers.remove(idx);

        let buffer = env.mem.read(in_buffer);
        env.mem.free(buffer.audio_data);
        env.mem.free(in_buffer.cast());
        0
    } else { kAudioQueueErr_InvalidBuffer }
}

pub fn AudioQueueDispose(env: &mut Environment, in_aq: AudioQueueRef, _: bool) -> OSStatus {
    return_if_null!(in_aq);

    let (state, context) = State::get_with_context(
        &mut env.framework_state, &mut env.openal_manager);

    let mut host_object = match state.audio_queues.remove(&in_aq) {
        Some(obj) => obj,
        None => return 0,
    };
    env.mem.free(in_aq.cast());

    for b_ptr in host_object.buffers {
        let b = env.mem.read(b_ptr);
        env.mem.free(b.audio_data);
        env.mem.free(b_ptr.cast());
    }

    if let Some(src) = host_object.al_source {
        unsafe { context.SourceStop(src); }
        unqueue_buffers(src, &context, |b| host_object.al_unused_buffers.push(b));
        unsafe { context.DeleteBuffers(host_object.al_unused_buffers.len() as i32, 
                                       host_object.al_unused_buffers.as_ptr()); }
    }
    ns_run_loop::remove_audio_queue(env, host_object.run_loop, in_aq);

    0
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(AudioQueueNewOutput(_, _, _, _, _, _, _)),
    export_c_func!(AudioQueueNewInput(_, _, _, _, _, _, _)),
    export_c_func!(AudioQueueGetParameter(_, _, _)),
    export_c_func!(AudioQueueSetParameter(_, _, _)),
    export_c_func!(AudioQueueAllocateBufferWithPacketDescriptions(_, _, _, _)),
    export_c_func!(AudioQueueAllocateBuffer(_, _, _)),
    export_c_func!(AudioQueueEnqueueBuffer(_, _, _, _)),
    export_c_func!(AudioQueueEnqueueBufferWithParameters(_, _, _, _)),
    export_c_func!(AudioQueueAddPropertyListener(_, _, _, _)),
    export_c_func!(AudioQueueRemovePropertyListener(_, _, _, _)),
    export_c_func!(AudioQueueGetPropertySize(_, _, _)),
    export_c_func!(AudioQueueGetProperty(_, _, _, _)),
    export_c_func!(AudioQueueSetProperty(_, _, _, _)),
  
    export_c_func!(AudioQueuePrime(_, _, _)),
    export_c_func!(AudioQueueStart(_, _)),
 
    export_c_func!(AudioQueuePause(_)),
    export_c_func!(AudioQueueStop(_, _)),
    export_c_func!(AudioQueueReset(_)),
    export_c_func!(AudioQueueFlush(_)),
    export_c_func!(AudioQueueFreeBuffer(_, _)),
    export_c_func!(AudioQueueDispose(_, _)),
];

