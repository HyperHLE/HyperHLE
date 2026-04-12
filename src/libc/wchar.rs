/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `wchar.h`

use crate::dyld::{export_c_func, FunctionExports};
use crate::mem::{ConstPtr, GuestUSize, MutPtr};
use crate::Environment;

use super::generic_char::GenericChar;

#[allow(non_camel_case_types)]
pub type wchar_t = i32;

#[allow(non_camel_case_types)]
type wint_t = i32;

const WEOF: wint_t = -1;

// MARK: - Existing functions (unchanged)

fn btowc(_env: &mut Environment, c: i32) -> wint_t {
    let c = c as u8;
    if c.is_ascii() { c as wint_t } else { WEOF }
}

fn wctob(_env: &mut Environment, c: wint_t) -> i32 {
    if u32::try_from(c)
        .ok()
        .and_then(char::from_u32)
        .is_some_and(|c| c.is_ascii())
    {
        c
    } else {
        WEOF
    }
}

fn wmemset(env: &mut Environment, dest: MutPtr<wchar_t>, ch: wchar_t, count: GuestUSize) -> MutPtr<wchar_t> {
    GenericChar::<wchar_t>::memset(env, dest, ch, count, GuestUSize::MAX)
}
fn wmemcpy(env: &mut Environment, dest: MutPtr<wchar_t>, src: ConstPtr<wchar_t>, size: GuestUSize) -> MutPtr<wchar_t> {
    GenericChar::<wchar_t>::memcpy(env, dest, src, size, GuestUSize::MAX)
}
fn __wmemcpy_chk(env: &mut Environment, dest: MutPtr<wchar_t>, src: ConstPtr<wchar_t>, size: GuestUSize, dest_size: GuestUSize) -> MutPtr<wchar_t> {
    GenericChar::<wchar_t>::memcpy(env, dest, src, size, dest_size)
}
fn wmemmove(env: &mut Environment, dest: MutPtr<wchar_t>, src: ConstPtr<wchar_t>, size: GuestUSize) -> MutPtr<wchar_t> {
    GenericChar::<wchar_t>::memmove(env, dest, src, size, GuestUSize::MAX)
}
fn __wmemmove_chk(env: &mut Environment, dest: MutPtr<wchar_t>, src: ConstPtr<wchar_t>, size: GuestUSize, dest_size: GuestUSize) -> MutPtr<wchar_t> {
    GenericChar::<wchar_t>::memmove(env, dest, src, size, dest_size)
}
fn wmemchr(env: &mut Environment, string: ConstPtr<wchar_t>, c: wchar_t, size: GuestUSize) -> ConstPtr<wchar_t> {
    GenericChar::<wchar_t>::memchr(env, string, c, size)
}
fn wmemcmp(env: &mut Environment, a: ConstPtr<wchar_t>, b: ConstPtr<wchar_t>, size: GuestUSize) -> i32 {
    GenericChar::<wchar_t>::memcmp(env, a, b, size)
}
fn wcslen(env: &mut Environment, s: ConstPtr<wchar_t>) -> GuestUSize {
    GenericChar::<wchar_t>::strlen(env, s)
}
fn wcscpy(env: &mut Environment, dest: MutPtr<wchar_t>, src: ConstPtr<wchar_t>) -> MutPtr<wchar_t> {
    GenericChar::<wchar_t>::strcpy(env, dest, src, GuestUSize::MAX)
}
fn wcscat(env: &mut Environment, dest: MutPtr<wchar_t>, src: ConstPtr<wchar_t>) -> MutPtr<wchar_t> {
    GenericChar::<wchar_t>::strcat(env, dest, src, GuestUSize::MAX)
}
fn wcscspn(env: &mut Environment, str: ConstPtr<wchar_t>, charset: ConstPtr<wchar_t>) -> GuestUSize {
    GenericChar::<wchar_t>::strcspn(env, str, charset)
}
fn wcsncpy(env: &mut Environment, dest: MutPtr<wchar_t>, src: ConstPtr<wchar_t>, size: GuestUSize) -> MutPtr<wchar_t> {
    GenericChar::<wchar_t>::strncpy(env, dest, src, size, GuestUSize::MAX)
}
fn __wcsncpy_chk(env: &mut Environment, dest: MutPtr<wchar_t>, src: ConstPtr<wchar_t>, size: GuestUSize, dest_size: GuestUSize) -> MutPtr<wchar_t> {
    GenericChar::<wchar_t>::strncpy(env, dest, src, size, dest_size)
}
fn wcsdup(env: &mut Environment, src: ConstPtr<wchar_t>) -> MutPtr<wchar_t> {
    GenericChar::<wchar_t>::strdup(env, src)
}
fn wcscmp(env: &mut Environment, a: ConstPtr<wchar_t>, b: ConstPtr<wchar_t>) -> i32 {
    GenericChar::<wchar_t>::strcmp(env, a, b)
}
fn wcsncmp(env: &mut Environment, a: ConstPtr<wchar_t>, b: ConstPtr<wchar_t>, n: GuestUSize) -> i32 {
    GenericChar::<wchar_t>::strncmp(env, a, b, n)
}
fn wcsncat(env: &mut Environment, s1: MutPtr<wchar_t>, s2: ConstPtr<wchar_t>, n: GuestUSize) -> MutPtr<wchar_t> {
    GenericChar::<wchar_t>::strncat(env, s1, s2, n)
}
fn wcsstr(env: &mut Environment, wcsing: ConstPtr<wchar_t>, subwcsing: ConstPtr<wchar_t>) -> ConstPtr<wchar_t> {
    GenericChar::<wchar_t>::strstr(env, wcsing, subwcsing)
}
fn wcschr(env: &mut Environment, wcsing: ConstPtr<wchar_t>, wchar: wchar_t) -> ConstPtr<wchar_t> {
    GenericChar::<wchar_t>::strchr(env, wcsing, wchar)
}
fn wcsrchr(env: &mut Environment, wcsing: ConstPtr<wchar_t>, wchar: wchar_t) -> ConstPtr<wchar_t> {
    GenericChar::<wchar_t>::strrchr(env, wcsing, wchar)
}
fn wcslcpy(env: &mut Environment, dst: MutPtr<wchar_t>, src: ConstPtr<wchar_t>, size: GuestUSize) -> GuestUSize {
    GenericChar::<wchar_t>::strlcpy(env, dst, src, size)
}

