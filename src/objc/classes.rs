/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Handling of Objective-C classes and metaclasses.

use super::{
    id, ivar_list_t, method_list_t, nil, objc_object, AnyHostObject, HostIMP, HostObject, ObjC,
    IMP, SEL,
};
use crate::mach_o::MachO;
use crate::mem::{guest_size_of, ConstPtr, ConstVoidPtr, GuestUSize, Mem, Ptr, SafeRead};
use std::collections::{HashMap, VecDeque};

pub type Class = id;

pub(super) struct ClassHostObject {
    pub(super) name: String,
    pub(super) is_metaclass: bool,
    pub(super) superclass: Class,
    pub(super) methods: HashMap<SEL, IMP>,
    pub(super) ivars: HashMap<String, (ConstPtr<GuestUSize>, u32)>,
    pub(super) instance_start: GuestUSize,
    pub(super) instance_size: GuestUSize,
}
impl HostObject for ClassHostObject {}

pub(super) struct UnimplementedClass {
    pub(super) name: String,
    pub(super) is_metaclass: bool,
}
impl HostObject for UnimplementedClass {}

pub(super) struct FakeClass {
    pub(super) name: String,
    pub(super) is_metaclass: bool,
}
impl HostObject for FakeClass {}

#[repr(C, packed)]
#[allow(dead_code)]
struct class_t {
    isa: Class,
    superclass: Class,
    _cache: ConstVoidPtr,
    _vtable: ConstVoidPtr,
    data: ConstPtr<class_rw_t>,
}
unsafe impl SafeRead for class_t {}

#[repr(C, packed)]
#[allow(dead_code)]
struct class_rw_t {
    _flags: u32,
    instance_start: GuestUSize,
    instance_size: GuestUSize,
    _reserved: u32,
    name: ConstPtr<u8>,
    base_methods: ConstPtr<method_list_t>,
    _base_protocols: ConstVoidPtr,
    ivars: ConstPtr<ivar_list_t>,
    _weak_ivar_layout: u32,
    _base_properties: ConstVoidPtr,
}
unsafe impl SafeRead for class_rw_t {}

#[repr(C, packed)]
struct category_t {
    name: ConstPtr<u8>,
    class: Class,
    instance_methods: ConstPtr<method_list_t>,
    class_methods: ConstPtr<method_list_t>,
    _protocols: ConstVoidPtr,
    _property_list: ConstVoidPtr,
}
unsafe impl SafeRead for category_t {}

pub struct ClassTemplate {
    pub name: &'static str,
    pub superclass: Option<&'static str>,
    pub class_methods: &'static [(&'static str, &'static dyn HostIMP)],
    pub instance_methods: &'static [(&'static str, &'static dyn HostIMP)],
}

pub type ClassExports = &'static [(&'static str, ClassTemplate)];

