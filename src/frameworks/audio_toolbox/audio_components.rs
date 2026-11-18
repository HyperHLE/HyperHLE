/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 * If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `AudioComponent.h` (Audio Component Services)

use std::collections::HashMap;
use std::time::Instant;

use crate::abi::GuestFunction;
use crate::audio::openal::al_types::ALuint;
use crate::dyld::FunctionExports;
use crate::environment::Environment;
use crate::export_c_func;
use crate::frameworks::carbon_core::{paramErr, OSStatus};
use crate::frameworks::core_audio_types::{
    fourcc, kAudioFormatFlagIsAlignedHigh, kAudioFormatFlagIsFloat, kAudioFormatFlagIsPacked,
    kAudioFormatFlagIsSignedInteger, kAudioFormatLinearPCM, AudioStreamBasicDescription,
};
use crate::mem::{ConstPtr, ConstVoidPtr, MutPtr, MutVoidPtr, SafeRead, SafeWrite};

const kAudioUnitType_Output: u32 = fourcc(b"auou");
const kAudioUnitSubType_RemoteIO: u32 = fourcc(b"rioc");
const kAudioUnitManufacturer_Apple: u32 = fourcc(b"appl");
const kAudioUnitType_Mixer: u32 = fourcc(b"aumx");
const kAudioUnitSubType_3DMixer: u32 = fourcc(b"3dem");
// Apple `AUComponent.h`:
//   kAudioUnitSubType_MultiChannelMixer = 'mcmx'
//   kAudioUnitType_FormatConverter      = 'aufc'
//   kAudioUnitSubType_AUConverter       = 'conv'
// The MultiChannelMixer mixes several input buses (each driven by a render
// callback) down to one output, exactly like the 3D Mixer path we already
// implement via `mixer_buses`. The format converter simply adapts a stream
// format between two units; for HLE purposes it behaves as a pass-through
// unit that pulls from its input render callback and hands samples on.
// Games such as Plants vs. Zombies build an AUGraph out of a
// MultiChannelMixer plus AUConverter units, so both must be instantiable or
// no sound is ever produced.
const kAudioUnitSubType_MultiChannelMixer: u32 = fourcc(b"mcmx");
const kAudioUnitType_FormatConverter: u32 = fourcc(b"aufc");
const kAudioUnitSubType_AUConverter: u32 = fourcc(b"conv");
// Apple `AUComponent.h`: kAudioUnitType_Effect = 'aufx'
const kAudioUnitType_Effect: u32 = fourcc(b"aufx");

// --- 3D Mixer Structures ---
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct MixerDistanceParams {
    pub reference_distance: f32,
    pub maximum_distance: f32,
    pub rolloff_factor: f32,
}

// ИСПРАВЛЕНИЕ: Реализация SafeRead для возможности чтения из памяти
unsafe impl SafeRead for MixerDistanceParams {}

#[derive(Clone)]
pub struct MixerBusState {
    pub al_source: Option<ALuint>,
    pub position: [f32; 3], // X, Y, Z
    pub volume: f32,
    pub distance_params: MixerDistanceParams,
    pub render_callback: Option<AURenderCallbackStruct>,
    pub stream_format: Option<AudioStreamBasicDescription>,
    /// `k3DMixerParam_PlaybackRate`, as a factor of the normal rate.
    pub playback_rate: f32,
    pub last_render_time: Option<Instant>,
    /// Running total of frames handed to this bus's render callback, used as
    /// the `mSampleTime` of the `AudioTimeStamp` we pass to it. Must advance
    /// monotonically: callbacks that pace a streamer against the audio clock
    /// (FMOD, used by Geometry Dash) treat a non-advancing or absent timestamp
    /// as "no time has passed" and mix nothing.
    pub sample_time: f64,
}

impl Default for MixerBusState {
    fn default() -> Self {
        Self {
            al_source: None,
            position: [0.0, 0.0, 0.0],
            volume: 1.0,
            distance_params: MixerDistanceParams {
                reference_distance: 1.0,
                maximum_distance: 100000.0,
                rolloff_factor: 1.0,
            },
            render_callback: None,
            stream_format: None,
            playback_rate: 1.0,
            last_render_time: None,
            sample_time: 0.0,
        }
    }
}
// ---------------------------

