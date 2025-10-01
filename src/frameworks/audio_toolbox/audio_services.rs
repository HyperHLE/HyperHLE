/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `AudioServices.h` (Audio Services)

use crate::dyld::{export_c_func, FunctionExports};
use crate::frameworks::carbon_core::OSStatus;
use crate::frameworks::core_audio_types::fourcc;
use crate::mem::{MutPtr, MutVoidPtr};
use crate::Environment;
use crate::frameworks::core_foundation::cf_run_loop::CFRunLoopRef;
use crate::frameworks::core_foundation::cf_string::CFStringRef;
use crate::frameworks::core_foundation::cf_url::CFURLRef;

/// Usually a FourCC.
type AudioServicesPropertyID = u32;
type AudioServicesSystemSoundCompletionProc = u32;
type SystemSoundID = u32;

const kAudioServicesUnsupportedPropertyError: OSStatus = fourcc(b"pty?") as _;
const kSystemSoundID_Vibrate: SystemSoundID = 0x00000FFF;

fn AudioServicesGetProperty(
    _env: &mut Environment,
    in_property_id: AudioServicesPropertyID,
    _in_specifier_size: u32,
    _in_specifier: crate::mem::ConstVoidPtr,
    _io_property_data_size: MutPtr<u32>,
    _out_property_data: MutVoidPtr,
) -> OSStatus {
    // Crash Bandicoot Nitro Kart 3D tries to use this property ID, which does
    // not seem to be documented anywhere? Assuming this is a bug.
    if in_property_id == 0xfff {
        kAudioServicesUnsupportedPropertyError
    } else {
        unimplemented!();
    }
}

fn AudioServicesPlaySystemSound(_env: &mut Environment, in_system_sound_id: SystemSoundID) {
    // assert_eq!(in_system_sound_id, kSystemSoundID_Vibrate);
    log!("TODO: vibration (AudioServicesPlaySystemSound)");
    // TODO: implement other system sounds
}

fn AudioServicesDisposeSystemSoundID(_env: &mut Environment, in_system_sound_id: SystemSoundID) {
    // assert_eq!(in_system_sound_id, kSystemSoundID_Vibrate);
    log_dbg!("TODO: vibration (AudioServicesDisposeSystemSoundID)");
    // TODO: implement other system sounds
}
fn AudioServicesCreateSystemSoundID(
    env: &mut Environment,
    in_file_url: CFURLRef,
    in_system_sound_id: SystemSoundID
) -> OSStatus {
    -1
}

fn AudioServicesAddSystemSoundCompletion(
    env: &mut Environment,
    in_system_sound_id: SystemSoundID,
    in_run_loop: CFRunLoopRef,
    in_run_loop_mode: CFStringRef,
    in_completion_routine: AudioServicesSystemSoundCompletionProc,
    in_client_data: MutPtr<u32>,
) -> OSStatus; {
    -1
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(AudioServicesGetProperty(_, _, _, _, _)),
    export_c_func!(AudioServicesPlaySystemSound(_)),
    export_c_func!(AudioServicesDisposeSystemSoundID(_)),
    export_c_func!(AudioServicesCreateSystemSoundID(_, _)),
    export_c_func!(AudioServicesAddSystemSoundCompletion(_, _, _, _, _,)),
];
