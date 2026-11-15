/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Types related to the virtual memory of the emulated application, or the
//! "guest memory".
//!
//! The virtual address space is 32-bit, as is the pointer size.
//!
//! No attempt is made to do endianness conversion for reads and writes to
//! memory, because all supported emulated and host platforms are little-endian.
//!
//! Relevant Apple documentation:
//! * [Memory Usage Performance Guidelines](https://developer.apple.com/library/archive/documentation/Performance/Conceptual/ManagingMemory/ManagingMemory.html)

use std::num::NonZeroU32;

use crate::libc::wchar::wchar_t;

mod allocator;
mod host;

/// Equivalent of `usize` for guest memory.
pub type GuestUSize = u32;

/// Equivalent of `isize` for guest memory.
pub type GuestISize = i32;

/// Nonzero version of [GuestUSize].
pub type NonZeroGuestUSize = NonZeroU32;

/// [std::mem::size_of], but returning a [GuestUSize].
pub const fn guest_size_of<T: Sized>() -> GuestUSize {
    assert!(std::mem::size_of::<T>() <= u32::MAX as usize);
    std::mem::size_of::<T>() as u32
}

/// Internal type for representing an untyped virtual address.
type VAddr = GuestUSize;

/// Internal type for representing an untyped virtual address.
type NonZeroVAddr = NonZeroGuestUSize;

/// Pointer type for guest memory, or the "guest pointer" type.
///
/// The `MUT` type parameter determines whether this is mutable or not.
/// Don't write it out explicitly, use [ConstPtr], [MutPtr], [ConstVoidPtr] or
/// [MutVoidPtr] instead instead.
///
/// The implemented methods try to mirror the Rust [pointer] type's methods,
/// where possible.
#[repr(transparent)]
pub struct Ptr<T, const MUT: bool>(VAddr, std::marker::PhantomData<T>);

// #[derive(...)] doesn't work for this type because it expects T to have the
// trait we want implemented
impl<T, const MUT: bool> Clone for Ptr<T, MUT> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T, const MUT: bool> Copy for Ptr<T, MUT> {}
impl<T, const MUT: bool> PartialEq for Ptr<T, MUT> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl<T, const MUT: bool> Eq for Ptr<T, MUT> {}
impl<T, const MUT: bool> std::hash::Hash for Ptr<T, MUT> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

/// Constant guest pointer type (like Rust's `*const T`).
pub type ConstPtr<T> = Ptr<T, false>;

/// Mutable guest pointer type (like Rust's `*mut T`).
pub type MutPtr<T> = Ptr<T, true>;

#[allow(dead_code)]
/// Constant guest pointer-to-void type (like C's `const void *`)
pub type ConstVoidPtr = ConstPtr<std::ffi::c_void>;

/// Mutable guest pointer-to-void type (like C's `void *`)
pub type MutVoidPtr = MutPtr<std::ffi::c_void>;

impl<T, const MUT: bool> Ptr<T, MUT> {
    pub const fn null() -> Self {
        Ptr(0, std::marker::PhantomData)
    }

    pub fn to_bits(self) -> VAddr {
        self.0
    }
    pub const fn from_bits(bits: VAddr) -> Self {
        Ptr(bits, std::marker::PhantomData)
    }

    pub fn cast<U>(self) -> Ptr<U, MUT> {
        Ptr::<U, MUT>::from_bits(self.to_bits())
    }

    pub fn cast_void(self) -> Ptr<std::ffi::c_void, MUT> {
        self.cast()
    }

    pub fn is_null(self) -> bool {
        self.to_bits() == 0
    }

    pub fn non_null(self) -> Option<NonNullPtr<T>> {
        NonNullPtr::try_from_bits(self.0)
    }
}

impl<T> ConstPtr<T> {
    #[allow(dead_code)]
    pub fn cast_mut(self) -> MutPtr<T> {
        Ptr::from_bits(self.to_bits())
    }
}
impl<T> MutPtr<T> {
    pub fn cast_const(self) -> ConstPtr<T> {
        Ptr::from_bits(self.to_bits())
    }
}

impl<T, const MUT: bool> Default for Ptr<T, MUT> {
    fn default() -> Self {
        Self::null()
    }
}

impl<T, const MUT: bool> std::fmt::Debug for Ptr<T, MUT> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_null() {
            write!(f, "(null)")
        } else {
            write!(f, "{:#x}", self.to_bits())
        }
    }
}

// C-like pointer arithmetic
impl<T, const MUT: bool> std::ops::Add<GuestUSize> for Ptr<T, MUT> {
    type Output = Self;

    fn add(self, other: GuestUSize) -> Self {
        let size: GuestUSize = guest_size_of::<T>();
        assert_ne!(size, 0);
        // Real 32-bit ARM (ARMv7-A) computes addresses modulo 2^32: pointer
        // arithmetic silently wraps around the 4 GiB address space and never
        // traps. A fault only occurs when the resulting address is actually
        // *accessed* and points at unmapped memory — and `Mem::bytes_at` /
        // `bytes_at_mut` already handle that case gracefully via the null/OOB
        // stub pages. Using `checked_*().unwrap()` here instead turned benign
        // (or already-corrupt, but guest-local) pointer math into a hard host
        // panic, e.g. when a buggy guest computes `base + (size_t)(-N)` while
        // building a `std::string`/shader buffer. Mirror the hardware: wrap.
        Self::from_bits(self.to_bits().wrapping_add(other.wrapping_mul(size)))
    }
}
impl<T, const MUT: bool> std::ops::AddAssign<GuestUSize> for Ptr<T, MUT> {
    fn add_assign(&mut self, rhs: GuestUSize) {
        *self = *self + rhs;
    }
}
impl<T, const MUT: bool> std::ops::Sub<GuestUSize> for Ptr<T, MUT> {
    type Output = Self;

    fn sub(self, other: GuestUSize) -> Self {
        let size: GuestUSize = guest_size_of::<T>();
        assert_ne!(size, 0);
        // See the note on `Add` above: 32-bit ARM address arithmetic wraps
        // modulo 2^32 and never traps, so subtracting past zero must wrap
        // rather than panic the host.
        Self::from_bits(self.to_bits().wrapping_sub(other.wrapping_mul(size)))
    }
}
impl<T, const MUT: bool> std::ops::SubAssign<GuestUSize> for Ptr<T, MUT> {
    fn sub_assign(&mut self, rhs: GuestUSize) {
        *self = *self - rhs;
    }
}

/// Non-null pointer type for guest memory, similar to [std::ptr::NonNull].
/// You should use this wrapped in [Option] when storing types instead of
/// storing null pointers.
///
/// You can convert to this type using [Ptr::non_null] (where null pointers
/// will become [None] and other pointers will becone [Some], and convert back
/// using [Self::const_ptr] and [Self::mut_ptr].
#[repr(transparent)]
pub struct NonNullPtr<T>(NonZeroVAddr, std::marker::PhantomData<T>);

#[allow(unused)]
pub type NonNullVoidPtr = NonNullPtr<std::ffi::c_void>;

// #[derive(...)] doesn't work for this type because it expects T to have the
// trait we want implemented
impl<T> Clone for NonNullPtr<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for NonNullPtr<T> {}
impl<T> PartialEq for NonNullPtr<T> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl<T> Eq for NonNullPtr<T> {}
impl<T> std::hash::Hash for NonNullPtr<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

#[allow(unused)]
impl<T> NonNullPtr<T> {
    pub fn to_bits(self) -> VAddr {
        self.0.into()
    }
    pub fn try_from_bits(bits: VAddr) -> Option<Self> {
        if bits == 0 {
            None
        } else {
            Some(Self(bits.try_into().unwrap(), std::marker::PhantomData))
        }
    }

    pub fn from_bits(bits: VAddr) -> Self {
        Self::try_from_bits(bits).expect("Tried to create a NonNullPtr with a null value!")
    }

    pub fn cast<U>(self) -> NonNullPtr<U> {
        NonNullPtr::<U>::try_from_bits(self.to_bits()).unwrap()
    }

    pub fn cast_void(self) -> NonNullPtr<std::ffi::c_void> {
        self.cast()
    }

    pub fn mut_ptr(self) -> MutPtr<T> {
        MutPtr::from_bits(self.0.into())
    }

    pub fn const_ptr(self) -> MutPtr<T> {
        MutPtr::from_bits(self.0.into())
    }
}

