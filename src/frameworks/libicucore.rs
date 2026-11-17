use crate::dyld::{export_c_func, FunctionExports, HostDylib};
use crate::mem::{ConstPtr, MutPtr};
use crate::Environment;
use regex::Regex;
use std::collections::HashMap;
use std::sync::Mutex;

// Структуры для честного хранения состояния регулярок
struct RegexState {
    re: Regex,
    pattern_str: String,
}

struct TextState {
    text: String,
    last_match_end_utf16: usize,
    // Границы захваченных групп в индексах UTF-16: (start, end)
    // -1 означает, что группа не найдена (например, опциональная)
    groups: Vec<(i32, i32)>,
}

lazy_static::lazy_static! {
    static ref UREGEX_MAP: Mutex<HashMap<u32, RegexState>> = Mutex::new(HashMap::new());
    static ref UREGEX_TEXT_MAP: Mutex<HashMap<u32, TextState>> = Mutex::new(HashMap::new());
    static ref NEXT_HANDLE: Mutex<u32> = Mutex::new(0x9000_0000);
}

const U_ZERO_ERROR: i32 = 0;

// --- ВСПОМОГАТЕЛЬНЫЕ ФУНКЦИИ ---

// Конвертируем специфичный ICU синтаксис \Q...\E (литералы) в экранированный
// Rust regex
fn convert_icu_to_rust_regex(pattern: &str) -> String {
    let mut res = String::new();
    let mut in_q = false;
    let mut i = 0;
    let chars: Vec<char> = pattern.chars().collect();
    while i < chars.len() {
        if i + 1 < chars.len() && chars[i] == '\\' && chars[i + 1] == 'Q' {
            in_q = true;
            i += 2;
            continue;
        }
        if i + 1 < chars.len() && chars[i] == '\\' && chars[i + 1] == 'E' {
            in_q = false;
            i += 2;
            continue;
        }
        if in_q {
            res.push_str(&regex::escape(&chars[i].to_string()));
        } else {
            res.push(chars[i]);
        }
        i += 1;
    }
    res
}

// Конвертация смещений из UTF-8 в UTF-16 (требуется архитектурой iOS)
fn utf8_to_utf16_index(s: &str, utf8_index: usize) -> i32 {
    if utf8_index > s.len() {
        return -1;
    }
    s[..utf8_index]
        .chars()
        .map(|c| c.len_utf16())
        .sum::<usize>() as i32
}

fn utf16_to_utf8_index(s: &str, utf16_index: usize) -> usize {
    let mut current_utf16 = 0;
    for (byte_idx, c) in s.char_indices() {
        if current_utf16 >= utf16_index {
            return byte_idx;
        }
        current_utf16 += c.len_utf16();
    }
    s.len()
}

// --- API ФУНКЦИИ ICU ---

#[allow(non_snake_case)]
pub fn uregex_open(
    env: &mut Environment,
    pattern: ConstPtr<u16>,
    pattern_length: i32,
    _flags: u32,
    _pe: MutPtr<u8>,
    status: MutPtr<i32>,
) -> u32 {
    let mut chars = Vec::new();
    let mut i = 0;
    loop {
        if pattern_length != -1 && i >= pattern_length {
            break;
        }
        let c: u16 = env.mem.read(pattern + (i as u32));
        if pattern_length == -1 && c == 0 {
            break;
        }
        chars.push(c);
        i += 1;
    }

    let pattern_str = String::from_utf16_lossy(&chars).replace('\0', "");
    if !status.is_null() {
        env.mem.write(status, U_ZERO_ERROR);
    }

    // Транслируем синтаксис и компилируем настоящую регулярку!
    let rust_pattern = convert_icu_to_rust_regex(&pattern_str);
    let re = Regex::new(&rust_pattern).unwrap_or_else(|_| Regex::new(".*").unwrap());

    let mut map = UREGEX_MAP.lock().unwrap();
    let mut next_id = NEXT_HANDLE.lock().unwrap();
    let handle = *next_id;
    *next_id += 4;

    map.insert(handle, RegexState { re, pattern_str });
    handle
}

#[allow(non_snake_case)]
pub fn uregex_close(_env: &mut Environment, regexp: u32) {
    UREGEX_MAP.lock().unwrap().remove(&regexp);
    UREGEX_TEXT_MAP.lock().unwrap().remove(&regexp);
}

#[allow(non_snake_case)]
pub fn uregex_groupCount(env: &mut Environment, regexp: u32, status: MutPtr<i32>) -> i32 {
    if !status.is_null() {
        env.mem.write(status, U_ZERO_ERROR);
    }
    let map = UREGEX_MAP.lock().unwrap();
    if let Some(state) = map.get(&regexp) {
        // Честно считаем группы, скомпилированные движком (без учета 0-й
        // группы)
        state.re.captures_len().saturating_sub(1) as i32
    } else {
        if !status.is_null() {
            env.mem.write(status, 1);
        }
        0
    }
}

