/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

//! Wrapper functions exposing OpenGL ES to the guest.

use crate::dyld::{export_c_func, export_c_func_aliased, FunctionExports};
use crate::frameworks::opengles::eagl::{EAGLContextHostObject, GLShadowState};
use crate::frameworks::opengles::software_vao::{self, ArrayPointer, FixedArray};
use crate::gles::{gles11_raw as gles11, GuestGlesCallGuard, GLES};
use crate::mem::{ConstPtr, ConstVoidPtr, GuestISize, GuestUSize, Mem, MutPtr, MutVoidPtr, Ptr};
use crate::objc::nil;
use crate::window::DeviceFamily;
use crate::Environment;
use std::collections::HashMap;
use std::slice::from_raw_parts;
use touchHLE_gl_bindings::gles11::{
    ARRAY_BUFFER, ELEMENT_ARRAY_BUFFER, ELEMENT_ARRAY_BUFFER_BINDING, WRITE_ONLY_OES,
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

fn trace_potatogold_render() -> bool {
    crate::env_flag_cached!("TOUCHHLE_TRACE_POTATOGOLD_RENDER")
}

const TEXTURE_CUBE_MAP: GLenum = 0x8513;
const TEXTURE_BINDING_CUBE_MAP: GLenum = 0x8514;
const TEXTURE_CUBE_MAP_POSITIVE_X: GLenum = 0x8515;
const TEXTURE_CUBE_MAP_NEGATIVE_Z: GLenum = 0x851A;
const TEXTURE_3D: GLenum = 0x806F;
const TEXTURE_BINDING_3D: GLenum = 0x806A;
const TEXTURE_2D_ARRAY: GLenum = 0x8C1A;
const TEXTURE_BINDING_2D_ARRAY: GLenum = 0x8C1D;

fn texture_binding_pname(target: GLenum) -> Option<GLenum> {
    if target == gles11::TEXTURE_2D {
        Some(gles11::TEXTURE_BINDING_2D)
    } else if target == TEXTURE_3D {
        Some(TEXTURE_BINDING_3D)
    } else if target == TEXTURE_2D_ARRAY {
        Some(TEXTURE_BINDING_2D_ARRAY)
    } else if target == TEXTURE_CUBE_MAP
        || (TEXTURE_CUBE_MAP_POSITIVE_X..=TEXTURE_CUBE_MAP_NEGATIVE_Z).contains(&target)
    {
        Some(TEXTURE_BINDING_CUBE_MAP)
    } else {
        None
    }
}

unsafe fn current_bound_texture(gles: &mut dyn GLES, target: GLenum) -> Option<GLuint> {
    let pname = texture_binding_pname(target)?;
    let mut texture = 0;
    gles.GetIntegerv(pname, &mut texture);
    GLuint::try_from(texture).ok()
}

fn pvrtc_subimage_matches_level(
    texture_level: Option<(GLsizei, GLsizei, GLenum)>,
    xoffset: GLint,
    yoffset: GLint,
    width: GLsizei,
    height: GLsizei,
    format: GLenum,
) -> bool {
    matches!(
        texture_level,
        Some((stored_width, stored_height, stored_format))
            if xoffset == 0
                && yoffset == 0
                && width == stored_width
                && height == stored_height
                && format == stored_format
    )
}

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
    // Clear any sticky host-GL error before the call so that the post-call
    // trace report only reflects errors raised by *this* dispatch, not
    // leftovers from untraced internal code paths (EAGL present, context
    // sync, etc.). Without this, one bad call makes every later traced
    // call report the same code at the wrong line.
    if trace {
        unsafe { gles.GetError() };
    }
    let res = {
        let _guest_call_guard = GuestGlesCallGuard::enter();
        f(gles.as_mut(), &mut env.mem)
    };
    if trace {
        let err = unsafe { gles.GetError() };
        if err != 0 {
            log!(
                "[--trace-gl-errors] {} call reported glGetError() = {:#x} \
                 dispatched from {}:{}",
                gles.diagnostic_profile(),
                err,
                caller.file(),
                caller.line()
            );
        }
    }
    #[allow(clippy::let_and_return)]
    res
}

/// Like [with_ctx_and_mem], but the closure also gets the current context's
/// [GLShadowState] (see its documentation). Use this for entry points that
/// either change the mirrored state or want to consult it instead of asking
/// the driver.
#[track_caller]
fn with_ctx_mem_and_shadow<T, U: Default>(env: &mut Environment, f: T) -> U
where
    T: FnOnce(&mut dyn GLES, &mut Mem, &mut GLShadowState) -> U,
{
    let Some(current_ctx) = *env
        .framework_state
        .opengles
        .current_ctx_for_thread(env.current_thread)
    else {
        log_dbg!(
            "Skipping GLES call without context (line {})",
            std::panic::Location::caller().line()
        );
        return U::default();
    };
    let trace = env.options.trace_gl_errors;
    let caller = std::panic::Location::caller();
    let window = env
        .window
        .as_mut()
        .expect("OpenGL ES is not supported in headless mode");
    let host_obj = env.objc.borrow_mut::<EAGLContextHostObject>(current_ctx);
    // Disjoint field borrows: the GL context box and the shadow state live
    // side by side in the host object.
    let shadow = &mut host_obj.shadow;
    let Some(gles_ctx) = host_obj.gles_ctx.as_deref_mut() else {
        log_dbg!(
            "Skipping GLES call: current EAGLContext has no GLES backend (line {})",
            caller.line()
        );
        return U::default();
    };
    let mut gles = gles_ctx.make_current(window);
    if trace {
        unsafe { gles.GetError() };
    }
    let res = {
        let _guest_call_guard = GuestGlesCallGuard::enter();
        f(gles.as_mut(), &mut env.mem, shadow)
    };
    if trace {
        let err = unsafe { gles.GetError() };
        if err != 0 {
            log!(
                "[--trace-gl-errors] {} call reported glGetError() = {:#x} \
                 dispatched from {}:{}",
                gles.diagnostic_profile(),
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
    let res = {
        let _guest_call_guard = GuestGlesCallGuard::enter();
        f(gles.as_mut(), &mut env.mem)
    };
    if trace {
        let err = unsafe { gles.GetError() };
        if err != 0 {
            log!(
                "[--trace-gl-errors] {} call reported glGetError() = {:#x} \
                 dispatched from {}:{}",
                gles.diagnostic_profile(),
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
                AtomicU32::new(0),
                AtomicU32::new(0),
                AtomicU32::new(0),
                AtomicU32::new(0),
                AtomicU32::new(0),
                AtomicU32::new(0),
                AtomicU32::new(0),
                AtomicU32::new(0),
                AtomicU32::new(0),
                AtomicU32::new(0),
                AtomicU32::new(0),
                AtomicU32::new(0),
                AtomicU32::new(0),
                AtomicU32::new(0),
                AtomicU32::new(0),
                AtomicU32::new(0),
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
                log_dbg!(
                    "{} glGetError() returned {:#x} (already reported)",
                    gles.diagnostic_profile(),
                    err
                );
            } else {
                log!(
                    "Warning: {} glGetError() returned {:#x} (subsequent repeats \
                     of the same code are silenced; rerun with \
                     --trace-gl-errors to identify the originating GL call)",
                    gles.diagnostic_profile(),
                    err
                );
            }
        }
        err
    })
}
fn glEnable(env: &mut Environment, cap: GLenum) {
    if cap == gles11::FOG {
        with_ctx_mem_and_shadow(env, |gles, _mem, shadow| unsafe {
            shadow.fog_enabled = true;
            gles.Enable(cap)
        });
    } else {
        with_ctx_and_mem(env, |gles, _mem| unsafe { gles.Enable(cap) });
    }
}
fn glIsEnabled(env: &mut Environment, cap: GLenum) -> GLboolean {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.IsEnabled(cap) })
}
fn glDisable(env: &mut Environment, cap: GLenum) {
    if cap == gles11::FOG {
        with_ctx_mem_and_shadow(env, |gles, _mem, shadow| unsafe {
            shadow.fog_enabled = false;
            gles.Disable(cap)
        });
    } else {
        with_ctx_and_mem(env, |gles, _mem| unsafe { gles.Disable(cap) });
    }
}
fn glClientActiveTexture(env: &mut Environment, texture: GLenum) {
    with_ctx_mem_and_shadow(env, |gles, _mem, shadow| unsafe {
        let unit = texture.wrapping_sub(gles11::TEXTURE0) as usize;
        if unit < software_vao::MAX_TEXTURE_UNITS {
            shadow.client_active_texture = unit;
        }
        gles.ClientActiveTexture(texture)
    })
}
fn glEnableClientState(env: &mut Environment, array: GLenum) {
    with_ctx_mem_and_shadow(env, |gles, _mem, shadow| unsafe {
        let unit = shadow.client_active_texture;
        shadow
            .vertex_array_state
            .set_client_state(array, unit, true);
        gles.EnableClientState(array)
    });
}
fn glDisableClientState(env: &mut Environment, array: GLenum) {
    with_ctx_mem_and_shadow(env, |gles, _mem, shadow| unsafe {
        let unit = shadow.client_active_texture;
        shadow
            .vertex_array_state
            .set_client_state(array, unit, false);
        gles.DisableClientState(array)
    });
}

fn glGetBooleanv(env: &mut Environment, pname: GLenum, params: MutPtr<GLboolean>) {
    if params.is_null() {
        return;
    }
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
    if params.is_null() {
        return;
    }
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
    if params.is_null() {
        return;
    }
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
        // GL_VERTEX_ARRAY_BINDING_OES: the software VAO fallback has no host
        // object to ask.
        0x85B5 if with_ctx_mem_and_shadow(env, |gles, _mem, _shadow| !gles.supports_vao_oes()) => {
            let binding =
                with_ctx_mem_and_shadow(env, |_gles, _mem, shadow| shadow.current_vertex_array);
            env.mem.write(params, binding as GLint);
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
fn glGetFixedv(env: &mut Environment, pname: GLenum, params: MutPtr<GLfixed>) {
    if params.is_null() {
        return;
    }
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
        unsafe { gles.GetFixedv(pname, params) };
    });
}
fn glGetPointerv(env: &mut Environment, pname: GLenum, params: MutPtr<ConstVoidPtr>) {
    if params.is_null() {
        return;
    }
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

    // The original iPhone, iPhone 3G and first two iPod touch generations use
    // the PowerVR MBX, which has no NPOT texture support. Every later device
    // has an SGX, whose ES 1.1 driver also advertises
    // GL_APPLE_texture_2D_limited_npot. cocos2d checks for it before loading
    // NPOT .pvr/.pvr.ccz textures and refuses to load them without it, which
    // left sprites in Forlorn drawn as plain white boxes.
    let is_mbx_device = matches!(
        env.options.device_family,
        Some(
            DeviceFamily::iPhone
                | DeviceFamily::iPhone3G
                | DeviceFamily::iPodTouch
                | DeviceFamily::iPodTouch2
        )
    );
    let s: &[u8] = if !is_es2 {
        match name {
            gles11::VENDOR => b"Imagination Technologies",
            gles11::RENDERER => b"PowerVR MBXLite with VGPLite",
            gles11::VERSION => b"OpenGL ES-CM 1.1 (76)",
            // Includes GL_OES_matrix_palette: the real PowerVR SGX in the
            // iPhone 3GS exposes it, and touchHLE now emulates palette
            // skinning CPU-side (see gles1_on_gl2's skin_vertices). Games such
            // as LEGO Ninjago: Rise of the Snakes feature-test this string and
            // use the palette path for skinned character meshes.
            gles11::EXTENSIONS => b"GL_APPLE_texture_max_level GL_EXT_discard_framebuffer GL_EXT_texture_filter_anisotropic GL_EXT_texture_lod_bias GL_IMG_read_format GL_IMG_texture_compression_pvrtc GL_IMG_texture_format_BGRA8888 GL_OES_blend_subtract GL_OES_compressed_paletted_texture GL_OES_depth24 GL_OES_draw_texture GL_OES_framebuffer_object GL_OES_mapbuffer GL_OES_matrix_palette GL_OES_point_size_array GL_OES_point_sprite GL_OES_read_format GL_OES_rgb8_rgba8 GL_OES_texture_mirrored_repeat GL_OES_vertex_array_object ",
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
            // GAMELOFT BYPASS: the Asphalt 8 engine feature-tests
            // GL_OES_element_index_uint before selecting its renderer; without
            // it the game never submits any draw calls and the screen stays
            // black. GL_APPLE_framebuffer_multisample is deliberately NOT
            // advertised (fazidroid "A8 gfx fix & vulkan optimization"): with
            // it the engine picks the Apple multisample-resolve FBO path,
            // whose resolve never lands on the visible renderbuffer here and
            // yields a black screen; without it the engine streams straight
            // to the single-sample framebuffer.
            gles11::EXTENSIONS => b"GL_APPLE_texture_2D_limited_npot GL_APPLE_texture_format_BGRA8888 GL_APPLE_texture_max_level GL_EXT_debug_label GL_EXT_discard_framebuffer GL_EXT_occlusion_query_boolean GL_EXT_read_format_bgra GL_EXT_texture_filter_anisotropic GL_EXT_texture_lod_bias GL_IMG_read_format GL_IMG_texture_compression_pvrtc GL_IMG_texture_format_BGRA8888 GL_OES_depth24 GL_OES_depth_texture GL_OES_element_index_uint GL_OES_fbo_render_mipmap GL_OES_framebuffer_object GL_OES_mapbuffer GL_OES_packed_depth_stencil GL_OES_rgb8_rgba8 GL_OES_standard_derivatives GL_OES_stencil_wrap GL_OES_texture_float GL_OES_texture_half_float GL_OES_texture_mirrored_repeat GL_OES_vertex_array_object GL_OES_vertex_half_float ",
            _ => b"Unknown",
        }
    };
    let npot_prefix: &[u8] = if !is_es2 && name == gles11::EXTENSIONS && !is_mbx_device {
        b"GL_APPLE_texture_2D_limited_npot "
    } else {
        b""
    };
    let new_str = env
        .mem
        .alloc_and_write_cstr(&[npot_prefix, s].concat())
        .cast_const();
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
    // ULTRAHLE_MINIONJUMP_VIEWPORT_BEGIN
    let (x, y, width, height) = if matches!(
        env.bundle.bundle_identifier(),
        "com.apprisetec9.minionjump" | "com.risinghighapps.kingdomprincepro"
    ) && x == 0
        && y == 0
        && width == 768
        && height == 1024
    {
        log!("UltraHLE MinionJump: viewport swap 768x1024 -> 1024x768");
        (0, 0, 1024, 768)
    } else if crate::env_flag_cached!("TOUCHHLE_FORCE_IPAD_LANDSCAPE_SCREEN")
        && x == 0
        && y == 0
        && width == 768
        && height == 1024
    {
        log!("UltraHLE MinionJump: env iPad landscape viewport swap 768x1024 -> 1024x768");
        (0, 0, 1024, 768)
    } else {
        (x, y, width, height)
    };
    // ULTRAHLE_MINIONJUMP_VIEWPORT_END
    let (mut x, mut y, mut width, mut height) = (x, y, width, height);

    if crate::env_flag_cached!("TOUCHHLE_FORCE_LANDSCAPE_VIEWPORT") {
        // PotatoGold/adrastea-style landscape apps can end up with a 20px
        // status-bar-shortened portrait-derived viewport, e.g. 460x320,
        // even after UIScreen/EAGL have been made landscape. That leaves the
        // final frame cropped/scuffed. In this compatibility mode, promote
        // the common iPhone landscape viewport cases to the full 480x320
        // logical viewport.
        let should_force = (x == 0 && y == 0 && width == 460 && height == 320)
            || (x == 0 && y == 0 && width == 320 && height == 460)
            || (x == 0 && y == 0 && width == 320 && height == 480);

        if should_force {
            log!(
                "TOUCHHLE_FORCE_LANDSCAPE_VIEWPORT=1: overriding glViewport({}, {}, {}, {}) to glViewport(0, 0, 480, 320)",
                x,
                y,
                width,
                height
            );
            x = 0;
            y = 0;
            width = 480;
            height = 320;
        }
    }

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
    // The scale hack only enlarges the EAGL drawable's renderbuffer, not the
    // framebuffer objects an app creates for itself. Scaling a viewport aimed
    // at one of those draws the app's content `factor` times too large inside
    // an unscaled target, which is why Flappy Bird (which renders through a
    // 288x512 offscreen buffer at its native art size) appeared hugely zoomed
    // in at any scale above 1, while apps that draw straight to the drawable
    // (e.g. Touch & Go) were unaffected.
    //
    // GL_FRAMEBUFFER_BINDING_OES and GL_FRAMEBUFFER_BINDING are both 0x8CA6, so
    // this single query is correct for ES 1.1 and ES 2.0 alike.
    let thread = env.current_thread;
    // Remember the request so it can be re-applied if the guest binds a
    // different framebuffer afterwards (see glBindFramebufferOES).
    env.framework_state
        .opengles
        .set_guest_viewport(thread, (x, y, width, height));
    let apply_scale_hack = env
        .framework_state
        .opengles
        .scale_hack_applies_to_viewport(thread);
    let (x, y, width, height) = if apply_scale_hack {
        (x * factor, y * factor, width * factor, height * factor)
    } else {
        (x, y, width, height)
    };
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

/// Keep the fog range mirror (see [GLShadowState]) in sync with a
/// `glFog{f,x}{,v}` call.
fn shadow_fog_param(shadow: &mut GLShadowState, pname: GLenum, value: f32) {
    match pname {
        gles11::FOG_START => shadow.fog_start = value,
        gles11::FOG_END => shadow.fog_end = value,
        _ => (),
    }
}
fn glFogf(env: &mut Environment, pname: GLenum, param: GLfloat) {
    with_ctx_mem_and_shadow(env, |gles, _mem, shadow| unsafe {
        shadow_fog_param(shadow, pname, param);
        gles.Fogf(pname, param)
    })
}
fn glFogx(env: &mut Environment, pname: GLenum, param: GLfixed) {
    with_ctx_mem_and_shadow(env, |gles, _mem, shadow| unsafe {
        shadow_fog_param(shadow, pname, param as f32 / 65536.0);
        gles.Fogx(pname, param)
    })
}
fn glFogfv(env: &mut Environment, pname: GLenum, params: ConstPtr<GLfloat>) {
    with_ctx_mem_and_shadow(env, |gles, mem, shadow| {
        if matches!(pname, gles11::FOG_START | gles11::FOG_END) {
            shadow_fog_param(shadow, pname, mem.read(params));
        }
        let params = mem.ptr_at(params, 4);
        unsafe { gles.Fogfv(pname, params) }
    })
}
fn glFogxv(env: &mut Environment, pname: GLenum, params: ConstPtr<GLfixed>) {
    with_ctx_mem_and_shadow(env, |gles, mem, shadow| {
        if matches!(pname, gles11::FOG_START | gles11::FOG_END) {
            shadow_fog_param(shadow, pname, mem.read(params) as f32 / 65536.0);
        }
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
    if n <= 0 || buffers.is_null() {
        return;
    }
    with_ctx_mem_and_shadow(env, |gles, mem, shadow| {
        let n_usize: GuestUSize = n.try_into().unwrap();
        let deleted: Vec<GLuint> = (0..n_usize).map(|i| mem.read(buffers + i)).collect();
        for buffer in &deleted {
            shadow.on_buffers_deleted(*buffer);
        }
        unsafe { gles.DeleteBuffers(n, deleted.as_ptr()) };
        crate::gles::perf_overlay::delete_gl_buffers(shadow.resource_group_token, &deleted);
    })
}
fn glBindBuffer(env: &mut Environment, target: GLenum, buffer: GLuint) {
    with_ctx_mem_and_shadow(env, |gles, _mem, shadow| unsafe {
        match target {
            ARRAY_BUFFER => shadow.array_buffer = buffer,
            ELEMENT_ARRAY_BUFFER => shadow.record_element_array_buffer_binding(buffer),
            _ => (),
        }
        if crate::env_flag_cached!("TOUCHHLE_TRACE_BUFFER_BINDINGS") {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static COUNT: AtomicUsize = AtomicUsize::new(0);
            if COUNT.fetch_add(1, Ordering::Relaxed) < 256 {
                let previous = match target {
                    ARRAY_BUFFER => Some(shadow.array_buffer),
                    ELEMENT_ARRAY_BUFFER => shadow.element_array_buffer,
                    _ => None,
                };
                log!(
                    "GLES_BIND_BUFFER target={:#x} buffer={} previous={:?}",
                    target,
                    buffer,
                    previous
                );
            }
        }
        gles.BindBuffer(target, buffer)
    })
}
/// DIAG: host-side cache of ARRAY_BUFFER contents keyed by buffer name, so
/// TOUCHHLE_TRACE_DRAW can read actual vertex (texcoord) values at draw time.
fn diag_buffer_cache() -> &'static std::sync::Mutex<HashMap<GLuint, Vec<u8>>> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<HashMap<GLuint, Vec<u8>>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}
unsafe fn debug_log_buffer_upload(
    gles: &mut dyn GLES,
    mem: &Mem,
    operation: &str,
    target: GLenum,
    offset: GuestGLintptr,
    size: GuestGLsizeiptr,
    data: ConstPtr<GLvoid>,
) {
    if !crate::env_flag_cached!("TOUCHHLE_DEBUG_ES2_DRAW") {
        return;
    }
    static COUNT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let index = COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if index >= 6 {
        return;
    }
    let binding_query = match target {
        ARRAY_BUFFER => 0x8894,
        ELEMENT_ARRAY_BUFFER => 0x8895,
        _ => 0,
    };
    let mut binding = 0;
    if binding_query != 0 {
        gles.GetIntegerv(binding_query, &mut binding);
        let _ = gles.GetError();
    }
    let sample_len = size.clamp(0, 960) as u32;
    let sample = if data.is_null() {
        "null".to_string()
    } else {
        let bytes = std::slice::from_raw_parts(
            mem.ptr_at(data.cast::<u8>(), sample_len),
            sample_len as usize,
        );
        bytes.iter().map(|byte| format!("{byte:02x}")).collect::<Vec<_>>().join(" ")
    };
    log!(
        "[GLES BUFFER TRACE] {} target={:#x} binding={} offset={} size={} data={:#x} first_bytes={}",
        operation,
        target,
        binding,
        offset,
        size,
        data.to_bits(),
        sample
    );
}

fn glBufferData(
    env: &mut Environment,
    target: GLenum,
    size: GuestGLsizeiptr,
    data: ConstPtr<GLvoid>,
    usage: GLenum,
) {
    with_ctx_mem_and_shadow(env, |gles, mem, shadow| unsafe {
        debug_log_buffer_upload(gles, mem, "BufferData", target, 0, size, data);
        if crate::env_flag_cached!("TOUCHHLE_TRACE_DRAW") && target == ARRAY_BUFFER && !data.is_null()
        {
            let mut bound: GLint = 0;
            gles.GetIntegerv(0x8894 /*ARRAY_BUFFER_BINDING*/, &mut bound);
            if bound > 0 && (1..=1_048_576).contains(&size) {
                let bytes = mem.bytes_at(data.cast::<u8>(), size as GuestUSize).to_vec();
                diag_buffer_cache().lock().unwrap().insert(bound as GLuint, bytes);
            }
        }
        if crate::env_flag_cached!("TOUCHHLE_TRACE_DRAW")
            && target == ARRAY_BUFFER
            && !data.is_null()
            && (1..=256).contains(&size)
        {
            use std::sync::atomic::{AtomicU32, Ordering};
            static BN: AtomicU32 = AtomicU32::new(0);
            let bn = BN.fetch_add(1, Ordering::Relaxed);
            if bn < 10 {
                let bytes = mem.bytes_at(data.cast::<u8>(), size as GuestUSize);
                let floats: Vec<f32> = bytes
                    .chunks_exact(4)
                    .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                    .collect();
                log!("BUFDATA-DIAG #{} ARRAY_BUFFER size={} floats={:?}", bn, size, floats);
            }
        }
        let binding = match target {
            ARRAY_BUFFER => gles11::ARRAY_BUFFER_BINDING,
            ELEMENT_ARRAY_BUFFER => ELEMENT_ARRAY_BUFFER_BINDING,
            0x88EB => 0x88ED,
            0x88EC => 0x88EF,
            0x8F36 => 0x8F36,
            0x8F37 => 0x8F37,
            0x8A11 => 0x8A28,
            0x8C8E => 0x8C8F,
            0x8F3F => 0x8F43,
            0x90EE => 0x90EF,
            0x90D2 => 0x90D3,
            0x92C0 => 0x92C1,
            _ => 0,
        };
        let mut bound_buffer: GLint = 0;
        if binding != 0 {
            gles.GetIntegerv(binding, &mut bound_buffer);
        }
        let data: *const GLvoid = if data.is_null() {
            std::ptr::null()
        } else {
            mem.ptr_at(data.cast::<u8>(), size.try_into().unwrap())
                .cast()
        };
        gles.BufferData(target, size as HostGLsizeiptr, data, usage);
        if bound_buffer > 0 && size >= 0 {
            crate::gles::perf_overlay::record_gl_buffer_allocation(
                shadow.resource_group_token,
                bound_buffer as GLuint,
                size as u64,
            );
        }
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
        debug_log_buffer_upload(gles, mem, "BufferSubData", target, offset, size, data);
        if crate::env_flag_cached!("TOUCHHLE_TRACE_DRAW") && target == ARRAY_BUFFER && !data.is_null()
        {
            let mut bound: GLint = 0;
            gles.GetIntegerv(0x8894 /*ARRAY_BUFFER_BINDING*/, &mut bound);
            if bound > 0 && offset >= 0 && size > 0 {
                let src = mem.bytes_at(data.cast::<u8>(), size as GuestUSize);
                let mut cache = diag_buffer_cache().lock().unwrap();
                let buf = cache.entry(bound as GLuint).or_default();
                let start = offset as usize;
                if let Some(end) = start.checked_add(size as usize).filter(|end| *end <= 4_194_304) {
                    if buf.len() < end {
                        buf.resize(end, 0);
                    }
                    buf[start..end].copy_from_slice(src);
                }
            }
        }
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

/// Is a buffer object bound to `GL_ARRAY_BUFFER` / `GL_ELEMENT_ARRAY_BUFFER`?
///
/// Answered from the [GLShadowState] mirror where possible. The element array
/// binding lives in vertex array object state, so right after a VAO switch
/// the mirror doesn't know it and we ask the driver once (and remember the
/// answer until the next switch).
unsafe fn buffer_is_bound(gles: &mut dyn GLES, shadow: &mut GLShadowState, target: GLenum) -> bool {
    match target {
        ARRAY_BUFFER => shadow.array_buffer != 0,
        ELEMENT_ARRAY_BUFFER => {
            if let Some(binding) = shadow.element_array_buffer {
                binding != 0
            } else {
                let mut buffer_binding: GLint = 0;
                gles.GetIntegerv(ELEMENT_ARRAY_BUFFER_BINDING, &mut buffer_binding);
                // Internal state query: strict native drivers (e.g. Adreno
                // GLES-CM) raise GL_INVALID_ENUM or GL_INVALID_OPERATION for
                // these binding queries even though the returned value is
                // valid. Swallow the error so the guest's error queue is not
                // polluted.
                let _ = gles.GetError();
                shadow.element_array_buffer = Some(buffer_binding as GLuint);
                buffer_binding != 0
            }
        }
        _ => false,
    }
}

/// Translate the `pointer` argument of a `gl*Pointer` / `glDrawElements`
/// call: if a buffer object is bound to `target`, it's an offset into that
/// buffer and passes through unchanged; otherwise it's a guest pointer to
/// client-side data that must become a host pointer.
unsafe fn translate_pointer_or_offset_to_host(
    gles: &mut dyn GLES,
    mem: &Mem,
    shadow: &mut GLShadowState,
    pointer_or_offset: ConstVoidPtr,
    target: GLenum,
) -> *const GLvoid {
    if buffer_is_bound(gles, shadow, target) {
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
    // Internal state query: strict native drivers (e.g. Adreno GLES-CM) raise
    // GL_INVALID_ENUM or GL_INVALID_OPERATION for these binding queries even
    // though the returned value is valid. Swallow the error so the guest's
    // error queue is not polluted on every draw call (same rationale as
    // clamp_fog_state_values).
    let _ = gles.GetError();
    if buffer_binding != 0 {
        let offset = pointer_or_offset as usize;
        // A buffer object is bound, so the "pointer" is really an offset and
        // must be passed through unchanged. Truncate to the guest's 32-bit
        // address space instead of panicking when a host pointer that cannot
        // be an offset at all arrives here (same convention as
        // `Mem::host_ptr_to_guest_ptr`).
        Ptr::from_bits(offset as u32)
    } else if pointer_or_offset.is_null() {
        Ptr::null()
    } else {
        mem.host_ptr_to_guest_ptr(pointer_or_offset)
    }
}

/// Describe a `gl*Pointer` call for [GLShadowState::vertex_array_state], so
/// the software vertex array object emulation can restore it later.
fn array_pointer_record(
    shadow: &GLShadowState,
    size: GLint,
    type_: GLenum,
    normalized: GLboolean,
    stride: GLsizei,
    pointer: ConstVoidPtr,
) -> ArrayPointer {
    ArrayPointer {
        set: true,
        size,
        type_,
        normalized,
        stride,
        pointer: pointer.to_bits(),
        buffer: shadow.array_buffer,
    }
}
fn record_fixed_array_pointer(
    shadow: &mut GLShadowState,
    array: FixedArray,
    size: GLint,
    type_: GLenum,
    stride: GLsizei,
    pointer: ConstVoidPtr,
) {
    let record = array_pointer_record(shadow, size, type_, 0, stride, pointer);
    shadow.vertex_array_state.set_fixed_pointer(array, record);
}

fn glColorPointer(
    env: &mut Environment,
    size: GLint,
    type_: GLenum,
    stride: GLsizei,
    pointer: ConstVoidPtr,
) {
    with_ctx_mem_and_shadow(env, |gles, mem, shadow| unsafe {
        record_fixed_array_pointer(shadow, FixedArray::Color, size, type_, stride, pointer);
        let pointer = translate_pointer_or_offset_to_host(gles, mem, shadow, pointer, ARRAY_BUFFER);
        gles.ColorPointer(size, type_, stride, pointer)
    })
}
fn glNormalPointer(env: &mut Environment, type_: GLenum, stride: GLsizei, pointer: ConstVoidPtr) {
    with_ctx_mem_and_shadow(env, |gles, mem, shadow| unsafe {
        record_fixed_array_pointer(shadow, FixedArray::Normal, 3, type_, stride, pointer);
        let pointer = translate_pointer_or_offset_to_host(gles, mem, shadow, pointer, ARRAY_BUFFER);
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
    with_ctx_mem_and_shadow(env, |gles, mem, shadow| unsafe {
        let record = array_pointer_record(shadow, size, type_, 0, stride, pointer);
        let unit = shadow.client_active_texture;
        shadow
            .vertex_array_state
            .set_tex_coord_pointer(unit, record);
        let pointer = translate_pointer_or_offset_to_host(gles, mem, shadow, pointer, ARRAY_BUFFER);
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
    with_ctx_mem_and_shadow(env, |gles, mem, shadow| unsafe {
        record_fixed_array_pointer(shadow, FixedArray::Vertex, size, type_, stride, pointer);
        let pointer = translate_pointer_or_offset_to_host(gles, mem, shadow, pointer, ARRAY_BUFFER);
        gles.VertexPointer(size, type_, stride, pointer)
    })
}

/// `glPointSizePointerOES` (`GL_OES_point_size_array`). Forwards through the
/// GLES trait so per-vertex point sizes are applied by backends that support
/// them (native ES 1.1).
fn glPointSizePointerOES(
    env: &mut Environment,
    type_: GLenum,
    stride: GLsizei,
    pointer: ConstVoidPtr,
) {
    with_ctx_mem_and_shadow(env, |gles, mem, shadow| unsafe {
        record_fixed_array_pointer(shadow, FixedArray::PointSize, 1, type_, stride, pointer);
        let pointer = translate_pointer_or_offset_to_host(gles, mem, shadow, pointer, ARRAY_BUFFER);
        gles.PointSizePointerOES(type_, stride, pointer)
    })
}

fn glGetTexParameteriv(
    env: &mut Environment,
    target: GLenum,
    pname: GLenum,
    params: MutPtr<GLint>,
) {
    with_ctx_and_mem(env, |gles, mem| {
        let params = mem.ptr_at_mut(params, 1);
        unsafe { gles.GetTexParameteriv(target, pname, params) }
    })
}
fn glGetTexParameterfv(
    env: &mut Environment,
    target: GLenum,
    pname: GLenum,
    params: MutPtr<GLfloat>,
) {
    with_ctx_and_mem(env, |gles, mem| {
        let params = mem.ptr_at_mut(params, 1);
        unsafe { gles.GetTexParameterfv(target, pname, params) }
    })
}
fn glGetTexParameterxv(
    env: &mut Environment,
    target: GLenum,
    pname: GLenum,
    params: MutPtr<GLfixed>,
) {
    with_ctx_and_mem(env, |gles, mem| {
        let params = mem.ptr_at_mut(params, 1);
        unsafe { gles.GetTexParameterxv(target, pname, params) }
    })
}
fn glGetTexEnvxv(env: &mut Environment, target: GLenum, pname: GLenum, params: MutPtr<GLfixed>) {
    with_ctx_and_mem(env, |gles, mem| {
        let params = mem.ptr_at_mut(params, 16);
        unsafe { gles.GetTexEnvxv(target, pname, params) }
    })
}
fn glGetClipPlanef(env: &mut Environment, plane: GLenum, equation: MutPtr<GLfloat>) {
    with_ctx_and_mem(env, |gles, mem| {
        let equation = mem.ptr_at_mut(equation, 4);
        unsafe { gles.GetClipPlanef(plane, equation) }
    })
}
fn glGetClipPlanex(env: &mut Environment, plane: GLenum, equation: MutPtr<GLfixed>) {
    with_ctx_and_mem(env, |gles, mem| {
        let equation = mem.ptr_at_mut(equation, 4);
        unsafe { gles.GetClipPlanex(plane, equation) }
    })
}
fn glGetLightfv(env: &mut Environment, light: GLenum, pname: GLenum, params: MutPtr<GLfloat>) {
    with_ctx_and_mem(env, |gles, mem| {
        let params = mem.ptr_at_mut(params, 4);
        unsafe { gles.GetLightfv(light, pname, params) }
    })
}
fn glGetLightxv(env: &mut Environment, light: GLenum, pname: GLenum, params: MutPtr<GLfixed>) {
    with_ctx_and_mem(env, |gles, mem| {
        let params = mem.ptr_at_mut(params, 4);
        unsafe { gles.GetLightxv(light, pname, params) }
    })
}
fn glGetMaterialfv(env: &mut Environment, face: GLenum, pname: GLenum, params: MutPtr<GLfloat>) {
    with_ctx_and_mem(env, |gles, mem| {
        let params = mem.ptr_at_mut(params, 4);
        unsafe { gles.GetMaterialfv(face, pname, params) }
    })
}
fn glGetMaterialxv(env: &mut Environment, face: GLenum, pname: GLenum, params: MutPtr<GLfixed>) {
    with_ctx_and_mem(env, |gles, mem| {
        let params = mem.ptr_at_mut(params, 4);
        unsafe { gles.GetMaterialxv(face, pname, params) }
    })
}

fn glCompressedTexSubImage2D(
    env: &mut Environment,
    target: GLenum,
    level: GLint,
    xoffset: GLint,
    yoffset: GLint,
    width: GLsizei,
    height: GLsizei,
    format: GLenum,
    image_size: GLsizei,
    data: ConstVoidPtr,
) {
    with_ctx_mem_and_shadow(env, |gles, mem, shadow| unsafe {
        let image_size_usize = match usize::try_from(image_size) {
            Ok(size) => size,
            Err(_) => {
                gles.CompressedTexSubImage2D(
                    target,
                    level,
                    xoffset,
                    yoffset,
                    width,
                    height,
                    format,
                    image_size,
                    std::ptr::null(),
                );
                return;
            }
        };
        let bound_texture = current_bound_texture(gles, target);
        let texture_level = bound_texture
            .and_then(|texture| shadow.pvrtc_texture_level(target, texture, level));
        if pvrtc_subimage_matches_level(
            texture_level,
            xoffset,
            yoffset,
            width,
            height,
            format,
        ) && !data.is_null()
            && crate::gles::util::pvrtc_payload_size(format, width, height)
                == Some(image_size_usize)
        {
            if let Some((is_2bit, is_opaque)) = crate::gles::util::pvrtc_format_properties(format) {
                let data = mem.ptr_at(data.cast::<u8>(), image_size_usize as GuestUSize);
                let payload = std::slice::from_raw_parts(data, image_size_usize);
                let pixels = crate::image::decode_pvrtc_with_alpha(
                    payload,
                    is_2bit,
                    width as u32,
                    height as u32,
                    is_opaque,
                );
                // DIAG (TOUCHHLE_DUMP_TEX=1): dump a few decoded PVRTC atlases
                // (RGBA) so we can see whether the decoder output is correct
                // vs scrambled/blank — the suspected source of texture glitches.
                if crate::env_flag_cached!("TOUCHHLE_DUMP_TEX") {
                    use std::sync::atomic::{AtomicU32, Ordering};
                    static N: AtomicU32 = AtomicU32::new(0);
                    let k = N.fetch_add(1, Ordering::Relaxed);
                    if k < 8 {
                        let w = width as usize;
                        let h = height as usize;
                        let mut ppm = format!("P6\n{} {}\n255\n", w, h).into_bytes();
                        for &px in pixels.iter() {
                            let b = (px as u32).to_le_bytes();
                            ppm.extend_from_slice(&[b[0], b[1], b[2]]);
                        }
                        let _ = std::fs::write(format!("tex_{}_{}x{}_fmt{:#x}.ppm", k, w, h, format), &ppm);
                        log!("DUMP-TEX wrote tex_{}_{}x{}_fmt{:#x}.ppm", k, w, h, format);
                    }
                }
                // PVRTC compressed subimages may only replace a complete level.
                // If the level's RGBA storage was never materialized (because
                // `glTexStorage2D` immutable storage was emulated away — see
                // `glTexStorage2D`), the first upload must allocate it via
                // `glTexImage2D`; a plain `glTexSubImage2D` onto unallocated
                // storage fails with `GL_INVALID_OPERATION`.
                let level_unallocated = bound_texture
                    .map(|texture| shadow.pvrtc_level_unallocated(target, texture, level))
                    .unwrap_or(false);
                if level_unallocated {
                    gles.TexImage2D(
                        target,
                        level,
                        gles11::RGBA as GLint,
                        width,
                        height,
                        0,
                        gles11::RGBA,
                        gles11::UNSIGNED_BYTE,
                        pixels.as_ptr().cast(),
                    );
                    if let Some(texture) = bound_texture {
                        shadow.mark_pvrtc_level_allocated(target, texture, level);
                    }
                } else {
                    gles.TexSubImage2D(
                        target,
                        level,
                        xoffset,
                        yoffset,
                        width,
                        height,
                        gles11::RGBA,
                        gles11::UNSIGNED_BYTE,
                        pixels.as_ptr().cast(),
                    );
                }
                log_once!(
                    "Software-decoded a full PVRTC glCompressedTexSubImage2D update into RGBA storage"
                );
                return;
            }
        }
        let data = if data.is_null() {
            std::ptr::null()
        } else {
            mem.ptr_at(data.cast::<u8>(), image_size_usize as GuestUSize)
                .cast()
        };
        // DIAG: a PVRTC sub-update that did NOT take the decode path above
        // (partial / non-full-level) falls through to the raw driver call on
        // RGBA storage, which fails → that region of the atlas stays blank.
        if crate::env_flag_cached!("TOUCHHLE_TRACE_TEX") {
            let is_pvrtc = crate::gles::util::pvrtc_format_properties(format).is_some();
            log!(
                "TEX-DIAG SUBIMAGE-FALLTHROUGH format={:#x} pvrtc={} off=({},{}) {}x{} image_size={} level_known={}",
                format, is_pvrtc, xoffset, yoffset, width, height, image_size,
                texture_level.is_some()
            );
        }
        gles.CompressedTexSubImage2D(
            target, level, xoffset, yoffset, width, height, format, image_size, data,
        )
    })
}

fn glDrawTexfOES(
    env: &mut Environment,
    x: GLfloat,
    y: GLfloat,
    z: GLfloat,
    width: GLfloat,
    height: GLfloat,
) {
    {
        use std::sync::atomic::{AtomicBool, Ordering};
        static SEEN: AtomicBool = AtomicBool::new(false);
        if !SEEN.swap(true, Ordering::Relaxed) {
            log!(
                "First glDrawTexfOES({}, {}, {}, {}, {}) [this log will only be shown once]",
                x,
                y,
                z,
                width,
                height
            );
        }
    }
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        crate::gles::perf_overlay::record_draw();
        gles.DrawTexfOES(x, y, z, width, height)
    })
}
fn glDrawTexiOES(env: &mut Environment, x: GLint, y: GLint, z: GLint, width: GLint, height: GLint) {
    {
        use std::sync::atomic::{AtomicBool, Ordering};
        static SEEN: AtomicBool = AtomicBool::new(false);
        if !SEEN.swap(true, Ordering::Relaxed) {
            log!(
                "First glDrawTexiOES({}, {}, {}, {}, {}) [this log will only be shown once]",
                x,
                y,
                z,
                width,
                height
            );
        }
    }
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        crate::gles::perf_overlay::record_draw();
        gles.DrawTexiOES(x, y, z, width, height)
    })
}
fn glDrawTexxOES(
    env: &mut Environment,
    x: GLfixed,
    y: GLfixed,
    z: GLfixed,
    width: GLfixed,
    height: GLfixed,
) {
    {
        use std::sync::atomic::{AtomicBool, Ordering};
        static SEEN: AtomicBool = AtomicBool::new(false);
        if !SEEN.swap(true, Ordering::Relaxed) {
            log!(
                "First glDrawTexxOES({}, {}, {}, {}, {}) [this log will only be shown once]",
                x,
                y,
                z,
                width,
                height
            );
        }
    }
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        crate::gles::perf_overlay::record_draw();
        gles.DrawTexxOES(x, y, z, width, height)
    })
}
fn glDrawTexfvOES(env: &mut Environment, coords: ConstPtr<GLfloat>) {
    with_ctx_and_mem(env, |gles, mem| {
        let coords = mem.ptr_at(coords, 5);
        crate::gles::perf_overlay::record_draw();
        unsafe { gles.DrawTexfvOES(coords) }
    })
}
fn glDrawTexivOES(env: &mut Environment, coords: ConstPtr<GLint>) {
    with_ctx_and_mem(env, |gles, mem| {
        let coords = mem.ptr_at(coords, 5);
        crate::gles::perf_overlay::record_draw();
        unsafe { gles.DrawTexivOES(coords) }
    })
}
fn glDrawTexxvOES(env: &mut Environment, coords: ConstPtr<GLfixed>) {
    with_ctx_and_mem(env, |gles, mem| {
        let coords = mem.ptr_at(coords, 5);
        crate::gles::perf_overlay::record_draw();
        unsafe { gles.DrawTexxvOES(coords) }
    })
}
fn glDrawTexsOES(env: &mut Environment, x: i16, y: i16, z: i16, width: i16, height: i16) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        crate::gles::perf_overlay::record_draw();
        gles.DrawTexsOES(x, y, z, width, height)
    })
}
fn glDrawTexsvOES(env: &mut Environment, coords: ConstPtr<i16>) {
    with_ctx_and_mem(env, |gles, mem| {
        let coords = mem.ptr_at(coords, 5);
        crate::gles::perf_overlay::record_draw();
        unsafe { gles.DrawTexsvOES(coords) }
    })
}
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
    env: &mut Environment,
    target: GLenum,
    numAttachments: GLsizei,
    attachments: ConstPtr<GLenum>,
) {
    // GL_EXT_discard_framebuffer is a bandwidth hint. On tile-based GPUs
    // (ARM Mali, Qualcomm Adreno, PowerVR) honouring it lets the driver skip
    // writing tile memory back to system RAM at the end of the frame. The
    // guest attachment enums (GL_COLOR_EXT / GL_DEPTH_EXT / GL_STENCIL_EXT)
    // share their values with the host extension, so forward them unchanged.
    with_ctx_and_mem(env, |gles, mem| unsafe {
        // Discarding is only a bandwidth hint; on backends without
        // GL_EXT_discard_framebuffer there is nothing to forward, and asking
        // anyway would log an "unimplemented GLES API" warning per frame.
        if !gles.discard_ext_supported() {
            return;
        }
        let n = numAttachments.max(0) as GuestUSize;
        let ptr = if n == 0 || attachments.is_null() {
            std::ptr::null()
        } else {
            mem.bytes_at(attachments.cast(), n * 4).as_ptr().cast()
        };
        gles.DiscardFramebufferEXT(target, numAttachments, ptr);
    });
}

/// `glPushGroupMarkerEXT` — debug marker from `GL_EXT_debug_marker`.
/// No-op on hosts that don't expose the extension.
fn glPushGroupMarkerEXT(_env: &mut Environment, _length: GLsizei, _marker: ConstPtr<u8>) {
    // Debug markers are hints; safe to ignore.
}

/// `glPopGroupMarkerEXT` — debug marker from `GL_EXT_debug_marker`.
/// No-op on hosts that don't expose the extension.
fn glPopGroupMarkerEXT(_env: &mut Environment) {
    // Debug markers are hints; safe to ignore.
}

fn glBindVertexArrayOES(env: &mut Environment, array: GLuint) {
    with_ctx_mem_and_shadow(env, |gles, mem, shadow| unsafe {
        if gles.supports_vao_oes() {
            shadow.current_vertex_array = array;
            shadow.invalidate_vao_state();
            gles.BindVertexArrayOES(array);
        } else {
            if array != 0 && !shadow.is_software_vertex_array(array) {
                log_once!("Ignoring glBindVertexArrayOES for an ungenerated name on the software fallback");
                return;
            }
            let (element_array_buffer, outgoing) = shadow.bind_software_vertex_array(array);
            gles.BindBuffer(ELEMENT_ARRAY_BUFFER, element_array_buffer);
            if let Some(outgoing) = outgoing {
                apply_software_vertex_array_state(gles, mem, shadow, &outgoing);
            }
        }
    });
}
/// Bring the host's client array state from `outgoing` to the state of the
/// newly bound software vertex array object.
unsafe fn apply_software_vertex_array_state(
    gles: &mut dyn GLES,
    mem: &Mem,
    shadow: &GLShadowState,
    outgoing: &software_vao::VertexArrayState,
) {
    software_vao::apply_vertex_array_state(
        gles,
        mem,
        outgoing,
        &shadow.vertex_array_state,
        shadow.array_buffer,
        shadow.client_active_texture,
    );
}
fn glDeleteVertexArraysOES(env: &mut Environment, n: GLsizei, arrays: ConstPtr<GLuint>) {
    if n <= 0 || arrays.is_null() {
        return;
    }
    with_ctx_mem_and_shadow(env, |gles, mem, shadow| unsafe {
        let count = n as GuestUSize;
        if gles.supports_vao_oes() {
            let slice = mem.bytes_at(arrays.cast(), count * 4);
            for array in slice.chunks_exact(4).map(|bytes| u32::from_ne_bytes(bytes.try_into().unwrap())) {
                if array == shadow.current_vertex_array {
                    shadow.current_vertex_array = 0;
                    shadow.invalidate_vao_state();
                }
            }
            gles.DeleteVertexArraysOES(n, slice.as_ptr().cast());
        } else {
            for index in 0..count {
                let array = mem.read(arrays + index);
                if let Some((element_array_buffer, outgoing)) =
                    shadow.delete_software_vertex_array(array)
                {
                    gles.BindBuffer(ELEMENT_ARRAY_BUFFER, element_array_buffer);
                    if let Some(outgoing) = outgoing {
                        apply_software_vertex_array_state(gles, mem, shadow, &outgoing);
                    }
                }
            }
        }
    });
}
fn glGenVertexArraysOES(env: &mut Environment, n: GLsizei, arrays: MutPtr<GLuint>) {
    if n <= 0 || arrays.is_null() {
        return;
    }
    with_ctx_mem_and_shadow(env, |gles, mem, shadow| unsafe {
        let count = n as GuestUSize;
        if gles.supports_vao_oes() {
            let slice = mem.bytes_at_mut(arrays.cast(), count * 4);
            gles.GenVertexArraysOES(n, slice.as_mut_ptr().cast());
        } else {
            for index in 0..count {
                let array = shadow.generate_software_vertex_array();
                mem.write(arrays + index, array);
            }
        }
    });
}
fn glIsVertexArrayOES(env: &mut Environment, array: GLuint) -> GLboolean {
    with_ctx_mem_and_shadow(env, |gles, _mem, shadow| unsafe {
        if gles.supports_vao_oes() {
            gles.IsVertexArrayOES(array)
        } else {
            u8::from(shadow.is_software_vertex_array(array))
        }
    })
}
fn glCurrentPaletteMatrixOES(env: &mut Environment, matrixpaletteindex: GLuint) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.CurrentPaletteMatrixOES(matrixpaletteindex)
    })
}
fn glLoadPaletteFromModelViewMatrixOES(env: &mut Environment) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.LoadPaletteFromModelViewMatrixOES()
    })
}
fn glMatrixIndexPointerOES(
    env: &mut Environment,
    size: GLint,
    type_: GLenum,
    stride: GLsizei,
    pointer: ConstVoidPtr,
) {
    with_ctx_mem_and_shadow(env, |gles, mem, shadow| unsafe {
        record_fixed_array_pointer(
            shadow,
            FixedArray::MatrixIndex,
            size,
            type_,
            stride,
            pointer,
        );
        let pointer = translate_pointer_or_offset_to_host(gles, mem, shadow, pointer, ARRAY_BUFFER);
        gles.MatrixIndexPointerOES(size, type_, stride, pointer)
    })
}
fn glWeightPointerOES(
    env: &mut Environment,
    size: GLint,
    type_: GLenum,
    stride: GLsizei,
    pointer: ConstVoidPtr,
) {
    with_ctx_mem_and_shadow(env, |gles, mem, shadow| unsafe {
        record_fixed_array_pointer(shadow, FixedArray::Weight, size, type_, stride, pointer);
        let pointer = translate_pointer_or_offset_to_host(gles, mem, shadow, pointer, ARRAY_BUFFER);
        gles.WeightPointerOES(size, type_, stride, pointer)
    })
}
fn glGetBufferPointervOES(
    env: &mut Environment,
    _target: GLenum,
    _pname: GLenum,
    params: MutPtr<ConstVoidPtr>,
) {
    env.mem.write(params, Ptr::null());
}

