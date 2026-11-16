#!/usr/bin/env python3
# This Source Code Form is subject to the terms of the Mozilla Public License,
# v. 2.0. https://mozilla.org/MPL/2.0/
"""Measure the translator's real GL uniform upload and CPU index scan.

Requires Python 3, git, rustc >= 1.82, Linux GLES headers, libEGL and libGLESv2.
No Cargo build, guest app or display server is needed. See
tests/profile-emulation.md for scope and interpretation.
"""

import argparse
from pathlib import Path
import re
import subprocess
import tempfile

REPO = Path(__file__).resolve().parents[1]
SOURCE = "src/gles/gles1_on_gles2.rs"
BASELINE = "228546e58233a8e7578ff9af1f10b4a8dbd88bde"


def function(source, signature):
    """Extract the routines used here, whose string braces are balanced."""
    start = source.index(signature)
    cursor = source.index("{", start) + 1
    depth = 1
    while depth:
        depth += (source[cursor] == "{") - (source[cursor] == "}")
        cursor += 1
    return source[start:cursor]


# Only declarations used by the extracted routines; GL is the real host driver.
GL_FUNCTIONS = {
    "CreateShader": "kind:u32) -> u32",
    "ShaderSource": "shader:u32,count:i32,string:*const *const GLchar,length:*const i32)",
    "CompileShader": "shader:u32)",
    "GetShaderiv": "shader:u32,pname:u32,params:*mut i32)",
    "GetShaderInfoLog": "shader:u32,max_length:i32,length:*mut i32,info_log:*mut GLchar)",
    "CreateProgram": ") -> u32",
    "AttachShader": "program:u32,shader:u32)",
    "BindAttribLocation": "program:u32,index:u32,name:*const GLchar)",
    "LinkProgram": "program:u32)",
    "GetProgramiv": "program:u32,pname:u32,params:*mut i32)",
    "GetProgramInfoLog": "program:u32,max_length:i32,length:*mut i32,info_log:*mut GLchar)",
    "DeleteShader": "shader:u32)",
    "GetUniformLocation": "program:u32,name:*const GLchar) -> i32",
    "UseProgram": "program:u32)",
    "UniformMatrix4fv": "location:i32,count:i32,transpose:u8,value:*const f32)",
    "Uniform1i": "location:i32,value:i32)",
    "Uniform1f": "location:i32,value:f32)",
    "Uniform1iv": "location:i32,count:i32,value:*const i32)",
    "Uniform1fv": "location:i32,count:i32,value:*const f32)",
    "Uniform3fv": "location:i32,count:i32,value:*const f32)",
    "Uniform4fv": "location:i32,count:i32,value:*const f32)",
    "ActiveTexture": "texture:u32)",
    "BindTexture": "target:u32,texture:u32)",
    "GetString": "name:u32) -> *const u8",
}

PREAMBLE = r'''
#![allow(non_snake_case, dead_code, unused_unsafe)]
use std::ffi::{CStr, CString, c_void};
use std::marker::PhantomData;
use std::collections::HashMap;
use std::time::Instant;
use std::hint::black_box;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
type GLint=i32; type GLuint=u32; type GLenum=u32; type GLfloat=f32;
type GLfixed=i32; type GLsizei=i32; type GLchar=std::os::raw::c_char;
type GLvoid=c_void; type GLclampf=f32;
static COUNT_ALLOCATIONS:AtomicBool=AtomicBool::new(false);
static ALLOC_CALLS:AtomicUsize=AtomicUsize::new(0);
struct AllocCounter;
unsafe impl GlobalAlloc for AllocCounter {
    unsafe fn alloc(&self, layout:Layout)->*mut u8 {
        if COUNT_ALLOCATIONS.load(Ordering::Relaxed) {
            ALLOC_CALLS.fetch_add(1, Ordering::Relaxed);
        }
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, pointer:*mut u8, layout:Layout) {
        System.dealloc(pointer, layout)
    }
    unsafe fn realloc(&self, pointer:*mut u8, layout:Layout, size:usize)->*mut u8 {
        if COUNT_ALLOCATIONS.load(Ordering::Relaxed) {
            ALLOC_CALLS.fetch_add(1, Ordering::Relaxed);
        }
        System.realloc(pointer, layout, size)
    }
}
#[global_allocator] static ALLOC:AllocCounter=AllocCounter;
'''

