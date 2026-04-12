/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Threads.

use crate::abi::GuestFunction;
use crate::dyld::{export_c_func, FunctionExports};
use crate::libc::errno::{EDEADLK, EINVAL, ESRCH};
use crate::mem::{
    self, ConstPtr, ConstVoidPtr, GuestUSize, MutPtr, MutVoidPtr, Ptr, SafeRead, PAGE_SIZE,
};
use crate::{Environment, ThreadId};
use std::collections::HashMap;

#[derive(Default)]
pub struct State {
    threads: HashMap<pthread_t, ThreadHostObject>,
    main_thread_object_created: bool,
}
impl State {
    fn get(env: &mut Environment) -> &mut Self {
        &mut env.libc_state.pthread.thread
    }
}

/// Apple's implementation is a 4-byte magic number followed by an 36-byte
/// opaque region. We only have to match the size theirs has.
#[derive(Copy, Clone, Debug)]
#[repr(C, packed)]
pub struct pthread_attr_t {
    /// Magic number (must be [MAGIC_ATTR])
    magic: u32,
    detachstate: i32,
    stacksize: GuestUSize,
    _unused: [u32; 7],
}
unsafe impl SafeRead for pthread_attr_t {}

#[derive(Copy, Clone, Debug)]
#[repr(C, packed)]
struct sched_param {
    sched_priority: i32,
}
unsafe impl SafeRead for sched_param {}

const DEFAULT_ATTR: pthread_attr_t = pthread_attr_t {
    magic: MAGIC_ATTR,
    detachstate: PTHREAD_CREATE_JOINABLE,
    stacksize: mem::Mem::SECONDARY_THREAD_DEFAULT_STACK_SIZE,
    _unused: [0; 7],
};

/// Apple's implementation is a 4-byte magic number followed by a massive
/// (>4KiB) opaque region. We will store the actual data on the host instead.
#[repr(C, packed)]
pub struct OpaqueThread {
    /// Magic number (must be [MAGIC_THREAD])
    magic: u32,
}
unsafe impl SafeRead for OpaqueThread {}

#[allow(non_camel_case_types)]
pub type pthread_t = MutPtr<OpaqueThread>;

// Cancellation state constants
const PTHREAD_CANCEL_ENABLE:       i32 = 0;
const PTHREAD_CANCEL_DISABLE:      i32 = 1;

// Cancellation type constants
const PTHREAD_CANCEL_DEFERRED:     i32 = 0;
const PTHREAD_CANCEL_ASYNCHRONOUS: i32 = 1;

/// Sentinel return value for a cancelled thread (POSIX specifies this as
/// a special non-NULL pointer value distinct from any valid pointer).
pub const PTHREAD_CANCELED: MutVoidPtr = Ptr::from_bits(u32::MAX);

struct ThreadHostObject {
    thread_id: ThreadId,
    joined_by: Option<ThreadId>,
    attr: pthread_attr_t,
    /// Set by pthread_cancel — thread has been asked to terminate.
    cancel_requested: bool,
    /// Set by pthread_setcancelstate(PTHREAD_CANCEL_DISABLE).
    cancel_disabled: bool,
    /// Set by pthread_setcanceltype(PTHREAD_CANCEL_ASYNCHRONOUS).
    cancel_async: bool,
}

impl ThreadHostObject {
    fn new(thread_id: ThreadId, attr: pthread_attr_t) -> Self {
        ThreadHostObject {
            thread_id,
            joined_by: None,
            attr,
            cancel_requested: false,
            cancel_disabled: false,
            cancel_async: false,
        }
    }
}

/// Arbitrarily-chosen magic number for `pthread_attr_t` (not Apple's).
const MAGIC_ATTR: u32 = u32::from_be_bytes(*b"ThAt");
/// Arbitrarily-chosen magic number for `pthread_t` (not Apple's).
const MAGIC_THREAD: u32 = u32::from_be_bytes(*b"THRD");

/// Custom typedef for readability (the C API just uses `int`)
type DetachState = i32;
const PTHREAD_CREATE_JOINABLE: DetachState = 1;
pub const PTHREAD_CREATE_DETACHED: DetachState = 2;

/// Value taken from an iOS 2.0 simulator
const PTHREAD_STACK_MIN: GuestUSize = 2 * PAGE_SIZE;

// =========================================================================
// MARK: - Attribute functions
// =========================================================================

pub fn pthread_attr_init(env: &mut Environment, attr: MutPtr<pthread_attr_t>) -> i32 {
    env.mem.write(attr, DEFAULT_ATTR);
    0
}

