/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Handling of Objective-C properties.
//!
//! Objective-C properties and instance variables (ivars) are distinct, but
//! both use class metadata. This module parses that metadata and implements the
//! corresponding runtime accessors.
//!
//! Resources:
//! - `objc_setProperty` and friends are not documented, so [reading the source code](https://opensource.apple.com/source/objc4/objc4-551.1/runtime/Accessors.subproj/objc-accessors.mm.auto.html) is useful.
//!
//! See also: [crate::frameworks::foundation::ns_object].

use super::classes::IvarInfo;
use super::{id, msg, nil, release, retain, Class, ClassHostObject, ObjC, SEL};
use crate::mem::{
    guest_size_of, ConstPtr, ConstVoidPtr, GuestISize, GuestUSize, Mem, MutPtr, MutVoidPtr, Ptr,
    SafeRead,
};
use crate::{Environment, MutexType};
use std::sync::atomic::{AtomicUsize, Ordering};

static IVAR_ACCESS_DIAGNOSTICS: AtomicUsize = AtomicUsize::new(0);

fn trace_ivar_access(message: &str) {
    if crate::env_flag_cached!("TOUCHHLE_TRACE_IVARS")
        && IVAR_ACCESS_DIAGNOSTICS.fetch_add(1, Ordering::Relaxed) < 96
    {
        log!("OBJC-IVAR {}", message);
    }
}

/// The layout of a property list in an app binary.
///
/// The name, field names and field layout are based on what Ghidra outputs.
#[repr(C, packed)]
pub(super) struct ivar_list_t {
    entsize: GuestUSize,
    count: GuestUSize,
    // entries follow the struct
}
unsafe impl SafeRead for ivar_list_t {}

/// The layout of an ivar in an app binary.
///
/// The name, field names and field layout are based on what Ghidra outputs.
#[repr(C, packed)]
struct ivar_t {
    offset: ConstPtr<GuestUSize>,
    name: ConstPtr<u8>,
    type_: ConstPtr<u8>,
    alignment: u32,
    size: u32,
}
unsafe impl SafeRead for ivar_t {}

/// The layout of an Objective-C property list in an app binary.
/// Matches `objc_property_list` from Apple's objc4 runtime source.
#[repr(C, packed)]
pub(super) struct property_list_t {
    entsize_and_flags: GuestUSize,
    count: GuestUSize,
    // property_t entries follow the struct
}
unsafe impl SafeRead for property_list_t {}

/// The layout of a single declared @property in an app binary.
/// Matches `property_t` from Apple's objc4 runtime source.
/// `class_getProperty` returns a pointer to this structure as the
/// opaque `objc_property_t`.
#[repr(C, packed)]
pub(super) struct property_t {
    name: ConstPtr<u8>,
    attributes: ConstPtr<u8>,
}
unsafe impl SafeRead for property_t {}

impl ClassHostObject {
    pub(super) fn add_ivars_from_bin(&mut self, ivar_list_ptr: ConstPtr<ivar_list_t>, mem: &Mem) {
        let ivar_list_t { entsize, count } = mem.read(ivar_list_ptr);
        let min_entsize = guest_size_of::<ivar_t>();
        if entsize < min_entsize {
            log!(
                "Warning: add_ivars_from_bin: ivar_list_t at {:?} declares entsize {} smaller than ivar_t ({}); skipping list.",
                ivar_list_ptr,
                entsize,
                min_entsize
            );
            return;
        }

        let ivars_base_ptr: ConstPtr<ivar_t> = (ivar_list_ptr + 1).cast();

        for i in 0..count {
            let ivar_ptr: ConstPtr<ivar_t> = Ptr::from_bits(ivars_base_ptr.to_bits() + i * entsize);

            let ivar_t {
                offset,
                name,
                alignment,
                type_,
                size,
            } = mem.read(ivar_ptr);

            let Ok(name_string) = mem.cstr_at_utf8(name) else {
                log!(
                    "Warning: add_ivars_from_bin: ivar name at {:?} is not valid UTF-8; skipping entry.",
                    name
                );
                continue;
            };
            // Only plain scalar types, never pointers, objects or aggregates.
            self.scalar_ivars.remove(name_string);
            if let Some(bytes) = mem.get_bytes_fallible(type_.cast(), 2) {
                let code = bytes[0];
                let width = match code {
                    b'c' | b'C' => 1,
                    b's' | b'S' => 2,
                    b'i' | b'I' | b'l' | b'L' | b'f' => 4,
                    _ => 0,
                };
                if bytes[1] == 0 && width != 0 && size == width {
                    self.scalar_ivars.insert(name_string.into(), (code, width));
                }
            }
            self.ivars.insert(
                name_string.into(),
                IvarInfo {
                    offset,
                    alignment,
                    metadata: ivar_ptr.cast_void(),
                },
            );
        }
    }