// MARK: - New functions

/// wcsspn — length of prefix consisting entirely of wchars in `accept`.
fn wcsspn(
    env: &mut Environment,
    s: ConstPtr<wchar_t>,
    accept: ConstPtr<wchar_t>,
) -> GuestUSize {
    let mut i: GuestUSize = 0;
    loop {
        let c = env.mem.read(s + i);
        if c == 0 { break; }
        // Check if c is in accept set.
        let mut j: GuestUSize = 0;
        let mut found = false;
        loop {
            let a = env.mem.read(accept + j);
            if a == 0 { break; }
            if a == c { found = true;
            break; }
            j += 1;
        }
        if !found { break;
        }
        i += 1;
    }
    i
}

/// wcspbrk — find first occurrence of any wchar from `accept` in `s`.
fn wcspbrk(
    env: &mut Environment,
    s: ConstPtr<wchar_t>,
    accept: ConstPtr<wchar_t>,
) -> ConstPtr<wchar_t> {
    let mut i: GuestUSize = 0;
    loop {
        let c = env.mem.read(s + i);
        if c == 0 { return ConstPtr::null(); }
        let mut j: GuestUSize = 0;
        loop {
            let a = env.mem.read(accept + j);
            if a == 0 { break; }
            if a == c { return s + i;
            }
            j += 1;
        }
        i += 1;
    }
}

