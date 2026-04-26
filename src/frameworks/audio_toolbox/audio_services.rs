/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 * If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `AudioServices.h` (Audio Services)

use std::collections::HashMap;

use crate::audio;
use crate::audio::openal as al;
use crate::audio::openal::al_types::*;
use crate::audio::openal::{OpenAL, OpenALManager};

use super::audio_queue::decode_buffer;
use crate::dyld::{export_c_func, FunctionExports};
use crate::frameworks::carbon_core::{paramErr, OSStatus};
use crate::frameworks::core_audio_types::{debug_fourcc, fourcc, AudioStreamBasicDescription};
use crate::frameworks::core_foundation::cf_url::CFURLRef;
use crate::frameworks::foundation::ns_url::to_rust_path;
use crate::mem::{ConstVoidPtr, GuestUSize, MutPtr, MutVoidPtr};
use crate::Environment;

type AudioServicesPropertyID = u32;
pub type SystemSoundID = u32;
type AudioServicesSystemSoundCompletionProc = u32; // guest function pointer

const kAudioServicesUnsupportedPropertyError: OSStatus = fourcc(b"pty?") as _;
const kAudioServicesBadSystemSoundIDError: OSStatus = fourcc(b"!ids") as _;
const kAudioServicesSystemSoundUnspecifiedError: OSStatus = -1500;

const kSystemSoundID_Vibrate: SystemSoundID = 0x00000FFF;
const kSystemSoundID_UserPreferredAlert: SystemSoundID = 0x00001000;
const INITIAL_SYSTEM_SOUND_ID: SystemSoundID = 0x1001;

pub struct SystemSoundData {
    al_source: ALuint,
    al_buffer: ALuint,
}

// Комбинируем логику из форка (фоллбэк) и оригинала (реальное воспроизведение)
pub enum SoundEntry {
    Real(SystemSoundData),
    Dummy, // Используется, если парсер не смог открыть файл (из форка)
}

pub struct State {
    pub system_sounds: HashMap<SystemSoundID, SoundEntry>,
    pub next_sound_id: SystemSoundID,
}

impl Default for State {
    fn default() -> Self {
        Self {
            system_sounds: Default::default(),
            next_sound_id: INITIAL_SYSTEM_SOUND_ID,
        }
    }
}

impl State {
    pub fn get(framework_state: &mut crate::frameworks::State) -> &mut Self {
        &mut framework_state.audio_toolbox.audio_services
    }

    fn get_with_context<'s, 'm: 's>(
        framework_state: &'s mut crate::frameworks::State,
        manager: &'m mut OpenALManager,
    ) -> (&'s mut Self, OpenAL<'s>) {
        let toolbox = &mut framework_state.audio_toolbox;
        (
            &mut toolbox.audio_services,
            toolbox.al_context.make_al_context_current(manager),
        )
    }
}

fn AudioServicesGetProperty(
    _env: &mut Environment,
    in_property_id: AudioServicesPropertyID,
    _in_specifier_size: u32,
    _in_specifier: ConstVoidPtr,
    _io_property_data_size: MutPtr<u32>,
    _out_property_data: MutVoidPtr,
) -> OSStatus {
    // Crash Bandicoot Nitro Kart 3D пытается использовать это свойство.
    if in_property_id == 0xfff {
        return kAudioServicesUnsupportedPropertyError;
    }
    
    // В форке используется мягкий подход без паники
    log!(
        "AudioServicesGetProperty: property {} is unimplemented",
        debug_fourcc(in_property_id)
    );
    kAudioServicesUnsupportedPropertyError
}

fn AudioServicesSetProperty(
    _env: &mut Environment,
    in_property_id: AudioServicesPropertyID,
    _in_specifier_size: u32,
    _in_specifier: ConstVoidPtr,
    _in_property_data_size: u32,
    _in_property_data: ConstVoidPtr,
) -> OSStatus {
    log!(
        "AudioServicesSetProperty: property {} is unimplemented",
        debug_fourcc(in_property_id)
    );
    0
}

fn AudioServicesCreateSystemSoundID(
    env: &mut Environment,
    in_file_url: CFURLRef,
    out_system_sound_id: MutPtr<SystemSoundID>,
) -> OSStatus {
    if in_file_url.is_null() {
        log!("Warning: AudioServicesCreateSystemSoundID called with NULL in_file_url");
        if !out_system_sound_id.is_null() { env.mem.write(out_system_sound_id, 0); }
        return paramErr;
    }

    let path = to_rust_path(env, in_file_url);
    log!("AudioServicesCreateSystemSoundID: opening {:?}", path);
    
    let audio_file_result = audio::AudioFile::open_for_reading(&path, &env.fs);
    
    // Подход из форка + декодирование из оригинала
    let sound_entry = match audio_file_result {
        Ok(mut audio_file) => {
            let mut data = vec![0; audio_file.byte_count().try_into().unwrap()];
            let format = AudioStreamBasicDescription::from_audio_description(audio_file.audio_description());
            let size = audio_file.read_bytes(0, data.as_mut_slice()).unwrap();
            let tmp = env.mem.alloc(size as GuestUSize);
            env.mem
                .bytes_at_mut(tmp.cast(), size as GuestUSize)
                .copy_from_slice(data.as_slice());
                
            let (al_format, al_frequency, decoded_data) =
                decode_buffer(&env.mem, &format, tmp.cast(), size as GuestUSize);
            env.mem.free(tmp.cast());
            log!(
                "AudioServicesCreateSystemSoundID: {:?} -> al_format=0x{:x}, al_freq={}, pcm_bytes={}",
                path, al_format, al_frequency, decoded_data.len()
            );

            let (_, context) = State::get_with_context(&mut env.framework_state, &mut env.openal_manager);

            let mut al_source = 0;
            unsafe {
                context.GenSources(1, &mut al_source);
                assert!(context.GetError() == 0);
            }

            let mut al_buffer = 0;
            unsafe {
                context.GenBuffers(1, &mut al_buffer);
                context.BufferData(
                    al_buffer,
                    al_format,
                    decoded_data.as_ptr() as *const ALvoid,
                    decoded_data.len().try_into().unwrap(),
                    al_frequency,
                );
                context.Sourcei(al_source, al::AL_BUFFER, al_buffer.try_into().unwrap());
                assert!(context.GetError() == 0);
            }
            
            SoundEntry::Real(SystemSoundData { al_source, al_buffer })
        },
        Err(_) => {
            log!("Warning: AudioServicesCreateSystemSoundID failed to open file {:?}. Generating silent/dummy sound ID.", path);
            SoundEntry::Dummy
        }
    };

    let state = State::get(&mut env.framework_state);
    
    if state.next_sound_id <= kSystemSoundID_UserPreferredAlert {
        state.next_sound_id = kSystemSoundID_UserPreferredAlert + 1;
    }
    
    let id = state.next_sound_id;
    state.next_sound_id += 1;

    state.system_sounds.insert(id, sound_entry);

    if !out_system_sound_id.is_null() {
        env.mem.write(out_system_sound_id, id);
    }
    
    log!("AudioServicesCreateSystemSoundID: id={} for {:?}", id, path);
    0 
}

