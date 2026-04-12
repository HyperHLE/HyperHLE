/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Conditional variables.

use super::mutex::pthread_mutex_t;
use crate::dyld::FunctionExports;
use crate::libc::errno::EINVAL;
use crate::libc::pthread::mutex::pthread_mutex_unlock;
use crate::mem::{ConstPtr, MutPtr, Ptr, SafeRead};
use crate::{export_c_func, Environment};
use std::collections::{HashMap, VecDeque};

use crate::environment::{MutexId, ThreadBlock, ThreadId};

#[repr(C, packed)]
pub struct pthread_condattr_t {
    _pad: [u8; 4],  // Apple's pthread_condattr_t = 4 bytes
}
unsafe impl SafeRead for pthread_condattr_t {}

#[repr(C, packed)]
pub struct Timespec {
    pub tv_sec:  u32,
    pub tv_nsec: u32,
}
unsafe impl SafeRead for Timespec {}

/// Arbitrarily-chosen magic number for `pthread_cond_t` (not Apple's).
const MAGIC_COND: u32 = u32::from_be_bytes(*b"COND");
/// Magic number used by `PTHREAD_COND_INITIALIZER`. This is part of the ABI!
const MAGIC_COND_STATIC: u32 = 0x3CB0B1BB;

/// Apple's implementation is a 4-byte magic number followed by an 24-byte
/// opaque region. We only have to match the size theirs has.
#[repr(C, packed)]
pub struct pthread_cond_t {
    /// Magic number (must be [MAGIC_COND])
    magic: u32,
    _unused: [u32; 6],
}
unsafe impl SafeRead for pthread_cond_t {}

#[derive(Default)]
pub struct State {
    pub condition_variables: HashMap<MutPtr<pthread_cond_t>, CondHostObject>,
}
impl State {
    fn get(env: &Environment) -> &Self {
        &env.libc_state.pthread.cond
    }
    fn get_mut(env: &mut Environment) -> &mut Self {
        &mut env.libc_state.pthread.cond
    }
}

pub struct CondHostObject {
    waiting: VecDeque<ThreadId>,
    pub(crate) waking: VecDeque<ThreadId>,
    pub(crate) curr_mutex: Option<MutexId>,
}

pub fn pthread_cond_init(
    env: &mut Environment,
    cond: MutPtr<pthread_cond_t>,
    attr: ConstPtr<pthread_condattr_t>,
) -> i32 {
    // Игнорируем атрибуты, используем дефолтные значения
    // MCPE передаёт ненулевой attr, но нам он не нужен
    let _ = attr;
    
    let opaque = pthread_cond_t {
        magic: MAGIC_COND,
        _unused: [0; 6],
    };
    env.mem.write(cond, opaque);

    assert!(!State::get(env).condition_variables.contains_key(&cond));
    State::get_mut(env).condition_variables.insert(
        cond,
        CondHostObject {
            waiting: VecDeque::new(),
            waking: VecDeque::new(),
            curr_mutex: None,
        },
    );
    0 // success
}

fn check_or_register_cond(env: &mut Environment, cond: MutPtr<pthread_cond_t>) -> Result<(), i32> {
    let magic: u32 = env.mem.read(cond.cast());
    // This is a statically-initialized cond, we need to register it, and
    // change the magic number in the process.
    if magic == MAGIC_COND_STATIC {
        log_dbg!(
            "Detected statically-initialized cond at {:?}, registering.",
            cond
        );
        pthread_cond_init(env, cond, Ptr::null());
        Ok(())
    } else if magic == MAGIC_COND {
        Ok(())
    } else {
        Err(EINVAL)
    }
}

pub fn pthread_cond_wait(
    env: &mut Environment,
    cond: MutPtr<pthread_cond_t>,
    mutex: MutPtr<pthread_mutex_t>,
) -> i32 {
    if let Err(e) = check_or_register_cond(env, cond) {
        return e;
    }
    let res = pthread_mutex_unlock(env, mutex);
    assert_eq!(res, 0);
    log_dbg!(
        "Thread {} is blocking on condition variable {:?}",
        env.current_thread,
        cond
    );
    let current_thread = env.current_thread;
    let mutex_id = env.mem.read(mutex).mutex_id;
    let host_object = State::get_mut(env)
        .condition_variables
        .get_mut(&cond)
        .unwrap();
        
    // The mutex used must be the same as the currently waiting mutex, or there
    // must be no other waiters.
    assert!(
        host_object.curr_mutex == Some(mutex_id)
            || host_object.waking.is_empty() && host_object.waiting.is_empty()
    );
    host_object.curr_mutex = Some(mutex_id);
    host_object.waiting.push_back(current_thread);
    env.yield_thread(ThreadBlock::Condition(cond.cast()));
    0 // success
}

pub fn pthread_cond_signal(env: &mut Environment, cond: MutPtr<pthread_cond_t>) -> i32 {
    if let Err(e) = check_or_register_cond(env, cond) {
        return e;
    }
    let host_object = State::get_mut(env)
        .condition_variables
        .get_mut(&cond)
        .unwrap();
    if let Some(tid) = host_object.waiting.pop_front() {
        host_object.waking.push_back(tid);
        log_dbg!(
            "Thread {} unblocks one thread ({}) waiting on condition variable {:?}",
            env.current_thread,
            tid,
            cond
        );
    } else {
        log_dbg!(
            "Thread {} signals condition variable {:?}, no waiters",
            env.current_thread,
            cond
        );
    }
    0 // success
}

