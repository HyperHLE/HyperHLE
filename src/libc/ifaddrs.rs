/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `ifaddrs.h` and `net/if.h` (interface addresses and interface naming)

use crate::dyld::FunctionExports;
use crate::export_c_func;
use crate::libc::errno::{set_errno, ENXIO};
use crate::mem::{ConstPtr, MutPtr, SafeRead};
use crate::Environment;
use crate::log;

#[allow(non_camel_case_types)]
#[repr(C, packed)]
pub struct ifaddrs {
    pub ifa_next: MutPtr<ifaddrs>,
    pub ifa_name: ConstPtr<u8>,
    pub ifa_flags: u32,
    pub ifa_addr: MutPtr<u8>, // Changed from u32 to MutPtr
    pub ifa_netmask: MutPtr<u8>,
    pub ifa_broadaddr: MutPtr<u8>,
    pub ifa_data: u32,
}
unsafe impl SafeRead for ifaddrs {}

/// `int getifaddrs(struct ifaddrs **ifap)`
fn getifaddrs(env: &mut Environment, ifap: MutPtr<MutPtr<ifaddrs>>) -> i32 {
    if ifap.is_null() { return -1; }

    let mem = env.mem.as_mut();

    // 1. Allocate name "lo0"
    let name_str = "lo0\0";
    let name_ptr: MutPtr<u8> = mem.alloc(name_str.len() as u32).cast();
    for (i, &byte) in name_str.as_bytes().iter().enumerate() {
        mem.write(MutPtr::from_bits(name_ptr.to_bits() + i as u32), byte);
    }

    // 2. Allocate a dummy sockaddr (minimal size 16 bytes for sockaddr_in)
    // This prevents the NULL dereference when the app checks ifa_addr
    let sa_ptr: MutPtr<u8> = mem.alloc(16).cast();
    // Fill with zeros, but set sa_family to AF_INET (2) 
    // and sa_len to 16 (iOS/Darwin specific)
    mem.write(sa_ptr, 16u8); // sa_len
    mem.write(MutPtr::from_bits(sa_ptr.to_bits() + 1), 2u8); // sa_family (AF_INET)

    // 3. Allocate the ifaddrs struct
    let ifa_size = std::mem::size_of::<ifaddrs>() as u32;
    let ifa_ptr: MutPtr<ifaddrs> = mem.alloc(ifa_size).cast();
    
    let dummy_ifa = ifaddrs {
        ifa_next: MutPtr::null(),
        ifa_name: name_ptr.cast_const(),
        ifa_flags: 0x1 | 0x8, // IFF_UP | IFF_LOOPBACK
        ifa_addr: sa_ptr,      // No longer 0!
        ifa_netmask: MutPtr::null(),
        ifa_broadaddr: MutPtr::null(),
        ifa_data: 0,
    };

    mem.write(ifa_ptr, dummy_ifa);
    mem.write(ifap, ifa_ptr);

    log!("getifaddrs(): Reported fake lo0 interface with valid dummy sockaddr.");
    0 
}

// ... keep the rest of the file (freeifaddrs, if_nametoindex, etc) the same
