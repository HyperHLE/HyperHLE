/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
use super::{GuestUSize, Mem, VAddr, PAGE_SIZE, PAGE_SIZE_ALIGN_MASK};
use std::collections::BTreeMap;
use std::num::NonZeroU32;

/// iPhone OS's allocator always aligns to 16 bytes at minimum, and this
/// is also the minimum allocation size.
pub const MIN_CHUNK_SIZE: GuestUSize = 16;

/// A non-empty range of bytes in virtual address space.
///
/// Similar to [`RangeInclusive<u32>`][std::ops::RangeInclusive] but with a
/// more convenient representation.
#[derive(PartialEq, Eq, Copy, Clone)]
pub struct Chunk {
    pub(super) base: VAddr,
    pub(super) size: NonZeroU32,
}

impl Chunk {
    pub fn new(base: VAddr, size: GuestUSize) -> Chunk {
        if size >= PAGE_SIZE {
            // Check for invariant of page alignment
            assert!(base & PAGE_SIZE_ALIGN_MASK == 0);
        }
        Chunk {
            base,
            size: NonZeroU32::new(size).unwrap(),
        }
    }

    #[inline(always)]
    fn last_byte(&self) -> VAddr {
        self.base + (self.size.get() - 1)
    }

    #[inline(always)]
    fn contains(&self, addr: VAddr) -> bool {
        self.base <= addr && addr <= self.last_byte()
    }

    #[inline(always)]
    fn trisect_by(&self, middle: Chunk) -> Option<(Option<Chunk>, Option<Chunk>)> {
        if !self.contains(middle.base) || !self.contains(middle.last_byte()) {
            return None;
        }

        let left = match middle.base - self.base {
            0 => None,
            size => Some(Chunk::new(self.base, size)),
        };
        let right = match self.last_byte() - middle.last_byte() {
            0 => None,
            size => Some(Chunk::new(middle.last_byte() + 1, size)),
        };
        Some((left, right))
    }
}

impl std::fmt::Debug for Chunk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Chunk ({:#x}–{:#x}; {:#x} bytes)",
            self.base,
            self.base + (self.size.get() - 1),
            self.size.get()
        )
    }
}

#[cfg(test)]
mod chunk_tests {
    use super::Chunk;
    #[test]
    fn test() {
        assert!(Chunk::new(2, 4).contains(2));
        assert!(Chunk::new(2, 4).contains(5));
        assert!(!Chunk::new(2, 4).contains(6));

        assert_eq!(
            Chunk::new(2, 4).trisect_by(Chunk::new(3, 2)),
            Some((Some(Chunk::new(2, 1)), Some(Chunk::new(5, 1))))
        );
        assert_eq!(
            Chunk::new(2, 4).trisect_by(Chunk::new(2, 2)),
            Some((None, Some(Chunk::new(4, 2))))
        );
        assert_eq!(
            Chunk::new(2, 4).trisect_by(Chunk::new(4, 2)),
            Some((Some(Chunk::new(2, 2)), None))
        );
        assert_eq!(Chunk::new(2, 4).trisect_by(Chunk::new(1, 2)), None);
        assert_eq!(Chunk::new(2, 4).trisect_by(Chunk::new(5, 2)), None);
    }
}

/// Specialized collection types. They're kept in their own module so the
/// allocator can only access them via their public methods, so that there's
/// less places inconsistencies between the sub-collections could happen.
mod collections {
    use super::*;

    #[derive(Default, Debug)]
    pub struct ChunkMap {
        chunks: BTreeMap<VAddr, NonZeroU32>,
    }
    impl ChunkMap {
        #[inline(always)]
        pub fn insert(&mut self, Chunk { base, size }: Chunk) {
            assert!(self.chunks.insert(base, size).is_none());
        }
        #[inline(always)]
        pub fn remove_with_base(&mut self, base: VAddr) -> Option<Chunk> {
            self.chunks.remove(&base).map(|size| Chunk { base, size })
        }
        #[inline(always)]
        pub fn remove_with_end(&mut self, end: VAddr) -> Option<Chunk> {
            let (&base, &size) = self.chunks.range(..end).next_back()?;
            let chunk = Chunk { base, size };
            if chunk.last_byte() + 1 != end {
                return None;
            }
            Some(self.remove_with_base(chunk.base).unwrap())
        }
        #[inline(always)]
        pub fn get_size_with_base(&self, base: VAddr) -> Option<NonZeroU32> {
            self.chunks.get(&base).copied()
        }