#[derive(Default)]
pub struct State {
    pub audio_component: AudioComponent,
    pub audio_component_instances:
        HashMap<AudioComponentInstance, AudioComponentInstanceHostObject>,
    /// Описание (type, subtype, manufacturer) последнего компонента,
    /// найденного `AudioComponentFindNext`. Используется, чтобы заполнить
    /// `AudioComponentInstanceHostObject::component_desc` при создании
    /// инстанса — для ответов на `kAudioUnitProperty_ClassInfo`.
    pub pending_component_desc: Option<(u32, u32, u32)>,
    /// Connections made with `kAudioUnitProperty_MakeConnection`, from
    /// (destination unit, input bus) to the source unit feeding it.
    pub connections: HashMap<(AudioComponentInstance, u32), AudioComponentInstance>,
}
impl State {
    pub fn get(framework_state: &mut crate::frameworks::State) -> &mut Self {
        &mut framework_state.audio_toolbox.audio_components
    }
}

#[derive(Clone)]
pub struct AudioComponentInstanceHostObject {
    pub started: bool,
    pub initialized: bool,
    pub render_thread: Option<crate::environment::ThreadId>,
    pub maximum_frames_per_slice: u32,
    pub global_stream_format: AudioStreamBasicDescription,
    pub input_stream_format: Option<AudioStreamBasicDescription>,
    pub output_stream_format: Option<AudioStreamBasicDescription>,
    pub render_callback: Option<AURenderCallbackStruct>,
    pub last_render_time: Option<Instant>,
    pub al_source: Option<ALuint>,
    pub is_running_handler: bool,

    /// Running total of frames handed to this unit's render callback, used as
    /// the `mSampleTime` of the `AudioTimeStamp` we pass to it (see
    /// `MixerBusState::sample_time`).
    pub sample_time: f64,

    /// Property listeners registered through `AudioUnitAddPropertyListener`.
    /// The callback is kept as a guest function and invoked with the same
    /// ref-con that was supplied during registration.
    pub property_listeners: Vec<(u32, GuestFunction, MutVoidPtr)>,

    // --- 3D Mixer State ---
    pub is_3d_mixer: bool,
    pub mixer_buses: HashMap<u32, MixerBusState>,

    /// RemoteIO input element (element=1, scope=Input) enabled — the unit is
    /// being used to capture microphone audio. Filled by
    /// `AudioUnitRender` with real host-mic samples when available.
    pub mic_input_enabled: bool,

    /// Описание компонента (type, subtype, manufacturer), с которым был
    /// создан инстанс (из `AudioComponentFindNext` или узла AUGraph).
    /// Нужно для `kAudioUnitProperty_ClassInfo` — гость идентифицирует юнит
    /// через CFDictionary с этими значениями.
    pub component_desc: Option<(u32, u32, u32)>,

    /// Сырые байты `kAudioUnitProperty_AudioChannelLayout`, записанные
    /// гостем через `AudioUnitSetProperty`, ключ — (scope, element).
    /// Возвращаются обратно через `AudioUnitGetProperty`.
    pub audio_channel_layouts: HashMap<(u32, u32), Vec<u8>>,

    /// Флаги `kAudioUnitProperty_ShouldAllocateBuffer` (property 51),
    /// ключ — (scope, element); GET без SET отвечает 1 (как раньше).
    pub should_allocate_buffers: HashMap<(u32, u32), u32>,

    /// Render notify callbacks registered through
    /// `AudioUnitAddRenderNotify`. Real CoreAudio calls these before and
    /// after every render operation (with the corresponding action flag
    /// set); engines like FMOD use them to drive their mixing from inside
    /// the render cycle, so they must actually be invoked.
    pub render_notifies: Vec<(AURenderCallback, ConstVoidPtr)>,
}

impl Default for AudioComponentInstanceHostObject {
    fn default() -> Self {
        AudioComponentInstanceHostObject {
            started: false,
            initialized: false,
            render_thread: None,
            maximum_frames_per_slice: 1024,
            global_stream_format: AudioStreamBasicDescription {
                sample_rate: 44100.0,
                format_id: kAudioFormatLinearPCM,
                format_flags: kAudioFormatFlagIsFloat
                    | kAudioFormatFlagIsSignedInteger
                    | kAudioFormatFlagIsPacked
                    | kAudioFormatFlagIsAlignedHigh,
                bytes_per_packet: 4,
                frames_per_packet: 1,
                bytes_per_frame: 4,
                channels_per_frame: 2,
                bits_per_channel: 32,
                _reserved: 0,
            },
            input_stream_format: None,
            output_stream_format: None,
            render_callback: None,
            last_render_time: None,
            al_source: None,
            is_running_handler: false,
            sample_time: 0.0,
            property_listeners: Vec::new(),
            is_3d_mixer: false,
            mixer_buses: HashMap::new(),
            mic_input_enabled: false,
            component_desc: None,
            audio_channel_layouts: HashMap::new(),
            should_allocate_buffers: HashMap::new(),
            render_notifies: Vec::new(),
        }
    }
}

