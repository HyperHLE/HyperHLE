/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 * If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `AudioServices.h` (Audio Services)

use crate::audio;
use crate::dyld::{export_c_func, FunctionExports};
use crate::frameworks::carbon_core::OSStatus;
use crate::frameworks::core_audio_types::fourcc;
use crate::frameworks::core_foundation::cf_url::CFURLRef;
use crate::frameworks::foundation::ns_url::to_rust_path;
use crate::mem::{ConstVoidPtr, MutPtr, MutVoidPtr};
use crate::Environment;
use std::collections::HashMap;

type AudioServicesPropertyID = u32;
pub type SystemSoundID = u32;
type AudioServicesSystemSoundCompletionProc = u32; // guest function pointer

const kAudioServicesUnsupportedPropertyError: OSStatus = fourcc(b"pty?") as _;
const kAudioServicesBadSystemSoundIDError: OSStatus = fourcc(b"!ids") as _;

const kSystemSoundID_Vibrate: SystemSoundID = 0x00000FFF;
const kSystemSoundID_UserPreferredAlert: SystemSoundID = 0x00001000;

#[derive(Default)]
pub struct State {
    pub system_sounds: HashMap<SystemSoundID, audio::AudioFile>,
    pub next_sound_id: SystemSoundID,
}

impl State {
    pub fn get(framework_state: &mut crate::frameworks::State) -> &mut Self {
        // Ensure this field exists in your framework_state definition
        &mut framework_state.audio_toolbox.audio_services
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
    if in_property_id == 0xfff {
        kAudioServicesUnsupportedPropertyError
    } else {
        log!("AudioServicesGetProperty: property {} is unimplemented", in_property_id);
        0
    }
}

fn AudioServicesSetProperty(
    _env: &mut Environment,
    in_property_id: AudioServicesPropertyID,
    _in_specifier_size: u32,
    _in_specifier: ConstVoidPtr,
    _in_property_data_size: u32,
    _in_property_data: ConstVoidPtr,
) -> OSStatus {
    log!("AudioServicesSetProperty: property {} is unimplemented", in_property_id);
    0
}

fn AudioServicesCreateSystemSoundID(
    env: &mut Environment,
    in_file_url: CFURLRef, // Changed from `id` to `CFURLRef` to match typical CoreFoundation usage
    out_system_sound_id: MutPtr<SystemSoundID>,
) -> OSStatus {
    return_if_null!(in_file_url);

    let path = to_rust_path(env, in_file_url);
    
    // Open the audio file
    let audio_file = match audio::AudioFile::open_for_reading(path.clone(), &env.fs) {
        Ok(file) => file,
        Err(e) => {
            log!("Warning: AudioServicesCreateSystemSoundID failed to open file {:?}: {:?}", path, e);
            // fnfErr (File not found) or unspec error
            return -43; 
        }
    };

    let state = State::get(&mut env.framework_state);
    
    // Start generating IDs from 4096 to avoid colliding with built-in system IDs
    if state.next_sound_id < 4096 {
        state.next_sound_id = 4096;
    }
    
    let id = state.next_sound_id;
    state.next_sound_id += 1;

    state.system_sounds.insert(id, audio_file);

    if !out_system_sound_id.is_null() {
        env.mem.write(out_system_sound_id, id);
    }
    
    log_dbg!("AudioToolbox: AudioServicesCreateSystemSoundID created ID {} for url {:?}", id, in_file_url);
    0
}

fn AudioServicesDisposeSystemSoundID(
    env: &mut Environment,
    in_system_sound_id: SystemSoundID,
) -> OSStatus {
    let state = State::get(&mut env.framework_state);
    
    if state.system_sounds.remove(&in_system_sound_id).is_some() {
        log_dbg!("AudioToolbox: Disposed system sound ID {}", in_system_sound_id);
        0
    } else {
        log!("AudioToolbox: Attempted to dispose invalid system sound ID {}", in_system_sound_id);
        kAudioServicesBadSystemSoundIDError
    }
}

fn AudioServicesPlaySystemSound(env: &mut Environment, in_system_sound_id: SystemSoundID) {
    if in_system_sound_id == kSystemSoundID_Vibrate {
        log!("TODO: vibration (AudioServicesPlaySystemSound)");
        return;
    } else if in_system_sound_id == kSystemSoundID_UserPreferredAlert {
        log!("TODO: alert sound (AudioServicesPlaySystemSound)");
        return;
    }

    let state = State::get(&mut env.framework_state);
    
    if let Some(audio_file) = state.system_sounds.get(&in_system_sound_id) {
        log_dbg!("AudioToolbox: Playing system sound ID: {}", in_system_sound_id);
        
        // TODO: Pass the audio_file data to your specific audio mixing/playback backend!
        // This usually looks something like this depending on your emulator architecture:
        // env.audio_mixer.play_buffer(audio_file.clone());
        // OR
        // audio::play_system_sound(&mut env.audio_queue, audio_file);
        
    } else {
        log!("AudioToolbox: Attempted to play unknown system sound ID: {}", in_system_sound_id);
    }
}

fn AudioServicesPlayAlertSound(env: &mut Environment, in_system_sound_id: SystemSoundID) {
    // PlayAlertSound acts exactly like PlaySystemSound, just with potential vibration overrides on physical devices
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
    log!("AudioToolbox: AudioServicesAddSystemSoundCompletion stubbed (completion will never fire)");
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
