/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Stub for `CoreData.framework/CoreData`.
//!
//! On iOS, CoreData provides an object-graph persistence framework
//! (`NSManagedObjectContext`, `NSPersistentStoreCoordinator`, etc.). Apps
//! that link against CoreData typically do so transitively through other
//! frameworks; the vast majority of touchHLE's target games never actually
//! use CoreData at runtime.
//!
//! Without a [crate::dyld::HostDylib] entry for the path
//! `/System/Library/Frameworks/CoreData.framework/CoreData`, touchHLE
//! prints a `Warning: app binary depends on unimplemented or missing
//! dylib …` at startup.
//!
//! This stub exists so the dependency is recognized and the warning is
//! suppressed. Real CoreData handling is not implemented; specific APIs
//! can be added here as they come up.

use crate::dyld::FunctionExports;

pub const FUNCTIONS: FunctionExports = &[];
