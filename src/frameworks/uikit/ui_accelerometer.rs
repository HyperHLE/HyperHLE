/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UIAccelerometer`.

use crate::frameworks::foundation::NSTimeInterval;
use crate::objc::{
    autorelease, id, msg, msg_class, nil, objc_classes, release, ClassExports, HostObject,
    NSZonePtr, TrivialHostObject, SEL,
};
use crate::Environment;
use std::time::{Duration, Instant};

// =========================================================================
// MARK: - State
// =========================================================================

#[derive(Default)]
pub struct State {
    /// `[UIAccelerometer sharedAccelerometer]` — singleton.
    shared_accelerometer: Option<id>,
    /// Something implementing `UIAccelerometerDelegate` — weak reference.
    delegate: Option<id>,
    update_interval: Option<NSTimeInterval>,
    due_by: Option<Instant>,
    /// Whether the accelerometer is currently "active" (delegate is set).
    active: bool,
    /// Whether the app has called setDelegate: at least once.
    ever_set_delegate: bool,
    /// Last delivered acceleration values (for querying without a delegate).
    last_x: f64,
    last_y: f64,
    last_z: f64,
    last_timestamp: NSTimeInterval,
}

// =========================================================================
// MARK: - Types / constants
// =========================================================================

type UIAccelerationValue = f64;

const DEFAULT_UPDATE_INTERVAL: f64 = 1.0 / 60.0;
const MIN_UPDATE_INTERVAL:     f64 = 1.0 / 100.0; // 100 Hz — hardware limit
const MAX_UPDATE_INTERVAL:     f64 = 1.0;          // 1 Hz minimum

// =========================================================================
// MARK: - UIAcceleration host object
// =========================================================================

struct UIAccelerationHostObject {
    x: UIAccelerationValue,
    y: UIAccelerationValue,
    z: UIAccelerationValue,
    timestamp: NSTimeInterval,
}
impl HostObject for UIAccelerationHostObject {}

// =========================================================================
// MARK: - ObjC classes
// =========================================================================

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

// =========================================================================
// UIAccelerometer — singleton
// =========================================================================

@implementation UIAccelerometer: NSObject

+ (id)sharedAccelerometer {
    if let Some(acc) = env.framework_state.uikit.ui_accelerometer.shared_accelerometer {
        return acc;
    }
    let new = env.objc.alloc_static_object(
        this,
        Box::new(TrivialHostObject),
        &mut env.mem,
    );
    env.framework_state.uikit.ui_accelerometer.shared_accelerometer = Some(new);
    new
}

// Singleton — memory management is a no-op.
- (id)retain      { this }
- (())release     {}
- (id)autorelease { this }
- (u32)retainCount { u32::MAX }

// MARK: Delegate

- (id)delegate {
    env.framework_state.uikit.ui_accelerometer.delegate.unwrap_or(nil)
}

- (())setDelegate:(id)delegate {
    if delegate == nil {
        env.framework_state.uikit.ui_accelerometer.delegate = None;
        env.framework_state.uikit.ui_accelerometer.active   = false;
        log_dbg!("UIAccelerometer: delegate cleared, stopping updates");
    } else {
        env.framework_state.uikit.ui_accelerometer.delegate        = Some(delegate);
        env.framework_state.uikit.ui_accelerometer.active          = true;
        env.framework_state.uikit.ui_accelerometer.ever_set_delegate = true;
        // Reset due_by so the first update is delayed by one interval,
        // matching Apple's behaviour (avoids crashing early-init delegates).
        env.framework_state.uikit.ui_accelerometer.due_by = None;
        env.window().print_accelerometer_notice(&env.options);
        log_dbg!("UIAccelerometer: delegate set, starting updates");
    }
}

// MARK: Update interval

- (NSTimeInterval)updateInterval {
    env.framework_state.uikit.ui_accelerometer
        .update_interval
        .unwrap_or(DEFAULT_UPDATE_INTERVAL)
}

- (())setUpdateInterval:(NSTimeInterval)interval {
    // Clamp to hardware limits. Some apps pass 0 which causes divide-by-zero.
    let clamped = interval.clamp(MIN_UPDATE_INTERVAL, MAX_UPDATE_INTERVAL);
    if (clamped - interval).abs() > 1e-6 {
        log_dbg!(
            "UIAccelerometer setUpdateInterval: {:.4}s clamped to {:.4}s",
            interval, clamped
        );
    }
    env.framework_state.uikit.ui_accelerometer.update_interval = Some(clamped);
    // Reset scheduling so the new interval takes effect immediately.
    env.framework_state.uikit.ui_accelerometer.due_by = None;
}

// MARK: Last known values (iOS 4 deprecated API polyfill)

// Some apps read raw acceleration values directly from the shared accelerometer
// object rather than waiting for the delegate callback.  We expose the last
// delivered values for this purpose.

- (UIAccelerationValue)x {
    env.framework_state.uikit.ui_accelerometer.last_x
}

- (UIAccelerationValue)y {
    env.framework_state.uikit.ui_accelerometer.last_y
}

- (UIAccelerationValue)z {
    env.framework_state.uikit.ui_accelerometer.last_z
}

- (NSTimeInterval)timestamp {
    env.framework_state.uikit.ui_accelerometer.last_timestamp
}

// MARK: isActive (non-Apple extension used by some games)

- (bool)isActive {
    env.framework_state.uikit.ui_accelerometer.active
}

// MARK: Description

- (id)description {
    let (x, y, z, interval, active) = {
        let s = &env.framework_state.uikit.ui_accelerometer;
        (
            s.last_x, s.last_y, s.last_z,
            s.update_interval.unwrap_or(DEFAULT_UPDATE_INTERVAL),
            s.active,
        )
    };
    let s = format!(
        "<UIAccelerometer: x={:.3} y={:.3} z={:.3}; interval={:.4}s; active={}>",
        x, y, z, interval, active
    );
    let cstr = env.mem.alloc_and_write_cstr(s.as_bytes());
    msg_class![env; NSString stringWithUTF8String:cstr]
}

@end

// =========================================================================
// UIAcceleration
// =========================================================================

@implementation UIAcceleration: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(UIAccelerationHostObject {
        x: 0.0, y: 0.0, z: 0.0, timestamp: 0.0,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (UIAccelerationValue)x {
    env.objc.borrow::<UIAccelerationHostObject>(this).x
}
- (UIAccelerationValue)y {
    env.objc.borrow::<UIAccelerationHostObject>(this).y
}
- (UIAccelerationValue)z {
    env.objc.borrow::<UIAccelerationHostObject>(this).z
}
- (NSTimeInterval)timestamp {
    env.objc.borrow::<UIAccelerationHostObject>(this).timestamp
}

- (id)description {
    let (x, y, z, ts) = {
        let h = env.objc.borrow::<UIAccelerationHostObject>(this);
        (h.x, h.y, h.z, h.timestamp)
    };
    let s = format!(
        "<UIAcceleration: x={:.3} y={:.3} z={:.3} timestamp={:.3}>",
        x, y, z, ts
    );
    let cstr = env.mem.alloc_and_write_cstr(s.as_bytes());
    msg_class![env; NSString stringWithUTF8String:cstr]
}

@end

};

// =========================================================================
// MARK: - handle_accelerometer (called by NSRunLoop)
// =========================================================================

/// Check if an accelerometer update is due and deliver it.
/// Returns the time the next update is due, if any.
pub(super) fn handle_accelerometer(env: &mut Environment) -> Option<Instant> {
    let state = &env.framework_state.uikit.ui_accelerometer;

    // No delegate — nothing to do.
    let delegate = state.delegate?;
    if !state.active { return None; }

    let ns_interval = state.update_interval.unwrap_or(DEFAULT_UPDATE_INTERVAL);
    let rust_interval = Duration::from_secs_f64(ns_interval);
    let now = Instant::now();

    // Schedule the first update one interval from now (avoids crashing
    // delegates that aren't fully initialised yet).
    let due_by = match env.framework_state.uikit.ui_accelerometer.due_by {
        Some(t) => t,
        None => {
            let first_due = now.checked_add(rust_interval).unwrap();
            env.framework_state.uikit.ui_accelerometer.due_by = Some(first_due);
            return Some(first_due);
        }
    };

    if due_by > now {
        return Some(due_by);
    }

    // Advance the deadline, catching up any missed intervals.
    let overdue_by = now.duration_since(due_by);
    let advance_intervals = (overdue_by.as_secs_f64() / ns_interval)
        .max(1.0)
        .ceil() as u32;
    if advance_intervals > 1 {
        log_dbg!(
            "UIAccelerometer: lagging by {:.3}s, skipping {} interval(s)",
            overdue_by.as_secs_f64(),
            advance_intervals - 1
        );
    }
    let advance = rust_interval.checked_mul(advance_intervals).unwrap();
    let next_due = due_by.checked_add(advance).unwrap();
    env.framework_state.uikit.ui_accelerometer.due_by = Some(next_due);

    // UIKit creates and drains an autorelease pool per event.
    let pool: id = msg_class![env; NSAutoreleasePool new];

    // Get current acceleration values from the window.
    let (x, y, z) = env.window().get_acceleration(&env.options);
    let timestamp: NSTimeInterval = {
        let pi: id = msg_class![env; NSProcessInfo processInfo];
        msg![env; pi systemUptime]
    };

    // Cache the values on the shared accelerometer for direct queries.
    {
        let s = &mut env.framework_state.uikit.ui_accelerometer;
        s.last_x         = x.into();
        s.last_y         = y.into();
        s.last_z         = z.into();
        s.last_timestamp = timestamp;
    }

    // Build the UIAcceleration object.
    let acceleration: id = msg_class![env; UIAcceleration alloc];
    *env.objc.borrow_mut(acceleration) = UIAccelerationHostObject {
        x: x.into(),
        y: y.into(),
        z: z.into(),
        timestamp,
    };
    autorelease(env, acceleration);

    let accelerometer: id = msg_class![env; UIAccelerometer sharedAccelerometer];

    log_dbg!(
        "UIAccelerometer: delivering x={:.3} y={:.3} z={:.3} to delegate {:?}",
        x, y, z, delegate
    );

    // Guard with respondsToSelector: so delegates that don't implement
    // the optional method don't crash.
    let sel: SEL = env.objc.lookup_selector("accelerometer:didAccelerate:")
        .unwrap_or_else(|| {
            env.objc.register_host_selector(
                "accelerometer:didAccelerate:".to_string(),
                &mut env.mem,
            )
        });
    let responds: bool = msg![env; delegate respondsToSelector:sel];
    if responds {
        let _: () = msg![env; delegate accelerometer:accelerometer
                                       didAccelerate:acceleration];
    } else {
        log_dbg!("UIAccelerometer: delegate {:?} does not respond to accelerometer:didAccelerate:", delegate);
    }

    release(env, pool);

    env.framework_state.uikit.ui_accelerometer.due_by
}