/// Guard against a whole-emulator crash caused by enabled *client-side* vertex
/// attribute arrays whose pointer is not actually inside guest memory.
///
/// When no buffer object is bound for an attribute array, the host GL driver
/// reads the vertex data straight from the pointer touchHLE gave it. That
/// pointer is only safe if it addresses guest memory. A guest can leave an
/// array enabled with a bogus pointer — e.g. a stale offset into a vertex
/// buffer that was unbound or deleted before the draw (observed in My Talking
/// Tom: tapping Tom issues a glDrawElements whose attribute #3 still carries the
/// raw offset 0x9c0). The driver (both Mesa/llvmpipe and real GPU drivers) then
/// dereferences that wild address and segfaults the process.
///
/// To stay robust we check every enabled array that has no buffer bound and
/// temporarily disable any whose pointer falls outside guest memory, restoring
/// them after the draw. The draw then renders with default attribute values
/// instead of taking the whole emulator down.
///
/// This guard relies on vertex-attrib query entry points that are not valid on
/// native ES 1.1 backends. On real ES 1.1 drivers (e.g. Qualcomm Adreno) those
/// queries return GL_INVALID_OPERATION and poison the error queue, so we only
/// apply this on the GLES1-on-GL2 emulation backend where the queries are
/// supported.
unsafe fn guard_client_vertex_arrays(
    gles: &mut dyn GLES,
    mem: &Mem,
    shadow: &GLShadowState,
) -> Vec<GLuint> {
    // PERF: fixed-function-only apps never enable a generic vertex attribute
    // array, so there is nothing to guard and no reason to spend 1 + 2×N
    // driver queries per draw call on it.
    if !shadow.generic_attribs_used || !gles.is_gles1_on_gl2() {
        return Vec::new();
    }

    const VERTEX_ATTRIB_ARRAY_ENABLED: GLenum = 0x8622;
    const VERTEX_ATTRIB_ARRAY_BUFFER_BINDING: GLenum = 0x889F;
    const VERTEX_ATTRIB_ARRAY_POINTER: GLenum = 0x8645;
    const MAX_VERTEX_ATTRIBS: GLenum = 0x8869;

    let mut max_attribs: GLint = 0;
    gles.GetIntegerv(MAX_VERTEX_ATTRIBS, &mut max_attribs);
    // Be defensive about a backend that doesn't answer the query.
    let max_attribs = if (1..=64).contains(&max_attribs) {
        max_attribs as GLuint
    } else {
        16
    };

    let mut disabled = Vec::new();
    for index in 0..max_attribs {
        let mut enabled: GLint = 0;
        gles.GetVertexAttribiv(index, VERTEX_ATTRIB_ARRAY_ENABLED, &mut enabled);
        if enabled == 0 {
            continue;
        }
        let mut bound: GLint = 0;
        gles.GetVertexAttribiv(index, VERTEX_ATTRIB_ARRAY_BUFFER_BINDING, &mut bound);
        if bound != 0 {
            // Data comes from a buffer object; the driver handles bounds.
            continue;
        }
        let mut ptr: *mut GLvoid = std::ptr::null_mut();
        gles.GetVertexAttribPointerv(index, VERTEX_ATTRIB_ARRAY_POINTER, &mut ptr);
        if ptr.is_null() {
            // A null client pointer is the OpenGL default for an array that
            // has not been populated yet. Keep the enabled array state intact:
            // the fixed-function backend supplies its normal default attribute
            // values, while disabling it changes the guest-visible state and
            // can make later draws lose their vertex streams.
            continue;
        }
        if mem.is_host_ptr_in_guest_mem(ptr) {
            // A legitimate client-side array pointing into guest memory.
            continue;
        }
        log!(
            "Warning: disabling enabled vertex attribute array #{} for this draw: \
             no buffer is bound and its client pointer {:?} is outside guest memory \
             (would crash the host GL driver)",
            index,
            ptr
        );
        gles.DisableVertexAttribArray(index);
        disabled.push(index);
    }
    disabled
}

/// Valid primitive modes for GLES1/2 draw calls. A guest passing anything else
/// would raise GL_INVALID_ENUM (0x500) on the host driver; we filter those
/// draws out with a one-time warning instead of feeding the driver garbage.
const VALID_DRAW_MODES: [GLenum; 7] = [
    0x0000, // GL_POINTS
    0x0001, // GL_LINES
    0x0002, // GL_LINE_LOOP
    0x0003, // GL_LINE_STRIP
    0x0004, // GL_TRIANGLES
    0x0005, // GL_TRIANGLE_STRIP
    0x0006, // GL_TRIANGLE_FAN
];

fn draw_mode_is_valid(mode: GLenum) -> bool {
    VALID_DRAW_MODES.contains(&mode)
}

/// Warn once per (call site, bad mode) about an invalid draw mode.
fn warn_invalid_draw_mode(what: &str, mode: GLenum) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static SEEN: AtomicBool = AtomicBool::new(false);
    if !SEEN.swap(true, std::sync::atomic::Ordering::Relaxed) {
        log!(
            "Warning: guest issued a draw call with invalid mode 0x{:x} ({}); \
             skipping the draw instead of feeding the host driver an invalid \
             enum (avoids GL_INVALID_ENUM spam and driver-side stalls)",
            mode,
            what
        );
    }
}

fn glDrawArrays(env: &mut Environment, mode: GLenum, first: GLint, count: GLsizei) {
    {
        use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
        static SEEN: AtomicBool = AtomicBool::new(false);
        if !SEEN.swap(true, Ordering::Relaxed) {
            log!(
                "First glDrawArrays(mode=0x{:x}, first={}, count={}) (app submitting first draw) [this log will only be shown once]",
                mode, first, count
            );
        }

        if trace_potatogold_render() {
            static COUNT: AtomicU32 = AtomicU32::new(0);
            static LARGE_COUNT: AtomicU32 = AtomicU32::new(0);
            let n = COUNT.fetch_add(1, Ordering::Relaxed);
            let log_large = n >= 120
                && count > 12
                && LARGE_COUNT.fetch_add(1, Ordering::Relaxed) < 256;
            if n < 120 || log_large {
                log!(
                    "[POTATO RENDER TRACE] glDrawArrays #{} mode=0x{:x} first={} count={}",
                    n + 1,
                    mode,
                    first,
                    count
                );
            }
        }
    }
    if !draw_mode_is_valid(mode) {
        warn_invalid_draw_mode("glDrawArrays", mode);
        return;
    }
    with_ctx_mem_and_shadow(env, |gles, mem, shadow| unsafe {
        // DIAG (TOUCHHLE_TRACE_DRAW=1): dump the vertex-attribute + texture
        // state for the first several draws, to see whether the texcoord
        // attribute (A1 / loc 1) is actually fed and a texture is bound.
        if crate::env_flag_cached!("TOUCHHLE_TRACE_DRAW") {
            use std::sync::atomic::{AtomicU32, Ordering};
            static DN: AtomicU32 = AtomicU32::new(0);
            let dn = DN.fetch_add(1, Ordering::Relaxed);
            if dn < 12 {
                let mut prog: GLint = 0;
                gles.GetIntegerv(0x8B8D /*CURRENT_PROGRAM*/, &mut prog);
                let mut tex: GLint = 0;
                gles.GetIntegerv(0x8069 /*TEXTURE_BINDING_2D*/, &mut tex);
                let mut maxa: GLint = 0;
                gles.GetIntegerv(0x8869 /*MAX_VERTEX_ATTRIBS*/, &mut maxa);
                // Blend / depth / scissor state (layering suspects).
                let blend_on = gles.IsEnabled(0x0BE2 /*BLEND*/);
                let depth_on = gles.IsEnabled(0x0B71 /*DEPTH_TEST*/);
                let scissor_on = gles.IsEnabled(0x0C11 /*SCISSOR_TEST*/);
                let mut bsrc: GLint = 0;
                let mut bdst: GLint = 0;
                gles.GetIntegerv(0x0BE1 /*BLEND_SRC_RGB*/, &mut bsrc);
                gles.GetIntegerv(0x0BE0 /*BLEND_DST_RGB*/, &mut bdst);
                let mut dfunc: GLint = 0;
                gles.GetIntegerv(0x0B74 /*DEPTH_FUNC*/, &mut dfunc);
                let mut vp = [0i32; 4];
                gles.GetIntegerv(0x0BA2 /*VIEWPORT*/, vp.as_mut_ptr());
                // Bound-texture filter/wrap (mipmap-incompleteness suspect).
                let mut minf: GLint = 0;
                let mut magf: GLint = 0;
                let mut wraps: GLint = 0;
                gles.GetTexParameteriv(0x0DE1, 0x2801 /*MIN_FILTER*/, &mut minf);
                gles.GetTexParameteriv(0x0DE1, 0x2800 /*MAG_FILTER*/, &mut magf);
                gles.GetTexParameteriv(0x0DE1, 0x2802 /*WRAP_S*/, &mut wraps);
                let mut line = format!(
                    "DRAW-DIAG #{} mode={:#x} count={} program={} tex2d={} minf={:#x} magf={:#x} wrapS={:#x} blend={} src={:#x} dst={:#x} depth={} dfunc={:#x} scissor={} vp={:?} attribs:",
                    dn, mode, count, prog, tex, minf, magf, wraps, blend_on, bsrc, bdst, depth_on, dfunc, scissor_on, vp
                );
                for i in 0..(maxa.clamp(0, 16) as GLuint) {
                    let mut en: GLint = 0;
                    gles.GetVertexAttribiv(i, 0x8622 /*ARRAY_ENABLED*/, &mut en);
                    if en == 0 {
                        continue;
                    }
                    let mut sz: GLint = 0;
                    gles.GetVertexAttribiv(i, 0x8623 /*ARRAY_SIZE*/, &mut sz);
                    let mut buf: GLint = 0;
                    gles.GetVertexAttribiv(i, 0x889F /*ARRAY_BUFFER_BINDING*/, &mut buf);
                    let mut ty: GLint = 0;
                    gles.GetVertexAttribiv(i, 0x8625 /*ARRAY_TYPE*/, &mut ty);
                    let mut norm: GLint = 0;
                    gles.GetVertexAttribiv(i, 0x886A /*ARRAY_NORMALIZED*/, &mut norm);
                    let mut strd: GLint = 0;
                    gles.GetVertexAttribiv(i, 0x8624 /*ARRAY_STRIDE*/, &mut strd);
                    let mut p: *mut GLvoid = std::ptr::null_mut();
                    gles.GetVertexAttribPointerv(i, 0x8645 /*ARRAY_POINTER*/, &mut p);
                    line.push_str(&format!(
                        " [#{} sz={} type={:#x} norm={} stride={} vbo={} off={:?}]",
                        i, sz, ty, norm, strd, buf, p
                    ));
                }
                log!("{}", line);
                // Read the ACTUAL texcoord values for attr #1 from the cached
                // VBO content, for `first..first+count` vertices.
                {
                    let mut en: GLint = 0;
                    gles.GetVertexAttribiv(1, 0x8622, &mut en);
                    let mut vbo: GLint = 0;
                    gles.GetVertexAttribiv(1, 0x889F, &mut vbo);
                    let mut strd: GLint = 0;
                    gles.GetVertexAttribiv(1, 0x8624, &mut strd);
                    let mut p: *mut GLvoid = std::ptr::null_mut();
                    gles.GetVertexAttribPointerv(1, 0x8645, &mut p);
                    if en != 0 && vbo > 0 {
                        let off = p as usize;
                        let stride = if strd > 0 { strd as usize } else { 16 };
                        let cache = diag_buffer_cache().lock().unwrap();
                        if let Some(bytes) = cache.get(&(vbo as GLuint)) {
                            let mut tc = String::new();
                            for v in 0..(count.min(6) as usize) {
                                let base = off + (first as usize + v) * stride;
                                if base + 8 <= bytes.len() {
                                    let rd = |o: usize| {
                                        f32::from_le_bytes([
                                            bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3],
                                        ])
                                    };
                                    tc.push_str(&format!(" ({:.4},{:.4})", rd(base), rd(base + 4)));
                                } else {
                                    tc.push_str(" (oob)");
                                }
                            }
                            log!("  DRAW-TC vbo={} off={} stride={} tc:{}", vbo, off, stride, tc);
                        } else {
                            log!("  DRAW-TC vbo={} not in cache", vbo);
                        }
                    }
                }
            }
        }
        let disabled_arrays = guard_client_vertex_arrays(gles, mem, shadow);
        let fog_state_backup = clamp_fog_state_values(gles, shadow);
        if crate::env_flag_cached!("TOUCHHLE_POTATO_NATIVE_GLES2_PC_STATE") {
            static SEEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
            if !SEEN.swap(true, std::sync::atomic::Ordering::Relaxed) {
                log!(
                    "TOUCHHLE_POTATO_NATIVE_GLES2_PC_STATE=1: disabling strict native GLES2 scissor/depth/cull state before Potato draws [this log will only be shown once]"
                );
            }

            // PC desktop GL path is more forgiving about stale/clipping state.
            // Adreno native GLES2 can happily draw only a tiny clipped piece.
            gles.Disable(0x0c11); // GL_SCISSOR_TEST
            gles.Disable(0x0b71); // GL_DEPTH_TEST
            gles.Disable(0x0b44); // GL_CULL_FACE
        }

        crate::gles::perf_overlay::record_draw();
        gles.DrawArrays(mode, first, count);
        restore_fog_state_values(gles, fog_state_backup);
        for index in disabled_arrays {
            gles.EnableVertexAttribArray(index);
        }
    })
}

