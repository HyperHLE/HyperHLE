/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `NSInvocation` and `NSMethodSignature`.

use crate::frameworks::foundation::{NSInteger, NSUInteger};
use crate::mem::MutVoidPtr;
use crate::objc::{
    autorelease, id, msg, msg_class, nil, objc_classes, release, retain, ClassExports, HostObject,
    NSZonePtr, SEL,
};
use std::collections::HashMap;

// =========================================================================
// MARK: - NSMethodSignature Host Object
// =========================================================================

struct NSMethodSignatureHostObject {
    // ИСПРАВЛЕНИЕ 1: Добавили поле в структуру
    number_of_arguments: NSUInteger,
}
impl HostObject for NSMethodSignatureHostObject {}

// =========================================================================
// MARK: - NSInvocation Host Object
// =========================================================================

struct NSInvocationHostObject {
    signature: id,
    target: id,
    selector: Option<SEL>,
    arguments: HashMap<NSInteger, u32>,
    arguments_retained: bool,
}
impl HostObject for NSInvocationHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

// =========================================================================
// MARK: - NSMethodSignature
// =========================================================================

@implementation NSMethodSignature: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(NSMethodSignatureHostObject {
        number_of_arguments: 2, // По умолчанию 2 аргумента (self, _cmd)
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

+ (id)signatureWithObjCTypes:(MutVoidPtr)_types {
    let sig: id = msg_class![env; NSMethodSignature alloc];
    let sig: id = msg![env; sig init];
    autorelease(env, sig)
}

- (id)init {
    this
}

- (NSUInteger)numberOfArguments {
    env.objc.borrow::<NSMethodSignatureHostObject>(this).number_of_arguments
}

// Внутренний метод для touchHLE, чтобы мы могли задать количество аргументов 
// при создании сигнатуры из ns_object.rs
- (())_touchHLE_setNumberOfArguments:(NSUInteger)count {
    env.objc.borrow_mut::<NSMethodSignatureHostObject>(this).number_of_arguments = count;
}
    
// ИСПРАВЛЕНИЕ 2: Вернули правильное выделение памяти без alloc_bytes
- (crate::mem::ConstPtr<std::ffi::c_char>)methodReturnType {
    log!("Warning: stubbed NSMethodSignature methodReturnType — returning 'v' (void)");
    let ptr: crate::mem::MutPtr<u16> = env.mem.alloc(2).cast();
    env.mem.write(ptr, 0x0076);
    ptr.cast_const().cast()
}

// ИСПРАВЛЕНИЕ 2: Вернули правильное выделение памяти без alloc_bytes
- (crate::mem::ConstPtr<std::ffi::c_char>)getArgumentTypeAtIndex:(NSUInteger)_index {
    log!("Warning: stubbed NSMethodSignature getArgumentTypeAtIndex: — returning '@' (id)");
    let ptr: crate::mem::MutPtr<u16> = env.mem.alloc(2).cast();
    env.mem.write(ptr, 0x0040);
    ptr.cast_const().cast()
}

- (NSUInteger)methodReturnLength {
    let ret_type_ptr: crate::mem::ConstPtr<std::ffi::c_char> = msg![env; this methodReturnType];
    if ret_type_ptr.is_null() {
        return 0;
    }
    
    // Читаем строку типа из памяти и приводим указатель к нужному типу
    let ret_type_str = env.mem.cstr_at_utf8(ret_type_ptr.cast()).unwrap_or("");
    
    // Пропускаем спецификаторы Objective-C (in, out, inout, const, oneway и т.д.)
    let core_type = ret_type_str.trim_start_matches(|c| "rnNoORV".contains(c));
    
    // Честная калькуляция размера типа (в байтах) для 32-битного ARM
    match core_type.chars().next() {
        Some('v') => 0, // void
        Some('c') | Some('C') | Some('B') => 1, // char, unsigned char, bool
        Some('s') | Some('S') => 2, // short, unsigned short
        Some('i') | Some('I') | Some('l') | Some('L') | Some('f') => 4, // int, long, float
        Some('q') | Some('Q') | Some('d') => 8, // long long, unsigned long long, double
        Some('@') | Some('#') | Some('*') | Some('^') | Some(':') | Some('?') => 4, // объекты, классы, указатели, SEL
        Some('{') => {
            // Для структур нужен парсинг вложенных типов, пока возвращаем 0, чтобы не упало
            log!("Warning: methodReturnLength for struct {} is not fully calculated, returning 0", core_type);
            0
        }
        _ => {
            log!("Warning: methodReturnLength unknown type '{}', returning default 4", core_type);
            4
        }
    }
}
    
// -----------------------------------------------------------------------

- (())dealloc {
    env.objc.dealloc_object(this, &mut env.mem)
}

@end

// =========================================================================
// MARK: - NSInvocation
// =========================================================================

@implementation NSInvocation: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(NSInvocationHostObject {
        signature: nil,
        target: nil,
        selector: None,
        arguments: HashMap::new(),
        arguments_retained: false,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

+ (id)invocationWithMethodSignature:(id)sig {
    let inv: id = msg_class![env; NSInvocation alloc];
    let inv: id = msg![env; inv init];
    
    env.objc.borrow_mut::<NSInvocationHostObject>(inv).signature = sig;
    retain(env, sig);
    autorelease(env, inv)
}

- (id)init {
    this
}

- (())dealloc {
    let (target, sig, retained) = {
        let host = env.objc.borrow::<NSInvocationHostObject>(this);
        (host.target, host.signature, host.arguments_retained)
    };
    
    if retained && target != nil {
        release(env, target);
    }
    if sig != nil {
        release(env, sig);
    }
    
    env.objc.dealloc_object(this, &mut env.mem)
}

- (id)methodSignature {
    env.objc.borrow::<NSInvocationHostObject>(this).signature
}

- (())retainArguments {
    let (retained, target) = {
        let host = env.objc.borrow::<NSInvocationHostObject>(this);
        (host.arguments_retained, host.target)
    };
    
    if !retained {
        env.objc.borrow_mut::<NSInvocationHostObject>(this).arguments_retained = true;
        if target != nil {
            retain(env, target);
        }
    }
}

- (bool)argumentsRetained {
    env.objc.borrow::<NSInvocationHostObject>(this).arguments_retained
}

- (id)target {
    env.objc.borrow::<NSInvocationHostObject>(this).target
}

- (())setTarget:(id)target {
    env.objc.borrow_mut::<NSInvocationHostObject>(this).target = target;
}

- (SEL)selector {
    env.objc.borrow::<NSInvocationHostObject>(this).selector.expect("NSInvocation selector not set")
}

- (())setSelector:(SEL)selector {
    env.objc.borrow_mut::<NSInvocationHostObject>(this).selector = Some(selector);
}

- (())getArgument:(MutVoidPtr)buffer atIndex:(NSInteger)index {
    let val = env.objc.borrow::<NSInvocationHostObject>(this).arguments.get(&index).copied().unwrap_or(0);
    env.mem.write(buffer.cast::<u32>(), val);
}

- (())setArgument:(MutVoidPtr)buffer atIndex:(NSInteger)index {
    let val = env.mem.read(buffer.cast::<u32>());
    env.objc.borrow_mut::<NSInvocationHostObject>(this).arguments.insert(index, val);
}

- (())invoke {
    let target = env.objc.borrow::<NSInvocationHostObject>(this).target;
    if target != nil {
        () = msg![env; this invokeWithTarget:target];
    }
}

- (())invokeWithTarget:(id)target {
    let sel = env.objc.borrow::<NSInvocationHostObject>(this).selector.expect("NSInvocation invoked without a selector");
    let args = env.objc.borrow::<NSInvocationHostObject>(this).arguments.clone();
    let sel_str = sel.as_str(&env.mem);
    let arg_count = sel_str.chars().filter(|&c| c == ':').count();
    
    // В Objective-C индексы 0 и 1 заняты под `self` и `_cmd`.
    // Аргументы пользователя начинаются с индекса 2.
    if arg_count == 0 {
        let _: u32 = crate::objc::msg_send_no_type_checking(env, (target, sel));
    } else if arg_count == 1 {
        let arg2 = args.get(&2).copied().unwrap_or(0);
        let _: u32 = crate::objc::msg_send_no_type_checking(env, (target, sel, arg2));
    } else if arg_count == 2 {
        let arg2 = args.get(&2).copied().unwrap_or(0);
        let arg3 = args.get(&3).copied().unwrap_or(0);
        let _: u32 = crate::objc::msg_send_no_type_checking(env, (target, sel, arg2, arg3));
    } else if arg_count == 3 {
        let arg2 = args.get(&2).copied().unwrap_or(0);
        let arg3 = args.get(&3).copied().unwrap_or(0);
        let arg4 = args.get(&4).copied().unwrap_or(0);
        let _: u32 = crate::objc::msg_send_no_type_checking(env, (target, sel, arg2, arg3, arg4));
    } else {
        log!("TODO: NSInvocation invokeWithTarget: {} with > 3 arguments is not fully supported yet", sel_str);
    }
}

@end

};
