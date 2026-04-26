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

static SYSCTL_VALUES: [((i32, i32), &str, SysInfoType); 19] = [
    // Generic CPU, I/O
    ((6,1), "hw.machine" , String(b"iPhone1,1")), // overridden dynamically below
    ((6,2), "hw.model" , String(b"M68AP")),
    ((6,3), "hw.ncpu" , SysInfoType::Int32(1)),
    ((0,0), "hw.cputype" , SysInfoType::Int32(12)),
    ((0,0), "hw.cpusubtype" , SysInfoType::Int32(6)),
    ((6,15), "hw.cpufrequency" , SysInfoType::Int64(412000000)),
    ((6,16), "hw.cpufrequency_max", SysInfoType::Int64(412000000)), // Добавлена макс. частота
    ((6,14), "hw.busfrequency" , SysInfoType::Int64(103000000)),
    ((1, 14), "kern.osversion", String(b"5A347")),
    ((6,5), "hw.physmem" , SysInfoType::Int32(121634816)),
    ((6,6), "hw.usermem" , SysInfoType::Int32(93564928)),
    ((6,24), "hw.memsize" , SysInfoType::Int32(121634816)),
    ((6,7), "hw.pagesize" , SysInfoType::Int32(PAGE_SIZE as i32)),
    // High kernel limits
    ((1,1), "kern.ostype" , String(b"Darwin")),
    ((1,2), "kern.osrelease" , String(b"10.4.0")),
    ((1,3), "kern.osversion" , String(b"8A293")),
    ((1,10), "kern.hostname" , String(b"touchHLE")),
    ((1,4), "kern.version" , String(b"Darwin Kernel Version 10.4.0: Thu Jun 10 14:26:58 PDT 2010; root:xnu-1504.58.2~1/RELEASE_ARM_S5L8900X")),
    ((1,21), "kern.boottime" , SysInfoType::Int64(1600000000)), // Добавлено время загрузки (struct timeval в виде 8 байт)
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
            // Используем INT_MAP для поиска по числовым идентификаторам (name0, name1)
            let Some((name_str, val)) = INT_MAP.get(&(name0, name1)) else {
                // Убираем unimplemented!, чтобы избежать паники, просто логируем и возвращаем ошибку, как в sysctlbyname
                log!("sysctl(): unknown parameter [{}, {}], returning -1", name0, name1);
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
    assert!(newp.is_null());
    assert_eq!(newlen, 0);

    let Some((name_str, val)) = name_lookup(env) else {
        return -1;
    };
    let len: GuestUSize = match val {
        String(str) => str.len() as GuestUSize + 1,
        SysInfoType::Int32(_) => guest_size_of::<i32>(),
        SysInfoType::Int64(_) => guest_size_of::<i64>(),
    };
    if oldp.is_null() {
        env.mem.write(oldlenp, len);
        return 0;
    }
    assert!(!oldp.is_null() && !oldlenp.is_null());
    let oldlen = env.mem.read(oldlenp);
    if oldlen < len {
        // TODO: set errno
        // TODO: write partial data
        log!("sysctl(byname) for '{name_str}': the buffer of size {oldlen} is too low to fit the value of size {len}, returning -1");
        return -1;
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
    }
    env.mem.write(oldlenp, len);
    0 // success
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(sysctl(_, _, _, _, _, _)),
    export_c_func!(sysctlbyname(_, _, _, _, _)),
];