/// One-shot state dump at the first larger indexed draw, gated by
/// `TOUCHHLE_DEBUG_ES2_DRAW`. Helps diagnose "render loop alive but
/// renderbuffer stays black" situations.
unsafe fn log_es2_draw_state_once(gles: &mut dyn GLES, shadow: &GLShadowState, mem: &Mem) {
    if !crate::env_flag_cached!("TOUCHHLE_DEBUG_ES2_DRAW") {
        return;
    }
    static SEEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if SEEN.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let mut fbo: GLint = 0;
    gles.GetIntegerv(0x8CA6 /* GL_FRAMEBUFFER_BINDING */, &mut fbo);
    let mut program: GLint = 0;
    gles.GetIntegerv(0x8B8D /* GL_CURRENT_PROGRAM */, &mut program);
    let mut tex: GLint = 0;
    gles.GetIntegerv(0x8069 /* GL_TEXTURE_BINDING_2D */, &mut tex);
    let mut arr_buf: GLint = 0;
    gles.GetIntegerv(0x8894 /* GL_ARRAY_BUFFER_BINDING */, &mut arr_buf);
    let mut elem_buf: GLint = 0;
    gles.GetIntegerv(0x8895 /* GL_ELEMENT_ARRAY_BUFFER_BINDING */, &mut elem_buf);
    let mut viewport = [0 as GLint; 4];
    gles.GetIntegerv(0x0BA2 /* GL_VIEWPORT */, viewport.as_mut_ptr());
    let mut rb: GLint = 0;
    gles.GetIntegerv(0x8CA7 /* GL_RENDERBUFFER_BINDING */, &mut rb);
    let status = gles.CheckFramebufferStatus(0x8D40 /* GL_FRAMEBUFFER */);
    // Drain any error the queries raised so the guest doesn't inherit it.
    let mut err = gles.GetError();
    let mut errs = Vec::new();
    while err != 0 && errs.len() < 8 {
        errs.push(err);
        err = gles.GetError();
    }
    let mut color_mask = [0u8; 4];
    gles.GetBooleanv(0x0C23 /* GL_COLOR_WRITEMASK */, color_mask.as_mut_ptr());
    let states = [0x0BE2, 0x0B71, 0x0B44, 0x0C11, 0x0B90]
        .map(|cap| gles.IsEnabled(cap) != 0);
    let mut attribs = Vec::new();
    for name in ["a_position", "a_texCoord", "a_color"] {
        let name_c = std::ffi::CString::new(name).unwrap();
        let loc = gles.GetAttribLocation(program as GLuint, name_c.as_ptr());
        if loc < 0 { continue; }
        let i = loc as GLuint;
        let mut enabled = 0;
        let mut size = 0;
        let mut type_ = 0;
        let mut stride = 0;
        let mut buffer = 0;
        let mut ptr: *mut GLvoid = std::ptr::null_mut();
        gles.GetVertexAttribiv(i, 0x8622, &mut enabled);
        gles.GetVertexAttribiv(i, 0x8623, &mut size);
        gles.GetVertexAttribiv(i, 0x8625, &mut type_);
        gles.GetVertexAttribiv(i, 0x8624, &mut stride);
        gles.GetVertexAttribiv(i, 0x889F, &mut buffer);
        gles.GetVertexAttribPointerv(i, 0x8645, &mut ptr);
        let first = if buffer == 0 && type_ as u32 == 0x1406 && !ptr.is_null() && mem.is_host_ptr_in_guest_mem(ptr) {
            Some(std::slice::from_raw_parts(ptr.cast::<f32>(), (size as usize).min(4)).to_vec())
        } else { None };
        attribs.push((name, loc, enabled, size, type_, stride, buffer, ptr as usize, first));
    }
    let mut active_count = 0;
    gles.GetProgramiv(program as GLuint, 0x8B89 /* GL_ACTIVE_ATTRIBUTES */, &mut active_count);
    let mut active_attribs = Vec::new();
    for index in 0..active_count.clamp(0, 32) as GLuint {
        let mut name = [0_u8; 128];
        let mut length: GLsizei = 0;
        let mut size: GLint = 0;
        let mut type_: GLenum = 0;
        gles.GetActiveAttrib(
            program as GLuint,
            index,
            name.len() as GLsizei,
            &mut length,
            &mut size,
            &mut type_,
            name.as_mut_ptr().cast(),
        );
        let used = length.clamp(0, name.len() as GLsizei) as usize;
        active_attribs.push((String::from_utf8_lossy(&name[..used]).into_owned(), size, type_));
    }
    let mut enabled_attribs = Vec::new();
    let mut max_attribs: GLint = 0;
    gles.GetIntegerv(0x8869 /* GL_MAX_VERTEX_ATTRIBS */, &mut max_attribs);
    for index in 0..max_attribs.clamp(0, 32) as GLuint {
        let mut enabled = 0;
        gles.GetVertexAttribiv(index, 0x8622 /* GL_VERTEX_ATTRIB_ARRAY_ENABLED */, &mut enabled);
        if enabled == 0 { continue; }
        let mut size = 0;
        let mut type_ = 0;
        let mut stride = 0;
        let mut buffer = 0;
        let mut ptr: *mut GLvoid = std::ptr::null_mut();
        gles.GetVertexAttribiv(index, 0x8623 /* GL_VERTEX_ATTRIB_ARRAY_SIZE */, &mut size);
        gles.GetVertexAttribiv(index, 0x8625 /* GL_VERTEX_ATTRIB_ARRAY_TYPE */, &mut type_);
        gles.GetVertexAttribiv(index, 0x8624 /* GL_VERTEX_ATTRIB_ARRAY_STRIDE */, &mut stride);
        gles.GetVertexAttribiv(index, 0x889F /* GL_VERTEX_ATTRIB_ARRAY_BUFFER_BINDING */, &mut buffer);
        gles.GetVertexAttribPointerv(index, 0x8645 /* GL_VERTEX_ATTRIB_ARRAY_POINTER */, &mut ptr);
        let first = if buffer == 0 && type_ as u32 == 0x1406 && !ptr.is_null() && mem.is_host_ptr_in_guest_mem(ptr) {
            Some(std::slice::from_raw_parts(ptr.cast::<f32>(), (size as usize).min(4)).to_vec())
        } else { None };
        enabled_attribs.push((index, size, type_, stride, buffer, ptr as usize, first));
    }
    let mut active_uniform_count: GLint = 0;
    gles.GetProgramiv(program as GLuint, 0x8B86 /* GL_ACTIVE_UNIFORMS */, &mut active_uniform_count);
    let mut active_uniforms = Vec::new();
    for index in 0..active_uniform_count.clamp(0, 32) as GLuint {
        let mut name = [0_u8; 128];
        let mut length: GLsizei = 0;
        let mut size: GLint = 0;
        let mut type_: GLenum = 0;
        gles.GetActiveUniform(
            program as GLuint,
            index,
            name.len() as GLsizei,
            &mut length,
            &mut size,
            &mut type_,
            name.as_mut_ptr().cast(),
        );
        let used = length.clamp(0, name.len() as GLsizei) as usize;
        let uniform_name = String::from_utf8_lossy(&name[..used]).into_owned();
        let c_name = std::ffi::CString::new(uniform_name.as_str()).unwrap();
        let location = gles.GetUniformLocation(program as GLuint, c_name.as_ptr());
        let value = if location < 0 {
            "unused".to_string()
        } else if type_ == 0x8B5C {
            let mut value = [0.0_f32; 16];
            gles.GetUniformfv(program as GLuint, location, value.as_mut_ptr());
            format!("{:?}", value)
        } else if matches!(type_, 0x1406 | 0x8B50 | 0x8B51 | 0x8B52) {
            let mut value = [0.0_f32; 4];
            gles.GetUniformfv(program as GLuint, location, value.as_mut_ptr());
            let components = match type_ {
                0x8B50 => 2,
                0x8B51 => 3,
                0x8B52 => 4,
                _ => 1,
            };
            format!("{:?}", &value[..components])
        } else {
            "non-float uniform".to_string()
        };
        active_uniforms.push((uniform_name, size, type_, location, value));
    }
    let mut err = gles.GetError();
    while err != 0 && errs.len() < 16 {
        errs.push(err);
        err = gles.GetError();
    }
    log!(
        "ES2 draw state: fbo={} status={:#x} program={} texture={} \
         array_buf={} elem_buf={} renderbuffer={} viewport={:?} color_mask={:?} states={:?} attribs={:?} \
         active_count={} active_attribs={:?} enabled_attribs={:?} active_uniform_count={} active_uniforms={:?} generic_attribs_used={} err={:?}",
        fbo,
        status,
        program,
        tex,
        arr_buf,
        elem_buf,
        rb,
        viewport,
        color_mask,
        states,
        attribs,
        active_count,
        active_attribs,
        enabled_attribs,
        active_uniform_count,
        active_uniforms,
        shadow.generic_attribs_used,
        errs,
    );
}

fn glDrawElements(
    env: &mut Environment,
    mode: GLenum,
    count: GLsizei,
    type_: GLenum,
    indices: ConstVoidPtr,
) {
    {
        use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
        static SEEN: AtomicBool = AtomicBool::new(false);
        if !SEEN.swap(true, Ordering::Relaxed) {
            log!(
                "First glDrawElements(mode=0x{:x}, count={}, type=0x{:x}) (app submitting first indexed draw) [this log will only be shown once]",
                mode, count, type_
            );
        }

        if trace_potatogold_render() {
            static COUNT: AtomicU32 = AtomicU32::new(0);
            static LARGE_COUNT: AtomicU32 = AtomicU32::new(0);
            let n = COUNT.fetch_add(1, Ordering::Relaxed);
            let log_large = n >= 160
                && count > 3
                && LARGE_COUNT.fetch_add(1, Ordering::Relaxed) < 256;
            if n < 160 || log_large {
                log!(
                    "[POTATO RENDER TRACE] glDrawElements #{} mode=0x{:x} count={} type=0x{:x} indices=0x{:x}",
                    n + 1,
                    mode,
                    count,
                    type_,
                    indices.to_bits()
                );
            }
        }
    }
    if !draw_mode_is_valid(mode) {
        warn_invalid_draw_mode("glDrawElements", mode);
        return;
    }
    with_ctx_mem_and_shadow(env, |gles, mem, shadow| unsafe {
        if count > 0 && indices.is_null() && !buffer_is_bound(gles, shadow, ELEMENT_ARRAY_BUFFER) {
            log_once!(
                "Skipping glDrawElements with positive count and null indices because no element buffer is bound"
            );
            return;
        }
        if count > 3 && crate::env_flag_cached!("TOUCHHLE_DEBUG_ES2_DRAW") {
            log!(
                "ES2 first larger indexed draw: mode=0x{:x} count={} type=0x{:x} indices=0x{:x}",
                mode,
                count,
                type_,
                indices.to_bits()
            );
            log_es2_draw_state_once(gles, shadow, mem);
        }
        let disabled_arrays = guard_client_vertex_arrays(gles, mem, shadow);
        let fog_state_backup = clamp_fog_state_values(gles, shadow);
        if crate::env_flag_cached!("TOUCHHLE_POTATO_NATIVE_GLES2_PC_STATE") {
            static SEEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
            if !SEEN.swap(true, std::sync::atomic::Ordering::Relaxed) {
                log!(
                    "TOUCHHLE_POTATO_NATIVE_GLES2_PC_STATE=1: disabling strict native GLES2 scissor/depth/cull state before Potato draws [this log will only be shown once]"
                );
            }

            // PC desktop GL path is more forgiving about stale/clipping state.
            // Adreno native GLES2 can happily draw only a tiny clipped piece.
            gles.Disable(0x0c11); // GL_SCISSOR_TEST
            gles.Disable(0x0b71); // GL_DEPTH_TEST
            gles.Disable(0x0b44); // GL_CULL_FACE
        }

        let indices =
            translate_pointer_or_offset_to_host(gles, mem, shadow, indices, ELEMENT_ARRAY_BUFFER);
        crate::gles::perf_overlay::record_draw();
        gles.DrawElements(mode, count, type_, indices);
        if count > 3 && crate::env_flag_cached!("TOUCHHLE_DEBUG_ES2_DRAW") {
            static FB_DUMPED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
            if !FB_DUMPED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                let mut vp = [0 as GLint; 4];
                gles.GetIntegerv(0x0BA2 /* GL_VIEWPORT */, vp.as_mut_ptr());
                if vp[2] > 0 && vp[3] > 0 {
                    let w = vp[2] as usize;
                    let h = vp[3] as usize;
                    let mut pix = vec![0u8; w * h * 4];
                    gles.ReadPixels(
                        vp[0],
                        vp[1],
                        vp[2],
                        vp[3],
                        0x1908, /* GL_RGBA */
                        0x1401, /* GL_UNSIGNED_BYTE */
                        pix.as_mut_ptr() as *mut _,
                    );
                    dump_rgb_ppm(&pix, w as u32, h as u32, 0x1908, 0x1401, "/tmp/a8run/fb_after_larger_draw.ppm");
                }
                let mut err = gles.GetError();
                let mut errs = Vec::new();
                while err != 0 && errs.len() < 4 {
                    errs.push(err);
                    err = gles.GetError();
                }
                log!("[ES2DIAG] post-draw ReadPixels errs={:?}", errs);
            }
        }
        restore_fog_state_values(gles, fog_state_backup);
        for index in disabled_arrays {
            gles.EnableVertexAttribArray(index);
        }
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
    if trace_potatogold_render() {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNT: AtomicU32 = AtomicU32::new(0);
        let n = COUNT.fetch_add(1, Ordering::Relaxed);
        if n < 80 {
            log!("[POTATO RENDER TRACE] glMatrixMode(0x{:x})", mode);
        }
    }

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
    if trace_potatogold_render() {
        log!(
            "[POTATO RENDER TRACE] glOrthof(left={}, right={}, bottom={}, top={}, near={}, far={})",
            left,
            right,
            bottom,
            top,
            near,
            far
        );
    }

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
    if trace_potatogold_render() {
        log!(
            "[POTATO RENDER TRACE] glOrthox(left={}, right={}, bottom={}, top={}, near={}, far={})",
            left,
            right,
            bottom,
            top,
            near,
            far
        );
    }

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
    if trace_potatogold_render() {
        log!(
            "[POTATO RENDER TRACE] glFrustumf(left={}, right={}, bottom={}, top={}, near={}, far={})",
            left,
            right,
            bottom,
            top,
            near,
            far
        );
    }

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
    if trace_potatogold_render() {
        log!(
            "[POTATO RENDER TRACE] glFrustumx(left={}, right={}, bottom={}, top={}, near={}, far={})",
            left,
            right,
            bottom,
            top,
            near,
            far
        );
    }

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
    with_ctx_mem_and_shadow(env, |gles, mem, shadow| {
        let n_usize: GuestUSize = n.try_into().unwrap();
        // Copy the names out with unaligned reads: the guest array needn't be
        // 4-byte aligned (Costume Quest passes one that isn't), and a slice
        // over it would be undefined behaviour.
        let deleted: Vec<GLuint> = (0..n_usize).map(|i| mem.read(textures + i)).collect();
        unsafe { gles.DeleteTextures(n, deleted.as_ptr()) };
        for texture in &deleted {
            shadow.forget_pvrtc_texture(*texture);
            crate::gles::perf_overlay::delete_gl_texture(
                shadow.resource_group_token,
                *texture,
            );
        }
    })
}
fn glActiveTexture(env: &mut Environment, texture: GLenum) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.ActiveTexture(texture) })
}
fn glIsTexture(env: &mut Environment, texture: GLuint) -> GLboolean {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.IsTexture(texture) })
}
fn glBindTexture(env: &mut Environment, target: GLenum, texture: GLuint) {
    if trace_potatogold_render() {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNT: AtomicU32 = AtomicU32::new(0);
        let n = COUNT.fetch_add(1, Ordering::Relaxed);
        if n < 80 {
            log!(
                "[POTATO RENDER TRACE] glBindTexture(target=0x{:x}, texture={})",
                target,
                texture
            );
        }
    }

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
        if !crate::env_flag_cached!("TOUCHHLE_ENABLE_TEXTURE_CROP_RECT") {
            return;
        }

        with_ctx_and_mem(env, |gles, mem| unsafe {
            let params_ptr = mem.ptr_at(params, 4);
            {
                use std::sync::atomic::{AtomicBool, Ordering};
                static SEEN: AtomicBool = AtomicBool::new(false);
                if !SEEN.swap(true, Ordering::Relaxed) {
                    let crop = from_raw_parts(params_ptr, 4);
                    log!(
                        "TOUCHHLE_ENABLE_TEXTURE_CROP_RECT=1: first glTexParameteriv(GL_TEXTURE_CROP_RECT_OES) = [{}, {}, {}, {}]",
                        crop[0],
                        crop[1],
                        crop[2],
                        crop[3]
                    );
                }
            }
            gles.TexParameteriv(target, pname, params_ptr)
        });
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
        if !crate::env_flag_cached!("TOUCHHLE_ENABLE_TEXTURE_CROP_RECT") {
            return;
        }

        with_ctx_and_mem(env, |gles, mem| unsafe {
            let params_ptr = mem.ptr_at(params, 4);
            {
                use std::sync::atomic::{AtomicBool, Ordering};
                static SEEN: AtomicBool = AtomicBool::new(false);
                if !SEEN.swap(true, Ordering::Relaxed) {
                    let crop = from_raw_parts(params_ptr, 4);
                    log!(
                        "TOUCHHLE_ENABLE_TEXTURE_CROP_RECT=1: first glTexParameterfv(GL_TEXTURE_CROP_RECT_OES) = [{}, {}, {}, {}]",
                        crop[0],
                        crop[1],
                        crop[2],
                        crop[3]
                    );
                }
            }
            gles.TexParameterfv(target, pname, params_ptr)
        });
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
        if !crate::env_flag_cached!("TOUCHHLE_ENABLE_TEXTURE_CROP_RECT") {
            return;
        }

        with_ctx_and_mem(env, |gles, mem| unsafe {
            let params_ptr = mem.ptr_at(params, 4);
            {
                use std::sync::atomic::{AtomicBool, Ordering};
                static SEEN: AtomicBool = AtomicBool::new(false);
                if !SEEN.swap(true, Ordering::Relaxed) {
                    let crop = from_raw_parts(params_ptr, 4);
                    log!(
                        "TOUCHHLE_ENABLE_TEXTURE_CROP_RECT=1: first glTexParameterxv(GL_TEXTURE_CROP_RECT_OES) = [{}, {}, {}, {}]",
                        crop[0],
                        crop[1],
                        crop[2],
                        crop[3]
                    );
                }
            }
            gles.TexParameterxv(target, pname, params_ptr)
        });
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
fn texture_level_bytes(width: GLsizei, height: GLsizei, format: GLenum, type_: GLenum) -> u64 {
    if width <= 0 || height <= 0 {
        return 0;
    }
    let bytes_per_pixel = match type_ {
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
    bytes_per_pixel.map_or(0, |bytes_per_pixel| {
        (width as u64)
            .saturating_mul(height as u64)
            .saturating_mul(bytes_per_pixel)
    })
}

fn copied_texture_level_bytes(width: GLsizei, height: GLsizei, internalformat: GLenum) -> u64 {
    texture_storage_level_bytes(width, height, internalformat)
}

fn texture_storage_bytes_per_pixel(internalformat: GLenum) -> Option<u64> {
    match internalformat {
        gles11::ALPHA | gles11::LUMINANCE | 0x8229 | 0x8055 | 0x8F94 => Some(1),
        gles11::LUMINANCE_ALPHA
        | 0x822A
        | 0x822B
        | 0x822D
        | 0x8056
        | 0x8057
        | 0x8D62
        | 0x8F95
        | 0x81A5 => Some(2),
        gles11::RGB | 0x8051 | 0x8C41 | 0x8F96 => Some(3),
        gles11::RGBA
        | 0x8052
        | 0x8058
        | 0x8059
        | 0x8C43
        | 0x8F97
        | 0x81A6
        | 0x81A7
        | 0x88F0
        | 0x8CAC
        | 0x822C
        | 0x822E
        | 0x822F => Some(4),
        0x8053 | 0x8054 | 0x881B => Some(6),
        0x8230 | 0x805A | 0x805B | 0x881A => Some(8),
        0x8815 => Some(12),
        0x8814 => Some(16),
        0x8CAD => Some(8),
        _ => None,
    }
}

fn texture_storage_level_bytes(width: GLsizei, height: GLsizei, internalformat: GLenum) -> u64 {
    if width <= 0 || height <= 0 {
        return 0;
    }
    let Some(bytes_per_pixel) = texture_storage_bytes_per_pixel(internalformat) else {
        return 0;
    };
    (width as u64)
        .saturating_mul(height as u64)
        .saturating_mul(bytes_per_pixel)
}

fn texture_storage_volume_bytes(
    width: GLsizei,
    height: GLsizei,
    depth: GLsizei,
    internalformat: GLenum,
) -> u64 {
    if depth <= 0 {
        return 0;
    }
    texture_storage_level_bytes(width, height, internalformat).saturating_mul(depth as u64)
}

fn compressed_texture_level_bytes(
    internalformat: GLenum,
    width: GLsizei,
    height: GLsizei,
    image_size: usize,
) -> u64 {
    if width <= 0 || height <= 0 {
        return 0;
    }
    if let Some(format) = crate::gles::util::PalettedTextureFormat::get_info(internalformat) {
        let size = texture_level_bytes(
            width,
            height,
            format.palette_entry_format,
            format.palette_entry_type,
        );
        if size > 0 {
            return size;
        }
    }
    image_size as u64
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
        use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
        static SEEN: AtomicBool = AtomicBool::new(false);
        if !SEEN.swap(true, Ordering::Relaxed) {
            log!(
                "First glTexImage2D({}x{}, internalformat=0x{:x}, format=0x{:x}, type=0x{:x}) (app uploading texture data) [this log will only be shown once]",
                width, height, internalformat as u32, format, type_
            );
        }

        if trace_potatogold_render() {
            static COUNT: AtomicU32 = AtomicU32::new(0);
            let n = COUNT.fetch_add(1, Ordering::Relaxed);
            if n < 80 {
                log!(
                    "[POTATO RENDER TRACE] glTexImage2D #{} target=0x{:x} level={} size={}x{} internal=0x{:x} format=0x{:x} type=0x{:x} pixels_null={}",
                    n + 1,
                    target,
                    level,
                    width,
                    height,
                    internalformat as u32,
                    format,
                    type_,
                    pixels.is_null()
                );
            }
        }
    }
    let guest_pixels = pixels;
    let fix_filter = env.options.fix_texture_min_filter && level == 0;
    with_ctx_mem_and_shadow(env, |gles, mem, shadow| unsafe {
        let bound_texture = current_bound_texture(gles, target);
        let storage_size = texture_level_bytes(width, height, format, type_);
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
        if let Some(texture) = bound_texture {
            shadow.forget_pvrtc_texture_level(target, texture, level);
            crate::gles::perf_overlay::record_gl_image_allocation(
                shadow.resource_group_token,
                target,
                texture,
                level,
                storage_size,
            );
        }
        if crate::env_flag_cached!("TOUCHHLE_DEBUG_ES2_DRAW") {
            static TEX_DUMP_COUNT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let n = TEX_DUMP_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if n < 8 && !pixels.is_null() && level == 0 {
                let tex_id = {
                    let mut t: GLint = 0;
                    gles.GetIntegerv(0x8069 /* GL_TEXTURE_BINDING_2D */, &mut t);
                    t
                };
                let bytes_pp: usize = match (format, type_) {
                    (0x1908, 0x1401) => 4, // RGBA UNSIGNED_BYTE
                    (0x1908, 0x8033) => 2, // RGBA UNSIGNED_SHORT_4_4_4_4
                    (0x1907, 0x8363) => 2, // RGB UNSIGNED_SHORT_5_6_5
                    _ => 0,
                };
                if bytes_pp > 0 {
                    let px_count = (width as usize) * (height as usize);
                    let byte_size = px_count * bytes_pp;
                    let buf: Vec<u8> = mem
                        .bytes_at(guest_pixels.cast::<u8>(), byte_size as u32)
                        .to_vec();
                    let path = format!(
                        "/tmp/a8run/tex{}_id{}_{}x{}.ppm", n, tex_id, width, height
                    );
                    dump_rgb_ppm(&buf, width as u32, height as u32, format, type_, &path);
                }
            }
        }
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
    if crate::env_flag_cached!("TOUCHHLE_TRACE_TEX") {
        log!(
            "TEX-DIAG glCompressedTexImage2D target={:#x} level={} internalformat={:#x} {}x{} border={} image_size={} data={:#x}",
            target, level, internalformat, width, height, border, image_size, data.to_bits()
        );
    }
    let fix_filter = env.options.fix_texture_min_filter && level == 0;
    with_ctx_mem_and_shadow(env, |gles, mem, shadow| unsafe {
        let bound_texture = current_bound_texture(gles, target);
        if let Some(texture) = bound_texture {
            shadow.forget_pvrtc_texture_level(target, texture, level);
        }
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

        let image_size_usize = match usize::try_from(image_size) {
            Ok(size) => size,
            Err(_) => return,
        };
        // A guest NULL data pointer is legal for (Compressed)TexImage2D: it
        // allocates texture storage without initialising it (the app fills it
        // later with CompressedTexSubImage2D). We MUST detect it on the GUEST
        // pointer: `mem.ptr_at(NULL, image_size)` returns the 4 KiB null-stub
        // page clamped to one page, and building an `image_size`-length slice
        // over that (e.g. 512 KiB for a 1024x1024 PVRTC level) and handing it
        // to the PVRTC decoder reads ~508 KiB off the end of the stub page —
        // a host access violation outside the guest region that the
        // lazy-commit handler can't satisfy, crashing the process. (Minecraft
        // Story Mode allocates its 1024x1024 PVRTC atlases exactly this way.)
        let data_was_null = data.is_null();
        let data: *const GLvoid = if data_was_null {
            std::ptr::null()
        } else {
            mem.ptr_at(data.cast::<u8>(), image_size_usize as GuestUSize)
                .cast()
        };
        let is_pvrtc = matches!(
            internalformat,
            gles11::COMPRESSED_RGBA_PVRTC_2BPPV1_IMG
                | gles11::COMPRESSED_RGBA_PVRTC_4BPPV1_IMG
                | gles11::COMPRESSED_RGB_PVRTC_2BPPV1_IMG
                | gles11::COMPRESSED_RGB_PVRTC_4BPPV1_IMG
        );
        // PVRTC storage allocated with no data: desktop GL can't honour the
        // PVRTC internalformat, so our decode path uploads RGBA instead.
        // Mirror that for the allocate-only case by reserving RGBA storage of
        // the same dimensions (uninitialised), so the texture object is
        // complete and later CompressedTexSubImage2D updates have somewhere to
        // land. Without this the texture would be left in its default state.
        if is_pvrtc && data_was_null && width > 0 && height > 0 {
            gles.TexImage2D(
                target,
                level,
                gles11::RGBA as GLint,
                width,
                height,
                border,
                gles11::RGBA,
                gles11::UNSIGNED_BYTE,
                std::ptr::null(),
            );
            if border == 0 {
                if let Some(texture) = bound_texture {
                    crate::gles::perf_overlay::record_gl_image_allocation(
                        shadow.resource_group_token,
                        target,
                        texture,
                        level,
                        texture_level_bytes(width, height, gles11::RGBA, gles11::UNSIGNED_BYTE),
                    );
                }
            }

            // Record this as a PVRTC level so later glCompressedTexSubImage2D
            // updates (how MCSM actually fills these atlases) take the
            // software-decode-into-RGBA path instead of handing PVRTC bytes to
            // a desktop driver that rejects them (leaving the atlas empty).
            if border == 0 {
                if let Some(texture) = bound_texture {
                    shadow.record_pvrtc_texture_level(
                        target,
                        texture,
                        level,
                        width,
                        height,
                        internalformat,
                    );
                    // This full-image upload allocated the level's storage via
                    // glTexImage2D, so any pending unallocated marker from the
                    // glTexStorage2D PVRTC emulation is now satisfied.
                    shadow.mark_pvrtc_level_allocated(target, texture, level);
                }
            }
            if fix_filter {
                gles.TexParameteri(target, gles11::TEXTURE_MIN_FILTER, gles11::LINEAR as GLint);
            }
            return;
        }
        if is_pvrtc && !data.is_null() && image_size > 0 {
            let payload = std::slice::from_raw_parts(data.cast::<u8>(), image_size_usize);
            if crate::gles::try_decode_pvrtc(
                gles,
                target,
                level,
                internalformat,
                width,
                height,
                border,
                payload,
            ) {
                if border == 0
                    && width > 0
                    && height > 0
                    && crate::gles::util::pvrtc_payload_size(internalformat, width, height)
                        == Some(image_size_usize)
                {
                    if let Some(texture) = bound_texture {
                        shadow.record_pvrtc_texture_level(
                            target,
                            texture,
                            level,
                            width,
                            height,
                            internalformat,
                        );
                        // This full-image upload allocated the level's storage
                        // via glTexImage2D, so any pending unallocated marker
                        // from the glTexStorage2D PVRTC emulation is satisfied.
                        shadow.mark_pvrtc_level_allocated(target, texture, level);
                        crate::gles::perf_overlay::record_gl_image_allocation(
                            shadow.resource_group_token,
                            target,
                            texture,
                            level,
                            texture_level_bytes(width, height, gles11::RGBA, gles11::UNSIGNED_BYTE),
                        );
                    }
                }
                if fix_filter {
                    gles.TexParameteri(target, gles11::TEXTURE_MIN_FILTER, gles11::LINEAR as GLint);
                }
                return;
            }
        }
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
        if post_err == 0 {
            if let Some(texture) = bound_texture {
                crate::gles::perf_overlay::record_gl_image_allocation(
                    shadow.resource_group_token,
                    target,
                    texture,
                    level,
                    compressed_texture_level_bytes(
                        internalformat,
                        width,
                        height,
                        image_size_usize,
                    ),
                );
            }
        }
        if post_err != 0 {
            use std::sync::atomic::{AtomicU64, Ordering};
            // Pack (internalformat << 32) | post_err into a single 64-bit
            // slot per "ever seen" entry; cap at 8 distinct entries.
            const MAX_REPORTED: usize = 8;
            static REPORTED: [AtomicU64; MAX_REPORTED] = [
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
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
    with_ctx_mem_and_shadow(env, |gles, _mem, shadow| unsafe {
        let bound_texture = current_bound_texture(gles, target);
        gles.CopyTexImage2D(target, level, internalformat, x, y, width, height, border);
        if let Some(texture) = bound_texture {
            shadow.forget_pvrtc_texture_level(target, texture, level);
            crate::gles::perf_overlay::record_gl_image_allocation(
                shadow.resource_group_token,
                target,
                texture,
                level,
                copied_texture_level_bytes(width, height, internalformat),
            );
        }
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
    if target != gles11::TEXTURE_ENV && target != gles11::TEXTURE_FILTER_CONTROL_EXT {
        return;
    }
    if params.is_null() {
        return;
    }
    with_ctx_and_mem(env, |gles, mem| {
        let params = mem.ptr_at(params, 4);
        unsafe { gles.TexEnvfv(target, pname, params) }
    })
}
fn glTexEnvxv(env: &mut Environment, target: GLenum, pname: GLenum, params: ConstPtr<GLfixed>) {
    if target != gles11::TEXTURE_ENV || params.is_null() {
        return;
    }
    with_ctx_and_mem(env, |gles, mem| {
        let params = mem.ptr_at(params, 4);
        unsafe { gles.TexEnvxv(target, pname, params) }
    })
}
fn glTexEnviv(env: &mut Environment, target: GLenum, pname: GLenum, params: ConstPtr<GLint>) {
    if target != gles11::TEXTURE_ENV || params.is_null() {
        return;
    }
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
/// BioShock's framebuffer bind in `[EAGLView CreateFramebuffer:]` returns here.
const BIOSHOCK_CREATE_FRAMEBUFFER_RETURN: u32 = 0x187ff6c;
/// `FES2RHI::SetRenderTarget`'s tail call for the default framebuffer, and the
/// instruction expected at that address (`b _glBindFramebuffer`).
const BIOSHOCK_BIND_DEFAULT_FRAMEBUFFER: (u32, u32) = (0x17bf528, 0xea04c912);

/// BioShock renders to the default framebuffer unless it uses multisampling,
/// which we don't advertise. It finds that framebuffer by querying the binding,
/// but only after already having bound 0, which isn't a usable target on iOS.
/// Remember the framebuffer its EAGLView made and use that instead of 0.
fn bioshock_framebuffer(env: &mut Environment, framebuffer: GLuint) -> GLuint {
    if env.bundle.bundle_identifier() != "com.2k.bioshock" {
        return framebuffer;
    }
    let (address, instruction) = BIOSHOCK_BIND_DEFAULT_FRAMEBUFFER;
    if env.mem.read(ConstPtr::<u32>::from_bits(address)) != instruction {
        return framebuffer;
    }
    let state = &mut env.framework_state.opengles;
    if framebuffer == 0 {
        return state.bioshock_view_framebuffer;
    }
    if env.cpu.regs()[crate::cpu::Cpu::LR] == BIOSHOCK_CREATE_FRAMEBUFFER_RETURN {
        state.bioshock_view_framebuffer = framebuffer;
    }
    framebuffer
}

fn glBindFramebufferOES(env: &mut Environment, target: GLenum, framebuffer: GLuint) {
    let framebuffer = bioshock_framebuffer(env, framebuffer);
    let thread = env.current_thread;
    env.framework_state
        .opengles
        .set_bound_framebuffer(thread, framebuffer);
    // The viewport is global GL state, so a viewport the guest set while a
    // different framebuffer was bound may need different scale-hack treatment
    // now. Re-apply the guest's last request against the new binding. Without
    // the scale hack both treatments are identical, so there is nothing to do.
    let factor = env.options.scale_hack.get() as GLsizei;
    let viewport = if factor != 1 {
        env.framework_state.opengles.guest_viewport(thread)
    } else {
        None
    };
    let apply_scale_hack = env
        .framework_state
        .opengles
        .scale_hack_applies_to_viewport(thread);
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.BindFramebufferOES(target, framebuffer);
        if let Some((x, y, width, height)) = viewport {
            if apply_scale_hack {
                gles.Viewport(x * factor, y * factor, width * factor, height * factor)
            } else {
                gles.Viewport(x, y, width, height)
            }
        }
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
    if attachment == gles11::COLOR_ATTACHMENT0_OES {
        // Renderbuffers are enlarged by the scale hack, so a viewport aimed at
        // this framebuffer should be scaled as usual.
        let thread = env.current_thread;
        env.framework_state
            .opengles
            .set_colour_attachment_is_texture(thread, false);
    }
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
    if attachment == gles11::COLOR_ATTACHMENT0_OES {
        let thread = env.current_thread;
        env.framework_state
            .opengles
            .set_colour_attachment_is_texture(thread, texture != 0);
    }
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
/// Dump raw pixel data (RGB) to a PPM file for diagnosis. Supports the
/// formats Geometry Dash / cocos2d-x use (RGBA8, RGBA4444, RGB565).
pub fn dump_rgb_ppm(pix: &[u8], width: u32, height: u32, format: u32, type_: u32, path: &str) {
    let mut rgb = Vec::with_capacity((width * height * 3) as usize);
    match (format, type_) {
        (0x1908, 0x1401) => {
            for px in pix.chunks_exact(4) {
                rgb.extend_from_slice(&[px[0], px[1], px[2]]);
            }
        }
        (0x1908, 0x8033) => {
            for px in pix.chunks_exact(2) {
                let v = u16::from_be_bytes([px[0], px[1]]);
                let r = (((v >> 12) & 0xF) * 17) as u8;
                let g = (((v >> 8) & 0xF) * 17) as u8;
                let b = (((v >> 4) & 0xF) * 17) as u8;
                rgb.extend_from_slice(&[r, g, b]);
            }
        }
        (0x1907, 0x8363) => {
            for px in pix.chunks_exact(2) {
                let v = u16::from_le_bytes([px[0], px[1]]);
                let r = (((v >> 11) & 0x1F) * 255 / 31) as u8;
                let g = (((v >> 5) & 0x3F) * 255 / 63) as u8;
                let b = ((v & 0x1F) * 255 / 31) as u8;
                rgb.extend_from_slice(&[r, g, b]);
            }
        }
        _ => {
            log!("dump_rgb_ppm: unsupported format=0x{:x} type=0x{:x}", format, type_);
            return;
        }
    }
    let header = format!("P6\n{} {}\n255\n", width, height);
    let mut out = header.into_bytes();
    out.extend_from_slice(&rgb);
    match std::fs::write(path, &out) {
        Ok(()) => log!("Dumped {}x{} pixels to {}", width, height, path),
        Err(e) => log!("Failed to dump pixels to {}: {}", path, e),
    }
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
        ARRAY_BUFFER => gles11::ARRAY_BUFFER_BINDING,
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
    // Real drivers accept the three buffer-access modes; only the target and
    // the access value have to be legal. Returning nil for anything else (the
    // previous behaviour) made the caller write vertex/index data through a
    // NULL pointer, which is exactly the silent nil-page write flood seen in
    // Asphalt 8's drift events. Accept the full set instead of WRITE_ONLY.
    const READ_ONLY: GLenum = 0x88B8;
    const READ_WRITE: GLenum = 0x88BA;
    if !matches!(target, ARRAY_BUFFER | ELEMENT_ARRAY_BUFFER)
        || !matches!(access, WRITE_ONLY_OES | READ_ONLY | READ_WRITE)
    {
        log!(
            "Warning: glMapBufferOES(target={:#x}, access={:#x}) is not a supported \
             combination; returning NULL. If the app then writes through it, expect \
             nil-page writes.",
            target,
            access
        );
        return nil.cast();
    }
    // glMapBufferOES requires a current context (Apple: "An EAGLContext must
    // be current on the thread before any GL calls"). Without one the call
    // fails and glMapBufferOES returns NULL.
    let Some(current_ctx) = *env
        .framework_state
        .opengles
        .current_ctx_for_thread(env.current_thread)
    else {
        log!(
            "Warning: glMapBufferOES on thread {} with no current EAGLContext; \
             returning NULL.",
            env.current_thread
        );
        return nil.cast();
    };
    let buffer_object_name = _get_currently_bound_buffer_object_name(env, target);
    let buffer_size = match usize::try_from(_get_buffer_size(env, target)) {
        Ok(size) if size > 0 => size,
        _ => {
            // GL_OES_mapbuffer: mapping succeeds only if the buffer has a
            // data store; a buffer created with size 0 has none.
            log!(
                "Warning: glMapBufferOES(target={:#x}, access={:#x}) on buffer {} whose \
                 data store is empty or unreadable; returning NULL.",
                target,
                access,
                buffer_object_name
            );
            return nil.cast();
        }
    };
    // The mapping the guest sees is a mirror in guest memory, uploaded to the
    // buffer object by glUnmapBufferOES. The driver's own mapping is never
    // held across a guest call, because the driver is free to invalidate it
    // whenever the buffer is re-specified, re-mapped or deleted, and writing
    // through the stale pointer afterwards corrupted the heap and killed the
    // emulator with a SIGSEGV at a garbage address (Republique, Unity 4.5).
    let guest_buffer: MutVoidPtr = env.mem.alloc(buffer_size as GuestUSize).cast();
    // Mirror the buffer's current contents only when the app is allowed to
    // read them. With GL_WRITE_ONLY_OES the contents are undefined until the
    // app writes them, so the read would be pure overhead. Either way the
    // map/copy/unmap happens here, with no guest code in between, so the
    // driver pointer cannot outlive this function.
    if access != WRITE_ONLY_OES {
        let host_buffer = with_ctx_and_mem(env, |gles, _mem| unsafe {
            gles.MapBufferOES(target, access)
        });
        if !host_buffer.is_null() {
            unsafe {
                env.mem
                    .bytes_at_mut(guest_buffer.cast(), buffer_size as GuestUSize)
                    .copy_from_slice(from_raw_parts(host_buffer as *const u8, buffer_size));
            }
            with_ctx_and_mem(env, |gles, _mem| unsafe {
                gles.UnmapBufferOES(target);
            });
        }
    }
    let host_object = env.objc.borrow_mut::<EAGLContextHostObject>(current_ctx);
    // Per the GL_OES_mapbuffer spec, calling glMapBufferOES while a buffer is
    // already mapped is a GL_INVALID_OPERATION and returns NULL. Some apps
    // (e.g. ZAS) accidentally re-map without unmapping first; the previous
    // guest mirror is still owned by the guest, so we drop it and install the
    // new one so the app can continue uploading data instead of crashing.
    if let Some((stale_guest_buffer, _stale_host_ptr, _stale_size)) = host_object
        .mapped_buffers
        .remove(&(target, buffer_object_name))
    {
        log!(
            "Warning: glMapBufferOES called on buffer {} that was already mapped; \
             discarding the previous mapping (the previous pointer is no longer valid).",
            buffer_object_name
        );
        env.mem.free(stale_guest_buffer);
    }
    host_object
        .mapped_buffers
        .insert((target, buffer_object_name), (guest_buffer, None, buffer_size));
    guest_buffer
}
fn unmap_buffer(env: &mut Environment, target: GLenum, oes: bool) -> GLboolean {
    let buffer_object_name = _get_currently_bound_buffer_object_name(env, target);
    let Some(current_ctx) = env
        .framework_state
        .opengles
        .current_ctx_for_thread(env.current_thread)
        .as_ref()
        .copied()
    else {
        return gles11::FALSE;
    };
    let mapping = env
        .objc
        .borrow_mut::<EAGLContextHostObject>(current_ctx)
        .mapped_buffers
        .remove(&(target, buffer_object_name));
    let Some((guest_buffer, host_ptr, buffer_size)) = mapping else {
        // An unbalanced unmap (no guest mapping recorded for this buffer):
        // every driver-side mapping is owned by a recorded guest mapping, so
        // the buffer cannot be driver-mapped here. Forwarding the call would
        // only raise a spurious GL_INVALID_OPERATION (seen with Asphalt 8's
        // Jet engine, which unmaps buffers it failed to map). Treat it as a
        // no-op reporting success, matching Apple's lenient behaviour.
        log_dbg!(
            "glUnmapBuffer{} on buffer {} with no matching mapping; ignoring",
            if oes { "OES" } else { "" },
            buffer_object_name
        );
        return gles11::TRUE;
    };
    // Upload the guest's mirror to the buffer object. A mapping recorded by
    // glMapBufferRange still owns a real driver mapping, so it is filled and
    // then unmapped; a mapping recorded by glMapBufferOES has no driver
    // pointer (see the comment there) and is uploaded with glBufferSubData.
    // Either way the guest never gets to see, or write through, a driver
    // pointer that may have been invalidated in the meantime.
    match host_ptr {
        Some(host_ptr) => unsafe {
            host_ptr.copy_from(
                env.mem
                    .bytes_at(guest_buffer.cast(), buffer_size as GuestUSize)
                    .as_ptr() as *mut GLvoid,
                buffer_size,
            );
        },
        None => {
            with_ctx_and_mem(env, |gles, mem| unsafe {
                let data = mem
                    .bytes_at(guest_buffer.cast(), buffer_size as GuestUSize)
                    .as_ptr() as *const GLvoid;
                gles.BufferSubData(target, 0, buffer_size as HostGLsizeiptr, data);
            });
        }
    }
    // DIAG: cache the unmapped buffer content so TOUCHHLE_TRACE_DRAW can read
    // texcoords at draw time (the game streams its vertex data via map/unmap,
    // not glBufferData, so it was otherwise invisible to the cache).
    if crate::env_flag_cached!("TOUCHHLE_TRACE_DRAW") && target == ARRAY_BUFFER {
        let bytes = env
            .mem
            .bytes_at(guest_buffer.cast(), buffer_size as GuestUSize)
            .to_vec();
        if bytes.len() <= 4_194_304 {
            diag_buffer_cache()
                .lock()
                .unwrap()
                .insert(buffer_object_name, bytes);
        }
    }
    env.mem.free(guest_buffer);
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        let result = if host_ptr.is_none() {
            // The data was already uploaded with glBufferSubData above and no
            // driver mapping is open, so there is nothing left to unmap.
            gles11::TRUE
        } else if oes {
            gles.UnmapBufferOES(target)
        } else {
            gles.UnmapBuffer(target)
        };
        // Strict drivers (e.g. Qualcomm Adreno) can raise GL_INVALID_OPERATION
        // here even for mappings we believe are balanced, poisoning the error
        // queue for the app's own glGetError() polling. Apple's unmap never
        // surfaces such phantom errors, so purge whatever this call raised and
        // keep reporting success (same lenient philosophy as the unbalanced
        // unmap path above).
        let raised = gles.GetError();
        if raised != 0 {
            log_dbg!(
                "glUnmapBuffer{}: driver raised error {:#x} on unmap of target {:#x}; purging (treated as benign)",
                if oes { "OES" } else { "" },
                raised,
                target
            );
        }
        result
    })
}

fn glUnmapBufferOES(env: &mut Environment, target: GLenum) -> GLboolean {
    unmap_buffer(env, target, true)
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
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        let shader = gles.CreateShader(type_);
        if shader != 0 {
            record_shader_type(shader, type_);
        }
        shader
    })
}
fn glBindAttribLocation(
    env: &mut Environment,
    program: GLuint,
    index: GLuint,
    name: ConstPtr<GLubyte>,
) {
    with_ctx_mem_and_shadow(env, |gles, mem, shadow| unsafe {
        let cstr = read_guest_cstring(mem, name);
        let name_str = String::from_utf8_lossy(cstr.as_bytes()).into_owned();
        shadow
            .guest_bound_attribs
            .entry(program)
            .or_default()
            .insert(name_str);
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
        let loc = gles.GetUniformLocation(program, cstr.as_ptr());
        if crate::env_flag_cached!("TOUCHHLE_DEBUG_ES2_DRAW") {
            static SEEN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let n = SEEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if n < 64 {
                log!(
                    "[ES2DIAG] glGetUniformLocation(program={}, \"{}\") = {}",
                    program,  String::from_utf8_lossy(cstr.as_bytes()).to_string(), loc
                );
            }
        }
        loc
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
        if crate::env_flag_cached!("TOUCHHLE_DEBUG_ES2_DRAW") && location >= 0 {
            static SEEN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let seen = SEEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if seen < 8 {
                let mut m = [0.0_f32; 16];
                std::ptr::copy_nonoverlapping(ptr, m.as_mut_ptr(), 16);
                log!(
                    "[ES2DIAG] glUniformMatrix4fv(loc={}, count={}, transpose={}, m0={:?})",
                    location, count, transpose, m
                );
            }
        }
        gles.UniformMatrix4fv(location, count, transpose, ptr);
    });
}
fn glUseProgram(env: &mut Environment, program: GLuint) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.UseProgram(program) });
}
fn glDeleteProgram(env: &mut Environment, program: GLuint) {
    with_ctx_mem_and_shadow(env, |gles, _mem, shadow| unsafe {
        shadow.guest_bound_attribs.remove(&program);
        record_program_deleted(program);
        gles.DeleteProgram(program)
    });
}
fn glDeleteShader(env: &mut Environment, shader: GLuint) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        record_shader_deleted(shader);
        gles.DeleteShader(shader)
    });
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
        gles.AttachShader(program, shader);
        record_shader_attach(program, shader);
    });
}
fn glDetachShader(env: &mut Environment, program: GLuint, shader: GLuint) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.DetachShader(program, shader);
        record_shader_detach(program, shader);
    });
}
/// New locations for [crate::options::Options::reverse_attrib_locations].
/// `attribs` holds each active attribute's location as the driver chose it
/// and the number of locations it takes. The result gives the attributes the
/// locations 0 to N-1 in the reverse order, in the same order as `attribs`.
fn reversed_attrib_locations(attribs: &[(GLint, GLint)]) -> Vec<GLint> {
    let mut order: Vec<usize> = (0..attribs.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(attribs[i].0));
    let mut locations = vec![0; attribs.len()];
    let mut next = 0;
    for i in order {
        locations[i] = next;
        next += attribs[i].1;
    }
    locations
}

