/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Handling of Objective-C objects.

use super::{Class, ClassHostObject};
use crate::mem::{guest_size_of, GuestUSize, Mem, MutPtr, Ptr, SafeRead};
use std::any::Any;
use std::num::NonZeroU32;

#[repr(C, packed)]
pub struct objc_object {
    pub(super) isa: Class,
}
unsafe impl SafeRead for objc_object {}

#[allow(non_camel_case_types)]
pub type id = MutPtr<objc_object>;

#[allow(non_upper_case_globals)]
pub const nil: id = Ptr::null();

pub(super) struct HostObjectEntry {
    host_object: Box<dyn AnyHostObject>,
    refcount: Option<NonZeroU32>,
}

pub trait HostObject: Any + 'static {
    fn as_superclass<'a>(&'a self) -> Option<&'a (dyn AnyHostObject + 'static)> {
        None
    }
    fn as_superclass_mut<'a>(&'a mut self) -> Option<&'a mut (dyn AnyHostObject + 'static)> {
        None
    }
}

#[macro_export]
macro_rules! impl_HostObject_with_superclass {
    ( $ty:ty ) => {
        impl $crate::objc::HostObject for $ty {
            fn as_superclass<'a>(
                &'a self,
            ) -> Option<&'a (dyn $crate::objc::AnyHostObject + 'static)> {
                Some(&self.superclass)
            }
            fn as_superclass_mut<'a>(
                &'a mut self,
            ) -> Option<&'a mut (dyn $crate::objc::AnyHostObject + 'static)> {
                Some(&mut self.superclass)
            }
        }
    };
}
pub use crate::impl_HostObject_with_superclass;

pub trait AnyHostObject: HostObject {
    fn as_any<'a>(&'a self) -> &'a (dyn Any + 'static);
    fn as_any_mut<'a>(&'a mut self) -> &'a mut (dyn Any + 'static);
    fn type_name(&self) -> &'static str;
}
impl<T: HostObject> AnyHostObject for T {
    fn as_any<'a>(&'a self) -> &'a (dyn Any + 'static) {
        self
    }
    fn as_any_mut<'a>(&'a mut self) -> &'a mut (dyn Any + 'static) {
        self
    }
    fn type_name(&self) -> &'static str {
        std::any::type_name::<T>()
    }
}

pub struct TrivialHostObject;
impl HostObject for TrivialHostObject {}

impl super::ObjC {
    pub fn read_isa(object: id, mem: &Mem) -> Class {
        if object == nil {
            return Ptr::null();
        }
        mem.read(object).isa
    }

    fn alloc_object_inner(
        &mut self,
        isa: Class,
        instance_size: GuestUSize,
        host_object: Box<dyn AnyHostObject>,
        mem: &mut Mem,
        refcount: Option<NonZeroU32>,
    ) -> id {
        let guest_object = objc_object { isa };
        let ptr: MutPtr<objc_object> = mem.alloc(instance_size).cast();
        mem.write(ptr, guest_object);
        self.objects.insert(
            ptr,
            HostObjectEntry {
                host_object,
                refcount,
            },
        );
        ptr
    }

    pub fn alloc_object(
        &mut self,
        isa: Class,
        host_object: Box<dyn AnyHostObject>,
        mem: &mut Mem,
    ) -> id {
        let instance_size = self.get_host_object(isa)
            .and_then(|h| h.as_any().downcast_ref::<ClassHostObject>())
            .map(|c| c.instance_size)
            .unwrap_or(guest_size_of::<objc_object>());

        self.alloc_object_inner(
            isa,
            instance_size,
            host_object,
            mem,
            Some(NonZeroU32::new(1).unwrap()),
        )
    }

    pub fn alloc_static_object(
        &mut self,
        isa: Class,
        host_object: Box<dyn AnyHostObject>,
        mem: &mut Mem,
    ) -> id {
        let size = guest_size_of::<objc_object>();
        self.alloc_object_inner(isa, size, host_object, mem, None)
    }

