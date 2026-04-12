/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `NSCondition` and `NSConditionLock`.

use crate::frameworks::foundation::NSInteger;
use crate::objc::{
    id, msg, msg_class, nil, objc_classes, release, retain, ClassExports, HostObject, NSZonePtr,
};

// =========================================================================
// MARK: - NSCondition host object
// =========================================================================

struct NSConditionHostObject {
    /// Name NSString* — for debugging.
    name: id,
    /// Simulated signal count — incremented by signal/broadcast,
    /// decremented by wait. Since we have no real OS condition variable
    /// in the guest, we track signals so that wait: returns immediately
    /// when a signal is already pending.
    pending_signals: u32,
}
impl HostObject for NSConditionHostObject {}

// =========================================================================
// MARK: - NSConditionLock host object
// =========================================================================

struct NSConditionLockHostObject {
    name: id,
    /// The current condition value.
    condition: NSInteger,
    /// Whether the lock is currently held.
    locked: bool,
}
impl HostObject for NSConditionLockHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

// =========================================================================
// MARK: - NSCondition
// =========================================================================

@implementation NSCondition: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(NSConditionHostObject {
        name: nil,
        pending_signals: 0,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)init {
    this
}

- (())dealloc {
    let name = env.objc.borrow::<NSConditionHostObject>(this).name;
    release(env, name);
    env.objc.dealloc_object(this, &mut env.mem)
}

// MARK: Name

- (id)name { // NSString*
    env.objc.borrow::<NSConditionHostObject>(this).name
}

- (())setName:(id)name { // NSString*
    let old = env.objc.borrow::<NSConditionHostObject>(this).name;
    release(env, old);
    retain(env, name);
    env.objc.borrow_mut::<NSConditionHostObject>(this).name = name;
}

// MARK: NSLocking protocol

- (())lock {
    // In a real implementation this would acquire the underlying mutex.
    // Since touchHLE is single-threaded at the guest level we treat all
    // locking as no-ops, but keep the signal accounting intact.
    log_dbg!("NSCondition lock {:?}", this);
}

- (())unlock {
    log_dbg!("NSCondition unlock {:?}", this);
}

// MARK: Wait / signal / broadcast

- (())wait {
    // Consume one pending signal if available, otherwise log a warning
    // (a real wait would block until signalled).
    let pending = env.objc.borrow::<NSConditionHostObject>(this).pending_signals;
    if pending > 0 {
        env.objc.borrow_mut::<NSConditionHostObject>(this).pending_signals -= 1;
        log_dbg!("NSCondition wait {:?}: consumed pending signal", this);
    } else {
        log!(
            "Warning: NSCondition wait {:?}: no pending signal — \
             returning immediately (single-threaded guest)",
            this
        );
    }
}

- (bool)waitUntilDate:(id)limit_date { // NSDate*
    // Check if the date has already passed.
    let ti: f64 = msg![env; limit_date timeIntervalSinceNow];
    if ti <= 0.0 {
        log_dbg!("NSCondition waitUntilDate: date already passed => NO", );
        return false;
    }
    // Try to consume a pending signal; if none just return YES as a
    // spurious wake-up (acceptable per POSIX).
    let pending = env.objc.borrow::<NSConditionHostObject>(this).pending_signals;
    if pending > 0 {
        env.objc.borrow_mut::<NSConditionHostObject>(this).pending_signals -= 1;
        log_dbg!("NSCondition waitUntilDate: consumed pending signal => YES");
        return true;
    }
    log_dbg!(
        "NSCondition waitUntilDate: no pending signal, returning YES (spurious wake-up)"
    );
    true
}

- (())signal {
    // Record a pending signal so the next wait() returns immediately.
    env.objc.borrow_mut::<NSConditionHostObject>(this).pending_signals += 1;
    log_dbg!("NSCondition signal {:?}: pending={}", this,
        env.objc.borrow::<NSConditionHostObject>(this).pending_signals);
}

- (())broadcast {
    // A broadcast wakes all waiters. We set pending to a large number
    // so any subsequent waits return immediately.
    env.objc.borrow_mut::<NSConditionHostObject>(this).pending_signals = u32::MAX;
    log_dbg!("NSCondition broadcast {:?}", this);
}

// MARK: Description

- (id)description {
    let (name, pending) = {
        let h = env.objc.borrow::<NSConditionHostObject>(this);
        (h.name, h.pending_signals)
    };
    let name_str = if name != nil {
        crate::frameworks::foundation::ns_string::to_rust_string(env, name).into_owned()
    } else {
        "(nil)".to_string()
    };
    let s = format!(
        "<NSCondition: {:?}; name={}; pendingSignals={}>",
        this, name_str, pending
    );
    let cstr = env.mem.alloc_and_write_cstr(s.as_bytes());
    msg_class![env; NSString stringWithUTF8String:cstr]
}

@end

// =========================================================================
// MARK: - NSConditionLock
// =========================================================================

@implementation NSConditionLock: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(NSConditionLockHostObject {
        name: nil,
        condition: 0,
        locked: false,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)init {
    this
}

- (id)initWithCondition:(NSInteger)condition {
    env.objc.borrow_mut::<NSConditionLockHostObject>(this).condition = condition;
    this
}

- (())dealloc {
    let name = env.objc.borrow::<NSConditionLockHostObject>(this).name;
    release(env, name);
    env.objc.dealloc_object(this, &mut env.mem)
}

// MARK: Name

- (id)name {
    env.objc.borrow::<NSConditionLockHostObject>(this).name
}

- (())setName:(id)name {
    let old = env.objc.borrow::<NSConditionLockHostObject>(this).name;
    release(env, old);
    retain(env, name);
    env.objc.borrow_mut::<NSConditionLockHostObject>(this).name = name;
}

// MARK: Condition

- (NSInteger)condition {
    env.objc.borrow::<NSConditionLockHostObject>(this).condition
}

// MARK: NSLocking protocol

- (())lock {
    log_dbg!("NSConditionLock lock {:?}", this);
    env.objc.borrow_mut::<NSConditionLockHostObject>(this).locked = true;
}

- (())unlock {
    log_dbg!("NSConditionLock unlock {:?}", this);
    env.objc.borrow_mut::<NSConditionLockHostObject>(this).locked = false;
}

// MARK: Conditional locking

- (())lockWhenCondition:(NSInteger)condition {
    // In a real implementation this would block until the condition matches.
    // We just set the condition and lock immediately.
    log_dbg!("NSConditionLock lockWhenCondition:{} — locking immediately", condition);
    env.objc.borrow_mut::<NSConditionLockHostObject>(this).locked = true;
}

- (bool)lockWhenCondition:(NSInteger)condition
               beforeDate:(id)limit_date { // NSDate*
    let ti: f64 = msg![env; limit_date timeIntervalSinceNow];
    if ti <= 0.0 {
        log_dbg!("NSConditionLock lockWhenCondition:{} beforeDate: expired => NO", condition);
        return false;
    }
    let current = env.objc.borrow::<NSConditionLockHostObject>(this).condition;
    if current == condition {
        env.objc.borrow_mut::<NSConditionLockHostObject>(this).locked = true;
        log_dbg!("NSConditionLock lockWhenCondition:{} matched => YES", condition);
        return true;
    }
    log_dbg!(
        "NSConditionLock lockWhenCondition:{} current={} — locking anyway (stub) => YES",
        condition, current
    );
    env.objc.borrow_mut::<NSConditionLockHostObject>(this).locked = true;
    true
}

- (bool)tryLock {
    let locked = env.objc.borrow::<NSConditionLockHostObject>(this).locked;
    if locked {
        log_dbg!("NSConditionLock tryLock: already locked => NO");
        return false;
    }
    env.objc.borrow_mut::<NSConditionLockHostObject>(this).locked = true;
    log_dbg!("NSConditionLock tryLock => YES");
    true
}

- (bool)tryLockWhenCondition:(NSInteger)condition {
    let host = env.objc.borrow::<NSConditionLockHostObject>(this);
    if host.locked || host.condition != condition {
        log_dbg!(
            "NSConditionLock tryLockWhenCondition:{} failed (locked={} cond={}) => NO",
            condition, host.locked, host.condition
        );
        return false;
    }
    env.objc.borrow_mut::<NSConditionLockHostObject>(this).locked = true;
    log_dbg!("NSConditionLock tryLockWhenCondition:{} => YES", condition);
    true
}

- (())unlockWithCondition:(NSInteger)condition {
    log_dbg!("NSConditionLock unlockWithCondition:{}", condition);
    let host = env.objc.borrow_mut::<NSConditionLockHostObject>(this);
    host.locked = false;
    host.condition = condition;
}

// MARK: Description

- (id)description {
    let (condition, locked) = {
        let h = env.objc.borrow::<NSConditionLockHostObject>(this);
        (h.condition, h.locked)
    };
    let s = format!(
        "<NSConditionLock: {:?}; condition={}; locked={}>",
        this, condition, locked
    );
    let cstr = env.mem.alloc_and_write_cstr(s.as_bytes());
    msg_class![env; NSString stringWithUTF8String:cstr]
}

@end

};