RUNNER = r'''
#[link(name="EGL")] unsafe extern "C" {
    fn eglGetPlatformDisplay(platform:u32, native:*mut c_void, attrs:*const isize)->*mut c_void;
    fn eglInitialize(display:*mut c_void, major:*mut i32, minor:*mut i32)->u32;
    fn eglBindAPI(api:u32)->u32;
    fn eglChooseConfig(display:*mut c_void, attrs:*const i32, config:*mut *mut c_void, size:i32, count:*mut i32)->u32;
    fn eglCreateContext(display:*mut c_void, config:*mut c_void, share:*mut c_void, attrs:*const i32)->*mut c_void;
    fn eglCreatePbufferSurface(display:*mut c_void, config:*mut c_void, attrs:*const i32)->*mut c_void;
    fn eglMakeCurrent(display:*mut c_void, draw:*mut c_void, read:*mut c_void, context:*mut c_void)->u32;
}
fn measure(work:impl FnOnce()) -> (f64,usize) {
    ALLOC_CALLS.store(0,Ordering::Relaxed);
    let start=Instant::now();
    work();
    (start.elapsed().as_secs_f64()*1000.0,ALLOC_CALLS.load(Ordering::Relaxed))
}
fn main() { unsafe {
    // EGL_PLATFORM_SURFACELESS_MESA; pbuffer and ES 2.0 config.
    let display=eglGetPlatformDisplay(0x31DD,std::ptr::null_mut(),std::ptr::null());
    assert_eq!(eglInitialize(display,std::ptr::null_mut(),std::ptr::null_mut()),1);
    assert_eq!(eglBindAPI(0x30A0),1); // EGL_OPENGL_ES_API
    let mut config=std::ptr::null_mut();
    let mut count=0;
    let attrs=[0x3040,4,0x3033,1,0x3024,8,0x3023,8,0x3022,8,0x3038];
    assert_eq!(eglChooseConfig(display,attrs.as_ptr(),&mut config,1,&mut count),1);
    assert_eq!(count,1);
    let context=eglCreateContext(display,config,std::ptr::null_mut(),[0x3098,2,0x3038].as_ptr());
    let surface=eglCreatePbufferSurface(display,config,[0x3057,16,0x3056,16,0x3038].as_ptr());
    assert_eq!(eglMakeCurrent(display,surface,surface,context),1);
    println!("GL renderer: {}",CStr::from_ptr(gl::GetString(0x1F01).cast()).to_string_lossy());
    let program=create_program(false).unwrap();
    let uniforms=UniformLocations::load_with(|name|gl::GetUniformLocation(program,name.as_ptr()));
    let mut state=TranslatorState::new();
    state.program=Some(TranslatorProgram{name:program,uniforms});
    // Nonzero byte offset keeps the baseline's offset-zero bug out of timing.
    let mut data=vec![0u8,0];
    for i in 0..60000u32 {
        data.extend_from_slice(&((i*7%50000) as u16).to_ne_bytes());
    }
    state.element_array_buffer_data.insert(1,data);
    state.element_array_buffer_binding=1;
    let gles=GLES1OnGLES2{state:&mut state,_gl_lifetime:PhantomData};
    for _ in 0..100 {gles.baseline_upload(program);gles.upload_uniforms(program);}
    COUNT_ALLOCATIONS.store(std::env::args().any(|arg|arg=="--count-allocations"),Ordering::Relaxed);
    let count_allocations=COUNT_ALLOCATIONS.load(Ordering::Relaxed);
    for run in 0..5 {
        let (old,old_alloc)=measure(||for _ in 0..2000 {gles.baseline_upload(black_box(program));});
        let (new,new_alloc)=measure(||for _ in 0..2000 {gles.upload_uniforms(black_box(program));});
        println!("Uniform setup 2000 draws run {run}: baseline {old:.3} ms cached {new:.3} ms speedup {:.2}x",old/new);
        if count_allocations {println!("  Rust allocations/reallocations: {old_alloc}/{new_alloc}");}
        let (old,old_alloc)=measure(||for _ in 0..1000 {
            black_box(gles.baseline_range(gl::UNSIGNED_SHORT,black_box(2usize as *const _),60000));
        });
        let (new,new_alloc)=measure(||for _ in 0..1000 {
            black_box(gles.indexed_vertex_range(gl::UNSIGNED_SHORT,black_box(2usize as *const _),60000));
        });
        println!("Index max 60k x1000 run {run}: baseline {old:.3} ms streaming {new:.3} ms speedup {:.2}x",old/new);
        if count_allocations {println!("  Rust allocations/reallocations: {old_alloc}/{new_alloc}");}
    }
}}
'''


