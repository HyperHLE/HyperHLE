/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! The `NSCharacterSet` class cluster, including `NSMutableCharacterSet`.

use super::{ns_string, unichar};
use crate::frameworks::foundation::NSRange;
use crate::objc::{
    autorelease, id, msg, msg_class, nil, objc_classes, retain, ClassExports, HostObject,
    NSZonePtr,
};
use std::collections::HashSet;

// Unicode General Category Zs and CHARACTER TABULATION (U+0009).
const WHITESPACE_CHARACTERS: [char; 18] = [
    '\u{0020}', '\u{00A0}', '\u{1680}', '\u{2000}', '\u{2001}', '\u{2002}', '\u{2003}', '\u{2004}',
    '\u{2005}', '\u{2006}', '\u{2007}', '\u{2008}', '\u{2009}', '\u{200A}', '\u{202F}', '\u{205F}',
    '\u{3000}', '\u{0009}',
];
// The newline characters (U+000A - U+000D, U+0085, U+2028, and U+2029).
const NEWLINE_CHARACTERS: [char; 7] = [
    '\u{000A}', '\u{000B}', '\u{000C}', '\u{000D}', '\u{0085}', '\u{2028}', '\u{2029}',
];

// =========================================================================
// MARK: - Helpers for building sets from Unicode ranges
// =========================================================================

fn ascii_range_set(ranges: &[(u32, u32)]) -> HashSet<unichar> {
    unicode_range_set(ranges)
}

fn unicode_range_set(ranges: &[(u32, u32)]) -> HashSet<unichar> {
    let mut set = HashSet::new();
    for &(lo, hi) in ranges {
        for cp in lo..=hi {
            // Only BMP (plane 0) code points can be represented as a single
            // UTF-16 code unit; ignore anything else.
            if let Ok(uc) = unichar::try_from(cp) {
                set.insert(uc);
            }
        }
    }
    set
}

// =========================================================================
// MARK: - Host object
// =========================================================================

#[derive(Default)]
struct CharacterSetHostObject {
    set: HashSet<unichar>,
    inverted: bool,
}
impl HostObject for CharacterSetHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

// =========================================================================
// MARK: - NSCharacterSet
// =========================================================================

@implementation NSCharacterSet: NSObject

+ (id)allocWithZone:(NSZonePtr)zone {
    assert!(this == env.objc.get_known_class("NSCharacterSet", &mut env.mem));
    msg_class![env; _touchHLE_NSCharacterSet allocWithZone:zone]
}

// MARK: Standard character sets

+ (id)characterSetWithCharactersInString:(id)string { // NSString*
    let mut set = HashSet::new();
    ns_string::for_each_code_unit(env, string, |_idx, c| { set.insert(c); });
    let new: id = msg![env; this alloc];
    env.objc.borrow_mut::<CharacterSetHostObject>(new).set = set;
    autorelease(env, new)
}

+ (id)characterSetWithRange:(NSRange)range {
    // NSRange here is { location: first_codepoint, length: count }
    let set = if range.length == 0 {
        HashSet::new()
    } else {
        let end = range.location.saturating_add(range.length);
        unicode_range_set(&[(range.location, end - 1)])
    };
    let new: id = msg![env; this alloc];
    env.objc.borrow_mut::<CharacterSetHostObject>(new).set = set;
    autorelease(env, new)
}

+ (id)alphanumericCharacterSet {
    // A-Z, a-z, 0-9 plus Unicode letters and digits (approximated to BMP).
    let mut set = ascii_range_set(&[
        (b'A' as u32, b'Z' as u32),
        (b'a' as u32, b'z' as u32),
        (b'0' as u32, b'9' as u32),
    ]);
    // Add common Unicode letter ranges (Latin Extended, etc.)
    for s in unicode_range_set(&[
        (0x00C0, 0x00FF), // Latin Extended-A/B
        (0x0100, 0x017F),
        (0x0180, 0x024F),
        (0x0370, 0x03FF), // Greek
        (0x0400, 0x04FF), // Cyrillic
        (0x4E00, 0x9FFF), // CJK Unified Ideographs (subset)
    ]) {
        set.insert(s);
    }
    let new: id = msg![env; this alloc];
    env.objc.borrow_mut::<CharacterSetHostObject>(new).set = set;
    autorelease(env, new)
}

