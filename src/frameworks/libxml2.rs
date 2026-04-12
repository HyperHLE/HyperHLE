use crate::dyld::{export_c_func, ConstantExports, FunctionExports};
use crate::Environment;

pub const DYLIB: crate::dyld::HostDylib = crate::dyld::HostDylib {
    path: "/usr/lib/libxml2.2.dylib",
    aliases: &["/usr/lib/libxml2.dylib"],
    class_exports: &[],
    constant_exports: &[],
    function_exports: &[FUNCTIONS],
};

const FUNCTIONS: FunctionExports = &[
    export_c_func!(xmlNewParserCtxt()),
    export_c_func!(xmlClearParserCtxt()),
    export_c_func!(xmlCtxtReadMemory(_, _, _, _, _, _)),
    export_c_func!(xmlReadFile(_, _, _, _, _, _)),
    export_c_func!(xmlReadMemory(_, _, _, _, _)),
    export_c_func!(xmlParseMemory(_, _)),
    export_c_func!(xmlDocGetRootElement(_)),
    export_c_func!(xmlFree(_)),
    export_c_func!(xmlCtxtGetLastError(_)),
    export_c_func!(xmlFreeDoc(_)),
    export_c_func!(xmlCleanupParser()),
    export_c_func!(xmlFreeParserCtxt(_)),
    export_c_func!(xmlGetProp(_, _)),
    export_c_func!(xmlHasProp(_, _)),
    export_c_func!(xmlStrcmp(_, _)),
    export_c_func!(xmlNodeGetContent(_)),
];

// Хелпер для выделения памяти под фейковые XML-структуры (ноды, контексты, доки)
fn alloc_xml_mem(env: &mut Environment) -> u32 {
    let size = 512; 
    let ptr: crate::mem::MutPtr<u8> = env.mem.alloc(size).cast();
    let slice = env.mem.bytes_at_mut(ptr, size);
    for byte in slice.iter_mut() { *byte = 0; }
    ptr.to_bits()
}

#[allow(non_snake_case)]
fn xmlNewParserCtxt(env: &mut Environment) -> u32 { alloc_xml_mem(env) }

#[allow(non_snake_case)]
fn xmlClearParserCtxt(env: &mut Environment) -> u32 { alloc_xml_mem(env) }

#[allow(non_snake_case)]
fn xmlCtxtReadMemory(env: &mut Environment, _ctxt: u32, _buf: u32, _sz: u32, _url: u32, _enc: u32, _opt: u32) -> u32 { 
    alloc_xml_mem(env) // Возвращаем фейковый xmlDocPtr
}

#[allow(non_snake_case)]
fn xmlReadFile(env: &mut Environment, _ctxt: u32, _buf: u32, _sz: u32, _url: u32, _enc: u32, _opt: u32) -> u32 { 
    alloc_xml_mem(env) // Возвращаем фейковый xmlDocPtr
}

#[allow(non_snake_case)]
fn xmlReadMemory(env: &mut Environment, _buf: u32, _sz: u32, _url: u32, _enc: u32, _opt: u32) -> u32 { 
    alloc_xml_mem(env) 
}

#[allow(non_snake_case)]
fn xmlParseMemory(env: &mut Environment, _buf: u32, _sz: u32) -> u32 { 
    alloc_xml_mem(env) 
}

#[allow(non_snake_case)]
fn xmlDocGetRootElement(env: &mut Environment, _doc: u32) -> u32 { 
    alloc_xml_mem(env) // Возвращаем фейковый xmlNodePtr
}

#[allow(non_snake_case)]
fn xmlGetProp(_env: &mut Environment, _node: u32, _name: u32) -> u32 { 0 }

#[allow(non_snake_case)]
fn xmlHasProp(_env: &mut Environment, _node: u32, _name: u32) -> u32 { 0 }

#[allow(non_snake_case)]
fn xmlStrcmp(_env: &mut Environment, _node: u32, _name: u32) -> u32 { 0 }

#[allow(non_snake_case)]
fn xmlNodeGetContent(_env: &mut Environment, _node: u32) -> u32 { 0 }

#[allow(non_snake_case)]
fn xmlCtxtGetLastError(_env: &mut Environment, _ctxt: u32) -> u32 { 0 }

#[allow(non_snake_case)]
fn xmlFree(_env: &mut Environment, _ptr: u32) {}
#[allow(non_snake_case)]
fn xmlFreeDoc(_env: &mut Environment, _doc: u32) {}
#[allow(non_snake_case)]
fn xmlCleanupParser(_env: &mut Environment) {}
#[allow(non_snake_case)]
fn xmlFreeParserCtxt(_env: &mut Environment, _ctxt: u32) {}

