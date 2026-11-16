/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! OpenGL ES 1.1 fixed-function emulation on an OpenGL ES 2.0 or 3.0 context.
//!
//! OpenGL ES 2.0 removed the fixed-function pipeline. This backend keeps the
//! GLES 1.1 API state on the CPU and renders it through a small GLSL ES 1.00
//! program. It is intended for Android devices whose native GLES 1.1 path can
//! render a black frame while their GLES 2.0/3.0 path works correctly.

use super::gles11_raw as es1;
use super::gles2_raw as gl;
use super::gles2_raw::types::*;
use super::gles_generic::{GLchar, GLES};
use super::util::{fixed_to_float, float_to_fixed, try_decode_pvrtc, PalettedTextureFormat};
use super::GLESContext;
use crate::window::{GLContext, GLVersion, Window};
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::marker::PhantomData;

const ATTR_POSITION: GLuint = 0;
const ATTR_COLOR: GLuint = 1;
const ATTR_NORMAL: GLuint = 2;
const ATTR_TEX0: GLuint = 3;
const ATTR_TEX1: GLuint = 4;
const ATTR_TEX2: GLuint = 5;
const ATTR_TEX3: GLuint = 6;
const ATTR_MATRIX_INDEX: GLuint = 7;
const ATTR_WEIGHT: GLuint = 8;
const ATTR_POINT_SIZE: GLuint = 9;
const MAX_TEXTURE_UNITS: usize = 4;
const MAX_PALETTE_MATRICES: usize = 9;
const MATRIX_IDENTITY: [GLfloat; 16] = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
];

#[derive(Clone, Copy)]
struct ArrayState {
    size: GLint,
    type_: GLenum,
    stride: GLsizei,
    pointer: *const GLvoid,
    buffer_binding: GLuint,
    enabled: bool,
    fixed: bool,
    normalized: bool,
}

impl Default for ArrayState {
    fn default() -> Self {
        Self {
            size: 4,
            type_: gl::FLOAT,
            stride: 0,
            pointer: std::ptr::null(),
            buffer_binding: 0,
            enabled: false,
            fixed: false,
            normalized: false,
        }
    }
}

struct MatrixState {
    current: [GLfloat; 16],
    stack: Vec<[GLfloat; 16]>,
}

impl MatrixState {
    fn new() -> Self {
        Self {
            current: MATRIX_IDENTITY,
            stack: Vec::new(),
        }
    }
}

#[derive(Clone, Copy)]
struct LightState {
    ambient: [GLfloat; 4],
    diffuse: [GLfloat; 4],
    specular: [GLfloat; 4],
    position: [GLfloat; 4],
    spot_direction: [GLfloat; 3],
    spot_cutoff: GLfloat,
    spot_exponent: GLfloat,
    constant_attenuation: GLfloat,
    linear_attenuation: GLfloat,
    quadratic_attenuation: GLfloat,
}

impl Default for LightState {
    fn default() -> Self {
        Self {
            ambient: [0.0, 0.0, 0.0, 1.0],
            diffuse: [1.0, 1.0, 1.0, 1.0],
            specular: [1.0, 1.0, 1.0, 1.0],
            position: [0.0, 0.0, 1.0, 0.0],
            spot_direction: [0.0, 0.0, -1.0],
            spot_cutoff: 180.0,
            spot_exponent: 0.0,
            constant_attenuation: 1.0,
            linear_attenuation: 0.0,
            quadratic_attenuation: 0.0,
        }
    }
}

struct TranslatorState {
    modelview: MatrixState,
    projection: MatrixState,
    texture: [MatrixState; MAX_TEXTURE_UNITS],
    matrix_mode: GLenum,
    active_texture: usize,
    client_active_texture: usize,
    bound_textures: [GLuint; MAX_TEXTURE_UNITS],
    color: [GLfloat; 4],
    normal: [GLfloat; 3],
    texcoords: [[GLfloat; 4]; MAX_TEXTURE_UNITS],
    arrays: [ArrayState; 3],
    texcoord_arrays: [ArrayState; MAX_TEXTURE_UNITS],
    palette_index_array: ArrayState,
    palette_weight_array: ArrayState,
    point_size_array: ArrayState,
    palette_matrices: [MatrixState; MAX_PALETTE_MATRICES],
    current_palette_matrix: usize,
    matrix_palette_enabled: bool,
    texture_enabled: [bool; MAX_TEXTURE_UNITS],
    texture_env_mode: [GLint; MAX_TEXTURE_UNITS],
    texture_env_color: [[GLfloat; 4]; MAX_TEXTURE_UNITS],
    texture_combine_rgb: [GLenum; MAX_TEXTURE_UNITS],
    texture_combine_alpha: [GLenum; MAX_TEXTURE_UNITS],
    texture_src_rgb: [[GLenum; 3]; MAX_TEXTURE_UNITS],
    texture_src_alpha: [[GLenum; 3]; MAX_TEXTURE_UNITS],
    texture_operand_rgb: [[GLenum; 3]; MAX_TEXTURE_UNITS],
    texture_operand_alpha: [[GLenum; 3]; MAX_TEXTURE_UNITS],
    texture_rgb_scale: [GLfloat; MAX_TEXTURE_UNITS],
    texture_alpha_scale: [GLfloat; MAX_TEXTURE_UNITS],
    fixed_buffers: [Vec<GLfloat>; 3],
    client_array_vbos: [GLuint; 10],
    client_element_vbo: GLuint,
    array_buffer_binding: GLuint,
    element_array_buffer_binding: GLuint,
    array_buffer_data: HashMap<GLuint, Vec<u8>>,
    element_array_buffer_data: HashMap<GLuint, Vec<u8>>,
    mapped_buffer: Option<(GLenum, GLuint)>,
    point_size: GLfloat,
    alpha_test_enabled: bool,
    alpha_func: GLenum,
    alpha_ref: GLclampf,
    fog_enabled: bool,
    fog_mode: GLenum,
    fog_density: GLfloat,
    fog_start: GLfloat,
    fog_end: GLfloat,
    fog_color: [GLfloat; 4],
    lighting_enabled: bool,
    light_enabled: [bool; 8],
    color_material_enabled: bool,
    normalize_enabled: bool,
    lights: [LightState; 8],
    material_ambient: [GLfloat; 4],
    material_diffuse: [GLfloat; 4],
    material_specular: [GLfloat; 4],
    material_emission: [GLfloat; 4],
    material_shininess: GLfloat,
    model_ambient: [GLfloat; 4],
    light_model_local_viewer: bool,
    light_model_two_side: bool,
    shade_model: GLenum,
    hints: [GLenum; 4],
    clip_planes: [[GLfloat; 4]; 6],
    clip_plane_enabled: [bool; 6],
    point_distance_attenuation: [GLfloat; 3],
    point_fade_threshold: GLfloat,
    texture_crop_rect: [GLint; 4],
    viewport: [GLint; 4],
    program: Option<GLuint>,
    program_creation_failed: bool,
    logic_op_enabled: bool,
    logic_op: GLenum,
    gles3: bool,
}

impl TranslatorState {
    fn new() -> Self {
        Self {
            modelview: MatrixState::new(),
            projection: MatrixState::new(),
            texture: std::array::from_fn(|_| MatrixState::new()),
            matrix_mode: es1::MODELVIEW,
            active_texture: 0,
            client_active_texture: 0,
            bound_textures: [0; MAX_TEXTURE_UNITS],
            color: [1.0; 4],
            normal: [0.0, 0.0, 1.0],
            texcoords: [[0.0, 0.0, 0.0, 1.0]; MAX_TEXTURE_UNITS],
            arrays: [ArrayState::default(); 3],
            texcoord_arrays: [ArrayState::default(); MAX_TEXTURE_UNITS],
            palette_index_array: ArrayState::default(),
            palette_weight_array: ArrayState::default(),
            point_size_array: ArrayState::default(),
            palette_matrices: std::array::from_fn(|_| MatrixState::new()),
            current_palette_matrix: 0,
            matrix_palette_enabled: false,
            texture_enabled: [false; MAX_TEXTURE_UNITS],
            texture_env_mode: [es1::MODULATE as GLint; MAX_TEXTURE_UNITS],
            texture_env_color: [[0.0, 0.0, 0.0, 0.0]; MAX_TEXTURE_UNITS],
            texture_combine_rgb: [es1::MODULATE; MAX_TEXTURE_UNITS],
            texture_combine_alpha: [es1::MODULATE; MAX_TEXTURE_UNITS],
            texture_src_rgb: [[es1::TEXTURE, es1::PREVIOUS, es1::CONSTANT]; MAX_TEXTURE_UNITS],
            texture_src_alpha: [[es1::TEXTURE, es1::PREVIOUS, es1::CONSTANT]; MAX_TEXTURE_UNITS],
            texture_operand_rgb: [[es1::SRC_COLOR, es1::SRC_COLOR, es1::SRC_COLOR];
                MAX_TEXTURE_UNITS],
            texture_operand_alpha: [[es1::SRC_ALPHA, es1::SRC_ALPHA, es1::SRC_ALPHA];
                MAX_TEXTURE_UNITS],
            texture_rgb_scale: [1.0; MAX_TEXTURE_UNITS],
            texture_alpha_scale: [1.0; MAX_TEXTURE_UNITS],
            fixed_buffers: std::array::from_fn(|_| Vec::new()),
            client_array_vbos: [0; 10],
            client_element_vbo: 0,
            array_buffer_binding: 0,
            element_array_buffer_binding: 0,
            array_buffer_data: HashMap::new(),
            element_array_buffer_data: HashMap::new(),
            mapped_buffer: None,
            point_size: 1.0,
            alpha_test_enabled: false,
            alpha_func: es1::ALWAYS,
            alpha_ref: 0.0,
            fog_enabled: false,
            fog_mode: es1::EXP,
            fog_density: 1.0,
            fog_start: 0.0,
            fog_end: 1.0,
            fog_color: [0.0, 0.0, 0.0, 1.0],
            lighting_enabled: false,
            light_enabled: [false; 8],
            color_material_enabled: false,
            normalize_enabled: false,
            lights: [LightState::default(); 8],
            material_ambient: [0.2, 0.2, 0.2, 1.0],
            material_diffuse: [0.8, 0.8, 0.8, 1.0],
            material_specular: [0.0, 0.0, 0.0, 1.0],
            material_emission: [0.0, 0.0, 0.0, 1.0],
            material_shininess: 0.0,
            model_ambient: [0.2, 0.2, 0.2, 1.0],
            light_model_local_viewer: false,
            light_model_two_side: false,
            shade_model: es1::SMOOTH,
            hints: [es1::DONT_CARE; 4],
            clip_planes: [[0.0, 0.0, 0.0, 0.0]; 6],
            clip_plane_enabled: [false; 6],
            point_distance_attenuation: [1.0, 0.0, 0.0],
            point_fade_threshold: 1.0,
            texture_crop_rect: [0, 0, 0, 0],
            viewport: [0, 0, 0, 0],
            program: None,
            program_creation_failed: false,
            logic_op_enabled: false,
            logic_op: es1::COPY,
            gles3: false,
        }
    }

    fn matrix_mut(&mut self) -> &mut MatrixState {
        match self.matrix_mode {
            es1::PROJECTION => &mut self.projection,
            es1::TEXTURE => &mut self.texture[self.active_texture],
            es1::MATRIX_PALETTE_OES => &mut self.palette_matrices[self.current_palette_matrix],
            _ => &mut self.modelview,
        }
    }

    fn mvp(&self) -> [GLfloat; 16] {
        multiply(&self.projection.current, &self.modelview.current)
    }
}

pub struct GLES1OnGLES2Context {
    gl_ctx: GLContext,
    is_loaded: bool,
    gles3: bool,
    state: TranslatorState,
}

impl GLES1OnGLES2Context {
    pub fn new_with_gl_version(window: &mut Window, version: GLVersion) -> Result<Self, String> {
        let gles3 = matches!(version, GLVersion::GLES30);
        Ok(Self {
            gl_ctx: window.create_gl_context(version)?,
            is_loaded: false,
            gles3,
            state: TranslatorState {
                gles3,
                ..TranslatorState::new()
            },
        })
    }
}

impl GLESContext for GLES1OnGLES2Context {
    fn description() -> &'static str {
        "OpenGL ES 1.1 translated to native OpenGL ES 2.0 shaders"
    }

    fn new(window: &mut Window) -> Result<Self, String> {
        Self::new_with_gl_version(window, GLVersion::GLES20)
    }

    fn make_current<'gl_ctx, 'win: 'gl_ctx>(
        &'gl_ctx mut self,
        window: &'win mut Window,
    ) -> Box<dyn GLES + 'gl_ctx> {
        if !self.gl_ctx.is_current() || !self.is_loaded {
            unsafe { window.make_gl_context_current(&self.gl_ctx) };
            gl::load_with(|s| window.gl_get_proc_address(s));
            es1::load_with(|s| window.gl_get_proc_address(s));
            self.is_loaded = true;
        }
        Box::new(GLES1OnGLES2 {
            state: &mut self.state,
            _gl_lifetime: PhantomData,
        })
    }

    unsafe fn make_current_unchecked_for_window<'gl_ctx>(
        &'gl_ctx mut self,
        make_current_fn: &mut dyn FnMut(&GLContext),
        loader_fn: &mut dyn FnMut(&'static str) -> *const std::ffi::c_void,
    ) -> Box<dyn GLES + 'gl_ctx> {
        if !self.gl_ctx.is_current() || !self.is_loaded {
            make_current_fn(&self.gl_ctx);
            gl::load_with(&mut *loader_fn);
            es1::load_with(&mut *loader_fn);
            self.is_loaded = true;
        }
        Box::new(GLES1OnGLES2 {
            state: &mut self.state,
            _gl_lifetime: PhantomData,
        })
    }
}

pub struct GLES1OnGLES2<'a> {
    state: &'a mut TranslatorState,
    _gl_lifetime: PhantomData<&'a ()>,
}

fn multiply(a: &[GLfloat; 16], b: &[GLfloat; 16]) -> [GLfloat; 16] {
    let mut out = [0.0; 16];
    for col in 0..4 {
        for row in 0..4 {
            out[col * 4 + row] = (0..4).map(|k| a[k * 4 + row] * b[col * 4 + k]).sum();
        }
    }
    out
}

fn translation(x: GLfloat, y: GLfloat, z: GLfloat) -> [GLfloat; 16] {
    let mut m = MATRIX_IDENTITY;
    m[12] = x;
    m[13] = y;
    m[14] = z;
    m
}

fn scaling(x: GLfloat, y: GLfloat, z: GLfloat) -> [GLfloat; 16] {
    let mut m = MATRIX_IDENTITY;
    m[0] = x;
    m[5] = y;
    m[10] = z;
    m
}

fn rotation(angle: GLfloat, x: GLfloat, y: GLfloat, z: GLfloat) -> [GLfloat; 16] {
    let length = (x * x + y * y + z * z).sqrt();
    if length == 0.0 {
        return MATRIX_IDENTITY;
    }
    let (x, y, z) = (x / length, y / length, z / length);
    let r = angle.to_radians();
    let (s, c) = (r.sin(), r.cos());
    let t = 1.0 - c;
    [
        t * x * x + c,
        t * x * y + s * z,
        t * x * z - s * y,
        0.0,
        t * x * y - s * z,
        t * y * y + c,
        t * y * z + s * x,
        0.0,
        t * x * z + s * y,
        t * y * z - s * x,
        t * z * z + c,
        0.0,
        0.0,
        0.0,
        0.0,
        1.0,
    ]
}

fn ortho(
    left: GLfloat,
    right: GLfloat,
    bottom: GLfloat,
    top: GLfloat,
    near: GLfloat,
    far: GLfloat,
) -> [GLfloat; 16] {
    [
        2.0 / (right - left),
        0.0,
        0.0,
        0.0,
        0.0,
        2.0 / (top - bottom),
        0.0,
        0.0,
        0.0,
        0.0,
        -2.0 / (far - near),
        0.0,
        -(right + left) / (right - left),
        -(top + bottom) / (top - bottom),
        -(far + near) / (far - near),
        1.0,
    ]
}

fn frustum(
    left: GLfloat,
    right: GLfloat,
    bottom: GLfloat,
    top: GLfloat,
    near: GLfloat,
    far: GLfloat,
) -> [GLfloat; 16] {
    [
        2.0 * near / (right - left),
        0.0,
        0.0,
        0.0,
        0.0,
        2.0 * near / (top - bottom),
        0.0,
        0.0,
        (right + left) / (right - left),
        (top + bottom) / (top - bottom),
        -(far + near) / (far - near),
        -1.0,
        0.0,
        0.0,
        -2.0 * far * near / (far - near),
        0.0,
    ]
}

fn translate_fixed_function_shader(source: &str, kind: GLenum, gles3: bool) -> String {
    if !gles3 {
        return source.to_owned();
    }

    let mut translated = source.replacen("#version 100", "#version 300 es", 1);
    if kind == gl::VERTEX_SHADER {
        translated = translated.replace("attribute ", "in ");
        translated = translated.replace("varying ", "out ");
    } else {
        translated = translated.replace("varying ", "in ");
        translated = translated.replace("texture2D(", "texture(");
        translated = translated.replace("texture2DProj(", "textureProj(");
        translated = translated.replace("gl_FragColor", "frag_color");
        translated = translated.replacen(
            "precision mediump float;",
            "precision mediump float;\nout vec4 frag_color;",
            1,
        );
    }
    translated
}

fn compile_shader(kind: GLenum, source: &str, gles3: bool) -> Result<GLuint, String> {
    unsafe {
        let shader = gl::CreateShader(kind);
        let source = translate_fixed_function_shader(source, kind, gles3);
        let source = CString::new(source).unwrap();
        let pointer = source.as_ptr();
        gl::ShaderSource(shader, 1, &pointer, std::ptr::null());
        gl::CompileShader(shader);
        let mut ok = 0;
        gl::GetShaderiv(shader, gl::COMPILE_STATUS, &mut ok);
        if ok == 0 {
            let mut log = [0i8; 2048];
            let mut len = 0;
            gl::GetShaderInfoLog(
                shader,
                log.len() as GLsizei,
                &mut len,
                log.as_mut_ptr() as _,
            );
            return Err(format!(
                "GLES1-on-GLES2 shader compilation failed: {}",
                String::from_utf8_lossy(std::slice::from_raw_parts(
                    log.as_ptr() as *const u8,
                    len.max(0) as usize
                ))
            ));
        }
        Ok(shader)
    }
}

