/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `CFAttributedString`, toll-free bridged to `NSAttributedString`.
//!
//! Reference: <https://developer.apple.com/documentation/corefoundation/cfattributedstring>

use super::cf_allocator::{kCFAllocatorDefault, CFAllocatorRef};
use super::cf_dictionary::CFDictionaryRef;
use super::cf_string::CFStringRef;
use super::cf_type::{CFTypeID, CFTypeRef};
use super::{CFIndex, CFRange};
use crate::dyld::{export_c_func, FunctionExports};
use crate::frameworks::foundation::ns_attributed_string;
use crate::frameworks::foundation::{NSRange, NSUInteger};
use crate::mem::MutPtr;
use crate::objc::{id, msg, msg_class, nil, Class};
use crate::Environment;

pub type CFAttributedStringRef = CFTypeRef;

fn validate_allocator(env: &mut Environment, allocator: CFAllocatorRef) -> bool {
    allocator == kCFAllocatorDefault
        || allocator.is_null()
        || env.mem.read(allocator).is_system_default()
}

fn ns_range_from_cf_range(range: CFRange, string_length: NSUInteger) -> Option<NSRange> {
    if range.location < 0 || range.length < 0 {
        return None;
    }
    let location: NSUInteger = range.location.try_into().ok()?;
    let length: NSUInteger = range.length.try_into().ok()?;
    let end = location.checked_add(length)?;
    if end > string_length {
        return None;
    }
    Some(NSRange { location, length })
}

fn write_effective_range(env: &mut Environment, out: MutPtr<CFRange>, range: Option<NSRange>) {
    if !out.is_null() {
        let range = range.unwrap_or(NSRange {
            location: 0,
            length: 0,
        });
        env.mem.write(
            out,
            CFRange {
                location: range.location.try_into().unwrap_or(CFIndex::MAX),
                length: range.length.try_into().unwrap_or(CFIndex::MAX),
            },
        );
    }
}

fn is_valid_location(location: CFIndex, length: CFIndex) -> bool {
    location >= 0 && location < length
}

fn full_range(length: CFIndex) -> NSRange {
    NSRange {
        location: 0,
        length: length.max(0) as NSUInteger,
    }
}

fn CFAttributedStringCreate(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    string: CFStringRef,
    attributes: CFDictionaryRef,
) -> CFAttributedStringRef {
    if !validate_allocator(env, allocator) || string.is_null() {
        return nil;
    }
    let attributed_string: id = msg_class![env; NSAttributedString alloc];
    msg![env; attributed_string initWithString:string attributes:attributes]
}

fn CFAttributedStringCreateCopy(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    string: CFAttributedStringRef,
) -> CFAttributedStringRef {
    if !validate_allocator(env, allocator) || string.is_null() {
        return nil;
    }
    msg![env; string copy]
}

fn CFAttributedStringCreateWithSubstring(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    string: CFAttributedStringRef,
    range: CFRange,
) -> CFAttributedStringRef {
    if !validate_allocator(env, allocator) || string.is_null() {
        return nil;
    }
    let length = CFAttributedStringGetLength(env, string);
    let Some(range) = ns_range_from_cf_range(range, length.max(0) as NSUInteger) else {
        return nil;
    };
    let substring: CFAttributedStringRef = msg![env; string attributedSubstringFromRange:range];
    if substring.is_null() {
        nil
    } else {
        crate::objc::retain(env, substring)
    }
}

fn CFAttributedStringGetLength(env: &mut Environment, string: CFAttributedStringRef) -> CFIndex {
    if string.is_null() {
        return 0;
    }
    let text: CFStringRef = msg![env; string string];
    if text.is_null() {
        return 0;
    }
    let length: NSUInteger = msg![env; text length];
    length.try_into().unwrap_or(CFIndex::MAX)
}

fn CFAttributedStringGetString(
    env: &mut Environment,
    string: CFAttributedStringRef,
) -> CFStringRef {
    if string.is_null() {
        nil
    } else {
        msg![env; string string]
    }
}

