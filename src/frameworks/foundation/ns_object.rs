/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 * If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//!
//! `NSObject`, the root of most class hierarchies in Objective-C.

use super::ns_dictionary::dict_from_keys_and_objects;
use super::ns_run_loop::NSDefaultRunLoopMode;
use super::ns_string::{from_rust_string, get_static_str, to_rust_string};
use super::{NSTimeInterval, NSUInteger};
// ДОБАВЛЕНЫ ИМПОРТЫ ДЛЯ ЭКСПОРТА ФУНКЦИИ И ОКРУЖЕНИЯ
use crate::dyld::{export_c_func, FunctionExports};
use crate::Environment;
use crate::frameworks::foundation::ns_thread::detach_new_thread_inner;
use crate::mem::MutVoidPtr;
use crate::objc::{
    autorelease, id, msg, msg_class, msg_send, msg_send_no_type_checking, nil, objc_classes,
    retain, Class, ClassExports, NSZonePtr, ObjC, TrivialHostObject, SEL,
};

// Хранилище для отмененных таймеров (target, имя селектора в виде строки)
pub static mut CANCELLED_PERFORMS: std::vec::Vec<(u32, std::option::Option<std::string::String>)> = std::vec::Vec::new();

// ДОБАВЛЕНА РЕАЛИЗАЦИЯ NSAllocateObject
fn NSAllocateObject(
    env: &mut Environment,
    class: Class,
    extra_bytes: NSUInteger,
    _zone: NSZonePtr,
) -> id {
    if extra_bytes > 0 {
        log!("Warning: NSAllocateObject called with extra_bytes={}, which is currently unhandled!", extra_bytes);
    }
    
    // Перенаправляем вызов в стандартный метод alloc данного класса
    msg![env; class alloc]
}

// ДОБАВЛЕН ЭКСПОРТ ФУНКЦИЙ ДЛЯ ДИНАМИЧЕСКОГО ЛИНКЕРА
pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(NSAllocateObject(_, _, _)),
];

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation NSObject

+ (id)alloc {
    msg![env; this allocWithZone:(MutVoidPtr::null())]
}
+ (id)allocWithZone:(NSZonePtr)_zone { 
    log_dbg!("[{:?} allocWithZone:]", this);
    env.objc.alloc_object(this, Box::new(TrivialHostObject), &mut env.mem)
}

+ (id)new {
    let new_object: id = msg![env; this alloc];
    msg![env; new_object init]
}

