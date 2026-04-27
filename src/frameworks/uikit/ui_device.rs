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
use crate::objc::{id, msg, msg_class, objc_classes, ClassExports, TrivialHostObject};
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
    battery_monitoring_enabled: bool,
    proximity_monitoring_enabled: bool,
    generates_device_orientation_notifications: bool,
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
    env.framework_state.uikit.ui_device.generates_device_orientation_notifications = true;
}

- (())endGeneratingDeviceOrientationNotifications {
    env.framework_state.uikit.ui_device.generates_device_orientation_notifications = false;
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
    env.framework_state.uikit.ui_device.generates_device_orientation_notifications
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
    // Upgraded to 3.1.3 because the game requires at least 3.0
    ns_string::get_static_str(env, "3.1.3")
}
- (id)uniqueIdentifier {
    // A standard 40-character UDID format helps prevent crashes in engine logic
    ns_string::get_static_str(env, "0000000000000000000000000000000000000000")
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
    env.framework_state.uikit.ui_device.proximity_monitoring_enabled
}

- (())setProximityMonitoringEnabled:(bool)enabled {
    env.framework_state.uikit.ui_device.proximity_monitoring_enabled = enabled;
}

- (bool)proximityState {
    // Proximity sensor never triggered.
    false
}

// MARK: - Battery

- (bool)isBatteryMonitoringEnabled {
    env.framework_state.uikit.ui_device.battery_monitoring_enabled
}

- (())setBatteryMonitoringEnabled:(bool)enabled {
    env.framework_state.uikit.ui_device.battery_monitoring_enabled = enabled;
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
