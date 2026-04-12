/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Miscellaneous parts of `unistd.h`

use crate::dyld::{export_c_func, FunctionExports};
use crate::fs::GuestPath;
use crate::libc::errno::{set_errno, EACCES, EINVAL, ENOENT, ENOSYS, EROFS};
use crate::libc::posix_io::{FileDescriptor, STDERR_FILENO, STDIN_FILENO, STDOUT_FILENO};
use crate::mem::{ConstPtr, GuestISize, GuestUSize, MutPtr, PAGE_SIZE};
use crate::Environment;
use std::time::Duration;

#[allow(non_camel_case_types)]
type useconds_t = u32;

const F_OK: i32 = 0; // file existence
const X_OK: i32 = 1; // execute/search permission
const W_OK: i32 = 2; // write permission
const R_OK: i32 = 4; // read permission
const B_OK: i32 = 5;
const T_OK: i32 = 6;
const P_OK: i32 = 7;
const A_OK: i32 = 8;
const C_OK: i32 = 9;
const D_OK: i32 = 10;

type SysConfName = i32;
const _SC_ARG_MAX:           SysConfName = 1;
const _SC_CHILD_MAX:         SysConfName = 2;
const _SC_CLK_TCK:           SysConfName = 3;
const _SC_NGROUPS_MAX:       SysConfName = 4;
const _SC_OPEN_MAX:          SysConfName = 5;
const _SC_JOB_CONTROL:       SysConfName = 6;
const _SC_SAVED_IDS:         SysConfName = 7;
const _SC_VERSION:           SysConfName = 8;
const _SC_BC_BASE_MAX:       SysConfName = 9;
const _SC_BC_DIM_MAX:        SysConfName = 10;
const _SC_BC_SCALE_MAX:      SysConfName = 11;
const _SC_BC_STRING_MAX:     SysConfName = 12;
const _SC_COLL_WEIGHTS_MAX:  SysConfName = 13;
const _SC_EXPR_NEST_MAX:     SysConfName = 14;
const _SC_LINE_MAX:          SysConfName = 15;
const _SC_RE_DUP_MAX:        SysConfName = 16;
const _SC_2_VERSION:         SysConfName = 17;
const _SC_2_C_BIND:          SysConfName = 18;
const _SC_2_C_DEV:           SysConfName = 19;
const _SC_2_CHAR_TERM:       SysConfName = 20;
const _SC_2_FORT_DEV:        SysConfName = 21;
const _SC_2_FORT_RUN:        SysConfName = 22;
const _SC_2_LOCALEDEF:       SysConfName = 23;
const _SC_2_SW_DEV:          SysConfName = 24;
const _SC_2_UPE:             SysConfName = 25;
const _SC_STREAM_MAX:        SysConfName = 26;
const _SC_TZNAME_MAX:        SysConfName = 27;
const _SC_PAGESIZE:          SysConfName = 29;
const _SC_NPROCESSORS_CONF:  SysConfName = 57;
const _SC_NPROCESSORS_ONLN:  SysConfName = 58;
const _SC_PHYS_PAGES:        SysConfName = 200;
const _SC_MONOTONIC_CLOCK:   SysConfName = 201;
const _SC_THREAD_SAFE_FUNCTIONS: SysConfName = 202;

fn sleep(env: &mut Environment, seconds: u32) -> u32 {
    env.sleep(Duration::from_secs(seconds.into()));
    // sleep() returns the amount of time remaining that should have been slept,
    // but wasn't, if the thread was woken up early by a signal.
    // touchHLE never does that currently, so 0 is always correct here.
    0
}

fn usleep(env: &mut Environment, useconds: useconds_t) -> i32 {
    // TODO: handle errno properly
    set_errno(env, 0);

    env.sleep(Duration::from_micros(useconds.into()));
    0 // success
}

#[allow(non_camel_case_types)]
pub type pid_t = i32;
#[allow(non_camel_case_types)]
type gid_t = u32;

