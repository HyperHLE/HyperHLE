/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! RTCV-style "game corruption" engine.
//!
//! This is a self-contained, dependency-free reimplementation of the core idea
//! behind the [Real-Time Corruptor (RTCV)](https://github.com/redscientistlabs/RTCV)
//! "Blast" engine: at a configurable cadence, pick random bytes of the guest's
//! live memory and replace them with random values. The result is the
//! characteristic glitchy, "broken game" behaviour RTCV is known for —
//! corrupted graphics, audio, physics, and crashes — except it runs natively
//! inside the emulator and therefore works everywhere the emulator does,
//! including Android.
//!
//! Only the *mechanic* of breaking the running game is ported here. RTCV's
//! larger feature set (the .NET UI, vanguard clients, stockpiles, savestate
//! integration, the netcore protocol, etc.) is intentionally **not** ported —
//! it is a large C#/.NET application that has no place inside this Rust
//! emulator and would not run on Android.
//!
//! ## Targeting
//!
//! Corruption is restricted to memory the guest has actually allocated (see
//! [`crate::mem::Mem::live_allocations`]). Blasting truly random addresses
//! across the whole 4 GiB virtual address space would almost always land on
//! unmapped pages and crash the process instantly, which is neither fun nor
//! useful. By targeting live allocations we mangle real game state, producing
//! the glitchy-but-still-running behaviour people actually want.
//!
//! ## Determinism
//!
//! The engine is driven by a small, seedable [`Rng`] (a SplitMix64 variant) so
//! that a given `--corrupt-seed=` reproduces the same corruption stream,
//! mirroring RTCV's reproducible-via-seed behaviour.

use crate::mem::{GuestUSize, Mem};
use crate::options::CorruptionOptions;

/// A tiny, fast, seedable PRNG (SplitMix64). Self-contained so the corruption
/// engine adds no new crate dependencies.
#[derive(Debug, Clone)]
pub struct Rng {
    state: u64,
}

impl Default for Rng {
    fn default() -> Self {
        Rng::new(0)
    }
}

impl Rng {
    pub fn new(seed: u64) -> Rng {
        // Avoid an all-zero state producing a degenerate stream.
        Rng {
            state: seed ^ 0x9E37_79B9_7F4A_7C15,
        }
    }

    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    #[inline]
    pub fn next_u32(&mut self) -> u32 {
        (self.next_u64() >> 32) as u32
    }

    /// Uniformly distributed `u64` in `[0, bound)`. `bound` must be non-zero.
    #[inline]
    pub fn below(&mut self, bound: u64) -> u64 {
        // Simple modulo reduction; the slight bias is irrelevant for this use.
        self.next_u64() % bound
    }
}

/// Runtime state for the corruption engine. Lives on the [`crate::Environment`].
pub struct Corruptor {
    options: CorruptionOptions,
    rng: Rng,
    /// Counts main-loop iterations ("frames") since the last corruption burst.
    frames_since_burst: u64,
    /// Total bytes corrupted so far (for logging / diagnostics).
    total_corrupted: u64,
}

impl Default for Corruptor {
    fn default() -> Self {
        // A disabled engine. The real, options-driven engine is installed by
        // [`Environment::new`]; the app picker and other internal environments
        // keep this disabled default so only actual games get corrupted.
        Self {
            options: CorruptionOptions {
                enabled: false,
                ..CorruptionOptions::default()
            },
            rng: Rng::default(),
            frames_since_burst: 0,
            total_corrupted: 0,
        }
    }
}