+ (Class)class {
    this
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

+ (bool)instancesRespondToSelector:(SEL)selector {
    env.objc.class_has_method(this, selector)
}

// ИЗМЕНЕНО: Ищем _objc_msgSend через create_proc_address (без логов)
+ (u32)instanceMethodForSelector:(SEL)selector {
    // Разделяем заимствования (borrows) чтобы компилятор Rust был счастлив
    let dyld = &mut env.dyld;
    let mem = &mut env.mem;
    let cpu = &mut env.cpu;
    
    match dyld.create_proc_address(mem, cpu, "_objc_msgSend") {
        Ok(guest_func) => guest_func.addr_with_thumb_bit(),
        Err(_) => {
            log!("Error: _objc_msgSend not found! Returning dummy IMP.");
            let ptr: crate::mem::MutPtr<u16> = mem.alloc(2).cast();
            mem.write(ptr, 0x4770);
            ptr.to_bits() | 1
        }
    }
}

+ (id)instanceMethodSignatureForSelector:(SEL)selector {
    let sig: id = msg_class![env; NSMethodSignature signatureWithObjCTypes:(MutVoidPtr::null())];
    
    let sel_str = selector.as_str(&env.mem);
    let explicit_args = sel_str.chars().filter(|&c| c == ':').count() as NSUInteger;
    
    let total_args = explicit_args + 2;
    () = msg![env; sig _touchHLE_setNumberOfArguments:total_args];
    
    sig
}

+ (bool)accessInstanceVariablesDirectly {
    true
}

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
                                       object:(id)object {
    let sel_str = selector.as_str(&env.mem).to_string();
    unsafe {
        crate::frameworks::foundation::ns_object::CANCELLED_PERFORMS.push((target.to_bits(), Some(sel_str)));
    }
}

+ (())cancelPreviousPerformRequestsWithTarget:(id)target {
    unsafe {
        crate::frameworks::foundation::ns_object::CANCELLED_PERFORMS.push((target.to_bits(), None));
    }
}

- (id)init {
    this
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
    let key_string = to_rust_string(env, key);
    assert!(key_string.is_ascii()); 
    let camel_case_key_string = format!("{}{}", key_string.as_bytes()[0].to_ascii_uppercase() as char, &key_string[1..]);

    let class = msg![env; this class];

    assert!(value != nil);
    let value_class = msg![env; value class];
    let ns_value_class = env.objc.get_known_class("NSValue", &mut env.mem);
    assert!(!env.objc.class_is_subclass_of(value_class, ns_value_class));

    if let Some(sel) = env.objc.lookup_selector(&format!("set{camel_case_key_string}:")) {
        if env.objc.class_has_method(class, sel) {
            () = msg_send(env, (this, sel, value));
            return;
        }
    }

    if let Some(sel) = env.objc.lookup_selector(&format!("_set{camel_case_key_string}:")) {
        if env.objc.class_has_method(class, sel) {
            () = msg_send(env, (this, sel, value));
            return;
        }
    }

    let sel = env.objc.lookup_selector("accessInstanceVariablesDirectly").unwrap();
    let accessInstanceVariablesDirectly = msg_send(env, (class, sel));

    if accessInstanceVariablesDirectly {
        if let Some(ivar_ptr) = env.objc.object_lookup_ivar(&env.mem, this, &format!("_{key_string}"))
            .or_else(|| env.objc.object_lookup_ivar(&env.mem, this, &format!("_is{camel_case_key_string}")))
            .or_else(|| env.objc.object_lookup_ivar(&env.mem, this, &format!("{key_string}")))
            .or_else(|| env.objc.object_lookup_ivar(&env.mem, this, &format!("is{camel_case_key_string}"))
        ) {
            retain(env, value);
            env.mem.write(ivar_ptr.cast(), value);
            return;
        }
    }

    let sel = env.objc.lookup_selector("setValue:forUndefinedKey:").unwrap();
    () = msg_send(env, (this, sel, value, key));
}

- (())setValue:(id)_value forUndefinedKey:(id)key { 
    let class: Class = ObjC::read_isa(this, &env.mem);
    let class_name_string = env.objc.get_class_name(class).to_owned();
    let key_string = to_rust_string(env, key);
    log!("Warning: Object {:?} of class {:?} does not have a setter for {} — ignoring",
        this, class_name_string, key_string);
}

- (bool)respondsToSelector:(SEL)selector {
    env.objc.object_has_method(&env.mem, this, selector)
}

- (bool)conformsToProtocol:(id)_protocol {
    true
}
    
// ИЗМЕНЕНО: Ищем _objc_msgSend через create_proc_address (без логов)
- (u32)methodForSelector:(SEL)selector {
    // Разделяем заимствования (borrows) чтобы компилятор Rust был счастлив
    let dyld = &mut env.dyld;
    let mem = &mut env.mem;
    let cpu = &mut env.cpu;
    
    match dyld.create_proc_address(mem, cpu, "_objc_msgSend") {
        Ok(guest_func) => guest_func.addr_with_thumb_bit(),
        Err(_) => {
            log!("Error: _objc_msgSend not found! Returning dummy IMP.");
            let ptr: crate::mem::MutPtr<u16> = mem.alloc(2).cast();
            mem.write(ptr, 0x4770);
            ptr.to_bits() | 1
        }
    }
}

- (id)methodSignatureForSelector:(SEL)selector {
    let sig: id = msg_class![env; NSMethodSignature signatureWithObjCTypes:(MutVoidPtr::null())];
    
    let sel_str = selector.as_str(&env.mem);
    let explicit_args = sel_str.chars().filter(|&c| c == ':').count() as NSUInteger;
    
    let total_args = explicit_args + 2;
    () = msg![env; sig _touchHLE_setNumberOfArguments:total_args];
    
    sig
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
                              repeats:false];
    
    let run_loop: id = msg_class![env; NSRunLoop mainRunLoop];
    let mode: id = get_static_str(env, NSDefaultRunLoopMode);
    () = msg![env; run_loop addTimer:timer forMode:mode];
}

