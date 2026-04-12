/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! The `NSValue` class cluster, including `NSNumber`.

use super::ns_string::{from_rust_ordering, from_rust_string};
use super::{NSComparisonResult, NSOrderedSame, NSUInteger, NSRange, _nib_archive_decoder};
use crate::frameworks::core_foundation::cf_number::{
    kCFNumberCharType, kCFNumberFloat32Type, kCFNumberFloat64Type, kCFNumberFloatType,
    kCFNumberIntType, kCFNumberSInt16Type, kCFNumberSInt32Type, kCFNumberSInt64Type,
    kCFNumberSInt8Type, kCFNumberShortType, CFNumberType,
};
use crate::frameworks::core_graphics::{CGPoint, CGRect, CGSize};
use crate::frameworks::foundation::NSInteger;
use crate::mem::{ConstVoidPtr, MutVoidPtr};
use crate::objc::{
    autorelease, id, msg, msg_class, objc_classes, release, retain, Class, ClassExports,
    HostObject, NSZonePtr,
};
use crate::Environment;
use std::cmp::Ordering;

#[derive(Debug)]
pub(super) enum NSValueHostObject {
    CGPoint(CGPoint),
    CGSize(CGSize),
    CGRect(CGRect),
    NSRange(NSRange),
}
impl HostObject for NSValueHostObject {}

macro_rules! impl_AsValue {
    ($method_name:tt, $typ:tt) => {
        pub fn $method_name(&self) -> $typ {
            match self {
                // Cast to u8 is needed for float conversions
                NSNumberHostObject::Bool(x) => *x as u8 as _,
                NSNumberHostObject::UnsignedLongLong(x) => *x as _,
                NSNumberHostObject::UnsignedInt(x) => *x as _,
                NSNumberHostObject::Int(x) => *x as _,
                NSNumberHostObject::LongLong(x) => *x as _,
                NSNumberHostObject::Float(x) => *x as _,
                NSNumberHostObject::Double(x) => *x as _,
                NSNumberHostObject::Short(x) => *x as _,
                NSNumberHostObject::UnsignedShort(x) => *x as _,
                NSNumberHostObject::Char(x) => *x as _,
            }
        }
    };
}

#[derive(Debug)]
pub(super) enum NSNumberHostObject {
    Bool(bool),
    UnsignedLongLong(u64),
    UnsignedInt(u32),
    Int(i32), // Also covers Integer and Long since this is a 32-bit platform.
    LongLong(i64),
    Float(f32),
    Double(f64),
    Short(i16),
    UnsignedShort(u16),
    Char(i8),
}
impl HostObject for NSNumberHostObject {}

