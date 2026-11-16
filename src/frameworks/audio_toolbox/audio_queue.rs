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
use crate::audio::encode_ima4;
use crate::audio::mic;
use crate::frameworks::audio_toolbox::audio_converter::{
    convert_pcm, pcm_shape_from_asbd, AudioStreamPacketDescription, PcmShape,
};
use crate::frameworks::audio_toolbox::audio_unit::alloc_render_timestamp;
use crate::audio::openal as al;
use crate::audio::parse_mp3_frame_header;
use crate::audio::symphonia_formats::FrameDecoder;
use crate::audio::openal::al_types::*;
use crate::audio::openal::{OpenAL, OpenALManager};
use crate::dyld::{export_c_func, FunctionExports};
use crate::frameworks::carbon_core::OSStatus;
use crate::frameworks::core_audio_types::{
    debug_fourcc, fourcc, kAudioFormatAppleIMA4, kAudioFormatFlagIsBigEndian,
    kAudioFormatFlagIsFloat, kAudioFormatFlagIsPacked, kAudioFormatLinearPCM, kAudioFormatMPEG4AAC,
    kAudioFormatMPEGLayer3, AudioStreamBasicDescription,
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
    /// Stereo pan, -1.0 (full left) .. 1.0 (full right), 0.0 (centered).
    pan: f32,
    /// Playback rate (kAudioQueueParam_PlayRate). 1.0 = normal speed.
    play_rate: f32,
    /// Pitch offset in cents (kAudioQueueParam_Pitch), -1200..1200.
    pitch: f32,
    /// Stored kAudioQueueParam_VolumeRampTime (seconds).
    volume_ramp_time: f32,
    /// Stored kAudioQueueProperty_EnableLevelMetering value.
    level_metering_enabled: bool,
    buffers: Vec<AudioQueueBufferRef>,
    /// There is also a queue of OpenAL buffers, which must be kept in sync:
    /// the nth item in this queue must also be the nth item in the OpenAL
    /// queue, though the OpenAL queue may be shorter.
    buffer_queue: VecDeque<AudioQueueBufferRef>,
    /// Per-buffer packet boundaries `(start_offset, byte_size)` captured from
    /// `AudioQueueEnqueueBuffer` for compressed formats. Compressed AAC/MP3
    /// access units carry no length prefix, and raw AAC (as opposed to ADTS)
    /// has no self-synchronising header, so the caller-supplied packet
    /// descriptions are the only way to split a buffer back into frames.
    /// Keyed by the guest buffer pointer and overwritten on each enqueue.
    buffer_packet_descs: HashMap<AudioQueueBufferRef, Vec<(u32, u32)>>,
    is_running: AudioQueueIsRunning,
    al_source: Option<ALuint>,
    al_unused_buffers: Vec<ALuint>,
    aq_is_running_proc: Option<AudioQueuePropertyListenerProc>,
    aq_is_running_user_data: Option<MutVoidPtr>,
    /// Listeners registered via `AudioQueueAddPropertyListener` for
    /// properties other than `kAudioQueueProperty_IsRunning` (which uses
    /// dedicated storage above, matching historical touchHLE behaviour).
    property_listeners: Vec<(
        AudioQueuePropertyID,
        AudioQueuePropertyListenerProc,
        MutVoidPtr,
    )>,
    is_running_handler: bool,
    is_input: bool,
    /// Whether this input queue currently holds a reference on the shared
    /// host capture stream ([mic::start]/[mic::stop]). Pause keeps it; stop
    /// and dispose release it, like iOS powers the input hardware down.
    input_capture_ref: bool,
    input_delay: u32,
    /// For input queues: read position into the host microphone stream (see
    /// `crate::audio::mic`), in host samples since the capture was last
    /// (re)started. Fractional when the queue's sample rate differs from the
    /// host rate so that no drift accumulates.
    input_host_cursor: f64,
    /// Stored `kAudioQueueProperty_HardwareCodecPolicy` value. Defaults to
    /// `kAudioQueueHardwareCodecPolicy_Default` (0). touchHLE has no hardware
    /// codecs, so this is informational only.
    hardware_codec_policy: u32,
    /// PCM format set via `AudioQueueSetOfflineRenderFormat`. `None` means the
    /// queue is in real-time mode (default). `Some(format)` means the queue is
    /// in offline-rendering mode and the caller is expected to drive playback
    /// via `AudioQueueOfflineRender`.
    offline_render_format: Option<AudioStreamBasicDescription>,
    /// Offline mode: decoded audio that `AudioQueueOfflineRender` hasn't
    /// handed out yet, interleaved 16-bit samples at `offline_pcm_channels`.
    offline_pcm: VecDeque<i16>,
    offline_pcm_channels: u32,
    /// For MP3 and AAC queues: the decoder, which has to carry over from one
    /// buffer to the next. Created on first use.
    frame_decoder: Option<FrameDecoder>,
    /// OpenAL format and frequency of the buffers queued on `al_source`.
    /// OpenAL refuses to queue a buffer with a different format behind them.
    al_queued_format: Option<(ALenum, ALsizei)>,
}

/// Track whether the audio queue is meant to be running, in order to handle
/// OpenAL stop events caused by running out of data:
/// - If it's running, the OpenAL source can be restarted.
/// - If it's stopping asynchronously, the audio queue stop can be completed.
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
    /// Should be a `MutPtr<AudioStreamPacketDescription>`, but that's not
    /// implemented yet.
    _packet_descriptions: MutVoidPtr,
    _packet_description_count: u32,
}
unsafe impl SafeRead for AudioQueueBuffer {}

pub type AudioQueueBufferRef = MutPtr<AudioQueueBuffer>;

/// (*void)(void *in_user_data, AudioQueueRef in_aq, AudioQueueBufferRef in_buf)
pub type AudioQueueOutputCallback = GuestFunction;

type AudioQueueParameterID = u32;
pub const kAudioQueueParam_Volume: AudioQueueParameterID = 1;
// `kAudioQueueParam_PlayRate` per Apple's `AudioQueue.h`: playback rate as a
// fraction of normal speed (0.5 .. 2.0 on real hardware). Implemented with
// OpenAL's per-source `AL_PITCH`, which resamples the source (changing both
// speed and pitch) — close enough for game SFX/music speed-up effects.
pub const kAudioQueueParam_PlayRate: AudioQueueParameterID = 2;
// `kAudioQueueParam_Pitch` per Apple's `AudioQueue.h`: pitch shift in half
// steps (-24 .. 24), 0 = normal. Approximated with `AL_PITCH` as 2^(pitch/12)
// (OpenAL can't shift pitch without also changing tempo).
pub const kAudioQueueParam_Pitch: AudioQueueParameterID = 3;
// `kAudioQueueParam_VolumeRampTime` per Apple's `AudioQueue.h`: duration in
// seconds over which volume changes are ramped. Stored for get; ramps are
// applied instantly.
pub const kAudioQueueParam_VolumeRampTime: AudioQueueParameterID = 4;
// `kAudioQueueParam_Pan` per Apple's `AudioQueue.h`. Range -1.0 (full left)
// to 1.0 (full right). Mirrors `AVAudioPlayer.pan`.
pub const kAudioQueueParam_Pan: AudioQueueParameterID = 13;

type AudioQueueParameterValue = f32;

pub type AudioQueuePropertyID = u32;
pub const kAudioQueueProperty_IsRunning: AudioQueuePropertyID = fourcc(b"aqrn");
const kAudioQueueProperty_MagicCookie: AudioQueuePropertyID = fourcc(b"aqmc");
const kAudioQueueProperty_StreamDescription: AudioQueuePropertyID = fourcc(b"aqft");
const kAudioQueueProperty_MaximumOutputPacketSize: AudioQueuePropertyID = fourcc(b"aqmv");
const kAudioQueueProperty_EnableLevelMetering: AudioQueuePropertyID = fourcc(b"aqme");
/// `kAudioQueueProperty_CurrentLevelMeterDB` ('aqmd') from Apple's
/// `AudioQueue.h`: an array of [AudioQueueLevelMeterState], one element per
/// channel of the queue, with values in decibels. Backed by the host
/// microphone stream for input queues (see `crate::audio::mic`) and by the
/// OpenAL output level for output queues.
const kAudioQueueProperty_CurrentLevelMeterDB: AudioQueuePropertyID = fourcc(b"aqmd");

/// `AudioQueueLevelMeterState` from Apple's `AudioQueue.h`: one entry per
/// audio channel of the queue, holding the average and peak power in dB.
#[repr(C, packed)]
pub struct AudioQueueLevelMeterState {
    pub mAveragePower: f32,
    pub mPeakPower: f32,
}
unsafe impl SafeRead for AudioQueueLevelMeterState {}
/// `kAudioQueueDeviceProperty_SampleRate` ('devr') from Apple's
/// `AudioQueue.h`: the hardware sample rate the queue plays at. We always
/// play at the format's native rate, so this reports the queue's format
/// sample rate.
const kAudioQueueProperty_DeviceSampleRate: AudioQueuePropertyID = fourcc(b"devr");
/// `kAudioQueueDeviceProperty_NumberChannels` ('dev#') from Apple's
/// `AudioQueue.h`: the number of channels the device mixes. Reports the
/// queue format's channel count.
const kAudioQueueProperty_DeviceNumberChannels: AudioQueuePropertyID = fourcc(b"dev#");
/// `kAudioQueueProperty_HardwareCodecPolicy` from Apple's `AudioQueue.h`.
/// Controls whether the queue uses hardware or software codecs. touchHLE only
/// ships software codecs (the host has no audio hardware codec), so this is
/// stored as a hint and the codec choice never changes.
const kAudioQueueProperty_HardwareCodecPolicy: AudioQueuePropertyID = fourcc(b"aqcp");
type AudioQueuePropertyListenerProc = GuestFunction;


/// `AudioQueueHardwareCodecPolicy` from Apple's `AudioQueue.h`.
#[allow(dead_code)]
mod codec_policy {
    pub const DEFAULT: u32 = 0;
    pub const USE_SOFTWARE_ONLY: u32 = 1;
    pub const USE_HARDWARE_ONLY: u32 = 2;
    pub const PREFER_SOFTWARE: u32 = 3;
    pub const PREFER_HARDWARE: u32 = 4;
}

const kAudioQueueErr_InvalidBuffer: OSStatus = -66687;
const kAudioQueueErr_InvalidPropertySize: OSStatus = -66683;
const kAudioQueueErr_BufferInQueue: OSStatus = -66679;
const kAudioQueueErr_InvalidProperty: OSStatus = -66684;
/// `kAudioQueueErr_QueueNotStopped` from Apple's `AudioQueue.h`. Returned by
/// `AudioQueueSetOfflineRenderFormat` when the queue is currently running.
const kAudioQueueErr_QueueNotStopped: OSStatus = -66677;
/// `kAudioQueueErr_InvalidOfflineMode`: `AudioQueueOfflineRender` on a queue
/// that isn't in offline mode.
const kAudioQueueErr_InvalidOfflineMode: OSStatus = -66626;
/// Floor for `AudioQueueLevelMeterState` values (Apple clamps meters at
/// -160 dB).
const MIN_LEVEL_METER_DB: f32 = -160.0;

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
    log!("AudioQueueNewOutput(format={:?})", env.mem.read(in_format));
    // reserved: real Audio Queue Services ignores non-zero flags as a
    // forward-compatibility measure. Don't panic if a game passes garbage.
    if in_flags != 0 {
        log!(
            "Warning: AudioQueueNewOutput: ignoring unexpected non-zero flags {:#x}",
            in_flags
        );
    }

    // NULL is a synonym of kCFRunLoopCommonModes here. Anything else is
    // technically unsupported, but real iOS quietly accepts arbitrary
    // strings and just runs the callback on the requested loop.
    if !in_callback_run_loop_mode.is_null() {
        let common_modes = get_static_str(env, kCFRunLoopCommonModes);
        let is_common: bool = msg![env; in_callback_run_loop_mode isEqual:common_modes];
        if !is_common {
            log!(
                "Warning: AudioQueueNewOutput called with non-kCFRunLoopCommonModes \
                 run loop mode {:?}; treating as kCFRunLoopCommonModes.",
                in_callback_run_loop_mode
            );
        }
    }

    let in_callback_run_loop = if in_callback_run_loop.is_null() {
        CFRunLoopGetMain(env)
    } else {
        in_callback_run_loop
    };

    let mut format = env.mem.read(in_format);
    if env.bundle.bundle_identifier().starts_with("com.ea.candcra")
        && format.format_id == fourcc(b".mp3")
    {
        log!("Applying game-specific hack for C&C Red Alert: Fixing hardcoded audio format from .mp3 to PCM.");
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
        pan: 0.0,
        play_rate: 1.0,
        pitch: 0.0,
        volume_ramp_time: 0.0,
        level_metering_enabled: false,
        buffers: Vec::new(),
        buffer_queue: VecDeque::new(),
        buffer_packet_descs: HashMap::new(),
        is_running: AudioQueueIsRunning::Stopped,
        al_source: None,
        al_unused_buffers: Vec::new(),
        aq_is_running_proc: None,
        aq_is_running_user_data: None,
        property_listeners: Vec::new(),
        is_running_handler: false,
        is_input: false,
        input_capture_ref: false,
        input_delay: 0,
        input_host_cursor: 0.0,
        hardware_codec_policy: codec_policy::DEFAULT,
        offline_render_format: None,
        offline_pcm: VecDeque::new(),
        offline_pcm_channels: 0,
        frame_decoder: None,
        al_queued_format: None,
    };

    let aq_ref = env.mem.alloc_and_write(OpaqueAudioQueue { _filler: 0 });
    State::get(&mut env.framework_state)
        .audio_queues
        .insert(aq_ref, host_object);

    env.mem.write(out_aq, aq_ref);

    ns_run_loop::add_audio_queue(env, in_callback_run_loop, aq_ref);

    log_if_broken_audio_format(&format);

    if !is_supported_audio_format(&format) {
        log_dbg!("Warning: Audio queue {:?} will be ignored because its format is not yet supported: {:#?}", aq_ref, format);
    }

    log_dbg!(
        "AudioQueueNewOutput() for format {:#?}, new audio queue handle: {:?}",
        format,
        aq_ref,
    );

    0 // success
}

/// Apply the queue's pan to its OpenAL source. Pan is implemented per Apple's
/// AudioToolbox semantics: -1.0 = full left, 0.0 = centered, 1.0 = full
/// right. We place the source on the x-axis with a small forward offset so
/// that the listener (default at origin facing -z) hears it equally loud at
/// pan 0 and panned at ±1.
// Standard OpenAL enum values (al.h). They are missing from the local
// `openal_soft_wrapper::al_defines` re-export but are part of the public
// 1.1 ABI, see <https://www.openal.org/documentation/openal-1.1-specification.pdf>.
const AL_SOURCE_RELATIVE: ALenum = 0x202;
const AL_POSITION: ALenum = 0x1004;
const AL_TRUE_I32: ALint = 1;

fn apply_al_pan(context: &OpenAL<'_>, al_source: ALuint, pan: f32) {
    let pan = pan.clamp(-1.0, 1.0);
    // Keep source relative to the listener so head tracking can't drift.
    unsafe {
        context.Sourcei(al_source, AL_SOURCE_RELATIVE, AL_TRUE_I32);
        context.Source3f(
            al_source,
            AL_POSITION,
            pan,
            0.0,
            -(1.0 - pan * pan).sqrt().max(0.0),
        );
        // Panning is purely cosmetic; if the driver rejects any of these calls
        // just clear the error rather than crashing the whole emulator.
        unsafe {
            let _ = context.GetError();
        }
    }
}

