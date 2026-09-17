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
                let left = Chunk::new(existing.base + size, PAGE_SIZE - size);
                // (maybe) more than page size, aligned
                let right = Chunk::new(existing.base + PAGE_SIZE, existing.size.get() - PAGE_SIZE);
                assert_eq!(left.last_byte() + 1, right.base); // sanity check, bases
                assert_eq!(left.size.get() + right.size.get(), rump_size); // sanity check, sizes
                self.insert(left);
                self.insert(right);
            } else {
                let rump = Chunk::new(rump_base, rump_size);
                self.insert(rump);
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
}

impl Allocator {
    pub fn new() -> Allocator {
        let main_thread_stack =
            Chunk::new(Mem::MAIN_THREAD_STACK_LOW_END, Mem::MAIN_THREAD_STACK_SIZE);
        let rest = Chunk::new(0, Mem::MAIN_THREAD_STACK_LOW_END);

        let mut used_chunks: ChunkMap = Default::default();
        used_chunks.insert(main_thread_stack);

        let mut unused_chunks: SizeBucketedChunkMap = Default::default();
        unused_chunks.insert(rest);

        Allocator {
            used_chunks,
            unused_chunks,
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
        if let Some(before) = before {
            self.unused_chunks.insert(before);
        }
        if let Some(after) = after {
            self.unused_chunks.insert(after);
        }
        self.used_chunks.insert(chunk);
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

        let Some(alloc) = self.unused_chunks.allocate(aligned_size) else {
            log!(
                "Warning: Allocator::alloc: out of memory (could not find a large enough chunk for {:#x} bytes); returning NULL.",
                aligned_size
            );
            return 0;
        };
        self.used_chunks.insert(alloc);

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

    /// Returns the size of the freed chunk so it can be zeroed if desired
    #[must_use]
    pub fn free(&mut self, base: VAddr) -> GuestUSize {
        let Some(freed) = self.used_chunks.remove_with_base(base) else {
            log!("Can't free {:#x}, unknown allocation!", base);
            return 0;
        };

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
                self.unused_chunks.insert(adjacent);
                self.unused_chunks.insert(freed);
            } else {
                // We are good to combine
                let combined = Chunk::new(
                    freed.base.min(adjacent.base),
                    freed.size.get() + adjacent.size.get(),
                );
                self.unused_chunks.insert(combined);
            }
        } else {
            self.unused_chunks.insert(freed);
        }

        freed.size.get()
    }

    /// Returns a snapshot of all currently-live allocations as
    /// `(base_address, size_in_bytes)` pairs.
    ///
    /// This is used by the optional RTCV-style game-corruption engine
    /// (see [`crate::corrupt`]) so that random byte corruption can be
    /// restricted to memory the guest has actually allocated, instead of
    /// blindly poking anywhere in the 4 GiB address space (which would
    /// almost always hit unmapped pages and crash immediately).
    pub fn live_allocations(&self) -> Vec<(VAddr, GuestUSize)> {
        self.used_chunks
            .iter()
            .map(|chunk| (chunk.base, chunk.size.get()))
            .collect()
    }
}