+ (id)letterCharacterSet {
    let mut set = ascii_range_set(&[
        (b'A' as u32, b'Z' as u32),
        (b'a' as u32, b'z' as u32),
    ]);
    for s in unicode_range_set(&[
        (0x00C0, 0x00FF),
        (0x0100, 0x017F),
        (0x0180, 0x024F),
        (0x0370, 0x03FF),
        (0x0400, 0x04FF),
    ]) {
        set.insert(s);
    }
    let new: id = msg![env; this alloc];
    env.objc.borrow_mut::<CharacterSetHostObject>(new).set = set;
    autorelease(env, new)
}

+ (id)lowercaseLetterCharacterSet {
    let mut set = ascii_range_set(&[(b'a' as u32, b'z' as u32)]);
    for s in unicode_range_set(&[
        (0x00DF, 0x00F6), (0x00F8, 0x00FF),
        (0x0101, 0x012F), (0x0131, 0x0131),
    ]) {
        set.insert(s);
    }
    let new: id = msg![env; this alloc];
    env.objc.borrow_mut::<CharacterSetHostObject>(new).set = set;
    autorelease(env, new)
}

+ (id)uppercaseLetterCharacterSet {
    let mut set = ascii_range_set(&[(b'A' as u32, b'Z' as u32)]);
    for s in unicode_range_set(&[
        (0x00C0, 0x00D6), (0x00D8, 0x00DE),
        (0x0100, 0x012E), (0x0130, 0x0130),
    ]) {
        set.insert(s);
    }
    let new: id = msg![env; this alloc];
    env.objc.borrow_mut::<CharacterSetHostObject>(new).set = set;
    autorelease(env, new)
}

+ (id)decimalDigitCharacterSet {
    let set = ascii_range_set(&[(b'0' as u32, b'9' as u32)]);
    let new: id = msg![env; this alloc];
    env.objc.borrow_mut::<CharacterSetHostObject>(new).set = set;
    autorelease(env, new)
}

+ (id)newlineCharacterSet {
    let set = HashSet::from(NEWLINE_CHARACTERS.map(|c| unichar::try_from(c as u16).unwrap()));
    let new: id = msg![env; this alloc];
    env.objc.borrow_mut::<CharacterSetHostObject>(new).set = set;
    autorelease(env, new)
}

+ (id)whitespaceCharacterSet {
    let set = HashSet::from(WHITESPACE_CHARACTERS.map(|c| unichar::try_from(c as u16).unwrap()));
    let new: id = msg![env; this alloc];
    env.objc.borrow_mut::<CharacterSetHostObject>(new).set = set;
    autorelease(env, new)
}

+ (id)whitespaceAndNewlineCharacterSet {
    let set1: HashSet<unichar> = HashSet::from(NEWLINE_CHARACTERS.map(|c| unichar::try_from(c as u16).unwrap()));
    let set2: HashSet<unichar> = HashSet::from(WHITESPACE_CHARACTERS.map(|c| unichar::try_from(c as u16).unwrap()));
    let set = set1.union(&set2).copied().collect();
    let new: id = msg![env; this alloc];
    env.objc.borrow_mut::<CharacterSetHostObject>(new).set = set;
    autorelease(env, new)
}

+ (id)punctuationCharacterSet {
    let set = ascii_range_set(&[
        (0x0021, 0x002F), // !"#$%&'()*+,-./
        (0x003A, 0x0040), // :;<=>?@
        (0x005B, 0x0060), // [\]^_`
        (0x007B, 0x007E), // {|}~
    ]);
    let new: id = msg![env; this alloc];
    env.objc.borrow_mut::<CharacterSetHostObject>(new).set = set;
    autorelease(env, new)
}

+ (id)symbolCharacterSet {
    let set = unicode_range_set(&[
        (0x2000, 0x206F), // General Punctuation
        (0x2100, 0x214F), // Letterlike Symbols
        (0x2190, 0x21FF), // Arrows
        (0x2200, 0x22FF), // Mathematical Operators
        (0x2300, 0x23FF), // Miscellaneous Technical
        (0x25A0, 0x25FF), // Geometric Shapes
        (0x2600, 0x26FF), // Miscellaneous Symbols
        (0x2700, 0x27BF), // Dingbats
    ]);
    let new: id = msg![env; this alloc];
    env.objc.borrow_mut::<CharacterSetHostObject>(new).set = set;
    autorelease(env, new)
}