        /// Iterate over all chunks currently stored in this map.
        pub fn iter(&self) -> impl Iterator<Item = Chunk> + '_ {
            self.chunks
                .iter()
                .map(|(&base, &size)| Chunk { base, size })
        }
    }

    #[derive(Default, Debug)]
    pub struct SizeBucketedChunkMap {
        chunks: ChunkMap,
        chunks_by_log2_size: [Vec<Chunk>; Self::bucket_for(u32::MAX) + 1],
    }
    impl SizeBucketedChunkMap {
        /// Get log2 size bucket for chunk.
        #[inline(always)]
        const fn bucket_for(size: GuestUSize) -> usize {
            (size.ilog2() - MIN_CHUNK_SIZE.ilog2()) as usize
        }

        pub fn insert(&mut self, chunk: Chunk) {
            assert!(chunk.size.get() >= MIN_CHUNK_SIZE);
            self.chunks.insert(chunk);
            self.chunks_by_log2_size[Self::bucket_for(chunk.size.get())].push(chunk);
        }

        #[inline(always)]
        fn remove_from_bucket(&mut self, chunk: Chunk) {
            let bucket = &mut self.chunks_by_log2_size[Self::bucket_for(chunk.size.get())];
            // Search from the end (recent frees are usually at the end, so
            // following the generational hypothesis, that's a better place to
            // start)
            let idx = bucket
                .iter()
                .rposition(|chunk2| chunk.base == chunk2.base)
                .unwrap();
            assert_eq!(chunk, bucket.swap_remove(idx));
        }

        pub fn remove_with_base(&mut self, base: VAddr) -> Option<Chunk> {
            let chunk = self.chunks.remove_with_base(base)?;
            self.remove_from_bucket(chunk);
            Some(chunk)
        }

        pub fn remove_with_end(&mut self, end: VAddr) -> Option<Chunk> {
            let chunk = self.chunks.remove_with_end(end)?;
            self.remove_from_bucket(chunk);
            Some(chunk)
        }

        fn allocate_in_bucket(&mut self, size: GuestUSize, bucket: usize) -> Option<Chunk> {
            let (idx, _) = {
                let mut best_chunk: Option<(usize, GuestUSize)> = None;

                // Search from end because we should prefer recently-freed
                // allocations that might be the right size.
                for (idx, chunk) in self.chunks_by_log2_size[bucket]
                    .iter_mut()
                    .enumerate()
                    .rev()
                {
                    if chunk.size.get() >= size
                        && (best_chunk.is_none() || best_chunk.unwrap().1 > chunk.size.get())
                    {
                        best_chunk = Some((idx, chunk.size.get()));
                        if chunk.size.get() == size {
                            break;
                        }
                    }
                }

                best_chunk
            }?;

            let existing = self.chunks_by_log2_size[bucket].swap_remove(idx);
            let existing2 = self.chunks.remove_with_base(existing.base);
            assert_eq!(Some(existing), existing2);

            if existing.size.get() == size {
                return Some(existing);
            }

            let alloc = Chunk::new(existing.base, size);
            let rump_base = existing.base + size;
            let rump_size = existing.size.get() - size;
            if rump_size >= PAGE_SIZE && rump_base & PAGE_SIZE_ALIGN_MASK != 0 {
                assert!(existing.base & PAGE_SIZE_ALIGN_MASK == 0);
                // re-align base address by splitting in 2 chunks:
                // less than page size, not aligned
                let left_size = PAGE_SIZE - size;
                let left = Chunk::new(existing.base + size, left_size);
                // (maybe) more than page size, aligned
                let right = Chunk::new(existing.base + PAGE_SIZE, existing.size.get() - PAGE_SIZE);
                assert_eq!(left.last_byte() + 1, right.base); // sanity check, bases
                assert_eq!(left.size.get() + right.size.get(), rump_size); // sanity check, sizes
                if left_size >= MIN_CHUNK_SIZE {
                    self.insert(left);
                    self.insert(right);
                } else {
                    // The unaligned left sliver is below MIN_CHUNK_SIZE and
                    // can't be tracked by the size-bucketed free list. Give
                    // the caller the whole existing chunk instead; its base
                    // is page-aligned, so the page-alignment invariant holds.
                    return Some(Chunk::new(existing.base, existing.size.get()));
                }
            } else if rump_size >= MIN_CHUNK_SIZE {
                let rump = Chunk::new(rump_base, rump_size);
                self.insert(rump);
            } else {
                // The rump is below MIN_CHUNK_SIZE and can't go into the
                // size-bucketed free list (its invariant is `size >=
                // MIN_CHUNK_SIZE`). Absorb it into the allocation instead of
                // panicking; at most MIN_CHUNK_SIZE - 1 bytes are lost, which
                // is negligible internal fragmentation.
                return Some(Chunk::new(existing.base, existing.size.get()));
            }

            Some(alloc)
        }

        pub fn allocate(&mut self, size: GuestUSize) -> Option<Chunk> {
            assert!(size >= MIN_CHUNK_SIZE);

            // Look in the smallest bucket first. This is the only bucket where
            // an exact match can be found.

            let bucket = Self::bucket_for(size);
            if let Some(alloc) = self.allocate_in_bucket(size, bucket) {
                return Some(alloc);
            }

            // Exact match has been ruled out, find the smallest chunk in the
            // next largest non-empty bucket.

            let bucket = self.chunks_by_log2_size[bucket + 1..]
                .iter()
                .position(|bucket| !bucket.is_empty())?
                + bucket
                + 1;
            self.allocate_in_bucket(size, bucket)
        }

        pub fn iter(&self) -> impl Iterator<Item = Chunk> + '_ {
            self.chunks_by_log2_size
                .iter()
                .flat_map(|chunks| chunks.iter())
                .copied()
        }
    }
}
use collections::{ChunkMap, SizeBucketedChunkMap};

