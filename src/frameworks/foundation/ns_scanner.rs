/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! The `NSScanner` class.

use crate::frameworks::foundation::ns_string::{from_u16_vec, to_rust_string};
use crate::frameworks::foundation::{unichar, NSInteger, NSNotFound, NSRange, NSUInteger};
use crate::mem::MutPtr;
use crate::objc::{
    autorelease, id, msg, msg_class, nil, objc_classes, release, retain, ClassExports, HostObject,
    NSZonePtr,
};
use crate::Environment;

// TODO: Speed up by optimizing for internal subclasses
#[derive(Default, Clone)]
struct NSScannerHostObject {
    /// NSCharacterSet *, characters to be skipped
    to_be_skipped: id,
    /// NSString *, should always be immutable since it's copied
    string: id,
    /// Length is cached since it is immutable.
    len: NSUInteger,
    pos: NSUInteger,
}
impl HostObject for NSScannerHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation NSScanner: NSObject

+ (id)scannerWithString:(id)string {
    let new: id = msg![env; this alloc];
    let new = msg![env; new initWithString:string];
    autorelease(env, new)
}

+ (id)allocWithZone:(NSZonePtr)zone {
    // NSScanner might be subclassed by something which needs
    // allocWithZone: to have the normal behaviour. Unimplemented: call
    // superclass alloc then.
    assert!(this == env.objc.get_known_class("NSScanner", &mut env.mem));
    msg_class![env; _touchHLE_NSScanner allocWithZone:zone]
}

- (bool)scanInteger:(MutPtr<NSInteger>)result {
    msg![env; this scanInt:result]
}

@end

// Our private subclass that is the single implementation of NSScanner for
// the time being.
@implementation _touchHLE_NSScanner: NSScanner

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(NSScannerHostObject::default());
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)initWithString:(id)string { // NSString *
    // Per Apple's NSScanner, a scanner created over a nil string does not
    // raise — it simply has nothing to scan (every scan operation fails and
    // -isAtEnd is true). Real-world iOS apps rely on this lenient behaviour,
    // so instead of asserting we handle nil as an empty (length 0) scanner.
    let (string, len): (id, NSUInteger) = if string == nil {
        (nil, 0)
    } else {
        let string: id = msg![env; string copy]; // Same behaviour as simulator
        let len: NSUInteger = msg![env; string length];
        (string, len)
    };
    let default_set = msg_class![env; NSCharacterSet whitespaceAndNewlineCharacterSet];
    retain(env, default_set);
    *env.objc.borrow_mut(this) = NSScannerHostObject {
        to_be_skipped: default_set,
        string,
        len,
        pos: 0
    };
    this
}

- (())dealloc {
    let &NSScannerHostObject {
        to_be_skipped,
        string,
        ..
    } = env.objc.borrow(this);
    release(env, string);
    release(env, to_be_skipped);
    env.objc.dealloc_object(this, &mut env.mem);
}

- (id)charactersToBeSkipped { // NSCharacterSet *
    env.objc.borrow::<NSScannerHostObject>(this).to_be_skipped
}

- (())setCharactersToBeSkipped:(id)set { // NSCharacterSet *
    // Per Apple's NSScanner documentation:
    // https://developer.apple.com/documentation/foundation/nsscanner/1413024-characterstobeskipped
    // "Characters to be skipped are skipped prior to the scan."
    // Ownership: the scanner retains the new set and releases the old one
    // (Foundation convention for Objective-C copy properties — real Apple
    // copies, but for HLE retaining is behaviourally equivalent since the
    // guest doesn't mutate the character set after assignment).
    let old = env.objc.borrow::<NSScannerHostObject>(this).to_be_skipped;
    retain(env, set);
    env.objc.borrow_mut::<NSScannerHostObject>(this).to_be_skipped = set;
    release(env, old);
}

- (bool)isAtEnd {
    skip_characters(env, this);
    let NSScannerHostObject { len, pos, .. } = env.objc.borrow::<NSScannerHostObject>(this);
    len == pos
}

- (NSUInteger)scanLocation {
    env.objc.borrow::<NSScannerHostObject>(this).pos
}

