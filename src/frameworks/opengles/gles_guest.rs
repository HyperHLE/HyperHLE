/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

//! Wrapper functions exposing OpenGL ES to the guest.

use crate::dyld::{export_c_func, FunctionExports};
use crate::frameworks::opengles::eagl::EAGLContextHostObject;
use crate::gles::{gles11_raw as gles11, GLES};
use crate::mem::{ConstPtr, ConstVoidPtr, GuestISize, GuestUSize, Mem, MutPtr, MutVoidPtr, Ptr};
use crate::objc::nil;
use crate::Environment;
use std::slice::from_raw_parts;
use touchHLE_gl_bindings::gles11::{
    ARRAY_BUFFER, ELEMENT_ARRAY_BUFFER, ELEMENT_ARRAY_BUFFER_BINDING, VERTEX_ARRAY_BUFFER_BINDING,
    WRITE_ONLY_OES,
};

use crate::gles::gles11_raw::types::{
    GLbitfield, GLboolean, GLclampf, GLclampx, GLenum, GLfixed, GLfloat, GLint,
    GLintptr as HostGLintptr, GLsizei, GLsizeiptr as HostGLsizeiptr, GLubyte, GLuint, GLvoid,
};
type GuestGLsizeiptr = GuestISize;
type GuestGLintptr = GuestISize;

const SUPPORTED_COMPRESSED_TEXTURE_FORMATS: &[GLenum] = &[
    gles11::COMPRESSED_RGBA_PVRTC_2BPPV1_IMG,
    gles11::COMPRESSED_RGBA_PVRTC_4BPPV1_IMG,
    gles11::COMPRESSED_RGB_PVRTC_2BPPV1_IMG,
    gles11::COMPRESSED_RGB_PVRTC_4BPPV1_IMG,
    gles11::PALETTE4_R5_G6_B5_OES,
    gles11::PALETTE4_RGB5_A1_OES,
    gles11::PALETTE4_RGB8_OES,
    gles11::PALETTE4_RGBA4_OES,
    gles11::PALETTE4_RGBA8_OES,
    gles11::PALETTE8_R5_G6_B5_OES,
    gles11::PALETTE8_RGB5_A1_OES,
    gles11::PALETTE8_RGB8_OES,
    gles11::PALETTE8_RGBA4_OES,
    gles11::PALETTE8_RGBA8_OES,
];
#[track_caller]
fn with_ctx_and_mem<T, U: Default>(env: &mut Environment, f: T) -> U
where
    T: FnOnce(&mut dyn GLES, &mut Mem) -> U,
{
    if env
        .framework_state
        .opengles
        .current_ctx_for_thread(env.current_thread)
        .is_none()
    {
        log_dbg!(
            "Skipping GLES call without context (line {})",
            std::panic::Location::caller().line()
        );
        return U::default();
    }
    let trace = env.options.trace_gl_errors;
    let caller = std::panic::Location::caller();
    // `sync_context` now returns None when no GL context is bound to the
    // calling thread. The guard above already short-circuits the common
    // case where the *guest* never made a context current, but on edge
    // cases (e.g. context destroyed mid-call, headless GLES driver missing,
    // see HyperHLE log #5) we may still hit None here. Treat it the same
    // as the no-context branch above: log once and return the default.
    let Some(mut gles) = super::sync_context(
        &mut env.framework_state.opengles,
        &mut env.objc,
        env.window
            .as_mut()
            .expect("OpenGL ES is not supported in headless mode"),
        env.current_thread,
    ) else {
        log_dbg!(
            "Skipping GLES call after sync_context returned None (line {})",
            caller.line()
        );
        return U::default();
    };
    let res = f(gles.as_mut(), &mut env.mem);
    if trace {
        let err = unsafe { gles.GetError() };
        if err != 0 {
            log!(
                "[--trace-gl-errors] glGetError() = {:#x} after host GLES call \
                 dispatched from {}:{}",
                err,
                caller.file(),
                caller.line()
            );
        }
    }
    #[allow(clippy::let_and_return)]
    res
}

#[track_caller]
fn with_ctx_and_mem_no_skip<T, U: Default>(env: &mut Environment, f: T) -> U
where
    T: FnOnce(&mut dyn GLES, &mut Mem) -> U,
{
    let trace = env.options.trace_gl_errors;
    let caller = std::panic::Location::caller();
    // _no_skip historically panicked if there was no current context,
    // but real games (HyperHLE log #5 / Resident Evil 4) hit this on
    // worker threads that issue GL calls after `setCurrentContext:nil`.
    // Apple's documented behaviour is "GL calls silently fail" — mirror
    // that by returning the type's default instead of aborting the
    // emulator. The trade-off vs. with_ctx_and_mem is unchanged: we still
    // attempt the call when a context exists, even if it isn't the one
    // the guest expects.
    let Some(mut gles) = super::sync_context(
        &mut env.framework_state.opengles,
        &mut env.objc,
        env.window
            .as_mut()
            .expect("OpenGL ES is not supported in headless mode"),
        env.current_thread,
    ) else {
        log!(
            "Warning: with_ctx_and_mem_no_skip dispatched from {}:{} found \
             no current GL context; returning default.",
            caller.file(),
            caller.line()
        );
        return U::default();
    };
    let res = f(gles.as_mut(), &mut env.mem);
    if trace {
        let err = unsafe { gles.GetError() };
        if err != 0 {
            log!(
                "[--trace-gl-errors] glGetError() = {:#x} after host GLES call \
                 dispatched from {}:{}",
                err,
                caller.file(),
                caller.line()
            );
        }
    }
    #[allow(clippy::let_and_return)]
    res
}

fn glGetError(env: &mut Environment) -> GLenum {
    let ignore_gl_errors = env.options.ignore_gl_errors;
    with_ctx_and_mem(env, |gles, _mem| {
        let err = unsafe { gles.GetError() };
        if err != 0 {
            if ignore_gl_errors {
                return 0;
            }
            // Many engines (Unity, Cocos2d, Mono Game) call glGetError
            // every frame as an instrumentation hook. Once an error is
            // sticky in the host GL state machine (e.g. a strict Mali
            // driver returns GL_INVALID_ENUM for a call that the
            // emulator tolerated), every subsequent app-side
            // glGetError gets the same code, flooding the log. We
            // remember which error codes we've already reported and
            // demote repeats of the same code to log_dbg so the
            // diagnostic information is preserved (developers can
            // still use `--trace-gl-errors` to find the originating
            // call) but the console isn't drowning in identical lines.
            //
            // The set is per-process (no thread synchronisation
            // needed because GL contexts are accessed serialised
            // through `with_ctx_and_mem`); we cap it at 16 distinct
            // codes which is more than the entire OpenGL ES error
            // enum space.
            use std::sync::atomic::{AtomicU32, Ordering};
            const MAX_REPORTED: usize = 16;
            static REPORTED: [AtomicU32; MAX_REPORTED] = [
                AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
                AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
                AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
                AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
            ];
            let mut already_seen = false;
            for slot in &REPORTED {
                let cur = slot.load(Ordering::Relaxed);
                if cur == err {
                    already_seen = true;
                    break;
                }
                if cur == 0
                    && slot
                        .compare_exchange(0, err, Ordering::Relaxed, Ordering::Relaxed)
                        .is_ok()
                {
                    break;
                }
            }
            if already_seen {
                log_dbg!("glGetError() returned {:#x} (already reported)", err);
            } else {
                log!(
                    "Warning: glGetError() returned {:#x} (subsequent repeats \
                     of the same code are silenced; rerun with \
                     --trace-gl-errors to identify the originating GL call)",
                    err
                );
            }
        }
        err
    })
}
fn glEnable(env: &mut Environment, cap: GLenum) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.Enable(cap) });
}
fn glIsEnabled(env: &mut Environment, cap: GLenum) -> GLboolean {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.IsEnabled(cap) })
}
fn glDisable(env: &mut Environment, cap: GLenum) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.Disable(cap) });
}
fn glClientActiveTexture(env: &mut Environment, texture: GLenum) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.ClientActiveTexture(texture)
    })
}
fn glEnableClientState(env: &mut Environment, array: GLenum) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.EnableClientState(array) });
}
fn glDisableClientState(env: &mut Environment, array: GLenum) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.DisableClientState(array) });
}

fn glGetBooleanv(env: &mut Environment, pname: GLenum, params: MutPtr<GLboolean>) {
    if env
        .framework_state
        .opengles
        .current_ctx_for_thread(env.current_thread)
        .is_none()
    {
        env.mem.write(params, 0);
        return;
    }
    with_ctx_and_mem(env, |gles, mem| {
        let params = mem.ptr_at_mut(params, 16);
        unsafe { gles.GetBooleanv(pname, params) };
    });
}
fn glGetFloatv(env: &mut Environment, pname: GLenum, params: MutPtr<GLfloat>) {
    if env
        .framework_state
        .opengles
        .current_ctx_for_thread(env.current_thread)
        .is_none()
    {
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
        gles11::NUM_COMPRESSED_TEXTURE_FORMATS => {
            env.mem
                .write(params, SUPPORTED_COMPRESSED_TEXTURE_FORMATS.len() as _);
        }
        gles11::COMPRESSED_TEXTURE_FORMATS => {
            for (idx, &format) in SUPPORTED_COMPRESSED_TEXTURE_FORMATS.iter().enumerate() {
                env.mem.write(params + idx as GuestUSize, format as _);
            }
        }
        0x8cdf | 0x8d57 => {
            env.mem.write(params, 1 as _);
        }
        gles11::MAX_TEXTURE_SIZE => {
            env.mem.write(params, 2048 as _);
        }
        _ => {
            if env
                .framework_state
                .opengles
                .current_ctx_for_thread(env.current_thread)
                .is_none()
            {
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
    if env
        .framework_state
        .opengles
        .current_ctx_for_thread(env.current_thread)
        .is_none()
    {
        env.mem.write(params, Ptr::null());
        return;
    }
    use crate::gles::gles1_on_gl2::{ArrayInfo, ARRAYS};
    let &ArrayInfo { buffer_binding, .. } =
        ARRAYS.iter().find(|info| info.pointer == pname).unwrap();
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

fn glHint(env: &mut Environment, target: GLenum, mode: GLenum) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.Hint(target, mode) })
}
fn glFinish(env: &mut Environment) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.Finish() })
}
fn glFlush(env: &mut Environment) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.Flush() })
}
fn glGetString(env: &mut Environment, name: GLenum) -> ConstPtr<GLubyte> {
    // Per Apple's `EAGLContext` / OpenGL ES Programming Guide, the values
    // returned by `glGetString(GL_VERSION)` and `GL_SHADING_LANGUAGE_VERSION`
    // depend on which `kEAGLRenderingAPI` the context was created with:
    //
    //   * `kEAGLRenderingAPIOpenGLES1` – reports a `"OpenGL ES-CM 1.1 …"`
    //     version string (the `-CM` denotes the Common profile) and no
    //     `GL_SHADING_LANGUAGE_VERSION` entry.
    //   * `kEAGLRenderingAPIOpenGLES2` – reports `"OpenGL ES 2.0 …"` (note:
    //     NO `-CM`) plus `GL_SHADING_LANGUAGE_VERSION == "OpenGL ES GLSL ES
    //     1.00 …"`. Apps such as Unity 3.5 (Bad Piggies, iOS 4.0 build) use
    //     this string to decide whether to follow their ES 2.0 renderer code
    //     path. If we return the ES-CM 1.1 string while serving an ES 2.0
    //     context, Unity leaves itself in a half-initialised state and the
    //     resulting frames look torn / overlapped on screen.
    //
    // We therefore branch on the *currently-bound context's* API instead of
    // hard-coding the ES 1.1 strings. The cache is keyed by `(is_es2, name)`
    // because the two profiles share the same `name` enum values.
    let is_es2 = with_ctx_and_mem(env, |gles, _mem| gles.is_es2());

    if let Some(&str) = env
        .framework_state
        .opengles
        .strings_cache
        .get(&(is_es2, name))
    {
        return str;
    }

    let s: &[u8] = if !is_es2 {
        match name {
            gles11::VENDOR => b"Imagination Technologies",
            gles11::RENDERER => b"PowerVR MBXLite with VGPLite",
            gles11::VERSION => b"OpenGL ES-CM 1.1 (76)",
            gles11::EXTENSIONS => b"GL_APPLE_framebuffer_multisample GL_APPLE_texture_max_level GL_EXT_discard_framebuffer GL_EXT_texture_filter_anisotropic GL_EXT_texture_lod_bias GL_IMG_read_format GL_IMG_texture_compression_pvrtc GL_IMG_texture_format_BGRA8888 GL_OES_blend_subtract GL_OES_compressed_paletted_texture GL_OES_depth24 GL_OES_draw_texture GL_OES_framebuffer_object GL_OES_mapbuffer GL_OES_matrix_palette GL_OES_point_size_array GL_OES_point_sprite GL_OES_read_format GL_OES_rgb8_rgba8 GL_OES_texture_mirrored_repeat GL_OES_vertex_array_object ",
            _ => b"Unknown",
        }
    } else {
        // Strings reported by an iPhone 3GS / iPhone 4 running iOS 4.0 in an
        // `kEAGLRenderingAPIOpenGLES2` context. Crucially the `VERSION`
        // string does NOT contain the `-CM` Common-profile suffix, and the
        // `SHADING_LANGUAGE_VERSION` query is supported.
        match name {
            gles11::VENDOR => b"Imagination Technologies",
            gles11::RENDERER => b"PowerVR SGX 535",
            gles11::VERSION => b"OpenGL ES 2.0 IMGSGX535-63.27",
            // `GL_SHADING_LANGUAGE_VERSION` is 0x8B8C in both ES 2.0 and
            // desktop GL; reference it by numeric literal so we don't have
            // to pull in the ES 2.0 enum table here.
            0x8B8C => b"OpenGL ES GLSL ES 1.00",
            gles11::EXTENSIONS => b"GL_APPLE_framebuffer_multisample GL_APPLE_texture_max_level GL_EXT_discard_framebuffer GL_EXT_texture_filter_anisotropic GL_EXT_texture_lod_bias GL_IMG_read_format GL_IMG_texture_compression_pvrtc GL_IMG_texture_format_BGRA8888 GL_OES_depth24 GL_OES_depth_texture GL_OES_packed_depth_stencil GL_OES_rgb8_rgba8 GL_OES_standard_derivatives GL_OES_texture_float GL_OES_texture_half_float GL_OES_vertex_array_object ",
            _ => b"Unknown",
        }
    };
    let new_str = env.mem.alloc_and_write_cstr(s).cast_const();
    env.framework_state
        .opengles
        .strings_cache
        .insert((is_es2, name), new_str);
    new_str
}