    /// Parse the property list from the binary and populate `self.properties`.
    /// This allows `class_getProperty` to return proper `objc_property_t`
    /// pointers for declared @property entries.
    pub(super) fn add_properties_from_bin(&mut self, prop_list_ptr: ConstVoidPtr, mem: &Mem) {
        let prop_list_ptr: ConstPtr<property_list_t> = prop_list_ptr.cast();
        let property_list_t {
            entsize_and_flags,
            count,
        } = mem.read(prop_list_ptr);

        // The entsize field may have flags in the high bits; mask to get
        // the actual entry size (Apple's runtime uses & ~3u for alignment,
        // but we just mask the lower 16 bits which is safe for any
        // reasonable entry size).
        let entsize = entsize_and_flags & 0xFFFF;
        let min_entsize = guest_size_of::<property_t>();
        if entsize < min_entsize {
            log_dbg!(
                "add_properties_from_bin: property_list_t at {:?} declares entsize {} smaller than property_t ({}); skipping list.",
                prop_list_ptr,
                entsize,
                min_entsize
            );
            return;
        }

        let props_base_ptr: ConstPtr<property_t> = (prop_list_ptr + 1).cast();

        for i in 0..count {
            let prop_ptr: ConstPtr<property_t> =
                Ptr::from_bits(props_base_ptr.to_bits() + i * entsize);

            let property_t { name, .. } = mem.read(prop_ptr);

            if name.is_null() {
                continue;
            }
            let Ok(name_string) = mem.cstr_at_utf8(name) else {
                continue;
            };
            // Store the guest pointer to the property_t entry itself.
            // class_getProperty returns this as the opaque objc_property_t.
            self.properties
                .insert(name_string.to_string(), prop_ptr.cast());
        }
    }
}

impl ObjC {
    /// Identify an exact scalar field on a registered object. This diagnostic
    /// never messages guest objects or invokes the permissive fake-borrow path.
    /// The caller supplies the live allocation containing the candidate.
    pub(crate) fn diagnostic_scalar_field(
        &self,
        mem: &Mem,
        base: u32,
        allocation_size: u32,
        addr: u32,
        width: u32,
        encoding: u8,
    ) -> Option<&str> {
        let object = id::from_bits(base);
        self.get_host_object(object)?;
        let offset = addr.checked_sub(base)?;
        if offset < 4 || offset.checked_add(width)? > allocation_size {
            return None;
        }
        let bytes = mem.get_bytes_fallible(ConstVoidPtr::from_bits(base), 4)?;
        let mut class = id::from_bits(u32::from_le_bytes(bytes.try_into().ok()?));
        let mut budget = 256usize;
        for _ in 0..16 {
            let host = self
                .get_host_object(class)?
                .as_any()
                .downcast_ref::<ClassHostObject>()?;
            if host.is_metaclass {
                return None;
            }
            for (name, &(code, size)) in &host.scalar_ivars {
                budget = budget.checked_sub(1)?;
                let code = match code {
                    b'l' => b'i',
                    b'L' => b'I',
                    _ => code,
                };
                if code != encoding || size != width {
                    continue;
                }
                let ivar = host.ivars.get(name)?;
                let bytes = mem.get_bytes_fallible(ivar.offset.cast(), 4)?;
                let field_offset = u32::from_le_bytes(bytes.try_into().ok()?);
                if field_offset == offset && offset.checked_add(width)? <= host.instance_size {
                    return Some(name);
                }
            }
            if host.superclass == nil {
                break;
            }
            class = host.superclass;
        }
        None
    }

    pub(crate) fn class_lookup_ivar(&self, mut class: Class, name: &str) -> Option<ConstVoidPtr> {
        for _ in 0..256 {
            if class == nil {
                return None;
            }
            let host = self
                .get_host_object(class)?
                .as_any()
                .downcast_ref::<ClassHostObject>()?;
            if let Some(ivar) = host.ivars.get(name) {
                return Some(ivar.metadata);
            }
            class = host.superclass;
        }
        None
    }

    pub(crate) fn class_lookup_ivar_offset_by_metadata(
        &self,
        mut class: Class,
        metadata: ConstVoidPtr,
    ) -> Option<ConstPtr<GuestUSize>> {
        if metadata.is_null() {
            return None;
        }
        for _ in 0..256 {
            if class == nil {
                return None;
            }
            let host = self
                .get_host_object(class)?
                .as_any()
                .downcast_ref::<ClassHostObject>()?;
            if let Some(ivar) = host
                .ivars
                .values()
                .find(|ivar| ivar.metadata.to_bits() == metadata.to_bits())
            {
                return Some(ivar.offset);
            }
            class = host.superclass;
        }
        None
    }

