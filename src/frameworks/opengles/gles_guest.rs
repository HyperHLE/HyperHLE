/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

//! Wrapper functions exposing OpenGL ES to the guest.

use touchHLE_gl_bindings::gles11::{
    ARRAY_BUFFER, ELEMENT_ARRAY_BUFFER, ELEMENT_ARRAY_BUFFER_BINDING, VERTEX_ARRAY_BUFFER_BINDING,
    WRITE_ONLY_OES,
};
use crate::dyld::{export_c_func, FunctionExports};
use crate::frameworks::opengles::eagl::EAGLContextHostObject;
use crate::gles::{gles11_raw as gles11, GLES};
use crate::mem::{ConstPtr, ConstVoidPtr, GuestISize, GuestUSize, Mem, MutPtr, MutVoidPtr, Ptr};
use crate::objc::nil;
use crate::Environment;
use std::slice::from_raw_parts;

use crate::gles::gles11_raw::types::{
    GLbitfield, GLboolean, GLclampf, GLclampx, GLenum, GLfixed, GLfloat, GLint, GLsizei, GLubyte,
    GLuint, GLvoid, GLintptr as HostGLintptr, GLsizeiptr as HostGLsizeiptr
};
type GuestGLsizeiptr = GuestISize;
type GuestGLintptr = GuestISize;

const SUPPORTED_COMPRESSED_TEXTURE_FORMATS: &[GLenum] = &[
    gles11::COMPRESSED_RGBA_PVRTC_2BPPV1_IMG, gles11::COMPRESSED_RGBA_PVRTC_4BPPV1_IMG,
    gles11::COMPRESSED_RGB_PVRTC_2BPPV1_IMG, gles11::COMPRESSED_RGB_PVRTC_4BPPV1_IMG,
    gles11::PALETTE4_R5_G6_B5_OES, gles11::PALETTE4_RGB5_A1_OES, gles11::PALETTE4_RGB8_OES,
    gles11::PALETTE4_RGBA4_OES, gles11::PALETTE4_RGBA8_OES, gles11::PALETTE8_R5_G6_B5_OES,
    gles11::PALETTE8_RGB5_A1_OES, gles11::PALETTE8_RGB8_OES, gles11::PALETTE8_RGBA4_OES,
    gles11::PALETTE8_RGBA8_OES,
];
#[track_caller]
fn with_ctx_and_mem<T, U: Default>(env: &mut Environment, f: T) -> U
where T: FnOnce(&mut dyn GLES, &mut Mem) -> U,
{
    if env.framework_state.opengles.current_ctx_for_thread(env.current_thread).is_none() {
        log_dbg!("Skipping GLES call without context (line {})", std::panic::Location::caller().line());
        return U::default();
    }
    let mut gles = super::sync_context(
        &mut env.framework_state.opengles, &mut env.objc,
        env.window.as_mut().expect("OpenGL ES is not supported in headless mode"),
        env.current_thread,
    );
    let res = f(gles.as_mut(), &mut env.mem);
    #[allow(clippy::let_and_return)] res
}

#[track_caller]
fn with_ctx_and_mem_no_skip<T, U>(env: &mut Environment, f: T) -> U
where T: FnOnce(&mut dyn GLES, &mut Mem) -> U,
{
    let mut gles = super::sync_context(
        &mut env.framework_state.opengles, &mut env.objc,
        env.window.as_mut().expect("OpenGL ES is not supported in headless mode"),
        env.current_thread,
    );
    let res = f(gles.as_mut(), &mut env.mem);
    #[allow(clippy::let_and_return)] res
}

fn glGetError(env: &mut Environment) -> GLenum {
    let ignore_gl_errors = env.options.ignore_gl_errors;
    with_ctx_and_mem(env, |gles, _mem| {
        let err = unsafe { gles.GetError() };
        if err != 0 {
            if ignore_gl_errors { return 0; }
            log!("Warning: glGetError() returned {:#x}", err);
        }
        err
    })
}
fn glEnable(env: &mut Environment, cap: GLenum) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.Enable(cap) }); }
fn glIsEnabled(env: &mut Environment, cap: GLenum) -> GLboolean { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.IsEnabled(cap) }) }
fn glDisable(env: &mut Environment, cap: GLenum) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.Disable(cap) }); }
fn glClientActiveTexture(env: &mut Environment, texture: GLenum) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.ClientActiveTexture(texture) }) }
fn glEnableClientState(env: &mut Environment, array: GLenum) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.EnableClientState(array) }); }
fn glDisableClientState(env: &mut Environment, array: GLenum) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.DisableClientState(array) }); }

fn glGetBooleanv(env: &mut Environment, pname: GLenum, params: MutPtr<GLboolean>) {
    if env.framework_state.opengles.current_ctx_for_thread(env.current_thread).is_none() {
        env.mem.write(params, 0);
        return;
    }
    with_ctx_and_mem(env, |gles, mem| {
        let params = mem.ptr_at_mut(params, 16);
        unsafe { gles.GetBooleanv(pname, params) };
    });
}
fn glGetFloatv(env: &mut Environment, pname: GLenum, params: MutPtr<GLfloat>) {
    if env.framework_state.opengles.current_ctx_for_thread(env.current_thread).is_none() {
        env.mem.write(params, 0.0);
        return;
    }
    with_ctx_and_mem(env, |gles, mem| {
        let params = mem.ptr_at_mut(params, 16);
        unsafe { gles.GetFloatv(pname, params) };
    });
}
fn glGetIntegerv(env: &mut Environment, pname: GLenum, params: MutPtr<GLint>) {
    match pname {
        gles11::NUM_COMPRESSED_TEXTURE_FORMATS => { env.mem.write(params, SUPPORTED_COMPRESSED_TEXTURE_FORMATS.len() as _); }
        gles11::COMPRESSED_TEXTURE_FORMATS => {
            for (idx, &format) in SUPPORTED_COMPRESSED_TEXTURE_FORMATS.iter().enumerate() {
                env.mem.write(params + idx as GuestUSize, format as _);
            }
        }
        0x8cdf | 0x8d57 => { env.mem.write(params, 1 as _); }
        gles11::MAX_TEXTURE_SIZE => { env.mem.write(params, 2048 as _); }
        _ => {
            if env.framework_state.opengles.current_ctx_for_thread(env.current_thread).is_none() {
                env.mem.write(params, 1);
                return;
            }
            with_ctx_and_mem(env, |gles, mem| {
                let params = mem.ptr_at_mut(params, 16);
                unsafe { gles.GetIntegerv(pname, params) };
            });
        }
    }
}
fn glGetPointerv(env: &mut Environment, pname: GLenum, params: MutPtr<ConstVoidPtr>) {
    if env.framework_state.opengles.current_ctx_for_thread(env.current_thread).is_none() {
        env.mem.write(params, Ptr::null());
        return;
    }
    use crate::gles::gles1_on_gl2::{ArrayInfo, ARRAYS};
    let &ArrayInfo { buffer_binding, .. } = ARRAYS.iter().find(|info| info.pointer == pname).unwrap();
    with_ctx_and_mem(env, |gles, mem| {
        let mut host_pointer_or_offset = std::ptr::null();
        let guest_pointer_or_offset = unsafe {
            gles.GetPointerv(pname, &mut host_pointer_or_offset);
            translate_pointer_or_offset_to_guest(gles, mem, host_pointer_or_offset, buffer_binding)
        };
        mem.write(params, guest_pointer_or_offset);
    });
}
fn glGetTexEnviv(env: &mut Environment, target: GLenum, pname: GLenum, params: MutPtr<GLint>) {
    with_ctx_and_mem(env, |gles, mem| {
        let params = mem.ptr_at_mut(params, 16);
        unsafe { gles.GetTexEnviv(target, pname, params) };
    });
}
fn glGetTexEnvfv(env: &mut Environment, target: GLenum, pname: GLenum, params: MutPtr<GLfloat>) {
    with_ctx_and_mem(env, |gles, mem| {
        let params = mem.ptr_at_mut(params, 16);
        unsafe { gles.GetTexEnvfv(target, pname, params) };
    });
}

fn glHint(env: &mut Environment, target: GLenum, mode: GLenum) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.Hint(target, mode) }) }
fn glFinish(env: &mut Environment) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.Finish() }) }
fn glFlush(env: &mut Environment) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.Flush() }) }
fn glGetString(env: &mut Environment, name: GLenum) -> ConstPtr<GLubyte> {
    let res = if let Some(&str) = env.framework_state.opengles.strings_cache.get(&name) { str } else {
        let s: &[u8] = match name {
            gles11::VENDOR => b"Imagination Technologies",
            gles11::RENDERER => b"PowerVR MBXLite with VGPLite",
            gles11::VERSION => b"OpenGL ES-CM 1.1 (76)",
            gles11::EXTENSIONS => b"GL_APPLE_framebuffer_multisample GL_APPLE_texture_max_level GL_EXT_discard_framebuffer GL_EXT_texture_filter_anisotropic GL_EXT_texture_lod_bias GL_IMG_read_format GL_IMG_texture_compression_pvrtc GL_IMG_texture_format_BGRA8888 GL_OES_blend_subtract GL_OES_compressed_paletted_texture GL_OES_depth24 GL_OES_draw_texture GL_OES_framebuffer_object GL_OES_mapbuffer GL_OES_matrix_palette GL_OES_point_size_array GL_OES_point_sprite GL_OES_read_format GL_OES_rgb8_rgba8 GL_OES_texture_mirrored_repeat GL_OES_vertex_array_object ",
            _ => b"Unknown"
        };
        let new_str = env.mem.alloc_and_write_cstr(s).cast_const();
        env.framework_state.opengles.strings_cache.insert(name, new_str);
        new_str
    };
    res
}

