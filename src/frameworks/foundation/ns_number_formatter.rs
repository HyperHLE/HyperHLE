/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 */
//! `NSNumberFormatter` - formats numbers into strings and parses strings into numbers.

use crate::frameworks::foundation::NSUInteger;
use crate::frameworks::foundation::ns_string::{from_rust_string, to_rust_string};
use crate::objc::{
    id, msg, msg_class, nil, objc_classes, ClassExports, HostObject, NSZonePtr,
};

// =========================================================================
// MARK: - NSNumberFormatter Host Object
// =========================================================================

struct NSNumberFormatterHostObject {
    number_style: NSUInteger,
}
impl HostObject for NSNumberFormatterHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation NSNumberFormatter : NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(NSNumberFormatterHostObject {
        number_style: 0, // По умолчанию NSNumberFormatterNoStyle
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)init {
    this
}

- (())dealloc {
    env.objc.dealloc_object(this, &mut env.mem)
}

// Установка и чтение стиля форматирования
- (NSUInteger)numberStyle {
    env.objc.borrow::<NSNumberFormatterHostObject>(this).number_style
}

- (())setNumberStyle:(NSUInteger)style {
    env.objc.borrow_mut::<NSNumberFormatterHostObject>(this).number_style = style;
}

// Главный метод: конвертация объекта NSNumber в NSString
- (id)stringFromNumber:(id)number {
    if number == nil {
        return nil;
    }

    // Запрашиваем у NSNumber его значение в виде f64 (double)
    let val: f64 = msg![env; number doubleValue];
    let style = env.objc.borrow::<NSNumberFormatterHostObject>(this).number_style;

    // Стили в Objective-C:
    // 0 = NoStyle, 1 = DecimalStyle, 2 = CurrencyStyle, 3 = PercentStyle, 4 = ScientificStyle
    let rust_string = match style {
        1 => format!("{}", val), // Decimal: обычное десятичное число
        2 => format!("${:.2}", val), // Currency: добавляем знак доллара и 2 знака после запятой
        3 => format!("{}%", val * 100.0), // Percent: умножаем на 100 и добавляем %
        4 => format!("{:e}", val), // Scientific: экспоненциальная запись
        _ => format!("{}", val), // NoStyle или неизвестный стиль
    };

    // Конвертируем строку Rust обратно в объект NSString
    from_rust_string(env, rust_string)
}

// Обратный метод: конвертация NSString в NSNumber
- (id)numberFromString:(id)string {
    if string == nil {
        return nil;
    }

    let rust_str = to_rust_string(env, string);
    
    // Очищаем строку от знаков валюты и процентов, чтобы Rust мог её распарсить
    let clean_str = rust_str.replace("$", "").replace(",", "").replace("%", "").trim().to_string();

    // Пытаемся распарсить строку в f64
    if let Ok(val) = clean_str.parse::<f64>() {
        let ns_number_class = env.objc.get_known_class("NSNumber", &mut env.mem);
        // Создаем новый объект NSNumber
        msg![env; ns_number_class numberWithDouble:val]
    } else {
        log!("Warning: NSNumberFormatter failed to parse string '{}'", rust_str);
        nil
    }
}

@end

};

