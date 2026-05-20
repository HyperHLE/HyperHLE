/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Our implementations of various things that Apple's libSystem would provide.
//!
//! On other platforms these are part of the "libc", so let's call it that.
//!
//! Useful resources:
//!
//! - Apple's [iOS Manual Pages](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/) (contains what would be `man` pages if iOS had a command line)

pub mod arch;
pub mod arpa;
pub mod asl;
pub mod blocks;
pub mod clocale;
pub mod crypto;
pub mod ctype;
pub mod cxxabi;
pub mod dirent;
pub mod dispatch;
pub mod dlfcn;
pub mod dns_sd;
pub mod errno;
pub mod fnmatch;
mod generic_char;
pub mod glob;
pub mod ifaddrs;
pub mod keymgr;
pub mod libkern;
pub mod mach;
pub mod mach_o;
pub mod math;
pub mod mmap;
pub mod net;
pub mod netdb;
pub mod posix_io;
pub mod pthread;
pub mod sched;
pub mod semaphore;
pub mod setjmp;
pub mod signal;
pub mod ssp;
pub mod stdio;
pub mod stdlib;
pub mod string;
pub mod sys;
pub mod sysctl;
pub mod time;
pub mod unistd;
pub mod wchar;

pub const DYLIB: crate::dyld::HostDylib = crate::dyld::HostDylib {
    path: "/usr/lib/libSystem.B.dylib",
    aliases: &["/usr/lib/libSystem.dylib"],
    class_exports: &[],
    constant_exports: &[
        ctype::CONSTANTS,
        dispatch::CONSTANTS,
        stdio::CONSTANTS,
        mach::init::CONSTANTS,
        ssp::CONSTANTS,
    ],
    function_exports: &[
        arch::FUNCTIONS,
        arpa::inet::FUNCTIONS,
        asl::FUNCTIONS,
        blocks::FUNCTIONS,
        clocale::FUNCTIONS,
        ctype::FUNCTIONS,
        cxxabi::FUNCTIONS,
        crypto::FUNCTIONS,
        dirent::FUNCTIONS,
        dispatch::FUNCTIONS,
        dlfcn::FUNCTIONS,
        dns_sd::FUNCTIONS,
        errno::FUNCTIONS,
        fnmatch::FUNCTIONS,
        glob::FUNCTIONS,
        ifaddrs::FUNCTIONS,
        keymgr::FUNCTIONS,
        libkern::os_atomic::FUNCTIONS,
        mach::arm::task::FUNCTIONS,
        mach::arm::thread_act::FUNCTIONS,
        libkern::task::FUNCTIONS,
        mach::host::FUNCTIONS,
        mach::init::FUNCTIONS,
        mach::mach_port::FUNCTIONS,
        mach::message::FUNCTIONS,
        mach::semaphore::FUNCTIONS,
        mach::thread_info::FUNCTIONS,
        mach::time::FUNCTIONS,
        mach::vm_map::FUNCTIONS,
        mach_o::FUNCTIONS,
        math::FUNCTIONS,
        mmap::FUNCTIONS,
        net::if_::FUNCTIONS,
        netdb::FUNCTIONS,
        posix_io::FUNCTIONS,
        posix_io::stat::FUNCTIONS,
        posix_io::statvfs::FUNCTIONS,
        pthread::cond::FUNCTIONS,
        pthread::key::FUNCTIONS,
        pthread::mutex::FUNCTIONS,
        pthread::once::FUNCTIONS,
        pthread::thread::FUNCTIONS,
        sched::FUNCTIONS,
        semaphore::FUNCTIONS,
        setjmp::FUNCTIONS,
        signal::FUNCTIONS,
        ssp::FUNCTIONS,
        stdio::FUNCTIONS,
        stdio::printf::FUNCTIONS,
        stdlib::FUNCTIONS,
        stdlib::qsort::FUNCTIONS,
        string::FUNCTIONS,
        sys::mount::FUNCTIONS,
        sys::ptrace::FUNCTIONS,
        sys::timeb::FUNCTIONS,
        sys::socket::FUNCTIONS,
        sys::utsname::FUNCTIONS,
        sys::wait::FUNCTIONS,
        sysctl::FUNCTIONS,
        time::FUNCTIONS,
        unistd::FUNCTIONS,
        wchar::FUNCTIONS,
    ],
};

/// Container for state of various child modules
#[derive(Default)]
pub struct State {
    dirent: dirent::State,
    dispatch: dispatch::State,
    keymgr: keymgr::State,
    math: math::State,
    netdb: netdb::State,
    posix_io: posix_io::State,
    pub pthread: pthread::State,
    pub semaphore: semaphore::State,
    pub socket: sys::socket::State,
    stdlib: stdlib::State,
    string: string::State,
    signal: signal::State,
    stdio: stdio::State,
    time: time::State,
    errno: errno::State,
    clocale: clocale::State,
    mach_vm: mach::vm_map::State,
    mmap: mmap::State,
}