#[derive(Copy, Clone, Debug)]
#[repr(C, packed)]
#[allow(dead_code)]
pub struct AURenderCallbackStruct {
    pub input_proc: AURenderCallback,
    pub input_proc_ref_con: ConstVoidPtr,
}
unsafe impl SafeRead for AURenderCallbackStruct {}

/// Token returned by `AudioComponentFindNext`.
///
/// It carries the descriptor of the component it stands for, because the guest
/// passes it straight back in to continue the search: Apple's iterator skips
/// the component it returned last time.
#[repr(C, packed)]
#[derive(Debug, Default)]
pub struct OpaqueAudioComponent {
    component_type: u32,
    component_sub_type: u32,
    component_manufacturer: u32,
}
unsafe impl SafeRead for OpaqueAudioComponent {}

type AudioComponent = MutPtr<OpaqueAudioComponent>;
pub type AURenderCallback = GuestFunction;

#[repr(C, packed)]
pub struct OpaqueAudioComponentInstance {
    _pad: u8,
}
unsafe impl SafeRead for OpaqueAudioComponentInstance {}

pub type AudioComponentInstance = MutPtr<OpaqueAudioComponentInstance>;

#[repr(C, packed)]
struct AudioComponentDescription {
    component_type: u32,
    component_sub_type: u32,
    component_manufacturer: u32,
    component_flags: u32,
    component_flags_mask: u32,
}
unsafe impl SafeRead for AudioComponentDescription {}

/// `AudioComponentDescription` fields are matched verbatim, except that 0
/// matches anything (Apple's `AudioComponent.h`: "the type, subtype and
/// manufacturer fields can be set to 0, in which case they match any value").
fn component_field_matches(wanted: u32, available: u32) -> bool {
    wanted == 0 || wanted == available
}

/// Components that `AudioComponentFindNext` reports as available.
///
/// touchHLE implements one pass-through audio unit: it pulls frames from the
/// render callback of each input bus and mixes them, which covers output
/// units, mixers and format converters. Effects are also worth matching
/// because apps that install one (a reverb on a mixer's output, say) behave
/// the same way whether the effect passes audio through untouched or is
/// bypassed -- but they abort or fall back to silence if the component is
/// missing. Sampler/instrument units are deliberately absent: they synthesise
/// their own audio rather than pulling from an input, so passing them through
/// would produce silence instead of the samples the app asked for.
const AVAILABLE_COMPONENTS: &[(u32, u32)] = &[
    // kAudioUnitType_Output
    (kAudioUnitType_Output, kAudioUnitSubType_RemoteIO), // 'rioc'
    (kAudioUnitType_Output, fourcc(b"vpio")),            // VoiceProcessingIO
    (kAudioUnitType_Output, fourcc(b"genr")),            // GenericOutput
    (kAudioUnitType_Output, fourcc(b"sys ")),            // SystemOutput
    (kAudioUnitType_Output, fourcc(b"ahal")),            // HALOutput
    (kAudioUnitType_Output, fourcc(b"def ")),            // DefaultOutput
    // kAudioUnitType_Mixer
    (kAudioUnitType_Mixer, kAudioUnitSubType_3DMixer),
    (kAudioUnitType_Mixer, kAudioUnitSubType_MultiChannelMixer),
    (kAudioUnitType_Mixer, fourcc(b"smxr")), // StereoMixer
    (kAudioUnitType_Mixer, fourcc(b"mxmx")), // MatrixMixer
    // kAudioUnitType_FormatConverter
    (
        kAudioUnitType_FormatConverter,
        kAudioUnitSubType_AUConverter,
    ),
    (kAudioUnitType_FormatConverter, fourcc(b"vare")), // VariSpeed
    (kAudioUnitType_FormatConverter, fourcc(b"tmpt")), // TimePitch
    (kAudioUnitType_FormatConverter, fourcc(b"nutp")), // NewTimePitch
    // kAudioUnitType_Effect
    (kAudioUnitType_Effect, fourcc(b"rvb2")), // Reverb2
    (kAudioUnitType_Effect, fourcc(b"dely")), // Delay
    (kAudioUnitType_Effect, fourcc(b"nbeq")), // NBandEQ
    (kAudioUnitType_Effect, fourcc(b"bpas")), // Bandpass
    (kAudioUnitType_Effect, fourcc(b"hpas")), // Highpass
    (kAudioUnitType_Effect, fourcc(b"lpas")), // Lowpass
    (kAudioUnitType_Effect, fourcc(b"dist")), // Distortion
    (kAudioUnitType_Effect, fourcc(b"dcmp")), // DynamicsProcessor
    (kAudioUnitType_Effect, fourcc(b"lmtr")), // PeakLimiter
];

