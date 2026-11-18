/*
 * Эта лицензия Source Code Form подпадает под условия Mozilla Public
 * License, v. 2.0.
 * Если копия MPL не распространялась вместе с этим
 * файлом, вы можете получить ее на https://mozilla.org/MPL/2.0/.
 */
//! `AudioUnit.h` (Audio Unit Services)

use std::time::Instant;

use crate::audio::openal::al_types::{ALsizei, ALuint, ALvoid};
use crate::audio::openal::{
    AL_BUFFERS_PROCESSED, AL_BUFFERS_QUEUED, AL_GAIN, AL_PITCH, AL_PLAYING, AL_SOURCE_STATE,
};

const AL_POSITION: i32 = 0x1004;
const AL_REFERENCE_DISTANCE: i32 = 0x1020;
const AL_ROLLOFF_FACTOR: i32 = 0x1021;
const AL_MAX_DISTANCE: i32 = 0x1023;

use crate::abi::{CallFromHost, GuestFunction};
use crate::dyld::FunctionExports;
use crate::environment::Environment;
use crate::export_c_func;
use crate::frameworks::audio_toolbox::au_graph;
use crate::frameworks::audio_toolbox::audio_components;
use crate::frameworks::audio_toolbox::audio_queue::{
    decode_buffer, decode_buffer_from_bytes, log_if_broken_audio_format,
};
use crate::frameworks::carbon_core::{paramErr, OSStatus};
use crate::frameworks::core_audio_types::{
    fourcc, kAudioFormatFlagIsBigEndian, kAudioFormatFlagIsFloat, kAudioFormatFlagIsNonInterleaved,
    kAudioFormatFlagIsPacked, kAudioFormatFlagIsSignedInteger, kAudioFormatLinearPCM,
    AudioStreamBasicDescription, AudioTimeStamp, SMPTETime,
};
use crate::frameworks::core_foundation::cf_run_loop::CFRunLoopGetMain;
use crate::frameworks::foundation::ns_dictionary::dict_from_keys_and_objects;
use crate::frameworks::foundation::ns_run_loop;
use crate::frameworks::foundation::ns_string;
use crate::mem::{guest_size_of, ConstVoidPtr, MutPtr, MutVoidPtr, SafeRead};
use crate::objc::{autorelease, id, msg, msg_class};

use super::audio_components::{AURenderCallback, AURenderCallbackStruct, AudioComponentInstance};

pub type AudioUnit = AudioComponentInstance;

fn interleave_planar_pcm(planes: &[&[u8]], bytes_per_sample: usize) -> Option<Vec<u8>> {
    let first = *planes.first()?;
    if bytes_per_sample == 0
        || first.len() % bytes_per_sample != 0
        || planes.iter().any(|plane| plane.len() != first.len())
    {
        return None;
    }

    let capacity = first.len().checked_mul(planes.len())?;
    let mut interleaved = Vec::with_capacity(capacity);
    for offset in (0..first.len()).step_by(bytes_per_sample) {
        for plane in planes {
            interleaved.extend_from_slice(&plane[offset..offset + bytes_per_sample]);
        }
    }
    Some(interleaved)
}

fn interleaved_format_for_planar_pcm(
    mut format: AudioStreamBasicDescription,
) -> AudioStreamBasicDescription {
    format.format_flags &= !kAudioFormatFlagIsNonInterleaved;
    format.bytes_per_frame *= format.channels_per_frame;
    format.bytes_per_packet *= format.channels_per_frame;
    format
}

fn uses_planar_stereo_buffers(
    format: &AudioStreamBasicDescription,
    has_input_format: bool,
) -> bool {
    let bytes_per_sample = format.bits_per_channel / 8;
    if format.format_id != kAudioFormatLinearPCM
        || format.channels_per_frame != 2
        || !matches!(format.bits_per_channel, 8 | 16 | 32)
        || format.bytes_per_frame != bytes_per_sample
        || format.format_flags & kAudioFormatFlagIsBigEndian != 0
    {
        return false;
    }

    if format.format_flags & kAudioFormatFlagIsNonInterleaved != 0 {
        return true;
    }

    let packed_signed_integer_flags = kAudioFormatFlagIsPacked | kAudioFormatFlagIsSignedInteger;
    let legacy_stereo = !has_input_format
        && format.bits_per_channel == 16
        && format.format_flags & packed_signed_integer_flags == packed_signed_integer_flags
        && format.format_flags & kAudioFormatFlagIsFloat == 0
        && format.bytes_per_packet == bytes_per_sample
        && format.frames_per_packet == 1;
    if legacy_stereo {
        log_once!("AudioUnit: treating under-reported packed stereo PCM as separate output planes");
    }
    legacy_stereo
}

fn audio_unit_render_sample_rate(
    format: &AudioStreamBasicDescription,
    hardware_sample_rate: f64,
) -> f64 {
    if format.sample_rate.is_finite() && format.sample_rate > 0.0 {
        format.sample_rate
    } else if hardware_sample_rate.is_finite() && hardware_sample_rate > 0.0 {
        hardware_sample_rate
    } else {
        44_100.0
    }
}

type AudioUnitPropertyID = u32;
type AudioUnitScope = u32;
type AudioUnitElement = u32;
type AudioUnitParameterID = u32;
type AudioUnitParameterValue = f32;
type AudioUnitPropertyListenerProc = GuestFunction;

// =========================================================================
// MARK: - Структуры
// =========================================================================

#[repr(C, packed)]
pub struct AudioBufferList<const COUNT: usize> {
    pub number_buffers: u32,
    pub buffers: [AudioBuffer; COUNT],
}
unsafe impl SafeRead for AudioBuffer {}
unsafe impl SafeRead for AudioBufferList<1> {}
unsafe impl SafeRead for AudioBufferList<2> {}

#[repr(C, packed)]
pub struct AudioBuffer {
    pub number_channels: u32,
    pub data_byte_size: u32,
    pub data: MutVoidPtr,
}

/// `AudioUnitConnection` — используется для kAudioUnitProperty_MakeConnection.
#[repr(C, packed)]
#[derive(Copy, Clone)]
struct AudioUnitConnection {
    source_audio_unit: AudioUnit,
    source_output_number: u32,
    dest_input_number: u32,
}
unsafe impl SafeRead for AudioUnitConnection {}

// =========================================================================
// MARK: - Константы Scope / element
// =========================================================================

const kAudioUnitScope_Global: AudioUnitScope = 0;
const kAudioUnitScope_Input: AudioUnitScope = 1;
const kAudioUnitScope_Output: AudioUnitScope = 2;
const kAudioUnitScope_Group: AudioUnitScope = 3;
const kAudioUnitScope_Part: AudioUnitScope = 4;
const kAudioUnitScope_Note: AudioUnitScope = 5;

// =========================================================================
// MARK: - Константы Property ID
// =========================================================================

const kAudioUnitProperty_ClassInfo: AudioUnitPropertyID = 0;
const kAudioUnitProperty_MakeConnection: AudioUnitPropertyID = 1;
const kAudioUnitProperty_SampleRate: AudioUnitPropertyID = 2;
const kAudioUnitProperty_ParameterList: AudioUnitPropertyID = 3;
const kAudioUnitProperty_ParameterInfo: AudioUnitPropertyID = 4;
const kAudioUnitProperty_CPULoad: AudioUnitPropertyID = 6;
const kAudioUnitProperty_StreamFormat: AudioUnitPropertyID = 8;
const kAudioUnitProperty_ElementCount: AudioUnitPropertyID = 11;
const kAudioUnitProperty_Latency: AudioUnitPropertyID = 12;
const kAudioUnitProperty_SupportedNumChannels: AudioUnitPropertyID = 13;
const kAudioUnitProperty_MaximumFramesPerSlice: AudioUnitPropertyID = 14;
const kAudioUnitProperty_ParameterValueStrings: AudioUnitPropertyID = 16;
const kAudioUnitProperty_AudioChannelLayout: AudioUnitPropertyID = 19;
const kAudioUnitProperty_TailTime: AudioUnitPropertyID = 20;
const kAudioUnitProperty_BypassEffect: AudioUnitPropertyID = 21;
const kAudioUnitProperty_LastRenderError: AudioUnitPropertyID = 22;
const kAudioUnitProperty_SetRenderCallback: AudioUnitPropertyID = 23;
const kAudioUnitProperty_FactoryPresets: AudioUnitPropertyID = 24;
const kAudioUnitProperty_RenderQuality: AudioUnitPropertyID = 26;
const kAudioUnitProperty_HostCallbacks: AudioUnitPropertyID = 27;
const kAudioUnitProperty_InPlaceProcessing: AudioUnitPropertyID = 29;
const kAudioUnitProperty_ElementName: AudioUnitPropertyID = 30;
const kAudioUnitProperty_SupportedChannelLayoutTags: AudioUnitPropertyID = 32;
const kAudioUnitProperty_PresentPreset: AudioUnitPropertyID = 36;
const kAudioUnitProperty_DependentParameters: AudioUnitPropertyID = 45;
const kAudioUnitProperty_InputSamplesInOutput: AudioUnitPropertyID = 49;
const kAudioUnitProperty_ShouldAllocateBuffer: AudioUnitPropertyID = 51;
const kAudioUnitProperty_FrequencyResponse: AudioUnitPropertyID = 52;
const kAudioUnitProperty_ParameterHistoryInfo: AudioUnitPropertyID = 53;
const kAudioUnitProperty_NickName: AudioUnitPropertyID = 54;
const kAudioUnitProperty_OfflineRender: AudioUnitPropertyID = 37;
const kAudioUnitProperty_ParameterIDName: AudioUnitPropertyID = 34;
const kAudioOutputUnitProperty_EnableIO: AudioUnitPropertyID = 2003;
const kAudioOutputUnitProperty_HasIO: AudioUnitPropertyID = 2006;
const kAudioOutputUnitProperty_StartTime: AudioUnitPropertyID = 2004;
const kAudioOutputUnitProperty_SetInputCallback: AudioUnitPropertyID = 2005;
const kAudioOutputUnitProperty_IsRunning: AudioUnitPropertyID = 2001;
const kAudioMixerProperty_Volume: AudioUnitPropertyID = 7;
const kAudioMixerProperty_Metering: AudioUnitPropertyID = 1003;
const kAudioUnitProperty_MeteringMode: AudioUnitPropertyID = 1003;

// 3D Mixer Property IDs
const kAudioUnitProperty_3DMixerDistanceParams: AudioUnitPropertyID = fourcc(b"3ddp");
const kAudioUnitProperty_MatrixLevels: AudioUnitPropertyID = fourcc(b"mxmv");
const kAudioUnitProperty_SpatializationAlgorithm: AudioUnitPropertyID = fourcc(b"spat");
const kAudioUnitProperty_3DMixerRenderingFlags: AudioUnitPropertyID = fourcc(b"3drf");

// 3D Mixer Parameter IDs
const k3DMixerParam_Azimuth: AudioUnitParameterID = 0;
const k3DMixerParam_Elevation: AudioUnitParameterID = 1;
const k3DMixerParam_Distance: AudioUnitParameterID = 2;
const k3DMixerParam_Gain: AudioUnitParameterID = 3;
const k3DMixerParam_PlaybackRate: AudioUnitParameterID = 4;

// =========================================================================
// MARK: - Инициализация / Деинициализация AudioUnit
// =========================================================================

fn AudioUnitInitialize(env: &mut Environment, in_unit: AudioUnit) -> OSStatus {
    log_dbg!("AudioUnitInitialize({:?})", in_unit);
    if let Some(unit) = audio_components::State::get(&mut env.framework_state)
        .audio_component_instances
        .get_mut(&in_unit)
    {
        unit.initialized = true;
    }
    let run_loop = CFRunLoopGetMain(env);
    ns_run_loop::add_audio_unit(env, run_loop, in_unit);
    0
}

fn AudioUnitUninitialize(env: &mut Environment, in_unit: AudioUnit) -> OSStatus {
    if let Some(unit) = audio_components::State::get(&mut env.framework_state)
        .audio_component_instances
        .get_mut(&in_unit)
    {
        unit.initialized = false;
    }
    let run_loop = CFRunLoopGetMain(env);
    match ns_run_loop::remove_audio_unit(env, run_loop, in_unit) {
        Ok(_) => 0,
        Err(_) => paramErr,
    }
}