/// Relink `program` with [reversed_attrib_locations]. Only called when the
/// app bound no attribute locations itself.
unsafe fn relink_with_reversed_attrib_locations(gles: &mut dyn GLES, program: GLuint) {
    let mut ok: GLint = 0;
    gles.GetProgramiv(program, 0x8B82 /* GL_LINK_STATUS */, &mut ok);
    if ok == 0 {
        return;
    }
    let mut count: GLint = 0;
    gles.GetProgramiv(program, 0x8B89 /* GL_ACTIVE_ATTRIBUTES */, &mut count);
    let mut names = Vec::new();
    let mut attribs = Vec::new();
    for i in 0..count {
        let mut name = [0u8; 256];
        let mut len: GLsizei = 0;
        let mut size: GLint = 0;
        let mut type_: GLenum = 0;
        gles.GetActiveAttrib(
            program,
            i as GLuint,
            name.len() as GLsizei,
            &mut len,
            &mut size,
            &mut type_,
            name.as_mut_ptr().cast(),
        );
        let name = &name[..len.clamp(0, name.len() as GLsizei - 1) as usize];
        if name.is_empty() || name.starts_with(b"gl_") {
            continue;
        }
        let name = std::ffi::CString::new(name).unwrap();
        let location = gles.GetAttribLocation(program, name.as_ptr());
        if location < 0 {
            continue;
        }
        // Matrix attributes take one location per column.
        let slots = match type_ {
            0x8B5A /* GL_FLOAT_MAT2 */ => 2,
            0x8B5B /* GL_FLOAT_MAT3 */ => 3,
            0x8B5C /* GL_FLOAT_MAT4 */ => 4,
            _ => 1,
        } * size.max(1);
        names.push(name);
        attribs.push((location, slots));
    }
    let locations = reversed_attrib_locations(&attribs);
    if locations
        .iter()
        .zip(&attribs)
        .all(|(new, (old, _))| new == old)
    {
        return;
    }
    for (name, &location) in names.iter().zip(&locations) {
        gles.BindAttribLocation(program, location as GLuint, name.as_ptr());
    }
    gles.LinkProgram(program);
}
fn glLinkProgram(env: &mut Environment, program: GLuint) {
    let reverse_locations = env.options.reverse_attrib_locations;
    with_ctx_mem_and_shadow(env, |gles, _mem, shadow| unsafe {
        // If the app explicitly bound attribute locations for this program,
        // respect them: forcing canonical bindings here would override the
        // app's own vertex layout (on real hardware, app bindings made before
        // glLinkProgram win, so our injected bindings must not clobber them).
        let app_bound = shadow
            .guest_bound_attribs
            .get(&program)
            .map_or(false, |names| !names.is_empty());
        // The option needs the driver's own locations, see
        // `Options::reverse_attrib_locations`.
        if gles.is_es2() && !app_bound && !reverse_locations {
            for (index, names) in [
                (
                    0,
                    &[
                        "position",
                        "a_position",
                        "inPos",
                        "aPosition",
                        "inPosition",
                        "rm_Vertex",
                    ][..],
                ),
                (
                    1,
                    &["normal", "a_normal", "aNormal", "inNormal", "rm_Normal"][..],
                ),
                (
                    1,
                    &["a_color"][..],
                ),
                (
                    2,
                    &["color", "aColor", "inColor", "inVtxColor", "rm_Color", "a_texCoord"][..],
                ),
                (
                    3,
                    &[
                        "texCoord",
                        "texcoord",
                        "inUV0",
                        "aTexCoord",
                        "inTexCoord",
                        "rm_TexCoord0",
                    ][..],
                ),
                (
                    4,
                    &[
                        "texCoord1",
                        "a_texCoord1",
                        "aTexCoord1",
                        "inTexCoord1",
                        "rm_TexCoord1",
                    ][..],
                ),
                (
                    5,
                    &[
                        "tangent",
                        "a_tangent",
                        "aTangent",
                        "inTangent",
                        "rm_Tangent",
                    ][..],
                ),
                (
                    6,
                    &[
                        "binormal",
                        "a_binormal",
                        "aBinormal",
                        "inBinormal",
                        "rm_Binormal",
                    ][..],
                ),
            ] {
                for name in names {
                    let name = std::ffi::CString::new(*name).unwrap();
                    gles.BindAttribLocation(program, index, name.as_ptr());
                }
            }
        }
        if gles.is_es2() {
            // Strict linkers also reject uniform arrays whose element count
            // differs between stages (lenient PowerVR drivers accepted it);
            // Gangstar Rio trips this with its `light` array. Rewrite both
            // stages to the max size before anything else touches them.
            reconcile_uniform_array_sizes(gles, program);
            // Strict linkers also compare *struct member lists* of uniforms
            // shared between stages ("Field numbers of uniform 'X' differ…");
            // unify differing struct definitions to their member union.
            reconcile_uniform_struct_definitions(gles, program);
            // Strict linkers (ANGLE) reject programs whose fragment shader
            // declares varyings the vertex shader doesn't (legal-but-undefined
            // on real iPhone-era hardware). Patch the vertex shader first so
            // the link below succeeds; see `fix_fragment_only_varyings`.
            fix_fragment_only_varyings(gles, program);
        }
        gles.LinkProgram(program);
        if gles.is_es2() && !app_bound && reverse_locations {
            relink_with_reversed_attrib_locations(gles, program);
        }
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
            // Last resort: the pre-link pass may have missed a fragment-only
            // varying (e.g. exotic source layout). Use the names the driver
            // itself reported and retry the link once.
            if gles.is_es2() && inject_driver_reported_varyings(gles, program, s) {
                gles.LinkProgram(program);
                gles.GetProgramiv(program, 0x8B82 /* GL_LINK_STATUS */, &mut ok);
                if ok != 0 {
                    log!(
                        "Program {} linked successfully after injecting \
                         driver-reported fragment-only varyings",
                        program
                    );
                }
            }
            if ok == 0 {
                log!("Program {} link failed: {}", program, s);
                if s.contains("Field numbers of uniform") {
                    log_uniform_array_declarations(program);
                }
            }
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
/// OpenGL ES 2.0 `glGetShaderSource`. Returns the source string previously
/// uploaded via `glShaderSource` (which may have been translated to desktop
/// GLSL on the host side, but apps that round-trip via this entry point are
/// rare — the gles2_native backend returns the exact text the driver stored).
/// <https://registry.khronos.org/OpenGL-Refpages/es2.0/xhtml/glGetShaderSource.xml>
fn glGetShaderSource(
    env: &mut Environment,
    shader: GLuint,
    bufSize: GLsizei,
    length: MutPtr<GLsizei>,
    source: MutPtr<GLubyte>,
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
        gles.GetShaderSource(shader, bufSize, &mut written, buf.as_mut_ptr().cast());
        if !length.is_null() {
            mem.write(length, written);
        }
        if !source.is_null() && written >= 0 {
            // The driver writes a NUL terminator at `written` and may write up
            // to `bufSize` bytes including the terminator. Copy what was
            // produced (plus the terminator when there's room) into guest
            // memory.
            let count = written as usize + 1; // include trailing NUL
            let count = count.min(buf.len()).min(bufSize as usize);
            let dst = mem.ptr_at_mut(source, count.try_into().unwrap_or(0));
            std::ptr::copy_nonoverlapping(buf.as_ptr(), dst, count);
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

fn strip_captain_tomato_shader_precision(source: &str) -> String {
    fn replace_precision_tokens(line: &str) -> String {
        let mut out = String::with_capacity(line.len());
        let mut token = String::new();

        let flush = |token: &mut String, out: &mut String| {
            if token == "lowp" || token == "mediump" || token == "highp" {
                out.push_str("highp");
            } else {
                out.push_str(token);
            }
            token.clear();
        };

        for ch in line.chars() {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                token.push(ch);
            } else {
                if !token.is_empty() {
                    flush(&mut token, &mut out);
                }
                out.push(ch);
            }
        }

        if !token.is_empty() {
            flush(&mut token, &mut out);
        }

        out
    }

    let mut lines: Vec<String> = Vec::new();
    let mut has_float_precision = false;
    let mut has_int_precision = false;

    for line in source.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("precision ") && trimmed.ends_with(" float;") {
            has_float_precision = true;
            lines.push("precision highp float;".to_string());
            continue;
        }

        if trimmed.starts_with("precision ") && trimmed.ends_with(" int;") {
            has_int_precision = true;
            lines.push("precision highp int;".to_string());
            continue;
        }

        lines.push(replace_precision_tokens(line));
    }

    let mut insert_at = 0usize;
    while insert_at < lines.len()
        && (lines[insert_at].trim_start().starts_with("#version")
            || lines[insert_at].trim_start().starts_with("#extension"))
    {
        insert_at += 1;
    }

    if !has_int_precision {
        lines.insert(insert_at, "precision highp int;".to_string());
    }
    if !has_float_precision {
        lines.insert(insert_at, "precision highp float;".to_string());
    }

    lines.join("\n")
}

fn normalize_shader_preprocessor_whitespace(source: &str) -> String {
    let mut out = String::with_capacity(source.len() + 32);
    for (i, line) in source.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let line = line.trim_end_matches('\r');
        let trimmed_start = line.trim_start();
        let is_directive = trimmed_start.starts_with('#');
        if is_directive {
            let comment_at = match (line.find("//"), line.find("/*")) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (Some(a), None) => Some(a),
                (None, Some(b)) => Some(b),
                (None, None) => None,
            };
            if let Some(comment_at) = comment_at {
                let before = &line[..comment_at];
                let comment = &line[comment_at..];
                if !before.ends_with(' ') && !before.ends_with('\t') && !before.is_empty() {
                    out.push_str(before.trim_end());
                    out.push(' ');
                    out.push_str(comment);
                    continue;
                }
            }
        }
        out.push_str(line);
    }
    out
}

fn normalize_asphalt8_shader_source(source: &str) -> String {
    source
        .replace("#endif]", "#endif")
        .replace("||\r\n", "|| ")
        .replace("||\n", "|| ")
        .replace("|| \r\n", "|| ")
        .replace("|| \n", "|| ")
        .replace("&&\r\n", "&& ")
        .replace("&&\n", "&& ")
        .replace("&& \r\n", "&& ")
        .replace("&& \n", "&& ")
        .replace("vec3(1,1,1)", "vec3(1.0, 1.0, 1.0)")
        .replace("vec4(0.5, 0, 0, 0)", "vec4(0.5, 0.0, 0.0, 0.0)")
        .replace("vec4(0, 0.5, 0, 0)", "vec4(0.0, 0.5, 0.0, 0.0)")
        .replace("vec4(0, 0, 0.5, 0)", "vec4(0.0, 0.0, 0.5, 0.0)")
        .replace("vec4(0.5, 0.5, 0.5, 1)", "vec4(0.5, 0.5, 0.5, 1.0)")
}

/// Move every `#extension` directive to the top of the shader source (right
/// after the `#version` line, if any). GLSL ES requires extension directives
/// to appear before any non-preprocessor tokens; strict compilers (ANGLE,
/// Adreno, Mali) reject shaders that violate this — e.g. Gangstar's fragment
/// shaders, which the lenient PowerVR drivers of real iPhone-era hardware
/// accepted. Also normalizes whitespace between `#` and the directive name
/// (`#  extension` → `#extension`) so such lines are recognized. Lines that
/// merely contain the word "extension" as part of a longer identifier are
/// left alone.
fn hoist_shader_extension_directives(source: &str) -> String {
    let mut version_line: Option<&str> = None;
    let mut extension_lines: Vec<String> = Vec::new();
    let mut body_lines: Vec<&str> = Vec::new();
    for line in source.split('\n') {
        let trimmed = line.trim_start();
        // Recognize `#extension` even with whitespace after `#`.
        let normalized: Option<String> = trimmed.strip_prefix('#').and_then(|after_hash| {
            let dir = after_hash.trim_start();
            if dir.starts_with("extension") {
                let after_kw = &dir["extension".len()..];
                if after_kw.is_empty()
                    || !after_kw
                        .chars()
                        .next()
                        .map_or(false, |c| c.is_ascii_alphanumeric() || c == '_')
                {
                    Some(format!("#extension{}", after_kw))
                } else {
                    None
                }
            } else {
                None
            }
        });
        if let Some(norm) = normalized {
            extension_lines.push(norm);
            continue;
        }
        if version_line.is_none() && trimmed.starts_with("#version") {
            version_line = Some(line);
        } else if trimmed.starts_with("#extension") {
            extension_lines.push(line.to_string());
        } else {
            body_lines.push(line);
        }
    }
    if extension_lines.is_empty() {
        return source.to_string();
    }
    let mut out = String::with_capacity(source.len() + 32);
    if let Some(v) = version_line {
        out.push_str(v);
        out.push('\n');
    }
    for ext in &extension_lines {
        out.push_str(ext);
        out.push('\n');
    }
    for (i, line) in body_lines.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(line);
    }
    if source.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Strip `//` and `/* */` comments from GLSL source so declaration scanning
/// never sees commented-out code (Gameloft's shader generator emits the full
/// varying block in both stages but comments unused entries out — a
/// comment-blind parser would treat `/* varying float vAlpha; */` in the
/// vertex shader as a real declaration and skip the fix-up).
fn strip_glsl_comments(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut chars = source.chars().peekable();
    let mut in_line_comment = false;
    let mut in_block_comment = false;
    while let Some(c) = chars.next() {
        if in_line_comment {
            if c == '\n' {
                in_line_comment = false;
                out.push('\n');
            }
            continue;
        }
        if in_block_comment {
            if c == '*' && matches!(chars.peek(), Some('/')) {
                chars.next();
                in_block_comment = false;
                out.push(' ');
            }
            continue;
        }
        if c == '/' {
            if matches!(chars.peek(), Some('/')) {
                chars.next();
                in_line_comment = true;
                continue;
            }
            if matches!(chars.peek(), Some('*')) {
                chars.next();
                in_block_comment = true;
                out.push(' ');
                continue;
            }
        }
        out.push(c);
    }
    out
}

/// Parse top-level `varying` declarations from a GLSL ES 1.00 / desktop GLSL
/// 1.20 shader source, returning `(type, name)` pairs. Comments are stripped
/// first; the whole source is scanned token-wise, so declarations anywhere on
/// a line and several declarations per line (`varying vec2 a, b; varying
/// float c;`) are all found. Handles precision qualifiers and (by skipping
/// bracketed parts) array declarators.
fn parse_varying_declarations(source: &str) -> Vec<(String, String)> {
    let src = strip_glsl_comments(source);
    let bytes = src.as_bytes();
    let mut out = Vec::new();
    let mut search_start = 0usize;
    while let Some(rel) = src[search_start..].find("varying") {
        let start = search_start + rel;
        let end = start + "varying".len();
        let before_ok = start == 0 || {
            let b = bytes[start - 1];
            !(b.is_ascii_alphanumeric() || b == b'_')
        };
        let after_ok = end >= src.len() || {
            let b = bytes[end];
            !(b.is_ascii_alphanumeric() || b == b'_')
        };
        if !before_ok || !after_ok {
            search_start = end;
            continue;
        }
        // Scan tokens up to the first ';' (declarations after it will be
        // picked up by the next outer-loop iteration).
        let rest = &src[end..];
        let semi = rest.find(';').unwrap_or(rest.len());
        let body = &rest[..semi];
        let mut items: Vec<(String, bool)> = Vec::new(); // (token, comma-followed)
        let mut cur = String::new();
        let mut comma = false;
        let mut in_brackets = false;
        for ch in body.chars() {
            if ch == '[' {
                in_brackets = true;
            } else if ch == ']' {
                in_brackets = false;
            }
            if in_brackets {
                continue;
            }
            if ch.is_ascii_alphanumeric() || ch == '_' {
                cur.push(ch);
            } else {
                if !cur.is_empty() {
                    items.push((cur.clone(), comma));
                    cur.clear();
                    comma = false;
                }
                if ch == ',' {
                    comma = true;
                }
            }
        }
        if !cur.is_empty() {
            items.push((cur, comma));
        }
        if !items.is_empty() {
            let mut idx = 0usize;
            if matches!(items[0].0.as_str(), "highp" | "mediump" | "lowp") && items.len() >= 2 {
                idx = 1;
            }
            let ty = items[idx].0.clone();
            let mut k = idx + 1;
            while k < items.len() {
                let had_comma = items[k].1;
                if k > idx + 1 && !had_comma {
                    break;
                }
                let name = items[k].0.clone();
                if !name.is_empty() {
                    out.push((ty.clone(), name));
                }
                k += 1;
            }
        }
        search_start = end;
    }
    out
}

/// Guest-side bookkeeping of the ES 2.0 shader/program graph: which type each
/// shader object has, the (normalized) source last submitted for it, and
/// which shaders are attached to each program. Populated by the
/// `glCreateShader` / `glShaderSource` / `glAttachShader` / `glDetachShader` /
/// `glDeleteShader` / `glDeleteProgram` hooks below. `fix_fragment_only_varyings`
/// reads this instead of calling `glGetAttachedShaders` / `glGetShaderSource`
/// on the host backend — those entry points are optional and some backends
/// (notably `GLES2Native`, whose `GetAttachedShaders` default panics) do not
/// implement them.
#[derive(Default)]
struct ShaderBookkeeping {
    shader_types: HashMap<GLuint, GLuint>,
    shader_sources: HashMap<GLuint, String>,
    program_attachments: HashMap<GLuint, Vec<GLuint>>,
}
static SHADER_BOOKKEEPING: std::sync::Mutex<Option<ShaderBookkeeping>> =
    std::sync::Mutex::new(None);

fn with_shader_bookkeeping<R>(f: impl FnOnce(&mut ShaderBookkeeping) -> R) -> R {
    let mut guard = SHADER_BOOKKEEPING.lock().unwrap();
    f(guard.get_or_insert_with(ShaderBookkeeping::default))
}

fn record_shader_type(shader: GLuint, type_: GLuint) {
    with_shader_bookkeeping(|bk| {
        bk.shader_types.insert(shader, type_);
    });
}

fn record_shader_source(shader: GLuint, source: String) {
    with_shader_bookkeeping(|bk| {
        bk.shader_sources.insert(shader, source);
    });
}

fn record_shader_attach(program: GLuint, shader: GLuint) {
    with_shader_bookkeeping(|bk| {
        let list = bk.program_attachments.entry(program).or_default();
        if !list.contains(&shader) {
            list.push(shader);
        }
    });
}

fn record_shader_detach(program: GLuint, shader: GLuint) {
    with_shader_bookkeeping(|bk| {
        if let Some(list) = bk.program_attachments.get_mut(&program) {
            list.retain(|s| *s != shader);
        }
    });
}

fn record_shader_deleted(shader: GLuint) {
    with_shader_bookkeeping(|bk| {
        bk.shader_types.remove(&shader);
        bk.shader_sources.remove(&shader);
        for list in bk.program_attachments.values_mut() {
            list.retain(|s| *s != shader);
        }
    });
}

fn record_program_deleted(program: GLuint) {
    with_shader_bookkeeping(|bk| {
        bk.program_attachments.remove(&program);
    });
}

/// Some apps (e.g. Gangstar) declare `varying` variables in the fragment
/// shader that the vertex shader never declares. GLSL ES 1.00 tolerates this
/// (the varying gets an undefined value), and so did the PowerVR drivers of
/// real devices, but strict linkers — notably ANGLE's — fail the whole
/// program link with "FRAGMENT varying X does not match any VERTEX varying",
/// leaving the app drawing with a stale program and producing garbage
/// (magenta) geometry. Fix it generically: re-declare the fragment-only
/// varyings at the end of the vertex shader source (top-level declarations
/// are legal after `main()`), swap in a recompiled vertex shader, and let the
/// link proceed. Shader/program relationships come from the guest-side
/// [ShaderBookkeeping], never from optional backend entry points.
unsafe fn fix_fragment_only_varyings(gles: &mut dyn GLES, program: GLuint) {
    const VERTEX_SHADER: GLuint = 0x8B31;
    const FRAGMENT_SHADER: GLuint = 0x8B30;

    let Some((vertex_shader, vertex_src, fragment_src)) = with_shader_bookkeeping(|bk| {
        let attached = bk.program_attachments.get(&program)?.clone();
        if attached.len() < 2 {
            return None;
        }
        let mut vertex: Option<(GLuint, String)> = None;
        let mut fragment_src: Option<String> = None;
        for shader in attached {
            match bk.shader_types.get(&shader).copied() {
                Some(VERTEX_SHADER) => {
                    if let Some(src) = bk.shader_sources.get(&shader) {
                        vertex = Some((shader, src.clone()));
                    }
                }
                Some(FRAGMENT_SHADER) => {
                    if let Some(src) = bk.shader_sources.get(&shader) {
                        fragment_src = Some(src.clone());
                    }
                }
                _ => {}
            }
        }
        match (vertex, fragment_src) {
            (Some(vertex), Some(fragment_src)) => Some((vertex.0, vertex.1, fragment_src)),
            _ => None,
        }
    }) else {
        return;
    };
    let vertex_varyings = parse_varying_declarations(&vertex_src);
    let fragment_varyings = parse_varying_declarations(&fragment_src);
    if fragment_varyings.is_empty() {
        return;
    }
    let mut missing: Vec<(String, String)> = Vec::new();
    for (ty, name) in fragment_varyings {
        if vertex_varyings.iter().any(|(_, n)| n == &name) {
            continue;
        }
        if missing.iter().any(|(_, n)| n == &name) {
            continue;
        }
        missing.push((ty, name));
    }
    if missing.is_empty() {
        return;
    }
    if swap_in_patched_vertex_shader(gles, program, vertex_shader, &vertex_src, &missing) {
        log!(
            "Program {}: injected {} fragment-only varying declaration(s) ({}) \
             into the vertex shader so strict linkers accept the program",
            program,
            missing.len(),
            missing
                .iter()
                .map(|(_, n)| n.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
}

/// Append `missing` varying declarations to the program's current vertex
/// shader source, compile the result and swap it in (updating the guest-side
/// bookkeeping). Returns `false` (leaving everything as-is) if the patched
/// shader fails to compile.
unsafe fn swap_in_patched_vertex_shader(
    gles: &mut dyn GLES,
    program: GLuint,
    vertex_shader: GLuint,
    vertex_src: &str,
    missing: &[(String, String)],
) -> bool {
    const VERTEX_SHADER: GLuint = 0x8B31;
    const COMPILE_STATUS: GLenum = 0x8B81;

    let mut patched = vertex_src.to_string();
    if !patched.ends_with('\n') {
        patched.push('\n');
    }
    for (ty, name) in missing {
        patched.push_str(&format!("varying {} {};\n", ty, name));
    }
    let Ok(csrc) = std::ffi::CString::new(patched.clone()) else {
        return false;
    };
    let new_shader = gles.CreateShader(VERTEX_SHADER as GLenum);
    if new_shader == 0 {
        return false;
    }
    let ptr = csrc.as_ptr();
    gles.ShaderSource(new_shader, 1, &ptr, std::ptr::null());
    gles.CompileShader(new_shader);
    let mut ok: GLint = 0;
    gles.GetShaderiv(new_shader, COMPILE_STATUS, &mut ok);
    if ok == 0 {
        gles.DeleteShader(new_shader);
        return false;
    }
    gles.DetachShader(program, vertex_shader);
    gles.AttachShader(program, new_shader);
    // Keep the guest-side bookkeeping in sync (the Attach/Detach calls above
    // go straight to the backend, bypassing the glAttachShader hook).
    with_shader_bookkeeping(|bk| {
        if let Some(list) = bk.program_attachments.get_mut(&program) {
            if let Some(slot) = list.iter_mut().find(|s| **s == vertex_shader) {
                *slot = new_shader;
            } else {
                list.push(new_shader);
            }
        }
        bk.shader_types.insert(new_shader, VERTEX_SHADER);
        bk.shader_sources.insert(new_shader, patched);
        // The old vertex shader's type/source entries stay in the maps until
        // the guest deletes it — it may still be attached to other programs.
    });
    true
}

/// A `uniform … name[N]` declaration: the uniform's name, the declared
/// element count (None for unsized `name[]`), the raw text found between
/// the brackets (for diagnostics) and the byte span of the bracket
/// interior so the source can be rewritten in place.
struct UniformArrayDecl {
    name: String,
    size: Option<u32>,
    raw_size: String,
    /// Type token preceding the name (e.g. `vec4`, or a user struct name).
    ty: String,
    digits_span: (usize, usize),
}

/// Collect `#define NAME <integer>` (optionally wrapped in parentheses)
/// macro definitions so uniform array sizes written through macros can be
/// resolved to numbers.
fn collect_int_defines(source: &str) -> std::collections::HashMap<String, u32> {
    let mut out = std::collections::HashMap::new();
    for line in source.lines() {
        let trimmed = line.trim_start();
        let Some(rest) = trimmed.strip_prefix('#') else {
            continue;
        };
        let mut tokens = rest.split_whitespace();
        if tokens.next() != Some("define") {
            continue;
        }
        let Some(name) = tokens.next() else { continue };
        if !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            continue;
        }
        let Some(value) = tokens.next() else { continue };
        let value = value.trim_matches(|c| c == '(' || c == ')');
        if let Ok(n) = value.parse::<u32>() {
            out.insert(name.to_string(), n);
        }
    }
    out
}

/// Parse top-level `uniform` declarations that carry an `[N]` array size.
/// The input must already have comments stripped (see
/// [strip_glsl_comments]) so byte spans line up with the string being
/// rewritten. Integer macros from `defines` are resolved; `[]` (unsized)
/// declarations are reported with `size: None` so the caller can fill in
/// the other stage's size. Only the common single-declarator form is
/// handled; exotic layouts are skipped rather than misparsed.
fn parse_uniform_array_declarations(
    source: &str,
    defines: &std::collections::HashMap<String, u32>,
) -> Vec<UniformArrayDecl> {
    let bytes = source.as_bytes();
    let mut out = Vec::new();
    let mut search_start = 0usize;
    while let Some(rel) = source[search_start..].find("uniform") {
        let start = search_start + rel;
        let kw_end = start + "uniform".len();
        let before_ok = start == 0 || {
            let b = bytes[start - 1];
            !(b.is_ascii_alphanumeric() || b == b'_')
        };
        let after_ok = kw_end >= source.len() || {
            let b = bytes[kw_end];
            !(b.is_ascii_alphanumeric() || b == b'_')
        };
        if !before_ok || !after_ok {
            search_start = kw_end;
            continue;
        }
        let rest = &source[kw_end..];
        let semi = rest.find(';').unwrap_or(rest.len());
        let body = &rest[..semi];
        search_start = (kw_end + semi + 1).min(source.len());
        let Some(open_bracket) = body.find('[') else {
            continue;
        };
        let Some(close_rel) = body[open_bracket..].find(']') else {
            continue;
        };
        let inner = &body[open_bracket + 1..open_bracket + close_rel];
        let trimmed = inner.trim();
        // Sized literally, through an integer macro, or unsized (`[]`).
        let size = if trimmed.is_empty() {
            None
        } else if let Ok(n) = trimmed.parse::<u32>() {
            Some(n)
        } else if trimmed.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            defines.get(trimmed).copied()
        } else {
            None
        };
        // The declarator's name is the identifier token right before '['.
        let before = body[..open_bracket].trim_end();
        // take_while on the reversed iterator yields the trailing identifier
        // run right-to-left; .last() is therefore its leftmost character.
        let name_start = before
            .char_indices()
            .rev()
            .take_while(|&(_, c)| c.is_ascii_alphanumeric() || c == '_')
            .last()
            .map(|(i, _)| i)
            .unwrap_or(0);
        let name = &before[name_start..];
        if name.is_empty() || name.bytes().next().map_or(true, |b| b.is_ascii_digit()) {
            continue;
        }
        // The type token is the last whitespace-separated token before the
        // name (precision qualifiers sit further left).
        let ty = before[..name_start]
            .split_whitespace()
            .last()
            .unwrap_or("")
            .to_string();
        let digits_start = kw_end + open_bracket + 1;
        let digits_end = kw_end + open_bracket + close_rel;
        out.push(UniformArrayDecl {
            name: name.to_string(),
            size,
            raw_size: trimmed.to_string(),
            ty,
            digits_span: (digits_start, digits_end),
        });
    }
    out
}

/// Rewrite every declaration whose name appears in `fixes` (name → new
/// element count), applying replacements from the end of the source so
/// earlier byte spans stay valid.
fn rewrite_uniform_array_sizes(
    source: &str,
    decls: &[UniformArrayDecl],
    fixes: &[(String, u32)],
) -> String {
    let mut spans: Vec<((usize, usize), u32)> = decls
        .iter()
        .filter_map(|d| {
            fixes
                .iter()
                .find(|(n, _)| n == &d.name)
                .map(|(_, size)| (d.digits_span, *size))
        })
        .collect();
    spans.sort_by(|a, b| b.0.0.cmp(&a.0.0));
    let mut out = source.to_string();
    for ((s, e), size) in spans {
        out.replace_range(s..e, &size.to_string());
    }
    out
}

unsafe fn compile_shader_source(gles: &mut dyn GLES, type_: GLuint, src: &str) -> Option<GLuint> {
    const COMPILE_STATUS: GLenum = 0x8B81;
    let cs = std::ffi::CString::new(src.as_bytes().to_vec()).ok()?;
    let ptr = cs.as_ptr();
    let shader = gles.CreateShader(type_);
    if shader == 0 {
        return None;
    }
    gles.ShaderSource(shader, 1, &ptr, std::ptr::null());
    gles.CompileShader(shader);
    let mut ok: GLint = 0;
    gles.GetShaderiv(shader, COMPILE_STATUS, &mut ok);
    if ok == 0 {
        gles.DeleteShader(shader);
        None
    } else {
        Some(shader)
    }
}

/// Lenient iPhone-era drivers (PowerVR) accepted a uniform array declared
/// with *different* element counts in the vertex and fragment shaders;
/// strict linkers fail the whole program with "Field numbers of uniform
/// 'X' differ between VERTEX and FRAGMENT shaders" (Gangstar Rio e.g.
/// declares `light` with a per-stage element count). Mirror the lenient
/// hardware: before linking, rewrite both stages so every shared uniform
/// array uses the maximum of the two declared sizes.
unsafe fn reconcile_uniform_array_sizes(gles: &mut dyn GLES, program: GLuint) {
    const VERTEX_SHADER: GLuint = 0x8B31;
    const FRAGMENT_SHADER: GLuint = 0x8B30;

    let Some((vertex_shader, vertex_src, fragment_shader, fragment_src)) =
        with_shader_bookkeeping(|bk| {
            let attached = bk.program_attachments.get(&program)?.clone();
            let mut vertex: Option<(GLuint, String)> = None;
            let mut fragment: Option<(GLuint, String)> = None;
            for shader in attached {
                match bk.shader_types.get(&shader).copied() {
                    Some(VERTEX_SHADER) => {
                        if let Some(src) = bk.shader_sources.get(&shader) {
                            vertex = Some((shader, src.clone()));
                        }
                    }
                    Some(FRAGMENT_SHADER) => {
                        if let Some(src) = bk.shader_sources.get(&shader) {
                            fragment = Some((shader, src.clone()));
                        }
                    }
                    _ => {}
                }
            }
            match (vertex, fragment) {
                (Some(v), Some(f)) => Some((v.0, v.1, f.0, f.1)),
                _ => None,
            }
        })
    else {
        return;
    };

    // Comments are stripped so the parser's byte spans match the string
    // being rewritten; dropping them from the patched source is harmless.
    let vertex_stripped = strip_glsl_comments(&vertex_src);
    let fragment_stripped = strip_glsl_comments(&fragment_src);
    let vertex_defines = collect_int_defines(&vertex_stripped);
    let fragment_defines = collect_int_defines(&fragment_stripped);
    let vertex_uniforms = parse_uniform_array_declarations(&vertex_stripped, &vertex_defines);
    let fragment_uniforms =
        parse_uniform_array_declarations(&fragment_stripped, &fragment_defines);
    if vertex_uniforms.is_empty() || fragment_uniforms.is_empty() {
        return;
    }
    let mut fixes: Vec<(String, u32)> = Vec::new();
    for vd in &vertex_uniforms {
        if fixes.iter().any(|(n, _)| n == &vd.name) {
            continue;
        }
        if let Some(fd) = fragment_uniforms.iter().find(|fd| fd.name == vd.name) {
            match (vd.size, fd.size) {
                (Some(a), Some(b)) if a != b => fixes.push((vd.name.clone(), a.max(b))),
                // Unsized / unresolvable on one side: adopt the other's size.
                (Some(a), None) => fixes.push((vd.name.clone(), a)),
                (None, Some(b)) => fixes.push((vd.name.clone(), b)),
                _ => {}
            }
        }
    }
    if fixes.is_empty() {
        return;
    }

    let patched_vertex = rewrite_uniform_array_sizes(&vertex_stripped, &vertex_uniforms, &fixes);
    let patched_fragment =
        rewrite_uniform_array_sizes(&fragment_stripped, &fragment_uniforms, &fixes);
    if !swap_both_patched_shaders(
        gles,
        program,
        vertex_shader,
        fragment_shader,
        &patched_vertex,
        &patched_fragment,
    ) {
        return;
    }
    log!(
        "Program {}: reconciled {} uniform array size difference(s) ({}) \\\
         between vertex and fragment shaders so strict linkers accept the \\\
         program",
        program,
        fixes.len(),
        fixes
            .iter()
            .map(|(n, size)| format!("{}[{}]", n, size))
            .collect::<Vec<_>>()
            .join(", ")
    );
}

/// Compile `patched_vertex`/`patched_fragment` and replace the stage shaders
/// attached to `program` with the new objects, updating all bookkeeping.
/// Returns false (and leaves the program untouched) if either compilation
/// failed.
unsafe fn swap_both_patched_shaders(
    gles: &mut dyn GLES,
    program: GLuint,
    vertex_shader: GLuint,
    fragment_shader: GLuint,
    patched_vertex: &str,
    patched_fragment: &str,
) -> bool {
    const VERTEX_SHADER: GLuint = 0x8B31;
    const FRAGMENT_SHADER: GLuint = 0x8B30;

    let Some(new_vertex) = compile_shader_source(gles, VERTEX_SHADER, patched_vertex) else {
        return false;
    };
    let Some(new_fragment) = compile_shader_source(gles, FRAGMENT_SHADER, patched_fragment) else {
        gles.DeleteShader(new_vertex);
        return false;
    };
    gles.DetachShader(program, vertex_shader);
    gles.AttachShader(program, new_vertex);
    gles.DetachShader(program, fragment_shader);
    gles.AttachShader(program, new_fragment);
    with_shader_bookkeeping(|bk| {
        if let Some(list) = bk.program_attachments.get_mut(&program) {
            for (old, new) in [(vertex_shader, new_vertex), (fragment_shader, new_fragment)] {
                if let Some(slot) = list.iter_mut().find(|s| **s == old) {
                    *slot = new;
                } else {
                    list.push(new);
                }
            }
        }
        bk.shader_types.insert(new_vertex, VERTEX_SHADER);
        bk.shader_types.insert(new_fragment, FRAGMENT_SHADER);
        bk.shader_sources
            .insert(new_vertex, patched_vertex.to_string());
        bk.shader_sources
            .insert(new_fragment, patched_fragment.to_string());
    });
    true
}

/// Types that can never be user-defined structs.
const GLSL_BUILTIN_TYPES: &[&str] = &[
    "float",
    "int",
    "bool",
    "vec2",
    "vec3",
    "vec4",
    "ivec2",
    "ivec3",
    "ivec4",
    "bvec2",
    "bvec3",
    "bvec4",
    "mat2",
    "mat3",
    "mat4",
    "sampler2D",
    "samplerCube",
    "samplerExternalOES",
    "highp",
    "mediump",
    "lowp",
];

struct StructDefinition {
    name: String,
    /// Span of the body between `{` and `}` inclusive.
    body_span: (usize, usize),
    /// Members as (type text, name, full declaration text).
    members: Vec<(String, String, String)>,
}

/// Parse `struct NAME { … };` definitions out of GLSL source (comments must
/// already be stripped). Struct bodies cannot nest braces, so the first `}`
/// ends the body.
fn parse_struct_definitions(source: &str) -> Vec<StructDefinition> {
    let mut out = Vec::new();
    let bytes = source.as_bytes();
    for (idx, _) in source.match_indices("struct") {
        let prev_ok =
            idx == 0 || !(bytes[idx - 1].is_ascii_alphanumeric() || bytes[idx - 1] == b'_');
        let next = idx + "struct".len();
        let next_ok = next >= bytes.len()
            || !(bytes[next].is_ascii_alphanumeric() || bytes[next] == b'_');
        if !prev_ok || !next_ok {
            continue;
        }
        let rest = &source[next..];
        let Some(name_off) = rest.find(|c: char| c.is_ascii_alphanumeric() || c == '_') else {
            continue;
        };
        let name_end = rest[name_off..]
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .map(|e| name_off + e)
            .unwrap_or(rest.len());
        let name = &rest[name_off..name_end];
        if name.is_empty() {
            continue;
        }
        let Some(brace_off) = rest[name_end..].find('{') else {
            continue;
        };
        let brace = next + name_end + brace_off;
        let Some(close) = source[brace..].find('}') else {
            continue;
        };
        let body = &source[brace + 1..brace + close];
        let mut members = Vec::new();
        for member in body.split(';') {
            let m = member.trim();
            if m.is_empty() {
                continue;
            }
            let tokens: Vec<&str> = m.split_whitespace().collect();
            if tokens.len() < 2 {
                continue;
            }
            let member_name = tokens[tokens.len() - 1]
                .trim_end_matches(|c: char| c.is_ascii_digit() || c == '[' || c == ']')
                .to_string();
            let member_type = tokens[..tokens.len() - 1].join(" ");
            members.push((member_type, member_name, m.to_string()));
        }
        out.push(StructDefinition {
            name: name.to_string(),
            body_span: (brace, brace + close + 1),
            members,
        });
    }
    out
}

/// Build the union of two struct bodies: same order as `a`, with members
/// present only in `b` appended. Returns None if a member exists in both
/// with a different type (that is a genuine program error, not a linker
/// quirk).
fn merged_struct_body(a: &StructDefinition, b: &StructDefinition) -> Option<String> {
    let mut members: Vec<(String, String, String)> = a.members.clone();
    for (ty, name, raw) in &b.members {
        match members.iter().find(|(_, n, _)| n == name) {
            Some((existing_ty, _, _)) if existing_ty == ty => {}
            Some(_) => return None,
            None => members.push((ty.clone(), name.clone(), raw.clone())),
        }
    }
    Some(
        members
            .iter()
            .map(|(_, _, raw)| format!("{};", raw))
            .collect::<Vec<_>>()
            .join(" "),
    )
}

/// Some strict desktop GL linkers reject a program when the vertex and
/// fragment stages define the *same struct type with different member lists*
/// (even if each stage only touches its own subset):
/// "Field numbers of uniform 'X' differ between VERTEX and FRAGMENT shaders".
/// Detect shared struct types used by array uniforms in both stages, unify
/// their definitions to the member union, and swap in recompiled stages.
/// Returns true if the program's shaders were replaced.
unsafe fn reconcile_uniform_struct_definitions(gles: &mut dyn GLES, program: GLuint) -> bool {
    const VERTEX_SHADER: GLuint = 0x8B31;
    const FRAGMENT_SHADER: GLuint = 0x8B30;

    let Some((vertex_shader, fragment_shader, vertex_source, fragment_source)) =
        with_shader_bookkeeping(|bk| {
            let attached = bk.program_attachments.get(&program)?;
            let vertex_shader = attached
                .iter()
                .find(|shader| bk.shader_types.get(*shader) == Some(&VERTEX_SHADER))
                .copied()?;
            let fragment_shader = attached
                .iter()
                .find(|shader| bk.shader_types.get(*shader) == Some(&FRAGMENT_SHADER))
                .copied()?;
            let vertex_source = bk.shader_sources.get(&vertex_shader)?.clone();
            let fragment_source = bk.shader_sources.get(&fragment_shader)?.clone();
            Some((
                vertex_shader,
                fragment_shader,
                vertex_source,
                fragment_source,
            ))
        })
    else {
        return false;
    };
    let defines_v = collect_int_defines(&vertex_source);
    let defines_f = collect_int_defines(&fragment_source);
    let uniforms_v = parse_uniform_array_declarations(&vertex_source, &defines_v);
    let uniforms_f = parse_uniform_array_declarations(&fragment_source, &defines_f);
    let mut structs_v = parse_struct_definitions(&vertex_source);
    let mut structs_f = parse_struct_definitions(&fragment_source);
    // Struct types used by array uniforms in both stages.
    let mut shared_types: Vec<String> = Vec::new();
    for dv in &uniforms_v {
        if GLSL_BUILTIN_TYPES.contains(&dv.ty.as_str()) || dv.ty.is_empty() {
            continue;
        }
        if uniforms_f
            .iter()
            .any(|df| df.name == dv.name && df.ty == dv.ty)
        {
            if !shared_types.contains(&dv.ty) {
                shared_types.push(dv.ty.clone());
            }
        }
    }
    let mut patched_vertex = vertex_source.clone();
    let mut patched_fragment = fragment_source.clone();
    let mut changed = Vec::new();
    for ty in shared_types {
        let Some(sv) = structs_v.iter().find(|s| s.name == ty) else {
            continue;
        };
        let Some(sf) = structs_f.iter().find(|s| s.name == ty) else {
            continue;
        };
        // Only act when the member lists actually differ.
        let same = sv.members.len() == sf.members.len()
            && sv
                .members
                .iter()
                .zip(&sf.members)
                .all(|(a, b)| a.0 == b.0 && a.1 == b.1 && a.2 == b.2);
        if same {
            continue;
        }
        let Some(merged) = merged_struct_body(sv, sf) else {
            log!(
                "Program {}: struct {} has conflicting member types between \\\
                 vertex and fragment shaders; leaving as-is",
                program,
                ty
            );
            return false;
        };
        // body_span includes the braces, so re-wrap the merged members.
        // Spans shift as the source is edited, so re-parse after each edit.
        let merged_braced = format!("{{ {} }}", merged);
        let new_vertex = format!(
            "{}{}{}",
            &patched_vertex[..sv.body_span.0],
            merged_braced,
            &patched_vertex[sv.body_span.1..]
        );
        patched_vertex = new_vertex;
        let new_fragment = format!(
            "{}{}{}",
            &patched_fragment[..sf.body_span.0],
            merged_braced,
            &patched_fragment[sf.body_span.1..]
        );
        patched_fragment = new_fragment;
        changed.push(ty);
        // Spans moved: re-parse the patched sources for the next iteration.
        structs_v = parse_struct_definitions(&patched_vertex);
        structs_f = parse_struct_definitions(&patched_fragment);
    }
    if changed.is_empty() {
        return false;
    }
    if !swap_both_patched_shaders(
        gles,
        program,
        vertex_shader,
        fragment_shader,
        &patched_vertex,
        &patched_fragment,
    ) {
        return false;
    }
    log!(
        "Program {}: unified struct definition(s) ({}) between vertex and \\\
         fragment shaders so strict linkers accept the program",
        program,
        changed.join(", ")
    );
    true
}

/// Dump the `uniform …[N]` declarations parsed from each stage attached to
/// `program` (name, raw bracket text, resolved size). Diagnostic for
/// strict-linker uniform mismatches the pre-link reconciliation could not
/// fix; the next user log then shows exactly what the shaders declare.
fn log_uniform_array_declarations(program: GLuint) {
    const VERTEX_SHADER: GLuint = 0x8B31;
    const FRAGMENT_SHADER: GLuint = 0x8B30;
    let Some((vertex_src, fragment_src)) = with_shader_bookkeeping(|bk| {
        let attached = bk.program_attachments.get(&program)?.clone();
        let mut vertex: Option<String> = None;
        let mut fragment: Option<String> = None;
        for shader in attached {
            match bk.shader_types.get(&shader).copied() {
                Some(VERTEX_SHADER) => vertex = bk.shader_sources.get(&shader).cloned(),
                Some(FRAGMENT_SHADER) => fragment = bk.shader_sources.get(&shader).cloned(),
                _ => {}
            }
        }
        Some((vertex?, fragment?))
    }) else {
        return;
    };
    let fmt = |src: &str| -> String {
        let stripped = strip_glsl_comments(src);
        let defines = collect_int_defines(&stripped);
        let decls = parse_uniform_array_declarations(&stripped, &defines);
        if decls.is_empty() {
            return "<none>".to_string();
        }
        decls
            .iter()
            .map(|d| format!("{}[{}] as {:?}", d.name, d.raw_size, d.size))
            .collect::<Vec<_>>()
            .join(", ")
    };
    log!(
        "Program {} uniform array declarations - vertex: {}; fragment: {}",
        program,
        fmt(&vertex_src),
        fmt(&fragment_src)
    );
}

/// Last-resort fix-up: if a link still failed, parse varying names reported
/// by the driver (ANGLE: `FRAGMENT varying <name> does not match any VERTEX
/// varying`; Mesa: `fragment shader input '<name>' has no matching output in
/// the previous stage`), look their types up in the recorded fragment source
/// and inject them into the vertex shader. The caller re-links afterwards.
fn parse_driver_reported_varying_names(info_log: &str) -> Vec<String> {
    let mut names = Vec::new();
    for line in info_log.lines() {
        let marker = if line.contains("FRAGMENT varying")
            && line.contains("does not match any VERTEX varying")
        {
            "FRAGMENT varying"
        } else if line.contains("fragment shader input")
            && line.contains("has no matching output in the previous stage")
        {
            "fragment shader input"
        } else {
            continue;
        };
        let Some(index) = line.find(marker) else {
            continue;
        };
        let rest = line[index + marker.len()..].trim_start();
        let rest = rest.trim_start_matches(|c| matches!(c, '\'' | '"' | '`'));
        let name: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if !name.is_empty() && !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

unsafe fn inject_driver_reported_varyings(
    gles: &mut dyn GLES,
    program: GLuint,
    info_log: &str,
) -> bool {
    const VERTEX_SHADER: GLuint = 0x8B31;
    const FRAGMENT_SHADER: GLuint = 0x8B30;

    let names = parse_driver_reported_varying_names(info_log);
    if names.is_empty() {
        return false;
    }
    let Some((vertex_shader, vertex_src, missing)) = with_shader_bookkeeping(|bk| {
        let attached = bk.program_attachments.get(&program)?.clone();
        let mut vertex: Option<(GLuint, String)> = None;
        let mut fragment_src: Option<String> = None;
        for shader in attached {
            match bk.shader_types.get(&shader).copied() {
                Some(VERTEX_SHADER) => {
                    if let Some(src) = bk.shader_sources.get(&shader) {
                        vertex = Some((shader, src.clone()));
                    }
                }
                Some(FRAGMENT_SHADER) => {
                    if let Some(src) = bk.shader_sources.get(&shader) {
                        fragment_src = Some(src.clone());
                    }
                }
                _ => {}
            }
        }
        let (vertex_shader, vertex_src) = vertex?;
        let fragment_src = fragment_src?;
        let fs_decls = parse_varying_declarations(&fragment_src);
        let missing: Vec<(String, String)> = names
            .iter()
            .filter_map(|name| fs_decls.iter().find(|(_, n)| n == name).cloned())
            .collect();
        if missing.is_empty() {
            return None;
        }
        Some((vertex_shader, vertex_src, missing))
    }) else {
        return false;
    };
    swap_in_patched_vertex_shader(gles, program, vertex_shader, &vertex_src, &missing)
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
    // Concatenate first so preprocessor normalization can see directive
    // boundaries that guest code split across multiple source strings.
    let mut raw_source = Vec::<u8>::new();
    for i in 0..count {
        let str_ptr_ptr: ConstPtr<ConstPtr<GLubyte>> = string + (i as GuestUSize);
        let str_ptr: ConstPtr<GLubyte> = env.mem.read(str_ptr_ptr);
        if str_ptr.is_null() {
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
            // GLSL shader sources can legitimately exceed the default 64KB
            // `cstr_at` safety cap (e.g. Unreal Engine's generated shaders in
            // UDKGame). Using the default cap silently truncates the source,
            // which then fails to compile with errors like
            // "Unterminated #if/#ifdef/#ifndef". Allow up to 16 MB here.
            const MAX_SHADER_SRC_LEN: u32 = 16 * 1024 * 1024;
            env.mem
                .cstr_at_with_max_len(str_ptr, MAX_SHADER_SRC_LEN)
                .to_vec()
        };
        raw_source.extend_from_slice(&bytes_vec);
    }
    // Normalize shader text before sending it to the driver:
    // 1. Fix up preprocessor-directive whitespace (see doc comment on
    //    `normalize_shader_preprocessor_whitespace`) — real-world guest
    //    shaders (e.g. Gameloft's "9mm") glue same-line comments
    //    directly onto `#endif`/`#if`/`#elif` or use CRLF endings,
    //    which real PowerVR SGX drivers tolerated but modern GLSL
    //    compilers reject with "unexpected tokens following #endif".
    // 2. Normalize GLES precision qualifiers for all Cocos2D shaders.
    //    Fixes Mesa link failures like:
    //    uniform `CC_PMatrix` declared as type `f16mat4` and type `mat4`.
    let src = String::from_utf8_lossy(&raw_source);
    let src = normalize_shader_preprocessor_whitespace(&src);
    let src = normalize_asphalt8_shader_source(&src);
    // 3. Hoist `#extension` directives to the top — strict compilers (ANGLE
    //    etc.) reject them after non-preprocessor tokens; see
    //    `hoist_shader_extension_directives`.
    let src = hoist_shader_extension_directives(&src);
    let bytes_vec = strip_captain_tomato_shader_precision(&src).into_bytes();

    let cs = std::ffi::CString::new(bytes_vec).unwrap_or_default();
    let ptr = cs.as_ptr();
    // Remember what we submitted so `fix_fragment_only_varyings` can rewrite
    // the vertex shader at link time without depending on optional backend
    // entry points (GetShaderSource / GetAttachedShaders are unimplemented
    // on some backends, e.g. GLES2Native's GetAttachedShaders panics).
    record_shader_source(
        shader,
        String::from_utf8_lossy(cs.as_bytes()).into_owned(),
    );
    if crate::env_flag_cached!("TOUCHHLE_DUMP_SHADER_SOURCE") {
        let _ = std::fs::write(format!("shader_dump_{}.glsl", shader), cs.as_bytes());
    }
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.ShaderSource(shader, 1, &ptr, std::ptr::null());
    });
}
fn glEnableVertexAttribArray(env: &mut Environment, index: GLuint) {
    with_ctx_mem_and_shadow(env, |gles, _mem, shadow| unsafe {
        shadow.generic_attribs_used = true;
        shadow
            .vertex_array_state
            .set_generic_enabled(index as usize, true);
        gles.EnableVertexAttribArray(index)
    });
}
fn glDisableVertexAttribArray(env: &mut Environment, index: GLuint) {
    with_ctx_mem_and_shadow(env, |gles, _mem, shadow| unsafe {
        shadow
            .vertex_array_state
            .set_generic_enabled(index as usize, false);
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
    with_ctx_mem_and_shadow(env, |gles, mem, shadow| unsafe {
        let record = array_pointer_record(shadow, size, type_, normalized, stride, pointer);
        shadow
            .vertex_array_state
            .set_generic_pointer(index as usize, record);
        let host_ptr = if !buffer_is_bound(gles, shadow, ARRAY_BUFFER) {
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
fn glVertexAttrib1fv(env: &mut Environment, index: GLuint, values: ConstPtr<GLfloat>) {
    let value = env.mem.read(values);
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.VertexAttrib1fv(index, &value)
    });
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
fn glVertexAttrib2fv(env: &mut Environment, index: GLuint, values: ConstPtr<GLfloat>) {
    let value = env.mem.read(values);
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.VertexAttrib2fv(index, &value)
    });
}
fn glVertexAttrib3fv(env: &mut Environment, index: GLuint, values: ConstPtr<GLfloat>) {
    let value = env.mem.read(values);
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.VertexAttrib3fv(index, &value)
    });
}
fn glVertexAttrib4fv(env: &mut Environment, index: GLuint, values: ConstPtr<GLfloat>) {
    let value = env.mem.read(values);
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.VertexAttrib4fv(index, &value)
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
        if crate::env_flag_cached!("TOUCHHLE_TRACE_DRAW") && location >= 0 {
            use std::sync::atomic::{AtomicU32, Ordering};
            static UN: AtomicU32 = AtomicU32::new(0);
            let un = UN.fetch_add(1, Ordering::Relaxed);
            if un < 40 {
                log!("UNIFORM4F loc={} vals=[{},{},{},{}]", location, v0, v1, v2, v3);
            }
        }
        gles.Uniform4f(location, v0, v1, v2, v3)
    });
}
fn glUniform1iv(env: &mut Environment, location: GLint, count: GLsizei, value: ConstPtr<GLint>) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let n = count as usize;
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
        if crate::env_flag_cached!("TOUCHHLE_TRACE_DRAW") && location >= 0 {
            use std::sync::atomic::{AtomicU32, Ordering};
            static UN: AtomicU32 = AtomicU32::new(0);
            let un = UN.fetch_add(1, Ordering::Relaxed);
            if un < 40 {
                let mut v = vec![0.0f32; n.min(16)];
                std::ptr::copy_nonoverlapping(ptr, v.as_mut_ptr(), v.len());
                log!("UNIFORM4FV loc={} count={} vals={:?}", location, count, v);
            }
        }
        // EXPERIMENT (TOUCHHLE_FORCE_IDENTITY_TC): rewrite a shear-looking
        // texcoord-matrix uniform [1,-0,1,1] to identity [1,0,0,1] to test
        // whether the shear is what breaks textured rendering.
        if count == 1 && crate::env_flag_cached!("TOUCHHLE_FORCE_IDENTITY_TC") {
            let mut v = [0.0f32; 4];
            std::ptr::copy_nonoverlapping(ptr, v.as_mut_ptr(), 4);
            if v[0] == 1.0 && v[1] == 0.0 && v[2] == 1.0 && v[3] == 1.0 {
                let ident = [1.0f32, 0.0, 0.0, 1.0];
                gles.Uniform4fv(location, 1, ident.as_ptr());
                return;
            }
        }
        gles.Uniform4fv(location, count, ptr);
    });
}
/// `void glShaderBinary(GLsizei count, const GLuint *shaders,
///                      GLenum binaryformat, const void *binary,
///                      GLsizei length)` — OpenGL ES 2.0+.
/// Loads a pre-compiled shader binary. We forward count, format, and
/// raw bytes to the host so backends that support
/// `GL_EXT_shader_binary` can load them; backends that don't accept
/// any binary format will simply set GL_INVALID_ENUM, which is the
/// spec-mandated behaviour.
fn glShaderBinary(
    env: &mut Environment,
    count: GLsizei,
    shaders: ConstPtr<GLuint>,
    binary_format: GLenum,
    binary: ConstVoidPtr,
    length: GLsizei,
) {
    if count <= 0 || shaders.is_null() {
        return;
    }
    let n = count as usize;
    let mut shaders_vec: Vec<GLuint> = Vec::with_capacity(n);
    for i in 0..n {
        shaders_vec.push(env.mem.read(shaders + (i as GuestUSize)));
    }
    let bin_len = length.max(0) as GuestUSize;
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let bin_host = if binary.is_null() || bin_len == 0 {
            std::ptr::null()
        } else {
            mem.bytes_at(binary.cast(), bin_len).as_ptr().cast()
        };
        gles.ShaderBinary(count, shaders_vec.as_ptr(), binary_format, bin_host, length);
    });
}