fn glAlphaFunc(env: &mut Environment, func: GLenum, ref_: GLclampf) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.AlphaFunc(func, ref_) }) }
fn glAlphaFuncx(env: &mut Environment, func: GLenum, ref_: GLclampx) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.AlphaFuncx(func, ref_) }) }
fn glBlendFunc(env: &mut Environment, sfactor: GLenum, dfactor: GLenum) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.BlendFunc(sfactor, dfactor) }) }
fn glBlendEquationOES(env: &mut Environment, mode: GLenum) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.BlendEquationOES(mode) }) }
fn glColorMask(env: &mut Environment, red: GLboolean, green: GLboolean, blue: GLboolean, alpha: GLboolean) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.ColorMask(red, green, blue, alpha) }) }
fn glClipPlanef(env: &mut Environment, plane: GLenum, equation: ConstPtr<GLfloat>) { with_ctx_and_mem(env, |gles, mem| { let equation = mem.ptr_at(equation, 4); unsafe { gles.ClipPlanef(plane, equation) } }) }
fn glClipPlanex(env: &mut Environment, plane: GLenum, equation: ConstPtr<GLfixed>) { with_ctx_and_mem(env, |gles, mem| { let equation = mem.ptr_at(equation, 4); unsafe { gles.ClipPlanex(plane, equation) } }) }
fn glCullFace(env: &mut Environment, mode: GLenum) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.CullFace(mode) }) }
fn glDepthFunc(env: &mut Environment, func: GLenum) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.DepthFunc(func) }) }
fn glDepthMask(env: &mut Environment, flag: GLboolean) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.DepthMask(flag) }) }
fn glDepthRangef(env: &mut Environment, near: GLclampf, far: GLclampf) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.DepthRangef(near, far) }) }
fn glDepthRangex(env: &mut Environment, near: GLclampx, far: GLclampx) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.DepthRangex(near, far) }) }
fn glFrontFace(env: &mut Environment, mode: GLenum) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.FrontFace(mode) }) }
fn glPolygonOffset(env: &mut Environment, factor: GLfloat, units: GLfloat) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.PolygonOffset(factor, units) }) }
fn glPolygonOffsetx(env: &mut Environment, factor: GLfixed, units: GLfixed) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.PolygonOffsetx(factor, units) }) }
fn glSampleCoverage(env: &mut Environment, value: GLclampf, invert: GLboolean) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.SampleCoverage(value, invert) }) }
fn glSampleCoveragex(env: &mut Environment, value: GLclampx, invert: GLboolean) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.SampleCoveragex(value, invert) }) }
fn glShadeModel(env: &mut Environment, mode: GLenum) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.ShadeModel(mode) }) }
fn glScissor(env: &mut Environment, x: GLint, y: GLint, width: GLsizei, height: GLsizei) {
    let factor = env.options.scale_hack.get() as GLsizei;
    let (x, y) = (x * factor, y * factor);
    let (width, height) = (width * factor, height * factor);
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.Scissor(x, y, width, height) })
}
fn glViewport(env: &mut Environment, x: GLint, y: GLint, width: GLsizei, height: GLsizei) {
    let factor = env.options.scale_hack.get() as GLsizei;
    let (x, y) = (x * factor, y * factor);
    let (width, height) = (width * factor, height * factor);
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.Viewport(x, y, width, height) })
}
fn glLineWidth(env: &mut Environment, val: GLfloat) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.LineWidth(val) }) }
fn glLineWidthx(env: &mut Environment, val: GLfixed) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.LineWidthx(val) }) }
fn glStencilFunc(env: &mut Environment, func: GLenum, ref_: GLint, mask: GLuint) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.StencilFunc(func, ref_, mask) }); }
fn glStencilOp(env: &mut Environment, sfail: GLenum, dpfail: GLenum, dppass: GLenum) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.StencilOp(sfail, dpfail, dppass) }); }
fn glStencilMask(env: &mut Environment, mask: GLuint) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.StencilMask(mask) }); }
fn glLogicOp(env: &mut Environment, opcode: GLenum) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.LogicOp(opcode) }); }
fn glPointSize(env: &mut Environment, size: GLfloat) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.PointSize(size) }) }
fn glPointSizex(env: &mut Environment, size: GLfixed) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.PointSizex(size) }) }
fn glPointParameterf(env: &mut Environment, pname: GLenum, param: GLfloat) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.PointParameterf(pname, param) }) }
fn glPointParameterx(env: &mut Environment, pname: GLenum, param: GLfixed) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.PointParameterx(pname, param) }) }
fn glPointParameterfv(env: &mut Environment, pname: GLenum, params: ConstPtr<GLfloat>) { with_ctx_and_mem(env, |gles, mem| { let params = mem.ptr_at(params, 4); unsafe { gles.PointParameterfv(pname, params) } }) }
fn glPointParameterxv(env: &mut Environment, pname: GLenum, params: ConstPtr<GLfixed>) { with_ctx_and_mem(env, |gles, mem| { let params = mem.ptr_at(params, 4); unsafe { gles.PointParameterxv(pname, params) } }) }

fn glFogf(env: &mut Environment, pname: GLenum, param: GLfloat) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.Fogf(pname, param) }) }
fn glFogx(env: &mut Environment, pname: GLenum, param: GLfixed) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.Fogx(pname, param) }) }
fn glFogfv(env: &mut Environment, pname: GLenum, params: ConstPtr<GLfloat>) { with_ctx_and_mem(env, |gles, mem| { let params = mem.ptr_at(params, 4); unsafe { gles.Fogfv(pname, params) } }) }
fn glFogxv(env: &mut Environment, pname: GLenum, params: ConstPtr<GLfixed>) { with_ctx_and_mem(env, |gles, mem| { let params = mem.ptr_at(params, 4); unsafe { gles.Fogxv(pname, params) } }) }
fn glLightf(env: &mut Environment, light: GLenum, pname: GLenum, param: GLfloat) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.Lightf(light, pname, param) }) }
fn glLightx(env: &mut Environment, light: GLenum, pname: GLenum, param: GLfixed) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.Lightx(light, pname, param) }) }
fn glLightfv(env: &mut Environment, light: GLenum, pname: GLenum, params: ConstPtr<GLfloat>) { with_ctx_and_mem(env, |gles, mem| { let params = mem.ptr_at(params, 4); unsafe { gles.Lightfv(light, pname, params) } }) }
fn glLightxv(env: &mut Environment, light: GLenum, pname: GLenum, params: ConstPtr<GLfixed>) { with_ctx_and_mem(env, |gles, mem| { let params = mem.ptr_at(params, 4); unsafe { gles.Lightxv(light, pname, params) } }) }
fn glLightModelf(env: &mut Environment, pname: GLenum, param: GLfloat) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.LightModelf(pname, param) }) }
fn glLightModelx(env: &mut Environment, pname: GLenum, param: GLfixed) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.LightModelx(pname, param) }) }
fn glLightModelfv(env: &mut Environment, pname: GLenum, params: ConstPtr<GLfloat>) { with_ctx_and_mem(env, |gles, mem| { let params = mem.ptr_at(params, 4); unsafe { gles.LightModelfv(pname, params) } }) }
fn glLightModelxv(env: &mut Environment, pname: GLenum, params: ConstPtr<GLfixed>) { with_ctx_and_mem(env, |gles, mem| { let params = mem.ptr_at(params, 4); unsafe { gles.LightModelxv(pname, params) } }) }
fn glMaterialf(env: &mut Environment, face: GLenum, pname: GLenum, param: GLfloat) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.Materialf(face, pname, param) }) }
fn glMaterialx(env: &mut Environment, face: GLenum, pname: GLenum, param: GLfixed) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.Materialx(face, pname, param) }) }
fn glMaterialfv(env: &mut Environment, face: GLenum, pname: GLenum, params: ConstPtr<GLfloat>) { with_ctx_and_mem(env, |gles, mem| { let params = mem.ptr_at(params, 4); unsafe { gles.Materialfv(face, pname, params) } }) }
fn glMaterialxv(env: &mut Environment, face: GLenum, pname: GLenum, params: ConstPtr<GLfixed>) { with_ctx_and_mem(env, |gles, mem| { let params = mem.ptr_at(params, 4); unsafe { gles.Materialxv(face, pname, params) } }) }

