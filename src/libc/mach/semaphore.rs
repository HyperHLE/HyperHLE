/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `mach/semaphore.h`
//!
//! Implemented as a wrapper around libc semaphore

#![allow(non_camel_case_types)]

use std::time::Duration;

use crate::dyld::FunctionExports;
use crate::environment::Environment;
use crate::export_c_func;
use crate::libc::mach::arm::task::task_t;
use crate::libc::mach::init::MACH_TASK_SELF;
use crate::libc::mach::thread_info::{kern_return_t, KERN_INVALID_ARGUMENT, KERN_SUCCESS};
use crate::libc::semaphore::{sem_destroy, sem_init, sem_post, sem_t, sem_wait};
use crate::mem::MutPtr;

// Opaque type. Reusing sem_t for convenience
// TODO: `semaphore_t` should be `mach_port_t`
type semaphore = sem_t;
type semaphore_t = MutPtr<semaphore>;

/// `KERN_OPERATION_TIMED_OUT` from `mach/kern_return.h`: what the timed waits
/// below return when the timeout expires before the semaphore is acquired.
const KERN_OPERATION_TIMED_OUT: kern_return_t = 49;

/// Upper bound applied to the `tv_sec` of a timed wait. Mach semaphores are
/// never legitimately waited on for longer than this, and clamping keeps a
/// garbage (e.g. `UINT32_MAX`) timeout from overflowing the `Instant`
/// arithmetic below.
const MAX_TIMED_WAIT_SECS: u32 = 3600;

/// How long a timed wait parks the guest thread between acquisition attempts.
/// Mach semaphores have no deadline-aware block in our scheduler (the
/// `ThreadBlock::Semaphore` variant is woken only by `sem_post`), so timed
/// waits poll. 1ms is well below the granularity any Apple audio/engine worker
/// uses (they typically wait in 5–50ms slices) while costing almost nothing.
const TIMED_WAIT_POLL_INTERVAL: Duration = Duration::from_millis(1);

/// DIAG (TOUCHHLE_TRACE_MACHSEM=1): trace Mach semaphore traffic. FMOD drives
/// its async/bank-loader threads entirely through these, so a lost signal shows
/// up here as a wait with no matching signal.
fn machsem_trace(env: &Environment, op: &str, sem: semaphore_t) {
    if !crate::env_flag_cached!("TOUCHHLE_TRACE_MACHSEM") {
        return;
    }
    let value = env
        .libc_state
        .semaphore
        .open_semaphores
        .get(&sem)
        .map(|rc| (**rc).borrow().value);
    let waiters = env
        .libc_state
        .semaphore
        .open_semaphores
        .get(&sem)
        .map(|rc| (**rc).borrow().waiting.len());
    log!(
        "MACHSEM t={} {} sem={:?} value={:?} waiters={:?} lr={:#x}",
        env.current_thread,
        op,
        sem,
        value,
        waiters,
        env.cpu.regs()[14]
    );
}

fn semaphore_create(
    env: &mut Environment,
    task: task_t,
    semaphore: MutPtr<semaphore_t>,
    policy: i32,
    value: i32,
) -> kern_return_t {
    assert_eq!(task, MACH_TASK_SELF);
    assert_eq!(policy, 0);

    let open_semaphore: semaphore_t = env.mem.alloc_and_write(0);
    let res = sem_init(env, open_semaphore, 0, value.try_into().unwrap());
    assert_eq!(res, 0);

    env.mem.write(semaphore, open_semaphore);
    let caller_lr = env.cpu.regs()[14];
    machsem_trace(env, "create", open_semaphore);
    if crate::env_flag_cached!("TOUCHHLE_TRACE_MACHSEM") {
        // Log the creator's LR so we can trace which code path owns each semaphore
        log!("  MACHSEM-CREATE-BT lr={:#x}", caller_lr);
    }
    let result = KERN_SUCCESS;
    log_dbg!(
        "semaphore_create({:?}, {:?}, {:?}, {:?}) -> {:?}",
        task,
        semaphore,
        policy,
        value,
        result
    );
    result
}

fn semaphore_signal(env: &mut Environment, semaphore: semaphore_t) -> kern_return_t {
    // Mirror `sem_post`: a valid semaphore signals successfully (KERN_SUCCESS),
    // an invalid handle maps to KERN_INVALID_ARGUMENT rather than aborting the
    // process. (Mach's `semaphore_signal` returns KERN_INVALID_ARGUMENT for a
    // bad semaphore port.)
    let caller_lr = env.cpu.regs()[14];
    machsem_trace(env, "signal", semaphore);
    if crate::env_flag_cached!("TOUCHHLE_TRACE_MACHSEM") {
        // Also log a symbolized backtrace for every signal, so we can trace
        // which code path is (or isn't) waking the worker.
        log!("  MACHSEM-BT signal lr={:#x}", caller_lr);
    }
    let result = if sem_post(env, semaphore) == 0 {
        KERN_SUCCESS
    } else {
        KERN_INVALID_ARGUMENT
    };
    log_dbg!("semaphore_signal({:?}) -> {:?}", semaphore, result);
    result
}