/// Tracks which memory is in use and makes allocations from it.
#[derive(Debug)]
pub struct Allocator {
    used_chunks: ChunkMap,
    unused_chunks: SizeBucketedChunkMap,
    /// Cap on the total size of the chunks handed out by [`Self::alloc`], in
    /// bytes. `None` means the heap is uncapped (the default).
    memory_limit: Option<GuestUSize>,
    /// Total size of the chunks currently handed out by [`Self::alloc`].
    /// Chunks only reserved with [`Self::reserve`] (Mach-O segments, the null
    /// segment, the main thread stack) don't count, because the cap models
    /// the app's heap and not the whole address space.
    allocated_bytes: GuestUSize,
}

impl Allocator {
    pub fn new() -> Allocator {
        // Reserve the guard above the stack too, so it can never be handed
        // out as an allocation and a stack overflow collides with it.
        let main_thread_stack = Chunk::new(
            Mem::MAIN_THREAD_STACK_LOW_END,
            Mem::MAIN_THREAD_STACK_SIZE + Mem::MAIN_THREAD_STACK_GUARD,
        );
        let rest = Chunk::new(0, Mem::MAIN_THREAD_STACK_LOW_END);

        let mut used_chunks: ChunkMap = Default::default();
        used_chunks.insert(main_thread_stack);

        let mut unused_chunks: SizeBucketedChunkMap = Default::default();
        unused_chunks.insert(rest);

        Allocator {
            used_chunks,
            unused_chunks,
            memory_limit: None,
            allocated_bytes: 0,
        }
    }

    /// Caps the total size of the live allocations made with [`Self::alloc`].
    ///
    /// `None` (the default) leaves the heap uncapped. When the cap is
    /// reached, [`Self::alloc`] returns NULL, which is what a real device
    /// does when an app runs out of RAM: `malloc` fails and the app has to
    /// cope. See `Options::memory_limit_mb`.
    pub fn set_memory_limit(&mut self, limit: Option<GuestUSize>) {
        self.memory_limit = limit;
    }

    /// Updates the heap total and mirrors it to the process-wide counter the
    /// performance overlay's VMEM field reads.
    fn set_allocated_bytes(&mut self, bytes: GuestUSize) {
        self.allocated_bytes = bytes;
        super::publish_guest_heap_bytes(bytes);
    }

    /// Whether `extra` more bytes of heap still fit under the cap.
    fn fits_under_limit(&self, extra: GuestUSize) -> bool {
        match self.memory_limit {
            None => true,
            Some(limit) => self
                .allocated_bytes
                .checked_add(extra)
                .is_some_and(|total| total <= limit),
        }
    }