pub fn AudioQueueGetParameter(
    env: &mut Environment,
    in_aq: AudioQueueRef,
    in_param_id: AudioQueueParameterID,
    out_value: MutPtr<AudioQueueParameterValue>,
) -> OSStatus {
    return_if_null!(in_aq);

    let state = State::get(&mut env.framework_state);

    let host_object = match state.audio_queues.get_mut(&in_aq) {
        Some(obj) => obj,
        None => return 0,
    };

    match in_param_id {
        kAudioQueueParam_Volume => {
            env.mem.write(out_value, host_object.volume);
            0
        }
        kAudioQueueParam_PlayRate => {
            env.mem.write(out_value, host_object.play_rate);
            0
        }
        kAudioQueueParam_Pitch => {
            env.mem.write(out_value, host_object.pitch);
            0
        }
        kAudioQueueParam_VolumeRampTime => {
            env.mem.write(out_value, host_object.volume_ramp_time);
            0
        }
        kAudioQueueParam_Pan => {
            env.mem.write(out_value, host_object.pan);
            0
        }
        // Real Audio Queue Services returns kAudioQueueErr_InvalidParameter
        // (-66670) in this case, which we approximate with
        // kAudioQueueErr_InvalidProperty.
        _ => {
            log!(
                "Warning: AudioQueueGetParameter: unsupported param id {}; \
                 returning 0.",
                in_param_id
            );
            env.mem.write(out_value, 0.0);
            kAudioQueueErr_InvalidProperty
        }
    }
}

pub fn AudioQueueSetParameter(
    env: &mut Environment,
    in_aq: AudioQueueRef,
    in_param_id: AudioQueueParameterID,
    in_value: AudioQueueParameterValue,
) -> OSStatus {
    return_if_null!(in_aq);

    let state = State::get(&mut env.framework_state);

    let host_object = match state.audio_queues.get_mut(&in_aq) {
        Some(obj) => obj,
        None => return 0,
    };

    match in_param_id {
        kAudioQueueParam_Volume => {
            host_object.volume = in_value;
            let al_source = host_object.al_source;
            log_dbg!(
                "AudioQueueSetParameter kAudioQueueParam_Volume is set to {}",
                in_value
            );
            if let Some(al_source) = al_source {
                let context = env
                    .framework_state
                    .audio_toolbox
                    .make_al_context_current(&mut env.openal_manager);
                let clamped = in_value.clamp(0.0, 1.0);
                unsafe {
                    context.Sourcef(al_source, al::AL_MAX_GAIN, clamped);
                }
            }
            0
        }
        kAudioQueueParam_PlayRate => {
            // Playback speed: clamp to the hardware range and feed OpenAL's
            // per-source pitch (resampling). Stored even when no source is
            // live yet; `AudioQueueStart`/enqueue re-applies it.
            let clamped = in_value.clamp(0.5, 2.0);
            host_object.play_rate = clamped;
            let al_source = host_object.al_source;
            log_dbg!(
                "AudioQueueSetParameter kAudioQueueParam_PlayRate is set to {}",
                clamped
            );
            if let Some(al_source) = al_source {
                let context = env
                    .framework_state
                    .audio_toolbox
                    .make_al_context_current(&mut env.openal_manager);
                unsafe {
                    context.Sourcef(al_source, al::AL_PITCH, clamped);
                }
                unsafe {
                    let _ = context.GetError();
                }
            }
            0
        }
        kAudioQueueParam_Pitch => {
            // Pitch shift in half steps (-24..24). OpenAL can't shift pitch
            // without tempo, approximate with 2^(pitch/12).
            let clamped = in_value.clamp(-24.0, 24.0);
            host_object.pitch = clamped;
            let al_source = host_object.al_source;
            log_dbg!(
                "AudioQueueSetParameter kAudioQueueParam_Pitch is set to {}",
                clamped
            );
            if let Some(al_source) = al_source {
                let context = env
                    .framework_state
                    .audio_toolbox
                    .make_al_context_current(&mut env.openal_manager);
                unsafe {
                    context.Sourcef(al_source, al::AL_PITCH, 2.0f32.powf(clamped / 12.0));
                }
                unsafe {
                    let _ = context.GetError();
                }
            }
            0
        }
        kAudioQueueParam_VolumeRampTime => {
            // Stored for Get; volume changes are applied instantly because
            // OpenAL has no ramp support.
            host_object.volume_ramp_time = in_value.max(0.0);
            0
        }
        kAudioQueueParam_Pan => {
            let clamped = in_value.clamp(-1.0, 1.0);
            host_object.pan = clamped;
            let al_source = host_object.al_source;
            log_dbg!(
                "AudioQueueSetParameter kAudioQueueParam_Pan is set to {}",
                clamped
            );
            if let Some(al_source) = al_source {
                let context = env
                    .framework_state
                    .audio_toolbox
                    .make_al_context_current(&mut env.openal_manager);
                apply_al_pan(&context, al_source, clamped);
            }
            0
        }
        _ => {
            log!(
                "Warning: AudioQueueSetParameter: unsupported param id {}; \
                 ignoring.",
                in_param_id
            );
            kAudioQueueErr_InvalidProperty
        }
    }
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

    if in_buffer_byte_size > 16 * 1024 * 1024 {
        log!(
            "Error: AudioQueueAllocateBuffer requested ridiculously large buffer: {:#x} bytes",
            in_buffer_byte_size
        );
        return -50;
    }

    let host_object = match State::get(&mut env.framework_state)
        .audio_queues
        .get_mut(&in_aq)
    {
        Some(obj) => obj,
        None => return 0,
    };

    let packet_description_capacity =
        if env.bundle.bundle_identifier().starts_with("com.ea.candcra") {
            1024
        } else {
            0
        };

    let audio_data = env.mem.alloc(in_buffer_byte_size);
    let buffer_ptr = env.mem.alloc_and_write(AudioQueueBuffer {
        audio_data_bytes_capacity: in_buffer_byte_size,
        audio_data,
        audio_data_byte_size: 0,
        user_data: Ptr::null(),
        packet_description_capacity,
        _packet_descriptions: Ptr::null(),
        _packet_description_count: 0,
    });

    host_object.buffers.push(buffer_ptr);
    env.mem.write(out_buffer, buffer_ptr);

    0 // success
}

pub fn AudioQueueEnqueueBuffer(
    env: &mut Environment,
    in_aq: AudioQueueRef,
    in_buffer: AudioQueueBufferRef,
    in_num_packet_descs: u32,
    in_packet_descs: MutVoidPtr,
) -> OSStatus {
    return_if_null!(in_aq);

    // Capture the caller-supplied packet boundaries (compressed formats only)
    // before borrowing the host object, since reading guest memory and the
    // mutable host-object borrow can't overlap.
    let packet_descs: Vec<(u32, u32)> =
        if in_num_packet_descs > 0 && !in_packet_descs.is_null() {
            let descs: ConstPtr<AudioStreamPacketDescription> = in_packet_descs.cast_const().cast();
            (0..in_num_packet_descs)
                .map(|i| {
                    let d = env.mem.read(descs + i);
                    (d.mStartOffset as u32, d.mDataByteSize)
                })
                .collect()
        } else {
            Vec::new()
        };

    let host_object = match State::get(&mut env.framework_state)
        .audio_queues
        .get_mut(&in_aq)
    {
        Some(obj) => obj,
        None => return 0,
    };

    if !host_object.buffers.contains(&in_buffer) {
        return kAudioQueueErr_InvalidBuffer;
    }

    if packet_descs.is_empty() {
        host_object.buffer_packet_descs.remove(&in_buffer);
    } else {
        host_object
            .buffer_packet_descs
            .insert(in_buffer, packet_descs);
    }

    host_object.buffer_queue.push_back(in_buffer);

    log_dbg!("New buffer enqueued: {:?}", in_buffer);

    0 // success
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

    let host_object = match State::get(&mut env.framework_state)
        .audio_queues
        .get_mut(&in_aq)
    {
        Some(obj) => obj,
        None => return 0,
    };

    if in_id == kAudioQueueProperty_IsRunning {
        host_object.aq_is_running_proc = Some(in_proc);
        host_object.aq_is_running_user_data = Some(in_user_data);
    } else {
        host_object
            .property_listeners
            .push((in_id, in_proc, in_user_data));
    }
    0 // success
}

fn AudioQueueRemovePropertyListener(
    env: &mut Environment,
    in_aq: AudioQueueRef,
    in_id: AudioQueuePropertyID,
    in_proc: AudioQueuePropertyListenerProc,
    in_user_data: MutVoidPtr,
) -> OSStatus {
    return_if_null!(in_aq);

    if in_id == kAudioQueueProperty_IsRunning {
        // The guest can hold on to AudioQueueRef values past
        // AudioQueueDispose; mirror real Audio Queue Services and
        // return an error instead of panicking on a stale ref.
        let Some(host_object) = State::get(&mut env.framework_state)
            .audio_queues
            .get_mut(&in_aq)
        else {
            log!(
                "Warning: AudioQueueRemovePropertyListener({:?}): unknown / disposed queue.",
                in_aq
            );
            return kAudioQueueErr_InvalidProperty;
        };

        host_object.aq_is_running_proc = None;
        host_object.aq_is_running_user_data = None;
    } else {
        let host_object = match State::get(&mut env.framework_state)
            .audio_queues
            .get_mut(&in_aq)
        {
            Some(obj) => obj,
            None => return kAudioQueueErr_InvalidProperty,
        };
        // Apple's API has no way to disambiguate listeners registered with
        // the same proc/user-data pair, so remove the first match, matching
        // the registration order behaviour of AudioQueueAddPropertyListener.
        if let Some(pos) = host_object
            .property_listeners
            .iter()
            .position(|&(_, proc, user_data)| proc == in_proc && user_data == in_user_data)
        {
            host_object.property_listeners.remove(pos);
        }
    }
    0 // success
}