/// `kern_return_t semaphore_signal_all(semaphore_t semaphore)`
///
/// Wakes *every* thread waiting on the semaphore. Mach does that by releasing
/// one unit per waiter, which is what we do here (a single `sem_post` would
/// only wake the first one and leave the rest blocked forever).
fn semaphore_signal_all(env: &mut Environment, semaphore: semaphore_t) -> kern_return_t {
    // `.cloned()` so the `Rc` is owned: the loop below needs `&mut env` for
    // `sem_post` and must not be holding a borrow of `env.libc_state`.
    let Some(host_sem_rc) = env
        .libc_state
        .semaphore
        .open_semaphores
        .get(&semaphore)
        .cloned()
    else {
        log!(
            "Warning: semaphore_signal_all({:?}) called with an unknown \
             semaphore; returning KERN_INVALID_ARGUMENT.",
            semaphore
        );
        return KERN_INVALID_ARGUMENT;
    };
    let waiters = {
        let host_sem = host_sem_rc.borrow();
        host_sem.waiting.len()
    };
    drop(host_sem_rc);
    for _ in 0..waiters.max(1) {
        // `sem_post` re-looks-up the semaphore and reports EINVAL for a stale
        // handle; ignore the result, the validity check above already covers
        // the interesting failure mode.
        let _ = sem_post(env, semaphore);
    }
    let result = KERN_SUCCESS;
    log_dbg!(
        "semaphore_signal_all({:?}) woke {} waiter(s) -> {:?}",
        semaphore,
        waiters,
        result
    );
    result
}

/// `kern_return_t semaphore_signal_thread(semaphore_t semaphore, thread_act_t thread)`
///
/// Signals the semaphore only if the given thread is waiting on it. We don't
/// track which thread a Mach semaphore port belongs to, so signal
/// unconditionally: over-signalling is benign (the waiter wakes up, which is
/// what the caller wanted) whereas under-signalling would deadlock it.
fn semaphore_signal_thread(
    env: &mut Environment,
    semaphore: semaphore_t,
    thread: u32,
) -> kern_return_t {
    let result = semaphore_signal(env, semaphore);
    log_dbg!(
        "semaphore_signal_thread({:?}, thread {:?}) -> {:?}",
        semaphore,
        thread,
        result
    );
    result
}

// Opt-in deadlock-breaker for the otherwise-infinite `semaphore_wait`,
// enabled with `--mach-semaphore-spurious-wake=<ms>`.
//
// FMOD's worker threads (its AsyncManager command pump and the Studio
// BankLoader queue) park on a Mach semaphore that the producer is supposed to
// signal when it posts work. In Minecraft: Story Mode
// (com.telltalegames.MC100) that producer-side signal never reaches the
// workers under our cooperative
// scheduler, so `Studio::System::loadBankFile` blocks forever in
// `system_checkBlockingBank` during `SoundSystem::Initialize` and the game
// never renders a second frame. Those two FMOD loops re-check their own
// queue state on wake, so for that app a spurious wake is harmless.
//
// This must NOT be on by default. Returning KERN_SUCCESS without having
// acquired the semaphore tells the waiter that a resource is ready when it
// is not. Mono/Unity build their thread synchronisation on Mach semaphores,
// so a lying wait lets a worker touch half-initialized state; the failure
// then appears far away as a managed `NullReferenceException` followed by
// Unity's `trap`/`exit(1)`, with nothing in the log pointing back here.
// (Observed in Turbo Dismount after this breaker was first added
// unconditionally.)
fn semaphore_wait(env: &mut Environment, semaphore: semaphore_t) -> kern_return_t {
    machsem_trace(env, "wait-enter", semaphore);
    if !is_known_semaphore(env, semaphore) {
        log!(
            "Warning: semaphore_wait({:?}) called with an unknown semaphore; \
             returning KERN_INVALID_ARGUMENT.",
            semaphore
        );
        return KERN_INVALID_ARGUMENT;
    }
    // By default this is a true blocking wait: it returns only once a real
    // signal arrives. The spurious-wake deadlock-breaker is opt-in per app
    // (see the doc comment above `--mach-semaphore-spurious-wake=`).
    //
    // IMPORTANT: apply the breaker ONLY to the main thread (thread 0). The
    // blanket version spuriously wakes the FMOD/Wwise *worker* threads, which
    // then loop on an empty command queue and leak memory until OOM (observed
    // in Deus Ex: The Fall: host RSS climbs to 5+ GB). Waking only the main
    // thread lets it time out of a blocking audio call (e.g. a FaceFX dialogue
    // waiting on VO whose Wwise .wem could not be decoded) and continue, while
    // the idle audio workers keep blocking normally and never spin/leak.
    let spurious_wake_after = env
        .options
        .mach_semaphore_spurious_wake_ms
        .filter(|_| env.current_thread == 0)
        .map(Duration::from_millis);
    let deadline = spurious_wake_after.map(|after| env.guest_clock.now() + after);
    let result = loop {
        if env.sem_decrement(semaphore, false) {
            break KERN_SUCCESS;
        }
        if !is_known_semaphore(env, semaphore) {
            break KERN_INVALID_ARGUMENT;
        }
        let Some(deadline) = deadline else {
            // No breaker configured: block until signalled, like Mach does.
            env.sleep_guest(TIMED_WAIT_POLL_INTERVAL);
            continue;
        };
        let now = env.guest_clock.now();
        if now >= deadline {
            // Spurious wake: no real signal arrived in time. Report success
            // so a re-check-on-wake worker loop makes progress. Say so
            // loudly: if the guest is not one of those loops, this is where
            // its state silently goes wrong.
            log!(
                "Warning: semaphore_wait({:?}) on thread {} timed out after \
                 {} ms and is reporting a spurious KERN_SUCCESS \
                 (--mach-semaphore-spurious-wake). The waiter will proceed \
                 without having acquired the semaphore.",
                semaphore,
                env.current_thread,
                spurious_wake_after.unwrap_or_default().as_millis()
            );
            break KERN_SUCCESS;
        }
        env.sleep_guest((deadline - now).min(TIMED_WAIT_POLL_INTERVAL));
    };
    if crate::env_flag_cached!("TOUCHHLE_TRACE_MACHSEM") {
        let ret_lr = env.cpu.regs()[14];
        let ret_pc = env.cpu.regs()[15];
        log!(
            "MACHSEM t={} wait-exit sem={:?} result={:?} returning-to-lr={:#x} pc={:#x}",
            env.current_thread,
            semaphore,
            result,
            ret_lr,
            ret_pc
        );
    }
    log_dbg!("semaphore_wait({:?}) -> {:?}", semaphore, result);
    result
}

