// =========================================================================
// MARK: - NXArchInfo / mach-o/arch.h stubs
// =========================================================================
//
// These are used by apps that inspect the running architecture at runtime,
// e.g. to choose code paths or log diagnostic information.

use crate::dyld::FunctionExports;
use crate::mem::{ConstPtr, MutPtr, SafeRead};
use crate::export_c_func;

/// `NXArchInfo` — describes a Mach-O architecture.
#[repr(C, packed)]
#[allow(non_camel_case_types)]
pub struct NXArchInfo {
   /// Short name string pointer (e.g. "arm", "armv7") — points to static data.
   name:        ConstPtr<u8>,
   /// `cpu_type_t`
   cputype:     i32,
   /// `cpu_subtype_t`
   cpusubtype:  i32,
   /// Byte order — 0 = big-endian, 1 = little-endian.
   byteorder:   u8,
   /// Human-readable description pointer.
   description: ConstPtr<u8>,
}
unsafe impl SafeRead for NXArchInfo {}

// cpu_type_t / cpu_subtype_t constants (subset used by iOS devices).
const CPU_TYPE_ARM:       i32 = 12;
const CPU_SUBTYPE_ARM_V7: i32 = 9;
const CPU_SUBTYPE_ARM_V7S:i32 = 11;
const CPU_SUBTYPE_ARM_ALL:i32 = 0;

const CPU_TYPE_X86:       i32 = 7;
const CPU_SUBTYPE_X86_ALL:i32 = 3;

const CPU_TYPE_ANY:       i32 = -1;
const CPU_SUBTYPE_MULTIPLE:i32 = -1;

/// Write a static `NXArchInfo` for ARMv7 into guest memory and return a pointer.
fn make_arch_info(env: &mut crate::Environment) -> MutPtr<NXArchInfo> {
   let name_bytes = b"armv7\0";
   let desc_bytes = b"ARM v7\0";

   let name_ptr  = env.mem.alloc_and_write_cstr(name_bytes);
   let desc_ptr  = env.mem.alloc_and_write_cstr(desc_bytes);

   let info = NXArchInfo {
       name:        name_ptr.cast_const(),
       cputype:     CPU_TYPE_ARM,
       cpusubtype:  CPU_SUBTYPE_ARM_V7,
       byteorder:   1, // little-endian
       description: desc_ptr.cast_const(),
   };
   env.mem.alloc_and_write(info).cast()
}

// =========================================================================
// MARK: - NXGetArchInfoFromCpuType
// =========================================================================

/// `const NXArchInfo *NXGetArchInfoFromCpuType(cpu_type_t cputype,
///                                              cpu_subtype_t cpusubtype)`
///
/// Returns a pointer to a static `NXArchInfo` for the given cpu type/subtype,
/// or NULL if not found.
fn NXGetArchInfoFromCpuType(
   env: &mut crate::Environment,
   cputype: i32,
   cpusubtype: i32,
) -> MutPtr<NXArchInfo> {
   log_dbg!(
       "NXGetArchInfoFromCpuType(cputype={}, cpusubtype={}) — returning ARMv7 stub",
       cputype, cpusubtype
   );
   // We only emulate ARM, so return the ARMv7 info for any ARM query.
   // Return NULL for anything else so callers handle the unknown case.
   if cputype == CPU_TYPE_ARM
       || cputype == CPU_TYPE_ANY
       || cpusubtype == CPU_SUBTYPE_MULTIPLE
   {
       make_arch_info(env)
   } else {
       crate::mem::Ptr::null()
   }
}

/// `const NXArchInfo *NXGetArchInfoFromName(const char *name)`
///
/// Returns a pointer to a static `NXArchInfo` for the given architecture name,
/// or NULL if not found.
fn NXGetArchInfoFromName(
   env: &mut crate::Environment,
   name: ConstPtr<u8>,
) -> MutPtr<NXArchInfo> {
   if name.is_null() {
       return crate::mem::Ptr::null();
   }
   let name_str = env.mem.cstr_at_utf8(name).unwrap_or_default().to_owned();
   log_dbg!("NXGetArchInfoFromName({:?})", name_str);
   match name_str.as_str() {
       "arm" | "armv6" | "armv7" | "armv7s" | "armv7k" => make_arch_info(env),
       _ => {
           log_dbg!("NXGetArchInfoFromName: unknown arch {:?} — returning NULL", name_str);
           crate::mem::Ptr::null()
       }
   }
}

