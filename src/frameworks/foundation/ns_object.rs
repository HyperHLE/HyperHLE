/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 * If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//!
//! `NSObject`, the root of most class hierarchies in Objective-C.

use super::ns_dictionary::dict_from_keys_and_objects;
use super::ns_run_loop::{self, NSDefaultRunLoopMode};
use super::ns_string::{from_rust_string, get_static_str, to_rust_string};
use super::{NSInteger, NSTimeInterval, NSUInteger};
// ДОБАВЛЕНЫ ИМПОРТЫ ДЛЯ ЭКСПОРТА ФУНКЦИИ И ОКРУЖЕНИЯ
use crate::abi::{CallFromHost, GuestFunction};
use crate::dyld::{export_c_func, FunctionExports};
use crate::frameworks::foundation::ns_thread::{self, detach_new_thread_inner};
use crate::libc::semaphore::{host_create_semaphore, host_destroy_semaphore, sem_post, sem_wait};
use crate::mem::MutVoidPtr;
use crate::objc::{
    autorelease, id, msg, msg_class, msg_send, msg_send_no_type_checking, msg_send_super2, nil,
    objc_classes, release, retain, Class, ClassExports, NSZonePtr, ObjC, TrivialHostObject, SEL,
};
use crate::Environment;
use std::sync::Mutex;

#[path = "ns_object_kvc.rs"]
mod kvc_helpers;
use self::kvc_helpers::{
    box_kvc_getter_result, box_kvc_ivar_value, kvc_setter_argument_encoding,
    send_numeric_kvc_setter, send_nsvalue_kvc_setter,
};

// Хранилище для отмененных таймеров (target, имя селектора в виде строки)
//
// These side-channel stores are guarded by mutexes rather than being
// `static mut`: re-entrant access (e.g. a `release` triggering `dealloc`
// while iterating) was previously undefined behaviour. Locks must never be
// held across a `msg!`/`msg_send` call, since guest code can re-enter these
// same accessors.
pub static CANCELLED_PERFORMS: Mutex<Vec<(u32, Option<String>)>> = Mutex::new(Vec::new());

// Хранилище для динамических свойств KVC (когда NIB устанавливает кастомные IBOutlet на базовые классы)
pub static DYNAMIC_KVC_STORAGE: Mutex<Vec<(u32, String, u32)>> = Mutex::new(Vec::new());

// Side-channel storage for `performSelectorOnMainThread:withObject:waitUntilDone:YES` requests
// scheduled from background threads. Each entry maps a pending NSTimer's id to the host semaphore
// the background thread is blocked on, so that `_touchHLE_timerFireMethod:` can post the
// semaphore once the selector has finished running on the main thread. Without this, the
// `waitUntilDone:YES` argument is effectively ignored and background threads race ahead of the
// scheduled selector — this manifests, for example, as Call of Duty: Zombies' Marmalade-based
// `RunOnMainThread` helper clobbering `s3eAppDelegate.m_Func` repeatedly before the main thread's
// `-[s3eAppDelegate Functor]` fires, eventually loading a NULL function pointer and crashing.
pub static SYNC_PERFORM_SEMAPHORES: Mutex<Vec<(u32, u32)>> = Mutex::new(Vec::new());

// KVO (Key-Value Observing) storage.
// Each entry: (observed_object_bits, observer_bits, keyPath string, options, context_bits)
pub static KVO_OBSERVERS: Mutex<Vec<(u32, u32, String, u32, u32)>> = Mutex::new(Vec::new());

// Old values snapshotted by willChangeValueForKey: so that
// didChangeValueForKey: can include them in the change dictionary when an
// observer registered with NSKeyValueObservingOptionOld.
// Each entry: (observed_object_bits, key string, retained old value bits)
pub static PENDING_KVO_OLD_VALUES: Mutex<Vec<(u32, String, u32)>> = Mutex::new(Vec::new());

// Values for the NSKeyValueObservingOptions bitmask, per Apple's
// Key-Value Observing documentation.
const NSKeyValueObservingOptionNew: NSUInteger = 0x01;
const NSKeyValueObservingOptionOld: NSUInteger = 0x02;
const NSKeyValueObservingOptionInitial: NSUInteger = 0x04;

/// Kind of change for `NSKeyValueChangeKindKey`: a simple set of the value
/// (`NSKeyValueChangeSetting`).
const NSKeyValueChangeSetting: i32 = 1;

/// `NSCopyObject(id object, NSUInteger extraBytes, NSZone *zone)`.
///
/// Per Apple's Objective-C Runtime Utilities documentation, NSCopyObject
/// "Creates an exact copy of an object." It allocates a new instance of the
/// same class as `object` (plus `extraBytes` of trailing storage) and copies
/// the original instance's bytes into it, returning the new instance. The
/// copy is shallow — object-pointer ivars are duplicated as raw pointers — so
/// classes that build their `-copyWithZone:` on top of NSCopyObject are
/// responsible for retaining any owned ivars themselves. The `zone` argument
/// is obsolete on modern runtimes and is ignored. NSCopyObject is deprecated
/// but plenty of shipping iPhone OS apps still call it from their
/// `-copyWithZone:` implementations.
fn NSCopyObject(
    env: &mut Environment,
    object: id,
    extra_bytes: NSUInteger,
    _zone: NSZonePtr,
) -> id {
    env.objc.object_copy(object, extra_bytes, &mut env.mem)
}

/// Exports `NSCopyObject` for the dynamic linker. `NSAllocateObject` is
/// already exported from `ns_file_manager::FUNCTIONS` — do not duplicate it
/// here or the linker will see two definitions for the same symbol.
pub const FUNCTIONS: FunctionExports = &[export_c_func!(NSCopyObject(_, _, _))];

/// Builds a KVO change dictionary and sends
/// `observeValueForKeyPath:ofObject:change:context:` to one observer.
///
/// Per Apple's Key-Value Observing documentation, the change dictionary
/// always contains `NSKeyValueChangeKindKey` (the string `"kind"`), and
/// contains the new/old values (under `"new"`/`"old"`) only when the
/// corresponding `NSKeyValueObservingOptions` bits were requested; a nil
/// value is represented by `NSNull`.
fn deliver_kvo_notification(
    env: &mut Environment,
    object: id,
    observer_bits: u32,
    key_path: id,
    options: NSUInteger,
    context_bits: u32,
    old_value: id,
    new_value: id,
) {
    let observer: id = crate::mem::Ptr::from_bits(observer_bits);
    let context: id = crate::mem::Ptr::from_bits(context_bits);

    let Some(sel) = env
        .objc
        .lookup_selector("observeValueForKeyPath:ofObject:change:context:")
    else {
        return;
    };

    let kind_key: id = get_static_str(env, "kind");
    let kind_value: id = msg_class![env; NSNumber numberWithInt:NSKeyValueChangeSetting];
    let mut pairs: Vec<(id, id)> = vec![(kind_key, kind_value)];

    if options & NSKeyValueObservingOptionNew != 0 {
        let new_key: id = get_static_str(env, "new");
        let value: id = if new_value == nil {
            msg_class![env; NSNull null]
        } else {
            new_value
        };
        pairs.push((new_key, value));
    }
    if options & NSKeyValueObservingOptionOld != 0 {
        let old_key: id = get_static_str(env, "old");
        let value: id = if old_value == nil {
            msg_class![env; NSNull null]
        } else {
            old_value
        };
        pairs.push((old_key, value));
    }

    let change_dict = dict_from_keys_and_objects(env, &pairs);
    let _: () = msg_send(env, (observer, sel, key_path, object, change_dict, context));
    release(env, change_dict);
}