fn create_program(gles3: bool) -> Result<GLuint, String> {
    let vertex = compile_shader(
        gl::VERTEX_SHADER,
        r#"#version 100
precision mediump float;
precision mediump int;
attribute vec4 a_position;
attribute vec4 a_color;
attribute vec3 a_normal;
attribute vec4 a_tex0;
attribute vec4 a_tex1;
attribute vec4 a_tex2;
attribute vec4 a_tex3;
attribute vec4 a_matrix_index;
attribute vec4 a_weight;
attribute float a_point_size;
uniform mat4 u_mvp;
uniform mat4 u_modelview;
uniform mat4 u_projection;
uniform mat4 u_texture_matrix0;
uniform mat4 u_texture_matrix1;
uniform mat4 u_texture_matrix2;
uniform mat4 u_texture_matrix3;
uniform vec4 u_color;
uniform float u_point_size;
uniform int u_point_size_array_enabled;
uniform mat4 u_palette_matrices[9];
uniform int u_matrix_palette_enabled;
uniform int u_lighting_enabled;
uniform int u_light_enabled[8];
uniform vec4 u_light_ambient[8];
uniform vec4 u_light_diffuse[8];
uniform vec4 u_light_specular[8];
uniform vec4 u_light_position[8];
uniform vec3 u_light_spot_direction[8];
uniform float u_light_spot_cutoff[8];
uniform float u_light_spot_exponent[8];
uniform float u_light_constant_attenuation[8];
uniform float u_light_linear_attenuation[8];
uniform float u_light_quadratic_attenuation[8];
uniform int u_color_material_enabled;
uniform int u_normalize_enabled;
uniform int u_light_model_local_viewer;
uniform int u_light_model_two_side;
uniform vec4 u_material_ambient;
uniform vec4 u_material_diffuse;
uniform vec4 u_material_specular;
uniform float u_material_shininess;
uniform vec4 u_model_ambient;
uniform vec4 u_material_emission;
uniform vec4 u_clip_planes[6];
uniform int u_clip_enabled[6];
uniform vec3 u_point_distance_attenuation;
uniform float u_point_fade_threshold;
varying vec4 v_color;
varying vec2 v_tex0;
varying vec2 v_tex1;
varying vec2 v_tex2;
varying vec2 v_tex3;
varying float v_fog_coord;
varying vec4 v_clip_distances0;
varying vec2 v_clip_distances1;
void main() {
    vec4 transformed_position = a_position;
    if (u_matrix_palette_enabled != 0) {
        transformed_position = vec4(0.0);
        for (int i = 0; i < 4; i++) {
            int matrix_index = int(a_matrix_index[i]);
            matrix_index = matrix_index < 0 ? 0 : matrix_index;
            matrix_index = matrix_index > 8 ? 8 : matrix_index;
            transformed_position += a_weight[i] * (u_palette_matrices[matrix_index] * a_position);
        }
    }
    vec4 eye_position;
    if (u_matrix_palette_enabled != 0) {
        eye_position = transformed_position;
        gl_Position = u_projection * transformed_position;
    } else {
        eye_position = u_modelview * transformed_position;
        gl_Position = u_mvp * transformed_position;
    }
    float point_distance = length(eye_position.xyz);
    float point_attenuation = sqrt(max(u_point_distance_attenuation.x + u_point_distance_attenuation.y * point_distance + u_point_distance_attenuation.z * point_distance * point_distance, 0.0001));
    gl_PointSize = (u_point_size_array_enabled != 0 ? a_point_size : u_point_size) / point_attenuation;
    vec3 transformed_normal = (u_modelview * vec4(a_normal, 0.0)).xyz;
    if (u_normalize_enabled != 0) transformed_normal = normalize(transformed_normal);
    vec4 base_color = a_color * u_color;
    if (u_lighting_enabled != 0) {
        vec4 material_diffuse = u_color_material_enabled != 0 ? base_color : u_material_diffuse;
        vec4 material_ambient = u_color_material_enabled != 0 ? base_color : u_material_ambient;
        vec3 lit_rgb = u_model_ambient.rgb * material_ambient.rgb + u_material_emission.rgb;
        vec3 view_direction = u_light_model_local_viewer != 0
            ? normalize(-eye_position.xyz)
            : normalize(vec3(0.0, 0.0, 1.0));
        for (int i = 0; i < 8; i++) {
            if (u_light_enabled[i] == 0) continue;
            vec3 light_direction = u_light_position[i].w == 0.0 ? normalize(u_light_position[i].xyz) : normalize(u_light_position[i].xyz - eye_position.xyz);
            float distance_to_light = u_light_position[i].w == 0.0 ? 1.0 : length(u_light_position[i].xyz - eye_position.xyz);
            float attenuation = 1.0 / max(u_light_constant_attenuation[i] + u_light_linear_attenuation[i] * distance_to_light + u_light_quadratic_attenuation[i] * distance_to_light * distance_to_light, 0.0001);
            float spot_factor = 1.0;
            if (u_light_position[i].w != 0.0 && u_light_spot_cutoff[i] < 180.0) {
                float spot_cos = dot(normalize(u_light_spot_direction[i]), normalize(eye_position.xyz - u_light_position[i].xyz));
                spot_factor = spot_cos < cos(radians(u_light_spot_cutoff[i])) ? 0.0 : pow(max(spot_cos, 0.0), u_light_spot_exponent[i]);
            }
            float diffuse_factor = max(dot(normalize(transformed_normal), light_direction), 0.0) * attenuation * spot_factor;
            vec3 half_vector = normalize(light_direction + view_direction);
            float specular_factor = pow(max(dot(normalize(transformed_normal), half_vector), 0.0), u_material_shininess);
            lit_rgb += u_light_ambient[i].rgb * material_ambient.rgb + u_light_diffuse[i].rgb * material_diffuse.rgb * diffuse_factor + u_light_specular[i].rgb * u_material_specular.rgb * specular_factor * attenuation * spot_factor;
        }
        v_color = vec4(lit_rgb, material_diffuse.a);
    } else {
        v_color = base_color;
    }
    v_tex0 = (u_texture_matrix0 * a_tex0).xy;
    v_tex1 = (u_texture_matrix1 * a_tex1).xy;
    v_tex2 = (u_texture_matrix2 * a_tex2).xy;
    v_tex3 = (u_texture_matrix3 * a_tex3).xy;
    v_fog_coord = abs(eye_position.z);
    v_clip_distances0 = vec4(dot(u_clip_planes[0], eye_position), dot(u_clip_planes[1], eye_position), dot(u_clip_planes[2], eye_position), dot(u_clip_planes[3], eye_position));
    v_clip_distances1 = vec2(dot(u_clip_planes[4], eye_position), dot(u_clip_planes[5], eye_position));
}
"#,
        gles3,
    )?;
    let fragment = compile_shader(
        gl::FRAGMENT_SHADER,
        r#"#version 100
precision mediump float;
precision mediump int;
varying vec4 v_color;
varying vec2 v_tex0;
varying vec2 v_tex1;
varying vec2 v_tex2;
varying vec2 v_tex3;
varying float v_fog_coord;
varying vec4 v_clip_distances0;
varying vec2 v_clip_distances1;
uniform sampler2D u_tex0;
uniform sampler2D u_tex1;
uniform sampler2D u_tex2;
uniform sampler2D u_tex3;
uniform vec4 u_env_color0;
uniform vec4 u_env_color1;
uniform vec4 u_env_color2;
uniform vec4 u_env_color3;
uniform int u_tex_enabled0;
uniform int u_tex_enabled1;
uniform int u_tex_enabled2;
uniform int u_tex_enabled3;
uniform int u_tex_mode0;
uniform int u_tex_mode1;
uniform int u_tex_mode2;
uniform int u_tex_mode3;
uniform int u_combine_rgb0;
uniform int u_combine_rgb1;
uniform int u_combine_rgb2;
uniform int u_combine_rgb3;
uniform int u_combine_alpha0;
uniform int u_combine_alpha1;
uniform int u_combine_alpha2;
uniform int u_combine_alpha3;
uniform int u_src_rgb0[3];
uniform int u_src_rgb1[3];
uniform int u_src_rgb2[3];
uniform int u_src_rgb3[3];
uniform int u_src_alpha0[3];
uniform int u_src_alpha1[3];
uniform int u_src_alpha2[3];
uniform int u_src_alpha3[3];
uniform int u_operand_rgb0[3];
uniform int u_operand_rgb1[3];
uniform int u_operand_rgb2[3];
uniform int u_operand_rgb3[3];
uniform int u_operand_alpha0[3];
uniform int u_operand_alpha1[3];
uniform int u_operand_alpha2[3];
uniform int u_operand_alpha3[3];
uniform float u_rgb_scale0;
uniform float u_rgb_scale1;
uniform float u_rgb_scale2;
uniform float u_rgb_scale3;
uniform float u_alpha_scale0;
uniform float u_alpha_scale1;
uniform float u_alpha_scale2;
uniform float u_alpha_scale3;
uniform int u_alpha_test_enabled;
uniform int u_alpha_func;
uniform float u_alpha_ref;
uniform int u_fog_enabled;
uniform vec4 u_fog_color;
uniform float u_fog_density;
uniform float u_fog_start;
uniform float u_fog_end;
uniform int u_fog_mode;
uniform int u_clip_enabled[6];
uniform int u_logic_op_enabled;
uniform int u_logic_op;
vec4 combine_source(int source, vec4 previous, vec4 texel, vec4 primary, vec4 constant_value) {
    if (source == 0x8576) return constant_value;
    if (source == 0x8577) return primary;
    if (source == 0x8578) return previous;
    return texel;
}
vec3 combine_rgb_operand(vec4 value, int operand) {
    if (operand == 0x0301) return vec3(1.0) - value.rgb;
    if (operand == 0x0302) return value.aaa;
    if (operand == 0x0303) return vec3(1.0) - value.aaa;
    return value.rgb;
}
float combine_alpha_operand(vec4 value, int operand) {
    if (operand == 0x0303) return 1.0 - value.a;
    return value.a;
}
vec4 combine_stage0(vec4 previous, vec4 texel, vec4 primary) {
    vec4 constant_value = u_env_color0;
    vec4 rgb0 = combine_source(u_src_rgb0[0], previous, texel, primary, constant_value);
    vec4 rgb1 = combine_source(u_src_rgb0[1], previous, texel, primary, constant_value);
    vec4 rgb2 = combine_source(u_src_rgb0[2], previous, texel, primary, constant_value);
    vec3 a = combine_rgb_operand(rgb0, u_operand_rgb0[0]);
    vec3 b = combine_rgb_operand(rgb1, u_operand_rgb0[1]);
    vec3 c = combine_rgb_operand(rgb2, u_operand_rgb0[2]);
    vec3 rgb;
    if (u_combine_rgb0 == 0x1E01) rgb = a;
    else if (u_combine_rgb0 == 0x0104) rgb = a + b;
    else if (u_combine_rgb0 == 0x8574) rgb = a + b - 0.5;
    else if (u_combine_rgb0 == 0x84E7) rgb = a - b;
    else if (u_combine_rgb0 == 0x8575) rgb = a * c + b * (vec3(1.0) - c);
    else if (u_combine_rgb0 == 0x86AE) rgb = vec3(4.0 * dot(a * 2.0 - 1.0, b * 2.0 - 1.0));
    else rgb = a * b;
    vec4 alpha0 = combine_source(u_src_alpha0[0], previous, texel, primary, constant_value);
    vec4 alpha1 = combine_source(u_src_alpha0[1], previous, texel, primary, constant_value);
    vec4 alpha2 = combine_source(u_src_alpha0[2], previous, texel, primary, constant_value);
    float aa = combine_alpha_operand(alpha0, u_operand_alpha0[0]);
    float ab = combine_alpha_operand(alpha1, u_operand_alpha0[1]);
    float ac = combine_alpha_operand(alpha2, u_operand_alpha0[2]);
    float alpha;
    if (u_combine_alpha0 == 0x1E01) alpha = aa;
    else if (u_combine_alpha0 == 0x0104) alpha = aa + ab;
    else if (u_combine_alpha0 == 0x8574) alpha = aa + ab - 0.5;
    else if (u_combine_alpha0 == 0x84E7) alpha = aa - ab;
    else if (u_combine_alpha0 == 0x8575) alpha = aa * ac + ab * (1.0 - ac);
    else alpha = aa * ab;
    return vec4(clamp(rgb * u_rgb_scale0, 0.0, 1.0), clamp(alpha * u_alpha_scale0, 0.0, 1.0));
}
vec4 combine_stage1(vec4 previous, vec4 texel, vec4 primary) {
    vec4 constant_value = u_env_color1;
    vec4 rgb0 = combine_source(u_src_rgb1[0], previous, texel, primary, constant_value);
    vec4 rgb1 = combine_source(u_src_rgb1[1], previous, texel, primary, constant_value);
    vec4 rgb2 = combine_source(u_src_rgb1[2], previous, texel, primary, constant_value);
    vec3 a = combine_rgb_operand(rgb0, u_operand_rgb1[0]);
    vec3 b = combine_rgb_operand(rgb1, u_operand_rgb1[1]);
    vec3 c = combine_rgb_operand(rgb2, u_operand_rgb1[2]);
    vec3 rgb;
    if (u_combine_rgb1 == 0x1E01) rgb = a;
    else if (u_combine_rgb1 == 0x0104) rgb = a + b;
    else if (u_combine_rgb1 == 0x8574) rgb = a + b - 0.5;
    else if (u_combine_rgb1 == 0x84E7) rgb = a - b;
    else if (u_combine_rgb1 == 0x8575) rgb = a * c + b * (vec3(1.0) - c);
    else if (u_combine_rgb1 == 0x86AE) rgb = vec3(4.0 * dot(a * 2.0 - 1.0, b * 2.0 - 1.0));
    else rgb = a * b;
    vec4 alpha0 = combine_source(u_src_alpha1[0], previous, texel, primary, constant_value);
    vec4 alpha1 = combine_source(u_src_alpha1[1], previous, texel, primary, constant_value);
    vec4 alpha2 = combine_source(u_src_alpha1[2], previous, texel, primary, constant_value);
    float aa = combine_alpha_operand(alpha0, u_operand_alpha1[0]);
    float ab = combine_alpha_operand(alpha1, u_operand_alpha1[1]);
    float ac = combine_alpha_operand(alpha2, u_operand_alpha1[2]);
    float alpha;
    if (u_combine_alpha1 == 0x1E01) alpha = aa;
    else if (u_combine_alpha1 == 0x0104) alpha = aa + ab;
    else if (u_combine_alpha1 == 0x8574) alpha = aa + ab - 0.5;
    else if (u_combine_alpha1 == 0x84E7) alpha = aa - ab;
    else if (u_combine_alpha1 == 0x8575) alpha = aa * ac + ab * (1.0 - ac);
    else alpha = aa * ab;
    return vec4(clamp(rgb * u_rgb_scale1, 0.0, 1.0), clamp(alpha * u_alpha_scale1, 0.0, 1.0));
}
vec4 combine_stage2(vec4 previous, vec4 texel, vec4 primary) {
    vec4 constant_value = u_env_color2;
    vec4 rgb0 = combine_source(u_src_rgb2[0], previous, texel, primary, constant_value);
    vec4 rgb1 = combine_source(u_src_rgb2[1], previous, texel, primary, constant_value);
    vec4 rgb2 = combine_source(u_src_rgb2[2], previous, texel, primary, constant_value);
    vec3 a = combine_rgb_operand(rgb0, u_operand_rgb2[0]);
    vec3 b = combine_rgb_operand(rgb1, u_operand_rgb2[1]);
    vec3 c = combine_rgb_operand(rgb2, u_operand_rgb2[2]);
    vec3 rgb;
    if (u_combine_rgb2 == 0x1E01) rgb = a;
    else if (u_combine_rgb2 == 0x0104) rgb = a + b;
    else if (u_combine_rgb2 == 0x8574) rgb = a + b - 0.5;
    else if (u_combine_rgb2 == 0x84E7) rgb = a - b;
    else if (u_combine_rgb2 == 0x8575) rgb = a * c + b * (vec3(1.0) - c);
    else if (u_combine_rgb2 == 0x86AE) rgb = vec3(4.0 * dot(a * 2.0 - 1.0, b * 2.0 - 1.0));
    else rgb = a * b;
    vec4 alpha0 = combine_source(u_src_alpha2[0], previous, texel, primary, constant_value);
    vec4 alpha1 = combine_source(u_src_alpha2[1], previous, texel, primary, constant_value);
    vec4 alpha2 = combine_source(u_src_alpha2[2], previous, texel, primary, constant_value);
    float aa = combine_alpha_operand(alpha0, u_operand_alpha2[0]);
    float ab = combine_alpha_operand(alpha1, u_operand_alpha2[1]);
    float ac = combine_alpha_operand(alpha2, u_operand_alpha2[2]);
    float alpha;
    if (u_combine_alpha2 == 0x1E01) alpha = aa;
    else if (u_combine_alpha2 == 0x0104) alpha = aa + ab;
    else if (u_combine_alpha2 == 0x8574) alpha = aa + ab - 0.5;
    else if (u_combine_alpha2 == 0x84E7) alpha = aa - ab;
    else if (u_combine_alpha2 == 0x8575) alpha = aa * ac + ab * (1.0 - ac);
    else alpha = aa * ab;
    return vec4(clamp(rgb * u_rgb_scale2, 0.0, 1.0), clamp(alpha * u_alpha_scale2, 0.0, 1.0));
}
vec4 combine_stage3(vec4 previous, vec4 texel, vec4 primary) {
    vec4 constant_value = u_env_color3;
    vec4 rgb0 = combine_source(u_src_rgb3[0], previous, texel, primary, constant_value);
    vec4 rgb1 = combine_source(u_src_rgb3[1], previous, texel, primary, constant_value);
    vec4 rgb2 = combine_source(u_src_rgb3[2], previous, texel, primary, constant_value);
    vec3 a = combine_rgb_operand(rgb0, u_operand_rgb3[0]);
    vec3 b = combine_rgb_operand(rgb1, u_operand_rgb3[1]);
    vec3 c = combine_rgb_operand(rgb2, u_operand_rgb3[2]);
    vec3 rgb;
    if (u_combine_rgb3 == 0x1E01) rgb = a;
    else if (u_combine_rgb3 == 0x0104) rgb = a + b;
    else if (u_combine_rgb3 == 0x8574) rgb = a + b - 0.5;
    else if (u_combine_rgb3 == 0x84E7) rgb = a - b;
    else if (u_combine_rgb3 == 0x8575) rgb = a * c + b * (vec3(1.0) - c);
    else if (u_combine_rgb3 == 0x86AE) rgb = vec3(4.0 * dot(a * 2.0 - 1.0, b * 2.0 - 1.0));
    else rgb = a * b;
    vec4 alpha0 = combine_source(u_src_alpha3[0], previous, texel, primary, constant_value);
    vec4 alpha1 = combine_source(u_src_alpha3[1], previous, texel, primary, constant_value);
    vec4 alpha2 = combine_source(u_src_alpha3[2], previous, texel, primary, constant_value);
    float aa = combine_alpha_operand(alpha0, u_operand_alpha3[0]);
    float ab = combine_alpha_operand(alpha1, u_operand_alpha3[1]);
    float ac = combine_alpha_operand(alpha2, u_operand_alpha3[2]);
    float alpha;
    if (u_combine_alpha3 == 0x1E01) alpha = aa;
    else if (u_combine_alpha3 == 0x0104) alpha = aa + ab;
    else if (u_combine_alpha3 == 0x8574) alpha = aa + ab - 0.5;
    else if (u_combine_alpha3 == 0x84E7) alpha = aa - ab;
    else if (u_combine_alpha3 == 0x8575) alpha = aa * ac + ab * (1.0 - ac);
    else alpha = aa * ab;
    return vec4(clamp(rgb * u_rgb_scale3, 0.0, 1.0), clamp(alpha * u_alpha_scale3, 0.0, 1.0));
}
float fog_factor() {
    if (u_fog_mode == 2048) return exp(-u_fog_density * v_fog_coord);
    if (u_fog_mode == 2049) {
        float d = u_fog_density * v_fog_coord;
        return exp(-d * d);
    }
    return (u_fog_end - v_fog_coord) / (u_fog_end - u_fog_start);
}
bool alpha_pass(float alpha) {
    if (u_alpha_func == 512) return false;
    if (u_alpha_func == 513) return alpha < u_alpha_ref;
    if (u_alpha_func == 514) return alpha == u_alpha_ref;
    if (u_alpha_func == 515) return alpha <= u_alpha_ref;
    if (u_alpha_func == 516) return alpha > u_alpha_ref;
    if (u_alpha_func == 517) return alpha != u_alpha_ref;
    if (u_alpha_func == 518) return alpha >= u_alpha_ref;
    return true;
}
void main() {
    vec4 color = v_color;
    if (u_tex_enabled0 != 0) {
        vec4 texel = texture2D(u_tex0, v_tex0);
        if (u_tex_mode0 == 1) color = vec4(texel.rgb * v_color.rgb, texel.a * v_color.a);
        else if (u_tex_mode0 == 2) color = color * texel;
        else if (u_tex_mode0 == 3) color = vec4(color.rgb + texel.rgb, color.a * texel.a);
        else if (u_tex_mode0 == 4) color = vec4(mix(color.rgb, texel.rgb, texel.a), color.a);
        else if (u_tex_mode0 == 5) color = vec4(mix(color.rgb, u_env_color0.rgb, texel.rgb), color.a * texel.a);
        else if (u_tex_mode0 == 0) color = combine_stage0(color, texel, v_color);
    }
    if (u_tex_enabled1 != 0) {
        vec4 texel = texture2D(u_tex1, v_tex1);
        if (u_tex_mode1 == 1) color = vec4(texel.rgb * color.rgb, texel.a * color.a);
        else if (u_tex_mode1 == 2) color = color * texel;
        else if (u_tex_mode1 == 3) color = vec4(color.rgb + texel.rgb, color.a * texel.a);
        else if (u_tex_mode1 == 4) color = vec4(mix(color.rgb, texel.rgb, texel.a), color.a);
        else if (u_tex_mode1 == 5) color = vec4(mix(color.rgb, u_env_color1.rgb, texel.rgb), color.a * texel.a);
        else if (u_tex_mode1 == 0) color = combine_stage1(color, texel, v_color);
    }
    if (u_tex_enabled2 != 0) {
        vec4 texel = texture2D(u_tex2, v_tex2);
        if (u_tex_mode2 == 1) color = vec4(texel.rgb * color.rgb, texel.a * color.a);
        else if (u_tex_mode2 == 2) color = color * texel;
        else if (u_tex_mode2 == 3) color = vec4(color.rgb + texel.rgb, color.a * texel.a);
        else if (u_tex_mode2 == 4) color = vec4(mix(color.rgb, texel.rgb, texel.a), color.a);
        else if (u_tex_mode2 == 5) color = vec4(mix(color.rgb, u_env_color2.rgb, texel.rgb), color.a * texel.a);
        else if (u_tex_mode2 == 0) color = combine_stage2(color, texel, v_color);
    }
    if (u_tex_enabled3 != 0) {
        vec4 texel = texture2D(u_tex3, v_tex3);
        if (u_tex_mode3 == 1) color = vec4(texel.rgb * color.rgb, texel.a * color.a);
        else if (u_tex_mode3 == 2) color = color * texel;
        else if (u_tex_mode3 == 3) color = vec4(color.rgb + texel.rgb, color.a * texel.a);
        else if (u_tex_mode3 == 4) color = vec4(mix(color.rgb, texel.rgb, texel.a), color.a);
        else if (u_tex_mode3 == 5) color = vec4(mix(color.rgb, u_env_color3.rgb, texel.rgb), color.a * texel.a);
        else if (u_tex_mode3 == 0) color = combine_stage3(color, texel, v_color);
    }
    if (u_alpha_test_enabled != 0 && !alpha_pass(color.a)) discard;
    if (u_clip_enabled[0] != 0 && v_clip_distances0.x < 0.0) discard;
    if (u_clip_enabled[1] != 0 && v_clip_distances0.y < 0.0) discard;
    if (u_clip_enabled[2] != 0 && v_clip_distances0.z < 0.0) discard;
    if (u_clip_enabled[3] != 0 && v_clip_distances0.w < 0.0) discard;
    if (u_clip_enabled[4] != 0 && v_clip_distances1.x < 0.0) discard;
    if (u_clip_enabled[5] != 0 && v_clip_distances1.y < 0.0) discard;
    if (u_fog_enabled != 0) {
        float factor = clamp(fog_factor(), 0.0, 1.0);
        color = mix(u_fog_color, color, factor);
    }
    if (u_logic_op_enabled != 0) {
        vec4 rounded = floor(color * 255.0 + 0.5) / 255.0;
        if (u_logic_op == 538) color = vec4(1.0) - rounded;
        else if (u_logic_op == 539) color = rounded;
    }
    gl_FragColor = color;
}
"#,
        gles3,
    )?;
    unsafe {
        let program = gl::CreateProgram();
        gl::AttachShader(program, vertex);
        gl::AttachShader(program, fragment);
        gl::BindAttribLocation(
            program,
            ATTR_POSITION,
            b"a_position\0".as_ptr() as *const GLchar,
        );
        gl::BindAttribLocation(program, ATTR_COLOR, b"a_color\0".as_ptr() as *const GLchar);
        gl::BindAttribLocation(
            program,
            ATTR_NORMAL,
            b"a_normal\0".as_ptr() as *const GLchar,
        );
        gl::BindAttribLocation(program, ATTR_TEX0, b"a_tex0\0".as_ptr() as *const GLchar);
        gl::BindAttribLocation(program, ATTR_TEX3, b"a_tex3\0".as_ptr() as *const GLchar);
        gl::BindAttribLocation(program, ATTR_TEX2, b"a_tex2\0".as_ptr() as *const GLchar);
        gl::BindAttribLocation(program, ATTR_TEX1, b"a_tex1\0".as_ptr() as *const GLchar);
        gl::BindAttribLocation(
            program,
            ATTR_MATRIX_INDEX,
            b"a_matrix_index\0".as_ptr() as *const GLchar,
        );
        gl::BindAttribLocation(
            program,
            ATTR_WEIGHT,
            b"a_weight\0".as_ptr() as *const GLchar,
        );
        gl::BindAttribLocation(
            program,
            ATTR_POINT_SIZE,
            b"a_point_size\0".as_ptr() as *const GLchar,
        );
        gl::LinkProgram(program);
        let mut ok = 0;
        gl::GetProgramiv(program, gl::LINK_STATUS, &mut ok);
        if ok == 0 {
            let mut log = [0i8; 2048];
            let mut len = 0;
            gl::GetProgramInfoLog(
                program,
                log.len() as GLsizei,
                &mut len,
                log.as_mut_ptr() as _,
            );
            gl::DeleteShader(vertex);
            gl::DeleteShader(fragment);
            return Err(format!(
                "GLES1-on-GLES2 program link failed: {}",
                String::from_utf8_lossy(std::slice::from_raw_parts(
                    log.as_ptr() as *const u8,
                    len.max(0) as usize
                ))
            ));
        }
        gl::DeleteShader(vertex);
        gl::DeleteShader(fragment);
        Ok(program)
    }
}

