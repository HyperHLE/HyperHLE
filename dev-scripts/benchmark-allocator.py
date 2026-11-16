#!/usr/bin/env python3
# This Source Code Form is subject to the terms of the Mozilla Public License,
# v. 2.0. https://mozilla.org/MPL/2.0/
"""Compare the production allocator's metadata operations without Cargo.

Requires Python 3, Git, and rustc. Guest memory and logging are stubbed; this
measures free-space lookup/accounting, excluding memory clearing and the
emulator's global allocator. See tests/profile-emulation.md.
"""

import argparse
from pathlib import Path
import subprocess
import tempfile

REPO = Path(__file__).resolve().parents[1]
SOURCE = "src/mem/allocator.rs"
BASELINE = "228546e58233a8e7578ff9af1f10b4a8dbd88bde"

PREAMBLE = r'''
#![allow(dead_code)]
macro_rules! log { ($($arg:tt)*) => {} }
type GuestUSize = u32;
type VAddr = u32;
const PAGE_SIZE: u32 = 4096;
const PAGE_SIZE_ALIGN_MASK: u32 = PAGE_SIZE - 1;
// Only the metadata allocator is exercised; no guest memory is mapped.
struct Mem;
impl Mem {
    const MAIN_THREAD_STACK_LOW_END: u32 = 0xffe00000;
    const MAIN_THREAD_STACK_SIZE: u32 = 1024 * 1024;
    const MAIN_THREAD_STACK_GUARD: u32 = 1024 * 1024;
}
fn publish_guest_heap_bytes(_: u32) {}
fn log_out_of_memory(_: &str, _: u32) {}
'''

RUNNER = r'''
use std::{hint::black_box, time::{Duration, Instant}};
macro_rules! workload {
    ($module:ident, $count:expr, $fragmented:expr) => {{
        let mut allocator = $module::Allocator::new();
        allocator.reserve($module::Chunk::new(0, PAGE_SIZE));
        let mut blocks = Vec::with_capacity($count);
        for _ in 0..$count {
            blocks.push(allocator.alloc(if $fragmented {1536} else {64}));
            if $fragmented { black_box(allocator.alloc(16)); }
        }
        if $fragmented {
            for base in blocks { black_box(allocator.free(base)); }
            let start = Instant::now();
            for _ in 0..$count {
                assert_ne!(black_box(allocator.alloc(1024)), 0);
            }
            start.elapsed()
        } else {
            let start = Instant::now();
            for i in 0..100_000 {
                let slot = i % $count;
                black_box(allocator.free(blocks[slot]));
                blocks[slot] = black_box(allocator.alloc(64));
            }
            start.elapsed()
        }
    }};
}
fn main() {
    for (count, fragmented) in [(1000, true), (4000, true), (16000, true), (1, false), (1000, false)] {
        let mut times: [Vec<Duration>; 2] = [Vec::new(), Vec::new()];
        for run in 0..9 {
            // Alternate order to reduce thermal/frequency bias.
            for old in if run % 2 == 0 {[true, false]} else {[false, true]} {
                let elapsed = if old {workload!(before, count, fragmented)}
                    else {workload!(after, count, fragmented)};
                times[usize::from(!old)].push(elapsed);
            }
        }
        times.iter_mut().for_each(|v| v.sort());
        println!("{} {count} blocks: baseline {:.3} ms, optimized {:.3} ms (medians of 9)",
            if fragmented {"fragmented allocations,"} else {"100k free/allocate pairs,"},
            times[0][4].as_secs_f64() * 1000.0, times[1][4].as_secs_f64() * 1000.0);
    }
    let mut allocator = after::Allocator::new();
    allocator.reserve(after::Chunk::new(0, PAGE_SIZE));
    let base = allocator.alloc(PAGE_SIZE);
    assert_eq!(allocator.grow_in_place(base, PAGE_SIZE, 2 * PAGE_SIZE), Some(2 * PAGE_SIZE));
}
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", default=BASELINE, help="pre-optimization Git revision")
    args = parser.parse_args()
    baseline = subprocess.check_output(
        ["git", "show", f"{args.baseline}:{SOURCE}"], cwd=REPO, text=True
    )
    current = (REPO / SOURCE).read_text()
    with tempfile.TemporaryDirectory(prefix="hyperhle-allocator-") as temporary:
        source = Path(temporary) / "benchmark.rs"
        binary = Path(temporary) / "benchmark"
        source.write_text(
            PREAMBLE + "mod before {\n" + baseline + "\n}\n"
            + "mod after {\n" + current + "\n}\n" + RUNNER
        )
        subprocess.run(["rustc", "--edition=2021", "-O", str(source), "-o", str(binary)], check=True)
        subprocess.run([str(binary)], check=True)


if __name__ == "__main__":
    main()
