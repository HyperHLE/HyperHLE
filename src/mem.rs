/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Types related to the virtual memory of the emulated application, or the
//! "guest memory".

use crate::libc::wchar::wchar_t;

mod allocator;
mod host;

pub type GuestUSize = u32;
pub type GuestISize = i32;

pub const fn guest_size_of<T: Sized>() -> GuestUSize {
    assert!(std::mem::size_of::<T>() <= u32::MAX as usize);
    std::mem::size_of::<T>() as u32
}

type VAddr = GuestUSize;

#[repr(transparent)]
pub struct Ptr<T, const MUT: bool>(VAddr, std::marker::PhantomData<T>);

impl<T, const MUT: bool> Clone for Ptr<T, MUT> {
    fn clone(&self) -> Self { *self }
}
impl<T, const MUT: bool> Copy for Ptr<T, MUT> {}
impl<T, const MUT: bool> PartialEq for Ptr<T, MUT> {
    fn eq(&self, other: &Self) -> bool { self.0 == other.0 }
}
impl<T, const MUT: bool> Eq for Ptr<T, MUT> {}
impl<T, const MUT: bool> std::hash::Hash for Ptr<T, MUT> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) { self.0.hash(state); }
}

pub type ConstPtr<T> = Ptr<T, false>;
pub type MutPtr<T> = Ptr<T, true>;
pub type ConstVoidPtr = ConstPtr<std::ffi::c_void>;
pub type MutVoidPtr = MutPtr<std::ffi::c_void>;

impl<T, const MUT: bool> Ptr<T, MUT> {
    pub const fn null() -> Self { Ptr(0, std::marker::PhantomData) }
    pub fn to_bits(self) -> VAddr { self.0 }
    pub fn as_vaddr(self) -> VAddr { self.0 }
    pub const fn from_bits(bits: VAddr) -> Self { Ptr(bits, std::marker::PhantomData) }
    pub const fn from_vaddr(bits: VAddr) -> Self { Ptr(bits, std::marker::PhantomData) }
    pub fn cast<U>(self) -> Ptr<U, MUT> { Ptr::<U, MUT>::from_bits(self.to_bits()) }
    pub fn is_null(self) -> bool { self.to_bits() == 0 }
}

impl<T, const MUT: bool> std::ops::Add<GuestUSize> for Ptr<T, MUT> {
    type Output = Self;
    fn add(self, other: GuestUSize) -> Self {
        let size = guest_size_of::<T>();
        Self::from_bits(self.to_bits().wrapping_add(other * size))
    }
}

pub unsafe trait SafeRead: Sized {}
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
pub trait SafeWrite: Sized {}
impl<T: SafeRead> SafeWrite for T {}

type Bytes = [u8; 1 << 32];
pub const PAGE_SIZE: GuestUSize = 4096;
pub const PAGE_SIZE_ALIGN_MASK: GuestUSize = 0xfff;

pub struct Mem {
    bytes: *mut Bytes,
    null_segment_size: VAddr,
    allocator: allocator::Allocator,
    pub(super) zero_memory_on_free: bool,
    /// SAFETY SHIELD: A dummy page to handle null-page reads/writes/calls
    null_stub_page: *mut u8,
}

impl Drop for Mem {
    fn drop(&mut self) {
        unsafe {
            let _ = crate::mem::host::free_memory(self.bytes.cast(), 1 << 32);
            if !self.null_stub_page.is_null() {
                let _ = crate::mem::host::free_memory(self.null_stub_page.cast(), PAGE_SIZE as usize);
            }
        }
    }
}

impl Mem {
    pub const MAIN_THREAD_STACK_SIZE: GuestUSize = 1024 * 1024;
    pub const MAIN_THREAD_STACK_LOW_END: VAddr = 0u32.wrapping_sub(Self::MAIN_THREAD_STACK_SIZE);

