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
    pub ifa_addr: MutPtr<u8>, 
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

    // 2. Allocate a dummy sockaddr (16 bytes for sockaddr_in)
    // This prevents the NULL-PAGE READ at 0x1
    let sa_ptr: MutPtr<u8> = mem.alloc(16).cast();
    mem.write(sa_ptr, 16u8); // sa_len (Offset 0)
    mem.write(MutPtr::from_bits(sa_ptr.to_bits() + 1), 2u8); // sa_family AF_INET (Offset 1)

    // 3. Allocate the ifaddrs struct
    let ifa_size = std::mem::size_of::<ifaddrs>() as u32;
    let ifa_ptr: MutPtr<ifaddrs> = mem.alloc(ifa_size).cast();
    
    let dummy_ifa = ifaddrs {
        ifa_next: MutPtr::null(),
        ifa_name: name_ptr.cast_const(),
        ifa_flags: 0x1 | 0x8, // IFF_UP | IFF_LOOPBACK
        ifa_addr: sa_ptr,      
        ifa_netmask: MutPtr::null(),
        ifa_broadaddr: MutPtr::null(),
        ifa_data: 0,
    };

    mem.write(ifa_ptr, dummy_ifa);
    mem.write(ifap, ifa_ptr);

    log!("getifaddrs(): Reported fake lo0 interface with valid dummy sockaddr.");
    0 
}

fn freeifaddrs(_env: &mut Environment, _ifa: MutPtr<ifaddrs>) {
    // Stubs usually don't need to actually free in this context
}

fn if_nametoindex(env: &mut Environment, ifname: ConstPtr<u8>) -> u32 {
    let name = env.mem.as_ref().cstr_at_utf8(ifname).unwrap_or("");
    if name == "en0" || name == "en1" || name == "lo0" {
        return 1;
    }
    set_errno(env, ENXIO);
    0
}

fn if_indextoname(_env: &mut Environment, ifindex: u32, ifname: MutPtr<u8>) -> MutPtr<u8> {
    if ifindex == 1 { return ifname; }
    MutPtr::null()
}

// THIS WAS THE MISSING PIECE:
pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(getifaddrs(_)),
    export_c_func!(freeifaddrs(_)),
    export_c_func!(if_nametoindex(_)),
    export_c_func!(if_indextoname(_, _)),
];
