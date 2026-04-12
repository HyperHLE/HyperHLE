/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! The Core Motion framework.

use crate::dyld::HostDylib;
use crate::objc::{id, nil, objc_classes, ClassExports};

pub const DYLIB: HostDylib = HostDylib {
    path: "/System/Library/Frameworks/CoreMotion.framework/CoreMotion",
    aliases: &[],
    class_exports: &[CLASSES],
    constant_exports: &[],
    function_exports: &[],
};

const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation CMMotionManager: NSObject

- (bool)isGyroAvailable {
    // FakeGyroCheck
    log!("TODO: [(CMMotionManager *){:?} isGyroAvailable] -> true", this);
    true
}
- (bool)isDeviceMotionAvailable {
    // FakeDeviceMotion
    log!("TODO: [(CMMotionManager *){:?} isDeviceMotionAvailable] -> true", this);
    true
}
- (bool)isAccelerometerAvailable {
    // FakeAccelerometerCheck
    log!("TODO: [(CMMotionManager *){:?} isAccelerometerAvailable] -> true", this);
    true
}

- (())setAccelerometerUpdateInterval:(f64)interval {
    // FakeAccelInterval
    log!("TODO: [(CMMotionManager *){:?} setAccelerometerUpdateInterval:{}]", this, interval);
}

- (())startAccelerometerUpdates {
    // FakeAccelStart
    log!("TODO: [(CMMotionManager *){:?} startAccelerometerUpdates]", this);
}

- (())setGyroUpdateInterval:(f64)interval {
    // FakeGyroInterval
    log!("TODO: [(CMMotionManager *){:?} setGyroUpdateInterval:{}]", this, interval);
}

- (())startGyroUpdates {
    // FakeGyroStart
    log!("TODO: [(CMMotionManager *){:?} startGyroUpdates]", this);
}

- (())setDeviceMotionUpdateInterval:(f64)interval {
    // FakeMotionInterval
    log!("TODO: [(CMMotionManager *){:?} setDeviceMotionUpdateInterval:{}]", this, interval);
}

- (())startDeviceMotionUpdates {
    // FakeMotionStart
    log!("TODO: [(CMMotionManager *){:?} startDeviceMotionUpdates]", this);
}

- (bool)isDeviceMotionActive {
    // FakeMotionActive
    log!("TODO: [(CMMotionManager *){:?} isDeviceMotionActive] -> true", this);
    true
}

- (bool)isAccelerometerActive {
    // FakeAccelActive
    log!("TODO: [(CMMotionManager *){:?} isAccelerometerActive] -> true", this);
    true
}

- (bool)isGyroActive {
    // FakeGyroActive
    log!("TODO: [(CMMotionManager *){:?} isGyroActive] -> true", this);
    true
}

- (id)deviceMotion {
    // FakeDeviceMotion
    log!("TODO: [(CMMotionManager *){:?} deviceMotion] -> nil", this);
    nil
}

- (id)accelerometerData {
    // FakeAccelData
    log!("TODO: [(CMMotionManager *){:?} accelerometerData] -> nil", this);
    nil
}

- (id)gyroData {
    // FakeGyroData
    log!("TODO: [(CMMotionManager *){:?} gyroData] -> nil", this);
    nil
}

@end

};
