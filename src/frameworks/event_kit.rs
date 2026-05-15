/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Stub for `EventKit.framework/EventKit`.
//!
//! On iOS, EventKit provides access to the calendar and reminders database
//! (`EKEventStore`, `EKEvent`, `EKReminder`, etc.). Apps that link against
//! EventKit typically do so because the SDK or a third-party library pulled
//! it in transitively; the vast majority of touchHLE's target games never
//! actually use calendar APIs at runtime.
//!
//! Without a [crate::dyld::HostDylib] entry for the path
//! `/System/Library/Frameworks/EventKit.framework/EventKit`, touchHLE
//! prints a `Warning: app binary depends on unimplemented or missing
//! dylib …` at startup.
//!
//! This stub exists so the dependency is recognized and the warning is
//! suppressed. Real EventKit handling is not implemented; specific APIs
//! can be added here as they come up.

use crate::dyld::FunctionExports;

pub const FUNCTIONS: FunctionExports = &[];