/// wcstok — tokenise a wide string (stateful via saveptr).
fn wcstok(
    env: &mut Environment,
    s: MutPtr<wchar_t>,
    delimiters: ConstPtr<wchar_t>,
    saveptr: MutPtr<MutPtr<wchar_t>>,
) -> MutPtr<wchar_t> {
    let start: MutPtr<wchar_t> = if !s.is_null() {
        s
    } else {
        env.mem.read(saveptr)
    };
    if start.is_null() {
        return MutPtr::null();
    }

    // Skip leading delimiters.
    let mut i: GuestUSize = 0;
    loop {
        let c = env.mem.read(start + i);
        if c == 0 {
            env.mem.write(saveptr, MutPtr::null());
            return MutPtr::null();
        }
        // Check if c is a delimiter.
        let mut j: GuestUSize = 0;
        let mut is_delim = false;
        loop {
            let d = env.mem.read(delimiters + j);
            if d == 0 { break; }
            if d == c { is_delim = true;
            break; }
            j += 1;
        }
        if !is_delim { break;
        }
        i += 1;
    }

    let token_start = start + i;

    // Find end of token.
    loop {
        let c = env.mem.read(start + i);
        if c == 0 {
            env.mem.write(saveptr, MutPtr::null());
            return token_start;
        }
        let mut j: GuestUSize = 0;
        let mut is_delim = false;
        loop {
            let d = env.mem.read(delimiters + j);
            if d == 0 { break; }
            if d == c { is_delim = true;
            break; }
            j += 1;
        }
        if is_delim {
            // Null-terminate and save position after delimiter.
            env.mem.write(start + i, 0); // Исправлено: 0 вместо wchar_t
            env.mem.write(saveptr, start + i + 1);
            return token_start;
        }
        i += 1;
    }
}

/// wcsncasecmp — case-insensitive wide string compare up to n chars.
fn wcsncasecmp(
    env: &mut Environment,
    a: ConstPtr<wchar_t>,
    b: ConstPtr<wchar_t>,
    n: GuestUSize,
) -> i32 {
    for i in 0..n {
        let ca = env.mem.read(a + i);
        let cb = env.mem.read(b + i);
        // Lowercase ASCII only — good enough for game use-cases.
        let la = if ca >= 'A' as i32 && ca <= 'Z' as i32 { ca + 32 } else { ca };
        let lb = if cb >= 'A' as i32 && cb <= 'Z' as i32 { cb + 32 } else { cb };
        if la != lb { return la - lb; }
        if la == 0  { return 0;
        }
    }
    0
}

/// wcscasecmp — case-insensitive wide string compare.
fn wcscasecmp(
    env: &mut Environment,
    a: ConstPtr<wchar_t>,
    b: ConstPtr<wchar_t>,
) -> i32 {
    wcsncasecmp(env, a, b, GuestUSize::MAX)
}

/// wcslcat — wide string bounded concatenation (BSD extension).
fn wcslcat(
    env: &mut Environment,
    dst: MutPtr<wchar_t>,
    src: ConstPtr<wchar_t>,
    size: GuestUSize,
) -> GuestUSize {
    if size == 0 {
        return wcslen(env, src);
    }
    let dst_len = GenericChar::<wchar_t>::strlen(env, dst.cast_const());
    let src_len = wcslen(env, src);
    // Total length that would be produced.
    let total = dst_len + src_len;
    if dst_len >= size - 1 {
        return total;
    }
    let copy_len = (size - dst_len - 1).min(src_len);
    for i in 0..copy_len {
        let c = env.mem.read(src + i);
        env.mem.write(dst + dst_len + i, c);
    }
    env.mem.write(dst + dst_len + copy_len, 0); // Исправлено: 0 вместо wchar_t
    total
}

/// wcswidth — number of columns needed to display n wide chars.
/// We return 1 per printable char and -1 for non-printable.
fn wcswidth(
    env: &mut Environment,
    s: ConstPtr<wchar_t>,
    n: GuestUSize,
) -> i32 {
    let mut width: i32 = 0;
    for i in 0..n {
        let c = env.mem.read(s + i);
        if c == 0 { break; }
        if c < 0x20 ||
        c == 0x7F { return -1; }
        width += 1;
    }
    width
}

/// wcwidth — column width of a single wide character.
fn wcwidth(_env: &mut Environment, c: wchar_t) -> i32 {
    if c == 0 { return 0;
    }
    if c < 0x20 || c == 0x7F { return -1;
    }
    1
}