fn pthread_attr_getdetachstate(
    env: &mut Environment,
    attr: MutPtr<pthread_attr_t>,
    detachstate_ptr: MutPtr<DetachState>,
) -> i32 {
    check_magic!(env, attr, MAGIC_ATTR);
    let detachstate = env.mem.read(attr).detachstate;
    assert!(detachstate == PTHREAD_CREATE_JOINABLE || detachstate == PTHREAD_CREATE_DETACHED);
    env.mem.write(detachstate_ptr, detachstate);
    0
}

pub fn pthread_attr_setdetachstate(
    env: &mut Environment,
    attr: MutPtr<pthread_attr_t>,
    detachstate: DetachState,
) -> i32 {
    check_magic!(env, attr, MAGIC_ATTR);
    assert!(detachstate == PTHREAD_CREATE_JOINABLE || detachstate == PTHREAD_CREATE_DETACHED);
    let mut attr_copy = env.mem.read(attr);
    attr_copy.detachstate = detachstate;
    env.mem.write(attr, attr_copy);
    0
}

fn pthread_attr_getstacksize(
    env: &mut Environment,
    attr: MutPtr<pthread_attr_t>,
    stacksize: MutPtr<GuestUSize>,
) -> i32 {
    if attr.is_null() {
        return EINVAL;
    }
    check_magic!(env, attr, MAGIC_THREAD);
    let size = env.mem.read(attr).stacksize;
    env.mem.write(stacksize, size);
    0
}

pub fn pthread_attr_setstacksize(
    env: &mut Environment,
    attr: MutPtr<pthread_attr_t>,
    stacksize: GuestUSize,
) -> i32 {
    if attr.is_null() ||
        stacksize < PTHREAD_STACK_MIN || !stacksize.is_multiple_of(PAGE_SIZE) {
        return EINVAL;
    }
    check_magic!(env, attr, MAGIC_ATTR);
    let mut attr_copy = env.mem.read(attr);
    attr_copy.stacksize = stacksize;
    env.mem.write(attr, attr_copy);
    0
}

fn pthread_attr_setinheritsched(
    env: &mut Environment,
    attr: MutPtr<pthread_attr_t>,
    inheritsched: i32,
) -> i32 {
    check_magic!(env, attr, MAGIC_ATTR);
    log!("TODO: pthread_attr_setinheritsched({:?}, {})", attr, inheritsched);
    0
}

fn pthread_attr_setschedpolicy(
    env: &mut Environment,
    attr: MutPtr<pthread_attr_t>,
    policy: i32,
) -> i32 {
    check_magic!(env, attr, MAGIC_ATTR);
    log!("TODO: pthread_attr_setschedpolicy({:?}, {}) (ignored)", attr, policy);
    0
}

fn pthread_attr_setschedparam(
    env: &mut Environment,
    attr: MutPtr<pthread_attr_t>,
    param: ConstPtr<sched_param>,
) -> i32 {
    check_magic!(env, attr, MAGIC_ATTR);
    let sched_param = env.mem.read(param);
    log!("TODO: pthread_attr_setschedparam({:?}, {:?}) (ignored)", attr, sched_param);
    0
}

fn pthread_attr_destroy(env: &mut Environment, attr: MutPtr<pthread_attr_t>) -> i32 {
    check_magic!(env, attr, MAGIC_ATTR);
    env.mem.write(attr, pthread_attr_t {
        magic: 0,
        detachstate: 0,
        stacksize: 0,
        _unused: Default::default(),
    });
    0
}

// =========================================================================
// MARK: - Thread lifecycle
// =========================================================================

pub fn pthread_create(
    env: &mut Environment,
    thread: MutPtr<pthread_t>,
    attr: ConstPtr<pthread_attr_t>,
    start_routine: GuestFunction,
    user_data: MutVoidPtr,
) -> i32 {
    let attr = if !attr.is_null() {
        check_magic!(env, attr, MAGIC_ATTR);
        env.mem.read(attr)
    } else {
        DEFAULT_ATTR
    };
    let thread_id = env.new_thread(start_routine, user_data, attr.stacksize);

    let opaque = env.mem.alloc_and_write(OpaqueThread { magic: MAGIC_THREAD });
    env.mem.write(thread, opaque);

    assert!(!State::get(env).threads.contains_key(&opaque));
    State::get(env).threads.insert(opaque, ThreadHostObject::new(thread_id, attr));
    log_dbg!(
        "pthread_create({:?}, {:?}, {:?}, {:?}) => 0, pthread_t={:?} thread_id={}",
        thread, attr, start_routine, user_data, opaque, thread_id
    );
    0
}

fn pthread_equal(env: &mut Environment, thread1: pthread_t, thread2: pthread_t) -> i32 {
    if State::get(env).threads.get(&thread1).unwrap().thread_id
        == State::get(env).threads.get(&thread2).unwrap().thread_id
    {
        1
    } else {
        0
    }
}

