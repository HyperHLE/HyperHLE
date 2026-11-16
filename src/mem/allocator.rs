/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
use super::{GuestUSize, Mem, VAddr, PAGE_SIZE, PAGE_SIZE_ALIGN_MASK};
use std::collections::{BTreeMap, BTreeSet};
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
    pub struct SizeIndexedChunkMap {
        chunks: ChunkMap,
        // Best-fit lookup and removal must not scan every free chunk in a
        // size bucket. Fragmented game heaps can have thousands of them.
        chunks_by_size: BTreeSet<(GuestUSize, VAddr)>,
    }
    impl SizeIndexedChunkMap {
        pub fn insert(&mut self, chunk: Chunk) {
            assert!(chunk.size.get() >= MIN_CHUNK_SIZE);
            self.chunks.insert(chunk);
            assert!(self.chunks_by_size.insert((chunk.size.get(), chunk.base)));
        }

        #[inline(always)]
        fn remove_from_size_index(&mut self, chunk: Chunk) {
            assert!(self.chunks_by_size.remove(&(chunk.size.get(), chunk.base)));
        }

        pub fn remove_with_base(&mut self, base: VAddr) -> Option<Chunk> {
            let chunk = self.chunks.remove_with_base(base)?;
            self.remove_from_size_index(chunk);
            Some(chunk)
        }

        pub fn remove_with_end(&mut self, end: VAddr) -> Option<Chunk> {
            let chunk = self.chunks.remove_with_end(end)?;
            self.remove_from_size_index(chunk);
            Some(chunk)
        }

        pub fn allocate(&mut self, size: GuestUSize) -> Option<Chunk> {
            assert!(size >= MIN_CHUNK_SIZE);
            let &(_, base) = self.chunks_by_size.range((size, 0)..).next()?;
            let existing = self.remove_with_base(base).unwrap();

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

        pub fn iter(&self) -> impl Iterator<Item = Chunk> + '_ {
            self.chunks.iter()
        }
    }
}
use collections::{ChunkMap, SizeIndexedChunkMap};

/// Tracks which memory is in use and makes allocations from it.
#[derive(Debug)]
pub struct Allocator {
    used_chunks: ChunkMap,
    unused_chunks: SizeIndexedChunkMap,
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

        let mut unused_chunks: SizeIndexedChunkMap = Default::default();
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
        // A sub-16-byte remainder is absorbed into the allocation. Check
        // the actual chunk size too, so that absorption cannot exceed a cap.
        if !self.fits_under_limit(alloc.size.get()) {
            self.unused_chunks.insert(alloc);
            super::log_out_of_memory("malloc", aligned_size);
            return 0;
        }
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
        let alignment = if new_size >= PAGE_SIZE {
            PAGE_SIZE
        } else {
            MIN_CHUNK_SIZE
        };
        let new_size = new_size.checked_next_multiple_of(alignment)?;
        // Validate before subtracting or removing anything from the free map.
        if new_size <= old_size
            || self.used_chunks.get_size_with_base(base).map(|s| s.get()) != Some(old_size)
            || (new_size >= PAGE_SIZE && base & PAGE_SIZE_ALIGN_MASK != 0)
        {
            return None;
        }
        let extra = new_size - old_size;

        let adjacent_base = base.checked_add(old_size)?;
        let adjacent = self.unused_chunks.remove_with_base(adjacent_base)?;
        let adjacent_size = adjacent.size.get();
        if adjacent_size < extra {
            // Not enough room; put the chunk back exactly as it was.
            self.unused_chunks.insert(adjacent);
            return None;
        }