impl<T> std::fmt::Debug for NonNullPtr<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:#x}", self.to_bits())
    }
}

/// Marker trait for types that can be safely read from guest memory.
///
/// See also [SafeWrite] and [crate::abi].
///
/// # Safety
/// Reading from guest memory is essentially doing a [std::mem::transmute],
/// which is notoriously unsafe in Rust.
/// Only types for which all possible bit
/// patterns are legal (e.g. integers) should have this trait.
pub unsafe trait SafeRead: Sized {}
// bool is one byte in size and has 0 as false, 1 as true in both Rust and ObjC
unsafe impl SafeRead for bool {}
unsafe impl SafeRead for i8 {}
unsafe impl SafeRead for u8 {}
unsafe impl SafeRead for i16 {}
unsafe impl SafeRead for u16 {}
unsafe impl SafeRead for i32 {}
unsafe impl SafeRead for u32 {}
unsafe impl SafeRead for i64 {}
unsafe impl SafeRead for u64 {}
unsafe impl SafeRead for f32 {}
unsafe impl SafeRead for f64 {}
unsafe impl<T, const MUT: bool> SafeRead for Ptr<T, MUT> {}

/// Marker trait for types that can be written to guest memory.
///
/// Unlike for [SafeRead], there is no (Rust) safety consideration here;
/// it's
/// just a way to catch accidental use of types unintended for guest use.
/// This was added after discovering that `()` is "[Sized]" and therefore a
/// single stray semicolon can wreak havoc...
///
/// Especially for structs, be careful that the type matches the expected ABI.
/// At minimum you should have `#[repr(C, packed)]` and appropriate padding
/// members.
///
/// See also [SafeRead] and [crate::abi].
pub trait SafeWrite: Sized {}
impl<T: SafeRead> SafeWrite for T {}

// XaView BypassOOBPanic: extend the guest address space by one page past the
// 4 GiB boundary so that off-by-one/OOB guest addresses (e.g. computations
// yielding 0x1_0000_0000..0x1_0000_0fff) stay addressable instead of tripping
// range assertions in the memory accessors.
const GUEST_ADDRESS_SPACE_SIZE: usize = 1_usize << 32;
const GUEST_MEMORY_TAIL_SIZE: usize = 4096;
const GUEST_MEMORY_SIZE: usize = GUEST_ADDRESS_SPACE_SIZE + GUEST_MEMORY_TAIL_SIZE;
type Bytes = [u8; GUEST_MEMORY_SIZE];
pub const PAGE_SIZE: GuestUSize = 4096;
pub const PAGE_SIZE_ALIGN_MASK: GuestUSize = 0xfff;

/// The type that owns the guest memory and provides accessors for it.
/// One-shot diagnostic probe. The first guest store into the null segment is
/// turned into a memory abort so `Environment::debug_cpu_error` can report the
/// exact faulting PC and stack. Guest stores are the only way to learn which
/// instruction wrote through NULL: the `LAST_GUEST_PC` snapshot is taken at
/// host-call boundaries, so for a store made while dynarmic is running it is
/// stale (Asphalt 8's drift event reports the previous call, `_atan2f`).
static NULL_WRITE_PROBE_FIRED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
static NULL_WRITE_PROBE_PENDING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Claim the one-shot null-write probe. Returns `true` for exactly one caller
/// in the whole process, which must arrange for the resulting memory abort to
/// be reported. Two flags rather than one: reusing a single flag let the
/// reporting side clear it, so every later nil write re-armed the probe and
/// the full register dump was printed again.
pub fn null_write_probe_claim() -> bool {
    use std::sync::atomic::Ordering;
    if NULL_WRITE_PROBE_FIRED.swap(true, Ordering::Relaxed) {
        return false;
    }
    NULL_WRITE_PROBE_PENDING.store(true, Ordering::Relaxed);
    true
}

/// Whether a claimed null-write probe is still waiting to be reported.
pub fn null_write_probe_pending() -> bool {
    NULL_WRITE_PROBE_PENDING.load(std::sync::atomic::Ordering::Relaxed)
}

/// Mark the claimed probe as reported.
pub fn null_write_probe_clear() {
    NULL_WRITE_PROBE_PENDING.store(false, std::sync::atomic::Ordering::Relaxed);
}

pub struct Mem {
    /// This array is 4GiB in size so that it can cover the entire 32-bit
    /// virtual address space, but it should not use that much physical memory,
    /// assuming that the host OS backs it with lazily-allocated pages and we
    /// are careful to avoid accessing most of it.
    ///
    /// iPhone OS devices only had 128MiB or 256MiB of RAM total, with no swap
    /// space, so less than 6.25% of this array should be used, assuming no
    /// fragmentation.
    ///
    /// This is a raw pointer because inevitably we will have to hand out
    /// pointers to memory sometimes, and being able to hold a `&mut` on this
    /// array simultaneously seems like an undefined behavior trap.
    /// This also
    /// means that the underlying memory should never be moved, and therefore
    /// the array can't be growable.
    ///
    /// One advantage of `[u8; 1 << 32]` over `[u8]` is that it might help rustc
    /// optimize away bounds checks for `memory.bytes[ptr_32bit as usize]`.
    ///
    /// Note that unless direct memory access is disabled, the CPU emulation
    /// (dynarmic) accesses memory via this pointer directly except when a page
    /// fault occurs.
    bytes: *mut Bytes,

    /// The size of the __PAGE_ZERO segment, where pointer accesses are trapped
    /// to prevent null pointer derefrences.
    ///
    /// We don't have full memory protection, but we can check accesses in that
    /// range.
    null_segment_size: VAddr,

    allocator: allocator::Allocator,

    /// The flag to control if memory is zeroed out on free (`true`, default)
    /// or on alloc (`false`).
    ///
    /// Right now only one game, Spore Origin, is setting this value to `false`
    /// via a game-specific hack.
    /// See [crate::Environment] for more info.
    pub(super) zero_memory_on_free: bool,

    /// HACK: 64 KiB backing region for null-page / OOB stub accesses, shared
    /// by reads and writes. Zero-filled, so reading `*(void**)NULL` returns
    /// NULL, and large enough that page-straddling `read<T>` / `write<T>` /
    /// `bytes_at` calls near `0xfff` never truncate or overrun the buffer.
    null_page: *mut u8,
    /// Pinned read-only fallback buffers `(ptr_addr, len)` for rare null/OOB
    /// slices larger than `NULL_STUB_SIZE`. Once allocated, each buffer is
    /// immutable until `Drop for Mem`, avoiding any aliasing or retag hazard.
    oversize_null_reads: std::sync::Mutex<Vec<(usize, usize)>>,
    /// Exclusive fallback buffer for mutable null/OOB slices larger than
    /// `NULL_STUB_SIZE`.
    oversize_null_write: Vec<u8>,
}

const NULL_STUB_SIZE: usize = 64 * 1024;
const MAX_OVERSIZE_NULL_STUB: usize = 64 * 1024 * 1024;

impl Drop for Mem {
    fn drop(&mut self) {
        unsafe {
            crate::mem::host::free_guest_memory(self.bytes.cast(), std::mem::size_of::<Bytes>())
                .unwrap();
            if !self.null_page.is_null() {
                crate::mem::host::free_memory(self.null_page.cast(), NULL_STUB_SIZE).unwrap();
            }
            if let Ok(mut buffers) = self.oversize_null_reads.lock() {
                for (ptr_addr, cap) in buffers.drain(..) {
                    if let Ok(layout) = std::alloc::Layout::from_size_align(cap, 16) {
                        std::alloc::dealloc(ptr_addr as *mut u8, layout);
                    }
                }
            }
        }
    }
}

impl Mem {
    /// [According to Apple](https://developer.apple.com/library/archive/documentation/Cocoa/Conceptual/Multithreading/CreatingThreads/CreatingThreads.html)
    /// among others, the iPhone OS main thread stack size is 1MiB.
    pub const MAIN_THREAD_STACK_SIZE: GuestUSize = 1024 * 1024;

