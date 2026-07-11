/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! POSIX `sys/stat.h`

use super::{off_t, FileDescriptor, STDERR_FILENO, STDIN_FILENO, STDOUT_FILENO};
use crate::dyld::{export_c_func, FunctionExports};
use crate::fs::{FsError, GuestFile, GuestPath};
use crate::libc::errno::{set_errno, EACCES, EBADF, EEXIST, ENOENT};
use crate::libc::time::timespec;
use crate::mem::{ConstPtr, MutPtr, SafeRead};
use crate::Environment;

#[allow(non_camel_case_types)]
pub type dev_t = u32;
#[allow(non_camel_case_types)]
pub type mode_t = u16;
#[allow(non_camel_case_types)]
pub type nlink_t = u16;
#[allow(non_camel_case_types)]
pub type ino_t = u64;
#[allow(non_camel_case_types)]
pub type uid_t = u32;
#[allow(non_camel_case_types)]
pub type gid_t = u32;
#[allow(non_camel_case_types)]
pub type blkcnt_t = u64;
#[allow(non_camel_case_types)]
pub type blksize_t = u32;

// enum values sourced from `man 2 stat`
pub const S_IFIFO: mode_t = 0o0010000;
pub const S_IFCHR: mode_t = 0o0020000;
pub const S_IFDIR: mode_t = 0o0040000;
pub const S_IFREG: mode_t = 0o0100000;

#[allow(non_camel_case_types)]
#[derive(Default)]
#[repr(C, packed)]
pub struct stat {
    st_dev: dev_t,
    st_mode: mode_t,
    st_nlink: nlink_t,
    st_ino: ino_t,
    st_uid: uid_t,
    st_gid: gid_t,
    st_rdev: dev_t,
    st_atimespec: timespec,
    st_mtimespec: timespec,
    st_ctimespec: timespec,
    st_birthtimespec: timespec,
    st_size: off_t,
    st_blocks: blkcnt_t,
    st_blksize: blksize_t,
    st_flags: u32,
    st_gen: u32,
    st_lspare: i32,
    st_qspare: [i64; 2],
}
unsafe impl SafeRead for stat {}

fn mkdir(env: &mut Environment, path: ConstPtr<u8>, mode: mode_t) -> i32 {
    set_errno(env, 0);

    // Безопасное чтение пути, чтобы избежать panic через unwrap()
    let path_str = match env.mem.cstr_at_utf8(path) {
        Ok(s) => s.to_string(), // Отвязываем от заимствования env.mem (как в функции stat ниже)
        Err(_) => {
            set_errno(env, ENOENT);
            return -1;
        }
    };

    // Защита от пустой строки (POSIX требует ENOENT)
    if path_str.is_empty() {
        log_dbg!("mkdir(empty string) -> ENOENT");
        set_errno(env, ENOENT);
        return -1;
    }

    match env.fs.create_dir(GuestPath::new(&path_str)) {
        Ok(()) => {
            log_dbg!("mkdir({:?} {:?}, {:#x}) => 0", path, path_str, mode);
            0
        }
        Err(err) => {
            // ИСПРАВЛЕНИЕ: Убираем спам в консоль через log! (превращаем в
            // log_dbg!),
            // так как приложения в iOS часто вызывают mkdir на уже существующих
            // папках просто для гарантии их наличия (ожидая поведение EEXIST).
            match err {
                FsError::AlreadyExist => {
                    log_dbg!(
                        "mkdir({:?} {:?}, {:#x}) failed with AlreadyExist, returning -1 (EEXIST)",
                        path,
                        path_str,
                        mode
                    );
                    set_errno(env, EEXIST);
                }
                FsError::NonexistentParentDir => {
                    log_dbg!("mkdir({:?} {:?}, {:#x}) failed with NonexistentParentDir, returning -1 (ENOENT)", path, path_str, mode);
                    set_errno(env, ENOENT);
                }
                FsError::ReadonlyParentDir => {
                    log_dbg!("mkdir({:?} {:?}, {:#x}) failed with ReadonlyParentDir, returning -1 (EACCES)", path, path_str, mode);
                    set_errno(env, EACCES);
                }
                _ => {
                    // ИСПРАВЛЕНИЕ: Убрана заглушка unimplemented!(), которая
                    // могла вызвать краш.
                    // Если произошла другая системная ошибка файловой системы,
                    // возвращаем ENOENT.
                    log_dbg!(
                        "mkdir({:?} {:?}, {:#x}) failed with {:?}, returning -1 (ENOENT)",
                        path,
                        path_str,
                        mode,
                        err
                    );
                    set_errno(env, ENOENT);
                }
            }
            -1
        }
    }
}

