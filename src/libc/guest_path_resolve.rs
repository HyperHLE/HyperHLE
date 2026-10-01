/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Shared guest-path resolution for open()/access().
//! Prefers app-bundle relative paths; only probes Unity `Data/` when that
//! directory exists in the bundle (avoids misleading lookups for Walaber etc.).

use crate::fs::GuestPath;
use crate::Environment;

/// Resolve a relative guest path for read-only open/access.
/// Returns the path that should be used (may equal `path_string`).
pub fn resolve_guest_path(env: &Environment, path_string: &str) -> String {
    if path_string.starts_with('/') {
        return path_string.to_string();
    }
    if env.fs.exists(GuestPath::new(path_string)) {
        return path_string.to_string();
    }
    let bundle_root = env.bundle.bundle_path().as_str().trim_end_matches('/');
    let relative = path_string.trim_start_matches("./");
    let bundle_rel = format!("{bundle_root}/{relative}");
    if env.fs.exists(GuestPath::new(&bundle_rel)) {
        return bundle_rel;
    }
    // Unity Data/ only when present.
    let data_dir = format!("{bundle_root}/Data");
    if env.fs.exists(GuestPath::new(&data_dir)) {
        let data_relative = relative.strip_prefix("Data/").unwrap_or(relative);
        let data_relative = data_relative.strip_prefix("Data/").unwrap_or(data_relative);
        let candidate = format!("{bundle_root}/Data/{data_relative}");
        if env.fs.exists(GuestPath::new(&candidate)) {
            return candidate;
        }
    }
    path_string.to_string()
}