fn AudioServicesDisposeSystemSoundID(
    env: &mut Environment,
    in_system_sound_id: SystemSoundID,
) -> OSStatus {
    let (state, context) = State::get_with_context(&mut env.framework_state, &mut env.openal_manager);

    if let Some(entry) = state.system_sounds.remove(&in_system_sound_id) {
        if let SoundEntry::Real(SystemSoundData { al_source, al_buffer }) = entry {
            unsafe {
                context.SourceStop(al_source);
                context.DeleteSources(1, &al_source as *const ALuint);
                context.DeleteBuffers(1, &al_buffer as *const ALuint);
                assert!(context.GetError() == 0);
            }
        }
        0
    } else {
        log!("Tried to dispose of invalid/unknown system sound {}!", in_system_sound_id);
        kAudioServicesSystemSoundUnspecifiedError
    }
}

fn AudioServicesPlaySystemSound(env: &mut Environment, in_system_sound_id: SystemSoundID) {
    log!("AudioServicesPlaySystemSound({})", in_system_sound_id);
    if in_system_sound_id == kSystemSoundID_Vibrate {
        log!("TODO: vibration (AudioServicesPlaySystemSound)");
        return;
    } else if in_system_sound_id == kSystemSoundID_UserPreferredAlert {
        log!("TODO: alert sound (AudioServicesPlaySystemSound)");
        return;
    }

    let (state, context) = State::get_with_context(&mut env.framework_state, &mut env.openal_manager);
    
    if let Some(entry) = state.system_sounds.get(&in_system_sound_id) {
        match entry {
            SoundEntry::Real(SystemSoundData { al_source, al_buffer: _ }) => {
                log_dbg!("AudioToolbox: Playing system sound ID: {}", in_system_sound_id);
                unsafe {
                    let al_source = *al_source;
                    context.SourcePlay(al_source);
                    assert!(context.GetError() == 0);
                    let mut al_state: i32 = 0;
                    context.GetSourcei(al_source, al::AL_SOURCE_STATE, &mut al_state as *mut i32);
                    assert!(context.GetError() == 0);
                    assert!(
                        al_state == al::AL_PLAYING,
                        "Expected AL_PLAYING after SourcePlay, got {:#x}",
                        al_state
                    );
                }
            },
            SoundEntry::Dummy => {
                log!("AudioToolbox: Attempted to play Dummy system sound ID: {} (Skipping gracefully)", in_system_sound_id);
            }
        }
    } else {
        log!("AudioToolbox: Attempted to play unknown system sound ID: {} (Skipping gracefully)", in_system_sound_id);
    }
}

fn AudioServicesPlayAlertSound(env: &mut Environment, in_system_sound_id: SystemSoundID) {
    AudioServicesPlaySystemSound(env, in_system_sound_id);
}

fn AudioServicesAddSystemSoundCompletion(
    _env: &mut Environment,
    _in_system_sound_id: SystemSoundID,
    _in_run_loop: MutVoidPtr,
    _in_run_loop_mode: MutVoidPtr,
    _in_completion_routine: AudioServicesSystemSoundCompletionProc,
    _in_client_data: MutVoidPtr,
) -> OSStatus {
    log!("AudioToolbox: AudioServicesAddSystemSoundCompletion stubbed");
    0
}

fn AudioServicesRemoveSystemSoundCompletion(
    _env: &mut Environment,
    _in_system_sound_id: SystemSoundID,
) {
    log!("AudioToolbox: AudioServicesRemoveSystemSoundCompletion stubbed");
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(AudioServicesGetProperty(_, _, _, _, _)),
    export_c_func!(AudioServicesSetProperty(_, _, _, _, _)),
    export_c_func!(AudioServicesCreateSystemSoundID(_, _)),
    export_c_func!(AudioServicesDisposeSystemSoundID(_)),
    export_c_func!(AudioServicesPlaySystemSound(_)),
    export_c_func!(AudioServicesPlayAlertSound(_)),
    export_c_func!(AudioServicesAddSystemSoundCompletion(_, _, _, _, _)),
    export_c_func!(AudioServicesRemoveSystemSoundCompletion(_)),
];
