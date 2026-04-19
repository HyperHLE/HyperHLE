/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

//! The AVFoundation framework.

mod av_audio_player;
pub mod av_audio_session;
pub mod av_asset_export_session;

#[derive(Default)]
pub struct State {
    pub av_audio_session: av_audio_session::State,
}


pub const DYLIB: crate::dyld::HostDylib = crate::dyld::HostDylib {
    path: "/System/Library/Frameworks/AVFoundation.framework/AVFoundation",
    aliases: &[],
    class_exports: &[av_audio_player::CLASSES, av_asset_export_session::CLASSES, av_audio_session::CLASSES],
    constant_exports: &[av_audio_session::CONSTANTS],
    function_exports: &[],
};
