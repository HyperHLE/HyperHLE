/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Mach VM functions

use crate::dyld::{export_c_func, FunctionExports};
use crate::libc::mach::init::MACH_TASK_SELF;
use crate::libc::mach::port::mach_port_t;
use crate::libc::mach::thread_info::{
    kern_return_t, KERN_INVALID_ADDRESS, KERN_INVALID_ARGUMENT, KERN_INVALID_TASK, KERN_NO_SPACE,
    KERN_SUCCESS,
};
use crate::mem::{
    ConstPtr, Mem, MutPtr, Ptr, SafeRead, SafeWrite, PAGE_SIZE, PAGE_SIZE_ALIGN_MASK,
};
use crate::Environment;
use std::collections::HashMap;

type vm_map_t = mach_port_t;
type vm_purgable_t = i32;
type mach_vm_address_t = u32;
type mach_vm_size_t = u32;
type vm_prot_t = i32;
type vm_inherit_t = u32;

const VM_PROT_READ: vm_prot_t = 1;
const VM_PROT_WRITE: vm_prot_t = 2;
const VM_PROT_EXECUTE: vm_prot_t = 4;

#[derive(Default)]
pub struct State {
    /// Keeping track of `vm_allocate` allocations: base address of the region
    /// -> its bookkeeping.
    allocations: HashMap<mach_vm_address_t, VmRegion>,
}

/// Round a byte count up to a whole number of pages, as Mach does for every
/// region size it hands out or accepts.
fn round_up_to_page(size: mach_vm_size_t) -> mach_vm_size_t {
    if !size.is_multiple_of(PAGE_SIZE) {
        size + PAGE_SIZE - (size % PAGE_SIZE)
    } else {
        size
    }
}

/// Bookkeeping for a single region handed out by `vm_allocate`/`vm_remap`.
#[derive(Default)]
struct VmRegion {
    /// Size the guest originally requested.
    size: mach_vm_size_t,
    /// Page-rounded size actually backed by a heap allocation.
    rounded_size: mach_vm_size_t,
    /// Sorted, disjoint `[start, end)` ranges within this region that the guest
    /// has already deallocated.
    ///
    /// Real Mach lets a caller deallocate *any* sub-range of a mapping — apps
    /// legitimately split one `vm_allocate` region into several buffers and
    /// release them one at a time — but the host heap allocation underneath can
    /// only be returned once the whole region has been released. Until then the
    /// sub-range deallocations are recorded here (and reported as successful, as
    /// the kernel would) instead of being rejected.
    released: Vec<(mach_vm_address_t, mach_vm_address_t)>,
}

impl VmRegion {
    /// Record that `[start, end)` has been deallocated and report whether that
    /// completes the region.
    fn release(
        &mut self,
        base: mach_vm_address_t,
        start: mach_vm_address_t,
        end: mach_vm_address_t,
    ) -> bool {
        let mut ranges = std::mem::take(&mut self.released);
        ranges.push((start, end));
        ranges.sort_unstable();
        let mut coalesced: Vec<(mach_vm_address_t, mach_vm_address_t)> =
            Vec::with_capacity(ranges.len());
        for (s, e) in ranges {
            // Sorted order means the new range can only extend the last one.
            let touches_previous = match coalesced.last() {
                Some(&(_, last_end)) => s <= last_end,
                None => false,
            };
            if touches_previous {
                let last = coalesced.last_mut().unwrap();
                last.1 = last.1.max(e);
            } else {
                coalesced.push((s, e));
            }
        }
        self.released = coalesced;

        let region_end = base.saturating_add(self.rounded_size);
        self.released.len() == 1
            && self.released[0].0 <= base
            && self.released[0].1 >= region_end
    }
}