    pub fn reserve(&mut self, chunk: Chunk) {
        let mut to_trisect = None;
        for unused_chunk in self.unused_chunks.iter() {
            if unused_chunk.trisect_by(chunk).is_some() {
                to_trisect = Some(unused_chunk);
                break;
            }
        }

        let Some(to_trisect) = to_trisect else {
            log!(
                "Warning: Allocator::reserve: could not reserve chunk {:?}; skipping reservation.",
                chunk
            );
            return;
        };

        let Some((before, after)) = to_trisect.trisect_by(chunk) else {
            log!(
                "Warning: Allocator::reserve: trisect_by returned None for chunk {:?}; skipping reservation.",
                chunk
            );
            return;
        };
        self.unused_chunks.remove_with_base(to_trisect.base);
        // Leftover free chunks smaller than MIN_CHUNK_SIZE can't be tracked
        // by the size-bucketed free list (its invariant is `size >=
        // MIN_CHUNK_SIZE`). Mach-O loaders regularly reserve odd-sized
        // segments (e.g. __IMPORT or padding between sections), which leaves
        // sub-16-byte slivers around them. Rather than panicking, drop them:
        // at most 15 bytes per segment boundary are lost, which is negligible
        // and matches how real allocators round reservations up.
        if let Some(before) = before.filter(|c| c.size.get() >= MIN_CHUNK_SIZE) {
            self.unused_chunks.insert(before);
        }
        if let Some(after) = after.filter(|c| c.size.get() >= MIN_CHUNK_SIZE) {
            self.unused_chunks.insert(after);
        }
        self.used_chunks.insert(chunk);
    }

    /// Returns whether all of `chunk` is free memory, i.e. it could be
    /// passed to [Self::reserve].
    pub fn is_free(&self, chunk: Chunk) -> bool {
        self.unused_chunks
            .iter()
            .any(|unused_chunk| unused_chunk.trisect_by(chunk).is_some())
    }

    pub fn alloc(&mut self, size: GuestUSize) -> VAddr {
        // ИСПРАВЛЕНИЕ: Выравнивание может привести к переполнению (overflow),
        // если игра запрашивает гигантский объем памяти (например, 0xffffffff).
        let aligned_size_opt = if size < PAGE_SIZE {
            let s = size.max(MIN_CHUNK_SIZE);
            Self::align(s, MIN_CHUNK_SIZE)
        } else {
            Self::align(size, PAGE_SIZE)
        };

        // Если переполнение произошло (вернулся None), значит запрошен
        // неадекватно большой размер. Честно возвращаем NULL (0).
        let Some(aligned_size) = aligned_size_opt else {
            log!(
                "Warning: Allocator::alloc: requested size {:#x} overflows after alignment; returning NULL.",
                size
            );
            return 0;
        };

        if !self.fits_under_limit(aligned_size) {
            // Shared with the Mach VM and mmap paths, and rate-limited: an app
            // that retries a failing allocation would otherwise flood the log.
            super::log_out_of_memory("malloc", aligned_size);
            return 0;
        }

        let Some(alloc) = self.unused_chunks.allocate(aligned_size) else {
            // The heap is full (often fragmented rather than truly used up), or
            // the RAM limiter's cap was reached above. Both share one
            // rate-limited log budget: a guest that retries every frame would
            // otherwise fill the log with this line.
            super::log_out_of_memory("malloc (heap full)", aligned_size);
            return 0;
        };
        self.used_chunks.insert(alloc);
        self.set_allocated_bytes(self.allocated_bytes.saturating_add(alloc.size.get()));

        alloc.base
    }

    // ИСПРАВЛЕНИЕ: Используем checked_add для безопасного сложения.
    fn align(size: GuestUSize, align: GuestUSize) -> Option<GuestUSize> {
        if !size.is_multiple_of(align) {
            let addend = align - (size % align);
            size.checked_add(addend)
        } else {
            Some(size)
        }
    }

    /// Used by `realloc` — logs a warning when given a non-malloc pointer,
    /// because realloc'ing a non-heap pointer is undefined behaviour on the
    /// guest side and almost always indicates a real bug.
    pub fn find_allocated_size(&mut self, base: VAddr) -> GuestUSize {
        let Some(size) = self.used_chunks.get_size_with_base(base) else {
            log!(
                "Warning: Allocator::find_allocated_size: unknown allocation at {:#x}; returning 0.",
                base
            );
            return 0;
        };
        size.get()
    }