/// Helper for [stat()] and [fstat()] that fills the data in the stat struct
fn fstat_inner(env: &mut Environment, fd: FileDescriptor, buf: MutPtr<stat>) -> i32 {
    // Apple `man 2 fstat`: the standard streams (stdin/stdout/stderr) are
    // always valid file descriptors on iOS — they are connected to either a
    // terminal-style character device or, when the app is launched by
    // SpringBoard, an Apple System Log pipe.  Returning EBADF here breaks
    // managed runtimes (Mono's `System.Console..cctor()` calls
    // `MonoIO.GetFileType()` which goes through `fstat()`; an EBADF result
    // makes `FileStream..ctor(IntPtr handle, ...)` throw
    // `ArgumentException("handle", "Invalid")` and aborts the process at
    // launch — observed with Bad Piggies / Unity 3.5).  Report the streams
    // as character devices so callers see a valid handle.
    if matches!(fd, STDIN_FILENO | STDOUT_FILENO | STDERR_FILENO) {
        let mut stat = stat::default();
        stat.st_mode = S_IFCHR | 0o600;
        stat.st_nlink = 1;
        env.mem.write(buf, stat);
        return 0;
    }

    let Some(file) = env.libc_state.posix_io.file_for_fd(fd) else {
        set_errno(env, EBADF);
        return -1;
    };

    // FIXME: This implementation is highly incomplete. fstat() returns a huge
    // struct with many kinds of data in it. This code is assuming the caller
    // only wants a small part of it.
    let mut stat = stat::default();

    match file.file {
        GuestFile::File(_) | GuestFile::IpaBundleFile(_) | GuestFile::ResourceFile(_) => {
            stat.st_mode |= S_IFREG;
            // TODO: use `std::fs::metadata()` instead

            // Obtain file size
            stat.st_size = match file.file.stream_len() {
                Ok(len) => len.try_into().unwrap_or(off_t::MAX),
                Err(err) => {
                    log!(
                        "Warning: fstat({:?}): could not determine file size ({:?}); reporting 0.",
                        fd,
                        err
                    );
                    0
                }
            };
        }
        GuestFile::Directory => {
            stat.st_mode |= S_IFDIR;
            // TODO: st_size
        }
        GuestFile::Random(_) => {
            // `/dev/random` and `/dev/urandom` are character devices.
            stat.st_mode |= S_IFCHR;
        }
        _ => {
            // Socket / pipe / other non-file kinds: fstat() on a socket on
            // real iOS would return a struct with st_mode = S_IFSOCK; we
            // don't model that here yet, but the safest thing is to leave
            // st_mode = 0 and st_size = 0 rather than crashing the host.
            log!(
                "Warning: fstat({:?}): unsupported GuestFile variant; returning zeroed stat.",
                fd
            );
        }
    }

    env.mem.write(buf, stat);
    0 // success
}

fn fstat(env: &mut Environment, fd: FileDescriptor, buf: MutPtr<stat>) -> i32 {
    set_errno(env, 0);
    let result = fstat_inner(env, fd, buf);
    log_dbg!("fstat({:?}, {:?}) -> {}", fd, buf, result);
    result
}

fn stat(env: &mut Environment, path: ConstPtr<u8>, buf: MutPtr<stat>) -> i32 {
    set_errno(env, 0);
    if path.is_null() {
        set_errno(env, ENOENT);
        return -1;
    }

    // Делаем путь владеемой строкой (String), чтобы «отвязать» нас от
    // заимствования env.mem до вызова env.mem.write() в конце.
    let path_str = match env.mem.cstr_at_utf8(path) {
        Ok(s) => s.to_string(),
        Err(_) => {
            set_errno(env, ENOENT);
            return -1;
        }
    };

    let guest_path = GuestPath::new(&path_str);
    if !env.fs.exists(guest_path) {
        set_errno(env, ENOENT);
        return -1;
    }

    let mut st = stat::default();

    if env.fs.is_dir(guest_path) {
        st.st_mode = S_IFDIR | 0o755;
        st.st_nlink = 2;
    } else {
        st.st_mode = S_IFREG | 0o644;
        st.st_nlink = 1;
    }

    if let Ok(size) = env.fs.size(guest_path) {
        st.st_size = size as off_t;
        st.st_blksize = 4096;
        st.st_blocks = size.div_ceil(512) as blkcnt_t;
    }

    if let Ok(mtime) = env.fs.modified(guest_path) {
        let sec = mtime as i32;
        st.st_mtimespec = timespec {
            tv_sec: sec,
            tv_nsec: 0,
        };
        st.st_atimespec = timespec {
            tv_sec: sec,
            tv_nsec: 0,
        };
        st.st_ctimespec = timespec {
            tv_sec: sec,
            tv_nsec: 0,
        };
    }

    // env.mem свободен для записи: path_str — это String, а не ссылка.
    env.mem.write(buf, st);

    log_dbg!("stat({:?} {:?}, {:?}) -> 0", path, path_str, buf);
    0
}

fn lstat(env: &mut Environment, path: ConstPtr<u8>, buf: MutPtr<stat>) -> i32 {
    set_errno(env, 0);
    // В touchHLE GuestFS пока не поддерживает реальные симлинки,
    // поэтому lstat работает так же, как stat.
    let result = stat(env, path, buf);
    log_dbg!("lstat({:?}, {:?}) -> {}", path, buf, result);
    result
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(mkdir(_, _)),
    export_c_func!(fstat(_, _)),
    export_c_func!(stat(_, _)),
    export_c_func!(lstat(_, _)),
];