    /// Checks if the object's class has an ivar in its class chain with the
    /// provided name and returns the pointer to the object's ivar, if any,
    /// or None if the object's class doesn't have an ivar with that name.
    pub fn object_lookup_ivar(
        &self,
        mem: &Mem,
        obj: id,
        name: &String,
    ) -> Option<MutPtr<GuestUSize>> {
        let mut class = ObjC::read_isa(obj, mem);
        loop {
            let &ClassHostObject {
                superclass,
                ref ivars,
                ..
            } = self.borrow(class);
            if let Some(ivar) = ivars.get(name) {
                let ivar_offset = mem.read(ivar.offset);
                let address = obj.to_bits().checked_add(ivar_offset)?;
                return Some(MutVoidPtr::from_bits(address).cast());
            } else if superclass == nil {
                return None;
            } else {
                class = superclass;
            }
        }
    }
    /// Like [Self::object_lookup_ivar] but also returns the ivar's `@encode`
    /// type string (read from its `ivar_t` metadata), so KVC can box a scalar
    /// or struct ivar correctly instead of reading its raw bits as an `id`.
    pub fn object_lookup_ivar_encoding(
        &self,
        mem: &Mem,
        obj: id,
        name: &String,
    ) -> Option<(MutPtr<GuestUSize>, Vec<u8>)> {
        let mut class = ObjC::read_isa(obj, mem);
        loop {
            let &ClassHostObject {
                superclass,
                ref ivars,
                ..
            } = self.borrow(class);
            if let Some(ivar) = ivars.get(name) {
                let ivar_offset = mem.read(ivar.offset);
                let address = obj.to_bits().checked_add(ivar_offset)?;
                let ptr: MutPtr<GuestUSize> = MutVoidPtr::from_bits(address).cast();
                let encoding = if ivar.metadata.is_null() {
                    Vec::new()
                } else {
                    let ivar_t { type_, .. } = mem.read(ivar.metadata.cast());
                    if type_.is_null() {
                        Vec::new()
                    } else {
                        mem.cstr_at(type_).to_vec()
                    }
                };
                return Some((ptr, encoding));
            } else if superclass == nil {
                return None;
            } else {
                class = superclass;
            }
        }
    }

    /// Like [Self::object_lookup_ivar] but returns the ivar's `@encode` type
    /// string (from the binary's `ivar_t`), used by KVC to box a direct ivar
    /// read of a scalar/struct into NSNumber/NSValue.
    pub fn object_lookup_ivar_type(&self, mem: &Mem, obj: id, name: &String) -> Option<Vec<u8>> {
        let mut class = ObjC::read_isa(obj, mem);
        loop {
            let &ClassHostObject {
                superclass,
                ref ivars,
                ..
            } = self.borrow(class);
            if let Some(ivar) = ivars.get(name) {
                if ivar.metadata.is_null() {
                    return None;
                }
                let ivar_t { type_, .. } = mem.read(ivar.metadata.cast());
                if type_.is_null() {
                    return None;
                }
                return Some(mem.cstr_at(type_).to_vec());
            } else if superclass == nil {
                return None;
            } else {
                class = superclass;
            }
        }
    }

    /// Like [Self::object_lookup_ivar], but also returns the ivar's `@encode`
    /// type string (read from its `ivar_t` metadata). Used by KVC to box
    /// scalar/struct ivars read directly.
    pub fn object_lookup_ivar_with_type(
        &self,
        mem: &Mem,
        obj: id,
        name: &String,
    ) -> Option<(MutPtr<GuestUSize>, Vec<u8>)> {
        let mut class = ObjC::read_isa(obj, mem);
        loop {
            let &ClassHostObject {
                superclass,
                ref ivars,
                ..
            } = self.borrow(class);
            if let Some(ivar) = ivars.get(name) {
                let ivar_offset = mem.read(ivar.offset);
                let address = obj.to_bits().checked_add(ivar_offset)?;
                let ptr: MutPtr<GuestUSize> = MutVoidPtr::from_bits(address).cast();
                let encoding = if ivar.metadata.is_null() {
                    Vec::new()
                } else {
                    let ivar_t_rec: ivar_t = mem.read(ivar.metadata.cast());
                    let type_ptr = ivar_t_rec.type_;
                    if type_ptr.is_null() {
                        Vec::new()
                    } else {
                        mem.cstr_at(type_ptr).to_vec()
                    }
                };
                return Some((ptr, encoding));
            } else if superclass == nil {
                return None;
            } else {
                class = superclass;
            }
        }
    }

    pub fn debug_all_class_ivars_as_strings(&self, class: Class) -> Vec<String> {
        let mut class = class;
        let mut ivars_strings = Vec::new();
        loop {
            let &ClassHostObject {
                superclass,
                ref ivars,
                ..
            } = self.borrow(class);
            let mut class_ivars_strings = ivars.keys().cloned().collect();
            ivars_strings.append(&mut class_ivars_strings);
            if superclass == nil {
                break;
            } else {
                class = superclass;
            }
        }
        ivars_strings
    }
}

fn object_ivar_value(objc: &ObjC, mem: &Mem, object: id, ivar: ConstVoidPtr) -> Option<id> {
    if object == nil || ivar.is_null() {
        return None;
    }
    let class = ObjC::read_isa(object, mem);
    let offset_pointer = objc.class_lookup_ivar_offset_by_metadata(class, ivar)?;
    if offset_pointer.is_null() {
        return None;
    }
    let offset_bytes = mem.get_bytes_fallible(offset_pointer.cast(), 4)?;
    let offset = u32::from_le_bytes(offset_bytes.try_into().ok()?);
    let field_address = object.to_bits().checked_add(offset)?;
    let value_bytes = mem.get_bytes_fallible(ConstVoidPtr::from_bits(field_address), 4)?;
    Some(id::from_bits(u32::from_le_bytes(
        value_bytes.try_into().ok()?,
    )))
}