    /// Silent variant of [`find_allocated_size`] for callers such as
    /// `malloc_size(3)` where Apple's documented contract is to *quietly*
    /// return 0 for any pointer that isn't the base of a malloc-managed
    /// allocation (see
    /// <https://developer.apple.com/library/archive/documentation/Performance/Conceptual/ManagingMemory/Articles/MallocDebug.html>
    /// and the `malloc_size(3)` manpage in the macOS / iOS SDK). Apps that
    /// hand `malloc_size` a stack pointer, a `__DATA` symbol, or an interior
    /// pointer routinely rely on this behaviour, so spamming a warning for
    /// every such call (as we used to) is both incorrect and noisy.
    pub fn try_find_allocated_size(&self, base: VAddr) -> Option<GuestUSize> {
        self.used_chunks.get_size_with_base(base).map(|s| s.get())
    }

    /// Returns whether `base` is currently a live allocation (the exact base
    /// address of a used chunk). Used by `free()` wrappers to reject clearly
    /// bogus pointers before passing them to the allocator.
    pub fn is_known_allocation(&self, base: VAddr) -> bool {
        self.used_chunks.get_size_with_base(base).is_some()
    }

    /// Try to grow the allocation at `base` in place, by taking over free
    /// space that immediately follows it.
    ///
    /// On success, returns the new (actual) size of the allocation, which is
    /// at least `new_size` and may exceed it. Returns [None] (leaving the
    /// allocator state untouched) if the following memory isn't free or
    /// growing in place would violate the allocator's invariants.
    #[must_use]
    pub fn grow_in_place(
        &mut self,
        base: VAddr,
        old_size: GuestUSize,
        new_size: GuestUSize,
    ) -> Option<GuestUSize> {
        // Keep sizes (and so the base of the free remainder split off below)
        // multiples of MIN_CHUNK_SIZE, like alloc() does. realloc() passes
        // the guest's requested size, and an odd remainder base would make
        // later allocations misaligned: an odd address handed out as a host
        // function stub is entered in Thumb mode.
        let new_size = new_size.checked_next_multiple_of(MIN_CHUNK_SIZE)?;
        let extra = new_size - old_size;

        // Only live allocations can be grown.
        if self.used_chunks.get_size_with_base(base).is_none() {
            return None;
        }

        let adjacent_base = base + old_size;
        let adjacent = self.unused_chunks.remove_with_base(adjacent_base)?;
        let adjacent_size = adjacent.size.get();
        if adjacent_size < extra {
            // Not enough room; put the chunk back exactly as it was.
            self.unused_chunks.insert(adjacent);
            return None;
        }

        let grown_size = if adjacent_size >= PAGE_SIZE {
            // Page-aligned free chunks can't be split at an arbitrary
            // offset without breaking the page-alignment invariant, so
            // take the whole chunk.
            old_size + adjacent_size
        } else {
            let remainder = adjacent_size - extra;
            if remainder < MIN_CHUNK_SIZE {
                // Too small to be a free chunk on its own; take the whole
                // chunk.
                old_size + adjacent_size
            } else {
                new_size
            }
        };

        // Chunks of PAGE_SIZE or larger must stay page-aligned. Growing a
        // small (16-byte-aligned) allocation into page-sized territory would
        // violate that, so bail out instead.
        if grown_size >= PAGE_SIZE && base & PAGE_SIZE_ALIGN_MASK != 0 {
            self.unused_chunks.insert(adjacent);
            return None;
        }

        // The real growth is known now (taking over a whole adjacent chunk can
        // cost much more than `extra`), so check it against the heap cap. The
        // caller falls back to allocate-copy-free, which fails the same way if
        // the cap is really reached.
        if !self.fits_under_limit(grown_size - old_size) {
            self.unused_chunks.insert(adjacent);
            return None;
        }

        if adjacent_size < PAGE_SIZE {
            let remainder = adjacent_size - extra;
            if remainder >= MIN_CHUNK_SIZE {
                self.unused_chunks
                    .insert(Chunk::new(adjacent_base + extra, remainder));
            }
            // else: the leftover sliver is below MIN_CHUNK_SIZE, so it can't
            // go into the size-bucketed free list; it is absorbed into the
            // grown allocation instead (grown_size already covers it because
            // we took the whole chunk in that case).
        }

        assert!(self.used_chunks.remove_with_base(base).is_some());
        self.used_chunks.insert(Chunk::new(base, grown_size));
        self.set_allocated_bytes(
            self.allocated_bytes
                .saturating_add(grown_size - old_size),
        );

        Some(grown_size)
    }