/// mbtowc — convert multibyte char to wide char (ASCII locale stub).
fn mbtowc(
    env: &mut Environment,
    pwc: MutPtr<wchar_t>,
    s: ConstPtr<u8>,
    n: GuestUSize,
) -> i32 {
    if s.is_null() {
        // No shift state — return 0.
        return 0;
    }
    if n == 0 { return -1; }
    let byte = env.mem.read(s);
    if !pwc.is_null() {
        env.mem.write(pwc, byte as wchar_t);
    }
    if byte == 0 { 0 } else { 1 }
}

/// wctomb — convert wide char to multibyte (ASCII locale stub).
fn wctomb(
    env: &mut Environment,
    s: MutPtr<u8>,
    wc: wchar_t,
) -> i32 {
    if s.is_null() { return 0;
    }
    if wc < 0 || wc > 0x7F { return -1;
    }
    env.mem.write(s, wc as u8);
    1
}

/// mbstowcs — convert multibyte string to wide string.
fn mbstowcs(
    env: &mut Environment,
    dest: MutPtr<wchar_t>,
    src: ConstPtr<u8>,
    n: GuestUSize,
) -> GuestUSize {
    let mut i: GuestUSize = 0;
    loop {
        let byte = env.mem.read(src + i);
        if i == n { break; }
        if !dest.is_null() {
            env.mem.write(dest + i, byte as wchar_t);
        }
        if byte == 0 { return i;
        }
        i += 1;
    }
    i
}

/// wcstombs — convert wide string to multibyte string.
fn wcstombs(
    env: &mut Environment,
    dest: MutPtr<u8>,
    src: ConstPtr<wchar_t>,
    n: GuestUSize,
) -> GuestUSize {
    let mut i: GuestUSize = 0;
    loop {
        let wc = env.mem.read(src + i);
        if i == n { break; }
        if wc < 0 ||
        wc > 0x7F {
            return GuestUSize::MAX;
// encoding error
        }
        let byte = wc as u8;
        if !dest.is_null() {
            env.mem.write(dest + i, byte);
        }
        if byte == 0 { return i;
        }
        i += 1;
    }
    i
}

/// wcstol — wide string to long.
fn wcstol(
    env: &mut Environment,
    s: ConstPtr<wchar_t>,
    endptr: MutPtr<MutPtr<wchar_t>>,
    base: i32,
) -> i32 {
    // Convert to a narrow string then parse.
    let mut buf = String::new();
    let mut i: GuestUSize = 0;
    loop {
        let wc = env.mem.read(s + i);
        if wc == 0 ||
        wc > 0x7F { break; }
        buf.push(wc as u8 as char);
        i += 1;
    }
    let buf = buf.trim();
    let result = if base == 0 ||
    base == 10 {
        buf.parse::<i32>().unwrap_or(0)
    } else if base == 16 {
        i32::from_str_radix(buf.trim_start_matches("0x"), 16).unwrap_or(0)
    } else {
        i32::from_str_radix(buf, base as u32).unwrap_or(0)
    };
    if !endptr.is_null() {
        env.mem.write(endptr, (s + i).cast_mut());
    }
    result
}

/// wcstoul — wide string to unsigned long.
fn wcstoul(
    env: &mut Environment,
    s: ConstPtr<wchar_t>,
    endptr: MutPtr<MutPtr<wchar_t>>,
    base: i32,
) -> u32 {
    wcstol(env, s, endptr, base) as u32
}

/// wcstod — wide string to double.
fn wcstod(
    env: &mut Environment,
    s: ConstPtr<wchar_t>,
    endptr: MutPtr<MutPtr<wchar_t>>,
) -> f64 {
    let mut buf = String::new();
    let mut i: GuestUSize = 0;
    loop {
        let wc = env.mem.read(s + i);
        if wc == 0 || wc > 0x7F { break;
        }
        buf.push(wc as u8 as char);
        i += 1;
    }
    if !endptr.is_null() {
        env.mem.write(endptr, (s + i).cast_mut());
    }
    buf.trim().parse::<f64>().unwrap_or(0.0)
}

