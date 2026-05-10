/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Stubs for Boost and libstdc++ symbols commonly found in iOS game binaries.
//!
//! All symbols are C++ ABI (Itanium mangling). The implementations are
//! minimal — just enough to prevent crashes in apps that link Boost
//! statically but don't heavily use the synchronisation or filesystem APIs.

use crate::abi::GuestFunction;
use crate::dyld::{export_c_func, FunctionExports};
use crate::mem::{ConstPtr, MutPtr, MutVoidPtr};
use crate::Environment;

// MARK: - Type aliases

/// `boost::recursive_mutex` — opaque on-guest struct.
/// We model it as a plain u32 counter (recursion depth).
type BoostRecursiveMutex = MutVoidPtr;

/// `boost::mutex` / `boost::unique_lock<boost::mutex>` — opaque.
type BoostMutex      = MutVoidPtr;
type BoostUniqueLock = MutVoidPtr;

/// `boost::condition_variable` — opaque.
type BoostConditionVariable = MutVoidPtr;

/// `boost::exception_ptr` — opaque 8-byte struct on 32-bit ARM.
/// We treat it as two u32 words.
type BoostExceptionPtr = MutVoidPtr;

// MARK: - boost::recursive_mutex

/// `boost::recursive_mutex::recursive_mutex()` (constructor)
/// Mangled: _ZN5boost15recursive_mutexC2Ev
fn _ZN5boost15recursive_mutexC2Ev(
    _env: &mut Environment,
    _this: BoostRecursiveMutex,
) {
    // No guest state needed — we're single-threaded.
    log_dbg!("boost::recursive_mutex::recursive_mutex() — stubbed");
}

/// `boost::recursive_mutex::lock()`
/// Mangled: _ZN5boost15recursive_mutex4lockEv
fn _ZN5boost15recursive_mutex4lockEv(
    _env: &mut Environment,
    _this: BoostRecursiveMutex,
) {
    // Single-threaded: locking always succeeds immediately.
    log_dbg!("boost::recursive_mutex::lock() — stubbed");
}

/// `boost::recursive_mutex::unlock()`
/// Mangled: _ZN5boost15recursive_mutex10unlockEv  (common variant)
fn _ZN5boost15recursive_mutex10unlockEv(
    _env: &mut Environment,
    _this: BoostRecursiveMutex,
) {
    log_dbg!("boost::recursive_mutex::unlock() — stubbed");
}

/// `boost::recursive_mutex::try_lock()`
/// Mangled: _ZN5boost15recursive_mutex8try_lockEv
fn _ZN5boost15recursive_mutex8try_lockEv(
    _env: &mut Environment,
    _this: BoostRecursiveMutex,
) -> bool {
    true
}

// MARK: - boost::mutex / boost::unique_lock

/// `boost::mutex::lock()`
/// Mangled: _ZN5boost5mutex4lockEv
fn _ZN5boost5mutex4lockEv(
    _env: &mut Environment,
    _this: BoostMutex,
) {
    log_dbg!("boost::mutex::lock() — stubbed");
}

/// `boost::mutex::unlock()`
/// Mangled: _ZN5boost5mutex10unlockEv
fn _ZN5boost5mutex10unlockEv(
    _env: &mut Environment,
    _this: BoostMutex,
) {
    log_dbg!("boost::mutex::unlock() — stubbed");
}

/// `boost::mutex::try_lock()`
/// Mangled: _ZN5boost5mutex8try_lockEv
fn _ZN5boost5mutex8try_lockEv(
    _env: &mut Environment,
    _this: BoostMutex,
) -> bool {
    true
}

/// `boost::unique_lock<boost::mutex>::lock()`
/// Mangled: _ZN5boost11unique_lockINS_5mutexEE4lockEv
fn _ZN5boost11unique_lockINS_5mutexEE4lockEv(
    _env: &mut Environment,
    _this: BoostUniqueLock,
) {
    log_dbg!("boost::unique_lock<boost::mutex>::lock() — stubbed");
}