#[doc(hidden)]
#[macro_export]
macro_rules! _objc_superclass {
    (: $name:ident) => {
        Some(stringify!($name))
    };
    () => {
        None
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! _objc_method {
    (
        $env:ident,
        $this:ident,
        $_cmd:ident,
        $cmd_name:ident,
        $retty:ty,
        $block:block
        $(, $ty:ty, $arg:ident)*
        $(, ...$va_arg:ident: $va_type:ty)?
    ) => {
        &((|
            #[allow(unused_variables)]
            $env: &mut $crate::Environment,
            #[allow(unused_variables)]
            $this: $crate::objc::id,
            #[allow(unused_variables)]
            $_cmd: $crate::objc::SEL,
            $($arg: $ty,)*
            $(#[allow(unused_mut)] mut $va_arg: $va_type,)?
        | -> $retty {
            const _OBJC_CURRENT_SELECTOR: &str = stringify!($cmd_name);
            $block
        }) as fn(
            &mut $crate::Environment,
            $crate::objc::id,
            $crate::objc::SEL,
            $($ty,)*
            $($va_type,)?
        ) -> $retty)
    }
}

#[macro_export]
macro_rules! objc_classes {
    {
        ($env:ident, $this:ident, $_cmd:ident);
        $(
            @implementation $class_name:ident $(: $superclass_name:ident)?
            $( + ($cm_type:ty) $cm_name:ident $(:($cm_type1:ty) $cm_arg1:ident $($($cm_namen:ident)?:($cm_typen:ty) $cm_argn:ident)*)?
                              $(, ...$cm_va_arg:ident)?
                 $cm_block:block )*
            $( - ($im_type:ty) $im_name:ident $(:($im_type1:ty) $im_arg1:ident $($($im_namen:ident)?:($im_typen:ty) $im_argn:ident)*)?
                              $(, ...$im_va_arg:ident)?
                 $im_block:block )*
            @end
        )+
    } => {
        &[
            $({
                const _OBJC_CURRENT_CLASS: &str = stringify!($class_name);
                (_OBJC_CURRENT_CLASS, $crate::objc::ClassTemplate {
                    name: _OBJC_CURRENT_CLASS,
                    superclass: $crate::_objc_superclass!($(: $superclass_name)?),
                    class_methods: &[
                        $(
                            (
                                $crate::objc::selector!(
                                    $(($cm_type1);)?
                                    $cm_name
                                    $($(, $($cm_namen)?)*)?
                                ),
                                $crate::_objc_method!(
                                    $env,
                                    $this,
                                    $_cmd,
                                    $cm_name,
                                    $cm_type,
                                    { $cm_block }
                                    $(, $cm_type1, $cm_arg1 $(, $cm_typen, $cm_argn)*)?
                                    $(, ...$cm_va_arg: $crate::abi::DotDotDot)?
                                )
                            )
                        ),*
                    ],
                    instance_methods: &[
                        $(
                            (
                                $crate::objc::selector!(
                                    $(($im_type1);)?
                                    $im_name
                                    $($(, $($im_namen)?)*)?
                                ),
                                $crate::_objc_method!(
                                    $env,
                                    $this,
                                    $_cmd,
                                    $im_name,
                                    $im_type,
                                    { $im_block }
                                    $(, $im_type1, $im_arg1 $(, $im_typen, $im_argn)*)?
                                    $(, ...$im_va_arg: $crate::abi::DotDotDot)?
                                )
                            )
                        ),*
                    ],
                })
            }),+
        ]
    }
}
pub use crate::objc_classes;

impl ClassHostObject {
    fn from_template(
        template: &ClassTemplate,
        is_metaclass: bool,
        superclass: Class,
        objc: &ObjC,
    ) -> Self {
        let size = guest_size_of::<objc_object>();
        ClassHostObject {
            name: template.name.to_string(),
            is_metaclass,
            superclass,
            methods: HashMap::from_iter(
                (if is_metaclass {
                    template.class_methods
                } else {
                    template.instance_methods
                })
                .iter()
                .map(|&(name, host_imp)| {
                    (objc.selectors[name], IMP::Host(host_imp))
                }),
            ),
            instance_start: size,
            instance_size: size,
            ivars: HashMap::default(),
        }
    }

    fn from_bin(class: Class, is_metaclass: bool, mem: &Mem, objc: &mut ObjC) -> Self {
        let class_t {
            superclass, data, ..
        } = mem.read(class.cast());
        let class_rw_t {
            instance_start,
            instance_size,
            name,
            base_methods,
            ivars,
            ..
        } = mem.read(data);

        let name = mem.cstr_at_utf8(name).unwrap().to_string();

        let mut host_object = ClassHostObject {
            name,
            is_metaclass,
            superclass,
            methods: HashMap::new(),
            instance_start,
            instance_size,
            ivars: HashMap::new(),
        };

        if !base_methods.is_null() {
            host_object.add_methods_from_bin(base_methods, mem, objc);
        }

        if !ivars.is_null() {
            host_object.add_ivars_from_bin(ivars, mem);
        }

        host_object
    }
}

fn substitute_classes(
    mem: &Mem,
    class: Class,
    metaclass: Class,
) -> Option<(Box<FakeClass>, Box<FakeClass>)> {
    let class_t { data, .. } = mem.read(class.cast());
    let class_rw_t { name, .. } = mem.read(data);
    let name = mem.cstr_at_utf8(name).unwrap();

    if !(name.starts_with("AdMob")
        || name.starts_with("AltAds")
        || name.starts_with("Mobclix")
        || name.starts_with("FB")
        || name.starts_with("Flurry")
        || name.starts_with("OpenFeint")
        || name.starts_with("Tapjoy")
        || name.starts_with("UA"))
    {
        return None;
    }

    {
        let class_t { data, .. } = mem.read(metaclass.cast());
        let class_rw_t {
            name: metaclass_name,
            ..
        } = mem.read(data);
        let metaclass_name = mem.cstr_at_utf8(metaclass_name).unwrap();
        assert!(name == metaclass_name);
    }

    log!(
        "Note: substituting fake class for {} to improve compatibility",
        name
    );

    let class_host_object = Box::new(FakeClass {
        name: name.to_string(),
        is_metaclass: false,
    });
    let metaclass_host_object = Box::new(FakeClass {
        name: name.to_string(),
        is_metaclass: true,
    });
    Some((class_host_object, metaclass_host_object))
}

impl ObjC {
    fn get_class(&self, name: &str, is_metaclass: bool, mem: &Mem) -> Option<Class> {
        let class = self.classes.get(name).copied()?;
        Some(if is_metaclass {
            Self::read_isa(class, mem)
        } else {
            class
        })
    }

    fn find_template(name: &str) -> Option<&'static ClassTemplate> {
        crate::dyld::search_host_dylibs(|dylib| dylib.class_exports, name)
            .map(|&(_name, ref template)| template)
    }

    pub fn link_class(&mut self, name: &str, is_metaclass: bool, mem: &mut Mem) -> Class {
        self.link_class_inner(name, is_metaclass, mem, true)
    }

    pub fn get_known_class(&mut self, name: &str, mem: &mut Mem) -> Class {
        self.link_class_inner(name, /* is_metaclass: */ false, mem, false)
    }

    fn link_class_inner(
        &mut self,
        name: &str,
        is_metaclass: bool,
        mem: &mut Mem,
        use_placeholder: bool,
    ) -> Class {
        if let Some(class) = self.get_class(name, is_metaclass, mem) {
            return class;
        };

        let class_host_object: Box<dyn AnyHostObject>;
        let metaclass_host_object: Box<dyn AnyHostObject>;
        if let Some(template) = Self::find_template(name) {
            if let Some(superclass_name) = template.superclass {
                assert!(Self::find_template(superclass_name).is_some());
            }

            class_host_object = Box::new(ClassHostObject::from_template(
                template,
                false,
                template
                    .superclass
                    .map(|name| self.link_class(name, false, mem))
                    .unwrap_or(nil),
                self,
            ));
            metaclass_host_object = Box::new(ClassHostObject::from_template(
                template,
                true,
                template
                    .superclass
                    .map(|name| self.link_class(name, true, mem))
                    .unwrap_or(nil),
                self,
            ));
        } else {
            if !use_placeholder {
                panic!("Missing implementation for class {name}!");
            }

            class_host_object = Box::new(UnimplementedClass {
                name: name.to_string(),
                is_metaclass: false,
            });
            metaclass_host_object = Box::new(UnimplementedClass {
                name: name.to_string(),
                is_metaclass: true,
            });
        }

        let metaclass = if name == "NSObject" {
            let metaclass = mem.alloc_and_write(objc_object { isa: nil });
            mem.write(metaclass, objc_object { isa: metaclass });
            self.register_static_object(metaclass, metaclass_host_object);
            metaclass
        } else {
            let isa = self.link_class("NSObject", true, mem);
            self.alloc_static_object(isa, metaclass_host_object, mem)
        };

        let class = self.alloc_static_object(metaclass, class_host_object, mem);

        if name == "NSObject" {
            self.borrow_mut::<ClassHostObject>(metaclass).superclass = class;
        }

        self.classes.insert(name.to_string(), class);

        if is_metaclass {
            metaclass
        } else {
            class
        }
    }

    pub fn register_bin_classes(&mut self, bin: &MachO, mem: &mut Mem) {
        let Some(list) = bin.get_section("__objc_classlist") else {
            return;
        };

        assert!(list.size % 4 == 0);
        let base: ConstPtr<Class> = Ptr::from_bits(list.addr);
        for i in 0..(list.size / 4) {
            let class = mem.read(base + i);
            let metaclass = Self::read_isa(class, mem);

            let name = if let Some(fakes) = substitute_classes(mem, class, metaclass) {
                let (class_host_object, metaclass_host_object) = fakes;
                let name = class_host_object.name.clone();
                self.register_static_object(class, class_host_object);
                self.register_static_object(metaclass, metaclass_host_object);
                name
            } else {
                let class_host_object = Box::new(ClassHostObject::from_bin(
                    class, false, mem, self,
                ));
                let metaclass_host_object = Box::new(ClassHostObject::from_bin(
                    metaclass, true, mem, self,
                ));
                let name = class_host_object.name.clone();
                self.register_static_object(class, class_host_object);
                self.register_static_object(metaclass, metaclass_host_object);
                name
            };

            self.classes.insert(name.to_string(), class);
        }

        let mut queue = VecDeque::<Class>::new();
        let mut found_ns_object = false;
        let mut inverted_inheritance = HashMap::<Class, Vec<Class>>::new();
        for (name, class) in self.classes.iter() {
            if name == "NSObject" {
                assert!(!found_ns_object);
                found_ns_object = true;
            }
            let class_host_object = self
                .get_host_object(*class)
                .unwrap()
                .as_any()
                .downcast_ref();
            let Some(ClassHostObject { superclass, .. }) = class_host_object else {
                continue;
            };

            if *superclass != nil {
                inverted_inheritance
                    .entry(*superclass)
                    .and_modify(|v| v.push(*class))
                    .or_insert(vec![*class]);
            } else {
                queue.push_back(*class);
            }
        }
        assert!(found_ns_object);

        while !queue.is_empty() {
            let next = queue.pop_front().unwrap();
            let (need, mut diff) = self.need_ivar_reconciliation(next);
            if need {
                let ClassHostObject {
                    ref mut instance_start,
                    ref mut instance_size,
                    ref mut ivars,
                    ..
                } = self.borrow_mut(next);

                if !ivars.is_empty() {
                    let mut max_alignment: u32 = 1;
                    for (offset, align) in ivars.values() {
                        if !offset.is_null() {
                            max_alignment = max_alignment.max(*align);
                        }
                    }

                    let align_mask = max_alignment - 1;
                    diff = (diff + align_mask) & !align_mask;

                    for (offset, _) in ivars.values_mut() {
                        if !offset.is_null() {
                            *offset = Ptr::from_bits((*offset).to_bits() + diff);
                        }
                    }
                }

                *instance_start += diff;
                *instance_size += diff;
            }
            if let Some(subclasses) = inverted_inheritance.get(&next) {
                queue.extend(subclasses);
            }
        }
    }

    fn need_ivar_reconciliation(&mut self, class: Class) -> (bool, u32) {
        let host_obj = self.get_host_object(class).unwrap().as_any();
        let Some(ClassHostObject {
            superclass,
            instance_start,
            ..
        }) = host_obj.downcast_ref::<ClassHostObject>()
        else {
            return (false, 0);
        };

        if *superclass == nil {
            return (false, 0);
        }

        let super_host = self.get_host_object(*superclass).unwrap().as_any();
        let Some(ClassHostObject {
            instance_size: super_size,
            ..
        }) = super_host.downcast_ref::<ClassHostObject>()
        else {
            return (false, 0);
        };

        let need = instance_start < super_size;
        let diff = if need { super_size - instance_start } else { 0 };
        (need, diff)
    }

    pub fn dump_classes(&self, file: &mut std::fs::File) -> Result<(), std::io::Error> {
        use std::io::Write;
        writeln!(file, "{{\n    \"object\": \"classes\",\n    \"classes\": [")?;
        for (i, (_, o)) in self.classes.iter().enumerate() {
            let comma = if i == self.classes.len() - 1 { "" } else { "," };
            let host_obj = self.get_host_object(*o).unwrap();

            if let Some(ClassHostObject { name, superclass: sup, .. }) =
                host_obj.as_any().downcast_ref()
            {
                if *sup == nil {
                    writeln!(
                        file,
                        "        {{ \"name\": \"{name}\", \
                         \"class_type\": \"normal\" }}{comma}"
                    )?;
                } else {
                    writeln!(
                        file,
                        "        {{ \"name\": \"{}\", \"super\": \"{}\", \
                         \"class_type\": \"normal\" }}{}",
                        name,
                        self.get_class_name(*sup),
                        comma
                    )?;
                }
            } else if let Some(UnimplementedClass { name, .. }) =
                host_obj.as_any().downcast_ref()
            {
                writeln!(
                    file,
                    "        {{ \"name\": \"{name}\", \
                     \"class_type\": \"unimplemented\" }}{comma}"
                )?;
            } else if let Some(FakeClass { name, .. }) = host_obj.as_any().downcast_ref() {
                writeln!(
                    file,
                    "        {{ \"name\": \"{name}\", \
                     \"class_type\": \"fake\" }}{comma}"
                )?;
            }
        }
        writeln!(file, "    ]\n}}")
    }

    pub fn register_bin_categories(&mut self, bin: &MachO, mem: &mut Mem) {
        let Some(list) = bin.get_section("__objc_catlist") else {
            return;
        };
        assert!(list.size % 4 == 0);
        let base: ConstPtr<ConstPtr<category_t>> = Ptr::from_bits(list.addr);
        for i in 0..(list.size / 4) {
            let cat_ptr = mem.read(base + i);
            let data = mem.read(cat_ptr);
            let class = data.class;
            let metaclass = Self::read_isa(class, mem);

            for (target_class, methods) in [
                (class, data.instance_methods),
                (metaclass, data.class_methods),
            ] {
                if methods.is_null() {
                    continue;
                }
                let any = self.get_host_object(target_class).unwrap().as_any();
                if any.is::<FakeClass>() || any.is::<UnimplementedClass>() {
                    continue;
                }

                let mut host_obj = std::mem::replace(
                    self.borrow_mut::<ClassHostObject>(target_class),
                    ClassHostObject {
                        name: Default::default(),
                        is_metaclass: Default::default(),
                        superclass: nil,
                        methods: Default::default(),
                        instance_start: Default::default(),
                        instance_size: Default::default(),
                        ivars: Default::default(),
                    },
                );
                host_obj.add_methods_from_bin(methods, mem, self);
                *self.borrow_mut::<ClassHostObject>(target_class) = host_obj;
            }
        }
    }

    pub fn class_is_subclass_of(&self, class: Class, superclass: Class) -> bool {
        if class == superclass {
            return true;
        }
        let mut curr = class;
        loop {
            let &ClassHostObject { superclass: next, .. } = self.borrow(curr);
            if next == nil {
                return false;
            }
            if next == superclass {
                return true;
            }
            curr = next;
        }
    }

    pub fn get_class_name(&self, class: Class) -> &str {
        self.try_get_class_name(class).expect("Could not get class name!")
    }

    pub fn get_superclass(&self, class: Class) -> Class {
        let &ClassHostObject { superclass, .. } = self.borrow(class);
        superclass
    }

    pub fn try_get_class_name(&self, class: Class) -> Option<&str> {
        let host_object = self.get_host_object(class)?;
        let any = host_object.as_any();
        if let Some(c) = any.downcast_ref::<ClassHostObject>() {
            Some(&c.name)
        } else if let Some(u) = any.downcast_ref::<UnimplementedClass>() {
            Some(&u.name)
        } else if let Some(f) = any.downcast_ref::<FakeClass>() {
            Some(&f.name)
        } else {
            None
        }
    }
}

pub fn objc_getClass(env: &mut crate::Environment, name: ConstPtr<u8>) -> Class {
    if name.is_null() {
        return nil;
    }
    
    // Читаем C-строку имени класса из памяти гостя и КОПИРУЕМ её в String (.to_string()),
    // чтобы освободить неизменяемое заимствование (borrow) env.mem.
    let name_str = match env.mem.cstr_at_utf8(name) {
        Ok(s) => s.to_string(),
        Err(_) => return nil,
    };

    // Проверяем, зарегистрирован ли уже этот класс (среди известных)
    // Обратите внимание на амперсанд & перед name_str
    if let Some(class) = env.objc.get_class(&name_str, false, &env.mem) {
        return class;
    }

    // Если класса еще нет, но у нас есть для него хост-шаблон, линкуем его
    if ObjC::find_template(&name_str).is_some() {
        return env.objc.link_class(&name_str, false, &mut env.mem);
    }

    // Стандартное поведение objc_getClass: если класс не найден, возвращаем nil
    nil
}

pub fn objc_begin_catch(env: &mut crate::Environment, name: ConstPtr<u8>) -> Class {
    if name.is_null() {
        return nil;
    }
    
    // Читаем C-строку имени класса из памяти гостя и КОПИРУЕМ её в String (.to_string()),
    // чтобы освободить неизменяемое заимствование (borrow) env.mem.
    let name_str = match env.mem.cstr_at_utf8(name) {
        Ok(s) => s.to_string(),
        Err(_) => return nil,
    };

    // Проверяем, зарегистрирован ли уже этот класс (среди известных)
    // Обратите внимание на амперсанд & перед name_str
    if let Some(class) = env.objc.get_class(&name_str, false, &env.mem) {
        return class;
    }

    // Если класса еще нет, но у нас есть для него хост-шаблон, линкуем его
    if ObjC::find_template(&name_str).is_some() {
        return env.objc.link_class(&name_str, false, &mut env.mem);
    }

    // Стандартное поведение objc_getClass: если класс не найден, возвращаем nil
    nil
}

pub fn objc_end_catch(env: &mut crate::Environment, name: ConstPtr<u8>) -> Class {
    if name.is_null() {
        return nil;
    }
    
    // Читаем C-строку имени класса из памяти гостя и КОПИРУЕМ её в String (.to_string()),
    // чтобы освободить неизменяемое заимствование (borrow) env.mem.
    let name_str = match env.mem.cstr_at_utf8(name) {
        Ok(s) => s.to_string(),
        Err(_) => return nil,
    };

    // Проверяем, зарегистрирован ли уже этот класс (среди известных)
    // Обратите внимание на амперсанд & перед name_str
    if let Some(class) = env.objc.get_class(&name_str, false, &env.mem) {
        return class;
    }

    // Если класса еще нет, но у нас есть для него хост-шаблон, линкуем его
    if ObjC::find_template(&name_str).is_some() {
        return env.objc.link_class(&name_str, false, &mut env.mem);
    }

    // Стандартное поведение objc_getClass: если класс не найден, возвращаем nil
    nil
}

pub fn objc_exception_throw(env: &mut crate::Environment, name: ConstPtr<u8>) -> Class {
    if name.is_null() {
        return nil;
    }
    
    // Читаем C-строку имени класса из памяти гостя и КОПИРУЕМ её в String (.to_string()),
    // чтобы освободить неизменяемое заимствование (borrow) env.mem.
    let name_str = match env.mem.cstr_at_utf8(name) {
        Ok(s) => s.to_string(),
        Err(_) => return nil,
    };

    // Проверяем, зарегистрирован ли уже этот класс (среди известных)
    // Обратите внимание на амперсанд & перед name_str
    if let Some(class) = env.objc.get_class(&name_str, false, &env.mem) {
        return class;
    }

    // Если класса еще нет, но у нас есть для него хост-шаблон, линкуем его
    if ObjC::find_template(&name_str).is_some() {
        return env.objc.link_class(&name_str, false, &mut env.mem);
    }

    // Стандартное поведение objc_getClass: если класс не найден, возвращаем nil
    nil
}

pub fn object_getClassName(env: &mut crate::Environment, obj: id) -> Class {
    if obj.is_null() {
        return nil;
    }
    
    // В Objective-C любой объект в памяти (id) начинается с указателя isa.
    // Приводим указатель, читаем из памяти гостя структуру objc_object 
    // и просто возвращаем её поле isa.
    let objc_obj: objc_object = env.mem.read(obj.cast());
    objc_obj.isa
}

pub fn object_getClass(env: &mut crate::Environment, obj: id) -> Class {
    if obj.is_null() {
        return nil;
    }
    
    // В Objective-C любой объект в памяти (id) начинается с указателя isa.
    // Приводим указатель, читаем из памяти гостя структуру objc_object 
    // и просто возвращаем её поле isa.
    let objc_obj: objc_object = env.mem.read(obj.cast());
    objc_obj.isa
}

pub fn objc_retainAutoreleasedReturnValue(env: &mut crate::Environment, name: ConstPtr<u8>) -> Class {
    if name.is_null() {
        return nil;
    }
    
    // Читаем C-строку имени класса из памяти гостя и КОПИРУЕМ её в String (.to_string()),
    // чтобы освободить неизменяемое заимствование (borrow) env.mem.
    let name_str = match env.mem.cstr_at_utf8(name) {
        Ok(s) => s.to_string(),
        Err(_) => return nil,
    };

    // Проверяем, зарегистрирован ли уже этот класс (среди известных)
    // Обратите внимание на амперсанд & перед name_str
    if let Some(class) = env.objc.get_class(&name_str, false, &env.mem) {
        return class;
    }

    // Если класса еще нет, но у нас есть для него хост-шаблон, линкуем его
    if ObjC::find_template(&name_str).is_some() {
        return env.objc.link_class(&name_str, false, &mut env.mem);
    }

    // Стандартное поведение objc_getClass: если класс не найден, возвращаем nil
    nil
}

pub fn objc_autoreleasePoolPush(env: &mut crate::Environment, name: ConstPtr<u8>) -> Class {
    if name.is_null() {
        return nil;
    }
    
    // Читаем C-строку имени класса из памяти гостя и КОПИРУЕМ её в String (.to_string()),
    // чтобы освободить неизменяемое заимствование (borrow) env.mem.
    let name_str = match env.mem.cstr_at_utf8(name) {
        Ok(s) => s.to_string(),
        Err(_) => return nil,
    };

    // Проверяем, зарегистрирован ли уже этот класс (среди известных)
    // Обратите внимание на амперсанд & перед name_str
    if let Some(class) = env.objc.get_class(&name_str, false, &env.mem) {
        return class;
    }

    // Если класса еще нет, но у нас есть для него хост-шаблон, линкуем его
    if ObjC::find_template(&name_str).is_some() {
        return env.objc.link_class(&name_str, false, &mut env.mem);
    }

    // Стандартное поведение objc_getClass: если класс не найден, возвращаем nil
    nil
}

pub fn objc_setProperty_nonatomic(env: &mut crate::Environment, name: ConstPtr<u8>) -> Class {
    if name.is_null() {
        return nil;
    }
    
    // Читаем C-строку имени класса из памяти гостя и КОПИРУЕМ её в String (.to_string()),
    // чтобы освободить неизменяемое заимствование (borrow) env.mem.
    let name_str = match env.mem.cstr_at_utf8(name) {
        Ok(s) => s.to_string(),
        Err(_) => return nil,
    };

    // Проверяем, зарегистрирован ли уже этот класс (среди известных)
    // Обратите внимание на амперсанд & перед name_str
    if let Some(class) = env.objc.get_class(&name_str, false, &env.mem) {
        return class;
    }

    // Если класса еще нет, но у нас есть для него хост-шаблон, линкуем его
    if ObjC::find_template(&name_str).is_some() {
        return env.objc.link_class(&name_str, false, &mut env.mem);
    }

    // Стандартное поведение objc_getClass: если класс не найден, возвращаем nil
    nil
}

pub fn class_getSuperclass(env: &mut crate::Environment, cls: Class) -> Class {
    // По спецификации Objective-C, если передать nil, возвращается nil.
    if cls.is_null() {
        return nil;
    }
    
    // В touchHLE уже есть внутренняя функция для получения суперкласса, 
    // просто проксируем вызов в неё:
    env.objc.get_superclass(cls)
}

pub fn class_getInstanceSize(env: &mut crate::Environment, cls: Class, name: SEL) -> ConstVoidPtr {
    // Согласно спецификации Objective-C, если передан nil класс, возвращаем nil
    if cls.is_null() {
        return ConstVoidPtr::null();
    }

    let mut curr = cls;
    while !curr.is_null() {
        // Достаём хост-объект класса, чтобы получить доступ к списку методов
        if let Some(host_obj) = env.objc.get_host_object(curr) {
            if let Some(class_obj) = host_obj.as_any().downcast_ref::<ClassHostObject>() {
                // Если селектор зарегистрирован в данном классе
                if class_obj.methods.contains_key(&name) {
                    // Возвращаем non-null указатель. 
                    // В реальном iOS/macOS это указатель на структуру `method_t`.
                    // Но поскольку touchHLE абстрагирует методы в хостовый `HashMap`
                    // и не выделяет под них память в гостевом пространстве, 
                    // возврат указателя на сам класс — это безопасный способ дать 
                    // приложению "валидный" (читаемый) адрес, означающий, что метод существует.
                    return curr.cast_const().cast();
                }
            }
        }
        
        // Поднимаемся вверх по иерархии к суперклассу
        let next = env.objc.get_superclass(curr);
        
        // Защита от бесконечного цикла, если иерархия зациклена
        if next == curr {
            break;
        }
        curr = next;
    }

    // Если прошли всю цепочку и ничего не нашли, метод не существует
    ConstVoidPtr::null()
}

pub fn class_getInstanceMethod(env: &mut crate::Environment, cls: Class, name: SEL) -> ConstVoidPtr {
    // Согласно спецификации Objective-C, если передан nil класс, возвращаем nil
    if cls.is_null() {
        return ConstVoidPtr::null();
    }

    let mut curr = cls;
    while !curr.is_null() {
        // Достаём хост-объект класса, чтобы получить доступ к списку методов
        if let Some(host_obj) = env.objc.get_host_object(curr) {
            if let Some(class_obj) = host_obj.as_any().downcast_ref::<ClassHostObject>() {
                // Если селектор зарегистрирован в данном классе
                if class_obj.methods.contains_key(&name) {
                    // Возвращаем non-null указатель. 
                    // В реальном iOS/macOS это указатель на структуру `method_t`.
                    // Но поскольку touchHLE абстрагирует методы в хостовый `HashMap`
                    // и не выделяет под них память в гостевом пространстве, 
                    // возврат указателя на сам класс — это безопасный способ дать 
                    // приложению "валидный" (читаемый) адрес, означающий, что метод существует.
                    return curr.cast_const().cast();
                }
            }
        }
        
        // Поднимаемся вверх по иерархии к суперклассу
        let next = env.objc.get_superclass(curr);
        
        // Защита от бесконечного цикла, если иерархия зациклена
        if next == curr {
            break;
        }
        curr = next;
    }

    // Если прошли всю цепочку и ничего не нашли, метод не существует
    ConstVoidPtr::null()
}

pub fn method_getImplementation(env: &mut crate::Environment, cls: Class, name: SEL) -> ConstVoidPtr {
    // Согласно спецификации Objective-C, если передан nil класс, возвращаем nil
    if cls.is_null() {
        return ConstVoidPtr::null();
    }

    let mut curr = cls;
    while !curr.is_null() {
        // Достаём хост-объект класса, чтобы получить доступ к списку методов
        if let Some(host_obj) = env.objc.get_host_object(curr) {
            if let Some(class_obj) = host_obj.as_any().downcast_ref::<ClassHostObject>() {
                // Если селектор зарегистрирован в данном классе
                if class_obj.methods.contains_key(&name) {
                    // Возвращаем non-null указатель. 
                    // В реальном iOS/macOS это указатель на структуру `method_t`.
                    // Но поскольку touchHLE абстрагирует методы в хостовый `HashMap`
                    // и не выделяет под них память в гостевом пространстве, 
                    // возврат указателя на сам класс — это безопасный способ дать 
                    // приложению "валидный" (читаемый) адрес, означающий, что метод существует.
                    return curr.cast_const().cast();
                }
            }
        }
        
        // Поднимаемся вверх по иерархии к суперклассу
        let next = env.objc.get_superclass(curr);
        
        // Защита от бесконечного цикла, если иерархия зациклена
        if next == curr {
            break;
        }
        curr = next;
    }

    // Если прошли всю цепочку и ничего не нашли, метод не существует
    ConstVoidPtr::null()
}

pub fn method_setImplementation(env: &mut crate::Environment, cls: Class, name: SEL) -> ConstVoidPtr {
    // Согласно спецификации Objective-C, если передан nil класс, возвращаем nil
    if cls.is_null() {
        return ConstVoidPtr::null();
    }

    let mut curr = cls;
    while !curr.is_null() {
        // Достаём хост-объект класса, чтобы получить доступ к списку методов
        if let Some(host_obj) = env.objc.get_host_object(curr) {
            if let Some(class_obj) = host_obj.as_any().downcast_ref::<ClassHostObject>() {
                // Если селектор зарегистрирован в данном классе
                if class_obj.methods.contains_key(&name) {
                    // Возвращаем non-null указатель. 
                    // В реальном iOS/macOS это указатель на структуру `method_t`.
                    // Но поскольку touchHLE абстрагирует методы в хостовый `HashMap`
                    // и не выделяет под них память в гостевом пространстве, 
                    // возврат указателя на сам класс — это безопасный способ дать 
                    // приложению "валидный" (читаемый) адрес, означающий, что метод существует.
                    return curr.cast_const().cast();
                }
            }
        }
        
        // Поднимаемся вверх по иерархии к суперклассу
        let next = env.objc.get_superclass(curr);
        
        // Защита от бесконечного цикла, если иерархия зациклена
        if next == curr {
            break;
        }
        curr = next;
    }

    // Если прошли всю цепочку и ничего не нашли, метод не существует
    ConstVoidPtr::null()
}

pub fn method_getTypeEncoding(env: &mut crate::Environment, cls: Class, name: SEL) -> ConstVoidPtr {
    // Согласно спецификации Objective-C, если передан nil класс, возвращаем nil
    if cls.is_null() {
        return ConstVoidPtr::null();
    }

    let mut curr = cls;
    while !curr.is_null() {
        // Достаём хост-объект класса, чтобы получить доступ к списку методов
        if let Some(host_obj) = env.objc.get_host_object(curr) {
            if let Some(class_obj) = host_obj.as_any().downcast_ref::<ClassHostObject>() {
                // Если селектор зарегистрирован в данном классе
                if class_obj.methods.contains_key(&name) {
                    // Возвращаем non-null указатель. 
                    // В реальном iOS/macOS это указатель на структуру `method_t`.
                    // Но поскольку touchHLE абстрагирует методы в хостовый `HashMap`
                    // и не выделяет под них память в гостевом пространстве, 
                    // возврат указателя на сам класс — это безопасный способ дать 
                    // приложению "валидный" (читаемый) адрес, означающий, что метод существует.
                    return curr.cast_const().cast();
                }
            }
        }
        
        // Поднимаемся вверх по иерархии к суперклассу
        let next = env.objc.get_superclass(curr);
        
        // Защита от бесконечного цикла, если иерархия зациклена
        if next == curr {
            break;
        }
        curr = next;
    }

    // Если прошли всю цепочку и ничего не нашли, метод не существует
    ConstVoidPtr::null()
}