/// wcstof — wide string to float.
fn wcstof(
    env: &mut Environment,
    s: ConstPtr<wchar_t>,
    endptr: MutPtr<MutPtr<wchar_t>>,
) -> f32 {
    wcstod(env, s, endptr) as f32
}

/// iswspace — test if wide char is whitespace.
fn iswspace(_env: &mut Environment, c: wint_t) -> i32 {
    matches!(c, 0x09 | 0x0A | 0x0B | 0x0C | 0x0D | 0x20) as i32
}

/// iswdigit — test if wide char is ASCII digit.
fn iswdigit(_env: &mut Environment, c: wint_t) -> i32 {
    (c >= '0' as i32 && c <= '9' as i32) as i32
}

/// iswalpha — test if wide char is ASCII letter.
fn iswalpha(_env: &mut Environment, c: wint_t) -> i32 {
    ((c >= 'a' as i32 && c <= 'z' as i32) || (c >= 'A' as i32 && c <= 'Z' as i32)) as i32
}

/// iswalnum — test if wide char is ASCII alphanumeric.
fn iswalnum(_env: &mut Environment, c: wint_t) -> i32 {
    let is_alpha = (c >= 'a' as i32 && c <= 'z' as i32) ||
    (c >= 'A' as i32 && c <= 'Z' as i32);
    let is_digit = c >= '0' as i32 && c <= '9' as i32;
    (is_alpha || is_digit) as i32
}

/// iswupper — test if wide char is uppercase ASCII.
fn iswupper(_env: &mut Environment, c: wint_t) -> i32 {
    (c >= 'A' as i32 && c <= 'Z' as i32) as i32
}

/// iswlower — test if wide char is lowercase ASCII.
fn iswlower(_env: &mut Environment, c: wint_t) -> i32 {
    (c >= 'a' as i32 && c <= 'z' as i32) as i32
}

/// iswprint — test if wide char is printable.
fn iswprint(_env: &mut Environment, c: wint_t) -> i32 {
    (c >= 0x20 && c != 0x7F) as i32
}

/// iswpunct — test if wide char is punctuation.
fn iswpunct(_env: &mut Environment, c: wint_t) -> i32 {
    let is_print = c >= 0x20 && c != 0x7F;
    let is_alnum = (c >= 'a' as i32 && c <= 'z' as i32)
        ||
    (c >= 'A' as i32 && c <= 'Z' as i32)
        ||
    (c >= '0' as i32 && c <= '9' as i32);
    (is_print && !is_alnum && c != ' ' as i32) as i32
}

/// iswcntrl — test if wide char is a control character.
fn iswcntrl(_env: &mut Environment, c: wint_t) -> i32 {
    (c < 0x20 || c == 0x7F) as i32
}

/// towlower — convert wide char to lowercase.
fn towlower(_env: &mut Environment, c: wint_t) -> wint_t {
    if c >= 'A' as i32 && c <= 'Z' as i32 { c + 32 } else { c }
}

/// towupper — convert wide char to uppercase.
fn towupper(_env: &mut Environment, c: wint_t) -> wint_t {
    if c >= 'a' as i32 && c <= 'z' as i32 { c - 32 } else { c }
}

/// putwchar — write wide char to stdout (stub).
fn putwchar(_env: &mut Environment, _c: wint_t) -> wint_t {
    WEOF
}

/// putwc — write wide char to stream (stub).
fn putwc(_env: &mut Environment, _c: wint_t, _stream: crate::mem::MutVoidPtr) -> wint_t {
    WEOF
}

/// getwchar — read wide char from stdin (stub).
fn getwchar(_env: &mut Environment) -> wint_t {
    WEOF
}

/// getwc — read wide char from stream (stub).
fn getwc(_env: &mut Environment, _stream: crate::mem::MutVoidPtr) -> wint_t {
    WEOF
}

/// ungetwc — push wide char back to stream (stub).
fn ungetwc(_env: &mut Environment, c: wint_t, _stream: crate::mem::MutVoidPtr) -> wint_t {
    c
}