        let remainder = adjacent_size - extra;
        let grown_size = if remainder < MIN_CHUNK_SIZE {
            old_size + adjacent_size
        } else {
            new_size
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

        if remainder >= MIN_CHUNK_SIZE {
            let remainder_base = adjacent_base + extra;
            if remainder >= PAGE_SIZE && remainder_base & PAGE_SIZE_ALIGN_MASK != 0 {
                // Keep the large remainder page-aligned by splitting off its
                // leading partial page. Never absorb a whole large free
                // region: Mem::realloc would clear all of it, potentially
                // gigabytes, for a tiny buffer growth.
                let leading_size = PAGE_SIZE - (remainder_base & PAGE_SIZE_ALIGN_MASK);
                self.unused_chunks
                    .insert(Chunk::new(remainder_base, leading_size));
                self.unused_chunks.insert(Chunk::new(
                    remainder_base + leading_size,
                    remainder - leading_size,
                ));
            } else {
                self.unused_chunks
                    .insert(Chunk::new(remainder_base, remainder));
            }
        }

        assert!(self.used_chunks.remove_with_base(base).is_some());
        self.used_chunks.insert(Chunk::new(base, grown_size));
        self.set_allocated_bytes(self.allocated_bytes.saturating_add(grown_size - old_size));

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

    // realloc may grow in place, but must retain the original contents and
    // leave other allocations untouched. Apple's malloc(3) contract:
    // https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man3/malloc.3.html
    #[test]
    fn grow_before_large_free_region_keeps_the_rest_available() {
        let mut allocator = arena();
        let base = alloc_at(&mut allocator, PAGE_SIZE);
        assert_eq!(
            allocator.grow_in_place(base, PAGE_SIZE, 2 * PAGE_SIZE),
            Some(2 * PAGE_SIZE)
        );
        assert_eq!(allocator.try_find_allocated_size(base), Some(2 * PAGE_SIZE));
        assert_eq!(alloc_at(&mut allocator, PAGE_SIZE), base + 2 * PAGE_SIZE);
    }

    #[test]
    fn grow_splits_partial_page_without_overallocating() {
        let mut allocator = arena();
        let base = alloc_at(&mut allocator, PAGE_SIZE - 16);
        assert_eq!(
            allocator.grow_in_place(base, PAGE_SIZE - 16, PAGE_SIZE - 1),
            Some(PAGE_SIZE)
        );
        // Now grow by one byte into the large page-aligned free region.
        assert_eq!(
            allocator.grow_in_place(base, PAGE_SIZE, PAGE_SIZE + 1),
            Some(2 * PAGE_SIZE)
        );
        let next = alloc_at(&mut allocator, PAGE_SIZE);
        assert_eq!(next, base + 2 * PAGE_SIZE);
        assert_eq!(next & PAGE_SIZE_ALIGN_MASK, 0);
    }

    #[test]
    fn small_growth_into_page_keeps_partial_page_and_large_remainder() {
        let mut allocator = arena();
        // Fill the first page with small allocations, then grow its last
        // block into the next page by only 16 bytes.
        let _prefix = alloc_at(&mut allocator, PAGE_SIZE - 16);
        let base = alloc_at(&mut allocator, 16);
        assert_eq!(allocator.grow_in_place(base, 16, 32), Some(32));
        assert_eq!(alloc_at(&mut allocator, 16), base + 32);
        let page = alloc_at(&mut allocator, PAGE_SIZE);
        assert_eq!(page & PAGE_SIZE_ALIGN_MASK, 0);
        assert_eq!(page, (base + 32 + PAGE_SIZE - 1) & !PAGE_SIZE_ALIGN_MASK);
    }

    #[test]
    fn failed_growth_preserves_allocation_and_free_indexes() {
        let mut allocator = arena();
        let base = alloc_at(&mut allocator, PAGE_SIZE);
        allocator.set_memory_limit(Some(2 * PAGE_SIZE));
        assert_eq!(
            allocator.grow_in_place(base, PAGE_SIZE, 3 * PAGE_SIZE),
            None
        );
        assert_eq!(
            allocator.grow_in_place(base, PAGE_SIZE - 16, 2 * PAGE_SIZE),
            None
        );
        assert_eq!(allocator.grow_in_place(base, PAGE_SIZE, u32::MAX), None);
        assert_eq!(allocator.try_find_allocated_size(base), Some(PAGE_SIZE));
        assert_eq!(
            allocator.grow_in_place(base, PAGE_SIZE, PAGE_SIZE + 1),
            Some(2 * PAGE_SIZE)
        );
        assert_eq!(allocator.free(base), 2 * PAGE_SIZE);
        allocator.set_memory_limit(None);
        assert_eq!(alloc_at(&mut allocator, 3 * PAGE_SIZE), base);
    }
}

#[cfg(test)]
mod free_index_tests {
    use super::{Allocator, Chunk, PAGE_SIZE};