pub fn vm_allocate(
    env: &mut Environment,
    target_task: vm_map_t,
    address_ptr: MutPtr<mach_vm_address_t>,
    size: mach_vm_size_t,
    flags: i32, // in other docs it is defined as `anywhere: boolean_t`
) -> kern_return_t {
    assert_eq!(target_task, MACH_TASK_SELF);
    assert_eq!(flags, 1); // TRUE

    // `size is always rounded up to an integral number of pages`
    let new_size = round_up_to_page(size);
    // touchHLE delegates page-granularity Mach VM allocations to the
    // standard guest heap allocator. This is fine in practice — apps
    // call vm_allocate to obtain page-aligned scratch buffers, which
    // env.mem.alloc does honour — but it does mean we can't enforce
    // protection bits or sparse mappings. Demoted to debug because Mono
    // / Boehm GC use vm_allocate as their primary heap source and the
    // log_once line was the very first noisy entry in long sessions.
    log_dbg!("vm_allocate() implemented atop standard allocator");
    let allocated = env.mem.alloc(new_size);
    let address = allocated.to_bits();
    if address == 0 {
        // Out of guest memory (including the RAM limiter's cap). Reporting
        // success with address 0 would hand the guest a null "mapping" it
        // believes it owns, so fail like the kernel does instead.
        crate::mem::log_out_of_memory("vm_allocate", new_size);
        return KERN_NO_SPACE;
    }
    assert!(address & PAGE_SIZE_ALIGN_MASK == 0);
    crate::mem::note_allocation_site(env.cpu.regs()[crate::cpu::Cpu::LR], new_size);
    env.mem.write(address_ptr, address);

    assert!(!env.libc_state.mach_vm.allocations.contains_key(&address));
    // Note: we keep track of the original size as well as the page-rounded one,
    // because guests pass either back to vm_deallocate.
    env.libc_state.mach_vm.allocations.insert(
        address,
        VmRegion { size, rounded_size: new_size, released: Vec::new() },
    );

    KERN_SUCCESS
}

fn vm_deallocate(
    env: &mut Environment,
    target_task: vm_map_t,
    address: mach_vm_address_t,
    size: mach_vm_size_t,
) -> kern_return_t {
    assert_eq!(target_task, MACH_TASK_SELF);
    log_dbg!("vm_deallocate() implemented atop standard allocator");

    if size == 0 {
        return KERN_SUCCESS;
    }
    let Some(end) = address.checked_add(size) else {
        log!(
            "Warning: vm_deallocate({:#x}, {:#x}) overflows the address space; returning KERN_INVALID_ADDRESS.",
            address,
            size
        );
        return KERN_INVALID_ADDRESS;
    };

    // Find the region this range belongs to. Mach accepts the deallocation of
    // any sub-range of a mapping, so an exact base-address match is only the
    // most common case: apps that carve one vm_allocate region into several
    // buffers release each buffer separately, and all of those calls succeed on
    // a real kernel.
    let base = if env.libc_state.mach_vm.allocations.contains_key(&address) {
        Some(address)
    } else {
        env.libc_state
            .mach_vm
            .allocations
            .iter()
            .find(|(base, region)| {
                let region_end = base.saturating_add(region.rounded_size);
                **base <= address && end <= region_end
            })
            .map(|(&base, _)| base)
    };

    // The guest may ask us to free a range that was never handed out via
    // vm_allocate (a double free, a region obtained by other means, or a bogus
    // pointer). A real Mach kernel returns KERN_INVALID_ADDRESS in that case
    // rather than aborting the task, so mirror that instead of panicking.
    let Some(base) = base else {
        log!(
            "Warning: vm_deallocate({:#x}, {:#x}) for an address that was not allocated via vm_allocate; returning KERN_INVALID_ADDRESS.",
            address,
            size
        );
        return KERN_INVALID_ADDRESS;
    };

    let region = env.libc_state.mach_vm.allocations.get_mut(&base).unwrap();
    let requested_size = region.size;
    let region_end = base.saturating_add(region.rounded_size);
    // We record the original requested size in `vm_allocate`, but the guest is
    // free to pass either that or the page-rounded size it was effectively
    // given. Anything wider than the region itself is worth a note, since those
    // extra bytes belong to whatever is mapped next; the region is released
    // either way.
    if base == address && end > region_end {
        log!(
            "Warning: vm_deallocate({:#x}, {:#x}) size mismatch (region was allocated with size {:#x}); freeing the whole region anyway.",
            address,
            size,
            requested_size
        );
    }
    let complete = region.release(base, address.max(base), end.min(region_end));
    log_dbg!(
        "vm_deallocate({:#x}, {:#x}) released part of region {:#x} (size {:#x}); complete: {}",
        address,
        size,
        base,
        region.rounded_size,
        complete
    );

    if complete {
        env.mem.free(Ptr::from_bits(base));
        env.libc_state.mach_vm.allocations.remove(&base);
    }

    KERN_SUCCESS
}