impl NSNumberHostObject {
    fn as_bool(&self) -> bool {
        match self {
            NSNumberHostObject::Bool(x) => *x,
            NSNumberHostObject::UnsignedLongLong(x) => *x != 0,
            NSNumberHostObject::UnsignedInt(x) => *x != 0,
            NSNumberHostObject::Int(x) => *x != 0,
            NSNumberHostObject::LongLong(x) => *x != 0,
            NSNumberHostObject::Float(x) => *x != 0.0,
            NSNumberHostObject::Double(x) => *x != 0.0,
            NSNumberHostObject::Short(x) => *x != 0,
            NSNumberHostObject::UnsignedShort(x) => *x != 0,
            NSNumberHostObject::Char(x) => *x != 0,
        }
    }
    fn is_float(&self) -> bool {
        matches!(
            self,
            NSNumberHostObject::Float(_) | NSNumberHostObject::Double(_)
        )
    }
    impl_AsValue!(as_int, i32);
    impl_AsValue!(as_long_long, i64);
    impl_AsValue!(as_unsigned_long_long, u64);
    impl_AsValue!(as_unsigned_int, u32);
    impl_AsValue!(as_float, f32);
    impl_AsValue!(as_double, f64);
    impl_AsValue!(as_short, i16);
    impl_AsValue!(as_unsigned_short, u16);
    impl_AsValue!(as_char, i8);
    impl_AsValue!(as_i128, i128);
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

// NSValue is an abstract class.
// None of the things it should provide are
// implemented here yet (TODO).
@implementation NSValue: NSObject

+ (id)valueWithPointer:(ConstVoidPtr)ptr {
    // TODO: implement with `value:withObjCType:` instead
    msg_class![env; NSNumber numberWithUnsignedInt:(ptr.to_bits())]
}

+ (id)valueWithCGPoint:(CGPoint)value {
    let host_object = Box::new(NSValueHostObject::CGPoint(value));
    let new = env.objc.alloc_object(this, host_object, &mut env.mem);
    autorelease(env, new)
}

+ (id)valueWithCGSize:(CGSize)value {
    let host_object = Box::new(NSValueHostObject::CGSize(value));
    let new = env.objc.alloc_object(this, host_object, &mut env.mem);
    autorelease(env, new)
}

+ (id)valueWithCGRect:(CGRect)value {
    let host_object = Box::new(NSValueHostObject::CGRect(value));
    let new = env.objc.alloc_object(this, host_object, &mut env.mem);
    autorelease(env, new)
}

+ (id)valueWithRange:(NSRange)value {
    // Упаковываем структуру в наш HostObject и выделяем под это память
    let host_object = Box::new(NSValueHostObject::NSRange(value));
    let new = env.objc.alloc_object(this, host_object, &mut env.mem);
    autorelease(env, new)
}
    
+ (id)valueWithNonretainedObject:(id)object {
    // Store the pointer bits as an unsigned int.
    msg_class![env; NSNumber numberWithUnsignedInt:(object.to_bits())]
}

+ (id)valueWithBytes:(ConstVoidPtr)value objCType:(ConstVoidPtr)_type {
    // Store as a pointer/uint — we don't model arbitrary ObjC types.
    let bits = value.to_bits();
    msg_class![env; NSNumber numberWithUnsignedInt:bits]
}

// MARK: - Additional NSValue accessors

- (id)nonretainedObjectValue {
    // Reverse of valueWithNonretainedObject — recover the id from bits.
    let bits: u32 = msg![env; this unsignedIntValue];
    crate::objc::id::from_bits(bits)
}

- (bool)isEqual:(id)other {
    if this == other { return true; }
    if other == crate::objc::nil { return false; }
    
    // Сначала вызываем функции, использующие env, ДО заимствования `this`
    let host_b_class: crate::objc::Class = msg![env; other class];
    let ns_value_class = env.objc.get_known_class("NSValue", &mut env.mem);
    if !env.objc.class_is_subclass_of(host_b_class, ns_value_class) {
        return false;
    }
    
    // Теперь можно безопасно заимствовать оба объекта
    let host_a = env.objc.borrow::<NSValueHostObject>(this);
    let b = env.objc.borrow::<NSValueHostObject>(other);
    
    match (host_a, b) {
        (NSValueHostObject::CGPoint(a), NSValueHostObject::CGPoint(b)) => {
            a.x == b.x && a.y == b.y
        }
        (NSValueHostObject::CGSize(a), NSValueHostObject::CGSize(b)) => {
            a.width == b.width && a.height == b.height
        }
        (NSValueHostObject::CGRect(a), NSValueHostObject::CGRect(b)) => {
            a.origin.x == b.origin.x && a.origin.y == b.origin.y
                && a.size.width == b.size.width && a.size.height == b.size.height
        }
        (NSValueHostObject::NSRange(a), NSValueHostObject::NSRange(b)) => {
            a.location == b.location && a.length == b.length
        }
        _ => false,
    }
}

- (id)description {
    let s = match env.objc.borrow::<NSValueHostObject>(this) {
        NSValueHostObject::CGPoint(p) => {
            let (x, y) = (p.x, p.y);
            format!("NSPoint: {{{}, {}}}", x, y)
        }
        NSValueHostObject::CGSize(s) => {
            let (w, h) = (s.width, s.height);
            format!("NSSize: {{{}, {}}}", w, h)
        }
        NSValueHostObject::CGRect(r) => {
            let (ox, oy) = (r.origin.x, r.origin.y);
            let (sw, sh) = (r.size.width, r.size.height);
            format!(
                "NSRect: {{{{{}, {}}}, {{{}, {}}}}}",
                ox, oy, sw, sh
            )
        }
        NSValueHostObject::NSRange(r) => {
            // Копируем значения в локальные переменные, чтобы избежать взятия ссылки на packed-структуру
            let loc = r.location;
            let len = r.length;
            format!("NSRange: {{{}, {}}}", loc, len)
        }
    };
    let ns = from_rust_string(env, s);
    autorelease(env, ns)
}

- (CGPoint)CGPointValue {
    let host_object = env.objc.borrow::<NSValueHostObject>(this);
    match host_object {
        NSValueHostObject::CGPoint(cg_point) => *cg_point,
        _ => unimplemented!()
    }
}

- (CGSize)CGSizeValue {
    let host_object = env.objc.borrow::<NSValueHostObject>(this);
    match host_object {
        NSValueHostObject::CGSize(cg_size) => *cg_size,
        _ => unimplemented!()
    }
}

- (CGRect)CGRectValue {
    let host_object = env.objc.borrow::<NSValueHostObject>(this);
    match host_object {
        NSValueHostObject::CGRect(cg_rect) => *cg_rect,
        _ => unimplemented!()
    }
}

- (NSRange)rangeValue {
    let host_object = env.objc.borrow::<NSValueHostObject>(this);
    match host_object {
        NSValueHostObject::NSRange(r) => NSRange { location: r.location, length: r.length },
        _ => unimplemented!("Called rangeValue on non-range NSValue")
    }
}
    
// NSCopying implementation
- (id)copyWithZone:(NSZonePtr)_zone {
    retain(env, this)
}

- (MutVoidPtr)pointerValue {
    let class: Class = msg![env; this class];
    assert!(class == env.objc.get_known_class("NSNumber", &mut env.mem));
    // According to the docs, `If the value object was not created to hold
    // a pointer-sized data item, the result is undefined.`
    let val = msg![env; this unsignedIntValue];
    MutVoidPtr::from_bits(val)
}

@end

// NSNumber is not an abstract class.
@implementation NSNumber: NSValue

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(NSNumberHostObject::Bool(false));
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

+ (id)numberWithBool:(bool)value {
    // TODO: for greater efficiency we could return a static-lifetime value

    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithBool:value];
    autorelease(env, new)
}