fn glIsBuffer(env: &mut Environment, buffer: GLuint) -> GLboolean { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.IsBuffer(buffer) }) }
fn glGenBuffers(env: &mut Environment, n: GLsizei, buffers: MutPtr<GLuint>) {
    if env.framework_state.opengles.current_ctx_for_thread(env.current_thread).is_none() {
        for i in 0..n { env.mem.write(buffers + (i as GuestUSize), (i + 1) as GLuint); }
        return;
    }
    with_ctx_and_mem(env, |gles, mem| {
        let n_usize: GuestUSize = n.try_into().unwrap();
        let buffers = mem.ptr_at_mut(buffers, n_usize);
        unsafe { gles.GenBuffers(n, buffers) }
    })
}
fn glDeleteBuffers(env: &mut Environment, n: GLsizei, buffers: ConstPtr<GLuint>) {
    with_ctx_and_mem(env, |gles, mem| {
        let n_usize: GuestUSize = n.try_into().unwrap();
        let buffers = mem.ptr_at(buffers, n_usize);
        unsafe { gles.DeleteBuffers(n, buffers) }
    })
}
fn glBindBuffer(env: &mut Environment, target: GLenum, buffer: GLuint) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.BindBuffer(target, buffer) }) }
fn glBufferData(env: &mut Environment, target: GLenum, size: GuestGLsizeiptr, data: ConstPtr<GLvoid>, usage: GLenum) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let data = if data.is_null() { std::ptr::null() } else { mem.ptr_at(data.cast::<u8>(), size.try_into().unwrap()).cast() };
        gles.BufferData(target, size as HostGLsizeiptr, data, usage)
    })
}
fn glBufferSubData(env: &mut Environment, target: GLenum, offset: GuestGLintptr, size: GuestGLsizeiptr, data: ConstPtr<GLvoid>) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let data = if data.is_null() { std::ptr::null() } else { mem.ptr_at(data.cast::<u8>(), size.try_into().unwrap()).cast() };
        gles.BufferSubData(target, offset as HostGLintptr, size as HostGLsizeiptr, data)
    })
}

fn glColor4f(env: &mut Environment, red: GLfloat, green: GLfloat, blue: GLfloat, alpha: GLfloat) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.Color4f(red, green, blue, alpha) }) }
fn glColor4x(env: &mut Environment, red: GLfixed, green: GLfixed, blue: GLfixed, alpha: GLfixed) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.Color4x(red, green, blue, alpha) }) }
fn glColor4ub(env: &mut Environment, red: GLubyte, green: GLubyte, blue: GLubyte, alpha: GLubyte) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.Color4ub(red, green, blue, alpha) }) }
fn glNormal3f(env: &mut Environment, nx: GLfloat, ny: GLfloat, nz: GLfloat) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.Normal3f(nx, ny, nz) }) }
fn glNormal3x(env: &mut Environment, nx: GLfixed, ny: GLfixed, nz: GLfixed) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.Normal3x(nx, ny, nz) }) }

unsafe fn translate_pointer_or_offset_to_host(gles: &mut dyn GLES, mem: &Mem, pointer_or_offset: ConstVoidPtr, which_binding: GLenum) -> *const GLvoid {
    let mut buffer_binding = 0;
    gles.GetIntegerv(which_binding, &mut buffer_binding);
    if buffer_binding != 0 {
        let offset = pointer_or_offset.to_bits();
        offset as usize as *const _
    } else if pointer_or_offset.is_null() {
        std::ptr::null()
    } else {
        mem.unchecked_ptr_at(pointer_or_offset.cast::<u8>(), 0).cast::<GLvoid>()
    }
}
unsafe fn translate_pointer_or_offset_to_guest(gles: &mut dyn GLES, mem: &Mem, pointer_or_offset: *const GLvoid, which_binding: GLenum) -> ConstVoidPtr {
    let mut buffer_binding = 0;
    gles.GetIntegerv(which_binding, &mut buffer_binding);
    if buffer_binding != 0 {
        let offset = pointer_or_offset as usize;
        Ptr::from_bits(u32::try_from(offset).unwrap())
    } else if pointer_or_offset.is_null() {
        Ptr::null()
    } else {
        mem.host_ptr_to_guest_ptr(pointer_or_offset)
    }
}

fn glColorPointer(env: &mut Environment, size: GLint, type_: GLenum, stride: GLsizei, pointer: ConstVoidPtr) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let pointer = translate_pointer_or_offset_to_host(gles, mem, pointer, gles11::ARRAY_BUFFER_BINDING);
        gles.ColorPointer(size, type_, stride, pointer)
    })
}
fn glNormalPointer(env: &mut Environment, type_: GLenum, stride: GLsizei, pointer: ConstVoidPtr) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let pointer = translate_pointer_or_offset_to_host(gles, mem, pointer, gles11::ARRAY_BUFFER_BINDING);
        gles.NormalPointer(type_, stride, pointer)
    })
}
fn glTexCoordPointer(env: &mut Environment, size: GLint, type_: GLenum, stride: GLsizei, pointer: ConstVoidPtr) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let pointer = translate_pointer_or_offset_to_host(gles, mem, pointer, gles11::ARRAY_BUFFER_BINDING);
        gles.TexCoordPointer(size, type_, stride, pointer)
    })
}
fn glVertexPointer(env: &mut Environment, size: GLint, type_: GLenum, stride: GLsizei, pointer: ConstVoidPtr) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let pointer = translate_pointer_or_offset_to_host(gles, mem, pointer, gles11::ARRAY_BUFFER_BINDING);
        gles.VertexPointer(size, type_, stride, pointer)
    })
}

fn glPointSizePointerOES(_env: &mut Environment, _type_: GLenum, _stride: GLsizei, _pointer: ConstVoidPtr) { log_once!("glPointSizePointerOES — stubbed"); }
fn glGetTexParameteriv(env: &mut Environment, _target: GLenum, _pname: GLenum, params: MutPtr<GLint>) {
    log_once!("glGetTexParameteriv — stubbed");
    env.mem.write(params, 0);
}

fn glDrawTexfOES(_env: &mut Environment, _x: GLfloat, _y: GLfloat, _z: GLfloat, _width: GLfloat, _height: GLfloat) {}
fn glDrawTexiOES(_env: &mut Environment, _x: GLint, _y: GLint, _z: GLint, _width: GLint, _height: GLint) {}
fn glDrawTexxOES(_env: &mut Environment, _x: GLfixed, _y: GLfixed, _z: GLfixed, _width: GLfixed, _height: GLfixed) {}
fn glDrawTexfvOES(_env: &mut Environment, _coords: ConstPtr<GLfloat>) {}
fn glDrawTexivOES(_env: &mut Environment, _coords: ConstPtr<GLint>) {}
fn glDrawTexxvOES(_env: &mut Environment, _coords: ConstPtr<GLfixed>) {}
fn glRenderbufferStorageMultisampleAPPLE(env: &mut Environment, target: GLenum, _samples: GLsizei, internalformat: GLenum, width: GLsizei, height: GLsizei) { glRenderbufferStorageOES(env, target, internalformat, width, height); }
fn glResolveMultisampleFramebufferAPPLE(_env: &mut Environment) {}
fn glDiscardFramebufferEXT(_env: &mut Environment, _target: GLenum, _numAttachments: GLsizei, _attachments: ConstPtr<GLenum>) {}
fn glBindVertexArrayOES(_env: &mut Environment, _array: GLuint) {}
fn glDeleteVertexArraysOES(_env: &mut Environment, _n: GLsizei, _arrays: ConstPtr<GLuint>) {}
fn glGenVertexArraysOES(env: &mut Environment, n: GLsizei, arrays: MutPtr<GLuint>) { for i in 0..n { env.mem.write(arrays + (i as GuestUSize), (i + 1) as GLuint); } }
fn glIsVertexArrayOES(_env: &mut Environment, _array: GLuint) -> GLboolean { 0 }
fn glCurrentPaletteMatrixOES(_env: &mut Environment, _matrixpaletteindex: GLuint) {}
fn glLoadPaletteFromModelViewMatrixOES(_env: &mut Environment) {}
fn glMatrixIndexPointerOES(_env: &mut Environment, _size: GLint, _type_: GLenum, _stride: GLsizei, _pointer: ConstVoidPtr) {}
fn glWeightPointerOES(_env: &mut Environment, _size: GLint, _type_: GLenum, _stride: GLsizei, _pointer: ConstVoidPtr) {}
fn glGetBufferPointervOES(env: &mut Environment, _target: GLenum, _pname: GLenum, params: MutPtr<ConstVoidPtr>) { env.mem.write(params, Ptr::null()); }

fn glDrawArrays(env: &mut Environment, mode: GLenum, first: GLint, count: GLsizei) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        let fog_state_backup = clamp_fog_state_values(gles);
        gles.DrawArrays(mode, first, count);
        restore_fog_state_values(gles, fog_state_backup);
    })
}
fn glDrawElements(env: &mut Environment, mode: GLenum, count: GLsizei, type_: GLenum, indices: ConstVoidPtr) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let fog_state_backup = clamp_fog_state_values(gles);
        let indices = translate_pointer_or_offset_to_host(gles, mem, indices, gles11::ELEMENT_ARRAY_BUFFER_BINDING);
        gles.DrawElements(mode, count, type_, indices);
        restore_fog_state_values(gles, fog_state_backup);
    })
}