+ (id)controlCharacterSet {
    let mut set = ascii_range_set(&[
        (0x0000, 0x001F), // C0 controls
        (0x007F, 0x009F), // DEL + C1 controls
    ]);
    set.insert(unichar::try_from(0x007Fu16).unwrap()); // DEL
    let new: id = msg![env; this alloc];
    env.objc.borrow_mut::<CharacterSetHostObject>(new).set = set;
    autorelease(env, new)
}

+ (id)nonBaseCharacterSet {
    // Combining characters (approximate — marks and combining diacritics).
    let set = unicode_range_set(&[
        (0x0300, 0x036F), // Combining Diacritical Marks
        (0x1DC0, 0x1DFF), // Combining Diacritical Marks Supplement
        (0x20D0, 0x20FF), // Combining Diacritical Marks for Symbols
        (0xFE20, 0xFE2F), // Combining Half Marks
    ]);
    let new: id = msg![env; this alloc];
    env.objc.borrow_mut::<CharacterSetHostObject>(new).set = set;
    autorelease(env, new)
}

+ (id)decomposableCharacterSet {
    // Per Apple's documentation, this set contains individual Unicode
    // characters that can also be represented as composed character
    // sequences (e.g. letters with accents), by the definition of
    // "standard decomposition" in the Unicode standard.
    // This is an approximation covering the most common precomposed
    // ranges in the BMP.
    let mut set = unicode_range_set(&[
        (0x1E00, 0x1EFF), // Latin Extended Additional (all precomposed)
        (0x1F00, 0x1FFE), // Greek Extended (virtually all precomposed)
        (0xAC00, 0xD7A3), // Hangul Syllables (all canonically decomposable)
    ]);
    // Kana with (semi-)voiced sound marks (が, ぱ, ヴ, …): the precomposed
    // forms alternate with the base forms in the Hiragana/Katakana blocks.
    for &cp in &[
        0x304Cu32, 0x304E, 0x3050, 0x3052, 0x3054, 0x3056, 0x3058, 0x305A,
        0x305C, 0x305E, 0x3060, 0x3062, 0x3065, 0x3067, 0x3069, 0x3070,
        0x3071, 0x3073, 0x3074, 0x3076, 0x3077, 0x3079, 0x307A, 0x307C,
        0x307D, 0x3094, 0x30AC, 0x30AE, 0x30B0, 0x30B2, 0x30B4, 0x30B6,
        0x30B8, 0x30BA, 0x30BC, 0x30BE, 0x30C0, 0x30C2, 0x30C5, 0x30C7,
        0x30C9, 0x30D0, 0x30D1, 0x30D3, 0x30D4, 0x30D6, 0x30D7, 0x30D9,
        0x30DA, 0x30DC, 0x30DD, 0x30F4, 0x30F7, 0x30F8, 0x30F9, 0x30FA,
    ] {
        set.insert(cp as unichar);
    }
    // Latin-1 letters with diacritics (excluding Æ, Ð, ×, Ø, Þ, ß, æ, ð,
    // ÷, ø, þ, ÿ has a decomposition so it *is* included).
    for cp in 0x00C0u32..=0x00FF {
        if matches!(cp, 0x00C6 | 0x00D0 | 0x00D7 | 0x00D8 | 0x00DE | 0x00DF
                      | 0x00E6 | 0x00F0 | 0x00F7 | 0x00F8 | 0x00FE) {
            continue;
        }
        set.insert(cp as unichar);
    }
    // Latin Extended-A, excluding the letters with no canonical
    // decomposition (Đđ, Ħħ, ı, ĸ, Łł, ŋŊ, Œœ, Ŧŧ, ſ).
    for cp in 0x0100u32..=0x017F {
        if matches!(cp, 0x0110 | 0x0111 | 0x0126 | 0x0127 | 0x0131 | 0x0138
                      | 0x0141 | 0x0142 | 0x014A | 0x014B | 0x0152 | 0x0153
                      | 0x0166 | 0x0167 | 0x017F) {
            continue;
        }
        set.insert(cp as unichar);
    }
    // Greek letters with diacritics.
    for &cp in &[0x0386u32, 0x0388, 0x0389, 0x038A, 0x038C, 0x038E, 0x038F, 0x0390] {
        set.insert(cp as unichar);
    }
    for cp in 0x03AAu32..=0x03B0 {
        set.insert(cp as unichar);
    }
    for cp in 0x03CAu32..=0x03CE {
        set.insert(cp as unichar);
    }
    // Cyrillic letters with diacritics (Ѐ, Ё, Ѓ, Ї, Ќ, Ѝ, Ў, Й and the
    // corresponding lowercase letters).
    for &cp in &[
        0x0400u32, 0x0401, 0x0403, 0x0407, 0x040C, 0x040D, 0x040E, 0x0419,
        0x0439, 0x0450, 0x0451, 0x0453, 0x0457, 0x045C, 0x045D, 0x045E,
    ] {
        set.insert(cp as unichar);
    }

    let new: id = msg![env; this alloc];
    env.objc.borrow_mut::<CharacterSetHostObject>(new).set = set;
    autorelease(env, new)
}