- (())setScanLocation:(NSUInteger)location {
    let len = env.objc.borrow::<NSScannerHostObject>(this).len;
    // Apple docs: setting scanLocation beyond the string length raises
    // NSRangeException.  Here we silently clamp, matching the spirit of HLE.
    let clamped = if location > len { len } else { location };
    env.objc.borrow_mut::<NSScannerHostObject>(this).pos = clamped;
}

- (id)string {
    env.objc.borrow::<NSScannerHostObject>(this).string
}

- (bool)scanUpToCharactersFromSet:(id)cset intoString:(MutPtr<id>)str {
    skip_characters(env, this);

    let NSScannerHostObject { to_be_skipped, string, len, mut pos } = env.objc.borrow::<NSScannerHostObject>(this).clone();
    if pos >= len {
        // Does nothing (same as simulator)
        return false;
    }
    let first_scan: unichar = msg![env; string characterAtIndex:pos];
    if msg![env; cset characterIsMember:first_scan] {
        // Does nothing (same as simulator)
        return false;
    }
    let mut chars = vec![first_scan];
    pos += 1;
    while pos < len {
        let curr = msg![env; string characterAtIndex:pos];
        if msg![env; cset characterIsMember:curr] {
            break
        }
        pos += 1;
        chars.push(curr);
    }
    if !str.is_null() {
        let out = from_u16_vec(env, chars);
        autorelease(env, out);
        env.mem.write(str, out);
    }

    *env.objc.borrow_mut::<NSScannerHostObject>(this) = NSScannerHostObject { to_be_skipped, string, len, pos };
    true
}

- (bool)scanCharactersFromSet:(id)cset intoString:(MutPtr<id>)str {
    let inv_cset: id = msg![env; cset invertedSet];
    msg![env; this scanUpToCharactersFromSet:inv_cset intoString:str]
}

- (bool)scanHexInt:(MutPtr<u32>)result {
    skip_characters(env, this);

    let NSScannerHostObject { to_be_skipped, string, len, pos } =
        std::mem::take(env.objc.borrow_mut::<NSScannerHostObject>(this));
    if pos >= len {
        *env.objc.borrow_mut::<NSScannerHostObject>(this) =
            NSScannerHostObject { to_be_skipped, string, len, pos };
        return false;
    }

    let substring: id = msg![env; string substringFromIndex:pos];
    let st = to_rust_string(env, substring);
    let chars: Vec<char> = st.chars().collect();

    // Per Apple's documentation for -scanHexInt::
    // "The hexadecimal integer representation may optionally be preceded by
    //  0x or 0X." The prefix is only consumed when an actual hex digit
    // follows it.
    let mut i = 0usize;
    if chars.len() >= 3
        && chars[0] == '0'
        && (chars[1] == 'x' || chars[1] == 'X')
        && chars[2].is_ascii_hexdigit()
    {
        i = 2;
    }

    let digits_start = i;
    let mut value: u64 = 0;
    while i < chars.len() && chars[i].is_ascii_hexdigit() {
        let digit = chars[i].to_digit(16).unwrap() as u64;
        value = value.saturating_mul(16).saturating_add(digit);
        // "If the receiver contains a hexadecimal representation greater than
        //  UINT_MAX, then the value [...] is UINT_MAX."
        if value > u32::MAX as u64 {
            value = u32::MAX as u64;
        }
        i += 1;
    }

    if i == digits_start {
        // No hexadecimal digits were found; scanner is unchanged.
        *env.objc.borrow_mut::<NSScannerHostObject>(this) =
            NSScannerHostObject { to_be_skipped, string, len, pos };
        return false;
    }

    if !result.is_null() {
        env.mem.write(result, value as u32);
    }

    // Only ASCII characters ("0x" + hex digits) were consumed, so the number
    // of Rust chars equals the number of UTF-16 code units to advance by.
    let consumed = i as NSUInteger;
    *env.objc.borrow_mut::<NSScannerHostObject>(this) =
        NSScannerHostObject { to_be_skipped, string, len, pos: pos + consumed };
    log_dbg!("scanHexInt: from '{}' -> {:#x}", st, value);
    true
}