/// Returns the named ivar metadata, searching the class and its superclasses.
pub fn class_getInstanceVariable(
    env: &mut Environment,
    class: Class,
    name: ConstPtr<u8>,
) -> MutVoidPtr {
    if class == nil || name.is_null() {
        return MutVoidPtr::null();
    }
    let Ok(name) = env.mem.cstr_at_utf8(name) else {
        return MutVoidPtr::null();
    };
    let class_name = env.objc.get_class_name(class).to_string();
    let ivar = env.objc.class_lookup_ivar(class, name);
    trace_ivar_access(&format!(
        "class_getInstanceVariable class={} name={} metadata={:?}",
        class_name, name, ivar
    ));
    ivar.map(ConstVoidPtr::cast_mut)
        .unwrap_or_else(MutVoidPtr::null)
}

/// Reads an object reference from the specified ivar, returning nil for nil or invalid inputs.
pub fn object_getIvar(env: &mut Environment, object: id, ivar: MutVoidPtr) -> id {
    let class_name = if object == nil {
        "nil".to_string()
    } else {
        env.objc
            .get_class_name(ObjC::read_isa(object, &env.mem))
            .to_string()
    };
    let offset = if object == nil || ivar.is_null() {
        None
    } else {
        env.objc
            .class_lookup_ivar_offset_by_metadata(ObjC::read_isa(object, &env.mem), ivar.cast_const())
            .map(|pointer| env.mem.read(pointer))
    };
    let value = object_ivar_value(&env.objc, &env.mem, object, ivar.cast_const()).unwrap_or(nil);
    trace_ivar_access(&format!(
        "object_getIvar class={} object={:?} ivar={:?} offset={:?} value={:?}",
        class_name, object, ivar, offset, value
    ));
    value
}

/// `Ivar object_getInstanceVariable(id obj, const char *name, void **outValue)`
///
/// Classic runtime lookup: finds the named ivar, stores the pointer-sized
/// value at its offset in `*outValue`, and returns the ivar metadata (or NULL
/// if the class does not declare it). Used by code that reads outlets through
/// runtime metadata instead of a compile-time ivar reference.
/// <https://developer.apple.com/documentation/objectivec/1418639-object_getinstancevariable>
pub fn object_getInstanceVariable(
    env: &mut Environment,
    object: id,
    name: ConstPtr<u8>,
    out_value: MutPtr<MutVoidPtr>,
) -> MutVoidPtr {
    if !out_value.is_null() {
        env.mem.write(out_value, MutVoidPtr::null());
    }
    if object == nil || name.is_null() {
        return MutVoidPtr::null();
    }
    let class = ObjC::read_isa(object, &env.mem);
    let ivar = class_getInstanceVariable(env, class, name);
    if ivar.is_null() {
        return ivar;
    }
    let value = object_ivar_value(&env.objc, &env.mem, object, ivar.cast_const()).unwrap_or(nil);
    if !out_value.is_null() {
        env.mem.write(out_value, value.cast());
    }
    ivar
}

/// `void object_setIvar(id obj, Ivar ivar, id value)`
///
/// Classic/ARC runtime primitive that stores an object pointer directly into
/// an instance variable (bypassing any synthesized setter). This is used by
/// code that wires outlets or callback objects through runtime ivar metadata.
///
/// Like Apple's `object_setIvar`, this does not retain `value` or release the
/// previous value; it just stores the raw pointer at the ivar's offset.
pub fn object_setIvar(env: &mut Environment, object: id, ivar: MutVoidPtr, value: id) {
    if object == nil || ivar.is_null() {
        return;
    }
    let class = ObjC::read_isa(object, &env.mem);
    let class_name = env.objc.get_class_name(class).to_string();
    let Some(offset_pointer) = env
        .objc
        .class_lookup_ivar_offset_by_metadata(class, ivar.cast_const())
    else {
        trace_ivar_access(&format!(
            "object_setIvar class={} object={:?} ivar={:?} value={:?} — unknown ivar, ignored",
            class_name, object, ivar, value
        ));
        return;
    };
    if offset_pointer.is_null() {
        return;
    }
    let offset: GuestUSize = env.mem.read(offset_pointer);
    let field_address = object.to_bits().wrapping_add(offset);
    let field_ptr: MutPtr<id> = Ptr::from_bits(field_address);
    env.mem.write(field_ptr, value);
    trace_ivar_access(&format!(
        "object_setIvar class={} object={:?} ivar={:?} offset={} value={:?}",
        class_name, object, ivar, offset, value
    ));
}
/// Acquire the per-object recursive mutex used to protect `atomic`
/// property accesses. Lazily creates the mutex on first use.
///
/// Mirrors the role of Apple's striped `PropertyLocks` table from
/// `objc-accessors.mm` — see [`ObjC::property_locks`] for the full
/// design rationale. Returns `None` if `this` is `nil` (in which case
/// no locking is needed because no real ivar will be accessed below).
fn lock_property_atomic(env: &mut Environment, this: id) -> Option<crate::MutexId> {
    if this == nil {
        return None;
    }
    let mutex_id = if let Some(&existing) = env.objc.property_locks.get(&this) {
        existing
    } else {
        let new_id = env
            .mutex_state
            .init_mutex(MutexType::PTHREAD_MUTEX_RECURSIVE);
        env.objc.property_locks.insert(this, new_id);
        log_dbg!(
            "Created property-lock mutex #{} for object {:#x}",
            new_id,
            this.to_bits()
        );
        new_id
    };
    // Lock errors here would be a host-side bug (recursive mutex on the
    // same thread can never fail), so unwrap is safe.
    env.lock_mutex(mutex_id).unwrap();
    Some(mutex_id)
}