/// Notify listeners registered for one Audio Unit property.
///
/// Take a snapshot before entering guest code. A property listener is allowed
/// to add or remove listeners (or even dispose the unit) re-entrantly, so the
/// host-side state must not remain borrowed while the callback is running.
fn notify_audio_unit_property(
    env: &mut Environment,
    in_unit: AudioUnit,
    in_id: AudioUnitPropertyID,
    in_scope: AudioUnitScope,
    in_element: AudioUnitElement,
) {
    let listeners = audio_components::State::get(&mut env.framework_state)
        .audio_component_instances
        .get(&in_unit)
        .map(|host_object| {
            host_object
                .property_listeners
                .iter()
                .filter(|(property_id, _, _)| *property_id == in_id)
                .copied()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    for (_, callback, ref_con) in listeners {
        let _: OSStatus =
            callback.call_from_host(env, (ref_con, in_unit, in_id, in_scope, in_element));
    }
}

/// Notify the standard running-state property used by output units and
/// AUGraph. Kept crate-visible so `au_graph` can report graph-driven stops too.
pub(crate) fn notify_audio_unit_is_running(env: &mut Environment, in_unit: AudioUnit) {
    notify_audio_unit_property(
        env,
        in_unit,
        kAudioOutputUnitProperty_IsRunning,
        kAudioUnitScope_Global,
        0,
    );
}

/// `AudioUnitAddPropertyListener` stores the guest callback instead of
/// installing the generic return-0 stub. This is particularly important for
/// `kAudioOutputUnitProperty_IsRunning`: many RemoteIO clients use this
/// listener to observe a start/stop transition.
fn AudioUnitAddPropertyListener(
    env: &mut Environment,
    in_unit: AudioUnit,
    in_id: AudioUnitPropertyID,
    in_proc: AudioUnitPropertyListenerProc,
    in_proc_ref_con: MutVoidPtr,
) -> OSStatus {
    log_dbg!(
        "AudioUnitAddPropertyListener(unit={:?}, property={}, proc={:?}, ref_con={:?})",
        in_unit,
        in_id,
        in_proc,
        in_proc_ref_con
    );

    if in_unit.is_null() || in_proc.to_ptr().is_null() {
        return paramErr;
    }

    let Some(host_object) = audio_components::State::get(&mut env.framework_state)
        .audio_component_instances
        .get_mut(&in_unit)
    else {
        return paramErr;
    };

    host_object
        .property_listeners
        .push((in_id, in_proc, in_proc_ref_con));
    0
}

fn AudioUnitRemovePropertyListener(
    env: &mut Environment,
    in_unit: AudioUnit,
    in_id: AudioUnitPropertyID,
    in_proc: AudioUnitPropertyListenerProc,
) -> OSStatus {
    log_dbg!(
        "AudioUnitRemovePropertyListener(unit={:?}, property={}, proc={:?})",
        in_unit,
        in_id,
        in_proc
    );

    if in_unit.is_null() {
        return paramErr;
    }

    let Some(host_object) = audio_components::State::get(&mut env.framework_state)
        .audio_component_instances
        .get_mut(&in_unit)
    else {
        return paramErr;
    };

    // Нативный API (без userData) удаляет первую регистрацию, совпавшую по
    // (property, proc). Удаляется только одно совпадение: один и тот же
    // колбэк может быть зарегистрирован несколько раз.
    if let Some(index) = host_object
        .property_listeners
        .iter()
        .position(|&(property_id, callback, _)| property_id == in_id && callback == in_proc)
    {
        host_object.property_listeners.remove(index);
    }

    0
}

/// `AudioUnitRemovePropertyListenerWithUserData` — как
/// `AudioUnitRemovePropertyListener`, но регистрация считается своей только
/// при полном совпадении (property, proc, userData), как при
/// `AudioUnitAddPropertyListener`.
fn AudioUnitRemovePropertyListenerWithUserData(
    env: &mut Environment,
    in_unit: AudioUnit,
    in_id: AudioUnitPropertyID,
    in_proc: AudioUnitPropertyListenerProc,
    in_proc_ref_con: MutVoidPtr,
) -> OSStatus {
    log_dbg!(
        "AudioUnitRemovePropertyListenerWithUserData(unit={:?}, property={}, proc={:?}, \
         ref_con={:?})",
        in_unit,
        in_id,
        in_proc,
        in_proc_ref_con
    );

    if in_unit.is_null() {
        return paramErr;
    }

    let Some(host_object) = audio_components::State::get(&mut env.framework_state)
        .audio_component_instances
        .get_mut(&in_unit)
    else {
        return paramErr;
    };

    // Удаляется только одно полное совпадение (см. AudioUnitAddPropertyListener).
    if let Some(index) = host_object
        .property_listeners
        .iter()
        .position(|&(property_id, callback, ref_con)| {
            property_id == in_id && callback == in_proc && ref_con == in_proc_ref_con
        })
    {
        host_object.property_listeners.remove(index);
    }

    0
}

// =========================================================================
// MARK: - Установка свойств AudioUnit
// =========================================================================

fn AudioUnitSetProperty(
    env: &mut Environment,
    in_unit: AudioUnit,
    in_id: AudioUnitPropertyID,
    in_scope: AudioUnitScope,
    in_element: AudioUnitElement,
    in_data: ConstVoidPtr,
    in_data_size: u32,
) -> OSStatus {
    log_dbg!(
        "AudioUnitSetProperty(unit={:?}, prop={}, scope={}, element={}, \
         data={:?}, size={})",
        in_unit,
        in_id,
        in_scope,
        in_element,
        in_data,
        in_data_size
    );
    let mut update_al_distance = None;
    let mut make_connection = None;
    let mut set_up_running_unit = false;

    // Ограничиваем область видимости заимствования
    {
        let Some(host_object) = audio_components::State::get(&mut env.framework_state)
            .audio_component_instances
            .get_mut(&in_unit)
        else {
            return paramErr;
        };

        match in_id {
            kAudioUnitProperty_3DMixerDistanceParams => {
                let params = env
                    .mem
                    .read::<audio_components::MixerDistanceParams, false>(in_data.cast());
                let bus = host_object.mixer_buses.entry(in_element).or_default();
                bus.distance_params = params;

                // Сохраняем значения для OpenAL, чтобы применить их после
                // завершения borrow
                if let Some(source) = bus.al_source {
                    update_al_distance = Some((source, params));
                }
            }
            kAudioUnitProperty_MatrixLevels => {
                log_dbg!(
                    "Заглушка для kAudioUnitProperty_MatrixLevels \
                     на шине {}",
                    in_element
                );
            }
            kAudioUnitProperty_SpatializationAlgorithm
            | kAudioUnitProperty_3DMixerRenderingFlags => {
                log_dbg!(
                    "AudioUnitSetProperty: флаги \
                     spatialization/rendering проигнорированы"
                );
            }
            kAudioUnitProperty_SetRenderCallback => {
                let render_callback = env
                    .mem
                    .read::<AURenderCallbackStruct, false>(in_data.cast());
                // A NULL procedure removes the callback.
                let callback = if render_callback.input_proc.addr_with_thumb_bit() == 0 {
                    None
                } else {
                    Some(render_callback)
                };
                if in_scope == kAudioUnitScope_Input {
                    let bus = host_object.mixer_buses.entry(in_element).or_default();
                    bus.render_callback = callback;
                } else {
                    host_object.render_callback = callback;
                }
                // A unit that is already rendering needs an OpenAL source for
                // the new callback.
                set_up_running_unit = callback.is_some() && host_object.started;
                let proc_copy = render_callback.input_proc;
                let ref_con_copy = render_callback.input_proc_ref_con;
                log_dbg!(
                    "AudioUnitSetProperty(SetRenderCallback) \
                     unit={:?} scope={} element={} proc={:?} ref_con={:?}",
                    in_unit,
                    in_scope,
                    in_element,
                    proc_copy,
                    ref_con_copy
                );
            }
            kAudioOutputUnitProperty_SetInputCallback => {
                let cb = env
                    .mem
                    .read::<AURenderCallbackStruct, false>(in_data.cast());
                host_object.render_callback = Some(cb);
                let proc_copy = cb.input_proc;
                let ref_con_copy = cb.input_proc_ref_con;
                log_dbg!(
                    "AudioUnitSetProperty(SetInputCallback) \
                     unit={:?} scope={} element={} proc={:?} ref_con={:?}",
                    in_unit,
                    in_scope,
                    in_element,
                    proc_copy,
                    ref_con_copy
                );
            }
            kAudioUnitProperty_StreamFormat => {
                let stream_format = env
                    .mem
                    .read::<AudioStreamBasicDescription, false>(in_data.cast());
                log_if_broken_audio_format(&stream_format);
                let (sf_id, sf_sr, sf_ch, sf_bc, sf_bpf, sf_flags) = (
                    stream_format.format_id,
                    stream_format.sample_rate,
                    stream_format.channels_per_frame,
                    stream_format.bits_per_channel,
                    stream_format.bytes_per_frame,
                    stream_format.format_flags,
                );
                log_dbg!(
                    "AudioUnitSetProperty(StreamFormat) \
                     unit={:?} scope={} element={} \
                     format_id=0x{:x} sr={} ch={} bits={} bpf={} flags=0x{:x}",
                    in_unit,
                    in_scope,
                    in_element,
                    sf_id,
                    sf_sr,
                    sf_ch,
                    sf_bc,
                    sf_bpf,
                    sf_flags
                );
                match in_scope {
                    kAudioUnitScope_Global => {
                        host_object.global_stream_format = stream_format;
                    }
                    kAudioUnitScope_Output => {
                        host_object.output_stream_format = Some(stream_format);
                    }
                    kAudioUnitScope_Input => {
                        host_object.input_stream_format = Some(stream_format);
                        // Для 3D Mixer: формат шины N задаётся
                        // scope=Input, element=N.
                        let bus = host_object.mixer_buses.entry(in_element).or_default();
                        bus.stream_format = Some(stream_format);
                    }
                    _ => log_dbg!(
                        "AudioUnitSetProperty StreamFormat: \
                         неподдерживаемая область (scope) {}",
                        in_scope
                    ),
                }
            }
            kAudioUnitProperty_SampleRate => {
                let rate: f64 = env.mem.read::<f64, false>(in_data.cast());
                host_object.global_stream_format.sample_rate = rate;
            }
            kAudioUnitProperty_MaximumFramesPerSlice => {
                let frames: u32 = env.mem.read::<u32, false>(in_data.cast());
                host_object.maximum_frames_per_slice = frames;
            }
            kAudioUnitProperty_MakeConnection => {
                let conn = env.mem.read::<AudioUnitConnection, false>(in_data.cast());
                let src_unit = conn.source_audio_unit;
                let src_out = conn.source_output_number;
                let dst_in = conn.dest_input_number;
                log_dbg!(
                    "AudioUnitSetProperty(MakeConnection) \
                     dest_unit={:?} dest_input={} \
                     src_unit={:?} src_output={}",
                    in_unit,
                    dst_in,
                    src_unit,
                    src_out
                );
                make_connection = Some((dst_in, src_unit));
            }
            kAudioOutputUnitProperty_EnableIO => {
                let enabled: u32 = env.mem.read::<u32, false>(in_data.cast());
                log_dbg!(
                    "AudioUnitSetProperty(EnableIO) \
                     unit={:?} scope={} element={} enabled={}",
                    in_unit,
                    in_scope,
                    in_element,
                    enabled
                );
                // Track RemoteIO input enablement (scope=Input, element=1) so
                // AudioUnitRender can fill the guest's buffers with real mic
                // samples from the host.
                if in_scope == kAudioUnitScope_Input && in_element == 1 {
                    if let Some(obj) = env
                        .framework_state
                        .audio_toolbox
                        .audio_components
                        .audio_component_instances
                        .get_mut(&in_unit)
                    {
                        obj.mic_input_enabled = enabled != 0;
                    }
                    // Start/stop the host mic stream so read_mic_chunk()
                    // actually delivers PCM (games enable input only when the
                    // user starts recording, so this maps 1:1).
                    if enabled != 0 {
                        crate::android_media::start_mic();
                    } else {
                        crate::android_media::stop_mic();
                    }
                }
            }
            kAudioUnitProperty_ElementCount => {
                // Apple docs: kAudioUnitProperty_ElementCount (11)
                // Sets the number of input or output buses (elements) on a
                // multi-bus audio unit such as the 3D Mixer or Matrix Mixer.
                // scope=1 (Input) sets how many input buses exist;
                // scope=2 (Output) sets output bus count.
                // We pre-allocate the requested number of MixerBusState
                // entries so that subsequent per-bus Set/Get calls find
                // an existing entry rather than creating one on the fly.
                let count: u32 = env.mem.read::<u32, false>(in_data.cast());
                log_dbg!(
                    "AudioUnitSetProperty(ElementCount) \
                     unit={:?} scope={} element={} count={}",
                    in_unit,
                    in_scope,
                    in_element,
                    count
                );
                // Only the Input scope bus-count is meaningful for the
                // 3D Mixer / MultiChannelMixer.  Pre-populate entries so
                // that subsequent per-bus property calls find an existing slot.
                if in_scope == kAudioUnitScope_Input {
                    for bus_idx in 0..count {
                        host_object.mixer_buses.entry(bus_idx).or_default();
                    }
                }
                // Output / Global element counts are accepted silently —
                // there is nothing extra to initialise on our side.
            }
            kAudioUnitProperty_AudioChannelLayout => {
                // Сохраняем сырые байты макета каналов (например,
                // 32-байтовый layout с одним AudioChannelDescription) —
                // возвращаем их затем через AudioUnitGetProperty.
                if !in_data.is_null() && in_data_size > 0 {
                    let bytes = env
                        .mem
                        .bytes_at(in_data.cast(), in_data_size)
                        .to_vec();
                    host_object
                        .audio_channel_layouts
                        .insert((in_scope, in_element), bytes);
                } else {
                    host_object
                        .audio_channel_layouts
                        .remove(&(in_scope, in_element));
                }
            }
            kAudioUnitProperty_ShouldAllocateBuffer => {
                // kAudioUnitProperty_ShouldAllocateBuffer (51): UInt32-флаг
                // на (scope, element) — хост просит/не просит, чтобы юнит
                // сам выделял буферы. Запоминаем, чтобы GET вернул то же.
                if !in_data.is_null() && in_data_size >= guest_size_of::<u32>() {
                    let flag: u32 = env.mem.read::<u32, false>(in_data.cast());
                    host_object
                        .should_allocate_buffers
                        .insert((in_scope, in_element), flag);
                }
            }
            _ => {
                log!(
                    "AudioUnitSetProperty: UNHANDLED property {} \
                     (unit={:?}, scope={}, element={}, size={})",
                    in_id,
                    in_unit,
                    in_scope,
                    in_element,
                    in_data_size
                );
            }
        }
    } // Конец заимствования host_object и env.framework_state

    // Теперь безопасно вызываем OpenAL
    if let Some((source, params)) = update_al_distance {
        let context = env
            .framework_state
            .audio_toolbox
            .make_al_context_current(&mut env.openal_manager);
        unsafe {
            context.Sourcef(source, AL_REFERENCE_DISTANCE, params.reference_distance);
            context.Sourcef(source, AL_MAX_DISTANCE, params.maximum_distance);
            context.Sourcef(source, AL_ROLLOFF_FACTOR, params.rolloff_factor);
        }
    }

    if set_up_running_unit {
        setup_audio_unit_for_render(env, in_unit);
    }
    if let Some((dest_input, source)) = make_connection {
        connect_audio_units(env, in_unit, dest_input, source);
    }

    // Audio Unit Services notifies property listeners synchronously after a
    // successful property write. The listener snapshot is taken by the
    // helper, so callbacks may safely re-enter Audio Unit Services.
    notify_audio_unit_property(env, in_unit, in_id, in_scope, in_element);

    0
}

// =========================================================================
// MARK: - Connections (kAudioUnitProperty_MakeConnection)
// =========================================================================
//
// Every unit with a render callback is played through OpenAL sources of its
// own and OpenAL mixes them, so a connection doesn't carry audio. What it
// decides is whether a unit is pulled at all: on iOS a unit only renders while
// a running output unit pulls on it, directly or through other units (e.g.
// per-voice converters feeding a mixer feeding RemoteIO, as built by hand in
// Plants vs. Zombies HD instead of with an AUGraph).

/// The units connected to the inputs of `unit`.
fn units_feeding(env: &mut Environment, unit: AudioUnit) -> Vec<AudioUnit> {
    audio_components::State::get(&mut env.framework_state)
        .connections
        .iter()
        .filter(|((dest, _), _)| *dest == unit)
        .map(|(_, src)| *src)
        .collect()
}

/// Whether `unit` is, or feeds into, an output unit that has been started.
fn feeds_running_output(env: &mut Environment, unit: AudioUnit) -> bool {
    let state = audio_components::State::get(&mut env.framework_state);
    let mut pending = vec![unit];
    let mut visited = Vec::new();
    while let Some(unit) = pending.pop() {
        if visited.contains(&unit) {
            continue;
        }
        visited.push(unit);
        let is_running_output = state
            .audio_component_instances
            .get(&unit)
            .is_some_and(|obj| {
                obj.started
                    && obj
                        .component_desc
                        .is_some_and(|(component_type, _, _)| component_type == fourcc(b"auou"))
            });
        if is_running_output {
            return true;
        }
        pending.extend(
            state
                .connections
                .iter()
                .filter(|(_, src)| **src == unit)
                .map(|((dest, _), _)| *dest),
        );
    }
    false
}

/// Start or stop rendering every unit feeding `unit`.
fn set_upstream_units_running(env: &mut Environment, unit: AudioUnit, running: bool) {
    let sources = units_feeding(env, unit);
    set_units_running(env, sources, running, 0);
}

/// Start or stop rendering `units` and every unit feeding them.
fn set_units_running(env: &mut Environment, units: Vec<AudioUnit>, running: bool, depth: u32) {
    // Core Audio rejects connection cycles; don't loop forever on one anyway.
    if depth > 16 {
        return;
    }
    for unit in units {
        if running {
            let needs_start = audio_components::State::get(&mut env.framework_state)
                .audio_component_instances
                .get(&unit)
                .is_some_and(|obj| {
                    !obj.started
                        && (obj.render_callback.is_some()
                            || obj.mixer_buses.values().any(|bus| bus.render_callback.is_some()))
                });
            if needs_start {
                setup_audio_unit_for_render(env, unit);
            }
        } else if let Some(obj) = audio_components::State::get(&mut env.framework_state)
            .audio_component_instances
            .get_mut(&unit)
        {
            // Stop pulling on the callbacks. The OpenAL sources are kept so
            // that what is already queued plays out, and are reused when the
            // unit is connected again.
            obj.started = false;
            obj.last_render_time = None;
            for bus in obj.mixer_buses.values_mut() {
                bus.last_render_time = None;
            }
        }
        let sources = units_feeding(env, unit);
        set_units_running(env, sources, running, depth + 1);
    }
}

fn connect_audio_units(env: &mut Environment, dest: AudioUnit, dest_input: u32, source: AudioUnit) {
    let connections = &mut audio_components::State::get(&mut env.framework_state).connections;
    let old_source = if source.is_null() {
        connections.remove(&(dest, dest_input))
    } else {
        connections.insert((dest, dest_input), source)
    };
    if let Some(old_source) = old_source.filter(|&old| old != source) {
        if !feeds_running_output(env, old_source) {
            set_units_running(env, vec![old_source], false, 0);
        }
    }
    if !source.is_null() && feeds_running_output(env, dest) {
        set_units_running(env, vec![source], true, 0);
    }
    if !source.is_null() {
        apply_mixer_bus_gain_and_rate(env, dest, dest_input);
    }
}

/// Apply a mixer input bus's gain and playback rate to the OpenAL sources
/// playing it: the bus's own (if it has a render callback) and those of the
/// unit connected to it. Real mixers apply them while mixing, but here each
/// input is mixed by OpenAL.
fn apply_mixer_bus_gain_and_rate(env: &mut Environment, mixer: AudioUnit, bus_id: u32) {
    let state = audio_components::State::get(&mut env.framework_state);
    let Some(bus) = state
        .audio_component_instances
        .get(&mixer)
        .and_then(|obj| obj.mixer_buses.get(&bus_id))
    else {
        return;
    };
    let (gain, rate) = (bus.volume, bus.playback_rate);
    let mut sources: Vec<ALuint> = bus.al_source.into_iter().collect();
    if let Some(source_unit) = state
        .connections
        .get(&(mixer, bus_id))
        .and_then(|unit| state.audio_component_instances.get(unit))
    {
        sources.extend(source_unit.al_source);
        sources.extend(
            source_unit
                .mixer_buses
                .values()
                .filter_map(|bus| bus.al_source),
        );
    }
    if sources.is_empty() {
        return;
    }
    let context = env
        .framework_state
        .audio_toolbox
        .make_al_context_current(&mut env.openal_manager);
    for source in sources {
        unsafe {
            context.Sourcef(source, AL_GAIN, gain.max(0.0));
            // OpenAL accepts any positive pitch; Core Audio clamps the rate
            // to 0.5-2.
            context.Sourcef(source, AL_PITCH, rate.clamp(0.5, 2.0));
        }
    }
}

// =========================================================================
// MARK: - Получение свойств AudioUnit
// =========================================================================

/// Вспомогательная функция: безопасная запись значения в гостевую память.
/// Если указатель нулевой — запись пропускается (API допускает NULL).
fn write_if_nonnull<T: crate::mem::SafeWrite>(env: &mut Environment, ptr: MutPtr<T>, value: T) {
    if !ptr.is_null() {
        env.mem.write(ptr, value);
    }
}

/// NSNumber (int32) для построения CFDictionary вроде kAudioUnitProperty_ClassInfo.
/// Возвращает автоназначенный объект: словарь при вставке сделает retain.
fn ns_number_from_i32(env: &mut Environment, value: i32) -> id {
    let num: id = msg_class![env; NSNumber alloc];
    let num = msg![env; num initWithInt:value];
    autorelease(env, num)
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
    log_dbg!(
        "AudioUnitGetProperty(unit={:?}, prop={}, scope={}, element={})",
        in_unit,
        in_id,
        in_scope,
        in_element
    );
    // Описание компонента (type, subtype, manufacturer) инстанса. Нужно для
    // kAudioUnitProperty_ClassInfo; при отсутствии инстанса — как и раньше,
    // сразу возвращаем paramErr.
    let component_desc = {
        let state = audio_components::State::get(&mut env.framework_state);
        match state.audio_component_instances.get(&in_unit) {
            Some(host) => host.component_desc,
            None => return paramErr,
        }
    };
    if in_id == kAudioUnitProperty_ClassInfo {
        // Возвращаем CFDictionaryRef (правило Create — гость сам вызывает
        // CFRelease): словарь-идентификатор юнита, как в AUPreset'е.
        // Обрабатываем до заимствования host_object, т.к. построение
        // NSDictionary требует полного &mut env.
        let (ctype, csub, cman) = component_desc.unwrap_or((0, 0, 0));
        if !out_data.is_null() {
            // Вложенные вызовы с &mut Environment запрещены — сначала
            // создаём объект, затем отдельно отдаём его в autorelease.
            let key_type = ns_string::from_rust_string(env, "type".to_string());
            let key_type = autorelease(env, key_type);
            let key_subtype = ns_string::from_rust_string(env, "subtype".to_string());
            let key_subtype = autorelease(env, key_subtype);
            let key_manufacturer = ns_string::from_rust_string(env, "manufacturer".to_string());
            let key_manufacturer = autorelease(env, key_manufacturer);
            let val_type = ns_number_from_i32(env, ctype as i32);
            let val_subtype = ns_number_from_i32(env, csub as i32);
            let val_manufacturer = ns_number_from_i32(env, cman as i32);
            let dict = dict_from_keys_and_objects(
                env,
                &[
                    (key_type, val_type),
                    (key_subtype, val_subtype),
                    (key_manufacturer, val_manufacturer),
                ],
            );
            write_if_nonnull(env, out_data.cast(), dict);
        }
        write_if_nonnull(env, io_data_size, guest_size_of::<MutVoidPtr>());
        return 0;
    }

    let Some(host_object) = audio_components::State::get(&mut env.framework_state)
        .audio_component_instances
        .get_mut(&in_unit)
    else {
        return paramErr;
    };

    match in_id {
        kAudioUnitProperty_MaximumFramesPerSlice => {
            let v = host_object.maximum_frames_per_slice;
            write_if_nonnull(env, out_data.cast(), v);
            write_if_nonnull(env, io_data_size, guest_size_of::<u32>());
        }
        kAudioUnitProperty_StreamFormat => {
            // Для scope=Input сначала смотрим per-bus формат
            // (element=N соответствует шине N у MultiChannelMixer/3DMixer),
            // затем input_stream_format, затем global_stream_format.
            // Для любых других scope — аналогично, но без bus-lookup.
            let fmt = match in_scope {
                kAudioUnitScope_Input => host_object
                    .mixer_buses
                    .get(&in_element)
                    .and_then(|b| b.stream_format)
                    .or(host_object.input_stream_format)
                    .unwrap_or(host_object.global_stream_format),
                kAudioUnitScope_Output => host_object
                    .output_stream_format
                    .unwrap_or(host_object.global_stream_format),
                _ => host_object.global_stream_format,
            };
            // out_data может быть NULL — в таком случае игра просто
            // запрашивает размер (см. документацию AudioUnitGetProperty).
            write_if_nonnull(env, out_data.cast(), fmt);
            write_if_nonnull(
                env,
                io_data_size,
                guest_size_of::<AudioStreamBasicDescription>(),
            );
        }
        kAudioUnitProperty_SampleRate => {
            let rate = host_object.global_stream_format.sample_rate;
            write_if_nonnull(env, out_data.cast(), rate);
            write_if_nonnull(env, io_data_size, guest_size_of::<f64>());
        }
        kAudioUnitProperty_ElementCount => {
            // Возвращаем количество шин микшера или 1 как дефолт.
            let count = if !host_object.mixer_buses.is_empty() {
                host_object.mixer_buses.len() as u32
            } else {
                1u32
            };
            write_if_nonnull(env, out_data.cast(), count);
            write_if_nonnull(env, io_data_size, guest_size_of::<u32>());
        }
        kAudioOutputUnitProperty_IsRunning => {
            let running: u32 = if host_object.started { 1 } else { 0 };
            write_if_nonnull(env, out_data.cast(), running);
            write_if_nonnull(env, io_data_size, guest_size_of::<u32>());
        }
        kAudioUnitProperty_Latency => {
            // Возвращаем нулевую задержку как заглушку.
            write_if_nonnull(env, out_data.cast(), 0.0f64);
            write_if_nonnull(env, io_data_size, guest_size_of::<f64>());
        }
        kAudioUnitProperty_LastRenderError => {
            write_if_nonnull(env, out_data.cast(), 0u32);
            write_if_nonnull(env, io_data_size, guest_size_of::<u32>());
        }
        kAudioUnitProperty_ShouldAllocateBuffer => {
            let flag = host_object
                .should_allocate_buffers
                .get(&(in_scope, in_element))
                .copied()
                .unwrap_or(1);
            write_if_nonnull(env, out_data.cast(), flag);
            write_if_nonnull(env, io_data_size, guest_size_of::<u32>());
        }
        kAudioUnitProperty_InPlaceProcessing
        | kAudioUnitProperty_BypassEffect => {
            // Булевые свойства — возвращаем 1 (да/включено) как заглушку.
            write_if_nonnull(env, out_data.cast(), 1u32);
            write_if_nonnull(env, io_data_size, guest_size_of::<u32>());
        }
        kAudioOutputUnitProperty_HasIO => {
            // IO активен по умолчанию.
            write_if_nonnull(env, out_data.cast(), 1u32);
            write_if_nonnull(env, io_data_size, guest_size_of::<u32>());
        }
        kAudioUnitProperty_AudioChannelLayout => {
            // Ранее записанные через SetProperty сырые байты, либо дефолтный
            // 12-байтовый заголовок (mChannelLayoutTag=0 → UseChannelDescriptions).
            let default_layout = [0u8; 12];
            let bytes: &[u8] = host_object
                .audio_channel_layouts
                .get(&(in_scope, in_element))
                .map(Vec::as_slice)
                .unwrap_or(&default_layout);
            let full_len = bytes.len() as u32;
            if !out_data.is_null() {
                let capacity = if io_data_size.is_null() {
                    full_len
                } else {
                    env.mem.read::<u32, true>(io_data_size)
                };
                let copy_len = std::cmp::min(full_len, capacity) as usize;
                let dst = env.mem.bytes_at_mut(out_data.cast(), copy_len as u32);
                dst.copy_from_slice(&bytes[..copy_len]);
            }
            write_if_nonnull(env, io_data_size, full_len);
        }
        _ => {
            log!(
                "AudioUnitGetProperty: UNHANDLED property {} \
                 (unit={:?}, scope={}, element={})",
                in_id,
                in_unit,
                in_scope,
                in_element
            );
            // Записываем размер 0, чтобы гость не читал мусор.
            write_if_nonnull(env, io_data_size, 0u32);
            return -1;
        }
    }
    0
}

fn AudioUnitGetPropertyInfo(
    env: &mut Environment,
    in_unit: AudioUnit,
    in_id: AudioUnitPropertyID,
    in_scope: AudioUnitScope,
    in_element: AudioUnitElement,
    out_data_size: MutPtr<u32>,
    out_writable: MutPtr<bool>,
) -> OSStatus {
    // Размер ранее записанного макета каналов (или дефолтные 12 байт).
    let channel_layout_size = audio_components::State::get(&mut env.framework_state)
        .audio_component_instances
        .get(&in_unit)
        .and_then(|h| h.audio_channel_layouts.get(&(in_scope, in_element)))
        .map(|bytes| bytes.len() as u32)
        .unwrap_or(12);

    let (size, writable) = match in_id {
        kAudioUnitProperty_StreamFormat => (guest_size_of::<AudioStreamBasicDescription>(), true),
        kAudioUnitProperty_SampleRate => (guest_size_of::<f64>(), true),
        kAudioUnitProperty_MaximumFramesPerSlice => (guest_size_of::<u32>(), true),
        kAudioUnitProperty_ElementCount => (guest_size_of::<u32>(), true),
        kAudioOutputUnitProperty_IsRunning => (guest_size_of::<u32>(), false),
        kAudioOutputUnitProperty_HasIO => (guest_size_of::<u32>(), true),
        kAudioUnitProperty_Latency => (guest_size_of::<f64>(), false),
        kAudioUnitProperty_LastRenderError => (guest_size_of::<u32>(), false),
        kAudioUnitProperty_ShouldAllocateBuffer
        | kAudioUnitProperty_InPlaceProcessing
        | kAudioUnitProperty_BypassEffect => (guest_size_of::<u32>(), true),
        kAudioUnitProperty_ClassInfo => (guest_size_of::<MutVoidPtr>(), false),
        kAudioUnitProperty_AudioChannelLayout => (channel_layout_size, true),
        _ => {
            log_dbg!("AudioUnitGetPropertyInfo: unknown property {}", in_id);
            return -1;
        }
    };

    if !out_data_size.is_null() {
        env.mem.write(out_data_size, size);
    }
    if !out_writable.is_null() {
        env.mem.write(out_writable, writable);
    }
    0
}

// =========================================================================
// MARK: - Получение/установка параметров (Parameters)
// =========================================================================

fn AudioUnitSetParameter(
    env: &mut Environment,
    in_unit: AudioUnit,
    in_id: AudioUnitParameterID,
    in_scope: AudioUnitScope,
    in_element: AudioUnitElement,
    in_value: AudioUnitParameterValue,
    _in_offset: u32,
) -> OSStatus {
    log_dbg!(
        "AudioUnitSetParameter(unit={:?}, param={}, scope={}, \
         element={}, value={})",
        in_unit,
        in_id,
        in_scope,
        in_element,
        in_value
    );
    let mut update_al_pos = None;
    let mut update_bus_gain_and_rate = false;

    // Ограничиваем область видимости заимствования
    {
        let Some(host_object) = audio_components::State::get(&mut env.framework_state)
            .audio_component_instances
            .get_mut(&in_unit)
        else {
            return paramErr;
        };

        match in_id {
            k3DMixerParam_Azimuth | k3DMixerParam_Elevation | k3DMixerParam_Distance => {
                let bus = host_object.mixer_buses.entry(in_element).or_default();
                if in_id == k3DMixerParam_Azimuth {
                    let radians = in_value.to_radians();
                    bus.position[0] = radians.sin();
                    bus.position[2] = -radians.cos();
                } else if in_id == k3DMixerParam_Elevation {
                    let radians = in_value.to_radians();
                    bus.position[1] = radians.sin();
                }

                // Сохраняем значения для OpenAL
                if let Some(source) = bus.al_source {
                    update_al_pos = Some((source, bus.position));
                }
            }
            k3DMixerParam_Gain | k3DMixerParam_PlaybackRate
                if in_scope == kAudioUnitScope_Input
                    && host_object
                        .component_desc
                        .is_some_and(|(_, sub_type, _)| sub_type == fourcc(b"3dem")) =>
            {
                let bus = host_object.mixer_buses.entry(in_element).or_default();
                if in_id == k3DMixerParam_Gain {
                    // The gain is in decibels.
                    bus.volume = 10f32.powf(in_value / 20.0);
                } else {
                    bus.playback_rate = in_value;
                }
                update_bus_gain_and_rate = true;
            }
            _ => {}
        }
    } // Конец заимствования

    if update_bus_gain_and_rate {
        apply_mixer_bus_gain_and_rate(env, in_unit, in_element);
    }

    // Теперь безопасно вызываем OpenAL
    if let Some((source, pos)) = update_al_pos {
        let context = env
            .framework_state
            .audio_toolbox
            .make_al_context_current(&mut env.openal_manager);
        unsafe {
            context.Source3f(source, AL_POSITION, pos[0], pos[1], pos[2]);
        }
    }

    0
}

fn AudioUnitGetParameter(
    env: &mut Environment,
    in_unit: AudioUnit,
    in_id: AudioUnitParameterID,
    in_scope: AudioUnitScope,
    in_element: AudioUnitElement,
    out_value: MutPtr<AudioUnitParameterValue>,
) -> OSStatus {
    log_dbg!(
        "AudioUnitGetParameter(unit={:?}, param={}, scope={}, element={})",
        in_unit,
        in_id,
        in_scope,
        in_element
    );
    if !out_value.is_null() {
        env.mem.write(out_value, 1.0);
    }
    0
}

fn AudioUnitScheduleParameters(
    _e: &mut Environment,
    _u: AudioUnit,
    _p: ConstVoidPtr,
    _n: u32,
) -> OSStatus {
    0
}

fn AudioUnitReset(
    env: &mut Environment,
    in_unit: AudioUnit,
    _s: AudioUnitScope,
    _e: AudioUnitElement,
) -> OSStatus {
    if let Some(obj) = audio_components::State::get(&mut env.framework_state)
        .audio_component_instances
        .get_mut(&in_unit)
    {
        obj.last_render_time = None;
    }
    0
}

// =========================================================================
// MARK: - Запуск / Остановка AudioOutputUnit
// =========================================================================

fn AudioOutputUnitStart(env: &mut Environment, ci: AudioUnit) -> OSStatus {
    let has_callback = audio_components::State::get(&mut env.framework_state)
        .audio_component_instances
        .get(&ci)
        .map(|o| o.render_callback.is_some())
        .unwrap_or(false);
    log_dbg!(
        "AudioOutputUnitStart({:?}) render_callback_set={}",
        ci,
        has_callback
    );
    if !au_graph::start_graph_for_output(env, ci) {
        setup_audio_unit_for_render(env, ci);
    }
    set_upstream_units_running(env, ci, true);
    0
}

/// Подготовить AudioUnit к работе в run-loop'e: завести OpenAL-источник для
/// прямого render-callback'а (если он есть) и/или для каждой input-шины
/// 3D-Mixer'а (если callback'и заданы через
/// `AUGraphSetNodeInputCallback`).
/// Используется как из `AudioOutputUnitStart`, так и из `AUGraphStart`.
pub fn setup_audio_unit_for_render(env: &mut Environment, ci: AudioUnit) {
    // Сначала собираем номера шин, у которых есть callback, но ещё нет
    // OpenAL-источника, чтобы обойтись без двойного `&mut`.
    let bus_ids_needing_source: Vec<u32> = {
        let state = audio_components::State::get(&mut env.framework_state);
        let Some(obj) = state.audio_component_instances.get(&ci) else {
            return;
        };
        obj.mixer_buses
            .iter()
            .filter_map(|(id, bus)| {
                if bus.render_callback.is_some() && bus.al_source.is_none() {
                    Some(*id)
                } else {
                    None
                }
            })
            .collect()
    };

    let was_started = audio_components::State::get(&mut env.framework_state)
        .audio_component_instances
        .get(&ci)
        .map(|obj| obj.started)
        .unwrap_or(false);

    let need_unit_source = {
        let state = audio_components::State::get(&mut env.framework_state);
        let Some(obj) = state.audio_component_instances.get(&ci) else {
            return;
        };
        obj.al_source.is_none()
    };

    let context = env
        .framework_state
        .audio_toolbox
        .al_context
        .make_al_context_current(&mut env.openal_manager);

    let unit_source: Option<ALuint> = if need_unit_source {
        let mut s: ALuint = 0;
        unsafe {
            context.GenSources(1, &mut s);
            context.SourcePlay(s);
        }
        Some(s)
    } else {
        None
    };

    let mut bus_sources: Vec<(u32, ALuint)> = Vec::with_capacity(bus_ids_needing_source.len());
    for bus_id in &bus_ids_needing_source {
        let mut s: ALuint = 0;
        unsafe {
            context.GenSources(1, &mut s);
            context.SourcePlay(s);
        }
        bus_sources.push((*bus_id, s));
    }
    drop(context);

    let now = Instant::now();
    {
        let state = audio_components::State::get(&mut env.framework_state);
        let Some(obj) = state.audio_component_instances.get_mut(&ci) else {
            return;
        };
        if let Some(s) = unit_source {
            obj.al_source = Some(s);
        }
        for (bus_id, src) in bus_sources {
            if let Some(bus) = obj.mixer_buses.get_mut(&bus_id) {
                bus.al_source = Some(src);
                if bus.last_render_time.is_none() {
                    bus.last_render_time = Some(now);
                }
            }
        }
        if obj.last_render_time.is_none() {
            obj.last_render_time = Some(now);
        }
        obj.started = true;
    }

    if !was_started {
        notify_audio_unit_is_running(env, ci);
    }
    if env.options.audio_unit_render_thread {
        start_audio_unit_render_thread(env, ci);
    }
}

// Apple's render callbacks run asynchronously on a real-time audio thread:
// https://developer.apple.com/library/archive/documentation/MusicAudio/Conceptual/AudioUnitHostingGuide_iOS/AudioUnitHostingFundamentals/AudioUnitHostingFundamentals.html
// Driving them only from NSRunLoop deadlocks FMOD's blocking bank load before
// UIApplicationMain reaches that loop. Use a cooperative guest thread so the
// callbacks can run while the main thread is asleep or waiting on a semaphore.
fn start_audio_unit_render_thread(env: &mut Environment, unit: AudioUnit) {
    let Some(state) = audio_components::State::get(&mut env.framework_state)
        .audio_component_instances
        .get(&unit)
    else {
        return;
    };
    if state.render_thread.is_some() {
        return;
    }
    let routine = env
        .dyld
        .create_proc_address(
            &mut env.mem,
            &mut env.cpu,
            "_touchHLE_audio_unit_render_thread",
        )
        .unwrap();
    let thread = env.new_thread(routine, unit.cast(), 128 * 1024);
    audio_components::State::get(&mut env.framework_state)
        .audio_component_instances
        .get_mut(&unit)
        .unwrap()
        .render_thread = Some(thread);
    log!("Audio Unit {:?} rendering on guest thread {}", unit, thread);
}

/// Internal entry point for the guest audio worker, not an iOS API.
fn touchHLE_audio_unit_render_thread(env: &mut Environment, unit: AudioUnit) -> MutVoidPtr {
    loop {
        let Some(state) = audio_components::State::get(&mut env.framework_state)
            .audio_component_instances
            .get(&unit)
        else {
            break;
        };
        // A disposed handle can be reused before this worker next runs.
        if state.render_thread != Some(env.current_thread) {
            break;
        }
        if state.initialized && state.started {
            render_audio_unit(env, unit);
        }
        env.sleep_guest(std::time::Duration::from_millis(5));
    }
    MutVoidPtr::null()
}

fn AudioOutputUnitStop(env: &mut Environment, ci: AudioUnit) -> OSStatus {
    au_graph::stop_graph_for_output(env, ci);
    set_upstream_units_running(env, ci, false);
    let was_started = audio_components::State::get(&mut env.framework_state)
        .audio_component_instances
        .get(&ci)
        .map(|obj| obj.started)
        .unwrap_or(false);

    let result = {
        let at_state = &mut env.framework_state.audio_toolbox;
        let context = at_state
            .al_context
            .make_al_context_current(&mut env.openal_manager);

        if let Some(audio_unit_state) = at_state
            .audio_components
            .audio_component_instances
            .get_mut(&ci)
        {
            audio_unit_state.started = false;
            if let Some(al_source) = audio_unit_state.al_source {
                unsafe {
                    context.DeleteSources(1, &al_source);
                }
            }
            audio_unit_state.al_source = None;
            0
        } else {
            -1
        }
    };

    if result == 0 && was_started {
        notify_audio_unit_is_running(env, ci);
    }
    result
}

// =========================================================================
// MARK: - Рендеринг (Render)
// =========================================================================

fn AudioUnitAddRenderNotify(
    env: &mut Environment,
    u: AudioUnit,
    p: AURenderCallback,
    r: ConstVoidPtr,
) -> OSStatus {
    log_dbg!(
        "AudioUnitAddRenderNotify(unit={:?}, proc={:?}, ref_con={:?})",
        u,
        p,
        r
    );
    if p.to_ptr().is_null() {
        return paramErr;
    }
    if let Some(obj) = audio_components::State::get(&mut env.framework_state)
        .audio_component_instances
        .get_mut(&u)
    {
        obj.render_notifies.push((p, r));
    }
    0
}
fn AudioUnitRemoveRenderNotify(
    env: &mut Environment,
    u: AudioUnit,
    p: AURenderCallback,
    r: ConstVoidPtr,
) -> OSStatus {
    log_dbg!(
        "AudioUnitRemoveRenderNotify(unit={:?}, proc={:?}, ref_con={:?})",
        u,
        p,
        r
    );
    if let Some(obj) = audio_components::State::get(&mut env.framework_state)
        .audio_component_instances
        .get_mut(&u)
    {
        obj.render_notifies
            .retain(|(proc, ref_con)| proc.to_ptr() != p.to_ptr() || *ref_con != r);
    }
    0
}

/// Invoke every render notify registered on this unit, mimicking CoreAudio's
/// pre/post-render notification pass. `action_flags_ptr` is written with
/// `flag` before each call so guest code can tell the phases apart; the
/// remaining arguments mirror the AURenderCallback signature.
fn call_render_notifies(
    env: &mut Environment,
    audio_unit: AudioUnit,
    action_flags_ptr: MutPtr<u32>,
    flag: u32,
    timestamp: ConstVoidPtr,
    bus: u32,
    frames: u32,
    io_data: MutVoidPtr,
) {
    let notifies: Vec<(GuestFunction, ConstVoidPtr)> = {
        let obj = audio_components::State::get(&mut env.framework_state)
            .audio_component_instances
            .get(&audio_unit);
        match obj {
            Some(obj) if !obj.render_notifies.is_empty() => obj
                .render_notifies
                .iter()
                .map(|(proc, ref_con)| (*proc, *ref_con))
                .collect(),
            _ => return,
        }
    };
    env.mem.write(action_flags_ptr, flag);
    for (proc, ref_con) in notifies {
        let _: OSStatus = proc.call_from_host(
            env,
            (ref_con, action_flags_ptr, timestamp, bus, frames, io_data),
        );
    }
}

fn AudioUnitRender(
    env: &mut Environment,
    in_unit: AudioUnit,
    _f: MutPtr<u32>,
    _t: ConstVoidPtr,
    in_bus_number: u32,
    in_number_frames: u32,
    io_data: MutVoidPtr,
) -> OSStatus {
    // RemoteIO input: when the guest enabled the input element (scope=Input,
    // element=1) the io_data AudioBufferList is meant to be filled with fresh
    // microphone samples. Fill it from the host mic (silence when the host
    // has none), then fall through to the normal playback render path.
    fill_mic_input(env, in_unit, in_bus_number, in_number_frames, io_data);

    render_audio_unit(env, in_unit);
    0
}

/// Fill the caller-provided `AudioBufferList` at `io_data` with microphone
/// PCM, converted to the unit's input stream format. `mic_input_enabled` is
/// tracked per instance from kAudioOutputUnitProperty_EnableIO writes.
fn fill_mic_input(
    env: &mut Environment,
    audio_unit: AudioUnit,
    bus_number: u32,
    frames: u32,
    io_data: MutVoidPtr,
) {
    if io_data.is_null() || frames == 0 {
        return;
    }
    let mic_enabled = {
        let at = &mut env.framework_state.audio_toolbox;
        at.audio_components
            .audio_component_instances
            .get(&audio_unit)
            .map(|obj| obj.mic_input_enabled)
            .unwrap_or(false)
    };
    if !mic_enabled || bus_number != 1 {
        return;
    }
    let stream_format = {
        let at = &mut env.framework_state.audio_toolbox;
        at.audio_components
            .audio_component_instances
            .get(&audio_unit)
            .and_then(|obj| obj.input_stream_format)
            .unwrap_or_else(|| {
                at.audio_components
                    .audio_component_instances
                    .get(&audio_unit)
                    .map(|obj| obj.global_stream_format)
                    .unwrap()
            })
    };

    // Pull a chunk of 16-bit mono host PCM and resample it to the unit's
    // sample rate (naive linear interpolation is plenty for voice).
    let host_rate = crate::android_media::MIC_SAMPLE_RATE as f64;
    let dst_rate = if stream_format.sample_rate > 0.0 {
        stream_format.sample_rate
    } else {
        host_rate
    };
    let want = ((frames as f64) * host_rate / dst_rate).ceil() as usize;
    let src: Vec<i16> = if crate::android_media::has_microphone() {
        let chunk = crate::android_media::read_mic_chunk();
        if chunk.is_empty() {
            vec![0i16; want]
        } else if want <= chunk.len() {
            chunk[..want].to_vec()
        } else {
            // Repeat the tail of the chunk (host delivers in ~20-40ms bursts;
            // stretching by repetition keeps the pitch right).
            let mut v = Vec::with_capacity(want);
            while v.len() < want {
                let take = want - v.len();
                v.extend_from_slice(&chunk[..take.min(chunk.len())]);
            }
            v
        }
    } else {
        vec![0i16; want]
    };

    // Convert to the requested sample format.
    let is_float = stream_format.format_flags & crate::frameworks::core_audio_types::kAudioFormatFlagIsFloat
        != 0;
    let bytes_per_out = (stream_format.bits_per_channel / 8) as usize;
    let channels = stream_format.channels_per_frame.max(1) as usize;
    let mut out: Vec<u8> = Vec::with_capacity(frames as usize * channels * bytes_per_out);
    for &s in &src {
        let src_s = s as f32 / 32768.0;
        for _ in 0..channels {
            if is_float {
                match bytes_per_out {
                    4 => out.extend_from_slice(&src_s.to_le_bytes()),
                    8 => out.extend_from_slice(&((src_s as f64).to_le_bytes())),
                    _ => out.extend_from_slice(&s.to_le_bytes()),
                }
            } else {
                match bytes_per_out {
                    1 => out.push(((s >> 8) as u8).wrapping_add(0x80)),
                    2 => out.extend_from_slice(&s.to_le_bytes()),
                    4 => out.extend_from_slice(&((s as i32) << 16).to_le_bytes()),
                    _ => out.extend_from_slice(&s.to_le_bytes()),
                }
            }
        }
    }

    // Read the AudioBufferList header and fill every buffer.
    let list = io_data.cast::<AudioBufferList<1>>();
    let number_buffers: u32 = env.mem.read(list.cast());
    // buffers[0] follows the u32 count (sizeof(u32) = 4 bytes).
    let buf_ptr: MutPtr<AudioBuffer> = (list.cast::<u8>() + 4u32).cast();
    for i in 0..number_buffers.min(2) as u64 {
        let b: AudioBuffer = env.mem.read(buf_ptr + (i * 12) as u32);
        if b.data.is_null() || b.data_byte_size == 0 {
            continue;
        }
        let n = (b.data_byte_size as usize).min(out.len());
        let slice = env.mem.bytes_at_mut(b.data.cast(), n as u32);
        slice.copy_from_slice(&out[..n]);
        // Zero the remainder rather than leaving stale guest memory.
        if n < b.data_byte_size as usize {
            let rest = env.mem.bytes_at_mut(
                b.data.cast::<u8>() + n as u32,
                (b.data_byte_size as usize - n) as u32,
            );
            rest.fill(0);
        }
    }
}

fn AudioUnitProcess(
    env: &mut Environment,
    in_unit: AudioUnit,
    _f: MutPtr<u32>,
    _t: ConstVoidPtr,
    _n: u32,
    _d: MutVoidPtr,
) -> OSStatus {
    render_audio_unit(env, in_unit);
    0
}

fn AudioUnitProcessMultiple(
    _e: &mut Environment,
    _u: AudioUnit,
    _f: MutPtr<u32>,
    _t: ConstVoidPtr,
    _n: u32,
    _in_b: u32,
    _in_bl: ConstVoidPtr,
    _out_bl: MutVoidPtr,
) -> OSStatus {
    0
}
fn AudioUnitComplexRender(
    _e: &mut Environment,
    _u: AudioUnit,
    _f: MutPtr<u32>,
    _t: ConstVoidPtr,
    _b: u32,
    _n: u32,
    _p: MutPtr<u32>,
    _pd: MutVoidPtr,
    _d: MutVoidPtr,
) -> OSStatus {
    0
}

/// `AudioTimeStampFlags` from `<CoreAudio/CoreAudioTypes.h>`.
const kAudioTimeStampSampleTimeValid: u32 = 1 << 0;
const kAudioTimeStampHostTimeValid: u32 = 1 << 1;
const kAudioTimeStampRateScalarValid: u32 = 1 << 2;

/// Allocates a guest-visible `AudioTimeStamp` describing the current render
/// pass. The caller owns it and must `env.mem.free()` it once the callback has
/// returned.
///
/// `sample_time` is the number of frames rendered so far: it has to advance by
/// `frames` on every pass. `host_time` is expressed in the same units
/// `mach_absolute_time` reports, so a callback can correlate the two clocks.
///
/// Passing `nil` here instead — which this code used to do — is not a harmless
/// omission. `inTimeStamp` is a documented input of `AURenderCallback`, and
/// middleware that mixes *streams* (long music tracks decoded on a worker
/// thread) rather than one-shot samples reads `mSampleTime`/`mHostTime` to work
/// out how much audio the device consumed and therefore how far ahead to mix.
/// FMOD's AudioUnit output — the engine Geometry Dash 2.11 uses for both its
/// music and its sound effects — is such a mixer; with no timestamp at all it
/// has nothing to synchronise its stream buffer against.
pub(crate) fn alloc_render_timestamp(env: &mut Environment, sample_time: f64) -> MutVoidPtr {
    let host_time: u64 = env
        .guest_clock
        .now()
        .duration_since(env.startup_time)
        .as_nanos() as u64;
    let timestamp = env.mem.alloc_and_write(AudioTimeStamp {
        sample_time,
        host_time,
        // We hand the callback exactly as many frames as it asked for, at the
        // format's nominal rate, i.e. no drift.
        rate_scalar: 1.0,
        world_clock_type: 0,
        SMPTE_time: SMPTETime {
            subframes: 0,
            subframe_divisor: 0,
            counter: 0,
            type_: 0,
            flags: 0,
            hours: 0,
            minutes: 0,
            seconds: 0,
            frames: 0,
        },
        flags: kAudioTimeStampSampleTimeValid
            | kAudioTimeStampHostTimeValid
            | kAudioTimeStampRateScalarValid,
        _reserved: 0,
    });
    timestamp.cast_void()
}

/// Per-bus рендеринг для 3D Mixer / любого юнита, в котором через
/// `AUGraphSetNodeInputCallback` (или эквивалент) задан input render
/// callback на отдельные шины. Для каждой такой шины вызывает гостевой
/// callback, получает PCM и складывает его в свой OpenAL-источник.
/// OpenAL Soft сам микширует все источники вместе.
fn render_audio_unit_buses(env: &mut Environment, audio_unit: AudioUnit) {
    use crate::frameworks::core_audio_types::{
        kAudioFormatFlagIsNonInterleaved, kAudioFormatFlagIsPacked,
        kAudioFormatFlagIsSignedInteger, kAudioFormatLinearPCM,
    };

    // Готовим план: список
    // (bus_id, callback, al_source, last_render_time, format).
    let plan: Vec<(
        u32,
        AURenderCallbackStruct,
        ALuint,
        Instant,
        AudioStreamBasicDescription,
        f64,
    )> = {
        let at = &mut env.framework_state.audio_toolbox;
        let hardware_sr = at.audio_session.current_hardware_sample_rate;
        let Some(obj) = at
            .audio_components
            .audio_component_instances
            .get(&audio_unit)
        else {
            return;
        };
        if !obj.started || obj.mixer_buses.is_empty() {
            return;
        }
        // Дефолтный формат шины 3D Mixer, если игра его явно не задавала:
        // 16-bit signed integer LE PCM, моно, текущая частота железа.
        let default_format = AudioStreamBasicDescription {
            sample_rate: if hardware_sr > 0.0 {
                hardware_sr
            } else {
                22050.0
            },
            format_id: kAudioFormatLinearPCM,
            format_flags: kAudioFormatFlagIsSignedInteger | kAudioFormatFlagIsPacked,
            bytes_per_packet: 2,
            frames_per_packet: 1,
            bytes_per_frame: 2,
            channels_per_frame: 1,
            bits_per_channel: 16,
            _reserved: 0,
        };
        let mut v = Vec::new();
        for (bus_id, bus) in obj.mixer_buses.iter() {
            let (Some(cb), Some(src), Some(last)) =
                (bus.render_callback, bus.al_source, bus.last_render_time)
            else {
                continue;
            };
            let fmt = bus.stream_format.unwrap_or(default_format);
            v.push((*bus_id, cb, src, last, fmt, bus.sample_time));
        }
        v
    };
    if plan.is_empty() {
        return;
    }

    log_once!(
        "render_audio_unit_buses: rendering bus callback(s) \
         on first iteration"
    );

    let now = Instant::now();
    for (bus_id, callback, al_source, last_render_time, fmt, start_sample_time) in plan {
        // Frames rendered for this bus so far; handed to the callback through
        // the `AudioTimeStamp` below and advanced by every render pass.
        let mut sample_time = start_sample_time;
        // Ограничиваем глубину очереди OpenAL, чтобы буферы не накапливались
        // быстрее, чем воспроизводятся. Если этого не делать, при длительной
        // игре источник набирает всё больше необработанных буферов, звук
        // отстаёт по времени и начинает «скрипеть». Поведение зеркалит
        // `handle_audio_queue` в audio_queue.rs.
        let mut queued = 0;
        let mut processed = 0;
        {
            let context = env
                .framework_state
                .audio_toolbox
                .al_context
                .make_al_context_current(&mut env.openal_manager);
            unsafe {
                context.GetSourcei(al_source, AL_BUFFERS_QUEUED, &mut queued);
                context.GetSourcei(al_source, AL_BUFFERS_PROCESSED, &mut processed);
            }
        }
        if queued.saturating_sub(processed) > AUDIO_RENDER_TARGET_DEPTH {
            // Источник ещё не успел проиграть то, что уже в очереди.
            // Сливаем отыгранные буферы и пропускаем рендер на этот тик.
            let mut drained: Vec<ALuint> = Vec::new();
            {
                let context = env
                    .framework_state
                    .audio_toolbox
                    .al_context
                    .make_al_context_current(&mut env.openal_manager);
                unsafe {
                    while processed > 0 {
                        let mut b = 0;
                        context.SourceUnqueueBuffers(al_source, 1, &mut b);
                        drained.push(b);
                        processed -= 1;
                    }
                    if !drained.is_empty() {
                        context.DeleteBuffers(drained.len() as i32, drained.as_ptr());
                    }
                }
            }
            if let Some(obj) = audio_components::State::get(&mut env.framework_state)
                .audio_component_instances
                .get_mut(&audio_unit)
            {
                if let Some(bus) = obj.mixer_buses.get_mut(&bus_id) {
                    bus.last_render_time = Some(now);
                }
            }
            continue;
        }

        let elapsed = now.duration_since(last_render_time);
        let nominal_frames = ((env.framework_state.audio_toolbox.audio_session
            .current_hardware_io_buffer_duration as f64)
            * fmt.sample_rate as f64) as u32;
        let mut frames = ((elapsed.as_secs_f64() * fmt.sample_rate) as u32).clamp(64, 4096);
        let is_planar_stereo = fmt.format_id == kAudioFormatLinearPCM
            && fmt.channels_per_frame == 2
            && fmt.format_flags & kAudioFormatFlagIsNonInterleaved != 0
            && fmt.bytes_per_frame == fmt.bits_per_channel / 8;
        let mut buffer_size = if is_planar_stereo {
            frames * fmt.bytes_per_frame
        } else {
            frames * fmt.channels_per_frame * (fmt.bits_per_channel / 8)
        };
        if buffer_size == 0 {
            continue;
        }

        // Дренируем уже отыгранные буферы.
        let mut free_buffers: Vec<ALuint> = Vec::new();
        {
            let context = env
                .framework_state
                .audio_toolbox
                .al_context
                .make_al_context_current(&mut env.openal_manager);
            unsafe {
                let mut processed = 0;
                context.GetSourcei(al_source, AL_BUFFERS_PROCESSED, &mut processed);
                while processed > 0 {
                    let mut b = 0;
                    context.SourceUnqueueBuffers(al_source, 1, &mut b);
                    free_buffers.push(b);
                    context.GetSourcei(al_source, AL_BUFFERS_PROCESSED, &mut processed);
                }
            }
        }

        // PERF OPTIMIZATION (audio): burst top-up — рендерим столько буферов,
        // сколько нужно, чтобы довести очередь до AUDIO_RENDER_TARGET_DEPTH за
        // один тик. После underrun дозаполнение по одному буферу за тик
        // восстанавливало запас слишком медленно, и звук продолжал хрипеть на
        // протяжённых хитчах главного потока.
        let render_passes = (AUDIO_RENDER_TARGET_DEPTH - (queued - processed))
            .max(1) as usize;

        let input_proc = callback.input_proc;
        let input_proc_ref = callback.input_proc_ref_con;

        for audio_pass in 0..render_passes {
            if audio_pass > 0 {
                frames = nominal_frames.clamp(64, 4096);
                buffer_size = if is_planar_stereo {
                    frames * fmt.bytes_per_frame
                } else {
                    frames * fmt.channels_per_frame * (fmt.bits_per_channel / 8)
                };
                if buffer_size == 0 {
                    break;
                }
            }

            // Prepare one interleaved buffer or two mono planes, depending on the stream format.
            let action_flags = env.mem.alloc_and_write(0u32);
            let (abl, buffer_data, second_buffer_data) = if is_planar_stereo {
                let left = env.mem.alloc(buffer_size);
                let right = env.mem.alloc(buffer_size);
                let abl = env.mem.alloc_and_write(AudioBufferList::<2> {
                    number_buffers: 2,
                    buffers: [
                        AudioBuffer {
                            number_channels: 1,
                            data_byte_size: buffer_size,
                            data: left,
                        },
                        AudioBuffer {
                            number_channels: 1,
                            data_byte_size: buffer_size,
                            data: right,
                        },
                    ],
                });
                (abl.cast::<std::ffi::c_void>(), left, Some(right))
            } else {
                let buffer = env.mem.alloc(buffer_size);
                let abl = env.mem.alloc_and_write(AudioBufferList::<1> {
                    number_buffers: 1,
                    buffers: [AudioBuffer {
                        number_channels: fmt.channels_per_frame,
                        data_byte_size: buffer_size,
                        data: buffer,
                    }],
                });
                (abl.cast::<std::ffi::c_void>(), buffer, None)
            };

            let timestamp = alloc_render_timestamp(env, sample_time);
            call_render_notifies(
                env,
                audio_unit,
                action_flags,
                kAudioUnitRenderAction_PreRender,
                timestamp.cast_const(),
                bus_id,
                frames,
                abl.cast::<std::ffi::c_void>(),
            );
            env.mem.write(action_flags, 0u32);
            let _: OSStatus = input_proc.call_from_host(
                env,
                (
                    input_proc_ref,
                    action_flags,
                    timestamp.cast_const(),
                    bus_id,
                    frames,
                    abl,
                ),
            );
            call_render_notifies(
                env,
                audio_unit,
                action_flags,
                kAudioUnitRenderAction_PostRender,
                timestamp.cast_const(),
                bus_id,
                frames,
                abl.cast::<std::ffi::c_void>(),
            );
            sample_time += frames as f64;

            let (al_fmt, _, processed) = if let Some(right) = second_buffer_data {
                let planes = [
                    env.mem.bytes_at(buffer_data.cast(), buffer_size),
                    env.mem.bytes_at(right.cast(), buffer_size),
                ];
                let interleaved =
                    interleave_planar_pcm(&planes, (fmt.bits_per_channel / 8) as usize)
                        .unwrap_or_default();
                let interleaved_format = interleaved_format_for_planar_pcm(fmt);
                decode_buffer_from_bytes(&interleaved_format, &interleaved, &[])
            } else {
                decode_buffer(&env.mem, &fmt, buffer_data.cast(), buffer_size, &[])
            };
            if processed.is_empty() {
                // Если callback ничего не записал — прекращаем burst.
                env.mem.free(action_flags.cast_void());
                env.mem.free(buffer_data.cast_void());
                if let Some(right) = second_buffer_data {
                    env.mem.free(right.cast_void());
                }
                env.mem.free(abl);
                env.mem.free(timestamp);
                break;
            }

            {
                let context = env
                    .framework_state
                    .audio_toolbox
                    .al_context
                    .make_al_context_current(&mut env.openal_manager);
                unsafe {
                    let b = free_buffers.pop().unwrap_or_else(|| {
                        let mut x = 0;
                        context.GenBuffers(1, &mut x);
                        x
                    });
                    context.BufferData(
                        b,
                        al_fmt,
                        processed.as_ptr() as *const ALvoid,
                        processed.len() as i32,
                        fmt.sample_rate as i32,
                    );
                    context.SourceQueueBuffers(al_source, 1, &b);
                    let mut state = 0;
                    context.GetSourcei(al_source, AL_SOURCE_STATE, &mut state);
                    if state != AL_PLAYING {
                        context.SourcePlay(al_source);
                    }
                }
            }

            env.mem.free(action_flags.cast_void());
            env.mem.free(buffer_data.cast_void());
            if let Some(right) = second_buffer_data {
                env.mem.free(right.cast_void());
            }
            env.mem.free(abl);
            env.mem.free(timestamp);
        }

        // Освобождаем неиспользованные дренированные буферы, чтобы они не утекли.
        if !free_buffers.is_empty() {
            let context = env
                .framework_state
                .audio_toolbox
                .al_context
                .make_al_context_current(&mut env.openal_manager);
            unsafe {
                context.DeleteBuffers(free_buffers.len() as i32, free_buffers.as_ptr());
            }
        }

        // Обновляем last_render_time и sample_time для шины.
        if let Some(obj) = audio_components::State::get(&mut env.framework_state)
            .audio_component_instances
            .get_mut(&audio_unit)
        {
            if let Some(bus) = obj.mixer_buses.get_mut(&bus_id) {
                bus.last_render_time = Some(now);
                bus.sample_time = sample_time;
            }
        }
    }
}

// Depth of the OpenAL queue we try to maintain for RemoteIO-style units.
// Each buffer is one hardware I/O duration (~23ms), so 6 buffers give
// ~140ms of ride-through for main-thread hitches (Unity scene loads, GC)
// before an audible underrun. The throttle below still drains and re-syncs
// if we ever run ahead of playback.
const AUDIO_RENDER_TARGET_DEPTH: i32 = 6;

/// `kAudioUnitRenderAction_PreRender` / `PostRender` flags passed to render
/// notify callbacks (see AUComponent.h).
const kAudioUnitRenderAction_PreRender: u32 = 1 << 2;
const kAudioUnitRenderAction_PostRender: u32 = 1 << 3;

/// Run the render notifies of a unit that has no render callback of its own,
/// such as a RemoteIO unit pulling from a mixer through a connection. Its
/// input is rendered elsewhere (every callback has its own OpenAL source), but
/// apps still expect the render cycle: Plants vs. Zombies HD connects, retunes
/// and disconnects its voices from a post-render notify on RemoteIO.
fn pump_render_notifies(
    env: &mut Environment,
    audio_unit: AudioUnit,
    sample_rate: f64,
    format: &AudioStreamBasicDescription,
    last_render_time: Instant,
    sample_time: f64,
) {
    let has_notifies = audio_components::State::get(&mut env.framework_state)
        .audio_component_instances
        .get(&audio_unit)
        .is_some_and(|obj| !obj.render_notifies.is_empty());
    if !has_notifies {
        return;
    }
    let io_buffer_duration = env
        .framework_state
        .audio_toolbox
        .audio_session
        .current_hardware_io_buffer_duration as f64;
    let nominal_frames = ((io_buffer_duration * sample_rate) as u32).clamp(64, 4096);
    let now = Instant::now();
    let elapsed_frames = (now.duration_since(last_render_time).as_secs_f64() * sample_rate) as u32;
    if elapsed_frames < nominal_frames {
        return;
    }
    let frames = elapsed_frames.min(4096);

    // Hand over silence in the unit's format: one buffer, or one per channel
    // for non-interleaved formats.
    let planar = format.format_flags & kAudioFormatFlagIsNonInterleaved != 0;
    let bytes_per_frame = if format.bytes_per_frame > 0 {
        format.bytes_per_frame
    } else {
        4
    };
    let channels = format.channels_per_frame.clamp(1, 2);
    let buffer_size = frames * bytes_per_frame;
    let first = env.mem.alloc(buffer_size);
    let (abl, second) = if planar && channels == 2 {
        let second = env.mem.alloc(buffer_size);
        let abl = env.mem.alloc_and_write(AudioBufferList::<2> {
            number_buffers: 2,
            buffers: [
                AudioBuffer {
                    number_channels: 1,
                    data_byte_size: buffer_size,
                    data: first,
                },
                AudioBuffer {
                    number_channels: 1,
                    data_byte_size: buffer_size,
                    data: second,
                },
            ],
        });
        (abl.cast::<std::ffi::c_void>(), Some(second))
    } else {
        let abl = env.mem.alloc_and_write(AudioBufferList::<1> {
            number_buffers: 1,
            buffers: [AudioBuffer {
                number_channels: if planar { 1 } else { channels },
                data_byte_size: buffer_size,
                data: first,
            }],
        });
        (abl.cast::<std::ffi::c_void>(), None)
    };
    let action_flags = env.mem.alloc_and_write(0u32);
    let timestamp = alloc_render_timestamp(env, sample_time);
    for flag in [
        kAudioUnitRenderAction_PreRender,
        kAudioUnitRenderAction_PostRender,
    ] {
        call_render_notifies(
            env,
            audio_unit,
            action_flags,
            flag,
            timestamp.cast_const(),
            0,
            frames,
            abl,
        );
    }
    env.mem.free(action_flags.cast_void());
    env.mem.free(timestamp);
    env.mem.free(abl);
    env.mem.free(first);
    if let Some(second) = second {
        env.mem.free(second);
    }

    if let Some(obj) = audio_components::State::get(&mut env.framework_state)
        .audio_component_instances
        .get_mut(&audio_unit)
    {
        obj.last_render_time = Some(now);
        obj.sample_time = sample_time + f64::from(frames);
    }
}

pub fn render_audio_unit(env: &mut Environment, audio_unit: AudioUnit) {
    if env.bundle.bundle_identifier().starts_with("com.ea.simcity") {
        // Применяем хак специфичный для SimCity: пропускаем рендеринг
        return;
    }

    // Прокачиваем все input-шины (3D Mixer / AUGraph): каждой шине свой
    // OpenAL-источник.
    render_audio_unit_buses(env, audio_unit);

    let (
        sample_rate,
        started,
        is_running,
        stream_format,
        has_input_format,
        al_source,
        last_render_time,
        callback,
        start_sample_time,
    ) = {
        let at = &mut env.framework_state.audio_toolbox;
        let Some(obj) = at
            .audio_components
            .audio_component_instances
            .get_mut(&audio_unit)
        else {
            log_once!("render_audio_unit: instance not found");
            return;
        };
        let stream_format = obj
            .input_stream_format
            .unwrap_or(obj.output_stream_format.unwrap_or(obj.global_stream_format));
        (
            audio_unit_render_sample_rate(
                &stream_format,
                at.audio_session.current_hardware_sample_rate,
            ),
            obj.started,
            obj.is_running_handler,
            stream_format,
            obj.input_stream_format.is_some(),
            obj.al_source,
            obj.last_render_time,
            obj.render_callback,
            obj.sample_time,
        )
    };

    if !started {
        log_once!("render_audio_unit: skipped (started=false)");
        return;
    }
    if is_running {
        log_once!("render_audio_unit: skipped (already running handler)");
        return;
    }

    if let Some(obj) = env
        .framework_state
        .audio_toolbox
        .audio_components
        .audio_component_instances
        .get_mut(&audio_unit)
    {
        obj.is_running_handler = true;
    }

    let Some(al_source) = al_source else {
        log_once!("render_audio_unit: skipped (al_source = None)");
        if let Some(obj) = env
            .framework_state
            .audio_toolbox
            .audio_components
            .audio_component_instances
            .get_mut(&audio_unit)
        {
            obj.is_running_handler = false;
        }
        return;
    };
    let Some(last_render_time) = last_render_time else {
        log_once!("render_audio_unit: skipped (last_render_time = None)");
        if let Some(obj) = env
            .framework_state
            .audio_toolbox
            .audio_components
            .audio_component_instances
            .get_mut(&audio_unit)
        {
            obj.is_running_handler = false;
        }
        return;
    };
    let Some(callback) = callback else {
        // Без unit-level callback'а просто молча выходим: бус-рендер уже
        // сделан, а 3D Mixer / RemoteIO без своего собственного callback'а
        // — это нормальный кейс при работе через AUGraph.
        pump_render_notifies(
            env,
            audio_unit,
            sample_rate,
            &stream_format,
            last_render_time,
            start_sample_time,
        );
        if let Some(obj) = env
            .framework_state
            .audio_toolbox
            .audio_components
            .audio_component_instances
            .get_mut(&audio_unit)
        {
            obj.is_running_handler = false;
        }
        return;
    };
    let now = Instant::now();
    let mut queued_buffers = 0;
    let mut processed_buffers = 0;
    {
        let context = env
            .framework_state
            .audio_toolbox
            .al_context
            .make_al_context_current(&mut env.openal_manager);
        unsafe {
            context.GetSourcei(al_source, AL_BUFFERS_QUEUED, &mut queued_buffers);
            context.GetSourcei(al_source, AL_BUFFERS_PROCESSED, &mut processed_buffers);
        }
    }

    // PERF OPTIMIZATION (audio): keep a few buffers queued ahead of playback
    // instead of throttling at ~1. The old cap meant any main-thread hitch
    // longer than a single buffer (~12-23ms) drained the OpenAL queue dry and
    // produced an audible dropout/crackle. Allowing ~6 buffers of slack
    // (~140ms) rides out hitches at the cost of imperceptible extra latency;
    // the throttle below still reclaims buffers and re-syncs if we run ahead.
    let remaining_buffers = queued_buffers.saturating_sub(processed_buffers);
    if remaining_buffers > AUDIO_RENDER_TARGET_DEPTH {
        let mut drained_buffers = Vec::new();
        {
            let context = env
                .framework_state
                .audio_toolbox
                .al_context
                .make_al_context_current(&mut env.openal_manager);
            unsafe {
                while processed_buffers > 0 {
                    let mut b = 0;
                    context.SourceUnqueueBuffers(al_source, 1, &mut b);
                    drained_buffers.push(b);
                    processed_buffers -= 1;
                }
                if !drained_buffers.is_empty() {
                    context.DeleteBuffers(drained_buffers.len() as i32, drained_buffers.as_ptr());
                }
            }
        }

        if let Some(obj) = env
            .framework_state
            .audio_toolbox
            .audio_components
            .audio_component_instances
            .get_mut(&audio_unit)
        {
            obj.last_render_time = Some(now);
            obj.is_running_handler = false;
        }
        return;
    }

    let mut al_buffers = Vec::new();
    {
        let context = env
            .framework_state
            .audio_toolbox
            .al_context
            .make_al_context_current(&mut env.openal_manager);
        unsafe {
            while processed_buffers > 0 {
                let mut b = 0;
                context.SourceUnqueueBuffers(al_source, 1, &mut b);
                al_buffers.push(b);
                processed_buffers -= 1;
            }
        }
    }

    let target_frames = (sample_rate
        * env
            .framework_state
            .audio_toolbox
            .audio_session
            .current_hardware_io_buffer_duration as f64)
        .round() as u32;
    let frames = target_frames.clamp(64, 2048);
    let planar_stereo_buffers = uses_planar_stereo_buffers(&stream_format, has_input_format);
    let bytes_per_sample = stream_format.bits_per_channel / 8;
    let bytes_per_frame = if planar_stereo_buffers {
        bytes_per_sample
    } else {
        stream_format.channels_per_frame * bytes_per_sample
    };
    let buffer_size = frames * bytes_per_frame;
    // Resident Evil 4 asks for non-interleaved 16-bit stereo, but its render
    // callback ignores that and writes whole interleaved frames (4 bytes each)
    // into every buffer (then copies buffer 0 into the others). With planes of
    // exactly `buffer_size` bytes it overran the heap on every render and
    // corrupted unrelated game objects, freezing the title screen. Back each
    // plane with room for full frames, and fill the extra space with a canary
    // so we can tell afterwards how the data was written.
    let interleaved_size = frames * stream_format.channels_per_frame * bytes_per_sample;
    let plane_alloc_size = buffer_size.max(interleaved_size);
    const PLANE_SLACK_CANARY: u8 = 0xA5;

    // PERF OPTIMIZATION (audio): top the queue up to AUDIO_RENDER_TARGET_DEPTH
    // in a single run-loop tick instead of exactly one buffer per tick. After
    // an underrun, a one-buffer-per-tick refill took many frames to rebuild
    // any slack, so sustained main-thread hitches kept the queue dry and the
    // audio kept crackling. Burst-filling makes the pipeline self-heal on the
    // very next tick.
    let render_passes = (AUDIO_RENDER_TARGET_DEPTH - remaining_buffers).max(1) as usize;

    let action_flags = env.mem.alloc_and_write(0u32);

    let input_proc = callback.input_proc;
    let input_proc_ref = callback.input_proc_ref_con;
    log_once!("render_audio_unit: entering callback for the first time");

    // Frames rendered by this unit so far, passed to the callback through the
    // `AudioTimeStamp` below and advanced by every render pass.
    let mut sample_time = start_sample_time;

    for _ in 0..render_passes {
        // Восстанавливаем логику из оригинала: Resident Evil 4 ожидает 2 буфера
        let (audio_buffer_list, buffer1_data, buffer2_data): (
            MutVoidPtr,
            MutVoidPtr,
            Option<MutVoidPtr>,
        ) = if planar_stereo_buffers {
            let buf1 = env.mem.alloc(plane_alloc_size);
            let buf2 = env.mem.alloc(plane_alloc_size);
            env.mem.bytes_at_mut(buf1.cast(), plane_alloc_size)[buffer_size as usize..]
                .fill(PLANE_SLACK_CANARY);
            let abl = env.mem.alloc_and_write(AudioBufferList::<2> {
                number_buffers: 2,
                buffers: [
                    AudioBuffer {
                        number_channels: 1,
                        data_byte_size: buffer_size,
                        data: buf1,
                    },
                    AudioBuffer {
                        number_channels: 1,
                        data_byte_size: buffer_size,
                        data: buf2,
                    },
                ],
            });
            (abl.cast(), buf1, Some(buf2))
        } else if has_input_format {
            let buf = env.mem.alloc(buffer_size);
            let abl = env.mem.alloc_and_write(AudioBufferList::<1> {
                number_buffers: 1,
                buffers: [AudioBuffer {
                    number_channels: stream_format.channels_per_frame,
                    data_byte_size: buffer_size,
                    data: buf,
                }],
            });
            (abl.cast(), buf, None)
        } else {
            let buf1 = env.mem.alloc(buffer_size);
            let buf2 = env.mem.alloc(buffer_size);
            let abl = env.mem.alloc_and_write(AudioBufferList::<2> {
                number_buffers: 2,
                buffers: [
                    AudioBuffer {
                        number_channels: stream_format.channels_per_frame,
                        data_byte_size: buffer_size,
                        data: buf1,
                    },
                    AudioBuffer {
                        number_channels: stream_format.channels_per_frame,
                        data_byte_size: buffer_size,
                        data: buf2,
                    },
                ],
            });
            (abl.cast(), buf1, Some(buf2))
        };

        let timestamp = alloc_render_timestamp(env, sample_time);
        call_render_notifies(
            env,
            audio_unit,
            action_flags,
            kAudioUnitRenderAction_PreRender,
            timestamp.cast_const(),
            0u32,
            frames,
            audio_buffer_list,
        );
        env.mem.write(action_flags, 0u32);
        let _: OSStatus = input_proc.call_from_host(
            env,
            (
                input_proc_ref,
                action_flags,
                timestamp.cast_const(),
                0u32,
                frames,
                audio_buffer_list,
            ),
        );
        call_render_notifies(
            env,
            audio_unit,
            action_flags,
            kAudioUnitRenderAction_PostRender,
            timestamp.cast_const(),
            0u32,
            frames,
            audio_buffer_list,
        );
        sample_time += frames as f64;

        let wrote_interleaved_frames = planar_stereo_buffers
            && env.mem.bytes_at(buffer1_data.cast(), plane_alloc_size)[buffer_size as usize..]
                .iter()
                .any(|&byte| byte != PLANE_SLACK_CANARY);
        if wrote_interleaved_frames {
            log_once!(
                "AudioUnit: render callback wrote interleaved frames into a \
                 non-interleaved buffer; playing the first buffer as interleaved"
            );
        }
        let (al_fmt, decoded_sample_rate, processed) = if wrote_interleaved_frames {
            let interleaved_format = interleaved_format_for_planar_pcm(stream_format);
            decode_buffer(
                &env.mem,
                &interleaved_format,
                buffer1_data.cast(),
                interleaved_size,
                &[],
            )
        } else if planar_stereo_buffers {
            match buffer2_data {
                Some(buffer2_data) => {
                    let planes = [
                        env.mem.bytes_at(buffer1_data.cast(), buffer_size),
                        env.mem.bytes_at(buffer2_data.cast(), buffer_size),
                    ];
                    let interleaved = interleave_planar_pcm(&planes, bytes_per_sample as usize)
                        .unwrap_or_default();
                    let interleaved_format = interleaved_format_for_planar_pcm(stream_format);
                    decode_buffer_from_bytes(&interleaved_format, &interleaved, &[])
                }
                None => decode_buffer(
                    &env.mem,
                    &stream_format,
                    buffer1_data.cast(),
                    buffer_size,
                    &[],
                ),
            }
        } else {
            decode_buffer(
                &env.mem,
                &stream_format,
                buffer1_data.cast(),
                buffer_size,
                &[],
            )
        };
        let output_sample_rate = if decoded_sample_rate > 0 {
            decoded_sample_rate
        } else {
            sample_rate as ALsizei
        };
        {
            let context = env
                .framework_state
                .audio_toolbox
                .al_context
                .make_al_context_current(&mut env.openal_manager);
            unsafe {
                let b = al_buffers.pop().unwrap_or_else(|| {
                    let mut x = 0;
                    context.GenBuffers(1, &mut x);
                    x
                });
                context.BufferData(
                    b,
                    al_fmt,
                    processed.as_ptr() as *const ALvoid,
                    processed.len() as i32,
                    output_sample_rate,
                );
                context.SourceQueueBuffers(al_source, 1, &b);
                let mut state = 0;
                context.GetSourcei(al_source, AL_SOURCE_STATE, &mut state);
                if state != AL_PLAYING {
                    context.SourcePlay(al_source);
                }
            }
        }

        env.mem.free(audio_buffer_list.cast_void());
        env.mem.free(buffer1_data.cast_void());
        env.mem.free(timestamp);
        if let Some(b2) = buffer2_data {
            env.mem.free(b2.cast_void());
        }
    }

    {
        let context = env
            .framework_state
            .audio_toolbox
            .al_context
            .make_al_context_current(&mut env.openal_manager);
        unsafe {
            if !al_buffers.is_empty() {
                context.DeleteBuffers(al_buffers.len() as i32, al_buffers.as_ptr());
            }
        }
    }

    env.mem.free(action_flags.cast_void());

    if let Some(obj) = env
        .framework_state
        .audio_toolbox
        .audio_components
        .audio_component_instances
        .get_mut(&audio_unit)
    {
        obj.last_render_time = Some(now);
        obj.is_running_handler = false;
        obj.sample_time = sample_time;
    }
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(touchHLE_audio_unit_render_thread(_)),
    export_c_func!(AudioUnitInitialize(_)),
    export_c_func!(AudioUnitUninitialize(_)),
    export_c_func!(AudioUnitAddPropertyListener(_, _, _, _)),
    export_c_func!(AudioUnitRemovePropertyListener(_, _, _)),
    export_c_func!(AudioUnitRemovePropertyListenerWithUserData(_, _, _, _)),
    export_c_func!(AudioUnitSetProperty(_, _, _, _, _, _)),
    export_c_func!(AudioUnitGetProperty(_, _, _, _, _, _)),
    export_c_func!(AudioUnitGetPropertyInfo(_, _, _, _, _, _)),
    export_c_func!(AudioUnitSetParameter(_, _, _, _, _, _)),
    export_c_func!(AudioUnitGetParameter(_, _, _, _, _)),
    export_c_func!(AudioUnitScheduleParameters(_, _, _)),
    export_c_func!(AudioUnitReset(_, _, _)),
    export_c_func!(AudioOutputUnitStart(_)),
    export_c_func!(AudioOutputUnitStop(_)),
    export_c_func!(AudioUnitAddRenderNotify(_, _, _)),
    export_c_func!(AudioUnitRemoveRenderNotify(_, _, _)),
    export_c_func!(AudioUnitRender(_, _, _, _, _, _)),
    export_c_func!(AudioUnitProcess(_, _, _, _, _)),
    export_c_func!(AudioUnitProcessMultiple(_, _, _, _, _, _, _)),
];

#[cfg(test)]
mod tests {
    use super::{
        audio_unit_render_sample_rate, interleave_planar_pcm, interleaved_format_for_planar_pcm,
        uses_planar_stereo_buffers,
    };
    use crate::audio::openal::AL_FORMAT_STEREO16;
    use crate::frameworks::audio_toolbox::audio_queue::decode_buffer_from_bytes;
    use crate::frameworks::core_audio_types::{
        kAudioFormatFlagIsFloat, kAudioFormatFlagIsNonInterleaved, kAudioFormatFlagIsPacked,
        kAudioFormatFlagIsSignedInteger, kAudioFormatLinearPCM, AudioStreamBasicDescription,
    };

    fn planar_f32_stereo_format() -> AudioStreamBasicDescription {
        AudioStreamBasicDescription {
            sample_rate: 48_000.0,
            format_id: kAudioFormatLinearPCM,
            format_flags: kAudioFormatFlagIsFloat
                | kAudioFormatFlagIsPacked
                | kAudioFormatFlagIsNonInterleaved,
            bytes_per_packet: 4,
            frames_per_packet: 1,
            bytes_per_frame: 4,
            channels_per_frame: 2,
            bits_per_channel: 32,
            _reserved: 0,
        }
    }

    #[test]
    fn re4_stereo_output_uses_both_buffers_at_guest_sample_rate() {
        let format = AudioStreamBasicDescription {
            sample_rate: 11_025.0,
            format_id: kAudioFormatLinearPCM,
            format_flags: kAudioFormatFlagIsPacked | kAudioFormatFlagIsSignedInteger,
            bytes_per_packet: 2,
            frames_per_packet: 1,
            bytes_per_frame: 2,
            channels_per_frame: 2,
            bits_per_channel: 16,
            _reserved: 0,
        };
        assert!(uses_planar_stereo_buffers(&format, false));
        assert!(!uses_planar_stereo_buffers(&format, true));

        let mut ordinary_interleaved = format;
        ordinary_interleaved.bytes_per_packet = 4;
        ordinary_interleaved.bytes_per_frame = 4;
        assert!(!uses_planar_stereo_buffers(&ordinary_interleaved, false));
        assert_eq!(audio_unit_render_sample_rate(&format, 44_100.0), 11_025.0);

        let left = [0x34, 0x12, 0x78, 0x56];
        let right = [0xdc, 0xed, 0xc4, 0xe3];
        let interleaved = interleave_planar_pcm(&[&left, &right], 2).unwrap();
        let interleaved_format = interleaved_format_for_planar_pcm(format);
        let (decoded_format, sample_rate, decoded) =
            decode_buffer_from_bytes(&interleaved_format, &interleaved, &[]);

        assert_eq!(decoded_format, AL_FORMAT_STEREO16);
        assert_eq!(sample_rate, 11_025);
        assert_eq!(decoded, [0x34, 0x12, 0xdc, 0xed, 0x78, 0x56, 0xc4, 0xe3]);
    }

    #[test]
    fn planar_f32_stereo_is_interleaved_and_decoded() {
        assert!(uses_planar_stereo_buffers(
            &planar_f32_stereo_format(),
            false
        ));
        let left = [0.5_f32, -0.25]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect::<Vec<_>>();
        let right = [-0.5_f32, 0.75]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect::<Vec<_>>();
        let interleaved = interleave_planar_pcm(&[&left, &right], 4).unwrap();
        let format = interleaved_format_for_planar_pcm(planar_f32_stereo_format());
        let (decoded_format, sample_rate, decoded) =
            decode_buffer_from_bytes(&format, &interleaved, &[]);
        let decoded_samples = decoded
            .chunks_exact(2)
            .map(|sample| i16::from_le_bytes([sample[0], sample[1]]))
            .collect::<Vec<_>>();

        assert_eq!(decoded_format, AL_FORMAT_STEREO16);
        assert_eq!(sample_rate, 48_000);
        assert_eq!(decoded_samples, [16_383, -16_383, -8_191, 24_575]);
    }

    #[test]
    fn planar_pcm_rejects_mismatched_plane_lengths() {
        assert_eq!(interleave_planar_pcm(&[&[1, 2], &[3]], 1), None);
    }
}