- (bool)scanHexLongLong:(MutPtr<u64>)result {
    skip_characters(env, this);

    let NSScannerHostObject { to_be_skipped, string, len, pos } =
        std::mem::take(env.objc.borrow_mut::<NSScannerHostObject>(this));
    if pos >= len {
        *env.objc.borrow_mut::<NSScannerHostObject>(this) =
            NSScannerHostObject { to_be_skipped, string, len, pos };
        return false;
    }

    let substring: id = msg![env; string substringFromIndex:pos];
    let st = to_rust_string(env, substring);
    let chars: Vec<char> = st.chars().collect();
    let mut i = 0usize;
    if chars.len() >= 3
        && chars[0] == '0'
        && (chars[1] == 'x' || chars[1] == 'X')
        && chars[2].is_ascii_hexdigit()
    {
        i = 2;
    }

    let digits_start = i;
    let mut value = 0u64;
    while i < chars.len() && chars[i].is_ascii_hexdigit() {
        value = value
            .saturating_mul(16)
            .saturating_add(chars[i].to_digit(16).unwrap() as u64);
        i += 1;
    }

    if i == digits_start {
        *env.objc.borrow_mut::<NSScannerHostObject>(this) =
            NSScannerHostObject { to_be_skipped, string, len, pos };
        return false;
    }

    if !result.is_null() {
        env.mem.write(result, value);
    }

    let consumed = i as NSUInteger;
    *env.objc.borrow_mut::<NSScannerHostObject>(this) =
        NSScannerHostObject { to_be_skipped, string, len, pos: pos + consumed };
    log_dbg!("scanHexLongLong: from '{}' -> {:#x}", st, value);
    true
}

- (bool)scanUpToString:(id)stop_string // NSString *
            intoString:(MutPtr<id>)result { // NSString **
    skip_characters(env, this);

    let NSScannerHostObject { to_be_skipped, string, len, pos } = std::mem::take(env.objc.borrow_mut::<NSScannerHostObject>(this));
    log_dbg!("scanUpToString:'{}' intoString: from '{}' at {}", to_rust_string(env, stop_string), to_rust_string(env, string), pos);

    // TODO: avoid string copying
    let left: id = msg![env; string substringFromIndex:pos];
    let range: NSRange = msg![env; left rangeOfString:stop_string];
    if range.location == 0 {
        *env.objc.borrow_mut::<NSScannerHostObject>(this) = NSScannerHostObject { to_be_skipped, string, len, pos };
        return false;
    }

    let scan_len = if range.location == NSNotFound as NSUInteger {
        len - pos
    } else {
        range.location
    };
    assert!(pos + scan_len <= len);
    *env.objc.borrow_mut::<NSScannerHostObject>(this) = NSScannerHostObject { to_be_skipped, string, len, pos: pos + scan_len };

    if !result.is_null() {
        let copy: id = msg![env; left substringToIndex:scan_len];
        log_dbg!("scanned '{}' up to {}", to_rust_string(env, copy), pos + scan_len);
        // Note: substring is already autoreleased
        env.mem.write(result, copy);
    }
    true
}

- (bool)scanString:(id)scan_string // NSString *
        intoString:(MutPtr<id>)result { // NSString **
    skip_characters(env, this);

    let NSScannerHostObject { to_be_skipped, string, len, pos } = std::mem::take(env.objc.borrow_mut::<NSScannerHostObject>(this));
    log_dbg!("scanString:{} intoString: from '{}' at {}", to_rust_string(env, scan_string), to_rust_string(env, string), pos);

    // TODO: avoid string copying
    let left: id = msg![env; string substringFromIndex:pos];
    let same_prefix: bool = msg![env; left hasPrefix:scan_string];
    if !same_prefix {
        *env.objc.borrow_mut::<NSScannerHostObject>(this) = NSScannerHostObject { to_be_skipped, string, len, pos };
        return false;
    }

    let scan_len: NSUInteger = msg![env; scan_string length];
    assert!(pos + scan_len <= len);
    *env.objc.borrow_mut::<NSScannerHostObject>(this) = NSScannerHostObject { to_be_skipped, string, len, pos: pos + scan_len };

    if !result.is_null() {
        let copy: id = msg![env; scan_string copy];
        autorelease(env, copy);
        env.mem.write(result, copy);
    }
    true
}