fn glClear(env: &mut Environment, mask: GLbitfield) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.Clear(mask) }); }
fn glClearColor(env: &mut Environment, red: GLclampf, green: GLclampf, blue: GLclampf, alpha: GLclampf) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.ClearColor(red, green, blue, alpha) }); }
fn glClearColorx(env: &mut Environment, red: GLclampx, green: GLclampx, blue: GLclampx, alpha: GLclampx) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.ClearColorx(red, green, blue, alpha) }); }
fn glClearDepthf(env: &mut Environment, depth: GLclampf) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.ClearDepthf(depth) }); }
fn glClearDepthx(env: &mut Environment, depth: GLclampx) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.ClearDepthx(depth) }); }
fn glClearStencil(env: &mut Environment, s: GLint) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.ClearStencil(s) }); }

fn glMatrixMode(env: &mut Environment, mode: GLenum) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.MatrixMode(mode) }); }
fn glLoadIdentity(env: &mut Environment) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.LoadIdentity() }); }
fn glLoadMatrixf(env: &mut Environment, m: ConstPtr<GLfloat>) { with_ctx_and_mem(env, |gles, mem| { let m = mem.ptr_at(m, 16); unsafe { gles.LoadMatrixf(m) }; }); }
fn glLoadMatrixx(env: &mut Environment, m: ConstPtr<GLfixed>) { with_ctx_and_mem(env, |gles, mem| { let m = mem.ptr_at(m, 16); unsafe { gles.LoadMatrixx(m) }; }); }
fn glMultMatrixf(env: &mut Environment, m: ConstPtr<GLfloat>) { with_ctx_and_mem(env, |gles, mem| { let m = mem.ptr_at(m, 16); unsafe { gles.MultMatrixf(m) }; }); }
fn glMultMatrixx(env: &mut Environment, m: ConstPtr<GLfixed>) { with_ctx_and_mem(env, |gles, mem| { let m = mem.ptr_at(m, 16); unsafe { gles.MultMatrixx(m) }; }); }
fn glPushMatrix(env: &mut Environment) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.PushMatrix() }); }
fn glPopMatrix(env: &mut Environment) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.PopMatrix() }); }
fn glOrthof(env: &mut Environment, left: GLfloat, right: GLfloat, bottom: GLfloat, top: GLfloat, near: GLfloat, far: GLfloat) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.Orthof(left, right, bottom, top, near, far) }); }
fn glOrthox(env: &mut Environment, left: GLfixed, right: GLfixed, bottom: GLfixed, top: GLfixed, near: GLfixed, far: GLfixed) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.Orthox(left, right, bottom, top, near, far) }); }
fn glFrustumf(env: &mut Environment, left: GLfloat, right: GLfloat, bottom: GLfloat, top: GLfloat, near: GLfloat, far: GLfloat) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.Frustumf(left, right, bottom, top, near, far) }); }
fn glFrustumx(env: &mut Environment, left: GLfixed, right: GLfixed, bottom: GLfixed, top: GLfixed, near: GLfixed, far: GLfixed) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.Frustumx(left, right, bottom, top, near, far) }); }
fn glRotatef(env: &mut Environment, angle: GLfloat, x: GLfloat, y: GLfloat, z: GLfloat) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.Rotatef(angle, x, y, z) }); }
fn glRotatex(env: &mut Environment, angle: GLfixed, x: GLfixed, y: GLfixed, z: GLfixed) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.Rotatex(angle, x, y, z) }); }
fn glScalef(env: &mut Environment, x: GLfloat, y: GLfloat, z: GLfloat) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.Scalef(x, y, z) }); }
fn glScalex(env: &mut Environment, x: GLfixed, y: GLfixed, z: GLfixed) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.Scalex(x, y, z) }); }
fn glTranslatef(env: &mut Environment, x: GLfloat, y: GLfloat, z: GLfloat) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.Translatef(x, y, z) }); }
fn glTranslatex(env: &mut Environment, x: GLfixed, y: GLfixed, z: GLfixed) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.Translatex(x, y, z) }); }

fn glPixelStorei(env: &mut Environment, pname: GLenum, param: GLint) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.PixelStorei(pname, param) }) }
fn glReadPixels(env: &mut Environment, x: GLint, y: GLint, width: GLsizei, height: GLsizei, format: GLenum, type_: GLenum, pixels: MutVoidPtr) {
    with_ctx_and_mem(env, |gles, mem| {
        let pixels = {
            let pixel_count: GuestUSize = width.checked_mul(height).unwrap().try_into().unwrap();
            let size = image_size_estimate(pixel_count, format, type_);
            mem.ptr_at_mut(pixels.cast::<u8>(), size).cast::<GLvoid>()
        };
        unsafe { gles.ReadPixels(x, y, width, height, format, type_, pixels) }
    })
}
fn glGenTextures(env: &mut Environment, n: GLsizei, textures: MutPtr<GLuint>) {
    if env.framework_state.opengles.current_ctx_for_thread(env.current_thread).is_none() {
        for i in 0..n { env.mem.write(textures + (i as GuestUSize), (i + 1) as GLuint); }
        return;
    }
    with_ctx_and_mem(env, |gles, mem| {
        let n_usize: GuestUSize = n.try_into().unwrap();
        let textures = mem.ptr_at_mut(textures, n_usize);
        unsafe { gles.GenTextures(n, textures) }
    })
}
fn glDeleteTextures(env: &mut Environment, n: GLsizei, textures: ConstPtr<GLuint>) {
    with_ctx_and_mem(env, |gles, mem| {
        let n_usize: GuestUSize = n.try_into().unwrap();
        let textures = mem.ptr_at(textures, n_usize);
        unsafe { gles.DeleteTextures(n, textures) }
    })
}
fn glActiveTexture(env: &mut Environment, texture: GLenum) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.ActiveTexture(texture) }) }
fn glIsTexture(env: &mut Environment, texture: GLuint) -> GLboolean { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.IsTexture(texture) }) }
fn glBindTexture(env: &mut Environment, target: GLenum, texture: GLuint) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.BindTexture(target, texture) }) }
fn glTexParameteri(env: &mut Environment, target: GLenum, pname: GLenum, param: GLint) {
    if pname == gles11::TEXTURE_CROP_RECT_OES { return; }
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.TexParameteri(target, pname, param) })
}
fn glTexParameterf(env: &mut Environment, target: GLenum, pname: GLenum, param: GLfloat) {
    if pname == gles11::TEXTURE_CROP_RECT_OES { return; }
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.TexParameterf(target, pname, param) })
}
fn glTexParameterx(env: &mut Environment, target: GLenum, pname: GLenum, param: GLfixed) {
    if pname == gles11::TEXTURE_CROP_RECT_OES { return; }
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.TexParameterx(target, pname, param) })
}
fn glTexParameteriv(env: &mut Environment, target: GLenum, pname: GLenum, params: ConstPtr<GLint>) {
    if pname == gles11::TEXTURE_CROP_RECT_OES { return; }
    with_ctx_and_mem(env, |gles, mem| unsafe { let params = mem.ptr_at(params, 1); gles.TexParameteriv(target, pname, params) })
}
fn glTexParameterfv(env: &mut Environment, target: GLenum, pname: GLenum, params: ConstPtr<GLfloat>) {
    if pname == gles11::TEXTURE_CROP_RECT_OES { return; }
    with_ctx_and_mem(env, |gles, mem| unsafe { let params = mem.ptr_at(params, 1); gles.TexParameterfv(target, pname, params) })
}
fn glTexParameterxv(env: &mut Environment, target: GLenum, pname: GLenum, params: ConstPtr<GLfixed>) {
    if pname == gles11::TEXTURE_CROP_RECT_OES { return; }
    with_ctx_and_mem(env, |gles, mem| unsafe { let params = mem.ptr_at(params, 1); gles.TexParameterxv(target, pname, params) })
}
fn image_size_estimate(pixel_count: GuestUSize, format: GLenum, type_: GLenum) -> GuestUSize {
    let bytes_per_pixel: GuestUSize = match type_ {
        gles11::UNSIGNED_BYTE => match format {
            gles11::ALPHA | gles11::LUMINANCE => 1, gles11::LUMINANCE_ALPHA => 2, gles11::RGB => 3,
            gles11::RGBA | gles11::BGRA_EXT => 4, _ => panic!("Unexpected format {format:#x}"),
        },
        gles11::UNSIGNED_SHORT_5_6_5 | gles11::UNSIGNED_SHORT_4_4_4_4 | gles11::UNSIGNED_SHORT_5_5_5_1 => 2,
        _ => panic!("Unexpected type {type_:#x}"),
    };
    pixel_count.checked_mul(bytes_per_pixel).unwrap()
}
fn glTexImage2D(env: &mut Environment, target: GLenum, level: GLint, internalformat: GLint, width: GLsizei, height: GLsizei, border: GLint, format: GLenum, type_: GLenum, pixels: ConstVoidPtr) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let pixels = if pixels.is_null() { std::ptr::null() } else {
            let pixel_count: GuestUSize = width.checked_mul(height).unwrap().try_into().unwrap();
            let size = image_size_estimate(pixel_count, format, type_);
            mem.ptr_at(pixels.cast::<u8>(), size).cast::<GLvoid>()
        };
        gles.TexImage2D(target, level, internalformat, width, height, border, format, type_, pixels)
    })
}
fn glTexSubImage2D(env: &mut Environment, target: GLenum, level: GLint, xoffset: GLint, yoffset: GLint, width: GLsizei, height: GLsizei, format: GLenum, type_: GLenum, pixels: ConstVoidPtr) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let pixel_count: GuestUSize = width.checked_mul(height).unwrap().try_into().unwrap();
        let size = image_size_estimate(pixel_count, format, type_);
        let pixels = mem.ptr_at(pixels.cast::<u8>(), size).cast::<GLvoid>();
        gles.TexSubImage2D(target, level, xoffset, yoffset, width, height, format, type_, pixels)
    })
}
fn glCompressedTexImage2D(env: &mut Environment, target: GLenum, level: GLint, internalformat: GLenum, width: GLsizei, height: GLsizei, border: GLint, image_size: GLsizei, data: ConstVoidPtr) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let data = mem.ptr_at(data.cast::<u8>(), image_size.try_into().unwrap()).cast();
        gles.CompressedTexImage2D(target, level, internalformat, width, height, border, image_size, data)
    })
}
fn glCopyTexImage2D(env: &mut Environment, target: GLenum, level: GLint, internalformat: GLenum, x: GLint, y: GLint, width: GLsizei, height: GLsizei, border: GLint) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.CopyTexImage2D(target, level, internalformat, x, y, width, height, border) })
}
fn glCopyTexSubImage2D(env: &mut Environment, target: GLenum, level: GLint, xoffset: GLint, yoffset: GLint, x: GLint, y: GLint, width: GLsizei, height: GLsizei) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.CopyTexSubImage2D(target, level, xoffset, yoffset, x, y, width, height) })
}
fn glTexEnvf(env: &mut Environment, target: GLenum, pname: GLenum, param: GLfloat) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.TexEnvf(target, pname, param) }) }
fn glTexEnvx(env: &mut Environment, target: GLenum, pname: GLenum, param: GLfixed) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.TexEnvx(target, pname, param) }) }
fn glTexEnvi(env: &mut Environment, target: GLenum, pname: GLenum, param: GLint) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.TexEnvi(target, pname, param) }) }
fn glTexEnvfv(env: &mut Environment, target: GLenum, pname: GLenum, params: ConstPtr<GLfloat>) {
    assert!(target == gles11::TEXTURE_ENV || target == gles11::TEXTURE_FILTER_CONTROL_EXT);
    with_ctx_and_mem(env, |gles, mem| { let params = mem.ptr_at(params, 4); unsafe { gles.TexEnvfv(target, pname, params) } })
}
fn glTexEnvxv(env: &mut Environment, target: GLenum, pname: GLenum, params: ConstPtr<GLfixed>) {
    assert!(target == gles11::TEXTURE_ENV);
    with_ctx_and_mem(env, |gles, mem| { let params = mem.ptr_at(params, 4); unsafe { gles.TexEnvxv(target, pname, params) } })
}
fn glTexEnviv(env: &mut Environment, target: GLenum, pname: GLenum, params: ConstPtr<GLint>) {
    assert!(target == gles11::TEXTURE_ENV);
    with_ctx_and_mem(env, |gles, mem| { let params = mem.ptr_at(params, 4); unsafe { gles.TexEnviv(target, pname, params) } })
}
fn glMultiTexCoord4f(env: &mut Environment, target: GLenum, s: GLfloat, t: GLfloat, r: GLfloat, q: GLfloat) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.MultiTexCoord4f(target, s, t, r, q) }) }
fn glMultiTexCoord4x(env: &mut Environment, target: GLenum, s: GLfixed, t: GLfixed, r: GLfixed, q: GLfixed) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.MultiTexCoord4x(target, s, t, r, q) }) }