fn CFAttributedStringGetAttribute(
    env: &mut Environment,
    string: CFAttributedStringRef,
    location: CFIndex,
    key: CFStringRef,
    effective_range: MutPtr<CFRange>,
) -> CFTypeRef {
    let length = CFAttributedStringGetLength(env, string);
    if string.is_null() || key.is_null() || !is_valid_location(location, length) {
        write_effective_range(env, effective_range, None);
        return nil;
    }
    let (value, range) = ns_attributed_string::attribute_at_index_with_range(
        env,
        string,
        key,
        location as NSUInteger,
        full_range(length),
    );
    write_effective_range(env, effective_range, Some(range));
    value
}

fn CFAttributedStringGetAttributes(
    env: &mut Environment,
    string: CFAttributedStringRef,
    location: CFIndex,
    effective_range: MutPtr<CFRange>,
) -> CFDictionaryRef {
    let length = CFAttributedStringGetLength(env, string);
    if string.is_null() || !is_valid_location(location, length) {
        write_effective_range(env, effective_range, None);
        return nil;
    }
    let (attributes, range) = ns_attributed_string::attributes_at_index_with_range(
        env,
        string,
        location as NSUInteger,
        full_range(length),
    );
    write_effective_range(env, effective_range, Some(range));
    attributes
}

fn CFAttributedStringGetAttributeAndLongestEffectiveRange(
    env: &mut Environment,
    string: CFAttributedStringRef,
    location: CFIndex,
    key: CFStringRef,
    range_limit: CFRange,
    effective_range: MutPtr<CFRange>,
) -> CFTypeRef {
    let length = CFAttributedStringGetLength(env, string);
    let Some(range_limit) = ns_range_from_cf_range(range_limit, length.max(0) as NSUInteger) else {
        write_effective_range(env, effective_range, None);
        return nil;
    };
    if string.is_null() || key.is_null() || !is_valid_location(location, length) {
        write_effective_range(env, effective_range, None);
        return nil;
    }
    let (value, range) = ns_attributed_string::attribute_at_index_with_range(
        env,
        string,
        key,
        location as NSUInteger,
        range_limit,
    );
    write_effective_range(env, effective_range, Some(range));
    value
}

fn CFAttributedStringGetAttributesAndLongestEffectiveRange(
    env: &mut Environment,
    string: CFAttributedStringRef,
    location: CFIndex,
    range_limit: CFRange,
    effective_range: MutPtr<CFRange>,
) -> CFDictionaryRef {
    let length = CFAttributedStringGetLength(env, string);
    let Some(range_limit) = ns_range_from_cf_range(range_limit, length.max(0) as NSUInteger) else {
        write_effective_range(env, effective_range, None);
        return nil;
    };
    if string.is_null() || !is_valid_location(location, length) {
        write_effective_range(env, effective_range, None);
        return nil;
    }
    let (attributes, range) = ns_attributed_string::attributes_at_index_with_range(
        env,
        string,
        location as NSUInteger,
        range_limit,
    );
    write_effective_range(env, effective_range, Some(range));
    attributes
}

fn CFAttributedStringGetTypeID(env: &mut Environment) -> CFTypeID {
    let class: Class = msg_class![env; NSAttributedString class];
    class.to_bits() as CFTypeID
}

// MARK: - Mutable attributed strings

/// `CFMutableAttributedStringRef CFAttributedStringCreateMutable(
///     CFAllocatorRef alloc, CFIndex maxLength)`
///
/// `maxLength` is only a capacity hint (-1 means no limit). The result is
/// toll-free bridged to an empty `NSMutableAttributedString`.
/// <https://developer.apple.com/documentation/corefoundation/1463402-cfattributedstringcreatemutable>
fn CFAttributedStringCreateMutable(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    _max_length: CFIndex,
) -> CFAttributedStringRef {
    if !validate_allocator(env, allocator) {
        return nil;
    }
    let empty: id = msg_class![env; NSString string];
    let string: id = msg_class![env; NSMutableAttributedString alloc];
    msg![env; string initWithString:empty]
}