fn glAlphaFunc(env: &mut Environment, func: GLenum, ref_: GLclampf) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.AlphaFunc(func, ref_) })
}
fn glAlphaFuncx(env: &mut Environment, func: GLenum, ref_: GLclampx) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.AlphaFuncx(func, ref_) })
}
fn glBlendFunc(env: &mut Environment, sfactor: GLenum, dfactor: GLenum) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.BlendFunc(sfactor, dfactor)
    })
}
fn glBlendEquationOES(env: &mut Environment, mode: GLenum) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.BlendEquationOES(mode) })
}
fn glColorMask(
    env: &mut Environment,
    red: GLboolean,
    green: GLboolean,
    blue: GLboolean,
    alpha: GLboolean,
) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.ColorMask(red, green, blue, alpha)
    })
}
fn glClipPlanef(env: &mut Environment, plane: GLenum, equation: ConstPtr<GLfloat>) {
    with_ctx_and_mem(env, |gles, mem| {
        let equation = mem.ptr_at(equation, 4);
        unsafe { gles.ClipPlanef(plane, equation) }
    })
}
fn glClipPlanex(env: &mut Environment, plane: GLenum, equation: ConstPtr<GLfixed>) {
    with_ctx_and_mem(env, |gles, mem| {
        let equation = mem.ptr_at(equation, 4);
        unsafe { gles.ClipPlanex(plane, equation) }
    })
}
fn glCullFace(env: &mut Environment, mode: GLenum) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.CullFace(mode) })
}
fn glDepthFunc(env: &mut Environment, func: GLenum) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.DepthFunc(func) })
}
fn glDepthMask(env: &mut Environment, flag: GLboolean) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.DepthMask(flag) })
}
fn glDepthRangef(env: &mut Environment, near: GLclampf, far: GLclampf) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.DepthRangef(near, far) })
}
fn glDepthRangex(env: &mut Environment, near: GLclampx, far: GLclampx) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.DepthRangex(near, far) })
}
fn glFrontFace(env: &mut Environment, mode: GLenum) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.FrontFace(mode) })
}
fn glPolygonOffset(env: &mut Environment, factor: GLfloat, units: GLfloat) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.PolygonOffset(factor, units)
    })
}
fn glPolygonOffsetx(env: &mut Environment, factor: GLfixed, units: GLfixed) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.PolygonOffsetx(factor, units)
    })
}
fn glSampleCoverage(env: &mut Environment, value: GLclampf, invert: GLboolean) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.SampleCoverage(value, invert)
    })
}
fn glSampleCoveragex(env: &mut Environment, value: GLclampx, invert: GLboolean) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.SampleCoveragex(value, invert)
    })
}
fn glShadeModel(env: &mut Environment, mode: GLenum) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.ShadeModel(mode) })
}
fn glScissor(env: &mut Environment, x: GLint, y: GLint, width: GLsizei, height: GLsizei) {
    {
        use std::sync::atomic::{AtomicBool, Ordering};
        static SEEN: AtomicBool = AtomicBool::new(false);
        if !SEEN.swap(true, Ordering::Relaxed) {
            log!(
                "First glScissor({}, {}, {}, {}) [this log will only be shown once]",
                x,
                y,
                width,
                height
            );
        }
    }
    let factor = env.options.scale_hack.get() as GLsizei;
    let (x, y) = (x * factor, y * factor);
    let (width, height) = (width * factor, height * factor);
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.Scissor(x, y, width, height)
    })
}
fn glViewport(env: &mut Environment, x: GLint, y: GLint, width: GLsizei, height: GLsizei) {
    {
        use std::sync::atomic::{AtomicBool, Ordering};
        static SEEN: AtomicBool = AtomicBool::new(false);
        if !SEEN.swap(true, Ordering::Relaxed) {
            log!(
                "First glViewport({}, {}, {}, {}) [this log will only be shown once]",
                x,
                y,
                width,
                height
            );
        }
    }
    let factor = env.options.scale_hack.get() as GLsizei;
    let (x, y) = (x * factor, y * factor);
    let (width, height) = (width * factor, height * factor);
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.Viewport(x, y, width, height)
    })
}
fn glLineWidth(env: &mut Environment, val: GLfloat) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.LineWidth(val) })
}
fn glLineWidthx(env: &mut Environment, val: GLfixed) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.LineWidthx(val) })
}
fn glStencilFunc(env: &mut Environment, func: GLenum, ref_: GLint, mask: GLuint) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.StencilFunc(func, ref_, mask)
    });
}
fn glStencilOp(env: &mut Environment, sfail: GLenum, dpfail: GLenum, dppass: GLenum) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.StencilOp(sfail, dpfail, dppass)
    });
}
fn glStencilMask(env: &mut Environment, mask: GLuint) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.StencilMask(mask) });
}
fn glLogicOp(env: &mut Environment, opcode: GLenum) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.LogicOp(opcode) });
}
fn glPointSize(env: &mut Environment, size: GLfloat) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.PointSize(size) })
}
fn glPointSizex(env: &mut Environment, size: GLfixed) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.PointSizex(size) })
}
fn glPointParameterf(env: &mut Environment, pname: GLenum, param: GLfloat) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.PointParameterf(pname, param)
    })
}
fn glPointParameterx(env: &mut Environment, pname: GLenum, param: GLfixed) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.PointParameterx(pname, param)
    })
}
fn glPointParameterfv(env: &mut Environment, pname: GLenum, params: ConstPtr<GLfloat>) {
    with_ctx_and_mem(env, |gles, mem| {
        let params = mem.ptr_at(params, 4);
        unsafe { gles.PointParameterfv(pname, params) }
    })
}
fn glPointParameterxv(env: &mut Environment, pname: GLenum, params: ConstPtr<GLfixed>) {
    with_ctx_and_mem(env, |gles, mem| {
        let params = mem.ptr_at(params, 4);
        unsafe { gles.PointParameterxv(pname, params) }
    })
}

fn glFogf(env: &mut Environment, pname: GLenum, param: GLfloat) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.Fogf(pname, param) })
}
fn glFogx(env: &mut Environment, pname: GLenum, param: GLfixed) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.Fogx(pname, param) })
}
fn glFogfv(env: &mut Environment, pname: GLenum, params: ConstPtr<GLfloat>) {
    with_ctx_and_mem(env, |gles, mem| {
        let params = mem.ptr_at(params, 4);
        unsafe { gles.Fogfv(pname, params) }
    })
}
fn glFogxv(env: &mut Environment, pname: GLenum, params: ConstPtr<GLfixed>) {
    with_ctx_and_mem(env, |gles, mem| {
        let params = mem.ptr_at(params, 4);
        unsafe { gles.Fogxv(pname, params) }
    })
}
fn glLightf(env: &mut Environment, light: GLenum, pname: GLenum, param: GLfloat) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.Lightf(light, pname, param)
    })
}
fn glLightx(env: &mut Environment, light: GLenum, pname: GLenum, param: GLfixed) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.Lightx(light, pname, param)
    })
}
fn glLightfv(env: &mut Environment, light: GLenum, pname: GLenum, params: ConstPtr<GLfloat>) {
    with_ctx_and_mem(env, |gles, mem| {
        let params = mem.ptr_at(params, 4);
        unsafe { gles.Lightfv(light, pname, params) }
    })
}
fn glLightxv(env: &mut Environment, light: GLenum, pname: GLenum, params: ConstPtr<GLfixed>) {
    with_ctx_and_mem(env, |gles, mem| {
        let params = mem.ptr_at(params, 4);
        unsafe { gles.Lightxv(light, pname, params) }
    })
}
fn glLightModelf(env: &mut Environment, pname: GLenum, param: GLfloat) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.LightModelf(pname, param) })
}
fn glLightModelx(env: &mut Environment, pname: GLenum, param: GLfixed) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.LightModelx(pname, param) })
}
fn glLightModelfv(env: &mut Environment, pname: GLenum, params: ConstPtr<GLfloat>) {
    with_ctx_and_mem(env, |gles, mem| {
        let params = mem.ptr_at(params, 4);
        unsafe { gles.LightModelfv(pname, params) }
    })
}
fn glLightModelxv(env: &mut Environment, pname: GLenum, params: ConstPtr<GLfixed>) {
    with_ctx_and_mem(env, |gles, mem| {
        let params = mem.ptr_at(params, 4);
        unsafe { gles.LightModelxv(pname, params) }
    })
}
fn glMaterialf(env: &mut Environment, face: GLenum, pname: GLenum, param: GLfloat) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.Materialf(face, pname, param)
    })
}
fn glMaterialx(env: &mut Environment, face: GLenum, pname: GLenum, param: GLfixed) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.Materialx(face, pname, param)
    })
}
fn glMaterialfv(env: &mut Environment, face: GLenum, pname: GLenum, params: ConstPtr<GLfloat>) {
    with_ctx_and_mem(env, |gles, mem| {
        let params = mem.ptr_at(params, 4);
        unsafe { gles.Materialfv(face, pname, params) }
    })
}
fn glMaterialxv(env: &mut Environment, face: GLenum, pname: GLenum, params: ConstPtr<GLfixed>) {
    with_ctx_and_mem(env, |gles, mem| {
        let params = mem.ptr_at(params, 4);
        unsafe { gles.Materialxv(face, pname, params) }
    })
}

fn glIsBuffer(env: &mut Environment, buffer: GLuint) -> GLboolean {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.IsBuffer(buffer) })
}
fn glGenBuffers(env: &mut Environment, n: GLsizei, buffers: MutPtr<GLuint>) {
    if env
        .framework_state
        .opengles
        .current_ctx_for_thread(env.current_thread)
        .is_none()
    {
        for i in 0..n {
            env.mem
                .write(buffers + (i as GuestUSize), (i + 1) as GLuint);
        }
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
fn glBindBuffer(env: &mut Environment, target: GLenum, buffer: GLuint) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.BindBuffer(target, buffer) })
}
fn glBufferData(
    env: &mut Environment,
    target: GLenum,
    size: GuestGLsizeiptr,
    data: ConstPtr<GLvoid>,
    usage: GLenum,
) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let data = if data.is_null() {
            std::ptr::null()
        } else {
            mem.ptr_at(data.cast::<u8>(), size.try_into().unwrap())
                .cast()
        };
        gles.BufferData(target, size as HostGLsizeiptr, data, usage)
    })
}
fn glBufferSubData(
    env: &mut Environment,
    target: GLenum,
    offset: GuestGLintptr,
    size: GuestGLsizeiptr,
    data: ConstPtr<GLvoid>,
) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let data = if data.is_null() {
            std::ptr::null()
        } else {
            mem.ptr_at(data.cast::<u8>(), size.try_into().unwrap())
                .cast()
        };
        gles.BufferSubData(target, offset as HostGLintptr, size as HostGLsizeiptr, data)
    })
}

fn glColor4f(env: &mut Environment, red: GLfloat, green: GLfloat, blue: GLfloat, alpha: GLfloat) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.Color4f(red, green, blue, alpha)
    })
}
fn glColor4x(env: &mut Environment, red: GLfixed, green: GLfixed, blue: GLfixed, alpha: GLfixed) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.Color4x(red, green, blue, alpha)
    })
}
fn glColor4ub(env: &mut Environment, red: GLubyte, green: GLubyte, blue: GLubyte, alpha: GLubyte) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.Color4ub(red, green, blue, alpha)
    })
}
fn glNormal3f(env: &mut Environment, nx: GLfloat, ny: GLfloat, nz: GLfloat) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.Normal3f(nx, ny, nz) })
}
fn glNormal3x(env: &mut Environment, nx: GLfixed, ny: GLfixed, nz: GLfixed) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.Normal3x(nx, ny, nz) })
}