fn glGenFramebuffersOES(env: &mut Environment, n: GLsizei, framebuffers: MutPtr<GLuint>) {
    if env.framework_state.opengles.current_ctx_for_thread(env.current_thread).is_none() {
        for i in 0..n { env.mem.write(framebuffers + (i as GuestUSize), (i + 1) as GLuint); }
        return;
    }
    with_ctx_and_mem(env, |gles, mem| {
        let n_usize: GuestUSize = n.try_into().unwrap();
        let framebuffers = mem.ptr_at_mut(framebuffers, n_usize);
        unsafe { gles.GenFramebuffersOES(n, framebuffers) }
    })
}
fn glGenRenderbuffersOES(env: &mut Environment, n: GLsizei, renderbuffers: MutPtr<GLuint>) {
    if env.framework_state.opengles.current_ctx_for_thread(env.current_thread).is_none() {
        for i in 0..n { env.mem.write(renderbuffers + (i as GuestUSize), (i + 1) as GLuint); }
        return;
    }
    with_ctx_and_mem(env, |gles, mem| {
        let n_usize: GuestUSize = n.try_into().unwrap();
        let renderbuffers = mem.ptr_at_mut(renderbuffers, n_usize);
        unsafe { gles.GenRenderbuffersOES(n, renderbuffers) }
    })
}
fn glIsFramebufferOES(env: &mut Environment, framebuffer: GLuint) -> GLboolean { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.IsFramebufferOES(framebuffer) }) }
fn glIsRenderbufferOES(env: &mut Environment, renderbuffer: GLuint) -> GLboolean { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.IsRenderbufferOES(renderbuffer) }) }
fn glBindFramebufferOES(env: &mut Environment, target: GLenum, framebuffer: GLuint) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.BindFramebufferOES(target, framebuffer) }) }
fn glBindRenderbufferOES(env: &mut Environment, target: GLenum, renderbuffer: GLuint) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.BindRenderbufferOES(target, renderbuffer) }) }
fn glRenderbufferStorageOES(env: &mut Environment, target: GLenum, internalformat: GLenum, width: GLsizei, height: GLsizei) {
    let factor = env.options.scale_hack.get() as GLsizei;
    let (width, height) = (width * factor, height * factor);
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.RenderbufferStorageOES(target, internalformat, width, height) })
}
fn glFramebufferRenderbufferOES(env: &mut Environment, target: GLenum, attachment: GLenum, renderbuffertarget: GLenum, renderbuffer: GLuint) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.FramebufferRenderbufferOES(target, attachment, renderbuffertarget, renderbuffer) })
}
fn glFramebufferTexture2DOES(env: &mut Environment, target: GLenum, attachment: GLenum, textarget: GLenum, texture: GLuint, level: i32) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.FramebufferTexture2DOES(target, attachment, textarget, texture, level) })
}
fn glGetFramebufferAttachmentParameterivOES(env: &mut Environment, target: GLenum, attachment: GLenum, pname: GLenum, params: MutPtr<GLint>) {
    with_ctx_and_mem(env, |gles, mem| { let params = mem.ptr_at_mut(params, 1); unsafe { gles.GetFramebufferAttachmentParameterivOES(target, attachment, pname, params) } })
}
fn glGetRenderbufferParameterivOES(env: &mut Environment, target: GLenum, pname: GLenum, params: MutPtr<GLint>) {
    let factor = env.options.scale_hack.get() as GLint;
    with_ctx_and_mem(env, |gles, mem| {
        let params = mem.ptr_at_mut(params, 1);
        unsafe { gles.GetRenderbufferParameterivOES(target, pname, params) };
        if pname == gles11::RENDERBUFFER_WIDTH_OES || pname == gles11::RENDERBUFFER_HEIGHT_OES {
            unsafe { params.write_unaligned(params.read_unaligned() / factor) }
        }
    })
}
fn glCheckFramebufferStatusOES(env: &mut Environment, target: GLenum) -> GLenum { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.CheckFramebufferStatusOES(target) }) }
fn glDeleteFramebuffersOES(env: &mut Environment, n: GLsizei, framebuffers: ConstPtr<GLuint>) {
    with_ctx_and_mem(env, |gles, mem| {
        let n_usize: GuestUSize = n.try_into().unwrap();
        let framebuffers = mem.ptr_at(framebuffers, n_usize);
        unsafe { gles.DeleteFramebuffersOES(n, framebuffers) }
    })
}
fn glDeleteRenderbuffersOES(env: &mut Environment, n: GLsizei, renderbuffers: ConstPtr<GLuint>) {
    with_ctx_and_mem(env, |gles, mem| {
        let n_usize: GuestUSize = n.try_into().unwrap();
        let renderbuffers = mem.ptr_at(renderbuffers, n_usize);
        unsafe { gles.DeleteRenderbuffersOES(n, renderbuffers) }
    })
}
fn glGenerateMipmapOES(env: &mut Environment, target: GLenum) { with_ctx_and_mem(env, |gles, _mem| unsafe { gles.GenerateMipmapOES(target) }) }