fn property_size(property_id: AudioQueuePropertyID) -> Option<GuestUSize> {
    match property_id {
        kAudioQueueProperty_IsRunning => Some(guest_size_of::<u32>()),
        kAudioQueueProperty_MagicCookie => Some(0),
        kAudioQueueProperty_StreamDescription => {
            Some(guest_size_of::<AudioStreamBasicDescription>())
        }
        kAudioQueueProperty_MaximumOutputPacketSize => Some(guest_size_of::<u32>()),
        kAudioQueueProperty_EnableLevelMetering => Some(guest_size_of::<u32>()),
        kAudioQueueProperty_HardwareCodecPolicy => Some(guest_size_of::<u32>()),
        kAudioQueueProperty_DeviceSampleRate => Some(guest_size_of::<f64>()),
        kAudioQueueProperty_DeviceNumberChannels => Some(guest_size_of::<u32>()),
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

    // The level meter array is sized "one entry per channel", so it needs the
    // queue at hand; everything else has a fixed size.
    let size = if in_property_id == kAudioQueueProperty_CurrentLevelMeterDB {
        let Some(host_object) = State::get(&mut env.framework_state)
            .audio_queues
            .get(&in_aq)
        else {
            log!(
                "Warning: AudioQueueGetPropertySize({:?}): unknown / disposed queue.",
                in_aq
            );
            return kAudioQueueErr_InvalidProperty;
        };
        guest_size_of::<AudioQueueLevelMeterState>()
            .wrapping_mul(host_object.format.channels_per_frame.max(1) as GuestUSize)
    } else {
        match property_size(in_property_id) {
            Some(size) => size,
            None => {
                log!(
                    "TODO: AudioQueueGetPropertySize({:?}, {}): unknown property, returning error",
                    in_aq,
                    debug_fourcc(in_property_id)
                );
                return kAudioQueueErr_InvalidProperty;
            }
        }
    };

    env.mem.write(out_data_size, size as u32);

    0 // success
}

fn AudioQueueGetProperty(
    env: &mut Environment,
    in_aq: AudioQueueRef,
    in_property_id: AudioQueuePropertyID,
    out_property_data: MutVoidPtr,
    io_data_size: MutPtr<u32>,
) -> OSStatus {
    return_if_null!(in_aq);

    // The level meter array is sized "one entry per channel" and needs the
    // queue at hand; everything else has a fixed size.
    let required_size = if in_property_id == kAudioQueueProperty_CurrentLevelMeterDB {
        let Some(host_object) = State::get(&mut env.framework_state)
            .audio_queues
            .get(&in_aq)
        else {
            log!(
                "Warning: AudioQueueGetProperty({:?}): unknown / disposed queue.",
                in_aq
            );
            return kAudioQueueErr_InvalidProperty;
        };
        guest_size_of::<AudioQueueLevelMeterState>()
            .wrapping_mul(host_object.format.channels_per_frame.max(1) as GuestUSize)
    } else {
        match property_size(in_property_id) {
            Some(size) => size,
            None => {
                log!(
                    "TODO: AudioQueueGetProperty({:?}, {}): unknown property, returning error",
                    in_aq,
                    debug_fourcc(in_property_id)
                );
                return kAudioQueueErr_InvalidProperty;
            }
        }
    };
    let provided_size = env.mem.read(io_data_size);

    if required_size != 0 && provided_size < required_size {
        log!(
            "Warning: AudioQueueGetProperty() failed: provided size {} < required size {}",
            provided_size,
            required_size
        );
        return kAudioQueueErr_InvalidPropertySize;
    }

    // Don't panic on stale AudioQueueRef values: real Audio Queue
    // Services returns an error instead.
    let Some(host_object) = State::get(&mut env.framework_state)
        .audio_queues
        .get_mut(&in_aq)
    else {
        log!(
            "Warning: AudioQueueGetProperty({:?}): unknown / disposed queue.",
            in_aq
        );
        return kAudioQueueErr_InvalidProperty;
    };

    match in_property_id {
        kAudioQueueProperty_IsRunning => {
            let is_running: u32 = match host_object.is_running {
                AudioQueueIsRunning::Running => 1,
                AudioQueueIsRunning::Stopping => 1,
                AudioQueueIsRunning::Stopped => 0,
            };
            env.mem.write(out_property_data.cast(), is_running);
        }
        kAudioQueueProperty_MagicCookie => {
            log_dbg!("AudioQueueGetProperty: kAudioQueueProperty_MagicCookie requested, returning empty.");
        }
        kAudioQueueProperty_MaximumOutputPacketSize => {
            // Return the bytes_per_packet from the queue's stream format.
            // If the format is VBR (bytes_per_packet == 0), report a
            // reasonable upper bound so the caller can size its read buffer.
            let max_packet = if host_object.format.bytes_per_packet > 0 {
                host_object.format.bytes_per_packet
            } else {
                // Conservative upper bound for common compressed formats
                // (AAC, MP3, etc.) — matches what Core Audio typically
                // reports on real hardware.
                2048
            };
            env.mem.write(out_property_data.cast(), max_packet);
            log_dbg!(
                "AudioQueueGetProperty: kAudioQueueProperty_MaximumOutputPacketSize => {}",
                max_packet
            );
        }
        kAudioQueueProperty_StreamDescription => {
            env.mem.write(out_property_data.cast(), host_object.format);
        }
        kAudioQueueProperty_DeviceSampleRate => {
            env.mem.write(
                out_property_data.cast::<f64>(),
                host_object.format.sample_rate,
            );
        }
        kAudioQueueProperty_DeviceNumberChannels => {
            env.mem.write(
                out_property_data.cast::<u32>(),
                host_object.format.channels_per_frame,
            );
        }
        kAudioQueueProperty_EnableLevelMetering => {
            env.mem.write(
                out_property_data.cast::<u32>(),
                u32::from(host_object.level_metering_enabled),
            );
        }
        kAudioQueueProperty_HardwareCodecPolicy => {
            env.mem.write(
                out_property_data.cast::<u32>(),
                host_object.hardware_codec_policy,
            );
        }
        kAudioQueueProperty_CurrentLevelMeterDB => {
            // One meter per channel. Only the host microphone stream has a
            // measurable level; playback metering would require tapping the
            // OpenAL source, so output queues report silence.
            let channels = host_object.format.channels_per_frame;
            let (avg_db, peak_db) = if host_object.is_input {
                mic::levels_db()
            } else {
                (MIN_LEVEL_METER_DB, MIN_LEVEL_METER_DB)
            };
            for channel in 0..channels {
                let entry = AudioQueueLevelMeterState {
                    // Mono capture duplicated to N channels reports the same
                    // level on each channel, like a mono device feeding a
                    // multi-tap meter on real hardware.
                    mAveragePower: avg_db,
                    mPeakPower: peak_db,
                };
                let dst: MutPtr<AudioQueueLevelMeterState> = out_property_data.cast();
                env.mem.write(dst + channel, entry);
            }
        }
        _ => {
            // We only advertise known properties as readable via
            // property_size; if we somehow get here with a different ID it
            // means the size table and this match got out of sync. Don't
            // crash the host: return an InvalidProperty error code as Apple
            // does for unknown properties.
            log!(
                "Warning: AudioQueueGetProperty({:?}, {}): unsupported property id; returning kAudioQueueErr_InvalidProperty.",
                in_aq,
                debug_fourcc(in_property_id)
            );
            return kAudioQueueErr_InvalidProperty;
        }
    }

    0 // success
}

fn AudioQueueSetProperty(
    env: &mut Environment,
    in_aq: AudioQueueRef,
    in_property_id: AudioQueuePropertyID,
    in_property_data: ConstVoidPtr,
    in_data_size: u32,
) -> OSStatus {
    return_if_null!(in_aq);

    // Per Apple `AudioQueue.h`: magic-cookie writes have format-specific
    // semantics we don't model, so refuse them rather than silently
    // succeeding (returning success here previously confused some apps).
    if in_property_id == kAudioQueueProperty_MagicCookie {
        return kAudioQueueErr_InvalidProperty;
    }

    match in_property_id {
        kAudioQueueProperty_HardwareCodecPolicy => {
            let required = guest_size_of::<u32>();
            if in_data_size < required || in_property_data.is_null() {
                return kAudioQueueErr_InvalidPropertySize;
            }
            let policy = env.mem.read(in_property_data.cast::<u32>());
            // Apple defines five valid policy values; anything else is
            // rejected to match Audio Queue Services behaviour.
            let valid = matches!(
                policy,
                codec_policy::DEFAULT
                    | codec_policy::USE_SOFTWARE_ONLY
                    | codec_policy::USE_HARDWARE_ONLY
                    | codec_policy::PREFER_SOFTWARE
                    | codec_policy::PREFER_HARDWARE
            );
            if !valid {
                return kAudioQueueErr_InvalidProperty;
            }
            let Some(host_object) = State::get(&mut env.framework_state)
                .audio_queues
                .get_mut(&in_aq)
            else {
                return kAudioQueueErr_InvalidProperty;
            };
            host_object.hardware_codec_policy = policy;
            log_dbg!(
                "AudioQueueSetProperty(kAudioQueueProperty_HardwareCodecPolicy = {})",
                policy
            );
            0 // success
        }
        kAudioQueueProperty_EnableLevelMetering => {
            let required = guest_size_of::<u32>();
            if in_data_size < required || in_property_data.is_null() {
                return kAudioQueueErr_InvalidPropertySize;
            }
            let enabled = env.mem.read(in_property_data.cast::<u32>()) != 0;
            let Some(host_object) = State::get(&mut env.framework_state)
                .audio_queues
                .get_mut(&in_aq)
            else {
                return kAudioQueueErr_InvalidProperty;
            };
            host_object.level_metering_enabled = enabled;
            0 // success
        }
        _ => {
            // Per Apple `AudioQueue.h`, an unknown property ID yields
            // `kAudioQueueErr_InvalidProperty`. Log it so we can spot apps
            // that depend on an unsupported property.
            log!(
                "Warning: AudioQueueSetProperty({:?}, {}, {:?}, {}): unsupported property; \
                 returning kAudioQueueErr_InvalidProperty.",
                in_aq,
                debug_fourcc(in_property_id),
                in_property_data,
                in_data_size,
            );
            kAudioQueueErr_InvalidProperty
        }
    }
}

/// `AudioQueueSetOfflineRenderFormat` from Apple's `AudioQueue.h`.
///
/// > Sets the format for offline rendering. If `inFormat` is non-NULL, the
/// > queue switches to offline rendering mode using the given PCM format.
/// > Passing NULL leaves offline rendering mode.
///
/// Apple requires the queue to be stopped and the format (when non-NULL) to be
/// uncompressed PCM. The channel layout argument is accepted for API
/// compatibility but not modelled by touchHLE — every guest we have seen
/// passes NULL here, matching Apple's own examples.
fn AudioQueueSetOfflineRenderFormat(
    env: &mut Environment,
    in_aq: AudioQueueRef,
    in_format: ConstPtr<AudioStreamBasicDescription>,
    _in_layout: ConstVoidPtr,
) -> OSStatus {
    return_if_null!(in_aq);

    let Some(host_object) = State::get(&mut env.framework_state)
        .audio_queues
        .get_mut(&in_aq)
    else {
        return kAudioQueueErr_InvalidProperty;
    };

    // Per Apple docs, the queue must not be running when offline rendering is
    // (re)configured.
    if host_object.is_running != AudioQueueIsRunning::Stopped {
        log!(
            "AudioQueueSetOfflineRenderFormat({:?}): queue is running; \
             returning kAudioQueueErr_QueueNotStopped.",
            in_aq
        );
        return kAudioQueueErr_QueueNotStopped;
    }

    if in_format.is_null() {
        // NULL format leaves offline rendering mode.
        host_object.offline_render_format = None;
        log_dbg!(
            "AudioQueueSetOfflineRenderFormat({:?}): leaving offline mode",
            in_aq
        );
        return 0;
    }

    let format = env.mem.read(in_format);

    // Apple's offline rendering only supports uncompressed PCM destinations.
    if format.format_id != kAudioFormatLinearPCM {
        log!(
            "AudioQueueSetOfflineRenderFormat({:?}): non-PCM format {} \
             rejected (offline rendering only supports PCM).",
            in_aq,
            debug_fourcc(format.format_id)
        );
        return kAudioQueueErr_InvalidProperty;
    }
    if !is_supported_audio_format(&format) {
        log!(
            "AudioQueueSetOfflineRenderFormat({:?}): unsupported PCM format \
             {:?}.",
            in_aq,
            format
        );
        return kAudioQueueErr_InvalidProperty;
    }

    host_object.offline_render_format = Some(format);
    log_dbg!(
        "AudioQueueSetOfflineRenderFormat({:?}): entering offline mode with {:?}",
        in_aq,
        format
    );
    0 // success
}

/// `OSStatus AudioQueueOfflineRender(AudioQueueRef inAQ,
///     const AudioTimeStamp *inTimestamp, AudioQueueBufferRef ioBuffer,
///     UInt32 inNumberFrames)`
///
/// Renders up to `inNumberFrames` frames of the queue's audio into `ioBuffer`
/// in the offline render format, without playing it. Apps use this to decode
/// compressed audio: FMOD's iOS MP3 codec (and so Real Racing 3's music)
/// feeds MP3 packets in and pulls PCM out. Enqueued buffers are decoded as
/// needed, and each one is handed back to the output callback once used up
/// so that the app can refill it. Fewer frames are rendered once the app
/// stops supplying data.
fn AudioQueueOfflineRender(
    env: &mut Environment,
    in_aq: AudioQueueRef,
    _in_timestamp: ConstVoidPtr,
    io_buffer: AudioQueueBufferRef,
    in_number_frames: u32,
) -> OSStatus {
    return_if_null!(in_aq);
    let Some(host_object) = State::get(&mut env.framework_state)
        .audio_queues
        .get(&in_aq)
    else {
        return kAudioQueueErr_InvalidProperty;
    };
    let Some(out_format) = host_object.offline_render_format else {
        return kAudioQueueErr_InvalidOfflineMode;
    };
    let (in_format, callback_proc, callback_user_data) = (
        host_object.format,
        host_object.callback_proc,
        host_object.callback_user_data,
    );
    if io_buffer.is_null() {
        return kAudioQueueErr_InvalidBuffer;
    }

    let bytes_per_frame = out_format.bytes_per_frame.max(1);
    let capacity = env.mem.read(io_buffer).audio_data_bytes_capacity;
    let frames = in_number_frames.min(capacity / bytes_per_frame) as usize;

    // Decode enqueued buffers until enough audio is available.
    loop {
        let Some(host_object) = State::get(&mut env.framework_state)
            .audio_queues
            .get_mut(&in_aq)
        else {
            // The output callback disposed of the queue.
            return 0;
        };
        let channels = host_object.offline_pcm_channels.max(1) as usize;
        if host_object.offline_pcm.len() >= frames * channels {
            break;
        }
        let Some(buffer_ref) = host_object.buffer_queue.pop_front() else {
            break;
        };
        let packet_descs = host_object
            .buffer_packet_descs
            .get(&buffer_ref)
            .cloned()
            .unwrap_or_default();
        let buffer = env.mem.read(buffer_ref);
        let (al_format, sample_rate, pcm) = decode_queue_buffer(
            &mut host_object.frame_decoder,
            &env.mem,
            &in_format,
            buffer.audio_data.cast(),
            buffer.audio_data_byte_size,
            &packet_descs,
        );
        let decoded_channels = if al_format == al::AL_FORMAT_STEREO16 {
            2
        } else {
            1
        };
        if sample_rate as f64 != out_format.sample_rate {
            static LOGGED: std::sync::Once = std::sync::Once::new();
            LOGGED.call_once(|| {
                log!(
                    "AudioQueueOfflineRender: decoded audio is {} Hz but the offline \
                     render format is {} Hz; not resampling.",
                    sample_rate,
                    { out_format.sample_rate }
                )
            });
        }
        let host_object = State::get(&mut env.framework_state)
            .audio_queues
            .get_mut(&in_aq)
            .unwrap();
        if host_object.offline_pcm.is_empty() {
            host_object.offline_pcm_channels = decoded_channels;
        }
        let samples = pcm
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]));
        if decoded_channels == host_object.offline_pcm_channels {
            host_object.offline_pcm.extend(samples);
        } else {
            let samples: Vec<i16> = samples.collect();
            let mut converted = Vec::new();
            remix_i16(
                &samples,
                decoded_channels,
                host_object.offline_pcm_channels,
                &mut converted,
            );
            host_object.offline_pcm.extend(converted);
        }
        let () = callback_proc.call_from_host(env, (callback_user_data, in_aq, buffer_ref));
    }

    let Some(host_object) = State::get(&mut env.framework_state)
        .audio_queues
        .get_mut(&in_aq)
    else {
        return 0;
    };
    let channels = host_object.offline_pcm_channels.max(1);
    let available = host_object.offline_pcm.len() / channels as usize;
    let rendered = frames.min(available);
    let samples: Vec<i16> = host_object
        .offline_pcm
        .drain(..rendered * channels as usize)
        .collect();
    let mut bytes = Vec::with_capacity(rendered * bytes_per_frame as usize);
    write_offline_pcm(&samples, channels, &out_format, &mut bytes);

    let mut buffer = env.mem.read(io_buffer);
    let byte_count = bytes.len() as GuestUSize;
    if byte_count > 0 {
        env.mem
            .bytes_at_mut(buffer.audio_data.cast(), byte_count)
            .copy_from_slice(&bytes);
    }
    buffer.audio_data_byte_size = byte_count;
    env.mem.write(io_buffer, buffer);
    0 // success
}

/// Convert interleaved samples from `from` channels to `to` channels (mono
/// to stereo duplicates, stereo to mono averages, otherwise channels are
/// dropped or zero-filled), appending to `out`.
fn remix_i16(samples: &[i16], from: u32, to: u32, out: &mut Vec<i16>) {
    let (from, to) = (from.max(1) as usize, to.max(1) as usize);
    for frame in samples.chunks_exact(from) {
        if from == 2 && to == 1 {
            out.push(((frame[0] as i32 + frame[1] as i32) / 2) as i16);
        } else if from == 1 {
            out.extend(std::iter::repeat(frame[0]).take(to));
        } else {
            out.extend((0..to).map(|c| frame.get(c).copied().unwrap_or(0)));
        }
    }
}

/// Write interleaved 16-bit samples with `channels` channels in `format`
/// (linear PCM, 16 or 32-bit integer or 32-bit float), appending to `out`.
fn write_offline_pcm(
    samples: &[i16],
    channels: u32,
    format: &AudioStreamBasicDescription,
    out: &mut Vec<u8>,
) {
    let mut remixed = Vec::new();
    let samples = if channels == format.channels_per_frame {
        samples
    } else {
        remix_i16(samples, channels, format.channels_per_frame, &mut remixed);
        &remixed
    };
    let is_float = (format.format_flags & kAudioFormatFlagIsFloat) != 0;
    let big_endian = (format.format_flags & kAudioFormatFlagIsBigEndian) != 0;
    for &sample in samples {
        match (format.bits_per_channel, is_float, big_endian) {
            (32, true, false) => out.extend((sample as f32 / 32768.0).to_le_bytes()),
            (32, true, true) => out.extend((sample as f32 / 32768.0).to_be_bytes()),
            (32, false, false) => out.extend(((sample as i32) << 16).to_le_bytes()),
            (32, false, true) => out.extend(((sample as i32) << 16).to_be_bytes()),
            (_, _, true) => out.extend(sample.to_be_bytes()),
            _ => out.extend(sample.to_le_bytes()),
        }
    }
}

pub fn log_if_broken_audio_format(format: &AudioStreamBasicDescription) {
    // Compressed formats (MPEG Layer III, AAC) intentionally use
    // zero for `bytes_per_packet`, `bytes_per_frame` and
    // `bits_per_channel` because their packets are variable-size and
    // their PCM frame width is not known until decode. Skip the
    // PCM-shape sanity check for them.
    if format.format_id == kAudioFormatMPEGLayer3 || format.format_id == kAudioFormatMPEG4AAC {
        return;
    }

    // Float PCM formats (e.g. 32-bit float stereo at 32kHz used by
    // Sonic Runners / Unity games) are perfectly valid per Apple's
    // Core Audio documentation. The bytes_per_frame for a 2-channel
    // 32-bit float format is 4 (not 8) when kAudioFormatFlagIsPacked
    // is set and frames_per_packet is 1 — this is valid because the
    // format uses interleaved float samples where each sample is 4 bytes
    // but channels_per_frame * bytes_per_channel may differ from
    // bytes_per_frame in packed non-interleaved layouts.
    // See: https://developer.apple.com/documentation/coreaudiotypes/audiostreambasicdescription
    if format.format_id == kAudioFormatLinearPCM
        && (format.format_flags & kAudioFormatFlagIsFloat) != 0
    {
        // Float PCM is valid; don't warn about it.
        return;
    }

    let bytes_per_channel = format.bits_per_channel / 8;
    let expected_bytes_per_packet = format.bytes_per_frame * format.frames_per_packet;
    let expected_bytes_per_frame = format.channels_per_frame * bytes_per_channel;

    if format.bytes_per_packet < expected_bytes_per_packet
        || format.bytes_per_frame < expected_bytes_per_frame
    {
        log!(
            "Warning: Stream format has non-sensical values: {:?}",
            format
        );
    }
}