/// `void glGetActiveUniformsiv(GLuint program, GLsizei uniformCount,
///                             const GLuint *uniformIndices, GLenum pname,
///                             GLint *params)` — OpenGL ES 3.0 §2.12.6.
fn glGetActiveUniformsiv(
    env: &mut Environment,
    program: GLuint,
    uniform_count: GLsizei,
    uniform_indices: ConstPtr<GLuint>,
    pname: GLenum,
    params: MutPtr<GLint>,
) {
    if uniform_count <= 0 {
        return;
    }
    let n = uniform_count as usize;
    let mut indices: Vec<GLuint> = Vec::with_capacity(n);
    for i in 0..n {
        indices.push(env.mem.read(uniform_indices + (i as GuestUSize)));
    }
    let mut results: Vec<GLint> = vec![0; n];
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.GetActiveUniformsiv(
            program,
            uniform_count,
            indices.as_ptr(),
            pname,
            results.as_mut_ptr(),
        );
    });
    for (i, v) in results.iter().enumerate() {
        env.mem.write(params + (i as GuestUSize), *v);
    }
}

/// `void glGetActiveUniformBlockiv(GLuint program,
///                                 GLuint uniformBlockIndex, GLenum pname,
///                                 GLint *params)` — OpenGL ES 3.0 §2.12.6.
fn glGetActiveUniformBlockiv(
    env: &mut Environment,
    program: GLuint,
    uniform_block_index: GLuint,
    pname: GLenum,
    params: MutPtr<GLint>,
) {
    // The number of values written depends on `pname`; the largest is
    // GL_UNIFORM_BLOCK_ACTIVE_UNIFORM_INDICES which returns up to
    // `GL_MAX_UNIFORM_BLOCK_BINDINGS` values. We allocate a generous
    // 64-element scratch buffer (more than enough for any GLES 3.0
    // hardware) and only write back what the backend reports.
    let mut scratch: [GLint; 64] = [0; 64];
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.GetActiveUniformBlockiv(program, uniform_block_index, pname, scratch.as_mut_ptr());
    });
    // Conservative: write back one int — sufficient for the scalar
    // queries (BINDING, DATA_SIZE, NAME_LENGTH, *_ACTIVE_UNIFORMS,
    // REFERENCED_BY_*_SHADER). For the array query the guest passes a
    // buffer it sized using GL_UNIFORM_BLOCK_ACTIVE_UNIFORMS, which it
    // queried right before; the backend writes into the host scratch
    // and we copy the count of ints the guest had asked for. Without
    // a way to know the buffer size we copy 1 int (covers all scalar
    // pnames) — the GL_UNIFORM_BLOCK_ACTIVE_UNIFORM_INDICES path
    // is exercised by very few apps and falls back to GL_INVALID_VALUE
    // on backends that don't implement uniform blocks.
    if !params.is_null() {
        env.mem.write(params, scratch[0]);
    }
}