pub fn pthread_self(env: &mut Environment) -> pthread_t {
    let current_thread = env.current_thread;
    if current_thread == 0 && !State::get(env).main_thread_object_created {
        State::get(env).main_thread_object_created = true;
        let opaque = env.mem.alloc_and_write(OpaqueThread { magic: MAGIC_THREAD });
        assert!(!State::get(env).threads.contains_key(&opaque));
        State::get(env).threads.insert(opaque, ThreadHostObject::new(0, DEFAULT_ATTR));
        log_dbg!("pthread_self: created pthread object {:?} for main thread", opaque);
    }

    let (&ptr, _) = State::get(env)
        .threads
        .iter()
        .find(|&(_ptr, host_obj)| host_obj.thread_id == current_thread)
        .unwrap();
    ptr
}

// ИЗМЕНЕНО: Возвращаемый тип `-> !` удален, чтобы макрос не выдавал ошибку.
pub fn pthread_exit(_env: &mut Environment, retval: MutVoidPtr) {
    log_dbg!("pthread_exit({:?})", retval);
    panic!("pthread_exit({:?}): thread exit not yet implemented", retval);
}

fn pthread_join(env: &mut Environment, thread: pthread_t, retval: MutPtr<MutVoidPtr>) -> i32 {
    let current_thread = env.current_thread;
    let curr_pthread_t = pthread_self(env);
    let joinee_thread = State::get(env).threads.get_mut(&thread).unwrap().thread_id;

    assert!(joinee_thread != 0);
    if joinee_thread == current_thread {
        log_dbg!("Thread attempted join with self, returning EDEADLK!");
        return EDEADLK;
    }

    let host_obj_curr = State::get(env).threads.get(&curr_pthread_t).unwrap();
    if let Some(thread) = host_obj_curr.joined_by {
        if thread == joinee_thread {
            log_dbg!("Thread attempted deadlocking join, returning EDEADLK!");
            return EDEADLK;
        }
    }

    let host_obj_joinee = State::get(env).threads.get_mut(&thread).unwrap();
    if host_obj_joinee.attr.detachstate == PTHREAD_CREATE_DETACHED {
        log_dbg!("Thread attempted join with detached thread, returning EINVAL!");
        return EINVAL;
    }

    host_obj_joinee.joined_by = Some(current_thread);
    env.join_with_thread(joinee_thread, retval);
    0
}

fn pthread_detach(env: &mut Environment, thread: pthread_t) -> i32 {
    let Some(host_obj) = State::get(env).threads.get_mut(&thread) else {
        log_dbg!("pthread_detach: thread doesn't exist, returning ESRCH");
        return ESRCH;
    };
    if host_obj.attr.detachstate == PTHREAD_CREATE_DETACHED {
        log_dbg!("pthread_detach: thread already detached, returning EINVAL");
        return EINVAL;
    }
    log!("TODO: pthread_detach({:?})", thread);
    0
}

// =========================================================================
// MARK: - Cancellation
// =========================================================================

fn pthread_cancel(env: &mut Environment, thread: pthread_t) -> i32 {
    let Some(host_obj) = State::get(env).threads.get_mut(&thread) else {
        log_dbg!("pthread_cancel: unknown thread {:?}, returning ESRCH", thread);
        return ESRCH;
    };
    if host_obj.cancel_requested {
        // Already pending — POSIX still considers this success.
        return 0;
    }
    host_obj.cancel_requested = true;
    log_dbg!("pthread_cancel({:?}): cancel pending on thread_id={}", thread, host_obj.thread_id);
    0
}

fn pthread_setcancelstate(env: &mut Environment, state: i32, oldstate: MutPtr<i32>) -> i32 {
    if state != PTHREAD_CANCEL_ENABLE && state != PTHREAD_CANCEL_DISABLE {
        return EINVAL;
    }
    let self_t = pthread_self(env);
    
    let prev = {
        let host_obj = State::get(env).threads.get_mut(&self_t).unwrap();
        if host_obj.cancel_disabled {
            PTHREAD_CANCEL_DISABLE
        } else {
            PTHREAD_CANCEL_ENABLE
        }
    };
    
    if !oldstate.is_null() {
        env.mem.write(oldstate, prev);
    }
    
    let host_obj = State::get(env).threads.get_mut(&self_t).unwrap();
    host_obj.cancel_disabled = state == PTHREAD_CANCEL_DISABLE;
    log_dbg!("pthread_setcancelstate({})", state);
    0
}

