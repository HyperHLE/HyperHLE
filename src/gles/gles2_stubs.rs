/*
 * OpenGL ES 2.0 Stub Functions for touchHLE
 * * Этот модуль предоставляет заглушки для функций OpenGL ES 2.0,
 * чтобы предотвратить краши на устройствах без поддержки ES 2.0.
 */

use std::os::raw::{c_char, c_int, c_uchar, c_uint, c_void};
use crate::mem::MutVoidPtr;
use crate::environment::Environment;

// Определение базовых типов OpenGL
pub type GLenum = c_uint;
pub type GLuint = c_uint;
pub type GLint = c_int;
pub type GLchar = c_char;
pub type GLsizei = c_int;
pub type GLboolean = c_uchar;
pub type GLfloat = f32;
pub type GLdouble = f64;
pub type GLbitfield = c_uint;
pub type GLintptr = isize;
pub type GLsizeiptr = isize;
pub type GLubyte = u8;

// Константы ES 2.0 которых нет в ES 1.1
pub const GL_ES_VERSION_2_0: GLenum = 1;
pub const GL_VERTEX_SHADER: GLenum = 0x8B31;
pub const GL_FRAGMENT_SHADER: GLenum = 0x8B30;
pub const GL_COMPILE_STATUS: GLenum = 0x8B81;
pub const GL_LINK_STATUS: GLenum = 0x8B82;
pub const GL_INFO_LOG_LENGTH: GLenum = 0x8B84;
pub const GL_SHADER_TYPE: GLenum = 0x8B4F;
pub const GL_SHADER_SOURCE_LENGTH: GLenum = 0x8B88;
pub const GL_FLOAT_VEC2: GLenum = 0x8B50;
pub const GL_FLOAT_VEC3: GLenum = 0x8B51;
pub const GL_FLOAT_VEC4: GLenum = 0x8B52;
pub const GL_INT_VEC2: GLenum = 0x8B53;
pub const GL_INT_VEC3: GLenum = 0x8B54;
pub const GL_INT_VEC4: GLenum = 0x8B55;
pub const GL_BOOL: GLenum = 0x8B56;
pub const GL_BOOL_VEC2: GLenum = 0x8B57;
pub const GL_BOOL_VEC3: GLenum = 0x8B58;
pub const GL_BOOL_VEC4: GLenum = 0x8B59;
pub const GL_FLOAT_MAT2: GLenum = 0x8B5A;
pub const GL_FLOAT_MAT3: GLenum = 0x8B5B;
pub const GL_FLOAT_MAT4: GLenum = 0x8B5C;
pub const GL_SAMPLER_2D: GLenum = 0x8B5E;
pub const GL_SAMPLER_CUBE: GLenum = 0x8B60;
pub const GL_ACTIVE_ATTRIBUTES: GLenum = 0x8B89;
pub const GL_ACTIVE_UNIFORMS: GLenum = 0x8B86;
pub const GL_ATTACHED_SHADERS: GLenum = 0x8B85;
pub const GL_DELETE_STATUS: GLenum = 0x8B80;
pub const GL_VALIDATE_STATUS: GLenum = 0x8B83;

static mut NEXT_SHADER_ID: GLuint = 1;
static mut NEXT_PROGRAM_ID: GLuint = 1000;

fn next_shader_id() -> GLuint {
    unsafe {
        let id = NEXT_SHADER_ID;
        NEXT_SHADER_ID += 1;
        id
    }
}

fn next_program_id() -> GLuint {
    unsafe {
        let id = NEXT_PROGRAM_ID;
        NEXT_PROGRAM_ID += 1;
        id
    }
}

// MARK: - Shader Functions

#[no_mangle]
pub extern "C" fn glCreateShader(_type: GLenum) -> GLuint {
    log!("STUB: glCreateShader(type=0x{:x})", _type);
    next_shader_id()
}

#[no_mangle]
pub extern "C" fn glShaderSource(
    shader: GLuint,
    count: GLsizei,
    _string: *const *const GLchar,
    _length: *const GLint,
) {
    log!("STUB: glShaderSource(shader={}, count={})", shader, count);
}

#[no_mangle]
pub extern "C" fn glCompileShader(shader: GLuint) {
    log!("STUB: glCompileShader(shader={})", shader);
}

#[no_mangle]
pub extern "C" fn glDeleteShader(shader: GLuint) {
    log!("STUB: glDeleteShader(shader={})", shader);
}