/// Release a property lock previously acquired with [`lock_property_atomic`].
fn unlock_property_atomic(env: &mut Environment, mutex_id: Option<crate::MutexId>) {
    if let Some(mutex_id) = mutex_id {
        // Same reasoning as above: unlocking a mutex we just locked on
        // the current thread cannot fail.
        let _ = env.unlock_mutex(mutex_id);
    }
}

/// Undocumented function (see link above) apparently used by auto-generated
/// methods for properties to get an ivar.
pub(super) fn objc_getProperty(
    env: &mut Environment,
    this: id,
    _cmd: SEL,
    offset: GuestISize,
    atomic: bool,
) -> id {
    // We currently aren't touching the ivar layouts contained in the binary, so
    // we are assuming they are already correctly set by the compiler. Since we
    // aren't using ivars at all in our host classes, we shouldn't have any
    // issues with host classes' ivars clobbering guest classes' ivars, but
    // what if the compiler doesn't set the ivar layout at all? This is a simple
    // safeguard: any real ivar offset will be after the isa pointer.
    if offset < 4 {
        log!(
            "Warning: objc_getProperty: suspicious ivar offset {} (would clobber isa); returning nil.",
            offset
        );
        return nil;
    }

    let lock = if atomic {
        lock_property_atomic(env, this)
    } else {
        None
    };

    if crate::env_flag_cached!("TOUCHHLE_TRACE_IVARS") && this != nil {
        let class_name = env
            .objc
            .get_class_name(ObjC::read_isa(this, &env.mem))
            .to_string();
        if class_name.contains("CCDirector") {
            trace_ivar_access(&format!(
                "objc_getProperty class={} selector={} this={:?} offset={} atomic={}",
                class_name,
                _cmd.as_str(&env.mem),
                this,
                offset,
                atomic
            ));
        }
    }

    let Some(addr) = this.to_bits().checked_add_signed(offset) else {
        log!(
            "Warning: objc_getProperty: overflow computing ivar address for this={:#x}, offset={}; returning nil.",
            this.to_bits(),
            offset
        );
        unlock_property_atomic(env, lock);
        return nil;
    };
    let ivar: MutPtr<id> = Ptr::from_bits(addr);
    let value = env.mem.read(ivar);
    if crate::env_flag_cached!("TOUCHHLE_TRACE_IVARS") && this != nil {
        let class_name = env
            .objc
            .get_class_name(ObjC::read_isa(this, &env.mem))
            .to_string();
        if class_name.contains("CCDirector") {
            trace_ivar_access(&format!(
                "objc_getProperty result class={} selector={} offset={} value={:?}",
                class_name,
                _cmd.as_str(&env.mem),
                offset,
                value
            ));
        }
    }
    unlock_property_atomic(env, lock);
    value
}

/// Undocumented function (see link above) apparently used by auto-generated
/// methods for properties to set an ivar and handle reference counting, copying
/// and locking.
pub(super) fn objc_setProperty(
    env: &mut Environment,
    this: id,
    _cmd: SEL,
    offset: GuestISize,
    value: id,
    atomic: bool,
    should_copy: i8,
) {
    // We currently aren't touching the ivar layouts contained in the binary, so
    // we are assuming they are already correctly set by the compiler. Since we
    // aren't using ivars at all in our host classes, we shouldn't have any
    // issues with host classes' ivars clobbering guest classes' ivars, but
    // what if the compiler doesn't set the ivar layout at all? This is a simple
    // safeguard: any real ivar offset will be after the isa pointer.
    if offset < 4 {
        log!(
            "Warning: objc_setProperty: suspicious ivar offset {} (would clobber isa); ignoring write.",
            offset
        );
        return;
    }

    let lock = if atomic {
        lock_property_atomic(env, this)
    } else {
        None
    };

    let Some(addr) = this.to_bits().checked_add_signed(offset) else {
        log!(
            "Warning: objc_setProperty: overflow computing ivar address for this={:#x}, offset={}; ignoring write.",
            this.to_bits(),
            offset
        );
        unlock_property_atomic(env, lock);
        return;
    };
    let ivar: MutPtr<id> = Ptr::from_bits(addr);
    let old = env.mem.read(ivar);

    let void_null: MutVoidPtr = Ptr::null();
    let value: id = if value != nil {
        match should_copy {
            0 => retain(env, value),
            1 => msg![env; value copyWithZone:void_null],
            2 => msg![env; value mutableCopyWithZone:void_null],
            // Apple's source code implies that any non-zero value that isn't 2
            // should mean "copy", but that seems weird; treat unknown values
            // as a regular copy and just log it instead of crashing.
            other => {
                log!(
                    "Warning: objc_setProperty: unknown \"should copy\" value: {}; treating as copyWithZone:.",
                    other
                );
                msg![env; value copyWithZone:void_null]
            }
        }
    } else {
        nil
    };
    env.mem.write(ivar, value);

    if old != nil {
        release(env, old);
    }

    unlock_property_atomic(env, lock);
}

