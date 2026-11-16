# Emulation hot-path profiling

Baseline: `228546e58233a8e7578ff9af1f10b4a8dbd88bde`.
The measurements below isolate emulator work; they do not predict a game's
frame rate or establish performance on a physical phone.

## GLES 1.1 translator

`src/gles/gles1_on_gles2.rs` translates fixed-function GLES 1.1 draws to a
private GLES 2.0 shader. Before the change, every `DrawArrays` repeated 105
`glGetUniformLocation` calls and 60 Rust string allocations. Those locations
remain fixed until the program is linked again, so they are now queried once
per linked program and stored as typed fields. The draw path makes no location
queries or string allocations. The cache also preserves `-1` for uniforms the
driver optimized out.

Indexed draws now share the same uniform upload as non-indexed draws. This
fixes stale alpha-test, fog, matrix-palette, point-size-array and logic-operation
state. Clip-plane flags are uploaded as six `GLint` values: casting a pointer
to six packed Rust `bool` values previously made GL read beyond the array.

The index-range scan now computes the maximum in one pass. It previously
collected every index into a growing temporary vector before computing the
maximum. The scan is also skipped when enabled arrays all use native VBO data.
Only staged client arrays and converted `FIXED` arrays need that range. A null
index pointer is correctly accepted as byte offset zero in a bound element
buffer. Conversion of a `FIXED` VBO also borrows its CPU mirror rather than
cloning the entire buffer on each draw.

### Reproduce

On Linux, install EGL/GLES development libraries and headers, Python 3, Git,
and Rust 1.82 or newer. The benchmark uses a surfaceless Mesa EGL context and
compiles the actual translator shader; it does not require Cargo, a guest app,
or a display server.

```sh
python3 dev-scripts/benchmark-gles1.py --count-allocations
```

The script extracts the original upload/scan routines from the baseline Git
revision and the current routines from the working tree. It calls real host
GL functions for both uniform-upload variants, warms the driver, and runs five
measurements. Allocation counts include Rust allocations and reallocations;
they exclude allocations inside the GL driver. Uniform-cache construction and
shader compilation are outside the timed draws. The index workload is 60,000
`u16` indices at byte offset two, so the baseline's offset-zero bug does not
invalidate the comparison. The generated harness uses Rust's system allocator.

Measured on an AArch64 Linux host, Rust 1.99.1 with `rustc -O -g`, and
Mesa llvmpipe (LLVM 19.1.7, 128 bits), with allocation counting enabled:

| Workload | Baseline median | Optimized median | Ratio | Rust allocations/reallocations |
| --- | ---: | ---: | ---: | ---: |
| Uniform upload, 2,000 draws | 65.255 ms | 13.863 ms | 4.7× | 120,000 → 0 |
| Maximum index, 60,000 indices × 1,000 scans | 608.320 ms | 47.437 ms | 12.8× | 15,000 → 0 |

These are the measured routines' speedups, not whole-emulator speedups.
Software rendering was available on this host; physical PC GPUs and native Android/iOS
builds were not exercised. Allocation counting changes the cost of the allocating
baseline; omit `--count-allocations` to measure without the counter increments.

For sampling on a machine with Linux performance-event access:

```sh
python3 dev-scripts/benchmark-gles1.py --output /tmp/hyperhle-gles1-benchmark
perf record -g -- /tmp/hyperhle-gles1-benchmark
perf report
```

The benchmark keeps its generated Rust source alongside the binary when
`--output` is supplied. Performance-event access was denied by the test
container, so the recorded evidence here is wall-clock timing and allocation
counts, rather than a `perf` sampling profile.

The regression tests use a GL call recorder to exercise the actual indexed and
non-indexed draw entry points, verify identical integer uniforms and the clip
payload, and assert that repeated draws do not query locations. Separate tests
cover inactive locations, VBO offset zero, bounds, unaligned client indices,
and the cases that require index scans.

References:

- [Khronos: glGetUniformLocation](https://registry.khronos.org/OpenGL-Refpages/es2.0/xhtml/glGetUniformLocation.xml): locations remain fixed until the next link command.
- [Khronos: glUniform](https://registry.khronos.org/OpenGL-Refpages/es2.0/xhtml/glUniform.xml): integer uniform arrays consume `GLint` values.

## Guest heap

The old free-space buckets scanned every candidate to find the smallest fit,
and scanned again to remove a chunk by address. A fragmented heap with many
similarly sized holes made a batch of allocations take quadratic time. Free
chunks now have an address index and an ordered `(size, address)` index; lookup
and removal take logarithmic time and preserve best-fit allocation.

`grow_in_place` also absorbed an entire large adjacent free chunk instead of
splitting it. On a fresh heap, growing a 4 KiB allocation to 8 KiB returned
4,292,866,048 bytes. `Mem::realloc` then cleared the enlarged tail. The result
could be gigabytes of physical memory use and a long stall for a small buffer
increase. Growth now rounds to the appropriate alignment and returns 8 KiB,
leaving the rest free. A partial-page remainder is split to retain the
allocator's page-alignment invariant. Failed growth leaves both indexes and
the original allocation intact; absorbed tiny remainders respect the heap cap.

To reproduce metadata timings on a desktop or ARM64 host:

```sh
python3 dev-scripts/benchmark-allocator.py
```

The script compiles the actual baseline and current allocator code using
`rustc -O`, with guest memory, logging, and global counter publication stubbed.
It uses Rust's system allocator. It alternates baseline/optimized order over
nine measurements. The fragmented workload frees 1,536-byte blocks separated
by live 16-byte blocks, then times allocating 1,024-byte blocks from those holes.
Setup and destruction are outside the timed portion. A simple reuse workload
is included to expose the tradeoff of maintaining the second ordered index.

Measured on the same ARM64 Linux host with Rust 1.99.1:

| Workload | Baseline median | Optimized median |
| --- | ---: | ---: |
| 1,000 fragmented allocations | 0.973 ms | 0.705 ms |
| 4,000 fragmented allocations | 10.455 ms | 3.106 ms |
| 16,000 fragmented allocations | 152.187 ms | 13.658 ms |
| 100,000 free/allocate pairs, one live block | 22.483 ms | 32.011 ms |
| 100,000 free/allocate pairs, 1,000 live blocks | 28.658 ms | 36.214 ms |

The size index adds approximately 76–95 ns per simple reuse pair in this
workload, while eliminating long scans in fragmented heaps. Equal-sized fits
now prefer the lower address rather than the most recently freed block;
`malloc` does not promise allocation addresses or reuse order.

The normal Rust suite exercises allocation/growth failure, alignment, best-fit
reuse, cap accounting, and 6,000 mixed operations checking full address-space
coverage and unchanged state after failed growth. A manual timing test also
runs against the full emulator's allocator/host allocator:

```sh
RUSTFLAGS="-C link-arg=-latomic" cargo test --release benchmark_fragmented_heap -- --ignored --nocapture
```

## Scheduling and audio

Timed condition-variable waits previously did not participate in the imminent
wakeup check that selects the CPU batch size. They could wait behind a full
one-million-tick batch while another guest thread ran. They now participate
alongside sleeping threads. Inactive threads do not force shorter batches.
One host clock sample is shared across a scheduling pass, and wall time is
sampled only if a timed condition waiter needs it. At normal guest speed,
duration arithmetic no longer takes a floating-point round trip; this reduces
clock-call overhead and preserves nanoseconds.

Repeating NSTimers now advance to a scheduled deadline strictly after the
current guest time. Exact missed intervals formerly scheduled at `now` and
immediately fired again. Deadlines missed during a slow callback are skipped
as well, preserving the original phase, guest invalidation, and explicit
fire-date changes. This follows Apple's
[Timer Programming Topics](https://developer.apple.com/library/archive/documentation/Cocoa/Conceptual/Timers/Articles/timerConcepts.html).

PCM32 conversion now borrows correctly laid-out input until it allocates the
converted output, removing an intermediate allocation/copy. Mono/stereo float
and signed-integer conversion and trailing partial frames have regression
coverage. Exploratory `rustc -O` benchmarks using the original/current PCM
conversion arms and system allocator measured 20,000 stereo buffers of
1,024 frames, with nine alternating baseline/optimized runs: float conversion
206.5 → 169.2 ms and signed integer conversion 116.8 → 79.1 ms. Each buffer
avoids copying an 8 KiB intermediate input. Those measurements exclude audio
device work and the decoder's other formats.

Similarly, isolated production clock routines measured about 1.8× faster
`now`/deadline calls, and a scan of 64 guest sleepers improved from 20.63 µs to
2.18 µs by sharing the clock sample (seven medians, one million calls for
individual routines and 50,000 scans). Scheduler and timer regressions use
deterministic timestamps; they do not depend on wall-clock timing thresholds.

For game-level validation, compare release builds on the same game, device,
backend, scene, and settings, and record frame-time distributions with the
performance overlay. GLES1 translator timings apply to that backend; native
GLES2 games benefit from the shared heap/scheduler/audio changes. No commercial
guest game or native Android/iOS build was exercised in this run, so smooth
frame pacing across all games and platforms is not established by these tests.

## Verification status

The compiled TestApp integration suite passed (2 tests), as did all 164 Rust
framework tests and the targeted graphics, allocator, timer, scheduler, clock,
and PCM32 regressions. The full `cargo test` run exposed an existing ARM64
Dynarmic debug memory-abort-check crash: a 64-bit acquire load reads a
four-byte-aligned 32-bit halt flag. The emitter must use a 32-bit load/test.
The final fix is included in the wrapper's private patched source copy; the
submodule and GitHub Actions files remain unchanged. A complete suite rerun
after that final correction remains pending.
The private-source patch helper's regression passed. Changing the faulting
instruction from `ldar x16` to `ldar w16` in GDB also made the existing fastmem
regression pass and exit normally; that does not replace a rebuilt suite run.

`dev-scripts/lint.sh` stops on pre-existing comment lines longer than 80
characters. Changed comments pass that check and `git diff --check` passes.
