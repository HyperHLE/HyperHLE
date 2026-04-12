/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 * If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//!
//! `CFString` and `CFMutableString`.
//!
//! This is toll-free bridged to `NSString` and `NSMutableString` in
//! Apple's implementation.
//!
//! Here it is the same type.

use super::cf_allocator::{kCFAllocatorDefault, CFAllocatorRef};
use super::cf_array::CFArrayRef;
use super::cf_dictionary::CFDictionaryRef;
use super::cf_locale::CFLocaleRef;
use super::{kCFNotFound, CFComparisonResult, CFIndex, CFOptionFlags, CFRange, CFRelease, CFRetain, CFTypeRef};
use crate::abi::{DotDotDot, VaList};
use crate::dyld::{export_c_func, FunctionExports};
use crate::frameworks::foundation::{ns_string, unichar, NSNotFound, NSRange, NSUInteger};
use crate::mem::{ConstPtr, MutPtr, MutVoidPtr};
use crate::objc::{id, msg, msg_class, nil};
use crate::Environment;

pub type CFStringRef = super::CFTypeRef;
pub type CFMutableStringRef = CFStringRef;

// String encodings
pub type CFStringEncoding = u32;
pub const kCFStringEncodingMacRoman: CFStringEncoding = 0;
pub const kCFStringEncodingNextStepLatin: CFStringEncoding = 0x422;
pub const kCFStringEncodingASCII: CFStringEncoding = 0x600;
pub const kCFStringEncodingUTF8: CFStringEncoding = 0x8000100;
pub const kCFStringEncodingUnicode: CFStringEncoding = 0x100;
pub const kCFStringEncodingUTF16: CFStringEncoding = kCFStringEncodingUnicode;
pub const kCFStringEncodingUTF16BE: CFStringEncoding = 0x10000100;
pub const kCFStringEncodingUTF16LE: CFStringEncoding = 0x14000100;
pub const kCFStringEncodingISOLatin1: CFStringEncoding = 0x0201;
pub const kCFStringEncodingWindowsLatin1: CFStringEncoding = 0x0500;
pub const kCFStringEncodingUTF32: CFStringEncoding = 0x0c000100;
pub const kCFStringEncodingUTF32BE: CFStringEncoding = 0x18000100;
pub const kCFStringEncodingUTF32LE: CFStringEncoding = 0x1c000100;

// String normalization forms
pub type CFStringNormalizationForm = CFIndex;
pub const kCFStringNormalizationFormD: CFStringNormalizationForm = 0;
pub const kCFStringNormalizationFormKD: CFStringNormalizationForm = 1;
pub const kCFStringNormalizationFormC: CFStringNormalizationForm = 2;
pub const kCFStringNormalizationFormKC: CFStringNormalizationForm = 3;

// String comparison options (subset of NSStringCompareOptions)
pub const kCFCompareCaseInsensitive: CFOptionFlags = 1;
pub const kCFCompareBackwards: CFOptionFlags = 4;
pub const kCFCompareAnchored: CFOptionFlags = 8;
pub const kCFCompareNonliteral: CFOptionFlags = 16;
pub const kCFCompareLocalized: CFOptionFlags = 32;
pub const kCFCompareNumerically: CFOptionFlags = 64;

// Built-in string constants
pub const kCFStringTransformStripCombiningMarks: &str = "StringTransformStripCombiningMarks";
pub const kCFStringTransformToLatin: &str = "StringTransformToLatin";
pub const kCFStringTransformFullwidthHalfwidth: &str = "StringTransformFullwidthHalfwidth";
pub const kCFStringTransformLatinKatakana: &str = "StringTransformLatinKatakana";
pub const kCFStringTransformLatinHiragana: &str = "StringTransformLatinHiragana";
pub const kCFStringTransformHiraganaKatakana: &str = "StringTransformHiraganaKatakana";
pub const kCFStringTransformMandarinLatin: &str = "StringTransformMandarinLatin";
pub const kCFStringTransformLatinHangul: &str = "StringTransformLatinHangul";
pub const kCFStringTransformLatinArabic: &str = "StringTransformLatinArabic";
pub const kCFStringTransformLatinHebrew: &str = "StringTransformLatinHebrew";
pub const kCFStringTransformLatinThai: &str = "StringTransformLatinThai";
pub const kCFStringTransformLatinCyrillic: &str = "StringTransformLatinCyrillic";
pub const kCFStringTransformLatinGreek: &str = "StringTransformLatinGreek";
pub const kCFStringTransformToXMLHex: &str = "StringTransformToXMLHex";
pub const kCFStringTransformToUnicodeName: &str = "StringTransformToUnicodeName";
pub const kCFStringTransformStripDiacritics: &str = "StringTransformStripDiacritics";

type ConstStr255Param = ConstPtr<u8>;
type StringPtr = MutPtr<u8>;

// MARK: - Helper functions

fn validate_allocator(env: &mut Environment, allocator: CFAllocatorRef) -> bool {
    allocator == kCFAllocatorDefault ||
    allocator.is_null() || env.mem.read(allocator).is_system_default()
}

fn safe_cf_range_to_ns_range(range: CFRange) -> Option<NSRange> {
    if range.location < 0 ||
    range.length < 0 {
        return None;
    }
    Some(NSRange {
        location: range.location.try_into().ok()?,
        length: range.length.try_into().ok()?,
    })
}

// MARK: - Retain / Release

fn CFStringRetain(env: &mut Environment, str: CFStringRef) -> CFStringRef {
    if !str.is_null() {
        CFRetain(env, str)
    } else {
        str
    }
}