#[allow(non_snake_case)]
pub fn uregex_setText(
    env: &mut Environment,
    regexp: u32,
    text: ConstPtr<u16>,
    text_length: i32,
    status: MutPtr<i32>,
) {
    if !status.is_null() {
        env.mem.write(status, U_ZERO_ERROR);
    }

    let mut chars = Vec::new();
    let mut i = 0;
    loop {
        if text_length != -1 && i >= text_length {
            break;
        }
        let c: u16 = env.mem.read(text + (i as u32));
        if text_length == -1 && c == 0 {
            break;
        }
        chars.push(c);
        i += 1;
    }

    let text_str = String::from_utf16_lossy(&chars).replace('\0', "");

    let mut text_map = UREGEX_TEXT_MAP.lock().unwrap();
    text_map.insert(
        regexp,
        TextState {
            text: text_str,
            last_match_end_utf16: 0,
            groups: Vec::new(),
        },
    );
}

// Поиск совпадений
#[allow(non_snake_case)]
pub fn uregex_find(
    env: &mut Environment,
    regexp: u32,
    start_index: i32,
    status: MutPtr<i32>,
) -> u32 {
    if !status.is_null() {
        env.mem.write(status, U_ZERO_ERROR);
    }

    let map = UREGEX_MAP.lock().unwrap();
    let mut text_map = UREGEX_TEXT_MAP.lock().unwrap();

    if let (Some(reg_state), Some(txt_state)) = (map.get(&regexp), text_map.get_mut(&regexp)) {
        let start_utf16 = if start_index == -1 {
            txt_state.last_match_end_utf16
        } else {
            start_index as usize
        };

        let start_utf8 = utf16_to_utf8_index(&txt_state.text, start_utf16);
        if start_utf8 > txt_state.text.len() {
            return 0;
        } // false

        let search_text = &txt_state.text[start_utf8..];
        if let Some(captures) = reg_state.re.captures(search_text) {
            txt_state.groups.clear();

            // Сохраняем границы всех найденных групп (они понадобятся в
            // uregex_start)
            for i in 0..reg_state.re.captures_len() {
                if let Some(m) = captures.get(i) {
                    let abs_start_utf8 = start_utf8 + m.start();
                    let abs_end_utf8 = start_utf8 + m.end();
                    txt_state.groups.push((
                        utf8_to_utf16_index(&txt_state.text, abs_start_utf8),
                        utf8_to_utf16_index(&txt_state.text, abs_end_utf8),
                    ));
                } else {
                    txt_state.groups.push((-1, -1));
                }
            }

            if let Some(full_match) = captures.get(0) {
                let abs_end_utf8 = start_utf8 + full_match.end();
                txt_state.last_match_end_utf16 =
                    utf8_to_utf16_index(&txt_state.text, abs_end_utf8) as usize;
            }

            1 // true (нашли совпадение)
        } else {
            0 // false (совпадений нет)
        }
    } else {
        if !status.is_null() {
            env.mem.write(status, 1);
        }
        0
    }
}

#[allow(non_snake_case)]
pub fn uregex_start(
    _env: &mut Environment,
    regexp: u32,
    group_num: i32,
    status: MutPtr<i32>,
) -> i32 {
    if !status.is_null() {
        _env.mem.write(status, U_ZERO_ERROR);
    }
    if let Some(txt_state) = UREGEX_TEXT_MAP.lock().unwrap().get(&regexp) {
        if group_num >= 0 && (group_num as usize) < txt_state.groups.len() {
            return txt_state.groups[group_num as usize].0;
        }
    }
    -1
}

#[allow(non_snake_case)]
pub fn uregex_end(_env: &mut Environment, regexp: u32, group_num: i32, status: MutPtr<i32>) -> i32 {
    if !status.is_null() {
        _env.mem.write(status, U_ZERO_ERROR);
    }
    if let Some(txt_state) = UREGEX_TEXT_MAP.lock().unwrap().get(&regexp) {
        if group_num >= 0 && (group_num as usize) < txt_state.groups.len() {
            return txt_state.groups[group_num as usize].1;
        }
    }
    -1
}

/// `int32_t uregex_reset(URegularExpression *regexp, int32_t index,
///     UErrorCode *status)`
///
/// Rewinds the matcher so the next `uregex_findNext` starts at `index`
/// (UTF-16 code units), and returns that index. Clears the groups of the
/// previous match.
/// <https://unicode-org.github.io/icu-docs/apidoc/released/icu4c/uregex_8h.html>
#[allow(non_snake_case)]
pub fn uregex_reset(
    env: &mut Environment,
    regexp: u32,
    index: i32,
    status: MutPtr<i32>,
) -> i32 {
    if !status.is_null() {
        env.mem.write(status, U_ZERO_ERROR);
    }
    let mut text_map = UREGEX_TEXT_MAP.lock().unwrap();
    let Some(txt_state) = text_map.get_mut(&regexp) else {
        if !status.is_null() {
            env.mem.write(status, 1); // U_ILLEGAL_ARGUMENT_ERROR
        }
        return -1;
    };
    let text_len = txt_state.text.encode_utf16().count() as i32;
    if index < 0 || index > text_len {
        if !status.is_null() {
            env.mem.write(status, 8); // U_INDEX_OUTOFBOUNDS_ERROR
        }
        return -1;
    }
    txt_state.last_match_end_utf16 = index as usize;
    txt_state.groups.clear();
    index
}