#[no_mangle]
pub extern "C" fn glGetShaderiv(shader: GLuint, pname: GLenum, params: *mut GLint) {
    log!("STUB: glGetShaderiv(shader={}, pname=0x{:x})", shader, pname);
    if !params.is_null() {
        unsafe {
            match pname {
                GL_COMPILE_STATUS => *params = 1, // Успешно скомпилирован
                GL_DELETE_STATUS => *params = 0,
                GL_SHADER_TYPE => *params = GL_VERTEX_SHADER as GLint,
                GL_INFO_LOG_LENGTH => *params = 0,
                GL_SHADER_SOURCE_LENGTH => *params = 0,
                _ => *params = 0,
            }
        }
    }
}

#[no_mangle]
pub extern "C" fn glGetShaderInfoLog(
    shader: GLuint,
    _bufSize: GLsizei,
    length: *mut GLsizei,
    _infoLog: *mut GLchar,
) {
    log!("STUB: glGetShaderInfoLog(shader={})", shader);
    if !length.is_null() {
        unsafe { *length = 0; }
    }
}

// MARK: - Program Functions

#[no_mangle]
pub extern "C" fn glCreateProgram() -> GLuint {
    log!("STUB: glCreateProgram()");
    next_program_id()
}

#[no_mangle]
pub extern "C" fn glAttachShader(program: GLuint, shader: GLuint) {
    log!("STUB: glAttachShader(program={}, shader={})", program, shader);
}

#[no_mangle]
pub extern "C" fn glLinkProgram(program: GLuint) {
    log!("STUB: glLinkProgram(program={})", program);
}

#[no_mangle]
pub extern "C" fn glUseProgram(program: GLuint) {
    log!("STUB: glUseProgram(program={})", program);
}

#[no_mangle]
pub extern "C" fn glDeleteProgram(program: GLuint) {
    log!("STUB: glDeleteProgram(program={})", program);
}

#[no_mangle]
pub extern "C" fn glGetProgramiv(program: GLuint, pname: GLenum, params: *mut GLint) {
    log!("STUB: glGetProgramiv(program={}, pname=0x{:x})", program, pname);
    if !params.is_null() {
        unsafe {
            match pname {
                GL_LINK_STATUS => *params = 1, // Успешно слинкован
                GL_DELETE_STATUS => *params = 0,
                GL_VALIDATE_STATUS => *params = 1,
                GL_ATTACHED_SHADERS => *params = 2,
                GL_ACTIVE_ATTRIBUTES => *params = 0,
                GL_ACTIVE_UNIFORMS => *params = 0,
                GL_INFO_LOG_LENGTH => *params = 0,
                _ => *params = 0,
            }
        }
    }
}

#[no_mangle]
pub extern "C" fn glGetProgramInfoLog(
    program: GLuint,
    _bufSize: GLsizei,
    length: *mut GLsizei,
    _infoLog: *mut GLchar,
) {
    log!("STUB: glGetProgramInfoLog(program={})", program);
    if !length.is_null() {
        unsafe { *length = 0; }
    }
}

#[no_mangle]
pub extern "C" fn glValidateProgram(program: GLuint) {
    log!("STUB: glValidateProgram(program={})", program);
}

// MARK: - Attribute & Uniform Functions

#[no_mangle]
pub extern "C" fn glGetAttribLocation(program: GLuint, name: *const GLchar) -> GLint {
    let name_str = if name.is_null() {
        "(null)"
    } else {
        unsafe {
            std::ffi::CStr::from_ptr(name).to_str().unwrap_or("invalid")
        }
    };
    log!("STUB: glGetAttribLocation(program={}, name={})", program, name_str);
    -1 // Не найдено
}

#[no_mangle]
pub extern "C" fn glGetUniformLocation(program: GLuint, name: *const GLchar) -> GLint {
    let name_str = if name.is_null() {
        "(null)"
    } else {
        unsafe {
            std::ffi::CStr::from_ptr(name).to_str().unwrap_or("invalid")
        }
    };
    log!("STUB: glGetUniformLocation(program={}, name={})", program, name_str);
    -1 // Не найдено
}

#[no_mangle]
pub extern "C" fn glVertexAttribPointer(
    index: GLuint,
    size: GLint,
    _type: GLenum,
    _normalized: GLboolean,
    _stride: GLsizei,
    _pointer: *const c_void,
) {
    log!("STUB: glVertexAttribPointer(index={}, size={})", index, size);
}

#[no_mangle]
pub extern "C" fn glEnableVertexAttribArray(index: GLuint) {
    log!("STUB: glEnableVertexAttribArray(index={})", index);
}

#[no_mangle]
pub extern "C" fn glDisableVertexAttribArray(index: GLuint) {
    log!("STUB: glDisableVertexAttribArray(index={})", index);
}