+ (id)numberWithFloat:(f32)value {
    // TODO: for greater efficiency we could return a static-lifetime value

    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithFloat:value];
    autorelease(env, new)
}

+ (id)numberWithDouble:(f64)value {
    // TODO: for greater efficiency we could return a static-lifetime value

    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithDouble:value];
    autorelease(env, new)
}

+ (id)numberWithUnsignedInt:(u32)value {
    // TODO: for greater efficiency we could return a static-lifetime value

    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithUnsignedInt:value];
    autorelease(env, new)
}

+ (id)numberWithInt:(i32)value {
    // TODO: for greater efficiency we could return a static-lifetime value

    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithInt:value];
    autorelease(env, new)
}

+ (id)numberWithLong:(i32)value {
    // TODO: for greater efficiency we could return a static-lifetime value

    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithLong:value];
    autorelease(env, new)
}

+ (id)numberWithInteger:(NSInteger)value {
    // TODO: for greater efficiency we could return a static-lifetime value

    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithInteger:value];
    autorelease(env, new)
}

+ (id)numberWithUnsignedInteger:(NSUInteger)value {
    // TODO: for greater efficiency we could return a static-lifetime value

    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithUnsignedInteger:value];
    autorelease(env, new)
}

+ (id)numberWithLongLong:(i64)value {
    // TODO: for greater efficiency we could return a static-lifetime value

    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithLongLong:value];
    autorelease(env, new)
}

+ (id)numberWithUnsignedLongLong:(u64)value {
    // TODO: for greater efficiency we could return a static-lifetime value

    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithUnsignedLongLong:value];
    autorelease(env, new)
}

