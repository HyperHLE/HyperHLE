/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `sys/sysctl.h`

use std::collections::HashMap;
use std::sync::LazyLock;

use crate::dyld::{export_c_func, FunctionExports};
use crate::libc::errno::set_errno;
use crate::libc::sysctl::SysInfoType::String;
use crate::mem::{guest_size_of, ConstPtr, GuestUSize, MutPtr, MutVoidPtr, PAGE_SIZE};
use crate::Environment;

static SYSCTL_VALUES: [((i32, i32), &str, SysInfoType); 29] = [
    // Generic CPU, I/O
    ((6,1), "hw.machine" , String(b"iPhone2,1")), // overridden dynamically below
    ((6,2), "hw.model" , String(b"N88AP")),
    ((6,3), "hw.ncpu" , SysInfoType::Int32(1)),
    ((6,25), "hw.activecpu" , SysInfoType::Int32(1)), // Активные ядра
    // Physical / logical CPU counters introduced in 10.5 / iOS 4 and
    // documented in `<sys/sysctl.h>`. iPhone 1/2G/3G are single-core, so
    // every counter reads back as 1 — matching what a real iOS 4 device
    // returns for these names (see Apple's
    // <https://developer.apple.com/library/archive/documentation/Darwin/Conceptual/KernelProgramming/sysctl/sysctl.html>).
    ((0,0), "hw.physicalcpu" , SysInfoType::Int32(1)),
    ((0,0), "hw.physicalcpu_max", SysInfoType::Int32(1)),
    ((0,0), "hw.logicalcpu" , SysInfoType::Int32(1)),
    ((0,0), "hw.logicalcpu_max" , SysInfoType::Int32(1)),
    ((0,0), "hw.cputype" , SysInfoType::Int32(12)),
    ((0,0), "hw.cpusubtype" , SysInfoType::Int32(6)),
    ((6,15), "hw.cpufrequency" , SysInfoType::Int64(412000000)),
    ((6,16), "hw.cpufrequency_max", SysInfoType::Int64(412000000)),
    ((6,14), "hw.busfrequency" , SysInfoType::Int64(103000000)),

    // Честные параметры кэша для ARM1176JZF-S (iPhone 2G / 3G)
    ((0,0), "hw.cachelinesize", SysInfoType::Int32(32)),
    ((0,0), "hw.l1dcachesize", SysInfoType::Int32(16384)),
    ((0,0), "hw.l2cachesize", SysInfoType::Int32(0)),
    ((0,0), "hw.l3cachesize", SysInfoType::Int32(0)),

    ((1, 14), "kern.osversion", String(b"10B141")),
    ((6,5), "hw.physmem" , SysInfoType::Int32(536870912)),
    ((6,6), "hw.usermem" , SysInfoType::Int32(402653184)),
    ((6,24), "hw.memsize" , SysInfoType::Int32(536870912)),
    ((6,7), "hw.pagesize" , SysInfoType::Int32(PAGE_SIZE as i32)),
    // High kernel limits
    ((1,1), "kern.ostype" , String(b"Darwin")),
    ((1,2), "kern.osrelease" , String(b"13.0.0")),
    ((1,3), "kern.osversion" , String(b"10B141")),
    ((1,10), "kern.hostname" , String(b"touchHLE")),
    ((1,4), "kern.version" , String(b"Darwin Kernel Version 13.0.0: Wed Jun 13 16:55:00 PDT 2012; root:xnu-2107.7.55~11/RELEASE_ARM_S5L8920X")),
    ((1,21), "kern.boottime" , SysInfoType::Int64(1600000000)),
    // kern.proc.pid is a node for process information. Some games probe
    // it with sysctl([CTL_KERN, KERN_PROC, ...]) and only need success.
    ((1,65), "kern.proc.pid", SysInfoType::Bytes(b"")),
];

static STRING_MAP: LazyLock<HashMap<&str, SysInfoType>> = LazyLock::new(|| {
    // Can't use from_iter because the closure erases the lifetime
    let mut hashmap = HashMap::new();
    for (_, str, value) in SYSCTL_VALUES.iter() {
        hashmap.insert(*str, value.clone());
    }
    hashmap
});