    /// Returns the size of the freed chunk so it can be zeroed if desired
    #[must_use]
    pub fn free(&mut self, base: VAddr) -> GuestUSize {
        let Some(freed) = self.used_chunks.remove_with_base(base) else {
            log!("Can't free {:#x}, unknown allocation!", base);
            return 0;
        };
        // Chunks reserved by `reserve` were never counted, so the saturating
        // subtraction keeps the bookkeeping harmless for them.
        self.set_allocated_bytes(self.allocated_bytes.saturating_sub(freed.size.get()));

        if let Some(adjacent) = self
            .unused_chunks
            .remove_with_base(freed.last_byte() + 1)
            .or_else(|| self.unused_chunks.remove_with_end(freed.base))
        {
            let new_base = freed.base.min(adjacent.base);
            let new_size = freed.size.get() + adjacent.size.get();
            if new_size >= PAGE_SIZE && new_base & PAGE_SIZE_ALIGN_MASK != 0 {
                // Invariant of page alignment would be violated!
                // So we're not combining
                // Sub-MIN_CHUNK_SIZE slivers can't live in the bucketed free
                // list, so drop them (bounded loss, matches reserve()).
                if adjacent.size.get() >= MIN_CHUNK_SIZE {
                    self.unused_chunks.insert(adjacent);
                }
                if freed.size.get() >= MIN_CHUNK_SIZE {
                    self.unused_chunks.insert(freed);
                }
            } else {
                // We are good to combine
                let combined = Chunk::new(new_base, new_size);
                if combined.size.get() >= MIN_CHUNK_SIZE {
                    self.unused_chunks.insert(combined);
                }
            }
        } else if freed.size.get() >= MIN_CHUNK_SIZE {
            self.unused_chunks.insert(freed);
        }

        freed.size.get()
    }

    /// Returns a snapshot of all currently-live allocations as
    /// `(base_address, size_in_bytes)` pairs.
    ///
    /// The game trainer uses this to search and inspect guest heap allocations.
    pub fn live_allocations(&self) -> Vec<(VAddr, GuestUSize)> {
        self.used_chunks
            .iter()
            .map(|chunk| (chunk.base, chunk.size.get()))
            .collect()
    }
}

#[cfg(test)]
mod memory_limit_tests {
    use super::{Allocator, Chunk, PAGE_SIZE};

    /// The allocator starts with no free memory at all (in production the
    /// null segment and the Mach-O segments are reserved before the app runs),
    /// so tests must reserve an arena first.
    fn arena() -> Allocator {
        let mut allocator = Allocator::new();
        // As in `grow_in_place_tests`: reserve the null segment at address 0
        // first, so that the first allocation cannot legally return base 0.
        allocator.reserve(Chunk::new(0, PAGE_SIZE));
        allocator.reserve(Chunk::new(PAGE_SIZE, 64 * PAGE_SIZE));
        allocator
    }

    #[test]
    fn allocations_fail_once_the_cap_is_reached() {
        let mut allocator = arena();
        allocator.set_memory_limit(Some(128));

        let first = allocator.alloc(64);
        let second = allocator.alloc(64);
        assert_ne!(first, 0);
        assert_ne!(second, 0);

        // 128 bytes are in use, so a third allocation of the same size must
        // fail the way malloc() does on a device with no RAM left.
        assert_eq!(allocator.alloc(64), 0);

        // Freeing gives the room back.
        assert_eq!(allocator.free(first), 64);
        let third = allocator.alloc(64);
        assert_ne!(third, 0);
    }

    #[test]
    fn growing_in_place_counts_the_extra_bytes() {
        let mut allocator = arena();
        allocator.set_memory_limit(Some(64));

        let base = allocator.alloc(16);
        assert_ne!(base, 0);
        assert_eq!(allocator.grow_in_place(base, 16, 64), Some(64));

        // The allocation now fills the whole cap, so it can't grow further.
        assert_eq!(allocator.grow_in_place(base, 64, 128), None);
    }