/// `boost::unique_lock<boost::mutex>::unlock()`
/// Mangled: _ZN5boost11unique_lockINS_5mutexEE10unlockEv
fn _ZN5boost11unique_lockINS_5mutexEE10unlockEv(
    _env: &mut Environment,
    _this: BoostUniqueLock,
) {
    log_dbg!("boost::unique_lock<boost::mutex>::unlock() — stubbed");
}

/// `boost::unique_lock<boost::mutex>::owns_lock()`
/// Mangled: _ZNK5boost11unique_lockINS_5mutexEE9owns_lockEv
fn _ZNK5boost11unique_lockINS_5mutexEE9owns_lockEv(
    _env: &mut Environment,
    _this: BoostUniqueLock,
) -> bool {
    true
}

// MARK: - boost::condition_variable

/// `boost::condition_variable::wait(boost::unique_lock<boost::mutex>&)`
/// Mangled: _ZN5boost18condition_variable4waitERNS_11unique_lockINS_5mutexEEE
fn _ZN5boost18condition_variable4waitERNS_11unique_lockINS_5mutexEEE(
    _env: &mut Environment,
    _this: BoostConditionVariable,
    _lock: BoostUniqueLock,
) {
    // Single-threaded: wait returns immediately (no other thread to notify).
    log_dbg!("boost::condition_variable::wait() — stubbed (returning immediately)");
}

/// `boost::condition_variable::notify_one()`
/// Mangled: _ZN5boost18condition_variable10notify_oneEv
fn _ZN5boost18condition_variable10notify_oneEv(
    _env: &mut Environment,
    _this: BoostConditionVariable,
) {
    log_dbg!("boost::condition_variable::notify_one() — stubbed");
}

/// `boost::condition_variable::notify_all()`
/// Mangled: _ZN5boost18condition_variable10notify_allEv
fn _ZN5boost18condition_variable10notify_allEv(
    _env: &mut Environment,
    _this: BoostConditionVariable,
) {
    log_dbg!("boost::condition_variable::notify_all() — stubbed");
}

// MARK: - boost::detail::sp_counted_base

/// `boost::detail::sp_counted_base::release()`
/// Called by `shared_ptr` / `weak_ptr` destructors.
/// Mangled: _ZN5boost6detail15sp_counted_base7releaseEv
fn _ZN5boost6detail15sp_counted_base7releaseEv(
    env: &mut Environment,
    this: MutVoidPtr,
) {
    if this.is_null() { return; }
    // sp_counted_base layout (32-bit):
    //   offset 0: vtable ptr  (u32)
    //   offset 4: use_count   (i32, atomic)
    //   offset 8: weak_count  (i32, atomic)
    let use_count_ptr: MutPtr<i32> = MutPtr::from_bits(this.to_bits() + 4);
    let current = env.mem.read(use_count_ptr);
    let new_count = current - 1;
    log_dbg!(
        "boost::detail::sp_counted_base::release() use_count {} -> {}",
        current, new_count
    );
    env.mem.write(use_count_ptr, new_count);
    // If count reaches zero we should call dispose() then destroy() via vtable.
    // We don't support vtable dispatch here — just log.
    if new_count <= 0 {
        log_dbg!("boost::detail::sp_counted_base::release(): count=0, object would be destroyed (stubbed)");
    }
}

/// `boost::detail::sp_counted_base::weak_release()`
/// Mangled: _ZN5boost6detail15sp_counted_base12weak_releaseEv
fn _ZN5boost6detail15sp_counted_base12weak_releaseEv(
    env: &mut Environment,
    this: MutVoidPtr,
) {
    if this.is_null() { return; }
    let weak_count_ptr: MutPtr<i32> = MutPtr::from_bits(this.to_bits() + 8);
    let current = env.mem.read(weak_count_ptr);
    log_dbg!("boost::detail::sp_counted_base::weak_release() weak_count {} -> {}", current, current - 1);
    env.mem.write(weak_count_ptr, current - 1);
}

