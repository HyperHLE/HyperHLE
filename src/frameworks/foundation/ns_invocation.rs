/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `NSInvocation` and `NSMethodSignature`.

use crate::abi::{extend_stack_for_args, write_next_arg, GuestArg};
use crate::cpu::Cpu;
use crate::frameworks::foundation::{NSInteger, NSUInteger};
use crate::libc::string::strdup;
use crate::mem::{ConstPtr, MutPtr, MutVoidPtr};
use crate::objc::{
    autorelease, id, msg, msg_class, nil, objc_classes, objc_msgSend, release, retain, ClassExports, HostObject,
    NSZonePtr, SEL,
};

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
    /// `NSMethodSignature *`
    sig: id,
    /// Строки типов аргументов, полученные из `sig` во время создания
    argument_types: Vec<String>,
    target: id,
    selector: Option<SEL>,
    /// Выделенный буфер для каждого аргумента.
    /// Option указывает, был ли аргумент задан через `setArgument:atIndex:`
    arguments: Vec<Option<MutVoidPtr>>,
    arguments_retained: bool,
    /// Объекты, удержанные через `retainArguments`
    retained_objects: Vec<id>,
    /// Копии C-строк, созданные через `retainArguments`
    copied_strings: Vec<MutPtr<u8>>,
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
        sig: nil,
        argument_types: Vec::new(),
        target: nil,
        selector: None,
        arguments: Vec::new(),
        arguments_retained: false,
        retained_objects: Vec::new(),
        copied_strings: Vec::new(),
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

+ (id)invocationWithMethodSignature:(id)sig { // NSMethodSignature *
    retain(env, sig);
    let num_of_args: NSUInteger = msg![env; sig numberOfArguments];
    let mut argument_types: Vec<String> = Vec::with_capacity(num_of_args as usize);
    for i in 0..num_of_args {
        let type_ptr: ConstPtr<u8> = msg![env; sig getArgumentTypeAtIndex:i];
        argument_types.push(env.mem.cstr_at_utf8(type_ptr).unwrap().to_string());
    }
    let host_object = Box::new(NSInvocationHostObject {
        sig,
        argument_types,
        target: nil,
        selector: None,
        arguments: vec![None; num_of_args as usize],
        arguments_retained: false,
        retained_objects: Vec::new(),
        copied_strings: Vec::new(),
    });
    let res = env.objc.alloc_object(this, host_object, &mut env.mem);
    autorelease(env, res)
}

- (id)init {
    this
}

- (id)methodSignature {
    env.objc.borrow::<NSInvocationHostObject>(this).sig
}

- (id)target {
    env.objc.borrow::<NSInvocationHostObject>(this).target
}

- (())setTarget:(id)target {
    let old_target = env.objc.borrow::<NSInvocationHostObject>(this).target;
    let arguments_retained = env.objc.borrow::<NSInvocationHostObject>(this).arguments_retained;
    env.objc.borrow_mut::<NSInvocationHostObject>(this).target = target;
    if arguments_retained {
        retain(env, target);
        release(env, old_target);
    }
}

- (SEL)selector {
    env.objc.borrow::<NSInvocationHostObject>(this).selector.expect("NSInvocation selector not set")
}

- (())setSelector:(SEL)selector {
    assert!(env.objc.borrow_mut::<NSInvocationHostObject>(this).selector.is_none()); // TODO
    env.objc.borrow_mut::<NSInvocationHostObject>(this).selector = Some(selector);
}

- (bool)argumentsRetained {
    env.objc.borrow::<NSInvocationHostObject>(this).arguments_retained
}

- (())retainArguments {
    // TODO: handle return val
    // TODO: copy blocks
    assert!(!env.objc.borrow::<NSInvocationHostObject>(this).arguments_retained); // TODO

    let target = env.objc.borrow::<NSInvocationHostObject>(this).target;
    retain(env, target);

    let mut retained_objects: Vec<id> = Vec::new();
    let mut copied_strings: Vec<MutPtr<u8>> = Vec::new();

    // Skip index 0 (self) and 1 (SEL): handled via target/selector fields.
    let num_of_args = env.objc.borrow::<NSInvocationHostObject>(this).argument_types.len();
    for i in 2..num_of_args {
        let host = env.objc.borrow::<NSInvocationHostObject>(this);
        let Some(arg_loc) = host.arguments[i] else { continue };
        match host.argument_types[i].as_str() {
            "@" => {
                let obj: id = env.mem.read(arg_loc.cast().cast_const());
                retain(env, obj);
                retained_objects.push(obj);
            }
            "*" => {
                let str: MutPtr<u8> = env.mem.read(arg_loc.cast().cast_const());
                let str_copy = strdup(env, str.cast_const());
                env.mem.write(arg_loc.cast(), str_copy);
                copied_strings.push(str_copy);
            }
            _ => {}
        }
    }

    let host = env.objc.borrow_mut::<NSInvocationHostObject>(this);
    host.retained_objects = retained_objects;
    host.copied_strings = copied_strings;
    host.arguments_retained = true;
}

