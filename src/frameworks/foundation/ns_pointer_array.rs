/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `NSPointerArray` — Foundation's array that stores arbitrary pointers,
//! shipped in `<Foundation/NSPointerArray.h>`.
//!
//! Unlike `NSArray`, an `NSPointerArray`:
//!   * may contain `NULL` slots (they count towards `-count`),
//!   * exposes a *read-write* `-count` that grows/shrinks the array,
//!   * governs retain / release / equality / zeroing-weak behaviour through
//!     an `NSPointerFunctions` options mask.
//!
//! The two overwhelmingly common constructors are `+strongObjectsPointerArray`
//! and `+weakObjectsPointerArray`. touchHLE has no zeroing-weak runtime, so —
//! exactly like our `NSHashTable` implementation — weak references degrade to
//! strong ones: we retain every stored object and release it on removal /
//! replacement / dealloc. This keeps the pointers valid (avoiding the
//! use-after-free the guest would otherwise hit when a weakly-held object is
//! freed) at the cost of extending object lifetimes. Because we treat stored
//! pointers as Objective-C objects, the memory (`void *`) personality is not
//! supported; the observed uses all store objects.
//!
//! References:
//! - <https://developer.apple.com/documentation/foundation/nspointerarray>
//! - <https://developer.apple.com/documentation/foundation/nspointerfunctions>

use super::ns_enumerator::{fast_enumeration_helper, NSFastEnumerationState};
use super::NSUInteger;
use crate::mem::MutPtr;
use crate::objc::{
    autorelease, id, msg, nil, objc_classes, release, retain, ClassExports, HostObject, NSZonePtr,
};

// `NSPointerFunctionsOptions` (subset). Apple `<Foundation/NSPointerFunctions.h>`.
// touchHLE always uses strong-object personality, so these are exposed for the
// public type signatures only and do not switch behaviour.
pub type NSPointerFunctionsOptions = NSUInteger;

#[derive(Debug, Default)]
struct PointerArrayHostObject {
    /// Stored pointers in order. `nil` represents a `NULL` slot. Every
    /// non-`nil` entry is retained by this Vec (weak degrades to strong).
    pointers: Vec<id>,
    options: NSPointerFunctionsOptions,
}
impl HostObject for PointerArrayHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation NSPointerArray: NSObject

+ (id)allocWithZone:(NSZonePtr)zone {
    if this != env.objc.get_known_class("NSPointerArray", &mut env.mem) {
        log!(
            "Warning: +[{:?} allocWithZone:{:?}] called on NSPointerArray subclass; \
             falling back to NSPointerArray.",
            this, zone
        );
    }
    let host_object = Box::<PointerArrayHostObject>::default();
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

+ (id)pointerArrayWithOptions:(NSPointerFunctionsOptions)options {
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithOptions:options];
    autorelease(env, new)
}

+ (id)strongObjectsPointerArray {
    // `NSPointerFunctionsStrongMemory == 0`, object personality implicit.
    msg![env; this pointerArrayWithOptions:0u32]
}

+ (id)weakObjectsPointerArray {
    // `NSPointerFunctionsWeakMemory == 5`. Degrades to strong here; see the
    // module comment.
    msg![env; this pointerArrayWithOptions:5u32]
}

- (id)init {
    msg![env; this initWithOptions:0u32]
}

- (id)initWithOptions:(NSPointerFunctionsOptions)options {
    env.objc.borrow_mut::<PointerArrayHostObject>(this).options = options;
    this
}

- (())dealloc {
    let host_obj: PointerArrayHostObject = std::mem::take(env.objc.borrow_mut(this));
    for &ptr in &host_obj.pointers {
        if ptr != nil {
            release(env, ptr);
        }
    }
    env.objc.dealloc_object(this, &mut env.mem)
}

// MARK: - Querying

- (NSUInteger)count {
    env.objc.borrow::<PointerArrayHostObject>(this).pointers.len() as NSUInteger
}