/// `void CFAttributedStringReplaceString(
///     CFMutableAttributedStringRef str, CFRange range, CFStringRef replacement)`
/// <https://developer.apple.com/documentation/corefoundation/1463485-cfattributedstringreplacestring>
fn CFAttributedStringReplaceString(
    env: &mut Environment,
    string: CFAttributedStringRef,
    range: CFRange,
    replacement: CFStringRef,
) {
    if string.is_null() || replacement.is_null() {
        return;
    }
    let length = CFAttributedStringGetLength(env, string);
    let Some(range) = ns_range_from_cf_range(range, length.max(0) as NSUInteger) else {
        return;
    };
    () = msg![env; string replaceCharactersInRange:range withString:replacement];
}

/// `void CFAttributedStringSetAttribute(
///     CFMutableAttributedStringRef str, CFRange range,
///     CFStringRef attr_name, CFTypeRef value)`
///
/// Per Apple, a `NULL` value removes the attribute from the range.
/// <https://developer.apple.com/documentation/corefoundation/1463420-cfattributedstringsetattribute>
fn CFAttributedStringSetAttribute(
    env: &mut Environment,
    string: CFAttributedStringRef,
    range: CFRange,
    attribute: CFStringRef,
    value: CFTypeRef,
) {
    if string.is_null() || attribute.is_null() {
        return;
    }
    let length = CFAttributedStringGetLength(env, string);
    let Some(range) = ns_range_from_cf_range(range, length.max(0) as NSUInteger) else {
        return;
    };
    if range.length == 0 {
        return;
    }
    if value.is_null() {
        () = msg![env; string removeAttribute:attribute range:range];
    } else {
        () = msg![env; string addAttribute:attribute value:value range:range];
    }
}

/// `void CFAttributedStringBeginEditing(CFMutableAttributedStringRef str)`
///
/// Editing is only an optimisation in Core Foundation (it batches mutations
/// and defers change notifications), so an unbiased attributed string needs
/// no host-side bookkeeping for it.
/// <https://developer.apple.com/documentation/corefoundation/1463532-cfattributedstringbeginediting>
fn CFAttributedStringBeginEditing(_env: &mut Environment, _string: CFAttributedStringRef) {}

/// `void CFAttributedStringEndEditing(CFMutableAttributedStringRef str)`
///
/// See [CFAttributedStringBeginEditing].
/// <https://developer.apple.com/documentation/corefoundation/1463515-cfattributedstringendediting>
fn CFAttributedStringEndEditing(_env: &mut Environment, _string: CFAttributedStringRef) {}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(CFAttributedStringCreate(_, _, _)),
    export_c_func!(CFAttributedStringCreateCopy(_, _)),
    export_c_func!(CFAttributedStringCreateWithSubstring(_, _, _)),
    export_c_func!(CFAttributedStringGetLength(_)),
    export_c_func!(CFAttributedStringGetString(_)),
    export_c_func!(CFAttributedStringGetAttribute(_, _, _, _)),
    export_c_func!(CFAttributedStringGetAttributes(_, _, _)),
    export_c_func!(CFAttributedStringGetAttributeAndLongestEffectiveRange(_, _, _, _, _)),
    export_c_func!(CFAttributedStringGetAttributesAndLongestEffectiveRange(_, _, _, _)),
    export_c_func!(CFAttributedStringCreateMutable(_, _)),
    export_c_func!(CFAttributedStringReplaceString(_, _, _)),
    export_c_func!(CFAttributedStringSetAttribute(_, _, _, _)),
    export_c_func!(CFAttributedStringBeginEditing(_)),
    export_c_func!(CFAttributedStringEndEditing(_)),
    export_c_func!(CFAttributedStringGetTypeID()),
];