#[no_mangle]
pub extern "C" fn glVertexAttrib1f(index: GLuint, _x: GLfloat) {
    log!("STUB: glVertexAttrib1f(index={})", index);
}

#[no_mangle]
pub extern "C" fn glVertexAttrib2f(index: GLuint, _x: GLfloat, _y: GLfloat) {
    log!("STUB: glVertexAttrib2f(index={})", index);
}

#[no_mangle]
pub extern "C" fn glVertexAttrib3f(index: GLuint, _x: GLfloat, _y: GLfloat, _z: GLfloat) {
    log!("STUB: glVertexAttrib3f(index={})", index);
}

#[no_mangle]
pub extern "C" fn glVertexAttrib4f(index: GLuint, _x: GLfloat, _y: GLfloat, _z: GLfloat, _w: GLfloat) {
    log!("STUB: glVertexAttrib4f(index={})", index);
}

// MARK: - Uniform Setters

#[no_mangle]
pub extern "C" fn glUniform1f(location: GLint, _v0: GLfloat) {
    log!("STUB: glUniform1f(location={})", location);
}

#[no_mangle]
pub extern "C" fn glUniform2f(location: GLint, _v0: GLfloat, _v1: GLfloat) {
    log!("STUB: glUniform2f(location={})", location);
}

#[no_mangle]
pub extern "C" fn glUniform3f(location: GLint, _v0: GLfloat, _v1: GLfloat, _v2: GLfloat) {
    log!("STUB: glUniform3f(location={})", location);
}

#[no_mangle]
pub extern "C" fn glUniform4f(location: GLint, _v0: GLfloat, _v1: GLfloat, _v2: GLfloat, _v3: GLfloat) {
    log!("STUB: glUniform4f(location={})", location);
}

#[no_mangle]
pub extern "C" fn glUniform1i(location: GLint, _v0: GLint) {
    log!("STUB: glUniform1i(location={})", location);
}

#[no_mangle]
pub extern "C" fn glUniform2i(location: GLint, _v0: GLint, _v1: GLint) {
    log!("STUB: glUniform2i(location={})", location);
}

#[no_mangle]
pub extern "C" fn glUniform3i(location: GLint, _v0: GLint, _v1: GLint, _v2: GLint) {
    log!("STUB: glUniform3i(location={})", location);
}

#[no_mangle]
pub extern "C" fn glUniform4i(location: GLint, _v0: GLint, _v1: GLint, _v2: GLint, _v3: GLint) {
    log!("STUB: glUniform4i(location={})", location);
}

#[no_mangle]
pub extern "C" fn glUniform1fv(location: GLint, _count: GLsizei, _value: *const GLfloat) {
    log!("STUB: glUniform1fv(location={})", location);
}

#[no_mangle]
pub extern "C" fn glUniform2fv(location: GLint, _count: GLsizei, _value: *const GLfloat) {
    log!("STUB: glUniform2fv(location={})", location);
}

#[no_mangle]
pub extern "C" fn glUniform3fv(location: GLint, _count: GLsizei, _value: *const GLfloat) {
    log!("STUB: glUniform3fv(location={})", location);
}

#[no_mangle]
pub extern "C" fn glUniform4fv(location: GLint, _count: GLsizei, _value: *const GLfloat) {
    log!("STUB: glUniform4fv(location={})", location);
}

#[no_mangle]
pub extern "C" fn glUniform1iv(location: GLint, _count: GLsizei, _value: *const GLint) {
    log!("STUB: glUniform1iv(location={})", location);
}

#[no_mangle]
pub extern "C" fn glUniform2iv(location: GLint, _count: GLsizei, _value: *const GLint) {
    log!("STUB: glUniform2iv(location={})", location);
}

#[no_mangle]
pub extern "C" fn glUniform3iv(location: GLint, _count: GLsizei, _value: *const GLint) {
    log!("STUB: glUniform3iv(location={})", location);
}

#[no_mangle]
pub extern "C" fn glUniform4iv(location: GLint, _count: GLsizei, _value: *const GLint) {
    log!("STUB: glUniform4iv(location={})", location);
}

#[no_mangle]
pub extern "C" fn glUniformMatrix2fv(location: GLint, _count: GLsizei, _transpose: GLboolean, _value: *const GLfloat) {
    log!("STUB: glUniformMatrix2fv(location={})", location);
}

#[no_mangle]
pub extern "C" fn glUniformMatrix3fv(location: GLint, _count: GLsizei, _transpose: GLboolean, _value: *const GLfloat) {
    log!("STUB: glUniformMatrix3fv(location={})", location);
}