fn find_component_desc(
    desc: &AudioComponentDescription,
    after: Option<(u32, u32)>,
) -> Option<(u32, u32)> {
    let mut passed_after = after.is_none();
    for &(component_type, sub_type) in AVAILABLE_COMPONENTS {
        if !component_field_matches(desc.component_type, component_type)
            || !component_field_matches(desc.component_sub_type, sub_type)
            || !component_field_matches(desc.component_manufacturer, kAudioUnitManufacturer_Apple)
        {
            continue;
        }
        if !passed_after {
            if after == Some((component_type, sub_type)) {
                passed_after = true;
            }
            continue;
        }
        return Some((component_type, sub_type));
    }
    None
}

fn AudioComponentFindNext(
    env: &mut Environment,
    in_component: AudioComponent,
    in_desc: ConstPtr<AudioComponentDescription>,
) -> AudioComponent {
    let desc = env.mem.read::<_, false>(in_desc);

    // Calling with a component found earlier continues the search, so a unit
    // whose first match the app did not like can still reach the next one.
    let after = if in_component.is_null() {
        None
    } else {
        let previous = env.mem.read::<_, true>(in_component);
        Some((previous.component_type, previous.component_sub_type))
    };

    let Some((component_type, sub_type)) = find_component_desc(&desc, after) else {
        let (want_type, want_sub_type, want_manufacturer) = (
            desc.component_type,
            desc.component_sub_type,
            desc.component_manufacturer,
        );
        log!(
            "AudioComponentFindNext: no component for type={:#010x} sub_type={:#010x} manufacturer={:#010x}",
            want_type,
            want_sub_type,
            want_manufacturer,
        );
        return MutPtr::null();
    };
    let component_manufacturer = kAudioUnitManufacturer_Apple;

    let state = State::get(&mut env.framework_state);
    if state.audio_component.is_null() {
        state.audio_component = env.mem.alloc_and_write(OpaqueAudioComponent::default());
    }
    env.mem.write(
        state.audio_component,
        OpaqueAudioComponent {
            component_type,
            component_sub_type: sub_type,
            component_manufacturer,
        },
    );

    log_dbg!(
        "AudioComponentFindNext: matched type=0x{:08x} sub_type=0x{:08x} -> {:?}",
        component_type,
        sub_type,
        state.audio_component
    );
    state.pending_component_desc = Some((component_type, sub_type, component_manufacturer));
    state.audio_component
}

fn AudioComponentInstanceNew(
    env: &mut Environment,
    in_component: AudioComponent,
    out_instance: MutPtr<AudioComponentInstance>,
) -> OSStatus {
    if in_component.is_null() {
        return paramErr;
    }

    let mut host_object = AudioComponentInstanceHostObject::default();
    host_object.is_3d_mixer = true;
    // Описание компонента, запомненное последним AudioComponentFindNext
    // (см. State::pending_component_desc) — для kAudioUnitProperty_ClassInfo.
    host_object.component_desc = State::get(&mut env.framework_state).pending_component_desc;

    let guest_instance: AudioComponentInstance = env
        .mem
        .alloc_and_write(OpaqueAudioComponentInstance { _pad: 0 });

    State::get(&mut env.framework_state)
        .audio_component_instances
        .insert(guest_instance, host_object);

    env.mem.write(out_instance, guest_instance);

    log_dbg!(
        "AudioComponentInstanceNew(component={:?}) -> instance={:?}",
        in_component,
        guest_instance
    );
    0
}