fn glGenFramebuffers(env: &mut Environment, n: GLsizei, framebuffers: MutPtr<GLuint>) { glGenFramebuffersOES(env, n, framebuffers) }
fn glGenRenderbuffers(env: &mut Environment, n: GLsizei, renderbuffers: MutPtr<GLuint>) { glGenRenderbuffersOES(env, n, renderbuffers) }
fn glIsFramebuffer(env: &mut Environment, framebuffer: GLuint) -> GLboolean { glIsFramebufferOES(env, framebuffer) }
fn glIsRenderbuffer(env: &mut Environment, renderbuffer: GLuint) -> GLboolean { glIsRenderbufferOES(env, renderbuffer) }
fn glBindFramebuffer(env: &mut Environment, target: GLenum, framebuffer: GLuint) { glBindFramebufferOES(env, target, framebuffer) }
fn glBindRenderbuffer(env: &mut Environment, target: GLenum, renderbuffer: GLuint) { glBindRenderbufferOES(env, target, renderbuffer) }
fn glRenderbufferStorage(env: &mut Environment, target: GLenum, internalformat: GLenum, width: GLsizei, height: GLsizei) { glRenderbufferStorageOES(env, target, internalformat, width, height) }
fn glFramebufferRenderbuffer(env: &mut Environment, target: GLenum, attachment: GLenum, renderbuffertarget: GLenum, renderbuffer: GLuint) { glFramebufferRenderbufferOES(env, target, attachment, renderbuffertarget, renderbuffer) }
fn glFramebufferTexture2D(env: &mut Environment, target: GLenum, attachment: GLenum, textarget: GLenum, texture: GLuint, level: i32) { glFramebufferTexture2DOES(env, target, attachment, textarget, texture, level) }
fn glGetFramebufferAttachmentParameteriv(env: &mut Environment, target: GLenum, attachment: GLenum, pname: GLenum, params: MutPtr<GLint>) { glGetFramebufferAttachmentParameterivOES(env, target, attachment, pname, params) }
fn glGetRenderbufferParameteriv(env: &mut Environment, target: GLenum, pname: GLenum, params: MutPtr<GLint>) { glGetRenderbufferParameterivOES(env, target, pname, params) }
fn glCheckFramebufferStatus(env: &mut Environment, target: GLenum) -> GLenum { glCheckFramebufferStatusOES(env, target) }
fn glDeleteFramebuffers(env: &mut Environment, n: GLsizei, framebuffers: ConstPtr<GLuint>) { glDeleteFramebuffersOES(env, n, framebuffers) }
fn glDeleteRenderbuffers(env: &mut Environment, n: GLsizei, renderbuffers: ConstPtr<GLuint>) { glDeleteRenderbuffersOES(env, n, renderbuffers) }
fn glGenerateMipmap(env: &mut Environment, target: GLenum) { glGenerateMipmapOES(env, target) }

fn _get_currently_bound_buffer_object_name(
    env: &mut Environment,
    target: GLenum,
) -> GLuint {
    let binding = match target {
        ARRAY_BUFFER => VERTEX_ARRAY_BUFFER_BINDING,
        ELEMENT_ARRAY_BUFFER => ELEMENT_ARRAY_BUFFER_BINDING,
        _ => panic!("Unexpected buffer target {:#x}", target),
    };
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        let mut name: GLint = 0;
        gles.GetIntegerv(binding, &mut name);
        name as GLuint
    })
}

fn _get_buffer_size(env: &mut Environment, target: GLenum) -> GLint {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        let mut size: GLint = 0;
        gles.GetBufferParameteriv(target, gles11::BUFFER_SIZE, &mut size);
        size
    })
}

fn glGetBufferParameteriv(env: &mut Environment, target: GLenum, pname: GLenum, params: MutPtr<GLint>) {
    let params = env.mem.ptr_at_mut(params, 1);
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.GetBufferParameteriv(target, pname, params) })
}
fn glMapBufferOES(env: &mut Environment, target: GLenum, access: GLenum) -> MutPtr<GLvoid> {
    assert!(matches!(target, ARRAY_BUFFER | ELEMENT_ARRAY_BUFFER));
    assert!(access == WRITE_ONLY_OES);
    let buffer_object_name = _get_currently_bound_buffer_object_name(env, target);
    let host_buffer = with_ctx_and_mem_no_skip(env, |gles, _mem| unsafe { gles.MapBufferOES(target, access) });
    if host_buffer.is_null() { nil.cast() } else {
        let buffer_size = _get_buffer_size(env, target) as u32;
        let guest_buffer: MutVoidPtr = env.mem.alloc(buffer_size).cast();
        unsafe { env.mem.bytes_at_mut(guest_buffer.cast(), buffer_size).copy_from_slice(from_raw_parts(host_buffer as *mut u8, buffer_size as usize)); }
        let current_ctx = env.framework_state.opengles.current_ctx_for_thread(env.current_thread);
        let current_ctx_host_object = env.objc.borrow_mut::<EAGLContextHostObject>(current_ctx.unwrap());
        assert!(current_ctx_host_object.mapped_buffers.insert(buffer_object_name, (guest_buffer, host_buffer)).is_none());
        guest_buffer
    }
}
fn glUnmapBufferOES(env: &mut Environment, target: GLenum) -> GLboolean {
    let buffer_object_name = _get_currently_bound_buffer_object_name(env, target);
    let current_ctx = env.framework_state.opengles.current_ctx_for_thread(env.current_thread);
    let current_ctx_host_object = env.objc.borrow_mut::<EAGLContextHostObject>(current_ctx.unwrap());
    if let Some((guest_buffer, host_buffer)) = current_ctx_host_object.mapped_buffers.remove(&buffer_object_name) {
        let buffer_size = _get_buffer_size(env, target) as u32;
        unsafe { host_buffer.copy_from(env.mem.bytes_at(guest_buffer.cast(), buffer_size).as_ptr() as *mut GLvoid, buffer_size as usize); }
        env.mem.free(guest_buffer);
    }
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.UnmapBufferOES(target) })
}

// ============================================
// ЗАГЛУШКИ ДЛЯ OPENGL ES 2.0 ФУНКЦИЙ
// Эти функции нужны для совместимости с играми,
// которые пытаются использовать ES 2.0, но
// touchHLE поддерживает только ES 1.1
// ============================================

/// Создание шейдерной программы (ES 2.0) - заглушка
fn glCreateProgram(_env: &mut Environment) -> GLuint {
    log_once!("glCreateProgram() — ES 2.0 stub, returning 1");
    1 // Возвращаем фейковый ID программы
}

/// Создание шейдера (ES 2.0) - заглушка
fn glCreateShader(_env: &mut Environment, _type: GLenum) -> GLuint {
    log_once!("glCreateShader() — ES 2.0 stub, returning 1");
    1 // Возвращаем фейковый ID шейдера
}

/// Привязка атрибута (ES 2.0) - заглушка
fn glBindAttribLocation(_env: &mut Environment, _program: GLuint, _index: GLuint, _name: ConstPtr<GLubyte>) {
    log_once!("glBindAttribLocation() — ES 2.0 stub");
}

/// Получение uniform-переменной (ES 2.0) - заглушка
fn glGetUniformLocation(_env: &mut Environment, _program: GLuint, _name: ConstPtr<GLubyte>) -> GLint {
    log_once!("glGetUniformLocation() — ES 2.0 stub, returning -1");
    -1 // Возвращаем -1 (не найдено)
}

/// Установка uniform-матрицы (ES 2.0) - заглушка
fn glUniformMatrix4fv(_env: &mut Environment, _location: GLint, _count: GLsizei, _transpose: GLboolean, _value: ConstPtr<GLfloat>) {
    log_once!("glUniformMatrix4fv() — ES 2.0 stub");
}

/// Использование программы (ES 2.0) - заглушка
fn glUseProgram(_env: &mut Environment, _program: GLuint) {
    log_once!("glUseProgram() — ES 2.0 stub");
}

/// Удаление программы (ES 2.0) - заглушка
fn glDeleteProgram(_env: &mut Environment, _program: GLuint) {
    log_once!("glDeleteProgram() — ES 2.0 stub");
}

/// Удаление шейдера (ES 2.0) - заглушка
fn glDeleteShader(_env: &mut Environment, _shader: GLuint) {
    log_once!("glDeleteShader() — ES 2.0 stub");
}

/// Компиляция шейдера (ES 2.0) - заглушка
fn glCompileShader(_env: &mut Environment, _shader: GLuint) {
    log_once!("glCompileShader() — ES 2.0 stub");
}

/// Присоединение шейдера к программе (ES 2.0) - заглушка
fn glAttachShader(_env: &mut Environment, _program: GLuint, _shader: GLuint) {
    log_once!("glAttachShader() — ES 2.0 stub");
}

/// Линковка программы (ES 2.0) - заглушка
fn glLinkProgram(_env: &mut Environment, _program: GLuint) {
    log_once!("glLinkProgram() — ES 2.0 stub");
}

/// Получение параметра шейдера (ES 2.0) - заглушка
fn glGetShaderiv(env: &mut Environment, _shader: GLuint, _pname: GLenum, params: MutPtr<GLint>) {
    log_once!("glGetShaderiv() — ES 2.0 stub");
    // Возвращаем GL_TRUE для COMPILE_STATUS
    env.mem.write(params, 1);
}

/// Получение информационного лога шейдера (ES 2.0) - заглушка
fn glGetShaderInfoLog(env: &mut Environment, _shader: GLuint, _bufSize: GLsizei, length: MutPtr<GLsizei>, infoLog: MutPtr<GLubyte>) {
    log_once!("glGetShaderInfoLog() — ES 2.0 stub");
    if !length.is_null() {
        env.mem.write(length, 0);
    }
    if !infoLog.is_null() {
        env.mem.write(infoLog.cast::<u8>(), 0);
    }
}

