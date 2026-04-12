/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `mach_host.h` and some other related functions

#![allow(non_camel_case_types)]

use crate::dyld::FunctionExports;
use crate::libc::mach::core_types::natural_t;
use crate::libc::mach::port::mach_port_t;
use crate::libc::mach::thread_info::{kern_return_t, mach_msg_type_number_t, KERN_SUCCESS};
use crate::mem::{guest_size_of, MutPtr, SafeRead, PAGE_SIZE};
use crate::{export_c_func, Environment};

type host_t = mach_port_t;
type host_name_port_t = host_t;
type host_flavor_t = natural_t;
type host_info_t = MutPtr<natural_t>;
type vm_size_t = natural_t;
type clock_id_t = natural_t;
type clock_serv_t = mach_port_t;

// The value doesn't matter that much, only the fact that it's unique
// per host so we could assert against it in our code.
const MACH_HOST_SELF: host_name_port_t = 0x100c442e;

// Clock service port sentinels — arbitrary unique values, one per clock ID.
const CLOCK_PORT_REALTIME:  clock_serv_t = 0x200c0001;
const CLOCK_PORT_MONOTONIC: clock_serv_t = 0x200c0002;
const CLOCK_PORT_CALENDAR:  clock_serv_t = 0x200c0003;

// clock_id_t values (from <mach/clock_types.h>)
const SYSTEM_CLOCK:   clock_id_t = 0; // monotonic uptime
const CALENDAR_CLOCK: clock_id_t = 1; // wall-clock (UTC)
const REALTIME_CLOCK: clock_id_t = 2; // alias for SYSTEM_CLOCK on Darwin

// Values taken from an iPod Touch 4 running iOS 6.1
// Used in host_statistics function (returned in vm_statistics)
// Also used to calcuate PHYSICAL_MEMORY (used by NSProcessInfo)
const FREE_COUNT:     natural_t = 12897;
const ACTIVE_COUNT:   natural_t = 0;
const INACTIVE_COUNT: natural_t = 0;
const WIRE_COUNT:     natural_t = 0;

pub const PHYSICAL_MEMORY: natural_t =
    (FREE_COUNT + ACTIVE_COUNT + INACTIVE_COUNT + WIRE_COUNT) * PAGE_SIZE;

const HOST_VM_INFO: host_flavor_t = 2;

#[repr(C, packed)]
struct vm_statistics {
    free_count: natural_t,
    active_count: natural_t,
    inactive_count: natural_t,
    wire_count: natural_t,
    zero_fill_count: natural_t,
    reactivations: natural_t,
    pageins: natural_t,
    pageouts: natural_t,
    faults: natural_t,
    cow_faults: natural_t,
    lookups: natural_t,
    hits: natural_t,
    purgeable_count: natural_t,
    purges: natural_t,
    speculative_count: natural_t,
}
unsafe impl SafeRead for vm_statistics {}

fn mach_host_self(_env: &mut Environment) -> host_name_port_t {
    MACH_HOST_SELF
}

fn host_page_size(
    env: &mut Environment,
    host: host_t,
    out_page_size: MutPtr<vm_size_t>,
) -> kern_return_t {
    assert_eq!(host, MACH_HOST_SELF);
    env.mem.write(out_page_size, PAGE_SIZE);
    KERN_SUCCESS
}

fn host_statistics(
    env: &mut Environment,
    host: host_t,
    flavor: host_flavor_t,
    host_info_out: host_info_t,
    host_info_out_count: MutPtr<mach_msg_type_number_t>,
) -> kern_return_t {
    assert_eq!(host, MACH_HOST_SELF);
    assert_eq!(flavor, HOST_VM_INFO);
    let out_size_available = env.mem.read(host_info_out_count);
    let out_size_expected = guest_size_of::<vm_statistics>() / guest_size_of::<natural_t>();
    assert_eq!(out_size_expected, out_size_available);
    env.mem.write(
        host_info_out.cast(),
        vm_statistics {
            free_count: FREE_COUNT,
            active_count: ACTIVE_COUNT,
            inactive_count: INACTIVE_COUNT,
            wire_count: WIRE_COUNT,
            zero_fill_count: 0,
            reactivations: 0,
            pageins: 0,
            pageouts: 0,
            faults: 0,
            cow_faults: 0,
            lookups: 0,
            hits: 0,
            purgeable_count: 0,
            purges: 0,
            speculative_count: 0,
        },
    );
    KERN_SUCCESS
}

