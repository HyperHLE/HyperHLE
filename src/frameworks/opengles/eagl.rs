/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! EAGL.

use super::software_vao::VertexArrayState;
use crate::dyld::{export_c_func, ConstantExports, FunctionExports, HostConstant};
use crate::frameworks::core_animation::ca_eagl_layer::{
    find_fullscreen_eagl_layer, get_pixels_vec_for_presenting, layer_screen_rotation,
    mark_presented_directly, present_pixels,
};
use crate::frameworks::core_graphics::{CGRect, CGSize};
use crate::frameworks::foundation::ns_string::get_static_str;
use crate::frameworks::foundation::NSUInteger;
use crate::frameworks::uikit;
use crate::gles::gles11_raw as gles11; // constants only
use crate::gles::gles11_raw::types::*;
use crate::gles::present::{present_frame, FpsCounter};
use crate::gles::{
    create_gles1_ctx, create_gles2_ctx, create_gles3_ctx, gles1_on_gl2, GLESContext, GLES,
};
use crate::mem::MutPtr;
use crate::objc::{
    id, msg, msg_class, nil, objc_classes, release, retain, ClassExports, HostObject,
};
use crate::options::{Options, PresentMode};
use crate::Environment;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

#[derive(Copy, Clone)]
struct EaglPresentOptions {
    trace_gl_errors: bool,
    present_mode: PresentMode,
    present_finish: bool,
    /// The framebuffer that draws to the window. This is 0 everywhere except
    /// iOS, where SDL2 renders the window through a framebuffer object of its
    /// own (see `EAGLContextHostObject::drawable_framebuffer`).
    window_framebuffer: GLuint,
    /// Present without the device-rotation matrix. Only used by the iOS ES 2.0
    /// direct presenter (see `--ios-es2-direct-present`), whose frames are
    /// already in display orientation.
    skip_rotation: bool,
    /// Rotation, in radians, that Core Animation would have applied to the
    /// presented layer before it reached the screen (its `affineTransform`
    /// plus those of its ancestors). Zero when the layer is not transformed or
    /// when the drawable is not the layer that is being presented as fullscreen.
    layer_rotation: f32,
}

// These are used by the EAGLDrawable protocol implemented by CAEAGLayer.
// Since these have the ABI of constant symbols rather than literal constants,
// the values shouldn't matter, and haven't been checked against real iPhone OS.
pub const kEAGLDrawablePropertyColorFormat: &str = "ColorFormat";
pub const kEAGLDrawablePropertyRetainedBacking: &str = "RetainedBacking";
pub const kEAGLColorFormatRGBA8: &str = "RGBA8";
pub const kEAGLColorFormatRGB565: &str = "RGB565";
/// `kEAGLColorFormatSRGBA8` — sRGB 8888 EAGL color format, added in
/// iOS 7. Apps that supply this string in `drawableProperties` are
/// requesting a sRGB-encoded color renderbuffer (per
/// `EAGLDrawable.h`).
pub const kEAGLColorFormatSRGBA8: &str = "SRGBA8";

pub const CONSTANTS: ConstantExports = &[
    (
        "_kEAGLDrawablePropertyColorFormat",
        HostConstant::NSString(kEAGLDrawablePropertyColorFormat),
    ),
    (
        "_kEAGLDrawablePropertyRetainedBacking",
        HostConstant::NSString(kEAGLDrawablePropertyRetainedBacking),
    ),
    (
        "_kEAGLColorFormatRGBA8",
        HostConstant::NSString(kEAGLColorFormatRGBA8),
    ),
    (
        "_kEAGLColorFormatRGB565",
        HostConstant::NSString(kEAGLColorFormatRGB565),
    ),
    (
        "_kEAGLColorFormatSRGBA8",
        HostConstant::NSString(kEAGLColorFormatSRGBA8),
    ),
];

type EAGLRenderingAPI = u32;
const kEAGLRenderingAPIOpenGLES1: EAGLRenderingAPI = 1;
const kEAGLRenderingAPIOpenGLES2: EAGLRenderingAPI = 2;
const kEAGLRenderingAPIOpenGLES3: EAGLRenderingAPI = 3;

/// Resolve the EAGL rendering API the host should actually create a context
/// for. When `prefer_gles2_context` is set and the app requested ES 1.1, we
/// transparently upgrade to ES 2.0 so apps that ask for ES 1.1 but drive
/// rendering with shader entry points (`glUseProgram`, `glCreateShader`, …)
/// route through the real native ES 2.0 backend instead of falling through
/// to the GLES 1.1-only stubs in `gles_generic`.
fn effective_eagl_api(
    requested: EAGLRenderingAPI,
    prefer_gles2_context: bool,
    force_gles1_context: bool,
) -> EAGLRenderingAPI {
    // Hardcoded driver pin: TOUCHHLE_FORCE_EAGL_API forces the reported/
    // effective EAGL rendering API (1, 2 or 3) regardless of what the guest
    // app requested. This pins the GPU driver surface the app sees, mirroring
    // TOUCHHLE_FORCE_GLES1 on the context-creation side.
    if let Ok(forced) = std::env::var("TOUCHHLE_FORCE_EAGL_API") {
        let forced = forced.trim();
        if let Ok(v) = forced.parse::<EAGLRenderingAPI>() {
            if (1..=3).contains(&v) && v != requested {
                log!(
                    "EAGL: TOUCHHLE_FORCE_EAGL_API active, overriding requested API {} with {}",
                    requested,
                    v
                );
                return v;
            }
        }
    }
    if force_gles1_context && requested != kEAGLRenderingAPIOpenGLES1 {
        log!(
            "EAGL: --force-gles1-context active, downgrading initWithAPI:{} (kEAGLRenderingAPIOpenGLES{}) to kEAGLRenderingAPIOpenGLES1",
            requested,
            requested
        );
        return kEAGLRenderingAPIOpenGLES1;
    }
    if prefer_gles2_context && requested == kEAGLRenderingAPIOpenGLES1 {
        log!(
            "EAGL: --prefer-gles2-context active, upgrading initWithAPI:{} \
             (kEAGLRenderingAPIOpenGLES1) to kEAGLRenderingAPIOpenGLES2",
            requested
        );
        return kEAGLRenderingAPIOpenGLES2;
    }
    requested
}

/// Host-side mirror of a few pieces of guest-visible OpenGL ES state.
///
/// The guest wrappers in [super::gles_guest] used to ask the host driver
/// (`glGetIntegerv`, `glGetBooleanv`, `glGetFloatv`, each followed by a
/// `glGetError()` to swallow the errors strict drivers raise for those
/// queries) on *every* `gl*Pointer` and draw call, just to learn state that
/// only the guest itself can change. On mobile drivers and on ANGLE those
/// round-trips are far from free, and they add up to thousands of extra GL
/// calls per frame in draw-heavy games. Tracking the state here instead makes
/// them disappear.
///
/// Everything in here is per-context (bindings and fixed-function state are
/// context state, not sharegroup state) and is updated by the guest wrappers
/// that change it. Host-side code that touches this state on the app's
/// context (the presenter) always restores what it found, so the mirror stays
/// valid across presents.
pub(super) struct GLShadowState {
    pub(super) resource_group_token: usize,
    /// `GL_ARRAY_BUFFER_BINDING`.
    pub(super) array_buffer: GLuint,
    /// `GL_ELEMENT_ARRAY_BUFFER_BINDING`. `None` means "unknown, ask the
    /// driver": the element array binding is part of vertex array object
    /// state, so it is invalidated whenever the guest switches VAOs.
    pub(super) element_array_buffer: Option<GLuint>,
    /// `glIsEnabled(GL_FOG)`.
    pub(super) fog_enabled: bool,
    /// `GL_FOG_START` / `GL_FOG_END`.
    pub(super) fog_start: f32,
    pub(super) fog_end: f32,
    /// Whether `glEnableVertexAttribArray` was ever called on this context.
    /// Draw-call guards for generic vertex attributes are skipped entirely
    /// for the (very common) fixed-function-only apps that never use them.
    pub(super) generic_attribs_used: bool,
    /// Programs for which the guest app explicitly bound attribute locations
    /// via `glBindAttribLocation` before linking. For these, `glLinkProgram`
    /// must not force-rebind canonical attribute names, because that would
    /// override the app's own vertex layout (e.g. Gameloft's Jet engine).
    pub(super) guest_bound_attribs: HashMap<GLuint, std::collections::HashSet<String>>,
    /// Dimensions and format for PVRTC levels expanded to RGBA storage.
    pub(super) pvrtc_texture_levels: HashMap<(GLenum, GLuint, GLint), (GLsizei, GLsizei, GLenum)>,
    /// PVRTC levels whose RGBA storage has not been materialized yet. Set by
    /// the `glTexStorage2D` PVRTC emulation (immutable storage is skipped when
    /// the host can't take the PVRTC internalformat) and cleared once the
    /// level is first allocated via `glTexImage2D`, so subsequent updates can
    /// use `glTexSubImage2D`.
    pub(super) pvrtc_unallocated_levels: HashSet<(GLenum, GLuint, GLint)>,
    pub(super) current_vertex_array: GLuint,
    pub(super) software_vertex_arrays: HashSet<GLuint>,
    pub(super) software_vao_element_array_buffers: HashMap<GLuint, GLuint>,
    pub(super) next_software_vertex_array: GLuint,
    /// Client active texture unit (`GL_CLIENT_ACTIVE_TEXTURE - GL_TEXTURE0`).
    pub(super) client_active_texture: usize,
    /// Client array state of the currently bound vertex array object. Only
    /// meaningful with the software VAO fallback, see [super::software_vao].
    pub(super) vertex_array_state: VertexArrayState,
    /// Client array state of the other (software) vertex array objects.
    pub(super) software_vao_states: HashMap<GLuint, VertexArrayState>,
}
impl Default for GLShadowState {
    fn default() -> Self {
        GLShadowState {
            resource_group_token: 0,
            array_buffer: 0,
            element_array_buffer: Some(0),
            fog_enabled: false,
            // OpenGL ES 1.1 defaults.
            fog_start: 0.0,
            fog_end: 1.0,
            generic_attribs_used: false,
            guest_bound_attribs: HashMap::new(),
            pvrtc_texture_levels: HashMap::new(),
            pvrtc_unallocated_levels: HashSet::new(),
            current_vertex_array: 0,
            software_vertex_arrays: HashSet::new(),
            software_vao_element_array_buffers: HashMap::new(),
            next_software_vertex_array: 1,
            client_active_texture: 0,
            vertex_array_state: VertexArrayState::default(),
            software_vao_states: HashMap::new(),
        }
    }
}
impl GLShadowState {
    /// Forget everything that is stored in vertex array object state.
    pub(super) fn invalidate_vao_state(&mut self) {
        self.element_array_buffer = None;
    }
    /// Update the mirror after `glDeleteBuffers`: deleting a bound buffer
    /// resets the binding to zero.
    pub(super) fn on_buffers_deleted(&mut self, deleted: GLuint) {
        if deleted == 0 {
            return;
        }
        if self.array_buffer == deleted {
            self.array_buffer = 0;
        }
        if self.element_array_buffer == Some(deleted) {
            self.element_array_buffer = Some(0);
        }
        for (_, buffer) in self.software_vao_element_array_buffers.iter_mut() {
            if *buffer == deleted {
                *buffer = 0;
            }
        }
    }
    pub(super) fn record_pvrtc_texture_level(
        &mut self,
        target: GLenum,
        texture: GLuint,
        level: GLint,
        width: GLsizei,
        height: GLsizei,
        format: GLenum,
    ) {
        self.pvrtc_texture_levels
            .insert((target, texture, level), (width, height, format));
    }
    pub(super) fn pvrtc_texture_level(
        &self,
        target: GLenum,
        texture: GLuint,
        level: GLint,
    ) -> Option<(GLsizei, GLsizei, GLenum)> {
        self.pvrtc_texture_levels
            .get(&(target, texture, level))
            .copied()
    }
    pub(super) fn forget_pvrtc_texture_level(
        &mut self,
        target: GLenum,
        texture: GLuint,
        level: GLint,
    ) {
        self.pvrtc_texture_levels.remove(&(target, texture, level));
        self.pvrtc_unallocated_levels
            .remove(&(target, texture, level));
    }
    pub(super) fn forget_pvrtc_texture(&mut self, texture: GLuint) {
        self.pvrtc_texture_levels
            .retain(|key, _| key.1 != texture);
        self.pvrtc_unallocated_levels.retain(|key| key.1 != texture);
    }
    /// Mark a PVRTC level as not yet materialized: its first upload must use
    /// `glTexImage2D` (which defines the level) instead of
    /// `glTexSubImage2D` (which requires the level to already exist).
    pub(super) fn record_pvrtc_unallocated_level(
        &mut self,
        target: GLenum,
        texture: GLuint,
        level: GLint,
    ) {
        self.pvrtc_unallocated_levels
            .insert((target, texture, level));
    }
    pub(super) fn pvrtc_level_unallocated(
        &self,
        target: GLenum,
        texture: GLuint,
        level: GLint,
    ) -> bool {
        self.pvrtc_unallocated_levels
            .contains(&(target, texture, level))
    }
    pub(super) fn mark_pvrtc_level_allocated(
        &mut self,
        target: GLenum,
        texture: GLuint,
        level: GLint,
    ) {
        self.pvrtc_unallocated_levels
            .remove(&(target, texture, level));
    }
    pub(super) fn record_element_array_buffer_binding(&mut self, buffer: GLuint) {
        self.element_array_buffer = Some(buffer);
        self.software_vao_element_array_buffers
            .insert(self.current_vertex_array, buffer);
    }
    /// Switch the software VAO binding to `array`. Returns the element array
    /// buffer to bind and, if the binding changed, the client array state the
    /// host context is in (the outgoing VAO's), which
    /// [super::software_vao::apply_vertex_array_state] must turn into
    /// [Self::vertex_array_state].
    pub(super) fn bind_software_vertex_array(
        &mut self,
        array: GLuint,
    ) -> (GLuint, Option<VertexArrayState>) {
        if array == self.current_vertex_array {
            return (self.bind_software_element_array_buffer(array), None);
        }
        let incoming = self.software_vao_states.remove(&array).unwrap_or_default();
        let outgoing = std::mem::replace(&mut self.vertex_array_state, incoming);
        self.software_vao_states
            .insert(self.current_vertex_array, outgoing.clone());
        (
            self.bind_software_element_array_buffer(array),
            Some(outgoing),
        )
    }
    fn bind_software_element_array_buffer(&mut self, array: GLuint) -> GLuint {
        if let Some(binding) = self.element_array_buffer {
            self.software_vao_element_array_buffers
                .insert(self.current_vertex_array, binding);
        }
        self.current_vertex_array = array;
        let binding = *self
            .software_vao_element_array_buffers
            .entry(array)
            .or_insert(0);
        self.element_array_buffer = Some(binding);
        binding
    }
    pub(super) fn generate_software_vertex_array(&mut self) -> GLuint {
        let mut name = self.next_software_vertex_array.max(1);
        while self.software_vertex_arrays.contains(&name) {
            name = name.wrapping_add(1).max(1);
        }
        self.next_software_vertex_array = name.wrapping_add(1).max(1);
        self.software_vertex_arrays.insert(name);
        self.software_vao_element_array_buffers.entry(name).or_insert(0);
        name
    }
    pub(super) fn is_software_vertex_array(&self, array: GLuint) -> bool {
        array != 0 && self.software_vertex_arrays.contains(&array)
    }
    pub(super) fn delete_software_vertex_array(
        &mut self,
        array: GLuint,
    ) -> Option<(GLuint, Option<VertexArrayState>)> {
        if array == 0 || !self.software_vertex_arrays.remove(&array) {
            return None;
        }
        self.software_vao_element_array_buffers.remove(&array);
        if self.current_vertex_array == array {
            // Deleting the bound VAO reverts to the default one. Forget the
            // deleted VAO's element array binding before switching so that
            // it isn't saved under its name again.
            self.element_array_buffer = None;
            let (binding, outgoing) = self.bind_software_vertex_array(0);
            self.software_vao_states.remove(&array);
            Some((binding, outgoing))
        } else {
            self.software_vao_states.remove(&array);
            None
        }
    }
}

#[derive(Default)]
pub(super) struct EAGLContextHostObject {
    pub(super) gles_ctx: Option<Box<dyn GLESContext>>,
    /// Guest object addresses can be reused when a game session restarts.
    present_token: usize,
    resource_group_token: usize,
    /// See [GLShadowState].
    pub(super) shadow: GLShadowState,
    /// Which EAGL rendering API was requested. This influences how
    /// [super::gles_guest] dispatches calls and how the present-renderbuffer
    /// path saves and restores state.
    pub(super) api: EAGLRenderingAPI,
    /// The framebuffer that was bound when this context was created. On iOS,
    /// SDL2 draws the window through a framebuffer object of its own rather
    /// than framebuffer 0, so presenting has to target this one. Always 0 on
    /// other platforms.
    drawable_framebuffer: GLuint,
    /// Mapping of OpenGL ES renderbuffer names to `EAGLDrawable` instances
    /// (always `CAEAGLLayer*`). Retains the instance so it won't dangle.
    renderbuffer_drawable_bindings: Rc<RefCell<HashMap<GLuint, id>>>,
    fps_counter: Option<FpsCounter>,
    next_frame_due: Option<Instant>,
    /// Buffers the guest currently has mapped, keyed by target and buffer
    /// object name: `(guest mirror, driver mapping, size)`.
    ///
    /// The driver mapping is only set for `glMapBufferRange`, whose mapping is
    /// still open; `glMapBufferOES` keeps the data in the guest mirror alone
    /// and uploads it on unmap, so it never holds a driver pointer across a
    /// guest call (see `gles_guest::glMapBufferOES`).
    pub mapped_buffers:
        HashMap<(GLenum, GLuint), (MutPtr<GLvoid>, Option<*mut GLvoid>, usize)>,
}
impl HostObject for EAGLContextHostObject {}


/// Log the effective state of the `fix_texture_min_filter` option once,
/// the first time an EAGL context is created. This makes it easy to
/// diagnose "geometry rendering as flat black/white textures" reports
/// from a single log file: if the option is on we say so, if it's off
/// (e.g. because the user explicitly disabled it via
/// `--no-fix-texture-min-filter`) we say that too along with the
/// suggested switch to enable it. The log fires only once per process.
fn log_fix_min_filter_status(enabled: bool) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static SEEN: AtomicBool = AtomicBool::new(false);
    if SEEN.swap(true, Ordering::Relaxed) {
        return;
    }
    if enabled {
        log!(
            "fix_texture_min_filter: ON (after every level-0 texture upload, \
             GL_TEXTURE_MIN_FILTER will be forced to GL_LINEAR if the guest \
             never set it). This avoids flat-black/white-textured geometry on \
             strict drivers (Mali, Adreno) for iOS games that don't ship \
             mipmaps. Disable with --no-fix-texture-min-filter if a game \
             genuinely needs mipmap minification."
        );
    } else {
        log!(
            "fix_texture_min_filter: OFF. If textured geometry renders as \
             flat black or white shapes on this device, enable the fix-up \
             with --fix-texture-min-filter (or remove \
             --no-fix-texture-min-filter from your options)."
        );
    }
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation EAGLContext: NSObject