unsafe fn translate_pointer_or_offset_to_host(
    gles: &mut dyn GLES,
    mem: &Mem,
    pointer_or_offset: ConstVoidPtr,
    which_binding: GLenum,
) -> *const GLvoid {
    let mut buffer_binding = 0;
    gles.GetIntegerv(which_binding, &mut buffer_binding);
    if buffer_binding != 0 {
        let offset = pointer_or_offset.to_bits();
        offset as usize as *const _
    } else if pointer_or_offset.is_null() {
        std::ptr::null()
    } else {
        mem.unchecked_ptr_at(pointer_or_offset.cast::<u8>(), 0)
            .cast::<GLvoid>()
    }
}
unsafe fn translate_pointer_or_offset_to_guest(
    gles: &mut dyn GLES,
    mem: &Mem,
    pointer_or_offset: *const GLvoid,
    which_binding: GLenum,
) -> ConstVoidPtr {
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

fn glColorPointer(
    env: &mut Environment,
    size: GLint,
    type_: GLenum,
    stride: GLsizei,
    pointer: ConstVoidPtr,
) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let pointer =
            translate_pointer_or_offset_to_host(gles, mem, pointer, gles11::ARRAY_BUFFER_BINDING);
        gles.ColorPointer(size, type_, stride, pointer)
    })
}
fn glNormalPointer(env: &mut Environment, type_: GLenum, stride: GLsizei, pointer: ConstVoidPtr) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let pointer =
            translate_pointer_or_offset_to_host(gles, mem, pointer, gles11::ARRAY_BUFFER_BINDING);
        gles.NormalPointer(type_, stride, pointer)
    })
}
fn glTexCoordPointer(
    env: &mut Environment,
    size: GLint,
    type_: GLenum,
    stride: GLsizei,
    pointer: ConstVoidPtr,
) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let pointer =
            translate_pointer_or_offset_to_host(gles, mem, pointer, gles11::ARRAY_BUFFER_BINDING);
        gles.TexCoordPointer(size, type_, stride, pointer)
    })
}
fn glVertexPointer(
    env: &mut Environment,
    size: GLint,
    type_: GLenum,
    stride: GLsizei,
    pointer: ConstVoidPtr,
) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let pointer =
            translate_pointer_or_offset_to_host(gles, mem, pointer, gles11::ARRAY_BUFFER_BINDING);
        gles.VertexPointer(size, type_, stride, pointer)
    })
}

/// Per the OES_point_size_array extension spec:
/// glPointSizePointerOES sets the pointer to an array of point sizes.
/// type must be GL_FIXED or GL_FLOAT. stride specifies byte offset between
/// consecutive point sizes. pointer is the address of the first element.
///
/// In our implementation we forward to the underlying GLES context which
/// handles the vertex attribute state machine.
fn glPointSizePointerOES(
    _env: &mut Environment,
    _type_: GLenum,
    _stride: GLsizei,
    _pointer: ConstVoidPtr,
) {
    // OES_point_size_array is not wired through the host GLES trait, so this
    // export is a no-op stub. Apps that rely on per-vertex point sizes (rare
    // outside of particle systems) will fall back to the constant point size
    // set via glPointSize.
}

/// Per OpenGL ES 1.1 spec section 3.7:
/// glGetTexParameteriv returns texture parameter values for the specified target.
/// pname can be: GL_TEXTURE_MIN_FILTER, GL_TEXTURE_MAG_FILTER,
/// GL_TEXTURE_WRAP_S, GL_TEXTURE_WRAP_T, GL_GENERATE_MIPMAP
fn glGetTexParameteriv(
    env: &mut Environment,
    _target: GLenum,
    _pname: GLenum,
    params: MutPtr<GLint>,
) {
    // The host GLES trait doesn't expose GetTexParameteriv. Return 0 (a valid
    // GL_NONE / default-ish answer for the parameters Apple games typically
    // query) so guest code that only checks "did the call succeed" still
    // proceeds.
    env.mem.write(params, 0);
}

fn glDrawTexfOES(
    _env: &mut Environment,
    _x: GLfloat,
    _y: GLfloat,
    _z: GLfloat,
    _width: GLfloat,
    _height: GLfloat,
) {
}
fn glDrawTexiOES(
    _env: &mut Environment,
    _x: GLint,
    _y: GLint,
    _z: GLint,
    _width: GLint,
    _height: GLint,
) {
}
fn glDrawTexxOES(
    _env: &mut Environment,
    _x: GLfixed,
    _y: GLfixed,
    _z: GLfixed,
    _width: GLfixed,
    _height: GLfixed,
) {
}
fn glDrawTexfvOES(_env: &mut Environment, _coords: ConstPtr<GLfloat>) {}
fn glDrawTexivOES(_env: &mut Environment, _coords: ConstPtr<GLint>) {}
fn glDrawTexxvOES(_env: &mut Environment, _coords: ConstPtr<GLfixed>) {}
fn glRenderbufferStorageMultisampleAPPLE(
    env: &mut Environment,
    target: GLenum,
    samples: GLsizei,
    internalformat: GLenum,
    width: GLsizei,
    height: GLsizei,
) {
    // Apply --scale-hack so an MSAA renderbuffer matches the size of the
    // single-sample one it'll be resolved into.
    let factor = env.options.scale_hack.get() as GLsizei;
    let (width, height) = (width * factor, height * factor);
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.RenderbufferStorageMultisampleAPPLE(target, samples, internalformat, width, height)
    })
}
fn glResolveMultisampleFramebufferAPPLE(env: &mut Environment) {
    // Apple's MSAA pattern: the app binds the sample (multisample) framebuffer
    // to GL_READ_FRAMEBUFFER_APPLE and the resolve (single-sample) framebuffer
    // (whose color renderbuffer is the CAEAGLLayer drawable) to
    // GL_DRAW_FRAMEBUFFER_APPLE, then calls this to copy the resolved pixels
    // into the drawable. Without this copy the resolve renderbuffer stays
    // empty and `presentRenderbuffer:` has nothing to display.
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.ResolveMultisampleFramebufferAPPLE()
    })
}
fn glDiscardFramebufferEXT(
    _env: &mut Environment,
    _target: GLenum,
    _numAttachments: GLsizei,
    _attachments: ConstPtr<GLenum>,
) {
}
fn glBindVertexArrayOES(_env: &mut Environment, _array: GLuint) {}
fn glDeleteVertexArraysOES(_env: &mut Environment, _n: GLsizei, _arrays: ConstPtr<GLuint>) {}
fn glGenVertexArraysOES(env: &mut Environment, n: GLsizei, arrays: MutPtr<GLuint>) {
    for i in 0..n {
        env.mem.write(arrays + (i as GuestUSize), (i + 1) as GLuint);
    }
}
fn glIsVertexArrayOES(_env: &mut Environment, _array: GLuint) -> GLboolean {
    0
}
fn glCurrentPaletteMatrixOES(_env: &mut Environment, _matrixpaletteindex: GLuint) {}
fn glLoadPaletteFromModelViewMatrixOES(_env: &mut Environment) {}
fn glMatrixIndexPointerOES(
    _env: &mut Environment,
    _size: GLint,
    _type_: GLenum,
    _stride: GLsizei,
    _pointer: ConstVoidPtr,
) {
}
fn glWeightPointerOES(
    _env: &mut Environment,
    _size: GLint,
    _type_: GLenum,
    _stride: GLsizei,
    _pointer: ConstVoidPtr,
) {
}
fn glGetBufferPointervOES(
    env: &mut Environment,
    _target: GLenum,
    _pname: GLenum,
    params: MutPtr<ConstVoidPtr>,
) {
    env.mem.write(params, Ptr::null());
}

fn glDrawArrays(env: &mut Environment, mode: GLenum, first: GLint, count: GLsizei) {
    {
        use std::sync::atomic::{AtomicBool, Ordering};
        static SEEN: AtomicBool = AtomicBool::new(false);
        if !SEEN.swap(true, Ordering::Relaxed) {
            log!(
                "First glDrawArrays(mode=0x{:x}, first={}, count={}) (app submitting first draw) [this log will only be shown once]",
                mode, first, count
            );
        }
    }
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        let fog_state_backup = clamp_fog_state_values(gles);
        gles.DrawArrays(mode, first, count);
        restore_fog_state_values(gles, fog_state_backup);
    })
}
fn glDrawElements(
    env: &mut Environment,
    mode: GLenum,
    count: GLsizei,
    type_: GLenum,
    indices: ConstVoidPtr,
) {
    {
        use std::sync::atomic::{AtomicBool, Ordering};
        static SEEN: AtomicBool = AtomicBool::new(false);
        if !SEEN.swap(true, Ordering::Relaxed) {
            log!(
                "First glDrawElements(mode=0x{:x}, count={}, type=0x{:x}) (app submitting first indexed draw) [this log will only be shown once]",
                mode, count, type_
            );
        }
    }
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let fog_state_backup = clamp_fog_state_values(gles);
        let indices = translate_pointer_or_offset_to_host(
            gles,
            mem,
            indices,
            gles11::ELEMENT_ARRAY_BUFFER_BINDING,
        );
        gles.DrawElements(mode, count, type_, indices);
        restore_fog_state_values(gles, fog_state_backup);
    })
}

fn glClear(env: &mut Environment, mask: GLbitfield) {
    {
        use std::sync::atomic::{AtomicBool, Ordering};
        static SEEN: AtomicBool = AtomicBool::new(false);
        if !SEEN.swap(true, Ordering::Relaxed) {
            log!(
                "First glClear(mask=0x{:x}) [this log will only be shown once]",
                mask
            );
        }
    }
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.Clear(mask) });
}
fn glClearColor(
    env: &mut Environment,
    red: GLclampf,
    green: GLclampf,
    blue: GLclampf,
    alpha: GLclampf,
) {
    {
        use std::sync::atomic::{AtomicBool, Ordering};
        static SEEN: AtomicBool = AtomicBool::new(false);
        if !SEEN.swap(true, Ordering::Relaxed) {
            log!(
                "First glClearColor({}, {}, {}, {}) [this log will only be shown once]",
                red,
                green,
                blue,
                alpha
            );
        }
    }
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.ClearColor(red, green, blue, alpha)
    });
}
fn glClearColorx(
    env: &mut Environment,
    red: GLclampx,
    green: GLclampx,
    blue: GLclampx,
    alpha: GLclampx,
) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.ClearColorx(red, green, blue, alpha)
    });
}
fn glClearDepthf(env: &mut Environment, depth: GLclampf) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.ClearDepthf(depth) });
}
fn glClearDepthx(env: &mut Environment, depth: GLclampx) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.ClearDepthx(depth) });
}
fn glClearStencil(env: &mut Environment, s: GLint) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.ClearStencil(s) });
}

fn glMatrixMode(env: &mut Environment, mode: GLenum) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.MatrixMode(mode) });
}
fn glLoadIdentity(env: &mut Environment) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.LoadIdentity() });
}
fn glLoadMatrixf(env: &mut Environment, m: ConstPtr<GLfloat>) {
    with_ctx_and_mem(env, |gles, mem| {
        let m = mem.ptr_at(m, 16);
        unsafe { gles.LoadMatrixf(m) };
    });
}
fn glLoadMatrixx(env: &mut Environment, m: ConstPtr<GLfixed>) {
    with_ctx_and_mem(env, |gles, mem| {
        let m = mem.ptr_at(m, 16);
        unsafe { gles.LoadMatrixx(m) };
    });
}
fn glMultMatrixf(env: &mut Environment, m: ConstPtr<GLfloat>) {
    with_ctx_and_mem(env, |gles, mem| {
        let m = mem.ptr_at(m, 16);
        unsafe { gles.MultMatrixf(m) };
    });
}
fn glMultMatrixx(env: &mut Environment, m: ConstPtr<GLfixed>) {
    with_ctx_and_mem(env, |gles, mem| {
        let m = mem.ptr_at(m, 16);
        unsafe { gles.MultMatrixx(m) };
    });
}
fn glPushMatrix(env: &mut Environment) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.PushMatrix() });
}
fn glPopMatrix(env: &mut Environment) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.PopMatrix() });
}
fn glOrthof(
    env: &mut Environment,
    left: GLfloat,
    right: GLfloat,
    bottom: GLfloat,
    top: GLfloat,
    near: GLfloat,
    far: GLfloat,
) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.Orthof(left, right, bottom, top, near, far)
    });
}
fn glOrthox(
    env: &mut Environment,
    left: GLfixed,
    right: GLfixed,
    bottom: GLfixed,
    top: GLfixed,
    near: GLfixed,
    far: GLfixed,
) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.Orthox(left, right, bottom, top, near, far)
    });
}
fn glFrustumf(
    env: &mut Environment,
    left: GLfloat,
    right: GLfloat,
    bottom: GLfloat,
    top: GLfloat,
    near: GLfloat,
    far: GLfloat,
) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.Frustumf(left, right, bottom, top, near, far)
    });
}
fn glFrustumx(
    env: &mut Environment,
    left: GLfixed,
    right: GLfixed,
    bottom: GLfixed,
    top: GLfixed,
    near: GLfixed,
    far: GLfixed,
) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.Frustumx(left, right, bottom, top, near, far)
    });
}
fn glRotatef(env: &mut Environment, angle: GLfloat, x: GLfloat, y: GLfloat, z: GLfloat) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.Rotatef(angle, x, y, z) });
}
fn glRotatex(env: &mut Environment, angle: GLfixed, x: GLfixed, y: GLfixed, z: GLfixed) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.Rotatex(angle, x, y, z) });
}
fn glScalef(env: &mut Environment, x: GLfloat, y: GLfloat, z: GLfloat) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.Scalef(x, y, z) });
}
fn glScalex(env: &mut Environment, x: GLfixed, y: GLfixed, z: GLfixed) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.Scalex(x, y, z) });
}
fn glTranslatef(env: &mut Environment, x: GLfloat, y: GLfloat, z: GLfloat) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.Translatef(x, y, z) });
}
fn glTranslatex(env: &mut Environment, x: GLfixed, y: GLfixed, z: GLfixed) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.Translatex(x, y, z) });
}