#[no_mangle]
pub extern "C" fn glUniformMatrix4fv(location: GLint, _count: GLsizei, _transpose: GLboolean, _value: *const GLfloat) {
    log!("STUB: glUniformMatrix4fv(location={})", location);
}

// MARK: - Buffer Functions

#[no_mangle]
pub extern "C" fn glGenBuffers(n: GLsizei, buffers: *mut GLuint) {
    log!("STUB: glGenBuffers(n={})", n);
    if !buffers.is_null() && n > 0 {
        unsafe {
            for i in 0..n as isize {
                *buffers.offset(i) = (i + 1) as GLuint;
            }
        }
    }
}

#[no_mangle]
pub extern "C" fn glBindBuffer(target: GLenum, buffer: GLuint) {
    log!("STUB: glBindBuffer(target=0x{:x}, buffer={})", target, buffer);
}

#[no_mangle]
pub extern "C" fn glBufferData(target: GLenum, size: GLsizeiptr, _data: *const c_void, _usage: GLenum) {
    log!("STUB: glBufferData(target=0x{:x}, size={})", target, size);
}

#[no_mangle]
pub extern "C" fn glBufferSubData(target: GLenum, _offset: GLintptr, _size: GLsizeiptr, _data: *const c_void) {
    log!("STUB: glBufferSubData(target=0x{:x})", target);
}

#[no_mangle]
pub extern "C" fn glDeleteBuffers(n: GLsizei, _buffers: *const GLuint) {
    log!("STUB: glDeleteBuffers(n={})", n);
}

// MARK: - Framebuffer Functions

#[no_mangle]
pub extern "C" fn glBindFramebuffer(target: GLenum, framebuffer: GLuint) {
    log!("STUB: glBindFramebuffer(target=0x{:x}, framebuffer={})", target, framebuffer);
}

#[no_mangle]
pub extern "C" fn glGenFramebuffers(n: GLsizei, framebuffers: *mut GLuint) {
    log!("STUB: glGenFramebuffers(n={})", n);
    if !framebuffers.is_null() && n > 0 {
        unsafe {
            for i in 0..n as isize {
                *framebuffers.offset(i) = (i + 100) as GLuint;
            }
        }
    }
}

#[no_mangle]
pub extern "C" fn glDeleteFramebuffers(n: GLsizei, _framebuffers: *const GLuint) {
    log!("STUB: glDeleteFramebuffers(n={})", n);
}

#[no_mangle]
pub extern "C" fn glFramebufferTexture2D(target: GLenum, attachment: GLenum, _textarget: GLenum, _texture: GLuint, _level: GLint) {
    log!("STUB: glFramebufferTexture2D(target=0x{:x}, attachment=0x{:x})", target, attachment);
}

#[no_mangle]
pub extern "C" fn glFramebufferRenderbuffer(target: GLenum, attachment: GLenum, _renderbuffertarget: GLenum, _renderbuffer: GLuint) {
    log!("STUB: glFramebufferRenderbuffer(target=0x{:x}, attachment=0x{:x})", target, attachment);
}

#[no_mangle]
pub extern "C" fn glCheckFramebufferStatus(target: GLenum) -> GLenum {
    log!("STUB: glCheckFramebufferStatus(target=0x{:x})", target);
    0x8CD5 // GL_FRAMEBUFFER_COMPLETE
}

// MARK: - Renderbuffer Functions

#[no_mangle]
pub extern "C" fn glGenRenderbuffers(n: GLsizei, renderbuffers: *mut GLuint) {
    log!("STUB: glGenRenderbuffers(n={})", n);
    if !renderbuffers.is_null() && n > 0 {
        unsafe {
            for i in 0..n as isize {
                *renderbuffers.offset(i) = (i + 200) as GLuint;
            }
        }
    }
}

#[no_mangle]
pub extern "C" fn glBindRenderbuffer(target: GLenum, renderbuffer: GLuint) {
    log!("STUB: glBindRenderbuffer(target=0x{:x}, renderbuffer={})", target, renderbuffer);
}

#[no_mangle]
pub extern "C" fn glDeleteRenderbuffers(n: GLsizei, _renderbuffers: *const GLuint) {
    log!("STUB: glDeleteRenderbuffers(n={})", n);
}

#[no_mangle]
pub extern "C" fn glRenderbufferStorage(target: GLenum, internalformat: GLenum, width: GLsizei, height: GLsizei) {
    log!("STUB: glRenderbufferStorage(target=0x{:x}, internalformat=0x{:x}, {}x{})", target, internalformat, width, height);
}