impl GLES for GLES1OnGLES2<'_> {
    fn is_es2(&self) -> bool {
        true
    }
    fn is_translator(&self) -> bool {
        true
    }

    unsafe fn driver_description(&self) -> String {
        let version = CStr::from_ptr(gl::GetString(gl::VERSION) as *const _);
        let vendor = CStr::from_ptr(gl::GetString(gl::VENDOR) as *const _);
        let renderer = CStr::from_ptr(gl::GetString(gl::RENDERER) as *const _);
        crate::gles::trace_translator_event(format!(
            "host version={} vendor={} renderer={}",
            version.to_string_lossy(),
            vendor.to_string_lossy(),
            renderer.to_string_lossy()
        ));
        format!(
            "GLES1 translated by GLES2 / {} / {} / {}",
            version.to_string_lossy(),
            vendor.to_string_lossy(),
            renderer.to_string_lossy()
        )
    }

    unsafe fn CreateShader(&mut self, type_: GLenum) -> GLuint {
        gl::CreateShader(type_)
    }
    unsafe fn DeleteShader(&mut self, shader: GLuint) {
        gl::DeleteShader(shader);
    }
    unsafe fn ShaderSource(
        &mut self,
        shader: GLuint,
        count: GLsizei,
        string: *const *const GLchar,
        length: *const GLint,
    ) {
        gl::ShaderSource(shader, count, string, length);
    }
    unsafe fn CompileShader(&mut self, shader: GLuint) {
        gl::CompileShader(shader);
    }
    unsafe fn GetShaderiv(&mut self, shader: GLuint, pname: GLenum, params: *mut GLint) {
        gl::GetShaderiv(shader, pname, params);
    }
    unsafe fn GetShaderInfoLog(
        &mut self,
        shader: GLuint,
        max_length: GLsizei,
        length: *mut GLsizei,
        info_log: *mut GLchar,
    ) {
        gl::GetShaderInfoLog(shader, max_length, length, info_log);
    }
    unsafe fn IsShader(&mut self, shader: GLuint) -> GLboolean {
        gl::IsShader(shader)
    }
    unsafe fn CreateProgram(&mut self) -> GLuint {
        gl::CreateProgram()
    }
    unsafe fn DeleteProgram(&mut self, program: GLuint) {
        gl::DeleteProgram(program);
    }
    unsafe fn AttachShader(&mut self, program: GLuint, shader: GLuint) {
        gl::AttachShader(program, shader);
    }
    unsafe fn DetachShader(&mut self, program: GLuint, shader: GLuint) {
        gl::DetachShader(program, shader);
    }
    unsafe fn LinkProgram(&mut self, program: GLuint) {
        gl::LinkProgram(program);
    }
    unsafe fn UseProgram(&mut self, program: GLuint) {
        gl::UseProgram(program);
    }
    unsafe fn GetProgramiv(&mut self, program: GLuint, pname: GLenum, params: *mut GLint) {
        gl::GetProgramiv(program, pname, params);
    }
    unsafe fn GetProgramInfoLog(
        &mut self,
        program: GLuint,
        max_length: GLsizei,
        length: *mut GLsizei,
        info_log: *mut GLchar,
    ) {
        gl::GetProgramInfoLog(program, max_length, length, info_log);
    }
    unsafe fn IsProgram(&mut self, program: GLuint) -> GLboolean {
        gl::IsProgram(program)
    }
    unsafe fn ValidateProgram(&mut self, program: GLuint) {
        gl::ValidateProgram(program);
    }
    unsafe fn BindAttribLocation(&mut self, program: GLuint, index: GLuint, name: *const GLchar) {
        gl::BindAttribLocation(program, index, name);
    }
    unsafe fn GetAttribLocation(&mut self, program: GLuint, name: *const GLchar) -> GLint {
        gl::GetAttribLocation(program, name)
    }
    unsafe fn GetUniformLocation(&mut self, program: GLuint, name: *const GLchar) -> GLint {
        gl::GetUniformLocation(program, name)
    }
    unsafe fn GetActiveAttrib(
        &mut self,
        program: GLuint,
        index: GLuint,
        buf_size: GLsizei,
        length: *mut GLsizei,
        size: *mut GLint,
        type_: *mut GLenum,
        name: *mut GLchar,
    ) {
        gl::GetActiveAttrib(program, index, buf_size, length, size, type_, name);
    }
    unsafe fn GetActiveUniform(
        &mut self,
        program: GLuint,
        index: GLuint,
        buf_size: GLsizei,
        length: *mut GLsizei,
        size: *mut GLint,
        type_: *mut GLenum,
        name: *mut GLchar,
    ) {
        gl::GetActiveUniform(program, index, buf_size, length, size, type_, name);
    }
    unsafe fn EnableVertexAttribArray(&mut self, index: GLuint) {
        gl::EnableVertexAttribArray(index);
    }
    unsafe fn DisableVertexAttribArray(&mut self, index: GLuint) {
        gl::DisableVertexAttribArray(index);
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
        gl::VertexAttribPointer(index, size, type_, normalized, stride, pointer);
    }
    unsafe fn VertexAttrib1f(&mut self, index: GLuint, x: GLfloat) {
        gl::VertexAttrib1f(index, x);
    }
    unsafe fn VertexAttrib2f(&mut self, index: GLuint, x: GLfloat, y: GLfloat) {
        gl::VertexAttrib2f(index, x, y);
    }
    unsafe fn VertexAttrib3f(&mut self, index: GLuint, x: GLfloat, y: GLfloat, z: GLfloat) {
        gl::VertexAttrib3f(index, x, y, z);
    }
    unsafe fn VertexAttrib4f(
        &mut self,
        index: GLuint,
        x: GLfloat,
        y: GLfloat,
        z: GLfloat,
        w: GLfloat,
    ) {
        gl::VertexAttrib4f(index, x, y, z, w);
    }
    unsafe fn VertexAttrib1fv(&mut self, index: GLuint, v: *const GLfloat) {
        gl::VertexAttrib1fv(index, v);
    }
    unsafe fn VertexAttrib2fv(&mut self, index: GLuint, v: *const GLfloat) {
        gl::VertexAttrib2fv(index, v);
    }
    unsafe fn VertexAttrib3fv(&mut self, index: GLuint, v: *const GLfloat) {
        gl::VertexAttrib3fv(index, v);
    }
    unsafe fn VertexAttrib4fv(&mut self, index: GLuint, v: *const GLfloat) {
        gl::VertexAttrib4fv(index, v);
    }
    unsafe fn Uniform1f(&mut self, location: GLint, v0: GLfloat) {
        gl::Uniform1f(location, v0);
    }
    unsafe fn Uniform2f(&mut self, location: GLint, v0: GLfloat, v1: GLfloat) {
        gl::Uniform2f(location, v0, v1);
    }
    unsafe fn Uniform3f(&mut self, location: GLint, v0: GLfloat, v1: GLfloat, v2: GLfloat) {
        gl::Uniform3f(location, v0, v1, v2);
    }
    unsafe fn Uniform4f(
        &mut self,
        location: GLint,
        v0: GLfloat,
        v1: GLfloat,
        v2: GLfloat,
        v3: GLfloat,
    ) {
        gl::Uniform4f(location, v0, v1, v2, v3);
    }
    unsafe fn Uniform1i(&mut self, location: GLint, v0: GLint) {
        gl::Uniform1i(location, v0);
    }
    unsafe fn Uniform2i(&mut self, location: GLint, v0: GLint, v1: GLint) {
        gl::Uniform2i(location, v0, v1);
    }
    unsafe fn Uniform3i(&mut self, location: GLint, v0: GLint, v1: GLint, v2: GLint) {
        gl::Uniform3i(location, v0, v1, v2);
    }
    unsafe fn Uniform4i(&mut self, location: GLint, v0: GLint, v1: GLint, v2: GLint, v3: GLint) {
        gl::Uniform4i(location, v0, v1, v2, v3);
    }
    unsafe fn Uniform1fv(&mut self, location: GLint, count: GLsizei, value: *const GLfloat) {
        gl::Uniform1fv(location, count, value);
    }
    unsafe fn Uniform2fv(&mut self, location: GLint, count: GLsizei, value: *const GLfloat) {
        gl::Uniform2fv(location, count, value);
    }
    unsafe fn Uniform3fv(&mut self, location: GLint, count: GLsizei, value: *const GLfloat) {
        gl::Uniform3fv(location, count, value);
    }
    unsafe fn Uniform4fv(&mut self, location: GLint, count: GLsizei, value: *const GLfloat) {
        gl::Uniform4fv(location, count, value);
    }
    unsafe fn Uniform1iv(&mut self, location: GLint, count: GLsizei, value: *const GLint) {
        gl::Uniform1iv(location, count, value);
    }
    unsafe fn Uniform2iv(&mut self, location: GLint, count: GLsizei, value: *const GLint) {
        gl::Uniform2iv(location, count, value);
    }
    unsafe fn Uniform3iv(&mut self, location: GLint, count: GLsizei, value: *const GLint) {
        gl::Uniform3iv(location, count, value);
    }
    unsafe fn Uniform4iv(&mut self, location: GLint, count: GLsizei, value: *const GLint) {
        gl::Uniform4iv(location, count, value);
    }
    unsafe fn UniformMatrix2fv(
        &mut self,
        location: GLint,
        count: GLsizei,
        transpose: GLboolean,
        value: *const GLfloat,
    ) {
        gl::UniformMatrix2fv(location, count, transpose, value);
    }
    unsafe fn UniformMatrix3fv(
        &mut self,
        location: GLint,
        count: GLsizei,
        transpose: GLboolean,
        value: *const GLfloat,
    ) {
        gl::UniformMatrix3fv(location, count, transpose, value);
    }
    unsafe fn UniformMatrix4fv(
        &mut self,
        location: GLint,
        count: GLsizei,
        transpose: GLboolean,
        value: *const GLfloat,
    ) {
        gl::UniformMatrix4fv(location, count, transpose, value);
    }
    unsafe fn GetShaderSource(
        &mut self,
        shader: GLuint,
        buf_size: GLsizei,
        length: *mut GLsizei,
        source: *mut GLchar,
    ) {
        gl::GetShaderSource(shader, buf_size, length, source);
    }
    unsafe fn GetAttachedShaders(
        &mut self,
        program: GLuint,
        max_count: GLsizei,
        count: *mut GLsizei,
        shaders: *mut GLuint,
    ) {
        gl::GetAttachedShaders(program, max_count, count, shaders);
    }
    unsafe fn GetUniformiv(&mut self, program: GLuint, location: GLint, params: *mut GLint) {
        gl::GetUniformiv(program, location, params);
    }
    unsafe fn GetUniformfv(&mut self, program: GLuint, location: GLint, params: *mut GLfloat) {
        gl::GetUniformfv(program, location, params);
    }
    unsafe fn GetShaderPrecisionFormat(
        &mut self,
        shader_type: GLenum,
        precision_type: GLenum,
        range: *mut GLint,
        precision: *mut GLint,
    ) {
        gl::GetShaderPrecisionFormat(shader_type, precision_type, range, precision);
    }
    unsafe fn ReleaseShaderCompiler(&mut self) {
        gl::ReleaseShaderCompiler();
    }
    unsafe fn ShaderBinary(
        &mut self,
        count: GLsizei,
        shaders: *const GLuint,
        binary_format: GLenum,
        binary: *const GLvoid,
        length: GLsizei,
    ) {
        gl::ShaderBinary(count, shaders, binary_format, binary, length);
    }

    unsafe fn GetError(&mut self) -> GLenum {
        gl::GetError()
    }
    unsafe fn GetString(&mut self, name: GLenum) -> *const GLubyte {
        gl::GetString(name)
    }
    unsafe fn GetBooleanv(&mut self, pname: GLenum, params: *mut GLboolean) {
        match pname {
            es1::TEXTURE_2D => {
                *params = if self.state.texture_enabled[self.state.active_texture] {
                    gl::TRUE
                } else {
                    gl::FALSE
                }
            }
            es1::ALPHA_TEST => {
                *params = if self.state.alpha_test_enabled {
                    gl::TRUE
                } else {
                    gl::FALSE
                }
            }
            es1::FOG => {
                *params = if self.state.fog_enabled {
                    gl::TRUE
                } else {
                    gl::FALSE
                }
            }
            es1::LIGHTING => {
                *params = if self.state.lighting_enabled {
                    gl::TRUE
                } else {
                    gl::FALSE
                }
            }
            es1::LIGHT0..=es1::LIGHT7 => {
                let index = (pname - es1::LIGHT0) as usize;
                *params = if self.state.light_enabled[index] {
                    gl::TRUE
                } else {
                    gl::FALSE
                };
            }
            es1::COLOR_MATERIAL => {
                *params = if self.state.color_material_enabled {
                    gl::TRUE
                } else {
                    gl::FALSE
                }
            }
            es1::NORMALIZE => {
                *params = if self.state.normalize_enabled {
                    gl::TRUE
                } else {
                    gl::FALSE
                }
            }
            es1::CLIP_PLANE0..=es1::CLIP_PLANE5 => {
                let index = (pname - es1::CLIP_PLANE0) as usize;
                *params = if self.state.clip_plane_enabled[index] {
                    gl::TRUE
                } else {
                    gl::FALSE
                };
            }
            _ => gl::GetBooleanv(pname, params),
        }
    }
    unsafe fn GetFloatv(&mut self, pname: GLenum, params: *mut GLfloat) {
        match pname {
            es1::MODELVIEW_MATRIX => params.copy_from(self.state.modelview.current.as_ptr(), 16),
            es1::PROJECTION_MATRIX => params.copy_from(self.state.projection.current.as_ptr(), 16),
            es1::TEXTURE_MATRIX => params.copy_from(
                self.state.texture[self.state.active_texture]
                    .current
                    .as_ptr(),
                16,
            ),
            es1::CURRENT_COLOR => params.copy_from(self.state.color.as_ptr(), 4),
            es1::CURRENT_NORMAL => params.copy_from(self.state.normal.as_ptr(), 3),
            es1::FOG_COLOR => params.copy_from(self.state.fog_color.as_ptr(), 4),
            es1::FOG_DENSITY => *params = self.state.fog_density,
            es1::FOG_START => *params = self.state.fog_start,
            es1::FOG_END => *params = self.state.fog_end,
            es1::POINT_SIZE => *params = self.state.point_size,
            es1::CURRENT_TEXTURE_COORDS => {
                params.copy_from(self.state.texcoords[self.state.active_texture].as_ptr(), 4)
            }
            es1::POINT_DISTANCE_ATTENUATION => {
                params.copy_from(self.state.point_distance_attenuation.as_ptr(), 3)
            }
            es1::POINT_FADE_THRESHOLD_SIZE => *params = self.state.point_fade_threshold,
            es1::CLIP_PLANE0..=es1::CLIP_PLANE5 => {
                let index = (pname - es1::CLIP_PLANE0) as usize;
                params.copy_from(self.state.clip_planes[index].as_ptr(), 4);
            }
            _ => gl::GetFloatv(pname, params),
        }
    }
    unsafe fn GetTexEnviv(&mut self, target: GLenum, pname: GLenum, params: *mut GLint) {
        if target != es1::TEXTURE_ENV || params.is_null() {
            return;
        }
        if pname == es1::TEXTURE_ENV_MODE {
            *params = self.state.texture_env_mode[self.state.active_texture];
        } else if pname == es1::TEXTURE_ENV_COLOR {
            for (index, value) in self.state.texture_env_color[self.state.active_texture]
                .iter()
                .enumerate()
            {
                *params.add(index) = *value as GLint;
            }
        }
    }
    unsafe fn GetTexEnvfv(&mut self, target: GLenum, pname: GLenum, params: *mut GLfloat) {
        if target != es1::TEXTURE_ENV || params.is_null() {
            return;
        }
        if pname == es1::TEXTURE_ENV_MODE {
            *params = self.state.texture_env_mode[self.state.active_texture] as GLfloat;
        } else if pname == es1::TEXTURE_ENV_COLOR {
            params.copy_from(
                self.state.texture_env_color[self.state.active_texture].as_ptr(),
                4,
            );
        }
    }
    unsafe fn GetTexEnvxv(&mut self, target: GLenum, pname: GLenum, params: *mut GLfixed) {
        if params.is_null() {
            return;
        }
        let mut values = [0.0; 4];
        self.GetTexEnvfv(target, pname, values.as_mut_ptr());
        for (index, value) in values.iter().enumerate() {
            *params.add(index) = float_to_fixed(*value);
        }
    }
    unsafe fn GetLightfv(&mut self, light: GLenum, pname: GLenum, params: *mut GLfloat) {
        let Some(index) = Self::light_index(light) else {
            return;
        };
        if params.is_null() {
            return;
        }
        let light = &self.state.lights[index];
        match pname {
            es1::AMBIENT => params.copy_from(light.ambient.as_ptr(), 4),
            es1::DIFFUSE => params.copy_from(light.diffuse.as_ptr(), 4),
            es1::SPECULAR => params.copy_from(light.specular.as_ptr(), 4),
            es1::POSITION => params.copy_from(light.position.as_ptr(), 4),
            es1::SPOT_DIRECTION => params.copy_from(light.spot_direction.as_ptr(), 3),
            es1::SPOT_CUTOFF => *params = light.spot_cutoff,
            es1::SPOT_EXPONENT => *params = light.spot_exponent,
            es1::CONSTANT_ATTENUATION => *params = light.constant_attenuation,
            es1::LINEAR_ATTENUATION => *params = light.linear_attenuation,
            es1::QUADRATIC_ATTENUATION => *params = light.quadratic_attenuation,
            _ => {}
        }
    }
    unsafe fn GetLightxv(&mut self, light: GLenum, pname: GLenum, params: *mut GLfixed) {
        if params.is_null() {
            return;
        }
        let count = if pname == es1::SPOT_DIRECTION {
            3
        } else if matches!(
            pname,
            es1::AMBIENT | es1::DIFFUSE | es1::SPECULAR | es1::POSITION
        ) {
            4
        } else {
            1
        };
        let mut values = [0.0; 4];
        self.GetLightfv(light, pname, values.as_mut_ptr());
        for index in 0..count {
            *params.add(index) = float_to_fixed(values[index]);
        }
    }
    unsafe fn GetMaterialfv(&mut self, face: GLenum, pname: GLenum, params: *mut GLfloat) {
        if face != es1::FRONT_AND_BACK || params.is_null() {
            return;
        }
        match pname {
            es1::AMBIENT => params.copy_from(self.state.material_ambient.as_ptr(), 4),
            es1::DIFFUSE => params.copy_from(self.state.material_diffuse.as_ptr(), 4),
            es1::SPECULAR => params.copy_from(self.state.material_specular.as_ptr(), 4),
            es1::SHININESS => *params = self.state.material_shininess,
            _ => {}
        }
    }
    unsafe fn GetMaterialxv(&mut self, face: GLenum, pname: GLenum, params: *mut GLfixed) {
        if params.is_null() {
            return;
        }
        let mut values = [0.0; 4];
        self.GetMaterialfv(face, pname, values.as_mut_ptr());
        let count = if pname == es1::SHININESS { 1 } else { 4 };
        for i in 0..count {
            *params.add(i) = float_to_fixed(values[i]);
        }
    }
    unsafe fn GetTexParameteriv(&mut self, target: GLenum, pname: GLenum, params: *mut GLint) {
        gl::GetTexParameteriv(target, pname, params);
    }
    unsafe fn GetTexParameterfv(&mut self, target: GLenum, pname: GLenum, params: *mut GLfloat) {
        gl::GetTexParameterfv(target, pname, params);
    }
    unsafe fn GetTexParameterxv(&mut self, target: GLenum, pname: GLenum, params: *mut GLfixed) {
        if params.is_null() {
            return;
        }
        let is_enum = pname == es1::TEXTURE_MIN_FILTER
            || pname == es1::TEXTURE_MAG_FILTER
            || pname == es1::TEXTURE_WRAP_S
            || pname == es1::TEXTURE_WRAP_T;
        if is_enum {
            let mut value = 0;
            gl::GetTexParameteriv(target, pname, &mut value);
            *params = value as GLfixed;
        } else {
            let mut value = 0.0;
            gl::GetTexParameterfv(target, pname, &mut value);
            *params = float_to_fixed(value);
        }
    }
    unsafe fn Enable(&mut self, cap: GLenum) {
        if cap == es1::COLOR_LOGIC_OP {
            self.state.logic_op_enabled = true;
        } else if cap == es1::TEXTURE_2D {
            self.state.texture_enabled[self.state.active_texture] = true;
        } else if cap == es1::ALPHA_TEST {
            self.state.alpha_test_enabled = true;
        } else if cap == es1::FOG {
            self.state.fog_enabled = true;
        } else if cap == es1::LIGHTING {
            self.state.lighting_enabled = true;
        } else if let Some(index) = Self::light_index(cap) {
            self.state.light_enabled[index] = true;
        } else if cap == es1::COLOR_MATERIAL {
            self.state.color_material_enabled = true;
        } else if cap == es1::NORMALIZE {
            self.state.normalize_enabled = true;
        } else if cap == es1::MATRIX_PALETTE_OES {
            self.state.matrix_palette_enabled = true;
        } else if (es1::CLIP_PLANE0..=es1::CLIP_PLANE5).contains(&cap) {
            self.state.clip_plane_enabled[(cap - es1::CLIP_PLANE0) as usize] = true;
        } else {
            gl::Enable(cap);
        }
    }
    unsafe fn Disable(&mut self, cap: GLenum) {
        if cap == es1::COLOR_LOGIC_OP {
            self.state.logic_op_enabled = false;
        } else if cap == es1::TEXTURE_2D {
            self.state.texture_enabled[self.state.active_texture] = false;
        } else if cap == es1::ALPHA_TEST {
            self.state.alpha_test_enabled = false;
        } else if cap == es1::FOG {
            self.state.fog_enabled = false;
        } else if cap == es1::LIGHTING {
            self.state.lighting_enabled = false;
        } else if let Some(index) = Self::light_index(cap) {
            self.state.light_enabled[index] = false;
        } else if cap == es1::COLOR_MATERIAL {
            self.state.color_material_enabled = false;
        } else if cap == es1::NORMALIZE {
            self.state.normalize_enabled = false;
        } else if cap == es1::MATRIX_PALETTE_OES {
            self.state.matrix_palette_enabled = false;
        } else if (es1::CLIP_PLANE0..=es1::CLIP_PLANE5).contains(&cap) {
            self.state.clip_plane_enabled[(cap - es1::CLIP_PLANE0) as usize] = false;
        } else {
            gl::Disable(cap);
        }
    }
    unsafe fn IsEnabled(&mut self, cap: GLenum) -> GLboolean {
        if cap == es1::COLOR_LOGIC_OP {
            return if self.state.logic_op_enabled {
                gl::TRUE
            } else {
                gl::FALSE
            };
        }
        if cap == es1::TEXTURE_2D {
            return if self.state.texture_enabled[self.state.active_texture] {
                gl::TRUE
            } else {
                gl::FALSE
            };
        }
        if cap == es1::ALPHA_TEST {
            return if self.state.alpha_test_enabled {
                gl::TRUE
            } else {
                gl::FALSE
            };
        }
        if cap == es1::FOG {
            return if self.state.fog_enabled {
                gl::TRUE
            } else {
                gl::FALSE
            };
        }
        if cap == es1::LIGHTING {
            return if self.state.lighting_enabled {
                gl::TRUE
            } else {
                gl::FALSE
            };
        }
        if let Some(index) = Self::light_index(cap) {
            return if self.state.light_enabled[index] {
                gl::TRUE
            } else {
                gl::FALSE
            };
        }
        if cap == es1::COLOR_MATERIAL {
            return if self.state.color_material_enabled {
                gl::TRUE
            } else {
                gl::FALSE
            };
        }
        if cap == es1::NORMALIZE {
            return if self.state.normalize_enabled {
                gl::TRUE
            } else {
                gl::FALSE
            };
        }
        if cap == es1::MATRIX_PALETTE_OES {
            return if self.state.matrix_palette_enabled {
                gl::TRUE
            } else {
                gl::FALSE
            };
        }
        if (es1::CLIP_PLANE0..=es1::CLIP_PLANE5).contains(&cap) {
            return if self.state.clip_plane_enabled[(cap - es1::CLIP_PLANE0) as usize] {
                gl::TRUE
            } else {
                gl::FALSE
            };
        }
        gl::IsEnabled(cap)
    }
    unsafe fn ClientActiveTexture(&mut self, texture: GLenum) {
        self.state.client_active_texture = texture
            .saturating_sub(es1::TEXTURE0)
            .min((MAX_TEXTURE_UNITS - 1) as GLenum)
            as usize;
    }
    unsafe fn ActiveTexture(&mut self, texture: GLenum) {
        self.state.active_texture = texture
            .saturating_sub(es1::TEXTURE0)
            .min((MAX_TEXTURE_UNITS - 1) as GLenum) as usize;
        gl::ActiveTexture(gl::TEXTURE0 + self.state.active_texture as GLenum);
    }
    unsafe fn EnableClientState(&mut self, array: GLenum) {
        match array {
            es1::VERTEX_ARRAY => self.state.arrays[0].enabled = true,
            es1::COLOR_ARRAY => self.state.arrays[1].enabled = true,
            es1::NORMAL_ARRAY => self.state.arrays[2].enabled = true,
            es1::TEXTURE_COORD_ARRAY => {
                self.state.texcoord_arrays[self.state.client_active_texture].enabled = true
            }
            es1::MATRIX_INDEX_ARRAY_OES => self.state.palette_index_array.enabled = true,
            es1::WEIGHT_ARRAY_OES => self.state.palette_weight_array.enabled = true,
            es1::POINT_SIZE_ARRAY_OES => self.state.point_size_array.enabled = true,
            _ => {}
        }
    }
    unsafe fn DisableClientState(&mut self, array: GLenum) {
        match array {
            es1::VERTEX_ARRAY => self.state.arrays[0].enabled = false,
            es1::COLOR_ARRAY => self.state.arrays[1].enabled = false,
            es1::NORMAL_ARRAY => self.state.arrays[2].enabled = false,
            es1::TEXTURE_COORD_ARRAY => {
                self.state.texcoord_arrays[self.state.client_active_texture].enabled = false
            }
            es1::MATRIX_INDEX_ARRAY_OES => self.state.palette_index_array.enabled = false,
            es1::WEIGHT_ARRAY_OES => self.state.palette_weight_array.enabled = false,
            es1::POINT_SIZE_ARRAY_OES => self.state.point_size_array.enabled = false,
            _ => {}
        }
    }
    unsafe fn GetFixedv(&mut self, pname: GLenum, params: *mut GLfixed) {
        if params.is_null() {
            return;
        }
        let count = if matches!(
            pname,
            es1::MODELVIEW_MATRIX | es1::PROJECTION_MATRIX | es1::TEXTURE_MATRIX
        ) {
            16
        } else if pname == es1::CURRENT_NORMAL {
            3
        } else {
            4
        };
        let mut values = [0.0; 16];
        self.GetFloatv(pname, values.as_mut_ptr());
        for i in 0..count {
            *params.add(i) = float_to_fixed(values[i]);
        }
    }
    unsafe fn GetPointerv(&mut self, pname: GLenum, params: *mut *const GLvoid) {
        if params.is_null() {
            return;
        }
        *params = match pname {
            es1::VERTEX_ARRAY_POINTER => self.state.arrays[0].pointer,
            es1::COLOR_ARRAY_POINTER => self.state.arrays[1].pointer,
            es1::NORMAL_ARRAY_POINTER => self.state.arrays[2].pointer,
            es1::TEXTURE_COORD_ARRAY_POINTER => {
                self.state.texcoord_arrays[self.state.client_active_texture].pointer
            }
            es1::POINT_SIZE_ARRAY_POINTER_OES => self.state.point_size_array.pointer,
            es1::MATRIX_INDEX_ARRAY_POINTER_OES => self.state.palette_index_array.pointer,
            es1::WEIGHT_ARRAY_POINTER_OES => self.state.palette_weight_array.pointer,
            _ => std::ptr::null(),
        };
    }
    unsafe fn GetVertexAttribiv(&mut self, index: GLuint, pname: GLenum, params: *mut GLint) {
        gl::GetVertexAttribiv(index, pname, params);
        // Bounds probing on invalid attrib indices sets GL_INVALID_OPERATION
        // and pollutes the shared error queue (shows up later as spurious
        // 0x500 traces from unrelated calls). Swallow probe errors here; the
        // guard treats a failed query conservatively via the params value.
        let _ = gl::GetError();
    }
    unsafe fn GetVertexAttribfv(&mut self, index: GLuint, pname: GLenum, params: *mut GLfloat) {
        gl::GetVertexAttribfv(index, pname, params);
    }
    unsafe fn GetVertexAttribPointerv(
        &mut self,
        index: GLuint,
        pname: GLenum,
        pointer: *mut *mut GLvoid,
    ) {
        gl::GetVertexAttribPointerv(index, pname, pointer);
    }
    unsafe fn Hint(&mut self, target: GLenum, mode: GLenum) {
        let slot = match target {
            es1::PERSPECTIVE_CORRECTION_HINT => 0,
            es1::POINT_SMOOTH_HINT => 1,
            es1::LINE_SMOOTH_HINT => 2,
            es1::FOG_HINT => 3,
            _ => return,
        };
        self.state.hints[slot] = mode;
    }
    unsafe fn ClipPlanef(&mut self, plane: GLenum, equation: *const GLfloat) {
        if equation.is_null() || !(es1::CLIP_PLANE0..=es1::CLIP_PLANE5).contains(&plane) {
            return;
        }
        self.state.clip_planes[(plane - es1::CLIP_PLANE0) as usize] =
            std::slice::from_raw_parts(equation, 4).try_into().unwrap();
    }
    unsafe fn ClipPlanex(&mut self, plane: GLenum, equation: *const GLfixed) {
        if equation.is_null() || !(es1::CLIP_PLANE0..=es1::CLIP_PLANE5).contains(&plane) {
            return;
        }
        self.state.clip_planes[(plane - es1::CLIP_PLANE0) as usize] =
            std::slice::from_raw_parts(equation, 4)
                .iter()
                .map(|v| fixed_to_float(*v))
                .collect::<Vec<_>>()
                .try_into()
                .unwrap();
    }
    unsafe fn ClearDepthx(&mut self, depth: GLclampx) {
        self.ClearDepthf(fixed_to_float(depth));
    }
    unsafe fn LineWidthx(&mut self, width: GLfixed) {
        self.LineWidth(fixed_to_float(width));
    }
    unsafe fn StencilFunc(&mut self, func: GLenum, ref_: GLint, mask: GLuint) {
        gl::StencilFunc(func, ref_, mask);
    }
    unsafe fn StencilOp(&mut self, sfail: GLenum, dpfail: GLenum, dppass: GLenum) {
        gl::StencilOp(sfail, dpfail, dppass);
    }
    unsafe fn StencilMask(&mut self, mask: GLuint) {
        gl::StencilMask(mask);
    }
    unsafe fn PointParameterf(&mut self, pname: GLenum, param: GLfloat) {
        match pname {
            es1::POINT_SIZE_MIN | es1::POINT_SIZE_MAX => {}
            es1::POINT_FADE_THRESHOLD_SIZE => self.state.point_fade_threshold = param,
            _ => {}
        }
    }
    unsafe fn PointParameterx(&mut self, pname: GLenum, param: GLfixed) {
        self.PointParameterf(pname, fixed_to_float(param));
    }
    unsafe fn PointParameterfv(&mut self, pname: GLenum, params: *const GLfloat) {
        if params.is_null() {
            return;
        }
        if pname == es1::POINT_DISTANCE_ATTENUATION {
            self.state.point_distance_attenuation =
                std::slice::from_raw_parts(params, 3).try_into().unwrap();
        } else {
            self.PointParameterf(pname, *params);
        }
    }
    unsafe fn PointParameterxv(&mut self, pname: GLenum, params: *const GLfixed) {
        if params.is_null() {
            return;
        }
        if pname == es1::POINT_DISTANCE_ATTENUATION {
            self.state.point_distance_attenuation = std::slice::from_raw_parts(params, 3)
                .iter()
                .map(|v| fixed_to_float(*v))
                .collect::<Vec<_>>()
                .try_into()
                .unwrap();
        } else {
            self.PointParameterx(pname, *params);
        }
    }
    unsafe fn AlphaFunc(&mut self, func: GLenum, ref_: GLclampf) {
        self.state.alpha_func = func;
        self.state.alpha_ref = ref_;
    }
    unsafe fn AlphaFuncx(&mut self, func: GLenum, ref_: GLclampx) {
        self.AlphaFunc(func, fixed_to_float(ref_));
    }
    unsafe fn DepthRangef(&mut self, near: GLclampf, far: GLclampf) {
        gl::DepthRangef(near, far);
    }
    unsafe fn DepthRangex(&mut self, near: GLclampx, far: GLclampx) {
        self.DepthRangef(fixed_to_float(near), fixed_to_float(far));
    }
    unsafe fn PolygonOffset(&mut self, factor: GLfloat, units: GLfloat) {
        gl::PolygonOffset(factor, units);
    }
    unsafe fn PolygonOffsetx(&mut self, factor: GLfixed, units: GLfixed) {
        self.PolygonOffset(fixed_to_float(factor), fixed_to_float(units));
    }
    unsafe fn SampleCoverage(&mut self, value: GLclampf, invert: GLboolean) {
        gl::SampleCoverage(value, invert);
    }
    unsafe fn SampleCoveragex(&mut self, value: GLclampx, invert: GLboolean) {
        self.SampleCoverage(fixed_to_float(value), invert);
    }
    unsafe fn ShadeModel(&mut self, mode: GLenum) {
        if mode == es1::FLAT || mode == es1::SMOOTH {
            self.state.shade_model = mode;
        }
    }
    unsafe fn Lightf(&mut self, light: GLenum, pname: GLenum, param: GLfloat) {
        let Some(index) = Self::light_index(light) else {
            return;
        };
        let light = &mut self.state.lights[index];
        match pname {
            es1::SPOT_CUTOFF => light.spot_cutoff = param,
            es1::SPOT_EXPONENT => light.spot_exponent = param,
            es1::CONSTANT_ATTENUATION => light.constant_attenuation = param,
            es1::LINEAR_ATTENUATION => light.linear_attenuation = param,
            es1::QUADRATIC_ATTENUATION => light.quadratic_attenuation = param,
            _ => {}
        }
    }
    unsafe fn Lightx(&mut self, light: GLenum, pname: GLenum, param: GLfixed) {
        self.Lightf(light, pname, fixed_to_float(param));
    }
    unsafe fn Lightfv(&mut self, light: GLenum, pname: GLenum, params: *const GLfloat) {
        let Some(index) = Self::light_index(light) else {
            return;
        };
        if params.is_null() {
            return;
        }
        let modelview = self.state.modelview.current;
        let light = &mut self.state.lights[index];
        match pname {
            es1::AMBIENT => {
                light.ambient = std::slice::from_raw_parts(params, 4).try_into().unwrap()
            }
            es1::DIFFUSE => {
                light.diffuse = std::slice::from_raw_parts(params, 4).try_into().unwrap()
            }
            es1::SPECULAR => {
                light.specular = std::slice::from_raw_parts(params, 4).try_into().unwrap()
            }
            es1::POSITION => {
                let value: [GLfloat; 4] = std::slice::from_raw_parts(params, 4).try_into().unwrap();
                light.position = Self::transform_vec4(&modelview, value);
            }
            es1::SPOT_DIRECTION => {
                let value: [GLfloat; 3] = std::slice::from_raw_parts(params, 3).try_into().unwrap();
                let transformed =
                    Self::transform_vec4(&modelview, [value[0], value[1], value[2], 0.0]);
                let length = (transformed[0] * transformed[0]
                    + transformed[1] * transformed[1]
                    + transformed[2] * transformed[2])
                    .sqrt()
                    .max(0.000001);
                light.spot_direction = [
                    transformed[0] / length,
                    transformed[1] / length,
                    transformed[2] / length,
                ];
            }
            _ => {}
        }
    }
    unsafe fn Lightxv(&mut self, light: GLenum, pname: GLenum, params: *const GLfixed) {
        if params.is_null() {
            return;
        }
        let count = if pname == es1::SPOT_DIRECTION { 3 } else { 4 };
        let values: Vec<GLfloat> = std::slice::from_raw_parts(params, count)
            .iter()
            .map(|value| fixed_to_float(*value))
            .collect();
        self.Lightfv(light, pname, values.as_ptr());
    }
    unsafe fn LightModelf(&mut self, pname: GLenum, param: GLfloat) {
        match pname {
            0x0B51 => self.state.light_model_local_viewer = param != 0.0,
            es1::LIGHT_MODEL_TWO_SIDE => self.state.light_model_two_side = param != 0.0,
            _ => {}
        }
    }
    unsafe fn LightModelx(&mut self, pname: GLenum, param: GLfixed) {
        self.LightModelf(pname, fixed_to_float(param));
    }
    unsafe fn LightModelfv(&mut self, pname: GLenum, params: *const GLfloat) {
        if params.is_null() {
            return;
        }
        if pname == es1::LIGHT_MODEL_AMBIENT {
            self.state.model_ambient = std::slice::from_raw_parts(params, 4).try_into().unwrap();
        } else {
            self.LightModelf(pname, *params);
        }
    }
    unsafe fn LightModelxv(&mut self, pname: GLenum, params: *const GLfixed) {
        if params.is_null() {
            return;
        }
        if pname == es1::LIGHT_MODEL_AMBIENT {
            self.state.model_ambient = std::slice::from_raw_parts(params, 4)
                .iter()
                .map(|v| fixed_to_float(*v))
                .collect::<Vec<_>>()
                .try_into()
                .unwrap();
        } else {
            self.LightModelf(pname, fixed_to_float(*params));
        }
    }
    unsafe fn Materialf(&mut self, face: GLenum, pname: GLenum, param: GLfloat) {
        if face != es1::FRONT_AND_BACK {
            return;
        }
        if pname == es1::SHININESS {
            self.state.material_shininess = param;
        }
    }
    unsafe fn Materialx(&mut self, face: GLenum, pname: GLenum, param: GLfixed) {
        self.Materialf(face, pname, fixed_to_float(param));
    }
    unsafe fn Materialfv(&mut self, face: GLenum, pname: GLenum, params: *const GLfloat) {
        if face != es1::FRONT_AND_BACK || params.is_null() {
            return;
        }
        let values: [GLfloat; 4] = std::slice::from_raw_parts(params, 4).try_into().unwrap();
        match pname {
            es1::AMBIENT => self.state.material_ambient = values,
            es1::DIFFUSE | es1::AMBIENT_AND_DIFFUSE => self.state.material_diffuse = values,
            es1::SPECULAR => self.state.material_specular = values,
            es1::EMISSION => self.state.material_emission = values,
            _ => {}
        }
    }
    unsafe fn Materialxv(&mut self, face: GLenum, pname: GLenum, params: *const GLfixed) {
        if params.is_null() {
            return;
        }
        let values: [GLfloat; 4] = std::slice::from_raw_parts(params, 4)
            .iter()
            .map(|v| fixed_to_float(*v))
            .collect::<Vec<_>>()
            .try_into()
            .unwrap();
        self.Materialfv(face, pname, values.as_ptr());
    }
    unsafe fn Color4f(&mut self, r: GLfloat, g: GLfloat, b: GLfloat, a: GLfloat) {
        self.state.color = [r, g, b, a];
    }
    unsafe fn Color4x(&mut self, r: GLfixed, g: GLfixed, b: GLfixed, a: GLfixed) {
        self.Color4f(
            fixed_to_float(r),
            fixed_to_float(g),
            fixed_to_float(b),
            fixed_to_float(a),
        );
    }
    unsafe fn Color4ub(&mut self, r: GLubyte, g: GLubyte, b: GLubyte, a: GLubyte) {
        self.state.color = [
            r as f32 / 255.0,
            g as f32 / 255.0,
            b as f32 / 255.0,
            a as f32 / 255.0,
        ];
    }
    unsafe fn Normal3f(&mut self, x: GLfloat, y: GLfloat, z: GLfloat) {
        self.state.normal = [x, y, z];
    }
    unsafe fn Normal3x(&mut self, x: GLfixed, y: GLfixed, z: GLfixed) {
        self.Normal3f(fixed_to_float(x), fixed_to_float(y), fixed_to_float(z));
    }
    unsafe fn MultiTexCoord4f(
        &mut self,
        texture: GLenum,
        s: GLfloat,
        t: GLfloat,
        r: GLfloat,
        q: GLfloat,
    ) {
        let i = texture
            .saturating_sub(es1::TEXTURE0)
            .min((MAX_TEXTURE_UNITS - 1) as GLenum) as usize;
        self.state.texcoords[i] = [s, t, r, q];
    }
    unsafe fn MultiTexCoord4x(
        &mut self,
        texture: GLenum,
        s: GLfixed,
        t: GLfixed,
        r: GLfixed,
        q: GLfixed,
    ) {
        self.MultiTexCoord4f(
            texture,
            fixed_to_float(s),
            fixed_to_float(t),
            fixed_to_float(r),
            fixed_to_float(q),
        );
    }
    unsafe fn TexCoordPointer(
        &mut self,
        size: GLint,
        type_: GLenum,
        stride: GLsizei,
        pointer: *const GLvoid,
    ) {
        let enabled = self.state.texcoord_arrays[self.state.client_active_texture].enabled;
        let buffer_binding = self.state.array_buffer_binding;
        self.state.texcoord_arrays[self.state.client_active_texture] = ArrayState {
            size,
            type_,
            stride,
            pointer,
            buffer_binding,
            enabled,
            fixed: type_ == es1::FIXED,
            normalized: false,
        };
    }
    unsafe fn ColorPointer(
        &mut self,
        size: GLint,
        type_: GLenum,
        stride: GLsizei,
        pointer: *const GLvoid,
    ) {
        let enabled = self.state.arrays[1].enabled;
        let buffer_binding = self.state.array_buffer_binding;
        self.state.arrays[1] = ArrayState {
            size,
            type_,
            stride,
            pointer,
            buffer_binding,
            enabled,
            fixed: type_ == es1::FIXED,
            normalized: true,
        };
    }
    unsafe fn NormalPointer(&mut self, type_: GLenum, stride: GLsizei, pointer: *const GLvoid) {
        let enabled = self.state.arrays[2].enabled;
        let buffer_binding = self.state.array_buffer_binding;
        self.state.arrays[2] = ArrayState {
            size: 3,
            type_,
            stride,
            pointer,
            buffer_binding,
            enabled,
            fixed: type_ == es1::FIXED,
            normalized: false,
        };
    }
    unsafe fn VertexPointer(
        &mut self,
        size: GLint,
        type_: GLenum,
        stride: GLsizei,
        pointer: *const GLvoid,
    ) {
        let enabled = self.state.arrays[0].enabled;
        let buffer_binding = self.state.array_buffer_binding;
        self.state.arrays[0] = ArrayState {
            size,
            type_,
            stride,
            pointer,
            buffer_binding,
            enabled,
            fixed: type_ == es1::FIXED,
            normalized: false,
        };
    }
    unsafe fn BindBuffer(&mut self, target: GLenum, buffer: GLuint) {
        match target {
            gl::ARRAY_BUFFER => self.state.array_buffer_binding = buffer,
            gl::ELEMENT_ARRAY_BUFFER => self.state.element_array_buffer_binding = buffer,
            _ => {}
        }
        gl::BindBuffer(target, buffer);
    }
    unsafe fn GenBuffers(&mut self, n: GLsizei, buffers: *mut GLuint) {
        gl::GenBuffers(n, buffers);
    }
    unsafe fn IsBuffer(&mut self, buffer: GLuint) -> GLboolean {
        gl::IsBuffer(buffer)
    }
    unsafe fn DeleteBuffers(&mut self, n: GLsizei, buffers: *const GLuint) {
        if !buffers.is_null() {
            for i in 0..n.max(0) as usize {
                let buffer = buffers.add(i).read_unaligned();
                self.state.array_buffer_data.remove(&buffer);
                self.state.element_array_buffer_data.remove(&buffer);
            }
        }
        gl::DeleteBuffers(n, buffers);
    }
    unsafe fn BufferData(
        &mut self,
        target: GLenum,
        size: GLsizeiptr,
        data: *const GLvoid,
        usage: GLenum,
    ) {
        if size < 0 {
            gl::BufferData(target, size, std::ptr::null(), usage);
            return;
        }
        let size_usize = size as usize;
        let binding = match target {
            gl::ARRAY_BUFFER => self.state.array_buffer_binding,
            gl::ELEMENT_ARRAY_BUFFER => self.state.element_array_buffer_binding,
            _ => 0,
        };
        if binding != 0 {
            let store = if target == gl::ARRAY_BUFFER {
                &mut self.state.array_buffer_data
            } else {
                &mut self.state.element_array_buffer_data
            };
            let bytes = store.entry(binding).or_default();
            bytes.resize(size_usize, 0);
            if !data.is_null() && size_usize > 0 {
                std::ptr::copy_nonoverlapping(data.cast::<u8>(), bytes.as_mut_ptr(), size_usize);
            }
        }
        gl::BufferData(target, size, data, usage);
    }
    unsafe fn BufferSubData(
        &mut self,
        target: GLenum,
        offset: GLintptr,
        size: GLsizeiptr,
        data: *const GLvoid,
    ) {
        if offset < 0 || size < 0 {
            gl::BufferSubData(target, offset, size, std::ptr::null());
            return;
        }
        let binding = match target {
            gl::ARRAY_BUFFER => self.state.array_buffer_binding,
            gl::ELEMENT_ARRAY_BUFFER => self.state.element_array_buffer_binding,
            _ => 0,
        };
        if binding != 0 && !data.is_null() {
            if let Some(end) = (offset as usize).checked_add(size as usize) {
                let store = if target == gl::ARRAY_BUFFER {
                    &mut self.state.array_buffer_data
                } else {
                    &mut self.state.element_array_buffer_data
                };
                let bytes = store.entry(binding).or_default();
                if end > bytes.len() {
                    bytes.resize(end, 0);
                }
                if size > 0 {
                    std::ptr::copy_nonoverlapping(
                        data.cast::<u8>(),
                        bytes.as_mut_ptr().add(offset as usize),
                        size as usize,
                    );
                }
            }
        }
        gl::BufferSubData(target, offset, size, data);
    }
    unsafe fn BindTexture(&mut self, target: GLenum, texture: GLuint) {
        self.state.bound_textures[self.state.active_texture] = texture;
        gl::ActiveTexture(gl::TEXTURE0 + self.state.active_texture as GLenum);
        gl::BindTexture(target, texture);
    }
    unsafe fn GenTextures(&mut self, n: GLsizei, textures: *mut GLuint) {
        gl::GenTextures(n, textures);
    }
    unsafe fn DeleteTextures(&mut self, n: GLsizei, textures: *const GLuint) {
        gl::DeleteTextures(n, textures);
    }
    unsafe fn TexParameteri(&mut self, target: GLenum, pname: GLenum, param: GLint) {
        if pname == es1::GENERATE_MIPMAP {
            if param != 0 {
                gl::GenerateMipmap(target);
            }
            return;
        }
        gl::TexParameteri(target, pname, param);
    }
    unsafe fn TexParameterf(&mut self, target: GLenum, pname: GLenum, param: GLfloat) {
        if pname == es1::GENERATE_MIPMAP {
            if param != 0.0 {
                gl::GenerateMipmap(target);
            }
            return;
        }
        gl::TexParameterf(target, pname, param);
    }
    unsafe fn TexParameterx(&mut self, target: GLenum, pname: GLenum, param: GLfixed) {
        if pname == es1::GENERATE_MIPMAP {
            if param != 0 {
                gl::GenerateMipmap(target);
            }
            return;
        }
        gl::TexParameterf(target, pname, fixed_to_float(param));
    }
    unsafe fn TexParameteriv(&mut self, target: GLenum, pname: GLenum, params: *const GLint) {
        if params.is_null() {
            return;
        }
        if pname == es1::GENERATE_MIPMAP {
            if *params != 0 {
                gl::GenerateMipmap(target);
            }
            return;
        }
        if pname == es1::TEXTURE_CROP_RECT_OES {
            self.state.texture_crop_rect =
                std::slice::from_raw_parts(params, 4).try_into().unwrap();
            return;
        }
        gl::TexParameteriv(target, pname, params);
    }
    unsafe fn TexParameterfv(&mut self, target: GLenum, pname: GLenum, params: *const GLfloat) {
        if params.is_null() {
            return;
        }
        if pname == es1::GENERATE_MIPMAP {
            if *params != 0.0 {
                gl::GenerateMipmap(target);
            }
            return;
        }
        if pname == es1::TEXTURE_CROP_RECT_OES {
            self.state.texture_crop_rect = std::slice::from_raw_parts(params, 4)
                .iter()
                .map(|v| *v as GLint)
                .collect::<Vec<_>>()
                .try_into()
                .unwrap();
            return;
        }
        gl::TexParameterfv(target, pname, params);
    }
    unsafe fn TexParameterxv(&mut self, target: GLenum, pname: GLenum, params: *const GLfixed) {
        if params.is_null() {
            return;
        }
        if pname == es1::GENERATE_MIPMAP {
            if *params != 0 {
                gl::GenerateMipmap(target);
            }
            return;
        }
        if pname == es1::TEXTURE_CROP_RECT_OES {
            self.state.texture_crop_rect = std::slice::from_raw_parts(params, 4)
                .iter()
                .map(|v| fixed_to_float(*v) as GLint)
                .collect::<Vec<_>>()
                .try_into()
                .unwrap();
            return;
        }
        let is_enum = pname == es1::TEXTURE_MIN_FILTER
            || pname == es1::TEXTURE_MAG_FILTER
            || pname == es1::TEXTURE_WRAP_S
            || pname == es1::TEXTURE_WRAP_T;
        if is_enum {
            gl::TexParameteri(target, pname, *params as GLint);
        } else {
            gl::TexParameterf(target, pname, fixed_to_float(*params));
        }
    }
    unsafe fn DrawTexsOES(&mut self, x: i16, y: i16, z: i16, width: i16, height: i16) {
        self.DrawTexfOES(
            x as GLfloat,
            y as GLfloat,
            z as GLfloat,
            width as GLfloat,
            height as GLfloat,
        );
    }
    unsafe fn DrawTexiOES(&mut self, x: GLint, y: GLint, z: GLint, width: GLint, height: GLint) {
        self.DrawTexfOES(
            x as GLfloat,
            y as GLfloat,
            z as GLfloat,
            width as GLfloat,
            height as GLfloat,
        );
    }
    unsafe fn DrawTexxOES(
        &mut self,
        x: GLfixed,
        y: GLfixed,
        z: GLfixed,
        width: GLfixed,
        height: GLfixed,
    ) {
        self.DrawTexfOES(
            fixed_to_float(x),
            fixed_to_float(y),
            fixed_to_float(z),
            fixed_to_float(width),
            fixed_to_float(height),
        );
    }
    unsafe fn DrawTexsvOES(&mut self, coords: *const i16) {
        if !coords.is_null() {
            self.DrawTexsOES(
                coords.read_unaligned(),
                coords.add(1).read_unaligned(),
                coords.add(2).read_unaligned(),
                coords.add(3).read_unaligned(),
                coords.add(4).read_unaligned(),
            );
        }
    }
    unsafe fn DrawTexivOES(&mut self, coords: *const GLint) {
        if !coords.is_null() {
            self.DrawTexiOES(
                coords.read_unaligned(),
                coords.add(1).read_unaligned(),
                coords.add(2).read_unaligned(),
                coords.add(3).read_unaligned(),
                coords.add(4).read_unaligned(),
            );
        }
    }
    unsafe fn DrawTexxvOES(&mut self, coords: *const GLfixed) {
        if !coords.is_null() {
            self.DrawTexxOES(
                coords.read_unaligned(),
                coords.add(1).read_unaligned(),
                coords.add(2).read_unaligned(),
                coords.add(3).read_unaligned(),
                coords.add(4).read_unaligned(),
            );
        }
    }
    unsafe fn DrawTexfvOES(&mut self, coords: *const GLfloat) {
        if !coords.is_null() {
            self.DrawTexfOES(
                coords.read_unaligned(),
                coords.add(1).read_unaligned(),
                coords.add(2).read_unaligned(),
                coords.add(3).read_unaligned(),
                coords.add(4).read_unaligned(),
            );
        }
    }
    unsafe fn DrawTexfOES(
        &mut self,
        x: GLfloat,
        y: GLfloat,
        z: GLfloat,
        width: GLfloat,
        height: GLfloat,
    ) {
        let program = match self.state.program {
            Some(program) => program,
            None => {
                let Ok(program) = create_program(self.state.gles3) else {
                    return;
                };
                self.state.program = Some(program);
                program
            }
        };
        let crop = self.state.texture_crop_rect;
        let viewport = self.state.viewport;
        if width <= 0.0 || height <= 0.0 || viewport[2] <= 0 || viewport[3] <= 0 {
            return;
        }
        let sx = 2.0 / viewport[2] as GLfloat;
        let sy = 2.0 / viewport[3] as GLfloat;
        let x0 = (x - viewport[0] as GLfloat) * sx - 1.0;
        let y0 = (y - viewport[1] as GLfloat) * sy - 1.0;
        let x1 = (x + width - viewport[0] as GLfloat) * sx - 1.0;
        let y1 = (y + height - viewport[1] as GLfloat) * sy - 1.0;
        let vertices = [
            x0, y0, z, 1.0, x1, y0, z, 1.0, x0, y1, z, 1.0, x1, y1, z, 1.0,
        ];
        let tex_w = crop[2].max(1) as GLfloat;
        let tex_h = crop[3].max(1) as GLfloat;
        let u0 = crop[0] as GLfloat / tex_w;
        let v0 = crop[1] as GLfloat / tex_h;
        let u1 = (crop[0] + crop[2]) as GLfloat / tex_w;
        let v1 = (crop[1] + crop[3]) as GLfloat / tex_h;
        let texcoords = [u0, v1, u1, v1, u0, v0, u1, v0];
        gl::UseProgram(program);
        gl::UniformMatrix4fv(
            gl::GetUniformLocation(program, b"u_mvp\0".as_ptr() as *const _),
            1,
            gl::FALSE,
            MATRIX_IDENTITY.as_ptr(),
        );
        gl::Uniform4f(
            gl::GetUniformLocation(program, b"u_color\0".as_ptr() as *const _),
            1.0,
            1.0,
            1.0,
            1.0,
        );
        gl::Uniform1i(
            gl::GetUniformLocation(program, b"u_tex_enabled0\0".as_ptr() as *const _),
            1,
        );
        gl::Uniform1i(
            gl::GetUniformLocation(program, b"u_tex_mode0\0".as_ptr() as *const _),
            1,
        );
        gl::Uniform1i(
            gl::GetUniformLocation(program, b"u_tex0\0".as_ptr() as *const _),
            0,
        );
        gl::DisableVertexAttribArray(ATTR_COLOR);
        gl::VertexAttrib4f(ATTR_COLOR, 1.0, 1.0, 1.0, 1.0);
        gl::DisableVertexAttribArray(ATTR_NORMAL);
        gl::DisableVertexAttribArray(ATTR_TEX0);
        gl::EnableVertexAttribArray(ATTR_POSITION);
        gl::EnableVertexAttribArray(ATTR_TEX0);
        gl::VertexAttribPointer(
            ATTR_POSITION,
            4,
            gl::FLOAT,
            gl::FALSE,
            0,
            vertices.as_ptr().cast(),
        );
        gl::VertexAttribPointer(
            ATTR_TEX0,
            2,
            gl::FLOAT,
            gl::FALSE,
            0,
            texcoords.as_ptr().cast(),
        );
        gl::DrawArrays(gl::TRIANGLE_STRIP, 0, 4);
        gl::DisableVertexAttribArray(ATTR_POSITION);
        gl::DisableVertexAttribArray(ATTR_TEX0);
    }
    unsafe fn TexImage2D(
        &mut self,
        target: GLenum,
        level: GLint,
        mut internalformat: GLint,
        width: GLsizei,
        height: GLsizei,
        border: GLint,
        format: GLenum,
        type_: GLenum,
        pixels: *const GLvoid,
    ) {
        if format == es1::BGRA_EXT {
            internalformat = es1::BGRA_EXT as GLint;
        }
        gl::TexImage2D(
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
    }
    unsafe fn TexSubImage2D(
        &mut self,
        target: GLenum,
        level: GLint,
        x: GLint,
        y: GLint,
        width: GLsizei,
        height: GLsizei,
        format: GLenum,
        type_: GLenum,
        pixels: *const GLvoid,
    ) {
        gl::TexSubImage2D(target, level, x, y, width, height, format, type_, pixels);
    }
    unsafe fn CompressedTexSubImage2D(
        &mut self,
        target: GLenum,
        level: GLint,
        x: GLint,
        y: GLint,
        width: GLsizei,
        height: GLsizei,
        format: GLenum,
        image_size: GLsizei,
        data: *const GLvoid,
    ) {
        gl::CompressedTexSubImage2D(target, level, x, y, width, height, format, image_size, data);
    }
    unsafe fn GetBufferParameteriv(&mut self, target: GLenum, pname: GLenum, params: *mut GLint) {
        if !params.is_null() {
            gl::GetBufferParameteriv(target, pname, params);
        }
    }
    unsafe fn MapBufferOES(&mut self, target: GLenum, _access: GLenum) -> *mut GLvoid {
        let binding = match target {
            gl::ARRAY_BUFFER => self.state.array_buffer_binding,
            gl::ELEMENT_ARRAY_BUFFER => self.state.element_array_buffer_binding,
            _ => 0,
        };
        if binding == 0 {
            return std::ptr::null_mut();
        }
        let store = if target == gl::ARRAY_BUFFER {
            &mut self.state.array_buffer_data
        } else {
            &mut self.state.element_array_buffer_data
        };
        let Some(bytes) = store.get_mut(&binding) else {
            return std::ptr::null_mut();
        };
        self.state.mapped_buffer = Some((target, binding));
        bytes.as_mut_ptr().cast()
    }
    unsafe fn UnmapBufferOES(&mut self, target: GLenum) -> GLboolean {
        if self
            .state
            .mapped_buffer
            .map(|(mapped_target, _)| mapped_target)
            == Some(target)
        {
            self.state.mapped_buffer = None;
            gl::TRUE
        } else {
            gl::FALSE
        }
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
        gl::CopyTexImage2D(target, level, internalformat, x, y, width, height, border);
    }
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
        gl::CopyTexSubImage2D(target, level, xoffset, yoffset, x, y, width, height);
    }
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
        if !data.is_null() && image_size > 0 {
            let payload = std::slice::from_raw_parts(data.cast::<u8>(), image_size as usize);
            if try_decode_pvrtc(
                self,
                target,
                level,
                internalformat,
                width,
                height,
                border,
                payload,
            ) {
                return;
            }
            if let Some(decoded) =
                PalettedTextureFormat::decode_rgba8(internalformat, width, height, payload)
            {
                gl::TexImage2D(
                    target,
                    level,
                    es1::RGBA as GLint,
                    width,
                    height,
                    border,
                    es1::RGBA,
                    es1::UNSIGNED_BYTE,
                    decoded.as_ptr().cast(),
                );
                return;
            }
        }
        gl::CompressedTexImage2D(
            target,
            level,
            internalformat,
            width,
            height,
            border,
            image_size,
            data,
        );
    }
    unsafe fn TexEnvi(&mut self, target: GLenum, pname: GLenum, param: GLint) {
        if target != es1::TEXTURE_ENV {
            return;
        }
        let unit = self.state.active_texture;
        let value = param as GLenum;
        match pname {
            es1::TEXTURE_ENV_MODE => self.state.texture_env_mode[unit] = param,
            es1::TEXTURE_ENV_COLOR => self.state.texture_env_color[unit] = [param as GLfloat; 4],
            es1::COMBINE_RGB => self.state.texture_combine_rgb[unit] = value,
            es1::COMBINE_ALPHA => self.state.texture_combine_alpha[unit] = value,
            es1::SRC0_RGB..=es1::SRC2_RGB => {
                self.state.texture_src_rgb[unit][(pname - es1::SRC0_RGB) as usize] = value
            }
            es1::SRC0_ALPHA..=es1::SRC2_ALPHA => {
                self.state.texture_src_alpha[unit][(pname - es1::SRC0_ALPHA) as usize] = value
            }
            es1::OPERAND0_RGB..=es1::OPERAND2_RGB => {
                self.state.texture_operand_rgb[unit][(pname - es1::OPERAND0_RGB) as usize] = value
            }
            es1::OPERAND0_ALPHA..=es1::OPERAND2_ALPHA => {
                self.state.texture_operand_alpha[unit][(pname - es1::OPERAND0_ALPHA) as usize] =
                    value
            }
            es1::RGB_SCALE => self.state.texture_rgb_scale[unit] = param as GLfloat,
            es1::ALPHA_SCALE => self.state.texture_alpha_scale[unit] = param as GLfloat,
            _ => {}
        }
    }
    unsafe fn TexEnvf(&mut self, target: GLenum, pname: GLenum, param: GLfloat) {
        if pname == es1::TEXTURE_ENV_COLOR {
            self.state.texture_env_color[self.state.active_texture] = [param; 4];
        } else {
            self.TexEnvi(target, pname, param as GLint);
        }
    }
    unsafe fn TexEnvx(&mut self, target: GLenum, pname: GLenum, param: GLfixed) {
        self.TexEnvf(target, pname, fixed_to_float(param));
    }
    unsafe fn TexEnviv(&mut self, target: GLenum, pname: GLenum, params: *const GLint) {
        if params.is_null() {
            return;
        }
        if pname == es1::TEXTURE_ENV_COLOR {
            self.state.texture_env_color[self.state.active_texture] =
                std::slice::from_raw_parts(params, 4)
                    .iter()
                    .map(|v| *v as GLfloat)
                    .collect::<Vec<_>>()
                    .try_into()
                    .unwrap();
        } else {
            self.TexEnvi(target, pname, *params);
        }
    }
    unsafe fn TexEnvfv(&mut self, target: GLenum, pname: GLenum, params: *const GLfloat) {
        if params.is_null() {
            return;
        }
        if pname == es1::TEXTURE_ENV_COLOR {
            self.state.texture_env_color[self.state.active_texture] =
                std::slice::from_raw_parts(params, 4).try_into().unwrap();
        } else {
            self.TexEnvf(target, pname, *params);
        }
    }
    unsafe fn TexEnvxv(&mut self, target: GLenum, pname: GLenum, params: *const GLfixed) {
        if params.is_null() {
            return;
        }
        if pname == es1::TEXTURE_ENV_COLOR {
            self.state.texture_env_color[self.state.active_texture] =
                std::slice::from_raw_parts(params, 4)
                    .iter()
                    .map(|v| fixed_to_float(*v))
                    .collect::<Vec<_>>()
                    .try_into()
                    .unwrap();
        } else {
            self.TexEnvx(target, pname, *params);
        }
    }
    unsafe fn MatrixMode(&mut self, mode: GLenum) {
        self.state.matrix_mode = mode;
    }
    unsafe fn LoadIdentity(&mut self) {
        self.state.matrix_mut().current = MATRIX_IDENTITY;
    }
    unsafe fn LoadMatrixf(&mut self, m: *const GLfloat) {
        self.state
            .matrix_mut()
            .current
            .copy_from_slice(std::slice::from_raw_parts(m, 16));
    }
    unsafe fn LoadMatrixx(&mut self, m: *const GLfixed) {
        let mut out = [0.0; 16];
        for (d, s) in out.iter_mut().zip(std::slice::from_raw_parts(m, 16)) {
            *d = fixed_to_float(*s);
        }
        self.state.matrix_mut().current = out;
    }
    unsafe fn MultMatrixf(&mut self, m: *const GLfloat) {
        let a = self.state.matrix_mut().current;
        let b = std::slice::from_raw_parts(m, 16).try_into().unwrap();
        self.state.matrix_mut().current = multiply(&a, &b);
    }
    unsafe fn MultMatrixx(&mut self, m: *const GLfixed) {
        let mut b = [0.0; 16];
        for (d, s) in b.iter_mut().zip(std::slice::from_raw_parts(m, 16)) {
            *d = fixed_to_float(*s);
        }
        let a = self.state.matrix_mut().current;
        self.state.matrix_mut().current = multiply(&a, &b);
    }
    unsafe fn PushMatrix(&mut self) {
        let current = self.state.matrix_mut().current;
        self.state.matrix_mut().stack.push(current);
    }
    unsafe fn PopMatrix(&mut self) {
        if let Some(m) = self.state.matrix_mut().stack.pop() {
            self.state.matrix_mut().current = m;
        }
    }
    unsafe fn Orthof(
        &mut self,
        l: GLfloat,
        r: GLfloat,
        b: GLfloat,
        t: GLfloat,
        n: GLfloat,
        f: GLfloat,
    ) {
        let a = self.state.matrix_mut().current;
        self.state.matrix_mut().current = multiply(&a, &ortho(l, r, b, t, n, f));
    }
    unsafe fn Orthox(
        &mut self,
        l: GLfixed,
        r: GLfixed,
        b: GLfixed,
        t: GLfixed,
        n: GLfixed,
        f: GLfixed,
    ) {
        self.Orthof(
            fixed_to_float(l),
            fixed_to_float(r),
            fixed_to_float(b),
            fixed_to_float(t),
            fixed_to_float(n),
            fixed_to_float(f),
        );
    }
    unsafe fn Frustumf(
        &mut self,
        l: GLfloat,
        r: GLfloat,
        b: GLfloat,
        t: GLfloat,
        n: GLfloat,
        f: GLfloat,
    ) {
        let a = self.state.matrix_mut().current;
        self.state.matrix_mut().current = multiply(&a, &frustum(l, r, b, t, n, f));
    }
    unsafe fn Frustumx(
        &mut self,
        l: GLfixed,
        r: GLfixed,
        b: GLfixed,
        t: GLfixed,
        n: GLfixed,
        f: GLfixed,
    ) {
        self.Frustumf(
            fixed_to_float(l),
            fixed_to_float(r),
            fixed_to_float(b),
            fixed_to_float(t),
            fixed_to_float(n),
            fixed_to_float(f),
        );
    }
    unsafe fn Translatef(&mut self, x: GLfloat, y: GLfloat, z: GLfloat) {
        let a = self.state.matrix_mut().current;
        self.state.matrix_mut().current = multiply(&a, &translation(x, y, z));
    }
    unsafe fn Translatex(&mut self, x: GLfixed, y: GLfixed, z: GLfixed) {
        self.Translatef(fixed_to_float(x), fixed_to_float(y), fixed_to_float(z));
    }
    unsafe fn Scalef(&mut self, x: GLfloat, y: GLfloat, z: GLfloat) {
        let a = self.state.matrix_mut().current;
        self.state.matrix_mut().current = multiply(&a, &scaling(x, y, z));
    }
    unsafe fn Scalex(&mut self, x: GLfixed, y: GLfixed, z: GLfixed) {
        self.Scalef(fixed_to_float(x), fixed_to_float(y), fixed_to_float(z));
    }
    unsafe fn Rotatef(&mut self, a: GLfloat, x: GLfloat, y: GLfloat, z: GLfloat) {
        let m = self.state.matrix_mut().current;
        self.state.matrix_mut().current = multiply(&m, &rotation(a, x, y, z));
    }
    unsafe fn Rotatex(&mut self, a: GLfixed, x: GLfixed, y: GLfixed, z: GLfixed) {
        self.Rotatef(
            fixed_to_float(a),
            fixed_to_float(x),
            fixed_to_float(y),
            fixed_to_float(z),
        );
    }
    unsafe fn Viewport(&mut self, x: GLint, y: GLint, w: GLsizei, h: GLsizei) {
        self.state.viewport = [x, y, w, h];
        gl::Viewport(x, y, w, h);
    }
    unsafe fn Scissor(&mut self, x: GLint, y: GLint, w: GLsizei, h: GLsizei) {
        gl::Scissor(x, y, w, h);
    }
    unsafe fn Clear(&mut self, mask: GLbitfield) {
        gl::Clear(mask);
    }
    unsafe fn ClearColor(&mut self, r: GLclampf, g: GLclampf, b: GLclampf, a: GLclampf) {
        gl::ClearColor(r, g, b, a);
    }
    unsafe fn ClearColorx(&mut self, r: GLclampx, g: GLclampx, b: GLclampx, a: GLclampx) {
        self.ClearColor(
            fixed_to_float(r),
            fixed_to_float(g),
            fixed_to_float(b),
            fixed_to_float(a),
        );
    }
    unsafe fn ClearDepthf(&mut self, d: GLclampf) {
        gl::ClearDepthf(d);
    }
    unsafe fn ClearStencil(&mut self, s: GLint) {
        gl::ClearStencil(s);
    }
    unsafe fn Fogf(&mut self, pname: GLenum, param: GLfloat) {
        match pname {
            es1::FOG_MODE => self.state.fog_mode = param as GLenum,
            es1::FOG_DENSITY => self.state.fog_density = param,
            es1::FOG_START => self.state.fog_start = param,
            es1::FOG_END => self.state.fog_end = param,
            _ => {}
        }
    }
    unsafe fn Fogx(&mut self, pname: GLenum, param: GLfixed) {
        self.Fogf(pname, fixed_to_float(param));
    }
    unsafe fn Fogfv(&mut self, pname: GLenum, params: *const GLfloat) {
        if pname == es1::FOG_COLOR {
            self.state.fog_color = std::slice::from_raw_parts(params, 4).try_into().unwrap();
        } else {
            self.Fogf(pname, *params);
        }
    }
    unsafe fn Fogxv(&mut self, pname: GLenum, params: *const GLfixed) {
        if pname == es1::FOG_COLOR {
            self.state.fog_color = std::slice::from_raw_parts(params, 4)
                .iter()
                .map(|v| fixed_to_float(*v))
                .collect::<Vec<_>>()
                .try_into()
                .unwrap();
        } else {
            self.Fogx(pname, *params);
        }
    }
    unsafe fn GetClipPlanef(&mut self, plane: GLenum, equation: *mut GLfloat) {
        if equation.is_null() || !(es1::CLIP_PLANE0..=es1::CLIP_PLANE5).contains(&plane) {
            return;
        }
        equation.copy_from(
            self.state.clip_planes[(plane - es1::CLIP_PLANE0) as usize].as_ptr(),
            4,
        );
    }
    unsafe fn GetClipPlanex(&mut self, plane: GLenum, equation: *mut GLfixed) {
        if equation.is_null() || !(es1::CLIP_PLANE0..=es1::CLIP_PLANE5).contains(&plane) {
            return;
        }
        for (i, value) in self.state.clip_planes[(plane - es1::CLIP_PLANE0) as usize]
            .iter()
            .enumerate()
        {
            *equation.add(i) = float_to_fixed(*value);
        }
    }
    unsafe fn GetIntegerv(&mut self, pname: GLenum, params: *mut GLint) {
        if params.is_null() {
            return;
        }
        match pname {
            es1::VIEWPORT => params.copy_from(self.state.viewport.as_ptr(), 4),
            es1::TEXTURE_CROP_RECT_OES => {
                params.copy_from(self.state.texture_crop_rect.as_ptr(), 4)
            }
            es1::ACTIVE_TEXTURE => {
                *params = es1::TEXTURE0 as GLint + self.state.active_texture as GLint
            }
            es1::CLIENT_ACTIVE_TEXTURE => {
                *params = es1::TEXTURE0 as GLint + self.state.client_active_texture as GLint
            }
            es1::MATRIX_MODE => *params = self.state.matrix_mode as GLint,
            es1::ARRAY_BUFFER_BINDING => *params = self.state.array_buffer_binding as GLint,
            es1::ELEMENT_ARRAY_BUFFER_BINDING => {
                *params = self.state.element_array_buffer_binding as GLint
            }
            es1::POINT_SIZE_ARRAY_OES => {
                *params = if self.state.point_size_array.enabled {
                    gl::TRUE as GLint
                } else {
                    gl::FALSE as GLint
                }
            }
            es1::MAX_PALETTE_MATRICES_OES => *params = MAX_PALETTE_MATRICES as GLint,
            _ => gl::GetIntegerv(pname, params),
        }
    }
    unsafe fn DepthFunc(&mut self, f: GLenum) {
        gl::DepthFunc(f);
    }
    unsafe fn DepthMask(&mut self, f: GLboolean) {
        gl::DepthMask(f);
    }
    unsafe fn CullFace(&mut self, f: GLenum) {
        gl::CullFace(f);
    }
    unsafe fn FrontFace(&mut self, f: GLenum) {
        gl::FrontFace(f);
    }
    unsafe fn BlendFunc(&mut self, s: GLenum, d: GLenum) {
        gl::BlendFunc(s, d);
    }
    unsafe fn BlendEquation(&mut self, mode: GLenum) {
        gl::BlendEquation(mode);
    }
    unsafe fn BlendEquationSeparate(&mut self, mode_rgb: GLenum, mode_alpha: GLenum) {
        gl::BlendEquationSeparate(mode_rgb, mode_alpha);
    }
    unsafe fn BlendFuncSeparate(
        &mut self,
        src_rgb: GLenum,
        dst_rgb: GLenum,
        src_alpha: GLenum,
        dst_alpha: GLenum,
    ) {
        gl::BlendFuncSeparate(src_rgb, dst_rgb, src_alpha, dst_alpha);
    }
    unsafe fn StencilFuncSeparate(
        &mut self,
        face: GLenum,
        func: GLenum,
        ref_: GLint,
        mask: GLuint,
    ) {
        gl::StencilFuncSeparate(face, func, ref_, mask);
    }
    unsafe fn StencilOpSeparate(
        &mut self,
        face: GLenum,
        sfail: GLenum,
        dpfail: GLenum,
        dppass: GLenum,
    ) {
        gl::StencilOpSeparate(face, sfail, dpfail, dppass);
    }
    unsafe fn StencilMaskSeparate(&mut self, face: GLenum, mask: GLuint) {
        gl::StencilMaskSeparate(face, mask);
    }
    unsafe fn BlendColor(&mut self, r: GLclampf, g: GLclampf, b: GLclampf, a: GLclampf) {
        gl::BlendColor(r, g, b, a);
    }
    unsafe fn BlendEquationOES(&mut self, m: GLenum) {
        gl::BlendEquation(m);
    }
    unsafe fn LogicOp(&mut self, opcode: GLenum) {
        self.state.logic_op = opcode;
    }
    unsafe fn ColorMask(&mut self, r: GLboolean, g: GLboolean, b: GLboolean, a: GLboolean) {
        gl::ColorMask(r, g, b, a);
    }
    unsafe fn LineWidth(&mut self, w: GLfloat) {
        gl::LineWidth(w);
    }
    unsafe fn Finish(&mut self) {
        gl::Finish();
    }
    unsafe fn Flush(&mut self) {
        gl::Flush();
    }
    unsafe fn ReadPixels(
        &mut self,
        x: GLint,
        y: GLint,
        w: GLsizei,
        h: GLsizei,
        format: GLenum,
        type_: GLenum,
        pixels: *mut GLvoid,
    ) {
        gl::ReadPixels(x, y, w, h, format, type_, pixels);
    }
    unsafe fn PixelStorei(&mut self, p: GLenum, v: GLint) {
        gl::PixelStorei(p, v);
    }
    unsafe fn GenFramebuffersOES(&mut self, n: GLsizei, p: *mut GLuint) {
        gl::GenFramebuffers(n, p);
    }
    unsafe fn DeleteFramebuffersOES(&mut self, n: GLsizei, p: *const GLuint) {
        gl::DeleteFramebuffers(n, p);
    }
    unsafe fn BindFramebufferOES(&mut self, t: GLenum, f: GLuint) {
        gl::BindFramebuffer(t, f);
    }
    unsafe fn GenRenderbuffersOES(&mut self, n: GLsizei, p: *mut GLuint) {
        gl::GenRenderbuffers(n, p);
    }
    unsafe fn DeleteRenderbuffersOES(&mut self, n: GLsizei, p: *const GLuint) {
        gl::DeleteRenderbuffers(n, p);
    }
    unsafe fn BindRenderbufferOES(&mut self, t: GLenum, r: GLuint) {
        gl::BindRenderbuffer(t, r);
    }
    unsafe fn RenderbufferStorageOES(&mut self, t: GLenum, f: GLenum, w: GLsizei, h: GLsizei) {
        gl::RenderbufferStorage(t, f, w, h);
    }
    unsafe fn RenderbufferStorageMultisampleAPPLE(
        &mut self,
        t: GLenum,
        samples: GLsizei,
        f: GLenum,
        w: GLsizei,
        h: GLsizei,
    ) {
        if gl::RenderbufferStorageMultisampleAPPLE::is_loaded() {
            gl::RenderbufferStorageMultisampleAPPLE(t, samples, f, w, h);
        } else {
            gl::RenderbufferStorage(t, f, w, h);
        }
    }
    unsafe fn ResolveMultisampleFramebufferAPPLE(&mut self) {
        if gl::ResolveMultisampleFramebufferAPPLE::is_loaded() {
            gl::ResolveMultisampleFramebufferAPPLE();
        }
    }
    unsafe fn GetRenderbufferParameterivOES(&mut self, t: GLenum, p: GLenum, params: *mut GLint) {
        gl::GetRenderbufferParameteriv(t, p, params);
    }
    unsafe fn FramebufferRenderbufferOES(&mut self, t: GLenum, a: GLenum, rt: GLenum, r: GLuint) {
        gl::FramebufferRenderbuffer(t, a, rt, r);
    }
    unsafe fn FramebufferTexture2DOES(
        &mut self,
        t: GLenum,
        a: GLenum,
        tt: GLenum,
        tex: GLuint,
        level: GLint,
    ) {
        gl::FramebufferTexture2D(t, a, tt, tex, level);
    }
    unsafe fn GetFramebufferAttachmentParameterivOES(
        &mut self,
        t: GLenum,
        a: GLenum,
        p: GLenum,
        params: *mut GLint,
    ) {
        gl::GetFramebufferAttachmentParameteriv(t, a, p, params);
    }
    unsafe fn GenerateMipmapOES(&mut self, t: GLenum) {
        gl::GenerateMipmap(t);
    }
    unsafe fn CheckFramebufferStatus(&mut self, t: GLenum) -> GLenum {
        gl::CheckFramebufferStatus(t)
    }
    unsafe fn CheckFramebufferStatusOES(&mut self, t: GLenum) -> GLenum {
        gl::CheckFramebufferStatus(t)
    }
    unsafe fn IsFramebufferOES(&mut self, f: GLuint) -> GLboolean {
        gl::IsFramebuffer(f)
    }
    unsafe fn IsRenderbufferOES(&mut self, r: GLuint) -> GLboolean {
        gl::IsRenderbuffer(r)
    }
    unsafe fn GenerateMipmap(&mut self, t: GLenum) {
        gl::GenerateMipmap(t);
    }
    unsafe fn GetFramebufferAttachmentParameteriv(
        &mut self,
        t: GLenum,
        a: GLenum,
        p: GLenum,
        params: *mut GLint,
    ) {
        gl::GetFramebufferAttachmentParameteriv(t, a, p, params);
    }
    unsafe fn GetRenderbufferParameteriv(&mut self, t: GLenum, p: GLenum, params: *mut GLint) {
        gl::GetRenderbufferParameteriv(t, p, params);
    }
    unsafe fn BindFramebuffer(&mut self, t: GLenum, f: GLuint) {
        gl::BindFramebuffer(t, f);
    }
    unsafe fn DeleteFramebuffers(&mut self, n: GLsizei, p: *const GLuint) {
        gl::DeleteFramebuffers(n, p);
    }
    unsafe fn GenFramebuffers(&mut self, n: GLsizei, p: *mut GLuint) {
        gl::GenFramebuffers(n, p);
    }
    unsafe fn BindRenderbuffer(&mut self, t: GLenum, r: GLuint) {
        gl::BindRenderbuffer(t, r);
    }
    unsafe fn RenderbufferStorage(&mut self, t: GLenum, f: GLenum, w: GLsizei, h: GLsizei) {
        gl::RenderbufferStorage(t, f, w, h);
    }
    unsafe fn FramebufferRenderbuffer(&mut self, t: GLenum, a: GLenum, rt: GLenum, r: GLuint) {
        gl::FramebufferRenderbuffer(t, a, rt, r);
    }
    unsafe fn FramebufferTexture2D(
        &mut self,
        t: GLenum,
        a: GLenum,
        tt: GLenum,
        tex: GLuint,
        level: GLint,
    ) {
        gl::FramebufferTexture2D(t, a, tt, tex, level);
    }
    unsafe fn DeleteRenderbuffers(&mut self, n: GLsizei, p: *const GLuint) {
        gl::DeleteRenderbuffers(n, p);
    }
    unsafe fn GenRenderbuffers(&mut self, n: GLsizei, p: *mut GLuint) {
        gl::GenRenderbuffers(n, p);
    }
    unsafe fn IsFramebuffer(&mut self, f: GLuint) -> GLboolean {
        gl::IsFramebuffer(f)
    }
    unsafe fn IsRenderbuffer(&mut self, r: GLuint) -> GLboolean {
        gl::IsRenderbuffer(r)
    }
    unsafe fn IsTexture(&mut self, t: GLuint) -> GLboolean {
        gl::IsTexture(t)
    }
    unsafe fn PointSize(&mut self, size: GLfloat) {
        self.state.point_size = size;
    }
    unsafe fn PointSizex(&mut self, size: GLfixed) {
        self.state.point_size = fixed_to_float(size);
    }
    unsafe fn PointSizePointerOES(
        &mut self,
        type_: GLenum,
        stride: GLsizei,
        pointer: *const GLvoid,
    ) {
        self.state.point_size_array.size = 1;
        self.state.point_size_array.type_ = type_;
        self.state.point_size_array.stride = stride;
        self.state.point_size_array.pointer = pointer;
        self.state.point_size_array.buffer_binding = self.state.array_buffer_binding;
        self.state.point_size_array.fixed = type_ == es1::FIXED;
    }
    unsafe fn CurrentPaletteMatrixOES(&mut self, matrixpaletteindex: GLuint) {
        self.state.current_palette_matrix =
            (matrixpaletteindex as usize).min(MAX_PALETTE_MATRICES - 1);
    }
    unsafe fn LoadPaletteFromModelViewMatrixOES(&mut self) {
        let index = self.state.current_palette_matrix;
        self.state.palette_matrices[index].current = self.state.modelview.current;
    }
    unsafe fn MatrixIndexPointerOES(
        &mut self,
        size: GLint,
        type_: GLenum,
        stride: GLsizei,
        pointer: *const GLvoid,
    ) {
        self.state.palette_index_array.size = size;
        self.state.palette_index_array.type_ = type_;
        self.state.palette_index_array.stride = stride;
        self.state.palette_index_array.pointer = pointer;
        self.state.palette_index_array.buffer_binding = self.state.array_buffer_binding;
    }
    unsafe fn WeightPointerOES(
        &mut self,
        size: GLint,
        type_: GLenum,
        stride: GLsizei,
        pointer: *const GLvoid,
    ) {
        self.state.palette_weight_array.size = size;
        self.state.palette_weight_array.type_ = type_;
        self.state.palette_weight_array.stride = stride;
        self.state.palette_weight_array.pointer = pointer;
        self.state.palette_weight_array.buffer_binding = self.state.array_buffer_binding;
        self.state.palette_weight_array.fixed = type_ == es1::FIXED;
    }
    unsafe fn DrawArrays(&mut self, mode: GLenum, first: GLint, count: GLsizei) {
        let program = match self.ensure_program() {
            Some(program) => program,
            None => return,
        };
        gl::UseProgram(program);
        let mvp = unsafe { self.state.mvp() };
        let mvp_loc = gl::GetUniformLocation(program, b"u_mvp\0".as_ptr() as *const _);
        gl::UniformMatrix4fv(mvp_loc, 1, gl::FALSE, mvp.as_ptr());
        let modelview_loc = gl::GetUniformLocation(program, b"u_modelview\0".as_ptr() as *const _);
        gl::UniformMatrix4fv(
            modelview_loc,
            1,
            gl::FALSE,
            self.state.modelview.current.as_ptr(),
        );
        let projection_loc =
            gl::GetUniformLocation(program, b"u_projection\0".as_ptr() as *const _);
        gl::UniformMatrix4fv(
            projection_loc,
            1,
            gl::FALSE,
            self.state.projection.current.as_ptr(),
        );
        let texture_matrix_loc =
            gl::GetUniformLocation(program, b"u_texture_matrix0\0".as_ptr() as *const _);
        gl::UniformMatrix4fv(
            texture_matrix_loc,
            1,
            gl::FALSE,
            self.state.texture[0].current.as_ptr(),
        );
        for unit in 1..MAX_TEXTURE_UNITS {
            let name = format!("u_texture_matrix{}\0", unit);
            gl::UniformMatrix4fv(
                gl::GetUniformLocation(program, name.as_ptr() as *const _),
                1,
                gl::FALSE,
                self.state.texture[unit].current.as_ptr(),
            );
        }
        let color_loc = gl::GetUniformLocation(program, b"u_color\0".as_ptr() as *const _);
        let color_uniform = if self.state.arrays[1].enabled {
            [1.0; 4]
        } else {
            self.state.color
        };
        gl::Uniform4fv(color_loc, 1, color_uniform.as_ptr());
        gl::Uniform1i(
            gl::GetUniformLocation(program, b"u_lighting_enabled\0".as_ptr() as *const _),
            self.state.lighting_enabled as GLint,
        );
        let mut light_enabled = [0; 8];
        let mut light_ambient = [[0.0; 4]; 8];
        let mut light_diffuse = [[0.0; 4]; 8];
        let mut light_specular = [[0.0; 4]; 8];
        let mut light_position = [[0.0; 4]; 8];
        let mut light_spot_direction = [[0.0; 3]; 8];
        let mut light_spot_cutoff = [0.0; 8];
        let mut light_spot_exponent = [0.0; 8];
        let mut light_constant_attenuation = [0.0; 8];
        let mut light_linear_attenuation = [0.0; 8];
        let mut light_quadratic_attenuation = [0.0; 8];
        for index in 0..8 {
            let light = self.state.lights[index];
            light_enabled[index] = self.state.light_enabled[index] as GLint;
            light_ambient[index] = light.ambient;
            light_diffuse[index] = light.diffuse;
            light_specular[index] = light.specular;
            light_position[index] = light.position;
            light_spot_direction[index] = light.spot_direction;
            light_spot_cutoff[index] = light.spot_cutoff;
            light_spot_exponent[index] = light.spot_exponent;
            light_constant_attenuation[index] = light.constant_attenuation;
            light_linear_attenuation[index] = light.linear_attenuation;
            light_quadratic_attenuation[index] = light.quadratic_attenuation;
        }
        gl::Uniform1iv(
            gl::GetUniformLocation(program, b"u_light_enabled\0".as_ptr() as *const _),
            8,
            light_enabled.as_ptr(),
        );
        gl::Uniform4fv(
            gl::GetUniformLocation(program, b"u_light_ambient\0".as_ptr() as *const _),
            8,
            light_ambient.as_ptr().cast(),
        );
        gl::Uniform4fv(
            gl::GetUniformLocation(program, b"u_light_diffuse\0".as_ptr() as *const _),
            8,
            light_diffuse.as_ptr().cast(),
        );
        gl::Uniform4fv(
            gl::GetUniformLocation(program, b"u_light_specular\0".as_ptr() as *const _),
            8,
            light_specular.as_ptr().cast(),
        );
        gl::Uniform4fv(
            gl::GetUniformLocation(program, b"u_light_position\0".as_ptr() as *const _),
            8,
            light_position.as_ptr().cast(),
        );
        gl::Uniform3fv(
            gl::GetUniformLocation(program, b"u_light_spot_direction\0".as_ptr() as *const _),
            8,
            light_spot_direction.as_ptr().cast(),
        );
        gl::Uniform1fv(
            gl::GetUniformLocation(program, b"u_light_spot_cutoff\0".as_ptr() as *const _),
            8,
            light_spot_cutoff.as_ptr(),
        );
        gl::Uniform1fv(
            gl::GetUniformLocation(program, b"u_light_spot_exponent\0".as_ptr() as *const _),
            8,
            light_spot_exponent.as_ptr(),
        );
        gl::Uniform1fv(
            gl::GetUniformLocation(
                program,
                b"u_light_constant_attenuation\0".as_ptr() as *const _,
            ),
            8,
            light_constant_attenuation.as_ptr(),
        );
        gl::Uniform1fv(
            gl::GetUniformLocation(
                program,
                b"u_light_linear_attenuation\0".as_ptr() as *const _,
            ),
            8,
            light_linear_attenuation.as_ptr(),
        );
        gl::Uniform1fv(
            gl::GetUniformLocation(
                program,
                b"u_light_quadratic_attenuation\0".as_ptr() as *const _,
            ),
            8,
            light_quadratic_attenuation.as_ptr(),
        );
        gl::Uniform1i(
            gl::GetUniformLocation(program, b"u_color_material_enabled\0".as_ptr() as *const _),
            self.state.color_material_enabled as GLint,
        );
        gl::Uniform1i(
            gl::GetUniformLocation(program, b"u_normalize_enabled\0".as_ptr() as *const _),
            self.state.normalize_enabled as GLint,
        );
        gl::Uniform4fv(
            gl::GetUniformLocation(program, b"u_material_ambient\0".as_ptr() as *const _),
            1,
            self.state.material_ambient.as_ptr(),
        );
        gl::Uniform4fv(
            gl::GetUniformLocation(program, b"u_material_diffuse\0".as_ptr() as *const _),
            1,
            self.state.material_diffuse.as_ptr(),
        );
        gl::Uniform4fv(
            gl::GetUniformLocation(program, b"u_material_specular\0".as_ptr() as *const _),
            1,
            self.state.material_specular.as_ptr(),
        );
        gl::Uniform4fv(
            gl::GetUniformLocation(program, b"u_material_emission\0".as_ptr() as *const _),
            1,
            self.state.material_emission.as_ptr(),
        );
        gl::Uniform1i(
            gl::GetUniformLocation(
                program,
                b"u_light_model_local_viewer\0".as_ptr() as *const _,
            ),
            self.state.light_model_local_viewer as GLint,
        );
        gl::Uniform1i(
            gl::GetUniformLocation(program, b"u_light_model_two_side\0".as_ptr() as *const _),
            self.state.light_model_two_side as GLint,
        );
        gl::Uniform1f(
            gl::GetUniformLocation(program, b"u_material_shininess\0".as_ptr() as *const _),
            self.state.material_shininess,
        );
        gl::Uniform4fv(
            gl::GetUniformLocation(program, b"u_model_ambient\0".as_ptr() as *const _),
            1,
            self.state.model_ambient.as_ptr(),
        );
        gl::Uniform4fv(
            gl::GetUniformLocation(program, b"u_clip_planes\0".as_ptr() as *const _),
            6,
            self.state.clip_planes.as_ptr().cast(),
        );
        gl::Uniform1iv(
            gl::GetUniformLocation(program, b"u_clip_enabled\0".as_ptr() as *const _),
            6,
            self.state.clip_plane_enabled.as_ptr().cast(),
        );
        gl::Uniform3fv(
            gl::GetUniformLocation(
                program,
                b"u_point_distance_attenuation\0".as_ptr() as *const _,
            ),
            1,
            self.state.point_distance_attenuation.as_ptr(),
        );
        gl::Uniform1f(
            gl::GetUniformLocation(program, b"u_point_fade_threshold\0".as_ptr() as *const _),
            self.state.point_fade_threshold,
        );
        let alpha_test_loc =
            gl::GetUniformLocation(program, b"u_alpha_test_enabled\0".as_ptr() as *const _);
        gl::Uniform1i(
            alpha_test_loc,
            if self.state.alpha_test_enabled { 1 } else { 0 },
        );
        let alpha_func_loc =
            gl::GetUniformLocation(program, b"u_alpha_func\0".as_ptr() as *const _);
        gl::Uniform1i(alpha_func_loc, self.state.alpha_func as GLint);
        let alpha_ref_loc = gl::GetUniformLocation(program, b"u_alpha_ref\0".as_ptr() as *const _);
        gl::Uniform1f(alpha_ref_loc, self.state.alpha_ref);
        let fog_enabled_loc =
            gl::GetUniformLocation(program, b"u_fog_enabled\0".as_ptr() as *const _);
        gl::Uniform1i(fog_enabled_loc, if self.state.fog_enabled { 1 } else { 0 });
        let fog_color_loc = gl::GetUniformLocation(program, b"u_fog_color\0".as_ptr() as *const _);
        gl::Uniform4fv(fog_color_loc, 1, self.state.fog_color.as_ptr());
        let fog_density_loc =
            gl::GetUniformLocation(program, b"u_fog_density\0".as_ptr() as *const _);
        gl::Uniform1f(fog_density_loc, self.state.fog_density);
        let fog_start_loc = gl::GetUniformLocation(program, b"u_fog_start\0".as_ptr() as *const _);
        gl::Uniform1f(fog_start_loc, self.state.fog_start);
        let fog_end_loc = gl::GetUniformLocation(program, b"u_fog_end\0".as_ptr() as *const _);
        gl::Uniform1f(fog_end_loc, self.state.fog_end);
        let fog_mode_loc = gl::GetUniformLocation(program, b"u_fog_mode\0".as_ptr() as *const _);
        gl::Uniform1i(fog_mode_loc, self.state.fog_mode as GLint);
        let point_size_loc =
            gl::GetUniformLocation(program, b"u_point_size\0".as_ptr() as *const _);
        gl::Uniform1f(point_size_loc, self.state.point_size);
        gl::Uniform1i(
            gl::GetUniformLocation(
                program,
                b"u_point_size_array_enabled\0".as_ptr() as *const _,
            ),
            if self.state.point_size_array.enabled {
                1
            } else {
                0
            },
        );
        gl::Uniform1i(
            gl::GetUniformLocation(program, b"u_matrix_palette_enabled\0".as_ptr() as *const _),
            if self.state.matrix_palette_enabled {
                1
            } else {
                0
            },
        );
        for (i, matrix) in self.state.palette_matrices.iter().enumerate() {
            let name = format!("u_palette_matrices[{}]\0", i);
            gl::UniformMatrix4fv(
                gl::GetUniformLocation(program, name.as_ptr() as *const _),
                1,
                gl::FALSE,
                matrix.current.as_ptr(),
            );
        }
        for unit in 0..MAX_TEXTURE_UNITS {
            let enabled_name = format!("u_tex_enabled{}\0", unit);
            let mode_name = format!("u_tex_mode{}\0", unit);
            let env_name = format!("u_env_color{}\0", unit);
            let sampler_name = format!("u_tex{}\0", unit);
            gl::ActiveTexture(gl::TEXTURE0 + unit as GLenum);
            gl::BindTexture(gl::TEXTURE_2D, self.state.bound_textures[unit]);
            let mode = match self.state.texture_env_mode[unit] as GLenum {
                es1::REPLACE => 1,
                es1::ADD => 3,
                es1::DECAL => 4,
                es1::BLEND => 5,
                es1::COMBINE => 0,
                _ => 2,
            };
            let combine_name = format!("u_combine_rgb{}\0", unit);
            let combine_alpha_name = format!("u_combine_alpha{}\0", unit);
            let src_rgb_name = format!("u_src_rgb{}\0", unit);
            let src_alpha_name = format!("u_src_alpha{}\0", unit);
            let operand_rgb_name = format!("u_operand_rgb{}\0", unit);
            let operand_alpha_name = format!("u_operand_alpha{}\0", unit);
            let rgb_scale_name = format!("u_rgb_scale{}\0", unit);
            let alpha_scale_name = format!("u_alpha_scale{}\0", unit);
            gl::Uniform1i(
                gl::GetUniformLocation(program, combine_name.as_ptr() as *const _),
                self.state.texture_combine_rgb[unit] as GLint,
            );
            gl::Uniform1i(
                gl::GetUniformLocation(program, combine_alpha_name.as_ptr() as *const _),
                self.state.texture_combine_alpha[unit] as GLint,
            );
            gl::Uniform1iv(
                gl::GetUniformLocation(program, src_rgb_name.as_ptr() as *const _),
                3,
                self.state.texture_src_rgb[unit].as_ptr().cast(),
            );
            gl::Uniform1iv(
                gl::GetUniformLocation(program, src_alpha_name.as_ptr() as *const _),
                3,
                self.state.texture_src_alpha[unit].as_ptr().cast(),
            );
            gl::Uniform1iv(
                gl::GetUniformLocation(program, operand_rgb_name.as_ptr() as *const _),
                3,
                self.state.texture_operand_rgb[unit].as_ptr().cast(),
            );
            gl::Uniform1iv(
                gl::GetUniformLocation(program, operand_alpha_name.as_ptr() as *const _),
                3,
                self.state.texture_operand_alpha[unit].as_ptr().cast(),
            );
            gl::Uniform1f(
                gl::GetUniformLocation(program, rgb_scale_name.as_ptr() as *const _),
                self.state.texture_rgb_scale[unit],
            );
            gl::Uniform1f(
                gl::GetUniformLocation(program, alpha_scale_name.as_ptr() as *const _),
                self.state.texture_alpha_scale[unit],
            );
            gl::Uniform1i(
                gl::GetUniformLocation(program, enabled_name.as_ptr() as *const _),
                self.state.texture_enabled[unit] as GLint,
            );
            gl::Uniform1i(
                gl::GetUniformLocation(program, mode_name.as_ptr() as *const _),
                mode,
            );
            gl::Uniform4fv(
                gl::GetUniformLocation(program, env_name.as_ptr() as *const _),
                1,
                self.state.texture_env_color[unit].as_ptr(),
            );
            gl::Uniform1i(
                gl::GetUniformLocation(program, sampler_name.as_ptr() as *const _),
                unit as GLint,
            );
        }
        gl::ActiveTexture(gl::TEXTURE0 + self.state.active_texture as GLenum);
        gl::Uniform1i(
            gl::GetUniformLocation(program, b"u_logic_op_enabled\0".as_ptr() as *const _),
            self.state.logic_op_enabled as GLint,
        );
        gl::Uniform1i(
            gl::GetUniformLocation(program, b"u_logic_op\0".as_ptr() as *const _),
            self.state.logic_op as GLint,
        );
        let position = self.state.arrays[0];
        let color = self.state.arrays[1];
        let normal = self.state.arrays[2];
        let tex0 = self.state.texcoord_arrays[0];
        let tex1 = self.state.texcoord_arrays[1];
        let tex2 = self.state.texcoord_arrays[2];
        let tex3 = self.state.texcoord_arrays[3];
        self.bind_array_range(ATTR_POSITION, &position, first, count);
        self.bind_array_range(ATTR_COLOR, &color, first, count);
        self.bind_array_range(ATTR_NORMAL, &normal, first, count);
        self.bind_array_range(ATTR_TEX0, &tex0, first, count);
        self.bind_array_range(ATTR_TEX1, &tex1, first, count);
        self.bind_array_range(ATTR_TEX2, &tex2, first, count);
        self.bind_array_range(ATTR_TEX3, &tex3, first, count);
        let palette_index = self.state.palette_index_array;
        let palette_weight = self.state.palette_weight_array;
        let point_size_array = self.state.point_size_array;
        self.bind_array_range(ATTR_MATRIX_INDEX, &palette_index, first, count);
        self.bind_array_range(ATTR_WEIGHT, &palette_weight, first, count);
        self.bind_array_range(ATTR_POINT_SIZE, &point_size_array, first, count);
        gl::DrawArrays(mode, first, count);
        gl::BindBuffer(gl::ARRAY_BUFFER, self.state.array_buffer_binding);
    }
    unsafe fn DrawElements(
        &mut self,
        mode: GLenum,
        count: GLsizei,
        type_: GLenum,
        indices: *const GLvoid,
    ) {
        let program = match self.ensure_program() {
            Some(program) => program,
            None => return,
        };
        gl::UseProgram(program);
        let mvp = self.state.mvp();
        let mvp_loc = gl::GetUniformLocation(program, b"u_mvp\0".as_ptr() as *const _);
        gl::UniformMatrix4fv(mvp_loc, 1, gl::FALSE, mvp.as_ptr());
        let modelview_loc = gl::GetUniformLocation(program, b"u_modelview\0".as_ptr() as *const _);
        gl::UniformMatrix4fv(
            modelview_loc,
            1,
            gl::FALSE,
            self.state.modelview.current.as_ptr(),
        );
        let projection_loc =
            gl::GetUniformLocation(program, b"u_projection\0".as_ptr() as *const _);
        gl::UniformMatrix4fv(
            projection_loc,
            1,
            gl::FALSE,
            self.state.projection.current.as_ptr(),
        );
        let texture_matrix_loc =
            gl::GetUniformLocation(program, b"u_texture_matrix0\0".as_ptr() as *const _);
        gl::UniformMatrix4fv(
            texture_matrix_loc,
            1,
            gl::FALSE,
            self.state.texture[0].current.as_ptr(),
        );
        for unit in 1..MAX_TEXTURE_UNITS {
            let name = format!("u_texture_matrix{}\0", unit);
            gl::UniformMatrix4fv(
                gl::GetUniformLocation(program, name.as_ptr() as *const _),
                1,
                gl::FALSE,
                self.state.texture[unit].current.as_ptr(),
            );
        }
        let color_loc = gl::GetUniformLocation(program, b"u_color\0".as_ptr() as *const _);
        let color_uniform = if self.state.arrays[1].enabled {
            [1.0; 4]
        } else {
            self.state.color
        };
        gl::Uniform4fv(color_loc, 1, color_uniform.as_ptr());
        gl::Uniform1i(
            gl::GetUniformLocation(program, b"u_lighting_enabled\0".as_ptr() as *const _),
            self.state.lighting_enabled as GLint,
        );
        let mut light_enabled = [0; 8];
        let mut light_ambient = [[0.0; 4]; 8];
        let mut light_diffuse = [[0.0; 4]; 8];
        let mut light_specular = [[0.0; 4]; 8];
        let mut light_position = [[0.0; 4]; 8];
        let mut light_spot_direction = [[0.0; 3]; 8];
        let mut light_spot_cutoff = [0.0; 8];
        let mut light_spot_exponent = [0.0; 8];
        let mut light_constant_attenuation = [0.0; 8];
        let mut light_linear_attenuation = [0.0; 8];
        let mut light_quadratic_attenuation = [0.0; 8];
        for index in 0..8 {
            let light = self.state.lights[index];
            light_enabled[index] = self.state.light_enabled[index] as GLint;
            light_ambient[index] = light.ambient;
            light_diffuse[index] = light.diffuse;
            light_specular[index] = light.specular;
            light_position[index] = light.position;
            light_spot_direction[index] = light.spot_direction;
            light_spot_cutoff[index] = light.spot_cutoff;
            light_spot_exponent[index] = light.spot_exponent;
            light_constant_attenuation[index] = light.constant_attenuation;
            light_linear_attenuation[index] = light.linear_attenuation;
            light_quadratic_attenuation[index] = light.quadratic_attenuation;
        }
        gl::Uniform1iv(
            gl::GetUniformLocation(program, b"u_light_enabled\0".as_ptr() as *const _),
            8,
            light_enabled.as_ptr(),
        );
        gl::Uniform4fv(
            gl::GetUniformLocation(program, b"u_light_ambient\0".as_ptr() as *const _),
            8,
            light_ambient.as_ptr().cast(),
        );
        gl::Uniform4fv(
            gl::GetUniformLocation(program, b"u_light_diffuse\0".as_ptr() as *const _),
            8,
            light_diffuse.as_ptr().cast(),
        );
        gl::Uniform4fv(
            gl::GetUniformLocation(program, b"u_light_specular\0".as_ptr() as *const _),
            8,
            light_specular.as_ptr().cast(),
        );
        gl::Uniform4fv(
            gl::GetUniformLocation(program, b"u_light_position\0".as_ptr() as *const _),
            8,
            light_position.as_ptr().cast(),
        );
        gl::Uniform3fv(
            gl::GetUniformLocation(program, b"u_light_spot_direction\0".as_ptr() as *const _),
            8,
            light_spot_direction.as_ptr().cast(),
        );
        gl::Uniform1fv(
            gl::GetUniformLocation(program, b"u_light_spot_cutoff\0".as_ptr() as *const _),
            8,
            light_spot_cutoff.as_ptr(),
        );
        gl::Uniform1fv(
            gl::GetUniformLocation(program, b"u_light_spot_exponent\0".as_ptr() as *const _),
            8,
            light_spot_exponent.as_ptr(),
        );
        gl::Uniform1fv(
            gl::GetUniformLocation(
                program,
                b"u_light_constant_attenuation\0".as_ptr() as *const _,
            ),
            8,
            light_constant_attenuation.as_ptr(),
        );
        gl::Uniform1fv(
            gl::GetUniformLocation(
                program,
                b"u_light_linear_attenuation\0".as_ptr() as *const _,
            ),
            8,
            light_linear_attenuation.as_ptr(),
        );
        gl::Uniform1fv(
            gl::GetUniformLocation(
                program,
                b"u_light_quadratic_attenuation\0".as_ptr() as *const _,
            ),
            8,
            light_quadratic_attenuation.as_ptr(),
        );
        gl::Uniform1i(
            gl::GetUniformLocation(program, b"u_color_material_enabled\0".as_ptr() as *const _),
            self.state.color_material_enabled as GLint,
        );
        gl::Uniform1i(
            gl::GetUniformLocation(program, b"u_normalize_enabled\0".as_ptr() as *const _),
            self.state.normalize_enabled as GLint,
        );
        gl::Uniform4fv(
            gl::GetUniformLocation(program, b"u_material_ambient\0".as_ptr() as *const _),
            1,
            self.state.material_ambient.as_ptr(),
        );
        gl::Uniform4fv(
            gl::GetUniformLocation(program, b"u_material_diffuse\0".as_ptr() as *const _),
            1,
            self.state.material_diffuse.as_ptr(),
        );
        gl::Uniform4fv(
            gl::GetUniformLocation(program, b"u_material_specular\0".as_ptr() as *const _),
            1,
            self.state.material_specular.as_ptr(),
        );
        gl::Uniform4fv(
            gl::GetUniformLocation(program, b"u_material_emission\0".as_ptr() as *const _),
            1,
            self.state.material_emission.as_ptr(),
        );
        gl::Uniform1i(
            gl::GetUniformLocation(
                program,
                b"u_light_model_local_viewer\0".as_ptr() as *const _,
            ),
            self.state.light_model_local_viewer as GLint,
        );
        gl::Uniform1i(
            gl::GetUniformLocation(program, b"u_light_model_two_side\0".as_ptr() as *const _),
            self.state.light_model_two_side as GLint,
        );
        gl::Uniform1f(
            gl::GetUniformLocation(program, b"u_material_shininess\0".as_ptr() as *const _),
            self.state.material_shininess,
        );
        gl::Uniform4fv(
            gl::GetUniformLocation(program, b"u_model_ambient\0".as_ptr() as *const _),
            1,
            self.state.model_ambient.as_ptr(),
        );
        gl::Uniform4fv(
            gl::GetUniformLocation(program, b"u_clip_planes\0".as_ptr() as *const _),
            6,
            self.state.clip_planes.as_ptr().cast(),
        );
        gl::Uniform1iv(
            gl::GetUniformLocation(program, b"u_clip_enabled\0".as_ptr() as *const _),
            6,
            self.state.clip_plane_enabled.as_ptr().cast(),
        );
        gl::Uniform3fv(
            gl::GetUniformLocation(
                program,
                b"u_point_distance_attenuation\0".as_ptr() as *const _,
            ),
            1,
            self.state.point_distance_attenuation.as_ptr(),
        );
        gl::Uniform1f(
            gl::GetUniformLocation(program, b"u_point_fade_threshold\0".as_ptr() as *const _),
            self.state.point_fade_threshold,
        );
        let point_size_loc =
            gl::GetUniformLocation(program, b"u_point_size\0".as_ptr() as *const _);
        gl::Uniform1f(point_size_loc, self.state.point_size);
        for unit in 0..MAX_TEXTURE_UNITS {
            let enabled_name = format!("u_tex_enabled{}\0", unit);
            let mode_name = format!("u_tex_mode{}\0", unit);
            let env_name = format!("u_env_color{}\0", unit);
            let sampler_name = format!("u_tex{}\0", unit);
            gl::ActiveTexture(gl::TEXTURE0 + unit as GLenum);
            gl::BindTexture(gl::TEXTURE_2D, self.state.bound_textures[unit]);
            let mode = match self.state.texture_env_mode[unit] as GLenum {
                es1::REPLACE => 1,
                es1::ADD => 3,
                es1::DECAL => 4,
                es1::BLEND => 5,
                es1::COMBINE => 0,
                _ => 2,
            };
            let combine_name = format!("u_combine_rgb{}\0", unit);
            let combine_alpha_name = format!("u_combine_alpha{}\0", unit);
            let src_rgb_name = format!("u_src_rgb{}\0", unit);
            let src_alpha_name = format!("u_src_alpha{}\0", unit);
            let operand_rgb_name = format!("u_operand_rgb{}\0", unit);
            let operand_alpha_name = format!("u_operand_alpha{}\0", unit);
            let rgb_scale_name = format!("u_rgb_scale{}\0", unit);
            let alpha_scale_name = format!("u_alpha_scale{}\0", unit);
            gl::Uniform1i(
                gl::GetUniformLocation(program, combine_name.as_ptr() as *const _),
                self.state.texture_combine_rgb[unit] as GLint,
            );
            gl::Uniform1i(
                gl::GetUniformLocation(program, combine_alpha_name.as_ptr() as *const _),
                self.state.texture_combine_alpha[unit] as GLint,
            );
            gl::Uniform1iv(
                gl::GetUniformLocation(program, src_rgb_name.as_ptr() as *const _),
                3,
                self.state.texture_src_rgb[unit].as_ptr().cast(),
            );
            gl::Uniform1iv(
                gl::GetUniformLocation(program, src_alpha_name.as_ptr() as *const _),
                3,
                self.state.texture_src_alpha[unit].as_ptr().cast(),
            );
            gl::Uniform1iv(
                gl::GetUniformLocation(program, operand_rgb_name.as_ptr() as *const _),
                3,
                self.state.texture_operand_rgb[unit].as_ptr().cast(),
            );
            gl::Uniform1iv(
                gl::GetUniformLocation(program, operand_alpha_name.as_ptr() as *const _),
                3,
                self.state.texture_operand_alpha[unit].as_ptr().cast(),
            );
            gl::Uniform1f(
                gl::GetUniformLocation(program, rgb_scale_name.as_ptr() as *const _),
                self.state.texture_rgb_scale[unit],
            );
            gl::Uniform1f(
                gl::GetUniformLocation(program, alpha_scale_name.as_ptr() as *const _),
                self.state.texture_alpha_scale[unit],
            );
            gl::Uniform1i(
                gl::GetUniformLocation(program, enabled_name.as_ptr() as *const _),
                self.state.texture_enabled[unit] as GLint,
            );
            gl::Uniform1i(
                gl::GetUniformLocation(program, mode_name.as_ptr() as *const _),
                mode,
            );
            gl::Uniform4fv(
                gl::GetUniformLocation(program, env_name.as_ptr() as *const _),
                1,
                self.state.texture_env_color[unit].as_ptr(),
            );
            gl::Uniform1i(
                gl::GetUniformLocation(program, sampler_name.as_ptr() as *const _),
                unit as GLint,
            );
        }
        gl::ActiveTexture(gl::TEXTURE0 + self.state.active_texture as GLenum);
        let array_range = self
            .indexed_vertex_range(type_, indices, count)
            .unwrap_or((0, count));
        let position = self.state.arrays[0];
        let color = self.state.arrays[1];
        let normal = self.state.arrays[2];
        let tex0 = self.state.texcoord_arrays[0];
        let tex1 = self.state.texcoord_arrays[1];
        let tex2 = self.state.texcoord_arrays[2];
        let tex3 = self.state.texcoord_arrays[3];
        self.bind_array_range(ATTR_POSITION, &position, array_range.0, array_range.1);
        self.bind_array_range(ATTR_COLOR, &color, array_range.0, array_range.1);
        self.bind_array_range(ATTR_NORMAL, &normal, array_range.0, array_range.1);
        self.bind_array_range(ATTR_TEX0, &tex0, array_range.0, array_range.1);
        self.bind_array_range(ATTR_TEX1, &tex1, array_range.0, array_range.1);
        self.bind_array_range(ATTR_TEX2, &tex2, array_range.0, array_range.1);
        self.bind_array_range(ATTR_TEX3, &tex3, array_range.0, array_range.1);
        let palette_index = self.state.palette_index_array;
        let palette_weight = self.state.palette_weight_array;
        let point_size_array = self.state.point_size_array;
        self.bind_array_range(
            ATTR_MATRIX_INDEX,
            &palette_index,
            array_range.0,
            array_range.1,
        );
        self.bind_array_range(ATTR_WEIGHT, &palette_weight, array_range.0, array_range.1);
        self.bind_array_range(
            ATTR_POINT_SIZE,
            &point_size_array,
            array_range.0,
            array_range.1,
        );
        let (draw_indices, restore_element_buffer) =
            self.stage_client_indices(type_, indices, count);
        gl::DrawElements(mode, count, type_, draw_indices);
        if restore_element_buffer {
            gl::BindBuffer(
                gl::ELEMENT_ARRAY_BUFFER,
                self.state.element_array_buffer_binding,
            );
        }
        gl::BindBuffer(gl::ARRAY_BUFFER, self.state.array_buffer_binding);
    }
}