def build_source(baseline):
    current = (REPO / SOURCE).read_text()
    base = subprocess.check_output(
        ["git", "show", f"{baseline}:{SOURCE}"], cwd=REPO, text=True
    )
    structs = current[current.index("#[derive(Clone, Copy)]") : current.index("pub struct GLES1OnGLES2Context")]
    constants = current[current.index("const ATTR_POSITION") : current.index("#[derive(Clone, Copy)]")]
    shader = current[current.index("fn translate_fixed_function_shader(") : current.index("impl GLES for GLES1OnGLES2<'_>")]
    start = base.index("        gl::UseProgram(program);", base.index("    unsafe fn DrawArrays("))
    end = base.index("        let position = self.state.arrays[0];", start)
    methods = "\n".join([
        "unsafe fn baseline_upload(&self,program:GLuint) {\n" + base[start:end] + "\n}",
        function(current, "    unsafe fn upload_uniforms("),
        function(base, "    unsafe fn indexed_vertex_range(").replace("indexed_vertex_range", "baseline_range"),
        function(current, "    unsafe fn indexed_vertex_range("),
    ])
    headers = "\n".join(Path(path).read_text() for path in [
        "/usr/include/GLES/gl.h", "/usr/include/GLES/glext.h", "/usr/include/GLES2/gl2.h"
    ])
    values = dict(re.findall(r"#define GL_(\w+)\s+(0x[0-9a-fA-F]+|[0-9]+)\b", headers))
    names = set(re.findall(r"(?:es1|gl)::([A-Z][A-Z_0-9]*)\b", structs + constants + shader + methods))
    declarations = "\n".join(
        f"pub const {name}:u32={values[name]};" if name != "FALSE" else "pub const FALSE:u8=0;"
        for name in sorted(names)
    )
    functions = "\n".join(
        f'#[link_name="gl{name}"] pub fn {name}({args};' for name, args in GL_FUNCTIONS.items()
    )
    return (
        PREAMBLE + "mod gl { use super::GLchar;\n" + declarations
        + '\n#[link(name="GLESv2")] unsafe extern "C" {\n' + functions + "\n}}\nuse gl as es1;\n"
        + constants + structs
        + "struct GLES1OnGLES2<'a>{state:&'a mut TranslatorState,_gl_lifetime:PhantomData<&'a()>}\n"
        + function(current, "fn multiply(") + shader
        + "impl GLES1OnGLES2<'_>{\n" + methods + "\n}\n" + RUNNER
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", default=BASELINE, help="pre-optimization Git revision")
    parser.add_argument("--count-allocations", action="store_true", help="count Rust allocations/reallocations")
    parser.add_argument("--output", type=Path, help="keep the compiled binary and generated .rs for profiling")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="hyperhle-gles1-") as temporary:
        binary = args.output.resolve() if args.output else Path(temporary) / "benchmark"
        source = binary.with_suffix(".rs")
        source.write_text(build_source(args.baseline))
        subprocess.run(["rustc", "--edition=2021", "-O", "-g", str(source), "-o", str(binary)], check=True)
        command = [str(binary)] + (["--count-allocations"] if args.count_allocations else [])
        subprocess.run(command, check=True)


if __name__ == "__main__":
    main()