+ (id)alloc {
    let host_object = Box::new(EAGLContextHostObject {
        gles_ctx: None,
        present_token: crate::gles::perf_overlay::allocate_context_token(),
        resource_group_token: 0,
        shadow: GLShadowState::default(),
        api: kEAGLRenderingAPIOpenGLES1,
        drawable_framebuffer: 0,
        renderbuffer_drawable_bindings: Rc::new(RefCell::new(HashMap::new())),
        fps_counter: None,
        next_frame_due: None,
        mapped_buffers: HashMap::new(),
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

+ (id)currentContext {
    env.framework_state.opengles.current_ctx_for_thread(env.current_thread).unwrap_or(nil)
}
+ (bool)setCurrentContext:(id)context { // EAGLContext*
    retain(env, context);

    let current_ctx = env.framework_state.opengles.current_ctx_for_thread(env.current_thread);

    if let Some(old_ctx) = std::mem::take(current_ctx) {
        release(env, old_ctx);
    }

    // reborrow
    let current_ctx = env.framework_state.opengles.current_ctx_for_thread(env.current_thread);

    if context != nil {
        *current_ctx = Some(context);
    }

    true
}

- (id)initWithAPI:(EAGLRenderingAPI)api sharegroup:(id)group {
    if api != kEAGLRenderingAPIOpenGLES1
        && api != kEAGLRenderingAPIOpenGLES2
        && api != kEAGLRenderingAPIOpenGLES3
    {
        log!(
            "App requested EAGL initWithAPI:{} sharegroup:{:?}, returning nil as we only support APIs 1, 2 and 3",
            api,
            group
        );
        return nil;
    }

    if group == nil {
        return msg![env; this initWithAPI:api];
    }

    let resource_group_token = {
        let group_context = env.objc.borrow::<EAGLContextHostObject>(group);
        if group_context.resource_group_token != 0 {
            group_context.resource_group_token
        } else {
            group_context.present_token
        }
    };

    let window = env.window.as_mut().expect("OpenGL ES is not supported in headless mode");
    let prev_context = env.objc.borrow_mut::<EAGLContextHostObject>(group).gles_ctx.as_mut().unwrap();

    // This is sort of a hack - we set the "current" context, then immediately
    // drop it. Since we know all the code between here and creating the new
    // context, we know that there won't be any context switches, so it's fine
    // to do this.
    {
        let _prev_ctx = prev_context.make_current(window);
    }
    env.window.as_mut().unwrap().set_share_with_current_context(true);

    let effective_api = effective_eagl_api(
        api,
        env.options.prefer_gles2_context,
        env.options.force_gles1_context,
    );

    let mut gles_ins = match effective_api {
        kEAGLRenderingAPIOpenGLES3 => create_gles3_ctx(env),
        kEAGLRenderingAPIOpenGLES2 => create_gles2_ctx(env),
        _ => create_gles1_ctx(env),
    };

    let window = env.window.as_mut().expect("OpenGL ES is not supported in headless mode");
    let drawable_framebuffer = {
        #[allow(unused_mut)]
        let mut gles_ctx = gles_ins.make_current(window);
        log!("Driver info: {}", unsafe { gles_ctx.driver_description() });
        log_fix_min_filter_status(env.options.fix_texture_min_filter);
        #[allow(unused_mut)]
        let mut framebuffer: GLint = 0;
        #[cfg(target_os = "ios")]
        unsafe {
            gles_ctx.GetIntegerv(gles11::FRAMEBUFFER_BINDING_OES, &mut framebuffer);
        }
        framebuffer as GLuint
    };

    {
        let host_object = env.objc.borrow_mut::<EAGLContextHostObject>(this);
        host_object.gles_ctx = Some(gles_ins);
        host_object.api = effective_api;
        host_object.drawable_framebuffer = drawable_framebuffer;
        host_object.resource_group_token = resource_group_token;
        host_object.shadow.resource_group_token = resource_group_token;
    }
    crate::gles::perf_overlay::register_gl_resource_context(resource_group_token);

    env.window.as_mut().unwrap().set_share_with_current_context(false);

    env.objc.borrow_mut::<EAGLContextHostObject>(this).renderbuffer_drawable_bindings = env.objc.borrow::<EAGLContextHostObject>(group).renderbuffer_drawable_bindings.clone();
    this
}

- (id)initWithAPI:(EAGLRenderingAPI)api {
    if api != kEAGLRenderingAPIOpenGLES1
        && api != kEAGLRenderingAPIOpenGLES2
        && api != kEAGLRenderingAPIOpenGLES3
    {
        log!(
            "App requested EAGL initWithAPI:{}, returning nil as we only support APIs 1, 2 and 3",
            api
        );
        return nil;
    }

    let resource_group_token = env.objc.borrow::<EAGLContextHostObject>(this).present_token;

    let effective_api = effective_eagl_api(
        api,
        env.options.prefer_gles2_context,
        env.options.force_gles1_context,
    );

    let mut gles_ins = match effective_api {
        kEAGLRenderingAPIOpenGLES3 => create_gles3_ctx(env),
        kEAGLRenderingAPIOpenGLES2 => create_gles2_ctx(env),
        _ => create_gles1_ctx(env),
    };

    let window = env.window.as_mut().expect("OpenGL ES is not supported in headless mode");
    let drawable_framebuffer = {
        #[allow(unused_mut)]
        let mut gles_ctx = gles_ins.make_current(window);
        log!("Driver info: {}", unsafe { gles_ctx.driver_description() });
        log_fix_min_filter_status(env.options.fix_texture_min_filter);
        #[allow(unused_mut)]
        let mut framebuffer: GLint = 0;
        #[cfg(target_os = "ios")]
        unsafe {
            gles_ctx.GetIntegerv(gles11::FRAMEBUFFER_BINDING_OES, &mut framebuffer);
        }
        framebuffer as GLuint
    };

    {
        let host_object = env.objc.borrow_mut::<EAGLContextHostObject>(this);
        host_object.gles_ctx = Some(gles_ins);
        host_object.api = effective_api;
        host_object.drawable_framebuffer = drawable_framebuffer;
        host_object.resource_group_token = resource_group_token;
        host_object.shadow.resource_group_token = resource_group_token;
    }
    crate::gles::perf_overlay::register_gl_resource_context(resource_group_token);

    this
}

- (EAGLRenderingAPI)API {
    env.objc.borrow::<EAGLContextHostObject>(this).api
}

- (id)sharegroup {
    // We use object itself as the sharegroup.
    // Check initWithAPI:sharegroup: for more info
    this
}

- (())dealloc {
    let (resource_group_token, drawables, mapped_guest_buffers) = {
        let host_obj = env.objc.borrow_mut::<EAGLContextHostObject>(this);
        let mapped_guest_buffers = host_obj
            .mapped_buffers
            .values()
            .map(|&(guest_buf, _, _)| guest_buf)
            .collect::<Vec<_>>();
        let resource_group_token = host_obj.resource_group_token;
        host_obj.resource_group_token = 0;
        host_obj.shadow.resource_group_token = 0;
        let drawables = if Rc::strong_count(&host_obj.renderbuffer_drawable_bindings) == 1 {
            let bindings = std::mem::take(&mut host_obj.renderbuffer_drawable_bindings);
            bindings.take().into_values().collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        (resource_group_token, drawables, mapped_guest_buffers)
    };
    for guest_buffer in mapped_guest_buffers {
        env.mem.free(guest_buffer);
    }
    for drawable in drawables {
        release(env, drawable);
    }
    crate::gles::perf_overlay::unregister_gl_resource_context(resource_group_token);
    env.objc.dealloc_object(this, &mut env.mem);
}

- (bool)renderbufferStorage:(NSUInteger)target
               fromDrawable:(id)drawable { // EAGLDrawable (always CAEAGLayer*)
    log!("[EAGLContext renderbufferStorage:{:#x} fromDrawable:{:?}]", target, drawable);

    if target != gles11::RENDERBUFFER_OES {
        log!(
            "[EAGLContext renderbufferStorage:{:#x} fromDrawable:{:?}] invalid target; returning NO",
            target,
            drawable
        );
        return false;
    }

    // Apple's `EAGLContext` documentation for
    // `-renderbufferStorage:fromDrawable:` states that passing `nil` for the
    // drawable "deletes any earlier binding" of the currently bound
    // renderbuffer to a drawable and returns `YES`. touchHLE used to
    // assert against this case (`drawable != nil`), which crashed apps
    // that legitimately unbind during teardown (e.g. Resident Evil 4 —
    // HyperHLE log #5 — calls `renderbufferStorage:fromDrawable:nil`
    // while tearing down its EAGL surface during a scene transition).
    //
    // Spec behaviour: look up the currently bound renderbuffer (via
    // `RENDERBUFFER_BINDING_OES`), drop its entry from the
    // (renderbuffer -> drawable) map, and release the retained drawable.
    // No new storage is allocated in this case.
    if drawable == nil {
        let Some(window) = env.window.as_mut() else {
            log_dbg!(
                "[EAGLContext renderbufferStorage:{:#x} fromDrawable:nil] ignored in headless mode",
                target
            );
            return true;
        };
        let current_renderbuffer = {
            let Some(mut gles) = super::sync_context(
                &mut env.framework_state.opengles,
                &mut env.objc,
                window,
                env.current_thread,
            ) else {
                // No current EAGL context: there can't be a meaningful
                // renderbuffer binding to remove. Apple-style soft failure.
                log!(
                    "[EAGLContext renderbufferStorage:{:#x} fromDrawable:nil] \
                     called with no current GL context for thread {}; \
                     treating as no-op.",
                    target,
                    env.current_thread
                );
                return true;
            };
            let mut renderbuffer: gles11::types::GLint = 0;
            unsafe {
                gles.GetIntegerv(gles11::RENDERBUFFER_BINDING_OES, &mut renderbuffer);
            }
            renderbuffer as gles11::types::GLuint
        };

        let removed = {
            let host_obj = env.objc.borrow_mut::<EAGLContextHostObject>(this);
            host_obj
                .renderbuffer_drawable_bindings
                .borrow_mut()
                .remove(&current_renderbuffer)
        };
        if let Some(old_drawable) = removed {
            release(env, old_drawable);
        } else {
            log_dbg!(
                "[EAGLContext renderbufferStorage:{:#x} fromDrawable:nil]: \
                 no drawable was bound to renderbuffer {} — nothing to unbind.",
                target,
                current_renderbuffer
            );
        }
        return true;
    }

    let props: id = msg![env; drawable drawableProperties];

    let format_key = get_static_str(env, kEAGLDrawablePropertyColorFormat);
    let format_rgba8 = get_static_str(env, kEAGLColorFormatRGBA8);
    let format_rgb565 = get_static_str(env, kEAGLColorFormatRGB565);

    let format: id = msg![env; props objectForKey:format_key];
    // Theoretically this should map formats like:
    // - kColorFormatRGBA8 => RGBA8_OES
    // - kColorFormatRGB565 => RGB565_OES
    // However, the specification of EXT_framebuffer_object allows the
    // implementation to arbitrarily restrict which formats can be rendered to,
    // and it seems like RGB565 isn't supported, at least on a machine with
    // Intel HD Graphics 615 running macOS Monterey. I don't think RGBA8 is
    // guaranteed either, but it at least seems to work.
    if !msg![env; format isEqual:format_rgba8] && !msg![env; format isEqual:format_rgb565] {
        log!("[renderbufferStorage:{:?} fromDrawable:{:?}] Warning: unhandled format {:?}, using RGBA8", target, drawable, format);
    }
    let internalformat = gles11::RGBA8_OES;

    let (width, height) = {
        // ULTRAHLE_MINIONJUMP_RENDERBUFFER_BEGIN
        if matches!(
            env.bundle.bundle_identifier(),
            "com.apprisetec9.minionjump" | "com.risinghighapps.kingdomprincepro"
        ) {
            log!("UltraHLE MinionJump: forcing EAGL renderbuffer storage to 1024x768");
            (1024, 768)
        } else {
            let bounds: CGRect = msg![env; drawable bounds];
            let CGSize { width, height } = bounds.size;

            // Apple's `-renderbufferStorage:fromDrawable:` derives the
            // renderbuffer size from the CAEAGLayer's bounds. Some apps (e.g.
            // Beyond Gravity — HyperHLE log #1) momentarily present a layer
            // whose `bounds.size` is bogus (non-finite, negative, or zero)
            // during an unbind/rebind cycle while tearing down and recreating
            // their EAGL surface. touchHLE used to `assert!` the size was in a
            // sane range, which aborted the whole emulator. Real iOS never
            // crashes here — it simply allocates a renderbuffer sized to the
            // (screen-sized) drawable. Match that by falling back to the main
            // screen's bounds when the drawable reports an invalid size.
            let size_is_valid = |v: f32| v.is_finite() && (1.0..(u32::MAX as f32)).contains(&v);
            let (width, height) = if size_is_valid(width) && size_is_valid(height) {
                (width, height)
            } else {
                let screen: id = msg_class![env; UIScreen mainScreen];
                let screen_bounds: CGRect = msg![env; screen bounds];
                // Copy the fields out of the packed CGSize before using them
                // (taking a reference to a packed struct field is UB / a
                // compile error).
                let fallback_width = screen_bounds.size.width;
                let fallback_height = screen_bounds.size.height;
                log!(
                    "[renderbufferStorage:{:?} fromDrawable:{:?}] Warning: drawable \
                     reported invalid bounds size {}x{}; falling back to main screen \
                     bounds {}x{}",
                    target,
                    drawable,
                    width,
                    height,
                    fallback_width,
                    fallback_height
                );
                (fallback_width, fallback_height)
            };
            // A full-screen EAGL layer sized in points gets its backing
            // pixel resolution from the layer's `contentsScale` (which
            // init_common in ui_view.rs seeds with UIScreen.scale, and which
            // apps may override via setContentsScale:). Without honouring it,
            // retina devices (iPhone 4/5/5c, iPad 3/4/5, iPad mini 2/3,
            // iPod touch 4/5) would allocate a half-size renderbuffer and the
            // app would render zoomed-in and cropped. `scale_hack` is a
            // user-facing multiplier applied on top, as before.
            let contents_scale = {
                let layer_contents_scale: crate::frameworks::core_graphics::CGFloat =
                    env.objc
                        .borrow::<crate::frameworks::core_animation::ca_layer::CALayerHostObject>(
                            drawable,
                        )
                        .contents_scale;
                if layer_contents_scale.is_finite() && layer_contents_scale > 0.0 {
                    layer_contents_scale
                } else {
                    1.0
                }
            };

            let scale_hack = env.options.scale_hack.get();

            let mut width = (width * contents_scale).round() as u32 * scale_hack;
            let mut height = (height * contents_scale).round() as u32 * scale_hack;

            // If even the fallback produced a degenerate size, clamp to a
            // minimum 1x1 so the GL call below cannot receive a zero extent.
            width = width.max(1);
            height = height.max(1);
            // ... and to a sane maximum, so a bogus bounds/contentsScale/
            // scale-hack combination can't overflow the GLsizei conversion
            // below (a host panic) or ask the driver for gigabytes.
            const MAX_RENDERBUFFER_DIMENSION: u32 = 16384;
            if width > MAX_RENDERBUFFER_DIMENSION || height > MAX_RENDERBUFFER_DIMENSION {
                log!(
                    "[renderbufferStorage:{:?} fromDrawable:{:?}] Warning: clamping \
                     oversized renderbuffer {}x{} to at most {}x{}",
                    target,
                    drawable,
                    width,
                    height,
                    MAX_RENDERBUFFER_DIMENSION,
                    MAX_RENDERBUFFER_DIMENSION
                );
                width = width.min(MAX_RENDERBUFFER_DIMENSION);
                height = height.min(MAX_RENDERBUFFER_DIMENSION);
            }

            // RBSIZE-DIAG: log renderbuffer storage dimensions each time so a
            // mid-run resize (loading screen vs game scene) is visible.
            log!(
                "RBSIZE-DIAG renderbufferStorage drawable={:?} -> {}x{} (contents_scale={}, scale_hack={})",
                drawable, width, height, contents_scale, scale_hack
            );

            if std::env::var_os("TOUCHHLE_FORCE_LANDSCAPE_RENDERBUFFER").is_some() {
                let is_landscape = env
                    .window
                    .as_ref()
                    .map(|window| {
                        !matches!(
                            window.current_rotation(),
                            crate::window::DeviceOrientation::Portrait
                        )
                    })
                    .unwrap_or(false);

                if is_landscape && height > width {
                    log!(
                        "TOUCHHLE_FORCE_LANDSCAPE_RENDERBUFFER=1: swapping EAGL renderbuffer storage from {}x{} to {}x{}",
                        width,
                        height,
                        height,
                        width
                    );
                    std::mem::swap(&mut width, &mut height);
                } else {
                    log!(
                        "TOUCHHLE_FORCE_LANDSCAPE_RENDERBUFFER=1: keeping EAGL renderbuffer storage at {}x{} (is_landscape={})",
                        width,
                        height,
                        is_landscape
                    );
                }
            }

            (width, height)
        }
        // ULTRAHLE_MINIONJUMP_RENDERBUFFER_END
    };

    // Apple's documentation states that the receiver must be the current
    // context when calling `renderbufferStorage:fromDrawable:`.  If no
    // context is current for this thread but `this` has a valid backing GLES
    // context, temporarily make `this` the current context so the GL call
    // succeeds, then restore the previous state.  This matches the behaviour
    // observed on real iOS where calling the method on a non-current context
    // still works as long as the receiver has been initialised.
    //
    // Note: `window` must be borrowed from `env` AFTER any calls to
    // `retain`/`release` because those also borrow `env` mutably.
    let prior_ctx = *env.framework_state.opengles.current_ctx_for_thread(env.current_thread);
    let needs_temp_current = prior_ctx.is_none() && {
        env.objc.borrow::<EAGLContextHostObject>(this).gles_ctx.is_some()
    };
    if needs_temp_current {
        log_dbg!(
            "[EAGLContext renderbufferStorage:{:#x} fromDrawable:{:?}] \
             no current context for thread {}; temporarily making this context current.",
            target, drawable, env.current_thread
        );
        retain(env, this);
        *env.framework_state.opengles.current_ctx_for_thread(env.current_thread) = Some(this);
    }

    // Run the actual GL storage allocation inside a nested scope so that
    // `window` (which mutably borrows `env.window`) is dropped before we need
    // to call `retain`/`release` with a full `&mut env` borrow below.
    let renderbuffer_result: Option<u32> = {
        let window = env.window.as_mut().expect("OpenGL ES is not supported in headless mode");
        match super::sync_context(
            &mut env.framework_state.opengles,
            &mut env.objc,
            window,
            env.current_thread,
        ) {
            None => {
                log!(
                    "[EAGLContext renderbufferStorage:{:#x} fromDrawable:{:?}] \
                     called with no current GL context for thread {}; failing \
                     the call instead of crashing.",
                    target,
                    drawable,
                    env.current_thread
                );
                None
            }
            Some(mut gles) => {
                // Clear any pre-existing error so we can detect failure of the
                // storage allocation reliably.
                unsafe { while gles.GetError() != gles11::NO_ERROR {} }
                unsafe { gles.RenderbufferStorageOES(target, internalformat, width.try_into().unwrap(), height.try_into().unwrap()); }
                let needs_fallback = unsafe { gles.GetError() != gles11::NO_ERROR };
                let alloc_ok = if needs_fallback {
                    // RGBA8 is optional in OpenGL ES 1.1 Common Profile (requires
                    // OES_rgb8_rgba8). Fall back to RGBA4 (0x8056) which is
                    // required by OES_framebuffer_object.
                    const GL_RGBA4: gles11::types::GLenum = 0x8056;
                    unsafe { gles.RenderbufferStorageOES(target, GL_RGBA4, width.try_into().unwrap(), height.try_into().unwrap()); }
                    unsafe { gles.GetError() == gles11::NO_ERROR }
                } else {
                    true
                };
                if !alloc_ok {
                    log!(
                        "[EAGLContext renderbufferStorage:{:#x} fromDrawable:{:?}] \
                         failed to allocate renderbuffer storage (tried RGBA8 and RGBA4)",
                        target,
                        drawable
                    );
                    None
                } else {
                    let mut renderbuffer: gles11::types::GLint = 0;
                    unsafe { gles.GetIntegerv(gles11::RENDERBUFFER_BINDING_OES, &mut renderbuffer); }
                    Some(renderbuffer as u32)
                }
            }
        }
    };

    // `window` borrow dropped here — safe to use `retain`/`release` again.
    // `None` means either no GL context was available, or storage allocation failed.
    let renderbuffer = match renderbuffer_result {
        None => {
            if needs_temp_current {
                *env.framework_state.opengles.current_ctx_for_thread(env.current_thread) = None;
                release(env, this);
            }
            return false;
        }
        Some(rb) => rb,
    };

    // Restore the previous thread-local context (if we temporarily set `this`
    // as the current context earlier in this call).
    if needs_temp_current {
        *env.framework_state.opengles.current_ctx_for_thread(env.current_thread) = None;
        release(env, this);
    }

    retain(env, drawable);
    let host_obj = env.objc.borrow_mut::<EAGLContextHostObject>(this);
    let maybe_old_drawable = host_obj.renderbuffer_drawable_bindings.borrow_mut().insert(
        renderbuffer,
        drawable
    );
    if let Some(old_drawable) = maybe_old_drawable {
        release(env, old_drawable);
    }

    true
}

- (bool)presentRenderbuffer:(NSUInteger)target {
    // Some games (e.g. Angry Birds 1.0) run their main loop without going
    // through the NSRunLoop, so handle_events() in the run loop never fires.
    // Poll and dispatch pending input events here, at the natural per-frame
    // boundary, so touches always reach the game.
    //
    // In headless mode there is no window, and
    // [Environment::on_parent_stack_in_coroutine] unconditionally unwraps
    // the (absent) window, so routing through it here would panic before we
    // ever get to the "OpenGL ES is not supported in headless mode" checks
    // further down. There's nothing to poll without a window anyway, so
    // just skip this step when headless.
    if env.current_thread == 0 && env.window.is_some() {
        env.on_parent_stack_in_coroutine(|window, options| {
            window.poll_for_events(options);
        });
        uikit::handle_events(env);
    }

    // First-frame breadcrumb. presentRenderbuffer is called every frame, so a
    // plain log!() would flood, but the very first call is a key signal that
    // the app actually got past splash/init and is rendering.
    log_once!("[EAGLContext presentRenderbuffer:] first call (app reached first frame)");
    {
        static INPUT_POLL_THREAD_LOGGED: std::sync::Once = std::sync::Once::new();
        INPUT_POLL_THREAD_LOGGED.call_once(|| {
            log!("EAGL event polling first reached on guest thread {}", env.current_thread);
        });
    }

    // Frame-count milestones. presentRenderbuffer is called every frame, so we
    // want a small, fixed number of log lines that prove the render loop is
    // still progressing (useful for distinguishing "actually hung" from
    // "running but invisible because the app's stdout doesn't reach this log
    // sink"). The milestones are roughly logarithmic so they cover the range
    // from sub-second to ~10 minutes at 60 FPS without flooding.
    {
        use std::sync::atomic::{AtomicU64, Ordering};
        static FRAME_COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = FRAME_COUNTER.fetch_add(1, Ordering::Relaxed) + 1;
        if matches!(n, 10 | 60 | 300 | 1800 | 3600 | 7200 | 18000 | 36000) {
            log!(
                "[EAGLContext presentRenderbuffer:] frame {} reached (render loop is alive)",
                n
            );
        }
        if n == 3000 && crate::env_flag_cached!("TOUCHHLE_TRACE_FRAME_CALLS") {
            crate::dyld::TRACE_HOST_CALLS.store(400, Ordering::Relaxed);
        }
    }

    if target != gles11::RENDERBUFFER_OES {
        log!(
            "[EAGLContext presentRenderbuffer:{:#x}] invalid target; returning NO",
            target
        );
        return false;
    }

    // The presented frame should be displayed ASAP, but the next one must be
    // delayed, so this needs to be checked before returning.
    let frame_due = limit_framerate(&mut env.objc.borrow_mut::<EAGLContextHostObject>(this).next_frame_due, &env.options, env.guest_clock.speed().multiplier());

    if env.options.print_fps
        || cfg!(target_os = "ios")
        || crate::gles::present::onscreen_fps_enabled()
    {
        if crate::gles::perf_overlay::enabled() {
            crate::gles::perf_overlay::record_guest_flip();
        }
        env
            .objc
            .borrow_mut::<EAGLContextHostObject>(this)
            .fps_counter
            .get_or_insert_with(FpsCounter::start)
            .count_frame(format_args!("EAGLContext {this:?}"), env.options.print_fps);
    }

    let mut fullscreen_layer = find_fullscreen_eagl_layer(env);

    // Unclear from documentation if this method requires the context to be
    // current, but it would be weird if it didn't?
    let Some(window) = env.window.as_mut() else {
        log_dbg!(
            "[EAGLContext presentRenderbuffer:{:#x}] ignored in headless mode",
            target
        );
        return false;
    };
    let Some(mut gles) = super::sync_context(&mut env.framework_state.opengles, &mut env.objc, window, env.current_thread) else {
        // No current EAGL context. Apple's docs require the receiver to be
        // the current context for `presentRenderbuffer:` to succeed; the
        // canonical iOS behaviour is to silently return NO in this state
        // rather than abort. Returning here also keeps the framerate
        // limiter from leaking a sleep if we somehow get here without a
        // context.
        log!(
            "[EAGLContext presentRenderbuffer:{:#x}] called with no current \
             GL context for thread {}; returning NO.",
            target,
            env.current_thread
        );
        if let Some(frame_due) = frame_due {
            pace_frame(env, frame_due);
        }
        return false;
    };

    let (renderbuffer, backend_is_translator, backend_is_native_es1, backend_is_es2) = unsafe {
        let mut renderbuffer = 0;
        gles.GetIntegerv(gles11::RENDERBUFFER_BINDING_OES, &mut renderbuffer);
        (
            renderbuffer as _,
            gles.is_translator(),
            gles.is_native_es1(),
            gles.is_es2(),
        )
    };

    std::mem::drop(gles);

    let bindings = env
        .objc
        .borrow::<EAGLContextHostObject>(this)
        .renderbuffer_drawable_bindings
        .borrow();

    // The renderbuffer reported by the driver must be the drawable's colour
    // renderbuffer, and that is what gets keyed in the map. Some engines
    // (cocos2d 2.x, e.g. Geometry Dash) leave a different binding current
    // around presentRenderbuffer:, which makes the driver-reported id miss
    // the map — on iOS the renderbuffer attached to the drawable is still
    // presented, so fall back to the single registered binding instead of
    // silently dropping the frame (the silent drop manifests as a permanent
    // black screen).
    let drawable = match bindings.get(&renderbuffer) {
        Some(&drawable) => drawable,
        None => {
            if bindings.len() == 1 {
                let (&rb, &drawable) = bindings.iter().next().unwrap();
                {
                    static MISMATCH_LOGGED: std::sync::Once = std::sync::Once::new();
                    MISMATCH_LOGGED.call_once(|| {
                        log!(
                            "[EAGLContext presentRenderbuffer:] renderbuffer binding \
                             mismatch: driver reports {:#x}, drawable is bound to \
                             {:#x}; presenting the bound drawable anyway. \
                             [this log will only be shown once]",
                            renderbuffer,
                            rb
                        );
                    });
                }
                drawable
            } else {
                log!(
                    "Warning: can't present a renderbuffer {:#x} not bound to a \
                     drawable ({} bound renderbuffer(s): {:?}) - frame skipped.",
                    renderbuffer,
                    bindings.len(),
                    bindings.keys().copied().collect::<Vec<_>>(),
                );
                drop(bindings);
                if let Some(frame_due) = frame_due {
                    pace_frame(env, frame_due);
                }
                return false;
            }
        }
    };
    drop(bindings);
    if fullscreen_layer != drawable && attach_orphaned_eagl_view_to_root(env, drawable) {
        fullscreen_layer = find_fullscreen_eagl_layer(env);
    }

    // We're presenting to the opaque CAEAGLLayer that covers the screen.
    // We can use the fast path where we skip composition and present directly.
    //
    // We also take this fast path when a *different* full-screen layer was
    // discovered (`fullscreen_layer != nil && != drawable`). The game only
    // issues `presentRenderbuffer:` for the EAGL view it is actively drawing,
    // so that drawable is what should appear; some apps (Minecraft: Story
    // Mode) also keep a second full-screen layer in the window tree, and the
    // old code treated "this isn't the one layer I found" as "skip the frame",
    // which left the window black. Presenting the drawable the game just
    // rendered is the correct, visible choice.
    // On iOS, `--ios-es2-direct-present` (enabled per app in the default
    // options file) presents an OpenGL ES 2.0 drawable directly even when it is
    // not the fullscreen layer. The Sims Medieval needs this to show anything.
    let use_ios_es2_direct_path = cfg!(target_os = "ios")
        && env.options.ios_es2_direct_present
        && fullscreen_layer == nil
        && env.objc.borrow::<EAGLContextHostObject>(this).api == kEAGLRenderingAPIOpenGLES2;
    if use_ios_es2_direct_path {
        log_once!("Using the iOS ES2 direct presenter for a non-fullscreen CAEAGLLayer.");
    }

    if drawable == fullscreen_layer || fullscreen_layer != nil || use_ios_es2_direct_path {
        // Decide between presenting on the GPU (copy the renderbuffer into a
        // texture and draw it into the window — cheap) and reading the frame
        // back to RAM and pushing it through the compositor (a full pipeline
        // stall plus two full-frame copies per frame — very slow, but it
        // never touches the app's GL state).
        //
        // The readback route used to be the default for every native ES 1.1
        // backend, i.e. for every ES 1.1 game on Android, where it was by far
        // the biggest per-frame cost. It's now an explicit choice
        // (--present-mode=readback) or an automatic fallback when the GPU
        // route demonstrably produces black frames on this driver.
        let use_readback = match env.options.present_mode {
            PresentMode::Readback => {
                if backend_is_es2 && !backend_is_translator {
                    // The readback path speaks ES 1.1 (OES framebuffer entry
                    // points); the shader presenter is the only option on a
                    // real ES 2.0 driver.
                    log_once!(
                        "--present-mode=readback is not available on an OpenGL ES 2.0 backend; presenting directly"
                    );
                    false
                } else {
                    true
                }
            }
            PresentMode::Direct => false,
            // The ES 1.1-on-ES 2.0 translator can't run either GPU presenter
            // (its glDrawArrays is the fixed-function emulation itself), so
            // it keeps using readback unless explicitly overridden.
            PresentMode::Auto => {
                // Native ES 1.1 games keep the readback presenter by default.
                // The GPU-copy path changes guest-visible fixed-function state
                // in ways 2D engines (cocos2d etc.) notice — sprite
                // blending/tinting breaks even though the frame is not black,
                // so the automatic black-frame fallback never triggers. ES 2.0
                // backends keep the fast GPU path (its save/restore is exact,
                // and that is where the 3D games live).
                backend_is_translator
                    || backend_is_native_es1
                    || (backend_is_es2 && direct_present_is_broken())
            }
        };
        {
            static LOGGED: std::sync::Once = std::sync::Once::new();
            LOGGED.call_once(|| {
                log!(
                    "EAGL presenter: fullscreen layer {:?} will be presented via {} \
                     (present_mode={:?}, translator={}, native_es1={}, es2={}) \
                     [this log will only be shown once]",
                    drawable,
                    if use_readback {
                        "glReadPixels readback + compositor"
                    } else {
                        "GPU copy (direct)"
                    },
                    env.options.present_mode,
                    backend_is_translator,
                    backend_is_native_es1,
                    backend_is_es2,
                );
            });
        }
        if use_readback {
            log_dbg!(
                "Layer {:?} is the fullscreen layer, presenting renderbuffer {:?} through RAM readback.",
                drawable,
                renderbuffer,
            );
            unsafe {
                present_renderbuffer_readback(env, renderbuffer, drawable);
            }
        } else {
            log_dbg!(
                "Layer {:?} is the fullscreen layer, presenting renderbuffer {:?} directly (fast path).",
                drawable,
                renderbuffer,
            );
            let options = EaglPresentOptions {
                trace_gl_errors: env.options.trace_gl_errors,
                present_mode: env.options.present_mode,
                present_finish: env.options.present_finish,
                window_framebuffer: env
                    .objc
                    .borrow::<EAGLContextHostObject>(this)
                    .drawable_framebuffer,
                skip_rotation: use_ios_es2_direct_path,
                layer_rotation: if drawable == fullscreen_layer {
                    layer_screen_rotation(env, drawable)
                } else {
                    0.0
                },
            };
            // Tell the Core Animation compositor this layer is live, so it
            // takes its CAEAGLLayer fast path instead of compositing (and
            // swapping) a frame with a black EAGL layer in between ours.
            mark_presented_directly(env, drawable);
            if fullscreen_layer != nil && fullscreen_layer != drawable {
                mark_presented_directly(env, fullscreen_layer);
            }
            unsafe {
                let context_token = env.objc.borrow::<EAGLContextHostObject>(this).present_token;
                present_renderbuffer(env, renderbuffer, drawable, options, context_token);
            }
        }
    } else {
        if fullscreen_layer != nil {
            // If there's a single layer that covers the screen, and this isn't
            // it, there's no point in presenting the output because it won't be
            // seen. Using a noisy log because it's a weird scenario and might
            // indicate a bug.
            static LAYER_MISMATCH_DIAGNOSTIC_LOGGED: std::sync::Once = std::sync::Once::new();
            LAYER_MISMATCH_DIAGNOSTIC_LOGGED.call_once(|| {
                let drawable_view: id = msg![env; drawable delegate];
                let fullscreen_view: id = msg![env; fullscreen_layer delegate];
                let drawable_view_class = if drawable_view == nil {
                    "(nil)".to_string()
                } else {
                    let class: crate::objc::Class = msg![env; drawable_view class];
                    env.objc.get_class_name(class).to_owned()
                };
                let fullscreen_view_class = if fullscreen_view == nil {
                    "(nil)".to_string()
                } else {
                    let class: crate::objc::Class = msg![env; fullscreen_view class];
                    env.objc.get_class_name(class).to_owned()
                };
                let drawable_view_hidden = if drawable_view == nil {
                    false
                } else {
                    msg![env; drawable_view isHidden]
                };
                let fullscreen_view_hidden = if fullscreen_view == nil {
                    false
                } else {
                    msg![env; fullscreen_view isHidden]
                };
                log!(
                    "EAGL layer mismatch: drawable {:?} owner={} hidden={}, selected fullscreen layer {:?} owner={} hidden={}",
                    drawable,
                    drawable_view_class,
                    drawable_view_hidden,
                    fullscreen_layer,
                    fullscreen_view_class,
                    fullscreen_view_hidden
                );
            });
            if let Some(frame_due) = frame_due {
                pace_frame(env, frame_due);
            }
            return true;
        }

        // The very slow and inefficient path: not only does glReadPixels()
        // block the thread until rendering finishes, but the result has to be
        // copied back to system RAM, and then will have to be copied to VRAM
        // again during composition. find_fullscreen_eagl_layer() exists to
        // avoid this.
        {
            static SLOW_PATH_LOGGED: std::sync::Once = std::sync::Once::new();
            SLOW_PATH_LOGGED.call_once(|| {
                log!(
                    "EAGL presenter: no fullscreen layer found; presenting renderbuffer \
                     {:#x} to layer {:?} via RAM readback (slow path). [this log will \
                     only be shown once]",
                    renderbuffer,
                    drawable
                );
            });
        }
        let pixels_vec = get_pixels_vec_for_presenting(env, drawable);
        // re-borrow
        let read_result = {
            let maybe_gles = super::sync_context(
                &mut env.framework_state.opengles,
                &mut env.objc,
                env.window.as_mut().unwrap(),
                env.current_thread,
            );
            match maybe_gles {
                Some(mut gles) => Some(unsafe { read_renderbuffer(gles.as_mut(), renderbuffer, pixels_vec) }),
                None => {
                    log!(
                        "[EAGLContext presentRenderbuffer:{:#x}] lost GL \
                         context for thread {} between fast-path and \
                         slow-path; skipping copy-back.",
                        target,
                        env.current_thread
                    );
                    None
                }
            }
        };
        let Some((pixels_vec, width, height)) = read_result else {
            if let Some(frame_due) = frame_due {
                pace_frame(env, frame_due);
            }
            return false;
        };
        dump_readback_ppm(&pixels_vec, width, height);
        // DIAG (TOUCHHLE_TRACE_PRESENT=1): is the frame we read back actually
        // non-black? This tells apart "renderer produced a black frame"
        // (content/loading) from "we present fine but the pixels never reach
        // the window" (compositor/layer bug). Sample the max channel + count
        // of non-zero bytes, rate-limited.
        if crate::env_flag_cached!("TOUCHHLE_TRACE_PRESENT") {
            use std::sync::atomic::{AtomicU32, Ordering};
            static N: AtomicU32 = AtomicU32::new(0);
            let n = N.fetch_add(1, Ordering::Relaxed);
            if n < 8 || n % 60 == 0 {
                let max = pixels_vec.iter().copied().max().unwrap_or(0);
                let nonzero = pixels_vec.iter().filter(|&&b| b != 0).count();
                log!(
                    "PRESENT-DIAG slow-path frame #{} {}x{} max_channel={} nonzero_bytes={}/{}",
                    n, width, height, max, nonzero, pixels_vec.len()
                );
            }
        }
        present_pixels(env, drawable, pixels_vec, width, height);

        // The slow path stores the freshly rendered frame in `presented_pixels`
        // on the drawable, but the *screen* is only updated when the Core
        // Animation compositor runs. The compositor is normally driven from
        // NSRunLoop iterations, but some games (notably Temple Run) drive
        // their own render loop and only spin NSRunLoop very rarely, so the
        // screen would otherwise update at well below 2 FPS even though the
        // app is rendering at 60 FPS.
        //
        // Apple's documentation for `-[EAGLContext presentRenderbuffer:]`
        // states that the contents of the renderbuffer are displayed when
        // this method returns. To honour that contract — and to keep the
        // composited overlay (UIKit controls drawn on top of the EAGL view)
        // in step with the rendered frame — drive the compositor here
        // explicitly when we are on this slow path. We pass `force: true`
        // so the compositor's 60Hz throttle doesn't suppress this tick;
        // recomposite_if_necessary still updates its internal scheduler so
        // NSRunLoop-driven ticks remain rate-limited.
        crate::frameworks::core_animation::recomposite_if_necessary(env, true);
    }

    if let Some(frame_due) = frame_due {
        pace_frame(env, frame_due);
        }

    true
}

@end

};

/// Dump one renderbuffer readback to a PPM for black-screen diagnosis
/// (env var gated, as this is a developer-only diagnostic).
fn dump_readback_ppm(pixels: &[u8], width: u32, height: u32) {
    use std::sync::atomic::{AtomicU32, Ordering};
    static CALLS: AtomicU32 = AtomicU32::new(0);
    if !crate::env_flag_cached!("TOUCHHLE_DUMP_READBACK") {
        return;
    }
    let n = CALLS.fetch_add(1, Ordering::Relaxed);
    // Dump at several points in the session: the first frame can legitimately
    // be black (loading screen), so also sample later frames.
    let targets = [0u32, 60, 300, 600, 1200, 2400];
    let Some(idx) = targets.iter().position(|&t| t == n) else {
        return;
    };
    let header = format!("P6\n{} {}\n255\n", width, height);
    let mut out = header.into_bytes();
    let mut rgb = Vec::with_capacity((width * height * 3) as usize);
    for px in pixels.chunks_exact(4) {
        // read_renderbuffer gives RGBA8; PPM wants RGB.
        rgb.extend_from_slice(&[px[0], px[1], px[2]]);
    }
    out.extend_from_slice(&rgb);
    let path = format!("/tmp/a8run/readback_f{}.ppm", targets[idx]);
    match std::fs::write(&path, &out) {
        Ok(()) => log!(
            "Dumped renderbuffer readback #{} ({}x{}) to {}",
            n,
            width,
            height,
            path
        ),
        Err(e) => log!("Failed to dump readback to {}: {}", path, e),
    }
}

unsafe fn present_renderbuffer_readback(env: &mut Environment, renderbuffer: GLuint, drawable: id) {
    // PERF: recycle the layer's previous pixel buffer instead of allocating
    // (and page-faulting in) a fresh multi-megabyte Vec every frame.
    let pixels_vec = get_pixels_vec_for_presenting(env, drawable);
    let read_result = {
        let maybe_gles = super::sync_context(
            &mut env.framework_state.opengles,
            &mut env.objc,
            env.window.as_mut().unwrap(),
            env.current_thread,
        );
        match maybe_gles {
            Some(mut gles) => Some(read_renderbuffer(gles.as_mut(), renderbuffer, pixels_vec)),
            None => None,
        }
    };
    let Some((pixels, width, height)) = read_result else {
        log!("Native ES1 readback skipped because the GL context disappeared.");
        return;
    };
    dump_readback_ppm(&pixels, width, height);
    present_pixels(env, drawable, pixels, width, height);
    let force_composition = env.options.force_composition;
    env.options.force_composition = true;
    crate::frameworks::core_animation::recomposite_if_necessary(env, true);
    env.options.force_composition = force_composition;
}

/// Implement framerate limiting.
///
/// The real iPhone OS seems to force 60Hz v-sync in `presentRenderbuffer:`.
/// touchHLE does not force v-sync, and its users might not have 60Hz monitors
/// in any case, so to avoid excessive FPS or games running too fast, we need
/// to simulate it.
///
/// V-sync is essentially a limiter with no "slop", or allowance for frames
/// arriving late: if the frame misses a 60Hz interval, it must wait until the
/// next one. This is quite harsh: if frames consistently arrive very slightly
/// late, the framerate is halved!
///
/// Most games already use NSTimer, which is itself a v-sync-like limiter.
/// For the remainder, let's do something a bit kinder, for the benefit of users
/// with slow systems or which are using high scale hack settings: allow at most
/// an interval's worth of accumulated slop. Allowing infinite accumulation of
/// slop is not desirable, because if the game is running slowly for a long time
/// and suddenly speeds back up, it will then run too fast for a long time.
///
/// Returns the [Instant] the current frame is due at (or `None` if no pacing
/// is needed), so the caller can pace the *next* guest work precisely with
/// [pace_frame].
fn limit_framerate(next_frame_due: &mut Option<Instant>, options: &Options, speed: f64) -> Option<Instant> {
    let interval = if let Some(fps) = options.fps_limit {
        // Host frame spacing follows speed; the FPS counter itself stays real.
        1.0 / (fps * speed)
    } else {
        return None;
    };
    let interval_rust = Duration::from_secs_f64(interval);

    let &mut Some(current_frame_due) = next_frame_due else {
        // First frame presented: no delay yet.
        *next_frame_due = Some(Instant::now() + interval_rust);
        return None;
    };

    let now = Instant::now();
    *next_frame_due = if now > current_frame_due + interval_rust {
        // Too much slop has accumulated. Make the next frame wait for the next
        // interval.
        log_dbg!("Too much slop accumulated, skipping an interval.");
        Some(
            current_frame_due
                + Duration::from_secs_f64(
                    interval * (((now - current_frame_due).as_secs_f64() / interval).ceil()),
                ),
        )
    } else {
        // Time next frame based on when the current frame was due, not
        // the current time, so as to allow some slop.
        Some(current_frame_due + interval_rust)
    };

    if now < current_frame_due {
        // Frame was presented early, pace the next one to the due time.
        Some(current_frame_due)
    } else {
        // Frame was presented on time or late, don't pace.
        None
    }
}

/// How far before the frame deadline [pace_frame] stops the cooperative sleep
/// and switches to a spin-wait. Chosen to comfortably cover the typical
/// overshoot of the scheduler's final `std::thread::sleep` on Android
/// (hundreds of microseconds; a few ms when waking a parked core) without
/// burning significant CPU: at 60 FPS with a normal frame time the spin phase
/// usually lasts well under a millisecond.
const FRAME_PACING_SPIN_BUDGET: Duration = Duration::from_millis(2);

/// Pace the guest so its next frame's work resumes exactly at `frame_due`:
///
/// - **Coarse phase**: `env.sleep()` until shortly before the deadline. This
///   is a cooperative guest-thread sleep, so other guest threads (audio, run
///   loops, timers) still get CPU time while this frame waits.
/// - **Fine phase**: a short host-side spin until the deadline. Plain timer
///   sleeps wake up to several ms late, which used to start the guest's next
///   frame late and made its presentation miss the pacing grid (visible as
///   micro-stutter / cadence wobble). Spinning the last couple of
///   milliseconds lands the wake-up within tens of microseconds of the
///   deadline — the frame pacing equivalent of a vsync phase-lock.
fn pace_frame(env: &mut Environment, frame_due: Instant) {
    let now = Instant::now();
    if frame_due <= now {
        return;
    }
    let remaining = frame_due - now;
    if remaining > FRAME_PACING_SPIN_BUDGET {
        env.sleep(remaining - FRAME_PACING_SPIN_BUDGET);
    }
    // Fine phase: spin until the deadline. The extra half-budget past the
    // deadline is a circuit breaker in case the coarse sleep woke *late*
    // (a busy batch can do that): then the deadline is already in the past
    // and the deadline check exits immediately.
    let give_up_at = frame_due + FRAME_PACING_SPIN_BUDGET / 2;
    loop {
        let now = Instant::now();
        if now >= frame_due || now >= give_up_at {
            break;
        }
        std::hint::spin_loop();
    }
}

// These helper functions make the state backup code easier to read, but
// more importantly, they make it free of mutable variables that wouldn't
// get caught by Rust's unused variable warnings, which are useful to check
// we actually restore the stuff we back up.

unsafe fn get_ptr(gles: &mut dyn GLES, pname: GLenum) -> *const GLvoid {
    let mut ptr = std::ptr::null();
    gles.GetPointerv(pname, &mut ptr);
    ptr
}
// Safety: caller's responsibility to use appropriate N.
unsafe fn get_ints<const N: usize>(gles: &mut dyn GLES, pname: GLenum) -> [GLint; N] {
    let mut res = [0; N];
    gles.GetIntegerv(pname, res.as_mut_ptr());
    res
}
// Safety: caller's responsibility to only use this for scalars.
unsafe fn get_int(gles: &mut dyn GLES, pname: GLenum) -> GLint {
    get_ints::<1>(gles, pname)[0]
}
// Safety: caller's responsibility to use appropriate N.
unsafe fn get_tex_env_ints<const N: usize>(
    gles: &mut dyn GLES,
    target: GLenum,
    pname: GLenum,
) -> [GLint; N] {
    let mut res = [0; N];
    gles.GetTexEnviv(target, pname, res.as_mut_ptr());
    res
}
// Safety: caller's responsibility to only use this for scalars.
unsafe fn get_tex_env_int(gles: &mut dyn GLES, target: GLenum, pname: GLenum) -> GLint {
    get_tex_env_ints::<1>(gles, target, pname)[0]
}
// Safety: caller's responsibility to use appropriate N.
unsafe fn get_floats<const N: usize>(gles: &mut dyn GLES, pname: GLenum) -> [GLfloat; N] {
    let mut res = [0.0; N];
    gles.GetFloatv(pname, res.as_mut_ptr());
    res
}
unsafe fn get_renderbuffer_size(gles: &mut dyn GLES) -> (GLsizei, GLsizei) {
    let mut width: GLint = 0;
    let mut height: GLint = 0;
    gles.GetRenderbufferParameterivOES(
        gles11::RENDERBUFFER_OES,
        gles11::RENDERBUFFER_WIDTH_OES,
        &mut width,
    );
    gles.GetRenderbufferParameterivOES(
        gles11::RENDERBUFFER_OES,
        gles11::RENDERBUFFER_HEIGHT_OES,
        &mut height,
    );
    (width, height)
}

/// Copies the pixels in a renderbuffer bound to `GL_RENDERBUFFER_BINDING_OES`
/// (which should be provided by the app) to a provided [Vec], trying to avoid
/// noticeably modifying OpenGL ES state while doing so.
///
/// This uses `glReadPixels()`, with all the associated performance risks. Any
/// existing content in the [Vec] will bereplaced. The format is RGBA8.
/// The returned values are the [Vec], the width and height.
///
/// The provided context must be current.
unsafe fn read_renderbuffer(gles: &mut dyn GLES, renderbuffer: GLuint, mut pixel_buffer: Vec<u8>) -> (Vec<u8>, u32, u32) {
    let (width, height) = get_renderbuffer_size(gles);
    let width_u32: u32 = width.try_into().unwrap();
    let height_u32: u32 = height.try_into().unwrap();

    // Keep the application's framebuffer bound whenever possible. This is
    // not merely an optimisation: Adreno and other tile-based GLES drivers
    // may discard unresolved tile data when an application FBO is unbound.
    // The old implementation always switched to a temporary FBO, so
    // glReadPixels() then saw a cleared/black renderbuffer on those drivers.
    // Verify the attachment before trusting the current FBO: an app can leave
    // a different or incomplete FBO bound, in which case using it would be
    // just as wrong as the old unconditional temporary-FBO path.
    let old_framebuffer: GLuint = get_int(gles, gles11::FRAMEBUFFER_BINDING_OES) as _;
    let (attached_renderbuffer, framebuffer_status) = if old_framebuffer != 0 {
        let mut attached = 0;
        gles.GetFramebufferAttachmentParameterivOES(
            gles11::FRAMEBUFFER_OES,
            gles11::COLOR_ATTACHMENT0_OES,
            gles11::FRAMEBUFFER_ATTACHMENT_OBJECT_NAME_OES,
            &mut attached,
        );
        (
            attached as GLuint,
            gles.CheckFramebufferStatusOES(gles11::FRAMEBUFFER_OES),
        )
    } else {
        (0, gles11::FRAMEBUFFER_COMPLETE_OES)
    };
    let use_bound_framebuffer = old_framebuffer != 0
        && attached_renderbuffer == renderbuffer
        && framebuffer_status == gles11::FRAMEBUFFER_COMPLETE_OES;
    let mut src_framebuffer: GLuint = 0;
    if !use_bound_framebuffer {
        // Resolve the guest's current draw target before any fallback bind.
        // Binding another FBO first is exactly what can discard tile-local
        // contents on Adreno/Mali when the guest left framebuffer zero (or a
        // different FBO) bound.
        gles.Finish();
        gles.GenFramebuffersOES(1, &mut src_framebuffer);
        gles.BindFramebufferOES(gles11::FRAMEBUFFER_OES, src_framebuffer);
        gles.FramebufferRenderbufferOES(
            gles11::FRAMEBUFFER_OES,
            gles11::COLOR_ATTACHMENT0_OES,
            gles11::RENDERBUFFER_OES,
            renderbuffer,
        );
    }

    // On tile-based GPUs (Mali, Adreno, PowerVR) the per-tile color buffer
    // isn't guaranteed to be resolved to the renderbuffer's main memory
    // until the driver decides to flush. glReadPixels is supposed to imply
    // a flush, but some drivers don't kick off the resolve aggressively
    // enough and we end up reading uninitialized (black) pixels. Force the
    // tile resolve here while the application's FBO is still bound.
    gles.Finish();

    // Read the pixels
    let size = (width_u32 as usize)
        .checked_mul(height_u32 as usize)
        .unwrap()
        .checked_mul(4)
        .unwrap();
    pixel_buffer.clear();
    pixel_buffer.reserve_exact(size);
    let before = Instant::now();
    gles.ReadPixels(
        0,
        0,
        width,
        height,
        gles11::RGBA,
        gles11::UNSIGNED_BYTE,
        pixel_buffer.as_mut_ptr() as *mut _,
    );
    log_dbg!(
        "glReadPixels(0, 0, {}, {}, …) took {:?}",
        width,
        height,
        Instant::now().saturating_duration_since(before)
    );
    pixel_buffer.set_len(size);

    if !use_bound_framebuffer {
        gles.DeleteFramebuffersOES(1, &src_framebuffer);
        gles.BindFramebufferOES(gles11::FRAMEBUFFER_OES, old_framebuffer);
    }

    (pixel_buffer, width_u32, height_u32)
}

/// Shader-based variant of the renderbuffer presenter, used when the
/// underlying driver is a real OpenGL ES 2.0 driver (no fixed-function
/// pipeline available).
///
/// This is intentionally simpler than the fixed-function version: we save the
/// minimum amount of ES 2.0 state, draw the textured quad with a small
/// dedicated shader program, and restore. The app's matrices, vertex pointers
/// etc. are not part of ES 2.0 state and thus need no save/restore.
unsafe fn present_renderbuffer_es2(
    gles: &mut dyn GLES,
    renderbuffer: GLuint,
    window_framebuffer: GLuint,
    viewport: (u32, u32, u32, u32),
    rotation_matrix: crate::matrix::Matrix<2>,
    virtual_cursor_visible_at: Option<(f32, f32, bool)>,
    present_finish: bool,
    context_token: usize,
) {
    use crate::gles::gles2_raw as gles2;

    // Save state we are about to clobber
    let mut old_program: GLint = 0;
    gles.GetIntegerv(gles2::CURRENT_PROGRAM, &mut old_program);
    let mut old_array_buffer: GLint = 0;
    gles.GetIntegerv(gles2::ARRAY_BUFFER_BINDING, &mut old_array_buffer);
    let mut old_elem_buffer: GLint = 0;
    gles.GetIntegerv(gles2::ELEMENT_ARRAY_BUFFER_BINDING, &mut old_elem_buffer);
    let mut old_active_texture: GLint = 0;
    gles.GetIntegerv(gles2::ACTIVE_TEXTURE, &mut old_active_texture);
    let mut old_active_texture_binding: GLint = 0;
    gles.GetIntegerv(gles2::TEXTURE_BINDING_2D, &mut old_active_texture_binding);
    gles.ActiveTexture(gles2::TEXTURE0);
    let mut old_texture0_binding: GLint = 0;
    gles.GetIntegerv(gles2::TEXTURE_BINDING_2D, &mut old_texture0_binding);
    gles.ActiveTexture(old_active_texture as GLenum);
    let mut old_framebuffer: GLint = 0;
    gles.GetIntegerv(gles2::FRAMEBUFFER_BINDING, &mut old_framebuffer);
    let mut old_viewport = [0i32; 4];
    gles.GetIntegerv(gles2::VIEWPORT, old_viewport.as_mut_ptr());
    let mut old_clear_color = [0.0f32; 4];
    gles.GetFloatv(gles2::COLOR_CLEAR_VALUE, old_clear_color.as_mut_ptr());
    let mut old_color_mask = [0u8; 4];
    gles.GetBooleanv(gles2::COLOR_WRITEMASK, old_color_mask.as_mut_ptr());
    let mut old_depth_mask = 0u8;
    gles.GetBooleanv(gles2::DEPTH_WRITEMASK, &mut old_depth_mask);
    let mut old_stencil_mask: GLint = 0;
    gles.GetIntegerv(gles2::STENCIL_WRITEMASK, &mut old_stencil_mask);
    let depth_test_was_on = gles.IsEnabled(gles2::DEPTH_TEST) != 0;
    let stencil_test_was_on = gles.IsEnabled(gles2::STENCIL_TEST) != 0;
    let cull_was_on = gles.IsEnabled(gles2::CULL_FACE) != 0;
    let blend_was_on = gles.IsEnabled(gles2::BLEND) != 0;
    let scissor_was_on = gles.IsEnabled(gles2::SCISSOR_TEST) != 0;

    // Resolve the renderbuffer supplied by `-presentRenderbuffer:` into a
    // texture with cached ES 2.0 objects. Passing this explicit object avoids
    // trusting a guest's later renderbuffer binding.
    let (width, height) = {
        let mut w: GLint = 0;
        let mut h: GLint = 0;
        gles.GetRenderbufferParameteriv(gles2::RENDERBUFFER, gles2::RENDERBUFFER_WIDTH, &mut w);
        gles.GetRenderbufferParameteriv(gles2::RENDERBUFFER, gles2::RENDERBUFFER_HEIGHT, &mut h);
        (w, h)
    };

    let present_objects = ensure_present_objects(gles);
    if width > 0 && height > 0 {
        // Prefer the application's already-bound FBO as the copy source.
        // Switching away from it before CopyTexSubImage2D can discard
        // unresolved tile data on Adreno, leaving the presentation texture
        // black even though the guest just rendered a valid frame. Keep the
        // cached source FBO as a fallback only when the bound FBO cannot be
        // verified to contain this drawable.
        let (attached_renderbuffer, framebuffer_status) = if old_framebuffer != 0 {
            let mut attached = 0;
            gles.GetFramebufferAttachmentParameteriv(
                gles2::FRAMEBUFFER,
                gles2::COLOR_ATTACHMENT0,
                gles2::FRAMEBUFFER_ATTACHMENT_OBJECT_NAME,
                &mut attached,
            );
            (
                attached as GLuint,
                gles.CheckFramebufferStatus(gles2::FRAMEBUFFER),
            )
        } else {
            (0, gles2::FRAMEBUFFER_COMPLETE)
        };
        let use_bound_framebuffer = old_framebuffer != 0
            && attached_renderbuffer == renderbuffer
            && framebuffer_status == gles2::FRAMEBUFFER_COMPLETE;
        if !use_bound_framebuffer {
            // Resolve the guest's current draw target before switching to the
            // cached source FBO. Otherwise a tile-based driver can discard the
            // frame while the fallback FBO is being bound.
            gles.Finish();
            let source_renderbuffer = PRESENT_SOURCE_RENDERBUFFER.with(|cell| cell.get());
            gles.BindFramebuffer(gles2::FRAMEBUFFER, present_objects.framebuffer);
            if source_renderbuffer != renderbuffer {
                gles.FramebufferRenderbuffer(
                    gles2::FRAMEBUFFER,
                    gles2::COLOR_ATTACHMENT0,
                    gles2::RENDERBUFFER,
                    renderbuffer,
                );
                PRESENT_SOURCE_RENDERBUFFER.with(|cell| cell.set(renderbuffer));
            }
            static LOGGED_FALLBACK: std::sync::Once = std::sync::Once::new();
            LOGGED_FALLBACK.call_once(|| {
                log!(
                    "GLES2 presenter: no suitable guest FBO is bound; using cached source FBO fallback"
                )
            });
        } else {
            static LOGGED_APP_FBO: std::sync::Once = std::sync::Once::new();
            LOGGED_APP_FBO.call_once(|| {
                log!(
                    "GLES2 presenter: copying from the guest's bound FBO to preserve Adreno tile contents"
                )
            });
        }

        // Copy into preallocated texture storage. CopyTexImage2D reallocates
        // that storage on every frame, while CopyTexSubImage2D does not.
        //
        // glCopyTexSubImage2D from the bound framebuffer is ordered after the
        // app's draws by the driver, so no explicit sync is needed here. A
        // glFinish() at this point drains the whole GPU pipeline every frame
        // (the most expensive thing a presenter can do on a tile-based GPU),
        // so it is opt-in: --present-finish / TOUCHHLE_PRESENT_FINISH=1.
        if present_finish {
            gles.Finish();
        }
        // The guest's scissor test clips glCopyTexSubImage2D readouts the
        // same way it clips the ES 1.1 path's copies (2D engines that keep
        // scissor enabled at present time produce glitched frames). Save the
        // enable state, copy with the test disabled, restore.
        let es2_scissor_was_on = gles.IsEnabled(gles2::SCISSOR_TEST) != 0;
        gles.Disable(gles2::SCISSOR_TEST);
        gles.ActiveTexture(gles2::TEXTURE0);
        gles.BindTexture(gles2::TEXTURE_2D, present_objects.texture);
        let texture_size = PRESENT_TEXTURE_SIZE.with(|cell| cell.get());
        if texture_size != Some((width, height)) {
            gles.TexImage2D(
                gles2::TEXTURE_2D,
                0,
                gles2::RGBA as GLint,
                width,
                height,
                0,
                gles2::RGBA,
                gles2::UNSIGNED_BYTE,
                std::ptr::null(),
            );
            PRESENT_TEXTURE_SIZE.with(|cell| cell.set(Some((width, height))));
        }
        gles.CopyTexSubImage2D(
            gles2::TEXTURE_2D,
            0,
            0,
            0,
            0,
            0,
            width,
            height,
        );
        if es2_scissor_was_on {
            gles.Enable(gles2::SCISSOR_TEST);
        }
        gles.BindFramebuffer(gles2::FRAMEBUFFER, old_framebuffer as _);
        static LOGGED: std::sync::Once = std::sync::Once::new();
        LOGGED.call_once(|| {
            log!("GLES2 presenter: cached FBO and glCopyTexSubImage2D fast path")
        });
    }

    gles.ActiveTexture(gles2::TEXTURE0);
    gles.BindTexture(gles2::TEXTURE_2D, present_objects.texture);
    gles.BindBuffer(gles2::ARRAY_BUFFER, present_objects.quad_vbo);
    gles.BindFramebuffer(gles2::FRAMEBUFFER, window_framebuffer);

    // Configure the destination viewport (the window) and clear.
    gles.Viewport(
        viewport.0 as _,
        viewport.1 as _,
        viewport.2 as _,
        viewport.3 as _,
    );
    gles.ClearColor(0.0, 0.0, 0.0, 1.0);
    gles.Disable(gles2::DEPTH_TEST);
    gles.Disable(gles2::STENCIL_TEST);
    gles.Disable(gles2::CULL_FACE);
    gles.Disable(gles2::BLEND);
    gles.Disable(gles2::SCISSOR_TEST);
    gles.ColorMask(gles2::TRUE, gles2::TRUE, gles2::TRUE, gles2::TRUE);
    gles.DepthMask(gles2::TRUE);
    gles.StencilMask(!0);
    // The window has no depth/stencil attachments (see window.rs) and the
    // quad is drawn with depth/stencil testing off; only colour needs
    // clearing (for the letterbox area).
    gles.Clear(gles2::COLOR_BUFFER_BIT);

    // Compile the present shader program once and cache it. If the shader
    // fails to compile/link (e.g. on a host with a buggy GLSL ES driver),
    // skip the present pass instead of crashing — the previous frame
    // remains on screen and the app continues to run.
    let Some(program) = ensure_present_program(gles) else {
        log!("Warning: present_renderbuffer_es2: present shader unavailable, skipping frame.");
        gles.UseProgram(if old_program > 0 {
            old_program as GLuint
        } else {
            0
        });
        gles.BindBuffer(gles2::ARRAY_BUFFER, old_array_buffer as GLuint);
        gles.BindBuffer(gles2::ELEMENT_ARRAY_BUFFER, old_elem_buffer as GLuint);
        gles.BindFramebuffer(gles2::FRAMEBUFFER, old_framebuffer as GLuint);
        gles.ActiveTexture(gles2::TEXTURE0);
        gles.BindTexture(gles2::TEXTURE_2D, old_texture0_binding as GLuint);
        gles.ActiveTexture(old_active_texture as GLenum);
        gles.BindTexture(gles2::TEXTURE_2D, old_active_texture_binding as GLuint);
        gles.Viewport(
            old_viewport[0],
            old_viewport[1],
            old_viewport[2],
            old_viewport[3],
        );
        gles.ClearColor(
            old_clear_color[0],
            old_clear_color[1],
            old_clear_color[2],
            old_clear_color[3],
        );
        gles.ColorMask(
            old_color_mask[0],
            old_color_mask[1],
            old_color_mask[2],
            old_color_mask[3],
        );
        gles.DepthMask(old_depth_mask);
        gles.StencilMask(old_stencil_mask as _);
        if depth_test_was_on {
            gles.Enable(gles2::DEPTH_TEST);
        }
        if stencil_test_was_on {
            gles.Enable(gles2::STENCIL_TEST);
        }
        if cull_was_on {
            gles.Enable(gles2::CULL_FACE);
        }
        if blend_was_on {
            gles.Enable(gles2::BLEND);
        }
        if scissor_was_on {
            gles.Enable(gles2::SCISSOR_TEST);
        }
        return;
    };

    // The presenter only changes the two attributes it owns. Querying all 16
    // slots on every frame is unnecessary driver traffic.
    let attribute_slots = [program.a_pos as GLuint, program.a_uv as GLuint];
    let mut attrib_was_enabled = [0u8; 2];
    for (slot, &attribute) in attrib_was_enabled.iter_mut().zip(attribute_slots.iter()) {
        let mut v: GLint = 0;
        gles.GetVertexAttribiv(attribute, gles2::VERTEX_ATTRIB_ARRAY_ENABLED, &mut v);
        *slot = v as u8;
    }

    gles.UseProgram(program.program);
    gles.Uniform1i(program.u_tex, 0);
    // Keep rotation around the center of the texture. Applying a raw
    // 0..1-space rotation sends one or both axes negative for landscape
    // orientations; with CLAMP_TO_EDGE that samples only the border texel and
    // is indistinguishable from a black frame on strict Adreno drivers.
    let r = crate::matrix::Matrix::<4>::from(&rotation_matrix);
    let to_center = crate::matrix::Matrix::<4>::translate_3d(-0.5, -0.5, 0.0);
    let from_center = crate::matrix::Matrix::<4>::translate_3d(0.5, 0.5, 0.0);
    let m = to_center.multiply(&r).multiply(&from_center);
    let cols = m.columns();
    gles.UniformMatrix4fv(
        program.u_tex_mat,
        1,
        gles2::FALSE,
        cols.as_ptr() as *const _,
    );

    // The full-screen quad lives in `present_objects.quad_vbo`, uploaded
    // above. Core-profile desktop GL (and the GL-on-Vulkan Zink driver used
    // by Winlator on Android) reject client-side vertex arrays with
    // GL_INVALID_OPERATION, so we must source the quad from a real buffer
    // object — never from a host-memory pointer.
    gles.BindBuffer(gles2::ARRAY_BUFFER, present_objects.quad_vbo);
    gles.EnableVertexAttribArray(program.a_pos as _);
    gles.EnableVertexAttribArray(program.a_uv as _);
    gles.VertexAttribPointer(
        program.a_pos as _,
        2,
        gles2::FLOAT,
        gles2::FALSE,
        16,
        std::ptr::null(),
    );
    gles.VertexAttribPointer(
        program.a_uv as _,
        2,
        gles2::FLOAT,
        gles2::FALSE,
        16,
        8usize as *const _,
    );
    gles.DrawArrays(gles2::TRIANGLES, 0, 6);

    // Cheat Engine-style trainer overlay (floating button + panel), drawn
    // with a dedicated ES 2.0 shader so it also works on native ES 2.0
    // drivers (Android), where the fixed-function GLES 1.x path is unusable.
    crate::gles::perf_overlay::draw_es2(gles, viewport, context_token);
    crate::trainer_ui::draw_es2(gles, viewport, context_token);
    crate::quick_menu::draw_es2(gles, viewport, context_token);

    // Optional: virtual cursor.
    if let Some((cx, cy, pressed)) = virtual_cursor_visible_at {
        let (vx, vy, vw, vh) = viewport;
        let x = cx - vx as f32;
        let y = cy - vy as f32;
        let radius = 10.0_f32;
        // Build quad in NDC.
        let mut q: [f32; 24] = [
            -1.0, -1.0, 0.0, 0.0, 1.0, -1.0, 1.0, 0.0, -1.0, 1.0, 0.0, 1.0, 1.0, -1.0, 1.0, 0.0,
            1.0, 1.0, 1.0, 1.0, -1.0, 1.0, 0.0, 1.0,
        ];
        for i in (0..q.len()).step_by(4) {
            q[i] = (q[i] * radius + x) / (vw as f32 / 2.0) - 1.0;
            q[i + 1] = 1.0 - (q[i + 1] * radius + y) / (vh as f32 / 2.0);
        }
        // Use a solid black quasi-shadow via a separate program, but to keep
        // things simple just sample our present texture with very low alpha
        // — skip for now if no separate cursor shader.
        let _ = pressed;
    }

    // Restore vertex attribute enabled state so the app's next draw works.
    for (&attribute, &was) in attribute_slots.iter().zip(attrib_was_enabled.iter()) {
        if was != 0 {
            gles.EnableVertexAttribArray(attribute);
        } else {
            gles.DisableVertexAttribArray(attribute);
        }
    }

    // Restore state we touched
    gles.UseProgram(if old_program > 0 {
        old_program as GLuint
    } else {
        0
    });
    gles.BindBuffer(gles2::ARRAY_BUFFER, old_array_buffer as GLuint);
    gles.BindBuffer(gles2::ELEMENT_ARRAY_BUFFER, old_elem_buffer as GLuint);
    gles.BindFramebuffer(gles2::FRAMEBUFFER, old_framebuffer as GLuint);
    gles.ActiveTexture(gles2::TEXTURE0);
    gles.BindTexture(gles2::TEXTURE_2D, old_texture0_binding as GLuint);
    gles.ActiveTexture(old_active_texture as GLenum);
    gles.BindTexture(gles2::TEXTURE_2D, old_active_texture_binding as GLuint);
    gles.Viewport(
        old_viewport[0],
        old_viewport[1],
        old_viewport[2] as _,
        old_viewport[3] as _,
    );
    gles.ClearColor(
        old_clear_color[0],
        old_clear_color[1],
        old_clear_color[2],
        old_clear_color[3],
    );
    gles.ColorMask(
        old_color_mask[0],
        old_color_mask[1],
        old_color_mask[2],
        old_color_mask[3],
    );
    gles.DepthMask(old_depth_mask);
    gles.StencilMask(old_stencil_mask as _);
    if depth_test_was_on {
        gles.Enable(gles2::DEPTH_TEST);
    }
    if stencil_test_was_on {
        gles.Enable(gles2::STENCIL_TEST);
    }
    if cull_was_on {
        gles.Enable(gles2::CULL_FACE);
    }
    if blend_was_on {
        gles.Enable(gles2::BLEND);
    }
    if scissor_was_on {
        gles.Enable(gles2::SCISSOR_TEST);
    }

    // The renderbuffer present is performed by us (the emulator), not by the
    // guest app. On a real iPhone OS device the equivalent work happens
    // inside `-[EAGLContext presentRenderbuffer:]` / the windowserver, behind
    // a context the app never error-checks. Our present path runs in the
    // *same* host GL context and therefore shares a single `glGetError` error
    // queue with the guest. Any error our own present operations might queue
    // (e.g. a benign INVALID_OPERATION from a strict desktop GL / Zink core
    // profile that the guest's lenient iOS PowerVR/Adreno driver would never
    // raise) would otherwise be read back by the guest's *next* GL call and
    // misattributed to the app — this is exactly the per-frame
    // `OpenGLES error 0x0502 in .../GlesHelper.mm` flood seen on Unity titles.
    // Drain the queue so the guest only ever observes errors it actually
    // caused, matching the real-device contract.
    while gles.GetError() != 0 {}
}

#[derive(Copy, Clone)]
struct PresentProgram {
    program: GLuint,
    a_pos: GLint,
    a_uv: GLint,
    u_tex: GLint,
    u_tex_mat: GLint,
}

/// Reusable GL objects for the ES 2.0 present path. Allocating and freeing a
/// framebuffer object and a texture on *every* presented frame is very
/// expensive on tile-based mobile GPUs (e.g. ARM Mali, Qualcomm Adreno):
/// object creation/teardown forces driver-side synchronisation and breaks
/// the driver's ability to pipeline frames, which shows up as severe stutter
/// in 60 FPS games (notably Unity titles, which present through this path).
/// Caching the objects and reusing them across frames removes that per-frame
/// churn entirely. Texture storage is allocated only when its dimensions
/// change; steady-state frames use `glCopyTexSubImage2D`.
#[derive(Copy, Clone)]
struct PresentObjects {
    framebuffer: GLuint,
    texture: GLuint,
    quad_vbo: GLuint,
}

/// One flag per entry of [gles1_on_gl2::CAPABILITIES]: does the driver reject
/// the enum?
type RejectedCaps = [bool; gles1_on_gl2::CAPABILITIES.len()];

thread_local! {
    static PRESENT_PROGRAM: std::cell::Cell<Option<PresentProgram>> =
        const { std::cell::Cell::new(None) };
    static PRESENT_OBJECTS: std::cell::Cell<Option<PresentObjects>> =
        const { std::cell::Cell::new(None) };
    static PRESENT_SOURCE_RENDERBUFFER: std::cell::Cell<GLuint> =
        const { std::cell::Cell::new(0) };
    static PRESENT_TEXTURE_SIZE: std::cell::Cell<Option<(GLint, GLint)>> =
        const { std::cell::Cell::new(None) };
    // Cached texture for the ES 1.1 (fixed-function) present path. Keeping it
    // alive avoids per-frame allocation and storage redefinition.
    static PRESENT_ES1_TEXTURE: std::cell::Cell<Option<(GLuint, GLint, GLint)>> =
        const { std::cell::Cell::new(None) };
    // Which entries of gles1_on_gl2::CAPABILITIES the driver rejects
    // (GL_INVALID_ENUM on glGetBooleanv). Probed once per context by the ES
    // 1.1 present path.
    static PRESENT_ES1_REJECTED_CAPS: std::cell::Cell<Option<RejectedCaps>> =
        const { std::cell::Cell::new(None) };
    /// GL object names are context-local unless contexts share a sharegroup.
    /// Keep every presenter cache tied to the EAGLContext host object so a game
    /// switching contexts cannot use another context's program/FBO/texture.
    static PRESENT_CONTEXT_TOKEN: std::cell::Cell<Option<usize>> =
        const { std::cell::Cell::new(None) };
}

/// Drop presenter object names whenever the current EAGL context changes.
/// The caches are thread-local for the normal fast path, but OpenGL names are
/// only valid in the context (or sharegroup) that created them.
fn invalidate_present_cache_for_context(context_token: usize) {
    let changed = PRESENT_CONTEXT_TOKEN.with(|cell| {
        let changed = cell.get() != Some(context_token);
        cell.set(Some(context_token));
        changed
    });
    if changed {
        PRESENT_PROGRAM.with(|cell| cell.set(None));
        PRESENT_OBJECTS.with(|cell| cell.set(None));
        PRESENT_SOURCE_RENDERBUFFER.with(|cell| cell.set(0));
        PRESENT_TEXTURE_SIZE.with(|cell| cell.set(None));
        PRESENT_ES1_TEXTURE.with(|cell| cell.set(None));
        PRESENT_ES1_REJECTED_CAPS.with(|cell| cell.set(None));
        log!("EAGL presenter: invalidated cached GL objects after EAGL context switch");
    }
}

#[cfg(test)]
#[test]
fn presenter_cache_is_invalidated_across_recreated_contexts() {
    let first = crate::gles::perf_overlay::allocate_context_token();
    let second = crate::gles::perf_overlay::allocate_context_token();
    assert_ne!(first, second);

    invalidate_present_cache_for_context(first);
    PRESENT_ES1_TEXTURE.with(|cell| cell.set(Some((42, 1024, 768))));
    invalidate_present_cache_for_context(second);
    PRESENT_ES1_TEXTURE.with(|cell| assert_eq!(cell.get(), None));
}

/// Drain the GL error queue. Returns whether there was anything in it.
///
/// Bounded, unlike a bare `while gles.GetError() != 0 {}`: a driver that
/// reports a sticky error (e.g. `GL_CONTEXT_LOST`) must not hang the
/// presenter.
unsafe fn drain_gl_errors(gles: &mut dyn GLES) -> bool {
    let mut any = false;
    for _ in 0..16 {
        if gles.GetError() == 0 {
            break;
        }
        any = true;
    }
    any
}

// --- Black-frame detector for PresentMode::Auto -----------------------------
//
// Presenting on the GPU is the fast path, but on some vendor OpenGL ES 1.1
// drivers it has produced black frames in the past even though the app's
// renderbuffer had content (that's why Android used to read every frame back
// to RAM instead). Rather than paying the readback tax on every device
// forever, `PresentMode::Auto` checks the GPU path during the first seconds
// of a session: sample a few pixels of the source renderbuffer and, after the
// present quad has been drawn, the same points in the window. If the source
// repeatedly has content while the window stays black, the GPU path is
// considered broken and the session switches to readback.
//
// Only native ES 1.1 backends are probed (the desktop GL fallback and the ES
// 2.0 shader presenter never used readback), and only while the decision is
// pending, so the probe costs nothing in steady state.

/// Frames on which the source was too dark to draw a conclusion do count, so
/// the probe can't run forever on a game with a long black loading screen.
const DIRECT_PRESENT_PROBE_MAX_FRAMES: u32 = 600;
/// Probe every frame at first, then only every Nth frame.
const DIRECT_PRESENT_PROBE_DENSE_FRAMES: u32 = 90;
const DIRECT_PRESENT_PROBE_SPARSE_INTERVAL: u32 = 5;
/// The brightest sampled source channel must exceed this for the frame to
/// count as "has content".
const DIRECT_PRESENT_PROBE_CONTENT_THRESHOLD: u8 = 24;
/// The window counts as black if no sampled channel exceeds this.
const DIRECT_PRESENT_PROBE_BLACK_THRESHOLD: u8 = 8;
/// Consecutive conclusive frames needed for a verdict.
const DIRECT_PRESENT_PROBE_VERDICT_FRAMES: u32 = 3;

static DIRECT_PRESENT_BROKEN: AtomicBool = AtomicBool::new(false);
static DIRECT_PRESENT_PROBE_DONE: AtomicBool = AtomicBool::new(false);
static DIRECT_PRESENT_PROBE_FRAMES: AtomicU32 = AtomicU32::new(0);
static DIRECT_PRESENT_PROBE_FAILURES: AtomicU32 = AtomicU32::new(0);
static DIRECT_PRESENT_PROBE_SUCCESSES: AtomicU32 = AtomicU32::new(0);

/// Has the detector concluded that presenting on the GPU doesn't work here?
fn direct_present_is_broken() -> bool {
    DIRECT_PRESENT_BROKEN.load(Ordering::Relaxed)
}

/// Should this frame be probed? Also advances the frame counter.
fn direct_present_probe_pending() -> bool {
    if DIRECT_PRESENT_PROBE_DONE.load(Ordering::Relaxed) {
        return false;
    }
    let frame = DIRECT_PRESENT_PROBE_FRAMES.fetch_add(1, Ordering::Relaxed);
    if frame >= DIRECT_PRESENT_PROBE_MAX_FRAMES {
        DIRECT_PRESENT_PROBE_DONE.store(true, Ordering::Relaxed);
        log!(
            "EAGL presenter: GPU present check finished without a verdict after {} frames \
             (the frames sampled were too dark to judge); keeping the GPU present path.",
            frame
        );
        return false;
    }
    frame < DIRECT_PRESENT_PROBE_DENSE_FRAMES
        || frame.is_multiple_of(DIRECT_PRESENT_PROBE_SPARSE_INTERVAL)
}

/// Read five single pixels (centre and the four quadrant centres) of the
/// `width`×`height` region at (`x0`, `y0`) of the currently bound framebuffer
/// and return the brightest colour channel seen. The set of sample points is
/// invariant under the 90° rotations and flips the presenter may apply, so the
/// same function can sample both the source renderbuffer and the window.
unsafe fn probe_max_channel(
    gles: &mut dyn GLES,
    x0: GLint,
    y0: GLint,
    width: GLint,
    height: GLint,
) -> u8 {
    if width <= 0 || height <= 0 {
        return 0;
    }
    let points = [
        (width / 2, height / 2),
        (width / 4, height / 4),
        (width * 3 / 4, height / 4),
        (width / 4, height * 3 / 4),
        (width * 3 / 4, height * 3 / 4),
    ];
    let mut max = 0u8;
    for (x, y) in points {
        let mut pixel = [0u8; 4];
        gles.ReadPixels(
            x0 + x,
            y0 + y,
            1,
            1,
            gles11::RGBA,
            gles11::UNSIGNED_BYTE,
            pixel.as_mut_ptr() as *mut _,
        );
        max = max.max(pixel[0]).max(pixel[1]).max(pixel[2]);
    }
    // Never leak probe errors into the guest's error queue.
    drain_gl_errors(gles);
    max
}

/// Second half of the detector: called after the present quad was drawn into
/// the window (framebuffer 0 bound), with the source sample from before.
unsafe fn probe_direct_present(
    gles: &mut dyn GLES,
    viewport: (u32, u32, u32, u32),
    source_max: u8,
) {
    if source_max < DIRECT_PRESENT_PROBE_CONTENT_THRESHOLD {
        // Nothing (bright enough) to compare against; try again later.
        return;
    }
    let (vx, vy, vw, vh) = viewport;
    let window_max = probe_max_channel(gles, vx as GLint, vy as GLint, vw as GLint, vh as GLint);
    let (failures, successes) = if window_max <= DIRECT_PRESENT_PROBE_BLACK_THRESHOLD {
        (
            DIRECT_PRESENT_PROBE_FAILURES.fetch_add(1, Ordering::Relaxed) + 1,
            DIRECT_PRESENT_PROBE_SUCCESSES.load(Ordering::Relaxed),
        )
    } else {
        (
            DIRECT_PRESENT_PROBE_FAILURES.load(Ordering::Relaxed),
            DIRECT_PRESENT_PROBE_SUCCESSES.fetch_add(1, Ordering::Relaxed) + 1,
        )
    };
    if successes >= DIRECT_PRESENT_PROBE_VERDICT_FRAMES {
        DIRECT_PRESENT_PROBE_DONE.store(true, Ordering::Relaxed);
        log!(
            "EAGL presenter: GPU present path verified on this driver \
             (window shows the rendered frame; {} frame(s) checked, {} looked black).",
            successes + failures,
            failures
        );
    } else if failures >= DIRECT_PRESENT_PROBE_VERDICT_FRAMES && successes == 0 {
        DIRECT_PRESENT_PROBE_DONE.store(true, Ordering::Relaxed);
        DIRECT_PRESENT_BROKEN.store(true, Ordering::Relaxed);
        log!(
            "EAGL presenter: the GPU present path produced a black window on {} consecutive \
             frames although the app's renderbuffer has content (brightest source sample {}). \
             Switching to glReadPixels readback for the rest of this session. \
             Use --present-mode=direct to override, or --present-mode=readback to skip this check.",
            failures,
            source_max
        );
    }
}

unsafe fn ensure_present_program(gles: &mut dyn GLES) -> Option<PresentProgram> {
    use crate::gles::gles2_raw as gles2;
    if let Some(p) = PRESENT_PROGRAM.with(|c| c.get()) {
        return Some(p);
    }

    let vs_src = b"\
        attribute vec2 aPos;\n\
        attribute vec2 aUV;\n\
        uniform mat4 uTexMat;\n\
        varying vec2 vUV;\n\
        void main() {\n\
            gl_Position = vec4(aPos, 0.0, 1.0);\n\
            vUV = (uTexMat * vec4(aUV, 0.0, 1.0)).xy;\n\
        }\0";
    // Alpha is forced to 1.0: the window surface may have an alpha channel
    // that the OS compositor honours (Android SurfaceFlinger), and an app's
    // renderbuffer alpha is meaningless for an opaque CAEAGLLayer.
    let fs_src = b"\
        precision mediump float;\n\
        varying vec2 vUV;\n\
        uniform sampler2D uTex;\n\
        void main() {\n\
            gl_FragColor = vec4(texture2D(uTex, vUV).rgb, 1.0);\n\
        }\0";

    let vs = gles.CreateShader(gles2::VERTEX_SHADER);
    let vs_ptr = vs_src.as_ptr() as *const _;
    let vs_len = (vs_src.len() - 1) as GLint;
    gles.ShaderSource(vs, 1, &vs_ptr, &vs_len);
    gles.CompileShader(vs);
    let mut ok: GLint = 0;
    gles.GetShaderiv(vs, gles2::COMPILE_STATUS, &mut ok);
    if ok == 0 {
        let mut buf = [0u8; 1024];
        let mut len: GLsizei = 0;
        gles.GetShaderInfoLog(vs, 1024, &mut len, buf.as_mut_ptr() as *mut _);
        let s = std::str::from_utf8(std::slice::from_raw_parts(
            buf.as_ptr() as *const u8,
            len as _,
        ))
        .unwrap_or("?");
        log!("Warning: present_es2 vertex shader compile failed: {s}");
        gles.DeleteShader(vs);
        return None;
    }

    let fs = gles.CreateShader(gles2::FRAGMENT_SHADER);
    let fs_ptr = fs_src.as_ptr() as *const _;
    let fs_len = (fs_src.len() - 1) as GLint;
    gles.ShaderSource(fs, 1, &fs_ptr, &fs_len);
    gles.CompileShader(fs);
    gles.GetShaderiv(fs, gles2::COMPILE_STATUS, &mut ok);
    if ok == 0 {
        let mut buf = [0u8; 1024];
        let mut len: GLsizei = 0;
        gles.GetShaderInfoLog(fs, 1024, &mut len, buf.as_mut_ptr() as *mut _);
        let s = std::str::from_utf8(std::slice::from_raw_parts(
            buf.as_ptr() as *const u8,
            len as _,
        ))
        .unwrap_or("?");
        log!("Warning: present_es2 fragment shader compile failed: {s}");
        gles.DeleteShader(vs);
        gles.DeleteShader(fs);
        return None;
    }

    let prog = gles.CreateProgram();
    gles.AttachShader(prog, vs);
    gles.AttachShader(prog, fs);
    // Bind to high attribute slots so we never collide with the app's
    // attribute layout (which typically starts at 0).
    gles.BindAttribLocation(prog, 6, b"aPos\0".as_ptr() as *const _);
    gles.BindAttribLocation(prog, 7, b"aUV\0".as_ptr() as *const _);
    gles.LinkProgram(prog);
    gles.GetProgramiv(prog, gles2::LINK_STATUS, &mut ok);
    if ok == 0 {
        let mut buf = [0u8; 1024];
        let mut len: GLsizei = 0;
        gles.GetProgramInfoLog(prog, 1024, &mut len, buf.as_mut_ptr() as *mut _);
        let s = std::str::from_utf8(std::slice::from_raw_parts(
            buf.as_ptr() as *const u8,
            len as _,
        ))
        .unwrap_or("?");
        log!("Warning: present_es2 program link failed: {s}");
        gles.DeleteShader(vs);
        gles.DeleteShader(fs);
        gles.DeleteProgram(prog);
        return None;
    }

    let a_pos = gles.GetAttribLocation(prog, b"aPos\0".as_ptr() as *const _);
    let a_uv = gles.GetAttribLocation(prog, b"aUV\0".as_ptr() as *const _);
    let u_tex = gles.GetUniformLocation(prog, b"uTex\0".as_ptr() as *const _);
    let u_tex_mat = gles.GetUniformLocation(prog, b"uTexMat\0".as_ptr() as *const _);

    let result = PresentProgram {
        program: prog,
        a_pos,
        a_uv,
        u_tex,
        u_tex_mat,
    };
    PRESENT_PROGRAM.with(|c| c.set(Some(result)));
    Some(result)
}

/// Returns the reusable framebuffer + texture used by [present_renderbuffer_es2]
/// to resolve the guest renderbuffer into a presentable texture.
///
/// These objects used to be created with `glGenFramebuffers` / `glGenTextures`
/// and destroyed with `glDeleteFramebuffers` / `glDeleteTextures` on *every*
/// presented frame. On tile-based mobile GPUs (ARM Mali, Qualcomm Adreno,
/// PowerVR) allocating and freeing driver objects every frame is surprisingly
/// expensive — it forces driver-side bookkeeping and synchronization that can
/// dominate frame time, which is why a powerful Helio G99 (Mali-G57) could run
/// a Unity game far more slowly than a real iPhone 4S, where iOS forces 60Hz
/// v-sync and never does this churn. Caching the objects per thread and reusing
/// them every frame removes that churn entirely.
unsafe fn ensure_present_objects(gles: &mut dyn GLES) -> PresentObjects {
    if let Some(o) = PRESENT_OBJECTS.with(|c| c.get()) {
        return o;
    }

    let mut framebuffer: GLuint = 0;
    gles.GenFramebuffers(1, &mut framebuffer);
    let mut texture: GLuint = 0;
    gles.GenTextures(1, &mut texture);
    let mut quad_vbo: GLuint = 0;
    gles.GenBuffers(1, &mut quad_vbo);
    gles.BindBuffer(crate::gles::gles2_raw::ARRAY_BUFFER, quad_vbo);
    #[rustfmt::skip]
    let verts: [f32; 24] = [
        -1.0, -1.0, 0.0, 0.0,
         1.0, -1.0, 1.0, 0.0,
        -1.0,  1.0, 0.0, 1.0,
         1.0, -1.0, 1.0, 0.0,
         1.0,  1.0, 1.0, 1.0,
        -1.0,  1.0, 0.0, 1.0,
    ];
    gles.BufferData(
        crate::gles::gles2_raw::ARRAY_BUFFER,
        std::mem::size_of_val(&verts) as isize,
        verts.as_ptr().cast(),
        crate::gles::gles2_raw::STATIC_DRAW,
    );
    gles.ActiveTexture(crate::gles::gles2_raw::TEXTURE0);
    gles.BindTexture(crate::gles::gles2_raw::TEXTURE_2D, texture);
    gles.TexParameteri(
        crate::gles::gles2_raw::TEXTURE_2D,
        crate::gles::gles2_raw::TEXTURE_MIN_FILTER,
        crate::gles::gles2_raw::LINEAR as _,
    );
    gles.TexParameteri(
        crate::gles::gles2_raw::TEXTURE_2D,
        crate::gles::gles2_raw::TEXTURE_MAG_FILTER,
        crate::gles::gles2_raw::LINEAR as _,
    );
    gles.TexParameteri(
        crate::gles::gles2_raw::TEXTURE_2D,
        crate::gles::gles2_raw::TEXTURE_WRAP_S,
        crate::gles::gles2_raw::CLAMP_TO_EDGE as _,
    );
    gles.TexParameteri(
        crate::gles::gles2_raw::TEXTURE_2D,
        crate::gles::gles2_raw::TEXTURE_WRAP_T,
        crate::gles::gles2_raw::CLAMP_TO_EDGE as _,
    );

    let result = PresentObjects {
        framebuffer,
        texture,
        quad_vbo,
    };
    PRESENT_SOURCE_RENDERBUFFER.with(|cell| cell.set(0));
    PRESENT_TEXTURE_SIZE.with(|cell| cell.set(None));
    PRESENT_OBJECTS.with(|c| c.set(Some(result)));
    result
}

/// Copies the pixels in a renderbuffer bound to `GL_RENDERBUFFER_BINDING_OES`
/// (which should be provided by the app) to a texture and presents it with
/// [present_frame], trying to avoid noticeably modifying OpenGL ES state while
/// doing so. The front and back buffers are then swapped.
unsafe fn present_renderbuffer(
    env: &mut Environment,
    renderbuffer: GLuint,
    _drawable: id,
    options: EaglPresentOptions,
    context_token: usize,
) {
    // Capture this up front because the env borrow is moved into the GL
    // context machinery below.
    let trace_gl_errors = options.trace_gl_errors;

    // Save these for when we need to draw the frame
    let viewport = env.window.as_mut().unwrap().viewport();
    let device_family = env.window.as_mut().unwrap().device_family();
    let device_orientation = env.window.as_mut().unwrap().current_rotation();
    // For iPad apps in a non-portrait orientation, the UIKit auto-rotation
    // path (`UIWindow addSubview:` in ui_window.rs) applies a rotation
    // transform to the rootViewController's view so that the app, which
    // typically draws content "upright" inside the EAGL layer's portrait
    // bounds, ends up rotated for landscape display when Core Animation
    // composites it. touchHLE bypasses CA composition for EAGL apps that
    // call `presentRenderbuffer:` directly, so we have to replicate that
    // additional rotation here. Without it, iPad landscape games (e.g.
    // Plants vs. Zombies HD) render upside-down. iPhone-only landscape
    // games (e.g. Plants vs. Zombies, the iPhone version) typically rotate
    // their drawing themselves, so we must NOT apply the extra rotation
    // for them.
    // By default, retain the legacy iPad/non-portrait heuristic. The opt-in
    // `--rotation-fix` path instead checks whether UIKit actually applied the
    // autorotation transform, avoiding an extra 180-degree turn for apps that
    // do not rotate their root view.
    //
    // This is only a fallback: `layer_rotation` is the rotation Core Animation
    // itself would apply, measured from the layer tree. An extra 180 degrees
    // cannot stand in for it, because the tree can be rotated by a quarter turn
    // (and the two answers differ by 90 degrees, not 0 or 180).
    let rotation_compensation_trigger = if env.options.rotation_fix {
        env.framework_state.uikit.autorotation_transform_applied
    } else {
        device_family.is_ipad()
    };
    let needs_autorotation_compensation = rotation_compensation_trigger
        && !matches!(
            device_orientation,
            crate::window::DeviceOrientation::Portrait
        )
        && options.layer_rotation == 0.0;
    // PERF: cached read-once flag; present_renderbuffer runs every frame.
    let rotation_matrix = if crate::env_flag_cached!("TOUCHHLE_DISABLE_PRESENT_ROTATION") {
        log_once!(
            "TOUCHHLE_DISABLE_PRESENT_ROTATION=1: presenting EAGL renderbuffer without texture rotation"
        );
        crate::matrix::Matrix::<2>::identity()
    } else if needs_autorotation_compensation {
        env.window
            .as_mut()
            .unwrap()
            .rotation_matrix()
            .multiply(&crate::matrix::Matrix::z_rotation(std::f32::consts::PI))
    } else {
        env.window.as_mut().unwrap().rotation_matrix()
    };
    // Fixed present-rotation override: the `--present-rotate=N` option (per-app
    // via the default options file) takes precedence, then the live
    // TOUCHHLE_PRESENT_ROTATE env var, else the orientation-derived matrix.
    let rotate_override = env
        .options
        .present_rotate
        .map(|d| d.to_string())
        .or_else(|| crate::env_var_cached!("TOUCHHLE_PRESENT_ROTATE").map(|s| s.to_string()));
    let rotation_matrix = match rotate_override.as_deref() {
        Some("0") => crate::matrix::Matrix::<2>::identity(),
        Some("90") => crate::matrix::Matrix::z_rotation(std::f32::consts::FRAC_PI_2),
        Some("180") => crate::matrix::Matrix::z_rotation(std::f32::consts::PI),
        Some("270") => crate::matrix::Matrix::z_rotation(-std::f32::consts::FRAC_PI_2),
        _ => rotation_matrix,
    };
    // The iOS ES 2.0 direct presenter bypasses Core Animation, which is where
    // the rotation would otherwise have been applied, and the guest has already
    // drawn in display orientation, so rotating again would turn it sideways.
    let rotation_matrix = if options.skip_rotation && rotate_override.is_none() {
        crate::matrix::Matrix::<2>::identity()
    } else if options.layer_rotation != 0.0 {
        // The layer transform is what Core Animation would have applied, so
        // compose it with the window's rotation instead of guessing.
        rotation_matrix.multiply(&crate::matrix::Matrix::z_rotation(options.layer_rotation))
    } else {
        rotation_matrix
    };
    let virtual_cursor_visible_at = env.window.as_mut().unwrap().virtual_cursor_visible_at();

    if crate::env_flag_cached!("TOUCHHLE_TRACE_PRESENT") {
        static GEOM_LOGGED: std::sync::Once = std::sync::Once::new();
        GEOM_LOGGED.call_once(|| {
            log!(
                "PRESENT-GEOM viewport={:?} device_family_ipad={} orientation={:?} needs_autorot={} layer_rotation={}",
                viewport,
                device_family.is_ipad(),
                device_orientation,
                needs_autorotation_compensation,
                options.layer_rotation
            );
        });
    }

    let Some(gles_ctx) = super::get_thread_context(
        &mut env.framework_state.opengles,
        &mut env.objc,
        env.current_thread,
    ) else {
        // No current EAGL context for this thread. The caller already
        // returned `true` from `presentRenderbuffer:` (since the renderbuffer
        // existed in `renderbuffer_drawable_bindings`), so the only thing
        // left to do here is skip the host-side composition step and pump
        // a single SDL swap so the window keeps animating. Without this
        // guard the emulator used to abort with the panic visible in
        // HyperHLE log #5 (`opengles.rs:56` — Option::unwrap on a None
        // current_ctx).
        log!(
            "present_renderbuffer: no current GL context for thread {}; \
             skipping host present and swapping window only.",
            env.current_thread
        );
        if let Some(window) = env.window.as_mut() {
            window.swap_window();
        }
        return;
    };

    let mut gles_boxed = gles_ctx.make_current(env.window.as_mut().unwrap());
    let gles = gles_boxed.as_mut();

    invalidate_present_cache_for_context(context_token);

    // DIAG (TOUCHHLE_TRACE_PRESENT=1): dump the raw renderbuffer the game drew,
    // at its native size, placed EARLY so later early-returns can't skip it.
    if crate::env_flag_cached!("TOUCHHLE_TRACE_PRESENT") {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        if matches!(n, 600 | 1200 | 1700) {
            let (px, w, h) = read_renderbuffer(gles, renderbuffer, Vec::new());
            let mut out = format!("P6\n{} {}\n255\n", w, h).into_bytes();
            for c in px.chunks_exact(4) {
                out.extend_from_slice(&[c[0], c[1], c[2]]);
            }
            match std::fs::write(format!("rb_dump_{}.ppm", n), &out) {
                Ok(()) => log!("DUMP-RB wrote rb_dump_{}.ppm ({}x{})", n, w, h),
                Err(e) => log!("DUMP-RB failed: {}", e),
            }
        }
    }

    // Per-section diagnostic checkpoint helper. When --trace-gl-errors is
    // on, this drains GL errors after each named section of
    // present_renderbuffer and logs the *first* time each named section
    // produces an error. Without an explicit per-section split, all
    // errors generated by our host-side present logic accumulate into
    // the GL error queue and are either silently drained at the end
    // (see below) or get incorrectly attributed to whatever guest gl*
    // call happens next, which makes it impossible to tell which of our
    // own state queries / FBO operations / texture uploads is the actual
    // culprit on a strict native ES 1.1 driver (e.g. ARM Mali, Qualcomm
    // Adreno's ES 1.1 surface). The caller-provided AtomicBool means
    // each unique checkpoint logs at most once for the entire app run,
    // so this stays out of normal logs even when an error reproduces
    // every frame. Implemented as a free function (not a macro) so each
    // call site is a clean re-borrow of `gles` for NLL.
    unsafe fn present_check(
        gles: &mut dyn GLES,
        trace: bool,
        seen: &std::sync::atomic::AtomicBool,
        section: &'static str,
    ) {
        if !trace {
            return;
        }
        let err = gles.GetError();
        if err == 0 {
            return;
        }
        if !seen.swap(true, std::sync::atomic::Ordering::Relaxed) {
            log!(
                "[--trace-gl-errors] present_renderbuffer: section {:?} produced GL error {:#x} [this log will only be shown once]",
                section,
                err
            );
        }
        while gles.GetError() != 0 {}
    }

    // Drain anything the guest might have left in the queue so we can
    // attribute new errors below to our own code, not to whatever
    // happened before presentRenderbuffer:.
    if trace_gl_errors {
        while gles.GetError() != 0 {}
    }
    {
        static SEEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        present_check(gles, trace_gl_errors, &SEEN, "after make_current");
    }

    // On a real OpenGL ES 2.0 driver (Android etc.) the fixed-function code
    // path below cannot be used — there is no glMatrixMode / glColor4f /
    // glEnableClientState / glVertexPointer. Use a small dedicated
    // shader-based presenter instead.
    if gles.is_es2() && !gles.is_translator() {
        present_renderbuffer_es2(
            gles,
            renderbuffer,
            options.window_framebuffer,
            viewport,
            rotation_matrix,
            virtual_cursor_visible_at,
            options.present_finish,
            context_token,
        );
        std::mem::drop(gles_boxed);
        env.window.as_mut().unwrap().swap_window();
        return;
    }
    if gles.is_translator() {
        // Only reachable with --present-mode=direct: the ES 1.1-on-ES 2.0
        // translator normally presents through readback (see
        // presentRenderbuffer:). The translator emulates the whole
        // fixed-function API, so the ES 1.1 presenter below is at least
        // plausible on it, but this combination is not well tested.
        log_once!(
            "EAGL presenter: using the fixed-function presenter on the ES 1.1-on-ES 2.0 translator (experimental, forced by --present-mode=direct)"
        );
    }
    // Black-frame detector for PresentMode::Auto (see probe_direct_present).
    let probe_active = options.present_mode == PresentMode::Auto
        && gles.is_native_es1()
        && direct_present_probe_pending();

    // We can't directly copy the content of the renderbuffer to the default
    // framebuffer (the window), but if we attach it to a framebuffer object, we
    // can use glCopyTexImage2D() to copy it to a texture, which we can then
    // draw to the default framebuffer via a textured quad, which can be
    // rotated, scaled or letterboxed as appropriate.

    let (width, height) = get_renderbuffer_size(gles);
    {
        static SEEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

        present_check(
            gles,
            trace_gl_errors,
            &SEEN,
            "after renderbuffer-size queries",
        );
    }

    {
        use std::sync::atomic::{AtomicBool, Ordering};
        static SEEN: AtomicBool = AtomicBool::new(false);
        if !SEEN.swap(true, Ordering::Relaxed) {
            log!(
                "First present_renderbuffer ES1.1 path: renderbuffer={} size={}x{} (npot_w={} npot_h={}) [this log will only be shown once]",
                renderbuffer,
                width,
                height,
                !(width as u32).is_power_of_two(),
                !(height as u32).is_power_of_two(),
            );
        }
    }

    // To avoid confusing the guest app, we need to be able to undo any
    // state changes we make.
    let old_framebuffer: GLuint = get_int(gles, gles11::FRAMEBUFFER_BINDING_OES) as _;
    let old_active_texture: GLuint = get_int(gles, gles11::ACTIVE_TEXTURE) as _;
    // The present texture must be bound on unit 0: the guest may have left a
    // different unit active, and binding to it would both draw the present
    // quad with the wrong texture and corrupt the guest's unit binding.
    gles.ActiveTexture(gles11::TEXTURE0);
    let old_texture_2d: GLuint = get_int(gles, gles11::TEXTURE_BINDING_2D) as _;
    {
        static SEEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

        present_check(
            gles,
            trace_gl_errors,
            &SEEN,
            "after old_framebuffer/old_texture queries",
        );
    }

    // Tile-based GPUs (e.g. Qualcomm Adreno, ARM Mali) defer rasterisation:
    // the draws the app issued into the renderbuffer may still live in
    // fast tile-local memory at the moment the app calls
    // -presentRenderbuffer:. They are only "resolved" to the
    // renderbuffer's actual main-memory storage at well-defined points,
    // typically eglSwapBuffers, FBO unbinding, glReadPixels, or
    // CopyTexImage2D from the bound FBO.
    //
    // We *used* to create our own throwaway FBO (`src_framebuffer`),
    // attach the app's renderbuffer to it, and CopyTexImage2D from
    // there. That worked on lenient drivers (Mesa / llvmpipe / Apple
    // PowerVR / Adreno ES 3.x layer) but failed on ARM Mali r32p1
    // (Mali-G57 MC2 in OpenGL ES-CM 1.1 mode), which on this code path
    // produces an all-zero renderbuffer. The renderbuffer-content probe
    // we added in the previous PR confirms this on real hardware:
    // BL=BR=TL=TR=(0,0,0,255) at every first frame.
    //
    // Why did it fail on Mali? The act of `BindFramebufferOES(..,
    // src_framebuffer)` unbinds the FBO the app rendered into, and on
    // Mali r32p1 that unbind *discards* the tile data instead of
    // resolving it to the renderbuffer's main memory. By the time
    // CopyTexImage2D runs against `src_framebuffer`, the renderbuffer
    // is empty. Forcing a 1-pixel glReadPixels + glFinish *before* the
    // unbind also turned out not to be enough — the driver's resolve
    // heuristics on that path still produce zeros (verified by user
    // log on Mali-G57 MC2 with the previous attempt).
    //
    // The robust fix is to skip the FBO switch entirely: the app's own
    // FBO already has the renderbuffer attached at color attachment 0
    // (that's how the app rendered into it in the first place), so we
    // can CopyTexImage2D directly from the currently-bound FBO. That
    // way Mali never has a reason to discard the tile data: the same
    // FBO that the draws went into is the FBO we're now reading from.
    //
    // The standard iPhone EAGL pattern guarantees that the app's FBO has
    // the drawable attached at COLOR_ATTACHMENT0. Verify that assumption
    // instead of treating every non-zero FBO as a valid source: some engines
    // bind a temporary depth/post-processing FBO immediately before present.
    // Out of paranoia we keep a fallback that creates a temporary FBO and
    // attaches the renderbuffer when no suitable app FBO is bound.
    let (attached_renderbuffer, framebuffer_status) = if old_framebuffer != 0 {
        let mut attached = 0;
        gles.GetFramebufferAttachmentParameterivOES(
            gles11::FRAMEBUFFER_OES,
            gles11::COLOR_ATTACHMENT0_OES,
            gles11::FRAMEBUFFER_ATTACHMENT_OBJECT_NAME_OES,
            &mut attached,
        );
        (
            attached as GLuint,
            gles.CheckFramebufferStatusOES(gles11::FRAMEBUFFER_OES),
        )
    } else {
        (0, gles11::FRAMEBUFFER_COMPLETE_OES)
    };
    let used_app_fbo = old_framebuffer != 0
        && attached_renderbuffer == renderbuffer
        && framebuffer_status == gles11::FRAMEBUFFER_COMPLETE_OES;
    let mut src_framebuffer: GLuint = 0;
    if !used_app_fbo {
        // Resolve before the fallback bind; switching FBOs first can discard
        // tile-local contents on mobile drivers.
        gles.Finish();
        gles.GenFramebuffersOES(1, &mut src_framebuffer);
        gles.BindFramebufferOES(gles11::FRAMEBUFFER_OES, src_framebuffer);
        gles.FramebufferRenderbufferOES(
            gles11::FRAMEBUFFER_OES,
            gles11::COLOR_ATTACHMENT0_OES,
            gles11::RENDERBUFFER_OES,
            renderbuffer,
        );
    }
    {
        static SEEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

        present_check(
            gles,
            trace_gl_errors,
            &SEEN,
            if used_app_fbo {
                "after using app's bound FBO as copy source (no FBO switch)"
            } else {
                "after fallback FBO create+bind+attach (old_framebuffer==0)"
            },
        );
    }

    // Cache the ES1 texture across frames (but the context helper above
    // drops it before it could be used by another EAGLContext). Steady-state
    // frames can then use CopyTexSubImage2D without object churn or storage
    // reallocation.
    let (texture, storage_valid) =
        if let Some((texture, cached_width, cached_height)) = PRESENT_ES1_TEXTURE.with(|c| c.get()) {
            gles.BindTexture(gles11::TEXTURE_2D, texture);
            (texture, cached_width == width && cached_height == height)
        } else {
            let mut texture = 0;
            gles.GenTextures(1, &mut texture);
            gles.BindTexture(gles11::TEXTURE_2D, texture);

            // Set the texture parameters before the first copy: the
            // renderbuffer is typically non-power-of-two, and strict ES 1.1
            // drivers reject CopyTexImage2D into a texture that still uses the
            // GL_REPEAT default wrap mode.
            gles.TexParameteri(
                gles11::TEXTURE_2D,
                gles11::TEXTURE_MIN_FILTER,
                gles11::LINEAR as _,
            );
            gles.TexParameteri(
                gles11::TEXTURE_2D,
                gles11::TEXTURE_MAG_FILTER,
                gles11::LINEAR as _,
            );
            gles.TexParameteri(
                gles11::TEXTURE_2D,
                gles11::TEXTURE_WRAP_S,
                gles11::CLAMP_TO_EDGE as _,
            );
            gles.TexParameteri(
                gles11::TEXTURE_2D,
                gles11::TEXTURE_WRAP_T,
                gles11::CLAMP_TO_EDGE as _,
            );
            (texture, false)
        };
    // glCopyTex(Sub)Image2D from the bound framebuffer is ordered after the
    // app's draws into it by the driver; that's what the spec guarantees and
    // what every driver we've seen honours. We used to glFinish() here
    // regardless, on the suspicion that a Mali-G57 r32p1 "title menu renders
    // as a uniform colour" bug was a missing tile resolve — it turned out to
    // be mipmap-incomplete textures (see fix_texture_min_filter). A full
    // pipeline drain every frame is the single most expensive thing a
    // presenter can do on a tile-based GPU (it serialises CPU and GPU and
    // defeats the driver's frame pipelining), so it is opt-in now:
    // --present-finish / TOUCHHLE_PRESENT_FINISH=1.
    // Tile-based GPUs resolve the app's draws to the renderbuffer's main
    // memory at well-defined sync points. CopyTex(Sub)Image2D is spec-ordered
    // after the app's draws, but real ES 1.1 surfaces (Adreno/Mali, native or
    // over ANGLE) have historically needed an explicit drain for alpha-blended
    // 2D scenes (many small quads): without it, the copy can race the tile
    // resolve and produce partially-drawn / garbled frames, while 3D scenes
    // (few big depth-tested draws) usually happen to be fine. Keep the full
    // drain as the default on native ES 1.1 backends (it was unconditional
    // before the GPU-present rework); on ES 2.0 shader backends the resolve
    // is reliable, so it stays opt-in there.
    let native_es1 = gles.is_native_es1();
    let finish_before_copy = options.present_finish
        || (native_es1 && !crate::env_flag_cached!("TOUCHHLE_NO_PRESENT_FINISH"));
    if finish_before_copy && !options.present_finish {
        log_once!(
            "EAGL presenter: native ES1.1 backend - forcing glFinish before the \
             renderbuffer copy (tile-resolve safety for 2D alpha-blended games; \
             opt out with TOUCHHLE_NO_PRESENT_FINISH=1)."
        );
    }
    if finish_before_copy {
        gles.Finish();
    }
    // A guest that renders 2D UI (level-select lists, HUD panels, ...) very
    // commonly leaves GL_SCISSOR_TEST enabled at present time — iOS titles
    // never notice because iOS's own present path ignores scissor when
    // resolving the drawable. Our CopyTex(Sub)Image2D readout is NOT
    // ignored: pixels outside the guest's scissor box are never copied, so
    // every frame would present only the scissored sub-region while the
    // rest of the texture keeps stale content from older frames (the
    // "Geometry Dash glitched frame" symptom). Disable the test for the
    // copy; the generic caps save/restore loop below puts the enable flag
    // back for the guest after the present quad.
    let old_scissor_box: [GLint; 4] = get_ints(gles, gles11::SCISSOR_BOX);
    gles.Disable(gles11::SCISSOR_TEST);
    // Same story for the pack alignment: glCopyTex(Sub)Image2D reads rows
    // with GL_PACK_ALIGNMENT, and a guest that uploaded odd-stride data
    // with a non-default alignment skews every copied row and mangles the
    // presented image. RGBA8 rows are always 4-byte aligned, so alignment
    // 1 is always valid here and never changes the image.
    let old_pack_alignment: GLint = get_int(gles, gles11::PACK_ALIGNMENT);
    gles.PixelStorei(gles11::PACK_ALIGNMENT, 1);
    if storage_valid {
        // Steady state: copy into the preallocated storage without
        // redefining it.
        gles.CopyTexSubImage2D(
            gles11::TEXTURE_2D,
            0,
            0,
            0,
            0,
            0,
            width,
            height,
        );
    } else {
        gles.CopyTexImage2D(
            gles11::TEXTURE_2D,
            0,
            gles11::RGBA as _,
            0,
            0,
            width,
            height,
            0,
        );
        PRESENT_ES1_TEXTURE.with(|c| c.set(Some((texture, width, height))));
    }
    {
        static SEEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

        present_check(
            gles,
            trace_gl_errors,
            &SEEN,
            "after Finish + CopyTexImage2D",
        );
    }
    // Restore the guest's pixel-store state now that the copy is done; the
    // scissor box is restored too so a guest that reads pixels itself
    // (screenshots) is unaffected. The scissor TEST stays disabled until
    // the generic caps restore loop re-enables it after the present quad.
    gles.PixelStorei(gles11::PACK_ALIGNMENT, old_pack_alignment);
    gles.Scissor(
        old_scissor_box[0],
        old_scissor_box[1],
        old_scissor_box[2],
        old_scissor_box[3],
    );
    // Black-frame detector, part 1: sample the source while the framebuffer
    // we copied from is still bound.
    let probe_source_max = if probe_active {
        Some(probe_max_channel(gles, 0, 0, width, height))
    } else {
        None
    };
    // Diagnostic probe: read a few pixels of the renderbuffer the guest
    // just rendered into, so we can tell apart "renderbuffer is empty /
    // all-black" (a guest-side or attach-side / tile-resolve bug) from
    // "renderbuffer has content but present_frame is mis-displaying it"
    // (a present-side bug). At this point the read source FBO is
    // whichever copy source we used above: the app's own FBO
    // (used_app_fbo, normal iOS pattern) or the throwaway src_framebuffer
    // (fallback for old_framebuffer == 0). Either way, ReadPixels reads
    // from FRAMEBUFFER_BINDING, so the values logged here describe the
    // pixels CopyTexImage2D just copied.
    //
    // We sample several frames (the very first frame is often just a
    // glClear and shows zeros even on healthy drivers — we need to also
    // see what later "real game" frames look like) and we sample the
    // image centre as well as the corners (the corners on UI screens
    // are often legitimately black, while the centre is where the
    // actual artwork lives, so that's a much better "did anything
    // render?" signal). Gated on --trace-gl-errors.
    if trace_gl_errors {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static PROBE_COUNT: AtomicUsize = AtomicUsize::new(0);
        let count = PROBE_COUNT.fetch_add(1, Ordering::Relaxed);
        // Probe at frames 0, 1, 5, 30, 120, 600 — covers "first frame
        // before app drew anything", "second frame", a couple of early
        // splash frames, ~half-second-in, ~2s-in, ~10s-in. After that
        // every 600 frames so a long-running session still gives us
        // periodic snapshots without flooding the log.
        let should_log = matches!(count, 0 | 1 | 5 | 30 | 120 | 600) || count.is_multiple_of(600);
        if should_log {
            // 5 single-pixel reads: 4 corners + centre.
            let mut tl: [u8; 4] = [0; 4];
            let mut tr: [u8; 4] = [0; 4];
            let mut bl: [u8; 4] = [0; 4];
            let mut br: [u8; 4] = [0; 4];
            let mut cc: [u8; 4] = [0; 4];
            let xmax = width.saturating_sub(1).max(0);
            let ymax = height.saturating_sub(1).max(0);
            let xmid = width / 2;
            let ymid = height / 2;
            for (x, y, buf) in [
                (0, 0, &mut bl),
                (xmax, 0, &mut br),
                (0, ymax, &mut tl),
                (xmax, ymax, &mut tr),
                (xmid, ymid, &mut cc),
            ] {
                gles.ReadPixels(
                    x,
                    y,
                    1,
                    1,
                    gles11::RGBA,
                    gles11::UNSIGNED_BYTE,
                    buf.as_mut_ptr() as *mut _,
                );
            }
            // Drain any GL error this probe may have generated; it must
            // not leak into the guest-visible error queue.
            while gles.GetError() != 0 {}
            // Snapshot the guest's current GL state — this tells us how
            // the *app* was set up at presentRenderbuffer entry, which
            // is the moment the previous frame's draws were issued.
            // Useful for diagnosing "renderbuffer is uniform colour"
            // bugs that look like draws no-op'd: does the app even
            // have a vertex array enabled? Was a texture bound? Is
            // alpha-test or depth-test rejecting fragments? etc.
            //
            // Each query is wrapped with an error-drain so a strict
            // driver that rejects an enum just shows up as 0 in the
            // log without poisoning the queue for the next query.
            let qb = |gles: &mut dyn GLES, name: GLenum| -> GLboolean {
                while gles.GetError() != 0 {}
                let mut v: GLboolean = gles11::FALSE;
                gles.GetBooleanv(name, &mut v);
                while gles.GetError() != 0 {}
                v
            };
            let qi = |gles: &mut dyn GLES, name: GLenum| -> GLint {
                while gles.GetError() != 0 {}
                let mut v: GLint = 0;
                gles.GetIntegerv(name, &mut v);
                while gles.GetError() != 0 {}
                v
            };
            let qf4 = |gles: &mut dyn GLES, name: GLenum| -> [GLfloat; 4] {
                while gles.GetError() != 0 {}
                let mut v: [GLfloat; 4] = [0.0; 4];
                gles.GetFloatv(name, v.as_mut_ptr());
                while gles.GetError() != 0 {}
                v
            };
            let blend_on = qb(gles, gles11::BLEND);
            let blend_src = qi(gles, gles11::BLEND_SRC) as u32;
            let blend_dst = qi(gles, gles11::BLEND_DST) as u32;
            let depth_on = qb(gles, gles11::DEPTH_TEST);
            let alpha_on = qb(gles, gles11::ALPHA_TEST);
            let cull_on = qb(gles, gles11::CULL_FACE);
            let texture_2d_on = qb(gles, gles11::TEXTURE_2D);
            let varr_on = qb(gles, gles11::VERTEX_ARRAY);
            let carr_on = qb(gles, gles11::COLOR_ARRAY);
            let tarr_on = qb(gles, gles11::TEXTURE_COORD_ARRAY);
            let array_buffer = qi(gles, gles11::ARRAY_BUFFER_BINDING);
            let elem_buffer = qi(gles, gles11::ELEMENT_ARRAY_BUFFER_BINDING);
            let clear_color = qf4(gles, gles11::COLOR_CLEAR_VALUE);
            let current_color = qf4(gles, gles11::CURRENT_COLOR);
            log!(
                "[--trace-gl-errors] present_renderbuffer renderbuffer-content probe \
                 (frame={}, used_app_fbo={}, viewport={:?}, renderbuffer={}x{}): \
                 BL=({},{},{},{}) BR=({},{},{},{}) TL=({},{},{},{}) TR=({},{},{},{}) \
                 CENTER=({},{},{},{}) \
                 | guest GL state: app_fbo={} app_tex2d={} \
                 array_buffer={} element_buffer={} \
                 BLEND={} (src=0x{:04x} dst=0x{:04x}) DEPTH_TEST={} ALPHA_TEST={} \
                 CULL_FACE={} TEXTURE_2D={} \
                 vertex_arr={} color_arr={} texcoord_arr={} \
                 clear_color=({:.3},{:.3},{:.3},{:.3}) current_color=({:.3},{:.3},{:.3},{:.3})",
                count,
                used_app_fbo,
                viewport,
                width,
                height,
                bl[0],
                bl[1],
                bl[2],
                bl[3],
                br[0],
                br[1],
                br[2],
                br[3],
                tl[0],
                tl[1],
                tl[2],
                tl[3],
                tr[0],
                tr[1],
                tr[2],
                tr[3],
                cc[0],
                cc[1],
                cc[2],
                cc[3],
                old_framebuffer,
                old_texture_2d,
                array_buffer,
                elem_buffer,
                blend_on,
                blend_src,
                blend_dst,
                depth_on,
                alpha_on,
                cull_on,
                texture_2d_on,
                varr_on,
                carr_on,
                tarr_on,
                clear_color[0],
                clear_color[1],
                clear_color[2],
                clear_color[3],
                current_color[0],
                current_color[1],
                current_color[2],
                current_color[3],
            );
        }
    }
    // Texture filter/wrap parameters were set once at creation (see the
    // comment in the cold path above); re-issuing them every frame is
    // unnecessary driver work.
    {
        static SEEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

        present_check(
            gles,
            trace_gl_errors,
            &SEEN,
            "after TexParameteri (filter+wrap)",
        );
    }

    // Stop using the source FBO so the present_frame quad below renders
    // to the default framebuffer (the SDL window) instead of the
    // renderbuffer / our throwaway FBO. In the no-FBO-switch path we
    // simply unbind the app's FBO; we'll restore it again at the end of
    // this function. In the fallback path, deleting the throwaway FBO
    // also implicitly unbinds it, leaving FRAMEBUFFER_BINDING == 0.
    if used_app_fbo {
        gles.BindFramebufferOES(gles11::FRAMEBUFFER_OES, 0);
    } else {
        gles.DeleteFramebuffersOES(1, &src_framebuffer);
    }
    // On iOS the window is not framebuffer 0 (see
    // `EaglPresentOptions::window_framebuffer`).
    if options.window_framebuffer != 0 {
        gles.BindFramebufferOES(gles11::FRAMEBUFFER_OES, options.window_framebuffer);
    }
    {
        static SEEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

        present_check(
            gles,
            trace_gl_errors,
            &SEEN,
            if used_app_fbo {
                "after BindFramebufferOES(0) (release app's FBO for present)"
            } else {
                "after DeleteFramebuffersOES (fallback path)"
            },
        );
    }

    // Reset various things that could affect the quad or virtual cursor we're
    // going to draw. Back up the old state while doing so, so it can be
    // restored later. The app's subsequent drawing will be messed up if we
    // don't restore it.

    // We *used* to query GL_CURRENT_PROGRAM (0x8B8D) here and call
    // glUseProgram(0) to clear any program before drawing the fixed-function
    // present quad. The original assumption was: "ES 1.x contexts won't have
    // any program bound, so this is a harmless no-op on ES 1.1 backends; ES
    // 2.0 apps that nevertheless landed in this path needed the clear so the
    // fixed-function quad would work."
    //
    // That assumption is wrong on real-world Android ES 1.1 drivers. On
    // Adreno (and similar GPUs whose ES 1.1 surface is implemented on top of
    // an underlying ES 3.x engine), querying GL_CURRENT_PROGRAM returns a
    // non-zero handle pointing at the driver's own internal program (used
    // for fixed-function emulation). Calling our generic gles.UseProgram(0)
    // on a GLES1Native backend then no-ops (gles_generic stub) — the program
    // is *not* actually unbound, but our fixed-function quad below now runs
    // with that internal program intercepting the draws, which produces a
    // black screen.
    //
    // The reverse path (a GLES2 app that somehow landed here) is now
    // impossible: the `if gles.is_es2()` branch above returns early for any
    // ES 2.0 backend. So just drop the clear/restore entirely on the
    // remaining (non-ES2) path. See LEGO Ninjago: Spinjitzu Scavenger Hunt.

    let old_arrays = {
        let mut old_arrays = [gles11::FALSE; gles1_on_gl2::ARRAYS.len()];
        for (is_enabled, info) in old_arrays.iter_mut().zip(gles1_on_gl2::ARRAYS.iter()) {
            gles.GetBooleanv(info.name, is_enabled);
            gles.DisableClientState(info.name);
        }
        old_arrays
    };
    {
        static SEEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

        present_check(
            gles,
            trace_gl_errors,
            &SEEN,
            "after old_arrays save+disable",
        );
    }
    // On a strict native ES 1.1 driver (e.g. ARM Mali on Android), some
    // entries in CAPABILITIES are spec-invalid for ES 1.1 even though they
    // are valid in desktop GL 2.1 (where the gles1_on_gl2 emulator runs).
    // Querying / disabling those would yield GL_INVALID_ENUM. The hard-coded
    // CAPABILITIES_GL21_ONLY list catches the *known* offenders, but real
    // Android drivers have plenty of further per-vendor / per-extension
    // quirks (e.g. r32p1 Mali-G57 in OpenGL ES-CM 1.1 mode rejects more caps
    // than the spec strictly says it should). Rather than maintain a growing
    // allow-list per driver, we drain GL errors *per cap* here: any cap that
    // GetBooleanv rejects is recorded as "skip" (None) so the matching
    // restore loop below also leaves it alone instead of trying to Enable /
    // Disable it and producing yet another error. This keeps the GL error
    // queue clean for the subsequent present_check sections — without it,
    // the very first cap that errors would poison the queue and make every
    // later checkpoint look like *it* failed.
    let is_native_es1 = gles.is_native_es1();
    // Which caps does this driver reject? That's a property of the driver,
    // not of the frame, so probe it once per context (the answer is cached in
    // PRESENT_ES1_REJECTED_CAPS and dropped on context switch) instead of
    // wrapping every GetBooleanv/Disable of every frame in glGetError() drain
    // loops — that was ~100 extra GL calls per presented frame.
    let rejected_caps: RejectedCaps =
        if let Some(cached) = PRESENT_ES1_REJECTED_CAPS.with(|c| c.get()) {
            cached
        } else {
            let mut rejected: RejectedCaps = [false; gles1_on_gl2::CAPABILITIES.len()];
            // Collect rejected caps so we can log them as a single line the
            // first time present runs. Useful for diagnosing "title menu
            // black on Mali but logo works" style bugs: maybe Mali rejected
            // the very cap the title menu relies on (e.g. ALPHA_TEST,
            // POINT_SPRITE_OES, ...).
            let mut rejected_names: Vec<GLenum> = Vec::new();
            for (slot, &name) in rejected.iter_mut().zip(gles1_on_gl2::CAPABILITIES.iter()) {
                if is_native_es1 && gles1_on_gl2::CAPABILITIES_GL21_ONLY.contains(&name) {
                    *slot = true;
                    continue;
                }
                // Drain anything that leaked in from earlier so we can
                // attribute a fresh error to *this* cap.
                drain_gl_errors(gles);
                let mut value: GLboolean = gles11::FALSE;
                gles.GetBooleanv(name, &mut value);
                if drain_gl_errors(gles) {
                    // Driver doesn't accept this cap: skip it on save and
                    // restore, and never try to Enable/Disable it.
                    *slot = true;
                    rejected_names.push(name);
                }
            }
            if !rejected_names.is_empty() {
                let names: Vec<String> = rejected_names
                    .iter()
                    .map(|c| format!("0x{:04x}", c))
                    .collect();
                log!(
                    "present_renderbuffer: driver rejected {} ES1.1 caps \
                     (will be skipped on save/restore): [{}]. \
                     They may also be rejected when the *guest* tries to use them, \
                     which could explain why some screens render black. \
                     [this log will only be shown once per context]",
                    rejected_names.len(),
                    names.join(", "),
                );
            }
            PRESENT_ES1_REJECTED_CAPS.with(|c| c.set(Some(rejected)));
            rejected
        };
    let old_capabilities: [Option<GLboolean>; gles1_on_gl2::CAPABILITIES.len()] = {
        let mut old_capabilities: [Option<GLboolean>; gles1_on_gl2::CAPABILITIES.len()] =
            [None; gles1_on_gl2::CAPABILITIES.len()];
        for ((slot, &name), &rejected) in old_capabilities
            .iter_mut()
            .zip(gles1_on_gl2::CAPABILITIES.iter())
            .zip(rejected_caps.iter())
        {
            if rejected {
                continue;
            }
            let mut value: GLboolean = gles11::FALSE;
            gles.GetBooleanv(name, &mut value);
            *slot = Some(value);
            if value != gles11::FALSE {
                gles.Disable(name);
            }
        }
        old_capabilities
    };
    {
        static SEEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

        present_check(
            gles,
            trace_gl_errors,
            &SEEN,
            "after old_capabilities save+disable",
        );
    }
    let old_matrix_mode: GLenum = get_int(gles, gles11::MATRIX_MODE) as _;
    for mode in [gles11::MODELVIEW, gles11::PROJECTION, gles11::TEXTURE] {
        gles.MatrixMode(mode);
        gles.PushMatrix();
        gles.LoadIdentity();
    }
    {
        static SEEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

        present_check(
            gles,
            trace_gl_errors,
            &SEEN,
            "after matrix push+identity for all 3 stacks",
        );
    }
    let old_color: [GLfloat; 4] = get_floats(gles, gles11::CURRENT_COLOR);
    gles.Color4f(1.0, 1.0, 1.0, 1.0);
    {
        static SEEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

        present_check(
            gles,
            trace_gl_errors,
            &SEEN,
            "after old_color save + Color4f white",
        );
    }

    // Back up other things that will be modified while drawing.
    let old_viewport: (GLint, GLint, GLsizei, GLsizei) = {
        let [x, y, width, height] = get_ints(gles, gles11::VIEWPORT);
        (x, y, width as _, height as _)
    };
    let old_clear_color: [GLfloat; 4] = get_floats(gles, gles11::COLOR_CLEAR_VALUE);
    let old_color_mask: [GLboolean; 4] =
        get_ints::<4>(gles, gles11::COLOR_WRITEMASK).map(|value| value as _);
    let old_depth_mask: GLboolean = get_int(gles, gles11::DEPTH_WRITEMASK) as _;
    let old_stencil_mask: GLuint = get_int(gles, gles11::STENCIL_WRITEMASK) as _;
    let old_array_buffer: GLuint = get_int(gles, gles11::ARRAY_BUFFER_BINDING) as _;
    let old_vertex_array_binding: GLuint = get_int(gles, gles11::VERTEX_ARRAY_BUFFER_BINDING) as _;
    let old_vertex_array_size: GLint = get_int(gles, gles11::VERTEX_ARRAY_SIZE);
    let old_vertex_array_type: GLenum = get_int(gles, gles11::VERTEX_ARRAY_TYPE) as _;
    let old_vertex_array_stride: GLsizei = get_int(gles, gles11::VERTEX_ARRAY_STRIDE) as _;
    let old_vertex_array_pointer = get_ptr(gles, gles11::VERTEX_ARRAY_POINTER);
    let old_tex_coord_array_binding: GLuint =
        get_int(gles, gles11::TEXTURE_COORD_ARRAY_BUFFER_BINDING) as _;
    let old_tex_coord_array_size: GLint = get_int(gles, gles11::TEXTURE_COORD_ARRAY_SIZE);
    let old_tex_coord_array_type: GLenum = get_int(gles, gles11::TEXTURE_COORD_ARRAY_TYPE) as _;
    let old_tex_coord_array_stride: GLsizei =
        get_int(gles, gles11::TEXTURE_COORD_ARRAY_STRIDE) as _;
    let old_tex_coord_array_pointer = get_ptr(gles, gles11::TEXTURE_COORD_ARRAY_POINTER);
    let old_blend_sfactor: GLenum = get_int(gles, gles11::BLEND_SRC) as _;
    let old_blend_dfactor: GLenum = get_int(gles, gles11::BLEND_DST) as _;
    {
        static SEEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

        present_check(
            gles,
            trace_gl_errors,
            &SEEN,
            "after viewport/clear/blend/array-pointer state save",
        );
    }

    let old_tex_env_mode = get_tex_env_int(gles, gles11::TEXTURE_ENV, gles11::TEXTURE_ENV_MODE);
    // if the mode is REPLACE, we don't have to reset the other texture
    // environment values
    let tex_env_mode_arr = [gles11::REPLACE; 1];
    gles.TexEnviv(
        gles11::TEXTURE_ENV,
        gles11::TEXTURE_ENV_MODE,
        tex_env_mode_arr.as_ptr().cast(),
    );
    {
        static SEEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

        present_check(gles, trace_gl_errors, &SEEN, "after TexEnviv setup");
    }

    // A guest can leave a write mask, stencil test, or depth mask that rejects
    // the compositor quad. The presentation pass must always be able to write
    // every destination color channel.
    gles.ColorMask(gles11::TRUE, gles11::TRUE, gles11::TRUE, gles11::TRUE);
    gles.DepthMask(gles11::TRUE);
    gles.StencilMask(!0);
    gles.Disable(gles11::STENCIL_TEST);

    // DIAG (TOUCHHLE_DUMP_RB=1): dump the raw renderbuffer the game drew, at
    // its native size, to compare the game's render against what we present.
    if crate::env_flag_cached!("TOUCHHLE_DUMP_RB") {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        log_once!("DUMP-RB: present_renderbuffer reached, dumping enabled");
        if matches!(n, 600 | 1200 | 1700) {
            let (px, w, h) = read_renderbuffer(gles, renderbuffer, Vec::new());
            let mut out = format!("P6\n{} {}\n255\n", w, h).into_bytes();
            for c in px.chunks_exact(4) {
                out.extend_from_slice(&[c[0], c[1], c[2]]);
            }
            let path = format!("rb_dump_{}.ppm", n);
            match std::fs::write(&path, &out) {
                Ok(()) => log!("DUMP-RB wrote {} ({}x{})", path, w, h),
                Err(e) => log!("DUMP-RB failed {}: {}", path, e),
            }
        }
    }

    // Draw the quad
    present_frame(
        gles,
        viewport,
        rotation_matrix,
        virtual_cursor_visible_at,
        context_token,
    );
    {
        static SEEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

        present_check(
            gles,
            trace_gl_errors,
            &SEEN,
            "after present_frame (textured quad draw)",
        );
    }
    // Black-frame detector, part 2: compare what ended up in the window
    // with what the source looked like.
    if let Some(source_max) = probe_source_max {
        probe_direct_present(gles, viewport, source_max);
    }

    // PERF: the present texture is cached across frames
    // (PRESENT_ES1_TEXTURE); do not delete it here. The restore below binds
    // the app's previous texture, which also unbinds ours.

    // Restore all the state saved before rendering
    for (&is_enabled, info) in old_arrays.iter().zip(gles1_on_gl2::ARRAYS.iter()) {
        // Any non-zero GLboolean counts as enabled: a driver answering a
        // query with garbage must not be able to panic the emulator here.
        if is_enabled != gles11::FALSE {
            gles.EnableClientState(info.name);
        } else {
            gles.DisableClientState(info.name);
        }
    }
    for (&saved, &name) in old_capabilities
        .iter()
        .zip(gles1_on_gl2::CAPABILITIES.iter())
    {
        if is_native_es1 && gles1_on_gl2::CAPABILITIES_GL21_ONLY.contains(&name) {
            continue;
        }
        // None means the save loop above couldn't query this cap (the
        // driver rejected it with INVALID_ENUM). Don't try to Enable /
        // Disable it here either — that would just produce another error.
        let Some(is_enabled) = saved else {
            continue;
        };
        if is_enabled != gles11::FALSE {
            gles.Enable(name);
        } else {
            gles.Disable(name);
        }
    }
    for mode in [gles11::MODELVIEW, gles11::PROJECTION, gles11::TEXTURE] {
        gles.MatrixMode(mode);
        gles.PopMatrix();
    }
    gles.MatrixMode(old_matrix_mode);
    {
        static SEEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

        present_check(gles, trace_gl_errors, &SEEN, "after matrix pop+restore");
    }
    gles.Color4f(old_color[0], old_color[1], old_color[2], old_color[3]);
    gles.Viewport(
        old_viewport.0,
        old_viewport.1,
        old_viewport.2,
        old_viewport.3,
    );
    gles.ClearColor(
        old_clear_color[0],
        old_clear_color[1],
        old_clear_color[2],
        old_clear_color[3],
    );
    gles.ColorMask(
        old_color_mask[0],
        old_color_mask[1],
        old_color_mask[2],
        old_color_mask[3],
    );
    gles.DepthMask(old_depth_mask);
    gles.StencilMask(old_stencil_mask);
    // STENCIL_TEST is part of old_capabilities and is restored above; this
    // only restores the masks needed by the next guest draw.
    // GL_ARRAY_BUFFER is implicitly used by the Pointer functions but is also
    // an independent binding.
    gles.BindBuffer(gles11::ARRAY_BUFFER, old_vertex_array_binding);
    gles.VertexPointer(
        old_vertex_array_size,
        old_vertex_array_type,
        old_vertex_array_stride,
        old_vertex_array_pointer,
    );
    gles.BindBuffer(gles11::ARRAY_BUFFER, old_tex_coord_array_binding);
    gles.TexCoordPointer(
        old_tex_coord_array_size,
        old_tex_coord_array_type,
        old_tex_coord_array_stride,
        old_tex_coord_array_pointer,
    );
    gles.BindBuffer(gles11::ARRAY_BUFFER, old_array_buffer);
    gles.BlendFunc(old_blend_sfactor, old_blend_dfactor);
    {
        static SEEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

        present_check(
            gles,
            trace_gl_errors,
            &SEEN,
            "after vertex/texcoord/buffer/blend restore",
        );
    }

    let old_tex_env_mode_arr = [old_tex_env_mode; 1];
    gles.TexEnviv(
        gles11::TEXTURE_ENV,
        gles11::TEXTURE_ENV_MODE,
        old_tex_env_mode_arr.as_ptr().cast(),
    );
    {
        static SEEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

        present_check(gles, trace_gl_errors, &SEEN, "after TexEnviv restore");
    }

    std::mem::drop(gles_boxed);

    // SDL2's documentation warns 0 should be bound to the draw framebuffer
    // when swapping the window, so this is the perfect moment.
    env.window.as_mut().unwrap().swap_window();

    let mut gles_boxed = gles_ctx.make_current(env.window.as_mut().unwrap());
    let gles = gles_boxed.as_mut();
    {
        static SEEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

        present_check(
            gles,
            trace_gl_errors,
            &SEEN,
            "after swap_window + re-make-current",
        );
    }

    // Restore the other bindings. The present texture was bound on unit 0
    // (see the ActiveTexture switch at the top), so restore both the unit
    // and the texture binding.
    gles.BindTexture(gles11::TEXTURE_2D, old_texture_2d);
    gles.ActiveTexture(old_active_texture);
    gles.BindFramebufferOES(gles11::FRAMEBUFFER_OES, old_framebuffer);
    {
        static SEEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

        present_check(
            gles,
            trace_gl_errors,
            &SEEN,
            "after BindTexture + BindFramebufferOES restore",
        );
    }

    // (See the long comment above for why we no longer save/restore
    // GL_CURRENT_PROGRAM on this ES 1.1 present path.)

    // Drain any GL errors generated by our own host-side present logic so
    // they don't leak into the guest's GL error queue and get attributed
    // to whatever guest call happens next (which previously confused the
    // --trace-gl-errors output and could perturb apps that poll
    // glGetError themselves). On strict ES 1.1 drivers (Mali, Adreno
    // ES1.1 surface) some of the wide state save/restore queries above
    // can return GL_INVALID_ENUM for state variables the driver doesn't
    // recognise; that's a host-side issue with our save list, not
    // anything the guest did. Log the first error once per app run for
    // diagnostics, then silently drain the rest.
    {
        use std::sync::atomic::{AtomicBool, Ordering};
        static REPORTED_ERR: AtomicBool = AtomicBool::new(false);
        let first = gles.GetError();
        if first != 0 && !REPORTED_ERR.swap(true, Ordering::Relaxed) {
            log!(
                "Note: present_renderbuffer left GL error {:#x} in queue; \
                 draining. Further errors from the present path will be \
                 silently consumed [this log will only be shown once]",
                first
            );
        }
        // Drain any further pending errors (GL keeps them queued one at
        // a time per error code; spec says implementations may keep an
        // unbounded number).
        while gles.GetError() != 0 {}
    }
}

pub fn EAGLGetVersion(env: &mut Environment, major: MutPtr<u32>, minor: MutPtr<u32>) {
    let version_major: u32 = 1;
    let version_minor: u32 = 1;

    if !major.is_null() {
        env.mem.write(major, version_major);
    }
    if !minor.is_null() {
        env.mem.write(minor, version_minor);
    }

    log!(
        "EAGLGetVersion called: major={}, minor={}",
        version_major,
        version_minor
    );
}

pub const FUNCTIONS: FunctionExports = &[export_c_func!(EAGLGetVersion(_, _))];

// Attach a full-screen EAGLView that an app created but left detached, so the
// window compositor and UIKit hit-testing both see the app's actual surface.
fn attach_orphaned_eagl_view_to_root(env: &mut Environment, drawable: id) -> bool {
    if drawable == nil {
        return false;
    }
    let drawable_class: crate::objc::Class = msg![env; drawable class];
    if env.objc.get_class_name(drawable_class) != "CAEAGLLayer" {
        return false;
    }
    let view: id = msg![env; drawable delegate];
    if view == nil {
        return false;
    }
    let view_class: crate::objc::Class = msg![env; view class];
    let view_class_name = env.objc.get_class_name(view_class).to_owned();
    let ui_view_class = env.objc.get_known_class("UIView", &mut env.mem);
    if !view_class_name.contains("EAGLView")
        || !env.objc.class_is_subclass_of(view_class, ui_view_class)
    {
        return false;
    }
    let view_layer: id = msg![env; view layer];
    let view_superview: id = msg![env; view superview];
    let drawable_superlayer: id = msg![env; drawable superlayer];
    let view_hidden: bool = msg![env; view isHidden];
    if view_layer != drawable
        || view_superview != nil
        || drawable_superlayer != nil
        || view_hidden
    {
        return false;
    }
    let window_count = env.framework_state.uikit.ui_view.ui_window.windows.len();
    let key_window = env.framework_state.uikit.ui_view.ui_window.key_window;
    let search_count = window_count + if key_window.is_some() { 1 } else { 0 };
    for search_index in 0..search_count {
        let window = match key_window {
            Some(key_window) if search_index == 0 => key_window,
            Some(key_window) => {
                let window = env.framework_state.uikit.ui_view.ui_window.windows
                    [window_count - search_index];
                if window == key_window {
                    continue;
                }
                window
            }
            None => {
                env.framework_state.uikit.ui_view.ui_window.windows[window_count - search_index - 1]
            }
        };
        let window_hidden: bool = msg![env; window isHidden];
        if window_hidden {
            continue;
        }
        let root_view_controller: id = msg![env; window rootViewController];
        let root_view: id = if root_view_controller != nil {
            let loaded: bool = msg![env; root_view_controller isViewLoaded];
            if !loaded {
                continue;
            }
            msg![env; root_view_controller view]
        } else {
            window
        };
        let root_hidden: bool = msg![env; root_view isHidden];
        if root_view == nil || root_hidden {
            continue;
        }
        let parent: id = if root_view == view { window } else { root_view };
        let parent_class: crate::objc::Class = msg![env; parent class];
        if !env.objc.class_is_subclass_of(parent_class, ui_view_class) {
            continue;
        }
        let parent_bounds: CGRect = msg![env; parent bounds];
        let view_bounds: CGRect = msg![env; view bounds];
        let drawable_bounds: CGRect = msg![env; drawable bounds];
        let parent_width = parent_bounds.size.width;
        let parent_height = parent_bounds.size.height;
        let view_width = view_bounds.size.width;
        let view_height = view_bounds.size.height;
        let same_orientation =
            (parent_width - view_width).abs() <= 1.0 && (parent_height - view_height).abs() <= 1.0;
        let rotated_orientation =
            (parent_width - view_height).abs() <= 1.0 && (parent_height - view_width).abs() <= 1.0;
        if parent_bounds.origin.x.abs() > 1.0
            || parent_bounds.origin.y.abs() > 1.0
            || view_bounds.origin.x.abs() > 1.0
            || view_bounds.origin.y.abs() > 1.0
            || (!same_orientation && !rotated_orientation)
            || (view_width - drawable_bounds.size.width).abs() > 1.0
            || (view_height - drawable_bounds.size.height).abs() > 1.0
        {
            continue;
        }
        let _: () = msg![env; parent addSubview:view];
        let attached_superview: id = msg![env; view superview];
        if attached_superview == parent {
            log!("Attached detached full-screen EAGLView {:?} to window hierarchy.", view);
            return true;
        }
    }
    false
}