/// `UBool uregex_findNext(URegularExpression *regexp, UErrorCode *status)`
///
/// Finds the next match after the end of the previous one.
/// <https://unicode-org.github.io/icu-docs/apidoc/released/icu4c/uregex_8h.html>
#[allow(non_snake_case)]
pub fn uregex_findNext(env: &mut Environment, regexp: u32, status: MutPtr<i32>) -> u32 {
    uregex_find(env, regexp, -1, status)
}

/// `int32_t uregex_appendTail(URegularExpression *regexp, UChar **destBuf,
///     int32_t *destCapacity, UErrorCode *status)`
///
/// Appends the input text that follows the last match to `*destBuf` and
/// returns the number of UTF-16 code units appended (excluding the NUL).
/// As in ICU, a NULL `*destBuf` asks us to allocate a big-enough buffer, and
/// a caller-supplied buffer that is too small gets a truncated copy plus
/// `U_BUFFER_OVERFLOW_ERROR`.
/// <https://unicode-org.github.io/icu-docs/apidoc/released/icu4c/uregex_8h.html>
#[allow(non_snake_case)]
pub fn uregex_appendTail(
    env: &mut Environment,
    regexp: u32,
    dest_buf: MutPtr<MutPtr<u16>>,
    dest_capacity: MutPtr<i32>,
    status: MutPtr<i32>,
) -> i32 {
    if !status.is_null() {
        env.mem.write(status, U_ZERO_ERROR);
    }
    let Some((tail, required)) = ({
        let text_map = UREGEX_TEXT_MAP.lock().unwrap();
        text_map.get(&regexp).map(|txt_state| {
            let start_utf8 = utf16_to_utf8_index(&txt_state.text, txt_state.last_match_end_utf16);
            let tail: Vec<u16> = txt_state.text[start_utf8..].encode_utf16().collect();
            let required = tail.len() as i32;
            (tail, required)
        })
    }) else {
        if !status.is_null() {
            env.mem.write(status, 1); // U_ILLEGAL_ARGUMENT_ERROR
        }
        return 0;
    };
    if dest_buf.is_null() {
        if !status.is_null() {
            env.mem.write(status, 1); // U_ILLEGAL_ARGUMENT_ERROR
        }
        return required;
    }

    let existing = env.mem.read(dest_buf);
    let capacity = if dest_capacity.is_null() {
        0
    } else {
        env.mem.read(dest_capacity)
    };
    let buffer_capacity = if existing.is_null() {
        required + 1
    } else {
        capacity
    };
    // Keep one slot for the NUL terminator when the caller's buffer is too
    // small.
    let writable = if buffer_capacity >= required + 1 {
        required
    } else {
        (buffer_capacity - 1).max(0)
    };
    let buffer = if existing.is_null() {
        let buffer: MutPtr<u16> = env.mem.alloc((buffer_capacity as u32) * 2).cast();
        if !dest_capacity.is_null() {
            env.mem.write(dest_capacity, buffer_capacity);
        }
        buffer
    } else {
        existing
    };
    env.mem.write(dest_buf, buffer);

    // A zero-capacity caller buffer gets nothing written, not even the NUL.
    if buffer_capacity > 0 {
        let units = env.mem.bytes_at_mut(buffer.cast(), (writable as u32 + 1) * 2);
        for (i, unit) in tail.iter().take(writable as usize).enumerate() {
            units[i * 2..i * 2 + 2].copy_from_slice(&unit.to_le_bytes());
        }
        let nul_at = (writable as usize) * 2;
        units[nul_at..nul_at + 2].copy_from_slice(&0u16.to_le_bytes());
    }

    if writable < required && !status.is_null() {
        env.mem.write(status, 15); // U_BUFFER_OVERFLOW_ERROR
    }
    required
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(uregex_open(_, _, _, _, _)),
    export_c_func!(uregex_close(_)),
    export_c_func!(uregex_groupCount(_, _)),
    export_c_func!(uregex_setText(_, _, _, _)),
    export_c_func!(uregex_find(_, _, _)),
    export_c_func!(uregex_findNext(_, _)),
    export_c_func!(uregex_reset(_, _, _)),
    export_c_func!(uregex_appendTail(_, _, _, _)),
    export_c_func!(uregex_start(_, _, _)),
    export_c_func!(uregex_end(_, _, _)),
];

pub const DYLIB: HostDylib = HostDylib {
    path: "/usr/lib/libicucore.A.dylib",
    aliases: &["/usr/lib/libicucore.dylib"],
    class_exports: &[],
    constant_exports: &[],
    function_exports: &[FUNCTIONS],
};