/// Whether a non-empty guest range lies outside the reserved null segment and
/// entirely within the 32-bit guest address space.
fn guest_range_is_valid(mem: &Mem, address: mach_vm_address_t, size: mach_vm_size_t) -> bool {
    if size == 0 {
        return true;
    }

    let start = u64::from(address);
    let end = start + u64::from(size);
    start != 0 && start >= u64::from(mem.null_segment_size()) && end <= (1_u64 << 32)
}

/// Implementation shared with unit tests, which can exercise guest-memory
/// behavior without constructing a full emulator environment.
fn vm_read_overwrite_in_memory(
    mem: &mut Mem,
    target_task: vm_map_t,
    address: mach_vm_address_t,
    size: mach_vm_size_t,
    data: mach_vm_address_t,
    outsize: MutPtr<mach_vm_size_t>,
) -> kern_return_t {
    if target_task != MACH_TASK_SELF {
        log!(
            "Warning: vm_read_overwrite: unsupported task port {:#x}; returning KERN_INVALID_TASK.",
            target_task
        );
        return KERN_INVALID_TASK;
    }
    if outsize.is_null() {
        log!("Warning: vm_read_overwrite: null outsize pointer; returning KERN_INVALID_ARGUMENT.");
        return KERN_INVALID_ARGUMENT;
    }
    let outsize_size = std::mem::size_of::<mach_vm_size_t>() as u32;
    if !guest_range_is_valid(mem, outsize.to_bits(), outsize_size) {
        log!(
            "Warning: vm_read_overwrite: invalid outsize pointer {:?}; returning KERN_INVALID_ADDRESS.",
            outsize
        );
        return KERN_INVALID_ADDRESS;
    }

    // Mem::memmove intentionally refuses sizes that look like a negative
    // 32-bit value; keep this Mach entry point from reporting success for a
    // copy that the memory layer would silently skip.
    if size >= 0x8000_0000 {
        log!(
            "Warning: vm_read_overwrite: unreasonable copy size {:#x}; returning KERN_INVALID_ARGUMENT.",
            size
        );
        return KERN_INVALID_ARGUMENT;
    }
    if !guest_range_is_valid(mem, address, size) || !guest_range_is_valid(mem, data, size) {
        log!(
            "Warning: vm_read_overwrite: invalid source/destination range (src={:#x}, dst={:#x}, size={:#x}); returning KERN_INVALID_ADDRESS.",
            address,
            data,
            size
        );
        return KERN_INVALID_ADDRESS;
    }

    // A process has one flat guest address space here, and vm_read_overwrite
    // reads from and writes to that same space. Use memmove semantics so
    // overlapping source/destination ranges behave correctly.
    let source: ConstPtr<u8> = Ptr::from_bits(address);
    let destination: MutPtr<u8> = Ptr::from_bits(data);
    mem.memmove(destination.cast_void(), source.cast_void(), size);
    mem.write(outsize, size);
    KERN_SUCCESS
}

/// `kern_return_t vm_read_overwrite(vm_map_t target_task, vm_address_t address,
/// vm_size_t size, vm_address_t data, vm_size_t *outsize)`
///
/// Copy bytes from the specified task into the caller-provided buffer without
/// allocating a new VM region. touchHLE has one guest address space, so only
/// `MACH_TASK_SELF` is supported.
fn vm_read_overwrite(
    env: &mut Environment,
    target_task: vm_map_t,
    address: mach_vm_address_t,
    size: mach_vm_size_t,
    data: mach_vm_address_t,
    outsize: MutPtr<mach_vm_size_t>,
) -> kern_return_t {
    vm_read_overwrite_in_memory(&mut env.mem, target_task, address, size, data, outsize)
}