/// Optimised non-atomic, retain-property setter. Modern compilers emit
/// `_objc_setProperty_nonatomic` instead of the generic
/// `_objc_setProperty(…, atomic=false, should_copy=0)` for autosynthesised
/// `@property (nonatomic, retain)` / `@property (nonatomic, strong)`
/// setters. We just forward to the generic implementation so the ivar
/// receives a real `objc_retain` of the new value (with proper
/// `objc_release` of the previous value), instead of touchHLE silently
/// dropping the assignment.
///
/// Note the argument order: `(self, _cmd, newValue, offset)` — the
/// optimised variants put the value *before* the offset, the opposite of
/// the generic [objc_setProperty]. See Apple's open-source
/// `objc4/runtime/Accessors.subproj/objc-accessors.mm`.
pub(super) fn objc_setProperty_nonatomic(
    env: &mut Environment,
    this: id,
    _cmd: SEL,
    value: id,
    offset: GuestISize,
) {
    objc_setProperty(
        env, this, _cmd, offset, value, /* atomic: */ false, /* should_copy: */ 0,
    )
}

/// Optimised atomic, retain-property setter. Compilers emit
/// `_objc_setProperty_atomic` instead of the generic
/// `_objc_setProperty(…, atomic=true, should_copy=0)` for autosynthesised
/// `@property (atomic, retain)` / `@property (atomic, strong)` setters
/// (i.e. the default atomic kind when no explicit `nonatomic` is given).
/// touchHLE previously installed a return-0 stub for this entry point,
/// which silently dropped every assignment on atomic properties and led to
/// guest crashes when the property was later read back as nil.
///
/// Note the argument order: `(self, _cmd, newValue, offset)` — the
/// optimised variants put the value *before* the offset, the opposite of
/// the generic [objc_setProperty]. See Apple's open-source
/// `objc4/runtime/Accessors.subproj/objc-accessors.mm` for the canonical
/// definition.
pub(super) fn objc_setProperty_atomic(
    env: &mut Environment,
    this: id,
    _cmd: SEL,
    value: id,
    offset: GuestISize,
) {
    objc_setProperty(
        env, this, _cmd, offset, value, /* atomic: */ true, /* should_copy: */ 0,
    )
}

/// Optimised non-atomic, copy-property setter. Mid-iOS-6+ compilers emit
/// `_objc_setProperty_nonatomic_copy` instead of the generic
/// `_objc_setProperty(…, atomic=false, should_copy=1)` for autosynthesised
/// `@property (nonatomic, copy)` setters; we just forward to the generic
/// implementation so the ivar gets a real `copyWithZone:` of the new value
/// (with proper retain/release of the previous value), instead of touchHLE
/// installing a return-0 stub that silently drops every assignment.
///
/// Note the argument order: `(self, _cmd, newValue, offset)` — the
/// optimised variants put the value *before* the offset, the opposite of
/// the generic [objc_setProperty].
pub(super) fn objc_setProperty_nonatomic_copy(
    env: &mut Environment,
    this: id,
    _cmd: SEL,
    value: id,
    offset: GuestISize,
) {
    objc_setProperty(
        env, this, _cmd, offset, value, /* atomic: */ false, /* should_copy: */ 1,
    )
}

/// Optimised atomic, copy-property setter. Same idea as
/// [objc_setProperty_nonatomic_copy] but emitted for `@property (copy)` /
/// `@property (atomic, copy)` setters.
pub(super) fn objc_setProperty_atomic_copy(
    env: &mut Environment,
    this: id,
    _cmd: SEL,
    value: id,
    offset: GuestISize,
) {
    objc_setProperty(
        env, this, _cmd, offset, value, /* atomic: */ true, /* should_copy: */ 1,
    )
}

// note: https://opensource.apple.com/source/objc4/objc4-723/runtime/objc-accessors.mm.auto.html
//       says that hasStrong is unused.
pub(super) fn objc_copyStruct(
    env: &mut Environment,
    dest: MutVoidPtr,
    src: ConstVoidPtr,
    size: GuestUSize,
    _atomic: bool,
    _hasStrong: bool,
) {
    // It's safe to ignore atomic as we never switch thread unless we call back
    // into guest code and we're not doing that here, just calling memmove.
    // TODO: implement atomic support
    env.mem.memmove(dest, src, size);
}