fn CFStringRelease(env: &mut Environment, str: CFStringRef) {
    if !str.is_null() {
        CFRelease(env, str);
    }
}

// MARK: - Encoding conversion

pub fn CFStringConvertEncodingToNSStringEncoding(
    _env: &mut Environment,
    encoding: CFStringEncoding,
) -> ns_string::NSStringEncoding {
    match encoding {
        kCFStringEncodingMacRoman => ns_string::NSMacOSRomanStringEncoding,
        kCFStringEncodingASCII => ns_string::NSASCIIStringEncoding,
        kCFStringEncodingUTF8 => ns_string::NSUTF8StringEncoding,
        kCFStringEncodingUTF16 |
        kCFStringEncodingUnicode => ns_string::NSUTF16StringEncoding,
        kCFStringEncodingUTF16BE => ns_string::NSUTF16BigEndianStringEncoding,
        kCFStringEncodingUTF16LE => ns_string::NSUTF16LittleEndianStringEncoding,
        kCFStringEncodingISOLatin1 => ns_string::NSISOLatin1StringEncoding,
        kCFStringEncodingNextStepLatin => ns_string::NSNextStepLatinStringEncoding,
        kCFStringEncodingWindowsLatin1 => ns_string::NSWindowsCP1252StringEncoding,
        kCFStringEncodingUTF32 => ns_string::NSUTF32StringEncoding,
        kCFStringEncodingUTF32BE => ns_string::NSUTF32BigEndianStringEncoding,
        kCFStringEncodingUTF32LE => ns_string::NSUTF32LittleEndianStringEncoding,
        _ => {
            log!("Warning: Unhandled CFStringEncoding {:#x}, defaulting to UTF-8", encoding);
            ns_string::NSUTF8StringEncoding
        }
    }
}

fn CFStringConvertNSStringEncodingToEncoding(
    _env: &mut Environment,
    encoding: ns_string::NSStringEncoding,
) -> CFStringEncoding {
    match encoding {
        ns_string::NSMacOSRomanStringEncoding => kCFStringEncodingMacRoman,
        ns_string::NSASCIIStringEncoding => kCFStringEncodingASCII,
        ns_string::NSUTF8StringEncoding => kCFStringEncodingUTF8,
        ns_string::NSUTF16StringEncoding => kCFStringEncodingUTF16,
        ns_string::NSUTF16BigEndianStringEncoding => kCFStringEncodingUTF16BE,
        ns_string::NSUTF16LittleEndianStringEncoding => kCFStringEncodingUTF16LE,
        ns_string::NSISOLatin1StringEncoding => kCFStringEncodingISOLatin1,
        ns_string::NSNextStepLatinStringEncoding => kCFStringEncodingNextStepLatin,
        ns_string::NSWindowsCP1252StringEncoding => kCFStringEncodingWindowsLatin1,
        ns_string::NSUTF32StringEncoding => kCFStringEncodingUTF32,
        ns_string::NSUTF32BigEndianStringEncoding => kCFStringEncodingUTF32BE,
        ns_string::NSUTF32LittleEndianStringEncoding => kCFStringEncodingUTF32LE,
        _ => {
            log!("Warning: Unhandled NSStringEncoding {:#x}, defaulting to UTF-8", encoding);
            kCFStringEncodingUTF8
        }
    }
}

fn CFStringIsEncodingAvailable(_env: &mut Environment, encoding: CFStringEncoding) -> bool {
    // Most common encodings are available
    matches!(
        encoding,
        kCFStringEncodingMacRoman
            | kCFStringEncodingASCII
            | kCFStringEncodingUTF8
            | kCFStringEncodingUTF16
            | kCFStringEncodingUTF16BE
            | kCFStringEncodingUTF16LE
            | kCFStringEncodingISOLatin1
            | kCFStringEncodingWindowsLatin1
            | kCFStringEncodingUTF32
            | kCFStringEncodingUTF32BE
            | kCFStringEncodingUTF32LE
            | kCFStringEncodingNextStepLatin
    )
}

fn CFStringGetSystemEncoding(_env: &mut Environment) -> CFStringEncoding {
    // Default system encoding
    kCFStringEncodingUTF8
}

fn CFStringGetMostCompatibleMacStringEncoding(
    _env: &mut Environment,
    encoding: CFStringEncoding,
) -> CFStringEncoding {
    // Return closest Mac encoding
    match encoding {
        kCFStringEncodingUTF8 |
        kCFStringEncodingUTF16 | kCFStringEncodingUTF16BE
        |
        kCFStringEncodingUTF16LE => kCFStringEncodingUTF8,
        kCFStringEncodingASCII => kCFStringEncodingASCII,
        _ => kCFStringEncodingMacRoman,
    }
}

// MARK: - Immutable constructors

fn CFStringCreateCopy(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    the_string: CFStringRef,
) -> CFStringRef {
    if !validate_allocator(env, allocator) {
        return nil;
    }
    
    if the_string.is_null() {
        return nil;
    }
    
    msg![env; the_string copy]
}

fn CFStringCreateWithBytes(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    bytes: ConstPtr<u8>,
    num_bytes: CFIndex,
    encoding: CFStringEncoding,
    is_external: bool,
) -> CFStringRef {
    if !validate_allocator(env, allocator) {
        return nil;
    }
    
    if bytes.is_null() ||
    num_bytes < 0 {
        return nil;
    }
    
    if num_bytes == 0 {
        return msg_class![env;
        NSString string];
    }
    
    // is_external representation not currently supported
    if is_external {
        log!("Warning: CFStringCreateWithBytes with is_external=true not fully supported");
    }
    
    let encoding = CFStringConvertEncodingToNSStringEncoding(env, encoding);
    let length: NSUInteger = num_bytes.try_into().unwrap_or(0);
    let ns_string: id = msg_class![env; NSString alloc];
    msg![env; ns_string initWithBytes:bytes length:length encoding:encoding]
}