/// `kern_return_t vm_remap(vm_map_t target_task, vm_address_t *target_address,
/// vm_size_t size, vm_address_t mask, int flags, vm_map_t src_task,
/// vm_address_t src_address, boolean_t copy, vm_prot_t *cur_protection,
/// vm_prot_t *max_protection, vm_inherit_t inheritance)`
///
/// Real Mach `vm_remap` maps the physical pages backing a region of one task's
/// address space into another (or the same) task, optionally sharing them.
/// touchHLE has a single flat guest address space and no notion of shared
/// physical pages, so we approximate it: allocate a fresh page-aligned region
/// and copy the source bytes into it. This is what callers like the Mono/Boehm
/// GC actually need to keep running — a valid, readable mapping of the same
/// contents — even though writes won't be reflected back into the source.
#[allow(clippy::too_many_arguments)]
fn vm_remap(
    env: &mut Environment,
    target_task: vm_map_t,
    target_address: MutPtr<mach_vm_address_t>,
    size: mach_vm_size_t,
    _mask: mach_vm_address_t,
    _flags: i32,
    _src_task: vm_map_t,
    src_address: mach_vm_address_t,
    copy: i32,
    cur_protection: MutPtr<vm_prot_t>,
    max_protection: MutPtr<vm_prot_t>,
    _inheritance: vm_inherit_t,
) -> kern_return_t {
    assert_eq!(target_task, MACH_TASK_SELF);

    if size == 0 || src_address == 0 {
        log!(
            "Warning: vm_remap(src={:#x}, size={:#x}): invalid argument; returning KERN_INVALID_ADDRESS.",
            src_address,
            size
        );
        return KERN_INVALID_ADDRESS;
    }

    // `size` is always rounded up to an integral number of pages.
    let new_size = round_up_to_page(size);

    log_dbg!(
        "vm_remap(src={:#x}, size={:#x}, copy={}) approximated by allocate + copy",
        src_address,
        size,
        copy
    );

    let allocated = env.mem.alloc(new_size);
    let address = allocated.to_bits();
    if address == 0 {
        // Out of guest memory (including the RAM limiter's cap). Failing here
        // matters: the copy below would write through a null pointer.
        crate::mem::log_out_of_memory("vm_remap", new_size);
        return KERN_NO_SPACE;
    }
    assert!(address & PAGE_SIZE_ALIGN_MASK == 0);

    // Copy the source region's bytes so the new mapping reads back the same
    // data. We copy the page-rounded length so the trailing partial page is
    // also valid.
    let copy_len = new_size.min(u32::MAX);
    let src: ConstPtr<u8> = Ptr::from_bits(src_address);
    let dst: MutPtr<u8> = Ptr::from_bits(address);
    let bytes: Vec<u8> = env.mem.bytes_at(src, copy_len).to_vec();
    env.mem.bytes_at_mut(dst, copy_len).copy_from_slice(&bytes);

    env.mem.write(target_address, address);
    if !cur_protection.is_null() {
        env.mem.write(
            cur_protection,
            VM_PROT_READ | VM_PROT_WRITE | VM_PROT_EXECUTE,
        );
    }
    if !max_protection.is_null() {
        env.mem.write(
            max_protection,
            VM_PROT_READ | VM_PROT_WRITE | VM_PROT_EXECUTE,
        );
    }

    assert!(!env.libc_state.mach_vm.allocations.contains_key(&address));
    env.libc_state.mach_vm.allocations.insert(
        address,
        VmRegion { size, rounded_size: new_size, released: Vec::new() },
    );

    KERN_SUCCESS
}

fn vm_purgable_control(
    _env: &mut Environment,
    target_task: vm_map_t,
    address: mach_vm_address_t,
    control: vm_purgable_t,
    state: MutPtr<vm_purgable_t>,
) -> kern_return_t {
    assert_eq!(target_task, MACH_TASK_SELF);
    log!("TODO: vm_purgable_control({target_task:#x}, {address:#x}, {control:#x}, {state:?})");
    KERN_SUCCESS
}

