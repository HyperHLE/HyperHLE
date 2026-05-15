/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 * If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Passthrough for a native OpenGL ES 2.0 driver.
//!
//! This is the only backend that works on platforms that lack desktop OpenGL
//! (such as Android), so it is the preferred ES 2.0 backend.

use super::gles11_raw as gles11;
use super::gles11_raw::types::*;
use super::gles2_raw as gles2;
use super::gles_generic::{GLchar, GLES};
use super::GLESContext;
use crate::window::{GLContext, GLVersion, Window};
use std::ffi::CStr;
use std::marker::PhantomData;

pub struct GLES2NativeContext {
    gl_ctx: GLContext,
    is_loaded: bool,
}

impl GLESContext for GLES2NativeContext {
    fn description() -> &'static str {
        "Native OpenGL ES 2.0"
    }

    fn new(window: &mut Window) -> Result<Self, String> {
        Ok(Self {
            gl_ctx: window.create_gl_context(GLVersion::GLES20)?,
            is_loaded: false,
        })
    }

    fn make_current<'gl_ctx, 'win: 'gl_ctx>(
        &'gl_ctx mut self,
        window: &'win mut Window,
    ) -> Box<dyn GLES + 'gl_ctx> {
        if self.gl_ctx.is_current() && self.is_loaded {
            return Box::new(GLES2Native {
                _gl_lifetime: PhantomData,
            });
        }
        unsafe {
            window.make_gl_context_current(&self.gl_ctx);
        }
        gles2::load_with(|s| window.gl_get_proc_address(s));
        // Some symbols (e.g. glGetString) are technically also part of ES 1.1
        // and are referenced via gles11:: in the surrounding code; load those
        // too so the existing helpers continue to work.
        gles11::load_with(|s| window.gl_get_proc_address(s));
        self.is_loaded = true;
        Box::new(GLES2Native {
            _gl_lifetime: PhantomData,
        })
    }

    unsafe fn make_current_unchecked_for_window<'gl_ctx>(
        &'gl_ctx mut self,
        make_current_fn: &mut dyn FnMut(&GLContext),
        loader_fn: &mut dyn FnMut(&'static str) -> *const std::ffi::c_void,
    ) -> Box<dyn GLES + 'gl_ctx> {
        if self.gl_ctx.is_current() && self.is_loaded {
            return Box::new(GLES2Native {
                _gl_lifetime: PhantomData,
            });
        }
        make_current_fn(&self.gl_ctx);
        gles2::load_with(&mut *loader_fn);
        gles11::load_with(&mut *loader_fn);
        self.is_loaded = true;
        Box::new(GLES2Native {
            _gl_lifetime: PhantomData,
        })
    }
}

pub struct GLES2Native<'gl_ctx> {
    _gl_lifetime: PhantomData<&'gl_ctx ()>,
}