#[allow(clippy::type_complexity)]
static INT_MAP: LazyLock<HashMap<(i32, i32), (&str, SysInfoType)>> = LazyLock::new(|| {
    // Can't use from_iter because the closure erases the lifetime
    let mut hashmap = HashMap::new();
    for (ints, str, value) in SYSCTL_VALUES.iter() {
        hashmap.insert(*ints, (*str, value.clone()));
    }
    hashmap
});

#[derive(Clone)]
enum SysInfoType {
    String(&'static [u8]),
    Int32(i32),
    Int64(i64),
    Bytes(&'static [u8]),
}

fn sysctl(
    env: &mut Environment,
    name: MutPtr<i32>,
    name_len: u32,
    oldp: MutVoidPtr,
    oldlenp: MutPtr<GuestUSize>,
    newp: MutVoidPtr,
    newlen: GuestUSize,
) -> i32 {
    set_errno(env, 0);

    log_dbg!(
        "sysctl({:?}, {:#x}, {:?}, {:?}, {:?}, {:x})",
        name,
        name_len,
        oldp,
        oldlenp,
        newp,
        newlen
    );

    // MIB arrays with more than 2 components are valid (e.g. used by Mono).
    // We only key on the first two elements; extra elements are ignored.
    if name_len < 2 {
        log!("sysctl(): name_len {} < 2, returning -1", name_len);
        return -1;
    }

    let (name0, name1) = (env.mem.read(name), env.mem.read(name + 1));

    // hw.machine depends on the emulated device family
    // В SYSCTL_VALUES hw.machine соответствует ключу (6, 1)
    if name0 == 6 && name1 == 1 {
        let machine_bytes: &'static [u8] = env.window().device_family().machine_name().as_bytes();
        // write directly: length + null terminator
        let len = machine_bytes.len() as GuestUSize + 1;
        if oldp.is_null() {
            env.mem.write(oldlenp, len);
            return 0;
        }
        let oldlen = env.mem.read(oldlenp);
        if oldlen < len {
            log!("sysctl hw.machine: buffer too small ({oldlen} < {len})");
            return -1;
        }
        let tmp = env.mem.alloc_and_write_cstr(machine_bytes);
        env.mem.memmove(oldp, tmp.cast().cast_const(), len);
        env.mem.free(tmp.cast());
        env.mem.write(oldlenp, len);
        return 0;
    }

    sysctl_generic(
        env,
        |_env| {
            // Используем INT_MAP для поиска по числовым идентификаторам (name0,
            // name1)
            let Some((name_str, val)) = INT_MAP.get(&(name0, name1)) else {
                // Убираем unimplemented!, чтобы избежать паники, просто
                // логируем и возвращаем ошибку, как в sysctlbyname
                log!(
                    "sysctl(): unknown parameter [{}, {}], returning -1",
                    name0,
                    name1
                );
                return None;
            };
            Some((*name_str, val.clone()))
        },
        oldp,
        oldlenp,
        newp,
        newlen,
    )
}

fn sysctlbyname(
    env: &mut Environment,
    name: ConstPtr<u8>,
    oldp: MutVoidPtr,
    oldlenp: MutPtr<GuestUSize>,
    newp: MutVoidPtr,
    newlen: GuestUSize,
) -> i32 {
    // TODO: handle errno properly
    set_errno(env, 0);

    let name_str = env.mem.cstr_at_utf8(name).unwrap();
    log_dbg!(
        "sysctlbyname({:?}, {:?}, {:?}, {:?}, {:x})",
        name_str,
        oldp,
        oldlenp,
        newp,
        newlen
    );
    sysctl_generic(
        env,
        |env| {
            let name_str = env.mem.cstr_at_utf8(name).unwrap();
            // hw.machine depends on the emulated device family, like in the
            // numeric sysctl() path above. The static table value is only a
            // placeholder; without this, apps that check the device model via
            // sysctlbyname (e.g. BioShock's device whitelist) would always
            // see the first-gen iPhone regardless of --device-family.
            if name_str == "hw.machine" {
                let machine: &'static str = env.window().device_family().machine_name();
                return Some(("hw.machine", String(machine.as_bytes())));
            }
            let Some((name_str, val)) = STRING_MAP.get_key_value(name_str) else {
                log!(
                    "sysctlbyname(): unknown parameter {}, returning -1",
                    name_str
                );
                return None;
            };
            Some((name_str, val.clone()))
        },
        oldp,
        oldlenp,
        newp,
        newlen,
    )
}

fn sysctl_generic<F>(
    env: &mut Environment,
    // Returns the name and value of the property, or None if unknown.
    name_lookup: F,
    oldp: MutVoidPtr,
    oldlenp: MutPtr<GuestUSize>,
    newp: MutVoidPtr,
    newlen: GuestUSize,
) -> i32
where
    F: FnOnce(&mut Environment) -> Option<(&'static str, SysInfoType)>,
{
    // Per POSIX, sysctl with non-null newp sets a value. iOS apps sometimes
    // call this (e.g. Mono runtime trying to set kern.osrelease), but on
    // real iOS it silently fails with EPERM. Mirror that instead of crashing.
    if !newp.is_null() || newlen != 0 {
        log!(
            "Warning: sysctl: write attempt (newp={:?}, newlen={}) — returning EPERM (-1)",
            newp,
            newlen
        );
        set_errno(env, 1); // EPERM
        return -1;
    }

    let Some((name_str, val)) = name_lookup(env) else {
        return -1;
    };
    let len: GuestUSize = match val {
        String(str) => str.len() as GuestUSize + 1,
        SysInfoType::Int32(_) => guest_size_of::<i32>(),
        SysInfoType::Int64(_) => guest_size_of::<i64>(),
        SysInfoType::Bytes(bytes) => bytes.len() as GuestUSize,
    };
    if oldp.is_null() {
        env.mem.write(oldlenp, len);
        return 0;
    }
    assert!(!oldp.is_null() && !oldlenp.is_null());
    let oldlen = env.mem.read(oldlenp);
    if oldlen < len {
        // On real iOS, sysctl writes partial data when the buffer is too small
        // for integer types.  Many apps (e.g. GunstarHeroes) pass a 4-byte
        // buffer for hw.cpufrequency / hw.busfrequency which are Int64.
        // iOS truncates to the available buffer size rather than failing.
        match &val {
            SysInfoType::Int64(num) if oldlen >= guest_size_of::<i32>() => {
                // Truncate to low 32 bits (little-endian) — matches real device
                // behavior where the lower word is written.
                let truncated = *num as i32;
                log_dbg!(
                    "sysctl(byname) for '{name_str}': buffer {oldlen} < {len}, truncating Int64 to Int32 ({truncated})"
                );
                env.mem.write(oldp.cast(), truncated);
                env.mem.write(oldlenp, guest_size_of::<i32>());
                return 0;
            }
            SysInfoType::Bytes(bytes) => {
                let copy_len = oldlen.min(len);
                if copy_len > 0 {
                    let tmp = env.mem.alloc(copy_len);
                    env.mem
                        .bytes_at_mut(tmp.cast(), copy_len)
                        .copy_from_slice(&bytes[..copy_len as usize]);
                    env.mem.memmove(oldp, tmp.cast_const().cast(), copy_len);
                    env.mem.free(tmp);
                }
                env.mem.write(oldlenp, copy_len);
                return 0;
            }
            _ => {
                log!("sysctl(byname) for '{name_str}': the buffer of size {oldlen} is too low to fit the value of size {len}, returning -1");
                return -1;
            }
        }
    }
    match val {
        String(str) => {
            let sysctl_str = env.mem.alloc_and_write_cstr(str);
            env.mem.memmove(oldp, sysctl_str.cast().cast_const(), len);
            env.mem.free(sysctl_str.cast());
        }
        SysInfoType::Int32(num) => {
            env.mem.write(oldp.cast(), num);
        }
        SysInfoType::Int64(num) => {
            env.mem.write(oldp.cast(), num);
        }
        SysInfoType::Bytes(bytes) => {
            if len > 0 {
                let tmp = env.mem.alloc(len);
                env.mem.bytes_at_mut(tmp.cast(), len).copy_from_slice(bytes);
                env.mem.memmove(oldp, tmp.cast_const().cast(), len);
                env.mem.free(tmp);
            }
        }
    }
    env.mem.write(oldlenp, len);
    0 // success
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(sysctl(_, _, _, _, _, _)),
    export_c_func!(sysctlbyname(_, _, _, _, _)),
];
