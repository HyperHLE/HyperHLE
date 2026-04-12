/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `NSProcessInfo`.

use super::NSTimeInterval;
use crate::frameworks::foundation::ns_string;
use crate::libc::mach::host::PHYSICAL_MEMORY;
use crate::objc::{id, msg, msg_class, objc_classes, ClassExports};
use crate::Environment;
use std::time::Instant;

#[derive(Default)]
pub struct State {
    /// `NSProcessInfo*`
    process_info: Option<id>,
}

fn assert_process_info_singleton(env: &mut Environment, this: id) {
    assert_eq!(
        this,
        env.framework_state
            .foundation
            .ns_process_info
            .process_info
            .unwrap()
    );
}

/// Fake OS version used when the app queries the host system version.
const OS_VERSION_MAJOR: u64 = 12;
const OS_VERSION_MINOR: u64 = 0;
const OS_VERSION_PATCH: u64 = 0;

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation NSProcessInfo: NSObject

// =========================================================================
// MARK: - Singleton
// =========================================================================

+ (id)processInfo {
    if let Some(existing) = env.framework_state.foundation.ns_process_info.process_info {
        existing
    } else {
        let process_info: id = msg![env; this new];
        env.framework_state.foundation.ns_process_info.process_info = Some(process_info);
        process_info
    }
}

// =========================================================================
// MARK: - Process identity
// =========================================================================

- (id)processName {
    assert_process_info_singleton(env, this);
    let main_bundle: id = msg_class![env; NSBundle mainBundle];
    let name_key: id = ns_string::get_static_str(env, "CFBundleName");
    msg![env; main_bundle objectForInfoDictionaryKey:name_key]
}

- (())setProcessName:(id)_name {
    assert_process_info_singleton(env, this);
    log!("TODO: [NSProcessInfo setProcessName:] — ignored");
}

- (i32)processIdentifier {
    assert_process_info_singleton(env, this);
    1234
}

- (id)globallyUniqueString {
    assert_process_info_singleton(env, this);
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let uptime_ns = Instant::now()
        .duration_since(env.startup_time)
        .as_nanos() as u64;
    let s = format!("1234-{}-{}", uptime_ns, n);
    let cstr = env.mem.alloc_and_write_cstr(s.as_bytes());
    msg_class![env; NSString stringWithUTF8String:cstr]
}

// =========================================================================
// MARK: - Host / environment
// =========================================================================

- (id)hostName {
    assert_process_info_singleton(env, this);
    ns_string::get_static_str(env, "touchHLE-host.local")
}

- (id)arguments {
    assert_process_info_singleton(env, this);
    msg_class![env; NSArray array]
}

- (id)environment {
    assert_process_info_singleton(env, this);
    msg_class![env; NSDictionary dictionary]
}

// =========================================================================
// MARK: - Hardware / memory
// =========================================================================

- (u64)physicalMemory {
    assert_process_info_singleton(env, this);
    PHYSICAL_MEMORY.into()
}

- (u32)processorCount {
    assert_process_info_singleton(env, this);
    1
}

- (u32)activeProcessorCount {
    assert_process_info_singleton(env, this);
    1
}

// =========================================================================
// MARK: - Uptime
// =========================================================================

- (NSTimeInterval)systemUptime {
    assert_process_info_singleton(env, this);
    Instant::now().duration_since(env.startup_time).as_secs_f64()
}

// =========================================================================
// MARK: - OS version
// =========================================================================

// Returns an NSOperatingSystemVersion struct {major, minor, patch} packed
// into three consecutive NSUInteger fields. We return it as three separate
// values via an opaque struct id — most callers use
// isOperatingSystemAtLeastVersion: instead.
- (id)operatingSystemVersion {
    assert_process_info_singleton(env, this);
    log!(
        "TODO: [NSProcessInfo operatingSystemVersion] — returning nil \
         (use isOperatingSystemAtLeastVersion: instead)"
    );
    crate::objc::nil
}

- (id)operatingSystemVersionString {
    assert_process_info_singleton(env, this);
    let s = format!(
        "Version {}.{}.{} (Build 16A366)",
        OS_VERSION_MAJOR, OS_VERSION_MINOR, OS_VERSION_PATCH
    );
    let cstr = env.mem.alloc_and_write_cstr(s.as_bytes());
    msg_class![env; NSString stringWithUTF8String:cstr]
}

// NSOperatingSystemVersion is {major: NSInteger, minor: NSInteger, patch: NSInteger}.
// We receive it as three stacked guest integers; Objective-C ABI passes structs
// by value on the stack after the implicit (self, _cmd) arguments, so we model
// it as three separate NSUInteger parameters here.
- (bool)isOperatingSystemAtLeastVersion:(u64)major
                                  minor:(u64)minor
                                  patch:(u64)patch {
    assert_process_info_singleton(env, this);
    let (maj, min, pat) = (OS_VERSION_MAJOR, OS_VERSION_MINOR, OS_VERSION_PATCH);
    if major != maj { return major < maj; }
    if minor != min { return minor < min; }
    patch <= pat
}

// =========================================================================
// MARK: - Thermal state (iOS 11+)
// =========================================================================

// NSProcessInfoThermalStateNominal = 0
- (i64)thermalState {
    assert_process_info_singleton(env, this);
    0
}

// =========================================================================
// MARK: - Low Power Mode (iOS 9+)
// =========================================================================

- (bool)isLowPowerModeEnabled {
    assert_process_info_singleton(env, this);
    false
}

// =========================================================================
// MARK: - Activity assertions (iOS 7+)
// =========================================================================

- (id)beginActivityWithOptions:(u64)_options reason:(id)_reason {
    assert_process_info_singleton(env, this);
    log!("TODO: [NSProcessInfo beginActivityWithOptions:reason:] — returning stub token");
    this
}

- (())endActivity:(id)_activity {
    assert_process_info_singleton(env, this);
}

- (())performActivityWithOptions:(u64)_options
                          reason:(id)_reason
                      usingBlock:(id)block {
    assert_process_info_singleton(env, this);
    let _: () = msg![env; block invoke];
}

// =========================================================================
// MARK: - Sudden termination (macOS; silently ignored on iOS)
// =========================================================================

- (())disableSuddenTermination {
    assert_process_info_singleton(env, this);
}

- (())enableSuddenTermination {
    assert_process_info_singleton(env, this);
}

- (())disableAutomaticTermination:(id)_reason {
    assert_process_info_singleton(env, this);
}

- (())enableAutomaticTermination:(id)_reason {
    assert_process_info_singleton(env, this);
}

- (bool)automaticTerminationSupportEnabled {
    assert_process_info_singleton(env, this);
    false
}

- (())setAutomaticTerminationSupportEnabled:(bool)_enabled {
    assert_process_info_singleton(env, this);
}

@end

};