    /// Unused gap between the top of the main thread's stack and the top of
    /// the 32-bit address space.
    ///
    /// With the stack flush against `0xffffffff`, a stack object anywhere near
    /// the top of the stack runs off the end of the address space and wraps to
    /// zero. Asphalt 8's drift event copies a struct into a buffer at
    /// `0xffffffc8` (`str r4, [r5, #0x20]` and following, 4 bytes at a time);
    /// the first 24 bytes land at `0xffffffe8..0xfffffffc` and the rest wrap
    /// around to `0x0..0xfc`, which looked exactly like the guest storing
    /// through a NULL pointer. iPhone OS does not put the stack flush against
    /// the top of the address space, so this cannot happen there.
    pub const MAIN_THREAD_STACK_GUARD: GuestUSize = 1024 * 1024;

    /// Address of the lowest byte (not the base) of the main thread's stack.
    ///
    /// We are arbitrarily putting the stack at the top of the virtual address
    /// space (see also: stack.rs), I have no idea if this matches iPhone OS,
    /// but it is kept [Self::MAIN_THREAD_STACK_GUARD] below the very top so
    /// that stack objects cannot wrap around the address space.
    pub const MAIN_THREAD_STACK_LOW_END: VAddr = 0u32
        .wrapping_sub(Self::MAIN_THREAD_STACK_GUARD)
        .wrapping_sub(Self::MAIN_THREAD_STACK_SIZE);

    /// Address of the highest byte of the main thread's usable stack.
    pub const MAIN_THREAD_STACK_HIGH_END: VAddr =
        Self::MAIN_THREAD_STACK_LOW_END.wrapping_add(Self::MAIN_THREAD_STACK_SIZE - 1);

    /// iPhone OS secondary thread stack size.
    pub const SECONDARY_THREAD_DEFAULT_STACK_SIZE: GuestUSize = 512 * 1024;

    /// Create a fresh instance of guest memory.
    pub fn new() -> Mem {
        let size = std::mem::size_of::<Bytes>();
        let ptr = unsafe { crate::mem::host::allocate_guest_memory(size).unwrap() };

        assert_eq!(
            ptr as usize & PAGE_SIZE_ALIGN_MASK as usize,
            0,
            "Failed to align host memory with guest memory"
        );
        let bytes = ptr as *mut Bytes;

        // Allocate the page backing null-page accesses (4KB, zero-filled).
        // Data reads of a NULL pointer (e.g. `*(void**)0`) return NULL, and
        // writes through NULL are kept here so that reading the same address
        // back returns what was written rather than a zero.
        let null_page = unsafe {
            let page = crate::mem::host::allocate_memory(NULL_STUB_SIZE).unwrap();
            let page_slice = std::slice::from_raw_parts_mut(page as *mut u8, NULL_STUB_SIZE);
            page_slice.fill(0);
            page as *mut u8
        };

        let allocator = allocator::Allocator::new();
        Mem {
            bytes,
            null_segment_size: 0,
            allocator,
            zero_memory_on_free: true,
            null_page,
            oversize_null_reads: std::sync::Mutex::new(Vec::new()),
            oversize_null_write: Vec::new(),
        }
    }

    /// Sets up the null segment of the given size.
    /// There's no reason to call
    /// this outside of binary loading, and it won't be respected even if you
    /// do.
    /// The size must not have been set already, and must be page aligned.
    pub fn set_null_segment_size(&mut self, new_null_segment_size: VAddr) {
        // TODO?: Maybe this should be replaced with a per-page rwx/callback
        //        setting?
        //        Currently we don't properly follow segment
        //        protections, which means that applications can write into
        //        segments they shouldn't be able to.
        //        Adding that would fix
        //        this, along with removing this special case.
        assert!(self.null_segment_size == 0);
        assert!(new_null_segment_size.is_multiple_of(0x1000));
        self.allocator
            .reserve(allocator::Chunk::new(0, new_null_segment_size));
        self.null_segment_size = new_null_segment_size;
    }

    pub fn null_segment_size(&self) -> VAddr {
        self.null_segment_size
    }

    /// Get a pointer to the full 4GiB of memory.
    /// This is only for use when
    /// setting up the CPU, never call this otherwise.
    ///
    /// Safety: You must ensure that this pointer does not outlive the instance
    /// of [Mem].
    /// You must not use it while a `&mut` is held on some region of
    /// guest memory.
    pub unsafe fn direct_memory_access_ptr(&mut self) -> *mut std::ffi::c_void {
        self.bytes.cast()
    }