// MARK: - Texture Functions

#[no_mangle]
pub extern "C" fn glActiveTexture(texture: GLenum) {
    log!("STUB: glActiveTexture(texture=0x{:x})", texture);
}

// MARK: - State Functions

#[no_mangle]
pub extern "C" fn glDepthRangef(n: GLfloat, f: GLfloat) {
    log!("STUB: glDepthRangef({}, {})", n, f);
}

#[no_mangle]
pub extern "C" fn glClearDepthf(d: GLfloat) {
    log!("STUB: glClearDepthf({})", d);
}

#[no_mangle]
pub extern "C" fn glBlendEquationSeparate(modeRGB: GLenum, modeAlpha: GLenum) {
    log!("STUB: glBlendEquationSeparate(0x{:x}, 0x{:x})", modeRGB, modeAlpha);
}

#[no_mangle]
pub extern "C" fn glBlendFuncSeparate(srcRGB: GLenum, dstRGB: GLenum, srcAlpha: GLenum, dstAlpha: GLenum) {
    log!("STUB: glBlendFuncSeparate(0x{:x}, 0x{:x}, 0x{:x}, 0x{:x})", srcRGB, dstRGB, srcAlpha, dstAlpha);
}

#[no_mangle]
pub extern "C" fn glStencilOpSeparate(face: GLenum, _sfail: GLenum, _dpfail: GLenum, _dppass: GLenum) {
    log!("STUB: glStencilOpSeparate(face=0x{:x})", face);
}

#[no_mangle]
pub extern "C" fn glStencilFuncSeparate(face: GLenum, _func: GLenum, _ref_: GLint, _mask: GLuint) {
    log!("STUB: glStencilFuncSeparate(face=0x{:x})", face);
}

#[no_mangle]
pub extern "C" fn glStencilMaskSeparate(face: GLenum, _mask: GLuint) {
    log!("STUB: glStencilMaskSeparate(face=0x{:x})", face);
}

#[no_mangle]
pub extern "C" fn glColorMask(_red: GLboolean, _green: GLboolean, _blue: GLboolean, _alpha: GLboolean) {
    log!("STUB: glColorMask()");
}

// MARK: - Other Functions

#[no_mangle]
pub extern "C" fn glReleaseShaderCompiler() {
    log!("STUB: glReleaseShaderCompiler()");
}

#[no_mangle]
pub extern "C" fn glGetString(name: GLenum) -> *const GLubyte {
    log!("STUB: glGetString(name=0x{:x})", name);
    match name {
        0x1F02 => b"OpenGL ES 2.0 (touchHLE stub)\0".as_ptr() as *const GLubyte, // GL_VERSION
        0x1F00 => b"touchHLE\0".as_ptr() as *const GLubyte, // GL_VENDOR
        0x1F01 => b"touchHLE ES 2.0 Stub Renderer\0".as_ptr() as *const GLubyte, // GL_RENDERER
        0x8B8C => b"OpenGL ES GLSL ES 1.00 (touchHLE stub)\0".as_ptr() as *const GLubyte, // GL_SHADING_LANGUAGE_VERSION
        _ => std::ptr::null(),
    }
}

#[no_mangle]
pub extern "C" fn glGetIntegerv(pname: GLenum, data: *mut GLint) {
    log!("STUB: glGetIntegerv(pname=0x{:x})", pname);
    if !data.is_null() {
        unsafe {
            match pname {
                0x8B4D => *data = 8, // GL_MAX_VERTEX_ATTRIBS
                0x8B4F => *data = 16, // GL_MAX_TEXTURE_IMAGE_UNITS
                0x8DFD => *data = 8, // GL_MAX_COMBINED_TEXTURE_IMAGE_UNITS
                0x8B4A => *data = 16, // GL_MAX_VERTEX_TEXTURE_IMAGE_UNITS
                0x8DF9 => *data = 1, // GL_MAX_RENDERBUFFER_SIZE
                0x8D57 => *data = 1024, // GL_MAX_TEXTURE_SIZE
                0x8B82 => *data = 0, // GL_MAX_FRAGMENT_UNIFORM_VECTORS
                0x8B4B => *data = 128, // GL_MAX_VERTEX_UNIFORM_VECTORS
                0x8B4C => *data = 8, // GL_MAX_VARYING_VECTORS
                _ => *data = 0,
            }
        }
    }
}

#[no_mangle]
pub extern "C" fn glGetFloatv(pname: GLenum, data: *mut GLfloat) {
    log!("STUB: glGetFloatv(pname=0x{:x})", pname);
    if !data.is_null() {
        unsafe {
            *data = 0.0;
        }
    }
}