    #[test]
    fn best_fit_survives_fragmentation_and_coalescing() {
        let mut allocator = Allocator::new();
        allocator.reserve(Chunk::new(0, PAGE_SIZE));
        let mut holes = Vec::new();
        for size in [1536, 1280, 1792, 1024] {
            holes.push((allocator.alloc(size), size));
            assert_ne!(allocator.alloc(16), 0); // Prevent coalescing the holes.
        }
        for &(base, size) in &holes {
            assert_eq!(allocator.free(base), size);
        }
        for index in [3, 1, 0, 2] {
            let (base, size) = holes[index];
            assert_eq!(allocator.alloc(size), base);
        }
        for &(base, size) in &holes {
            assert_eq!(allocator.free(base), size);
        }
        assert_eq!(allocator.alloc(1280), holes[1].0);
    }

    #[test]
    fn cap_rejection_restores_absorbed_chunk() {
        let mut allocator = Allocator::new();
        allocator.reserve(Chunk::new(0, PAGE_SIZE));
        allocator.reserve(Chunk::new(PAGE_SIZE + 17, PAGE_SIZE - 17));
        allocator.set_memory_limit(Some(16));
        assert_eq!(allocator.alloc(16), 0);
        allocator.set_memory_limit(Some(17));
        assert_eq!(allocator.alloc(16), PAGE_SIZE);
        assert_eq!(allocator.try_find_allocated_size(PAGE_SIZE), Some(17));
        assert_eq!(allocator.free(PAGE_SIZE), 17);
        assert!(allocator.is_free(Chunk::new(PAGE_SIZE, 17)));
    }

    #[test]
    fn mixed_allocations_preserve_space_alignment_and_cap() {
        for mut seed in [1_u64, 0xbadfeed, 0x10293847] {
            let mut allocator = Allocator::new();
            allocator.reserve(Chunk::new(0, PAGE_SIZE));
            let cap = 1024 * 1024;
            allocator.set_memory_limit(Some(cap));
            let mut live = Vec::new();
            for _ in 0..2000 {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                let random = (seed >> 32) as u32;
                if live.is_empty() || (random % 3 == 0 && live.len() < 64) {
                    let base = allocator.alloc(1 + random % 32768);
                    if base != 0 {
                        live.push((base, allocator.try_find_allocated_size(base).unwrap()));
                    }
                } else {
                    let index = random as usize % live.len();
                    let (base, old_size) = live[index];
                    if random % 3 == 1 {
                        assert_eq!(allocator.free(base), old_size);
                        live.swap_remove(index);
                    } else {
                        // Debug includes both free-space indexes and counters.
                        let before = format!("{allocator:?}");
                        if let Some(size) =
                            allocator.grow_in_place(base, old_size, old_size + 1 + random % 32768)
                        {
                            live[index].1 = size;
                        } else {
                            assert_eq!(format!("{allocator:?}"), before);
                        }
                    }
                }
                let mut chunks: Vec<_> = allocator
                    .used_chunks
                    .iter()
                    .chain(allocator.unused_chunks.iter())
                    .collect();
                chunks.sort_by_key(|chunk| chunk.base);
                let mut end = 0_u64;
                for chunk in chunks {
                    // All guest addresses must be covered exactly once.
                    assert_eq!(chunk.base as u64, end);
                    if chunk.size.get() >= PAGE_SIZE {
                        assert_eq!(chunk.base % PAGE_SIZE, 0);
                    }
                    end += u64::from(chunk.size.get());
                }
                assert_eq!(end, 1_u64 << 32);
                assert_eq!(
                    allocator.allocated_bytes,
                    live.iter().map(|item| item.1).sum::<u32>()
                );
                assert!(allocator.allocated_bytes <= cap);
                for &(base, size) in &live {
                    assert_eq!(base % 16, 0);
                    assert_eq!(allocator.try_find_allocated_size(base), Some(size));
                }
            }
        }
    }

    #[test]
    #[ignore = "manual timing benchmark; run with --release --ignored --nocapture"]
    fn benchmark_fragmented_heap() {
        use std::{hint::black_box, time::Instant};
        for count in [1000, 4000, 16000] {
            let mut timings = Vec::new();
            for _ in 0..5 {
                let mut allocator = Allocator::new();
                allocator.reserve(Chunk::new(0, PAGE_SIZE));
                let mut holes = Vec::new();
                for _ in 0..count {
                    holes.push(allocator.alloc(1536));
                    black_box(allocator.alloc(16));
                }
                for base in holes {
                    black_box(allocator.free(base));
                }
                let start = Instant::now();
                for _ in 0..count {
                    assert_ne!(black_box(allocator.alloc(1024)), 0);
                }
                timings.push(start.elapsed());
            }
            timings.sort();
            println!(
                "{count} fragmented allocations: {:?} (median of 5)",
                timings[2]
            );
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