fn CFStringCreateWithBytesNoCopy(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    bytes: ConstPtr<u8>,
    num_bytes: CFIndex,
    encoding: CFStringEncoding,
    is_external: bool,
    contents_deallocator: CFAllocatorRef,
) -> CFStringRef {
    if !validate_allocator(env, allocator) {
        return nil;
    }
    
    // For simplicity, we copy the bytes anyway
    // In a real implementation, this would avoid copying
    let _ = contents_deallocator;
    CFStringCreateWithBytes(env, allocator, bytes, num_bytes, encoding, is_external)
}

fn CFStringCreateWithCString(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    c_string: ConstPtr<u8>,
    encoding: CFStringEncoding,
) -> CFStringRef {
    if !validate_allocator(env, allocator) {
        return nil;
    }
    
    if c_string.is_null() {
        return nil;
    }
    
    let encoding = CFStringConvertEncodingToNSStringEncoding(env, encoding);
    let ns_string: id = msg_class![env; NSString alloc];
    msg![env; ns_string initWithCString:c_string encoding:encoding]
}

fn CFStringCreateWithCStringNoCopy(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    c_string: ConstPtr<u8>,
    encoding: CFStringEncoding,
    contents_deallocator: CFAllocatorRef,
) -> CFStringRef {
    if !validate_allocator(env, allocator) {
        return nil;
    }
    
    // For simplicity, we copy anyway
    let _ = contents_deallocator;
    CFStringCreateWithCString(env, allocator, c_string, encoding)
}

fn CFStringCreateWithCharacters(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    chars: ConstPtr<unichar>,
    num_chars: CFIndex,
) -> CFStringRef {
    if !validate_allocator(env, allocator) {
        return nil;
    }
    
    if chars.is_null() ||
    num_chars < 0 {
        return nil;
    }
    
    if num_chars == 0 {
        return msg_class![env;
        NSString string];
    }
    
    let length: NSUInteger = num_chars.try_into().unwrap_or(0);
    let ns_string: id = msg_class![env;
    NSString alloc];
    msg![env; ns_string initWithCharacters:chars length:length]
}

fn CFStringCreateWithCharactersNoCopy(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    chars: ConstPtr<unichar>,
    num_chars: CFIndex,
    contents_deallocator: CFAllocatorRef,
) -> CFStringRef {
    if !validate_allocator(env, allocator) {
        return nil;
    }
    
    // For simplicity, we copy anyway
    let _ = contents_deallocator;
    CFStringCreateWithCharacters(env, allocator, chars, num_chars)
}

fn CFStringCreateWithFormat(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    format_options: CFDictionaryRef,
    format: CFStringRef,
    args: DotDotDot,
) -> CFStringRef {
    CFStringCreateWithFormatAndArguments(env, allocator, format_options, format, args.start())
}

fn CFStringCreateWithFormatAndArguments(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    // Apple's own docs say format_options are unimplemented!
    _format_options: CFDictionaryRef,
    format: CFStringRef,
    args: VaList,
) -> CFStringRef {
    if !validate_allocator(env, allocator) {
        return nil;
    }
    
    if format.is_null() {
        return nil;
    }
    
    let res = ns_string::with_format(env, format, args);
    ns_string::from_rust_string(env, res)
}

fn CFStringCreateWithPascalString(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    p_str: ConstStr255Param,
    encoding: CFStringEncoding,
) -> CFStringRef {
    if !validate_allocator(env, allocator) {
        return nil;
    }
    
    if p_str.is_null() {
        return nil;
    }
    
    let len: CFIndex = env.mem.read(p_str).into();
    if len < 0 ||
    len > 255 {
        return nil;
    }
    
    let res = CFStringCreateWithBytes(env, allocator, p_str + 1, len, encoding, false);
    log_dbg!(
        "CFStringCreateWithPascalString(len={}, '{}')",
        len,
        if !res.is_null() {
            ns_string::to_rust_string(env, res)
        } else {
            std::borrow::Cow::Borrowed("<null>")
        }
    );
    res
}

fn CFStringCreateWithSubstring(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    string: CFStringRef,
    range: CFRange,
) -> CFStringRef {
    if !validate_allocator(env, allocator) {
        return nil;
    }
    
    if string.is_null() {
        return nil;
    }
    
    let ns_range = match safe_cf_range_to_ns_range(range) {
        Some(r) => r,
        None => return nil,
    };
    msg![env; string substringWithRange:ns_range]
}

fn CFStringCreateArrayBySeparatingStrings(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    string: CFStringRef,
    separator: CFStringRef,
) -> CFArrayRef {
    if !validate_allocator(env, allocator) {
        return nil;
    }
    
    if string.is_null() ||
    separator.is_null() {
        return nil;
    }
    
    msg![env;
    string componentsSeparatedByString:separator]
}

fn CFStringCreateByCombiningStrings(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    array: CFArrayRef,
    separator: CFStringRef,
) -> CFStringRef {
    if !validate_allocator(env, allocator) {
        return nil;
    }
    
    if array.is_null() ||
    separator.is_null() {
        return nil;
    }
    
    msg![env;
    array componentsJoinedByString:separator]
}

// MARK: - Mutable constructors