#[no_mangle]
pub extern "C" fn glGetBooleanv(pname: GLenum, data: *mut GLboolean) {
    log!("STUB: glGetBooleanv(pname=0x{:x})", pname);
    if !data.is_null() {
        unsafe {
            *data = 0;
        }
    }
}

#[no_mangle]
pub extern "C" fn glIsEnabled(cap: GLenum) -> GLboolean {
    log!("STUB: glIsEnabled(cap=0x{:x})", cap);
    0 // GL_FALSE
}

#[no_mangle]
pub extern "C" fn glEnable(cap: GLenum) {
    log!("STUB: glEnable(cap=0x{:x})", cap);
}

#[no_mangle]
pub extern "C" fn glDisable(cap: GLenum) {
    log!("STUB: glDisable(cap=0x{:x})", cap);
}

#[no_mangle]
pub extern "C" fn glViewport(x: GLint, y: GLint, width: GLsizei, height: GLsizei) {
    log!("STUB: glViewport({}, {}, {}, {})", x, y, width, height);
}

#[no_mangle]
pub extern "C" fn glScissor(x: GLint, y: GLint, width: GLsizei, height: GLsizei) {
    log!("STUB: glScissor({}, {}, {}, {})", x, y, width, height);
}

#[no_mangle]
pub extern "C" fn glClear(mask: GLbitfield) {
    log!("STUB: glClear(mask=0x{:x})", mask);
}

#[no_mangle]
pub extern "C" fn glClearColor(red: GLfloat, green: GLfloat, blue: GLfloat, alpha: GLfloat) {
    log!("STUB: glClearColor({}, {}, {}, {})", red, green, blue, alpha);
}

#[no_mangle]
pub extern "C" fn glClearStencil(s: GLint) {
    log!("STUB: glClearStencil({})", s);
}

#[no_mangle]
pub extern "C" fn glClearDepth(d: GLdouble) {
    log!("STUB: glClearDepth({})", d);
}

#[no_mangle]
pub extern "C" fn glFinish() {
    log!("STUB: glFinish()");
}

#[no_mangle]
pub extern "C" fn glFlush() {
    log!("STUB: glFlush()");
}

#[no_mangle]
pub extern "C" fn glPixelStorei(pname: GLenum, param: GLint) {
    log!("STUB: glPixelStorei(pname=0x{:x}, param={})", pname, param);
}

#[no_mangle]
pub extern "C" fn glReadPixels(x: GLint, y: GLint, width: GLsizei, height: GLsizei, format: GLenum, type_: GLenum, _pixels: *mut c_void) {
    log!("STUB: glReadPixels({}, {}, {}, {}, 0x{:x}, 0x{:x})", x, y, width, height, format, type_);
}

// MARK: - Drawing Functions

#[no_mangle]
pub extern "C" fn glDrawArrays(mode: GLenum, first: GLint, count: GLsizei) {
    log!("STUB: glDrawArrays(mode=0x{:x}, first={}, count={})", mode, first, count);
}

#[no_mangle]
pub extern "C" fn glDrawElements(mode: GLenum, count: GLsizei, type_: GLenum, _indices: *const c_void) {
    log!("STUB: glDrawElements(mode=0x{:x}, count={}, type=0x{:x})", mode, count, type_);
}

// MARK: - Capability Queries

#[no_mangle]
pub extern "C" fn glIsShader(shader: GLuint) -> GLboolean {
    log!("STUB: glIsShader(shader={})", shader);
    1 // GL_TRUE
}

#[no_mangle]
pub extern "C" fn glIsProgram(program: GLuint) -> GLboolean {
    log!("STUB: glIsProgram(program={})", program);
    1 // GL_TRUE
}

#[no_mangle]
pub extern "C" fn glIsBuffer(buffer: GLuint) -> GLboolean {
    log!("STUB: glIsBuffer(buffer={})", buffer);
    if buffer > 0 { 1 } else { 0 }
}

#[no_mangle]
pub extern "C" fn glIsFramebuffer(framebuffer: GLuint) -> GLboolean {
    log!("STUB: glIsFramebuffer(framebuffer={})", framebuffer);
    if framebuffer > 0 { 1 } else { 0 }
}

#[no_mangle]
pub extern "C" fn glIsRenderbuffer(renderbuffer: GLuint) -> GLboolean {
    log!("STUB: glIsRenderbuffer(renderbuffer={})", renderbuffer);
    if renderbuffer > 0 { 1 } else { 0 }
}