pub fn is_supported_audio_format(format: &AudioStreamBasicDescription) -> bool {
    let &AudioStreamBasicDescription {
        format_id,
        format_flags,
        channels_per_frame,
        bits_per_channel,
        bytes_per_frame,
        ..
    } = format;

    match format_id {
        kAudioFormatAppleIMA4 => (channels_per_frame == 1) || (channels_per_frame == 2),
        kAudioFormatLinearPCM => {
            (channels_per_frame == 1 || channels_per_frame == 2)
                && (bits_per_channel == 8 || bits_per_channel == 16 || bits_per_channel == 32)
                && ((format_flags & kAudioFormatFlagIsPacked) != 0
                    || ((bits_per_channel / 8) * channels_per_frame) == bytes_per_frame)
                && (format_flags & kAudioFormatFlagIsBigEndian) == 0
        }
        // MPEG-1 / MPEG-2 Layer III. Apple's documentation for
        // `kAudioFormatMPEGLayer3` (see
        // <https://developer.apple.com/documentation/coreaudiotypes/kaudioformatmpeglayer3>)
        // says the format is compressed: `bytes_per_packet`,
        // `bytes_per_frame` and `bits_per_channel` are conventionally 0
        // because each MPEG audio frame is variable-size and decompresses
        // to 1152 PCM frames (`frames_per_packet`). We support mono /
        // stereo at any sample rate; symphonia handles the actual
        // decoding in `decode_buffer`.
        kAudioFormatMPEGLayer3 => channels_per_frame == 1 || channels_per_frame == 2,
        // MPEG-4 AAC. Like MP3, the format is compressed (variable-size
        // packets, zero `bytes_per_packet`/`bytes_per_frame`/
        // `bits_per_channel`); see Apple's documentation for
        // `kAudioFormatMPEG4AAC`. Buffers are expected to contain ADTS
        // frames, which is what `AudioFile` produces for `.aac` files and
        // AAC-in-CAF; symphonia handles the decoding in `decode_buffer`.
        kAudioFormatMPEG4AAC => channels_per_frame == 1 || channels_per_frame == 2,
        _ => false,
    }
}

fn downmix_i16_to_mono(pcm: &[u8], channels: u32) -> Vec<u8> {
    let channels = channels as usize;
    let Some(frame_size) = channels.checked_mul(2) else {
        return Vec::new();
    };
    if channels < 2 {
        return pcm.to_vec();
    }
    let mut mono = Vec::with_capacity(pcm.len() / channels);
    for frame in pcm.chunks_exact(frame_size) {
        let sum: i64 = frame
            .chunks_exact(2)
            .map(|sample| i16::from_le_bytes([sample[0], sample[1]]) as i64)
            .sum();
        let sample = (sum / channels as i64).clamp(i16::MIN as i64, i16::MAX as i64) as i16;
        mono.extend_from_slice(&sample.to_le_bytes());
    }
    mono
}

/// Map a sampling rate to the MPEG-4 sampling-frequency index used in the
/// ADTS header. Falls back to 44100 (index 4) for anything unrecognised.
fn adts_freq_index(sample_rate: u32) -> u8 {
    match sample_rate {
        96000 => 0,
        88200 => 1,
        64000 => 2,
        48000 => 3,
        44100 => 4,
        32000 => 5,
        24000 => 6,
        22050 => 7,
        16000 => 8,
        12000 => 9,
        11025 => 10,
        8000 => 11,
        7350 => 12,
        _ => 4,
    }
}

/// Wrap raw AAC access units in ADTS frame headers so symphonia's ADTS
/// reader can decode them.
///
/// The Nu2 engine feeds `AudioQueueEnqueueBuffer` raw AAC access units (no
/// sync words) plus an `AudioStreamPacketDescription` array giving each unit's
/// offset and length. We prepend a 7-byte ADTS header (no CRC) to each unit.
/// If we have no packet descriptions we treat the whole buffer as one access
/// unit — better than dropping the audio entirely.
fn wrap_raw_aac_as_adts(
    data: &[u8],
    packet_descs: &[(u32, u32)],
    sample_rate: u32,
    channels: u32,
    format_flags: u32,
) -> Vec<u8> {
    // Apple encodes the MPEG-4 audio object type in mFormatFlags for AAC.
    // AAC LC is object type 2; the ADTS "profile" field is objectType - 1.
    let object_type = if (1..=4).contains(&format_flags) {
        format_flags
    } else {
        2
    };
    let profile = ((object_type - 1) & 0x3) as u8;
    let freq_idx = adts_freq_index(sample_rate);
    let chan_cfg = (channels.clamp(1, 7)) as u8;

    // Build the list of (offset, len) access units.
    let units: Vec<(usize, usize)> = if packet_descs.is_empty() {
        vec![(0, data.len())]
    } else {
        packet_descs
            .iter()
            .filter_map(|&(off, len)| {
                let off = off as usize;
                let len = len as usize;
                if len == 0 || off.checked_add(len).map_or(true, |end| end > data.len()) {
                    None
                } else {
                    Some((off, len))
                }
            })
            .collect()
    };

    let mut out = Vec::with_capacity(data.len() + units.len() * 7);
    for (off, len) in units {
        let frame_len = len + 7;
        if frame_len > 0x1FFF {
            // ADTS frame length is 13 bits; skip anything that doesn't fit.
            continue;
        }
        let frame_len = frame_len as u32;
        out.push(0xFF);
        // MPEG-4, layer 0, protection absent (no CRC).
        out.push(0xF1);
        out.push((profile << 6) | ((freq_idx & 0xF) << 2) | ((chan_cfg >> 2) & 0x1));
        out.push((((chan_cfg & 0x3) as u32) << 6) as u8 | ((frame_len >> 11) & 0x3) as u8);
        out.push(((frame_len >> 3) & 0xFF) as u8);
        out.push((((frame_len & 0x7) << 5) as u8) | 0x1F);
        out.push(0xFC);
        out.extend_from_slice(&data[off..off + len]);
    }
    out
}

/// A parsed ADTS frame header: `(header_len, frame_len, object_type,
/// sample_rate, channels)`. `frame_len` includes the header.
fn parse_adts_header(bytes: &[u8]) -> Option<(usize, usize, u8, u32, u32)> {
    const SAMPLE_RATES: [u32; 13] = [
        96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350,
    ];
    let &[b0, b1, b2, b3, b4, b5, ..] = bytes else {
        return None;
    };
    // 12-bit sync word, then MPEG version (1 bit) and layer (2 bits, always 0).
    if b0 != 0xFF || (b1 & 0xF6) != 0xF0 {
        return None;
    }
    // "protection absent" is 0 when a 16-bit CRC follows the header.
    let header_len = if b1 & 1 == 1 { 7 } else { 9 };
    let object_type = (b2 >> 6) + 1;
    let sample_rate = *SAMPLE_RATES.get(usize::from((b2 >> 2) & 0xF))?;
    let channels = u32::from(((b2 & 1) << 2) | (b3 >> 6));
    let frame_len = (usize::from(b3 & 0x3) << 11) | (usize::from(b4) << 3) | usize::from(b5 >> 5);
    if frame_len <= header_len {
        return None;
    }
    Some((header_len, frame_len, object_type, sample_rate, channels))
}

/// Split an AAC audio queue buffer into raw access units. Apps either hand
/// over ADTS frames (self-framing, as read from an `.aac` file) or raw access
/// units whose boundaries are in the packet descriptions.
fn split_aac_access_units<'a>(data: &'a [u8], packet_descs: &[(u32, u32)]) -> Vec<&'a [u8]> {
    let strip_adts = |frame: &'a [u8]| match parse_adts_header(frame) {
        Some((header_len, frame_len, ..)) => &frame[header_len..frame_len.min(frame.len())],
        None => frame,
    };
    if !packet_descs.is_empty() {
        return packet_descs
            .iter()
            .filter_map(|&(start, size)| data.get(start as usize..)?.get(..size as usize))
            .filter(|unit| !unit.is_empty())
            .map(strip_adts)
            .collect();
    }
    if parse_adts_header(data).is_none() {
        return if data.is_empty() { Vec::new() } else { vec![data] };
    }
    let mut units = Vec::new();
    let mut pos = 0;
    while let Some((header_len, frame_len, ..)) = parse_adts_header(&data[pos..]) {
        let Some(frame) = data[pos..].get(..frame_len) else {
            break;
        };
        units.push(&frame[header_len..]);
        pos += frame_len;
    }
    units
}

/// [decode_buffer] for a buffer enqueued on an audio queue. MP3 and AAC are
/// decoded with the queue's own decoder, since a frame depends on the ones in
/// earlier buffers (MP3's bit reservoir, AAC's overlapping transform) and a
/// buffer may hold as little as one frame (FMOD, or the last frame of a
/// looping track).
fn decode_queue_buffer(
    frame_decoder: &mut Option<FrameDecoder>,
    mem: &Mem,
    format: &AudioStreamBasicDescription,
    audio_data: MutPtr<u8>,
    audio_data_byte_size: GuestUSize,
    packet_descs: &[(u32, u32)],
) -> (ALenum, ALsizei, Vec<u8>) {
    let is_mp3 = format.format_id == kAudioFormatMPEGLayer3;
    let is_aac = format.format_id == kAudioFormatMPEG4AAC;
    if !is_mp3 && !is_aac {
        return decode_buffer(mem, format, audio_data, audio_data_byte_size, packet_descs);
    }

    let data = mem.bytes_at(audio_data, audio_data_byte_size);
    if frame_decoder.is_none() {
        *frame_decoder = if is_mp3 {
            FrameDecoder::new_mp3()
        } else {
            // The ADTS header describes the coded stream; Apple's format flags
            // carry the MPEG-4 object type for raw access units. Only AAC-LC
            // (object type 2) can be decoded frame by frame.
            let (object_type, sample_rate, channels) = match parse_adts_header(data) {
                Some((_, _, object_type, sample_rate, channels)) => {
                    (u32::from(object_type), sample_rate, channels)
                }
                None => (
                    if format.format_flags == 0 {
                        2
                    } else {
                        format.format_flags
                    },
                    format.sample_rate as u32,
                    format.channels_per_frame,
                ),
            };
            if object_type == 2 {
                FrameDecoder::new_aac(sample_rate, channels)
            } else {
                None
            }
        };
    }
    let Some(decoder) = frame_decoder else {
        return decode_buffer(mem, format, audio_data, audio_data_byte_size, packet_descs);
    };

    let frames: Vec<&[u8]> = if is_aac {
        split_aac_access_units(data, packet_descs)
    } else {
        let mut frames: Vec<&[u8]> = packet_descs
            .iter()
            .filter_map(|&(start, size)| data.get(start as usize..)?.get(..size as usize))
            .collect();
        if frames.is_empty() {
            // MP3 is self-framing, so packet descriptions are optional.
            let mut pos = 0;
            while pos + 4 <= data.len() {
                match parse_mp3_frame_header(&data[pos..]) {
                    Some(header) if pos + header.frame_len <= data.len() => {
                        frames.push(&data[pos..][..header.frame_len]);
                        pos += header.frame_len;
                    }
                    _ => pos += 1,
                }
            }
        }
        frames
    };

    let mut pcm = Vec::new();
    let mut spec: Option<(u32, u32)> = None;
    for frame in frames {
        let Some((frame_pcm, rate, channels)) = decoder.decode_frame(frame) else {
            continue;
        };
        // An OpenAL buffer has one format, so a buffer that changes channel
        // count mid-way (very unusual) keeps the first one.
        if *spec.get_or_insert((rate, channels)) == (rate, channels) {
            pcm.extend_from_slice(frame_pcm);
        }
    }
    let (rate, channels) = spec.unwrap_or((
        format.sample_rate as u32,
        format.channels_per_frame,
    ));
    let al_format = if channels == 2 {
        al::AL_FORMAT_STEREO16
    } else {
        al::AL_FORMAT_MONO16
    };
    (al_format, rate as ALsizei, pcm)
}

pub fn decode_buffer(
    mem: &Mem,
    format: &AudioStreamBasicDescription,
    audio_data: MutPtr<u8>,
    audio_data_byte_size: GuestUSize,
    packet_descs: &[(u32, u32)],
) -> (ALenum, ALsizei, Vec<u8>) {
    let data_slice = mem.bytes_at(audio_data, audio_data_byte_size);
    decode_buffer_from_bytes(format, data_slice, packet_descs)
}