fn glPixelStorei(env: &mut Environment, pname: GLenum, param: GLint) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.PixelStorei(pname, param) })
}
fn glReadPixels(
    env: &mut Environment,
    x: GLint,
    y: GLint,
    width: GLsizei,
    height: GLsizei,
    format: GLenum,
    type_: GLenum,
    pixels: MutVoidPtr,
) {
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
    if env
        .framework_state
        .opengles
        .current_ctx_for_thread(env.current_thread)
        .is_none()
    {
        for i in 0..n {
            env.mem
                .write(textures + (i as GuestUSize), (i + 1) as GLuint);
        }
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
fn glActiveTexture(env: &mut Environment, texture: GLenum) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.ActiveTexture(texture) })
}
fn glIsTexture(env: &mut Environment, texture: GLuint) -> GLboolean {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.IsTexture(texture) })
}
fn glBindTexture(env: &mut Environment, target: GLenum, texture: GLuint) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.BindTexture(target, texture)
    })
}
/// If `pname` is `GL_TEXTURE_MIN_FILTER` and `param` is a mipmap min-filter
/// (e.g. `GL_NEAREST_MIPMAP_LINEAR`), return the closest non-mipmap value
/// (`GL_NEAREST` or `GL_LINEAR`). Otherwise return `param` unchanged.
///
/// On strict ES 1.1 drivers (notably ARM Mali r32p1) a texture that only had
/// level 0 uploaded becomes "incomplete" the moment the guest sets a mipmap
/// min-filter, and sampling such a texture returns black — which made the
/// LEGO Ninjago title menu render as a uniform-black quad on Mali-G57 MC2,
/// even though the LEGO splash logo (whose textures kept the touchHLE-forced
/// GL_LINEAR override from glTexImage2D) rendered fine.
fn demipmap_filter_value(pname: GLenum, param: GLint) -> GLint {
    if pname != gles11::TEXTURE_MIN_FILTER {
        return param;
    }
    let p = param as GLenum;
    let demipmapped = match p {
        gles11::NEAREST_MIPMAP_NEAREST | gles11::NEAREST_MIPMAP_LINEAR => gles11::NEAREST,
        gles11::LINEAR_MIPMAP_NEAREST | gles11::LINEAR_MIPMAP_LINEAR => gles11::LINEAR,
        _ => return param,
    };
    use std::sync::atomic::{AtomicBool, Ordering};
    static SEEN: AtomicBool = AtomicBool::new(false);
    if !SEEN.swap(true, Ordering::Relaxed) {
        log!(
            "First fix_texture_min_filter override: substituting guest's \
             glTexParameter(GL_TEXTURE_MIN_FILTER, 0x{:x}) with 0x{:x} \
             (mipmap modes leave the texture incomplete on strict ES 1.1 \
             drivers like Mali r32p1, which then samples them as black) \
             [this log will only be shown once]",
            p,
            demipmapped
        );
    }
    demipmapped as GLint
}

fn maybe_demipmap_min_filter(env: &Environment, pname: GLenum, param: GLint) -> GLint {
    if !env.options.fix_texture_min_filter {
        return param;
    }
    demipmap_filter_value(pname, param)
}

fn glTexParameteri(env: &mut Environment, target: GLenum, pname: GLenum, param: GLint) {
    {
        use std::sync::atomic::{AtomicBool, Ordering};
        static SEEN: AtomicBool = AtomicBool::new(false);
        if !SEEN.swap(true, Ordering::Relaxed) {
            log!(
                "First glTexParameteri(target=0x{:x}, pname=0x{:x}, param=0x{:x}) [this log will only be shown once]",
                target, pname, param as u32
            );
        }
    }
    if pname == gles11::TEXTURE_CROP_RECT_OES {
        return;
    }
    let param = maybe_demipmap_min_filter(env, pname, param);
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.TexParameteri(target, pname, param)
    })
}
fn glTexParameterf(env: &mut Environment, target: GLenum, pname: GLenum, param: GLfloat) {
    if pname == gles11::TEXTURE_CROP_RECT_OES {
        return;
    }
    // Floats can also be used to pass enum-valued min-filter params (an
    // OpenGL quirk), so route them through the same substitution.
    let param = maybe_demipmap_min_filter(env, pname, param as GLint) as GLfloat;
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.TexParameterf(target, pname, param)
    })
}
fn glTexParameterx(env: &mut Environment, target: GLenum, pname: GLenum, param: GLfixed) {
    if pname == gles11::TEXTURE_CROP_RECT_OES {
        return;
    }
    // Fixed-point can also encode enum values; route through substitution.
    let param = maybe_demipmap_min_filter(env, pname, param) as GLfixed;
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.TexParameterx(target, pname, param)
    })
}
fn glTexParameteriv(env: &mut Environment, target: GLenum, pname: GLenum, params: ConstPtr<GLint>) {
    if pname == gles11::TEXTURE_CROP_RECT_OES {
        return;
    }
    let fix_min_filter = env.options.fix_texture_min_filter && pname == gles11::TEXTURE_MIN_FILTER;
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let params_ptr = mem.ptr_at(params, 1);
        if fix_min_filter {
            let original: GLint = *params_ptr;
            let substituted = demipmap_filter_value(pname, original);
            if substituted != original {
                let v = [substituted];
                gles.TexParameteriv(target, pname, v.as_ptr());
                return;
            }
        }
        gles.TexParameteriv(target, pname, params_ptr)
    })
}
fn glTexParameterfv(
    env: &mut Environment,
    target: GLenum,
    pname: GLenum,
    params: ConstPtr<GLfloat>,
) {
    if pname == gles11::TEXTURE_CROP_RECT_OES {
        return;
    }
    let fix_min_filter = env.options.fix_texture_min_filter && pname == gles11::TEXTURE_MIN_FILTER;
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let params_ptr = mem.ptr_at(params, 1);
        if fix_min_filter {
            let original: GLfloat = *params_ptr;
            let substituted = demipmap_filter_value(pname, original as GLint) as GLfloat;
            if substituted != original {
                let v = [substituted];
                gles.TexParameterfv(target, pname, v.as_ptr());
                return;
            }
        }
        gles.TexParameterfv(target, pname, params_ptr)
    })
}
fn glTexParameterxv(
    env: &mut Environment,
    target: GLenum,
    pname: GLenum,
    params: ConstPtr<GLfixed>,
) {
    if pname == gles11::TEXTURE_CROP_RECT_OES {
        return;
    }
    let fix_min_filter = env.options.fix_texture_min_filter && pname == gles11::TEXTURE_MIN_FILTER;
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let params_ptr = mem.ptr_at(params, 1);
        if fix_min_filter {
            let original: GLfixed = *params_ptr;
            let substituted = demipmap_filter_value(pname, original) as GLfixed;
            if substituted != original {
                let v = [substituted];
                gles.TexParameterxv(target, pname, v.as_ptr());
                return;
            }
        }
        gles.TexParameterxv(target, pname, params_ptr)
    })
}
fn image_size_estimate(pixel_count: GuestUSize, format: GLenum, type_: GLenum) -> GuestUSize {
    // This is only an upper-bound estimate used for memory tracking, not
    // anything OpenGL actually needs. Unknown combos should yield a
    // conservative "don't try to copy guest pixels" sentinel (0) and log,
    // not crash the host. The driver will catch real format issues.
    let bytes_per_pixel: Option<GuestUSize> = match type_ {
        gles11::UNSIGNED_BYTE => match format {
            gles11::ALPHA | gles11::LUMINANCE => Some(1),
            gles11::LUMINANCE_ALPHA => Some(2),
            gles11::RGB => Some(3),
            gles11::RGBA | gles11::BGRA_EXT => Some(4),
            _ => None,
        },
        gles11::UNSIGNED_SHORT_5_6_5
        | gles11::UNSIGNED_SHORT_4_4_4_4
        | gles11::UNSIGNED_SHORT_5_5_5_1 => Some(2),
        _ => None,
    };
    let Some(bpp) = bytes_per_pixel else {
        log!(
            "Warning: image_size_estimate(): unsupported format/type combination (format={:#x}, type={:#x}); treating as 0 bytes.",
            format,
            type_
        );
        return 0;
    };
    pixel_count.checked_mul(bpp).unwrap_or_else(|| {
        log!(
            "Warning: image_size_estimate(): pixel_count {} * bpp {} overflowed GuestUSize; returning u32::MAX.",
            pixel_count,
            bpp
        );
        GuestUSize::MAX
    })
}
fn glTexImage2D(
    env: &mut Environment,
    target: GLenum,
    level: GLint,
    internalformat: GLint,
    width: GLsizei,
    height: GLsizei,
    border: GLint,
    format: GLenum,
    type_: GLenum,
    pixels: ConstVoidPtr,
) {
    {
        use std::sync::atomic::{AtomicBool, Ordering};
        static SEEN: AtomicBool = AtomicBool::new(false);
        if !SEEN.swap(true, Ordering::Relaxed) {
            log!(
                "First glTexImage2D({}x{}, internalformat=0x{:x}, format=0x{:x}, type=0x{:x}) (app uploading texture data) [this log will only be shown once]",
                width, height, internalformat as u32, format, type_
            );
        }
    }
    let fix_filter = env.options.fix_texture_min_filter && level == 0;
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let pixels = if pixels.is_null() {
            std::ptr::null()
        } else {
            let pixel_count: GuestUSize = width.checked_mul(height).unwrap().try_into().unwrap();
            let size = image_size_estimate(pixel_count, format, type_);
            mem.ptr_at(pixels.cast::<u8>(), size).cast::<GLvoid>()
        };
        gles.TexImage2D(
            target,
            level,
            internalformat,
            width,
            height,
            border,
            format,
            type_,
            pixels,
        );
        if fix_filter {
            // Set GL_TEXTURE_MIN_FILTER to GL_LINEAR for the bound
            // texture so it isn't sampled as opaque black on strict
            // ES 1.1 drivers (notably Qualcomm Adreno) just because
            // the guest never bothered to override the default
            // GL_NEAREST_MIPMAP_LINEAR. The guest's own
            // glTexParameteri(GL_TEXTURE_MIN_FILTER, …) will override
            // this on subsequent calls — see Options::fix_texture_min_filter.
            static SEEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
            if !SEEN.swap(true, std::sync::atomic::Ordering::Relaxed) {
                log!(
                    "First fix_texture_min_filter override: forcing GL_TEXTURE_MIN_FILTER=GL_LINEAR after glTexImage2D(level=0) [this log will only be shown once]"
                );
            }
            gles.TexParameteri(target, gles11::TEXTURE_MIN_FILTER, gles11::LINEAR as GLint);
        }
    })
}
fn glTexSubImage2D(
    env: &mut Environment,
    target: GLenum,
    level: GLint,
    xoffset: GLint,
    yoffset: GLint,
    width: GLsizei,
    height: GLsizei,
    format: GLenum,
    type_: GLenum,
    pixels: ConstVoidPtr,
) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let pixel_count: GuestUSize = width.checked_mul(height).unwrap().try_into().unwrap();
        let size = image_size_estimate(pixel_count, format, type_);
        let pixels = mem.ptr_at(pixels.cast::<u8>(), size).cast::<GLvoid>();
        gles.TexSubImage2D(
            target, level, xoffset, yoffset, width, height, format, type_, pixels,
        )
    })
}
fn glCompressedTexImage2D(
    env: &mut Environment,
    target: GLenum,
    level: GLint,
    internalformat: GLenum,
    width: GLsizei,
    height: GLsizei,
    border: GLint,
    image_size: GLsizei,
    data: ConstVoidPtr,
) {
    {
        use std::sync::atomic::{AtomicBool, Ordering};
        static SEEN: AtomicBool = AtomicBool::new(false);
        if !SEEN.swap(true, Ordering::Relaxed) {
            log!(
                "First glCompressedTexImage2D({}x{}, internalformat=0x{:x}, image_size={}) (app uploading PVRTC/compressed texture) [this log will only be shown once]",
                width, height, internalformat, image_size
            );
        }
    }
    let fix_filter = env.options.fix_texture_min_filter && level == 0;
    with_ctx_and_mem(env, |gles, mem| unsafe {
        // Pre-flight: drain any sticky GL error left by the previous call
        // (e.g. an oversized RGBA8 glTexImage2D on a strict Mali driver),
        // so when we check post-upload below we can attribute a fresh error
        // to *this* compressed upload only. Without this, every PVRTC /
        // paletted upload that follows a failed glTexImage2D would report
        // a misleading "compressed upload failed" on the very first call —
        // that's exactly what was happening on Mali-G57 with Temple Run
        // (HyperHLE log: 1024x1024 RGBA8 upload immediately followed by
        // 256x256 PVRTC upload, both blamed on the PVRTC path).
        let mut drained = 0u32;
        loop {
            let e = gles.GetError();
            if e == 0 {
                break;
            }
            drained += 1;
            if drained > 16 {
                // Pathological driver that won't clear; bail out instead of
                // looping forever.
                break;
            }
        }

        let data = mem
            .ptr_at(data.cast::<u8>(), image_size.try_into().unwrap())
            .cast();
        gles.CompressedTexImage2D(
            target,
            level,
            internalformat,
            width,
            height,
            border,
            image_size,
            data,
        );

        // Post-flight: if the upload itself produced a fresh error (and the
        // backend didn't already software-decode the payload to RGBA8 via
        // glTexImage2D, which is the success path), report it once with
        // enough context for the user to file a bug. This is `log!`
        // (visible at default level) but rate-limited per (format, error)
        // pair so a frame-by-frame texture stream can't flood the console.
        let post_err = gles.GetError();
        if post_err != 0 {
            use std::sync::atomic::{AtomicU64, Ordering};
            // Pack (internalformat << 32) | post_err into a single 64-bit
            // slot per "ever seen" entry; cap at 8 distinct entries.
            const MAX_REPORTED: usize = 8;
            static REPORTED: [AtomicU64; MAX_REPORTED] = [
                AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0),
                AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0),
            ];
            let key: u64 = ((internalformat as u64) << 32) | (post_err as u64);
            let mut already_seen = false;
            for slot in &REPORTED {
                let cur = slot.load(Ordering::Relaxed);
                if cur == key {
                    already_seen = true;
                    break;
                }
                if cur == 0
                    && slot
                        .compare_exchange(0, key, Ordering::Relaxed, Ordering::Relaxed)
                        .is_ok()
                {
                    break;
                }
            }
            if !already_seen {
                log!(
                    "Warning: glCompressedTexImage2D: host driver returned \
                     {post_err:#x} for {width}x{height} level {level} \
                     internalformat {internalformat:#x} (image_size={image_size}). \
                     This is the SOURCE of the GL error, not a sticky one — \
                     {drained} pre-existing error(s) were drained beforehand. \
                     [this format/error pair will be reported once]"
                );
            }
        }

        if fix_filter {
            gles.TexParameteri(target, gles11::TEXTURE_MIN_FILTER, gles11::LINEAR as GLint);
        }
    })
}
fn glCopyTexImage2D(
    env: &mut Environment,
    target: GLenum,
    level: GLint,
    internalformat: GLenum,
    x: GLint,
    y: GLint,
    width: GLsizei,
    height: GLsizei,
    border: GLint,
) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.CopyTexImage2D(target, level, internalformat, x, y, width, height, border)
    })
}
fn glCopyTexSubImage2D(
    env: &mut Environment,
    target: GLenum,
    level: GLint,
    xoffset: GLint,
    yoffset: GLint,
    x: GLint,
    y: GLint,
    width: GLsizei,
    height: GLsizei,
) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.CopyTexSubImage2D(target, level, xoffset, yoffset, x, y, width, height)
    })
}
fn glTexEnvf(env: &mut Environment, target: GLenum, pname: GLenum, param: GLfloat) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.TexEnvf(target, pname, param)
    })
}
fn glTexEnvx(env: &mut Environment, target: GLenum, pname: GLenum, param: GLfixed) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.TexEnvx(target, pname, param)
    })
}
fn glTexEnvi(env: &mut Environment, target: GLenum, pname: GLenum, param: GLint) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.TexEnvi(target, pname, param)
    })
}
fn glTexEnvfv(env: &mut Environment, target: GLenum, pname: GLenum, params: ConstPtr<GLfloat>) {
    assert!(target == gles11::TEXTURE_ENV || target == gles11::TEXTURE_FILTER_CONTROL_EXT);
    with_ctx_and_mem(env, |gles, mem| {
        let params = mem.ptr_at(params, 4);
        unsafe { gles.TexEnvfv(target, pname, params) }
    })
}
fn glTexEnvxv(env: &mut Environment, target: GLenum, pname: GLenum, params: ConstPtr<GLfixed>) {
    assert!(target == gles11::TEXTURE_ENV);
    with_ctx_and_mem(env, |gles, mem| {
        let params = mem.ptr_at(params, 4);
        unsafe { gles.TexEnvxv(target, pname, params) }
    })
}
fn glTexEnviv(env: &mut Environment, target: GLenum, pname: GLenum, params: ConstPtr<GLint>) {
    assert!(target == gles11::TEXTURE_ENV);
    with_ctx_and_mem(env, |gles, mem| {
        let params = mem.ptr_at(params, 4);
        unsafe { gles.TexEnviv(target, pname, params) }
    })
}
fn glMultiTexCoord4f(
    env: &mut Environment,
    target: GLenum,
    s: GLfloat,
    t: GLfloat,
    r: GLfloat,
    q: GLfloat,
) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.MultiTexCoord4f(target, s, t, r, q)
    })
}
fn glMultiTexCoord4x(
    env: &mut Environment,
    target: GLenum,
    s: GLfixed,
    t: GLfixed,
    r: GLfixed,
    q: GLfixed,
) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.MultiTexCoord4x(target, s, t, r, q)
    })
}