impl GLES1OnGLES2<'_> {
    fn transform_vec4(matrix: &[GLfloat; 16], value: [GLfloat; 4]) -> [GLfloat; 4] {
        [
            matrix[0] * value[0]
                + matrix[4] * value[1]
                + matrix[8] * value[2]
                + matrix[12] * value[3],
            matrix[1] * value[0]
                + matrix[5] * value[1]
                + matrix[9] * value[2]
                + matrix[13] * value[3],
            matrix[2] * value[0]
                + matrix[6] * value[1]
                + matrix[10] * value[2]
                + matrix[14] * value[3],
            matrix[3] * value[0]
                + matrix[7] * value[1]
                + matrix[11] * value[2]
                + matrix[15] * value[3],
        ]
    }

    fn light_index(light: GLenum) -> Option<usize> {
        (es1::LIGHT0..=es1::LIGHT7)
            .contains(&light)
            .then_some((light - es1::LIGHT0) as usize)
    }

    fn ensure_program(&mut self) -> Option<GLuint> {
        if let Some(program) = self.state.program {
            return Some(program);
        }
        match create_program(self.state.gles3) {
            Ok(program) => {
                self.state.program = Some(program);
                Some(program)
            }
            Err(error) => {
                if !self.state.program_creation_failed {
                    self.state.program_creation_failed = true;
                    unsafe {
                        let version = CStr::from_ptr(gl::GetString(gl::VERSION) as *const _);
                        let vendor = CStr::from_ptr(gl::GetString(gl::VENDOR) as *const _);
                        let renderer = CStr::from_ptr(gl::GetString(gl::RENDERER) as *const _);
                        log!(
                            "GLES1-on-GLES2 host: version={}, vendor={}, renderer={}",
                            version.to_string_lossy(),
                            vendor.to_string_lossy(),
                            renderer.to_string_lossy(),
                        );
                    }
                    log!(
                        "Error: GLES1-on-GLES2 shader program unavailable; texture_enabled={}, lighting_enabled={}, fog_enabled={}, alpha_test_enabled={}, matrix_palette_enabled={}; {}",
                        self.state.texture_enabled.iter().any(|enabled| *enabled),
                        self.state.lighting_enabled,
                        self.state.fog_enabled,
                        self.state.alpha_test_enabled,
                        self.state.matrix_palette_enabled,
                        error
                    );
                }
                None
            }
        }
    }

    unsafe fn indexed_vertex_range(
        &self,
        type_: GLenum,
        indices: *const GLvoid,
        count: GLsizei,
    ) -> Option<(GLint, GLsizei)> {
        if count <= 0 || indices.is_null() {
            return None;
        }
        let max_index = if self.state.element_array_buffer_binding != 0 {
            let index_size = match type_ {
                gl::UNSIGNED_BYTE => 1usize,
                gl::UNSIGNED_SHORT => 2,
                gl::UNSIGNED_INT => 4,
                _ => return None,
            };
            let bytes = self
                .state
                .element_array_buffer_data
                .get(&self.state.element_array_buffer_binding)?;
            let offset = indices as usize;
            let byte_count = (count as usize).checked_mul(index_size)?;
            let end = offset.checked_add(byte_count)?;
            if end > bytes.len() {
                return None;
            }
            (0..count as usize)
                .map(|i| {
                    let at = offset + i * index_size;
                    match type_ {
                        gl::UNSIGNED_BYTE => Some(bytes[at] as usize),
                        gl::UNSIGNED_SHORT => {
                            Some(u16::from_ne_bytes([bytes[at], bytes[at + 1]]) as usize)
                        }
                        gl::UNSIGNED_INT => Some(u32::from_ne_bytes([
                            bytes[at],
                            bytes[at + 1],
                            bytes[at + 2],
                            bytes[at + 3],
                        ]) as usize),
                        _ => None,
                    }
                })
                .collect::<Option<Vec<_>>>()?
                .into_iter()
                .max()?
        } else {
            (0..count as usize)
                .map(|i| match type_ {
                    gl::UNSIGNED_BYTE => {
                        Some((indices.cast::<u8>().add(i)).read_unaligned() as usize)
                    }
                    gl::UNSIGNED_SHORT => {
                        Some((indices.cast::<u16>().add(i)).read_unaligned() as usize)
                    }
                    gl::UNSIGNED_INT => {
                        Some((indices.cast::<u32>().add(i)).read_unaligned() as usize)
                    }
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()?
                .into_iter()
                .max()?
        };
        Some((0, max_index.checked_add(1)?.try_into().ok()?))
    }

    unsafe fn stage_client_indices(
        &mut self,
        type_: GLenum,
        indices: *const GLvoid,
        count: GLsizei,
    ) -> (*const GLvoid, bool) {
        if self.state.element_array_buffer_binding != 0 || indices.is_null() || count <= 0 {
            return (indices, false);
        }
        let index_size = match type_ {
            gl::UNSIGNED_BYTE => 1usize,
            gl::UNSIGNED_SHORT => 2usize,
            gl::UNSIGNED_INT => 4usize,
            _ => return (indices, false),
        };
        let byte_count = (count as usize).saturating_mul(index_size);
        if byte_count == 0 {
            return (indices, false);
        }
        if self.state.client_element_vbo == 0 {
            gl::GenBuffers(1, &mut self.state.client_element_vbo);
        }
        gl::BindBuffer(gl::ELEMENT_ARRAY_BUFFER, self.state.client_element_vbo);
        gl::BufferData(
            gl::ELEMENT_ARRAY_BUFFER,
            byte_count as GLsizeiptr,
            indices,
            gl::STREAM_DRAW,
        );
        (std::ptr::null(), true)
    }

    unsafe fn bind_array(&mut self, index: GLuint, array: &ArrayState) {
        self.bind_array_range(index, array, 0, 0);
    }

    unsafe fn bind_array_range(
        &mut self,
        index: GLuint,
        array: &ArrayState,
        first: GLint,
        count: GLsizei,
    ) {
        if !array.enabled {
            gl::DisableVertexAttribArray(index);
            let value = if index == ATTR_COLOR {
                [1.0, 1.0, 1.0, 1.0]
            } else if index == ATTR_TEX0 {
                self.state.texcoords[0]
            } else if index == ATTR_TEX1 {
                self.state.texcoords[1]
            } else if index == ATTR_TEX2 {
                self.state.texcoords[2]
            } else if index == ATTR_TEX3 {
                self.state.texcoords[3]
            } else if index == ATTR_NORMAL {
                [
                    self.state.normal[0],
                    self.state.normal[1],
                    self.state.normal[2],
                    1.0,
                ]
            } else {
                [0.0, 0.0, 0.0, 1.0]
            };
            gl::VertexAttrib4fv(index, value.as_ptr());
            return;
        }
        if array.buffer_binding != 0 {
            gl::BindBuffer(gl::ARRAY_BUFFER, array.buffer_binding);
            if array.type_ != gl::FIXED {
                gl::EnableVertexAttribArray(index);
                gl::VertexAttribPointer(
                    index,
                    array.size,
                    array.type_,
                    if array.normalized {
                        gl::TRUE
                    } else {
                        gl::FALSE
                    },
                    array.stride,
                    array.pointer,
                );
                return;
            }
            let bytes = match self.state.array_buffer_data.get(&array.buffer_binding) {
                Some(bytes) => bytes.clone(),
                None => {
                    gl::DisableVertexAttribArray(index);
                    return;
                }
            };
            let components = array.size as usize;
            let stride = if array.stride > 0 {
                array.stride as usize
            } else {
                components * 4
            };
            let first = first.max(0) as usize;
            let count = count as usize;
            let upload_count = first.saturating_add(count);
            let offset = array.pointer as usize;
            let byte_count = upload_count
                .saturating_sub(1)
                .saturating_mul(stride)
                .saturating_add(components * 4);
            let end = match offset.checked_add(byte_count) {
                Some(end) if end <= bytes.len() => end,
                _ => {
                    gl::DisableVertexAttribArray(index);
                    return;
                }
            };
            let mut converted = Vec::with_capacity(byte_count / 4 * std::mem::size_of::<GLfloat>());
            for vertex in 0..upload_count {
                let source = bytes.as_ptr().add(offset + vertex.saturating_mul(stride));
                for component in 0..components {
                    let value = source.add(component * 4).cast::<GLfixed>().read_unaligned();
                    converted.extend_from_slice(&fixed_to_float(value).to_ne_bytes());
                }
            }
            let _ = end;
            let vbo_slot = (index as usize).min(self.state.client_array_vbos.len() - 1);
            if self.state.client_array_vbos[vbo_slot] == 0 {
                gl::GenBuffers(1, &mut self.state.client_array_vbos[vbo_slot]);
            }
            let vbo = self.state.client_array_vbos[vbo_slot];
            gl::BindBuffer(gl::ARRAY_BUFFER, vbo);
            gl::BufferData(
                gl::ARRAY_BUFFER,
                converted.len() as GLsizeiptr,
                converted.as_ptr().cast(),
                gl::STREAM_DRAW,
            );
            gl::EnableVertexAttribArray(index);
            gl::VertexAttribPointer(
                index,
                array.size,
                gl::FLOAT,
                if array.normalized {
                    gl::TRUE
                } else {
                    gl::FALSE
                },
                (components * 4) as GLsizei,
                std::ptr::null(),
            );
            return;
        }
        if array.pointer.is_null() || count <= 0 || array.size <= 0 {
            gl::DisableVertexAttribArray(index);
            return;
        }
        let component_size = match array.type_ {
            gl::BYTE | gl::UNSIGNED_BYTE => 1usize,
            gl::SHORT | gl::UNSIGNED_SHORT => 2usize,
            gl::FIXED | gl::FLOAT => 4usize,
            _ => {
                gl::DisableVertexAttribArray(index);
                return;
            }
        };
        let components = array.size as usize;
        let stride = if array.stride > 0 {
            array.stride as usize
        } else {
            components * component_size
        };
        let first = first.max(0) as usize;
        let count = count as usize;
        let upload_count = first.saturating_add(count);
        let byte_count = upload_count
            .saturating_sub(1)
            .saturating_mul(stride)
            .saturating_add(components * component_size);
        let vbo_slot = (index as usize).min(self.state.client_array_vbos.len() - 1);
        if self.state.client_array_vbos[vbo_slot] == 0 {
            gl::GenBuffers(1, &mut self.state.client_array_vbos[vbo_slot]);
        }
        let vbo = self.state.client_array_vbos[vbo_slot];
        gl::BindBuffer(gl::ARRAY_BUFFER, vbo);
        if array.type_ == gl::FIXED {
            let mut converted =
                Vec::with_capacity(upload_count * components * std::mem::size_of::<GLfloat>());
            for vertex in 0..upload_count {
                let source = (array.pointer as *const u8).add(vertex.saturating_mul(stride));
                for component in 0..components {
                    let value = (source.add(component * 4) as *const GLfixed).read_unaligned();
                    converted.extend_from_slice(&fixed_to_float(value).to_ne_bytes());
                }
            }
            gl::BufferData(
                gl::ARRAY_BUFFER,
                converted.len() as GLsizeiptr,
                converted.as_ptr().cast(),
                gl::STREAM_DRAW,
            );
            gl::EnableVertexAttribArray(index);
            gl::VertexAttribPointer(
                index,
                array.size,
                gl::FLOAT,
                if array.normalized {
                    gl::TRUE
                } else {
                    gl::FALSE
                },
                (components * std::mem::size_of::<GLfloat>()) as GLsizei,
                std::ptr::null(),
            );
        } else {
            let source = array.pointer as *const u8;
            gl::BufferData(
                gl::ARRAY_BUFFER,
                byte_count as GLsizeiptr,
                source.cast(),
                gl::STREAM_DRAW,
            );
            gl::EnableVertexAttribArray(index);
            gl::VertexAttribPointer(
                index,
                array.size,
                array.type_,
                if array.normalized {
                    gl::TRUE
                } else {
                    gl::FALSE
                },
                array.stride,
                std::ptr::null(),
            );
        }
        if index == ATTR_POSITION {
            log_once!("GLES1-on-GLES2: uploaded client-side vertex arrays to a host VBO for GLES2 compatibility");
        }
    }
}