/// `void glGetActiveUniformBlockName(GLuint program,
///                                   GLuint uniformBlockIndex,
///                                   GLsizei bufSize, GLsizei *length,
///                                   GLchar *uniformBlockName)`.
fn glGetActiveUniformBlockName(
    env: &mut Environment,
    program: GLuint,
    uniform_block_index: GLuint,
    buf_size: GLsizei,
    length: MutPtr<GLsizei>,
    uniform_block_name: MutPtr<GLubyte>,
) {
    if buf_size <= 0 || uniform_block_name.is_null() {
        if !length.is_null() {
            env.mem.write(length, 0);
        }
        return;
    }
    let cap = buf_size as usize;
    let mut name_buf: Vec<u8> = vec![0u8; cap];
    let mut host_length: GLsizei = 0;
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.GetActiveUniformBlockName(
            program,
            uniform_block_index,
            buf_size,
            &mut host_length,
            name_buf.as_mut_ptr() as *mut std::os::raw::c_char,
        );
    });
    let n = (host_length.max(0) as usize).min(cap);
    let dst = env
        .mem
        .bytes_at_mut(uniform_block_name.cast(), n as GuestUSize);
    dst.copy_from_slice(&name_buf[..n]);
    if !length.is_null() {
        env.mem.write(length, host_length);
    }
}

/// `void glProgramBinary(GLuint program, GLenum binaryFormat,
///                       const void *binary, GLsizei length)`.
fn glProgramBinary(
    env: &mut Environment,
    program: GLuint,
    binary_format: GLenum,
    binary: ConstVoidPtr,
    length: GLsizei,
) {
    let len = length.max(0) as GuestUSize;
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let host_ptr: *const GLvoid = if binary.is_null() || len == 0 {
            std::ptr::null()
        } else {
            mem.bytes_at(binary.cast(), len).as_ptr().cast()
        };
        gles.ProgramBinary(program, binary_format, host_ptr, length);
    });
}

/// `void glGetProgramBinary(GLuint program, GLsizei bufSize,
///                          GLsizei *length, GLenum *binaryFormat,
///                          void *binary)`.
fn glGetProgramBinary(
    env: &mut Environment,
    program: GLuint,
    buf_size: GLsizei,
    length: MutPtr<GLsizei>,
    binary_format: MutPtr<GLenum>,
    binary: MutVoidPtr,
) {
    if buf_size <= 0 || binary.is_null() {
        if !length.is_null() {
            env.mem.write(length, 0);
        }
        return;
    }
    let cap = buf_size as usize;
    let mut host_buf: Vec<u8> = vec![0u8; cap];
    let mut host_length: GLsizei = 0;
    let mut host_format: GLenum = 0;
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.GetProgramBinary(
            program,
            buf_size,
            &mut host_length,
            &mut host_format,
            host_buf.as_mut_ptr() as *mut GLvoid,
        );
    });
    let n = (host_length.max(0) as usize).min(cap);
    let dst = env.mem.bytes_at_mut(binary.cast(), n as GuestUSize);
    dst.copy_from_slice(&host_buf[..n]);
    if !length.is_null() {
        env.mem.write(length, host_length);
    }
    if !binary_format.is_null() {
        env.mem.write(binary_format, host_format);
    }
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
// but some apps still call these as no-ops. On an ES 3.0 context (where
// VAOs are core) we route to the real driver instead.

/// VAO entry point. On ES 1.1 / ES 2.0 the GLES trait has no VAO support and
/// the OES stub path returns sequential fake IDs (which most apps tolerate
/// as no-op handles). On ES 3.0 we delegate to the real driver, which is
/// mandatory because Core profile requires VAOs and the per-bind state is
/// observable via uniform locations etc.
fn glGenVertexArrays(env: &mut Environment, n: GLsizei, arrays: MutPtr<GLuint>) {
    let is_es3 = with_ctx_and_mem(env, |gles, _mem| gles.is_es3());
    if is_es3 {
        with_ctx_and_mem(env, |gles, mem| unsafe {
            let slice = mem.bytes_at_mut(arrays.cast(), (n as GuestUSize) * 4);
            gles.GenVertexArrays(n, slice.as_mut_ptr().cast());
        });
    } else {
        glGenVertexArraysOES(env, n, arrays)
    }
}
fn glBindVertexArray(env: &mut Environment, array: GLuint) {
    let is_es3 = with_ctx_and_mem(env, |gles, _mem| gles.is_es3());
    if is_es3 {
        with_ctx_mem_and_shadow(env, |gles, _mem, shadow| unsafe {
            // The element array buffer binding is VAO state.
            shadow.invalidate_vao_state();
            gles.BindVertexArray(array);
        });
    } else {
        glBindVertexArrayOES(env, array)
    }
}
fn glDeleteVertexArrays(env: &mut Environment, n: GLsizei, arrays: ConstPtr<GLuint>) {
    let is_es3 = with_ctx_and_mem(env, |gles, _mem| gles.is_es3());
    if is_es3 {
        with_ctx_mem_and_shadow(env, |gles, mem, shadow| unsafe {
            shadow.invalidate_vao_state();
            let slice = mem.bytes_at(arrays.cast(), (n as GuestUSize) * 4);
            gles.DeleteVertexArrays(n, slice.as_ptr().cast());
        });
    } else {
        glDeleteVertexArraysOES(env, n, arrays)
    }
}
fn glIsVertexArray(env: &mut Environment, array: GLuint) -> GLboolean {
    let is_es3 = with_ctx_and_mem(env, |gles, _mem| gles.is_es3());
    if is_es3 {
        with_ctx_and_mem(env, |gles, _mem| unsafe { gles.IsVertexArray(array) })
    } else {
        glIsVertexArrayOES(env, array)
    }
}

/// ES 3.0 has the unsuffixed `glUnmapBuffer`. On the ES 2.0 / ES 1.1 paths
/// `glUnmapBufferOES` is the matching entry point — share the existing
/// implementation since the buffer-mapping bookkeeping is identical.
fn glUnmapBuffer(env: &mut Environment, target: GLenum) -> GLboolean {
    unmap_buffer(env, target, false)
}

// ===== OpenGL ES 3.0 guest dispatchers =====
//
// Every function below is a thin wrapper that grabs the current GL context
// and forwards the call to the corresponding `GLES` trait method. Pointers
// are translated from guest memory to host memory via the `Mem` helper so
// the host driver sees host addresses.

// -- Buffer object operations --
fn glMapBufferRange(
    env: &mut Environment,
    target: GLenum,
    offset: GuestGLintptr,
    length: GuestGLsizeiptr,
    access: GLbitfield,
) -> MutVoidPtr {
    let host_ptr: *mut GLvoid = with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.MapBufferRange(
            target,
            offset as HostGLintptr,
            length as HostGLsizeiptr,
            access,
        )
    });
    // The host pointer cannot be observed by the guest directly. EAGL
    // tracks pending mappings in `EAGLContextHostObject::mapped_buffers`;
    // the matching `glUnmapBuffer` consults that table. Allocate a guest
    // buffer of `length` bytes, copy the contents from the host pointer,
    // and remember the pairing.
    if host_ptr.is_null() || length == 0 {
        log!(
            "Warning: glMapBufferRange(target={:#x}, offset={}, length={}, access={:#x}) \
             produced a NULL host pointer or zero length; returning NULL.",
            target,
            offset,
            length,
            access
        );
        return Ptr::null();
    }
    let length_usize = match usize::try_from(length) {
        Ok(size) => size,
        Err(_) => {
            log!(
                "Warning: glMapBufferRange with negative/overflowing length {}; returning NULL.",
                length
            );
            return Ptr::null();
        }
    };
    let guest_buf: MutPtr<GLvoid> = env.mem.alloc(length_usize as GuestUSize).cast();
    unsafe {
        let host_slice = from_raw_parts(host_ptr as *const u8, length_usize);
        let guest_slice = env
            .mem
            .bytes_at_mut(guest_buf.cast(), length_usize as GuestUSize);
        guest_slice.copy_from_slice(host_slice);
    }
    let current_ctx: Option<crate::objc::id> = *env
        .framework_state
        .opengles
        .current_ctx_for_thread(env.current_thread);
    let Some(ctx) = current_ctx else {
        log!(
            "Warning: glMapBufferRange on thread {} with no current EAGLContext; \
             returning NULL.",
            env.current_thread
        );
        env.mem.free(guest_buf);
        with_ctx_and_mem(env, |gles, _mem| unsafe {
            gles.UnmapBuffer(target);
        });
        return Ptr::null();
    };
    let buffer_object_name = _get_currently_bound_buffer_object_name(env, target);
    let host_obj = env.objc.borrow_mut::<EAGLContextHostObject>(ctx);
    if let Some((old_guest_buf, _, _)) = host_obj.mapped_buffers.insert(
        (target, buffer_object_name),
        (guest_buf, Some(host_ptr), length_usize),
    ) {
        env.mem.free(old_guest_buf);
    }
    guest_buf.cast()
}

fn glFlushMappedBufferRange(
    env: &mut Environment,
    target: GLenum,
    offset: GuestGLintptr,
    length: GuestGLsizeiptr,
) {
    // Copy the relevant slice of the guest-visible buffer back to the
    // host-mapped buffer so the driver sees the writes.
    let current_ctx: Option<crate::objc::id> = *env
        .framework_state
        .opengles
        .current_ctx_for_thread(env.current_thread);
    if let Some(ctx) = current_ctx {
        let buffer_object_name = _get_currently_bound_buffer_object_name(env, target);
        let mapping = env
            .objc
            .borrow::<EAGLContextHostObject>(ctx)
            .mapped_buffers
            .get(&(target, buffer_object_name))
            .copied();
        if let Some((guest_buf, host_ptr, mapped_size)) = mapping {
            let offset_usize = match usize::try_from(offset) {
                Ok(value) => value,
                Err(_) => return,
            };
            let length_usize = match usize::try_from(length) {
                Ok(value) => value,
                Err(_) => return,
            };
            let end = match offset_usize.checked_add(length_usize) {
                Some(value) if value <= mapped_size => value,
                _ => return,
            };
            match host_ptr {
                Some(host_ptr) => {
                    let guest_slice = env.mem.bytes_at(guest_buf.cast(), end as GuestUSize);
                    unsafe {
                        let host_slice = std::slice::from_raw_parts_mut(
                            (host_ptr as *mut u8).add(offset_usize),
                            length_usize,
                        );
                        host_slice.copy_from_slice(&guest_slice[offset_usize..end]);
                    }
                }
                None => {
                    // A glMapBufferOES mapping has no driver pointer; flushing
                    // it means uploading the requested range. No driver-side
                    // mapping exists, so there is nothing to forward the flush
                    // to either.
                    with_ctx_and_mem(env, |gles, mem| unsafe {
                        let data = mem.bytes_at(guest_buf.cast(), end as GuestUSize).as_ptr()
                            .add(offset_usize) as *const GLvoid;
                        gles.BufferSubData(
                            target,
                            offset as HostGLintptr,
                            length_usize as HostGLsizeiptr,
                            data,
                        );
                    });
                    return;
                }
            }
        }
    }
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.FlushMappedBufferRange(target, offset as HostGLintptr, length as HostGLsizeiptr)
    });
}

fn glCopyBufferSubData(
    env: &mut Environment,
    read_target: GLenum,
    write_target: GLenum,
    read_offset: GuestGLintptr,
    write_offset: GuestGLintptr,
    size: GuestGLsizeiptr,
) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.CopyBufferSubData(
            read_target,
            write_target,
            read_offset as HostGLintptr,
            write_offset as HostGLintptr,
            size as HostGLsizeiptr,
        )
    });
}

fn glBindBufferBase(env: &mut Environment, target: GLenum, index: GLuint, buffer: GLuint) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.BindBufferBase(target, index, buffer)
    });
}

fn glBindBufferRange(
    env: &mut Environment,
    target: GLenum,
    index: GLuint,
    buffer: GLuint,
    offset: GuestGLintptr,
    size: GuestGLsizeiptr,
) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.BindBufferRange(
            target,
            index,
            buffer,
            offset as HostGLintptr,
            size as HostGLsizeiptr,
        )
    });
}

// -- Drawing --
fn glDrawRangeElements(
    env: &mut Environment,
    mode: GLenum,
    start: GLuint,
    end: GLuint,
    count: GLsizei,
    type_: GLenum,
    indices: ConstVoidPtr,
) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        // ELEMENT_ARRAY_BUFFER bound: indices is a byte offset (small int)
        // → pass through. Otherwise it's a guest pointer → translate.
        let mut buf: GLint = 0;
        gles.GetIntegerv(ELEMENT_ARRAY_BUFFER_BINDING, &mut buf);
        let host_ptr: *const GLvoid = if buf != 0 {
            indices.cast::<u8>().to_bits() as usize as *const GLvoid
        } else {
            let bytes_per_index = match type_ {
                gles11::UNSIGNED_BYTE => 1,
                gles11::UNSIGNED_SHORT => 2,
                _ => 4,
            };
            let total_bytes = (count as GuestUSize) * bytes_per_index;
            mem.bytes_at(indices.cast(), total_bytes).as_ptr().cast()
        };
        crate::gles::perf_overlay::record_draw();
        gles.DrawRangeElements(mode, start, end, count, type_, host_ptr)
    });
}

fn glDrawArraysInstanced(
    env: &mut Environment,
    mode: GLenum,
    first: GLint,
    count: GLsizei,
    instance_count: GLsizei,
) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        crate::gles::perf_overlay::record_draw();
        gles.DrawArraysInstanced(mode, first, count, instance_count)
    });
}

fn glDrawElementsInstanced(
    env: &mut Environment,
    mode: GLenum,
    count: GLsizei,
    type_: GLenum,
    indices: ConstVoidPtr,
    instance_count: GLsizei,
) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let mut buf: GLint = 0;
        gles.GetIntegerv(ELEMENT_ARRAY_BUFFER_BINDING, &mut buf);
        let host_ptr: *const GLvoid = if buf != 0 {
            indices.cast::<u8>().to_bits() as usize as *const GLvoid
        } else {
            let bytes_per_index = match type_ {
                gles11::UNSIGNED_BYTE => 1,
                gles11::UNSIGNED_SHORT => 2,
                _ => 4,
            };
            let total_bytes = (count as GuestUSize) * bytes_per_index;
            mem.bytes_at(indices.cast(), total_bytes).as_ptr().cast()
        };
        crate::gles::perf_overlay::record_draw();
        gles.DrawElementsInstanced(mode, count, type_, host_ptr, instance_count)
    });
}

fn glVertexAttribDivisor(env: &mut Environment, index: GLuint, divisor: GLuint) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.VertexAttribDivisor(index, divisor)
    });
}

// -- Multiple render targets / draw buffers --
fn glReadBuffer(env: &mut Environment, src: GLenum) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.ReadBuffer(src) });
}

fn glDrawBuffers(env: &mut Environment, n: GLsizei, bufs: ConstPtr<GLenum>) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let slice = mem.bytes_at(bufs.cast(), (n as GuestUSize) * 4);
        gles.DrawBuffers(n, slice.as_ptr().cast())
    });
}

// -- glClearBuffer* --
fn glClearBufferiv(
    env: &mut Environment,
    buffer: GLenum,
    drawbuffer: GLint,
    value: ConstPtr<GLint>,
) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let slice = mem.bytes_at(value.cast(), 16);
        gles.ClearBufferiv(buffer, drawbuffer, slice.as_ptr().cast())
    });
}

fn glClearBufferuiv(
    env: &mut Environment,
    buffer: GLenum,
    drawbuffer: GLint,
    value: ConstPtr<GLuint>,
) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let slice = mem.bytes_at(value.cast(), 16);
        gles.ClearBufferuiv(buffer, drawbuffer, slice.as_ptr().cast())
    });
}

fn glClearBufferfv(
    env: &mut Environment,
    buffer: GLenum,
    drawbuffer: GLint,
    value: ConstPtr<GLfloat>,
) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let slice = mem.bytes_at(value.cast(), 16);
        gles.ClearBufferfv(buffer, drawbuffer, slice.as_ptr().cast())
    });
}

fn glClearBufferfi(
    env: &mut Environment,
    buffer: GLenum,
    drawbuffer: GLint,
    depth: GLfloat,
    stencil: GLint,
) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.ClearBufferfi(buffer, drawbuffer, depth, stencil)
    });
}

// -- Framebuffer blits / multisample / layered attachments --
#[allow(clippy::too_many_arguments)]
fn glBlitFramebuffer(
    env: &mut Environment,
    src_x0: GLint,
    src_y0: GLint,
    src_x1: GLint,
    src_y1: GLint,
    dst_x0: GLint,
    dst_y0: GLint,
    dst_x1: GLint,
    dst_y1: GLint,
    mask: GLbitfield,
    filter: GLenum,
) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.BlitFramebuffer(
            src_x0, src_y0, src_x1, src_y1, dst_x0, dst_y0, dst_x1, dst_y1, mask, filter,
        )
    });
}

fn glRenderbufferStorageMultisample(
    env: &mut Environment,
    target: GLenum,
    samples: GLsizei,
    internalformat: GLenum,
    width: GLsizei,
    height: GLsizei,
) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.RenderbufferStorageMultisample(target, samples, internalformat, width, height)
    });
}

fn glFramebufferTextureLayer(
    env: &mut Environment,
    target: GLenum,
    attachment: GLenum,
    texture: GLuint,
    level: GLint,
    layer: GLint,
) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.FramebufferTextureLayer(target, attachment, texture, level, layer)
    });
}

fn glInvalidateFramebuffer(
    env: &mut Environment,
    target: GLenum,
    num_attachments: GLsizei,
    attachments: ConstPtr<GLenum>,
) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let slice = mem.bytes_at(attachments.cast(), (num_attachments as GuestUSize) * 4);
        gles.InvalidateFramebuffer(target, num_attachments, slice.as_ptr().cast())
    });
}

#[allow(clippy::too_many_arguments)]
fn glInvalidateSubFramebuffer(
    env: &mut Environment,
    target: GLenum,
    num_attachments: GLsizei,
    attachments: ConstPtr<GLenum>,
    x: GLint,
    y: GLint,
    width: GLsizei,
    height: GLsizei,
) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let slice = mem.bytes_at(attachments.cast(), (num_attachments as GuestUSize) * 4);
        gles.InvalidateSubFramebuffer(
            target,
            num_attachments,
            slice.as_ptr().cast(),
            x,
            y,
            width,
            height,
        )
    });
}

// -- 3D textures and immutable storage --
#[allow(clippy::too_many_arguments)]
fn glTexImage3D(
    env: &mut Environment,
    target: GLenum,
    level: GLint,
    internalformat: GLint,
    width: GLsizei,
    height: GLsizei,
    depth: GLsizei,
    border: GLint,
    format: GLenum,
    type_: GLenum,
    pixels: ConstVoidPtr,
) {
    with_ctx_mem_and_shadow(env, |gles, mem, shadow| unsafe {
        let bound_texture = current_bound_texture(gles, target);
        let mut buf: GLint = 0;
        gles.GetIntegerv(0x88EF, &mut buf);
        let host_pixels: *const GLvoid = if buf != 0 || pixels.is_null() {
            pixels.cast::<u8>().to_bits() as usize as *const GLvoid
        } else {
            let total = (width as GuestUSize) * (height as GuestUSize) * (depth as GuestUSize) * 4;
            mem.bytes_at(pixels.cast(), total).as_ptr().cast()
        };
        gles.TexImage3D(
            target,
            level,
            internalformat,
            width,
            height,
            depth,
            border,
            format,
            type_,
            host_pixels,
        );
        if border == 0 {
            if let Some(texture) = bound_texture {
                let level_bytes = texture_level_bytes(width, height, format, type_)
                    .saturating_mul(depth.max(0) as u64);
                crate::gles::perf_overlay::record_gl_image_allocation(
                    shadow.resource_group_token,
                    target,
                    texture,
                    level,
                    level_bytes,
                );
            }
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn glTexSubImage3D(
    env: &mut Environment,
    target: GLenum,
    level: GLint,
    xoffset: GLint,
    yoffset: GLint,
    zoffset: GLint,
    width: GLsizei,
    height: GLsizei,
    depth: GLsizei,
    format: GLenum,
    type_: GLenum,
    pixels: ConstVoidPtr,
) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let mut buf: GLint = 0;
        // See `glTexImage3D` above for the rationale for the literal.
        gles.GetIntegerv(0x88EF, &mut buf);
        let host_pixels: *const GLvoid = if buf != 0 || pixels.is_null() {
            pixels.cast::<u8>().to_bits() as usize as *const GLvoid
        } else {
            let total = (width as GuestUSize) * (height as GuestUSize) * (depth as GuestUSize) * 4;
            mem.bytes_at(pixels.cast(), total).as_ptr().cast()
        };
        gles.TexSubImage3D(
            target,
            level,
            xoffset,
            yoffset,
            zoffset,
            width,
            height,
            depth,
            format,
            type_,
            host_pixels,
        )
    });
}

#[allow(clippy::too_many_arguments)]
fn glCopyTexSubImage3D(
    env: &mut Environment,
    target: GLenum,
    level: GLint,
    xoffset: GLint,
    yoffset: GLint,
    zoffset: GLint,
    x: GLint,
    y: GLint,
    width: GLsizei,
    height: GLsizei,
) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.CopyTexSubImage3D(
            target, level, xoffset, yoffset, zoffset, x, y, width, height,
        )
    });
}

fn glTexStorage2D(
    env: &mut Environment,
    target: GLenum,
    levels: GLsizei,
    internalformat: GLenum,
    width: GLsizei,
    height: GLsizei,
) {
    with_ctx_mem_and_shadow(env, |gles, _mem, shadow| unsafe {
        let bound_texture = current_bound_texture(gles, target);
        // Apps that moved to the ES 3.0 API (Gameloft games such as Asphalt 8
        // v1.2.0+) allocate their PVRTC texture atlases via immutable storage:
        // `glTexStorage2D` with a PVRTC internalformat, then per-level
        // `glCompressedTexSubImage2D` uploads. A host driver without
        // `GL_IMG_texture_compression_pvrtc` rejects the internalformat with
        // `GL_INVALID_ENUM`, the storage is never created, and every subsequent
        // compressed upload fails with `GL_INVALID_OPERATION` — leaving all
        // atlases black while UI text (uploaded uncompressed) still renders.
        //
        // When the host can't do PVRTC, skip immutable storage and instead
        // pre-record every mip level as a PVRTC level (dimensions halving per
        // level) whose RGBA storage is materialized lazily by the
        // `glCompressedTexSubImage2D` / `glCompressedTexImage2D` software-decode
        // paths below.
        if crate::gles::util::pvrtc_format_properties(internalformat).is_some()
            && !gles.pvrtc_native()
        {
            if let Some(texture) = bound_texture {
                for level in 0..levels.max(0) {
                    let level_width = (width >> level).max(1);
                    let level_height = (height >> level).max(1);
                    shadow.record_pvrtc_texture_level(
                        target,
                        texture,
                        level,
                        level_width,
                        level_height,
                        internalformat,
                    );
                    shadow.record_pvrtc_unallocated_level(target, texture, level);
                }
            }
            log_once!("Emulating glTexStorage2D for a PVRTC internalformat: immutable storage skipped, uploads will be software-decoded to RGBA");
            return;
        }
        gles.TexStorage2D(target, levels, internalformat, width, height);
        if let Some(texture) = bound_texture {
            shadow.forget_pvrtc_texture(texture);
            crate::gles::perf_overlay::clear_gl_image_allocations(
                shadow.resource_group_token,
                texture,
            );
            let cube_faces = if target == TEXTURE_CUBE_MAP { 6 } else { 1 };
            let mut level_width = width;
            let mut level_height = height;
            for level in 0..levels.max(0) {
                let size = texture_storage_level_bytes(level_width, level_height, internalformat)
                    .saturating_mul(cube_faces);
                crate::gles::perf_overlay::record_gl_image_allocation(
                    shadow.resource_group_token,
                    target,
                    texture,
                    level,
                    size,
                );
                level_width = (level_width / 2).max(1);
                level_height = (level_height / 2).max(1);
            }
        }
    });
}

fn glTexStorage2DEXT(
    env: &mut Environment,
    target: GLenum,
    levels: GLsizei,
    internalformat: GLenum,
    width: GLsizei,
    height: GLsizei,
) {
    glTexStorage2D(env, target, levels, internalformat, width, height);
}

fn glTexStorage3D(
    env: &mut Environment,
    target: GLenum,
    levels: GLsizei,
    internalformat: GLenum,
    width: GLsizei,
    height: GLsizei,
    depth: GLsizei,
) {
    with_ctx_mem_and_shadow(env, |gles, _mem, shadow| unsafe {
        let bound_texture = current_bound_texture(gles, target);
        gles.TexStorage3D(target, levels, internalformat, width, height, depth);
        if let Some(texture) = bound_texture {
            shadow.forget_pvrtc_texture(texture);
            crate::gles::perf_overlay::clear_gl_image_allocations(
                shadow.resource_group_token,
                texture,
            );
            let mut level_width = width;
            let mut level_height = height;
            let mut level_depth = depth;
            for level in 0..levels.max(0) {
                let size = texture_storage_volume_bytes(
                    level_width,
                    level_height,
                    level_depth,
                    internalformat,
                );
                crate::gles::perf_overlay::record_gl_image_allocation(
                    shadow.resource_group_token,
                    target,
                    texture,
                    level,
                    size,
                );
                level_width = (level_width / 2).max(1);
                level_height = (level_height / 2).max(1);
                if target == TEXTURE_3D {
                    level_depth = (level_depth / 2).max(1);
                }
            }
        }
    });
}

// -- Query objects --
fn glGenQueries(env: &mut Environment, n: GLsizei, ids: MutPtr<GLuint>) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let slice = mem.bytes_at_mut(ids.cast(), (n as GuestUSize) * 4);
        gles.GenQueries(n, slice.as_mut_ptr().cast())
    });
}

fn glDeleteQueries(env: &mut Environment, n: GLsizei, ids: ConstPtr<GLuint>) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let slice = mem.bytes_at(ids.cast(), (n as GuestUSize) * 4);
        gles.DeleteQueries(n, slice.as_ptr().cast())
    });
}

fn glIsQuery(env: &mut Environment, id: GLuint) -> GLboolean {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.IsQuery(id) })
}

fn glBeginQuery(env: &mut Environment, target: GLenum, id: GLuint) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.BeginQuery(target, id) });
}

fn glEndQuery(env: &mut Environment, target: GLenum) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.EndQuery(target) });
}

fn glGetQueryiv(env: &mut Environment, target: GLenum, pname: GLenum, params: MutPtr<GLint>) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let slice = mem.bytes_at_mut(params.cast(), 4);
        gles.GetQueryiv(target, pname, slice.as_mut_ptr().cast())
    });
}

fn glGetQueryObjectuiv(env: &mut Environment, id: GLuint, pname: GLenum, params: MutPtr<GLuint>) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let slice = mem.bytes_at_mut(params.cast(), 4);
        gles.GetQueryObjectuiv(id, pname, slice.as_mut_ptr().cast())
    });
}

// -- Boolean occlusion queries (GL_EXT_occlusion_query_boolean) --
// The `*EXT` entry points are the OpenGL ES 2.0 form of the boolean occlusion
// query API. They are semantically identical to the ES 3.0 core query objects
// (only the accepted `target`s differ: GL_ANY_SAMPLES_PASSED_EXT and
// GL_ANY_SAMPLES_PASSED_CONSERVATIVE_EXT), so each `*EXT` wrapper forwards to
// the same backend trait method as its core counterpart. iPhone OS games such
// as Rush Rally 2 link against these symbols directly, so they must be exported
// or the dynamic linker leaves the guest's function pointer null and the app
// jumps to a null address on launch.
// Reference: https://registry.khronos.org/OpenGL/extensions/EXT/EXT_occlusion_query_boolean.txt
fn glGenQueriesEXT(env: &mut Environment, n: GLsizei, ids: MutPtr<GLuint>) {
    glGenQueries(env, n, ids)
}

fn glDeleteQueriesEXT(env: &mut Environment, n: GLsizei, ids: ConstPtr<GLuint>) {
    glDeleteQueries(env, n, ids)
}

fn glIsQueryEXT(env: &mut Environment, id: GLuint) -> GLboolean {
    glIsQuery(env, id)
}

fn glBeginQueryEXT(env: &mut Environment, target: GLenum, id: GLuint) {
    glBeginQuery(env, target, id)
}

fn glEndQueryEXT(env: &mut Environment, target: GLenum) {
    glEndQuery(env, target)
}

fn glGetQueryivEXT(env: &mut Environment, target: GLenum, pname: GLenum, params: MutPtr<GLint>) {
    glGetQueryiv(env, target, pname, params)
}

fn glGetQueryObjectuivEXT(
    env: &mut Environment,
    id: GLuint,
    pname: GLenum,
    params: MutPtr<GLuint>,
) {
    glGetQueryObjectuiv(env, id, pname, params)
}

// -- Sampler objects --
fn glGenSamplers(env: &mut Environment, count: GLsizei, samplers: MutPtr<GLuint>) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let slice = mem.bytes_at_mut(samplers.cast(), (count as GuestUSize) * 4);
        gles.GenSamplers(count, slice.as_mut_ptr().cast())
    });
}

fn glDeleteSamplers(env: &mut Environment, count: GLsizei, samplers: ConstPtr<GLuint>) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let slice = mem.bytes_at(samplers.cast(), (count as GuestUSize) * 4);
        gles.DeleteSamplers(count, slice.as_ptr().cast())
    });
}

fn glIsSampler(env: &mut Environment, sampler: GLuint) -> GLboolean {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.IsSampler(sampler) })
}

fn glBindSampler(env: &mut Environment, unit: GLuint, sampler: GLuint) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.BindSampler(unit, sampler) });
}

fn glSamplerParameteri(env: &mut Environment, sampler: GLuint, pname: GLenum, param: GLint) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.SamplerParameteri(sampler, pname, param)
    });
}

fn glSamplerParameterf(env: &mut Environment, sampler: GLuint, pname: GLenum, param: GLfloat) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.SamplerParameterf(sampler, pname, param)
    });
}

// -- Transform feedback --
fn glBeginTransformFeedback(env: &mut Environment, primitive_mode: GLenum) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.BeginTransformFeedback(primitive_mode)
    });
}

fn glEndTransformFeedback(env: &mut Environment) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.EndTransformFeedback() });
}

fn glBindTransformFeedback(env: &mut Environment, target: GLenum, id: GLuint) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.BindTransformFeedback(target, id)
    });
}

fn glDeleteTransformFeedbacks(env: &mut Environment, n: GLsizei, ids: ConstPtr<GLuint>) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let slice = mem.bytes_at(ids.cast(), (n as GuestUSize) * 4);
        gles.DeleteTransformFeedbacks(n, slice.as_ptr().cast())
    });
}

fn glGenTransformFeedbacks(env: &mut Environment, n: GLsizei, ids: MutPtr<GLuint>) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let slice = mem.bytes_at_mut(ids.cast(), (n as GuestUSize) * 4);
        gles.GenTransformFeedbacks(n, slice.as_mut_ptr().cast())
    });
}

