/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Our implementations of various frameworks.
//!
//! Each child module should be named after the framework it implements.
//! It can potentially have multiple child modules itself if it's a
//! particularly complex framework.
//!
//! See also `dyld/function_lists.rs` and `objc/classes/class_lists.rs`.
//!
//! Most modules in here are not going to link to documentation that should
//! be trivial to find by searching for the class or function name. For
//! example, the documentation of `NSArray` won't link to the main
//! developer.apple.com page documenting that class, but if there's
//! something interesting in the Documentation Archive relating to arrays,
//! that might be linked.

#![allow(non_upper_case_globals)] // Lots of Apple constants begin with "k"
#![allow(clippy::enum_variant_names)] // Lots of Apple enums have the same prefix
#![allow(clippy::too_many_arguments)] // It's not our fault!

pub mod accelerate;
pub mod address_book;
pub mod address_book_ui;
pub mod accounts;
pub mod audio_toolbox;
pub mod avfoundation;
pub mod assets_library;
pub mod captive_network;
pub mod carbon_core;
pub mod cf_http_message;
pub mod cf_network;
pub mod common_crypto;
pub mod core_animation;
pub mod core_audio;
pub mod core_audio_types;
pub mod core_bluetooth;
pub mod core_foundation;
pub mod core_graphics;
pub mod core_image;
pub mod core_location;
pub mod core_media;
pub mod core_motion;
pub mod core_telephony;
pub mod core_text;
pub mod core_video;
pub mod foundation;
pub mod game_controller;
pub mod game_kit;
pub mod gl_kit;
pub mod image_io;
pub mod libbz2;
pub mod libicucore;
pub mod libsqlite3;
pub mod libxml2;
pub mod map_kit;
pub mod media_player;
pub mod metal;
pub mod message_ui;
pub mod mobile_core_services;
pub mod openal;
pub mod opengles;
pub mod security;
pub mod social;
pub mod store_kit;
pub mod system_configuration;
pub mod tw_tweet_compose_view_controller;
pub mod uikit;

/// Container for state of various child modules
#[derive(Default)]
pub struct State {
    audio_toolbox: audio_toolbox::State,
    avfoundation: avfoundation::State,
    core_animation: core_animation::State,
    core_foundation: core_foundation::State,
    foundation: foundation::State,
    pub game_kit: game_kit::State,
    pub store_kit: store_kit::State,
    media_player: media_player::State,
    openal: openal::State,
    opengles: opengles::State,
    uikit: uikit::State,
}

/// Container for thread local state of various child modules
#[derive(Default)]
pub struct ThreadLocalState {
    foundation: foundation::ThreadLocalState,
}