/// `const NXArchInfo *NXGetLocalArchInfo(void)`
///
/// Returns the `NXArchInfo` for the currently running architecture.
/// Since touchHLE emulates ARMv7, we always return the ARMv7 info.
fn NXGetLocalArchInfo(env: &mut crate::Environment) -> MutPtr<NXArchInfo> {
   log_dbg!("NXGetLocalArchInfo() — returning ARMv7");
   make_arch_info(env)
}

/// `const NXArchInfo *NXGetAllArchInfos(void)`
///
/// Returns a pointer to a NULL-terminated array of `NXArchInfo` structs
/// describing all known architectures. We return a two-element array:
/// one ARMv7 entry and one zeroed terminator.
fn NXGetAllArchInfos(env: &mut crate::Environment) -> MutPtr<NXArchInfo> {
   let name_ptr = env.mem.alloc_and_write_cstr(b"armv7\0");
   let desc_ptr = env.mem.alloc_and_write_cstr(b"ARM v7\0");

   // Allocate room for two NXArchInfo entries (entry + NULL terminator).
   let arr: MutPtr<NXArchInfo> = env.mem.alloc(
       (std::mem::size_of::<NXArchInfo>() * 2) as u32
   ).cast();

   let entry = NXArchInfo {
       name:        name_ptr.cast_const(),
       cputype:     CPU_TYPE_ARM,
       cpusubtype:  CPU_SUBTYPE_ARM_V7,
       byteorder:   1,
       description: desc_ptr.cast_const(),
   };
   // NULL terminator — all-zero NXArchInfo.
   let terminator = NXArchInfo {
       name:        crate::mem::Ptr::null(),
       cputype:     0,
       cpusubtype:  0,
       byteorder:   0,
       description: crate::mem::Ptr::null(),
   };

   env.mem.write(arr,       entry);
   env.mem.write(arr + 1u32, terminator);
   arr
}

/// `void NXFreeArchInfo(NXArchInfo *info)`
///
/// Frees a dynamically-allocated `NXArchInfo`. In our implementation all
/// infos are heap-allocated (we have no static guest memory), so we free
/// both the struct and any string pointers it contains.
fn NXFreeArchInfo(env: &mut crate::Environment, info: MutPtr<NXArchInfo>) {
   if info.is_null() { return; }
   let arch = env.mem.read(info);
   if !arch.name.is_null() {
       env.mem.free(arch.name.cast_mut().cast());
   }
   if !arch.description.is_null() {
       env.mem.free(arch.description.cast_mut().cast());
   }
   env.mem.free(info.cast());
}

/// `cpu_type_t NXCPUType(void)` — returns the CPU type of the running process.
fn NXCPUType(_env: &mut crate::Environment) -> i32 {
   CPU_TYPE_ARM
}

/// `cpu_subtype_t NXCPUSubtype(void)` — returns the CPU subtype.
fn NXCPUSubtype(_env: &mut crate::Environment) -> i32 {
   CPU_SUBTYPE_ARM_V7
}

/// `const char *NXByteOrder(void)` — returns "Little Endian" or "Big Endian".
fn NXByteOrder(env: &mut crate::Environment) -> ConstPtr<u8> {
   env.mem.alloc_and_write_cstr(b"Little Endian\0").cast_const()
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(NXGetArchInfoFromCpuType(_, _)),
    export_c_func!(NXGetArchInfoFromName(_)),
    export_c_func!(NXGetLocalArchInfo()),
    export_c_func!(NXGetAllArchInfos()),
    export_c_func!(NXFreeArchInfo(_)),
    export_c_func!(NXCPUType()),
    export_c_func!(NXCPUSubtype()),
    export_c_func!(NXByteOrder()),
];

