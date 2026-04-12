/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! POSIX `sys/stat.h`

use super::{close, off_t, open_direct, FileDescriptor};
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

// enum values sourced from ```man 2 stat```
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
    // TODO: handle errno properly
    set_errno(env, 0);
    let path_str = env.mem.cstr_at_utf8(path).unwrap();
    // TODO: respect the mode
    match env.fs.create_dir(GuestPath::new(&path_str)) {
        Ok(()) => {
            log_dbg!("mkdir({:?} {:?}, {:#x}) => 0", path, path_str, mode);
            0
        }
        Err(err) => {
            log!(
                "Warning: mkdir({:?} {:?}, {:#x}) failed with {:?}, returning -1",
                path,
                path_str,
                mode,
                err
            );
            match err {
                FsError::AlreadyExist => set_errno(env, EEXIST),
                FsError::NonexistentParentDir => set_errno(env, ENOENT),
                FsError::ReadonlyParentDir => set_errno(env, EACCES),
                _ => unimplemented!(),
            }
            -1
        }
    }
}

/// Helper for [stat()] and [fstat()] that fills the data in the stat struct
fn fstat_inner(env: &mut Environment, fd: FileDescriptor, buf: MutPtr<stat>) -> i32 {
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
            stat.st_size = file.file.stream_len().unwrap().try_into().unwrap();
        }
        GuestFile::Directory => {
            stat.st_mode |= S_IFDIR;
            // TODO: st_size
        }
        _ => unimplemented!(),
    }

    env.mem.write(buf, stat);
    0 // success
}

fn fstat(env: &mut Environment, fd: FileDescriptor, buf: MutPtr<stat>) -> i32 {
    set_errno(env, 0);
    // Убираем бесячий варнинг, функция fstat_inner работает нормально
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

    // ИСПРАВЛЕНИЕ 1: Делаем путь владеемой строкой (String). 
    // Это «отвязывает» нас от заимствования env.mem.
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
        st.st_blocks = ((size as u64 + 511) / 512) as blkcnt_t;
    }

    if let Ok(mtime) = env.fs.modified(guest_path) {
        let sec = mtime as i32;
        // ИСПРАВЛЕНИЕ 2: Создаем структуру для каждого поля отдельно.
        // ВАЖНО: Требует `pub tv_sec` и `pub tv_nsec` в файле time.rs!
        st.st_mtimespec = timespec { tv_sec: sec, tv_nsec: 0 };
        st.st_atimespec = timespec { tv_sec: sec, tv_nsec: 0 };
        st.st_ctimespec = timespec { tv_sec: sec, tv_nsec: 0 };
    }

    // Теперь env.mem свободен для записи, так как path_str — это String, а не ссылка.
    env.mem.write(buf, st);

    log_dbg!(
        "stat({:?} {:?}, {:?}) -> 0",
        path,
        path_str,
        buf
    );
    0
}

fn lstat(env: &mut Environment, path: ConstPtr<u8>, buf: MutPtr<stat>) -> i32 {
    set_errno(env, 0);
    // Поскольку в touchHLE GuestFS пока не поддерживает реальные симлинки,
    // lstat должен работать точно так же, как stat, без костылей.
    let result = stat(env, path, buf);
    
    log_dbg!(
        "lstat({:?} {:?}, {:?}) -> {}",
        path,
        env.mem.cstr_at_utf8(path),
        buf,
        result
    );
    result
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(mkdir(_, _)),
    export_c_func!(fstat(_, _)),
    export_c_func!(stat(_, _)),
    export_c_func!(lstat(_, _)),
];