+ (id)numberWithShort:(i16)value {
    // TODO: for greater efficiency we could return a static-lifetime value

    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithShort:value];
    autorelease(env, new)
}

+ (id)numberWithUnsignedShort:(u16)value {
    // TODO: for greater efficiency we could return a static-lifetime value

    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithUnsignedShort:value];
    autorelease(env, new)
}

+ (id)numberWithChar:(i8)value {
    // TODO: for greater efficiency we could return a static-lifetime value

    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithChar:value];
    autorelease(env, new)
}

+ (id)numberWithUnsignedChar:(u8)value {
    let new: id = msg![env; this alloc];
    *env.objc.borrow_mut(new) = NSNumberHostObject::Char(value as i8);
    autorelease(env, new)
}

+ (id)numberWithUnsignedLong:(u32)value {
    msg_class![env; NSNumber numberWithUnsignedInt:value]
}

// MARK: - Additional NSNumber inits

- (id)initWithUnsignedChar:(u8)value {
    *env.objc.borrow_mut(this) = NSNumberHostObject::Char(value as i8);
    this
}

- (id)initWithUnsignedLong:(u32)value {
    *env.objc.borrow_mut(this) = NSNumberHostObject::UnsignedInt(value);
    this
}

// MARK: - Additional accessors

- (u8)unsignedCharValue {
    env.objc.borrow::<NSNumberHostObject>(this).as_char() as u8
}

- (u32)unsignedLongValue {
    env.objc.borrow::<NSNumberHostObject>(this).as_unsigned_int()
}

- (i32)unsignedCharValueAsInt {
    // Convenience — some apps read unsigned char values as int.
    env.objc.borrow::<NSNumberHostObject>(this).as_char() as u8 as i32
}

// MARK: - String representation

- (id)stringValue {
    msg![env; this description]
}

- (id)descriptionWithLocale:(id)_locale {
    msg![env; this description]
}

// MARK: - Formatting helpers

- (id)initWithBytes:(ConstVoidPtr)value objCType:(ConstVoidPtr)type_ptr {
    // Read the type encoding and store the appropriate value.
    // We support 'i', 'I', 'q', 'Q', 'f', 'd', 's', 'S', 'c', 'C', 'B'.
    let type_byte = env.mem.read(type_ptr.cast::<u8>());
    match type_byte {
        b'i' | b'l' => {
            let v = env.mem.read(value.cast::<i32>());
            *env.objc.borrow_mut(this) = NSNumberHostObject::Int(v);
        }
        b'I' | b'L' => {
            let v = env.mem.read(value.cast::<u32>());
            *env.objc.borrow_mut(this) = NSNumberHostObject::UnsignedInt(v);
        }
        b'q' => {
            let v = env.mem.read(value.cast::<i64>());
            *env.objc.borrow_mut(this) = NSNumberHostObject::LongLong(v);
        }
        b'Q' => {
            let v = env.mem.read(value.cast::<u64>());
            *env.objc.borrow_mut(this) = NSNumberHostObject::UnsignedLongLong(v);
        }
        b'f' => {
            let v = env.mem.read(value.cast::<f32>());
            *env.objc.borrow_mut(this) = NSNumberHostObject::Float(v);
        }
        b'd' => {
            let v = env.mem.read(value.cast::<f64>());
            *env.objc.borrow_mut(this) = NSNumberHostObject::Double(v);
        }
        b's' => {
            let v = env.mem.read(value.cast::<i16>());
            *env.objc.borrow_mut(this) = NSNumberHostObject::Short(v);
        }
        b'S' => {
            let v = env.mem.read(value.cast::<u16>());
            *env.objc.borrow_mut(this) = NSNumberHostObject::UnsignedShort(v);
        }
        b'c' | b'C' | b'B' => {
            let v = env.mem.read(value.cast::<i8>());
            *env.objc.borrow_mut(this) = NSNumberHostObject::Char(v);
        }
        _ => {
            log!("NSNumber initWithBytes:objCType: unknown type '{}', defaulting to int",
                 type_byte as char);
            let v = env.mem.read(value.cast::<i32>());
            *env.objc.borrow_mut(this) = NSNumberHostObject::Int(v);
        }
    }
    this
}

