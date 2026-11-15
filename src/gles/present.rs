/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Utilities for presenting frames to the window using an abstract OpenGL ES
//! implementation.

use super::gles11_raw as gles11; // constants and types only
use super::GLES;
use crate::matrix::Matrix;
use std::time::{Duration, Instant};

use std::sync::atomic::{AtomicU64, Ordering};

pub struct FpsCounter {
    time: Instant,
    frames: u32,
}

// DIAG (TOUCHHLE_STALL_DIAG): epoch-millis of the last presented frame, updated
// at the top of present_frame. The main run loop reads this to detect a guest
// hang (frames stop being presented while the guest keeps burning CPU) and dump
// the spinning thread's stack. 0 = nothing presented yet.
static LAST_PRESENT_MS: AtomicU64 = AtomicU64::new(0);

fn epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Milliseconds since the last presented frame, or `None` if no frame has been
/// presented yet.
pub fn ms_since_last_present() -> Option<u64> {
    let last = LAST_PRESENT_MS.load(Ordering::Relaxed);
    if last == 0 {
        None
    } else {
        Some(epoch_ms().saturating_sub(last))
    }
}

impl FpsCounter {
    pub fn start() -> Self {
        FpsCounter {
            time: Instant::now(),
            frames: 0,
        }
    }

    pub fn count_frame(&mut self, label: std::fmt::Arguments<'_>, log_output: bool) {
        self.frames += 1;
        let now = Instant::now();
        let duration = now - self.time;
        if duration >= Duration::from_secs(1) {
            self.time = now;
            let fps = std::mem::take(&mut self.frames) as f32 / duration.as_secs_f32();
            #[cfg(target_os = "ios")]
            crate::report_host_fps(fps);
            if log_output {
                echo!("touchHLE: {} FPS: {:.2}", label, fps);
            }
        }
    }
}

pub fn onscreen_fps_enabled() -> bool {
    crate::gles::perf_overlay::enabled()
}

pub fn cycle_onscreen_fps_corner() -> &'static str {
    crate::gles::perf_overlay::cycle_corner()
}