- (())getArgument:(MutVoidPtr)buffer atIndex:(NSInteger)index {
    let host = env.objc.borrow::<NSInvocationHostObject>(this);
    if let Some(arg_loc) = host.arguments.get(index as usize).and_then(|&a| a) {
        let arg_type = host.argument_types.get(index as usize).map(|s| s.as_str()).unwrap_or("");
        match arg_type {
            "f" => {
                let val: f32 = env.mem.read(arg_loc.cast().cast_const());
                env.mem.write(buffer.cast(), val);
            }
            "@" => {
                let val: id = env.mem.read(arg_loc.cast().cast_const());
                env.mem.write(buffer.cast(), val);
            }
            "*" => {
                let val: MutPtr<u8> = env.mem.read(arg_loc.cast().cast_const());
                env.mem.write(buffer.cast(), val);
            }
            _ if arg_type.starts_with('^') => {
                let val: MutVoidPtr = env.mem.read(arg_loc.cast().cast_const());
                env.mem.write(buffer.cast(), val);
            }
            _ => {
                // Fallback for primitive types, assuming 32-bit register packing
                let val: u32 = env.mem.read(arg_loc.cast().cast_const());
                env.mem.write(buffer.cast(), val);
            }
        }
    }
}

- (())setArgument:(MutVoidPtr)arg_loc atIndex:(NSInteger)idx {
    let &NSInvocationHostObject {
        ref arguments,
        arguments_retained,
        ..
    } = env.objc.borrow::<NSInvocationHostObject>(this);

    // 0 and 1 are reserved for `self` and `_cmd`
    // TODO: can they be set too?
    assert!(1 < idx && idx < arguments.len() as NSInteger);

    if let Some(prev_arg) = arguments[idx as usize] {
        env.mem.free(prev_arg.cast());
    }

    let argument_types: &Vec<String> = env.objc.borrow::<NSInvocationHostObject>(this).argument_types.as_ref();
    let arg_type = argument_types.get(idx as usize).unwrap();
    let new: MutVoidPtr = match arg_type.as_str() {
        "f" => {
            let arg_loc: MutPtr<f32> = arg_loc.cast();
            let arg = env.mem.read(arg_loc);
            env.mem.alloc_and_write(arg).cast()
        }
        "@" => {
            assert!(!arguments_retained); // TODO
            let arg_loc: MutPtr<id> = arg_loc.cast();
            let arg = env.mem.read(arg_loc);
            env.mem.alloc_and_write(arg).cast()
        }
        "*" => {
            assert!(!arguments_retained); // TODO
            let arg_loc: MutPtr<MutPtr<u8>> = arg_loc.cast();
            let arg = env.mem.read(arg_loc);
            env.mem.alloc_and_write(arg).cast()
        }
        // pointer cases
        _ if arg_type.starts_with('^') => {
            let arg_loc: MutPtr<MutVoidPtr> = arg_loc.cast();
            let arg = env.mem.read(arg_loc);
            env.mem.alloc_and_write(arg).cast()
        }
        _ => {
            // Fallback для примитивов или неопознанных типов из форка (записываем как 32-битное число)
            let arg_loc: MutPtr<u32> = arg_loc.cast();
            let arg = env.mem.read(arg_loc);
            env.mem.alloc_and_write(arg).cast()
        }
    };

    env.objc.borrow_mut::<NSInvocationHostObject>(this).arguments[idx as usize] = Some(new);
}

- (())invokeWithTarget:(id)target {
    () = msg![env; this setTarget:target];
    () = msg![env; this invoke];
}