fn CFStringCreateMutable(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    max_length: CFIndex,
) -> CFMutableStringRef {
    if !validate_allocator(env, allocator) {
        return nil;
    }
    
    // max_length is typically ignored (0 means unlimited)
    let _ = max_length;
    msg_class![env; NSMutableString new]
}

fn CFStringCreateMutableCopy(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    max_length: CFIndex,
    the_string: CFStringRef,
) -> CFMutableStringRef {
    if !validate_allocator(env, allocator) {
        return nil;
    }
    
    if the_string.is_null() {
        return CFStringCreateMutable(env, allocator, max_length);
    }
    
    // max_length typically ignored
    let _ = max_length;
    msg![env;
    the_string mutableCopy]
}

fn CFStringCreateMutableWithExternalCharactersNoCopy(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    chars: MutPtr<unichar>,
    num_chars: CFIndex,
    capacity: CFIndex,
    external_characters_allocator: CFAllocatorRef,
) -> CFMutableStringRef {
    if !validate_allocator(env, allocator) {
        return nil;
    }
    
    // Not fully supported - create a regular mutable copy
    let _ = (chars, external_characters_allocator);
    let string = CFStringCreateMutable(env, allocator, capacity);
    
    if !string.is_null() && !chars.is_null() && num_chars > 0 {
        let temp = CFStringCreateWithCharacters(env, allocator, chars.cast_const(), num_chars);
        if !temp.is_null() {
            () = msg![env; string appendString:temp];
            CFRelease(env, temp);
        }
    }
    
    string
}

// MARK: - Queries

fn CFStringGetLength(env: &mut Environment, the_string: CFStringRef) -> CFIndex {
    if the_string.is_null() {
        return 0;
    }
    
    let length: NSUInteger = msg![env; the_string length];
    length.try_into().unwrap_or(0)
}

fn CFStringGetCharacterAtIndex(
    env: &mut Environment,
    the_string: CFStringRef,
    idx: CFIndex,
) -> unichar {
    if the_string.is_null() ||
    idx < 0 {
        return 0;
    }
    
    let length = CFStringGetLength(env, the_string);
    if idx >= length {
        return 0;
    }
    
    let idx_u: NSUInteger = idx.try_into().unwrap();
    msg![env;
    the_string characterAtIndex:idx_u]
}

fn CFStringGetCharacters(
    env: &mut Environment,
    string: CFStringRef,
    range: CFRange,
    buffer: MutPtr<unichar>,
) {
    if string.is_null() ||
    buffer.is_null() {
        return;
    }
    
    let ns_range = match safe_cf_range_to_ns_range(range) {
        Some(r) => r,
        None => return,
    };
    let length = CFStringGetLength(env, string);
    if range.location + range.length > length {
        return;
    }
    
    msg![env; string getCharacters:buffer range:ns_range]
}

fn CFStringGetCharacterFromInlineBuffer(
    env: &mut Environment,
    buf: MutVoidPtr,
    idx: CFIndex,
) -> unichar {
    // This would normally use an inline buffer cache
    // For simplicity, we extract the string and get the character
    // In real implementation, this would be optimized
    if buf.is_null() ||
    idx < 0 {
        return 0;
    }
    
    // The inline buffer structure would contain the string pointer
    // For now, we just return 0 as this is an optimization function
    log!("TODO: CFStringGetCharacterFromInlineBuffer not fully implemented");
    0
}

fn CFStringGetCString(
    env: &mut Environment,
    the_string: CFStringRef,
    buffer: MutPtr<u8>,
    buffer_size: CFIndex,
    encoding: CFStringEncoding,
) -> bool {
    if the_string.is_null() ||
    buffer.is_null() || buffer_size <= 0 {
        return false;
    }
    
    let encoding = CFStringConvertEncodingToNSStringEncoding(env, encoding);
    let buffer_size_u = buffer_size as NSUInteger;
    msg![env;
    the_string getCString:buffer maxLength:buffer_size_u encoding:encoding]
}

fn CFStringGetCStringPtr(
    env: &mut Environment,
    the_string: CFStringRef,
    encoding: CFStringEncoding,
) -> ConstPtr<u8> {
    if the_string.is_null() {
        return ConstPtr::null();
    }
    
    let encoding = CFStringConvertEncodingToNSStringEncoding(env, encoding);
    msg![env;
    the_string cStringUsingEncoding:encoding]
}

fn CFStringGetPascalString(
    env: &mut Environment,
    the_string: CFStringRef,
    buffer: StringPtr,
    buffer_size: CFIndex,
    encoding: CFStringEncoding,
) -> bool {
    if the_string.is_null() ||
    buffer.is_null() || buffer_size < 1 {
        return false;
    }
    
    log_dbg!(
        "CFStringGetPascalString('{}')",
        ns_string::to_rust_string(env, the_string)
    );
    let len = CFStringGetLength(env, the_string);
    
    // Pascal string needs length byte + content
    if (len + 1) > buffer_size ||
    len > 255 {
        return false;
    }
    
    let len_u8: u8 = len.try_into().unwrap_or(0);
    env.mem.write(buffer, len_u8);
    
    let encoding = CFStringConvertEncodingToNSStringEncoding(env, encoding);
    ns_string::get_bytes_buffer_inner(
        env,
        the_string,
        buffer + 1,
        len_u8.into(),
        encoding,
        false,
    )
}