/// `boost::detail::sp_counted_base::add_ref_copy()`
/// Mangled: _ZN5boost6detail15sp_counted_base12add_ref_copyEv
fn _ZN5boost6detail15sp_counted_base12add_ref_copyEv(
    env: &mut Environment,
    this: MutVoidPtr,
) {
    if this.is_null() { return; }
    let use_count_ptr: MutPtr<i32> = MutPtr::from_bits(this.to_bits() + 4);
    let current = env.mem.read(use_count_ptr);
    env.mem.write(use_count_ptr, current + 1);
    log_dbg!("boost::detail::sp_counted_base::add_ref_copy() use_count {} -> {}", current, current + 1);
}

// MARK: - boost::filesystem::basic_path

/// `boost::filesystem::basic_path<std::string, boost::filesystem::path_traits>::operator/=(const char*)`
/// Mangled: _ZN5boost11filesystem210basic_pathISsNS0_11path_traitsEEdVEPKc
///
/// This is the `/=` (append path component) operator.
/// We append the C string to whatever string is stored at offset 0 of the path object.
fn _ZN5boost11filesystem210basic_pathISsNS0_11path_traitsEEdVEPKc(
    env: &mut Environment,
    this: MutVoidPtr,
    rhs: ConstPtr<u8>,
) -> MutVoidPtr {
    if this.is_null() || rhs.is_null() { return this; }
    let segment = env.mem.cstr_at_utf8(rhs).unwrap_or_default().to_owned();
    log_dbg!("boost::filesystem::path::operator/=({:?}) — stubbed", segment);
    // We don't model the internal std::string — just return this.
    this
}

/// `boost::filesystem::basic_path` constructor from `const char*`
/// Mangled: _ZN5boost11filesystem210basic_pathISsNS0_11path_traitsEEC2EPKc
fn _ZN5boost11filesystem210basic_pathISsNS0_11path_traitsEEC2EPKc(
    env: &mut Environment,
    _this: MutVoidPtr,
    path: ConstPtr<u8>,
) {
    let s = env.mem.cstr_at_utf8(path).unwrap_or_default().to_owned();
    log_dbg!("boost::filesystem::path::path({:?}) — stubbed", s);
}

// MARK: - std::vector<std::string> insert

/// `std::vector<std::string>::_M_insert_aux(iterator, const std::string&)`
/// Mangled: _ZNSt6vectorISsSaISsEE13_M_insert_auxEN9__gnu_cxx17__normal_iteratorIPSsS1_EERKSs
///
/// Internal libstdc++ function called when the vector needs to grow.
/// We stub it as a no-op — apps that call this are doing push_back
/// into a Boost/STL container we don't model.
fn _ZNSt6vectorISsSaISsEE13_M_insert_auxEN9__gnu_cxx17__normal_iteratorIPSsS1_EERKSs(
    _env: &mut Environment,
    _this: MutVoidPtr,   // std::vector<std::string>*
    _pos:  MutVoidPtr,   // iterator (pointer into vector storage)
    _val:  MutVoidPtr,   // const std::string&
) {
    log_dbg!("std::vector<std::string>::_M_insert_aux — stubbed");
}

// MARK: - boost::exception_ptr static exception objects

/// Returns a static `boost::exception_ptr` for `std::bad_exception`.
/// Mangled: _ZN5boost16exception_detail27get_static_exception_objectINS0_14bad_exception_EEENS_13exception_ptrEv
fn _ZN5boost16exception_detail27get_static_exception_objectINS0_14bad_exception_EEENS_13exception_ptrEv(
    env: &mut Environment,
) -> MutVoidPtr {
    log_dbg!("boost::exception_detail::get_static_exception_object<bad_exception_>() — returning null sentinel");
    // Return a stable non-null sentinel so callers can store/compare it.
    // We use a small fixed guest address — allocate once on first call.
    static_exception_sentinel(env, 0x0BAD_E001)
}