- (())getValue:(MutVoidPtr)buffer {
    // Write current value into a caller-provided buffer.
    match env.objc.borrow::<NSNumberHostObject>(this) {
        NSNumberHostObject::Bool(v)              => env.mem.write(buffer.cast::<i8>(), *v as i8),
        NSNumberHostObject::Int(v)               => env.mem.write(buffer.cast::<i32>(), *v),
        NSNumberHostObject::UnsignedInt(v)       => env.mem.write(buffer.cast::<u32>(), *v),
        NSNumberHostObject::LongLong(v)          => env.mem.write(buffer.cast::<i64>(), *v),
        NSNumberHostObject::UnsignedLongLong(v)  => env.mem.write(buffer.cast::<u64>(), *v),
        NSNumberHostObject::Float(v)             => env.mem.write(buffer.cast::<f32>(), *v),
        NSNumberHostObject::Double(v)            => env.mem.write(buffer.cast::<f64>(), *v),
        NSNumberHostObject::Short(v)             => env.mem.write(buffer.cast::<i16>(), *v),
        NSNumberHostObject::UnsignedShort(v)     => env.mem.write(buffer.cast::<u16>(), *v),
        NSNumberHostObject::Char(v)              => env.mem.write(buffer.cast::<i8>(), *v),
    }
}

// MARK: - CFNumber bridging helpers

- (CFNumberType)cfNumberType {
    match env.objc.borrow::<NSNumberHostObject>(this) {
        NSNumberHostObject::Bool(_)             => kCFNumberSInt8Type,
        NSNumberHostObject::Char(_)             => kCFNumberSInt8Type,
        NSNumberHostObject::Short(_)            => kCFNumberSInt16Type,
        NSNumberHostObject::UnsignedShort(_)    => kCFNumberSInt16Type,
        NSNumberHostObject::Int(_)              => kCFNumberSInt32Type,
        NSNumberHostObject::UnsignedInt(_)      => kCFNumberSInt32Type,
        NSNumberHostObject::LongLong(_)         => kCFNumberSInt64Type,
        NSNumberHostObject::UnsignedLongLong(_) => kCFNumberSInt64Type,
        NSNumberHostObject::Float(_)            => kCFNumberFloat32Type,
        NSNumberHostObject::Double(_)           => kCFNumberFloat64Type,
    }
}

// NSCoding implementation
- (id)initWithCoder:(id)coder {
    let class: Class = msg![env; coder class];
    let nib_archive_class: Class = msg_class![env; _touchHLE_NIBArchiveDecoder class];
    let new_num = if env.objc.class_is_subclass_of(class, nib_archive_class) {
        _nib_archive_decoder::decode_current_number(env, coder)
    } else {
        unimplemented!();
    };
    release(env, this);
    new_num
}

// ИЗМЕНЕНО: Добавлена заглушка для сохранения числа, предотвращающая вылет (Panic)
- (())encodeWithCoder:(id)_coder {
    log!("Warning: stubbed NSNumber encodeWithCoder:");
}

- (id)initWithBool:(bool)value {
    *env.objc.borrow_mut(this) = NSNumberHostObject::Bool(value);
    this
}

- (id)initWithFloat:(f32)value {
    *env.objc.borrow_mut(this) = NSNumberHostObject::Float(value);
    this
}

- (id)initWithDouble:(f64)value {
    *env.objc.borrow_mut(this) = NSNumberHostObject::Double(value);
    this
}

- (id)initWithLongLong:(i64)value {
    *env.objc.borrow_mut(this) = NSNumberHostObject::LongLong(value);
    this
}

- (id)initWithUnsignedInt:(u32)value {
    *env.objc.borrow_mut(this) = NSNumberHostObject::UnsignedInt(value);
    this
}

