/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `AudioUnit.h` (Audio Unit Services)

// Allow compiler to ignore warnings to prevent build failure on strict CI

use std::time::Instant;

use crate::audio::openal::al_types::{ALuint, ALvoid};
use crate::audio::openal::{AL_BUFFERS_PROCESSED, AL_PLAYING, AL_SOURCE_STATE};

use crate::abi::CallFromHost;
use crate::dyld::FunctionExports;
use crate::environment::Environment;
use crate::export_c_func;
use crate::frameworks::audio_toolbox::audio_components;
use crate::frameworks::audio_toolbox::audio_queue::{
    is_supported_audio_format, log_if_broken_audio_format,
};
use crate::frameworks::carbon_core::{paramErr, OSStatus};
use crate::frameworks::core_audio_types::AudioStreamBasicDescription;
use crate::frameworks::core_foundation::cf_run_loop::CFRunLoopGetMain;
use crate::frameworks::foundation::ns_run_loop;
use crate::mem::{guest_size_of, ConstVoidPtr, MutPtr, MutVoidPtr, SafeRead};
use crate::objc::nil;

use super::audio_components::{AURenderCallbackStruct, AudioComponentInstance};
use super::audio_queue::decode_buffer;
use super::audio_session;

pub type AudioUnit = AudioComponentInstance;
type AudioUnitPropertyID = u32;
type AudioUnitScope = u32;
type AudioUnitElement = u32;

// --- STRUCTURES ---

#[repr(C, packed)]
pub struct AudioBufferList<const COUNT: usize> {
    pub number_buffers: u32,
    pub buffers: [AudioBuffer; COUNT],
}
unsafe impl SafeRead for AudioBufferList<1> {}
unsafe impl SafeRead for AudioBufferList<2> {}

#[repr(C, packed)]
pub struct AudioBuffer {
    pub number_channels: u32,
    pub data_byte_size: u32,
    pub data: MutVoidPtr,
}

// -----------------------------------------------------

const kAudioUnitScope_Global: AudioUnitScope = 0;
const kAudioUnitScope_Input: AudioUnitScope = 1;
const kAudioUnitScope_Output: AudioUnitScope = 2;

const kAudioUnitProperty_SampleRate: AudioUnitPropertyID = 2;
const kAudioUnitProperty_SetRenderCallback: AudioUnitPropertyID = 23;
const kAudioUnitProperty_MaximumFramesPerSlice: AudioUnitPropertyID = 14;
const kAudioUnitProperty_StreamFormat: AudioUnitPropertyID = 8;

const kAudioOutputUnitProperty_EnableIO: AudioUnitPropertyID = 2003;

fn AudioUnitInitialize(env: &mut Environment, in_unit: AudioUnit) -> OSStatus {
    let run_loop = CFRunLoopGetMain(env);
    ns_run_loop::add_audio_unit(env, run_loop, in_unit);
    0
}

fn AudioUnitUninitialize(env: &mut Environment, in_unit: AudioUnit) -> OSStatus {
    let run_loop = CFRunLoopGetMain(env);
    match ns_run_loop::remove_audio_unit(env, run_loop, in_unit) {
        Ok(_) => 0,
        Err(_) => paramErr,
    }
}

fn AudioUnitSetProperty(
    env: &mut Environment,
    in_unit: AudioUnit,
    in_id: AudioUnitPropertyID,
    in_scope: AudioUnitScope,
    in_element: AudioUnitElement,
    in_data: ConstVoidPtr,
    in_data_size: u32,
) -> OSStatus {
    if in_element != 0 {
        log_dbg!(
            "AudioUnitSetProperty: ignoring non-zero element {}",
            in_element
        );
        return 0;
    }

    let Some(host_object) = audio_components::State::get(&mut env.framework_state)
        .audio_component_instances
        .get_mut(&in_unit)
    else {
        log_dbg!(
            "AudioUnitSetProperty: unknown audio unit {:?}, returning paramErr",
            in_unit
        );
        return paramErr;
    };

    let result;
    match in_id {
        kAudioUnitProperty_SetRenderCallback => {
            assert_eq!(in_scope, kAudioUnitScope_Global);
            assert_eq!(in_data_size, guest_size_of::<AURenderCallbackStruct>());
            let render_callback = env.mem.read(in_data.cast::<AURenderCallbackStruct>());
            host_object.render_callback = Some(render_callback);
            result = 0;
        }
        kAudioUnitProperty_StreamFormat => {
            assert_eq!(in_data_size, guest_size_of::<AudioStreamBasicDescription>());
            let stream_format = env.mem.read(in_data.cast::<AudioStreamBasicDescription>());
            log_if_broken_audio_format(&stream_format);
            match in_scope {
                kAudioUnitScope_Global => host_object.global_stream_format = stream_format,
                kAudioUnitScope_Output => host_object.output_stream_format = Some(stream_format),
                kAudioUnitScope_Input => host_object.input_stream_format = Some(stream_format),
                _ => unimplemented!("in_scope {}", in_scope),
            };
            result = 0;
        }
        kAudioOutputUnitProperty_EnableIO => {
            result = 0;
        }
        _ => {
            log_dbg!("AudioUnitSetProperty: unknown property {}", in_id);
            result = 0;
        }
    };

    result
}