// `-count` is read-write on NSPointerArray: growing appends NULL slots,
// shrinking drops (and releases) trailing entries.
- (())setCount:(NSUInteger)new_count {
    let new_count = new_count as usize;
    let mut host: PointerArrayHostObject = std::mem::take(env.objc.borrow_mut(this));
    if new_count < host.pointers.len() {
        let dropped: Vec<id> = host.pointers.split_off(new_count);
        *env.objc.borrow_mut(this) = host;
        for ptr in dropped {
            if ptr != nil {
                release(env, ptr);
            }
        }
    } else {
        host.pointers.resize(new_count, nil);
        *env.objc.borrow_mut(this) = host;
    }
}

- (id)pointerAtIndex:(NSUInteger)index {
    let host = env.objc.borrow::<PointerArrayHostObject>(this);
    let index = index as usize;
    assert!(
        index < host.pointers.len(),
        "-[NSPointerArray pointerAtIndex:] index {} out of bounds (count {})",
        index,
        host.pointers.len()
    );
    host.pointers[index]
}

- (id)allObjects {
    // Per Apple, `-allObjects` skips NULL entries.
    let host = env.objc.borrow::<PointerArrayHostObject>(this);
    let objects: Vec<id> = host.pointers.iter().copied().filter(|&p| p != nil).collect();
    for &obj in &objects {
        retain(env, obj);
    }
    let arr = super::ns_array::from_vec(env, objects);
    autorelease(env, arr)
}

- (NSUInteger)countByEnumeratingWithState:(MutPtr<NSFastEnumerationState>)state
                                  objects:(MutPtr<id>)stackbuf
                                    count:(NSUInteger)len {
    let objects: id = msg![env; this allObjects];
    let count: NSUInteger = msg![env; objects count];
    fast_enumeration_helper(env, this, |env, idx| {
        if idx < count {
            msg![env; objects objectAtIndex:idx]
        } else {
            nil
        }
    }, state, stackbuf, len)
}

// MARK: - Mutation

- (())addPointer:(id)pointer {
    if pointer != nil {
        retain(env, pointer);
    }
    env.objc.borrow_mut::<PointerArrayHostObject>(this).pointers.push(pointer);
}

- (())insertPointer:(id)pointer
            atIndex:(NSUInteger)index {
    let index = index as usize;
    if pointer != nil {
        retain(env, pointer);
    }
    let host = env.objc.borrow_mut::<PointerArrayHostObject>(this);
    assert!(
        index <= host.pointers.len(),
        "-[NSPointerArray insertPointer:atIndex:] index {} out of bounds (count {})",
        index,
        host.pointers.len()
    );
    host.pointers.insert(index, pointer);
}

- (())replacePointerAtIndex:(NSUInteger)index
                withPointer:(id)pointer {
    let index = index as usize;
    if pointer != nil {
        retain(env, pointer);
    }
    let host = env.objc.borrow_mut::<PointerArrayHostObject>(this);
    assert!(
        index < host.pointers.len(),
        "-[NSPointerArray replacePointerAtIndex:withPointer:] index {} out of bounds (count {})",
        index,
        host.pointers.len()
    );
    let old = std::mem::replace(&mut host.pointers[index], pointer);
    if old != nil {
        release(env, old);
    }
}

- (())removePointerAtIndex:(NSUInteger)index {
    let index = index as usize;
    let host = env.objc.borrow_mut::<PointerArrayHostObject>(this);
    assert!(
        index < host.pointers.len(),
        "-[NSPointerArray removePointerAtIndex:] index {} out of bounds (count {})",
        index,
        host.pointers.len()
    );
    let removed = host.pointers.remove(index);
    if removed != nil {
        release(env, removed);
    }
}

- (())compact {
    // Drop every NULL slot. NULL entries are never retained, so no release.
    let host = env.objc.borrow_mut::<PointerArrayHostObject>(this);
    host.pointers.retain(|&p| p != nil);
}

@end

};