/// `kern_return_t semaphore_trywait(semaphore_t semaphore)`
///
/// Non-blocking acquisition: KERN_SUCCESS if a unit was available, otherwise
/// KERN_OPERATION_TIMED_OUT (per `mach/semaphore.h`, which documents that a
/// failed try-wait reports the same code as a timeout).
fn semaphore_trywait(env: &mut Environment, semaphore: semaphore_t) -> kern_return_t {
    if !is_known_semaphore(env, semaphore) {
        log!(
            "Warning: semaphore_trywait({:?}) called with an unknown semaphore; \
             returning KERN_INVALID_ARGUMENT.",
            semaphore
        );
        return KERN_INVALID_ARGUMENT;
    }
    let result = if env.sem_decrement(semaphore, false) {
        KERN_SUCCESS
    } else {
        KERN_OPERATION_TIMED_OUT
    };
    log_dbg!("semaphore_trywait({:?}) -> {:?}", semaphore, result);
    result
}

/// Is `semaphore` a live (not destroyed, not never-created) semaphore handle?
/// Checked before attempting a decrement so `sem_decrement` doesn't spam its
/// "unknown semaphore" warning for handles we already know are bogus.
fn is_known_semaphore(env: &Environment, semaphore: semaphore_t) -> bool {
    env.libc_state
        .semaphore
        .open_semaphores
        .contains_key(&semaphore)
}