fn CFStringGetBytes(
    env: &mut Environment,
    the_string: CFStringRef,
    range: CFRange,
    encoding: CFStringEncoding,
    loss_byte: u8,
    is_external_representation: bool,
    buffer: MutPtr<u8>,
    max_buf_len: CFIndex,
    used_buf_len: MutPtr<CFIndex>,
) -> CFIndex {
    if the_string.is_null() ||
    max_buf_len < 0 {
        return 0;
    }
    
    let ns_range = match safe_cf_range_to_ns_range(range) {
        Some(r) => r,
        None => return 0,
    };
    let length = CFStringGetLength(env, the_string);
    if range.location + range.length > length {
        return 0;
    }
    
    let encoding_ns = CFStringConvertEncodingToNSStringEncoding(env, encoding);
    
    let range_length = ns_range.length;

    // For simplicity, create a substring and get its bytes
    let substring: id = msg![env; the_string substringWithRange:ns_range];
    let mut options: NSUInteger = 0;
    if is_external_representation {
        options |= 1;
        // NSStringEncodingConversionExternalRepresentation
    }
    if loss_byte != 0 {
        options |= 2;
        // NSStringEncodingConversionAllowLossy
    }
    
    let max_len_u: NSUInteger = max_buf_len.try_into().unwrap_or(0);
    // Allocate a temporary guest pointer to hold the used length output
    let temp_used_len_ptr: MutPtr<NSUInteger> = env.mem.alloc(4).cast();
    env.mem.write(temp_used_len_ptr, 0);

    let null_ptr: MutPtr<NSRange> = MutPtr::null();
    let success: bool = if buffer.is_null() {
        // Just compute required length
        msg![env;
             substring getBytes:buffer 
             maxLength:max_len_u 
             usedLength:temp_used_len_ptr 
             encoding:encoding_ns 
             options:options 
             range:(NSRange { location: 0, length: range_length }) 
             remainingRange:null_ptr]
    } else {
          msg![env; substring getBytes:buffer 
             maxLength:max_len_u 
             usedLength:temp_used_len_ptr 
             encoding:encoding_ns 
             options:options 
             range:(NSRange { location: 0, length: range_length }) 
             remainingRange:null_ptr]
    };
    
    let used_len = env.mem.read(temp_used_len_ptr);
    env.mem.free(temp_used_len_ptr.cast());

    if !used_buf_len.is_null() {
        env.mem.write(used_buf_len, used_len.try_into().unwrap_or(0));
    }
    
    if success {
        used_len.try_into().unwrap_or(0)
    } else {
        0
    }
}

fn CFStringGetIntValue(env: &mut Environment, string: CFStringRef) -> i32 {
    if string.is_null() {
        return 0;
    }
    
    msg![env; string intValue]
}

fn CFStringGetDoubleValue(env: &mut Environment, string: CFStringRef) -> f64 {
    if string.is_null() {
        return 0.0;
    }
    
    msg![env; string doubleValue]
}

// MARK: - Searching

fn CFStringFind(
    env: &mut Environment,
    string: CFStringRef,
    to_find: CFStringRef,
    options: CFStringCompareFlags,
) -> CFRange {
    if string.is_null() ||
    to_find.is_null() {
        return CFRange {
            location: kCFNotFound,
            length: 0,
        };
    }
    
    let range: NSRange = msg![env; string rangeOfString:to_find options:options];
    let location: CFIndex = if range.location == NSNotFound as NSUInteger {
        kCFNotFound
    } else {
        range.location.try_into().unwrap_or(kCFNotFound)
    };
    CFRange {
        location,
        length: range.length.try_into().unwrap_or(0),
    }
}

fn CFStringFindWithOptions(
    env: &mut Environment,
    string: CFStringRef,
    to_find: CFStringRef,
    range_to_search: CFRange,
    options: CFStringCompareFlags,
    result: MutPtr<CFRange>,
) -> bool {
    if string.is_null() ||
    to_find.is_null() {
        return false;
    }
    
    let search_range = match safe_cf_range_to_ns_range(range_to_search) {
        Some(r) => r,
        None => return false,
    };
    let found_range: NSRange = msg![env; string rangeOfString:to_find options:options range:search_range];
    
    if found_range.location == NSNotFound as NSUInteger {
        return false;
    }
    
    if !result.is_null() {
        let cf_range = CFRange {
            location: found_range.location.try_into().unwrap_or(kCFNotFound),
            length: found_range.length.try_into().unwrap_or(0),
        };
        env.mem.write(result, cf_range);
    }
    
    true
}

fn CFStringFindCharacterFromSet(
    env: &mut Environment,
    string: CFStringRef,
    set: CFTypeRef, // CFCharacterSetRef
    range_to_search: CFRange,
    options: CFStringCompareFlags,
    result: MutPtr<CFRange>,
) -> bool {
    if string.is_null() ||
    set.is_null() {
        return false;
    }
    
    let search_range = match safe_cf_range_to_ns_range(range_to_search) {
        Some(r) => r,
        None => return false,
    };
    let found_range: NSRange = msg![env; string rangeOfCharacterFromSet:set options:options range:search_range];
    
    if found_range.location == NSNotFound as NSUInteger {
        return false;
    }
    
    if !result.is_null() {
        let cf_range = CFRange {
            location: found_range.location.try_into().unwrap_or(kCFNotFound),
            length: found_range.length.try_into().unwrap_or(0),
        };
        env.mem.write(result, cf_range);
    }
    
    true
}

// MARK: - Comparison

pub type CFStringCompareFlags = CFOptionFlags;
fn CFStringCompare(
    env: &mut Environment,
    a: CFStringRef,
    b: CFStringRef,
    flags: CFStringCompareFlags,
) -> CFComparisonResult {
    if a.is_null() && b.is_null() {
        return 0;
        // kCFCompareEqualTo
    }
    if a.is_null() {
        return -1;
        // kCFCompareLessThan
    }
    if b.is_null() {
        return 1;
        // kCFCompareGreaterThan
    }
    
    msg![env;
    a compare:b options:flags]
}