+ (id)illegalCharacterSet {
    // Surrogates and non-characters.
    let set = unicode_range_set(&[
        (0xD800, 0xDFFF), // Surrogates
        (0xFDD0, 0xFDEF), // Non-characters
        (0xFFFE, 0xFFFF), // BOM / non-character
    ]);
    let new: id = msg![env; this alloc];
    env.objc.borrow_mut::<CharacterSetHostObject>(new).set = set;
    autorelease(env, new)
}

+ (id)URLHostAllowedCharacterSet {
    // RFC 3986 host characters.
    let mut set = ascii_range_set(&[
        (b'A' as u32, b'Z' as u32),
        (b'a' as u32, b'z' as u32),
        (b'0' as u32, b'9' as u32),
    ]);
    for ch in b"-._~!$&'()*+,;=[]:" {
        set.insert(unichar::try_from(*ch as u16).unwrap());
    }
    let new: id = msg![env; this alloc];
    env.objc.borrow_mut::<CharacterSetHostObject>(new).set = set;
    autorelease(env, new)
}

+ (id)URLPathAllowedCharacterSet {
    let mut set = ascii_range_set(&[
        (b'A' as u32, b'Z' as u32),
        (b'a' as u32, b'z' as u32),
        (b'0' as u32, b'9' as u32),
    ]);
    for ch in b"-._~!$&'()*+,;=:@/" {
        set.insert(unichar::try_from(*ch as u16).unwrap());
    }
    let new: id = msg![env; this alloc];
    env.objc.borrow_mut::<CharacterSetHostObject>(new).set = set;
    autorelease(env, new)
}

+ (id)URLQueryAllowedCharacterSet {
    let mut set = ascii_range_set(&[
        (b'A' as u32, b'Z' as u32),
        (b'a' as u32, b'z' as u32),
        (b'0' as u32, b'9' as u32),
    ]);
    for ch in b"-._~!$&'()*+,;=:@/?%" {
        set.insert(unichar::try_from(*ch as u16).unwrap());
    }
    let new: id = msg![env; this alloc];
    env.objc.borrow_mut::<CharacterSetHostObject>(new).set = set;
    autorelease(env, new)
}

+ (id)URLFragmentAllowedCharacterSet {
    let mut set = ascii_range_set(&[
        (b'A' as u32, b'Z' as u32),
        (b'a' as u32, b'z' as u32),
        (b'0' as u32, b'9' as u32),
    ]);
    for ch in b"-._~!$&'()*+,;=:@/?" {
        set.insert(unichar::try_from(*ch as u16).unwrap());
    }
    let new: id = msg![env; this alloc];
    env.objc.borrow_mut::<CharacterSetHostObject>(new).set = set;
    autorelease(env, new)
}

+ (id)URLUserAllowedCharacterSet {
    let mut set = ascii_range_set(&[
        (b'A' as u32, b'Z' as u32),
        (b'a' as u32, b'z' as u32),
        (b'0' as u32, b'9' as u32),
    ]);
    for ch in b"-._~!$&'()*+,;=" {
        set.insert(unichar::try_from(*ch as u16).unwrap());
    }
    let new: id = msg![env; this alloc];
    env.objc.borrow_mut::<CharacterSetHostObject>(new).set = set;
    autorelease(env, new)
}

+ (id)URLPasswordAllowedCharacterSet {
    msg![env; this URLUserAllowedCharacterSet]
}