    pub fn new() -> Mem {
        let ptr = unsafe { crate::mem::host::allocate_memory(1 << 32).unwrap() };
        let bytes = ptr as *mut Bytes;

        // --- GLOBAL SHIELD INITIALIZATION ---
        let null_stub_page = unsafe {
            let page = crate::mem::host::allocate_memory(PAGE_SIZE as usize).unwrap();
            let slice = std::slice::from_raw_parts_mut(page as *mut u8, PAGE_SIZE as usize);
            slice.fill(0); // All reads will return 0
            
            // Put a "Return" instruction (BX LR) at the end of the stub page
            // This prevents crashes if the game tries to CALL a null pointer.
            slice[PAGE_SIZE as usize - 2] = 0x70;
            slice[PAGE_SIZE as usize - 1] = 0x47;
            page as *mut u8
        };

        Mem {
            bytes,
            null_segment_size: 0,
            allocator: allocator::Allocator::new(),
            zero_memory_on_free: true,
            null_stub_page,
        }
    }
    pub fn set_null_segment_size(&mut self, size: VAddr) {
        self.null_segment_size = size;
        self.allocator.reserve(allocator::Chunk::new(0, size));
    }

    #[cold]
    fn handle_null_access(&self, addr: VAddr, size: GuestUSize, is_write: bool) {
        log!("--- NULL-PAGE INTERCEPTED --- addr: {:#x}, size: {}, write: {}", addr, size, is_write);
    }

    pub fn bytes_at<const MUT: bool>(&self, ptr: Ptr<u8, MUT>, count: GuestUSize) -> &[u8] {
        if ptr.to_bits() < self.null_segment_size {
            self.handle_null_access(ptr.to_bits(), count, false);
            return unsafe { std::slice::from_raw_parts(self.null_stub_page, count as usize) };
        }
        unsafe { &(*self.bytes)[ptr.to_bits() as usize..][..count as usize] }
    }

    pub fn bytes_at_mut(&mut self, ptr: MutPtr<u8>, count: GuestUSize) -> &mut [u8] {
        if ptr.to_bits() < self.null_segment_size {
            self.handle_null_access(ptr.to_bits(), count, true);
            return unsafe { std::slice::from_raw_parts_mut(self.null_stub_page, count as usize) };
        }
        unsafe { &mut (*self.bytes)[ptr.to_bits() as usize..][..count as usize] }
    }

    pub fn read<T, const MUT: bool>(&self, ptr: Ptr<T, MUT>) -> T where T: SafeRead {
        let size = guest_size_of::<T>();
        let bytes = self.bytes_at(ptr.cast(), size);
        unsafe { bytes.as_ptr().cast::<T>().read_unaligned() }
    }

    pub fn write<T>(&mut self, ptr: MutPtr<T>, value: T) where T: SafeWrite {
        let size = guest_size_of::<T>();
        let bytes = self.bytes_at_mut(ptr.cast(), size);
        unsafe { bytes.as_mut_ptr().cast::<T>().write_unaligned(value) }
    }

    // Standard alloc/free/realloc remain...
    pub fn alloc(&mut self, size: GuestUSize) -> MutVoidPtr {
        Ptr::from_bits(self.allocator.alloc(size))
    }
    pub fn free(&mut self, ptr: MutVoidPtr) {
        let size = self.allocator.free(ptr.to_bits());
        if self.zero_memory_on_free {
            self.write_zeros(ptr, size);
        }
    }
    fn write_zeros(&mut self, ptr: MutVoidPtr, size: GuestUSize) {
        if !ptr.is_null() { self.bytes_at_mut(ptr.cast(), size).fill(0); }
    }
    
    pub fn cstr_at<const MUT: bool>(&self, ptr: Ptr<u8, MUT>) -> &[u8] {
        let mut len = 0;
        while self.read(ptr + len) != 0 { len += 1; }
        self.bytes_at(ptr, len)
    }
}
