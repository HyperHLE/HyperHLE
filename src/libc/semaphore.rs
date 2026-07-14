/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `semaphore.h`

use crate::dyld::{export_c_func, FunctionExports};
use crate::libc::errno::set_errno;
use crate::libc::posix_io::stat::mode_t;
use crate::libc::posix_io::{O_CREAT, O_EXCL};
use crate::mem::{ConstPtr, MutPtr};
use crate::{Environment, ThreadId};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use super::errno::{EAGAIN, EEXIST, EINVAL, ENOENT};

// SEM_FAILED is defined as -1 while having a type of sem_t *
pub const SEM_FAILED: MutPtr<sem_t> = MutPtr::from_bits(u32::MAX);

#[derive(Default)]
pub struct State {
    named_semaphores: HashMap<String, Rc<RefCell<SemaphoreHostObject>>>,
    pub open_semaphores: HashMap<MutPtr<sem_t>, Rc<RefCell<SemaphoreHostObject>>>,
}
impl State {
    fn get(env: &Environment) -> &Self {
        &env.libc_state.semaphore
    }
    fn get_mut(env: &mut Environment) -> &mut Self {
        &mut env.libc_state.semaphore
    }
}

#[allow(non_camel_case_types)]
pub type sem_t = i32;

pub struct SemaphoreHostObject {
    pub value: i32,
    pub waiting: HashSet<ThreadId>,
    guest_sem: Option<MutPtr<sem_t>>,
    named: bool,
}

pub fn sem_init(env: &mut Environment, sem: MutPtr<sem_t>, pshared: i32, value: u32) -> i32 {
    // TODO: handle errno properly
    set_errno(env, 0);

    assert!(pshared == 0);

    let state = State::get_mut(env);
    if state.open_semaphores.contains_key(&sem) {
        return 0;
    }
    let host_sem_rc = Rc::new(RefCell::new(SemaphoreHostObject {
        value: value as i32,
        waiting: HashSet::new(),
        guest_sem: Some(sem),
        named: false,
    }));

    state.open_semaphores.insert(sem, host_sem_rc);
    0
}

pub fn sem_destroy(env: &mut Environment, sem: MutPtr<sem_t>) -> i32 {
    let state = State::get_mut(env);
    let sem = state.open_semaphores.remove(&sem);
    if let Some(sem) = sem {
        assert!(!sem.borrow().named);
        // Don't free, it's not our resposibility to.
        0
    } else {
        // No semaphores at that pointer.
        EINVAL
    }
}

pub fn sem_open(
    env: &mut Environment,
    name: ConstPtr<u8>,
    oflag: i32,
    _mode: mode_t,
    value: u32,
) -> MutPtr<sem_t> {
    // TODO: handle errno properly
    set_errno(env, 0);

    let sem_name_str = env.mem.cstr_at_utf8(name).unwrap().to_string();

    // Look up any existing named semaphore, cloning the `Rc` so we stop
    // borrowing `env` and can mutate it (e.g. set errno) below.
    let existing = State::get(env).named_semaphores.get(&sem_name_str).cloned();

    let host_sem_rc = if let Some(existing_host_sem_rc) = existing {
        // The named semaphore already exists. Per Apple/POSIX `sem_open(2)`:
        //   * `O_CREAT | O_EXCL` on an existing semaphore fails with `EEXIST`.
        //   * Otherwise repeated `sem_open()` calls with the same name return
        //     the same descriptor.
        // The previous logic was inverted: it returned `SEM_FAILED` whenever
        // `O_EXCL` was *not* set, so any app that opened the same named
        // semaphore twice (e.g. from multiple threads) got `SEM_FAILED`
        // (0xffffffff) and then spun forever calling `sem_wait`/`sem_post` on
        // the bogus handle.
        // Reference: https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/sem_open.2.html
        if (oflag & O_EXCL) != 0 {
            set_errno(env, EEXIST);
            return SEM_FAILED;
        }
        if let Some(existing_sem) = existing_host_sem_rc.borrow().guest_sem {
            return existing_sem;
        }
        existing_host_sem_rc
    } else {
        if (oflag & O_CREAT) == 0 {
            // `O_CREAT` not set and the named semaphore does not exist: `ENOENT`.
            set_errno(env, ENOENT);
            return SEM_FAILED;
        }
        let host_sem_rc = Rc::new(RefCell::new(SemaphoreHostObject {
            value: value as i32,
            waiting: HashSet::new(),
            guest_sem: None,
            named: true,
        }));
        State::get_mut(env)
            .named_semaphores
            .insert(sem_name_str, Rc::clone(&host_sem_rc));
        host_sem_rc
    };

    let sem = env.mem.alloc_and_write(0);
    (*host_sem_rc).borrow_mut().guest_sem = Some(sem);
    State::get_mut(env).open_semaphores.insert(sem, host_sem_rc);

    sem
}

pub fn sem_post(env: &mut Environment, sem: MutPtr<sem_t>) -> i32 {
    set_errno(env, 0);

    // Per POSIX/Apple `sem_post(3)`: on success returns 0, and if `sem` does
    // not refer to a valid semaphore it fails with -1 and `errno == EINVAL`.
    // We must not abort the whole emulator when a guest posts a stale or
    // uninitialised semaphore pointer.
    if env.sem_increment(sem) {
        0 // success
    } else {
        set_errno(env, EINVAL);
        -1
    }
}