// MARK: NSCopying / NSMutableCopying

- (id)copyWithZone:(NSZonePtr)_zone {
    retain(env, this)
}

- (id)mutableCopyWithZone:(NSZonePtr)_zone {
    let host = env.objc.borrow::<CharacterSetHostObject>(this);
    let new_host = Box::new(CharacterSetHostObject {
        set: host.set.clone(),
        inverted: host.inverted,
    });
    let class = env.objc.get_known_class("_touchHLE_NSMutableCharacterSet", &mut env.mem);
    let new = env.objc.alloc_object(class, new_host, &mut env.mem);
    autorelease(env, new)
}

@end

// =========================================================================
// MARK: - _touchHLE_NSCharacterSet
// =========================================================================

@implementation _touchHLE_NSCharacterSet: NSCharacterSet

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(CharacterSetHostObject {
        set: HashSet::new(),
        inverted: false,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (bool)characterIsMember:(unichar)code_unit {
    let host_object = env.objc.borrow::<CharacterSetHostObject>(this);
    host_object.set.contains(&code_unit) ^ host_object.inverted
}

- (bool)hasMemberInPlane:(u8)plane {
    if plane != 0 {
        // We only track BMP (plane 0) characters.
        return false;
    }
    let host = env.objc.borrow::<CharacterSetHostObject>(this);
    !host.set.is_empty() ^ host.inverted
}

- (bool)isSupersetOfSet:(id)other { // NSCharacterSet*
    // Check that every member of `other` is also a member of `this`.
    // We iterate over all BMP code points that `other` contains.
    // For performance we only test the other's explicit set members.
    let other_set: HashSet<unichar> = {
        let h = env.objc.borrow::<CharacterSetHostObject>(other);
        h.set.clone()
    };
    let other_inverted = env.objc.borrow::<CharacterSetHostObject>(other).inverted;
    let self_host = env.objc.borrow::<CharacterSetHostObject>(this);

    if other_inverted {
        // other is an inverted set — too large to iterate; log and return
        // false.
        log_dbg!("isSupersetOfSet: other is inverted, returning false (not supported)");
        return false;
    }
    for &cp in &other_set {
        let is_member = self_host.set.contains(&cp) ^ self_host.inverted;
        if !is_member {
            return false;
        }
    }
    true
}

- (bool)isEqual:(id)other {
    if this == other { return true; }
    if other == nil { return false; }
    // Check that other is also a character set before borrowing.
    let cs_class = env.objc.get_known_class("_touchHLE_NSCharacterSet", &mut env.mem);
    let mcs_class = env.objc.get_known_class("_touchHLE_NSMutableCharacterSet", &mut env.mem);
    let other_class: id = msg![env; other class];
    let is_cs: bool = msg![env; other_class isSubclassOfClass:cs_class];
    let is_mcs: bool = msg![env; other_class isSubclassOfClass:mcs_class];
    if !is_cs && !is_mcs {
        return false;
    }
    let a_set: HashSet<unichar>;
    let a_inv: bool;
    {
        let a = env.objc.borrow::<CharacterSetHostObject>(this);
        a_set = a.set.clone();
        a_inv = a.inverted;
    }
    let b = env.objc.borrow::<CharacterSetHostObject>(other);
    a_set == b.set && a_inv == b.inverted
}

- (id)invertedSet {
    let old = env.objc.borrow::<CharacterSetHostObject>(this);
    let new_host = Box::new(CharacterSetHostObject {
        set: old.set.clone(),
        inverted: !old.inverted,
    });
    let class = env.objc.get_known_class("_touchHLE_NSCharacterSet", &mut env.mem);
    let new = env.objc.alloc_object(class, new_host, &mut env.mem);
    autorelease(env, new)
}

- (id)mutableCopyWithZone:(NSZonePtr)_zone {
    let old = env.objc.borrow::<CharacterSetHostObject>(this);
    let new_host = Box::new(CharacterSetHostObject {
        set: old.set.clone(),
        inverted: old.inverted,
    });
    let class = env.objc.get_known_class("_touchHLE_NSMutableCharacterSet", &mut env.mem);
    let new = env.objc.alloc_object(class, new_host, &mut env.mem);
    autorelease(env, new)
}

@end

// =========================================================================
// MARK: - NSMutableCharacterSet
// =========================================================================

@implementation NSMutableCharacterSet: NSCharacterSet

+ (id)allocWithZone:(NSZonePtr)zone {
    assert!(this == env.objc.get_known_class("NSMutableCharacterSet", &mut env.mem));
    msg_class![env; _touchHLE_NSMutableCharacterSet allocWithZone:zone]
}

// NSMutableCopying — mutable copy is also mutable.
- (id)mutableCopyWithZone:(NSZonePtr)_zone {
    retain(env, this)
}

// Immutable copy.
- (id)copyWithZone:(NSZonePtr)_zone {
    let host = env.objc.borrow::<CharacterSetHostObject>(this);
    let new_host = Box::new(CharacterSetHostObject {
        set: host.set.clone(),
        inverted: host.inverted,
    });
    let class = env.objc.get_known_class("_touchHLE_NSCharacterSet", &mut env.mem);
    let new = env.objc.alloc_object(class, new_host, &mut env.mem);
    autorelease(env, new)
}

@end

// =========================================================================
// MARK: - _touchHLE_NSMutableCharacterSet
// =========================================================================

@implementation _touchHLE_NSMutableCharacterSet: NSMutableCharacterSet

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(CharacterSetHostObject {
        set: HashSet::new(),
        inverted: false,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (bool)characterIsMember:(unichar)code_unit {
    let host_object = env.objc.borrow::<CharacterSetHostObject>(this);
    host_object.set.contains(&code_unit) ^ host_object.inverted
}

// MARK: Mutation

- (())addCharactersInString:(id)string { // NSString*
    let mut chars = Vec::new();
    ns_string::for_each_code_unit(env, string, |_idx, c| chars.push(c));
    let host = env.objc.borrow_mut::<CharacterSetHostObject>(this);
    for c in chars { host.set.insert(c); }
}

- (())removeCharactersInString:(id)string { // NSString*
    let mut chars = Vec::new();
    ns_string::for_each_code_unit(env, string, |_idx, c| chars.push(c));
    let host = env.objc.borrow_mut::<CharacterSetHostObject>(this);
    for c in chars { host.set.remove(&c); }
}

- (())addCharactersInRange:(NSRange)range {
    let host = env.objc.borrow_mut::<CharacterSetHostObject>(this);
    for cp in range.location..range.location.saturating_add(range.length) {
        if let Ok(uc) = unichar::try_from(cp) {
            host.set.insert(uc);
        }
    }
}

- (())removeCharactersInRange:(NSRange)range {
    let host = env.objc.borrow_mut::<CharacterSetHostObject>(this);
    for cp in range.location..range.location.saturating_add(range.length) {
        if let Ok(uc) = unichar::try_from(cp) {
            host.set.remove(&uc);
        }
    }
}

- (())unionWithCharacterSet:(id)other { // NSCharacterSet*
    let other_chars: Vec<unichar> = {
        let h = env.objc.borrow::<CharacterSetHostObject>(other);
        h.set.iter().copied().collect()
    };
    let host = env.objc.borrow_mut::<CharacterSetHostObject>(this);
    for c in other_chars { host.set.insert(c); }
}

- (())intersectWithCharacterSet:(id)other { // NSCharacterSet*
    let other_set: HashSet<unichar> = {
        let h = env.objc.borrow::<CharacterSetHostObject>(other);
        h.set.clone()
    };
    let host = env.objc.borrow_mut::<CharacterSetHostObject>(this);
    host.set.retain(|c| other_set.contains(c));
}

- (())invert {
    let host = env.objc.borrow_mut::<CharacterSetHostObject>(this);
    host.inverted = !host.inverted;
}

- (id)invertedSet {
    let old = env.objc.borrow::<CharacterSetHostObject>(this);
    let new_host = Box::new(CharacterSetHostObject {
        set: old.set.clone(),
        inverted: !old.inverted,
    });
    let class = env.objc.get_known_class("_touchHLE_NSCharacterSet", &mut env.mem);
    let new = env.objc.alloc_object(class, new_host, &mut env.mem);
    autorelease(env, new)
}

- (bool)characterIsMemberOfSet:(unichar)code_unit {
    let host_object = env.objc.borrow::<CharacterSetHostObject>(this);
    host_object.set.contains(&code_unit) ^ host_object.inverted
}

@end

};