    pub(crate) fn enable_fastmem(&mut self) -> bool {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            if self.null_segment_size == 0 {
                log!("Dynarmic Fastmem enabled (no reserved null segment).");
                return true;
            }
            match unsafe {
                crate::mem::host::protect_guest_null_segment(
                    self.bytes.cast(),
                    self.null_segment_size as usize,
                )
            } {
                Ok(()) => {
                    log!(
                        "Dynarmic Fastmem enabled with a {}-byte guarded null segment.",
                        self.null_segment_size
                    );
                    true
                }
                Err(error) => {
                    log!(
                        "Dynarmic Fastmem unavailable (could not guard null segment: {}); using page table.",
                        error
                    );
                    false
                }
            }
        }
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        {
            false
        }
    }

    // Soft handler for null-page accesses. No panic; returns a stub page.
    // Rate-limited: only the first N unique (addr, is_write) pairs are logged,
    // further occurrences are silently counted. This prevents the log from
    // being flooded when the game repeatedly probes null-page addresses.
    #[cold]
    fn null_check_fail(at: VAddr, size: GuestUSize, is_write: bool, caller: &str, mem: &Mem) {
        use std::collections::HashSet;
        use std::sync::Mutex;
        // Separate budgets for reads and writes. A single shared budget lets a
        // burst of nil writes (Asphalt 8's drift event writes 64 words through
        // NULL) fill it and permanently silence the nil *reads* that follow,
        // which are exactly the accesses showing what the guest did with the
        // values it stored.
        static SEEN_READS: Mutex<Option<HashSet<VAddr>>> = Mutex::new(None);
        static SEEN_WRITES: Mutex<Option<HashSet<VAddr>>> = Mutex::new(None);
        const MAX_UNIQUE_LOGS: usize = 64;

        let mut guard = if is_write {
            SEEN_WRITES.lock().unwrap()
        } else {
            SEEN_READS.lock().unwrap()
        };
        let set = guard.get_or_insert_with(HashSet::new);
        if set.contains(&at) {
            return;
        }
        if set.len() >= MAX_UNIQUE_LOGS {
            if set.len() == MAX_UNIQUE_LOGS {
                // Insert a sentinel to emit the notice only once.
                set.insert(0xFFFF_FFFE);
                log!(
                    "touchHLE::mem: further NULL-PAGE {} warnings silenced \
                     after {} unique sites",
                    if is_write { "WRITE" } else { "READ" },
                    MAX_UNIQUE_LOGS
                );
            }
            return;
        }
        set.insert(at);
        if size > 0x1000_0000 {
            // Huge size is almost always a corrupted/-1 length; capture a
            // backtrace to identify the offending host function.
            log!(
                "touchHLE::mem: backtrace for huge-size NULL-PAGE/OOB access:\n{}",
                std::backtrace::Backtrace::force_capture()
            );
        }
        let op_type = if is_write { "WRITE" } else { "READ" };
        // Provide helpful context: small offsets are typically field accesses
        // on a nil Objective-C object pointer (nil + ivar offset). This is
        // defined behavior in ObjC (returns 0/nil) and is NOT a crash — just
        // a sign that the app is accessing a nil object's fields.
        let context = if !is_write && at < 0x1000 {
            " (likely nil ObjC object field access — returning zero)"
        } else if is_write && at < 0x1000 {
            " (likely nil ObjC object field write — discarding)"
        } else {
            " — returning stub page"
        };
        log!(
            "touchHLE::mem: NULL-PAGE {} at 0x{:08x} (size: 0x{:x}) from {}{} \
             (unique sites logged: {}/{})",
            op_type,
            at,
            size,
            caller,
            context,
            set.len(),
            MAX_UNIQUE_LOGS
        );
        if !is_write {
            return;
        }
        // For writes, the useful question is *who* dereferenced the nil
        // pointer. Print the guest side (PC/LR, the r0-r7 snapshot taken at
        // the host-function dispatch, and the raw code at both addresses)
        // plus a host backtrace naming the Rust stub that wrote through nil.
        // Emit all of it once per distinct guest PC: one host call filling a
        // buffer produces dozens of nil writes from a single site, and
        // repeating the full context for each one drowns the log.
        use std::sync::atomic::Ordering;
        let guest_pc = crate::environment::LAST_GUEST_PC.load(Ordering::Relaxed);
        let guest_lr = crate::environment::LAST_GUEST_LR.load(Ordering::Relaxed);
        static LAST_DIAG_GUEST_PC: std::sync::atomic::AtomicU32 =
            std::sync::atomic::AtomicU32::new(u32::MAX);
        if LAST_DIAG_GUEST_PC.swap(guest_pc, Ordering::Relaxed) == guest_pc {
            return;
        }
        let mut regs = String::new();
        for (i, reg) in crate::environment::LAST_HOST_CALL_REGS.iter().enumerate() {
            regs.push_str(&format!(" r{}={:#x}", i, reg.load(Ordering::Relaxed)));
        }
        let thread = crate::environment::LAST_HOST_CALL_THREAD.load(Ordering::Relaxed);
        let symbol = Self::last_host_call_symbol();
        log!(
            "touchHLE::mem:   guest PC={:#x} LR={:#x} (thread {}); host fn \
             {} called with:{}; code at PC: [{}]; code at LR: [{}]",
            guest_pc,
            guest_lr,
            thread,
            symbol,
            regs,
            Self::guest_code_dump(mem, guest_pc),
            Self::guest_code_dump(mem, guest_lr)
        );
        log!(
            "touchHLE::mem:   host backtrace:\n{}",
            std::backtrace::Backtrace::force_capture()
        );
    }

    /// Recover the symbol name recorded at the most recent host-function
    /// dispatch. The length is clamped and the bytes validated because the
    /// pointer and length are stored by two separate relaxed stores, so a
    /// concurrent dispatch can leave them mismatched.
    fn last_host_call_symbol() -> String {
        use std::sync::atomic::Ordering;
        let ptr = crate::environment::LAST_HOST_CALL_SYMBOL_PTR.load(Ordering::Relaxed);
        let len = crate::environment::LAST_HOST_CALL_SYMBOL_LEN.load(Ordering::Relaxed);
        const MAX_SYMBOL_LEN: usize = 512;
        if ptr == 0 || len == 0 || len > MAX_SYMBOL_LEN {
            return "<unknown>".to_string();
        }
        let bytes = unsafe { std::slice::from_raw_parts(ptr as *const u8, len) };
        match std::str::from_utf8(bytes) {
            Ok(s) => s.to_string(),
            Err(_) => "<unparsable>".to_string(),
        }
    }

    /// Dump 8 words of guest memory at `addr` as little-endian hex words, for
    /// logging the code around a fault. Unmapped words read as `????????`.
    /// Reads the backing array directly, so it cannot re-enter the null-page
    /// handler that calls it.
    fn guest_code_dump(mem: &Mem, addr: VAddr) -> String {
        let mut out = String::new();
        for word in 0..8u32 {
            let at = addr.wrapping_add(word * 4);
            match mem.get_bytes_fallible(ConstVoidPtr::from_bits(at), 4) {
                Some(bytes) if bytes.len() == 4 => out.push_str(&format!(
                    "{:02x}{:02x}{:02x}{:02x} ",
                    bytes[0], bytes[1], bytes[2], bytes[3]
                )),
                _ => out.push_str("???????? "),
            }
        }
        out
    }

    #[inline]
    fn null_stub_slice(&self, addr: u32, count: GuestUSize) -> &[u8] {
        let offset = (addr % PAGE_SIZE) as usize;
        let count_usize = count as usize;
        if offset.saturating_add(count_usize) <= NULL_STUB_SIZE {
            return unsafe { std::slice::from_raw_parts(self.null_page.add(offset), count_usize) };
        }
        let alloc_len = count_usize.min(MAX_OVERSIZE_NULL_STUB);
        if alloc_len == 0 {
            return &[];
        }
        let mut buffers = self.oversize_null_reads.lock().unwrap();
        if let Some(&(ptr_addr, _)) = buffers.iter().find(|&&(_, cap)| cap >= alloc_len) {
            return unsafe { std::slice::from_raw_parts(ptr_addr as *const u8, alloc_len) };
        }
        let cap = alloc_len.next_power_of_two().max(NULL_STUB_SIZE * 2);
        let layout = std::alloc::Layout::from_size_align(cap, 16).unwrap();
        let raw = unsafe { std::alloc::alloc_zeroed(layout) };
        if raw.is_null() {
            std::alloc::handle_alloc_error(layout);
        }
        let prefix = (NULL_STUB_SIZE - offset).min(cap);
        unsafe {
            std::ptr::copy_nonoverlapping(self.null_page.add(offset), raw, prefix);
        }
        buffers.push((raw as usize, cap));
        unsafe { std::slice::from_raw_parts(raw as *const u8, alloc_len) }
    }

    #[inline]
    fn null_stub_slice_mut(&mut self, addr: u32, count: GuestUSize) -> &mut [u8] {
        let offset = (addr % PAGE_SIZE) as usize;
        let count_usize = count as usize;
        if offset.saturating_add(count_usize) <= NULL_STUB_SIZE {
            return unsafe {
                std::slice::from_raw_parts_mut(self.null_page.add(offset), count_usize)
            };
        }
        let alloc_len = count_usize.min(MAX_OVERSIZE_NULL_STUB);
        if self.oversize_null_write.len() < alloc_len {
            self.oversize_null_write.resize(alloc_len, 0);
        }
        &mut self.oversize_null_write[..alloc_len]
    }

    /// Special version of [Self::bytes_at] that returns [None] rather than
    /// panicking on failure.
    /// Only for use by [crate::gdb::GdbServer].
    pub fn get_bytes_fallible(&self, addr: ConstVoidPtr, count: GuestUSize) -> Option<&[u8]> {
        if addr.to_bits() < self.null_segment_size {
            return Some(self.null_stub_slice(addr.to_bits(), count));
        }
        let start = addr.to_bits() as usize;
        let end = start.checked_add(count as usize)?;
        if end > GUEST_MEMORY_SIZE {
            return None;
        }
        Some(unsafe {
            std::slice::from_raw_parts(self.bytes.cast::<u8>().add(start), count as usize)
        })
    }
    /// Special version of [Self::bytes_at_mut] that returns [None] rather than
    /// panicking on failure.
    /// Only for use by [crate::gdb::GdbServer].
    pub fn get_bytes_fallible_mut(
        &mut self,
        addr: ConstVoidPtr,
        count: GuestUSize,
    ) -> Option<&mut [u8]> {
        if addr.to_bits() < self.null_segment_size {
            return None;
        }
        let start = addr.to_bits() as usize;
        let end = start.checked_add(count as usize)?;
        if end > GUEST_MEMORY_SIZE {
            return None;
        }
        Some(unsafe {
            std::slice::from_raw_parts_mut(self.bytes.cast::<u8>().add(start), count as usize)
        })
    }

    /// Get a slice for reading `count` bytes.
    /// This is the basic primitive for
    /// safe read-only memory access.
    #[inline]
    pub fn bytes_at<const MUT: bool>(&self, ptr: Ptr<u8, MUT>, count: GuestUSize) -> &[u8] {
        if ptr.to_bits() < self.null_segment_size {
            Self::null_check_fail(ptr.to_bits(), count, false, "bytes_at", self);
            return self.null_stub_slice(ptr.to_bits(), count);
        }
        let addr = ptr.to_bits() as usize;
        let end = addr.saturating_add(count as usize);
        if end > GUEST_MEMORY_SIZE || end < addr {
            Self::null_check_fail(ptr.to_bits(), count, false, "bytes_at(OOB)", self);
            return self.null_stub_slice(ptr.to_bits(), count);
        }
        unsafe { std::slice::from_raw_parts(self.bytes.cast::<u8>().add(addr), count as usize) }
    }
    /// Get a slice for reading `count` bytes without a null-page check.
    ///
    /// This **doesn't** panic at access within the null page.
    ///
    /// You shall have a good reason to use it instead of [Self::bytes_at]
    pub fn unchecked_bytes_at<const MUT: bool>(
        &self,
        ptr: Ptr<u8, MUT>,
        count: GuestUSize,
    ) -> &[u8] {
        let addr = ptr.to_bits() as usize;
        if ptr.to_bits() < self.null_segment_size {
            return self.null_stub_slice(ptr.to_bits(), count);
        }
        let end = addr.saturating_add(count as usize);
        if end > GUEST_MEMORY_SIZE || end < addr {
            Self::null_check_fail(ptr.to_bits(), count, false, "unchecked_bytes_at(OOB)", self);
            return self.null_stub_slice(ptr.to_bits(), count);
        }
        unsafe { std::slice::from_raw_parts(self.bytes.cast::<u8>().add(addr), count as usize) }
    }
    /// Get a slice for reading or writing `count` bytes.
    /// This is the basic
    /// primitive for safe read-write memory access.
    #[inline]
    pub fn bytes_at_mut(&mut self, ptr: MutPtr<u8>, count: GuestUSize) -> &mut [u8] {
        if ptr.to_bits() < self.null_segment_size {
            Self::null_check_fail(ptr.to_bits(), count, true, "bytes_at_mut", self);
            return self.null_stub_slice_mut(ptr.to_bits(), count);
        }
        let addr = ptr.to_bits() as usize;
        let end = addr.saturating_add(count as usize);
        if end > GUEST_MEMORY_SIZE || end < addr {
            Self::null_check_fail(ptr.to_bits(), count, true, "bytes_at_mut(OOB)", self);
            return self.null_stub_slice_mut(ptr.to_bits(), count);
        }
        unsafe { std::slice::from_raw_parts_mut(self.bytes.cast::<u8>().add(addr), count as usize) }
    }

    /// Get a pointer for reading an array of `count` elements of type `T`.
    /// Only use this for interfacing with unsafe C-like APIs.
    ///
    /// The `count` argument is purely for bounds-checking and does not affect
    /// the result.
    ///
    /// No guarantee is made about the alignment of the resulting pointer!
    /// Pointers that are well-aligned for the guest are not necessarily
    /// well-aligned for the host.
    /// Rust strictly requires pointers to be
    /// well-aligned when dereferencing them, or when constructing references or
    /// slices from them, so **be very careful**.
    #[inline]
    pub fn ptr_at<T, const MUT: bool>(&self, ptr: Ptr<T, MUT>, count: GuestUSize) -> *const T
    where
        T: SafeRead,
    {
        let size = count.checked_mul(guest_size_of::<T>()).unwrap();
        self.bytes_at(ptr.cast(), size).as_ptr().cast()
    }
    /// A variation of [Self::ptr_at] without a null-page check.
    ///
    /// This **doesn't** panic at access within the null page.
    ///
    /// You shall have a good reason to use it instead of [Self::ptr_at]
    pub fn unchecked_ptr_at<T, const MUT: bool>(
        &self,
        ptr: Ptr<T, MUT>,
        count: GuestUSize,
    ) -> *const T
    where
        T: SafeRead,
    {
        let size = count.checked_mul(guest_size_of::<T>()).unwrap();
        self.unchecked_bytes_at(ptr.cast(), size).as_ptr().cast()
    }
    /// Get a pointer for reading or writing to an array of `count` elements of
    /// type `T`.
    /// Only use this for interfacing with unsafe C-like APIs.
    ///
    /// The `count` argument is purely for bounds-checking and does not affect
    /// the result.
    ///
    /// No guarantee is made about the alignment of the resulting pointer!
    /// Pointers that are well-aligned for the guest are not necessarily
    /// well-aligned for the host.
    /// Rust strictly requires pointers to be
    /// well-aligned when dereferencing them, or when constructing references or
    /// slices from them, so **be very careful**.
    #[inline]
    pub fn ptr_at_mut<T>(&mut self, ptr: MutPtr<T>, count: GuestUSize) -> *mut T
    where
        T: SafeRead + SafeWrite,
    {
        let size = count.checked_mul(guest_size_of::<T>()).unwrap();
        self.bytes_at_mut(ptr.cast(), size).as_mut_ptr().cast()
    }

    /// Transform a host pointer addressing a location in guest memory back into
    /// a guest pointer.
    /// This exists solely to deal with OpenGL `glGetPointerv`.
    /// You should never have another reason to use this.
    ///
    /// A host pointer that does not address guest memory returns a null
    /// pointer, after logging a rate-limited warning. It used to panic, but
    /// this conversion runs on paths where the pointer comes from the app
    /// (`gl*Pointer`, `glGetPointerv`), and a panic there either unwinds
    /// through C frames (undefined behaviour) or aborts the emulator on the
    /// app's behalf. Callers must treat null as "no array".
    pub fn host_ptr_to_guest_ptr(&self, host_ptr: *const std::ffi::c_void) -> ConstVoidPtr {
        let host_ptr = host_ptr as usize;
        let guest_mem_start = self.bytes.cast::<u8>() as usize;
        let guest_mem_end = guest_mem_start + GUEST_MEMORY_SIZE;
        if !(guest_mem_start..guest_mem_end).contains(&host_ptr) {
            Self::warn_wild_host_ptr(host_ptr, guest_mem_start, guest_mem_end);
            return ConstVoidPtr::null();
        }
        let guest_addr = host_ptr - guest_mem_start;
        // XaView BypassGuestAddrOverflow: the extended address space (see
        // `Bytes`) can legitimately produce addresses past 32 bits; truncate
        // instead of panicking.
        Ptr::from_bits(guest_addr as u32)
    }

    /// Report a wild host pointer given to [Self::host_ptr_to_guest_ptr].
    ///
    /// A caller that presents the same bad pointer on every frame (a game with
    /// a broken vertex array setup does) would otherwise flood the log, so only
    /// the first few occurrences are reported.
    #[cold]
    fn warn_wild_host_ptr(host_ptr: usize, guest_mem_start: usize, guest_mem_end: usize) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        const MAX_LOGS: usize = 8;
        static LOGGED: AtomicUsize = AtomicUsize::new(0);
        let n = LOGGED.fetch_add(1, Ordering::Relaxed);
        if n < MAX_LOGS {
            log!(
                "touchHLE::mem: ignoring wild host pointer {:#x} handed to a GL array \
                 binding (guest memory is {:#x}..{:#x}); using NULL instead",
                host_ptr,
                guest_mem_start,
                guest_mem_end
            );
        } else if n == MAX_LOGS {
            log!("touchHLE::mem: further wild host pointer warnings silenced");
        }
    }

    /// Returns whether a host pointer addresses a location inside the guest's
    /// memory region. Used to sanity-check pointers that touchHLE hands to host
    /// APIs (e.g. client-side OpenGL vertex arrays): a pointer outside this
    /// range is wild and dereferencing it on the host would crash the emulator.
    pub fn is_host_ptr_in_guest_mem(&self, host_ptr: *const std::ffi::c_void) -> bool {
        let host_ptr = host_ptr as usize;
        let guest_mem_start = self.bytes.cast::<u8>() as usize;
        (guest_mem_start..guest_mem_start + GUEST_MEMORY_SIZE).contains(&host_ptr)
    }

    /// Read a value for memory.
    /// This is the preferred way to read memory in
    /// most cases.
    #[inline]
    pub fn read<T, const MUT: bool>(&self, ptr: Ptr<T, MUT>) -> T
    where
        T: SafeRead,
    {
        // This is unsafe unless we are careful with which types SafeRead is
        // implemented for!
        // This would also be unsafe if the non-unaligned method was used.
        unsafe { self.ptr_at(ptr, 1).read_unaligned() }
    }
    /// Write a value to memory.
    /// This is the preferred way to write memory in
    /// most cases.
    #[inline]
    pub fn write<T>(&mut self, ptr: MutPtr<T>, value: T)
    where
        T: SafeWrite,
    {
        let size = guest_size_of::<T>();
        assert!(size > 0);
        let slice = self.bytes_at_mut(ptr.cast(), size);
        let ptr: *mut T = slice.as_mut_ptr().cast();
        // It's unaligned because what is well-aligned for the guest is not
        // necessarily well-aligned for the host.
        // This would be unsafe if the non-unaligned method was used.
        unsafe { ptr.write_unaligned(value) }
    }

    /// C-style `memmove`.
    ///
    /// Sanity-checks the arguments. If `src + size` or `dest + size` would
    /// run off the end of the 4 GiB guest address space, the operation is
    /// logged and skipped instead of panicking. This is a defensive measure
    /// for guest code that calls `memmove`/`memcpy` with corrupted arguments
    /// (for example, an uninitialised `std::string` whose internal length
    /// happens to be wildly out of range): a guest bug shouldn't take down
    /// the whole emulator.
    pub fn memmove(&mut self, dest: MutVoidPtr, src: ConstVoidPtr, size: GuestUSize) {
        let src_addr = src.to_bits() as usize;
        let dest_addr = dest.to_bits() as usize;
        let size_us = size as usize;
        let max = GUEST_MEMORY_SIZE;

        // Early reject: if size looks like a negative i32 cast to u32
        // (>= 0x8000_0000), it's almost certainly corrupted. Guest code on
        // 32-bit ARM that passes (size_t)(-1) or similar huge values is
        // buggy — skip the operation to keep the emulator alive.
        if size >= 0x8000_0000 {
            log!(
                "WARNING: memmove with likely-negative size ({:#x} = {}); \
                 src={:#x}, dest={:#x} — skipping",
                size,
                size as i32,
                src_addr,
                dest_addr,
            );
            return;
        }

        // Also reject NULL source — real memmove(dest, NULL, n) is UB
        // but guest games (Geometry Dash) trigger it via corrupted strings.
        if src_addr == 0 && size > 0 {
            log!(
                "WARNING: memmove from NULL (dest={:#x}, size={:#x}) — skipping",
                dest_addr,
                size_us,
            );
            return;
        }

        let src_end = match src_addr.checked_add(size_us) {
            Some(v) if v <= max => v,
            _ => {
                log!(
                    "WARNING: memmove with bogus args (src={:#x}, dest={:#x}, \
                     size={:#x}) — skipping to avoid host crash",
                    src_addr,
                    dest_addr,
                    size_us
                );
                return;
            }
        };
        let dest_end = match dest_addr.checked_add(size_us) {
            Some(v) if v <= max => v,
            _ => {
                log!(
                    "WARNING: memmove with bogus args (src={:#x}, dest={:#x}, \
                     size={:#x}) — skipping to avoid host crash",
                    src_addr,
                    dest_addr,
                    size_us
                );
                return;
            }
        };
        let _ = (src_end, dest_end);

        if size_us == 0 {
            return;
        }
        if src_addr < self.null_segment_size as usize || dest_addr < self.null_segment_size as usize
        {
            const MAX_NULL_MEMMOVE_SIZE: usize = 16 * 1024 * 1024;
            if size_us > MAX_NULL_MEMMOVE_SIZE {
                log!(
                    "WARNING: memmove intersecting PAGEZERO with large size ({:#x}) — skipping",
                    size_us
                );
                return;
            }

            let mut source = Vec::with_capacity(size_us);
            let mut offset = 0;
            while offset < size_us {
                let source_address = src_addr + offset;
                let chunk_size = if source_address < self.null_segment_size as usize {
                    (size_us - offset)
                        .min(self.null_segment_size as usize - source_address)
                        .min(PAGE_SIZE as usize - source_address % PAGE_SIZE as usize)
                } else {
                    size_us - offset
                };
                if source_address < self.null_segment_size as usize {
                    let page_offset = source_address % PAGE_SIZE as usize;
                    source.extend_from_slice(unsafe {
                        std::slice::from_raw_parts(self.null_page.add(page_offset), chunk_size)
                    });
                } else {
                    source.extend_from_slice(unsafe {
                        std::slice::from_raw_parts(
                            self.bytes.cast::<u8>().add(source_address),
                            chunk_size,
                        )
                    });
                }
                offset += chunk_size;
            }

            let mut offset = 0;
            while offset < size_us {
                let destination_address = dest_addr + offset;
                let chunk_size = if destination_address < self.null_segment_size as usize {
                    (size_us - offset)
                        .min(self.null_segment_size as usize - destination_address)
                        .min(PAGE_SIZE as usize - destination_address % PAGE_SIZE as usize)
                } else {
                    size_us - offset
                };
                if destination_address < self.null_segment_size as usize {
                    let page_offset = destination_address % PAGE_SIZE as usize;
                    unsafe {
                        std::slice::from_raw_parts_mut(self.null_page.add(page_offset), chunk_size)
                    }
                    .copy_from_slice(&source[offset..offset + chunk_size]);
                } else {
                    unsafe {
                        std::slice::from_raw_parts_mut(
                            self.bytes.cast::<u8>().add(destination_address),
                            chunk_size,
                        )
                    }
                    .copy_from_slice(&source[offset..offset + chunk_size]);
                }
                offset += chunk_size;
            }
            return;
        }
        unsafe {
            std::ptr::copy(
                self.bytes.cast::<u8>().add(src_addr),
                self.bytes.cast::<u8>().add(dest_addr),
                size_us,
            );
        }
    }

    /// Allocate `size` bytes.
    pub fn alloc(&mut self, size: GuestUSize) -> MutVoidPtr {
        let ptr = Ptr::from_bits(self.allocator.alloc(size));
        if !self.zero_memory_on_free {
            self.bytes_at_mut(ptr.cast(), size).fill(0);
        }

        log_dbg!("Allocated {:?} ({:#x} bytes)", ptr, size);
        ptr
    }

    /// Allocate `size` bytes initialized to 0.
    pub fn calloc(&mut self, size: GuestUSize) -> MutVoidPtr {
        let ptr = self.alloc(size);
        self.bytes_at_mut(ptr.cast(), size).fill(0);
        ptr
    }

    /// Implements Apple's documented `malloc_size(3)` contract: returns the
    /// size of the memory block that backs the allocation pointed to by
    /// `ptr`, or `0` if `ptr` is `NULL` or doesn't belong to any block
    /// allocated through malloc. This is deliberately a *silent* lookup —
    /// it's perfectly normal for apps to call `malloc_size` on arbitrary
    /// pointers (interior pointers, `__DATA` symbols, stack addresses,
    /// etc.) and treat a `0` result as "this isn't a heap allocation",
    /// so we must not flood the log when it happens. See
    /// <https://developer.apple.com/library/archive/documentation/Performance/Conceptual/ManagingMemory/Articles/MallocDebug.html>.
    pub fn malloc_size(&self, ptr: ConstVoidPtr) -> GuestUSize {
        if ptr.is_null() {
            return 0;
        }
        self.allocator
            .try_find_allocated_size(ptr.to_bits())
            .unwrap_or(0)
    }

    /// Returns whether `addr` is the exact base of a live allocation. Used to
    /// defensively reject garbage pointers at the libc free() wrapper.
    pub fn is_known_allocation(&self, addr: VAddr) -> bool {
        self.allocator.is_known_allocation(addr)
    }

    /// Returns a snapshot of all currently-live heap allocations as
    /// `(base_address, size_in_bytes)` pairs. The game trainer uses this to
    /// inspect guest heap memory.
    pub fn live_allocations(&self) -> Vec<(GuestUSize, GuestUSize)> {
        self.allocator.live_allocations()
    }

    pub fn realloc(&mut self, old_ptr: MutVoidPtr, size: GuestUSize) -> MutVoidPtr {
        if old_ptr.is_null() {
            return self.alloc(size);
        }

        let old_size = self.allocator.find_allocated_size(old_ptr.to_bits());
        if old_size >= size {
            return old_ptr;
        }

        // Fast path: if the memory right after the allocation happens to be
        // free, grow the allocation in place instead of allocating a new
        // block, copying everything and freeing the old one. Apps that grow
        // buffers repeatedly (arrays, string builders, asset loading) hit
        // this path a lot.
        if let Some(grown_size) = self
            .allocator
            .grow_in_place(old_ptr.to_bits(), old_size, size)
        {
            // Mirror `alloc`: memory is only pre-zeroed when the allocator
            // is configured to hand out zeroed memory; zero just the tail.
            if self.zero_memory_on_free {
                self.bytes_at_mut(old_ptr.cast(), grown_size)[old_size as usize..].fill(0);
            }
            log_dbg!(
                "Reallocated in place {:?} ({:#x} -> {:#x} bytes)",
                old_ptr,
                old_size,
                grown_size
            );
            return old_ptr;
        }

        let new_ptr = self.alloc(size);
        self.memmove(new_ptr, old_ptr.cast_const(), old_size);
        self.free(old_ptr);
        new_ptr
    }

    /// Free an allocation made with one of the `alloc` methods on this type.
    pub fn free(&mut self, ptr: MutVoidPtr) {
        if ptr.is_null() {
            return;
        }
        let addr = ptr.to_bits();
        // Silently ignore attempts to free the MACH_TASK_SELF constant
        // (0x7461736b = "task"). The Mono runtime stores this value and
        // attempts to free it during shutdown; it's not a real allocation.
        if addr == 0x7461736b {
            return;
        }
        // Reject obviously bogus pointers before passing to the allocator.
        if !self.allocator.is_known_allocation(addr) {
            log!("Can't free {:#x}, unknown allocation!", addr);
            return;
        }
        let size = self.allocator.free(addr);
        if self.zero_memory_on_free {
            self.bytes_at_mut(ptr.cast(), size).fill(0);
        }

        log_dbg!("Freed {:?} ({:#x} bytes)", ptr, size);
    }

    /// Allocate memory large enough for a value of type `T` and write the value
    /// to it.
    /// Equivalent to [Self::alloc] + [Self::write].
    pub fn alloc_and_write<T>(&mut self, value: T) -> MutPtr<T>
    where
        T: SafeWrite,
    {
        let ptr = self.alloc(guest_size_of::<T>()).cast();
        self.write(ptr, value);
        ptr
    }

    /// Allocate and write a C string.
    /// This method will add a null terminator,
    /// so it is optimal if the input slice does not already contain one.
    pub fn alloc_and_write_cstr(&mut self, str_bytes: &[u8]) -> MutPtr<u8> {
        let len = str_bytes.len().try_into().unwrap();
        let ptr = self.alloc(len + 1).cast();
        self.bytes_at_mut(ptr, len).copy_from_slice(str_bytes);
        self.write(ptr + len, b'\0');
        ptr
    }

    /// Get a C string (null-terminated) as a slice.
    /// The null terminator is not
    /// included in the slice.
    ///
    /// Safety: includes a maximum length guard (64KB) to prevent infinite loops
    /// if the guest provides a pointer to non-terminated data.
    pub fn cstr_at<const MUT: bool>(&self, ptr: Ptr<u8, MUT>) -> &[u8] {
        const MAX_CSTR_LEN: u32 = 65536; // 64KB safety limit
        self.cstr_at_with_max_len(ptr, MAX_CSTR_LEN)
    }

    /// Like [Self::cstr_at], but with a caller-chosen maximum length instead of
    /// the default 64KB safety limit. Useful for data that can legitimately be
    /// larger than 64KB (e.g. GLSL shader source uploaded via `glShaderSource`
    /// without an explicit length), where the default cap would silently
    /// truncate the string and corrupt it.
    pub fn cstr_at_with_max_len<const MUT: bool>(&self, ptr: Ptr<u8, MUT>, max_len: u32) -> &[u8] {
        let mut len: u32 = 0;
        while self.read(ptr + len) != b'\0' {
            len += 1;
            if len >= max_len {
                log!(
                    "Warning: cstr_at({:?}): hit {}B safety limit without finding null terminator; truncating.",
                    ptr, max_len
                );
                break;
            }
        }
        self.bytes_at(ptr, len)
    }

    /// Get a C string (null-terminated) as a string slice, if it is valid
    /// UTF-8, otherwise returning a byte slice.
    /// The null terminator is not
    /// included in the slice.
    pub fn cstr_at_utf8<const MUT: bool>(&self, ptr: Ptr<u8, MUT>) -> Result<&str, &[u8]> {
        let bytes = self.cstr_at(ptr);
        std::str::from_utf8(bytes).map_err(|_| bytes)
    }

    /// Get a C string (null-terminated) as a string, replacing invalid UTF-8
    /// sequences with U+FFFD instead of failing.
    ///
    /// POSIX paths and most guest C strings are byte strings, so callers that
    /// must accept arbitrary bytes (e.g. `open(2)`, which the kernel never
    /// validates as UTF-8) use this instead of [Self::cstr_at_utf8].
    pub fn cstr_at_utf8_lossy<const MUT: bool>(&self, ptr: Ptr<u8, MUT>) -> std::borrow::Cow<'_, str> {
        match self.cstr_at_utf8(ptr) {
            Ok(s) => std::borrow::Cow::Borrowed(s),
            Err(bytes) => String::from_utf8_lossy(bytes),
        }
    }
    /// Compare a null-terminated guest C string against an expected byte
    /// string, returning true iff they are exactly equal. `expected` must not
    /// contain an interior NUL.
    ///
    /// Unlike [Self::cstr_at_utf8], this allocates nothing, performs no UTF-8
    /// validation, and early-exits at the first mismatching byte. It is meant
    /// for very hot paths (e.g. the per-message-send selector probes) where
    /// the usual outcome is a mismatch on the first byte, so scanning the
    /// whole string just to reject it is pure overhead.
    pub fn cstr_eq<const MUT: bool>(&self, ptr: Ptr<u8, MUT>, expected: &[u8]) -> bool {
        for (i, &b) in expected.iter().enumerate() {
            if self.read(ptr + i as u32) != b {
                return false;
            }
        }
        // Equal so far: it's a true match only if the string ends exactly here.
        self.read(ptr + expected.len() as u32) == b'\0'
    }

    pub fn wcstr_at<const MUT: bool>(&self, ptr: Ptr<wchar_t, MUT>) -> String {
        const MAX_WCSTR_LEN: u32 = 16384; // 16K chars safety limit
        let mut len: u32 = 0;
        while self.read(ptr + len) != wchar_t::default() {
            len += 1;
            if len >= MAX_WCSTR_LEN {
                log!(
                    "Warning: wcstr_at({:?}): hit {} char safety limit without finding null terminator; truncating.",
                    ptr, MAX_WCSTR_LEN
                );
                break;
            }
        }

        // iOS/macOS uses 4-byte wchar_t (UTF-32LE). char::from_u32 returns
        // None for surrogate values (U+D800..U+DFFF) and codepoints above
        // U+10FFFF; in those cases we substitute U+FFFD REPLACEMENT CHARACTER
        // instead of panicking so that bogus data from the guest does not
        // crash the host.
        let bytes = self.bytes_at(ptr.cast(), len * guest_size_of::<wchar_t>());
        let iter = bytes.chunks_exact(4).map(|chunk| {
            // chunks_exact(4) guarantees the length, so try_into never fails.
            let code = u32::from_le_bytes(chunk.try_into().unwrap());
            char::from_u32(code).unwrap_or('\u{FFFD}')
        });
        String::from_iter(iter)
    }

    /// Returns whether the `size` bytes at `base` are neither allocated nor
    /// reserved, so [Self::reserve] can claim them.
    pub fn is_range_free(&self, base: VAddr, size: GuestUSize) -> bool {
        size != 0 && self.allocator.is_free(allocator::Chunk::new(base, size))
    }

    /// Permanently mark a region of address space as being unusable to the
    /// memory allocator.
    ///
    /// A zero-byte reservation is a documented no-op: it matches what xnu's
    /// `mach_loader.c` does when handed a `LC_SEGMENT` whose `vmsize == 0`
    /// (the kernel reserves no address space, the segment is silently
    /// ignored). We mirror that here so the allocator's `Chunk` invariant —
    /// every chunk must contain at least one byte — is preserved even when
    /// callers (Mach-O loader, `dyld::do_initial_linking`, etc.) hand us a
    /// degenerate request.
    pub fn reserve(&mut self, base: VAddr, size: GuestUSize) {
        if size == 0 {
            log_dbg!(
                "Mem::reserve({:#x}, 0) — no-op (matches xnu mach_loader.c)",
                base
            );
            return;
        }
        self.allocator.reserve(allocator::Chunk::new(base, size));
    }
}