// =========================================================================
// MARK: - Clock services
// =========================================================================

/// `kern_return_t host_get_clock_service(host_t host, clock_id_t clock_id,
///                                       clock_serv_t *clock_serv)`
///
/// Returns a send right to the named clock service. Apps use the returned
/// port with `clock_get_time` / `clock_get_attributes`. We hand back a
/// stable sentinel port value per clock ID so the caller can hold and pass
/// it around freely.
fn host_get_clock_service(
    env: &mut Environment,
    host: host_t,
    clock_id: clock_id_t,
    clock_serv: MutPtr<clock_serv_t>,
) -> kern_return_t {
    assert_eq!(host, MACH_HOST_SELF);
    let port = match clock_id {
        SYSTEM_CLOCK   => CLOCK_PORT_MONOTONIC,
        CALENDAR_CLOCK => CLOCK_PORT_CALENDAR,
        REALTIME_CLOCK => CLOCK_PORT_REALTIME,
        other => {
            log!("host_get_clock_service: unknown clock_id {}, returning monotonic port", other);
            CLOCK_PORT_MONOTONIC
        }
    };
    env.mem.write(clock_serv, port);
    log_dbg!(
        "host_get_clock_service(clock_id={}) => port {:#010x}",
        clock_id, port
    );
    KERN_SUCCESS
}

/// `kern_return_t clock_get_time(clock_serv_t clock_serv,
///                               mach_timespec_t *cur_time)`
///
/// Fills `*cur_time` with the current time for the given clock service port.
/// `mach_timespec_t` is `{ tv_sec: u32, tv_nsec: u32 }`.
#[repr(C, packed)]
struct mach_timespec_t {
    tv_sec:  u32,
    tv_nsec: u32,
}
unsafe impl SafeRead for mach_timespec_t {}

fn clock_get_time(
    env: &mut Environment,
    clock_serv: clock_serv_t,
    cur_time: MutPtr<mach_timespec_t>,
) -> kern_return_t {
    let (secs, nanos): (u64, u32) = match clock_serv {
        CLOCK_PORT_MONOTONIC | CLOCK_PORT_REALTIME => {
            // Monotonic: seconds and nanoseconds since process startup.
            let d = std::time::Instant::now().duration_since(env.startup_time);
            (d.as_secs(), d.subsec_nanos())
        }
        CLOCK_PORT_CALENDAR => {
            // Calendar: seconds since Unix epoch via SystemTime.
            match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
                Ok(d) => (d.as_secs(), d.subsec_nanos()),
                Err(_) => (0, 0),
            }
        }
        other => {
            log!("clock_get_time: unknown clock_serv port {:#010x}", other);
            (0, 0)
        }
    };
    env.mem.write(cur_time, mach_timespec_t {
        tv_sec:  secs as u32,
        tv_nsec: nanos,
    });
    log_dbg!("clock_get_time(port={:#010x}) => {}.{:09}", clock_serv, secs, nanos);
    KERN_SUCCESS
}

/// `kern_return_t clock_get_attributes(clock_serv_t clock_serv,
///                                     clock_flavor_t flavor,
///                                     clock_attr_t attr,
///                                     mach_msg_type_number_t *attr_count)`
///
/// Stub — most apps only call this to query the clock resolution, which we
/// report as 1 ms (1 000 000 ns), a safe conservative value.
fn clock_get_attributes(
    env: &mut Environment,
    clock_serv: clock_serv_t,
    flavor: natural_t,
    attr: MutPtr<natural_t>,
    attr_count: MutPtr<mach_msg_type_number_t>,
) -> kern_return_t {
    log_dbg!(
        "clock_get_attributes(port={:#010x}, flavor={}) — returning 1ms resolution",
        clock_serv, flavor
    );
    // CLOCK_GET_TIME_RES (flavor 1): resolution in nanoseconds.
    // Write a single natural_t value of 1 000 000 (1 ms).
    if !attr.is_null() {
        env.mem.write(attr, 1_000_000u32);
    }
    if !attr_count.is_null() {
        env.mem.write(attr_count, 1);
    }
    KERN_SUCCESS
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(mach_host_self()),
    export_c_func!(host_page_size(_, _)),
    export_c_func!(host_statistics(_, _, _, _)),
    export_c_func!(host_get_clock_service(_, _, _)),
    export_c_func!(clock_get_time(_, _)),
    export_c_func!(clock_get_attributes(_, _, _, _)),
];