pub fn sem_wait(env: &mut Environment, sem: MutPtr<sem_t>) -> i32 {
    set_errno(env, 0);

    // Per POSIX/Apple `sem_wait(3)`: returns 0 once the semaphore is locked, or
    // -1 with `errno == EINVAL` if `sem` is not a valid semaphore. `sem_wait`
    // blocks, so `sem_decrement(_, true)` only returns `false` when the
    // semaphore is unknown; treat that as the documented EINVAL failure.
    if env.sem_decrement(sem, true) {
        0 // success
    } else {
        set_errno(env, EINVAL);
        -1
    }
}

fn sem_trywait(env: &mut Environment, sem: MutPtr<sem_t>) -> i32 {
    set_errno(env, 0);

    // `sem_trywait(3)` never blocks. It returns 0 on success, or -1 with
    // `errno == EAGAIN` when the semaphore is already at zero, or
    // `errno == EINVAL` when `sem` is not a valid semaphore. We distinguish the
    // two failure modes by checking whether the semaphore is tracked at all.
    if env.sem_decrement(sem, false) {
        0 // success
    } else if env
        .libc_state
        .semaphore
        .open_semaphores
        .contains_key(&sem)
    {
        set_errno(env, EAGAIN);
        -1
    } else {
        set_errno(env, EINVAL);
        -1
    }
}

pub fn sem_close(env: &mut Environment, sem: MutPtr<sem_t>) -> i32 {
    set_errno(env, 0);

    // Per POSIX/Apple `sem_close(3)`: fails with -1 and `errno == EINVAL` when
    // `sem` is not a valid (open, named) semaphore. Don't panic if the guest
    // passes a bad or already-closed handle.
    let Some(host_sem_rc) = env.libc_state.semaphore.open_semaphores.remove(&sem) else {
        log!(
            "Warning: sem_close called on unknown semaphore {:?}; returning EINVAL.",
            sem
        );
        set_errno(env, EINVAL);
        return -1;
    };
    let mut host_sem = (*host_sem_rc).borrow_mut();
    if !host_sem.named {
        // sem_close is only valid for named semaphores (sem_open). Put the
        // entry back so an unnamed semaphore isn't silently dropped, and report
        // the documented EINVAL failure.
        log!(
            "Warning: sem_close called on unnamed semaphore {:?}; returning EINVAL.",
            sem
        );
        std::mem::drop(host_sem);
        env.libc_state.semaphore.open_semaphores.insert(sem, host_sem_rc);
        set_errno(env, EINVAL);
        return -1;
    }
    if let Some(guest_sem) = host_sem.guest_sem {
        env.mem.free(guest_sem.cast());
    }
    host_sem.guest_sem = None;
    0 // success
}

pub fn sem_unlink(env: &mut Environment, name: ConstPtr<u8>) -> i32 {
    // TODO: handle errno properly
    set_errno(env, 0);

    let sem_name = env.mem.cstr_at_utf8(name).unwrap();
    env.libc_state.semaphore.named_semaphores.remove(sem_name);
    0 // success
}

/// `int sem_getvalue(sem_t *sem, int *sval)`
///
/// Writes the current value of the semaphore into `*sval` and returns 0. Per
/// POSIX/Apple, if `sem` is not a valid semaphore it fails with -1 and
/// `errno == EINVAL`. When there are threads blocked waiting on the semaphore,
/// the standard allows reporting either 0 or a negative number whose magnitude
/// is the number of waiters; we report the negative count, which is what Apple
/// platforms and glibc do.
fn sem_getvalue(env: &mut Environment, sem: MutPtr<sem_t>, sval: MutPtr<i32>) -> i32 {
    set_errno(env, 0);

    let Some(host_sem_rc) = env.libc_state.semaphore.open_semaphores.get(&sem) else {
        log!(
            "Warning: sem_getvalue called on unknown semaphore {:?}; returning EINVAL.",
            sem
        );
        set_errno(env, EINVAL);
        return -1;
    };
    let host_sem = (*host_sem_rc).borrow();
    let reported = if host_sem.value > 0 {
        host_sem.value
    } else {
        -(host_sem.waiting.len() as i32)
    };
    if !sval.is_null() {
        env.mem.write(sval, reported);
    }
    0 // success
}

/// Shortcut for host code to make an unnamed semaphore. Destroy with
/// host_destroy_semaphore, not sem_destroy
pub fn host_create_semaphore(env: &mut Environment, value: u32) -> MutPtr<sem_t> {
    let sem: MutPtr<sem_t> = env.mem.alloc_and_write(0);
    sem_init(env, sem, 0, value);
    sem
}
pub fn host_destroy_semaphore(env: &mut Environment, sem: MutPtr<sem_t>) {
    sem_destroy(env, sem);
    env.mem.free(sem.cast());
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(sem_init(_, _, _)),
    export_c_func!(sem_destroy(_)),
    export_c_func!(sem_open(_, _, _, _)),
    export_c_func!(sem_post(_)),
    export_c_func!(sem_wait(_)),
    export_c_func!(sem_trywait(_)),
    export_c_func!(sem_getvalue(_, _)),
    export_c_func!(sem_close(_)),
    export_c_func!(sem_unlink(_)),
];