pub fn decode_buffer_from_bytes(
    format: &AudioStreamBasicDescription,
    data_slice: &[u8],
    packet_descs: &[(u32, u32)],
) -> (ALenum, ALsizei, Vec<u8>) {
    if !is_supported_audio_format(format) {
        // Real CoreAudio would refuse the buffer back at
        // AudioQueueNewOutput, but if a previously valid queue is fed an
        // unsupported format mid-stream (e.g. after seek) we still don't
        // want to crash the host.
        log!(
            "Warning: decode_buffer: format is not supported by our HLE: {:?}; returning empty buffer.",
            format
        );
        return (
            al::AL_FORMAT_MONO16,
            format.sample_rate.max(8000.0) as ALsizei,
            Vec::new(),
        );
    }

    match format.format_id {
        kAudioFormatAppleIMA4 => {
            let valid_len = data_slice.len() / 34 * 34;
            if valid_len != data_slice.len() {
                log!(
                    "Warning: decode_buffer: ignoring {} trailing bytes in Apple IMA4 data.",
                    data_slice.len() - valid_len
                );
            }
            let data_slice = &data_slice[..valid_len];
            let mut out_pcm = Vec::<u8>::with_capacity((data_slice.len() / 34) * 64 * 2);
            let packets = data_slice.chunks(34);

            if format.channels_per_frame == 1 {
                for packet in packets {
                    let pcm_packet: [i16; 64] = decode_ima4(packet.try_into().unwrap());
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
                    let left_pcm_packet: [i16; 64] = decode_ima4(left.try_into().unwrap());
                    let right = peekable_packets.next().unwrap();
                    let right_pcm_packet: [i16; 64] = decode_ima4(right.try_into().unwrap());

                    for (l, r) in left_pcm_packet.iter().zip(right_pcm_packet.iter()) {
                        out_pcm.extend_from_slice(&l.to_le_bytes());
                        out_pcm.extend_from_slice(&r.to_le_bytes());
                    }
                }

                (
                    al::AL_FORMAT_STEREO16,
                    format.sample_rate as ALsizei,
                    out_pcm,
                )
            }
        }
        kAudioFormatLinearPCM => {
            let misaligned_by = data_slice.len() % (format.bytes_per_frame as usize);
            let data_slice = if misaligned_by != 0 {
                &data_slice[..data_slice.len() - misaligned_by]
            } else {
                data_slice
            };

            let bytes_per_channel = format.bits_per_channel / 8;
            let actual_bytes_per_frame = format.channels_per_frame * bytes_per_channel;
            let actual_channels_per_frame = format.bytes_per_frame / bytes_per_channel;

            let mut processed_data: Vec<u8> = if actual_bytes_per_frame == format.bytes_per_frame {
                data_slice.to_owned()
            } else {
                let actual_frame_count = data_slice.len() / actual_bytes_per_frame as usize;

                let processed_frame_count = format.bytes_per_frame as usize * actual_frame_count;
                let mut processed_data = Vec::<u8>::with_capacity(processed_frame_count);

                for frame in data_slice.chunks(actual_bytes_per_frame as usize) {
                    let frame_bytes = &frame[frame.len() - format.bytes_per_frame as usize..];

                    match format.bytes_per_frame {
                        1 => processed_data.extend(
                            &u8::from_be_bytes(frame_bytes.try_into().unwrap()).to_le_bytes(),
                        ),
                        2 => processed_data.extend_from_slice(
                            &u16::from_be_bytes(frame_bytes.try_into().unwrap()).to_le_bytes(),
                        ),
                        4 => processed_data.extend_from_slice(
                            &u32::from_be_bytes(frame_bytes.try_into().unwrap()).to_le_bytes(),
                        ),
                        8 => processed_data.extend_from_slice(
                            &u64::from_be_bytes(frame_bytes.try_into().unwrap()).to_le_bytes(),
                        ),
                        16 => processed_data.extend_from_slice(
                            &u128::from_be_bytes(frame_bytes.try_into().unwrap_or([0u8; 16]))
                                .to_le_bytes(),
                        ),
                        other => {
                            log!(
                                "Warning: decode_buffer: unsupported bytes_per_frame={}, dropping frame.",
                                other
                            );
                            // Pad with zeroes to keep frame alignment in
                            // the consumer; better than aborting.
                            processed_data.extend(std::iter::repeat_n(0u8, other as usize));
                        }
                    };
                }
                processed_data
            };

            let f = match (actual_channels_per_frame, format.bits_per_channel) {
                (1, 8) => al::AL_FORMAT_MONO8,
                (1, 16) => al::AL_FORMAT_MONO16,
                (2, 8) => al::AL_FORMAT_STEREO8,
                (2, 16) => al::AL_FORMAT_STEREO16,
                // --- ДОБАВЛЕНА РАБОЧАЯ ВЕТКА ДЛЯ (1, 32) ---
                (1, 32) => {
                    let valid_len = processed_data.len() / 4 * 4;
                    processed_data.truncate(valid_len);
                    let new_size = (processed_data.len() / 4) * 2;
                    let mut new_processed_data = Vec::<u8>::with_capacity(new_size);

                    if (format.format_flags & kAudioFormatFlagIsFloat) != 0 {
                        // 32-bit float PCM: convert float [-1.0, 1.0] to i16
                        // Apple Core Audio docs: kAudioFormatFlagIsFloat
                        // indicates IEEE 754 floating point samples.
                        // https://developer.apple.com/documentation/coreaudiotypes/kaudioformatflagisfloat
                        for chunk in processed_data.chunks(4) {
                            let val = f32::from_le_bytes(chunk.try_into().unwrap());
                            // Clamp to [-1.0, 1.0] then scale to i16 range
                            let clamped = val.clamp(-1.0, 1.0);
                            let new_val: i16 = (clamped * 32767.0) as i16;
                            new_processed_data.extend(new_val.to_le_bytes());
                        }
                    } else {
                        // 32-bit signed integer PCM: shift down to 16-bit
                        for chunk in processed_data.chunks(4) {
                            let val: i32 = i32::from_le_bytes(chunk.try_into().unwrap());
                            let new_val: i16 = (val >> 16) as i16;
                            new_processed_data.extend(new_val.to_le_bytes());
                        }
                    }
                    return (
                        al::AL_FORMAT_MONO16,
                        format.sample_rate as ALsizei,
                        new_processed_data,
                    );
                }
                // --- СУЩЕСТВУЮЩАЯ ВЕТКА (2, 32) ---
                (2, 32) => {
                    let valid_len = processed_data.len() / 4 * 4;
                    processed_data.truncate(valid_len);
                    let new_size = (processed_data.len() / 4) * 2;
                    let mut new_processed_data = Vec::<u8>::with_capacity(new_size);

                    if (format.format_flags & kAudioFormatFlagIsFloat) != 0 {
                        // 32-bit float PCM stereo: convert float [-1.0, 1.0] to i16
                        for chunk in processed_data.chunks(4) {
                            let val = f32::from_le_bytes(chunk.try_into().unwrap());
                            let clamped = val.clamp(-1.0, 1.0);
                            let new_val: i16 = (clamped * 32767.0) as i16;
                            new_processed_data.extend(new_val.to_le_bytes());
                        }
                    } else {
                        // 32-bit signed integer PCM: shift down to 16-bit
                        for chunk in processed_data.chunks(4) {
                            let val: i32 = i32::from_le_bytes(chunk.try_into().unwrap());
                            let new_val: i16 = (val >> 16) as i16;
                            new_processed_data.extend(new_val.to_le_bytes());
                        }
                    }
                    return (
                        al::AL_FORMAT_STEREO16,
                        format.sample_rate as ALsizei,
                        new_processed_data,
                    );
                }
                // ... предыдущие рабочие ветки (1, 32) и (2, 32) остаются как
                // есть ...
                _ => {
                    // Копируем значение в локальную переменную, чтобы избежать
                    // создания ссылки на packed-поле
                    let bits = format.bits_per_channel;
                    log!(
                        "Warning: decode_buffer: unhandled audio format: {} channels, {} bits; returning empty mono16 buffer.",
                        actual_channels_per_frame,
                        bits
                    );
                    return (
                        al::AL_FORMAT_MONO16,
                        format.sample_rate.max(8000.0) as ALsizei,
                        Vec::new(),
                    );
                }
            };

            (f, format.sample_rate as ALsizei, processed_data)
        }
        kAudioFormatMPEGLayer3 | kAudioFormatMPEG4AAC => {
            // Decode the buffer's worth of raw MPEG-1/2 Layer III frames
            // (or ADTS AAC frames) via symphonia (the same decoder we use
            // for `AudioFile` / `ExtAudioFile` MP3/AAC sources). Packets in
            // AudioQueueEnqueueBuffer are typically frame-aligned per the
            // contract of `AudioFileStreamParseBytes` /
            // `AudioFileReadPackets`, so feeding the slice as a single
            // raw stream works in practice. Frames that straddle
            // buffer boundaries are dropped by symphonia and logged at
            // debug level — better than letting AudioQueueStart fail.
            if crate::env_flag_cached!("TOUCHHLE_TRACE_AUDIO") {
                let head: Vec<String> = data_slice
                    .iter()
                    .take(24)
                    .map(|b| format!("{:02x}", b))
                    .collect();
                let fmt_id = format.format_id;
                let sr = format.sample_rate;
                let ch = format.channels_per_frame;
                let bpp = format.bytes_per_packet;
                let fpp = format.frames_per_packet;
                let bpc = format.bits_per_channel;
                let flags = format.format_flags;
                log!(
                    "TOUCHHLE_TRACE_AUDIO: id={} sr={} ch={} bpp={} fpp={} bpc={} flags={:#x} len={} head=[{}]",
                    debug_fourcc(fmt_id),
                    sr,
                    ch,
                    bpp,
                    fpp,
                    bpc,
                    flags,
                    data_slice.len(),
                    head.join(" ")
                );
            }
            // Raw AAC access units (as delivered by the Nu2 engine's
            // AudioQueue path) carry no self-framing: symphonia's probe
            // needs either MP4/CAF containers or ADTS sync words. The game
            // hands us the per-access-unit boundaries out-of-band via the
            // AudioStreamPacketDescription array on AudioQueueEnqueueBuffer.
            // If we have those and the data isn't already ADTS, wrap each
            // access unit in a 7-byte ADTS header so symphonia's ADTS reader
            // can parse the stream. MP3 is self-framing, so it's fed as-is.
            let is_aac = format.format_id == kAudioFormatMPEG4AAC;
            let already_adts = data_slice.len() >= 2
                && data_slice[0] == 0xFF
                && (data_slice[1] & 0xF6) == 0xF0;
            let feed_bytes: Vec<u8> = if is_aac && !already_adts {
                wrap_raw_aac_as_adts(
                    data_slice,
                    packet_descs,
                    format.sample_rate as u32,
                    format.channels_per_frame,
                    format.format_flags,
                )
            } else {
                data_slice.to_vec()
            };
            let cursor = std::io::Cursor::new(feed_bytes);
            let Ok(decoded) = crate::audio::symphonia_formats::decode_symphonia_to_pcm(cursor)
            else {
                let sample_rate = format.sample_rate;
                log!(
                    "Warning: decode_buffer: MP3/AAC chunk could not be decoded; \
                     returning empty mono16 buffer."
                );
                return (
                    al::AL_FORMAT_MONO16,
                    sample_rate.max(8000.0) as ALsizei,
                    Vec::new(),
                );
            };
            let (al_format, pcm) = match decoded.channels {
                1 => (al::AL_FORMAT_MONO16, decoded.bytes),
                2 => (al::AL_FORMAT_STEREO16, decoded.bytes),
                other => {
                    log!(
                        "Warning: decode_buffer: MP3/AAC produced unsupported channel count {}; downmixing to mono.",
                        other
                    );
                    (
                        al::AL_FORMAT_MONO16,
                        downmix_i16_to_mono(&decoded.bytes, other),
                    )
                }
            };
            (al_format, decoded.sample_rate as ALsizei, pcm)
        }
        _ => {
            // Copy values out of the packed struct before formatting to
            // avoid taking unaligned references.
            let format_id = format.format_id;
            let sample_rate = format.sample_rate;
            log!(
                "Warning: decode_buffer: unsupported audio format id {}; returning empty mono16 buffer.",
                format_id
            );
            (
                al::AL_FORMAT_MONO16,
                sample_rate.max(8000.0) as ALsizei,
                Vec::new(),
            )
        }
    }
}

fn prime_audio_queue(env: &mut Environment, in_aq: AudioQueueRef) {
    let (state, context) =
        State::get_with_context(&mut env.framework_state, &mut env.openal_manager);

    // The guest can hold on to AudioQueueRef pointers across
    // AudioQueueDispose, or pass references that were never created (e.g.
    // junk memory left in the AVAudioPlayer host object when its underlying
    // AudioFile became a Dummy and `prepareToPlay` returned early). Real
    // Audio Queue Services would return an error in those cases; mirror that
    // here instead of panicking on `unwrap()`.
    let Some(host_object) = state.audio_queues.get_mut(&in_aq) else {
        log!(
            "Warning: prime_audio_queue({:?}) called on an unknown / disposed \
             audio queue; skipping.",
            in_aq
        );
        return;
    };

    // Input queues have no OpenAL source; they are driven by the host
    // microphone (see handle_input_audio_queue). Don't waste a source on
    // them — some games create many queues, and the source pool is finite.
    // Offline queues don't play either: AudioQueueOfflineRender pulls their
    // buffers.
    if host_object.is_input || host_object.offline_render_format.is_some() {
        return;
    }

    if !is_supported_audio_format(&host_object.format) {
        return;
    }

    if host_object.al_source.is_none() {
        let volume = host_object.volume.clamp(0.0, 1.0);
        let pan = host_object.pan.clamp(-1.0, 1.0);
        let mut al_source = 0;

        // A real Audio Queue is backed by the system audio hardware, which can
        // always play at least a handful of concurrent sounds. OpenAL Soft,
        // however, has a finite source pool (mono_sources + stereo_sources).
        // Games like Ghost Blade and UDKGame allocate a large number of
        // AVAudioPlayer / AudioQueue objects at once and can exhaust it. When
        // that happens `alGenSources` sets AL_OUT_OF_MEMORY / AL_INVALID_VALUE
        // and returns no source. Previously we `assert!`ed the error was zero
        // here, which crashed the entire emulator. Apple's AudioToolbox never
        // tears down the process for this; the affected sound just doesn't
        // play. Mirror that: on failure, drop this source and leave the queue
        // silent instead of panicking.
        let err = unsafe {
            // Clear any pre-existing error so we only observe GenSources'.
            unsafe {
                let _ = context.GetError();
            }
            context.GenSources(1, &mut al_source);
            context.GetError()
        };
        if err != 0 || al_source == 0 {
            log!(
                "Warning: prime_audio_queue({:?}): could not allocate an OpenAL \
                 source (alGetError() = {:#x}); this audio queue will be silent.",
                in_aq,
                err
            );
            // Best-effort cleanup if a source id was (partially) produced.
            if al_source != 0 {
                unsafe {
                    context.DeleteSources(1, &al_source);
                    unsafe {
                        let _ = context.GetError();
                    }
                }
            }
            return;
        }
        unsafe {
            context.Sourcef(al_source, al::AL_MAX_GAIN, volume);
            unsafe {
                let _ = context.GetError();
            }
        };
        apply_al_pan(&context, al_source, pan);
        host_object.al_source = Some(al_source);
    }
    let Some(al_source) = host_object.al_source else {
        return;
    };

    loop {
        let mut al_buffers_queued = 0;
        let mut al_buffers_processed = 0;

        unsafe {
            context.GetSourcei(al_source, al::AL_BUFFERS_QUEUED, &mut al_buffers_queued);
            context.GetSourcei(
                al_source,
                al::AL_BUFFERS_PROCESSED,
                &mut al_buffers_processed,
            );
            let err = context.GetError();
            if err != 0 {
                log!(
                    "Warning: audio queue {:?} OpenAL query failed: {:#x}; skipping refill.",
                    in_aq,
                    err
                );
                break;
            }
        }
        let Ok(al_buffers_queued) = usize::try_from(al_buffers_queued) else {
            log!(
                "Warning: audio queue {:?} reported negative queued-buffer count.",
                in_aq
            );
            break;
        };
        let Ok(al_buffers_processed) = usize::try_from(al_buffers_processed) else {
            log!(
                "Warning: audio queue {:?} reported negative processed-buffer count.",
                in_aq
            );
            break;
        };

        if al_buffers_processed > al_buffers_queued
            || al_buffers_queued > host_object.buffer_queue.len()
        {
            log!("Warning: audio queue {:?} reported inconsistent buffer counts (queued={}, processed={}, tracked={}); skipping refill.", in_aq, al_buffers_queued, al_buffers_processed, host_object.buffer_queue.len());
            break;
        }
        let unprocessed_buffers = al_buffers_queued - al_buffers_processed;

        if unprocessed_buffers > 1 || al_buffers_queued == host_object.buffer_queue.len() {
            break;
        }

        let next_buffer_idx = al_buffers_queued;
        let next_buffer_ref = host_object.buffer_queue[next_buffer_idx];
        let next_buffer = env.mem.read(next_buffer_ref);

        log_dbg!(
            "Decoding buffer {:?} for queue {:?}",
            next_buffer_ref,
            in_aq
        );

        let next_al_buffer = match host_object.al_unused_buffers.pop() {
            Some(buffer) => buffer,
            None => {
                let mut al_buffer = 0;
                unsafe { context.GenBuffers(1, &mut al_buffer) };
                let err = unsafe { context.GetError() };
                if err != 0 || al_buffer == 0 {
                    log!("Warning: audio queue {:?} could not allocate an OpenAL buffer: {:#x}; stopping refill.", in_aq, err);
                    break;
                }
                al_buffer
            }
        };

        let packet_descs = host_object
            .buffer_packet_descs
            .get(&next_buffer_ref)
            .cloned()
            .unwrap_or_default();

        let (al_format, al_frequency, data) = decode_queue_buffer(
            &mut host_object.frame_decoder,
            &env.mem,
            &host_object.format,
            next_buffer.audio_data.cast(),
            next_buffer.audio_data_byte_size,
            &packet_descs,
        );

        // OpenAL only queues buffers that match the format and frequency of
        // those already on the source, and a buffer that couldn't be decoded
        // comes back empty, so convert anything else to the queued format.
        let target = match host_object.al_queued_format {
            Some(queued) if al_buffers_queued > 0 => queued,
            _ => (al_format, al_frequency),
        };
        let data = if (al_format, al_frequency) != target || data.is_empty() {
            conform_pcm((al_format, al_frequency), &data, target)
        } else {
            data
        };
        let (al_format, al_frequency) = target;

        let err = unsafe {
            context.BufferData(
                next_al_buffer,
                al_format,
                data.as_ptr() as *const ALvoid,
                data.len().try_into().unwrap(),
                al_frequency,
            );
            context.SourceQueueBuffers(al_source, 1, &next_al_buffer);
            context.GetError()
        };
        if err != 0 {
            log!(
                "Warning: audio queue {:?} could not queue an OpenAL buffer: {:#x}; stopping refill.",
                in_aq,
                err
            );
            host_object.al_unused_buffers.push(next_al_buffer);
            break;
        }
        host_object.al_queued_format = Some(target);
    }
}