fn CFStringCompareWithOptions(
    env: &mut Environment,
    a: CFStringRef,
    b: CFStringRef,
    range: CFRange,
    flags: CFStringCompareFlags,
) -> CFComparisonResult {
    if a.is_null() ||
    b.is_null() {
        return if a.is_null() && b.is_null() {
            0
        } else if a.is_null() {
            -1
        } else {
            1
        };
    }
    
    let ns_range = match safe_cf_range_to_ns_range(range) {
        Some(r) => r,
        None => return 0,
    };
    // Create substring and compare
    let a_sub: id = msg![env; a substringWithRange:ns_range];
    msg![env;
    a_sub compare:b options:flags]
}

fn CFStringCompareWithOptionsAndLocale(
    env: &mut Environment,
    a: CFStringRef,
    b: CFStringRef,
    range: CFRange,
    flags: CFStringCompareFlags,
    locale: CFLocaleRef,
) -> CFComparisonResult {
    if a.is_null() ||
    b.is_null() {
        return if a.is_null() && b.is_null() {
            0
        } else if a.is_null() {
            -1
        } else {
            1
        };
    }
    
    let ns_range = match safe_cf_range_to_ns_range(range) {
        Some(r) => r,
        None => return 0,
    };
    let a_sub: id = msg![env; a substringWithRange:ns_range];
    
    if locale.is_null() {
        msg![env;
        a_sub compare:b options:flags]
    } else {
        msg![env;
        a_sub compare:b options:flags range:(NSRange { location: 0, length: msg![env; b length] }) locale:locale]
    }
}

fn CFStringHasPrefix(env: &mut Environment, the_string: CFStringRef, prefix: CFStringRef) -> bool {
    if the_string.is_null() ||
    prefix.is_null() {
        return false;
    }
    
    msg![env;
    the_string hasPrefix:prefix]
}

fn CFStringHasSuffix(env: &mut Environment, the_string: CFStringRef, suffix: CFStringRef) -> bool {
    if the_string.is_null() ||
    suffix.is_null() {
        return false;
    }
    
    msg![env;
    the_string hasSuffix:suffix]
}

// MARK: - Mutation

fn CFStringAppend(
    env: &mut Environment,
    the_string: CFMutableStringRef,
    appended_string: CFStringRef,
) {
    if the_string.is_null() ||
    appended_string.is_null() {
        return;
    }
    
    () = msg![env;
    the_string appendString:appended_string]
}

fn CFStringAppendCString(
    env: &mut Environment,
    string: CFMutableStringRef,
    c_string: ConstPtr<u8>,
    encoding: CFStringEncoding,
) {
    if string.is_null() ||
    c_string.is_null() {
        return;
    }
    
    let encoding = CFStringConvertEncodingToNSStringEncoding(env, encoding);
    let to_append: id = msg_class![env;
    NSString stringWithCString:c_string encoding:encoding];
    
    if !to_append.is_null() {
        () = msg![env; string appendString:to_append];
    }
}

fn CFStringAppendCharacters(
    env: &mut Environment,
    the_string: CFMutableStringRef,
    chars: ConstPtr<unichar>,
    num_chars: CFIndex,
) {
    if the_string.is_null() ||
    chars.is_null() || num_chars <= 0 {
        return;
    }
    
    let temp = CFStringCreateWithCharacters(env, kCFAllocatorDefault, chars, num_chars);
    if !temp.is_null() {
        () = msg![env; the_string appendString:temp];
        CFRelease(env, temp);
    }
}

fn CFStringAppendFormat(
    env: &mut Environment,
    string: CFMutableStringRef,
    _format_options: CFDictionaryRef,
    format: CFStringRef,
    dots: DotDotDot,
) {
    if string.is_null() ||
    format.is_null() {
        return;
    }
    
    let res = ns_string::with_format(env, format, dots.start());
    let to_append: id = ns_string::from_rust_string(env, res);
    if !to_append.is_null() {
        () = msg![env; string appendString:to_append];
    }
}

fn CFStringInsert(
    env: &mut Environment,
    string: CFMutableStringRef,
    idx: CFIndex,
    inserted_str: CFStringRef,
) {
    if string.is_null() ||
    inserted_str.is_null() || idx < 0 {
        return;
    }
    
    let length = CFStringGetLength(env, string);
    if idx > length {
        return;
    }
    
    let idx_u: NSUInteger = idx.try_into().unwrap();
    () = msg![env; string insertString:inserted_str atIndex:idx_u];
}

fn CFStringDelete(
    env: &mut Environment, 
    string: CFMutableStringRef, 
    range: CFRange,
) {
    if string.is_null() {
        return;
    }
    
    let ns_range = match safe_cf_range_to_ns_range(range) {
        Some(r) => r,
        None => return,
    };
    let length = CFStringGetLength(env, string);
    if range.location + range.length > length {
        return;
    }
    
    () = msg![env; string deleteCharactersInRange:ns_range];
}

fn CFStringReplace(
    env: &mut Environment,
    string: CFMutableStringRef,
    range: CFRange,
    replacement: CFStringRef,
) {
    if string.is_null() ||
    replacement.is_null() {
        return;
    }
    
    let ns_range = match safe_cf_range_to_ns_range(range) {
        Some(r) => r,
        None => return,
    };
    let length = CFStringGetLength(env, string);
    if range.location + range.length > length {
        return;
    }
    
    () = msg![env; string replaceCharactersInRange:ns_range withString:replacement];
}