    #[test]
    fn an_uncapped_heap_never_rejects_an_allocation() {
        let mut allocator = arena();
        allocator.set_memory_limit(None);
        // Half of the reserved arena: the default (uncapped) configuration
        // must not hold anything back.
        assert_ne!(allocator.alloc(32 * PAGE_SIZE), 0);
    }
}

#[cfg(test)]
mod grow_in_place_tests {
    use super::{Allocator, Chunk, GuestUSize, PAGE_SIZE, PAGE_SIZE_ALIGN_MASK};

    /// The allocator starts with no free memory at all (in production the
    /// null segment / Mach-O segments are reserved by `Mem`), so tests must
    /// reserve an arena first.
    fn arena() -> Allocator {
        let mut allocator = Allocator::new();
        // Mirror what `Mem` does in production: reserve the null segment at
        // address 0 first. Without this, the very first `alloc` could legally
        // return base 0, which is indistinguishable from NULL to the guest.
        allocator.reserve(Chunk::new(0, PAGE_SIZE));
        allocator.reserve(Chunk::new(PAGE_SIZE, 64 * PAGE_SIZE));
        allocator
    }

    fn alloc_at(allocator: &mut Allocator, size: GuestUSize) -> GuestUSize {
        let base = allocator.alloc(size);
        assert_ne!(base, 0, "alloc returned NULL for size {size}");
        base
    }

    #[test]
    fn grow_in_place_takes_adjacent_free_chunk() {
        let mut allocator = arena();
        let a = alloc_at(&mut allocator, 16);
        let b = alloc_at(&mut allocator, 64);
        let c = alloc_at(&mut allocator, 16);
        // Free the middle allocation so `a`... wait, adjacency: free b, grow a.
        allocator.free(b);
        let grown = allocator.grow_in_place(a, 16, 32).expect("should grow");
        assert!(grown >= 32);
        // The grown allocation must not overlap c.
        assert!(a + grown <= c);
    }

    #[test]
    fn grow_in_place_without_adjacent_free_chunk_is_noop() {
        let mut allocator = arena();
        let a = alloc_at(&mut allocator, 16);
        let b = alloc_at(&mut allocator, 16);
        let grown = allocator.grow_in_place(a, 16, 32);
        assert!(grown.is_none());
        // Allocator state untouched: b is still live.
        assert_eq!(allocator.try_find_allocated_size(b), Some(16));
    }

    #[test]
    fn grow_in_place_rejects_non_live_base() {
        let mut allocator = arena();
        let a = alloc_at(&mut allocator, 16);
        allocator.free(a);
        assert!(allocator.grow_in_place(a, 16, 32).is_none());
    }

    #[test]
    fn grow_in_place_keeps_allocations_aligned() {
        let mut allocator = arena();
        let a = alloc_at(&mut allocator, 16);
        let b = alloc_at(&mut allocator, 256);
        let _c = alloc_at(&mut allocator, 16);
        allocator.free(b);
        let grown = allocator.grow_in_place(a, 16, 0x8b).expect("should grow");
        assert!(grown >= 0x8b);
        assert_eq!(grown % 16, 0);
        for _ in 0..4 {
            assert_eq!(alloc_at(&mut allocator, 8) % 16, 0);
        }
    }

    #[test]
    fn grow_in_place_preserves_page_alignment_invariant() {
        let mut allocator = arena();
        // Small (16-aligned) allocation directly before a large free region.
        let a = alloc_at(&mut allocator, 16);
        let b = alloc_at(&mut allocator, 2 * PAGE_SIZE);
        allocator.free(b);
        let grown = allocator.grow_in_place(a, 16, PAGE_SIZE);
        // Either it refuses (safe), or the result respects alignment.
        if let Some(grown) = grown {
            let new_base_ok = grown < PAGE_SIZE || a & PAGE_SIZE_ALIGN_MASK == 0;
            assert!(new_base_ok);
        }
    }
}

#[cfg(test)]
mod probe_tests {
    use super::Allocator;
    /// The null segment must prevent malloc from ever handing out address 0.
    #[test]
    fn alloc_never_returns_null_address_when_memory_is_available() {
        let mut a = Allocator::new();
        a.reserve(super::Chunk::new(0, super::PAGE_SIZE));
        let base = a.alloc(16);
        assert_ne!(base, 0, "malloc handed out address 0");
        assert_eq!(a.free(base), 16);
        let base2 = a.alloc(16);
        assert_ne!(base2, 0);
    }
}