fn glIsTransformFeedback(env: &mut Environment, id: GLuint) -> GLboolean {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.IsTransformFeedback(id) })
}

fn glPauseTransformFeedback(env: &mut Environment) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.PauseTransformFeedback() });
}

fn glResumeTransformFeedback(env: &mut Environment) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.ResumeTransformFeedback() });
}

// -- Integer vertex attributes --
fn glVertexAttribIPointer(
    env: &mut Environment,
    index: GLuint,
    size: GLint,
    type_: GLenum,
    stride: GLsizei,
    pointer: ConstVoidPtr,
) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        // ARRAY_BUFFER bound → `pointer` is interpreted as a byte offset
        // into the bound VBO; not as a guest pointer. In either case the
        // 32-bit "pointer" value is forwarded verbatim — when no VBO is
        // bound the host driver reads from guest memory at the actual
        // pointer, which is mapped into the host address space.
        let host_ptr = pointer.cast::<u8>().to_bits() as usize as *const GLvoid;
        gles.VertexAttribIPointer(index, size, type_, stride, host_ptr)
    });
}

fn glVertexAttribI4i(env: &mut Environment, index: GLuint, x: GLint, y: GLint, z: GLint, w: GLint) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.VertexAttribI4i(index, x, y, z, w)
    });
}

fn glVertexAttribI4ui(
    env: &mut Environment,
    index: GLuint,
    x: GLuint,
    y: GLuint,
    z: GLuint,
    w: GLuint,
) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.VertexAttribI4ui(index, x, y, z, w)
    });
}

// -- 3D / compressed-3D textures (OpenGL ES 3.0 §3.8.6) --
fn glCompressedTexImage3D(
    env: &mut Environment,
    target: GLenum,
    level: GLint,
    internalformat: GLenum,
    width: GLsizei,
    height: GLsizei,
    depth: GLsizei,
    border: GLint,
    image_size: GLsizei,
    data: ConstVoidPtr,
) {
    with_ctx_mem_and_shadow(env, |gles, mem, shadow| unsafe {
        let bound_texture = current_bound_texture(gles, target);
        let data = if data.is_null() {
            std::ptr::null()
        } else {
            mem.ptr_at(data.cast::<u8>(), image_size.max(0) as GuestUSize)
                .cast()
        };
        gles.CompressedTexImage3D(
            target,
            level,
            internalformat,
            width,
            height,
            depth,
            border,
            image_size,
            data,
        );
        if border == 0 && image_size > 0 {
            if let Some(texture) = bound_texture {
                crate::gles::perf_overlay::record_gl_image_allocation(
                    shadow.resource_group_token,
                    target,
                    texture,
                    level,
                    image_size as u64,
                );
            }
        }
    });
}

fn glCompressedTexSubImage3D(
    env: &mut Environment,
    target: GLenum,
    level: GLint,
    xoffset: GLint,
    yoffset: GLint,
    zoffset: GLint,
    width: GLsizei,
    height: GLsizei,
    depth: GLsizei,
    format: GLenum,
    image_size: GLsizei,
    data: ConstVoidPtr,
) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let data = if data.is_null() {
            std::ptr::null()
        } else {
            mem.ptr_at(data.cast::<u8>(), image_size.max(0) as GuestUSize)
                .cast()
        };
        gles.CompressedTexSubImage3D(
            target, level, xoffset, yoffset, zoffset, width, height, depth, format, image_size,
            data,
        )
    });
}

// -- Sampler vector parameters (OpenGL ES 3.0 §3.8.10) --
fn glSamplerParameteriv(
    env: &mut Environment,
    sampler: GLuint,
    pname: GLenum,
    params: ConstPtr<GLint>,
) {
    let params = env.mem.ptr_at(params, 4);
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.SamplerParameteriv(sampler, pname, params)
    });
}

fn glSamplerParameterfv(
    env: &mut Environment,
    sampler: GLuint,
    pname: GLenum,
    params: ConstPtr<GLfloat>,
) {
    let params = env.mem.ptr_at(params, 4);
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.SamplerParameterfv(sampler, pname, params)
    });
}

fn glGetSamplerParameteriv(
    env: &mut Environment,
    sampler: GLuint,
    pname: GLenum,
    params: MutPtr<GLint>,
) {
    let params = env.mem.ptr_at_mut(params, 4);
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.GetSamplerParameteriv(sampler, pname, params)
    });
}

fn glGetSamplerParameterfv(
    env: &mut Environment,
    sampler: GLuint,
    pname: GLenum,
    params: MutPtr<GLfloat>,
) {
    let params = env.mem.ptr_at_mut(params, 4);
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.GetSamplerParameterfv(sampler, pname, params)
    });
}

// -- Indexed / 64-bit state queries (OpenGL ES 3.0 §6.1.1) --
fn glGetInteger64v(env: &mut Environment, pname: GLenum, data: MutPtr<i64>) {
    let data = env.mem.ptr_at_mut(data, 16);
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.GetInteger64v(pname, data) });
}

fn glGetIntegeri_v(env: &mut Environment, target: GLenum, index: GLuint, data: MutPtr<GLint>) {
    let data = env.mem.ptr_at_mut(data, 4);
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.GetIntegeri_v(target, index, data)
    });
}

fn glGetInteger64i_v(env: &mut Environment, target: GLenum, index: GLuint, data: MutPtr<i64>) {
    let data = env.mem.ptr_at_mut(data, 4);
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.GetInteger64i_v(target, index, data)
    });
}

fn glGetBufferParameteri64v(
    env: &mut Environment,
    target: GLenum,
    pname: GLenum,
    params: MutPtr<i64>,
) {
    let params = env.mem.ptr_at_mut(params, 1);
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.GetBufferParameteri64v(target, pname, params)
    });
}

fn glGetInternalformativ(
    env: &mut Environment,
    target: GLenum,
    internalformat: GLenum,
    pname: GLenum,
    buf_size: GLsizei,
    params: MutPtr<GLint>,
) {
    if buf_size <= 0 {
        return;
    }
    let params = env.mem.ptr_at_mut(params, buf_size as GuestUSize);
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.GetInternalformativ(target, internalformat, pname, buf_size, params)
    });
}

// -- Integer vertex attributes (OpenGL ES 3.0 §2.7 / §6.1.10) --
fn glVertexAttribI4iv(env: &mut Environment, index: GLuint, v: ConstPtr<GLint>) {
    let v = env.mem.ptr_at(v, 4);
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.VertexAttribI4iv(index, v) });
}

fn glVertexAttribI4uiv(env: &mut Environment, index: GLuint, v: ConstPtr<GLuint>) {
    let v = env.mem.ptr_at(v, 4);
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.VertexAttribI4uiv(index, v)
    });
}

// -- Vertex attribute queries (OpenGL ES 2.0 §6.1.8) --
// The largest query (GL_CURRENT_VERTEX_ATTRIB) returns four values, so the
// out-buffer is always mapped as 4 elements, matching Apple's headers.

fn glGetVertexAttribfv(
    env: &mut Environment,
    index: GLuint,
    pname: GLenum,
    params: MutPtr<GLfloat>,
) {
    let params = env.mem.ptr_at_mut(params, 4);
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.GetVertexAttribfv(index, pname, params)
    });
}

fn glGetVertexAttribiv(env: &mut Environment, index: GLuint, pname: GLenum, params: MutPtr<GLint>) {
    let params = env.mem.ptr_at_mut(params, 4);
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.GetVertexAttribiv(index, pname, params)
    });
}

/// `void glGetVertexAttribPointerv(GLuint index, GLenum pname, void **pointer)`
/// — OpenGL ES 2.0 §6.1.8. The result is either a buffer offset (when a
/// buffer object is bound to the attribute) or a client-side array pointer,
/// which must be translated from a host address back to a guest address.
fn glGetVertexAttribPointerv(
    env: &mut Environment,
    index: GLuint,
    pname: GLenum,
    pointer: MutPtr<ConstVoidPtr>,
) {
    const VERTEX_ATTRIB_ARRAY_BUFFER_BINDING: GLenum = 0x889F;
    with_ctx_and_mem(env, |gles, mem| {
        let mut buffer_binding: GLint = 0;
        let mut host_pointer_or_offset: *mut GLvoid = std::ptr::null_mut();
        let guest_pointer_or_offset = unsafe {
            gles.GetVertexAttribiv(
                index,
                VERTEX_ATTRIB_ARRAY_BUFFER_BINDING,
                &mut buffer_binding,
            );
            gles.GetVertexAttribPointerv(index, pname, &mut host_pointer_or_offset);
            if buffer_binding != 0 {
                // A buffer object is bound: the "pointer" is really an offset
                // and must be passed through unchanged.
                Ptr::from_bits(u32::try_from(host_pointer_or_offset as usize).unwrap_or(0))
            } else if host_pointer_or_offset.is_null() {
                Ptr::null()
            } else {
                mem.host_ptr_to_guest_ptr(host_pointer_or_offset)
            }
        };
        mem.write(pointer, guest_pointer_or_offset);
    });
}

fn glGetVertexAttribIiv(
    env: &mut Environment,
    index: GLuint,
    pname: GLenum,
    params: MutPtr<GLint>,
) {
    let params = env.mem.ptr_at_mut(params, 4);
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.GetVertexAttribIiv(index, pname, params)
    });
}

fn glGetVertexAttribIuiv(
    env: &mut Environment,
    index: GLuint,
    pname: GLenum,
    params: MutPtr<GLuint>,
) {
    let params = env.mem.ptr_at_mut(params, 4);
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.GetVertexAttribIuiv(index, pname, params)
    });
}

// -- Unsigned-integer uniform queries (OpenGL ES 3.0 §6.1.14 / §2.12.6) --
fn glGetUniformuiv(
    env: &mut Environment,
    program: GLuint,
    location: GLint,
    params: MutPtr<GLuint>,
) {
    let params = env.mem.ptr_at_mut(params, 16);
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.GetUniformuiv(program, location, params)
    });
}

/// `void glGetUniformIndices(GLuint program, GLsizei uniformCount,
///   const GLchar *const *uniformNames, GLuint *uniformIndices)` — ES 3.0
/// §2.12.6.
fn glGetUniformIndices(
    env: &mut Environment,
    program: GLuint,
    uniform_count: GLsizei,
    uniform_names: ConstPtr<ConstPtr<GLubyte>>,
    uniform_indices: MutPtr<GLuint>,
) {
    if uniform_count <= 0 {
        return;
    }
    let n = uniform_count as usize;
    let mut owned: Vec<std::ffi::CString> = Vec::with_capacity(n);
    for i in 0..n {
        let name_ptr: ConstPtr<GLubyte> = env.mem.read(uniform_names + (i as GuestUSize));
        if name_ptr.is_null() {
            owned.push(std::ffi::CString::default());
        } else {
            owned.push(
                std::ffi::CString::new(env.mem.cstr_at(name_ptr).to_vec()).unwrap_or_default(),
            );
        }
    }
    let ptrs: Vec<*const std::os::raw::c_char> = owned.iter().map(|s| s.as_ptr()).collect();
    let mut results: Vec<GLuint> = vec![0; n];
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.GetUniformIndices(program, uniform_count, ptrs.as_ptr(), results.as_mut_ptr());
    });
    for (i, v) in results.iter().enumerate() {
        env.mem.write(uniform_indices + (i as GuestUSize), *v);
    }
}

// -- Transform feedback varyings (OpenGL ES 3.0 §2.15.2 / §6.1.12) --
fn glTransformFeedbackVaryings(
    env: &mut Environment,
    program: GLuint,
    count: GLsizei,
    varyings: ConstPtr<ConstPtr<GLubyte>>,
    buffer_mode: GLenum,
) {
    if count <= 0 {
        return;
    }
    let n = count as usize;
    let mut owned: Vec<std::ffi::CString> = Vec::with_capacity(n);
    for i in 0..n {
        let name_ptr: ConstPtr<GLubyte> = env.mem.read(varyings + (i as GuestUSize));
        if name_ptr.is_null() {
            owned.push(std::ffi::CString::default());
        } else {
            owned.push(
                std::ffi::CString::new(env.mem.cstr_at(name_ptr).to_vec()).unwrap_or_default(),
            );
        }
    }
    let ptrs: Vec<*const std::os::raw::c_char> = owned.iter().map(|s| s.as_ptr()).collect();
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.TransformFeedbackVaryings(program, count, ptrs.as_ptr(), buffer_mode);
    });
}

fn glGetTransformFeedbackVarying(
    env: &mut Environment,
    program: GLuint,
    index: GLuint,
    buf_size: GLsizei,
    length: MutPtr<GLsizei>,
    size: MutPtr<GLint>,
    type_: MutPtr<GLenum>,
    name: MutPtr<GLubyte>,
) {
    let mut host_length: GLsizei = 0;
    let mut host_size: GLint = 0;
    let mut host_type: GLenum = 0;
    let mut host_name: Vec<u8> = vec![0u8; buf_size.max(0) as usize];
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.GetTransformFeedbackVarying(
            program,
            index,
            buf_size,
            &mut host_length,
            &mut host_size,
            &mut host_type,
            host_name.as_mut_ptr().cast(),
        );
    });
    if !length.is_null() {
        env.mem.write(length, host_length);
    }
    if !size.is_null() {
        env.mem.write(size, host_size);
    }
    if !type_.is_null() {
        env.mem.write(type_, host_type);
    }
    if !name.is_null() && buf_size > 0 {
        let copy = (host_length as usize + 1).min(buf_size as usize);
        let dst = env.mem.ptr_at_mut(name, copy as GuestUSize);
        unsafe {
            std::ptr::copy_nonoverlapping(host_name.as_ptr(), dst, copy);
        }
    }
}

/// `void glGetBufferPointerv(GLenum target, GLenum pname, void **params)` —
/// ES 3.0 §6.1.15. The only valid `pname` is `GL_BUFFER_MAP_POINTER`, which
/// is NULL unless the buffer is currently mapped. touchHLE exposes buffer
/// mappings to the guest via glMapBufferOES (which hands the app a *guest*
/// pointer and keeps its own host<->guest mirror), so the raw host pointer
/// the driver returns is not meaningful in the guest address space. We still
/// forward the real query, and report the guest-visible result, which is NULL
/// when no guest-visible mapping is active — the GL default and common case.
fn glGetBufferPointerv(
    env: &mut Environment,
    target: GLenum,
    pname: GLenum,
    params: MutPtr<MutVoidPtr>,
) {
    let mut host_ptr: *mut GLvoid = std::ptr::null_mut();
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.GetBufferPointerv(target, pname, &mut host_ptr);
    });
    let _ = host_ptr;
    env.mem.write(params, Ptr::null());
}

// -- Integer uniforms --
fn glUniform1ui(env: &mut Environment, location: GLint, v0: GLuint) {
    with_ctx_and_mem(env, |gles, _mem| unsafe { gles.Uniform1ui(location, v0) });
}

fn glUniform2ui(env: &mut Environment, location: GLint, v0: GLuint, v1: GLuint) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.Uniform2ui(location, v0, v1)
    });
}

fn glUniform3ui(env: &mut Environment, location: GLint, v0: GLuint, v1: GLuint, v2: GLuint) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.Uniform3ui(location, v0, v1, v2)
    });
}

fn glUniform4ui(
    env: &mut Environment,
    location: GLint,
    v0: GLuint,
    v1: GLuint,
    v2: GLuint,
    v3: GLuint,
) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.Uniform4ui(location, v0, v1, v2, v3)
    });
}

fn glUniform1uiv(env: &mut Environment, location: GLint, count: GLsizei, value: ConstPtr<GLuint>) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let slice = mem.bytes_at(value.cast(), (count as GuestUSize) * 4);
        gles.Uniform1uiv(location, count, slice.as_ptr().cast())
    });
}

fn glUniform2uiv(env: &mut Environment, location: GLint, count: GLsizei, value: ConstPtr<GLuint>) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let slice = mem.bytes_at(value.cast(), (count as GuestUSize) * 8);
        gles.Uniform2uiv(location, count, slice.as_ptr().cast())
    });
}

fn glUniform3uiv(env: &mut Environment, location: GLint, count: GLsizei, value: ConstPtr<GLuint>) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let slice = mem.bytes_at(value.cast(), (count as GuestUSize) * 12);
        gles.Uniform3uiv(location, count, slice.as_ptr().cast())
    });
}

fn glUniform4uiv(env: &mut Environment, location: GLint, count: GLsizei, value: ConstPtr<GLuint>) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let slice = mem.bytes_at(value.cast(), (count as GuestUSize) * 16);
        gles.Uniform4uiv(location, count, slice.as_ptr().cast())
    });
}

fn glUniformMatrix2x3fv(
    env: &mut Environment,
    location: GLint,
    count: GLsizei,
    transpose: GLboolean,
    value: ConstPtr<GLfloat>,
) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let slice = mem.bytes_at(value.cast(), (count as GuestUSize) * 24);
        gles.UniformMatrix2x3fv(location, count, transpose, slice.as_ptr().cast())
    });
}

fn glUniformMatrix3x2fv(
    env: &mut Environment,
    location: GLint,
    count: GLsizei,
    transpose: GLboolean,
    value: ConstPtr<GLfloat>,
) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let slice = mem.bytes_at(value.cast(), (count as GuestUSize) * 24);
        gles.UniformMatrix3x2fv(location, count, transpose, slice.as_ptr().cast())
    });
}

fn glUniformMatrix2x4fv(
    env: &mut Environment,
    location: GLint,
    count: GLsizei,
    transpose: GLboolean,
    value: ConstPtr<GLfloat>,
) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let slice = mem.bytes_at(value.cast(), (count as GuestUSize) * 32);
        gles.UniformMatrix2x4fv(location, count, transpose, slice.as_ptr().cast())
    });
}

fn glUniformMatrix4x2fv(
    env: &mut Environment,
    location: GLint,
    count: GLsizei,
    transpose: GLboolean,
    value: ConstPtr<GLfloat>,
) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let slice = mem.bytes_at(value.cast(), (count as GuestUSize) * 32);
        gles.UniformMatrix4x2fv(location, count, transpose, slice.as_ptr().cast())
    });
}

fn glUniformMatrix3x4fv(
    env: &mut Environment,
    location: GLint,
    count: GLsizei,
    transpose: GLboolean,
    value: ConstPtr<GLfloat>,
) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let slice = mem.bytes_at(value.cast(), (count as GuestUSize) * 48);
        gles.UniformMatrix3x4fv(location, count, transpose, slice.as_ptr().cast())
    });
}

fn glUniformMatrix4x3fv(
    env: &mut Environment,
    location: GLint,
    count: GLsizei,
    transpose: GLboolean,
    value: ConstPtr<GLfloat>,
) {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let slice = mem.bytes_at(value.cast(), (count as GuestUSize) * 48);
        gles.UniformMatrix4x3fv(location, count, transpose, slice.as_ptr().cast())
    });
}

// -- Uniform blocks --
fn glGetUniformBlockIndex(
    env: &mut Environment,
    program: GLuint,
    uniform_block_name: ConstPtr<u8>,
) -> GLuint {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let name_cstr = mem.cstr_at(uniform_block_name);
        let name_c = std::ffi::CString::new(name_cstr).unwrap();
        gles.GetUniformBlockIndex(program, name_c.as_ptr())
    })
}

fn glUniformBlockBinding(
    env: &mut Environment,
    program: GLuint,
    uniform_block_index: GLuint,
    uniform_block_binding: GLuint,
) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.UniformBlockBinding(program, uniform_block_index, uniform_block_binding)
    });
}

// -- Sync objects (OpenGL ES 3.0 core / GL_APPLE_sync) --
//
// Host `GLsync` values are pointers, so they can't be handed to a 32-bit
// guest directly (truncating them, as this code used to, makes distinct
// syncs collide and hands the driver garbage pointers on 64-bit hosts).
// Instead the guest gets a small opaque handle that indexes
// `State::sync_objects`.
//
// Backends with native sync objects (`GLES::supports_sync_objects`, i.e.
// the ES 3.0 native and GL 3.3 Core backends, which also serve ES 2.0
// contexts on desktop) get a real host fence. On the other backends (the
// ES 1.1 backends and the native ES 2.0 driver) the fence is emulated: we
// `glFinish()` when the fence is inserted, so it is already signalled by the
// time the guest can query it. That's a valid implementation of the spec
// (the GPU merely "finished quickly"), and every later wait/query is exact.

const GL_SYNC_FLUSH_COMMANDS_BIT: GLbitfield = 0x0000_0001;
const GL_MAX_SERVER_WAIT_TIMEOUT: GLenum = 0x9111;
const GL_OBJECT_TYPE: GLenum = 0x9112;
const GL_SYNC_CONDITION: GLenum = 0x9113;
const GL_SYNC_STATUS: GLenum = 0x9114;
const GL_SYNC_FLAGS: GLenum = 0x9115;
const GL_SYNC_FENCE: GLenum = 0x9116;
const GL_SYNC_GPU_COMMANDS_COMPLETE: GLenum = 0x9117;
const GL_UNSIGNALED: GLenum = 0x9118;
const GL_SIGNALED: GLenum = 0x9119;
const GL_ALREADY_SIGNALED: GLenum = 0x911A;
const GL_TIMEOUT_EXPIRED: GLenum = 0x911B;
const GL_CONDITION_SATISFIED: GLenum = 0x911C;
const GL_WAIT_FAILED: GLenum = 0x911D;
const GL_TIMEOUT_IGNORED: u64 = u64::MAX;

/// Guest-visible `GLsync` handle (a 32-bit opaque value; 0 is "no sync").
type GuestGLsync = GLuint;

/// Book-keeping for one guest `GLsync`. See the comment above.
#[derive(Debug, Clone, Copy)]
pub(super) struct GuestSyncObject {
    /// Host `GLsync`, or `None` for an emulated fence.
    host: Option<usize>,
    /// For emulated (`host == None`) fences: whether the fence should now be
    /// reported as signalled. A freshly inserted emulated fence starts *not*
    /// signalled and flips to signalled on the first zero-timeout poll (or
    /// immediately on a blocking wait). This one-poll latency mimics real GPU
    /// fences and is required by guests that poll a just-inserted fence to
    /// throttle frames (e.g. cocos2d v3's `-[CCGLView presentFrame]`, which
    /// expects the newest fence to read "not complete" so its completion loop
    /// terminates). Guests that busy-wait for completion still make progress
    /// because the second poll reports signalled.
    emulated_ready: bool,
}

fn has_current_ctx(env: &mut Environment) -> bool {
    env.framework_state
        .opengles
        .current_ctx_for_thread(env.current_thread)
        .is_some()
}

fn lookup_sync(env: &Environment, sync: GuestGLsync) -> Option<GuestSyncObject> {
    env.framework_state.opengles.sync_objects.get(&sync).copied()
}

/// Observe an emulated (`host == None`) fence, returning whether it is now
/// signalled. A freshly inserted emulated fence reads "not yet signalled" the
/// first time it is observed and "signalled" on every observation after that
/// (see [GuestSyncObject::emulated_ready]). `blocking` observations (a wait
/// with a non-zero timeout) report the fence as signalled immediately, because
/// the queued work was already flushed synchronously at creation time.
fn observe_emulated_fence(env: &mut Environment, sync: GuestGLsync, blocking: bool) -> bool {
    if let Some(object) = env.framework_state.opengles.sync_objects.get_mut(&sync) {
        if object.emulated_ready {
            return true;
        }
        if blocking {
            object.emulated_ready = true;
            return true;
        }
        // First non-blocking poll: latch the fence so the next poll succeeds,
        // but report "not yet signalled" this time.
        object.emulated_ready = true;
        return false;
    }
    false
}

fn glFenceSync(env: &mut Environment, condition: GLenum, flags: GLbitfield) -> GuestGLsync {
    if condition != GL_SYNC_GPU_COMMANDS_COMPLETE || flags != 0 {
        // The spec mandates GL_INVALID_ENUM / GL_INVALID_VALUE and a 0 return.
        log!(
            "Warning: glFenceSync(condition={:#x}, flags={:#x}) called with invalid arguments",
            condition,
            flags
        );
        return 0;
    }
    if !has_current_ctx(env) {
        return 0;
    }
    // We emulate fences without real host sync objects: complete the queued
    // host work synchronously here, then report the fence as signalled only
    // after it has been observed once (see `emulated_ready`). The one-poll
    // latency is what guests like cocos2d v3 rely on to throttle frames; a
    // fence that reads "already signalled" on the very first poll makes
    // `-[CCGLView presentFrame]`'s completion loop spin forever.
    let host = with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.Finish();
        Some(None)
    });
    let Some(host) = host else {
        return 0;
    };
    let state = &mut env.framework_state.opengles;
    let handle = loop {
        let candidate = state.next_sync_handle.max(1);
        state.next_sync_handle = candidate.wrapping_add(1);
        if !state.sync_objects.contains_key(&candidate) {
            break candidate;
        }
    };
    state.sync_objects.insert(
        handle,
        GuestSyncObject {
            host,
            emulated_ready: false,
        },
    );
    handle
}

fn glIsSync(env: &mut Environment, sync: GuestGLsync) -> GLboolean {
    if sync != 0 && lookup_sync(env, sync).is_some() {
        gles11::TRUE
    } else {
        gles11::FALSE
    }
}

fn glDeleteSync(env: &mut Environment, sync: GuestGLsync) {
    if sync == 0 {
        // Silently ignored, per spec.
        return;
    }
    let Some(object) = env.framework_state.opengles.sync_objects.remove(&sync) else {
        log!("Warning: glDeleteSync({:#x}): not a sync object", sync);
        return;
    };
    if let Some(host) = object.host {
        with_ctx_and_mem(env, |gles, _mem| unsafe { gles.DeleteSync(host) });
    }
}

fn client_wait_sync_impl(
    env: &mut Environment,
    sync: GuestGLsync,
    flags: GLbitfield,
    timeout: u64,
) -> GLenum {
    let Some(object) = lookup_sync(env, sync) else {
        log!("Warning: glClientWaitSync({:#x}): not a sync object", sync);
        return GL_WAIT_FAILED;
    };
    if flags & !GL_SYNC_FLUSH_COMMANDS_BIT != 0 {
        log!("Warning: glClientWaitSync(): invalid flags {:#x}", flags);
        return GL_WAIT_FAILED;
    }
    match object.host {
        Some(host) => {
            let res = with_ctx_and_mem(env, |gles, _mem| unsafe {
                gles.ClientWaitSync(host, flags, timeout)
            });
            // `with_ctx_and_mem` yields 0 if no context is current.
            if res == 0 {
                GL_WAIT_FAILED
            } else {
                res
            }
        }
        None => {
            // Emulated fence: report signalled only after it has been observed
            // once (or immediately for a blocking wait). This one-poll latency
            // keeps cocos2d v3's -[CCGLView presentFrame] completion loop from
            // spinning forever on the just-inserted fence.
            if observe_emulated_fence(env, sync, /* blocking: */ timeout != 0) {
                GL_ALREADY_SIGNALED
            } else {
                GL_TIMEOUT_EXPIRED
            }
        }
    }
}

fn glClientWaitSync(
    env: &mut Environment,
    sync: GuestGLsync,
    flags: GLbitfield,
    timeout_lo: GLuint,
    timeout_hi: GLuint,
) -> GLenum {
    let timeout = ((timeout_hi as u64) << 32) | (timeout_lo as u64);
    client_wait_sync_impl(env, sync, flags, timeout)
}

fn wait_sync_impl(env: &mut Environment, sync: GuestGLsync, flags: GLbitfield, timeout: u64) {
    let Some(object) = lookup_sync(env, sync) else {
        log!("Warning: glWaitSync({:#x}): not a sync object", sync);
        return;
    };
    if flags != 0 || timeout != GL_TIMEOUT_IGNORED {
        log!(
            "Warning: glWaitSync(): invalid flags {:#x} / timeout {:#x}",
            flags,
            timeout
        );
        return;
    }
    // Emulated fences are signalled on creation, so a server-side wait on
    // them has nothing to wait for.
    if let Some(host) = object.host {
        with_ctx_and_mem(env, |gles, _mem| unsafe {
            gles.WaitSync(host, flags, timeout)
        });
    }
}

fn glWaitSync(
    env: &mut Environment,
    sync: GuestGLsync,
    flags: GLbitfield,
    timeout_lo: GLuint,
    timeout_hi: GLuint,
) {
    let timeout = ((timeout_hi as u64) << 32) | (timeout_lo as u64);
    wait_sync_impl(env, sync, flags, timeout)
}

/// `void glGetSynciv(GLsync sync, GLenum pname, GLsizei bufSize,
///                   GLsizei *length, GLint *values)`
fn glGetSynciv(
    env: &mut Environment,
    sync: GuestGLsync,
    pname: GLenum,
    buf_size: GLsizei,
    length: MutPtr<GLsizei>,
    values: MutPtr<GLint>,
) {
    let Some(object) = lookup_sync(env, sync) else {
        log!("Warning: glGetSynciv({:#x}): not a sync object", sync);
        return;
    };
    let value: GLint = match pname {
        GL_OBJECT_TYPE => GL_SYNC_FENCE as GLint,
        GL_SYNC_CONDITION => GL_SYNC_GPU_COMMANDS_COMPLETE as GLint,
        GL_SYNC_FLAGS => 0,
        GL_SYNC_STATUS => match object.host {
            Some(host) => {
                let mut status: GLint = 0;
                let mut written: GLsizei = 0;
                with_ctx_and_mem(env, |gles, _mem| unsafe {
                    gles.GetSynciv(host, pname, 1, &mut written, &mut status)
                });
                if written < 1 {
                    // No context current (or the query failed): nothing to
                    // report, mirror GL by leaving the outputs untouched.
                    return;
                }
                status
            }
            None => {
                // Emulated fence: mirror the one-poll latency used by
                // glClientWaitSync so a status poll of a just-inserted fence
                // reads unsignalled once before flipping to signalled.
                if observe_emulated_fence(env, sync, /* blocking: */ false) {
                    GL_SIGNALED as GLint
                } else {
                    GL_UNSIGNALED as GLint
                }
            }
        },
        _ => {
            log!("Warning: glGetSynciv(): invalid pname {:#x}", pname);
            return;
        }
    };
    let written: GLsizei = if buf_size >= 1 && !values.is_null() {
        env.mem.write(values, value);
        1
    } else {
        0
    };
    if !length.is_null() {
        env.mem.write(length, written);
    }
}

// GL_APPLE_sync entry points (iOS 6+). Same semantics and enum values as the
// ES 3.0 core functions above.
fn glFenceSyncAPPLE(env: &mut Environment, condition: GLenum, flags: GLbitfield) -> GuestGLsync {
    glFenceSync(env, condition, flags)
}
fn glIsSyncAPPLE(env: &mut Environment, sync: GuestGLsync) -> GLboolean {
    glIsSync(env, sync)
}
fn glDeleteSyncAPPLE(env: &mut Environment, sync: GuestGLsync) {
    glDeleteSync(env, sync)
}
fn glClientWaitSyncAPPLE(
    env: &mut Environment,
    sync: GuestGLsync,
    flags: GLbitfield,
    timeout_lo: GLuint,
    timeout_hi: GLuint,
) -> GLenum {
    glClientWaitSync(env, sync, flags, timeout_lo, timeout_hi)
}
fn glWaitSyncAPPLE(
    env: &mut Environment,
    sync: GuestGLsync,
    flags: GLbitfield,
    timeout_lo: GLuint,
    timeout_hi: GLuint,
) {
    glWaitSync(env, sync, flags, timeout_lo, timeout_hi)
}
fn glGetSyncivAPPLE(
    env: &mut Environment,
    sync: GuestGLsync,
    pname: GLenum,
    buf_size: GLsizei,
    length: MutPtr<GLsizei>,
    values: MutPtr<GLint>,
) {
    glGetSynciv(env, sync, pname, buf_size, length, values)
}

/// `void glGetInteger64vAPPLE(GLenum pname, GLint64 *params)`
///
/// GL_APPLE_sync adds this so `GL_MAX_SERVER_WAIT_TIMEOUT_APPLE` can be
/// queried, but it accepts every `glGetIntegerv` pname too. Backends without
/// a native `glGetInteger64v` are served by widening `glGetIntegerv`.
fn glGetInteger64vAPPLE(env: &mut Environment, pname: GLenum, params: MutPtr<i64>) {
    if params.is_null() || !has_current_ctx(env) {
        return;
    }
    let native = with_ctx_and_mem(env, |gles, _mem| gles.supports_sync_objects());
    if native {
        glGetInteger64v(env, pname, params);
        return;
    }
    if pname == GL_MAX_SERVER_WAIT_TIMEOUT {
        // Emulated server waits never block, so there is no meaningful
        // upper bound; 0 is an allowed value for this limit.
        env.mem.write(params, 0i64);
        return;
    }
    let count = get_integer_value_count(pname);
    let tmp: MutPtr<GLint> = env
        .mem
        .alloc((count.max(16) * 4) as GuestUSize)
        .cast();
    glGetIntegerv(env, pname, tmp);
    for i in 0..count {
        let value: GLint = env.mem.read(tmp + i as GuestUSize);
        env.mem.write(params + i as GuestUSize, value as i64);
    }
    env.mem.free(tmp.cast());
}