fn CFStringReplaceAll(
    env: &mut Environment,
    string: CFMutableStringRef,
    replacement: CFStringRef,
) {
    if string.is_null() ||
    replacement.is_null() {
        return;
    }
    
    () = msg![env; string setString:replacement];
}

fn CFStringFindAndReplace(
    env: &mut Environment,
    string: CFMutableStringRef,
    string_to_find: CFStringRef,
    replacement_string: CFStringRef,
    range_to_search: CFRange,
    compare_options: CFStringCompareFlags,
) -> CFIndex {
    if string.is_null() ||
    string_to_find.is_null() || replacement_string.is_null() {
        return 0;
    }
    
    let ns_range = match safe_cf_range_to_ns_range(range_to_search) {
        Some(r) => r,
        None => return 0,
    };
    let length = CFStringGetLength(env, string);
    if range_to_search.location + range_to_search.length > length {
        return 0;
    }
    
    let count: NSUInteger = msg![env;
    string 
        replaceOccurrencesOfString:string_to_find 
        withString:replacement_string 
        options:compare_options 
        range:ns_range];
    count.try_into().unwrap_or(0)
}

fn CFStringPad(
    env: &mut Environment,
    string: CFMutableStringRef,
    pad_string: CFStringRef,
    length: CFIndex,
    index_into_pad: CFIndex,
) {
    if string.is_null() ||
    pad_string.is_null() || length < 0 || index_into_pad < 0 {
        return;
    }
    
    let current_len = CFStringGetLength(env, string);
    if current_len >= length {
        return;
        // Already long enough
    }
    
    let pad_len = CFStringGetLength(env, pad_string);
    if pad_len == 0 {
        return;
    }
    
    let needed = length - current_len;
    let mut padded = 0;
    while padded < needed {
        let start_idx = index_into_pad % pad_len;
        let chars_to_add = (needed - padded).min(pad_len - start_idx);
        
        let range = CFRange {
            location: start_idx,
            length: chars_to_add,
        };
        let substring: id = msg![env; pad_string substringWithRange:(safe_cf_range_to_ns_range(range).unwrap())];
        () = msg![env; string appendString:substring];
        
        padded += chars_to_add;
    }
}

fn CFStringTrim(
    env: &mut Environment,
    string: CFMutableStringRef,
    trim_string: CFStringRef,
) {
    if string.is_null() ||
    trim_string.is_null() {
        return;
    }
    
    // Create character set first, then use it
    let char_set: id = msg_class![env;
    NSCharacterSet characterSetWithCharactersInString:trim_string];
    let trimmed: id = msg![env; string stringByTrimmingCharactersInSet:char_set];
    () = msg![env; string setString:trimmed];
}

fn CFStringTrimWhitespace(env: &mut Environment, string: CFMutableStringRef) {
    if string.is_null() {
        return;
    }
    
    let whitespace_set: id = msg_class![env; NSCharacterSet whitespaceAndNewlineCharacterSet];
    let trimmed: id = msg![env;
    string stringByTrimmingCharactersInSet:whitespace_set];
    () = msg![env; string setString:trimmed];
}

// MARK: - Case transformations

fn CFStringLowercase(
    env: &mut Environment,
    string: CFMutableStringRef,
    _locale: CFLocaleRef,
) {
    if string.is_null() {
        return;
    }
    
    // TODO: account for locale
    let lowercase: id = msg![env;
    string lowercaseString];
    () = msg![env; string setString:lowercase];
}

fn CFStringUppercase(
    env: &mut Environment,
    string: CFMutableStringRef,
    _locale: CFLocaleRef,
) {
    if string.is_null() {
        return;
    }
    
    // TODO: account for locale
    let uppercase: id = msg![env;
    string uppercaseString];
    () = msg![env; string setString:uppercase];
}

fn CFStringCapitalize(
    env: &mut Environment,
    string: CFMutableStringRef,
    _locale: CFLocaleRef,
) {
    if string.is_null() {
        return;
    }
    
    // TODO: account for locale
    let capitalized: id = msg![env;
    string capitalizedString];
    () = msg![env; string setString:capitalized];
}

// MARK: - Normalization and transformation

fn CFStringNormalize(
    env: &mut Environment,
    the_string: CFMutableStringRef,
    the_form: CFStringNormalizationForm,
) {
    if the_string.is_null() {
        return;
    }
    
    let str_content = ns_string::to_rust_string(env, the_string);
    // Basic normalization forms
    match the_form {
        kCFStringNormalizationFormD => {
            log!("TODO: Full CFStringNormalize FormD for '{}'", str_content);
            // NFD - Canonical Decomposition
            // For ASCII, this is a no-op
        }
        kCFStringNormalizationFormKD => {
            log!("TODO: Full CFStringNormalize FormKD for '{}'", str_content);
            // NFKD - Compatibility Decomposition
        }
        kCFStringNormalizationFormC => {
            log!("TODO: Full CFStringNormalize FormC for '{}'", str_content);
            // NFC - Canonical Composition
        }
        kCFStringNormalizationFormKC => {
            log!("TODO: Full CFStringNormalize FormKC for '{}'", str_content);
            // NFKC - Compatibility Composition
        }
        _ => {
            log!("Unknown normalization form: {}", the_form);
        }
    }
}