/// Получение параметра программы (ES 2.0) - заглушка
fn glGetProgramiv(env: &mut Environment, _program: GLuint, _pname: GLenum, params: MutPtr<GLint>) {
    log_once!("glGetProgramiv() — ES 2.0 stub");
    // Возвращаем GL_TRUE для LINK_STATUS
    env.mem.write(params, 1);
}

/// Получение информационного лога программы (ES 2.0) - заглушка
fn glGetProgramInfoLog(env: &mut Environment, _program: GLuint, _bufSize: GLsizei, length: MutPtr<GLsizei>, infoLog: MutPtr<GLubyte>) {
    log_once!("glGetProgramInfoLog() — ES 2.0 stub");
    if !length.is_null() {
        env.mem.write(length, 0);
    }
    if !infoLog.is_null() {
        env.mem.write(infoLog.cast::<u8>(), 0);
    }
}

/// Исходный код шейдера (ES 2.0) - заглушка
fn glShaderSource(_env: &mut Environment, _shader: GLuint, _count: GLsizei, _string: ConstPtr<ConstPtr<GLubyte>>, _length: ConstPtr<GLint>) {
    log_once!("glShaderSource() — ES 2.0 stub");
}

/// Включение/отключение вершинного атрибута (ES 2.0) - заглушка
fn glEnableVertexAttribArray(_env: &mut Environment, _index: GLuint) {
    log_once!("glEnableVertexAttribArray() — ES 2.0 stub");
}

fn glDisableVertexAttribArray(_env: &mut Environment, _index: GLuint) {
    log_once!("glDisableVertexAttribArray() — ES 2.0 stub");
}

/// Указатель на вершинный атрибут (ES 2.0) - заглушка
fn glVertexAttribPointer(_env: &mut Environment, _index: GLuint, _size: GLint, _type: GLenum, _normalized: GLboolean, _stride: GLsizei, _pointer: ConstVoidPtr) {
    log_once!("glVertexAttribPointer() — ES 2.0 stub");
}

/// Установка uniform-переменных (ES 2.0) - заглушки
fn glUniform1i(_env: &mut Environment, _location: GLint, _v0: GLint) {
    log_once!("glUniform1i() — ES 2.0 stub");
}

fn glUniform1f(_env: &mut Environment, _location: GLint, _v0: GLfloat) {
    log_once!("glUniform1f() — ES 2.0 stub");
}

fn glUniform2f(_env: &mut Environment, _location: GLint, _v0: GLfloat, _v1: GLfloat) {
    log_once!("glUniform2f() — ES 2.0 stub");
}

fn glUniform3f(_env: &mut Environment, _location: GLint, _v0: GLfloat, _v1: GLfloat, _v2: GLfloat) {
    log_once!("glUniform3f() — ES 2.0 stub");
}

fn glUniform4f(_env: &mut Environment, _location: GLint, _v0: GLfloat, _v1: GLfloat, _v2: GLfloat, _v3: GLfloat) {
    log_once!("glUniform4f() — ES 2.0 stub");
}

/// Генерация VAO (ES 2.0) - заглушка
fn glGenVertexArrays(env: &mut Environment, n: GLsizei, arrays: MutPtr<GLuint>) {
    log_once!("glGenVertexArrays() — ES 2.0 stub");
    for i in 0..n {
        env.mem.write(arrays + (i as GuestUSize), (i + 1) as GLuint);
    }
}

/// Привязка VAO (ES 2.0) - заглушка
fn glBindVertexArray(_env: &mut Environment, _array: GLuint) {
    log_once!("glBindVertexArray() — ES 2.0 stub");
}

/// Удаление VAO (ES 2.0) - заглушка
fn glDeleteVertexArrays(_env: &mut Environment, _n: GLsizei, _arrays: ConstPtr<GLuint>) {
    log_once!("glDeleteVertexArrays() — ES 2.0 stub");
}

/// Генерация VBO (ES 2.0) - используем существующую реализацию
fn glGenBuffersES2(env: &mut Environment, n: GLsizei, buffers: MutPtr<GLuint>) {
    glGenBuffers(env, n, buffers)
}

/// Привязка VBO (ES 2.0) - используем существующую реализацию
fn glBindBufferES2(env: &mut Environment, target: GLenum, buffer: GLuint) {
    glBindBuffer(env, target, buffer)
}

/// Удаление VBO (ES 2.0) - используем существующую реализацию
fn glDeleteBuffersES2(env: &mut Environment, n: GLsizei, buffers: ConstPtr<GLuint>) {
    glDeleteBuffers(env, n, buffers)
}

/// Буферные данные (ES 2.0) - используем существующую реализацию
fn glBufferDataES2(env: &mut Environment, target: GLenum, size: GuestGLsizeiptr, data: ConstPtr<GLvoid>, usage: GLenum) {
    glBufferData(env, target, size, data, usage)
}

fn glBufferSubDataES2(env: &mut Environment, target: GLenum, offset: GuestGLintptr, size: GuestGLsizeiptr, data: ConstPtr<GLvoid>) {
    glBufferSubData(env, target, offset, size, data)
}