fn glGenFramebuffersOES(env: &mut Environment, n: GLsizei, framebuffers: MutPtr<GLuint>) {
    if env
        .framework_state
        .opengles
        .current_ctx_for_thread(env.current_thread)
        .is_none()
    {
        for i in 0..n {
            env.mem
                .write(framebuffers + (i as GuestUSize), (i + 1) as GLuint);
        }
        return;
    }
    with_ctx_and_mem(env, |gles, mem| {
        let n_usize: GuestUSize = n.try_into().unwrap();
        let framebuffers = mem.ptr_at_mut(framebuffers, n_usize);
        unsafe { gles.GenFramebuffersOES(n, framebuffers) }
    })
}
fn glGenRenderbuffersOES(env: &mut Environment, n: GLsizei, renderbuffers: MutPtr<GLuint>) {
    if env
        .framework_state
        .opengles
        .current_ctx_for_thread(env.current_thread)
        .is_none()
    {
        for i in 0..n {
            env.mem
                .write(renderbuffers + (i as GuestUSize), (i + 1) as GLuint);
        }
        return;
    }
    with_ctx_and_mem(env, |gles, mem| {
        let n_usize: GuestUSize = n.try_into().unwrap();
        let renderbuffers = mem.ptr_at_mut(renderbuffers, n_usize);
        unsafe { gles.GenRenderbuffersOES(n, renderbuffers) }
    })
}
fn glIsFramebufferOES(env: &mut Environment, framebuffer: GLuint) -> GLboolean {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.IsFramebufferOES(framebuffer)
    })
}
fn glIsRenderbufferOES(env: &mut Environment, renderbuffer: GLuint) -> GLboolean {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.IsRenderbufferOES(renderbuffer)
    })
}
fn glBindFramebufferOES(env: &mut Environment, target: GLenum, framebuffer: GLuint) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.BindFramebufferOES(target, framebuffer)
    })
}
fn glBindRenderbufferOES(env: &mut Environment, target: GLenum, renderbuffer: GLuint) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.BindRenderbufferOES(target, renderbuffer)
    })
}
fn glRenderbufferStorageOES(
    env: &mut Environment,
    target: GLenum,
    internalformat: GLenum,
    width: GLsizei,
    height: GLsizei,
) {
    let factor = env.options.scale_hack.get() as GLsizei;
    let (width, height) = (width * factor, height * factor);
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.RenderbufferStorageOES(target, internalformat, width, height)
    })
}
fn glFramebufferRenderbufferOES(
    env: &mut Environment,
    target: GLenum,
    attachment: GLenum,
    renderbuffertarget: GLenum,
    renderbuffer: GLuint,
) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.FramebufferRenderbufferOES(target, attachment, renderbuffertarget, renderbuffer);
        // When an iOS app attaches its drawable color renderbuffer to an FBO,
        // it commonly forgets to also attach a depth renderbuffer. On real iOS
        // GPUs / desktop GL the missing depth attachment is treated leniently,
        // but a strict OpenGL ES 2.0 driver (e.g. Mesa or Mali) will then
        // discard every drawn fragment when the app enables GL_DEPTH_TEST.
        // To match the lenient behaviour we auto-create and attach a matching
        // depth renderbuffer the first time a color attachment is set up on a
        // user-managed framebuffer.
        if gles.is_es2()
            && attachment == gles11::COLOR_ATTACHMENT0_OES
            && renderbuffertarget == gles11::RENDERBUFFER_OES
            && renderbuffer != 0
        {
            // Don't clobber an explicitly-attached depth attachment.
            let mut depth_attached: GLint = 0;
            gles.GetFramebufferAttachmentParameterivOES(
                target,
                gles11::DEPTH_ATTACHMENT_OES,
                gles11::FRAMEBUFFER_ATTACHMENT_OBJECT_NAME_OES,
                &mut depth_attached,
            );
            // Drain any error from the lookup (e.g. on freshly-attached FBO).
            while gles.GetError() != 0 {}
            if depth_attached == 0 {
                // Query renderbuffer size from the just-attached color RB.
                let mut prev_rb: GLint = 0;
                gles.GetIntegerv(gles11::RENDERBUFFER_BINDING_OES, &mut prev_rb);
                gles.BindRenderbufferOES(gles11::RENDERBUFFER_OES, renderbuffer);
                let mut w: GLint = 0;
                let mut h: GLint = 0;
                gles.GetRenderbufferParameterivOES(
                    gles11::RENDERBUFFER_OES,
                    gles11::RENDERBUFFER_WIDTH_OES,
                    &mut w,
                );
                gles.GetRenderbufferParameterivOES(
                    gles11::RENDERBUFFER_OES,
                    gles11::RENDERBUFFER_HEIGHT_OES,
                    &mut h,
                );
                if w > 0 && h > 0 {
                    let mut depth_rb: GLuint = 0;
                    gles.GenRenderbuffersOES(1, &mut depth_rb);
                    gles.BindRenderbufferOES(gles11::RENDERBUFFER_OES, depth_rb);
                    gles.RenderbufferStorageOES(
                        gles11::RENDERBUFFER_OES,
                        gles11::DEPTH_COMPONENT16_OES,
                        w,
                        h,
                    );
                    gles.FramebufferRenderbufferOES(
                        target,
                        gles11::DEPTH_ATTACHMENT_OES,
                        gles11::RENDERBUFFER_OES,
                        depth_rb,
                    );
                    log_dbg!(
                        "Auto-attached depth renderbuffer ({}x{}) to FBO target={:#x} attach={:#x}",
                        w,
                        h,
                        target,
                        attachment
                    );
                }
                gles.BindRenderbufferOES(gles11::RENDERBUFFER_OES, prev_rb as GLuint);
            }
            while gles.GetError() != 0 {}
        }
    })
}
fn glFramebufferTexture2DOES(
    env: &mut Environment,
    target: GLenum,
    attachment: GLenum,
    textarget: GLenum,
    texture: GLuint,
    level: i32,
) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.FramebufferTexture2DOES(target, attachment, textarget, texture, level)
    })
}
fn glGetFramebufferAttachmentParameterivOES(
    env: &mut Environment,
    target: GLenum,
    attachment: GLenum,
    pname: GLenum,
    params: MutPtr<GLint>,
) {
    with_ctx_and_mem(env, |gles, mem| {
        let params = mem.ptr_at_mut(params, 1);
        unsafe { gles.GetFramebufferAttachmentParameterivOES(target, attachment, pname, params) }
    })
}
fn glGetRenderbufferParameterivOES(
    env: &mut Environment,
    target: GLenum,
    pname: GLenum,
    params: MutPtr<GLint>,
) {
    let factor = env.options.scale_hack.get() as GLint;
    with_ctx_and_mem(env, |gles, mem| {
        let params = mem.ptr_at_mut(params, 1);
        unsafe { gles.GetRenderbufferParameterivOES(target, pname, params) };
        if pname == gles11::RENDERBUFFER_WIDTH_OES || pname == gles11::RENDERBUFFER_HEIGHT_OES {
            unsafe { params.write_unaligned(params.read_unaligned() / factor) }
        }
    })
}
fn glCheckFramebufferStatusOES(env: &mut Environment, target: GLenum) -> GLenum {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.CheckFramebufferStatusOES(target)
    })
}
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
fn glGenerateMipmapOES(env: &mut Environment, target: GLenum) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.GenerateMipmapOES(target) })
}

