/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `fnmatch.h` — filename pattern matching.

use crate::dyld::{export_c_func, FunctionExports};
use crate::mem::ConstPtr;
use crate::Environment;

// fnmatch() return value on no match
const FNM_NOMATCH: i32 = 1;

// fnmatch() flags
#[allow(dead_code)]
const FNM_PATHNAME: i32 = 1 << 1; // slash must be matched by slash
#[allow(dead_code)]
const FNM_NOESCAPE: i32 = 1 << 2; // disable backslash escaping
#[allow(dead_code)]
const FNM_PERIOD:   i32 = 1 << 3; // leading dot must be matched explicitly
const FNM_CASEFOLD: i32 = 1 << 4; // case-insensitive matching (GNU extension)

/// Pure-Rust fnmatch implementation.
/// Supports `*`, `?`, `[...]`, `[!...]`, `[a-z]` ranges, and FNM_CASEFOLD.
fn fnmatch_impl(pattern: &[u8], string: &[u8], flags: i32) -> bool {
    let casefold = (flags & FNM_CASEFOLD) != 0;
    let norm = |b: u8| if casefold { b.to_ascii_lowercase() } else { b };

    let mut pi = 0usize;
    let mut si = 0usize;
    // backtracking state after a '*'
    let mut star_pi: Option<usize> = None;
    let mut star_si: Option<usize> = None;

    loop {
        if pi < pattern.len() {
            match pattern[pi] {
                b'*' => {
                    pi += 1;
                    // collapse consecutive stars
                    while pi < pattern.len() && pattern[pi] == b'*' {
                        pi += 1;
                    }
                    if pi == pattern.len() {
                        return true; // trailing '*' matches everything
                    }
                    star_pi = Some(pi);
                    star_si = Some(si);
                }
                b'?' => {
                    if si >= string.len() {
                        return false;
                    }
                    pi += 1;
                    si += 1;
                }
                b'[' => {
                    if si >= string.len() {
                        return false;
                    }
                    pi += 1;
                    let negate = pi < pattern.len() && pattern[pi] == b'!';
                    if negate {
                        pi += 1;
                    }
                    let sc = norm(string[si]);
                    let mut matched = false;
                    while pi < pattern.len() && pattern[pi] != b']' {
                        if pi + 2 < pattern.len()
                            && pattern[pi + 1] == b'-'
                            && pattern[pi + 2] != b']'
                        {
                            if sc >= norm(pattern[pi]) && sc <= norm(pattern[pi + 2]) {
                                matched = true;
                            }
                            pi += 3;
                        } else {
                            if norm(pattern[pi]) == sc {
                                matched = true;
                            }
                            pi += 1;
                        }
                    }
                    if pi < pattern.len() {
                        pi += 1; // skip ']'
                    }
                    if matched == negate {
                        return false;
                    }
                    si += 1;
                }
                pc => {
                    if si >= string.len() || norm(pc) != norm(string[si]) {
                        // mismatch — backtrack to last '*'
                        match (star_pi, star_si) {
                            (Some(spi), Some(ssi)) => {
                                let new_ssi = ssi + 1;
                                if new_ssi > string.len() {
                                    return false;
                                }
                                pi = spi;
                                star_si = Some(new_ssi);
                                si = new_ssi;
                            }
                            _ => return false,
                        }
                    } else {
                        pi += 1;
                        si += 1;
                    }
                }
            }
        } else {
            // pattern exhausted
            if si == string.len() {
                return true;
            }
            // string has remaining chars — backtrack if we have a '*'
            match (star_pi, star_si) {
                (Some(spi), Some(ssi)) => {
                    let new_ssi = ssi + 1;
                    if new_ssi > string.len() {
                        return false;
                    }
                    pi = spi;
                    star_si = Some(new_ssi);
                    si = new_ssi;
                }
                _ => return false,
            }
        }
    }
}

fn fnmatch(
    env: &mut Environment,
    pattern: ConstPtr<u8>,
    string: ConstPtr<u8>,
    flags: i32,
) -> i32 {
    let pat = env.mem.cstr_at(pattern);
    let s = env.mem.cstr_at(string);
    log_dbg!(
        "fnmatch({:?}, {:?}, {:#x})",
        std::str::from_utf8(pat),
        std::str::from_utf8(s),
        flags
    );
    if fnmatch_impl(pat, s, flags) {
        0
    } else {
        FNM_NOMATCH
    }
}

pub const FUNCTIONS: FunctionExports = &[export_c_func!(fnmatch(_, _, _))];