pub fn getpid(_env: &mut Environment) -> pid_t {
    // Not a real value, since touchHLE only simulates a single process.
    // PID 0 would be init, which is a bit unrealistic, so let's go with 1.
    1
}
fn getppid(_env: &mut Environment) -> pid_t {
    // Included just for completeness. Surely no app ever calls this.
    0
}
fn getgid(_env: &mut Environment) -> gid_t {
    // Not a real value
    0
}

fn isatty(env: &mut Environment, fd: FileDescriptor) -> i32 {
    // TODO: handle errno properly
    set_errno(env, 0);

    if [STDIN_FILENO, STDOUT_FILENO, STDERR_FILENO].contains(&fd) {
        1
    } else {
        0
    }
}

fn access(env: &mut Environment, path: ConstPtr<u8>, mode: i32) -> i32 {
    // TODO: handle errno properly
    set_errno(env, 0);

    let binding = env.mem.cstr_at_utf8(path).unwrap();
    let guest_path = GuestPath::new(&binding);
    let (exists, read, write, execute) = env.fs.access(guest_path);
    // TODO: support ORing
    match mode {
        F_OK => {
            if exists {
                0
            } else {
                set_errno(env, ENOENT);
                -1
            }
        }
        X_OK => {
            if execute {
                0
            } else {
                set_errno(env, EACCES);
                -1
            }
        }
        W_OK => {
            if write {
                0
            } else {
                set_errno(env, EROFS);
                -1
            }
        }
        R_OK => {
            if read {
                0
            } else {
                // TODO: is it the correct error?
                set_errno(env, EACCES);
                -1
            }
        }
        B_OK => {
            if read {
                0
            } else {
                // TODO: is it the correct error?
                set_errno(env, EACCES);
                -1
            }
        }
        T_OK => {
            if read {
                0
            } else {
                // TODO: is it the correct error?
                set_errno(env, EACCES);
                -1
            }
        }      
        P_OK => {
            if read {
                0
            } else {
                // TODO: is it the correct error?
                set_errno(env, EACCES);
                -1
            }
        }
        A_OK => {
            if read {
                0
            } else {
                // TODO: is it the correct error?
                set_errno(env, EACCES);
                -1
            }
        }
        C_OK => {
            if read {
                0
            } else {
                // TODO: is it the correct error?
                set_errno(env, EACCES);
                -1
            }
        }
        D_OK => {
            if read {
                0
            } else {
                // TODO: is it the correct error?
                set_errno(env, EACCES);
                -1
            }
        }
        _ => unimplemented!("{}", mode),
    }
}

fn fork(env: &mut Environment) -> i32 {
    // fork() is not supported in touchHLE — iOS does not support forking
    // Return -1 and set errno to ENOSYS
    log!("Warning: fork() called but is not supported on iOS, returning -1");
    set_errno(env, ENOSYS);
    -1
}

fn unlink(env: &mut Environment, path: ConstPtr<u8>) -> i32 {
    // TODO: handle errno properly
    set_errno(env, 0);

    log_dbg!("unlink({:?} '{:?}')", path, env.mem.cstr_at_utf8(path));

    let path_str = env.mem.cstr_at_utf8(path).unwrap();
    let guest_path = GuestPath::new(&path_str);
    match env.fs.remove(guest_path) {
        Ok(()) => 0,
        Err(_) => {
            log!(
                "unlink({:?} '{:?}') failed",
                path,
                env.mem.cstr_at_utf8(path)
            );
            -1
        }
    }
}

fn gethostname(env: &mut Environment, name: MutPtr<u8>, namelen: GuestUSize) -> i32 {
    // TODO: define unique hostname once networking is supported
    let hostname = "touchHLE";
    let len: GuestUSize = hostname.len().try_into().unwrap();
    // TODO: check against HOST_NAME_MAX
    assert!(namelen > len);
    env.mem
        .bytes_at_mut(name, len)
        .copy_from_slice(hostname.as_bytes());
    env.mem.write(name + len, b'\0');
    0 // Success
}