fn AudioUnitGetProperty(
    env: &mut Environment,
    in_unit: AudioUnit,
    in_id: AudioUnitPropertyID,
    in_scope: AudioUnitScope,
    in_element: AudioUnitElement,
    out_data: MutVoidPtr,
    io_data_size: MutPtr<u32>,
) -> OSStatus {
    if in_element != 0 {
        log_dbg!(
            "AudioUnitGetProperty: ignoring non-zero element {}",
            in_element
        );
        return 0;
    }

    let Some(host_object) = audio_components::State::get(&mut env.framework_state)
        .audio_component_instances
        .get_mut(&in_unit)
    else {
        log_dbg!(
            "AudioUnitGetProperty: unknown audio unit {:?}, returning paramErr",
            in_unit
        );
        return paramErr;
    };

    match in_id {
        kAudioUnitProperty_MaximumFramesPerSlice => {
            assert_eq!(env.mem.read(io_data_size), guest_size_of::<u32>());
            let max_frames: u32 = host_object.maximum_frames_per_slice;
            env.mem.write(out_data.cast(), max_frames);
            env.mem.write(io_data_size.cast(), guest_size_of::<u32>());
        }
        kAudioUnitProperty_StreamFormat => {
            let stream_format = match in_scope {
                kAudioUnitScope_Global => host_object.global_stream_format,
                kAudioUnitScope_Output => match host_object.output_stream_format {
                    Some(f) => f,
                    None => host_object.global_stream_format,
                },
                kAudioUnitScope_Input => match host_object.input_stream_format {
                    Some(f) => f,
                    None => host_object.global_stream_format,
                },
                _ => unimplemented!(),
            };
            env.mem.write(out_data.cast(), stream_format);
            env.mem.write(io_data_size.cast(), guest_size_of::<AudioStreamBasicDescription>());
        }
        kAudioUnitProperty_SampleRate => {
            let sample_rate = match in_scope {
                kAudioUnitScope_Global => host_object.global_stream_format.sample_rate,
                _ => host_object.global_stream_format.sample_rate,
            };
            env.mem.write(out_data.cast(), sample_rate);
            env.mem.write(io_data_size.cast(), guest_size_of::<f64>());
        }
        _ => return -1,
    };
    0
}

fn AudioOutputUnitStart(env: &mut Environment, ci: AudioUnit) -> OSStatus {
    let context = env.framework_state.audio_toolbox.make_al_context_current(&mut env.openal_manager);
    let mut source: ALuint = 0;
    unsafe {
        context.GenSources(1, &mut source);
        context.SourcePlay(source);
    }

    let audio_components_state = audio_components::State::get(&mut env.framework_state);
    let Some(audio_unit_state) = audio_components_state
        .audio_component_instances
        .get_mut(&ci)
    else {
        log_dbg!("AudioOutputUnitStart: unknown audio unit {:?}", ci);
        return paramErr;
    };
    audio_unit_state.al_source = Some(source);
    audio_unit_state.last_render_time = Some(Instant::now());
    audio_unit_state.started = true;
    0
}

fn AudioOutputUnitStop(env: &mut Environment, ci: AudioUnit) -> OSStatus {
    let at_state = &mut env.framework_state.audio_toolbox;
    let context = at_state.al_context.make_al_context_current(&mut env.openal_manager);

    if let Some(audio_unit_state) = at_state.audio_components.audio_component_instances.get_mut(&ci) {
        audio_unit_state.started = false;
        if let Some(al_source) = audio_unit_state.al_source {
            unsafe { context.DeleteSources(1, &al_source); }
        }
        audio_unit_state.al_source = None;
        0
    } else {
        -1
    }
}

// STUB: For games calling AddRenderNotify (e.g. SimCity)
fn AudioUnitAddRenderNotify(
    _env: &mut Environment,
    in_unit: AudioUnit,
    in_proc: ConstVoidPtr,
    in_proc_ref_con: ConstVoidPtr,
) -> OSStatus {
    log_dbg!("STUB: AudioUnitAddRenderNotify called");
    0
}