unsafe fn clamp_fog_state_values(gles: &mut dyn GLES) -> Option<(f32, f32)> {
    let mut fog_enabled: GLboolean = 0;
    gles.GetBooleanv(gles11::FOG, &mut fog_enabled);
    if fog_enabled != 0 {
        let mut fog_start: GLfloat = 0.0;
        let mut fog_end: GLfloat = 0.0;
        gles.GetFloatv(gles11::FOG_START, &mut fog_start);
        gles.GetFloatv(gles11::FOG_END, &mut fog_end);
        if fog_start == fog_end {
            let new_fog_start = fog_end - 0.001;
            gles.Fogf(gles11::FOG_START, new_fog_start);
            return Some((fog_start, fog_end));
        }
    }
    None
}
unsafe fn restore_fog_state_values(gles: &mut dyn GLES, from_backup: Option<(f32, f32)>) {
    if let Some((fog_start, fog_end)) = from_backup {
        gles.Fogf(gles11::FOG_START, fog_start);
        gles.Fogf(gles11::FOG_END, fog_end);
    }
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(glGetError()), export_c_func!(glEnable(_)), export_c_func!(glIsEnabled(_)),
    export_c_func!(glDisable(_)), export_c_func!(glClientActiveTexture(_)),
    export_c_func!(glEnableClientState(_)), export_c_func!(glDisableClientState(_)),
    export_c_func!(glGetBooleanv(_, _)), export_c_func!(glGetFloatv(_, _)),
    export_c_func!(glGetIntegerv(_, _)), export_c_func!(glGetPointerv(_, _)),
    export_c_func!(glGetTexEnviv(_, _, _)), export_c_func!(glGetTexEnvfv(_, _, _)),
    export_c_func!(glHint(_, _)), export_c_func!(glFinish()), export_c_func!(glFlush()),
    export_c_func!(glGetString(_)),
    export_c_func!(glAlphaFunc(_, _)), export_c_func!(glAlphaFuncx(_, _)),
    export_c_func!(glBlendFunc(_, _)), export_c_func!(glBlendEquationOES(_)),
    export_c_func!(glColorMask(_, _, _, _)),
    export_c_func!(glClipPlanef(_, _)), export_c_func!(glClipPlanex(_, _)),
    export_c_func!(glCullFace(_)), export_c_func!(glDepthFunc(_)),
    export_c_func!(glDepthMask(_)), export_c_func!(glDepthRangef(_, _)),
    export_c_func!(glDepthRangex(_, _)), export_c_func!(glFrontFace(_)),
    export_c_func!(glPolygonOffset(_, _)), export_c_func!(glPolygonOffsetx(_, _)),
    export_c_func!(glSampleCoverage(_, _)), export_c_func!(glSampleCoveragex(_, _)),
    export_c_func!(glShadeModel(_)), export_c_func!(glScissor(_, _, _, _)),
    export_c_func!(glViewport(_, _, _, _)),
    export_c_func!(glLineWidth(_)), export_c_func!(glLineWidthx(_)),
    export_c_func!(glStencilFunc(_, _, _)), export_c_func!(glStencilOp(_, _, _)),
    export_c_func!(glStencilMask(_)), export_c_func!(glLogicOp(_)),
    export_c_func!(glPointSize(_)), export_c_func!(glPointSizex(_)),
    export_c_func!(glPointParameterf(_, _)), export_c_func!(glPointParameterx(_, _)),
    export_c_func!(glPointParameterfv(_, _)), export_c_func!(glPointParameterxv(_, _)),
    export_c_func!(glFogf(_, _)), export_c_func!(glFogx(_, _)),
    export_c_func!(glFogfv(_, _)), export_c_func!(glFogxv(_, _)),
    export_c_func!(glLightf(_, _, _)), export_c_func!(glLightx(_, _, _)),
    export_c_func!(glLightfv(_, _, _)), export_c_func!(glLightxv(_, _, _)),
    export_c_func!(glLightModelf(_, _)), export_c_func!(glLightModelx(_, _)),
    export_c_func!(glLightModelfv(_, _)), export_c_func!(glLightModelxv(_, _)),
    export_c_func!(glMaterialf(_, _, _)), export_c_func!(glMaterialx(_, _, _)),
    export_c_func!(glMaterialfv(_, _, _)), export_c_func!(glMaterialxv(_, _, _)),
    export_c_func!(glIsBuffer(_)), export_c_func!(glGenBuffers(_, _)),
    export_c_func!(glDeleteBuffers(_, _)), export_c_func!(glBindBuffer(_, _)),
    export_c_func!(glBufferData(_, _, _, _)), export_c_func!(glBufferSubData(_, _, _, _)),
    export_c_func!(glColor4f(_, _, _, _)), export_c_func!(glColor4x(_, _, _, _)),
    export_c_func!(glColor4ub(_, _, _, _)),
    export_c_func!(glNormal3f(_, _, _)), export_c_func!(glNormal3x(_, _, _)),
    export_c_func!(glColorPointer(_, _, _, _)), export_c_func!(glNormalPointer(_, _, _)),
    export_c_func!(glTexCoordPointer(_, _, _, _)),
    export_c_func!(glVertexPointer(_, _, _, _)),
    export_c_func!(glPointSizePointerOES(_, _, _)),
    export_c_func!(glGetTexParameteriv(_, _, _)),
    export_c_func!(glDrawTexfOES(_, _, _, _, _)),
    export_c_func!(glDrawTexiOES(_, _, _, _, _)),
    export_c_func!(glDrawTexxOES(_, _, _, _, _)),
    export_c_func!(glDrawTexfvOES(_)), export_c_func!(glDrawTexivOES(_)),
    export_c_func!(glDrawTexxvOES(_)),
    export_c_func!(glRenderbufferStorageMultisampleAPPLE(_, _, _, _, _)),
    export_c_func!(glResolveMultisampleFramebufferAPPLE()),
    export_c_func!(glDiscardFramebufferEXT(_, _, _)),
    export_c_func!(glBindVertexArrayOES(_)),
    export_c_func!(glDeleteVertexArraysOES(_, _)),
    export_c_func!(glGenVertexArraysOES(_, _)),
    export_c_func!(glIsVertexArrayOES(_)),
    export_c_func!(glCurrentPaletteMatrixOES(_)),
    export_c_func!(glLoadPaletteFromModelViewMatrixOES()),
    export_c_func!(glMatrixIndexPointerOES(_, _, _, _)),
    export_c_func!(glWeightPointerOES(_, _, _, _)),
    export_c_func!(glGetBufferPointervOES(_, _, _)),
    export_c_func!(glDrawArrays(_, _, _)), export_c_func!(glDrawElements(_, _, _, _)),
    export_c_func!(glClear(_)), export_c_func!(glClearColor(_, _, _, _)),
    export_c_func!(glClearColorx(_, _, _, _)),
    export_c_func!(glClearDepthf(_)), export_c_func!(glClearDepthx(_)),
    export_c_func!(glClearStencil(_)),
    export_c_func!(glMatrixMode(_)), export_c_func!(glLoadIdentity()),
    export_c_func!(glLoadMatrixf(_)), export_c_func!(glLoadMatrixx(_)),
    export_c_func!(glMultMatrixf(_)), export_c_func!(glMultMatrixx(_)),
    export_c_func!(glPushMatrix()), export_c_func!(glPopMatrix()),
    export_c_func!(glOrthof(_, _, _, _, _, _)),
    export_c_func!(glOrthox(_, _, _, _, _, _)),
    export_c_func!(glFrustumf(_, _, _, _, _, _)),
    export_c_func!(glFrustumx(_, _, _, _, _, _)),
    export_c_func!(glRotatef(_, _, _, _)), export_c_func!(glRotatex(_, _, _, _)),
    export_c_func!(glScalef(_, _, _)), export_c_func!(glScalex(_, _, _)),
    export_c_func!(glTranslatef(_, _, _)), export_c_func!(glTranslatex(_, _, _)),
    export_c_func!(glPixelStorei(_, _)),
    export_c_func!(glReadPixels(_, _, _, _, _, _, _)),
    export_c_func!(glGenTextures(_, _)), export_c_func!(glDeleteTextures(_, _)),
    export_c_func!(glActiveTexture(_)), export_c_func!(glIsTexture(_)),
    export_c_func!(glBindTexture(_, _)),
    export_c_func!(glTexParameteri(_, _, _)), export_c_func!(glTexParameterf(_, _, _)),
    export_c_func!(glTexParameterx(_, _, _)), export_c_func!(glTexParameteriv(_, _, _)),
    export_c_func!(glTexParameterfv(_, _, _)), export_c_func!(glTexParameterxv(_, _, _)),
    export_c_func!(glTexImage2D(_, _, _, _, _, _, _, _, _)),
    export_c_func!(glTexSubImage2D(_, _, _, _, _, _, _, _, _)),
    export_c_func!(glCompressedTexImage2D(_, _, _, _, _, _, _, _)),
    export_c_func!(glCopyTexImage2D(_, _, _, _, _, _, _, _)),
    export_c_func!(glCopyTexSubImage2D(_, _, _, _, _, _, _, _)),
    export_c_func!(glTexEnvf(_, _, _)), export_c_func!(glTexEnvx(_, _, _)),
    export_c_func!(glTexEnvi(_, _, _)),
    export_c_func!(glTexEnvfv(_, _, _)), export_c_func!(glTexEnvxv(_, _, _)),
    export_c_func!(glTexEnviv(_, _, _)),
    export_c_func!(glMultiTexCoord4f(_, _, _, _, _)),
    export_c_func!(glMultiTexCoord4x(_, _, _, _, _)),
    export_c_func!(glGenFramebuffersOES(_, _)),
    export_c_func!(glGenRenderbuffersOES(_, _)),
    export_c_func!(glIsFramebufferOES(_)), export_c_func!(glIsRenderbufferOES(_)),
    export_c_func!(glBindFramebufferOES(_, _)),
    export_c_func!(glBindRenderbufferOES(_, _)),
    export_c_func!(glRenderbufferStorageOES(_, _, _, _)),
    export_c_func!(glFramebufferRenderbufferOES(_, _, _, _)),
    export_c_func!(glFramebufferTexture2DOES(_, _, _, _, _)),
    export_c_func!(glGetFramebufferAttachmentParameterivOES(_, _, _, _)),
    export_c_func!(glGetRenderbufferParameterivOES(_, _, _)),
    export_c_func!(glCheckFramebufferStatusOES(_)),
    export_c_func!(glDeleteFramebuffersOES(_, _)),
    export_c_func!(glDeleteRenderbuffersOES(_, _)),
    export_c_func!(glGenerateMipmapOES(_)),
    export_c_func!(glGenFramebuffers(_, _)),
    export_c_func!(glGenRenderbuffers(_, _)),
    export_c_func!(glIsFramebuffer(_)), export_c_func!(glIsRenderbuffer(_)),
    export_c_func!(glBindFramebuffer(_, _)),
    export_c_func!(glBindRenderbuffer(_, _)),
    export_c_func!(glRenderbufferStorage(_, _, _, _)),
    export_c_func!(glFramebufferRenderbuffer(_, _, _, _)),
    export_c_func!(glFramebufferTexture2D(_, _, _, _, _)),
    export_c_func!(glGetFramebufferAttachmentParameteriv(_, _, _, _)),
    export_c_func!(glGetRenderbufferParameteriv(_, _, _)),
    export_c_func!(glCheckFramebufferStatus(_)),
    export_c_func!(glDeleteFramebuffers(_, _)),
    export_c_func!(glDeleteRenderbuffers(_, _)),
    export_c_func!(glGenerateMipmap(_)),
    export_c_func!(glGetBufferParameteriv(_, _, _)),
    export_c_func!(glMapBufferOES(_, _)), export_c_func!(glUnmapBufferOES(_)),
    // ES 2.0 stubs
    export_c_func!(glCreateProgram()),
    export_c_func!(glCreateShader(_)),
    export_c_func!(glBindAttribLocation(_, _, _)),
    export_c_func!(glGetUniformLocation(_, _)),
    export_c_func!(glUniformMatrix4fv(_, _, _, _)),
    export_c_func!(glUseProgram(_)),
    export_c_func!(glDeleteProgram(_)),
    export_c_func!(glDeleteShader(_)),
    export_c_func!(glCompileShader(_)),
    export_c_func!(glAttachShader(_, _)),
    export_c_func!(glLinkProgram(_)),
    export_c_func!(glGetShaderiv(_, _, _)),
    export_c_func!(glGetShaderInfoLog(_, _, _, _)),
    export_c_func!(glGetProgramiv(_, _, _)),
    export_c_func!(glGetProgramInfoLog(_, _, _, _)),
    export_c_func!(glShaderSource(_, _, _, _)),
    export_c_func!(glEnableVertexAttribArray(_)),
    export_c_func!(glDisableVertexAttribArray(_)),
    export_c_func!(glVertexAttribPointer(_, _, _, _, _, _)),
    export_c_func!(glUniform1i(_, _)), export_c_func!(glUniform1f(_, _)),
    export_c_func!(glUniform2f(_, _, _)), export_c_func!(glUniform3f(_, _, _, _)),
    export_c_func!(glUniform4f(_, _, _, _, _)),
    export_c_func!(glGenVertexArrays(_, _)),
    export_c_func!(glBindVertexArray(_)),
    export_c_func!(glDeleteVertexArrays(_, _)),
];