#[cfg(test)]
mod mem_tests {
    use super::{Mem, MutPtr, Ptr};

    #[test]
    fn lazy_commit_far_addresses() {
        let mut mem = Mem::new();

        mem.set_null_segment_size(super::PAGE_SIZE);

        let probes: [u32; 6] = [
            0x0000_1000,
            0x1000_0000,
            0x4000_0000,
            0x8000_0000,
            0xC000_0000,
            0xFFFE_F000,
        ];
        for &addr in &probes {
            let p: MutPtr<u8> = Ptr::from_bits(addr);
            mem.write(p, 0xAB);
            assert_eq!(mem.read(p.cast_const()), 0xAB);
        }
    }

    #[test]
    fn wild_host_pointer_becomes_null_instead_of_panicking() {
        let mut mem = Mem::new();

        // A host pointer into the guest memory array converts back to the
        // matching guest address.
        let inside = unsafe { mem.direct_memory_access_ptr() }.cast::<u8>();
        let guest = mem.host_ptr_to_guest_ptr(inside as *const std::ffi::c_void);
        assert_eq!(guest.to_bits(), 0);

        // A plain stack variable is not guest memory. A guest that hands the
        // host GL a pointer like this (a broken vertex array setup, or a
        // pointer that only ever existed in host memory) used to make this
        // conversion panic; it must now report NULL so the GL call is skipped
        // instead of dereferencing garbage.
        let local: u64 = 0;
        let wild = &local as *const u64 as *const std::ffi::c_void;
        assert!(!mem.is_host_ptr_in_guest_mem(wild));
        assert_eq!(mem.host_ptr_to_guest_ptr(wild).to_bits(), 0);
    }