pub fn pthread_cond_broadcast(env: &mut Environment, cond: MutPtr<pthread_cond_t>) -> i32 {
    if let Err(e) = check_or_register_cond(env, cond) {
        return e;
    }
    log_dbg!(
        "Thread {} unblocks one thread waiting on condition variable {:?}",
        env.current_thread,
        cond
    );
    let host_object = State::get_mut(env)
        .condition_variables
        .get_mut(&cond)
        .unwrap();
    host_object.waking.extend(host_object.waiting.drain(..));
    0 // success
}

pub fn pthread_cond_destroy(env: &mut Environment, cond: MutPtr<pthread_cond_t>) -> i32 {
    if let Err(e) = check_or_register_cond(env, cond) {
        return e;
    }
    let old_object = State::get_mut(env)
        .condition_variables
        .remove(&cond)
        .unwrap();
    assert!(old_object.waiting.is_empty() && old_object.waking.is_empty());
    0 // success
}

/// pthread_cond_timedwait — like pthread_cond_wait but with an absolute timeout.
/// We ignore the timeout and treat it as a regular wait, which is safe for
/// game use-cases (the thread will still be woken by signal/broadcast).
pub fn pthread_cond_timedwait(
    env: &mut Environment,
    cond: MutPtr<pthread_cond_t>,
    mutex: MutPtr<pthread_mutex_t>,
    abstime: ConstPtr<Timespec>,
) -> i32 {
    // Log the timeout for debugging but otherwise behave like cond_wait.
    if !abstime.is_null() {
        let ts = env.mem.read(abstime);
        let sec = ts.tv_sec;
        let nsec = ts.tv_nsec;
        log_dbg!(
            "pthread_cond_timedwait: timeout at tv_sec={} tv_nsec={} (ignored, treating as wait)",
            sec, nsec
        );
    }
    pthread_cond_wait(env, cond, mutex)
}

/// pthread_cond_timedwait_relative_np — Apple extension with a relative timeout.
/// Same stub approach as timedwait.
pub fn pthread_cond_timedwait_relative_np(
    env: &mut Environment,
    cond: MutPtr<pthread_cond_t>,
    mutex: MutPtr<pthread_mutex_t>,
    reltime: ConstPtr<Timespec>,
) -> i32 {
    if !reltime.is_null() {
        let ts = env.mem.read(reltime);
        let sec = ts.tv_sec;
        let nsec = ts.tv_nsec;
        log_dbg!(
            "pthread_cond_timedwait_relative_np: relative timeout tv_sec={} tv_nsec={} (ignored)",
            sec, nsec
        );
    }
    pthread_cond_wait(env, cond, mutex)
}

/// pthread_condattr_init — initialise a cond attr object (always default).
pub fn pthread_condattr_init(
    env: &mut Environment,
    attr: MutPtr<pthread_condattr_t>,
) -> i32 {
    if !attr.is_null() {
        env.mem.write(attr, pthread_condattr_t { _pad: [0; 4] });
    }
    0
}

/// pthread_condattr_destroy — destroy a cond attr object (no-op).
pub fn pthread_condattr_destroy(
    _env: &mut Environment,
    _attr: MutPtr<pthread_condattr_t>,
) -> i32 {
    0
}

/// pthread_condattr_setpshared — set process-shared attribute (stub).
pub fn pthread_condattr_setpshared(
    _env: &mut Environment,
    _attr: MutPtr<pthread_condattr_t>,
    _pshared: i32,
) -> i32 {
    0
}

/// pthread_condattr_getpshared — get process-shared attribute (always private).
pub fn pthread_condattr_getpshared(
    env: &mut Environment,
    _attr: ConstPtr<pthread_condattr_t>,
    pshared: MutPtr<i32>,
) -> i32 {
    if !pshared.is_null() {
        env.mem.write(pshared, 0); // PTHREAD_PROCESS_PRIVATE
    }
    0
}

/// pthread_condattr_setclock — set clock attribute (stub, always CLOCK_REALTIME).
pub fn pthread_condattr_setclock(
    _env: &mut Environment,
    _attr: MutPtr<pthread_condattr_t>,
    _clock_id: i32,
) -> i32 {
    0
}

/// pthread_condattr_getclock — get clock attribute (always CLOCK_REALTIME = 0).
pub fn pthread_condattr_getclock(
    env: &mut Environment,
    _attr: ConstPtr<pthread_condattr_t>,
    clock_id: MutPtr<i32>,
) -> i32 {
    if !clock_id.is_null() {
        env.mem.write(clock_id, 0); // CLOCK_REALTIME
    }
    0
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(pthread_cond_init(_, _)),
    export_c_func!(pthread_cond_wait(_, _)),
    export_c_func!(pthread_cond_timedwait(_, _, _)),
    export_c_func!(pthread_cond_timedwait_relative_np(_, _, _)),
    export_c_func!(pthread_cond_signal(_)),
    export_c_func!(pthread_cond_broadcast(_)),
    export_c_func!(pthread_cond_destroy(_)),
    export_c_func!(pthread_condattr_init(_)),
    export_c_func!(pthread_condattr_destroy(_)),
    export_c_func!(pthread_condattr_setpshared(_, _)),
    export_c_func!(pthread_condattr_getpshared(_, _)),
    export_c_func!(pthread_condattr_setclock(_, _)),
    export_c_func!(pthread_condattr_getclock(_, _)),
];