/// `kern_return_t vm_protect(vm_map_t target_task, vm_address_t address,
/// vm_size_t size, boolean_t set_maximum, vm_prot_t new_protection)`
///
/// Guest memory has no per-page protection enforcement, so this is a no-op
/// that reports success (a real kernel would return KERN_INVALID_ADDRESS for
/// unmapped ranges; callers like Chrome's sandbox setup ignore the result).
fn vm_protect(
    _env: &mut Environment,
    target_task: vm_map_t,
    address: mach_vm_address_t,
    size: mach_vm_size_t,
    set_maximum: i32,
    new_protection: vm_prot_t,
) -> kern_return_t {
    if target_task != MACH_TASK_SELF {
        return KERN_INVALID_ADDRESS;
    }
    log_dbg!(
        "vm_protect({:#x}, {:#x}, set_max={}, prot={:#x}) accepted (no-op)",
        address,
        size,
        set_maximum != 0,
        new_protection
    );
    KERN_SUCCESS
}

#[repr(C)]
#[derive(Clone, Copy)]
struct vm_region_basic_info_data_t {
    protection: vm_prot_t,
    max_protection: vm_prot_t,
    inheritance: vm_inherit_t,
    reserved: u32,
    offset: u32,
}
unsafe impl SafeRead for vm_region_basic_info_data_t {}


/// `kern_return_t vm_region_recurse(vm_map_t target_task,
/// vm_address_t *address, vm_size_t *size, uint32_t *nesting_depth,
/// vm_region_recurse_info_t info, mach_msg_type_number_t *info_count)`
///
/// Reports tracked heap regions. Apps like Chrome use this to probe mappings
/// and tolerate failure, but returning success for tracked regions is closer
/// to a real kernel.
fn vm_region_recurse(
    env: &mut Environment,
    target_task: vm_map_t,
    address_ptr: MutPtr<mach_vm_address_t>,
    size_ptr: MutPtr<mach_vm_size_t>,
    nesting_depth_ptr: MutPtr<u32>,
    info_ptr: MutPtr<u8>,
    info_count_ptr: MutPtr<u32>,
) -> kern_return_t {
    if target_task != MACH_TASK_SELF || address_ptr.is_null() || size_ptr.is_null() {
        return KERN_INVALID_ADDRESS;
    }
    let address = env.mem.read(address_ptr);
    let Some(tracked) = env.libc_state.mach_vm.allocations.get(&address) else {
        log_dbg!(
            "vm_region_recurse({:#x}): untracked region; returning KERN_INVALID_ADDRESS",
            address
        );
        return KERN_INVALID_ADDRESS;
    };
    env.mem.write(size_ptr, tracked.size);
    if !nesting_depth_ptr.is_null() {
        env.mem.write(nesting_depth_ptr, 0);
    }
    if !info_ptr.is_null() && !info_count_ptr.is_null() {
        let info_count = env.mem.read(info_count_ptr);
        if info_count >= 9 {
            // vm_region_basic_info_data_t: five u32 fields in our layout
            let info: MutPtr<vm_region_basic_info_data_t> = Ptr::from_bits(info_ptr.to_bits());
            env.mem.write(
                info,
                vm_region_basic_info_data_t {
                    protection: VM_PROT_READ | VM_PROT_WRITE,
                    max_protection: VM_PROT_READ | VM_PROT_WRITE | VM_PROT_EXECUTE,
                    inheritance: 1, // VM_INHERIT_COPY
                    reserved: 0,
                    offset: 0,
                },
            );
        }
    }
    KERN_SUCCESS
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(vm_allocate(_, _, _, _)),
    export_c_func!(vm_deallocate(_, _, _)),
    export_c_func!(vm_read_overwrite(_, _, _, _, _)),
    export_c_func!(vm_remap(_, _, _, _, _, _, _, _, _, _, _)),
    export_c_func!(vm_purgable_control(_, _, _, _)),
    export_c_func!(vm_protect(_, _, _, _, _)),
    export_c_func!(vm_region_recurse(_, _, _, _, _, _)),
];

