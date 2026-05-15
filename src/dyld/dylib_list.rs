/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Separate module just for the dylib list, so it gets its own git history.

use crate::frameworks;
use crate::frameworks::libsqlite3;
use crate::libc;
use crate::objc;

// CoreAudio
pub const CORE_AUDIO: super::HostDylib = super::HostDylib {
    path: "/System/Library/Frameworks/CoreAudio.framework/CoreAudio",
    aliases: &[],
    class_exports: &[],
    constant_exports: &[],
    function_exports: &[frameworks::core_audio::FUNCTIONS],
};

// CFNetwork
pub const CF_NETWORK: super::HostDylib = super::HostDylib {
    path: "/System/Library/Frameworks/CFNetwork.framework/CFNetwork",
    aliases: &[],
    class_exports: &[],
    constant_exports: &[],
    function_exports: &[frameworks::cf_network::FUNCTIONS],
};

// MobileCoreServices (stub — no UTType implementation yet)
pub const MOBILE_CORE_SERVICES: super::HostDylib = super::HostDylib {
    path: "/System/Library/Frameworks/MobileCoreServices.framework/MobileCoreServices",
    aliases: &[],
    class_exports: &[],
    constant_exports: &[],
    function_exports: &[frameworks::mobile_core_services::FUNCTIONS],
};

// CoreMedia (stub — function exports are currently registered with CoreVideo)
pub const CORE_MEDIA: super::HostDylib = super::HostDylib {
    path: "/System/Library/Frameworks/CoreMedia.framework/CoreMedia",
    aliases: &[],
    class_exports: &[],
    constant_exports: &[],
    function_exports: &[frameworks::core_media::FUNCTIONS],
};

// MapKit (stub — no real map rendering yet, just satisfies the dependency)
pub const MAP_KIT: super::HostDylib = super::HostDylib {
    path: "/System/Library/Frameworks/MapKit.framework/MapKit",
    aliases: &[],
    class_exports: &[],
    constant_exports: &[],
    function_exports: &[frameworks::map_kit::FUNCTIONS],
};

// MessageUI (stub — no real compose-sheet implementation yet)
pub const MESSAGE_UI: super::HostDylib = super::HostDylib {
    path: "/System/Library/Frameworks/MessageUI.framework/MessageUI",
    aliases: &[],
    class_exports: &[],
    constant_exports: &[],
    function_exports: &[frameworks::message_ui::FUNCTIONS],
};

// EventKit (stub — no real calendar/reminders implementation yet)
pub const EVENT_KIT: super::HostDylib = super::HostDylib {
    path: "/System/Library/Frameworks/EventKit.framework/EventKit",
    aliases: &[],
    class_exports: &[],
    constant_exports: &[],
    function_exports: &[frameworks::event_kit::FUNCTIONS],
};

// CoreData (stub — no real object-graph persistence implementation yet)
pub const CORE_DATA: super::HostDylib = super::HostDylib {
    path: "/System/Library/Frameworks/CoreData.framework/CoreData",
    aliases: &[],
    class_exports: &[],
    constant_exports: &[],
    function_exports: &[frameworks::core_data::FUNCTIONS],
};

// AddressBookUI (stub — no real contacts-picker implementation yet)
pub const ADDRESS_BOOK_UI: super::HostDylib = super::HostDylib {
    path: "/System/Library/Frameworks/AddressBookUI.framework/AddressBookUI",
    aliases: &[],
    class_exports: &[],
    constant_exports: &[],
    function_exports: &[frameworks::address_book_ui::FUNCTIONS],
};

/// The single list of host dylibs that the linker (and Objective-C runtime)
/// searches through.
pub const DYLIB_LIST: &[&super::HostDylib] = &[
    &libc::DYLIB,
    &objc::DYLIB,
    &crate::environment::app_picker::DYLIB, // Not a real library; special internal classes.
    &frameworks::audio_toolbox::DYLIB,
    &frameworks::avfoundation::DYLIB,
    &frameworks::core_animation::DYLIB,
    &frameworks::core_foundation::DYLIB,
    &frameworks::core_graphics::DYLIB,
    &frameworks::core_location::DYLIB,
    &frameworks::core_motion::DYLIB,
    &frameworks::foundation::DYLIB,
    &frameworks::game_kit::DYLIB,
    &frameworks::media_player::DYLIB,
    &frameworks::openal::DYLIB,
    &frameworks::opengles::DYLIB,
    &frameworks::security::DYLIB,
    &frameworks::store_kit::DYLIB,
    &frameworks::system_configuration::DYLIB,
    &frameworks::uikit::DYLIB,
    &frameworks::libicucore::DYLIB,
    &frameworks::libsqlite3::DYLIB,
    &frameworks::libxml2::DYLIB,
    &frameworks::libbz2::DYLIB,
    &frameworks::common_crypto::DYLIB,
    &frameworks::core_video::DYLIB,
    &frameworks::address_book::DYLIB,
    &frameworks::game_controller::DYLIB,
    &CORE_AUDIO,
    &CF_NETWORK,
    &MOBILE_CORE_SERVICES,
    &CORE_MEDIA,
    &MAP_KIT,
    &MESSAGE_UI,
    &ADDRESS_BOOK_UI,
    &EVENT_KIT,
    &CORE_DATA,
];

#[cfg(test)]
mod tests {
    use crate::objc::ClassTemplate;

    use super::*;
    use std::collections::HashSet;

    #[test]
    fn no_duplicate_classes() {
        let mut seen_classes = HashSet::new();

        for (class_name, template) in DYLIB_LIST
            .iter()
            .flat_map(|dylib| dylib.class_exports)
            .copied()
            .flatten()
        {
            if !seen_classes.insert(class_name) {
                panic!("Found duplicate class export {class_name}");
            }

            let ClassTemplate {
                class_methods,
                instance_methods,
                ..
            } = template;

            let mut seen_class_methods = HashSet::with_capacity(class_methods.len());

            for (method_name, _) in *class_methods {
                if !seen_class_methods.insert(method_name) {
                    panic!(
                        "Found duplicate class method {method_name} \
                        for class {class_name}"
                    )
                }
            }

            let mut seen_instance_methods = HashSet::with_capacity(instance_methods.len());

            for (method_name, _) in *instance_methods {
                if !seen_instance_methods.insert(method_name) {
                    panic!(
                        "Found duplicate instance method {method_name} \
                        for class {class_name}"
                    )
                }
            }
        }
    }

    #[test]
    fn no_duplicate_functions() {
        let mut seen = HashSet::new();

        for (function_name, _) in DYLIB_LIST
            .iter()
            .flat_map(|dylib| dylib.function_exports)
            .copied()
            .flatten()
        {
            if !seen.insert(function_name) {
                panic!("Found duplicate function export {function_name}");
            }
        }
    }

    #[test]
    fn no_duplicate_constants() {
        let mut seen = HashSet::new();

        for (constant_name, _) in DYLIB_LIST
            .iter()
            .flat_map(|dylib| dylib.constant_exports)
            .copied()
            .flatten()
        {
            if !seen.insert(constant_name) {
                panic!("Found duplicate constant export {constant_name}");
            }
        }
    }
}