/// Interleaved 16-bit samples of decoded `data` in OpenAL `format`, as
/// `(samples, channels)`.
fn pcm_to_i16(format: ALenum, data: &[u8]) -> (Vec<i16>, usize) {
    match format {
        al::AL_FORMAT_MONO8 | al::AL_FORMAT_STEREO8 => (
            // 8-bit OpenAL samples are unsigned.
            data.iter().map(|&b| (i16::from(b) - 128) << 8).collect(),
            if format == al::AL_FORMAT_STEREO8 { 2 } else { 1 },
        ),
        _ => (
            data.chunks_exact(2)
                .map(|b| i16::from_le_bytes([b[0], b[1]]))
                .collect(),
            if format == al::AL_FORMAT_STEREO16 { 2 } else { 1 },
        ),
    }
}

/// Convert decoded PCM to another OpenAL format and frequency, so it can be
/// queued behind buffers of that format. Channels are duplicated or averaged,
/// and the rate is converted by picking the nearest sample. Empty input
/// becomes a short stretch of silence, so the queue still advances.
fn conform_pcm(from: (ALenum, ALsizei), data: &[u8], to: (ALenum, ALsizei)) -> Vec<u8> {
    let (samples, from_channels) = pcm_to_i16(from.0, data);
    let to_stereo = matches!(to.0, al::AL_FORMAT_STEREO8 | al::AL_FORMAT_STEREO16);
    let to_8bit = matches!(to.0, al::AL_FORMAT_MONO8 | al::AL_FORMAT_STEREO8);
    let from_frames = samples.len() / from_channels;
    let (from_rate, to_rate) = (from.1.max(1) as u64, to.1.max(1) as u64);
    let to_frames = if from_frames == 0 {
        // 10ms of silence.
        (to_rate / 100).max(1) as usize
    } else {
        ((from_frames as u64 * to_rate / from_rate) as usize).max(1)
    };

    let mut out = Vec::with_capacity(to_frames * if to_stereo { 2 } else { 1 } * 2);
    for i in 0..to_frames {
        let (left, right) = if from_frames == 0 {
            (0, 0)
        } else {
            let src = ((i as u64 * from_rate / to_rate) as usize).min(from_frames - 1);
            let frame = &samples[src * from_channels..][..from_channels];
            (frame[0], *frame.get(1).unwrap_or(&frame[0]))
        };
        let stereo = [left, right];
        let mono = [((i32::from(left) + i32::from(right)) / 2) as i16];
        let channels: &[i16] = if to_stereo { &stereo } else { &mono };
        for &sample in channels {
            if to_8bit {
                out.push(((sample >> 8) + 128) as u8);
            } else {
                out.extend_from_slice(&sample.to_le_bytes());
            }
        }
    }
    out
}

fn unqueue_buffers<F: FnMut(ALuint)>(al_source: ALuint, context: &OpenAL<'_>, mut callback: F) {
    loop {
        let mut al_buffers_processed = 0;

        unsafe {
            context.GetSourcei(
                al_source,
                al::AL_BUFFERS_PROCESSED,
                &mut al_buffers_processed,
            );
            assert!(context.GetError() == 0);
        }
        if al_buffers_processed == 0 {
            break;
        }

        let mut al_buffer = 0;

        unsafe {
            context.SourceUnqueueBuffers(al_source, 1, &mut al_buffer);
            assert!(context.GetError() == 0);
        }

        callback(al_buffer);
    }
}

/// One prepared microphone delivery: prepared while the queue's state is
/// still borrowed, executed once the borrow is dropped (the guest callback
/// may call any AudioQueue API, including `AudioQueueDispose` on this queue).
struct InputBufferDelivery {
    buffer_ref: AudioQueueBufferRef,
    /// Absolute host-sample position (a [mic::total_samples] cursor) of the
    /// first sample copied into the buffer.
    from_host: u64,
    /// How many host samples to copy (may be less than a full buffer while
    /// flushing the queue during an asynchronous stop).
    host_sample_count: usize,
    /// Sample time (in queue frames) of the first frame in the buffer, used
    /// for the callback's `inStartTime`.
    sample_time: f64,
}

/// Whether [fill_input_buffer] can encode the host microphone stream into
/// the queue's format. Linear PCM of any shape [pcm_shape_from_asbd] accepts
/// works, as does mono/stereo Apple IMA4 (the two formats iOS 2/3 hardware
/// could record). Queues with any other format still exist, but like output
/// queues with an unsupported format they stay silent.
fn is_supported_input_format(format: &AudioStreamBasicDescription) -> bool {
    match format.format_id {
        kAudioFormatLinearPCM => pcm_shape_from_asbd(format).is_some(),
        kAudioFormatAppleIMA4 => {
            // Each IMA4 packet holds 64 frames of ONE channel; multi-channel
            // audio interleaves whole packets (L packet, R packet, ...).
            format.channels_per_frame == 1 || format.channels_per_frame == 2
        }
        _ => false,
    }
}

/// Frames per packet for an input queue's format (see
/// [is_supported_input_format]): 64 for IMA4, 1 for linear PCM.
fn input_frames_per_packet(format: &AudioStreamBasicDescription) -> u32 {
    match format.format_id {
        kAudioFormatAppleIMA4 => 64,
        _ => 1,
    }
}

/// Bytes per packet for an input queue's format: 34 per channel for IMA4,
/// `bytes_per_packet` (== bytes per frame) for linear PCM.
fn input_bytes_per_packet(format: &AudioStreamBasicDescription) -> u32 {
    match format.format_id {
        kAudioFormatAppleIMA4 => 34 * format.channels_per_frame,
        _ => format.bytes_per_packet.max(1),
    }
}

/// Capacity of an input buffer in whole packets (the queue's currency).
fn input_packet_capacity(
    buffer: &AudioQueueBuffer,
    format: &AudioStreamBasicDescription,
) -> u32 {
    // Trailing partial packets are unusable.
    buffer.audio_data_bytes_capacity / input_bytes_per_packet(format)
}

/// Converts mono 16-bit host-microphone samples at [mic::MIC_SAMPLE_RATE]
/// into interleaved 16-bit PCM in the queue's channel count and sample rate.
fn convert_mic_to_pcm(
    format: &AudioStreamBasicDescription,
    host_samples: &[i16],
) -> Vec<u8> {
    let src_shape = PcmShape {
        sample_rate: f64::from(mic::MIC_SAMPLE_RATE),
        channels: 1,
        bits: 16,
        is_float: false,
        is_big_endian: false,
        bytes_per_frame: 2,
    };
    let channels = format.channels_per_frame.max(1);
    let dst_shape = PcmShape {
        sample_rate: format.sample_rate.max(1.0),
        channels,
        bits: 16,
        is_float: false,
        is_big_endian: false,
        bytes_per_frame: 2 * channels,
    };
    let src: Vec<u8> = host_samples
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect();
    convert_pcm(&src, &src_shape, &dst_shape)
}

/// Encodes `host_samples` (mono 16-bit at [mic::MIC_SAMPLE_RATE]) into the
/// queue's format and stores the result in the guest `AudioQueueBuffer`
/// (`mAudioData` / `mAudioDataByteSize`). Only whole packets are written and
/// only as many as fit `mAudioDataBytesCapacity`. Returns the packet count
/// for the callback's `inNumberPackets`.
fn fill_input_buffer(
    env: &mut Environment,
    buffer_ref: AudioQueueBufferRef,
    format: &AudioStreamBasicDescription,
    host_samples: &[i16],
) -> u32 {
    let buffer = env.mem.read(buffer_ref);
    let capacity = buffer.audio_data_bytes_capacity as usize;
    let channels = format.channels_per_frame.max(1) as usize;

    let encoded: Vec<u8> = match format.format_id {
        kAudioFormatAppleIMA4 => {
            let pcm = convert_mic_to_pcm(format, host_samples);
            let frames = pcm.len() / 2 / channels;
            let periods = frames / 64;
            // A multi-channel period is one 34-byte packet per channel.
            let bytes_per_period = 34 * channels;
            let periods = periods.min(capacity / bytes_per_period);
            let mut out = Vec::with_capacity(periods * bytes_per_period);
            for period in 0..periods {
                for channel in 0..channels {
                    let mut block = [0i16; 64];
                    for (frame, slot) in block.iter_mut().enumerate() {
                        let sample = (period * 64 + frame) * channels + channel;
                        let bytes: [u8; 2] =
                            [pcm[sample * 2], pcm[sample * 2 + 1]];
                        *slot = i16::from_le_bytes(bytes);
                    }
                    out.extend_from_slice(&encode_ima4(&block));
                }
            }
            out
        }
        _ => {
            // Linear PCM: convert to the queue's exact shape (rate, channels,
            // bit depth, float, endianness) via the AudioConverter code used
            // on the playback path.
            let shape = pcm_shape_from_asbd(format);
            let src_shape = PcmShape {
                sample_rate: f64::from(mic::MIC_SAMPLE_RATE),
                channels: 1,
                bits: 16,
                is_float: false,
                is_big_endian: false,
                bytes_per_frame: 2,
            };
            let src: Vec<u8> = host_samples
                .iter()
                .flat_map(|sample| sample.to_le_bytes())
                .collect();
            let mut out = match shape {
                Some(shape) => convert_pcm(&src, &src_shape, &shape),
                // is_supported_input_format() should prevent this; fall back
                // to raw mono LE 16-bit rather than crash.
                None => src,
            };
            // Truncate to whole frames within the buffer's capacity.
            let bytes_per_frame = shape.map(|s| s.bytes_per_frame as usize).unwrap_or(2).max(1);
            let usable = (capacity / bytes_per_frame) * bytes_per_frame;
            out.truncate(usable);
            out
        }
    };

    let num_packets = if format.format_id == kAudioFormatAppleIMA4 {
        (encoded.len() / (34 * channels)) as u32 * format.channels_per_frame.max(1)
    } else {
        let bytes_per_packet = input_bytes_per_packet(format) as usize;
        (encoded.len() / bytes_per_packet) as u32
    };

    // Write the payload and the final byte size back into the guest buffer.
    if !encoded.is_empty() {
        let dst = buffer.audio_data.cast::<u8>();
        env.mem
            .bytes_at_mut(dst, encoded.len() as GuestUSize)
            .copy_from_slice(&encoded);
    }
    env.mem.write(
        buffer_ref.cast(),
        AudioQueueBuffer {
            audio_data_bytes_capacity: buffer.audio_data_bytes_capacity,
            audio_data: buffer.audio_data,
            audio_data_byte_size: encoded.len() as u32,
            user_data: buffer.user_data,
            packet_description_capacity: buffer.packet_description_capacity,
            _packet_descriptions: buffer._packet_descriptions,
            _packet_description_count: 0,
        },
    );

    num_packets
}

/// Run-loop tick for an input (recording) queue: fills guest buffers from
/// the host microphone stream and hands them back via the guest's input
/// callback, exactly like the output path recycles played-out buffers.
///
/// While the queue is meant to be running, buffers are only handed back when
/// a full buffer's worth of fresh audio is available, which keeps
/// `mAudioDataByteSize == mAudioDataBytesCapacity` (apps like blow/gesture
/// detectors rely on the fixed chunk size). During an asynchronous stop
/// (`AudioQueueStop(inImmediate: false)`) the queue instead flushes what it
/// has — partial buffers included — and then finishes stopping, mirroring the
/// output queue's drain behaviour.
fn handle_input_audio_queue(env: &mut Environment, in_aq: AudioQueueRef) {
    // Phase 1: decide what can be delivered while the state is borrowed.
    // Reading guest buffer structs (env.mem) while the state map is borrowed
    // is fine: they are disjoint fields of `env`.
    let (format, is_stopping, deliveries, callback_proc, callback_user_data) = {
        let Some(host_object) = State::get(&mut env.framework_state)
            .audio_queues
            .get_mut(&in_aq)
        else {
            // ns_run_loop can hold a stale reference for one tick after
            // AudioQueueDispose.
            return;
        };
        if host_object.is_running_handler
            || host_object.is_running == AudioQueueIsRunning::Stopped
            || !is_supported_input_format(&host_object.format)
        {
            return;
        }
        host_object.is_running_handler = true;

        let format = host_object.format;
        let guest_rate = format.sample_rate.max(1.0);
        // Host samples per queue frame; fractional so no drift accumulates.
        let host_per_frame = f64::from(mic::MIC_SAMPLE_RATE) / guest_rate;
        let frames_per_packet = input_frames_per_packet(&format) as f64;
        let total_host = mic::total_samples() as f64;
        let is_stopping = host_object.is_running == AudioQueueIsRunning::Stopping;

        let mut deliveries = Vec::new();
        let mut cursor = host_object.input_host_cursor;

        while let Some(&buffer_ref) = host_object.buffer_queue.front() {
            let buffer = env.mem.read(buffer_ref);
            let packet_capacity =
                input_packet_capacity(&buffer, &format);
            let frames_capacity =
                (packet_capacity as f64) * frames_per_packet;
            let needed = frames_capacity * host_per_frame;

            if is_stopping {
                // Flushing: deliver whatever remains (>= 0 bytes), then stop.
                if cursor >= total_host {
                    break; // nothing left to flush
                }
                let available = total_host - cursor;
                let take = available.min(needed).ceil() as usize;
                let sample_time = cursor * guest_rate / f64::from(mic::MIC_SAMPLE_RATE);
                deliveries.push(InputBufferDelivery {
                    buffer_ref,
                    from_host: cursor as u64,
                    host_sample_count: take,
                    sample_time,
                });
                host_object.buffer_queue.pop_front();
                cursor += available.min(needed);
            } else {
                // Normal operation: wait for a full buffer's worth of audio.
                if cursor + needed > total_host {
                    break;
                }
                let sample_time = cursor * guest_rate / f64::from(mic::MIC_SAMPLE_RATE);
                deliveries.push(InputBufferDelivery {
                    buffer_ref,
                    from_host: cursor as u64,
                    host_sample_count: needed.ceil() as usize,
                    sample_time,
                });
                host_object.buffer_queue.pop_front();
                cursor += needed;
            }
        }
        host_object.input_host_cursor = cursor;

        (
            format,
            is_stopping,
            deliveries,
            host_object.callback_proc,
            host_object.callback_user_data,
        )
    };

    // Phase 2: fill buffers and call the guest callback. No state borrows may
    // be live here: the callback can call any AudioQueue API.
    for delivery in deliveries {
        let host_samples =
            mic::take_samples(delivery.from_host, delivery.host_sample_count);
        let num_packets =
            fill_input_buffer(env, delivery.buffer_ref, &format, &host_samples);
        let timestamp = alloc_render_timestamp(env, delivery.sample_time);
        let () = callback_proc.call_from_host(
            env,
            (
                callback_user_data,
                in_aq,
                delivery.buffer_ref,
                timestamp.cast_const(),
                num_packets,
                ConstPtr::<AudioStreamPacketDescription>::null(),
            ),
        );
        env.mem.free(timestamp);
    }

    // Phase 3: finish an asynchronous stop now that every queued buffer has
    // been flushed. (The callback may have disposed the queue already.)
    let queue_exists = State::get(&mut env.framework_state)
        .audio_queues
        .get(&in_aq)
        .is_some();
    if queue_exists {
        let state = State::get(&mut env.framework_state);
        if let Some(host_object) = state.audio_queues.get_mut(&in_aq) {
            host_object.is_running_handler = false;
        }
    }
    if is_stopping && queue_exists {
        finish_stopping_audio_queue(env, in_aq);
    }
}