fn perform_selector_on_thread(
    env: &mut Environment,
    receiver: id,
    selector: SEL,
    argument: id,
    thread: id,
    wait_until_done: bool,
) {
    assert!(!selector.is_null());
    let selector_name = selector.as_str(&env.mem).to_string();
    let Some(thread_id) = ns_thread::thread_id_for_ns_thread(env, thread) else {
        log!(
            "Warning: performSelector:{} on invalid or inactive NSThread {:?}; skipping",
            selector_name,
            thread
        );
        return;
    };

    log_dbg!(
        "performSelector:{} onThread:{:?} current_thread={} target_thread={} waitUntilDone={}",
        selector_name,
        thread,
        env.current_thread,
        thread_id,
        wait_until_done
    );

    if thread_id == env.current_thread && wait_until_done {
        if selector_name.ends_with(':') {
            let _: () = msg_send(env, (receiver, selector, argument));
        } else {
            let _: () = msg_send(env, (receiver, selector));
        }
        return;
    }

    let sel_key: id = get_static_str(env, "SEL");
    let sel_str = from_rust_string(env, selector_name);
    let arg_key: id = get_static_str(env, "arg");
    let user_info = dict_from_keys_and_objects(env, &[(sel_key, sel_str), (arg_key, argument)]);
    let fire_selector = env
        .objc
        .lookup_selector("_touchHLE_timerFireMethod:")
        .unwrap();
    let timer: id = msg_class![env;
        NSTimer timerWithTimeInterval:(0.0 as NSTimeInterval)
                               target:receiver
                             selector:fire_selector
                             userInfo:user_info
                              repeats:false
    ];

    let semaphore = if wait_until_done {
        Some(host_create_semaphore(env, 0))
    } else {
        None
    };
    if let Some(semaphore) = semaphore {
        SYNC_PERFORM_SEMAPHORES
            .lock()
            .unwrap()
            .push((timer.to_bits(), semaphore.to_bits()));
    }

    let run_loop_class = env.objc.get_known_class("NSRunLoop", &mut env.mem);
    let run_loop = ns_run_loop::run_loop_for_thread(env, run_loop_class, thread_id);
    let mode: id = get_static_str(env, NSDefaultRunLoopMode);
    () = msg![env; run_loop addTimer:timer forMode:mode];

    if let Some(semaphore) = semaphore {
        sem_wait(env, semaphore);
        host_destroy_semaphore(env, semaphore);
    }
}

/// Builds the `NSMethodSignature` for instances of `class` responding to
/// `selector`. Methods from the app binary (or added with `class_addMethod`)
/// carry a type encoding, which gives the real return and argument types: an
/// `NSInvocation` needs them for `-getReturnValue:` and for float arguments.
/// Host-implemented methods have none, so for those every argument is assumed
/// to be an object and the return type to be void.
fn method_signature_for(env: &mut Environment, class: Class, selector: SEL) -> id {
    let method = crate::objc::class_getInstanceMethod(env, class, selector);
    let types = crate::objc::method_getTypeEncoding(env, method);
    if !types.is_null() {
        return msg_class![env; NSMethodSignature signatureWithObjCTypes:(types.cast_mut())];
    }
    let sig: id = msg_class![env; NSMethodSignature signatureWithObjCTypes:(MutVoidPtr::null())];
    let sel_str = selector.as_str(&env.mem);
    let explicit_args = sel_str.chars().filter(|&c| c == ':').count() as NSUInteger;
    let total_args = explicit_args + 2;
    () = msg![env; sig _touchHLE_setNumberOfArguments:total_args];
    sig
}

fn object_class_hierarchy(env: &Environment, object: id) -> Vec<Class> {
    let mut class = ObjC::read_isa(object, &env.mem);
    let mut hierarchy = Vec::new();
    while !class.is_null() {
        hierarchy.push(class);
        let superclass = env.objc.get_superclass(class);
        if superclass.is_null() || superclass == class {
            break;
        }
        class = superclass;
    }
    hierarchy.reverse();
    hierarchy
}

pub(crate) fn invoke_cxx_constructors(env: &mut Environment, object: id) {
    let Some(selector) = env.objc.lookup_selector(".cxx_construct") else {
        return;
    };
    let hierarchy = object_class_hierarchy(env, object);
    if !hierarchy
        .iter()
        .any(|&class| env.objc.class_has_uninherited_method(class, selector))
    {
        return;
    }
    let return_value = env.cpu.regs()[0];
    let super_info = env.mem.alloc(8).cast::<u32>();
    for (index, &class) in hierarchy.iter().enumerate() {
        if !env.objc.class_has_uninherited_method(class, selector) {
            continue;
        }
        if index + 1 == hierarchy.len() {
            let _: id = msg_send(env, (object, selector));
        } else {
            env.mem.write(super_info, object.to_bits());
            env.mem
                .write(super_info + 1, hierarchy[index + 1].to_bits());
            let super_info_ptr: crate::mem::ConstPtr<crate::objc::objc_super> =
                super_info.cast_const().cast();
            let _: id = msg_send_super2(env, (super_info_ptr, selector));
        }
    }
    env.mem.free(super_info.cast::<std::ffi::c_void>());
    env.cpu.regs_mut()[0] = return_value;
}

