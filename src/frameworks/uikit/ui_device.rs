/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 * If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
#![allow(dead_code)]
//! `UIDevice`.

use crate::dyld::{ConstantExports, HostConstant};
use crate::frameworks::foundation::{ns_string, NSInteger};
use crate::objc::{id, msg, msg_class, objc_classes, todo_objc_setter, ClassExports, TrivialHostObject};
use crate::window::{get_battery_status, BatteryState, DeviceOrientation};

pub const UIDeviceOrientationDidChangeNotification: &str =
    "UIDeviceOrientationDidChangeNotification";
pub const UIDeviceBatteryLevelDidChangeNotification: &str =
    "UIDeviceBatteryLevelDidChangeNotification";

pub const UIDeviceBatteryStateDidChangeNotification: &str =
    "UIDeviceBatteryStateDidChangeNotification";
pub const UIDeviceProximityStateDidChangeNotification: &str =
    "UIDeviceProximityStateDidChangeNotification";

pub type UIDeviceOrientation = NSInteger;

pub const UIDeviceOrientationUnknown:           UIDeviceOrientation = 0;
pub const UIDeviceOrientationPortrait:          UIDeviceOrientation = 1;

pub const UIDeviceOrientationPortraitUpsideDown: UIDeviceOrientation = 2;
pub const UIDeviceOrientationLandscapeLeft:     UIDeviceOrientation = 3;
pub const UIDeviceOrientationLandscapeRight:    UIDeviceOrientation = 4;

pub const UIDeviceOrientationFaceUp:            UIDeviceOrientation = 5;

pub const UIDeviceOrientationFaceDown:          UIDeviceOrientation = 6;

pub type UIDeviceBatteryState = NSInteger;
pub const UIDeviceBatteryStateUnknown:   UIDeviceBatteryState = 0;
pub const UIDeviceBatteryStateUnplugged: UIDeviceBatteryState = 1;
pub const UIDeviceBatteryStateCharging:  UIDeviceBatteryState = 2;
pub const UIDeviceBatteryStateFull:      UIDeviceBatteryState = 3;

pub type UIUserInterfaceIdiom = NSInteger;
pub const UIUserInterfaceIdiomPhone: UIUserInterfaceIdiom = 0;
pub const UIUserInterfaceIdiomPad:   UIUserInterfaceIdiom = 1;

#[derive(Default)]
pub struct State {
    current_device: Option<id>,
}

pub const CONSTANTS: ConstantExports = &[
    (
        "_UIDeviceOrientationDidChangeNotification",
        HostConstant::NSString(UIDeviceOrientationDidChangeNotification),
    ),
    (
        "_UIDeviceBatteryLevelDidChangeNotification",
        HostConstant::NSString(UIDeviceBatteryLevelDidChangeNotification),
    ),
    (
        "_UIDeviceBatteryStateDidChangeNotification",
        HostConstant::NSString(UIDeviceBatteryStateDidChangeNotification),
    ),
    (
        "_UIDeviceProximityStateDidChangeNotification",
        HostConstant::NSString(UIDeviceProximityStateDidChangeNotification),
    ),
];

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UIDevice: NSObject

+ (id)currentDevice {
    if let Some(device) = env.framework_state.uikit.ui_device.current_device {
        device
    } else {
        let new = env.objc.alloc_static_object(
            this,
            Box::new(TrivialHostObject),
            &mut env.mem,
        );
        env.framework_state.uikit.ui_device.current_device = Some(new);
        new
    }
}

// MARK: - Orientation

- (())beginGeneratingDeviceOrientationNotifications {
    log!("TODO: beginGeneratingDeviceOrientationNotifications");
}

- (())endGeneratingDeviceOrientationNotifications {
    log!("TODO: endGeneratingDeviceOrientationNotifications");
}

- (UIDeviceOrientation)orientation {
    match env.window().current_rotation() {
        DeviceOrientation::Portrait      => UIDeviceOrientationPortrait,
        DeviceOrientation::LandscapeLeft  => UIDeviceOrientationLandscapeLeft,
        DeviceOrientation::LandscapeRight => UIDeviceOrientationLandscapeRight,
    }
}

- (())setOrientation:(UIDeviceOrientation)orientation {
    env.window_mut().rotate_device(match orientation {
        UIDeviceOrientationPortrait      => DeviceOrientation::Portrait,
        UIDeviceOrientationLandscapeLeft  => DeviceOrientation::LandscapeLeft,
        UIDeviceOrientationLandscapeRight => DeviceOrientation::LandscapeRight,
        _ => {
            log!("Warning: UIDevice setOrientation:{} not handled, ignoring", orientation);
            return;
        }
    });
}

- (bool)isGeneratingDeviceOrientationNotifications {
    false
}

// MARK: - Identity

- (id)model {
    ns_string::get_static_str(env, "iPhone")
}
- (id)localizedModel {
    msg![env; this model]
}
- (id)name {
    ns_string::get_static_str(env, "iPhone")
}
- (id)systemName {
    ns_string::get_static_str(env, "iPhone OS")
}
- (id)systemVersion {
    ns_string::get_static_str(env, "2.0")
}
- (id)uniqueIdentifier {
    ns_string::get_static_str(env, "touchHLEdevice..........................")
}

// MARK: - Idiom

- (UIUserInterfaceIdiom)userInterfaceIdiom {
    UIUserInterfaceIdiomPhone
}

// MARK: - Capabilities

- (bool)isMultitaskingSupported {
    false
}

- (bool)isProximityMonitoringEnabled {
    false
}
- (())setProximityMonitoringEnabled:(bool)_enabled {
    log!("TODO: UIDevice setProximityMonitoringEnabled: (stubbed)");
}

- (bool)proximityState {
    // Proximity sensor never triggered.
    false
}

// MARK: - Battery

- (bool)isBatteryMonitoringEnabled {
    true
}
- (())setBatteryMonitoringEnabled:(bool)enabled {
    todo_objc_setter!(this, enabled);
    assert!(enabled);
}

- (f32)batteryLevel {
    let pct = get_battery_status().0;
    if pct < 0 {
        log_dbg!("batteryLevel: could not determine percentage, returning 1.0");
        return 1.0;
    }
    pct as f32 / 100.0
}

- (UIDeviceBatteryState)batteryState {
    match get_battery_status().1 {
        BatteryState::Unknown   => UIDeviceBatteryStateUnknown,
        BatteryState::OnBattery => UIDeviceBatteryStateUnplugged,
        BatteryState::NoBattery |
        BatteryState::Charging  => UIDeviceBatteryStateCharging,
        BatteryState::Full      => UIDeviceBatteryStateFull,
    }
}

// MARK: - Hardware info (read-only stubs matching iPhone 2G/3G era)

- (id)platform {
    // Matches the sysctl hw.machine value on a first-gen iPhone.
    ns_string::get_static_str(env, "iPhone1,1")
}

- (id)hwModel {
    ns_string::get_static_str(env, "iPhone1,1")
}

// MARK: - Notifications (post helpers used by subcomponents)

- (())_postOrientationChangeNotification {
    let name = ns_string::get_static_str(
        env,
        UIDeviceOrientationDidChangeNotification,
    );
    let nc: id = msg_class![env; NSNotificationCenter defaultCenter];
    msg![env; nc postNotificationName:name object:this]
}

@end

};