pub fn handle_audio_queue(env: &mut Environment, in_aq: AudioQueueRef) {
    // Input queues never get an OpenAL source; they are driven by the host
    // microphone stream (crate::audio::mic) instead of the OpenAL buffer
    // lifecycle. Note: handle_input_audio_queue re-checks that the queue is
    // still alive, so a stale run-loop entry after AudioQueueDispose is
    // harmless.
    {
        let is_input_queue = State::get(&mut env.framework_state)
            .audio_queues
            .get(&in_aq)
            .is_some_and(|host_object| host_object.is_input);
        if is_input_queue {
            handle_input_audio_queue(env, in_aq);
            return;
        }
    }

    let (state, context) =
        State::get_with_context(&mut env.framework_state, &mut env.openal_manager);

    // ns_run_loop can still hold a stale `in_aq` for one tick after
    // AudioQueueDispose. Skip silently instead of panicking.
    let Some(host_object) = state.audio_queues.get_mut(&in_aq) else {
        return;
    };

    let Some(al_source) = host_object.al_source else {
        return;
    };

    if !is_supported_audio_format(&host_object.format) {
        return;
    }
    if host_object.is_running_handler {
        return;
    }

    host_object.is_running_handler = true;

    let mut buffers_to_reuse = Vec::new();

    unqueue_buffers(al_source, &context, |al_buffer| {
        host_object.al_unused_buffers.push(al_buffer);
        // OpenAL is reporting one buffer as processed, so the queue should
        // be non-empty. If the host and OpenAL views ever desync (e.g.
        // through an unexpected reset), just stop pulling from the empty
        // queue instead of panicking on `.unwrap()`.
        if let Some(buffer_ref) = host_object.buffer_queue.pop_front() {
            buffers_to_reuse.push(buffer_ref);
        } else {
            log!(
                "Warning: handle_audio_queue({:?}): OpenAL reported a processed \
                 buffer but the guest buffer_queue is empty; skipping.",
                in_aq
            );
        }
    });

    let &mut AudioQueueHostObject {
        callback_proc,
        callback_user_data,
        is_running,
        ..
    } = host_object;

    for buffer_ref in buffers_to_reuse.drain(..) {
        log_dbg!(
            "Recyling buffer {:?} for queue {:?}. Calling callback {:?} with user data {:?}.",
            buffer_ref,
            in_aq,
            callback_proc,
            callback_user_data
        );

        let () = callback_proc.call_from_host(env, (callback_user_data, in_aq, buffer_ref));
    }

    prime_audio_queue(env, in_aq);

    // The guest callback we just invoked above is allowed to call
    // AudioQueueDispose on `in_aq`. If it did, the queue is gone and there
    // is nothing left to do here.
    if State::get(&mut env.framework_state)
        .audio_queues
        .get(&in_aq)
        .is_none()
    {
        return;
    }

    let context = env
        .framework_state
        .audio_toolbox
        .make_al_context_current(&mut env.openal_manager);

    if is_running != AudioQueueIsRunning::Stopped {
        unsafe {
            let mut al_source_state = 0;

            context.GetSourcei(al_source, al::AL_SOURCE_STATE, &mut al_source_state);
            assert!(context.GetError() == 0);
            if al_source_state == al::AL_STOPPED {
                context.SourcePlay(al_source);
                log_dbg!("Restarted OpenAL source for queue {:?}", in_aq);
            }
        }
    }

    if is_running == AudioQueueIsRunning::Stopping {
        let mut al_source_state = 0;

        unsafe {
            context.GetSourcei(al_source, al::AL_SOURCE_STATE, &mut al_source_state);
            assert!(context.GetError() == 0);
        }

        if al_source_state == al::AL_STOPPED {
            log_dbg!(
                "OpenAL source stopped for queue {:?}, completing asynchronous stop.",
                in_aq
            );

            finish_stopping_audio_queue(env, in_aq);
        }
    }

    let state = State::get(&mut env.framework_state);

    if let Some(host_object) = state.audio_queues.get_mut(&in_aq) {
        host_object.is_running_handler = false;
    }
}

fn AudioQueuePrime(
    env: &mut Environment,
    in_aq: AudioQueueRef,
    in_number_of_frames_to_prepare: u32,
    out_number_of_frames_prepared: MutPtr<u32>,
) -> OSStatus {
    return_if_null!(in_aq);

    prime_audio_queue(env, in_aq);

    if !out_number_of_frames_prepared.is_null() {
        let Some(host_object) = State::get(&mut env.framework_state)
            .audio_queues
            .get(&in_aq)
        else {
            env.mem.write(out_number_of_frames_prepared, 0);
            return 0;
        };

        let mut prepared_frames = 0;
        let format = &host_object.format;

        for &buffer_ref in &host_object.buffer_queue {
            let buffer = env.mem.read(buffer_ref);
            let size = buffer.audio_data_byte_size;

            if format.bytes_per_packet > 0 && format.frames_per_packet > 0 {
                prepared_frames += (size / format.bytes_per_packet) * format.frames_per_packet;
            } else if format.bytes_per_frame > 0 {
                prepared_frames += size / format.bytes_per_frame;
            }
        }

        if in_number_of_frames_to_prepare > 0 && prepared_frames > in_number_of_frames_to_prepare {
            prepared_frames = in_number_of_frames_to_prepare;
        }

        env.mem
            .write(out_number_of_frames_prepared, prepared_frames);
    }

    0 // success
}

fn notify_aq_is_running(env: &mut Environment, in_aq: AudioQueueRef) {
    // Snapshot everything the queue holds before invoking any callbacks: a
    // listener may register or remove listeners re-entrantly, and the host
    // object borrow must not outlive the state it came from.
    let Some((is_running_proc, is_running_user_data, listeners)) =
        State::get(&mut env.framework_state)
            .audio_queues
            .get_mut(&in_aq)
            .map(|host_object| {
                (
                    host_object.aq_is_running_proc,
                    host_object.aq_is_running_user_data,
                    host_object.property_listeners.clone(),
                )
            })
    else {
        return;
    };

    if let (Some(in_proc), Some(in_user_data)) = (is_running_proc, is_running_user_data) {
        <GuestFunction as CallFromHost<(), (MutVoidPtr, Ptr<OpaqueAudioQueue, true>, u32)>>::
        call_from_host(
            &in_proc, env, (in_user_data, in_aq, kAudioQueueProperty_IsRunning)
        );
    }

    // Fire generic listeners registered for other properties.
    for &(property_id, in_proc, in_user_data) in listeners.iter() {
        <GuestFunction as CallFromHost<(), (MutVoidPtr, Ptr<OpaqueAudioQueue, true>, u32)>>::
        call_from_host(&in_proc, env, (in_user_data, in_aq, property_id));
    }
}

pub fn AudioQueueStart(
    env: &mut Environment,
    in_aq: AudioQueueRef,
    _in_device_start_time: ConstVoidPtr,
) -> OSStatus {
    return_if_null!(in_aq);

    // Input (recording) queues have no OpenAL side: starting one opens the
    // host microphone stream and arms the delivery loop.
    if let Some(result) = start_input_audio_queue(env, in_aq) {
        return result;
    }

    // Offline queues render on demand (AudioQueueOfflineRender) instead of
    // to the output hardware.
    if let Some(host_object) = State::get(&mut env.framework_state)
        .audio_queues
        .get_mut(&in_aq)
    {
        if host_object.offline_render_format.is_some() {
            host_object.is_running = AudioQueueIsRunning::Running;
            notify_aq_is_running(env, in_aq);
            return 0;
        }
    }

    prime_audio_queue(env, in_aq);

    let (state, context) =
        State::get_with_context(&mut env.framework_state, &mut env.openal_manager);

    let Some(host_object) = state.audio_queues.get_mut(&in_aq) else {
        log!(
            "Warning: AudioQueueStart({:?}) on an unknown / disposed queue; \
             returning error.",
            in_aq
        );
        return kAudioQueueErr_InvalidProperty;
    };

    if is_supported_audio_format(&host_object.format) {
        let Some(al_source) = host_object.al_source else {
            // prime_audio_queue should have created the OpenAL source, but
            // it bails out early for unsupported formats and missing
            // queues. Don't panic if we somehow get here without a source.
            log!(
                "Warning: AudioQueueStart({:?}) found no OpenAL source after \
                 priming; skipping playback.",
                in_aq
            );
            return kAudioQueueErr_InvalidProperty;
        };
        host_object.is_running = AudioQueueIsRunning::Running;
        unsafe { context.SourcePlay(al_source) };
        assert!(unsafe { context.GetError() } == 0);
    } else {
        log!(
            "AudioQueueStart: Unsupported format {:?}, not starting",
            host_object.format
        );
        return kAudioQueueErr_InvalidProperty;
    }

    notify_aq_is_running(env, in_aq);

    0 // success
}

pub fn AudioQueuePause(env: &mut Environment, in_aq: AudioQueueRef) -> OSStatus {
    return_if_null!(in_aq);

    let (state, context) =
        State::get_with_context(&mut env.framework_state, &mut env.openal_manager);

    let Some(host_object) = state.audio_queues.get_mut(&in_aq) else {
        return 0;
    };

    host_object.is_running = AudioQueueIsRunning::Stopped;

    if let Some(al_source) = host_object.al_source {
        unsafe { context.SourcePause(al_source) };
        assert!(unsafe { context.GetError() } == 0);
    }

    0 // success
}

fn finish_stopping_audio_queue(env: &mut Environment, in_aq: AudioQueueRef) {
    AudioQueueReset(env, in_aq);
    if let Some(host_object) = State::get(&mut env.framework_state)
        .audio_queues
        .get_mut(&in_aq)
    {
        // Stopping an input queue releases its hold on the host microphone
        // stream, like iOS powers the capture hardware down when the last
        // active recorder stops.
        if host_object.is_input && host_object.input_capture_ref {
            host_object.input_capture_ref = false;
            mic::stop();
        }
        host_object.is_running = AudioQueueIsRunning::Stopped;
    }

    notify_aq_is_running(env, in_aq);
}

/// AudioQueueStart for input queues. Returns `Some(status)` when the queue
/// is an input queue (the caller returns that status), `None` when it is an
/// output queue (the normal OpenAL path takes over).
fn start_input_audio_queue(env: &mut Environment, in_aq: AudioQueueRef) -> Option<OSStatus> {
    let host_object = State::get(&mut env.framework_state)
        .audio_queues
        .get_mut(&in_aq)?;

    if !host_object.is_input {
        return None;
    }

    if !is_supported_input_format(&host_object.format) {
        log!(
            "Warning: AudioQueueStart({:?}): input format {:?} is not supported \
             by our HLE; the queue will not deliver audio.",
            in_aq,
            host_object.format
        );
        return Some(kAudioQueueErr_InvalidProperty);
    }

    // Pause + Start also lands here (AudioQueuePause keeps the capture
    // reference), so only acquire the stream once per Stop..Start cycle.
    host_object.is_running = AudioQueueIsRunning::Running;
    // Recording starts "now": skip anything captured before this moment.
    host_object.input_host_cursor =
        mic::total_samples() as f64 + f64::from(host_object.input_delay);
    if !host_object.input_capture_ref {
        host_object.input_capture_ref = true;
        if !mic::start() {
            log_dbg!(
                "AudioQueueStart({:?}): no host microphone available; input \
                 will be silence.",
                in_aq
            );
        }
    }

    // The state borrow ends with the last use of host_object, so guest
    // code (the property listeners) can run now.
    notify_aq_is_running(env, in_aq);

    Some(0)
}

pub fn AudioQueueStop(env: &mut Environment, in_aq: AudioQueueRef, in_immediate: bool) -> OSStatus {
    return_if_null!(in_aq);

    if in_immediate {
        log_dbg!("Performing immediate AudioQueueStop for {:?}.", in_aq);

        let (state, context) =
            State::get_with_context(&mut env.framework_state, &mut env.openal_manager);

        let Some(host_object) = state.audio_queues.get_mut(&in_aq) else {
            return 0;
        };
        if let Some(al_source) = host_object.al_source {
            unsafe { context.SourceStop(al_source) };
            assert!(unsafe { context.GetError() } == 0);
        };

        finish_stopping_audio_queue(env, in_aq);
    } else {
        let state = State::get(&mut env.framework_state);

        let Some(host_object) = state.audio_queues.get_mut(&in_aq) else {
            return 0;
        };
        if host_object.is_running != AudioQueueIsRunning::Stopped {
            log_dbg!("Starting asynchronous AudioQueueStop for {:?}.", in_aq);

            host_object.is_running = AudioQueueIsRunning::Stopping;
        } else {
            log_dbg!(
                "Ignoring asynchronous AudioQueueStop for {:?} (already stopped).",
                in_aq
            );
        }
    }

    0 // success
}

fn AudioQueueReset(env: &mut Environment, in_aq: AudioQueueRef) -> OSStatus {
    return_if_null!(in_aq);

    let (state, context) =
        State::get_with_context(&mut env.framework_state, &mut env.openal_manager);

    log_dbg!("Resetting queue {:?}.", in_aq);

    let Some(host_object) = state.audio_queues.get_mut(&in_aq) else {
        return 0;
    };

    if let Some(al_source) = host_object.al_source {
        unsafe {
            let mut al_source_state = 0;

            context.GetSourcei(al_source, al::AL_SOURCE_STATE, &mut al_source_state);
            assert!(context.GetError() == 0);
            if al_source_state != al::AL_STOPPED {
                context.SourceStop(al_source);
                assert!(context.GetError() == 0);
            }
        }

        unqueue_buffers(al_source, &context, |al_buffer| {
            host_object.al_unused_buffers.push(al_buffer);
            host_object.buffer_queue.pop_front().unwrap();
        });
    }

    host_object.buffer_queue.clear();
    host_object.offline_pcm.clear();
    // Whatever's enqueued next needn't follow on from what came before.
    host_object.frame_decoder = None;

    if host_object.is_input {
        // Drop pending data: delivery resumes from the current capture
        // position (the ring keeps advancing, so stale samples between the
        // old cursor and now are simply skipped, like re-primed hardware).
        host_object.input_host_cursor = mic::total_samples() as f64;
    }

    0 // success
}

fn AudioQueueFlush(_env: &mut Environment, in_aq: AudioQueueRef) -> OSStatus {
    return_if_null!(in_aq);
    0 // success
}

fn AudioQueueFreeBuffer(
    env: &mut Environment,
    in_aq: AudioQueueRef,
    in_buffer: AudioQueueBufferRef,
) -> OSStatus {
    return_if_null!(in_aq);

    let Some(host_object) = State::get(&mut env.framework_state)
        .audio_queues
        .get_mut(&in_aq)
    else {
        return kAudioQueueErr_InvalidBuffer;
    };

    if host_object.buffer_queue.contains(&in_buffer) {
        return kAudioQueueErr_BufferInQueue;
    }

    if let Some(index) = host_object.buffers.iter().position(|x| x == &in_buffer) {
        host_object.buffers.remove(index);

        log_dbg!("Freeing buffer: {:?}", in_buffer);

        let buffer = env.mem.read(in_buffer);
        env.mem.free(buffer.audio_data);
        env.mem.free(in_buffer.cast());

        0 // success
    } else {
        kAudioQueueErr_InvalidBuffer
    }
}