/// `kern_return_t semaphore_timedwait(semaphore_t semaphore, mach_timespec_t wait_time)`
///
/// Blocking wait with a **relative** timeout. `mach_timespec_t` is
/// `{ unsigned int tv_sec; clock_res_t tv_nsec; }`; being an 8-byte composite
/// with 4-byte alignment it arrives in two consecutive registers per AAPCS32
/// (r1/r2 after the semaphore port in r0), so it is declared here as two scalar
/// parameters — the host-function ABI has no by-value struct support, and
/// `mach/host.rs`'s `clock_get_time` passes the same struct by pointer for the
/// same reason.
///
/// This used to be missing entirely, which meant the dyld fallback installed a
/// generic "return 0" stub for it. That stub reports KERN_SUCCESS *without
/// blocking and without consuming a semaphore unit*, i.e. it tells the caller
/// "your event fired" every single time. Engines whose worker threads pace
/// themselves with timed waits — the streaming/decoding threads of audio
/// middleware such as FMOD Ex (Geometry Dash), which feed long music tracks
/// while one-shot samples are decoded up-front — then never synchronise with
/// their producer/consumer ring buffers: streamed audio stays silent even
/// though the output device itself works.
fn semaphore_timedwait(
    env: &mut Environment,
    semaphore: semaphore_t,
    tv_sec: u32,
    tv_nsec: u32,
) -> kern_return_t {
    let secs = tv_sec.min(MAX_TIMED_WAIT_SECS);
    let timeout = Duration::from_secs(u64::from(secs)) + Duration::from_nanos(u64::from(tv_nsec));
    // Compare against the guest clock so the timeout follows the game-speed
    // setting, exactly like `sleep_guest` does.
    let deadline = env.guest_clock.now() + timeout;

    loop {
        if !is_known_semaphore(env, semaphore) {
            log!(
                "Warning: semaphore_timedwait({:?}) called with an unknown \
                 semaphore; returning KERN_INVALID_ARGUMENT.",
                semaphore
            );
            return KERN_INVALID_ARGUMENT;
        }
        if env.sem_decrement(semaphore, false) {
            log_dbg!("semaphore_timedwait({:?}) -> {:?}", semaphore, KERN_SUCCESS);
            return KERN_SUCCESS;
        }
        let now = env.guest_clock.now();
        if now >= deadline {
            log_dbg!(
                "semaphore_timedwait({:?}, {}s {}ns) -> {:?}",
                semaphore,
                tv_sec,
                tv_nsec,
                KERN_OPERATION_TIMED_OUT
            );
            return KERN_OPERATION_TIMED_OUT;
        }
        // Park the thread (letting the scheduler run everybody else) and retry.
        let remaining = deadline - now;
        env.sleep_guest(remaining.min(TIMED_WAIT_POLL_INTERVAL));
    }
}

/// `kern_return_t semaphore_wait_signal(semaphore_t wait_semaphore, semaphore_t signal_semaphore)`
///
/// Atomically waits on one semaphore and signals another — the classic
/// hand-off used to pass a token between two worker threads.
fn semaphore_wait_signal(
    env: &mut Environment,
    wait_semaphore: semaphore_t,
    signal_semaphore: semaphore_t,
) -> kern_return_t {
    let result = if sem_wait(env, wait_semaphore) == 0 {
        let _ = sem_post(env, signal_semaphore);
        KERN_SUCCESS
    } else {
        KERN_INVALID_ARGUMENT
    };
    log_dbg!(
        "semaphore_wait_signal({:?}, {:?}) -> {:?}",
        wait_semaphore,
        signal_semaphore,
        result
    );
    result
}

/// `kern_return_t semaphore_timedwait_signal(semaphore_t wait_semaphore, semaphore_t signal_semaphore, mach_timespec_t wait_time)`
///
/// Timed variant of [semaphore_wait_signal]. The second semaphore is only
/// signalled when the wait succeeded; on timeout nothing is posted (matching
/// Mach, where the pair is described as atomic).
fn semaphore_timedwait_signal(
    env: &mut Environment,
    wait_semaphore: semaphore_t,
    signal_semaphore: semaphore_t,
    tv_sec: u32,
    tv_nsec: u32,
) -> kern_return_t {
    let result = semaphore_timedwait(env, wait_semaphore, tv_sec, tv_nsec);
    if result == KERN_SUCCESS {
        let _ = sem_post(env, signal_semaphore);
    }
    log_dbg!(
        "semaphore_timedwait_signal({:?}, {:?}, {}s) -> {:?}",
        wait_semaphore,
        signal_semaphore,
        tv_sec,
        result
    );
    result
}

// https://github.com/apple-oss-distributions/xnu/blob/main/osfmk/man/semaphore_destroy.html
// The task port comes before the semaphore port. Treating r0 as the semaphore
// freed the task port instead of the FMOD semaphore passed in r1.
fn semaphore_destroy(env: &mut Environment, task: task_t, semaphore: semaphore_t) -> kern_return_t {
    if task != MACH_TASK_SELF || !is_known_semaphore(env, semaphore) {
        return KERN_INVALID_ARGUMENT;
    }
    let result = sem_destroy(env, semaphore);
    debug_assert_eq!(result, 0);
    env.mem.free(semaphore.cast());
    let result = KERN_SUCCESS;
    log_dbg!(
        "semaphore_destroy({:?}, {:?}) -> {:?}",
        task,
        semaphore,
        result
    );
    result
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(semaphore_create(_, _, _, _)),
    export_c_func!(semaphore_signal(_)),
    export_c_func!(semaphore_signal_all(_)),
    export_c_func!(semaphore_signal_thread(_, _)),
    export_c_func!(semaphore_wait(_)),
    export_c_func!(semaphore_trywait(_)),
    export_c_func!(semaphore_timedwait(_, _, _)),
    export_c_func!(semaphore_wait_signal(_, _)),
    export_c_func!(semaphore_timedwait_signal(_, _, _, _)),
    export_c_func!(semaphore_destroy(_, _)),
];