fn pthread_setcanceltype(env: &mut Environment, cancel_type: i32, oldtype: MutPtr<i32>) -> i32 {
    if cancel_type != PTHREAD_CANCEL_DEFERRED && cancel_type != PTHREAD_CANCEL_ASYNCHRONOUS {
        return EINVAL;
    }
    let self_t = pthread_self(env);
    
    let prev = {
        let host_obj = State::get(env).threads.get_mut(&self_t).unwrap();
        if host_obj.cancel_async {
            PTHREAD_CANCEL_ASYNCHRONOUS
        } else {
            PTHREAD_CANCEL_DEFERRED
        }
    };
    
    if !oldtype.is_null() {
        env.mem.write(oldtype, prev);
    }
    
    let host_obj = State::get(env).threads.get_mut(&self_t).unwrap();
    host_obj.cancel_async = cancel_type == PTHREAD_CANCEL_ASYNCHRONOUS;
    if cancel_type == PTHREAD_CANCEL_ASYNCHRONOUS {
        log!(
            "Warning: pthread_setcanceltype(PTHREAD_CANCEL_ASYNCHRONOUS) — \
             async cancellation not fully supported; treating as deferred"
        );
    }
    log_dbg!("pthread_setcanceltype({})", cancel_type);
    0
}

fn pthread_testcancel(env: &mut Environment) {
    let self_t = pthread_self(env);
    let host_obj = State::get(env).threads.get(&self_t).unwrap();
    if host_obj.cancel_disabled || !host_obj.cancel_requested {
        return;
    }
    log_dbg!("pthread_testcancel: cancelling current thread");
    pthread_exit(env, PTHREAD_CANCELED);
}

// =========================================================================
// MARK: - Darwin extensions
// =========================================================================

#[allow(non_camel_case_types)]
type mach_port_t = u32;
fn pthread_mach_thread_np(env: &mut Environment, thread: pthread_t) -> mach_port_t {
    let host_object = State::get(env).threads.get(&thread).unwrap();
    (host_object.thread_id + 1).try_into().unwrap()
}

fn pthread_get_stackaddr_np(env: &mut Environment, thread: pthread_t) -> MutVoidPtr {
    let thread_id = State::get(env).threads.get(&thread).unwrap().thread_id;
    Ptr::from_bits(*env.threads[thread_id].stack.as_ref().unwrap().end())
}

fn pthread_get_stacksize_np(env: &mut Environment, thread: pthread_t) -> GuestUSize {
    let thread_id = State::get(env).threads.get(&thread).unwrap().thread_id;
    let start_unadjusted = env.threads[thread_id].stack.as_ref().unwrap().start();
    let start = if thread_id == 0 {
        *start_unadjusted + PAGE_SIZE
    } else {
        *start_unadjusted
    };
    let end = env.threads[thread_id].stack.as_ref().unwrap().end();
    end.wrapping_add(1).wrapping_sub(start)
}

fn pthread_getschedparam(
    _env: &mut Environment,
    thread: pthread_t,
    policy: i32,
    param: MutVoidPtr,
) -> i32 {
    log_dbg!("TODO: pthread_getschedparam({:?}, {}, {:?})", thread, policy, param);
    0
}

fn pthread_setschedparam(
    _env: &mut Environment,
    thread: pthread_t,
    policy: i32,
    param: ConstVoidPtr,
) -> i32 {
    log_dbg!("TODO: pthread_setschedparam({:?}, {}, {:?})", thread, policy, param);
    0
}

pub const FUNCTIONS: FunctionExports = &[
    // Attributes
    export_c_func!(pthread_attr_init(_)),
    export_c_func!(pthread_attr_getdetachstate(_, _)),
    export_c_func!(pthread_attr_setdetachstate(_, _)),
    export_c_func!(pthread_attr_getstacksize(_, _)),
    export_c_func!(pthread_attr_setstacksize(_, _)),
    export_c_func!(pthread_attr_setinheritsched(_, _)),
    export_c_func!(pthread_attr_setschedpolicy(_, _)),
    export_c_func!(pthread_attr_setschedparam(_, _)),
    export_c_func!(pthread_attr_destroy(_)),
    // Lifecycle
    export_c_func!(pthread_create(_, _, _, _)),
    export_c_func!(pthread_equal(_, _)),
    export_c_func!(pthread_self()),
    export_c_func!(pthread_exit(_)),
    export_c_func!(pthread_join(_, _)),
    export_c_func!(pthread_detach(_)),
    // Cancellation
    export_c_func!(pthread_cancel(_)),
    export_c_func!(pthread_setcancelstate(_, _)),
    export_c_func!(pthread_setcanceltype(_, _)),
    export_c_func!(pthread_testcancel()),
    // Darwin extensions
    export_c_func!(pthread_mach_thread_np(_)),
    export_c_func!(pthread_get_stackaddr_np(_)),
    export_c_func!(pthread_get_stacksize_np(_)),
    export_c_func!(pthread_getschedparam(_, _, _)),
    export_c_func!(pthread_setschedparam(_, _, _)),
];