pub(crate) fn invoke_cxx_destructors(env: &mut Environment, object: id) {
    let Some(selector) = env.objc.lookup_selector(".cxx_destruct") else {
        return;
    };
    let hierarchy = object_class_hierarchy(env, object);
    if !hierarchy
        .iter()
        .any(|&class| env.objc.class_has_uninherited_method(class, selector))
    {
        return;
    }
    let super_info = env.mem.alloc(8).cast::<u32>();
    for index in (0..hierarchy.len()).rev() {
        let class = hierarchy[index];
        if !env.objc.class_has_uninherited_method(class, selector) {
            continue;
        }
        if index + 1 == hierarchy.len() {
            let _: () = msg_send(env, (object, selector));
        } else {
            env.mem.write(super_info, object.to_bits());
            env.mem
                .write(super_info + 1, hierarchy[index + 1].to_bits());
            let super_info_ptr: crate::mem::ConstPtr<crate::objc::objc_super> =
                super_info.cast_const().cast();
            let _: () = msg_send_super2(env, (super_info_ptr, selector));
        }
    }
    env.mem.free(super_info.cast::<std::ffi::c_void>());
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation NSObject

+ (id)alloc {
    msg![env; this allocWithZone:(MutVoidPtr::null())]
}
+ (id)allocWithZone:(NSZonePtr)_zone {
    log_dbg!("[{:?} allocWithZone:]", this);
    let object = env.objc.alloc_object(this, Box::new(TrivialHostObject), &mut env.mem);
    invoke_cxx_constructors(env, object);
    object
}

+ (id)new {
    let new_object: id = msg![env; this alloc];
    msg![env; new_object init]
}

+ (Class)class {
    this
}
// Per Apple's NSObject documentation, +superclass returns the class object
// for the receiver's superclass (nil only for a root class).
+ (Class)superclass {
    env.objc.get_superclass(this)
}
+ (bool)isSubclassOfClass:(Class)class {
    env.objc.class_is_subclass_of(this, class)
}

+ (id)retain {
    this
}
+ (())release {
}
+ (())autorelease {
}

+ (())layoutSubviews {
}

// Per Apple's [`+initialize`
// docs](https://developer.apple.com/documentation/objectivec/nsobject/1418639-initialize),
// the runtime sends this exactly once to every class before it receives any
// other message. Subclasses commonly chain to `[super initialize]`; if no
// class in the inheritance chain implemented +initialize then the dispatch
// would fall off the top of the chain and print a `superclass does not
// respond to selector "initialize"` warning (this used to break
// `ASIHTTPRequest` + initialize, which calls `[super initialize]`).
// Provide an explicit no-op at the root so `[super initialize]` is always
// safe.
+ (())initialize {
}

+ (bool)instancesRespondToSelector:(SEL)selector {
    env.objc.class_has_method(this, selector)
}

// `+instanceMethodForSelector:` must resolve the selector to the same IMP
// an instance would use — i.e. it has to agree with `-methodForSelector:`
// for the same class+selector. Objective-Chipmunk's override detection
// (`ChipmunkBody methodIsOverriden:`) compares
// `[self methodForSelector:sel]` against
// `[[self class] instanceMethodForSelector:sel]`; when the two disagree it
// concludes the method is overridden. The old implementation ignored the
// selector and always handed back the `_objc_msgSend` trampoline, so the
// comparison never matched the real IMP returned by `-methodForSelector:`.
// Every physics body then looked as if it overrode its velocity/position
// integrator, and Chipmunk installed an ObjC-dispatch update callback that
// was invoked with the C `cpBody*` calling convention, branching into
// garbage (an `UndefinedInstruction` at the image base / heap) on the very
// first physics step. Resolving the selector here — exactly like
// `-methodForSelector:` — keeps the two in agreement. In a class method
// `this` is the class itself, which is what `class_getMethodImplementation`
// expects. Host-implemented methods still come back as the `_objc_msgSend`
// trampoline via `class_getMethodImplementation`, preserving the previous
// behaviour for them.
+ (u32)instanceMethodForSelector:(SEL)selector {
    crate::objc::class_getMethodImplementation(env, this, selector).to_bits()
}

+ (id)instanceMethodSignatureForSelector:(SEL)selector {
    method_signature_for(env, this, selector)
}

+ (bool)accessInstanceVariablesDirectly {
    true
}

// Методы класса
+ (id)description {
    let name = env.objc.get_class_name(this);
    let str = from_rust_string(env, name.to_string());
    autorelease(env, str)
}

+ (id)debugDescription {
    msg![env; this description]
}

+ (())cancelPreviousPerformRequestsWithTarget:(id)target
                                     selector:(SEL)selector
                                       object:(id)_object {
    let sel_str = selector.as_str(&env.mem).to_string();
    CANCELLED_PERFORMS
        .lock()
        .unwrap()
        .push((target.to_bits(), Some(sel_str)));
}

+ (())cancelPreviousPerformRequestsWithTarget:(id)target {
    CANCELLED_PERFORMS
        .lock()
        .unwrap()
        .push((target.to_bits(), None));
}

- (id)init {
    this
}

// ИСПРАВЛЕНИЕ: Добавлены методы ЭКЗЕМПЛЯРА description и debugDescription
- (id)description {
    let class: Class = msg![env; this class];
    let name = env.objc.get_class_name(class);
    // Формируем классическую строку вида <ClassName: 0xAddress>
    let desc_str = format!("<{}: 0x{:x}>", name, this.to_bits());
    let str = from_rust_string(env, desc_str);
    autorelease(env, str)
}

- (id)debugDescription {
    msg![env; this description]
}

- (NSUInteger)retainCount {
    env.objc.get_refcount(this).into()
}

- (id)retain {
    log_dbg!("[{:?} retain]", this);
    env.objc.increment_refcount(this);
    this
}
- (())release {
    log_dbg!("[{:?} release]", this);
    if env.objc.decrement_refcount(this) {
        () = msg![env; this dealloc];
    }
}
- (id)autorelease {
    () = msg_class![env; NSAutoreleasePool addObject:this];
    this
}

- (())dealloc {
    log_dbg!("[{:?} dealloc]", this);
    invoke_cxx_destructors(env, this);

    // Очищаем и высвобождаем динамические свойства KVC
    let mut to_release = Vec::new();
    {
        let target_bits = this.to_bits();
        DYNAMIC_KVC_STORAGE.lock().unwrap().retain(|entry| {
            if entry.0 == target_bits {
                if entry.2 != 0 {
                    to_release.push(entry.2);
                }
                false
            } else {
                true
            }
        });
    }

    // The lock is released before sending any `release` message: releasing a
    // stored value can trigger its `dealloc`, which re-enters this storage.
    for val_bits in to_release {
        let val: id = crate::mem::Ptr::from_bits(val_bits);
        release(env, val);
    }

    env.objc.dealloc_object(this, &mut env.mem)
}

- (Class)class {
    ObjC::read_isa(this, &env.mem)
}
- (bool)isMemberOfClass:(Class)class {
    let this_class: Class = msg![env; this class];
    class == this_class
}
- (bool)isKindOfClass:(Class)class {
    let this_class: Class = msg![env; this class];
    env.objc.class_is_subclass_of(this_class, class)
}

- (NSUInteger)hash {
    this.to_bits()
}

- (bool)isEqual:(id)other {
    this == other
}

- (id)copy {
    msg![env; this copyWithZone:(MutVoidPtr::null())]
}

- (id)mutableCopy {
    msg![env; this mutableCopyWithZone:(MutVoidPtr::null())]
}

- (())setValue:(id)value forKey:(id)key {
    if key == nil {
        log_dbg!("setValue:forKey: — key is nil, ignored");
        return;
    }

    let key_string = to_rust_string(env, key);
    if key_string.is_empty() || !key_string.is_ascii() {
        log!("Warning: setValue:forKey: key {:?} is empty or non-ASCII — calling setValue:forUndefinedKey:", key_string);
        let sel = env
            .objc
            .register_host_selector("setValue:forUndefinedKey:".to_string(), &mut env.mem);
        let _: () = msg_send(env, (this, sel, value, key));
        return;
    }

    let camel_case_key_string = format!(
        "{}{}",
        key_string.as_bytes()[0].to_ascii_uppercase() as char,
        &key_string[1..]
    );
    let class = msg![env; this class];

    if value == nil {
        log_dbg!("setValue:forKey: value is nil for key {:?} — calling setNilValueForKey:", key_string);
        if let Some(sel) = env.objc.lookup_selector(&format!("set{camel_case_key_string}:")) {
            if env.objc.class_has_method(class, sel) {
                let _: () = msg_send(env, (this, sel, value));
                return;
            }
        }
        // Per Apple's KVC contract: when -setValue:forKey: receives nil for
        // a key that maps to a scalar (non-object) property, it must call
        // -setNilValueForKey: on the receiver. The default NSObject impl of
        // -setNilValueForKey: raises NSInvalidArgumentException. We register
        // the selector defensively so the lookup never returns None even if
        // no host class ever defined the method directly.
        let sel = env
            .objc
            .register_host_selector("setNilValueForKey:".to_string(), &mut env.mem);
        let _: () = msg_send(env, (this, sel, key));
        return;
    }

    let value_class = msg![env; value class];
    let ns_value_class = env.objc.get_known_class("NSValue", &mut env.mem);
    let ns_number_class = env.objc.get_known_class("NSNumber", &mut env.mem);
    let is_number = env
        .objc
        .class_is_subclass_of(value_class, ns_number_class);
    let is_value = env
        .objc
        .class_is_subclass_of(value_class, ns_value_class);
    if is_value {
        log_dbg!(
            "setValue:forKey: value {:?} is NSValue subclass for key {:?} — proceeding",
            value, key_string
        );
    }

    if let Some(sel) = env.objc.lookup_selector(&format!("set{camel_case_key_string}:")) {
        if env.objc.class_has_method(class, sel) {
            if is_number || is_value {
                if let Some(argument_encoding) = kvc_setter_argument_encoding(env, class, sel) {
                    if is_number {
                        if let Some(&argument_type) = argument_encoding.first() {
                            if send_numeric_kvc_setter(
                                env,
                                this,
                                sel,
                                value,
                                argument_type,
                            ) {
                                return;
                            }
                        }
                    } else if send_nsvalue_kvc_setter(
                        env,
                        this,
                        sel,
                        value,
                        &argument_encoding,
                    ) {
                        return;
                    }
                }
            }
            let _: () = msg_send(env, (this, sel, value));
            return;
        }
    }

    if let Some(sel) = env.objc.lookup_selector(&format!("_set{camel_case_key_string}:")) {
        if env.objc.class_has_method(class, sel) {
            if is_number || is_value {
                if let Some(argument_encoding) = kvc_setter_argument_encoding(env, class, sel) {
                    if is_number {
                        if let Some(&argument_type) = argument_encoding.first() {
                            if send_numeric_kvc_setter(
                                env,
                                this,
                                sel,
                                value,
                                argument_type,
                            ) {
                                return;
                            }
                        }
                    } else if send_nsvalue_kvc_setter(
                        env,
                        this,
                        sel,
                        value,
                        &argument_encoding,
                    ) {
                        return;
                    }
                }
            }
            let _: () = msg_send(env, (this, sel, value));
            return;
        }
    }

    let access_sel = env
        .objc
        .register_host_selector("accessInstanceVariablesDirectly".to_string(), &mut env.mem);
    let access_ivars: bool = msg_send(env, (class, access_sel));
    if access_ivars {
        if let Some(ivar_ptr) = env.objc
            .object_lookup_ivar(&env.mem, this, &format!("_{key_string}"))
            .or_else(|| env.objc.object_lookup_ivar(&env.mem, this, &format!("_is{camel_case_key_string}")))
            .or_else(|| env.objc.object_lookup_ivar(&env.mem, this, &format!("{key_string}")))
            .or_else(|| env.objc.object_lookup_ivar(&env.mem, this, &format!("is{camel_case_key_string}")))
        {
            retain(env, value);
            env.mem.write(ivar_ptr.cast(), value);
            return;
        }
    }

    let undef_sel = env
        .objc
        .register_host_selector("setValue:forUndefinedKey:".to_string(), &mut env.mem);
    let _: () = msg_send(env, (this, undef_sel, value, key));
}


- (())setValue:(id)value forUndefinedKey:(id)key {
    let class: Class = ObjC::read_isa(this, &env.mem);
    let class_name_string = env.objc.get_class_name(class).to_owned();
    let key_string = to_rust_string(env, key);

    log!("Warning: Object {:?} of class {:?} does not have a setter for {} — storing dynamically",
        this, class_name_string, key_string);

    // Честно сохраняем значение и делаем retain
    if value != nil {
        retain(env, value);
    }

    // Swap the stored value while holding the lock, but defer releasing the
    // old value until after the lock is dropped: `release` can trigger
    // `dealloc`, which re-enters this storage.
    let old_val_bits = {
        let target_bits = this.to_bits();
        let mut storage = DYNAMIC_KVC_STORAGE.lock().unwrap();
        if let Some(entry) = storage
            .iter_mut()
            .find(|entry| entry.0 == target_bits && entry.1 == key_string)
        {
            std::mem::replace(&mut entry.2, value.to_bits())
        } else {
            storage.push((target_bits, key_string.to_string(), value.to_bits()));
            0
        }
    };
    if old_val_bits != 0 {
        let old_val: id = crate::mem::Ptr::from_bits(old_val_bits);
        release(env, old_val);
    }
}

// Per Apple's NSKeyValueCoding (NSKeyValueCoding.h / Cocoa Key-Value
// Coding Programming Guide): when -setValue:forKey: receives a nil
// value for a key whose property is a non-object scalar (BOOL, NSInteger,
// CGFloat, struct, …), the receiver is sent -setNilValueForKey:. The
// default NSObject behaviour is to raise an NSInvalidArgumentException
// with reason "[<Class> 0x… setNilValueForKey:]: could not set nil as
// the value for the key <key>." We mirror that contract: log loudly so
// the developer can diagnose the bad write, and do nothing else — most
// iOS games rely on this being a non-fatal call (the surrounding code
// catches the exception or guards against it).
- (())setNilValueForKey:(id)key {
    let class: Class = ObjC::read_isa(this, &env.mem);
    let class_name_string = env.objc.get_class_name(class).to_owned();
    let key_string = if key == nil {
        "(null)".to_string()
    } else {
        to_rust_string(env, key).to_string()
    };
    log!(
        "Warning: -[{} setNilValueForKey:@\"{}\"] on {:?}: nil assigned to a scalar property; ignored (Apple's default would raise NSInvalidArgumentException).",
        class_name_string, key_string, this
    );
}

- (bool)respondsToSelector:(SEL)selector {
    env.objc.object_has_method(&env.mem, this, selector)
}

- (bool)conformsToProtocol:(id)protocol {
    // Sent to a class object, this asks about the class itself.
    let isa = ObjC::read_isa(this, &env.mem);
    let class = if env.objc.class_is_metaclass(isa) { this } else { isa };
    // Which protocols host classes adopt isn't modelled, so keep answering YES
    // for those rather than break apps that check for e.g. NSCoding.
    env.objc
        .class_conforms_to_bin_protocol(&env.mem, class, protocol)
        .unwrap_or(true)
}

- (u32)methodForSelector:(SEL)selector {
    let class = crate::objc::ObjC::read_isa(this, &env.mem);
    let implementation = crate::objc::class_getMethodImplementation(env, class, selector);
    if crate::env_flag_cached!("TOUCHHLE_TRACE_METHOD_FOR_SELECTOR") {
        let class_name = env.objc.get_class_name(class).to_owned();
        let selector_name = selector.as_str(&env.mem);
        log!(
            "methodForSelector receiver={:?} class={} selector={} IMP={:#x}",
            this,
            class_name,
            selector_name,
            implementation.to_bits()
        );
    }
    implementation.to_bits()
}

- (id)methodSignatureForSelector:(SEL)selector {
    // For a class object this is its metaclass, so class methods are found.
    let class = ObjC::read_isa(this, &env.mem);
    method_signature_for(env, class, selector)
}

// MARK: - Telemetry no-ops
// Some SDKs (e.g. the analytics bundled with MCPE 0.14.x) call generic
// setters like setSessionId:/enqueueTelemetryItem: on whatever object they
// keep in a static. Because these are ordinary method calls, every object
// must respond to avoid "does not respond to selector" warnings. Telemetry
// itself is a no-op in the emulator, so these are silent no-ops too.

- (())setSessionId:(id)session_id {
    log_dbg!("setSessionId:{:?} — telemetry no-op", session_id);
}

- (())setIsFirstSession:(bool)is_first_session {
    log_dbg!("setIsFirstSession:{} — telemetry no-op", is_first_session);
}

- (())setIsNewSession:(bool)is_new_session {
    log_dbg!("setIsNewSession:{} — telemetry no-op", is_new_session);
}

- (())enqueueTelemetryItem:(id)item {
    log_dbg!("enqueueTelemetryItem:{:?} — telemetry no-op", item);
}

- (id)performSelector:(SEL)sel {
    assert!(!sel.is_null());
    msg_send_no_type_checking(env, (this, sel))
}

- (id)performSelector:(SEL)sel withObject:(id)o1 {
    assert!(!sel.is_null());
    msg_send_no_type_checking(env, (this, sel, o1))
}

- (id)performSelector:(SEL)sel withObject:(id)o1 withObject:(id)o2 {
    assert!(!sel.is_null());
    msg_send_no_type_checking(env, (this, sel, o1, o2))
}

- (())performSelectorInBackground:(SEL)sel withObject:(id)arg {
    detach_new_thread_inner(env, sel, this, arg, /* tolerate_type_mismatch: */ true)
}

- (())performSelector:(SEL)sel withObject:(id)arg afterDelay:(NSTimeInterval)delay {
    log_dbg!("performSelector:{} withObject:{:?} afterDelay:{}", sel.as_str(&env.mem), arg, delay);
    let sel_key: id = get_static_str(env, "SEL");
    let sel_str = from_rust_string(env, sel.as_str(&env.mem).to_string());
    let arg_key: id = get_static_str(env, "arg");
    let dict = dict_from_keys_and_objects(env, &[(sel_key, sel_str), (arg_key, arg)]);

    let selector = env.objc.lookup_selector("_touchHLE_timerFireMethod:").unwrap();
    let timer:id = msg_class![env;
        NSTimer timerWithTimeInterval:delay
                               target:this
                             selector:selector
                             userInfo:dict
                              repeats:false
    ];
    let run_loop: id = msg_class![env; NSRunLoop mainRunLoop];
    let mode: id = get_static_str(env, NSDefaultRunLoopMode);
    () = msg![env; run_loop addTimer:timer forMode:mode];
}

// `- (void)performSelectorOnMainThread:(SEL)selector withObject:(id)arg
//      waitUntilDone:(BOOL)wait modes:(NSArray *)modes`
//
// The `modes:` variant is the one newer SDKs use (RetroArch's iOS build calls
// it from its `CocoaMainThreadJob` queue). touchHLE's run loop keeps a single
// set of sources and ignores modes — see `-[NSRunLoop addTimer:forMode:]` —
// so this is exactly the two-argument method with the modes list dropped.
// https://developer.apple.com/documentation/foundation/nsobject/1414893-performselectoronmainthread
- (())performSelectorOnMainThread:(SEL)sel
                       withObject:(id)arg
                    waitUntilDone:(bool)wait
                            modes:(id)_modes {
    () = msg![env; this performSelectorOnMainThread:sel withObject:arg waitUntilDone:wait];
}

- (())performSelectorOnMainThread:(SEL)sel
                       withObject:(id)arg
                    waitUntilDone:(bool)wait {
    let sel_name = sel.as_str(&env.mem);

    // Video playback selectors: instead of silently dropping these, we
    // forward them to the object so that MPMoviePlayerController's play/stop
    // implementations run and post the required notifications (e.g.
    // MPMoviePlayerPlaybackDidFinishNotification). This allows apps that
    // start video playback from a background thread to have their
    // completion handlers fire correctly.
    if sel_name == "play" || sel_name == "startMovie:" || sel_name == "stopMovie:" || sel_name == "stopMovie" || sel_name == "moviePlayerInit:" || sel_name == "loadMovie:" {
        log_dbg!(
            "performSelectorOnMainThread:SEL({}) — forwarding video selector to object {:?}",
            sel_name, this
        );
        if sel_name.ends_with(':') {
            () = msg_send(env, (this, sel, arg));
        } else {
            () = msg_send(env, (this, sel));
        }
        return;
    }

    if env.current_thread == 0 && wait {
        if sel_name.ends_with(':') {
            () = msg_send(env, (this, sel, arg));
        } else {
            () = msg_send(env, (this, sel));
        }
        return;
    }

    if env.bundle.bundle_identifier().starts_with("com.gameloft.Ferrari") && wait
        && (sel == env.objc.lookup_selector("initTextInput:").unwrap() ||
           sel == env.objc.lookup_selector("removeTextField:").unwrap()) {
            log!("Applying game-specific hack for Ferrari GT: performing performSelectorOnMainThread:SEL({}) waitUntilDone:true on thread {}", sel_name, env.current_thread);
            () = msg_send(env, (this, sel, arg));
            return;
        }

    if env.bundle.bundle_identifier().starts_with("com.gameloft.HOS2") && wait
        && (sel == env.objc.lookup_selector("sendGameInfo").unwrap() || sel == env.objc.lookup_selector("setStatusBar:").unwrap()) {
            log!("Applying game-specific hack for HOS2: performing performSelectorOnMainThread:SEL({}) waitUntilDone:true on thread {}", sel_name, env.current_thread);
            if sel_name.ends_with(':') {
                () = msg_send(env, (this, sel, arg));
            } else {
                () = msg_send(env, (this, sel));
            }
            return;
        }

    if wait {
        // `waitUntilDone:YES` from a background thread: schedule the selector to run on the main
        // thread and block the calling thread on a host semaphore that
        // `_touchHLE_timerFireMethod:` will post once the selector has finished executing.
        //
        // Games such as Call of Duty: Zombies (Marmalade SDK) rely on this synchronisation to
        // safely hand work off to the main thread via an `s3eAppDelegate.m_Func` slot: the
        // background thread sets `m_Func`, calls `performSelectorOnMainThread:Functor
        // withObject:nil waitUntilDone:YES`, and expects to block until `Functor` has called and
        // cleared `m_Func`. Returning early here lets the background thread overwrite `m_Func`
        // before the main thread's `Functor` has a chance to read it, eventually loading a
        // NULL function pointer and crashing into the guest's null page.
        let sel_name_owned = sel_name.to_string();
        log_dbg!(
            "performSelectorOnMainThread:{} from background thread {} (wait=true) — scheduling and waiting",
            sel_name_owned, env.current_thread
        );

        let sel_key: id = get_static_str(env, "SEL");
        let sel_str = from_rust_string(env, sel_name_owned);
        let arg_key: id = get_static_str(env, "arg");
        let dict = dict_from_keys_and_objects(env, &[(sel_key, sel_str), (arg_key, arg)]);

        let fire_selector = env.objc.lookup_selector("_touchHLE_timerFireMethod:").unwrap();
        let timer: id = msg_class![env;
            NSTimer timerWithTimeInterval:(0.0 as NSTimeInterval)
                                   target:this
                                 selector:fire_selector
                                 userInfo:dict
                                  repeats:false
        ];

        let sem = host_create_semaphore(env, 0);
        SYNC_PERFORM_SEMAPHORES
            .lock()
            .unwrap()
            .push((timer.to_bits(), sem.to_bits()));

        let run_loop: id = msg_class![env; NSRunLoop mainRunLoop];
        let mode: id = get_static_str(env, NSDefaultRunLoopMode);
        () = msg![env; run_loop addTimer:timer forMode:mode];

        sem_wait(env, sem);
        host_destroy_semaphore(env, sem);
        return;
    }

    log_dbg!(
        "performSelectorOnMainThread:{} from thread {} (wait={}) — scheduling",
        sel_name, env.current_thread, wait
    );
    msg![env; this performSelector:sel withObject:arg afterDelay:0.0]
}

- (())_touchHLE_timerFireMethod:(id)which {
    // Pull out any semaphore associated with this timer up-front so we can
    // always post it before returning, regardless of how this method exits.
    // (If we returned early without posting, a thread blocked in
    // performSelectorOnMainThread:waitUntilDone:YES would hang forever.)
    let sem_to_post = {
        let timer_bits = which.to_bits();
        let mut semaphores = SYNC_PERFORM_SEMAPHORES.lock().unwrap();
        semaphores
            .iter()
            .position(|x| x.0 == timer_bits)
            .map(|pos| semaphores.remove(pos).1)
    };

    let dict: id = msg![env; which userInfo];
    let sel_key: id = get_static_str(env, "SEL");
    let sel_str_id: id = msg![env; dict objectForKey:sel_key];
    let sel_str = to_rust_string(env, sel_str_id).into_owned();

    // The stored selector name should always be present, but guard against a
    // missing/empty entry (e.g. the timer's userInfo dictionary was released
    // out from under us) rather than panicking. With no selector there is
    // nothing to fire, so just release any waiter and bail.
    if sel_str.is_empty() {
        // The userInfo dictionary carries the performSelector bookkeeping.
        // If it is missing (e.g. the dict was released out from under us, or
        // the timer reached us through another creation path), fall back to
        // firing the timer's own target/selector pair, exactly as
        // -[NSTimer fire] does, so a legitimate timer is not silently
        // dropped. Never recurse through this method itself.
        let (t_target, t_selector, t_repeats, t_valid) = {
            let host = env
                .objc
                .borrow::<crate::frameworks::foundation::ns_timer::NSTimerHostObject>(which);
            (host.target, host.selector, host.repeats, host.due_by.is_some())
        };
        let fire_sel = env.objc.lookup_selector("_touchHLE_timerFireMethod:").unwrap();
        if t_valid && t_target != nil && !t_selector.is_null() && t_selector != fire_sel {
            log_dbg!(
                "_touchHLE_timerFireMethod: timer {:?} has no userInfo selector; \
                 firing its own selector {} instead.",
                which,
                t_selector.as_str(&env.mem)
            );
            let pool: id = msg_class![env; NSAutoreleasePool new];
            let _: () = msg_send(env, (t_target, t_selector, which));
            release(env, pool);
            if !t_repeats {
                let _: () = msg![env; which invalidate];
            }
        } else {
            log_dbg!(
                "_touchHLE_timerFireMethod: timer {:?} has no stored selector; skipping.",
                which
            );
        }
        if let Some(sem_bits) = sem_to_post {
            let sem: crate::mem::MutPtr<crate::libc::semaphore::sem_t> =
                crate::mem::MutPtr::from_bits(sem_bits);
            sem_post(env, sem);
        }
        return;
    }

    // Turn the stored name into a selector. This mirrors sel_registerName():
    // a non-empty method name always maps to a valid selector, registering a
    // new one if it has not been seen before, so this never returns None.
    let sel = env
        .objc
        .register_host_selector(sel_str.clone(), &mut env.mem);

    let arg_key: id = get_static_str(env, "arg");
    let arg: id = msg![env; dict objectForKey:arg_key];
    let target_bits = this.to_bits();
    let mut cancelled = false;

    {
        let mut cancelled_performs = CANCELLED_PERFORMS.lock().unwrap();
        if let Some(pos) = cancelled_performs
            .iter()
            .position(|x| x.0 == target_bits && x.1.as_deref() == Some(sel_str.as_str()))
        {
            cancelled_performs.remove(pos);
            cancelled = true;
        } else if cancelled_performs
            .iter()
            .any(|x| x.0 == target_bits && x.1.is_none())
        {
            cancelled = true;
        }
    }

    if !cancelled {
        if sel.as_str(&env.mem).ends_with(':') {
            () = msg_send(env, (this, sel, arg));
        } else {
            () = msg_send(env, (this, sel));
        }
    }

    if let Some(sem_bits) = sem_to_post {
        let sem: crate::mem::MutPtr<crate::libc::semaphore::sem_t> =
            crate::mem::MutPtr::from_bits(sem_bits);
        sem_post(env, sem);
    }
}

- (())awakeFromNib {
}

- (())performSelector:(SEL)sel onThread:(id)_thread withObject:(id)arg waitUntilDone:(bool)_wait {
    perform_selector_on_thread(env, this, sel, arg, _thread, _wait)
}

- (())performSelector:(SEL)sel onThread:(id)_thread withObject:(id)arg waitUntilDone:(bool)_wait modes:(id)_modes {
    perform_selector_on_thread(env, this, sel, arg, _thread, _wait)
}

- (id)valueForKey:(id)key {
    let key_str = super::ns_string::to_rust_string(env, key);
    if key_str.is_empty() { return nil; }

    let camel_case_key_string = format!(
        "{}{}",
        key_str.as_bytes()[0].to_ascii_uppercase() as char,
        &key_str[1..]
    );

    if key_str == "boolValue" {
        let value: bool = msg![env; this boolValue];
        return msg_class![env; NSNumber numberWithBool:value];
    }
    if key_str == "floatValue" {
        let value: f32 = msg![env; this floatValue];
        return msg_class![env; NSNumber numberWithFloat:value];
    }
    if key_str == "doubleValue" {
        let value: f64 = msg![env; this doubleValue];
        return msg_class![env; NSNumber numberWithDouble:value];
    }
    if key_str == "intValue" {
        let value: i32 = msg![env; this intValue];
        return msg_class![env; NSNumber numberWithInt:value];
    }
    if key_str == "integerValue" {
        let value: NSInteger = msg![env; this integerValue];
        return msg_class![env; NSNumber numberWithInteger:value];
    }
    if key_str == "longLongValue" {
        let value: i64 = msg![env; this longLongValue];
        return msg_class![env; NSNumber numberWithLongLong:value];
    }
    if key_str == "unsignedIntValue" {
        let value: u32 = msg![env; this unsignedIntValue];
        return msg_class![env; NSNumber numberWithUnsignedInt:value];
    }
    if key_str == "unsignedLongLongValue" {
        let value: u64 = msg![env; this unsignedLongLongValue];
        return msg_class![env; NSNumber numberWithUnsignedLongLong:value];
    }

    // 1. Поиск геттеров (get<Key>, <key>, is<Key>)
    if let Some(sel) = env.objc.lookup_selector(&key_str) {
        if env.objc.object_has_method(&env.mem, this, sel) {
            if let Some(boxed) = box_kvc_getter_result(env, this, sel) {
                return boxed;
            }
            return msg_send(env, (this, sel));
        }
    }
    let is_sel_name = format!("is{camel_case_key_string}");
    if let Some(sel) = env.objc.lookup_selector(&is_sel_name) {
        if env.objc.object_has_method(&env.mem, this, sel) {
            if let Some(boxed) = box_kvc_getter_result(env, this, sel) {
                return boxed;
            }
            return msg_send(env, (this, sel));
        }
    }
    let get_sel_name = format!("get{camel_case_key_string}");
    if let Some(sel) = env.objc.lookup_selector(&get_sel_name) {
        if env.objc.object_has_method(&env.mem, this, sel) {
            if let Some(boxed) = box_kvc_getter_result(env, this, sel) {
                return boxed;
            }
            return msg_send(env, (this, sel));
        }
    }

    // 2. Чтение реальных ivars
    let class = msg![env; this class];
    if let Some(access_sel) = env.objc.lookup_selector("accessInstanceVariablesDirectly") {
        if env.objc.class_has_method(class, access_sel) {
            let access_ivars: bool = msg_send(env, (class, access_sel));
            if access_ivars {
                if let Some((ivar_ptr, encoding)) = env.objc
                    .object_lookup_ivar_with_type(&env.mem, this, &format!("_{key_str}"))
                    .or_else(|| env.objc.object_lookup_ivar_with_type(&env.mem, this, &format!("_is{camel_case_key_string}")))
                    .or_else(|| env.objc.object_lookup_ivar_with_type(&env.mem, this, &format!("{key_str}")))
                    .or_else(|| env.objc.object_lookup_ivar_with_type(&env.mem, this, &format!("is{camel_case_key_string}")))
                {
                    if !encoding.is_empty() {
                        if let Some(boxed) = box_kvc_ivar_value(env, ivar_ptr, &encoding) {
                            return boxed;
                        }
                    }
                    let val: id = env.mem.read(ivar_ptr.cast());
                    return val;
                }
            }
        }
    }

    // 3. Чтение нашего динамического хранилища
    {
        let target_bits = this.to_bits();
        let storage = DYNAMIC_KVC_STORAGE.lock().unwrap();
        for entry in storage.iter() {
            if entry.0 == target_bits && entry.1 == key_str {
                let val: id = crate::mem::Ptr::from_bits(entry.2);
                return val;
            }
        }
    }

    // 4. Fallback (valueForUndefinedKey:)
    if let Some(undef_sel) = env.objc.lookup_selector("valueForUndefinedKey:") {
        if env.objc.object_has_method(&env.mem, this, undef_sel) {
            return msg_send(env, (this, undef_sel, key));
        }
    }

    log!("Warning: valueForKey:{} not found on {:?} — returning nil", key_str, this);
    nil
}

- (id)valueForUndefinedKey:(id)key {
    let key_string = to_rust_string(env, key);
    let class: Class = ObjC::read_isa(this, &env.mem);
    let class_name_string = env.objc.get_class_name(class).to_owned();
    log!("Warning: Object {:?} of class {:?} does not have a getter for {} (valueForUndefinedKey:) — returning nil",
        this, class_name_string, key_string);
    nil
}

- (id)valueForKeyPath:(id)key_path {
    let key_path_string = to_rust_string(env, key_path).into_owned();
    if key_path_string == "@count" {
        return msg![env; this count];
    }

    if let Some((operator, remainder)) = key_path_string.split_once('.') {
        if matches!(operator, "@sum" | "@avg" | "@min" | "@max") {
            let remainder_id = from_rust_string(env, remainder.to_string());
            let values: id = msg![env; this valueForKey:remainder_id];
            let count: NSUInteger = if values == nil {
                0
            } else {
                msg![env; values count]
            };
            let mut numbers = Vec::with_capacity(count as usize);
            for index in 0..count {
                let value: id = msg![env; values objectAtIndex:index];
                if value != nil {
                    numbers.push(msg![env; value doubleValue]);
                }
            }
            let result = match operator {
                "@sum" => numbers.iter().sum::<f64>(),
                "@avg" => numbers
                    .first()
                    .map(|_| numbers.iter().sum::<f64>() / numbers.len() as f64)
                    .unwrap_or(0.0),
                "@min" => numbers.iter().copied().reduce(f64::min).unwrap_or(0.0),
                "@max" => numbers.iter().copied().reduce(f64::max).unwrap_or(0.0),
                _ => unreachable!(),
            };
            return msg_class![env; NSNumber numberWithDouble:result];
        }
    }
    msg![env; this valueForKey:key_path]
}

- (())setValue:(id)value forKeyPath:(id)key_path {
    msg![env; this setValue:value forKey:key_path]
}

// Per Apple's NSKeyValueCoding informal protocol
// (https://developer.apple.com/documentation/objectivec/nsobject/1408301-dictionarywithvaluesforkeys):
// "Returns a dictionary containing the property values identified by each
//  of the keys in a given array. [...] The default implementation invokes
//  -valueForKey: for each key in keys and substitutes NSNull values in the
//  returned dictionary for any returned nil values."
- (id)dictionaryWithValuesForKeys:(id)keys { // NSArray<NSString*> *
    let result: id = msg_class![env; NSMutableDictionary dictionary];
    if keys == nil {
        return result;
    }
    let ns_null: id = msg_class![env; NSNull null];
    // Snapshot the keys first so any KVC side effects can't invalidate the
    // enumeration mid-iteration.
    let key_enum: id = msg![env; keys objectEnumerator];
    let mut key_list: Vec<id> = Vec::new();
    loop {
        let next: id = msg![env; key_enum nextObject];
        if next == nil {
            break;
        }
        retain(env, next);
        key_list.push(next);
    }
    for key in key_list {
        let value: id = msg![env; this valueForKey:key];
        let stored: id = if value == nil { ns_null } else { value };
        () = msg![env; result setObject:stored forKey:key];
        release(env, key);
    }
    result
}

// Per Apple's NSKeyValueCoding informal protocol
// (https://developer.apple.com/documentation/objectivec/nsobject/setvaluesforkeys(_:)):
// "For each key in keyedValues, the corresponding value is set in the
//  receiver by invoking -setValue:forKey:. The default implementation
//  substitutes nil for instances of NSNull."
//
// This is declared on NSObject (the informal protocol), so *any* object —
// not just NSDictionary — must respond to it. Real apps rely on this:
// e.g. Core Data / Unity code sends -setValuesForKeysWithDictionary: to
// plain model objects (NSManagedObjectModel and friends). We must route
// through -setValue:forKey: rather than -setObject:forKey: so KVC-only
// setters and guest overrides are honored. Keys are snapshotted first so
// side effects of the setters can't invalidate enumeration mid-iteration.
- (())setValuesForKeysWithDictionary:(id)keyed_values { // NSDictionary *
    if keyed_values == nil {
        return;
    }
    let key_enum: id = msg![env; keyed_values keyEnumerator];
    if key_enum == nil {
        return;
    }
    let mut keys: Vec<id> = Vec::new();
    loop {
        let next: id = msg![env; key_enum nextObject];
        if next == nil {
            break;
        }
        retain(env, next);
        keys.push(next);
    }
    let ns_null: id = msg_class![env; NSNull null];
    for key in keys {
        let val: id = msg![env; keyed_values objectForKey:key];
        // Per Apple docs, NSNull is treated as nil. -setValue:forKey:'s own
        // contract then handles the nil case (setNilValueForKey: / removal).
        let arg: id = if val == ns_null { nil } else { val };
        () = msg![env; this setValue:arg forKey:key];
        release(env, key);
    }
}

// MARK: - Key-Value Observing (KVO)

- (NSUInteger)version {
    0
}

// - (NSZone *)zone — zones are obsolete; Apple documents that this method
// returns an undefined value on modern runtimes. Return NULL, which is what
// apps treating it as "the default zone" expect.
- (NSZonePtr)zone {
    MutVoidPtr::null()
}

// Per Apple's NSObject documentation, -superclass returns the class object
// for the receiver's class's superclass (nil only for a root class).
- (Class)superclass {
    let class = ObjC::read_isa(this, &env.mem);
    env.objc.get_superclass(class)
}

- (())addObserver:(id)observer forKeyPath:(id)keyPath options:(NSUInteger)options context:(id)context {
    if observer == nil || keyPath == nil {
        log_dbg!("addObserver:forKeyPath:options:context: — observer or keyPath is nil, ignored");
        return;
    }
    let key_str = to_rust_string(env, keyPath);
    log_dbg!(
        "addObserver:{:?} forKeyPath:{:?} options:{} context:{:?}",
        observer, key_str, options, context
    );

    // Store the observer registration in our global KVO storage.
    // Format: (observed_object, observer, keyPath string, options, context)
    KVO_OBSERVERS.lock().unwrap().push((
        this.to_bits(),
        observer.to_bits(),
        key_str.into_owned(),
        options,
        context.to_bits(),
    ));

    // If NSKeyValueObservingOptionInitial is set, deliver an initial
    // notification immediately. Per Apple's KVO documentation, the change
    // dictionary of an initial notification contains the current value
    // under NSKeyValueChangeNewKey if NSKeyValueObservingOptionNew was also
    // requested (and never an old value).
    if options & NSKeyValueObservingOptionInitial != 0 {
        let new_value: id = if options & NSKeyValueObservingOptionNew != 0 {
            msg![env; this valueForKey:keyPath]
        } else {
            nil
        };
        deliver_kvo_notification(env, this, observer.to_bits(), keyPath, options & !NSKeyValueObservingOptionOld, context.to_bits(), nil, new_value);
    }
}

- (())removeObserver:(id)observer forKeyPath:(id)keyPath {
    if observer == nil || keyPath == nil {
        return;
    }
    let key_str = to_rust_string(env, keyPath);
    log_dbg!(
        "removeObserver:{:?} forKeyPath:{:?}",
        observer, key_str
    );
    KVO_OBSERVERS.lock().unwrap().retain(|entry| {
        !(entry.0 == this.to_bits()
            && entry.1 == observer.to_bits()
            && entry.2 == key_str.as_ref())
    });
}

- (())removeObserver:(id)observer forKeyPath:(id)keyPath context:(id)_context {
    () = msg![env; this removeObserver:observer forKeyPath:keyPath];
}

- (())willChangeValueForKey:(id)key {
    if key == nil { return; }
    let key_str = to_rust_string(env, key).into_owned();

    // Snapshot the old value, but only if some observer asked for it
    // (NSKeyValueObservingOptionOld) — valueForKey: can be expensive.
    let wants_old = KVO_OBSERVERS.lock().unwrap().iter().any(|entry| {
        entry.0 == this.to_bits()
            && entry.2 == key_str
            && (entry.3 & NSKeyValueObservingOptionOld) != 0
    });
    if !wants_old {
        return;
    }

    let old_value: id = msg![env; this valueForKey:key];
    if old_value != nil {
        retain(env, old_value);
    }
    PENDING_KVO_OLD_VALUES
        .lock()
        .unwrap()
        .push((this.to_bits(), key_str, old_value.to_bits()));
}

- (())didChangeValueForKey:(id)key {
    if key == nil { return; }
    let key_str = to_rust_string(env, key).into_owned();

    // Collect matching observers
    let observers: Vec<(u32, u32, u32)> = {
        KVO_OBSERVERS.lock().unwrap().iter()
            .filter(|entry| entry.0 == this.to_bits() && entry.2 == key_str)
            .map(|entry| (entry.1, entry.3, entry.4))
            .collect()
    };

    // Pop the old-value snapshot (if any) taken by willChangeValueForKey:.
    let old_value: id = {
        let mut pending = PENDING_KVO_OLD_VALUES.lock().unwrap();
        pending
            .iter()
            .rposition(|entry| entry.0 == this.to_bits() && entry.1 == key_str)
            .map(|pos| crate::mem::Ptr::from_bits(pending.remove(pos).2))
            .unwrap_or(nil)
    };

    if !observers.is_empty() {
        let needs_new = observers
            .iter()
            .any(|&(_, options, _)| options & NSKeyValueObservingOptionNew != 0);
        let new_value: id = if needs_new {
            msg![env; this valueForKey:key]
        } else {
            nil
        };

        for (observer_bits, options, context_bits) in observers {
            deliver_kvo_notification(env, this, observer_bits, key, options, context_bits, old_value, new_value);
        }
    }

    // Balance the retain made in willChangeValueForKey:.
    if old_value != nil {
        release(env, old_value);
    }
}

- (())observeValueForKeyPath:(id)_keyPath ofObject:(id)_object change:(id)_change context:(id)_context {
    // Default implementation does nothing — subclasses override this.
}

// =========================================================================
// Fallback UIView-like methods on NSObject for compatibility with game
// engines (e.g. Digital Chocolate) that send view-hierarchy messages to
// objects of the wrong type due to corrupted pointers. Returning an empty
// array / nil prevents infinite loops and log spam without crashing.
// =========================================================================
- (id)subviews {
    // Return an empty NSArray so iteration loops terminate immediately.
    msg_class![env; NSArray array]
}

- (id)superview {
    nil
}

@end

// `_touchHLE_NSBlock` is the ObjC class used as the `isa` of guest block
// objects (`__NSConcreteStackBlock` / `__NSConcreteGlobalBlock` /
// `__NSConcreteMallocBlock`; see `dyld::do_non_lazy_linking`). Blocks are
// allocated by guest code (block literals) and are NOT touchHLE host objects,
// so they must NOT use the host-object refcount table. Instead their memory
// management routes to the Blocks runtime in `libc::blocks`. Without this,
// `[someBlock copy]` / `[someBlock retain]` (used pervasively by cocos2d v3's
// scheduler and CCBReader to store callback blocks) would hit a dummy class
// with no methods and return nil, leaving e.g. `CCTimer._block` nil and
// crashing the scheduler when it later invokes the (nil/garbage) callback.
@implementation _touchHLE_NSBlock: NSObject

- (id)retain {
    let blk = crate::mem::ConstVoidPtr::from_bits(this.to_bits());
    let copied = crate::libc::blocks::_Block_copy(env, blk);
    crate::mem::Ptr::from_bits(copied.to_bits())
}

- (id)copy {
    msg![env; this copyWithZone:(MutVoidPtr::null())]
}

- (id)copyWithZone:(NSZonePtr)_zone {
    let blk = crate::mem::ConstVoidPtr::from_bits(this.to_bits());
    let copied = crate::libc::blocks::_Block_copy(env, blk);
    crate::mem::Ptr::from_bits(copied.to_bits())
}

- (())release {
    let blk = crate::mem::ConstVoidPtr::from_bits(this.to_bits());
    crate::libc::blocks::_Block_release(env, blk);
}

- (id)autorelease {
    // Blocks that outlive the current scope are retained via copy by their
    // owner, so a no-op autorelease is safe here and avoids routing a
    // non-host-object block through the autorelease pool's release path.
    this
}

// `-[NSBlock invoke]` calls a block that takes no arguments, which lets code
// run a block through `performSelector:@selector(invoke)` or NSInvocation
// (Crashlytics does). The invoke pointer is the fourth word of the literal.
// https://clang.llvm.org/docs/Block-ABI-Apple.html
- (())invoke {
    let invoke: u32 = env.mem.read(this.cast::<u32>() + 3u32);
    let invoke = GuestFunction::from_addr_with_thumb_bit(invoke);
    let (): () = invoke.call_from_host(env, (this,));
}

@end

};