#[cfg(test)]
mod vm_read_overwrite_tests {
    use super::*;

    fn new_test_mem() -> Mem {
        let mut mem = Mem::new();
        mem.set_null_segment_size(PAGE_SIZE);
        mem
    }

    #[test]
    fn vm_read_overwrite_copies_bytes_and_reports_the_count() {
        let mut mem = new_test_mem();
        let source: MutPtr<u8> = mem.alloc(4).cast();
        let destination: MutPtr<u8> = mem.alloc(4).cast();
        let outsize: MutPtr<mach_vm_size_t> = mem.alloc(4).cast();
        mem.bytes_at_mut(source, 4).copy_from_slice(&[0x11, 0x22, 0x33, 0x44]);

        assert_eq!(
            vm_read_overwrite_in_memory(
                &mut mem,
                MACH_TASK_SELF,
                source.to_bits(),
                4,
                destination.to_bits(),
                outsize,
            ),
            KERN_SUCCESS
        );
        assert_eq!(mem.bytes_at(destination.cast_const(), 4), &[0x11, 0x22, 0x33, 0x44]);
        assert_eq!(mem.read(outsize), 4);
    }

    #[test]
    fn vm_read_overwrite_supports_overlapping_ranges() {
        let mut mem = new_test_mem();
        let buffer: MutPtr<u8> = mem.alloc(8).cast();
        let outsize: MutPtr<mach_vm_size_t> = mem.alloc(4).cast();
        mem.bytes_at_mut(buffer, 6).copy_from_slice(&[1, 2, 3, 4, 5, 6]);

        assert_eq!(
            vm_read_overwrite_in_memory(
                &mut mem,
                MACH_TASK_SELF,
                buffer.to_bits(),
                4,
                (buffer + 1).to_bits(),
                outsize,
            ),
            KERN_SUCCESS
        );
        assert_eq!(mem.bytes_at(buffer.cast_const(), 6), &[1, 1, 2, 3, 4, 6]);
        assert_eq!(mem.read(outsize), 4);
    }

    #[test]
    fn vm_read_overwrite_rejects_bad_task_and_wrapping_ranges() {
        let mut mem = new_test_mem();
        let source: MutPtr<u8> = mem.alloc(4).cast();
        let destination: MutPtr<u8> = mem.alloc(4).cast();
        let outsize: MutPtr<mach_vm_size_t> = mem.alloc(4).cast();
        mem.bytes_at_mut(source, 4).copy_from_slice(&[1, 2, 3, 4]);
        mem.bytes_at_mut(destination, 4).fill(0xAA);
        mem.write(outsize, 0x1234);

        assert_eq!(
            vm_read_overwrite_in_memory(
                &mut mem,
                MACH_TASK_SELF + 1,
                source.to_bits(),
                4,
                destination.to_bits(),
                outsize,
            ),
            KERN_INVALID_TASK
        );
        assert_eq!(mem.read(outsize), 0x1234);

        assert_eq!(
            vm_read_overwrite_in_memory(
                &mut mem,
                MACH_TASK_SELF,
                0xFFFF_FFFC,
                8,
                destination.to_bits(),
                outsize,
            ),
            KERN_INVALID_ADDRESS
        );
        assert_eq!(mem.bytes_at(destination.cast_const(), 4), &[0xAA; 4]);
        assert_eq!(mem.read(outsize), 0x1234);
    }

    #[test]
    fn vm_read_overwrite_allows_zero_length_with_null_data_pointers() {
        let mut mem = new_test_mem();
        let outsize: MutPtr<mach_vm_size_t> = mem.alloc(4).cast();
        mem.write(outsize, 1);

        assert_eq!(
            vm_read_overwrite_in_memory(&mut mem, MACH_TASK_SELF, 0, 0, 0, outsize),
            KERN_SUCCESS
        );
        assert_eq!(mem.read(outsize), 0);
    }
}