/// `const char *property_getName(objc_property_t property)` — returns the
/// name of the declared `@property`, as a C string.
///
/// Per Apple's Objective-C Runtime Reference
/// (<https://developer.apple.com/documentation/objectivec/property_getname(_:)>):
///
/// > Returns the name of a property.
/// > Return Value: A C string containing the property's name.
///
/// `objc_property_t` is an opaque pointer; in touchHLE it points at the
/// `property_t { name, attributes }` entry parsed from the app binary's
/// property list (see [ClassHostObject::add_properties_from_bin]). We read
/// the `name` field and return the guest pointer to that C string directly,
/// matching real libobjc which returns a pointer into the property struct
/// (the caller must NOT free it).
///
/// touchHLE previously had no implementation, so dyld installed a return-0
/// stub. Returning NULL for `property_getName` breaks reflective code such
/// as KVC's `-dictionaryWithValuesForKeys:` and JSON/serialization helpers
/// that enumerate a class's properties (e.g. Spy Mouse HD's ad SDK).
pub fn property_getName(env: &mut Environment, property: ConstVoidPtr) -> ConstPtr<u8> {
    if property.is_null() {
        // Real libobjc returns "" (a pointer to an empty string) for a NULL
        // property rather than NULL. Returning NULL here is acceptable for
        // callers that null-check, but matching the documented behaviour is
        // safer: we have no empty-string constant handy, so return NULL,
        // which guest code treats the same as "no name".
        return Ptr::null();
    }
    let prop_ptr: ConstPtr<property_t> = property.cast();
    let property_t { name, .. } = env.mem.read(prop_ptr);
    name
}

/// `const char *property_getAttributes(objc_property_t property)` — returns
/// the attribute string of the declared `@property`, as a C string.
///
/// Per Apple's Objective-C Runtime Reference
/// (<https://developer.apple.com/documentation/objectivec/property_getattributes(_:)>):
///
/// > Returns the attribute string of a property.
/// > Return Value: A C string containing the property's attributes.
///
/// The format of this string is documented in "Declared Properties" of the
/// Objective-C Runtime Programming Guide
/// (<https://developer.apple.com/library/archive/documentation/Cocoa/Conceptual/ObjCRuntimeGuide/Articles/ocrtPropertyIntrospection.html>):
/// it begins with `T` followed by the `@encode` type, then comma-separated
/// attribute codes (e.g. `T@"NSString",&,N,V_name`).
///
/// We read the `attributes` field of the `property_t` entry parsed from the
/// app binary and return its guest pointer directly (the caller must NOT
/// free it), exactly like real libobjc. As with [property_getName], the
/// previous return-0 stub broke runtime introspection used by KVC and ad
/// SDK serialization in apps such as Spy Mouse HD.
pub fn property_getAttributes(env: &mut Environment, property: ConstVoidPtr) -> ConstPtr<u8> {
    if property.is_null() {
        return Ptr::null();
    }
    let prop_ptr: ConstPtr<property_t> = property.cast();
    let property_t { attributes, .. } = env.mem.read(prop_ptr);
    attributes
}

/// Logs a placeholder message for an unimplemented ObjC setter
///
/// This macro must be used inside [crate::_objc_method],
/// as it relies on constants for the current class and selector
/// set by it and [crate::objc::objc_classes]
#[macro_export]
macro_rules! todo_objc_setter {
    ($this:ident, $($arg:tt)+) => {
        const _: () = {
            let bytes = _OBJC_CURRENT_SELECTOR.as_bytes();
            let starts_with_set =
                bytes.len() > 3 && bytes[0] == b's' && bytes[1] == b'e' && bytes[2] == b't';
            assert!(starts_with_set, "Selector does not start with set.");
        };
        log!(
            "TODO: [({}*) {:?} {}:{:?}]",
            _OBJC_CURRENT_CLASS,
            $this,
            _OBJC_CURRENT_SELECTOR,
            $($arg)+
        );
    };
}
pub use crate::todo_objc_setter;

#[cfg(test)]
mod trainer_metadata_tests {
    use super::*;
    use crate::mem::PAGE_SIZE;
    use crate::objc::TrivialHostObject;