    pub fn register_static_object(
        &mut self,
        guest_object: id,
        host_object: Box<dyn AnyHostObject>,
    ) {
        if guest_object == nil { return; }
        self.objects.insert(
            guest_object,
            HostObjectEntry {
                host_object,
                refcount: None,
            },
        );
    }

    pub fn get_host_object(&self, object: id) -> Option<&dyn AnyHostObject> {
        if object == nil { return None; }
        self.objects.get(&object).map(|entry| &*entry.host_object)
    }

    pub fn borrow<T: AnyHostObject + 'static>(&self, object: id) -> &T {
        if let Some(entry) = self.objects.get(&object) {
            let mut host_object: &(dyn AnyHostObject + 'static) = &*entry.host_object;
            loop {
                if let Some(res) = host_object.as_any().downcast_ref() {
                    return res;
                } else if let Some(next) = host_object.as_superclass() {
                    host_object = next;
                } else {
                    break;
                }
            }
        }

        // SUPER HACK: Вместо паники создаем "мираж" объекта в памяти
        log!("Warning: SUPER HACK! Faking borrow for missing object {:?} of type {}", object, std::any::type_name::<T>());
        unsafe {
            static mut DUMMY_BUF: [u64; 256] = [0; 256];
            & *(&DUMMY_BUF as *const _ as *const T)
        }
    }

    pub fn borrow_mut<T: AnyHostObject + 'static>(&mut self, object: id) -> &mut T {
        if let Some(entry) = self.objects.get_mut(&object) {
            type Aho = dyn AnyHostObject + 'static;
            let mut host_object: &mut Aho = &mut *entry.host_object;
            loop {
                let current_ptr = host_object as *mut Aho;
                if let Some(res) = unsafe { &mut *current_ptr }.as_any_mut().downcast_mut() {
                    return res;
                }
                
                let has_super = unsafe { &*current_ptr }.as_superclass().is_some();
                if has_super {
                    host_object = unsafe { &mut *current_ptr }.as_superclass_mut().unwrap();
                } else {
                    break;
                }
            }
        }
        
        // SUPER HACK: Возвращаем кусок нулей под видом нужного объекта
        log!("Warning: SUPER HACK! Faking borrow_mut for missing object {:?} of type {}", object, std::any::type_name::<T>());
        unsafe {
            static mut DUMMY_BUF: [u64; 256] = [0; 256];
            &mut *(&mut DUMMY_BUF as *mut _ as *mut T)
        }
    }

    pub fn get_refcount(&mut self, object: id) -> NonZeroU32 {
        let default_rc = NonZeroU32::new(1).unwrap();
        if object == nil { return default_rc; }
        
        self.objects.get(&object)
            .and_then(|e| e.refcount)
            .unwrap_or(default_rc)
    }

    pub fn increment_refcount(&mut self, object: id) {
        if object == nil { return; }
        if let Some(entry) = self.objects.get_mut(&object) {
            if let Some(refcount) = entry.refcount.as_mut() {
                if let Some(new_rc) = refcount.get().checked_add(1) {
                    *refcount = NonZeroU32::new(new_rc).unwrap();
                }
            }
        }
    }

    #[must_use]
    pub fn decrement_refcount(&mut self, object: id) -> bool {
        if object == nil { return false; }
        if let Some(entry) = self.objects.get_mut(&object) {
            if let Some(refcount) = entry.refcount.as_mut() {
                if refcount.get() == 1 {
                    entry.refcount = None;
                    return true;
                } else {
                    *refcount = NonZeroU32::new(refcount.get() - 1).unwrap();
                }
            }
        }
        false
    }

    pub fn dealloc_object(&mut self, object: id, mem: &mut Mem) {
        if object == nil { return; }
        
        if let Some(entry) = self.objects.remove(&object) {
            std::mem::drop(entry.host_object);
            mem.free(object.cast());
        }
    }
}