fn getpagesize(_env: &mut Environment) -> i32 {
    PAGE_SIZE.try_into().unwrap()
}

fn readlink(
    env: &mut Environment,
    path: ConstPtr<u8>,
    buf: MutPtr<u8>,
    buf_size: GuestISize,
) -> GuestISize {
    log!(
        "TODO: readlink({:?} '{}', {:?}, {}) -> -1",
        path,
        env.mem.cstr_at_utf8(path).unwrap(),
        buf,
        buf_size,
    );
    // Current implementation of guest's file system doesn't
    // support symbolic links, so the call should unconditionally fail.
    set_errno(env, EINVAL);
    -1
}

fn getdtablesize(_env: &mut Environment) -> i32 {
    // Both macOS 15.7.4 and iOS 4.0.1 reports same dtable size.
    // TODO: Issue an error on `open` if table is full.
    256
}

fn sysconf(_env: &mut Environment, name: SysConfName) -> i32 {
    match name {
        _SC_PAGESIZE             => PAGE_SIZE.try_into().unwrap(),
        _SC_NPROCESSORS_CONF |
        _SC_NPROCESSORS_ONLN     => 1,
        _SC_PHYS_PAGES           => 131072,   // ~512 MiB / 4 KiB pages
        _SC_CLK_TCK              => 100,
        _SC_OPEN_MAX             => 256,
        _SC_ARG_MAX              => 262144,
        _SC_CHILD_MAX            => -1,       // no limit
        _SC_NGROUPS_MAX          => 16,
        _SC_STREAM_MAX           => 20,
        _SC_TZNAME_MAX           => 255,
        _SC_LINE_MAX             => 2048,
        _SC_RE_DUP_MAX           => 255,
        _SC_JOB_CONTROL          => 1,
        _SC_SAVED_IDS            => 1,
        _SC_VERSION              => 200112,   // POSIX.1-2001
        _SC_2_VERSION            => 200112,
        _SC_2_C_BIND |
        _SC_2_C_DEV  |
        _SC_2_SW_DEV             => 1,
        _SC_MONOTONIC_CLOCK      => 1,
        _SC_THREAD_SAFE_FUNCTIONS => 1,
        _ => {
            log!("sysconf({}): unknown name, returning -1", name);
            -1
        }
    }
}

fn pipe(env: &mut Environment, _fds: MutPtr<FileDescriptor>) -> i32 { 
    log!("pipe(): stubbed, returning -1"); 
    set_errno(env, ENOSYS); -1 
}


fn sbrk(env: &mut Environment, increment: GuestISize) -> GuestISize {
    // sbrk() is used by legacy malloc implementations to grow the heap.
    // touchHLE manages guest memory separately — return -1 to signal failure,
    // causing the C runtime to fall back to mmap-based allocation.
    log_dbg!("sbrk({}) -> -1 (not supported)", increment);
    set_errno(env, ENOSYS);
    -1
}

fn chmod(env: &mut Environment, path: ConstPtr<u8>, _mode: u32) -> i32 {
    // touchHLE guest filesystem is read-only for bundle files.
    // chmod is a no-op — return success to keep apps happy.
    log_dbg!("chmod('{}', ...) -> 0 (stubbed)",
        env.mem.cstr_at_utf8(path).unwrap_or_default());
    0
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(sleep(_)),
    export_c_func!(usleep(_)),
    export_c_func!(getpid()),
    export_c_func!(getppid()),
    export_c_func!(isatty(_)),
    export_c_func!(access(_, _)),
    export_c_func!(unlink(_)),
    export_c_func!(gethostname(_, _)),
    export_c_func!(getpagesize()),
    export_c_func!(getgid()),
    export_c_func!(readlink(_, _, _)),
    export_c_func!(getdtablesize()),
    export_c_func!(sysconf(_)),
    export_c_func!(pipe(_)),
    export_c_func!(fork()),
    export_c_func!(sbrk(_)),
    export_c_func!(chmod(_, _)),
];