    #[test]
    fn exact_field_requires_registered_object_offset_width_and_scalar_type() {
        let mut mem = Mem::new();
        mem.set_null_segment_size(PAGE_SIZE);
        let mut objc = ObjC::new();
        let offset = mem.alloc_and_write(8u32).cast_const();
        let mut host = ClassHostObject {
            instance_size: 16,
            ..Default::default()
        };
        host.ivars.insert(
            "_coins".into(),
            IvarInfo {
                offset,
                alignment: 2,
                metadata: ConstVoidPtr::null(),
            },
        );
        host.scalar_ivars.insert("_coins".into(), (b'i', 4));
        let class = objc.alloc_static_object(nil, Box::new(host), &mut mem);
        let object = objc.alloc_object_sized(class, 16, Box::new(TrivialHostObject), &mut mem);
        let base = object.to_bits();
        assert_eq!(
            objc.diagnostic_scalar_field(&mem, base, 16, base + 8, 4, b'i'),
            Some("_coins")
        );
        assert_eq!(
            objc.diagnostic_scalar_field(&mem, base, 16, base + 9, 4, b'i'),
            None
        );
        assert_eq!(
            objc.diagnostic_scalar_field(&mem, base, 16, base + 8, 1, b'C'),
            None
        );
        assert_eq!(
            objc.diagnostic_scalar_field(&mem, base, 16, base + 8, 4, b'f'),
            None
        );
        assert_eq!(
            objc.diagnostic_scalar_field(&mem, base, 8, base + 8, 4, b'i'),
            None
        );
        let subclass = objc.alloc_static_object(
            nil,
            Box::new(ClassHostObject {
                superclass: class,
                instance_size: 16,
                ..Default::default()
            }),
            &mut mem,
        );
        let inherited = objc
            .alloc_object_sized(subclass, 16, Box::new(TrivialHostObject), &mut mem)
            .to_bits();
        assert_eq!(
            objc.diagnostic_scalar_field(&mem, inherited, 16, inherited + 8, 4, b'i'),
            Some("_coins")
        );
        // A corrupt superclass cycle is bounded instead of hanging the trainer.
        objc.borrow_mut::<ClassHostObject>(class).superclass = subclass;
        assert_eq!(
            objc.diagnostic_scalar_field(&mem, inherited, 16, inherited + 12, 4, b'i'),
            None
        );
        // Same isa bytes in an arbitrary allocation are NOT proof of an object.
        let raw = mem.alloc(16);
        mem.write(raw.cast(), class);
        assert_eq!(
            objc.diagnostic_scalar_field(&mem, raw.to_bits(), 16, raw.to_bits() + 8, 4, b'i'),
            None
        );
    }

    #[test]
    fn ivar_lookup_walks_superclasses_and_object_get_ivar_reads_guest_storage() {
        let mut mem = Mem::new();
        mem.set_null_segment_size(PAGE_SIZE);
        let mut objc = ObjC::new();
        let original_offset = mem.alloc_and_write(8u32).cast_const();
        let adjusted_offset = mem.alloc_and_write(12u32).cast_const();
        let name = mem.alloc_and_write_cstr(b"_context").cast_const();
        let type_ = mem.alloc_and_write_cstr(b"@").cast_const();
        let entry_size = guest_size_of::<ivar_t>();
        let list: MutPtr<ivar_list_t> = mem
            .alloc(guest_size_of::<ivar_list_t>() + entry_size)
            .cast();
        mem.write(
            list,
            ivar_list_t {
                entsize: entry_size,
                count: 1,
            },
        );
        mem.write(
            (list + 1).cast(),
            ivar_t {
                offset: original_offset,
                name,
                type_,
                alignment: 2,
                size: 4,
            },
        );
        let mut superclass_host = ClassHostObject {
            instance_size: 12,
            ..Default::default()
        };
        superclass_host.add_ivars_from_bin(list.cast_const(), &mem);
        let expected_ivar_metadata = superclass_host.ivars.get("_context").unwrap().metadata;
        superclass_host.ivars.get_mut("_context").unwrap().offset = adjusted_offset;
        let superclass = objc.alloc_static_object(nil, Box::new(superclass_host), &mut mem);
        let subclass = objc.alloc_static_object(
            nil,
            Box::new(ClassHostObject {
                superclass,
                instance_size: 16,
                ..Default::default()
            }),
            &mut mem,
        );
        let object = objc.alloc_object_sized(subclass, 16, Box::new(TrivialHostObject), &mut mem);
        let expected = id::from_bits(0x1234_5678);
        mem.write(MutPtr::<id>::from_bits(object.to_bits() + 12), expected);

        let ivar = objc.class_lookup_ivar(subclass, "_context").unwrap();
        assert_eq!(ivar.to_bits(), expected_ivar_metadata.to_bits());
        assert_eq!(object_ivar_value(&objc, &mem, object, ivar), Some(expected));
        assert_eq!(objc.class_lookup_ivar(subclass, "_missing"), None);
        assert_eq!(object_ivar_value(&objc, &mem, nil, ivar), None);
        assert_eq!(
            object_ivar_value(&objc, &mem, object, ConstVoidPtr::null()),
            None
        );
    }

    #[test]
    fn binary_metadata_rejects_pointer_fields_even_if_named_money() {
        let mut mem = Mem::new();
        mem.set_null_segment_size(PAGE_SIZE);
        let offset = mem.alloc_and_write(8u32).cast_const();
        let name = mem.alloc_and_write_cstr(b"_money").cast_const();
        let size = guest_size_of::<ivar_t>();
        let list: MutPtr<ivar_list_t> = mem.alloc(guest_size_of::<ivar_list_t>() + size).cast();
        mem.write(
            list,
            ivar_list_t {
                entsize: size,
                count: 1,
            },
        );
        let mut host = ClassHostObject::default();
        for (encoding, expected) in [
            (b"i".as_slice(), true),
            (b"@".as_slice(), false),
            (b"^i".as_slice(), false),
        ] {
            let type_ = mem.alloc_and_write_cstr(encoding).cast_const();
            mem.write(
                (list + 1).cast(),
                ivar_t {
                    offset,
                    name,
                    type_,
                    alignment: 2,
                    size: 4,
                },
            );
            host.add_ivars_from_bin(list.cast_const(), &mem);
            assert_eq!(host.scalar_ivars.contains_key("_money"), expected);
        }
    }
}