fn mbsrtowcs(
    env: &mut Environment,
    dest: MutPtr<wchar_t>,
    src: MutPtr<ConstPtr<u8>>, // const char**
    len: GuestUSize,
    _ps: crate::mem::MutVoidPtr, // mbstate_t* — ignored (stateless ASCII)
) -> GuestUSize {
    if src.is_null() {
        return 0;
    }

    let src_ptr: ConstPtr<u8> = env.mem.read(src);
    if src_ptr.is_null() {
        return 0;
    }

    // Count or convert up to `len` wide chars.
    let mut i: GuestUSize = 0;
    loop {
        if i == len { break;
        }
        let byte = env.mem.read(src_ptr + i);
        if !dest.is_null() {
            env.mem.write(dest + i, byte as wchar_t);
        }
        if byte == 0 {
            // Null terminator consumed — set *src to NULL per POSIX.
            env.mem.write(src, ConstPtr::null());
            return i; // not counting the null terminator
        }
        i += 1;
    }

    // Reached `len` without hitting null — update *src to point past converted chars.
    if !dest.is_null() {
        env.mem.write(src, src_ptr + i);
    }
    i
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(btowc(_)),
    export_c_func!(wctob(_)),
    // Memory
    export_c_func!(wmemset(_, _, _)),
    export_c_func!(wmemcpy(_, _, _)),
    export_c_func!(__wmemcpy_chk(_, _, _, _)),
    export_c_func!(wmemmove(_, _, _)),
    export_c_func!(__wmemmove_chk(_, _, _, _)),
    export_c_func!(wmemchr(_, _, _)),
    export_c_func!(wmemcmp(_, _, _)),
    // String operations
    export_c_func!(wcslen(_)),
    export_c_func!(wcscpy(_, _)),
    export_c_func!(wcscat(_, _)),
    export_c_func!(wcscspn(_, _)),
    export_c_func!(wcsspn(_, _)),
    export_c_func!(wcspbrk(_, _)),
    export_c_func!(wcstok(_, _, _)),
    export_c_func!(wcsncpy(_, _, _)),
    export_c_func!(__wcsncpy_chk(_, _, _, _)),
    export_c_func!(wcsdup(_)),
    export_c_func!(wcscmp(_, _)),
    export_c_func!(wcsncmp(_, _, _)),
    export_c_func!(wcscasecmp(_, _)),
    export_c_func!(wcsncasecmp(_, _, _)),
    export_c_func!(wcsncat(_, _, _)),
    export_c_func!(wcslcpy(_, _, _)),
    export_c_func!(wcslcat(_, _, _)),
    export_c_func!(wcsstr(_, _)),
    export_c_func!(wcschr(_, _)),
    export_c_func!(wcsrchr(_, _)),
    export_c_func!(wcswidth(_, _)),
    export_c_func!(wcwidth(_)),
    // Conversion
    export_c_func!(mbtowc(_, _, _)),
    export_c_func!(wctomb(_, _)),
    export_c_func!(mbstowcs(_, _, _)),
    export_c_func!(wcstombs(_, _, _)),
    export_c_func!(wcstol(_, _, _)),
    export_c_func!(wcstoul(_, _, _)),
    export_c_func!(wcstod(_, _)),
    export_c_func!(wcstof(_, _)),
    // Classification
    export_c_func!(iswspace(_)),
    export_c_func!(iswdigit(_)),
    export_c_func!(iswalpha(_)),
    export_c_func!(iswalnum(_)),
    export_c_func!(iswupper(_)),
    export_c_func!(iswlower(_)),
    export_c_func!(iswprint(_)),
    export_c_func!(iswpunct(_)),
    export_c_func!(iswcntrl(_)),
    // Case conversion
    export_c_func!(towlower(_)),
    export_c_func!(towupper(_)),
    // I/O stubs
    export_c_func!(putwchar(_)),
    export_c_func!(putwc(_, _)),
    export_c_func!(getwchar()),
    export_c_func!(getwc(_)),
    export_c_func!(ungetwc(_, _)),
    export_c_func!(mbsrtowcs(_, _, _, _)),
];

