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
    pub ifa_addr: u32,
    pub ifa_netmask: u32,
    pub ifa_broadaddr: u32,
    pub ifa_data: u32,
}
unsafe impl SafeRead for ifaddrs {}

/// `int getifaddrs(struct ifaddrs **ifap)`
fn getifaddrs(env: &mut Environment, ifap: MutPtr<MutPtr<ifaddrs>>) -> i32 {
    if ifap.is_null() { return -1; }

    let mem = env.mem.as_mut();

    let name_str = "lo0\0";
    let name_len = name_str.len() as u32;
    
    // Step 1: Allocate name and write using raw bits to avoid .offset() errors
    let name_ptr: MutPtr<u8> = mem.alloc(name_len).cast();
    let name_base_addr = name_ptr.to_bits(); 
    
    for (i, &byte) in name_str.as_bytes().iter().enumerate() {
        let addr = MutPtr::<u8>::from_bits(name_base_addr + i as u32);
        mem.write(addr, byte);
    }

    // Step 2: Allocate the ifaddrs struct
    let ifa_size = std::mem::size_of::<ifaddrs>() as u32;
    let ifa_ptr: MutPtr<ifaddrs> = mem.alloc(ifa_size).cast();
    
    // Step 3: Create dummy data using bit-conversion to avoid .into() errors
    let dummy_ifa = ifaddrs {
        ifa_next: MutPtr::null(),
        ifa_name: ConstPtr::from_bits(name_base_addr),
        ifa_flags: 0x1 | 0x8, // IFF_UP | IFF_LOOPBACK
        ifa_addr: 0,
        ifa_netmask: 0,
        ifa_broadaddr: 0,
        ifa_data: 0,
    };

    mem.write(ifa_ptr, dummy_ifa);
    mem.write(ifap, ifa_ptr);

    log!("getifaddrs(): Reported fake lo0 interface.");
    0 
}

fn freeifaddrs(_env: &mut Environment, _ifa: MutPtr<ifaddrs>) {}

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

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(getifaddrs(_)),
    export_c_func!(freeifaddrs(_)),
    export_c_func!(if_nametoindex(_)),
    export_c_func!(if_indextoname(_, _)),
];