/// Number of values `glGetIntegerv(pname, ...)` writes, for the pnames that
/// return more than one.
fn get_integer_value_count(pname: GLenum) -> u32 {
    match pname {
        gles11::COMPRESSED_TEXTURE_FORMATS => SUPPORTED_COMPRESSED_TEXTURE_FORMATS.len() as u32,
        0x0BA6 | 0x0BA7 | 0x0BA8 => 16, // MODELVIEW/PROJECTION/TEXTURE_MATRIX
        0x0BA2 | 0x0C10 | 0x0C22 | 0x0C23 | 0x8005 | 0x0B00 | 0x0B03 | 0x0B66 | 0x0B53 => {
            // VIEWPORT, SCISSOR_BOX, COLOR_CLEAR_VALUE, COLOR_WRITEMASK,
            // BLEND_COLOR, CURRENT_COLOR, CURRENT_TEXTURE_COORDS, FOG_COLOR,
            // LIGHT_MODEL_AMBIENT
            4
        }
        0x0B02 => 3, // CURRENT_NORMAL
        0x0D3A | 0x846D | 0x846E | 0x0B12 | 0x0B22 | 0x0B70 => {
            // MAX_VIEWPORT_DIMS, ALIASED_POINT_SIZE_RANGE,
            // ALIASED_LINE_WIDTH_RANGE, SMOOTH_POINT_SIZE_RANGE,
            // SMOOTH_LINE_WIDTH_RANGE, DEPTH_RANGE
            2
        }
        _ => 1,
    }
}

// -- Misc --
fn glGetStringi(env: &mut Environment, name: GLenum, index: GLuint) -> ConstPtr<u8> {
    let host_ptr: *const GLubyte =
        with_ctx_and_mem(env, |gles, _mem| unsafe { gles.GetStringi(name, index) });
    if host_ptr.is_null() {
        return Ptr::null();
    }
    // Copy the C string into guest memory once and intern the pointer in
    // EAGL's per-context string cache.
    let bytes = unsafe { std::ffi::CStr::from_ptr(host_ptr as *const _) }.to_bytes_with_nul();
    let guest_buf: MutPtr<u8> = env.mem.alloc(bytes.len() as GuestUSize).cast();
    env.mem
        .bytes_at_mut(guest_buf, bytes.len() as GuestUSize)
        .copy_from_slice(bytes);
    guest_buf.cast_const()
}

fn glGetFragDataLocation(env: &mut Environment, program: GLuint, name: ConstPtr<u8>) -> GLint {
    with_ctx_and_mem(env, |gles, mem| unsafe {
        let name_cstr = mem.cstr_at(name);
        let name_c = std::ffi::CString::new(name_cstr).unwrap();
        gles.GetFragDataLocation(program, name_c.as_ptr())
    })
}

fn glProgramParameteri(env: &mut Environment, program: GLuint, pname: GLenum, value: GLint) {
    with_ctx_and_mem(env, |gles, _mem| unsafe {
        gles.ProgramParameteri(program, pname, value)
    });
}

/// Work around the fog-division-by-zero problem: with linear fog and
/// `GL_FOG_START == GL_FOG_END`, the fog factor `(end - z) / (end - start)`
/// is NaN. Apple's PowerVR driver tolerates that, desktop GL drivers turn it
/// into garbage (black or fully fogged geometry), so nudge the range apart
/// for the duration of the draw.
///
/// PERF: this used to query `GL_FOG`, `GL_FOG_START` and `GL_FOG_END` from the
/// driver (plus two `glGetError()` round-trips to swallow the errors strict
/// drivers raise) on every single draw call. The guest is the only thing that
/// can change that state, so it's answered from the [GLShadowState] mirror
/// now, which costs nothing when fog is off (the overwhelmingly common case).
unsafe fn clamp_fog_state_values(
    gles: &mut dyn GLES,
    shadow: &GLShadowState,
) -> Option<(f32, f32)> {
    if !shadow.fog_enabled {
        return None;
    }
    let (fog_start, fog_end) = (shadow.fog_start, shadow.fog_end);
    if fog_start == fog_end {
        let new_fog_start = fog_end - 0.001;
        gles.Fogf(gles11::FOG_START, new_fog_start);
        return Some((fog_start, fog_end));
    }
    None
}
unsafe fn restore_fog_state_values(gles: &mut dyn GLES, from_backup: Option<(f32, f32)>) {
    if let Some((fog_start, fog_end)) = from_backup {
        gles.Fogf(gles11::FOG_START, fog_start);
        gles.Fogf(gles11::FOG_END, fog_end);
    }
}

/// `void glLabelObjectEXT(GLenum type, GLuint object, GLsizei length, const GLchar *label)`
///
/// Part of GL_EXT_debug_label.  Labels an OpenGL ES object for debugging
/// purposes.  The label is used only by GPU debugging tools (Instruments,
/// RenderDoc) and has no effect on rendering.  We expose a no-op
/// implementation so that apps which unconditionally call this extension
/// function no longer trigger the "unimplemented function" warning.
///
/// Reference: <https://registry.khronos.org/OpenGL/extensions/EXT/EXT_debug_label.txt>
fn glLabelObjectEXT(
    _env: &mut Environment,
    _type_: GLenum,
    _object: GLuint,
    _length: GLsizei,
    _label: ConstPtr<u8>,
) {
    // No-op: debug labels have no functional impact.
}

/// `void glGetObjectLabelEXT(GLenum type, GLuint object, GLsizei bufSize,
///                            GLsizei *length, GLchar *label)`
///
/// Retrieves the debug label previously set by glLabelObjectEXT.  Since we
/// do not store labels, we return an empty string.
///
/// Reference: <https://registry.khronos.org/OpenGL/extensions/EXT/EXT_debug_label.txt>
fn glGetObjectLabelEXT(
    env: &mut Environment,
    _type_: GLenum,
    _object: GLuint,
    buf_size: GLsizei,
    length: MutPtr<GLsizei>,
    label: MutPtr<u8>,
) {
    if !label.is_null() && buf_size > 0 {
        env.mem.write(label, 0u8);
    }
    if !length.is_null() {
        env.mem.write(length, 0);
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
    export_c_func!(glGetFixedv(_, _)),
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
    export_c_func!(glGetTexParameterfv(_, _, _)),
    export_c_func!(glGetTexParameterxv(_, _, _)),
    export_c_func!(glGetTexEnvxv(_, _, _)),
    export_c_func!(glGetClipPlanef(_, _)),
    export_c_func!(glGetClipPlanex(_, _)),
    export_c_func!(glGetLightfv(_, _, _)),
    export_c_func!(glGetLightxv(_, _, _)),
    export_c_func!(glGetMaterialfv(_, _, _)),
    export_c_func!(glGetMaterialxv(_, _, _)),
    export_c_func!(glCompressedTexSubImage2D(_, _, _, _, _, _, _, _, _)),
    export_c_func!(glDrawTexfOES(_, _, _, _, _)),
    export_c_func!(glDrawTexiOES(_, _, _, _, _)),
    export_c_func!(glDrawTexxOES(_, _, _, _, _)),
    export_c_func!(glDrawTexfvOES(_)),
    export_c_func!(glDrawTexivOES(_)),
    export_c_func!(glDrawTexxvOES(_)),
    export_c_func!(glDrawTexsOES(_, _, _, _, _)),
    export_c_func!(glDrawTexsvOES(_)),
    export_c_func!(glRenderbufferStorageMultisampleAPPLE(_, _, _, _, _)),
    export_c_func!(glResolveMultisampleFramebufferAPPLE()),
    export_c_func!(glDiscardFramebufferEXT(_, _, _)),
    export_c_func!(glPushGroupMarkerEXT(_, _)),
    export_c_func!(glPopGroupMarkerEXT()),
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
    export_c_func!(glGetShaderSource(_, _, _, _)),
    export_c_func!(glGetProgramiv(_, _, _)),
    export_c_func!(glGetProgramInfoLog(_, _, _, _)),
    export_c_func!(glGetActiveUniform(_, _, _, _, _, _, _)),
    export_c_func!(glGetActiveAttrib(_, _, _, _, _, _, _)),
    export_c_func!(glShaderSource(_, _, _, _)),
    export_c_func!(glEnableVertexAttribArray(_)),
    export_c_func!(glDisableVertexAttribArray(_)),
    export_c_func!(glVertexAttribPointer(_, _, _, _, _, _)),
    export_c_func!(glVertexAttrib1f(_, _)),
    export_c_func!(glVertexAttrib1fv(_, _)),
    export_c_func!(glVertexAttrib2f(_, _, _)),
    export_c_func!(glVertexAttrib2fv(_, _)),
    export_c_func!(glVertexAttrib3f(_, _, _, _)),
    export_c_func!(glVertexAttrib3fv(_, _)),
    export_c_func!(glVertexAttrib4f(_, _, _, _, _)),
    export_c_func!(glVertexAttrib4fv(_, _)),
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
    // `GL_OES_blend_equation_separate` / `GL_OES_blend_func_separate`
    // aliases: identical behaviour, just the extension-suffixed names.
    export_c_func_aliased!("glBlendEquationSeparateOES", glBlendEquationSeparate(_, _)),
    export_c_func_aliased!("glBlendFuncSeparateOES", glBlendFuncSeparate(_, _, _, _)),
    export_c_func!(glShaderBinary(_, _, _, _, _)),
    export_c_func!(glGetActiveUniformsiv(_, _, _, _, _)),
    export_c_func!(glGetActiveUniformBlockiv(_, _, _, _)),
    export_c_func!(glGetActiveUniformBlockName(_, _, _, _, _)),
    export_c_func!(glProgramBinary(_, _, _, _)),
    export_c_func!(glGetProgramBinary(_, _, _, _, _)),
    export_c_func!(glStencilFuncSeparate(_, _, _, _)),
    export_c_func!(glStencilOpSeparate(_, _, _, _)),
    export_c_func!(glStencilMaskSeparate(_, _)),
    export_c_func!(glGenVertexArrays(_, _)),
    export_c_func!(glBindVertexArray(_)),
    export_c_func!(glDeleteVertexArrays(_, _)),
    export_c_func!(glIsVertexArray(_)),
    // OpenGL ES 3.0 entry points
    export_c_func!(glUnmapBuffer(_)),
    export_c_func_aliased!("glMapBufferRangeEXT", glMapBufferRange(_, _, _, _)),
    export_c_func!(glMapBufferRange(_, _, _, _)),
    export_c_func_aliased!(
        "glFlushMappedBufferRangeEXT",
        glFlushMappedBufferRange(_, _, _)
    ),
    export_c_func!(glFlushMappedBufferRange(_, _, _)),
    export_c_func!(glCopyBufferSubData(_, _, _, _, _)),
    export_c_func!(glBindBufferBase(_, _, _)),
    export_c_func!(glBindBufferRange(_, _, _, _, _)),
    export_c_func!(glDrawRangeElements(_, _, _, _, _, _)),
    export_c_func!(glDrawArraysInstanced(_, _, _, _)),
    export_c_func!(glDrawElementsInstanced(_, _, _, _, _)),
    export_c_func!(glVertexAttribDivisor(_, _)),
    export_c_func!(glReadBuffer(_)),
    export_c_func!(glDrawBuffers(_, _)),
    export_c_func!(glClearBufferiv(_, _, _)),
    export_c_func!(glClearBufferuiv(_, _, _)),
    export_c_func!(glClearBufferfv(_, _, _)),
    export_c_func!(glClearBufferfi(_, _, _, _)),
    export_c_func!(glBlitFramebuffer(_, _, _, _, _, _, _, _, _, _)),
    export_c_func!(glRenderbufferStorageMultisample(_, _, _, _, _)),
    export_c_func!(glFramebufferTextureLayer(_, _, _, _, _)),
    export_c_func!(glInvalidateFramebuffer(_, _, _)),
    export_c_func!(glInvalidateSubFramebuffer(_, _, _, _, _, _, _)),
    export_c_func!(glTexImage3D(_, _, _, _, _, _, _, _, _, _)),
    export_c_func!(glTexSubImage3D(_, _, _, _, _, _, _, _, _, _, _)),
    export_c_func!(glCopyTexSubImage3D(_, _, _, _, _, _, _, _, _)),
    export_c_func!(glTexStorage2D(_, _, _, _, _)),
    export_c_func!(glTexStorage2DEXT(_, _, _, _, _)),
    export_c_func!(glTexStorage3D(_, _, _, _, _, _)),
    export_c_func!(glGenQueries(_, _)),
    export_c_func!(glDeleteQueries(_, _)),
    export_c_func!(glIsQuery(_)),
    export_c_func!(glBeginQuery(_, _)),
    export_c_func!(glEndQuery(_)),
    export_c_func!(glGetQueryiv(_, _, _)),
    export_c_func!(glGetQueryObjectuiv(_, _, _)),
    // GL_EXT_occlusion_query_boolean (OpenGL ES 2.0 boolean occlusion queries)
    export_c_func!(glGenQueriesEXT(_, _)),
    export_c_func!(glDeleteQueriesEXT(_, _)),
    export_c_func!(glIsQueryEXT(_)),
    export_c_func!(glBeginQueryEXT(_, _)),
    export_c_func!(glEndQueryEXT(_)),
    export_c_func!(glGetQueryivEXT(_, _, _)),
    export_c_func!(glGetQueryObjectuivEXT(_, _, _)),
    export_c_func!(glGenSamplers(_, _)),
    export_c_func!(glDeleteSamplers(_, _)),
    export_c_func!(glIsSampler(_)),
    export_c_func!(glBindSampler(_, _)),
    export_c_func!(glSamplerParameteri(_, _, _)),
    export_c_func!(glSamplerParameterf(_, _, _)),
    export_c_func!(glBeginTransformFeedback(_)),
    export_c_func!(glEndTransformFeedback()),
    export_c_func!(glBindTransformFeedback(_, _)),
    export_c_func!(glDeleteTransformFeedbacks(_, _)),
    export_c_func!(glGenTransformFeedbacks(_, _)),
    export_c_func!(glIsTransformFeedback(_)),
    export_c_func!(glPauseTransformFeedback()),
    export_c_func!(glResumeTransformFeedback()),
    export_c_func!(glVertexAttribIPointer(_, _, _, _, _)),
    export_c_func!(glVertexAttribI4i(_, _, _, _, _)),
    export_c_func!(glVertexAttribI4ui(_, _, _, _, _)),
    export_c_func!(glUniform1ui(_, _)),
    export_c_func!(glUniform2ui(_, _, _)),
    export_c_func!(glUniform3ui(_, _, _, _)),
    export_c_func!(glUniform4ui(_, _, _, _, _)),
    export_c_func!(glUniform1uiv(_, _, _)),
    export_c_func!(glUniform2uiv(_, _, _)),
    export_c_func!(glUniform3uiv(_, _, _)),
    export_c_func!(glUniform4uiv(_, _, _)),
    export_c_func!(glUniformMatrix2x3fv(_, _, _, _)),
    export_c_func!(glUniformMatrix3x2fv(_, _, _, _)),
    export_c_func!(glUniformMatrix2x4fv(_, _, _, _)),
    export_c_func!(glUniformMatrix4x2fv(_, _, _, _)),
    export_c_func!(glUniformMatrix3x4fv(_, _, _, _)),
    export_c_func!(glUniformMatrix4x3fv(_, _, _, _)),
    export_c_func!(glGetUniformBlockIndex(_, _)),
    export_c_func!(glUniformBlockBinding(_, _, _)),
    export_c_func!(glFenceSync(_, _)),
    export_c_func!(glIsSync(_)),
    export_c_func!(glDeleteSync(_)),
    export_c_func!(glClientWaitSync(_, _, _, _)),
    export_c_func!(glWaitSync(_, _, _, _)),
    export_c_func!(glGetSynciv(_, _, _, _, _)),
    // GL_APPLE_sync (iOS 6+)
    export_c_func!(glFenceSyncAPPLE(_, _)),
    export_c_func!(glIsSyncAPPLE(_)),
    export_c_func!(glDeleteSyncAPPLE(_)),
    export_c_func!(glClientWaitSyncAPPLE(_, _, _, _)),
    export_c_func!(glWaitSyncAPPLE(_, _, _, _)),
    export_c_func!(glGetSyncivAPPLE(_, _, _, _, _)),
    export_c_func!(glGetInteger64vAPPLE(_, _)),
    export_c_func!(glGetStringi(_, _)),
    export_c_func!(glGetFragDataLocation(_, _)),
    export_c_func!(glProgramParameteri(_, _, _)),
    // OpenGL ES 3.0 entry points whose backend implementations already exist
    // but were missing from the guest export table (caused "unhandled
    // external relocation" warnings for GL ES 3.0 apps such as Marmalade/Life).
    export_c_func!(glCompressedTexImage3D(_, _, _, _, _, _, _, _, _)),
    export_c_func!(glCompressedTexSubImage3D(_, _, _, _, _, _, _, _, _, _, _)),
    export_c_func!(glSamplerParameteriv(_, _, _)),
    export_c_func!(glSamplerParameterfv(_, _, _)),
    export_c_func!(glGetSamplerParameteriv(_, _, _)),
    export_c_func!(glGetSamplerParameterfv(_, _, _)),
    export_c_func!(glGetInteger64v(_, _)),
    export_c_func!(glGetIntegeri_v(_, _, _)),
    export_c_func!(glGetInteger64i_v(_, _, _)),
    export_c_func!(glGetBufferParameteri64v(_, _, _)),
    export_c_func!(glGetBufferPointerv(_, _, _)),
    export_c_func!(glGetInternalformativ(_, _, _, _, _)),
    export_c_func!(glVertexAttribI4iv(_, _)),
    export_c_func!(glVertexAttribI4uiv(_, _)),
    export_c_func!(glGetVertexAttribfv(_, _, _)),
    export_c_func!(glGetVertexAttribiv(_, _, _)),
    export_c_func!(glGetVertexAttribPointerv(_, _, _)),
    export_c_func!(glGetVertexAttribIiv(_, _, _)),
    export_c_func!(glGetVertexAttribIuiv(_, _, _)),
    export_c_func!(glGetUniformuiv(_, _, _)),
    export_c_func!(glGetUniformIndices(_, _, _, _)),
    export_c_func!(glTransformFeedbackVaryings(_, _, _, _)),
    export_c_func!(glGetTransformFeedbackVarying(_, _, _, _, _, _, _)),
    // GL_EXT_debug_label — debug-label extension, no-op implementations.
    // Reference: <https://registry.khronos.org/OpenGL/extensions/EXT/EXT_debug_label.txt>
    export_c_func!(glLabelObjectEXT(_, _, _, _)),
    export_c_func!(glGetObjectLabelEXT(_, _, _, _, _)),
];

#[cfg(test)]
mod pvrtc_subimage_matching_tests {
    use super::pvrtc_subimage_matches_level;
    use crate::gles::gles11_raw as gles11;

    #[test]
    fn accepts_only_a_full_update_of_the_tracked_pvrtc_level() {
        let level = Some((2048, 2048, gles11::COMPRESSED_RGBA_PVRTC_4BPPV1_IMG));
        assert!(pvrtc_subimage_matches_level(
            level,
            0,
            0,
            2048,
            2048,
            gles11::COMPRESSED_RGBA_PVRTC_4BPPV1_IMG,
        ));
        assert!(!pvrtc_subimage_matches_level(
            level,
            0,
            0,
            1024,
            2048,
            gles11::COMPRESSED_RGBA_PVRTC_4BPPV1_IMG,
        ));
        assert!(!pvrtc_subimage_matches_level(
            level,
            4,
            0,
            2048,
            2048,
            gles11::COMPRESSED_RGBA_PVRTC_4BPPV1_IMG,
        ));
        assert!(!pvrtc_subimage_matches_level(
            level,
            0,
            0,
            2048,
            2048,
            gles11::COMPRESSED_RGB_PVRTC_4BPPV1_IMG,
        ));
        assert!(!pvrtc_subimage_matches_level(
            None,
            0,
            0,
            2048,
            2048,
            gles11::COMPRESSED_RGBA_PVRTC_4BPPV1_IMG,
        ));
    }
}

#[cfg(test)]
mod shader_preprocessor_normalization_tests {
    use super::{normalize_asphalt8_shader_source, normalize_shader_preprocessor_whitespace};

    #[test]
    fn normalizes_asphalt8_multiline_and_numeric_shader_tokens() {
        let src = "#if FOO ||\n BAR\n#endif]\nvec3(1,1,1); vec4(0.5, 0, 0, 0);";
        let out = normalize_asphalt8_shader_source(src);
        assert!(out.contains("FOO ||"));
        assert!(!out.contains("||\n"));
        assert!(out.contains("#endif\n"));
        assert!(out.contains("vec3(1.0, 1.0, 1.0)"));
        assert!(out.contains("vec4(0.5, 0.0, 0.0, 0.0)"));
    }

    #[test]
    fn inserts_space_before_comment_on_endif() {
        let src = "#if defined(FOO)\nvoid main() {}\n#endif//comment\n";
        let out = normalize_shader_preprocessor_whitespace(src);
        assert!(out.contains("#endif //comment"));
    }

    #[test]
    fn inserts_space_before_comment_on_if() {
        let src = "#if defined(FOO)//bar\nvoid main() {}\n#endif\n";
        let out = normalize_shader_preprocessor_whitespace(src);
        assert!(out.contains("#if defined(FOO) //bar"));
    }

    #[test]
    fn strips_stray_carriage_returns() {
        let src = "#if defined(FOO)\r\nvoid main() {}\r\n#endif\r\n";
        let out = normalize_shader_preprocessor_whitespace(src);
        assert!(!out.contains('\r'));
        assert!(out.contains("#if defined(FOO)"));
        assert!(out.contains("#endif"));
    }

    #[test]
    fn leaves_already_spaced_directives_unchanged() {
        let src = "#if defined(FOO) // bar\nvoid main() {}\n#endif // baz\n";
        let out = normalize_shader_preprocessor_whitespace(src);
        assert_eq!(out, src.replace('\r', ""));
    }

    #[test]
    fn does_not_touch_comments_in_body_code() {
        let src = "void main() {\n  float x = 1.0;//no space needed here\n}\n";
        let out = normalize_shader_preprocessor_whitespace(src);
        assert_eq!(out, src);
    }

    #[test]
    fn inserts_space_before_block_comment_on_directive() {
        let src = "#endif/* trailing */\nvoid main() {}\n";
        let out = normalize_shader_preprocessor_whitespace(src);
        assert!(out.contains("#endif /* trailing */"));
    }

    #[test]
    fn normalizes_directive_spanning_concatenated_source_strings() {
        // Guest code often calls glShaderSource with count > 1, splitting the
        // source into several strings. touchHLE concatenates them before
        // normalization; verify that a directive whose trailing `//comment`
        // only becomes adjacent *after* concatenation is still fixed up.
        // Real case: Gameloft's "9mm" glues `#endif//...` at a chunk boundary,
        // which real PowerVR SGX tolerated but desktop/Adreno GLSL rejects with
        // "unexpected tokens following #endif".
        let chunk_a = "#if defined(FOO)\nvoid main() {}\n#endif";
        let chunk_b = "//trailing comment\n";
        let joined = format!("{chunk_a}{chunk_b}");
        let out = normalize_shader_preprocessor_whitespace(&joined);
        assert!(out.contains("#endif //trailing comment"));
        assert!(!out.contains("#endif//trailing comment"));
    }
}

#[cfg(test)]
mod shader_extension_hoisting_tests {
    use super::hoist_shader_extension_directives;

    #[test]
    fn hoists_late_extension_before_code() {
        // Mirrors the Gangstar shader layout that fails on ANGLE with
        // "extension directive must occur before any non-preprocessor tokens".
        let src = "precision mediump float;\nuniform sampler2D t;\n#extension GL_OES_texture_3D : enable\nvarying vec2 vUV;\nvoid main() {}\n";
        let out = hoist_shader_extension_directives(src);
        assert!(
            out.starts_with("#extension GL_OES_texture_3D : enable\n"),
            "hoisted source must start with the extension directive, got: {out}"
        );
        assert!(out.find("#extension").unwrap() < out.find("precision").unwrap());
    }

    #[test]
    fn keeps_version_first_and_normalizes_whitespace() {
        let src = "#version 100\nvoid main() {}\n#  extension GL_OES_standard_derivatives : enable\n";
        let out = hoist_shader_extension_directives(src);
        assert!(
            out.starts_with("#version 100\n#extension GL_OES_standard_derivatives : enable\n"),
            "got: {out}"
        );
    }

    #[test]
    fn leaves_sources_without_extensions_untouched() {
        let src = "void main() { gl_FragColor = vec4(1.0); }\n";
        assert_eq!(hoist_shader_extension_directives(src), src);
    }

    #[test]
    fn does_not_misfire_on_extension_identifiers() {
        let src = "float extensionFlag = 1.0;\nvoid main() {}\n";
        assert_eq!(hoist_shader_extension_directives(src), src);
    }
}

#[cfg(test)]
mod varying_declaration_parsing_tests {
    use super::parse_varying_declarations;

    #[test]
    fn parses_precision_and_declarator_lists() {
        let frag = "precision mediump float;\nvarying lowp vec4 vAlpha;\nvarying vec2 vUV, vUV2;\nvarying highp vec3 vNormal; // lit\nvoid main() {}\n";
        let v = parse_varying_declarations(frag);
        assert!(v.contains(&("vec4".to_string(), "vAlpha".to_string())));
        assert!(v.contains(&("vec2".to_string(), "vUV".to_string())));
        assert!(v.contains(&("vec2".to_string(), "vUV2".to_string())));
        assert!(v.contains(&("vec3".to_string(), "vNormal".to_string())));
        assert_eq!(v.len(), 4);
    }

    #[test]
    fn fragment_only_varyings_detected_as_missing() {
        // The Gangstar case: fragment declares vAlpha, vertex does not.
        let vert = parse_varying_declarations("varying vec2 vUV;\nvoid main() {}\n");
        let frag = parse_varying_declarations("varying vec4 vAlpha;\nvarying vec2 vUV;\nvoid main() {}\n");
        let missing: Vec<_> = frag
            .into_iter()
            .filter(|(_, n)| !vert.iter().any(|(_, vn)| vn == n))
            .collect();
        assert_eq!(missing, vec![("vec4".to_string(), "vAlpha".to_string())]);
    }

    #[test]
    fn commented_out_varyings_are_ignored() {
        // Gameloft's generator emits the full varying block in both stages
        // but comments unused entries out; these must not count as declared.
        let vert = parse_varying_declarations(
            "varying vec2 vUV;\n/* varying float vAlpha; */\n// varying vec3 vNormal;\nvoid main() {}\n",
        );
        assert_eq!(vert, vec![("vec2".to_string(), "vUV".to_string())]);
    }

    #[test]
    fn multiple_declarations_per_line_and_mid_line() {
        let src = "varying vec2 vUV, vUV2; varying float vAlpha;\nuniform varying_less;\nvoid main() { varying_not_keyword(); }\n";
        let v = parse_varying_declarations(src);
        assert!(v.contains(&("vec2".to_string(), "vUV".to_string())));
        assert!(v.contains(&("vec2".to_string(), "vUV2".to_string())));
        assert!(v.contains(&("float".to_string(), "vAlpha".to_string())));
        assert!(!v.iter().any(|(_, n)| n == "varying_not_keyword"));
        assert_eq!(v.len(), 3);
    }
}

#[cfg(test)]
mod uniform_array_reconciliation_tests {
    use super::{
        merged_struct_body, parse_struct_definitions, parse_uniform_array_declarations,
        rewrite_uniform_array_sizes,
    };

    #[test]
    fn parses_array_sizes_and_rewrites_both_stages_to_max() {
        let vertex = "uniform vec4 light[4];\nuniform float pad;\nvoid main() {}\n";
        let fragment =
            "precision mediump float;\nuniform highp vec4 light[2];\nvoid main() {}\n";
        let defines = std::collections::HashMap::new();
        let vd = parse_uniform_array_declarations(vertex, &defines);
        let fd = parse_uniform_array_declarations(fragment, &defines);
        assert_eq!(vd.len(), 1);
        assert_eq!(fd.len(), 1);
        assert_eq!(vd[0].name, "light");
        assert_eq!(vd[0].size, Some(4));
        assert_eq!(fd[0].name, "light");
        assert_eq!(fd[0].size, Some(2));

        let fixes = vec![("light".to_string(), 4u32)];
        let patched_vertex = rewrite_uniform_array_sizes(vertex, &vd, &fixes);
        let patched_fragment = rewrite_uniform_array_sizes(fragment, &fd, &fixes);
        assert!(patched_vertex.contains("light[4]"), "{patched_vertex}");
        assert!(patched_fragment.contains("light[4]"), "{patched_fragment}");
        assert!(!patched_fragment.contains("light[2]"), "{patched_fragment}");
        // Untouched declarations stay as they were.
        assert!(patched_vertex.contains("uniform float pad;"));
    }

    #[test]
    fn non_array_uniforms_and_commented_ones_are_ignored() {
        let raw = "uniform vec4 nolit;\nuniform float lit2;\n\
                   /* uniform vec4 light[3]; */\nvoid main() {}\n";
        // Callers strip comments before parsing (as the link fix-up does).
        let src = super::strip_glsl_comments(raw);
        let defines = std::collections::HashMap::new();
        assert!(parse_uniform_array_declarations(&src, &defines).is_empty());
    }

    #[test]
    fn struct_definitions_are_parsed_and_merged() {
        let v = "struct Light { vec4 pos; float r; };\n\
                 uniform Light light[MAX_LIGHT];\nvoid main() {}\n";
        let f = "struct Light { vec4 pos; float r; vec3 color; };\n\
                 uniform Light light[MAX_LIGHT];\nvoid main() {}\n";
        let sv = parse_struct_definitions(v);
        let sf = parse_struct_definitions(f);
        assert_eq!(sv.len(), 1);
        assert_eq!(sf.len(), 1);
        assert_eq!(sv[0].name, "Light");
        assert_eq!(sv[0].members.len(), 2);
        assert_eq!(sf[0].members.len(), 3);
        let merged = merged_struct_body(&sv[0], &sf[0]).unwrap();
        assert_eq!(merged, "vec4 pos; float r; vec3 color;");
        // Reverse order: union follows the first struct's order.
        let merged2 = merged_struct_body(&sf[0], &sv[0]).unwrap();
        assert_eq!(merged2, "vec4 pos; float r; vec3 color;");
        // Conflicting types for the same member abort the merge.
        let conflict = parse_struct_definitions("struct Light { vec4 pos; int r; };\n");
        assert!(merged_struct_body(&sv[0], &conflict[0]).is_none());
    }

    #[test]
    fn struct_spans_cover_braces_for_replacement() {
        let src = "// c\nstruct S { float a; };\nuniform S s[2];\n";
        let defs = parse_struct_definitions(src);
        assert_eq!(defs.len(), 1);
        let (start, end) = defs[0].body_span;
        assert_eq!(&src[start..end], "{ float a; }");
    }

    #[test]
    fn fragment_varying_link_errors_are_parsed_for_angle_and_mesa() {
        let log = "FRAGMENT varying vTexCoord does not match any VERTEX varying\n\
                   error: fragment shader input `vAlpha` has no matching output in the previous stage\n\
                   error: fragment shader input `vAlpha` has no matching output in the previous stage";
        assert_eq!(
            super::parse_driver_reported_varying_names(log),
            vec!["vTexCoord".to_string(), "vAlpha".to_string()]
        );
    }

    #[test]
    fn unrelated_link_errors_do_not_trigger_varying_repair() {
        let log = "error: fragment shader input `vAlpha` has an unsupported type";
        assert!(super::parse_driver_reported_varying_names(log).is_empty());
    }

    #[test]
    fn macro_sized_and_unsized_arrays_are_resolved() {
        let vertex = "#define MAX_LIGHTS 4\nuniform vec4 light[MAX_LIGHTS];\n\
                      void main() {}\n";
        let fragment = "uniform vec4 light[];\nvoid main() {}\n";
        let vdefs = super::collect_int_defines(vertex);
        let fdefs = std::collections::HashMap::new();
        let vd = parse_uniform_array_declarations(vertex, &vdefs);
        let fd = parse_uniform_array_declarations(fragment, &fdefs);
        assert_eq!(vd.len(), 1);
        assert_eq!(vd[0].size, Some(4));
        assert_eq!(fd.len(), 1);
        assert_eq!(fd[0].size, None);
        // The unsized side adopts the resolved size of the other stage.
        let fixes = vec![("light".to_string(), 4u32)];
        let patched = rewrite_uniform_array_sizes(fragment, &fd, &fixes);
        assert!(patched.contains("light[4]"), "{patched}");
    }
}

#[cfg(test)]
mod reversed_attrib_location_tests {
    use super::reversed_attrib_locations;

    #[test]
    fn reverses_driver_order_into_locations_below_the_count() {
        // Noblemen: 1896's deferred light program: `position` and
        // `positionWorld`.
        assert_eq!(reversed_attrib_locations(&[(0, 1), (1, 1)]), [1, 0]);
        // Gaps in the driver's locations are closed up.
        assert_eq!(
            reversed_attrib_locations(&[(0, 1), (6, 1), (2, 1)]),
            [2, 0, 1]
        );
        // A mat4 takes four locations.
        assert_eq!(reversed_attrib_locations(&[(0, 4), (4, 1)]), [1, 0]);
    }
}

#[cfg(test)]
mod performance_texture_size_tests {
    use super::{texture_level_bytes, texture_storage_level_bytes, texture_storage_volume_bytes};
    use crate::gles::gles11_raw as gles11;

    #[test]
    fn estimates_common_texture_storage_formats() {
        assert_eq!(
            texture_level_bytes(10, 10, gles11::RGBA, gles11::UNSIGNED_BYTE),
            400
        );
        assert_eq!(texture_storage_level_bytes(16, 8, 0x8058), 512);
        assert_eq!(
            texture_storage_level_bytes(16, 8, gles11::RGB565_OES),
            256
        );
        assert_eq!(texture_storage_volume_bytes(8, 4, 2, 0x8058), 256);
        assert_eq!(texture_storage_level_bytes(0, 8, 0x8058), 0);
        assert_eq!(texture_storage_level_bytes(16, 8, 0x83F1), 0);
    }
}