fn CFStringTransform(
    env: &mut Environment,
    string: CFMutableStringRef,
    range: MutPtr<CFRange>,
    transform: CFStringRef,
    reverse: bool,
) -> bool {
    if string.is_null() ||
    transform.is_null() {
        return false;
    }
    
    let transform_name = ns_string::to_rust_string(env, transform);
    log!("TODO: CFStringTransform('{}', reverse={})", transform_name, reverse);
    // For now, basic implementation of common transforms
    match transform_name.as_ref() {
        kCFStringTransformStripDiacritics |
        kCFStringTransformStripCombiningMarks => {
            // Strip accents/diacritics - approximate implementation
            let folded: id = msg![env;
            string 
                stringByFoldingWithOptions:128 // NSCaseInsensitiveSearch + NSDiacriticInsensitiveSearch
                locale:nil];
            () = msg![env; string setString:folded];
            true
        }
        kCFStringTransformToLatin => {
            // For non-Latin scripts, transliterate to Latin
            // This is very complex - just log for now
            false
        }
        _ => {
        
            false
        }
    }
}

// MARK: - Type info

fn CFStringGetTypeID(_env: &mut Environment) -> u32 {
    // Return a fake CFTypeID for CFString
    0x43465374 // 'CFSt' in hex
}

// MARK: - Exports

pub const FUNCTIONS: FunctionExports = &[
    // Lifecycle
    export_c_func!(CFStringRetain(_)),
    export_c_func!(CFStringRelease(_)),
    
    // Encoding
    export_c_func!(CFStringConvertEncodingToNSStringEncoding(_)),
    export_c_func!(CFStringConvertNSStringEncodingToEncoding(_)),
    export_c_func!(CFStringIsEncodingAvailable(_)),
    export_c_func!(CFStringGetSystemEncoding()),
  
    export_c_func!(CFStringGetMostCompatibleMacStringEncoding(_)),
    
  
    // Immutable constructors
    export_c_func!(CFStringCreateCopy(_, _)),
    export_c_func!(CFStringCreateWithBytes(_, _, _, _, _)),
    export_c_func!(CFStringCreateWithBytesNoCopy(_, _, _, _, _, _)),
    export_c_func!(CFStringCreateWithCString(_, _, _)),
    export_c_func!(CFStringCreateWithCStringNoCopy(_, _, _, _)),
    export_c_func!(CFStringCreateWithCharacters(_, _, _)),
    export_c_func!(CFStringCreateWithCharactersNoCopy(_, _, _, _)),
    export_c_func!(CFStringCreateWithFormat(_, _, _, _)),
    export_c_func!(CFStringCreateWithFormatAndArguments(_, _, _, _)),
    export_c_func!(CFStringCreateWithPascalString(_, _, _)),
    export_c_func!(CFStringCreateWithSubstring(_, _, _)),
    export_c_func!(CFStringCreateArrayBySeparatingStrings(_, _, _)),
    export_c_func!(CFStringCreateByCombiningStrings(_, _, _)),
    
    // Mutable constructors
    export_c_func!(CFStringCreateMutable(_, _)),
    export_c_func!(CFStringCreateMutableCopy(_, _, _)),
    export_c_func!(CFStringCreateMutableWithExternalCharactersNoCopy(_, _, _, _, _)),
    
    // Queries
    export_c_func!(CFStringGetLength(_)),
    export_c_func!(CFStringGetCharacterAtIndex(_, _)),
    export_c_func!(CFStringGetCharacters(_, _, _)),
    export_c_func!(CFStringGetCharacterFromInlineBuffer(_, _)),
    export_c_func!(CFStringGetCString(_, _, _, _)),
    export_c_func!(CFStringGetCStringPtr(_, _)),
    export_c_func!(CFStringGetPascalString(_, _, _, _)),
    export_c_func!(CFStringGetBytes(_, _, _, _, _, _, _, _)),
    export_c_func!(CFStringGetIntValue(_)),
    export_c_func!(CFStringGetDoubleValue(_)),
    
    // Searching
    export_c_func!(CFStringFind(_, _, _)),
    export_c_func!(CFStringFindWithOptions(_, _, _, _, _)),
    export_c_func!(CFStringFindCharacterFromSet(_, _, _, _, _)),
    
    // Comparison
    export_c_func!(CFStringCompare(_, _, _)),
    export_c_func!(CFStringCompareWithOptions(_, _, _, _)),
    export_c_func!(CFStringCompareWithOptionsAndLocale(_, _, _, _, _)),
    export_c_func!(CFStringHasPrefix(_, _)),
    export_c_func!(CFStringHasSuffix(_, _)),
    
    // Mutation
    export_c_func!(CFStringAppend(_, _)),
    export_c_func!(CFStringAppendCString(_, _, _)),
    export_c_func!(CFStringAppendCharacters(_, _, _)),
    export_c_func!(CFStringAppendFormat(_, _, _, _)),
    export_c_func!(CFStringInsert(_, _, _)),
    export_c_func!(CFStringDelete(_, _)),
    export_c_func!(CFStringReplace(_, _, _)),
    export_c_func!(CFStringReplaceAll(_, _)),
    export_c_func!(CFStringFindAndReplace(_, _, _, _, _)),
    export_c_func!(CFStringPad(_, _, _, _)),
    export_c_func!(CFStringTrim(_, _)),
    export_c_func!(CFStringTrimWhitespace(_)),
    
    // Case transformations
    export_c_func!(CFStringLowercase(_, _)),
    export_c_func!(CFStringUppercase(_, _)),
    export_c_func!(CFStringCapitalize(_, _)),
    
    // Normalization and transformation
    export_c_func!(CFStringNormalize(_, _)),
    export_c_func!(CFStringTransform(_, _, _, _)),
    
  
    // Type info
    export_c_func!(CFStringGetTypeID()),
];