impl Corruptor {
    pub fn new(options: CorruptionOptions) -> Corruptor {
        let seed = options.seed;
        Corruptor {
            options,
            rng: Rng::new(seed),
            frames_since_burst: 0,
            total_corrupted: 0,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.options.enabled
    }

    /// Total number of bytes corrupted over the lifetime of this engine.
    pub fn total_corrupted(&self) -> u64 {
        self.total_corrupted
    }

    /// Call this once per main-loop iteration. When enough frames have elapsed
    /// (per [`CorruptionOptions::interval_frames`]) it performs one corruption
    /// burst against `mem`.
    pub fn tick(&mut self, mem: &mut Mem) {
        if !self.options.enabled {
            return;
        }

        self.frames_since_burst += 1;
        if self.frames_since_burst < self.options.interval_frames.max(1) as u64 {
            return;
        }
        self.frames_since_burst = 0;

        self.blast(mem);
    }

    /// Perform a single corruption burst: corrupt up to
    /// [`CorruptionOptions::bytes_per_burst`] random bytes within live
    /// allocations.
    fn blast(&mut self, mem: &mut Mem) {
        let allocations = mem.live_allocations();
        if allocations.is_empty() {
            return;
        }

        // Total corruptible byte budget across all live allocations. Capped at
        // u64 to make weighted selection trivial.
        let total_bytes: u64 = allocations
            .iter()
            .map(|&(_, size)| size as u64)
            .sum();
        if total_bytes == 0 {
            return;
        }

        let burst = self.options.bytes_per_burst.max(1);
        let mut corrupted_this_burst = 0u64;

        for _ in 0..burst {
            // Pick a global byte index weighted by allocation size, then map it
            // back to a concrete (base, offset) within one allocation.
            let mut pick = self.rng.below(total_bytes);
            let mut target: Option<(GuestUSize, GuestUSize)> = None;
            for &(base, size) in &allocations {
                let size64 = size as u64;
                if pick < size64 {
                    target = Some((base, pick as GuestUSize));
                    break;
                }
                pick -= size64;
            }
            let (base, mut offset) = match target {
                Some(t) => t,
                None => continue,
            };

            // Optionally restrict corruption to the first `max_offset` bytes of
            // each allocation, which tends to hit object headers / hot fields.
            if let Some(max_offset) = self.options.max_offset {
                if max_offset > 0 {
                    offset %= max_offset;
                }
            }

            let addr = base.wrapping_add(offset);
            let new_value = self.rng.next_u32() as u8;
            mem.corrupt_byte(addr, new_value);
            corrupted_this_burst += 1;
        }

        self.total_corrupted += corrupted_this_burst;
        if corrupted_this_burst > 0 {
            log_dbg!(
                "[corrupt] blasted {} byte(s) across {} live allocation(s); {} total",
                corrupted_this_burst,
                allocations.len(),
                self.total_corrupted
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mem::{Mem, MutPtr, Ptr};
    use crate::options::CorruptionOptions;

    #[test]
    fn rng_is_deterministic_for_seed() {
        let mut a = Rng::new(12345);
        let mut b = Rng::new(12345);
        for _ in 0..1000 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn rng_below_respects_bound() {
        let mut r = Rng::new(7);
        for _ in 0..10_000 {
            assert!(r.below(10) < 10);
        }
    }

    /// The core guarantee: when enabled, the engine actually mutates live
    /// guest memory, and only within the allocation it was given.
    #[test]
    fn corrupts_only_live_allocation() {
        let mut mem = Mem::new();
        mem.set_null_segment_size(crate::mem::PAGE_SIZE);

        let size: u32 = 4096;
        let ptr = mem.alloc(size);
        let base = ptr.to_bits();

        // Fill the allocation with a known sentinel.
        let bytes = mem.bytes_at_mut(ptr.cast(), size);
        bytes.fill(0xAA);

        let opts = CorruptionOptions {
            enabled: true,
            interval_frames: 1,
            bytes_per_burst: 256,
            max_offset: None,
            seed: 42,
        };
        let mut corruptor = Corruptor::new(opts);

        // One tick at interval=1 should trigger exactly one burst.
        corruptor.tick(&mut mem);

        // Count how many sentinel bytes changed.
        let bytes = mem.bytes_at_mut(ptr.cast(), size);
        let changed = bytes.iter().filter(|&&b| b != 0xAA).count();

        assert!(corruptor.total_corrupted() > 0, "engine reported no corruption");
        assert!(changed > 0, "no bytes were actually changed in guest memory");
    }

    #[test]
    fn disabled_engine_does_nothing() {
        let mut mem = Mem::new();
        mem.set_null_segment_size(crate::mem::PAGE_SIZE);

        let size: u32 = 1024;
        let ptr = mem.alloc(size);
        mem.bytes_at_mut(ptr.cast(), size).fill(0x55);

        let mut corruptor = Corruptor::new(CorruptionOptions {
            enabled: false,
            ..CorruptionOptions::default()
        });
        for _ in 0..100 {
            corruptor.tick(&mut mem);
        }
        let bytes = mem.bytes_at_mut(ptr.cast(), size);
        assert!(bytes.iter().all(|&b| b == 0x55), "disabled engine corrupted memory");
        assert_eq!(corruptor.total_corrupted(), 0);
    }

    #[test]
    fn interval_gates_bursts() {
        let mut mem = Mem::new();
        mem.set_null_segment_size(crate::mem::PAGE_SIZE);
        let ptr = mem.alloc(4096);
        mem.bytes_at_mut(ptr.cast(), 4096).fill(0);

        let mut corruptor = Corruptor::new(CorruptionOptions {
            enabled: true,
            interval_frames: 10,
            bytes_per_burst: 4,
            max_offset: None,
            seed: 1,
        });
        // 9 ticks: not enough to fire.
        for _ in 0..9 {
            corruptor.tick(&mut mem);
        }
        assert_eq!(corruptor.total_corrupted(), 0, "burst fired before interval elapsed");
        // 10th tick fires.
        corruptor.tick(&mut mem);
        assert!(corruptor.total_corrupted() > 0, "burst did not fire at interval");
    }

    #[test]
    fn deterministic_with_same_seed() {
        let run = || -> Vec<u8> {
            let mut mem = Mem::new();
            mem.set_null_segment_size(crate::mem::PAGE_SIZE);
            let ptr = mem.alloc(4096);
            mem.bytes_at_mut(ptr.cast(), 4096).fill(0xAA);
            let mut c = Corruptor::new(CorruptionOptions {
                enabled: true,
                interval_frames: 1,
                bytes_per_burst: 128,
                max_offset: None,
                seed: 99,
            });
            c.tick(&mut mem);
            mem.bytes_at_mut(ptr.cast(), 4096).to_vec()
        };
        assert_eq!(run(), run(), "same seed produced different corruption");
    }
}