fn glGenFramebuffers(env: &mut Environment, n: GLsizei, framebuffers: MutPtr<GLuint>) {
    glGenFramebuffersOES(env, n, framebuffers)
}
fn glGenRenderbuffers(env: &mut Environment, n: GLsizei, renderbuffers: MutPtr<GLuint>) {
    glGenRenderbuffersOES(env, n, renderbuffers)
}
fn glIsFramebuffer(env: &mut Environment, framebuffer: GLuint) -> GLboolean {
    glIsFramebufferOES(env, framebuffer)
}
fn glIsRenderbuffer(env: &mut Environment, renderbuffer: GLuint) -> GLboolean {
    glIsRenderbufferOES(env, renderbuffer)
}
fn glBindFramebuffer(env: &mut Environment, target: GLenum, framebuffer: GLuint) {
    glBindFramebufferOES(env, target, framebuffer)
}
fn glBindRenderbuffer(env: &mut Environment, target: GLenum, renderbuffer: GLuint) {
    glBindRenderbufferOES(env, target, renderbuffer)
}
fn glRenderbufferStorage(
    env: &mut Environment,
    target: GLenum,
    internalformat: GLenum,
    width: GLsizei,
    height: GLsizei,
) {
    glRenderbufferStorageOES(env, target, internalformat, width, height)
}
fn glFramebufferRenderbuffer(
    env: &mut Environment,
    target: GLenum,
    attachment: GLenum,
    renderbuffertarget: GLenum,
    renderbuffer: GLuint,
) {
    glFramebufferRenderbufferOES(env, target, attachment, renderbuffertarget, renderbuffer)
}
fn glFramebufferTexture2D(
    env: &mut Environment,
    target: GLenum,
    attachment: GLenum,
    textarget: GLenum,
    texture: GLuint,
    level: i32,
) {
    glFramebufferTexture2DOES(env, target, attachment, textarget, texture, level)
}
fn glGetFramebufferAttachmentParameteriv(
    env: &mut Environment,
    target: GLenum,
    attachment: GLenum,
    pname: GLenum,
    params: MutPtr<GLint>,
) {
    glGetFramebufferAttachmentParameterivOES(env, target, attachment, pname, params)
}
fn glGetRenderbufferParameteriv(
    env: &mut Environment,
    target: GLenum,
    pname: GLenum,
    params: MutPtr<GLint>,
) {
    glGetRenderbufferParameterivOES(env, target, pname, params)
}
fn glCheckFramebufferStatus(env: &mut Environment, target: GLenum) -> GLenum {
    glCheckFramebufferStatusOES(env, target)
}
fn glDeleteFramebuffers(env: &mut Environment, n: GLsizei, framebuffers: ConstPtr<GLuint>) {
    glDeleteFramebuffersOES(env, n, framebuffers)
}
fn glDeleteRenderbuffers(env: &mut Environment, n: GLsizei, renderbuffers: ConstPtr<GLuint>) {
    glDeleteRenderbuffersOES(env, n, renderbuffers)
}
fn glGenerateMipmap(env: &mut Environment, target: GLenum) {
    glGenerateMipmapOES(env, target)
}

fn _get_currently_bound_buffer_object_name(env: &mut Environment, target: GLenum) -> GLuint {
    let binding = match target {
        ARRAY_BUFFER => VERTEX_ARRAY_BUFFER_BINDING,
        ELEMENT_ARRAY_BUFFER => ELEMENT_ARRAY_BUFFER_BINDING,
        other => {
            // Anything else is a malformed call from the guest. Real GL
            // would set GL_INVALID_ENUM; we have nothing sensible to bind
            // against so just report "no object bound" and continue.
            log!(
                "Warning: _get_currently_bound_buffer_object_name(): unsupported buffer target {:#x}; returning 0.",
                other
            );
            return 0;
        }
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

fn glGetBufferParameteriv(
    env: &mut Environment,
    target: GLenum,
    pname: GLenum,
    params: MutPtr<GLint>,
) {
    let params = env.mem.ptr_at_mut(params, 1);
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.GetBufferParameteriv(target, pname, params)
    })
}
fn glMapBufferOES(env: &mut Environment, target: GLenum, access: GLenum) -> MutPtr<GLvoid> {
    assert!(matches!(target, ARRAY_BUFFER | ELEMENT_ARRAY_BUFFER));
    assert!(access == WRITE_ONLY_OES);
    let buffer_object_name = _get_currently_bound_buffer_object_name(env, target);
    let host_buffer = with_ctx_and_mem_no_skip(env, |gles, _mem| unsafe {
        gles.MapBufferOES(target, access)
    });
    if host_buffer.is_null() {
        nil.cast()
    } else {
        let buffer_size = _get_buffer_size(env, target) as u32;
        let guest_buffer: MutVoidPtr = env.mem.alloc(buffer_size).cast();
        unsafe {
            env.mem
                .bytes_at_mut(guest_buffer.cast(), buffer_size)
                .copy_from_slice(from_raw_parts(host_buffer as *mut u8, buffer_size as usize));
        }
        let Some(current_ctx) = *env
            .framework_state
            .opengles
            .current_ctx_for_thread(env.current_thread)
        else {
            // glMapBufferOES requires a current context (Apple: "An
            // EAGLContext must be current on the thread before any GL
            // calls"). Without one, free the temporary guest buffer we
            // just allocated and return NULL, matching the GL spec which
            // says glMapBufferOES returns NULL on failure.
            env.mem.free(guest_buffer);
            return nil.cast();
        };
        let current_ctx_host_object = env.objc.borrow_mut::<EAGLContextHostObject>(current_ctx);
        // Per the GL_OES_mapbuffer spec, calling glMapBufferOES while a
        // buffer is already mapped is a GL_INVALID_OPERATION and returns
        // NULL. Some apps (e.g. ZAS) accidentally re-map without unmapping
        // first; the previous host mapping is still owned by the GL
        // driver, so we cannot just drop it without unmapping. To stay
        // close to Apple's lenient behaviour, we unmap the stale mapping
        // (freeing the previous guest mirror) and install the new one so
        // the app can continue uploading data instead of crashing.
        if let Some((stale_guest_buffer, _stale_host_buffer)) = current_ctx_host_object
            .mapped_buffers
            .remove(&buffer_object_name)
        {
            log!(
                "Warning: glMapBufferOES called on buffer {} that was already mapped; \
                 discarding the previous mapping (the previous pointer is no longer valid).",
                buffer_object_name
            );
            env.mem.free(stale_guest_buffer);
            // Issue a real glUnmapBufferOES so the driver releases its
            // own mapping of the buffer that we just dropped.
            with_ctx_and_mem(env, |gles, _mem| unsafe {
                gles.UnmapBufferOES(target);
            });
            // Re-borrow because with_ctx_and_mem dropped our reference.
            let current_ctx_host_object = env.objc.borrow_mut::<EAGLContextHostObject>(current_ctx);
            current_ctx_host_object
                .mapped_buffers
                .insert(buffer_object_name, (guest_buffer, host_buffer));
        } else {
            current_ctx_host_object
                .mapped_buffers
                .insert(buffer_object_name, (guest_buffer, host_buffer));
        }
        guest_buffer
    }
}
fn glUnmapBufferOES(env: &mut Environment, target: GLenum) -> GLboolean {
    let buffer_object_name = _get_currently_bound_buffer_object_name(env, target);
    let current_ctx = env
        .framework_state
        .opengles
        .current_ctx_for_thread(env.current_thread);
    let current_ctx_host_object = env
        .objc
        .borrow_mut::<EAGLContextHostObject>(current_ctx.unwrap());
    if let Some((guest_buffer, host_buffer)) = current_ctx_host_object
        .mapped_buffers
        .remove(&buffer_object_name)
    {
        let buffer_size = _get_buffer_size(env, target) as u32;
        unsafe {
            host_buffer.copy_from(
                env.mem.bytes_at(guest_buffer.cast(), buffer_size).as_ptr() as *mut GLvoid,
                buffer_size as usize,
            );
        }
        env.mem.free(guest_buffer);
    }
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.UnmapBufferOES(target) })
}

// =====================================================================
// OpenGL ES 2.0 entry points.
//
// These wrap the [crate::gles::GLES] trait's ES 2.0 methods, which are
// implemented by [crate::gles::gles1_on_gl2::GLES1OnGL2] using OpenGL 2.1
// compatibility profile. EAGL routes ES 2.0 contexts to that backend, so by
// the time these are called the trait methods are real implementations.
//
// Strings (shader sources, attribute/uniform names) need to be copied out of
// the guest's address space before being passed to host GL.
// =====================================================================

fn read_guest_cstring(mem: &Mem, ptr: ConstPtr<GLubyte>) -> std::ffi::CString {
    if ptr.is_null() {
        return std::ffi::CString::default();
    }
    let bytes = mem.cstr_at(ptr);
    std::ffi::CString::new(bytes).unwrap_or_default()
}