- (bool)scanInt:(MutPtr<i32>)result {
    skip_characters(env, this);

    let NSScannerHostObject { to_be_skipped, string, len, pos } = std::mem::take(env.objc.borrow_mut::<NSScannerHostObject>(this));
    let left: id = msg![env; string substringFromIndex:pos];
    if left == nil {
        *env.objc.borrow_mut::<NSScannerHostObject>(this) = NSScannerHostObject { to_be_skipped, string, len, pos };
        return false;
    }

    let st = to_rust_string(env, left);
    let mut cutoff = 0;
    for (i, c) in st.char_indices() {
        if c.is_ascii_digit() || ((c == '+' || c == '-') && i == 0) {
            cutoff = i + 1;
        } else {
            break;
        }
    }
    if cutoff == 0 {
        log_dbg!("scanInt: no valid int found for '{}'", st);
        *env.objc.borrow_mut::<NSScannerHostObject>(this) = NSScannerHostObject { to_be_skipped, string, len, pos };
        return false;
    }

    if !result.is_null() {
        let res = st[..cutoff].parse().unwrap_or(0);
        log_dbg!("scanInt: from '{}' -> {}", st, res);
        env.mem.write(result, res);
    }

    *env.objc.borrow_mut::<NSScannerHostObject>(this) = NSScannerHostObject { to_be_skipped, string, len, pos: pos + cutoff as NSUInteger };
    true
}


- (bool)scanInteger:(MutPtr<NSInteger>)result {
    msg![env; this scanInt:result]
}
- (bool)scanLongLong:(MutPtr<i64>)result {
    scan_long_long(env, this, result)
}

- (bool)scanUnsignedLongLong:(MutPtr<u64>)result {
    scan_unsigned_long_long(env, this, result)
}

- (bool)scanFloat:(MutPtr<f32>)result {
    skip_characters(env, this);

    let NSScannerHostObject { to_be_skipped, string, len, pos } = std::mem::take(env.objc.borrow_mut::<NSScannerHostObject>(this));
    let left: id = msg![env; string substringFromIndex:pos];
    if left == nil {
        *env.objc.borrow_mut::<NSScannerHostObject>(this) = NSScannerHostObject { to_be_skipped, string, len, pos };
        return false;
    }

    let st = to_rust_string(env, left);
    let mut cutoff = 0;
    let mut seen_dot = false;
    let mut seen_digit = false;

    for (i, c) in st.char_indices() {
        if c.is_ascii_digit() {
            seen_digit = true;
            cutoff = i + 1;
        } else if c == '.' && !seen_dot {
            seen_dot = true;
            cutoff = i + 1;
        } else if (c == '-' || c == '+') && i == 0 {
            cutoff = i + 1;
        } else {
            break;
        }
    }

    if !seen_digit {
        log_dbg!("scanFloat: no valid float found for '{}'", st);
        *env.objc.borrow_mut::<NSScannerHostObject>(this) = NSScannerHostObject { to_be_skipped, string, len, pos };
        return false;
    }

    if !result.is_null() {
        let res: f32 = st[..cutoff].parse().unwrap_or(0.0);
        log_dbg!("scanFloat: from '{}' -> {}", st, res);
        env.mem.write(result, res);
    }

    *env.objc.borrow_mut::<NSScannerHostObject>(this) = NSScannerHostObject { to_be_skipped, string, len, pos: pos + cutoff as NSUInteger };
    true
}