    #[test]
    fn null_page_writes_round_trip() {
        let mut mem = Mem::new();

        mem.set_null_segment_size(super::PAGE_SIZE);

        // Asphalt 8's drift event writes a 256-byte table through a NULL base
        // pointer (64 four-byte stores at 0x0..0xfc). Reads and writes must
        // hit the same backing page, so reading an address back returns what
        // was stored instead of a zero from a separate stub page.
        for i in 0..64u32 {
            let p: MutPtr<u32> = Ptr::from_bits(i * 4);
            mem.write(p, 0x1000 + i);
        }
        for i in 0..64u32 {
            let p: MutPtr<u32> = Ptr::from_bits(i * 4);
            assert_eq!(mem.read(p.cast_const()), 0x1000 + i);
        }

        // Offsets that were never written still read as zero, so
        // `*(void**)NULL` remains NULL.
        let untouched: MutPtr<u32> = Ptr::from_bits(0x800);
        assert_eq!(mem.read(untouched.cast_const()), 0);
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn fastmem_null_guard_preserves_guest_memory_access() {
        let mut mem = Mem::new();
        mem.set_null_segment_size(super::PAGE_SIZE);
        let host_page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        let fastmem_enabled = mem.enable_fastmem();
        assert_eq!(
            fastmem_enabled,
            host_page_size > 0
                && (super::PAGE_SIZE as usize).is_multiple_of(host_page_size as usize)
        );

        let null_ptr: MutPtr<u32> = Ptr::from_bits(0);
        mem.write(null_ptr, 0x1234_5678);
        assert_eq!(mem.read(null_ptr.cast_const()), 0x1234_5678);

        let normal_ptr: MutPtr<u32> = Ptr::from_bits(super::PAGE_SIZE);
        mem.write(normal_ptr, 0x8765_4321);
        assert_eq!(mem.read(normal_ptr.cast_const()), 0x8765_4321);
    }

    #[test]
    fn memmove_handles_overlapping_and_null_page_ranges() {
        use super::ConstVoidPtr;

        let mut mem = Mem::new();
        mem.set_null_segment_size(super::PAGE_SIZE);
        let regular_src: MutPtr<u8> = Ptr::from_bits(super::PAGE_SIZE);
        mem.bytes_at_mut(regular_src, 6)
            .copy_from_slice(&[1, 2, 3, 4, 5, 6]);
        mem.memmove(
            Ptr::<u8, true>::from_bits(super::PAGE_SIZE + 1).cast(),
            regular_src.cast_const().cast(),
            5,
        );
        assert_eq!(
            mem.bytes_at(Ptr::<u8, false>::from_bits(super::PAGE_SIZE + 1), 5),
            &[1, 2, 3, 4, 5]
        );

        mem.bytes_at_mut(Ptr::<u8, true>::from_bits(1), 4)
            .copy_from_slice(&[7, 8, 9, 10]);
        mem.memmove(
            Ptr::<u8, true>::from_bits(super::PAGE_SIZE + 8).cast(),
            ConstVoidPtr::from_bits(1),
            4,
        );
        assert_eq!(
            mem.bytes_at(Ptr::<u8, false>::from_bits(super::PAGE_SIZE + 8), 4),
            &[7, 8, 9, 10]
        );

        mem.bytes_at_mut(Ptr::<u8, true>::from_bits(super::PAGE_SIZE - 1), 1)
            .copy_from_slice(&[11]);
        mem.bytes_at_mut(Ptr::<u8, true>::from_bits(super::PAGE_SIZE), 1)
            .copy_from_slice(&[12]);
        mem.memmove(
            Ptr::<u8, true>::from_bits(super::PAGE_SIZE + 0x20).cast(),
            ConstVoidPtr::from_bits(super::PAGE_SIZE - 1),
            2,
        );
        assert_eq!(
            mem.bytes_at(Ptr::<u8, false>::from_bits(super::PAGE_SIZE + 0x20), 2),
            &[11, 12]
        );

        let valid_src: MutPtr<u8> = Ptr::from_bits(super::PAGE_SIZE + 0x30);
        mem.bytes_at_mut(valid_src, 3)
            .copy_from_slice(&[13, 14, 15]);
        mem.memmove(
            Ptr::<u8, true>::from_bits(8).cast(),
            valid_src.cast_const().cast(),
            3,
        );
        assert_eq!(
            mem.bytes_at(Ptr::<u8, false>::from_bits(8), 3),
            &[13, 14, 15]
        );
    }

    #[test]
    fn cstr_at_utf8_lossy_keeps_non_utf8_paths_usable() {
        // Unity's shader cache opens paths whose name ends in uninitialised
        // bytes. POSIX paths are byte strings, so the helper must return a
        // string (with replacement characters) rather than failing.
        let mut mem = Mem::new();
        let p: MutPtr<u8> = Ptr::from_bits(0x1000);
        mem.bytes_at_mut(p, 6)
            .copy_from_slice(&[b'/', b'a', 0xA5, b'b', 0xAA, 0]);

        assert!(mem.cstr_at_utf8(p).is_err());
        assert_eq!(
            mem.cstr_at_utf8_lossy(p).as_ref(),
            "/a\u{FFFD}b\u{FFFD}"
        );
        // '/', 'a', U+FFFD, 'b', U+FFFD: each bad byte becomes one
        // replacement character, and the name stays stable across calls so
        // repeated opens of one cache entry still hit the same file.
        assert_eq!(mem.cstr_at_utf8_lossy(p).chars().count(), 5);
        assert_eq!(mem.cstr_at_utf8_lossy(p), mem.cstr_at_utf8_lossy(p));
    }

    #[test]
    fn ptr_arithmetic_wraps_modulo_2_32() {
        // Real 32-bit ARM computes addresses modulo 2^32 and never traps on
        // the arithmetic itself. These cases previously panicked the host via
        // `checked_*().unwrap()`; they must now wrap like the hardware.
        let near_top: Ptr<u8, true> = Ptr::from_bits(0xFFFF_FFFB);
        assert_eq!((near_top + 0x10).to_bits(), 0x0000_000B);

        let low: Ptr<u8, true> = Ptr::from_bits(0x0000_0004);
        assert_eq!((low - 0x10).to_bits(), 0xFFFF_FFF4);

        // Element-sized arithmetic (u32 = 4 bytes) must also wrap rather than
        // overflow when the multiplied offset exceeds the address space.
        let p: Ptr<u32, true> = Ptr::from_bits(0xFFFF_FFF0);
        assert_eq!((p + 0x8).to_bits(), 0x0000_0010);
    }

    #[test]
    fn null_stub_never_truncates_or_overruns_across_page_boundary() {
        let mut mem = Mem::new();
        mem.set_null_segment_size(super::PAGE_SIZE);

        let straddle: MutPtr<u64> = Ptr::from_bits(super::PAGE_SIZE - 4);
        mem.write(straddle, 0x0123_4567_89ab_cdef);
        assert_eq!(mem.read(straddle), 0x0123_4567_89ab_cdef);

        let oversize_len = (super::NULL_STUB_SIZE + 4096) as u32;
        assert_eq!(
            mem.bytes_at(Ptr::<u8, false>::from_bits(0), oversize_len)
                .len(),
            oversize_len as usize
        );
        assert_eq!(
            mem.bytes_at_mut(Ptr::<u8, true>::from_bits(0), oversize_len)
                .len(),
            oversize_len as usize
        );
    }
}