- (id)initWithInt:(i32)value {
    *env.objc.borrow_mut(this) = NSNumberHostObject::Int(value);
    this
}

- (id)initWithLong:(i32)value {
    *env.objc.borrow_mut(this) = NSNumberHostObject::Int(value);
    this
}

- (id)initWithInteger:(NSInteger)value {
    *env.objc.borrow_mut(this) = NSNumberHostObject::Int(value);
    this
}

- (id)initWithUnsignedInteger:(NSUInteger)value {
    *env.objc.borrow_mut(this) = NSNumberHostObject::UnsignedInt(value);
    this
}
    
- (id)initWithUnsignedLongLong:(u64)value {
    *env.objc.borrow_mut(this) = NSNumberHostObject::UnsignedLongLong(value);
    this
}

- (id)initWithShort:(i16)value {
    *env.objc.borrow_mut(this) = NSNumberHostObject::Short(value);
    this
}

- (id)initWithUnsignedShort:(u16)value {
    *env.objc.borrow_mut(this) = NSNumberHostObject::UnsignedShort(value);
    this
}

- (id)initWithChar:(i8)value {
    *env.objc.borrow_mut(this) = NSNumberHostObject::Char(value);
    this
}

- (bool)boolValue {
    env.objc.borrow::<NSNumberHostObject>(this).as_bool()
}

- (NSInteger)integerValue {
    env.objc.borrow::<NSNumberHostObject>(this).as_int()
}

- (i32)intValue {
    env.objc.borrow::<NSNumberHostObject>(this).as_int()
}

- (i32)longValue {
    env.objc.borrow::<NSNumberHostObject>(this).as_int()
}

- (f32)floatValue {
    env.objc.borrow::<NSNumberHostObject>(this).as_float()
}

- (f64)doubleValue {
    env.objc.borrow::<NSNumberHostObject>(this).as_double()
}

- (i64)longLongValue {
    env.objc.borrow::<NSNumberHostObject>(this).as_long_long()
}

- (u64)unsignedLongLongValue {
    env.objc.borrow::<NSNumberHostObject>(this).as_unsigned_long_long()
}

- (u32)unsignedIntValue {
    env.objc.borrow::<NSNumberHostObject>(this).as_unsigned_int()
}

- (NSUInteger)unsignedIntegerValue {
    env.objc.borrow::<NSNumberHostObject>(this).as_unsigned_int()
}

- (i16)shortValue {
    env.objc.borrow::<NSNumberHostObject>(this).as_short()
}

- (u16)unsignedShortValue {
    env.objc.borrow::<NSNumberHostObject>(this).as_unsigned_short()
}

- (i8)charValue {
    env.objc.borrow::<NSNumberHostObject>(this).as_char()
}

- (id)description {
    let desc = match env.objc.borrow(this) {
        NSNumberHostObject::Bool(value) => from_rust_string(env, (*value as i32).to_string()),
        NSNumberHostObject::UnsignedLongLong(value) => from_rust_string(env, value.to_string()),
        NSNumberHostObject::UnsignedInt(value) => from_rust_string(env, value.to_string()),
        NSNumberHostObject::Int(value) => from_rust_string(env, value.to_string()),
        NSNumberHostObject::LongLong(value) => from_rust_string(env, value.to_string()),
        NSNumberHostObject::Float(value) => from_rust_string(env, value.to_string()),
        NSNumberHostObject::Double(value) => from_rust_string(env, value.to_string()),
        NSNumberHostObject::Short(value) => from_rust_string(env, value.to_string()),
        NSNumberHostObject::UnsignedShort(value) => from_rust_string(env, value.to_string()),
        NSNumberHostObject::Char(value) => from_rust_string(env, value.to_string()),
    };
    autorelease(env, desc)
}