#[cfg(test)]
mod kvc_type_encoding_tests {
    use super::kvc_helpers::{objc_argument_encoding, objc_argument_type};

    #[test]
    fn reads_scalar_setter_argument_type() {
        assert_eq!(objc_argument_type(b"v12@0:4f8", 2), Some(b'f'));
        assert_eq!(objc_argument_type(b"v24@0:4f8f12", 3), Some(b'f'));
    }

    #[test]
    fn skips_compound_return_and_quoted_arguments() {
        assert_eq!(
            objc_argument_type(b"{CGRect={CGPoint=ff}{CGSize=ff}}16@0:4@\"NSNumber\"8", 2),
            Some(b'@')
        );
    }

    #[test]
    fn reads_struct_setter_argument_encoding() {
        assert_eq!(
            objc_argument_encoding(b"v16@0:4{CGPoint=ff}8", 2),
            Some(&b"{CGPoint=ff}"[..])
        );
        assert_eq!(
            objc_argument_encoding(b"v20@0:4{CGRect={CGPoint=ff}{CGSize=ff}}8", 2),
            Some(&b"{CGRect={CGPoint=ff}{CGSize=ff}}"[..])
        );
    }

    #[test]
    fn rejects_incomplete_method_encodings() {
        assert_eq!(objc_argument_type(b"v12@0", 2), None);
    }
}