#[no_mangle]
pub extern "C" fn glIsTexture(texture: GLuint) -> GLboolean {
    log!("STUB: glIsTexture(texture={})", texture);
    if texture > 0 { 1 } else { 0 }
}

// MARK: - Shader Precision

#[no_mangle]
pub extern "C" fn glGetShaderPrecisionFormat(_shadertype: GLenum, _precisiontype: GLenum, range: *mut GLint, precision: *mut GLint) {
    log!("STUB: glGetShaderPrecisionFormat");
    if !range.is_null() {
        unsafe {
            *range = 127;
            *range.offset(1) = 127;
        }
    }
    if !precision.is_null() {
        unsafe {
            *precision = 23;
        }
    }
}

// MARK: - Hint

#[no_mangle]
pub extern "C" fn glHint(target: GLenum, mode: GLenum) {
    log!("STUB: glHint(target=0x{:x}, mode=0x{:x})", target, mode);
}

// MARK: - Line Width

#[no_mangle]
pub extern "C" fn glLineWidth(width: GLfloat) {
    log!("STUB: glLineWidth({})", width);
}

// MARK: - Polygon Offset

#[no_mangle]
pub extern "C" fn glPolygonOffset(factor: GLfloat, units: GLfloat) {
    log!("STUB: glPolygonOffset({}, {})", factor, units);
}

// MARK: - Sample Coverage

#[no_mangle]
pub extern "C" fn glSampleCoverage(value: GLfloat, invert: GLboolean) {
    log!("STUB: glSampleCoverage({}, {})", value, invert);
}

// MARK: - Stencil Functions

#[no_mangle]
pub extern "C" fn glStencilFunc(func: GLenum, _ref_: GLint, _mask: GLuint) {
    log!("STUB: glStencilFunc(func=0x{:x})", func);
}

#[no_mangle]
pub extern "C" fn glStencilMask(mask: GLuint) {
    log!("STUB: glStencilMask({})", mask);
}

#[no_mangle]
pub extern "C" fn glStencilOp(fail: GLenum, zfail: GLenum, zpass: GLenum) {
    log!("STUB: glStencilOp(fail=0x{:x}, zfail=0x{:x}, zpass=0x{:x})", fail, zfail, zpass);
}

// MARK: - Depth Functions

#[no_mangle]
pub extern "C" fn glDepthFunc(func: GLenum) {
    log!("STUB: glDepthFunc(func=0x{:x})", func);
}

#[no_mangle]
pub extern "C" fn glDepthMask(flag: GLboolean) {
    log!("STUB: glDepthMask({})", flag);
}

// MARK: - Cull Face

#[no_mangle]
pub extern "C" fn glCullFace(mode: GLenum) {
    log!("STUB: glCullFace(mode=0x{:x})", mode);
}

#[no_mangle]
pub extern "C" fn glFrontFace(mode: GLenum) {
    log!("STUB: glFrontFace(mode=0x{:x})", mode);
}

// MARK: - Color Mask (duplicate for completeness)

#[no_mangle]
pub extern "C" fn glColorMaski(index: GLuint, _r: GLboolean, _g: GLboolean, _b: GLboolean, _a: GLboolean) {
    log!("STUB: glColorMaski(index={})", index);
}

// MARK: - Blend Functions

#[no_mangle]
pub extern "C" fn glBlendEquation(mode: GLenum) {
    log!("STUB: glBlendEquation(mode=0x{:x})", mode);
}

#[no_mangle]
pub extern "C" fn glBlendFunc(sfactor: GLenum, dfactor: GLenum) {
    log!("STUB: glBlendFunc(0x{:x}, 0x{:x})", sfactor, dfactor);
}

#[no_mangle]
pub extern "C" fn glBlendColor(red: GLfloat, green: GLfloat, blue: GLfloat, alpha: GLfloat) {
    log!("STUB: glBlendColor({}, {}, {}, {})", red, green, blue, alpha);
}

// MARK: - Generate Mipmap

#[no_mangle]
pub extern "C" fn glGenerateMipmap(target: GLenum) {
    log!("STUB: glGenerateMipmap(target=0x{:x})", target);
}

// MARK: - Tex Parameter

#[no_mangle]
pub extern "C" fn glTexParameterf(target: GLenum, pname: GLenum, _param: GLfloat) {
    log!("STUB: glTexParameterf(target=0x{:x}, pname=0x{:x})", target, pname);
}

#[no_mangle]
pub extern "C" fn glTexParameteri(target: GLenum, pname: GLenum, _param: GLint) {
    log!("STUB: glTexParameteri(target=0x{:x}, pname=0x{:x})", target, pname);
}