pub fn render_audio_unit(env: &mut Environment, audio_unit: AudioUnit) {
    if env.bundle.bundle_identifier().starts_with("com.ea.simcity") {
        return;
    }

    // Collect all needed state into locals so the borrow on env ends
    // before we call back into the guest (which needs &mut env).
    let (
        current_hardware_sample_rate,
        started,
        is_running_handler,
        input_stream_format,
        output_stream_format,
        global_stream_format,
        al_source,
        last_render_time,
        render_callback,
    ) = {
        let at = &mut env.framework_state.audio_toolbox;
        let Some(obj) = at.audio_components
            .audio_component_instances
            .get_mut(&audio_unit)
        else {
            return;
        };
        (
            at.audio_session.current_hardware_sample_rate,
            obj.started,
            obj.is_running_handler,
            obj.input_stream_format,
            obj.output_stream_format,
            obj.global_stream_format,
            obj.al_source,
            obj.last_render_time,
            obj.render_callback,
        )
    };

    if !started || is_running_handler {
        return;
    }

    {
        let at = &mut env.framework_state.audio_toolbox;
        if let Some(obj) = at.audio_components
            .audio_component_instances
            .get_mut(&audio_unit)
        {
            obj.is_running_handler = true;
        } else {
            return;
        }
    }

    let stream_format = input_stream_format
        .unwrap_or(output_stream_format.unwrap_or(global_stream_format));
    let sample_rate = input_stream_format
        .map(|f| f.sample_rate)
        .unwrap_or(current_hardware_sample_rate);

    // Return early if the unit is not fully initialised yet.
    let Some(al_source) = al_source else { return; };
    let Some(last_render_time) = last_render_time else { return; };
    let Some(callback) = render_callback else { return; };
    let mut al_buffers = Vec::new();

    // Scope the context borrow so it ends before the guest callback.
    {
        let at = &mut env.framework_state.audio_toolbox;
        let context = at.al_context
            .make_al_context_current(&mut env.openal_manager);
        unsafe {
            let mut buffers_processed = 0;
            context.GetSourcei(
                al_source, AL_BUFFERS_PROCESSED, &mut buffers_processed,
            );
            while buffers_processed > 0 {
                let mut al_buffer = 0;
                context.SourceUnqueueBuffers(al_source, 1, &mut al_buffer);
                al_buffers.push(al_buffer);
                context.GetSourcei(
                    al_source, AL_BUFFERS_PROCESSED, &mut buffers_processed,
                );
            }
        }
    }

    let now = Instant::now();
    let elapsed_time = now.duration_since(last_render_time);
    let number_frames =
        ((elapsed_time.as_secs_f64() * sample_rate) as u32).min(2048);
    let bytes_per_chan = stream_format.bits_per_channel / 8;
    let buffer_size =
        number_frames * stream_format.channels_per_frame * bytes_per_chan;

    let action_flags = env.mem.alloc_and_write(0u32);
    let buffer_data = env.mem.alloc(buffer_size);
    let audio_buffer_list = AudioBufferList {
        number_buffers: 1,
        buffers: [AudioBuffer {
            number_channels: stream_format.channels_per_frame,
            data_byte_size: buffer_size,
            data: buffer_data,
        }],
    };
    let abl_ptr = env.mem.alloc_and_write(audio_buffer_list);

    // Copy fields from packed struct to locals — taking a reference to a
    // field of a packed struct is undefined behaviour (E0793).
    let input_proc = callback.input_proc;
    let input_proc_ref_con = callback.input_proc_ref_con;

    // Explicit return type so compiler can resolve GuestRet for R (E0283).
    let _: OSStatus = input_proc.call_from_host(
        env,
        (
            input_proc_ref_con,
            action_flags,
            nil.cast_void().cast_const(),
            0u32,
            number_frames,
            abl_ptr.cast::<AudioBufferList<1>>(),
        ),
    );

    let (al_format, _, processed_data) = decode_buffer(
        &env.mem, &stream_format, buffer_data.cast(), buffer_size,
    );

    // Re-acquire context after guest callback.
    {
        let at = &mut env.framework_state.audio_toolbox;
        let context = at.al_context
            .make_al_context_current(&mut env.openal_manager);
        unsafe {
            let al_buffer = al_buffers.pop().unwrap_or_else(|| {
                let mut b = 0;
                context.GenBuffers(1, &mut b);
                b
            });
            context.BufferData(
                al_buffer,
                al_format,
                processed_data.as_ptr() as *const ALvoid,
                processed_data.len() as i32,
                sample_rate as i32,
            );
            context.SourceQueueBuffers(al_source, 1, &al_buffer);
            let mut state = 0;
            context.GetSourcei(al_source, AL_SOURCE_STATE, &mut state);
            if state != AL_PLAYING {
                context.SourcePlay(al_source);
            }
            if !al_buffers.is_empty() {
                context.DeleteBuffers(
                    al_buffers.len() as i32, al_buffers.as_ptr(),
                );
            }
        }
    }

    env.mem.free(action_flags.cast_void());
    env.mem.free(buffer_data.cast_void());
    env.mem.free(abl_ptr.cast_void());

    if let Some(obj) = env.framework_state
        .audio_toolbox
        .audio_components
        .audio_component_instances
        .get_mut(&audio_unit)
    {
        obj.last_render_time = Some(now);
        obj.is_running_handler = false;
    }
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(AudioUnitInitialize(_)),
    export_c_func!(AudioUnitUninitialize(_)),
    export_c_func!(AudioUnitSetProperty(_, _, _, _, _, _)),
    export_c_func!(AudioUnitGetProperty(_, _, _, _, _, _)),
    export_c_func!(AudioOutputUnitStart(_)),
    export_c_func!(AudioOutputUnitStop(_)),
    export_c_func!(AudioUnitAddRenderNotify(_, _, _)),
];