- (NSUInteger)hash {
    // The only requirement for [obj hash] is that values that compare equal
    // (via [obj isEqual] have the same hash. Hashing the underlying
    // bits works here.
    let value =
    match env.objc.borrow(this) {
        NSNumberHostObject::Bool(value) => *value as u64,
        NSNumberHostObject::UnsignedLongLong(value) => *value,
        NSNumberHostObject::UnsignedInt(value) => *value as u64,
        NSNumberHostObject::Int(value) => *value as u64,
        NSNumberHostObject::LongLong(value) => *value as u64,
        NSNumberHostObject::Float(value) => value.to_bits() as u64,
        NSNumberHostObject::Double(value) => value.to_bits(),
        NSNumberHostObject::Short(value) => *value as u64,
        NSNumberHostObject::UnsignedShort(value) => *value as u64,
        NSNumberHostObject::Char(value) => *value as u64,
    };
    super::hash_helper(&value)
}

- (bool)isEqual:(id)other {
    if this == other {
        return true;
    }
    let class: Class = msg_class![env; NSNumber class];
    if !msg![env; other isKindOfClass:class] {
        return false;
    }
    msg![env; this isEqualToNumber:other]
}

- (bool)isEqualToNumber:(id)other {
    let res: NSComparisonResult = msg![env; this compare:other];
    res == NSOrderedSame
}

- (NSComparisonResult)compare:(id)other { // NSNumber *
    let num = env.objc.borrow::<NSNumberHostObject>(this);
    let other_num = env.objc.borrow::<NSNumberHostObject>(other);
    let ordering = match (num.is_float(), other_num.is_float()) {
        (false, false) => num.as_i128().cmp(&other_num.as_i128()),
        // In case of having a float, we promote to double for comparison
        _ => {
            // TODO: handle partial cmp fails
            let res = num.as_double().partial_cmp(&other_num.as_double()).unwrap();
            if res == Ordering::Equal {
                // On ties, we compare as i128 as well
                num.as_i128().cmp(&other_num.as_i128())
            } else {
                res
            }
        },
    };
    from_rust_ordering(ordering)
}

// Returns the Objective-C type encoding for the wrapped number.
- (ConstVoidPtr)objCType {
    let typ: &[u8; 2] = match env.objc.borrow::<NSNumberHostObject>(this) {
        NSNumberHostObject::Bool(_) | NSNumberHostObject::Char(_) => b"c\0",
        NSNumberHostObject::UnsignedLongLong(_) => b"Q\0",
        NSNumberHostObject::UnsignedInt(_) => b"I\0",
        NSNumberHostObject::Int(_) => b"i\0",
        NSNumberHostObject::LongLong(_) => b"q\0",
        NSNumberHostObject::Float(_) => b"f\0",
        NSNumberHostObject::Double(_) => b"d\0",
        NSNumberHostObject::Short(_) => b"s\0",
        NSNumberHostObject::UnsignedShort(_) => b"S\0",
    };
    // Переводим [u8; 2] в u16 (little-endian), так как u16 поддерживает SafeWrite
    let typ_val = u16::from_le_bytes(*typ);
    // Выделяем память под u16 и возвращаем указатель
    env.mem.alloc_and_write(typ_val).cast_void().cast_const()
}

// TODO: accessors etc

@end

};

pub fn is_conversion_lossless(env: &mut Environment, this: id, type_: CFNumberType) -> bool {
    let num = env.objc.borrow::<NSNumberHostObject>(this);
    let num2: id = match type_ {
        kCFNumberSInt32Type | kCFNumberIntType => {
            let val: i32 = num.as_int();
            msg_class![env; NSNumber numberWithInt:val]
        }
        kCFNumberFloat32Type | kCFNumberFloatType => {
            let val: f32 = num.as_float();
            msg_class![env; NSNumber numberWithFloat:val]
        }
        kCFNumberSInt16Type | kCFNumberShortType => {
            let val: i16 = num.as_short();
            msg_class![env; NSNumber numberWithShort:val]
        }
        kCFNumberSInt8Type | kCFNumberCharType => {
            let val: i8 = num.as_char();
            msg_class![env; NSNumber numberWithChar:val]
        }
        _ => unimplemented!("is_conversion_lossless for {}", type_),
    };
    msg![env; this isEqualToNumber:num2]
}