/// Создать AudioUnit instance напрямую (используется из
//`au_graph::AUGraphOpen`),
/// минуя обычный путь `AudioComponentInstanceNew`.
/// `component_desc` — (type, subtype, manufacturer) узла графа, если известны
/// (иначе None; тогда ClassInfo вернёт нулевые значения).
pub fn create_audio_unit_instance(
    env: &mut Environment,
    component_desc: Option<(u32, u32, u32)>,
) -> AudioComponentInstance {
    let mut host_object = AudioComponentInstanceHostObject::default();
    host_object.is_3d_mixer = true;
    host_object.component_desc = component_desc;
    let guest_instance: AudioComponentInstance = env
        .mem
        .alloc_and_write(OpaqueAudioComponentInstance { _pad: 0 });
    State::get(&mut env.framework_state)
        .audio_component_instances
        .insert(guest_instance, host_object);
    guest_instance
}

fn AudioComponentInstanceDispose(
    env: &mut Environment,
    in_instance: AudioComponentInstance,
) -> OSStatus {
    log_dbg!("AudioComponentInstanceDispose({:?})", in_instance);
    if in_instance.is_null() {
        return paramErr;
    }

    let state = State::get(&mut env.framework_state);
    state.audio_component_instances.remove(&in_instance);
    state
        .connections
        .retain(|&(dest, _), src| dest != in_instance && *src != in_instance);
    env.mem.free(in_instance.cast());

    0
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(AudioComponentFindNext(_, _)),
    export_c_func!(AudioComponentInstanceNew(_, _)),
    export_c_func!(AudioComponentInstanceDispose(_)),
];

#[cfg(test)]
mod component_search_tests {
    use super::*;

    fn desc(component_type: u32, sub_type: u32, manufacturer: u32) -> AudioComponentDescription {
        AudioComponentDescription {
            component_type,
            component_sub_type: sub_type,
            component_manufacturer: manufacturer,
            component_flags: 0,
            component_flags_mask: 0,
        }
    }

    /// Getting the exact RemoteIO unit apps ask for by name.
    #[test]
    fn finds_remote_io_by_name() {
        let want = desc(
            fourcc(b"auou"),
            fourcc(b"rioc"),
            kAudioUnitManufacturer_Apple,
        );
        assert_eq!(
            find_component_desc(&want, None),
            Some((fourcc(b"auou"), fourcc(b"rioc")))
        );
    }

    /// `AudioComponent.h` says a zero field matches anything, so an app that
    /// lets the system pick the first output unit gets one:
    /// "the type, subtype and manufacturer fields can be set to 0, in which
    /// case they match any value".
    #[test]
    fn zero_fields_match_any_value() {
        let any_output = desc(0, fourcc(b"rioc"), 0);
        assert_eq!(
            find_component_desc(&any_output, None),
            Some((fourcc(b"auou"), fourcc(b"rioc")))
        );
        let anything = desc(0, 0, 0);
        assert_eq!(
            find_component_desc(&anything, None),
            AVAILABLE_COMPONENTS.first().copied()
        );
    }

    /// An app that installs a reverb keeps running because the component
    /// exists; the effect itself passes audio through untouched.
    #[test]
    fn finds_effects() {
        let reverb = desc(fourcc(b"aufx"), fourcc(b"rvb2"), fourcc(b"appl"));
        assert_eq!(
            find_component_desc(&reverb, None),
            Some((fourcc(b"aufx"), fourcc(b"rvb2")))
        );
    }

    /// Sampler units synthesise their own audio, so reporting one would hand
    /// the app silence instead of its samples.
    #[test]
    fn does_not_report_instrument_units() {
        let sampler = desc(fourcc(b"ausn"), fourcc(b"samp"), fourcc(b"appl"));
        assert_eq!(find_component_desc(&sampler, None), None);
    }

    /// Iterating continues after the component the app passed in.
    #[test]
    fn continues_after_the_previous_match() {
        let any_mixer = desc(fourcc(b"aumx"), 0, 0);
        let first = find_component_desc(&any_mixer, None).unwrap();
        let second = find_component_desc(&any_mixer, Some(first)).unwrap();
        assert_ne!(first, second);
        assert_eq!(second.1, fourcc(b"mcmx"));
    }
}