- (())performSelectorOnMainThread:(SEL)sel withObject:(id)arg waitUntilDone:(bool)wait {
    log_dbg!("performSelectorOnMainThread:{} withObject:{:?} waitUntilDone:{}", sel.as_str(&env.mem), arg, wait);

    if wait && env.current_thread == 0 {
        if sel.as_str(&env.mem).ends_with(':') {
            () = msg_send(env, (this, sel, arg));
        } else {
            assert!(arg.is_null());
            () = msg_send(env, (this, sel));
        }
        return;
    }

    if wait {
        log!("Warning: performSelectorOnMainThread:{} waitUntilDone:YES from background thread — wait not supported, scheduling without waiting", sel.as_str(&env.mem));
    }

    msg![env; this performSelector:sel withObject:arg afterDelay:0.0]
}

- (())_touchHLE_timerFireMethod:(id)which { 
    let dict: id = msg![env; which userInfo];
    let sel_key: id = get_static_str(env, "SEL");
    let sel_str_id: id = msg![env; dict objectForKey:sel_key];
    let sel_str = to_rust_string(env, sel_str_id);
    let sel = env.objc.lookup_selector(&sel_str).unwrap();

    let arg_key: id = get_static_str(env, "arg");
    let arg: id = msg![env; dict objectForKey:arg_key];

    let target_bits = this.to_bits();
    let mut cancelled = false;
    
    unsafe {
        if let Some(pos) = crate::frameworks::foundation::ns_object::CANCELLED_PERFORMS.iter().position(|x| x.0 == target_bits && x.1.as_deref() == Some(sel_str.as_ref())) {
            crate::frameworks::foundation::ns_object::CANCELLED_PERFORMS.remove(pos);
            cancelled = true;
        } else if let Some(_) = crate::frameworks::foundation::ns_object::CANCELLED_PERFORMS.iter().position(|x| x.0 == target_bits && x.1.is_none()) {
            cancelled = true;
        }
    }

    if cancelled {
        return;
    }

    if sel.as_str(&env.mem).ends_with(':') {
        () = msg_send(env, (this, sel, arg));
    } else {
        () = msg_send(env, (this, sel));
    }
}

- (())awakeFromNib {
}

- (())performSelector:(SEL)sel onThread:(id)_thread withObject:(id)arg waitUntilDone:(bool)_wait {
    log_dbg!("performSelector:{} onThread:withObject:waitUntilDone: — scheduling on main thread instead", sel.as_str(&env.mem));
    msg![env; this performSelector:sel withObject:arg afterDelay:0.0]
}

- (())performSelector:(SEL)sel onThread:(id)_thread withObject:(id)arg waitUntilDone:(bool)_wait modes:(id)_modes {
    log_dbg!("performSelector:{} onThread:withObject:waitUntilDone:modes: — scheduling on main thread instead", sel.as_str(&env.mem));
    msg![env; this performSelector:sel withObject:arg afterDelay:0.0]
}

- (id)valueForKey:(id)key {
    let key_str = super::ns_string::to_rust_string(env, key);
    let sel_name = key_str.to_string();
    if let Some(sel) = env.objc.lookup_selector(&sel_name) {
        if env.objc.object_has_method(&env.mem, this, sel) {
            return msg_send(env, (this, sel));
        }
    }
    let is_sel_name = format!("is{}{}", &key_str[..1].to_uppercase(), &key_str[1..]);
    if let Some(sel) = env.objc.lookup_selector(&is_sel_name) {
        if env.objc.object_has_method(&env.mem, this, sel) {
            return msg_send(env, (this, sel));
        }
    }
    log!("Warning: valueForKey:{} not found on {:?} — returning nil", key_str, this);
    nil
}

- (id)valueForKeyPath:(id)key_path {
    msg![env; this valueForKey:key_path]
}

- (())setValue:(id)value forKeyPath:(id)key_path {
    msg![env; this setValue:value forKey:key_path]
}

// MARK: - Key-Value Observing (KVO)

- (())willChangeValueForKey:(id)_key {
    // Базовая реализация NSObject: если нет активных наблюдателей, 
    // методы willChange и didChange ничего не делают.
}

- (())didChangeValueForKey:(id)_key {
}

- (())self {

}

- (())superclass {

}

- (())addObserver:(id)_observer forKeyPath:(id)_keyPath options:(NSUInteger)_options context:(id)_context {
    log!("Warning: NSObject addObserver:forKeyPath:options:context: is stubbed");
}

- (())removeObserver:(id)_observer forKeyPath:(id)_keyPath {
    log!("Warning: NSObject removeObserver:forKeyPath: is stubbed");
}

@end

};