/// Present the the latest frame (e.g. the app's splash screen or rendering
/// output), provided as a texture bound to `GL_TEXTURE_2D`, by drawing it on
/// the window. It may be rotated, scaled and/or letterboxed as necessary. The
/// virtual cursor is also drawn if it should be currently visible.
///
/// The provided context must be current.
pub unsafe fn present_frame(
    gles: &mut dyn GLES,
    viewport: (u32, u32, u32, u32),
    rotation_matrix: Matrix<2>,
    virtual_cursor_visible_at: Option<(f32, f32, bool)>,
    context_token: usize,
) {
    // While this is a generic utility, it is closely tied to
    // crate::frameworks::opengles::eagl::present_renderbuffer, which handles
    // backing up and restoring OpenGL ES state that this function might touch,
    // so these need to be updated in tandem.

    use gles11::types::*;

    // DIAG: record that a frame reached the present path.
    LAST_PRESENT_MS.store(epoch_ms(), Ordering::Relaxed);

    // Draw the quad
    gles.Viewport(
        viewport.0 as _,
        viewport.1 as _,
        viewport.2 as _,
        viewport.3 as _,
    );
    gles.ClearColor(0.0, 0.0, 0.0, 1.0);
    // PERF: only the color buffer needs clearing here. The present quad runs
    // with depth/stencil testing disabled (the caller disables caps before
    // drawing), so clearing DEPTH|STENCIL is a wasted full-viewport pass —
    // noticeable at scale-hack 4x on software rasterizers.
    gles.Clear(gles11::COLOR_BUFFER_BIT);
    // Keep the window's alpha channel at the 1.0 the clear just wrote. The
    // frame texture's alpha is meaningless for an opaque CAEAGLLayer, but a
    // window surface with an alpha channel may be blended by the OS
    // compositor (Android's SurfaceFlinger), which would show through
    // wherever the app left alpha < 1.
    gles.ColorMask(gles11::TRUE, gles11::TRUE, gles11::TRUE, gles11::FALSE);
    gles.BindBuffer(gles11::ARRAY_BUFFER, 0);
    // Stretch the full rendered frame to fill the active host viewport.
    //
    // This does NOT crop or shift the texture. The whole renderbuffer is
    // sampled from normal 0..1 texture coordinates and mapped to a full-screen
    // quad. This is the correct "fill the current window" behavior for
    // PotatoGold-style landscape tests.
    // PERF: cached read-once flag; present_frame runs every frame.
    if crate::env_flag_cached!("TOUCHHLE_PRESENT_STRETCH_TO_VIEWPORT") {
        log_once!(
            "TOUCHHLE_PRESENT_STRETCH_TO_VIEWPORT=1: stretching full rendered frame to the active viewport [this log will only be shown once]"
        );
    }

    let vertices: [f32; 12] = [
        -1.0, -1.0, -1.0, 1.0, 1.0, -1.0, 1.0, -1.0, -1.0, 1.0, 1.0, 1.0,
    ];
    gles.EnableClientState(gles11::VERTEX_ARRAY);
    gles.VertexPointer(2, gles11::FLOAT, 0, vertices.as_ptr() as *const GLvoid);
    #[allow(unused_mut)]
    let mut tex_coords: [f32; 12] = [0.0, 0.0, 0.0, 1.0, 1.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0];
    #[cfg(target_os = "ios")]
    for coords in tex_coords.chunks_exact_mut(2) {
        let transformed = rotation_matrix.transform([coords[0] - 0.5, coords[1] - 0.5]);
        coords[0] = transformed[0] + 0.5;
        coords[1] = transformed[1] + 0.5;
    }
    gles.EnableClientState(gles11::TEXTURE_COORD_ARRAY);
    gles.TexCoordPointer(2, gles11::FLOAT, 0, tex_coords.as_ptr() as *const GLvoid);
    #[cfg(not(target_os = "ios"))]
    {
    // Apply the device-rotation matrix to the TEXTURE matrix, but rotate
    // around the centre of the tex coord square (0.5, 0.5) instead of the
    // origin. The naive `LoadMatrixf(rotation_matrix)` rotates around (0, 0),
    // which sends standard [0, 1]² UVs out of range — e.g. for a 90° rotation
    // (v, -u) reaches v' = -u ∈ [-1, 0]. On lenient drivers (Mesa, Apple
    // PowerVR) GL_REPEAT wrap quietly maps that back into [0, 1], but
    // strict drivers (Qualcomm Adreno's native ES 1.1 path) treat the
    // resulting sample of an NPOT texture (renderbuffer is typically
    // 320x480) as undefined and produce a mangled / black presented
    // frame. Pre- and post-translating by (0.5, 0.5) keeps tex coords in
    // [0, 1]² for any 90°/180°/270°/identity device rotation while
    // producing the same visual output as before on lenient drivers.
    let r = Matrix::<4>::from(&rotation_matrix);
    let to_center = Matrix::<4>::translate_3d(-0.5, -0.5, 0.0);
    let from_center = Matrix::<4>::translate_3d(0.5, 0.5, 0.0);
    // Note: Matrix::multiply(&other) computes `other · self` in
    // linear-algebra terms (other is applied AFTER self), so to get
    // `from_center · r · to_center` we chain in the order
    // to_center.multiply(&r).multiply(&from_center).
    let centered_rotation = to_center.multiply(&r).multiply(&from_center);
    gles.MatrixMode(gles11::TEXTURE);
    gles.LoadMatrixf(centered_rotation.columns().as_ptr() as *const _);
    }
    #[cfg(target_os = "ios")]
    {
    gles.MatrixMode(gles11::TEXTURE);
    gles.LoadIdentity();
    }
    gles.Enable(gles11::TEXTURE_2D);
    gles.DrawArrays(gles11::TRIANGLES, 0, 6);
    // clean this up so we don't need to worry about it in e.g. Core Animation
    gles.LoadIdentity();

    // Display virtual cursor
    if let Some((x, y, pressed)) = virtual_cursor_visible_at {
        let (vx, vy, vw, vh) = viewport;
        let x = x - vx as f32;
        let y = y - vy as f32;

        gles.DisableClientState(gles11::TEXTURE_COORD_ARRAY);
        gles.Disable(gles11::TEXTURE_2D);

        gles.Enable(gles11::BLEND);
        gles.BlendFunc(gles11::ONE, gles11::ONE_MINUS_SRC_ALPHA);
        gles.Color4f(0.0, 0.0, 0.0, if pressed { 2.0 / 3.0 } else { 1.0 / 3.0 });

        let radius = 10.0;

        let mut vertices = vertices;
        for i in (0..vertices.len()).step_by(2) {
            vertices[i] = (vertices[i] * radius + x) / (vw as f32 / 2.0) - 1.0;
            vertices[i + 1] = 1.0 - (vertices[i + 1] * radius + y) / (vh as f32 / 2.0);
        }
        gles.VertexPointer(2, gles11::FLOAT, 0, vertices.as_ptr() as *const GLvoid);
        gles.DrawArrays(gles11::TRIANGLES, 0, 6);
    }

    if onscreen_fps_enabled() {
        crate::gles::perf_overlay::draw(gles, viewport, context_token);
    }

    // Cheat Engine-style trainer overlay (floating button + panel). Drawn
    // last so it sits on top of everything, including the performance display.
    crate::trainer_ui::draw(gles, viewport);
    // Quick menu button + panel (top-left corner).
    crate::quick_menu::draw(gles, viewport);

    gles.ColorMask(gles11::TRUE, gles11::TRUE, gles11::TRUE, gles11::TRUE);
}