- (bool)scanDouble:(MutPtr<f64>)result {
    skip_characters(env, this);

    let NSScannerHostObject { to_be_skipped, string, len, pos } = std::mem::take(env.objc.borrow_mut::<NSScannerHostObject>(this));
    let left: id = msg![env; string substringFromIndex:pos];
    if left == nil {
        *env.objc.borrow_mut::<NSScannerHostObject>(this) = NSScannerHostObject { to_be_skipped, string, len, pos };
        return false;
    }

    let st = to_rust_string(env, left);
    let mut cutoff = 0;
    let mut seen_dot = false;
    let mut seen_digit = false;

    for (i, c) in st.char_indices() {
        if c.is_ascii_digit() {
            seen_digit = true;
            cutoff = i + 1;
        } else if c == '.' && !seen_dot {
            seen_dot = true;
            cutoff = i + 1;
        } else if (c == '-' || c == '+') && i == 0 {
            cutoff = i + 1;
        } else {
            break;
        }
    }

    if !seen_digit {
        *env.objc.borrow_mut::<NSScannerHostObject>(this) = NSScannerHostObject { to_be_skipped, string, len, pos };
        return false;
    }

    if !result.is_null() {
        let res: f64 = st[..cutoff].parse().unwrap_or(0.0);
        env.mem.write(result, res);
    }

    *env.objc.borrow_mut::<NSScannerHostObject>(this) = NSScannerHostObject { to_be_skipped, string, len, pos: pos + cutoff as NSUInteger };
    true
}

@end

};

// Helper functions, skips characters from `charactersToBeSkipped` set
fn skip_characters(env: &mut Environment, scanner: id) {
    let &NSScannerHostObject {
        to_be_skipped,
        string,
        len,
        mut pos,
    } = env.objc.borrow::<NSScannerHostObject>(scanner);
    loop {
        if pos >= len {
            break;
        }
        let c: unichar = msg![env; string characterAtIndex:pos];
        if msg![env; to_be_skipped characterIsMember:c] {
            pos += 1;
        } else {
            break;
        }
    }
    env.objc.borrow_mut::<NSScannerHostObject>(scanner).pos = pos;
}

/// Scans an optionally signed run of decimal digits at the scanner's current
/// location, advancing past it on success and returning the matched text.
fn scan_digit_text(env: &mut Environment, scanner: id) -> Option<String> {
    skip_characters(env, scanner);

    let NSScannerHostObject { to_be_skipped, string, len, pos } =
        std::mem::take(env.objc.borrow_mut::<NSScannerHostObject>(scanner));
    let restore = |env: &mut Environment, next_pos: NSUInteger| {
        *env.objc.borrow_mut::<NSScannerHostObject>(scanner) =
            NSScannerHostObject { to_be_skipped, string, len, pos: next_pos };
    };
    let left: id = msg![env; string substringFromIndex:pos];
    if left == nil {
        restore(env, pos);
        return None;
    }

    let text = to_rust_string(env, left);
    let mut cutoff = 0;
    let mut saw_digit = false;
    for (i, c) in text.char_indices() {
        if c.is_ascii_digit() {
            saw_digit = true;
            cutoff = i + c.len_utf8();
        } else if (c == '+' || c == '-') && i == 0 {
            cutoff = c.len_utf8();
        } else {
            break;
        }
    }
    if !saw_digit {
        restore(env, pos);
        return None;
    }

    let matched = text[..cutoff].to_string();
    restore(env, pos + cutoff as NSUInteger);
    Some(matched)
}

fn scan_long_long(env: &mut Environment, scanner: id, result: MutPtr<i64>) -> bool {
    let Some(text) = scan_digit_text(env, scanner) else {
        return false;
    };
    if !result.is_null() {
        // Values that don't fit saturate, like `strtoll` does.
        let parsed = text
            .parse::<i64>()
            .unwrap_or_else(|_| if text.starts_with('-') { i64::MIN } else { i64::MAX });
        env.mem.write(result, parsed);
    }
    true
}

fn scan_unsigned_long_long(env: &mut Environment, scanner: id, result: MutPtr<u64>) -> bool {
    let Some(text) = scan_digit_text(env, scanner) else {
        return false;
    };
    if !result.is_null() {
        let parsed = if text.starts_with('-') {
            0
        } else {
            text.trim_start_matches('+')
                .parse::<u64>()
                .unwrap_or(u64::MAX)
        };
        env.mem.write(result, parsed);
    }
    true
}