fn glCreateProgram(env: &mut Environment) -> GLuint {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.CreateProgram() })
}
fn glCreateShader(env: &mut Environment, type_: GLenum) -> GLuint {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.CreateShader(type_) })
}
fn glBindAttribLocation(
    env: &mut Environment,
    program: GLuint,
    index: GLuint,
    name: ConstPtr<GLubyte>,
) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let cstr = read_guest_cstring(mem, name);
        gles.BindAttribLocation(program, index, cstr.as_ptr());
    });
}
fn glGetAttribLocation(env: &mut Environment, program: GLuint, name: ConstPtr<GLubyte>) -> GLint {
    with_ctx_and_mem_no_skip(env, |gles, mem| unsafe {
        let cstr = read_guest_cstring(mem, name);
        gles.GetAttribLocation(program, cstr.as_ptr())
    })
}
fn glGetUniformLocation(env: &mut Environment, program: GLuint, name: ConstPtr<GLubyte>) -> GLint {
    with_ctx_and_mem_no_skip(env, |gles, mem| unsafe {
        let cstr = read_guest_cstring(mem, name);
        gles.GetUniformLocation(program, cstr.as_ptr())
    })
}
fn glUniformMatrix2fv(
    env: &mut Environment,
    location: GLint,
    count: GLsizei,
    transpose: GLboolean,
    value: ConstPtr<GLfloat>,
) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let n = (count as usize) * 4;
        let ptr = mem.ptr_at(value, n.try_into().unwrap_or(0));
        gles.UniformMatrix2fv(location, count, transpose, ptr);
    });
}
fn glUniformMatrix3fv(
    env: &mut Environment,
    location: GLint,
    count: GLsizei,
    transpose: GLboolean,
    value: ConstPtr<GLfloat>,
) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let n = (count as usize) * 9;
        let ptr = mem.ptr_at(value, n.try_into().unwrap_or(0));
        gles.UniformMatrix3fv(location, count, transpose, ptr);
    });
}
fn glUniformMatrix4fv(
    env: &mut Environment,
    location: GLint,
    count: GLsizei,
    transpose: GLboolean,
    value: ConstPtr<GLfloat>,
) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let n = (count as usize) * 16;
        let ptr = mem.ptr_at(value, n.try_into().unwrap_or(0));
        gles.UniformMatrix4fv(location, count, transpose, ptr);
    });
}
fn glUseProgram(env: &mut Environment, program: GLuint) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.UseProgram(program) });
}
fn glDeleteProgram(env: &mut Environment, program: GLuint) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.DeleteProgram(program) });
}
fn glDeleteShader(env: &mut Environment, shader: GLuint) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.DeleteShader(shader) });
}
fn glCompileShader(env: &mut Environment, shader: GLuint) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.CompileShader(shader);
        let mut ok: GLint = 0;
        gles.GetShaderiv(shader, 0x8B81 /* GL_COMPILE_STATUS */, &mut ok);
        if ok == 0 {
            let mut buf = [0u8; 1024];
            let mut len: GLsizei = 0;
            gles.GetShaderInfoLog(shader, 1024, &mut len, buf.as_mut_ptr() as *mut _);
            let s = std::str::from_utf8(std::slice::from_raw_parts(
                buf.as_ptr() as *const u8,
                len as usize,
            ))
            .unwrap_or("?");
            log!("Shader {} compile failed: {}", shader, s);
        }
    });
}
/// OpenGL ES 2.0 `glGetShaderPrecisionFormat`. Defined here so that guest
/// apps that probe shader compiler precision (e.g. Minecraft PE 0.10.x) get
/// real numbers from the driver instead of a return-0 stub installed by dyld
/// for an unimplemented symbol.
/// <https://registry.khronos.org/OpenGL-Refpages/es2.0/xhtml/glGetShaderPrecisionFormat.xml>
fn glGetShaderPrecisionFormat(
    env: &mut Environment,
    shadertype: GLenum,
    precisiontype: GLenum,
    range: MutPtr<GLint>,
    precision: MutPtr<GLint>,
) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let mut host_range: [GLint; 2] = [0, 0];
        let mut host_precision: GLint = 0;
        gles.GetShaderPrecisionFormat(
            shadertype,
            precisiontype,
            host_range.as_mut_ptr(),
            &mut host_precision,
        );
        if !range.is_null() {
            mem.write(range + 0, host_range[0]);
            mem.write(range + 1, host_range[1]);
        }
        if !precision.is_null() {
            mem.write(precision, host_precision);
        }
    });
}
fn glAttachShader(env: &mut Environment, program: GLuint, shader: GLuint) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.AttachShader(program, shader)
    });
}
fn glDetachShader(env: &mut Environment, program: GLuint, shader: GLuint) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.DetachShader(program, shader)
    });
}
fn glLinkProgram(env: &mut Environment, program: GLuint) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.LinkProgram(program);
        let mut ok: GLint = 0;
        gles.GetProgramiv(program, 0x8B82 /* GL_LINK_STATUS */, &mut ok);
        if ok == 0 {
            let mut buf = [0u8; 1024];
            let mut len: GLsizei = 0;
            gles.GetProgramInfoLog(program, 1024, &mut len, buf.as_mut_ptr() as *mut _);
            let s = std::str::from_utf8(std::slice::from_raw_parts(
                buf.as_ptr() as *const u8,
                len as usize,
            ))
            .unwrap_or("?");
            log!("Program {} link failed: {}", program, s);
        }
    });
}
fn glValidateProgram(env: &mut Environment, program: GLuint) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.ValidateProgram(program) });
}
fn glIsShader(env: &mut Environment, shader: GLuint) -> GLboolean {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.IsShader(shader) })
}
fn glIsProgram(env: &mut Environment, program: GLuint) -> GLboolean {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.IsProgram(program) })
}
fn glGetShaderiv(env: &mut Environment, shader: GLuint, pname: GLenum, params: MutPtr<GLint>) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let mut val: GLint = 0;
        gles.GetShaderiv(shader, pname, &mut val);
        if !params.is_null() {
            mem.write(params, val);
        }
    });
}
fn glGetShaderInfoLog(
    env: &mut Environment,
    shader: GLuint,
    bufSize: GLsizei,
    length: MutPtr<GLsizei>,
    infoLog: MutPtr<GLubyte>,
) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        if bufSize <= 0 {
            if !length.is_null() {
                mem.write(length, 0);
            }
            return;
        }
        let mut buf: Vec<u8> = vec![0u8; bufSize as usize];
        let mut written: GLsizei = 0;
        gles.GetShaderInfoLog(shader, bufSize, &mut written, buf.as_mut_ptr().cast());
        if !length.is_null() {
            mem.write(length, written);
        }
        if !infoLog.is_null() && written >= 0 {
            let count = written as usize + 1; // include trailing NUL
            let count = count.min(buf.len()).min(bufSize as usize);
            let dst = mem.ptr_at_mut(infoLog, count.try_into().unwrap_or(0));
            std::ptr::copy_nonoverlapping(buf.as_ptr(), dst, count);
        }
    });
}
fn glGetProgramiv(env: &mut Environment, program: GLuint, pname: GLenum, params: MutPtr<GLint>) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let mut val: GLint = 0;
        gles.GetProgramiv(program, pname, &mut val);
        if !params.is_null() {
            mem.write(params, val);
        }
    });
}
fn glGetProgramInfoLog(
    env: &mut Environment,
    program: GLuint,
    bufSize: GLsizei,
    length: MutPtr<GLsizei>,
    infoLog: MutPtr<GLubyte>,
) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        if bufSize <= 0 {
            if !length.is_null() {
                mem.write(length, 0);
            }
            return;
        }
        let mut buf: Vec<u8> = vec![0u8; bufSize as usize];
        let mut written: GLsizei = 0;
        gles.GetProgramInfoLog(program, bufSize, &mut written, buf.as_mut_ptr().cast());
        if !length.is_null() {
            mem.write(length, written);
        }
        if !infoLog.is_null() && written >= 0 {
            let count = written as usize + 1;
            let count = count.min(buf.len()).min(bufSize as usize);
            let dst = mem.ptr_at_mut(infoLog, count.try_into().unwrap_or(0));
            std::ptr::copy_nonoverlapping(buf.as_ptr(), dst, count);
        }
    });
}
fn glGetActiveUniform(
    env: &mut Environment,
    program: GLuint,
    index: GLuint,
    bufSize: GLsizei,
    length: MutPtr<GLsizei>,
    size: MutPtr<GLint>,
    type_: MutPtr<GLenum>,
    name: MutPtr<GLubyte>,
) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        if bufSize <= 0 {
            if !length.is_null() {
                mem.write(length, 0);
            }
            return;
        }
        let mut host_length: GLsizei = 0;
        let mut host_size: GLint = 0;
        let mut host_type: GLenum = 0;
        let mut buf: Vec<u8> = vec![0u8; bufSize as usize];
        gles.GetActiveUniform(
            program,
            index,
            bufSize,
            &mut host_length,
            &mut host_size,
            &mut host_type,
            buf.as_mut_ptr().cast(),
        );
        if !length.is_null() {
            mem.write(length, host_length);
        }
        if !size.is_null() {
            mem.write(size, host_size);
        }
        if !type_.is_null() {
            mem.write(type_, host_type);
        }
        if !name.is_null() && host_length >= 0 {
            let count = host_length as usize + 1;
            let count = count.min(buf.len()).min(bufSize as usize);
            let dst = mem.ptr_at_mut(name, count.try_into().unwrap_or(0));
            std::ptr::copy_nonoverlapping(buf.as_ptr(), dst, count);
        }
    });
}
fn glGetActiveAttrib(
    env: &mut Environment,
    program: GLuint,
    index: GLuint,
    bufSize: GLsizei,
    length: MutPtr<GLsizei>,
    size: MutPtr<GLint>,
    type_: MutPtr<GLenum>,
    name: MutPtr<GLubyte>,
) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        if bufSize <= 0 {
            if !length.is_null() {
                mem.write(length, 0);
            }
            return;
        }
        let mut host_length: GLsizei = 0;
        let mut host_size: GLint = 0;
        let mut host_type: GLenum = 0;
        let mut buf: Vec<u8> = vec![0u8; bufSize as usize];
        gles.GetActiveAttrib(
            program,
            index,
            bufSize,
            &mut host_length,
            &mut host_size,
            &mut host_type,
            buf.as_mut_ptr().cast(),
        );
        if !length.is_null() {
            mem.write(length, host_length);
        }
        if !size.is_null() {
            mem.write(size, host_size);
        }
        if !type_.is_null() {
            mem.write(type_, host_type);
        }
        if !name.is_null() && host_length >= 0 {
            let count = host_length as usize + 1;
            let count = count.min(buf.len()).min(bufSize as usize);
            let dst = mem.ptr_at_mut(name, count.try_into().unwrap_or(0));
            std::ptr::copy_nonoverlapping(buf.as_ptr(), dst, count);
        }
    });
}
fn glShaderSource(
    env: &mut Environment,
    shader: GLuint,
    count: GLsizei,
    string: ConstPtr<ConstPtr<GLubyte>>,
    length: ConstPtr<GLint>,
) {
    if count <= 0 {
        return;
    }
    // Copy each source string out of the guest's memory into a host-side
    // buffer so we can pass real host pointers to the GLES implementation.
    let mut owned: Vec<std::ffi::CString> = Vec::with_capacity(count as usize);
    for i in 0..count {
        let str_ptr_ptr: ConstPtr<ConstPtr<GLubyte>> = string + (i as GuestUSize);
        let str_ptr: ConstPtr<GLubyte> = env.mem.read(str_ptr_ptr);
        if str_ptr.is_null() {
            owned.push(std::ffi::CString::default());
            continue;
        }
        // Check if explicit lengths were provided.
        let len_opt: Option<i32> = if length.is_null() {
            None
        } else {
            let len_ptr: ConstPtr<GLint> = length + (i as GuestUSize);
            let l: GLint = env.mem.read(len_ptr);
            if l < 0 {
                None
            } else {
                Some(l)
            }
        };
        let bytes_vec: Vec<u8> = if let Some(len) = len_opt {
            let slice = env
                .mem
                .bytes_at(str_ptr.cast(), len.try_into().unwrap_or(0));
            slice.to_vec()
        } else {
            env.mem.cstr_at(str_ptr).to_vec()
        };
        let cs = std::ffi::CString::new(bytes_vec).unwrap_or_default();
        owned.push(cs);
    }
    let ptrs: Vec<*const std::os::raw::c_char> = owned.iter().map(|s| s.as_ptr()).collect();
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.ShaderSource(shader, count, ptrs.as_ptr(), std::ptr::null());
    });
}
fn glEnableVertexAttribArray(env: &mut Environment, index: GLuint) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.EnableVertexAttribArray(index)
    });
}
fn glDisableVertexAttribArray(env: &mut Environment, index: GLuint) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.DisableVertexAttribArray(index)
    });
}
fn glVertexAttribPointer(
    env: &mut Environment,
    index: GLuint,
    size: GLint,
    type_: GLenum,
    normalized: GLboolean,
    stride: GLsizei,
    pointer: ConstVoidPtr,
) {
    // If a buffer is bound, `pointer` is treated as an offset within that
    // buffer (not as a pointer into client memory) and we can pass it through
    // as-is. Otherwise we need to translate the guest pointer to a host
    // pointer.
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let mut bound: GLint = 0;
        gles.GetIntegerv(gles11::ARRAY_BUFFER_BINDING, &mut bound);
        let host_ptr = if bound == 0 {
            if pointer.is_null() {
                std::ptr::null()
            } else {
                mem.ptr_at(pointer.cast::<u8>(), 1) as *const _
            }
        } else {
            pointer.to_bits() as usize as *const _
        };
        gles.VertexAttribPointer(index, size, type_, normalized, stride, host_ptr);
    });
}
fn glVertexAttrib1f(env: &mut Environment, index: GLuint, x: GLfloat) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.VertexAttrib1f(index, x) });
}
fn glVertexAttrib2f(env: &mut Environment, index: GLuint, x: GLfloat, y: GLfloat) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.VertexAttrib2f(index, x, y)
    });
}
fn glVertexAttrib3f(env: &mut Environment, index: GLuint, x: GLfloat, y: GLfloat, z: GLfloat) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.VertexAttrib3f(index, x, y, z)
    });
}
fn glVertexAttrib4f(
    env: &mut Environment,
    index: GLuint,
    x: GLfloat,
    y: GLfloat,
    z: GLfloat,
    w: GLfloat,
) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.VertexAttrib4f(index, x, y, z, w)
    });
}
fn glUniform1i(env: &mut Environment, location: GLint, v0: GLint) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.Uniform1i(location, v0) });
}
fn glUniform2i(env: &mut Environment, location: GLint, v0: GLint, v1: GLint) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.Uniform2i(location, v0, v1)
    });
}
fn glUniform3i(env: &mut Environment, location: GLint, v0: GLint, v1: GLint, v2: GLint) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.Uniform3i(location, v0, v1, v2)
    });
}
fn glUniform4i(env: &mut Environment, location: GLint, v0: GLint, v1: GLint, v2: GLint, v3: GLint) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.Uniform4i(location, v0, v1, v2, v3)
    });
}
fn glUniform1f(env: &mut Environment, location: GLint, v0: GLfloat) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.Uniform1f(location, v0) });
}
fn glUniform2f(env: &mut Environment, location: GLint, v0: GLfloat, v1: GLfloat) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.Uniform2f(location, v0, v1)
    });
}
fn glUniform3f(env: &mut Environment, location: GLint, v0: GLfloat, v1: GLfloat, v2: GLfloat) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.Uniform3f(location, v0, v1, v2)
    });
}
fn glUniform4f(
    env: &mut Environment,
    location: GLint,
    v0: GLfloat,
    v1: GLfloat,
    v2: GLfloat,
    v3: GLfloat,
) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.Uniform4f(location, v0, v1, v2, v3)
    });
}
fn glUniform1iv(env: &mut Environment, location: GLint, count: GLsizei, value: ConstPtr<GLint>) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let n = (count as usize).max(0);
        let ptr = mem.ptr_at(value, n.try_into().unwrap_or(0));
        gles.Uniform1iv(location, count, ptr);
    });
}
fn glUniform2iv(env: &mut Environment, location: GLint, count: GLsizei, value: ConstPtr<GLint>) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let n = (count as usize) * 2;
        let ptr = mem.ptr_at(value, n.try_into().unwrap_or(0));
        gles.Uniform2iv(location, count, ptr);
    });
}
fn glUniform3iv(env: &mut Environment, location: GLint, count: GLsizei, value: ConstPtr<GLint>) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let n = (count as usize) * 3;
        let ptr = mem.ptr_at(value, n.try_into().unwrap_or(0));
        gles.Uniform3iv(location, count, ptr);
    });
}
fn glUniform4iv(env: &mut Environment, location: GLint, count: GLsizei, value: ConstPtr<GLint>) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let n = (count as usize) * 4;
        let ptr = mem.ptr_at(value, n.try_into().unwrap_or(0));
        gles.Uniform4iv(location, count, ptr);
    });
}
fn glUniform1fv(env: &mut Environment, location: GLint, count: GLsizei, value: ConstPtr<GLfloat>) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let n = count as usize;
        let ptr = mem.ptr_at(value, n.try_into().unwrap_or(0));
        gles.Uniform1fv(location, count, ptr);
    });
}
fn glUniform2fv(env: &mut Environment, location: GLint, count: GLsizei, value: ConstPtr<GLfloat>) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let n = (count as usize) * 2;
        let ptr = mem.ptr_at(value, n.try_into().unwrap_or(0));
        gles.Uniform2fv(location, count, ptr);
    });
}
fn glUniform3fv(env: &mut Environment, location: GLint, count: GLsizei, value: ConstPtr<GLfloat>) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let n = (count as usize) * 3;
        let ptr = mem.ptr_at(value, n.try_into().unwrap_or(0));
        gles.Uniform3fv(location, count, ptr);
    });
}
fn glUniform4fv(env: &mut Environment, location: GLint, count: GLsizei, value: ConstPtr<GLfloat>) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let n = (count as usize) * 4;
        let ptr = mem.ptr_at(value, n.try_into().unwrap_or(0));
        gles.Uniform4fv(location, count, ptr);
    });
}
fn glReleaseShaderCompiler(env: &mut Environment) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.ReleaseShaderCompiler() });
}
fn glBlendColor(env: &mut Environment, r: GLclampf, g: GLclampf, b: GLclampf, a: GLclampf) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.BlendColor(r, g, b, a) });
}
fn glBlendEquation(env: &mut Environment, mode: GLenum) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.BlendEquation(mode) });
}
fn glBlendEquationSeparate(env: &mut Environment, modeRGB: GLenum, modeAlpha: GLenum) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.BlendEquationSeparate(modeRGB, modeAlpha)
    });
}
fn glBlendFuncSeparate(
    env: &mut Environment,
    srcRGB: GLenum,
    dstRGB: GLenum,
    srcAlpha: GLenum,
    dstAlpha: GLenum,
) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.BlendFuncSeparate(srcRGB, dstRGB, srcAlpha, dstAlpha)
    });
}
fn glStencilFuncSeparate(
    env: &mut Environment,
    face: GLenum,
    func: GLenum,
    ref_: GLint,
    mask: GLuint,
) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.StencilFuncSeparate(face, func, ref_, mask)
    });
}
fn glStencilOpSeparate(
    env: &mut Environment,
    face: GLenum,
    sfail: GLenum,
    dpfail: GLenum,
    dppass: GLenum,
) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.StencilOpSeparate(face, sfail, dpfail, dppass)
    });
}
fn glStencilMaskSeparate(env: &mut Environment, face: GLenum, mask: GLuint) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.StencilMaskSeparate(face, mask)
    });
}

