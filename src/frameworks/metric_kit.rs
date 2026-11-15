/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `MetricKit.framework` — the iOS 13 metric/telemetry framework.
//!
//! Apps built with a newer SDK list it in their load commands and touch it
//! from code that is guarded by an availability check. The framework is not
//! part of the iPhone OS versions this emulator models, so the app's
//! `NSClassFromString(@"MXMetricManager")` probe answering nil is the
//! behaviour of a real device from that era — but the *dylib* still has to
//! exist, otherwise the loader reports the dependency as missing at startup.
//! An empty host dylib satisfies the dependency without exporting anything.

use crate::dyld::HostDylib;

pub const DYLIB: HostDylib = HostDylib {
    path: "/System/Library/Frameworks/MetricKit.framework/MetricKit",
    aliases: &[],
    class_exports: &[],
    constant_exports: &[],
    function_exports: &[],
};