#[allow(clippy::missing_safety_doc)]
impl GLES for GLES2Native<'_> {
    fn is_es2(&self) -> bool {
        true
    }

    unsafe fn driver_description(&self) -> String {
        let version = CStr::from_ptr(gles2::GetString(gles2::VERSION) as *const _);
        let vendor = CStr::from_ptr(gles2::GetString(gles2::VENDOR) as *const _);
        let renderer = CStr::from_ptr(gles2::GetString(gles2::RENDERER) as *const _);
        format!(
            "{} / {} / {}",
            version.to_string_lossy(),
            vendor.to_string_lossy(),
            renderer.to_string_lossy()
        )
    }

    // Generic state manipulation
    unsafe fn GetError(&mut self) -> GLenum {
        gles2::GetError()
    }
    unsafe fn Enable(&mut self, cap: GLenum) {
        gles2::Enable(cap)
    }
    unsafe fn IsEnabled(&mut self, cap: GLenum) -> GLboolean {
        gles2::IsEnabled(cap)
    }
    unsafe fn Disable(&mut self, cap: GLenum) {
        gles2::Disable(cap)
    }
    unsafe fn GetBooleanv(&mut self, pname: GLenum, params: *mut GLboolean) {
        gles2::GetBooleanv(pname, params)
    }
    unsafe fn GetFloatv(&mut self, pname: GLenum, params: *mut GLfloat) {
        gles2::GetFloatv(pname, params)
    }
    unsafe fn GetIntegerv(&mut self, pname: GLenum, params: *mut GLint) {
        gles2::GetIntegerv(pname, params)
    }
    unsafe fn Hint(&mut self, target: GLenum, mode: GLenum) {
        gles2::Hint(target, mode)
    }
    unsafe fn Finish(&mut self) {
        gles2::Finish()
    }
    unsafe fn Flush(&mut self) {
        gles2::Flush()
    }
    unsafe fn GetString(&mut self, name: GLenum) -> *const GLubyte {
        gles2::GetString(name)
    }

    // Other state manipulation
    unsafe fn BlendFunc(&mut self, sfactor: GLenum, dfactor: GLenum) {
        gles2::BlendFunc(sfactor, dfactor)
    }
    unsafe fn ColorMask(
        &mut self,
        red: GLboolean,
        green: GLboolean,
        blue: GLboolean,
        alpha: GLboolean,
    ) {
        gles2::ColorMask(red, green, blue, alpha)
    }
    unsafe fn CullFace(&mut self, mode: GLenum) {
        gles2::CullFace(mode)
    }
    unsafe fn DepthFunc(&mut self, func: GLenum) {
        gles2::DepthFunc(func)
    }
    unsafe fn DepthMask(&mut self, flag: GLboolean) {
        gles2::DepthMask(flag)
    }
    unsafe fn DepthRangef(&mut self, near: GLclampf, far: GLclampf) {
        gles2::DepthRangef(near, far)
    }
    unsafe fn FrontFace(&mut self, mode: GLenum) {
        gles2::FrontFace(mode)
    }
    unsafe fn PolygonOffset(&mut self, factor: GLfloat, units: GLfloat) {
        gles2::PolygonOffset(factor, units)
    }
    unsafe fn SampleCoverage(&mut self, value: GLclampf, invert: GLboolean) {
        gles2::SampleCoverage(value, invert)
    }
    unsafe fn Scissor(&mut self, x: GLint, y: GLint, width: GLsizei, height: GLsizei) {
        gles2::Scissor(x, y, width, height)
    }
    unsafe fn Viewport(&mut self, x: GLint, y: GLint, width: GLsizei, height: GLsizei) {
        gles2::Viewport(x, y, width, height)
    }
    unsafe fn LineWidth(&mut self, val: GLfloat) {
        gles2::LineWidth(val)
    }
    unsafe fn StencilFunc(&mut self, func: GLenum, ref_: GLint, mask: GLuint) {
        gles2::StencilFunc(func, ref_, mask)
    }
    unsafe fn StencilOp(&mut self, sfail: GLenum, dpfail: GLenum, dppass: GLenum) {
        gles2::StencilOp(sfail, dpfail, dppass)
    }
    unsafe fn StencilMask(&mut self, mask: GLuint) {
        gles2::StencilMask(mask)
    }

    // Buffers
    unsafe fn IsBuffer(&mut self, buffer: GLuint) -> GLboolean {
        gles2::IsBuffer(buffer)
    }
    unsafe fn GenBuffers(&mut self, n: GLsizei, buffers: *mut GLuint) {
        gles2::GenBuffers(n, buffers)
    }
    unsafe fn DeleteBuffers(&mut self, n: GLsizei, buffers: *const GLuint) {
        gles2::DeleteBuffers(n, buffers)
    }
    unsafe fn BindBuffer(&mut self, target: GLenum, buffer: GLuint) {
        gles2::BindBuffer(target, buffer)
    }
    unsafe fn BufferData(
        &mut self,
        target: GLenum,
        size: GLsizeiptr,
        data: *const GLvoid,
        usage: GLenum,
    ) {
        gles2::BufferData(target, size, data, usage)
    }
    unsafe fn BufferSubData(
        &mut self,
        target: GLenum,
        offset: GLintptr,
        size: GLsizeiptr,
        data: *const GLvoid,
    ) {
        gles2::BufferSubData(target, offset, size, data)
    }

    // Drawing
    unsafe fn DrawArrays(&mut self, mode: GLenum, first: GLint, count: GLsizei) {
        gles2::DrawArrays(mode, first, count)
    }
    unsafe fn DrawElements(
        &mut self,
        mode: GLenum,
        count: GLsizei,
        type_: GLenum,
        indices: *const GLvoid,
    ) {
        gles2::DrawElements(mode, count, type_, indices)
    }
    unsafe fn Clear(&mut self, mask: GLbitfield) {
        gles2::Clear(mask)
    }
    unsafe fn ClearColor(
        &mut self,
        red: GLclampf,
        green: GLclampf,
        blue: GLclampf,
        alpha: GLclampf,
    ) {
        gles2::ClearColor(red, green, blue, alpha)
    }
    unsafe fn ClearDepthf(&mut self, depth: GLclampf) {
        gles2::ClearDepthf(depth)
    }
    unsafe fn ClearStencil(&mut self, s: GLint) {
        gles2::ClearStencil(s)
    }

    // Textures
    unsafe fn PixelStorei(&mut self, pname: GLenum, param: GLint) {
        gles2::PixelStorei(pname, param)
    }
    unsafe fn ReadPixels(
        &mut self,
        x: GLint,
        y: GLint,
        width: GLsizei,
        height: GLsizei,
        format: GLenum,
        type_: GLenum,
        pixels: *mut GLvoid,
    ) {
        gles2::ReadPixels(x, y, width, height, format, type_, pixels)
    }
    unsafe fn IsTexture(&mut self, texture: GLuint) -> GLboolean {
        gles2::IsTexture(texture)
    }
    unsafe fn GenTextures(&mut self, n: GLsizei, textures: *mut GLuint) {
        gles2::GenTextures(n, textures)
    }
    unsafe fn DeleteTextures(&mut self, n: GLsizei, textures: *const GLuint) {
        gles2::DeleteTextures(n, textures)
    }
    unsafe fn ActiveTexture(&mut self, texture: GLenum) {
        gles2::ActiveTexture(texture)
    }
    unsafe fn BindTexture(&mut self, target: GLenum, texture: GLuint) {
        gles2::BindTexture(target, texture)
    }
    unsafe fn TexParameteri(&mut self, target: GLenum, pname: GLenum, param: GLint) {
        gles2::TexParameteri(target, pname, param)
    }
    unsafe fn TexParameterf(&mut self, target: GLenum, pname: GLenum, param: GLfloat) {
        gles2::TexParameterf(target, pname, param)
    }
    unsafe fn TexParameteriv(&mut self, target: GLenum, pname: GLenum, params: *const GLint) {
        gles2::TexParameteriv(target, pname, params)
    }
    unsafe fn TexParameterfv(&mut self, target: GLenum, pname: GLenum, params: *const GLfloat) {
        gles2::TexParameterfv(target, pname, params)
    }
    #[allow(clippy::too_many_arguments)]
    unsafe fn TexImage2D(
        &mut self,
        target: GLenum,
        level: GLint,
        internalformat: GLint,
        width: GLsizei,
        height: GLsizei,
        border: GLint,
        format: GLenum,
        type_: GLenum,
        pixels: *const GLvoid,
    ) {
        gles2::TexImage2D(
            target,
            level,
            internalformat,
            width,
            height,
            border,
            format,
            type_,
            pixels,
        )
    }
    #[allow(clippy::too_many_arguments)]
    unsafe fn TexSubImage2D(
        &mut self,
        target: GLenum,
        level: GLint,
        xoffset: GLint,
        yoffset: GLint,
        width: GLsizei,
        height: GLsizei,
        format: GLenum,
        type_: GLenum,
        pixels: *const GLvoid,
    ) {
        gles2::TexSubImage2D(
            target, level, xoffset, yoffset, width, height, format, type_, pixels,
        )
    }
    #[allow(clippy::too_many_arguments)]
    unsafe fn CompressedTexImage2D(
        &mut self,
        target: GLenum,
        level: GLint,
        internalformat: GLenum,
        width: GLsizei,
        height: GLsizei,
        border: GLint,
        image_size: GLsizei,
        data: *const GLvoid,
    ) {
        gles2::CompressedTexImage2D(
            target,
            level,
            internalformat,
            width,
            height,
            border,
            image_size,
            data,
        )
    }
    unsafe fn CopyTexImage2D(
        &mut self,
        target: GLenum,
        level: GLint,
        internalformat: GLenum,
        x: GLint,
        y: GLint,
        width: GLsizei,
        height: GLsizei,
        border: GLint,
    ) {
        gles2::CopyTexImage2D(target, level, internalformat, x, y, width, height, border)
    }
    #[allow(clippy::too_many_arguments)]
    unsafe fn CopyTexSubImage2D(
        &mut self,
        target: GLenum,
        level: GLint,
        xoffset: GLint,
        yoffset: GLint,
        x: GLint,
        y: GLint,
        width: GLsizei,
        height: GLsizei,
    ) {
        gles2::CopyTexSubImage2D(target, level, xoffset, yoffset, x, y, width, height)
    }
    unsafe fn GenerateMipmapOES(&mut self, target: GLenum) {
        gles2::GenerateMipmap(target)
    }
    unsafe fn GenerateMipmap(&mut self, target: GLenum) {
        gles2::GenerateMipmap(target)
    }
    unsafe fn GenFramebuffers(&mut self, n: GLsizei, framebuffers: *mut GLuint) {
        gles2::GenFramebuffers(n, framebuffers)
    }
    unsafe fn GenRenderbuffers(&mut self, n: GLsizei, renderbuffers: *mut GLuint) {
        gles2::GenRenderbuffers(n, renderbuffers)
    }
    unsafe fn IsFramebuffer(&mut self, framebuffer: GLuint) -> GLboolean {
        gles2::IsFramebuffer(framebuffer)
    }
    unsafe fn IsRenderbuffer(&mut self, renderbuffer: GLuint) -> GLboolean {
        gles2::IsRenderbuffer(renderbuffer)
    }
    unsafe fn BindFramebuffer(&mut self, target: GLenum, framebuffer: GLuint) {
        gles2::BindFramebuffer(target, framebuffer)
    }
    unsafe fn BindRenderbuffer(&mut self, target: GLenum, renderbuffer: GLuint) {
        gles2::BindRenderbuffer(target, renderbuffer)
    }
    unsafe fn RenderbufferStorage(
        &mut self,
        target: GLenum,
        internalformat: GLenum,
        width: GLsizei,
        height: GLsizei,
    ) {
        gles2::RenderbufferStorage(target, internalformat, width, height)
    }
    unsafe fn FramebufferRenderbuffer(
        &mut self,
        target: GLenum,
        attachment: GLenum,
        renderbuffertarget: GLenum,
        renderbuffer: GLuint,
    ) {
        gles2::FramebufferRenderbuffer(target, attachment, renderbuffertarget, renderbuffer)
    }
    unsafe fn FramebufferTexture2D(
        &mut self,
        target: GLenum,
        attachment: GLenum,
        textarget: GLenum,
        texture: GLuint,
        level: i32,
    ) {
        gles2::FramebufferTexture2D(target, attachment, textarget, texture, level)
    }
    unsafe fn CheckFramebufferStatus(&mut self, target: GLenum) -> GLenum {
        gles2::CheckFramebufferStatus(target)
    }
    unsafe fn DeleteFramebuffers(&mut self, n: GLsizei, framebuffers: *const GLuint) {
        gles2::DeleteFramebuffers(n, framebuffers)
    }
    unsafe fn DeleteRenderbuffers(&mut self, n: GLsizei, renderbuffers: *const GLuint) {
        gles2::DeleteRenderbuffers(n, renderbuffers)
    }
    unsafe fn GetFramebufferAttachmentParameteriv(
        &mut self,
        target: GLenum,
        attachment: GLenum,
        pname: GLenum,
        params: *mut GLint,
    ) {
        gles2::GetFramebufferAttachmentParameteriv(target, attachment, pname, params)
    }
    unsafe fn GetRenderbufferParameteriv(
        &mut self,
        target: GLenum,
        pname: GLenum,
        params: *mut GLint,
    ) {
        gles2::GetRenderbufferParameteriv(target, pname, params)
    }
    unsafe fn GetBufferParameteriv(&mut self, target: GLenum, pname: GLenum, params: *mut GLint) {
        gles2::GetBufferParameteriv(target, pname, params)
    }

    // Framebuffers / renderbuffers (mapped via OES naming → core ES 2 calls)
    unsafe fn GenFramebuffersOES(&mut self, n: GLsizei, framebuffers: *mut GLuint) {
        gles2::GenFramebuffers(n, framebuffers)
    }
    unsafe fn DeleteFramebuffersOES(&mut self, n: GLsizei, framebuffers: *const GLuint) {
        gles2::DeleteFramebuffers(n, framebuffers)
    }
    unsafe fn BindFramebufferOES(&mut self, target: GLenum, framebuffer: GLuint) {
        gles2::BindFramebuffer(target, framebuffer)
    }
    unsafe fn IsFramebufferOES(&mut self, framebuffer: GLuint) -> GLboolean {
        gles2::IsFramebuffer(framebuffer)
    }
    unsafe fn CheckFramebufferStatusOES(&mut self, target: GLenum) -> GLenum {
        gles2::CheckFramebufferStatus(target)
    }
    unsafe fn FramebufferRenderbufferOES(
        &mut self,
        target: GLenum,
        attachment: GLenum,
        renderbuffertarget: GLenum,
        renderbuffer: GLuint,
    ) {
        gles2::FramebufferRenderbuffer(target, attachment, renderbuffertarget, renderbuffer)
    }
    #[allow(clippy::too_many_arguments)]
    unsafe fn FramebufferTexture2DOES(
        &mut self,
        target: GLenum,
        attachment: GLenum,
        textarget: GLenum,
        texture: GLuint,
        level: GLint,
    ) {
        gles2::FramebufferTexture2D(target, attachment, textarget, texture, level)
    }
    unsafe fn GetFramebufferAttachmentParameterivOES(
        &mut self,
        target: GLenum,
        attachment: GLenum,
        pname: GLenum,
        params: *mut GLint,
    ) {
        gles2::GetFramebufferAttachmentParameteriv(target, attachment, pname, params)
    }
    unsafe fn GenRenderbuffersOES(&mut self, n: GLsizei, renderbuffers: *mut GLuint) {
        gles2::GenRenderbuffers(n, renderbuffers)
    }
    unsafe fn DeleteRenderbuffersOES(&mut self, n: GLsizei, renderbuffers: *const GLuint) {
        gles2::DeleteRenderbuffers(n, renderbuffers)
    }
    unsafe fn BindRenderbufferOES(&mut self, target: GLenum, renderbuffer: GLuint) {
        gles2::BindRenderbuffer(target, renderbuffer)
    }
    unsafe fn IsRenderbufferOES(&mut self, renderbuffer: GLuint) -> GLboolean {
        gles2::IsRenderbuffer(renderbuffer)
    }
    unsafe fn RenderbufferStorageOES(
        &mut self,
        target: GLenum,
        internalformat: GLenum,
        width: GLsizei,
        height: GLsizei,
    ) {
        gles2::RenderbufferStorage(target, internalformat, width, height)
    }
    unsafe fn GetRenderbufferParameterivOES(
        &mut self,
        target: GLenum,
        pname: GLenum,
        params: *mut GLint,
    ) {
        gles2::GetRenderbufferParameteriv(target, pname, params)
    }

    // OpenGL ES 2.0 — shaders & programs
    unsafe fn CreateShader(&mut self, type_: GLenum) -> GLuint {
        gles2::CreateShader(type_)
    }
    unsafe fn DeleteShader(&mut self, shader: GLuint) {
        gles2::DeleteShader(shader)
    }
    unsafe fn ShaderSource(
        &mut self,
        shader: GLuint,
        count: GLsizei,
        string: *const *const GLchar,
        length: *const GLint,
    ) {
        // Translate any GLSL ES 1.00 source to GLSL ES too — but since we run
        // on a real ES 2.0 driver, we can pass the source through unchanged.
        gles2::ShaderSource(shader, count, string, length)
    }
    unsafe fn CompileShader(&mut self, shader: GLuint) {
        gles2::CompileShader(shader)
    }
    unsafe fn GetShaderiv(&mut self, shader: GLuint, pname: GLenum, params: *mut GLint) {
        gles2::GetShaderiv(shader, pname, params)
    }
    unsafe fn GetShaderInfoLog(
        &mut self,
        shader: GLuint,
        maxLength: GLsizei,
        length: *mut GLsizei,
        infoLog: *mut GLchar,
    ) {
        gles2::GetShaderInfoLog(shader, maxLength, length, infoLog)
    }
    unsafe fn IsShader(&mut self, shader: GLuint) -> GLboolean {
        gles2::IsShader(shader)
    }
    unsafe fn CreateProgram(&mut self) -> GLuint {
        gles2::CreateProgram()
    }
    unsafe fn DeleteProgram(&mut self, program: GLuint) {
        gles2::DeleteProgram(program)
    }
    unsafe fn AttachShader(&mut self, program: GLuint, shader: GLuint) {
        gles2::AttachShader(program, shader)
    }
    unsafe fn DetachShader(&mut self, program: GLuint, shader: GLuint) {
        gles2::DetachShader(program, shader)
    }
    unsafe fn LinkProgram(&mut self, program: GLuint) {
        gles2::LinkProgram(program)
    }
    unsafe fn UseProgram(&mut self, program: GLuint) {
        gles2::UseProgram(program)
    }
    unsafe fn GetProgramiv(&mut self, program: GLuint, pname: GLenum, params: *mut GLint) {
        gles2::GetProgramiv(program, pname, params)
    }
    unsafe fn GetProgramInfoLog(
        &mut self,
        program: GLuint,
        maxLength: GLsizei,
        length: *mut GLsizei,
        infoLog: *mut GLchar,
    ) {
        gles2::GetProgramInfoLog(program, maxLength, length, infoLog)
    }
    unsafe fn IsProgram(&mut self, program: GLuint) -> GLboolean {
        gles2::IsProgram(program)
    }
    unsafe fn ValidateProgram(&mut self, program: GLuint) {
        gles2::ValidateProgram(program)
    }
    unsafe fn BindAttribLocation(&mut self, program: GLuint, index: GLuint, name: *const GLchar) {
        gles2::BindAttribLocation(program, index, name)
    }
    unsafe fn GetAttribLocation(&mut self, program: GLuint, name: *const GLchar) -> GLint {
        gles2::GetAttribLocation(program, name)
    }
    unsafe fn GetUniformLocation(&mut self, program: GLuint, name: *const GLchar) -> GLint {
        gles2::GetUniformLocation(program, name)
    }
    #[allow(clippy::too_many_arguments)]
    unsafe fn GetActiveAttrib(
        &mut self,
        program: GLuint,
        index: GLuint,
        bufSize: GLsizei,
        length: *mut GLsizei,
        size: *mut GLint,
        type_: *mut GLenum,
        name: *mut GLchar,
    ) {
        gles2::GetActiveAttrib(program, index, bufSize, length, size, type_, name)
    }
    #[allow(clippy::too_many_arguments)]
    unsafe fn GetActiveUniform(
        &mut self,
        program: GLuint,
        index: GLuint,
        bufSize: GLsizei,
        length: *mut GLsizei,
        size: *mut GLint,
        type_: *mut GLenum,
        name: *mut GLchar,
    ) {
        gles2::GetActiveUniform(program, index, bufSize, length, size, type_, name)
    }

    // Vertex attributes
    unsafe fn EnableVertexAttribArray(&mut self, index: GLuint) {
        gles2::EnableVertexAttribArray(index)
    }
    unsafe fn DisableVertexAttribArray(&mut self, index: GLuint) {
        gles2::DisableVertexAttribArray(index)
    }
    unsafe fn VertexAttribPointer(
        &mut self,
        index: GLuint,
        size: GLint,
        type_: GLenum,
        normalized: GLboolean,
        stride: GLsizei,
        pointer: *const GLvoid,
    ) {
        gles2::VertexAttribPointer(index, size, type_, normalized, stride, pointer)
    }
    unsafe fn VertexAttrib1f(&mut self, index: GLuint, x: GLfloat) {
        gles2::VertexAttrib1f(index, x)
    }
    unsafe fn VertexAttrib2f(&mut self, index: GLuint, x: GLfloat, y: GLfloat) {
        gles2::VertexAttrib2f(index, x, y)
    }
    unsafe fn VertexAttrib3f(&mut self, index: GLuint, x: GLfloat, y: GLfloat, z: GLfloat) {
        gles2::VertexAttrib3f(index, x, y, z)
    }
    unsafe fn VertexAttrib4f(
        &mut self,
        index: GLuint,
        x: GLfloat,
        y: GLfloat,
        z: GLfloat,
        w: GLfloat,
    ) {
        gles2::VertexAttrib4f(index, x, y, z, w)
    }
    unsafe fn VertexAttrib1fv(&mut self, index: GLuint, v: *const GLfloat) {
        gles2::VertexAttrib1fv(index, v)
    }
    unsafe fn VertexAttrib2fv(&mut self, index: GLuint, v: *const GLfloat) {
        gles2::VertexAttrib2fv(index, v)
    }
    unsafe fn VertexAttrib3fv(&mut self, index: GLuint, v: *const GLfloat) {
        gles2::VertexAttrib3fv(index, v)
    }
    unsafe fn VertexAttrib4fv(&mut self, index: GLuint, v: *const GLfloat) {
        gles2::VertexAttrib4fv(index, v)
    }
    unsafe fn GetVertexAttribiv(&mut self, index: GLuint, pname: GLenum, params: *mut GLint) {
        gles2::GetVertexAttribiv(index, pname, params)
    }
    unsafe fn GetVertexAttribfv(&mut self, index: GLuint, pname: GLenum, params: *mut GLfloat) {
        gles2::GetVertexAttribfv(index, pname, params)
    }
    unsafe fn GetVertexAttribPointerv(
        &mut self,
        index: GLuint,
        pname: GLenum,
        pointer: *mut *mut GLvoid,
    ) {
        gles2::GetVertexAttribPointerv(index, pname, pointer)
    }

    // Uniforms
    unsafe fn Uniform1f(&mut self, location: GLint, v0: GLfloat) {
        gles2::Uniform1f(location, v0)
    }
    unsafe fn Uniform2f(&mut self, location: GLint, v0: GLfloat, v1: GLfloat) {
        gles2::Uniform2f(location, v0, v1)
    }
    unsafe fn Uniform3f(&mut self, location: GLint, v0: GLfloat, v1: GLfloat, v2: GLfloat) {
        gles2::Uniform3f(location, v0, v1, v2)
    }
    unsafe fn Uniform4f(
        &mut self,
        location: GLint,
        v0: GLfloat,
        v1: GLfloat,
        v2: GLfloat,
        v3: GLfloat,
    ) {
        gles2::Uniform4f(location, v0, v1, v2, v3)
    }
    unsafe fn Uniform1i(&mut self, location: GLint, v0: GLint) {
        gles2::Uniform1i(location, v0)
    }
    unsafe fn Uniform2i(&mut self, location: GLint, v0: GLint, v1: GLint) {
        gles2::Uniform2i(location, v0, v1)
    }
    unsafe fn Uniform3i(&mut self, location: GLint, v0: GLint, v1: GLint, v2: GLint) {
        gles2::Uniform3i(location, v0, v1, v2)
    }
    unsafe fn Uniform4i(&mut self, location: GLint, v0: GLint, v1: GLint, v2: GLint, v3: GLint) {
        gles2::Uniform4i(location, v0, v1, v2, v3)
    }
    unsafe fn Uniform1fv(&mut self, location: GLint, count: GLsizei, value: *const GLfloat) {
        gles2::Uniform1fv(location, count, value)
    }
    unsafe fn Uniform2fv(&mut self, location: GLint, count: GLsizei, value: *const GLfloat) {
        gles2::Uniform2fv(location, count, value)
    }
    unsafe fn Uniform3fv(&mut self, location: GLint, count: GLsizei, value: *const GLfloat) {
        gles2::Uniform3fv(location, count, value)
    }
    unsafe fn Uniform4fv(&mut self, location: GLint, count: GLsizei, value: *const GLfloat) {
        gles2::Uniform4fv(location, count, value)
    }
    unsafe fn Uniform1iv(&mut self, location: GLint, count: GLsizei, value: *const GLint) {
        gles2::Uniform1iv(location, count, value)
    }
    unsafe fn Uniform2iv(&mut self, location: GLint, count: GLsizei, value: *const GLint) {
        gles2::Uniform2iv(location, count, value)
    }
    unsafe fn Uniform3iv(&mut self, location: GLint, count: GLsizei, value: *const GLint) {
        gles2::Uniform3iv(location, count, value)
    }
    unsafe fn Uniform4iv(&mut self, location: GLint, count: GLsizei, value: *const GLint) {
        gles2::Uniform4iv(location, count, value)
    }
    unsafe fn UniformMatrix2fv(
        &mut self,
        location: GLint,
        count: GLsizei,
        transpose: GLboolean,
        value: *const GLfloat,
    ) {
        gles2::UniformMatrix2fv(location, count, transpose, value)
    }
    unsafe fn UniformMatrix3fv(
        &mut self,
        location: GLint,
        count: GLsizei,
        transpose: GLboolean,
        value: *const GLfloat,
    ) {
        gles2::UniformMatrix3fv(location, count, transpose, value)
    }
    unsafe fn UniformMatrix4fv(
        &mut self,
        location: GLint,
        count: GLsizei,
        transpose: GLboolean,
        value: *const GLfloat,
    ) {
        gles2::UniformMatrix4fv(location, count, transpose, value)
    }

    // Blending / stencil (ES 2.0 / GL 2.0 separate variants)
    unsafe fn BlendColor(
        &mut self,
        red: GLclampf,
        green: GLclampf,
        blue: GLclampf,
        alpha: GLclampf,
    ) {
        gles2::BlendColor(red, green, blue, alpha)
    }
    unsafe fn BlendEquation(&mut self, mode: GLenum) {
        gles2::BlendEquation(mode)
    }
    unsafe fn BlendEquationSeparate(&mut self, modeRGB: GLenum, modeAlpha: GLenum) {
        gles2::BlendEquationSeparate(modeRGB, modeAlpha)
    }
    unsafe fn BlendFuncSeparate(
        &mut self,
        sfactorRGB: GLenum,
        dfactorRGB: GLenum,
        sfactorAlpha: GLenum,
        dfactorAlpha: GLenum,
    ) {
        gles2::BlendFuncSeparate(sfactorRGB, dfactorRGB, sfactorAlpha, dfactorAlpha)
    }
    unsafe fn StencilFuncSeparate(
        &mut self,
        face: GLenum,
        func: GLenum,
        ref_: GLint,
        mask: GLuint,
    ) {
        gles2::StencilFuncSeparate(face, func, ref_, mask)
    }
    unsafe fn StencilOpSeparate(
        &mut self,
        face: GLenum,
        sfail: GLenum,
        dpfail: GLenum,
        dppass: GLenum,
    ) {
        gles2::StencilOpSeparate(face, sfail, dpfail, dppass)
    }
    unsafe fn StencilMaskSeparate(&mut self, face: GLenum, mask: GLuint) {
        gles2::StencilMaskSeparate(face, mask)
    }

    // Uniform query
    unsafe fn GetUniformiv(&mut self, program: GLuint, location: GLint, params: *mut GLint) {
        gles2::GetUniformiv(program, location, params)
    }
    unsafe fn GetUniformfv(&mut self, program: GLuint, location: GLint, params: *mut GLfloat) {
        gles2::GetUniformfv(program, location, params)
    }

    // Shader/program query
    unsafe fn GetAttachedShaders(
        &mut self,
        program: GLuint,
        maxCount: GLsizei,
        count: *mut GLsizei,
        shaders: *mut GLuint,
    ) {
        gles2::GetAttachedShaders(program, maxCount, count, shaders)
    }
    unsafe fn GetShaderSource(
        &mut self,
        shader: GLuint,
        bufSize: GLsizei,
        length: *mut GLsizei,
        source: *mut GLchar,
    ) {
        gles2::GetShaderSource(shader, bufSize, length, source)
    }
    unsafe fn GetShaderPrecisionFormat(
        &mut self,
        shadertype: GLenum,
        precisiontype: GLenum,
        range: *mut GLint,
        precision: *mut GLint,
    ) {
        gles2::GetShaderPrecisionFormat(shadertype, precisiontype, range, precision)
    }
    unsafe fn ShaderBinary(
        &mut self,
        count: GLsizei,
        shaders: *const GLuint,
        binaryformat: GLenum,
        binary: *const GLvoid,
        length: GLsizei,
    ) {
        gles2::ShaderBinary(count, shaders, binaryformat, binary, length)
    }

    // Fixed-function methods (ES 1.x) – no-ops on a real ES 2.0 driver. This
    // keeps the existing `present_renderbuffer` save/restore code paths quiet
    // without crashing. Real apps that rely on a true ES 2.0 driver will not
    // call these.
    unsafe fn ClientActiveTexture(&mut self, _texture: GLenum) {}
    unsafe fn EnableClientState(&mut self, _array: GLenum) {}
    unsafe fn DisableClientState(&mut self, _array: GLenum) {}
    unsafe fn GetTexEnviv(&mut self, _target: GLenum, _pname: GLenum, _params: *mut GLint) {}
    unsafe fn GetTexEnvfv(&mut self, _target: GLenum, _pname: GLenum, _params: *mut GLfloat) {}
    unsafe fn GetPointerv(&mut self, _pname: GLenum, _params: *mut *const GLvoid) {}
    unsafe fn AlphaFunc(&mut self, _func: GLenum, _ref_: GLclampf) {}
    unsafe fn AlphaFuncx(&mut self, _func: GLenum, _ref_: GLclampx) {}
    unsafe fn Color4f(&mut self, _r: GLfloat, _g: GLfloat, _b: GLfloat, _a: GLfloat) {}
    unsafe fn Color4x(&mut self, _r: GLfixed, _g: GLfixed, _b: GLfixed, _a: GLfixed) {}
    unsafe fn Color4ub(&mut self, _r: GLubyte, _g: GLubyte, _b: GLubyte, _a: GLubyte) {}
    unsafe fn ShadeModel(&mut self, _mode: GLenum) {}
    unsafe fn LoadIdentity(&mut self) {}
    unsafe fn LoadMatrixf(&mut self, _m: *const GLfloat) {}
    unsafe fn LoadMatrixx(&mut self, _m: *const GLfixed) {}
    unsafe fn MultMatrixf(&mut self, _m: *const GLfloat) {}
    unsafe fn MultMatrixx(&mut self, _m: *const GLfixed) {}
    unsafe fn PushMatrix(&mut self) {}
    unsafe fn PopMatrix(&mut self) {}
    unsafe fn MatrixMode(&mut self, _mode: GLenum) {}
    unsafe fn Frustumf(
        &mut self,
        _left: GLfloat,
        _right: GLfloat,
        _bottom: GLfloat,
        _top: GLfloat,
        _near: GLfloat,
        _far: GLfloat,
    ) {
    }
    unsafe fn Frustumx(
        &mut self,
        _left: GLfixed,
        _right: GLfixed,
        _bottom: GLfixed,
        _top: GLfixed,
        _near: GLfixed,
        _far: GLfixed,
    ) {
    }
    unsafe fn Orthof(
        &mut self,
        _left: GLfloat,
        _right: GLfloat,
        _bottom: GLfloat,
        _top: GLfloat,
        _near: GLfloat,
        _far: GLfloat,
    ) {
    }
    unsafe fn Orthox(
        &mut self,
        _left: GLfixed,
        _right: GLfixed,
        _bottom: GLfixed,
        _top: GLfixed,
        _near: GLfixed,
        _far: GLfixed,
    ) {
    }
    unsafe fn Rotatef(&mut self, _a: GLfloat, _x: GLfloat, _y: GLfloat, _z: GLfloat) {}
    unsafe fn Rotatex(&mut self, _a: GLfixed, _x: GLfixed, _y: GLfixed, _z: GLfixed) {}
    unsafe fn Scalef(&mut self, _x: GLfloat, _y: GLfloat, _z: GLfloat) {}
    unsafe fn Scalex(&mut self, _x: GLfixed, _y: GLfixed, _z: GLfixed) {}
    unsafe fn Translatef(&mut self, _x: GLfloat, _y: GLfloat, _z: GLfloat) {}
    unsafe fn Translatex(&mut self, _x: GLfixed, _y: GLfixed, _z: GLfixed) {}
    unsafe fn TexEnvf(&mut self, _target: GLenum, _pname: GLenum, _param: GLfloat) {}
    unsafe fn TexEnvx(&mut self, _target: GLenum, _pname: GLenum, _param: GLfixed) {}
    unsafe fn TexEnvi(&mut self, _target: GLenum, _pname: GLenum, _param: GLint) {}
    unsafe fn TexEnvfv(&mut self, _target: GLenum, _pname: GLenum, _params: *const GLfloat) {}
    unsafe fn TexEnvxv(&mut self, _target: GLenum, _pname: GLenum, _params: *const GLfixed) {}
    unsafe fn TexEnviv(&mut self, _target: GLenum, _pname: GLenum, _params: *const GLint) {}
    unsafe fn VertexPointer(
        &mut self,
        _size: GLint,
        _type_: GLenum,
        _stride: GLsizei,
        _pointer: *const GLvoid,
    ) {
    }
    unsafe fn ColorPointer(
        &mut self,
        _size: GLint,
        _type_: GLenum,
        _stride: GLsizei,
        _pointer: *const GLvoid,
    ) {
    }
    unsafe fn NormalPointer(&mut self, _type_: GLenum, _stride: GLsizei, _pointer: *const GLvoid) {}
    unsafe fn TexCoordPointer(
        &mut self,
        _size: GLint,
        _type_: GLenum,
        _stride: GLsizei,
        _pointer: *const GLvoid,
    ) {
    }
}