// Pre-existing GLES1 helpers reused for ES 2.0 — VAOs are not part of ES 2.0
// but some apps still call these as no-ops.

/// Stub for non-OES VAO entry point: ES 2.0 does not have VAOs and most
/// apps that call this expect a fake non-zero ID.
fn glGenVertexArrays(env: &mut Environment, n: GLsizei, arrays: MutPtr<GLuint>) {
    glGenVertexArraysOES(env, n, arrays)
}
fn glBindVertexArray(env: &mut Environment, array: GLuint) {
    glBindVertexArrayOES(env, array)
}
fn glDeleteVertexArrays(env: &mut Environment, n: GLsizei, arrays: ConstPtr<GLuint>) {
    glDeleteVertexArraysOES(env, n, arrays)
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
    export_c_func!(glGetError()),
    export_c_func!(glEnable(_)),
    export_c_func!(glIsEnabled(_)),
    export_c_func!(glDisable(_)),
    export_c_func!(glClientActiveTexture(_)),
    export_c_func!(glEnableClientState(_)),
    export_c_func!(glDisableClientState(_)),
    export_c_func!(glGetBooleanv(_, _)),
    export_c_func!(glGetFloatv(_, _)),
    export_c_func!(glGetIntegerv(_, _)),
    export_c_func!(glGetPointerv(_, _)),
    export_c_func!(glGetTexEnviv(_, _, _)),
    export_c_func!(glGetTexEnvfv(_, _, _)),
    export_c_func!(glHint(_, _)),
    export_c_func!(glFinish()),
    export_c_func!(glFlush()),
    export_c_func!(glGetString(_)),
    export_c_func!(glAlphaFunc(_, _)),
    export_c_func!(glAlphaFuncx(_, _)),
    export_c_func!(glBlendFunc(_, _)),
    export_c_func!(glBlendEquationOES(_)),
    export_c_func!(glColorMask(_, _, _, _)),
    export_c_func!(glClipPlanef(_, _)),
    export_c_func!(glClipPlanex(_, _)),
    export_c_func!(glCullFace(_)),
    export_c_func!(glDepthFunc(_)),
    export_c_func!(glDepthMask(_)),
    export_c_func!(glDepthRangef(_, _)),
    export_c_func!(glDepthRangex(_, _)),
    export_c_func!(glFrontFace(_)),
    export_c_func!(glPolygonOffset(_, _)),
    export_c_func!(glPolygonOffsetx(_, _)),
    export_c_func!(glSampleCoverage(_, _)),
    export_c_func!(glSampleCoveragex(_, _)),
    export_c_func!(glShadeModel(_)),
    export_c_func!(glScissor(_, _, _, _)),
    export_c_func!(glViewport(_, _, _, _)),
    export_c_func!(glLineWidth(_)),
    export_c_func!(glLineWidthx(_)),
    export_c_func!(glStencilFunc(_, _, _)),
    export_c_func!(glStencilOp(_, _, _)),
    export_c_func!(glStencilMask(_)),
    export_c_func!(glLogicOp(_)),
    export_c_func!(glPointSize(_)),
    export_c_func!(glPointSizex(_)),
    export_c_func!(glPointParameterf(_, _)),
    export_c_func!(glPointParameterx(_, _)),
    export_c_func!(glPointParameterfv(_, _)),
    export_c_func!(glPointParameterxv(_, _)),
    export_c_func!(glFogf(_, _)),
    export_c_func!(glFogx(_, _)),
    export_c_func!(glFogfv(_, _)),
    export_c_func!(glFogxv(_, _)),
    export_c_func!(glLightf(_, _, _)),
    export_c_func!(glLightx(_, _, _)),
    export_c_func!(glLightfv(_, _, _)),
    export_c_func!(glLightxv(_, _, _)),
    export_c_func!(glLightModelf(_, _)),
    export_c_func!(glLightModelx(_, _)),
    export_c_func!(glLightModelfv(_, _)),
    export_c_func!(glLightModelxv(_, _)),
    export_c_func!(glMaterialf(_, _, _)),
    export_c_func!(glMaterialx(_, _, _)),
    export_c_func!(glMaterialfv(_, _, _)),
    export_c_func!(glMaterialxv(_, _, _)),
    export_c_func!(glIsBuffer(_)),
    export_c_func!(glGenBuffers(_, _)),
    export_c_func!(glDeleteBuffers(_, _)),
    export_c_func!(glBindBuffer(_, _)),
    export_c_func!(glBufferData(_, _, _, _)),
    export_c_func!(glBufferSubData(_, _, _, _)),
    export_c_func!(glColor4f(_, _, _, _)),
    export_c_func!(glColor4x(_, _, _, _)),
    export_c_func!(glColor4ub(_, _, _, _)),
    export_c_func!(glNormal3f(_, _, _)),
    export_c_func!(glNormal3x(_, _, _)),
    export_c_func!(glColorPointer(_, _, _, _)),
    export_c_func!(glNormalPointer(_, _, _)),
    export_c_func!(glTexCoordPointer(_, _, _, _)),
    export_c_func!(glVertexPointer(_, _, _, _)),
    export_c_func!(glPointSizePointerOES(_, _, _)),
    export_c_func!(glGetTexParameteriv(_, _, _)),
    export_c_func!(glDrawTexfOES(_, _, _, _, _)),
    export_c_func!(glDrawTexiOES(_, _, _, _, _)),
    export_c_func!(glDrawTexxOES(_, _, _, _, _)),
    export_c_func!(glDrawTexfvOES(_)),
    export_c_func!(glDrawTexivOES(_)),
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
    export_c_func!(glDrawArrays(_, _, _)),
    export_c_func!(glDrawElements(_, _, _, _)),
    export_c_func!(glClear(_)),
    export_c_func!(glClearColor(_, _, _, _)),
    export_c_func!(glClearColorx(_, _, _, _)),
    export_c_func!(glClearDepthf(_)),
    export_c_func!(glClearDepthx(_)),
    export_c_func!(glClearStencil(_)),
    export_c_func!(glMatrixMode(_)),
    export_c_func!(glLoadIdentity()),
    export_c_func!(glLoadMatrixf(_)),
    export_c_func!(glLoadMatrixx(_)),
    export_c_func!(glMultMatrixf(_)),
    export_c_func!(glMultMatrixx(_)),
    export_c_func!(glPushMatrix()),
    export_c_func!(glPopMatrix()),
    export_c_func!(glOrthof(_, _, _, _, _, _)),
    export_c_func!(glOrthox(_, _, _, _, _, _)),
    export_c_func!(glFrustumf(_, _, _, _, _, _)),
    export_c_func!(glFrustumx(_, _, _, _, _, _)),
    export_c_func!(glRotatef(_, _, _, _)),
    export_c_func!(glRotatex(_, _, _, _)),
    export_c_func!(glScalef(_, _, _)),
    export_c_func!(glScalex(_, _, _)),
    export_c_func!(glTranslatef(_, _, _)),
    export_c_func!(glTranslatex(_, _, _)),
    export_c_func!(glPixelStorei(_, _)),
    export_c_func!(glReadPixels(_, _, _, _, _, _, _)),
    export_c_func!(glGenTextures(_, _)),
    export_c_func!(glDeleteTextures(_, _)),
    export_c_func!(glActiveTexture(_)),
    export_c_func!(glIsTexture(_)),
    export_c_func!(glBindTexture(_, _)),
    export_c_func!(glTexParameteri(_, _, _)),
    export_c_func!(glTexParameterf(_, _, _)),
    export_c_func!(glTexParameterx(_, _, _)),
    export_c_func!(glTexParameteriv(_, _, _)),
    export_c_func!(glTexParameterfv(_, _, _)),
    export_c_func!(glTexParameterxv(_, _, _)),
    export_c_func!(glTexImage2D(_, _, _, _, _, _, _, _, _)),
    export_c_func!(glTexSubImage2D(_, _, _, _, _, _, _, _, _)),
    export_c_func!(glCompressedTexImage2D(_, _, _, _, _, _, _, _)),
    export_c_func!(glCopyTexImage2D(_, _, _, _, _, _, _, _)),
    export_c_func!(glCopyTexSubImage2D(_, _, _, _, _, _, _, _)),
    export_c_func!(glTexEnvf(_, _, _)),
    export_c_func!(glTexEnvx(_, _, _)),
    export_c_func!(glTexEnvi(_, _, _)),
    export_c_func!(glTexEnvfv(_, _, _)),
    export_c_func!(glTexEnvxv(_, _, _)),
    export_c_func!(glTexEnviv(_, _, _)),
    export_c_func!(glMultiTexCoord4f(_, _, _, _, _)),
    export_c_func!(glMultiTexCoord4x(_, _, _, _, _)),
    export_c_func!(glGenFramebuffersOES(_, _)),
    export_c_func!(glGenRenderbuffersOES(_, _)),
    export_c_func!(glIsFramebufferOES(_)),
    export_c_func!(glIsRenderbufferOES(_)),
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
    export_c_func!(glIsFramebuffer(_)),
    export_c_func!(glIsRenderbuffer(_)),
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
    export_c_func!(glMapBufferOES(_, _)),
    export_c_func!(glUnmapBufferOES(_)),
    // OpenGL ES 2.0 entry points
    export_c_func!(glCreateProgram()),
    export_c_func!(glCreateShader(_)),
    export_c_func!(glBindAttribLocation(_, _, _)),
    export_c_func!(glGetAttribLocation(_, _)),
    export_c_func!(glGetUniformLocation(_, _)),
    export_c_func!(glUniformMatrix2fv(_, _, _, _)),
    export_c_func!(glUniformMatrix3fv(_, _, _, _)),
    export_c_func!(glUniformMatrix4fv(_, _, _, _)),
    export_c_func!(glUseProgram(_)),
    export_c_func!(glDeleteProgram(_)),
    export_c_func!(glDeleteShader(_)),
    export_c_func!(glCompileShader(_)),
    export_c_func!(glGetShaderPrecisionFormat(_, _, _, _)),
    export_c_func!(glAttachShader(_, _)),
    export_c_func!(glDetachShader(_, _)),
    export_c_func!(glLinkProgram(_)),
    export_c_func!(glValidateProgram(_)),
    export_c_func!(glIsShader(_)),
    export_c_func!(glIsProgram(_)),
    export_c_func!(glGetShaderiv(_, _, _)),
    export_c_func!(glGetShaderInfoLog(_, _, _, _)),
    export_c_func!(glGetProgramiv(_, _, _)),
    export_c_func!(glGetProgramInfoLog(_, _, _, _)),
    export_c_func!(glGetActiveUniform(_, _, _, _, _, _, _)),
    export_c_func!(glGetActiveAttrib(_, _, _, _, _, _, _)),
    export_c_func!(glShaderSource(_, _, _, _)),
    export_c_func!(glEnableVertexAttribArray(_)),
    export_c_func!(glDisableVertexAttribArray(_)),
    export_c_func!(glVertexAttribPointer(_, _, _, _, _, _)),
    export_c_func!(glVertexAttrib1f(_, _)),
    export_c_func!(glVertexAttrib2f(_, _, _)),
    export_c_func!(glVertexAttrib3f(_, _, _, _)),
    export_c_func!(glVertexAttrib4f(_, _, _, _, _)),
    export_c_func!(glUniform1i(_, _)),
    export_c_func!(glUniform2i(_, _, _)),
    export_c_func!(glUniform3i(_, _, _, _)),
    export_c_func!(glUniform4i(_, _, _, _, _)),
    export_c_func!(glUniform1f(_, _)),
    export_c_func!(glUniform2f(_, _, _)),
    export_c_func!(glUniform3f(_, _, _, _)),
    export_c_func!(glUniform4f(_, _, _, _, _)),
    export_c_func!(glUniform1iv(_, _, _)),
    export_c_func!(glUniform2iv(_, _, _)),
    export_c_func!(glUniform3iv(_, _, _)),
    export_c_func!(glUniform4iv(_, _, _)),
    export_c_func!(glUniform1fv(_, _, _)),
    export_c_func!(glUniform2fv(_, _, _)),
    export_c_func!(glUniform3fv(_, _, _)),
    export_c_func!(glUniform4fv(_, _, _)),
    export_c_func!(glReleaseShaderCompiler()),
    export_c_func!(glBlendColor(_, _, _, _)),
    export_c_func!(glBlendEquation(_)),
    export_c_func!(glBlendEquationSeparate(_, _)),
    export_c_func!(glBlendFuncSeparate(_, _, _, _)),
    export_c_func!(glStencilFuncSeparate(_, _, _, _)),
    export_c_func!(glStencilOpSeparate(_, _, _, _)),
    export_c_func!(glStencilMaskSeparate(_, _)),
    export_c_func!(glGenVertexArrays(_, _)),
    export_c_func!(glBindVertexArray(_)),
    export_c_func!(glDeleteVertexArrays(_, _)),
];