#[no_mangle]
pub extern "C" fn glTexParameterfv(target: GLenum, pname: GLenum, _params: *const GLfloat) {
    log!("STUB: glTexParameterfv(target=0x{:x}, pname=0x{:x})", target, pname);
}

#[no_mangle]
pub extern "C" fn glTexParameteriv(target: GLenum, pname: GLenum, _params: *const GLint) {
    log!("STUB: glTexParameteriv(target=0x{:x}, pname=0x{:x})", target, pname);
}

// MARK: - Get Tex Parameter

#[no_mangle]
pub extern "C" fn glGetTexParameterfv(target: GLenum, pname: GLenum, params: *mut GLfloat) {
    log!("STUB: glGetTexParameterfv(target=0x{:x}, pname=0x{:x})", target, pname);
    if !params.is_null() {
        unsafe { *params = 0.0; }
    }
}

#[no_mangle]
pub extern "C" fn glGetTexParameteriv(target: GLenum, pname: GLenum, params: *mut GLint) {
    log!("STUB: glGetTexParameteriv(target=0x{:x}, pname=0x{:x})", target, pname);
    if !params.is_null() {
        unsafe { *params = 0; }
    }
}

// MARK: - Bind Texture

#[no_mangle]
pub extern "C" fn glBindTexture(target: GLenum, texture: GLuint) {
    log!("STUB: glBindTexture(target=0x{:x}, texture={})", target, texture);
}

// MARK: - Delete Textures

#[no_mangle]
pub extern "C" fn glDeleteTextures(n: GLsizei, _textures: *const GLuint) {
    log!("STUB: glDeleteTextures(n={})", n);
}

// MARK: - Gen Textures

#[no_mangle]
pub extern "C" fn glGenTextures(n: GLsizei, textures: *mut GLuint) {
    log!("STUB: glGenTextures(n={})", n);
    if !textures.is_null() && n > 0 {
        unsafe {
            for i in 0..n as isize {
                *textures.offset(i) = (i + 1) as GLuint;
            }
        }
    }
}

// MARK: - Tex Image 2D

#[no_mangle]
pub extern "C" fn glTexImage2D(target: GLenum, _level: GLint, _internalformat: GLint, width: GLsizei, height: GLsizei, _border: GLint, _format: GLenum, _type_: GLenum, _pixels: *const c_void) {
    log!("STUB: glTexImage2D(target=0x{:x}, {}x{})", target, width, height);
}

// MARK: - Tex Sub Image 2D

#[no_mangle]
pub extern "C" fn glTexSubImage2D(target: GLenum, _level: GLint, _xoffset: GLint, _yoffset: GLint, _width: GLsizei, _height: GLsizei, _format: GLenum, _type_: GLenum, _pixels: *const c_void) {
    log!("STUB: glTexSubImage2D(target=0x{:x})", target);
}

// MARK: - Copy Tex Image 2D

#[no_mangle]
pub extern "C" fn glCopyTexImage2D(target: GLenum, _level: GLint, _internalformat: GLenum, _x: GLint, _y: GLint, _width: GLsizei, _height: GLsizei, _border: GLint) {
    log!("STUB: glCopyTexImage2D(target=0x{:x})", target);
}

// MARK: - Copy Tex Sub Image 2D

#[no_mangle]
pub extern "C" fn glCopyTexSubImage2D(target: GLenum, _level: GLint, _xoffset: GLint, _yoffset: GLint, _x: GLint, _y: GLint, _width: GLsizei, _height: GLsizei) {
    log!("STUB: glCopyTexSubImage2D(target=0x{:x})", target);
}

// MARK: - Compressed Tex Image 2D

#[no_mangle]
pub extern "C" fn glCompressedTexImage2D(target: GLenum, _level: GLint, _internalformat: GLenum, _width: GLsizei, _height: GLsizei, _border: GLint, _imageSize: GLsizei, _data: *const c_void) {
    log!("STUB: glCompressedTexImage2D(target=0x{:x})", target);
}

// MARK: - Compressed Tex Sub Image 2D

#[no_mangle]
pub extern "C" fn glCompressedTexSubImage2D(target: GLenum, _level: GLint, _xoffset: GLint, _yoffset: GLint, _width: GLsizei, _height: GLsizei, _format: GLenum, _imageSize: GLsizei, _data: *const c_void) {
    log!("STUB: glCompressedTexSubImage2D(target=0x{:x})", target);
}

// MARK: - Get Error

#[no_mangle]
pub extern "C" fn glGetError() -> GLenum {
    0 // GL_NO_ERROR
}