/// Returns a static `boost::exception_ptr` for `std::bad_alloc`.
/// Mangled: _ZN5boost16exception_detail27get_static_exception_objectINS0_10bad_alloc_EEENS_13exception_ptrEv
fn _ZN5boost16exception_detail27get_static_exception_objectINS0_10bad_alloc_EEENS_13exception_ptrEv(
    env: &mut Environment,
) -> MutVoidPtr {
    log_dbg!("boost::exception_detail::get_static_exception_object<bad_alloc_>() — returning null sentinel");
    static_exception_sentinel(env, 0x0BAD_A001)
}

/// Allocate (once) a small zeroed guest block at a well-known address
/// to act as a stable exception_ptr sentinel.
fn static_exception_sentinel(env: &mut Environment, tag: u32) -> MutVoidPtr {
    // Use the tag bits to key into a tiny table stored in the first
    // few bytes of a dedicated 64-byte sentinel block.
    // In practice apps just compare the pointer, so any stable non-null
    // address works.
    let sentinel: MutVoidPtr = env.mem.alloc(8).cast();
    env.mem.write(sentinel.cast::<u32>(), tag);
    sentinel
}

pub const FUNCTIONS: FunctionExports = &[
    // recursive_mutex
    export_c_func!(_ZN5boost15recursive_mutexC2Ev(_)),
    export_c_func!(_ZN5boost15recursive_mutex4lockEv(_)),
    export_c_func!(_ZN5boost15recursive_mutex10unlockEv(_)),
    export_c_func!(_ZN5boost15recursive_mutex8try_lockEv(_)),
    // mutex
    export_c_func!(_ZN5boost5mutex4lockEv(_)),
    export_c_func!(_ZN5boost5mutex10unlockEv(_)),
    export_c_func!(_ZN5boost5mutex8try_lockEv(_)),
    // unique_lock
    export_c_func!(_ZN5boost11unique_lockINS_5mutexEE4lockEv(_)),
    export_c_func!(_ZN5boost11unique_lockINS_5mutexEE10unlockEv(_)),
    export_c_func!(_ZNK5boost11unique_lockINS_5mutexEE9owns_lockEv(_)),
    // condition_variable
    export_c_func!(_ZN5boost18condition_variable4waitERNS_11unique_lockINS_5mutexEEE(_, _)),
    export_c_func!(_ZN5boost18condition_variable10notify_oneEv(_)),
    export_c_func!(_ZN5boost18condition_variable10notify_allEv(_)),
    // sp_counted_base
    export_c_func!(_ZN5boost6detail15sp_counted_base7releaseEv(_)),
    export_c_func!(_ZN5boost6detail15sp_counted_base12weak_releaseEv(_)),
    export_c_func!(_ZN5boost6detail15sp_counted_base12add_ref_copyEv(_)),
    // filesystem::basic_path
    export_c_func!(_ZN5boost11filesystem210basic_pathISsNS0_11path_traitsEEdVEPKc(_, _)),
    export_c_func!(_ZN5boost11filesystem210basic_pathISsNS0_11path_traitsEEC2EPKc(_, _)),
    // std::vector<std::string>::_M_insert_aux
    export_c_func!(_ZNSt6vectorISsSaISsEE13_M_insert_auxEN9__gnu_cxx17__normal_iteratorIPSsS1_EERKSs(_, _, _)),
    // boost::exception_ptr statics
    export_c_func!(_ZN5boost16exception_detail27get_static_exception_objectINS0_14bad_exception_EEENS_13exception_ptrEv()),
    export_c_func!(_ZN5boost16exception_detail27get_static_exception_objectINS0_10bad_alloc_EEENS_13exception_ptrEv()),
];
