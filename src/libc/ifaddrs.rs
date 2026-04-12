/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `ifaddrs.h` and `net/if.h` (interface addresses and interface naming)

use crate::dyld::FunctionExports;
use crate::export_c_func;
use crate::libc::errno::{set_errno, EINVAL, ENXIO};
use crate::mem::{ConstPtr, MutPtr, SafeRead};
use crate::Environment;

// Mirrors the POSIX `struct ifaddrs` layout as seen by 32-bit ARM guests.
// All pointer fields are 4-byte guest pointers.
#[allow(non_camel_case_types)]
#[repr(C, packed)]
pub struct ifaddrs {
    /// Next node in the linked list (NULL = end).
    pub ifa_next: MutPtr<ifaddrs>,
    /// NUL-terminated interface name, e.g. "en0".
    pub ifa_name: ConstPtr<u8>,
    /// Interface flags (IFF_UP, IFF_LOOPBACK, …).
    pub ifa_flags: u32,
    /// Primary address (may be NULL).
    pub ifa_addr: u32, // guest ptr to sockaddr – typed as u32 to avoid pulling in socket types
    /// Netmask (may be NULL).
    pub ifa_netmask: u32,
    /// Broadcast or point-to-point destination address (may be NULL).
    pub ifa_broadaddr: u32,
    /// Protocol-specific data (may be NULL).
    pub ifa_data: u32,
}
// SAFETY: the struct is plain data; every field is either a scalar or a guest
// pointer that touchHLE's pointer type already validates.
unsafe impl SafeRead for ifaddrs {}

// ---------------------------------------------------------------------------
// getifaddrs / freeifaddrs
// ---------------------------------------------------------------------------

/// `int getifaddrs(struct ifaddrs **ifap)`
///
/// Stub: we do not enumerate real host interfaces. Returns -1 / EOPNOTSUPP so
/// that callers fall back gracefully instead of crashing on a null list.
fn getifaddrs(env: &mut Environment, ifap: MutPtr<MutPtr<ifaddrs>>) -> i32 {
    // Write NULL into *ifap so callers that ignore the return value and
    // dereference ifap anyway get a well-defined NULL rather than garbage.
    if !ifap.is_null() {
        env.mem.write(ifap, MutPtr::null());
    }

    // EOPNOTSUPP = 102 on Darwin; reuse the closest available errno constant.
    // Using set_errno with literal 102 keeps us independent of whether
    // touchHLE has EOPNOTSUPP defined yet.
    set_errno(env, 102 /* EOPNOTSUPP */);
    log!("TODO: getifaddrs() – returning error (not implemented)");
    -1
}

/// `void freeifaddrs(struct ifaddrs *ifa)`
///
/// Since our `getifaddrs` never allocates anything, this is a no-op. If a
/// future implementation does allocate, the deallocation logic belongs here.
fn freeifaddrs(env: &mut Environment, ifa: MutPtr<ifaddrs>) {
    if !ifa.is_null() {
        // Future: walk the linked list and free each node + name string.
        log!(
            "TODO: freeifaddrs({:#x}) – list was not allocated by us, ignoring",
            ifa.to_bits()
        );
    }
}

// ---------------------------------------------------------------------------
// net/if.h – interface index / name mapping
// (commonly used together with ifaddrs by network-aware apps)
// ---------------------------------------------------------------------------

/// Maximum length of an interface name including the NUL terminator.
const IF_NAMESIZE: usize = 16;

/// `unsigned int if_nametoindex(const char *ifname)`
///
/// Returns the index for the named interface, or 0 on error.
/// Stub: we have no real interfaces, so always returns 0 / ENXIO.
fn if_nametoindex(env: &mut Environment, ifname: ConstPtr<u8>) -> u32 {
    let name = env.mem.cstr_at_utf8(ifname).unwrap_or("<invalid>");
    log!(
        "TODO: if_nametoindex(\"{}\") – returning 0 (not implemented)",
        name
    );
    set_errno(env, ENXIO);
    0
}

/// `char *if_indextoname(unsigned int ifindex, char *ifname)`
///
/// Writes the name of interface `ifindex` into `ifname` (at least
/// `IF_NAMESIZE` bytes) and returns `ifname`, or NULL on error.
/// Stub: always returns NULL / ENXIO.
fn if_indextoname(env: &mut Environment, ifindex: u32, ifname: MutPtr<u8>) -> MutPtr<u8> {
    log!(
        "TODO: if_indextoname({}) – returning NULL (not implemented)",
        ifindex
    );
    set_errno(env, ENXIO);
    MutPtr::null()
}

// `struct if_nameindex` used by if_nameindex() / if_freenameindex().
#[allow(non_camel_case_types)]
#[repr(C, packed)]
pub struct if_nameindex {
    pub if_index: u32,
    pub if_name: ConstPtr<u8>,
}
unsafe impl SafeRead for if_nameindex {}

/// `struct if_nameindex *if_nameindex(void)`
///
/// Returns an array of all interface name/index pairs terminated by an entry
/// with `if_index == 0` and `if_name == NULL`. Stub: returns NULL / EOPNOTSUPP.
fn if_nameindex(env: &mut Environment) -> MutPtr<if_nameindex> {
    log!("TODO: if_nameindex() – returning NULL (not implemented)");
    set_errno(env, 102 /* EOPNOTSUPP */);
    MutPtr::null()
}

/// `void if_freenameindex(struct if_nameindex *ptr)`
///
/// Frees the array returned by `if_nameindex`. No-op in the stub.
fn if_freenameindex(env: &mut Environment, ptr: MutPtr<if_nameindex>) {
    if !ptr.is_null() {
        log!(
            "TODO: if_freenameindex({:#x}) – not allocated by us, ignoring",
            ptr.to_bits()
        );
    }
}

// ---------------------------------------------------------------------------
// Export table
// ---------------------------------------------------------------------------

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(getifaddrs(_)),
    export_c_func!(freeifaddrs(_)),
    export_c_func!(if_nametoindex(_)),
    export_c_func!(if_indextoname(_, _)),
    export_c_func!(if_nameindex()),
    export_c_func!(if_freenameindex(_)),
];