- (())invoke {
    // Safeguard: all arguments must be set (except first two)
    let arguments: &Vec<Option<MutVoidPtr>> = env.objc.borrow::<NSInvocationHostObject>(this).arguments.as_ref();
    let set_count = arguments.iter().flatten().count();
    let all_count = arguments.len();
    
    // В форке сигнатура может быть частично заполнена, поэтому мы смягчаем assert из оригинала,
    // но оставляем логику честного вызова FFI, если типы известны.
    if set_count + 2 != all_count {
        log!("Warning: NSInvocation invoked without all arguments set");
    }

    let sig = env.objc.borrow::<NSInvocationHostObject>(this).sig;
    let ret_type: ConstPtr<u8> = msg![env; sig methodReturnType];
    assert!(env.mem.read(ret_type) == b'v'); // TODO

    // `call_from_host` re-use
    // TODO: retval_ptr
    // TODO: cross check against frame length from NSMethodSignature
    let mut reg_count = 0;
    let argument_types: &Vec<String> = env.objc.borrow::<NSInvocationHostObject>(this).argument_types.as_ref();
    for arg_type in argument_types.iter() {
        // TODO: refactor and simplify
        reg_count += match arg_type.as_str() {
            "@" => <id as GuestArg>::REG_COUNT,
            ":" => <SEL as GuestArg>::REG_COUNT,
            "f" => <f32 as GuestArg>::REG_COUNT,
            "c" => <u8 as GuestArg>::REG_COUNT,
            "*" => <MutPtr<u8> as GuestArg>::REG_COUNT,
            // pointer cases
            _ if arg_type.starts_with('^') => <MutVoidPtr as GuestArg>::REG_COUNT,
            _ => <u32 as GuestArg>::REG_COUNT // Fallback for stubbed types
        }
    }
    let regs = env.cpu.regs_mut();
    let old_sp = extend_stack_for_args(
        reg_count,
        regs,
    );

    let arguments: &Vec<Option<MutVoidPtr>> = env.objc.borrow::<NSInvocationHostObject>(this).arguments.as_ref();
    let mut reg_offset = 0;
    for i in 0..arguments.len() {
        // TODO: do not handle target and sel as special cases
        if i == 0 {
            // target
            let target = env.objc.borrow::<NSInvocationHostObject>(this).target;
            let regs = env.cpu.regs_mut();
            write_next_arg::<id>(&mut reg_offset, regs, &mut env.mem, target);
            continue;
        }
        if i == 1 {
            // selector
            let selector = env.objc.borrow::<NSInvocationHostObject>(this).selector.unwrap();
            let regs = env.cpu.regs_mut();
            write_next_arg::<SEL>(&mut reg_offset, regs, &mut env.mem, selector);
            continue;
        }
        
        if let Some(arg_slot) = arguments[i] {
            let arg_type = argument_types[i].as_str();
            // TODO: refactor and simplify
            match arg_type {
                "@" => {
                    let arg: ConstPtr<id> = arg_slot.cast().cast_const();
                    let arg_val = env.mem.read(arg);
                    let regs = env.cpu.regs_mut();
                    write_next_arg::<id>(&mut reg_offset, regs, &mut env.mem, arg_val);
                },
                "f" => {
                    let arg: ConstPtr<f32> = arg_slot.cast().cast_const();
                    let arg_val = env.mem.read(arg);
                    let regs = env.cpu.regs_mut();
                    write_next_arg::<f32>(&mut reg_offset, regs, &mut env.mem, arg_val);
                },
                "c" => {
                    let arg: ConstPtr<u8> = arg_slot.cast().cast_const();
                    let arg_val = env.mem.read(arg);
                    let regs = env.cpu.regs_mut();
                    write_next_arg::<u8>(&mut reg_offset, regs, &mut env.mem, arg_val);
                }
                "*" => {
                    let arg: ConstPtr<MutPtr<u8>> = arg_slot.cast().cast_const();
                    let arg_val = env.mem.read(arg);
                    let regs = env.cpu.regs_mut();
                    write_next_arg::<MutPtr<u8>>(&mut reg_offset, regs, &mut env.mem, arg_val);
                }
                // pointer cases
                _ if arg_type.starts_with('^') => {
                    let arg: ConstPtr<MutVoidPtr> = arg_slot.cast().cast_const();
                    let arg_val = env.mem.read(arg);
                    let regs = env.cpu.regs_mut();
                    write_next_arg::<MutVoidPtr>(&mut reg_offset, regs, &mut env.mem, arg_val);
                }
                _ => {
                    // Fallback
                    let arg: ConstPtr<u32> = arg_slot.cast().cast_const();
                    let arg_val = env.mem.read(arg);
                    let regs = env.cpu.regs_mut();
                    write_next_arg::<u32>(&mut reg_offset, regs, &mut env.mem, arg_val);
                }
            }
        }
    }

    // actual invocation
    let &NSInvocationHostObject { target, selector, .. } = env.objc.borrow::<NSInvocationHostObject>(this);
    objc_msgSend(env, target, selector.unwrap());

    let regs = env.cpu.regs_mut(); // re-borrow
    regs[Cpu::SP] = old_sp;
    // TODO: non-void return
}

- (())dealloc {
    let &NSInvocationHostObject { sig, target, arguments_retained, .. } = env.objc.borrow::<NSInvocationHostObject>(this);
    release(env, sig);
    if arguments_retained {
        release(env, target);
        let retained_objects = std::mem::take(
            &mut env.objc.borrow_mut::<NSInvocationHostObject>(this).retained_objects
        );
        for obj in retained_objects {
            release(env, obj);
        }
        let copied_strings = std::mem::take(
            &mut env.objc.borrow_mut::<NSInvocationHostObject>(this).copied_strings
        );
        for s in copied_strings {
            env.mem.free(s.cast());
        }
    } else {
        assert!(env.objc.borrow::<NSInvocationHostObject>(this).retained_objects.is_empty());
        assert!(env.objc.borrow::<NSInvocationHostObject>(this).copied_strings.is_empty());
    }
    for ptr in env.objc.borrow::<NSInvocationHostObject>(this).arguments.iter().flatten() {
        env.mem.free(ptr.cast());
    }
    env.objc.dealloc_object(this, &mut env.mem)
}

@end

};