pub fn AudioQueueDispose(
    env: &mut Environment,
    in_aq: AudioQueueRef,
    _in_immediate: bool,
) -> OSStatus {
    return_if_null!(in_aq);

    let (state, context) =
        State::get_with_context(&mut env.framework_state, &mut env.openal_manager);

    let Some(mut host_object) = state.audio_queues.remove(&in_aq) else {
        // Disposing a queue that was never created (or was already disposed)
        // is a no-op; don't panic and don't double-free the OpaqueAudioQueue
        // pointer.
        log_dbg!(
            "AudioQueueDispose({:?}) ignored: queue is unknown / already \
             disposed.",
            in_aq
        );
        return 0;
    };
    log_dbg!("Disposing of audio queue {:?}", in_aq);

    // Input queues: release the host microphone stream even if the queue was
    // disposed while still running (mic::stop() is a no-op when this queue
    // holds no reference).
    if host_object.is_input && host_object.input_capture_ref {
        host_object.input_capture_ref = false;
        mic::stop();
    }

    env.mem.free(in_aq.cast());

    for buffer_ptr in host_object.buffers {
        let buffer = env.mem.read(buffer_ptr);
        env.mem.free(buffer.audio_data);
        env.mem.free(buffer_ptr.cast());
    }

    if let Some(al_source) = host_object.al_source {
        unsafe {
            context.SourceStop(al_source);
            assert!(context.GetError() == 0);
        }

        unqueue_buffers(al_source, &context, |al_buffer| {
            host_object.al_unused_buffers.push(al_buffer)
        });

        unsafe {
            context.DeleteBuffers(
                host_object.al_unused_buffers.len().try_into().unwrap(),
                host_object.al_unused_buffers.as_ptr(),
            );
            assert!(context.GetError() == 0);
        }

        // Free the OpenAL source back to the (finite) source pool. OpenAL Soft
        // only provides a limited number of sources (mono_sources +
        // stereo_sources), unlike a real Audio Queue which is backed by the
        // system audio hardware. Previously the source was never deleted here,
        // so every disposed AudioQueue / AVAudioPlayer leaked one source.
        // Games that allocate many short-lived AVAudioPlayers for sound
        // effects (e.g. BAROQUE) eventually exhausted the pool: alGenSources
        // then failed with AL_OUT_OF_MEMORY (0xA005), all further audio went
        // silent, and the game aborted ("これ以上オーディを再生できません").
        //
        // Apple's AudioQueueDispose documents that it "disposes of an audio
        // queue object and all of its resources"
        // <https://developer.apple.com/documentation/audiotoolbox/audioqueuedispose(_:_:)>,
        // so releasing the source here matches the documented behaviour.
        unsafe {
            context.DeleteSources(1, &al_source);
            assert!(context.GetError() == 0);
        }
        host_object.al_source = None;
    }

    ns_run_loop::remove_audio_queue(env, host_object.run_loop, in_aq);

    0 // success
}

pub fn AudioQueueNewInput(
    env: &mut Environment,
    in_format: ConstPtr<AudioStreamBasicDescription>,
    in_callback_proc: AudioQueueOutputCallback,
    in_user_data: MutVoidPtr,
    in_callback_run_loop: CFRunLoopRef,
    _in_callback_run_loop_mode: CFRunLoopMode,
    in_flags: u32,
    out_aq: MutPtr<AudioQueueRef>,
) -> OSStatus {
    if in_flags != 0 {
        log!(
            "Warning: AudioQueueNewInput: ignoring unexpected non-zero flags {:#x}",
            in_flags
        );
    }
    if in_format.is_null() || out_aq.is_null() {
        return crate::frameworks::carbon_core::paramErr;
    }

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
        pan: 0.0,
        play_rate: 1.0,
        pitch: 0.0,
        volume_ramp_time: 0.0,
        level_metering_enabled: false,
        buffers: Vec::new(),
        buffer_queue: VecDeque::new(),
        buffer_packet_descs: HashMap::new(),
        is_running: AudioQueueIsRunning::Stopped,
        al_source: None,
        al_unused_buffers: Vec::new(),
        aq_is_running_proc: None,
        aq_is_running_user_data: None,
        property_listeners: Vec::new(),
        is_running_handler: false,
        is_input: true,
        input_capture_ref: false,
        input_delay: 0,
        input_host_cursor: 0.0,
        hardware_codec_policy: codec_policy::DEFAULT,
        offline_render_format: None,
        offline_pcm: VecDeque::new(),
        offline_pcm_channels: 0,
        frame_decoder: None,
        al_queued_format: None,
    };

    let aq_ref = env.mem.alloc_and_write(OpaqueAudioQueue { _filler: 0 });
    State::get(&mut env.framework_state)
        .audio_queues
        .insert(aq_ref, host_object);

    env.mem.write(out_aq, aq_ref);

    ns_run_loop::add_audio_queue(env, in_callback_run_loop, aq_ref);

    0
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(AudioQueueNewOutput(_, _, _, _, _, _, _)),
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
    export_c_func!(AudioQueueSetOfflineRenderFormat(_, _, _)),
    export_c_func!(AudioQueueOfflineRender(_, _, _, _)),
    export_c_func!(AudioQueuePrime(_, _, _)),
    export_c_func!(AudioQueueStart(_, _)),
    export_c_func!(AudioQueuePause(_)),
    export_c_func!(AudioQueueStop(_, _)),
    export_c_func!(AudioQueueReset(_)),
    export_c_func!(AudioQueueFlush(_)),
    export_c_func!(AudioQueueFreeBuffer(_, _)),
    export_c_func!(AudioQueueDispose(_, _)),
    export_c_func!(AudioQueueNewInput(_, _, _, _, _, _, _)),
];

#[cfg(test)]
mod tests {
    use super::downmix_i16_to_mono;
    use super::{
        input_bytes_per_packet, input_packet_capacity, is_supported_input_format,
        AudioQueueBuffer, AudioStreamBasicDescription, Ptr, kAudioFormatAppleIMA4,
        kAudioFormatFlagIsFloat, kAudioFormatFlagIsPacked, kAudioFormatLinearPCM,
        kAudioFormatMPEG4AAC,
    };

    #[test]
    fn adts_frames_are_split_into_raw_access_units() {
        use super::{parse_adts_header, split_aac_access_units};
        // AAC-LC, 22050 Hz, stereo, no CRC, 9-byte frame (2-byte payload).
        let frame = |payload: [u8; 2]| {
            let mut frame = vec![0xFF, 0xF1, 0x5C, 0x80, 0x01, 0x3F, 0xFC];
            frame.extend_from_slice(&payload);
            frame
        };
        let mut data = frame([1, 2]);
        assert_eq!(parse_adts_header(&data), Some((7, 9, 2, 22050, 2)));
        // A lone frame is enough: there's no container to probe.
        assert_eq!(split_aac_access_units(&data, &[]), [&[1u8, 2][..]]);
        data.extend(frame([3, 4]));
        assert_eq!(
            split_aac_access_units(&data, &[]),
            [&[1u8, 2][..], &[3, 4][..]]
        );
        // Raw access units are delimited by the packet descriptions.
        assert_eq!(
            split_aac_access_units(&[5, 6, 7], &[(0, 1), (1, 2)]),
            [&[5u8][..], &[6, 7][..]]
        );
    }

    #[test]
    fn conform_pcm_matches_the_queued_format() {
        use super::{al, conform_pcm};
        let mono = [100i16, -200]
            .into_iter()
            .flat_map(i16::to_le_bytes)
            .collect::<Vec<_>>();
        let stereo = conform_pcm(
            (al::AL_FORMAT_MONO16, 11025),
            &mono,
            (al::AL_FORMAT_STEREO16, 22050),
        );
        let samples = stereo
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect::<Vec<_>>();
        assert_eq!(samples, [100, 100, 100, 100, -200, -200, -200, -200]);
        // An undecodable buffer becomes 10ms of silence.
        let silence = conform_pcm(
            (al::AL_FORMAT_MONO16, 22050),
            &[],
            (al::AL_FORMAT_STEREO16, 22050),
        );
        assert_eq!(silence.len(), 220 * 2 * 2);
        assert!(silence.iter().all(|&b| b == 0));
    }

    #[test]
    fn downmix_averages_each_interleaved_frame() {
        let samples = [12000i16, -6000, 0, i16::MIN, i16::MAX, -32768];
        let input = samples
            .into_iter()
            .flat_map(i16::to_le_bytes)
            .collect::<Vec<_>>();
        let output = downmix_i16_to_mono(&input, 3);
        let decoded = output
            .chunks_exact(2)
            .map(|sample| i16::from_le_bytes([sample[0], sample[1]]))
            .collect::<Vec<_>>();
        assert_eq!(decoded, [2000, -10923]);
    }

    #[test]
    fn downmix_drops_incomplete_frames() {
        let input = [0x01, 0x00, 0x02];
        assert!(downmix_i16_to_mono(&input, 2).is_empty());
    }

    #[test]
    fn input_capacity_counts_whole_packets_only() {
        let mono_ima4 = AudioStreamBasicDescription {
            format_id: kAudioFormatAppleIMA4,
            channels_per_frame: 1,
            bytes_per_packet: 34,
            frames_per_packet: 64,
            bytes_per_frame: 34,
            ..pcm_test_shape()
        };
        // 3 full packets + 10 trailing bytes: only 3 packets are usable.
        let buffer = AudioQueueBuffer {
            audio_data_bytes_capacity: 3 * 34 + 10,
            ..test_buffer()
        };
        assert_eq!(input_packet_capacity(&buffer, &mono_ima4), 3);
        assert_eq!(input_bytes_per_packet(&mono_ima4), 34);

        let stereo_ima4 = AudioStreamBasicDescription {
            channels_per_frame: 2,
            ..mono_ima4
        };
        // A stereo "packet" is one L and one R channel packet (68 bytes).
        assert_eq!(input_bytes_per_packet(&stereo_ima4), 68);
        let buffer = AudioQueueBuffer {
            audio_data_bytes_capacity: 68 + 67,
            ..test_buffer()
        };
        assert_eq!(input_packet_capacity(&buffer, &stereo_ima4), 1);

        let stereo_pcm = AudioStreamBasicDescription {
            format_id: kAudioFormatLinearPCM,
            channels_per_frame: 2,
            bytes_per_packet: 4,
            frames_per_packet: 1,
            bytes_per_frame: 4,
            ..pcm_test_shape()
        };
        let buffer = AudioQueueBuffer {
            audio_data_bytes_capacity: 4 * 5 + 3,
            ..test_buffer()
        };
        assert_eq!(input_packet_capacity(&buffer, &stereo_pcm), 5);
    }

    #[test]
    fn input_format_support_matches_ios_recording_formats() {
        let lpcm_float_stereo = AudioStreamBasicDescription {
            format_id: kAudioFormatLinearPCM,
            format_flags: kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked,
            channels_per_frame: 2,
            bytes_per_packet: 8,
            frames_per_packet: 1,
            bytes_per_frame: 8,
            bits_per_channel: 32,
            ..pcm_test_shape()
        };
        assert!(is_supported_input_format(&lpcm_float_stereo));

        let ima4_mono = AudioStreamBasicDescription {
            format_id: kAudioFormatAppleIMA4,
            channels_per_frame: 1,
            ..pcm_test_shape()
        };
        assert!(is_supported_input_format(&ima4_mono));

        // 3-channel IMA4 and AAC input cannot be encoded for the guest.
        let ima4_three_channel = AudioStreamBasicDescription {
            channels_per_frame: 3,
            ..ima4_mono
        };
        assert!(!is_supported_input_format(&ima4_three_channel));
        let aac = AudioStreamBasicDescription {
            format_id: kAudioFormatMPEG4AAC,
            ..pcm_test_shape()
        };
        assert!(!is_supported_input_format(&aac));
    }

    fn pcm_test_shape() -> AudioStreamBasicDescription {
        AudioStreamBasicDescription {
            sample_rate: 44100.0,
            format_id: kAudioFormatLinearPCM,
            format_flags: kAudioFormatFlagIsPacked,
            bytes_per_packet: 2,
            frames_per_packet: 1,
            bytes_per_frame: 2,
            channels_per_frame: 1,
            bits_per_channel: 16,
            _reserved: 0,
        }
    }

    fn test_buffer() -> AudioQueueBuffer {
        AudioQueueBuffer {
            audio_data_bytes_capacity: 0,
            audio_data: Ptr::null(),
            audio_data_byte_size: 0,
            user_data: Ptr::null(),
            packet_description_capacity: 0,
            _packet_descriptions: Ptr::null(),
            _packet_description_count: 0,
        }
    }
}

#[cfg(test)]
mod offline_render_tests {
    use super::*;

    fn pcm_format(channels: u32, bits: u32, flags: u32) -> AudioStreamBasicDescription {
        AudioStreamBasicDescription {
            sample_rate: 44100.0,
            format_id: kAudioFormatLinearPCM,
            format_flags: flags,
            bytes_per_packet: channels * bits / 8,
            frames_per_packet: 1,
            bytes_per_frame: channels * bits / 8,
            channels_per_frame: channels,
            bits_per_channel: bits,
            _reserved: 0,
        }
    }

    #[test]
    fn decodes_mp3_a_frame_at_a_time() {
        // FMOD enqueues one MP3 frame per buffer. Most frames use bits from
        // the frames before them, so this only works if the queue's decoder
        // carries over from buffer to buffer.
        let bytes = include_bytes!("../../../tests/fixtures/sine-440hz-mono.mp3").to_vec();
        let file = crate::audio::AudioFile::read_mp3_packets_from_vec(bytes).unwrap();
        let packets = file.compressed_packets().unwrap();
        let format = AudioStreamBasicDescription {
            sample_rate: 44100.0,
            format_id: kAudioFormatMPEGLayer3,
            format_flags: 0,
            bytes_per_packet: 0,
            frames_per_packet: 1152,
            bytes_per_frame: 0,
            channels_per_frame: 1,
            bits_per_channel: 0,
            _reserved: 0,
        };

        let mut mem = Mem::new();
        let data: MutPtr<u8> = mem.alloc(packets.byte_count() as GuestUSize).cast();
        mem.bytes_at_mut(data, packets.byte_count() as GuestUSize)
            .copy_from_slice(&packets.packet_bytes);

        let mut decoder = None;
        let mut frame_by_frame = Vec::new();
        for i in 0..packets.packet_count() {
            let (offset, size) = packets.packet_info(i).unwrap();
            let (al_format, rate, pcm) = decode_queue_buffer(
                &mut decoder,
                &mem,
                &format,
                data + offset as GuestUSize,
                size,
                &[],
            );
            assert_eq!((al_format, rate), (al::AL_FORMAT_MONO16, 44100));
            frame_by_frame.extend_from_slice(&pcm);
        }
        let frame_count = packets.packet_count() as usize * 1152;
        assert_eq!(frame_by_frame.len(), frame_count * 2);

        let mut decoder = None;
        let (_, _, all_at_once) = decode_queue_buffer(
            &mut decoder,
            &mem,
            &format,
            data,
            packets.byte_count() as GuestUSize,
            &[],
        );
        assert_eq!(frame_by_frame, all_at_once);

        // It should be the 440 Hz tone: count the zero crossings in the
        // middle of it, away from the encoder's delay and padding.
        let samples: Vec<i16> = frame_by_frame
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect();
        let middle = &samples[4410..4410 + 11025];
        let crossings = middle
            .windows(2)
            .filter(|w| (w[0] < 0) != (w[1] < 0))
            .count();
        assert!((215..=225).contains(&crossings), "{}", crossings);
    }

    #[test]
    fn remixes_channels() {
        let mut out = Vec::new();
        remix_i16(&[100, 300, -50, 50], 2, 1, &mut out);
        assert_eq!(out, [200, 0]);
        out.clear();
        remix_i16(&[7, -7], 1, 2, &mut out);
        assert_eq!(out, [7, 7, -7, -7]);
    }

    #[test]
    fn writes_requested_sample_formats() {
        let mut out = Vec::new();
        write_offline_pcm(&[1, -2], 2, &pcm_format(2, 16, 0), &mut out);
        assert_eq!(out, [1, 0, 0xfe, 0xff]);

        out.clear();
        write_offline_pcm(
            &[16384],
            1,
            &pcm_format(1, 32, kAudioFormatFlagIsFloat),
            &mut out,
        );
        assert_eq!(out, 0.5f32.to_le_bytes());

        // Mono source, stereo destination.
        out.clear();
        write_offline_pcm(&[3], 1, &pcm_format(2, 16, 0), &mut out);
        assert_eq!(out, [3, 0, 3, 0]);
    }
}
