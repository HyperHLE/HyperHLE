/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Shared helpers for recreating the authentic iOS 5 (skeuomorphic) UIKit
//! appearance.
//!
//! iOS 5 controls are heavily *skeuomorphic*: bars and buttons are drawn with
//! a vertical two-part gloss gradient, a bright 1px highlight along the top
//! edge and a dark 1px shadow line along the bottom edge. This module provides
//! reusable primitives so the individual UIKit control implementations can
//! render that look consistently.
//!
//! The emulator's Core Graphics rasteriser only reliably rasterises axis
//! aligned rectangle fills and rectangle strokes (paths and real ellipses are
//! approximated). To stay within those guarantees, every gradient here is
//! synthesised as a stack of 1pt-tall rectangle "strips" whose colour is
//! linearly interpolated between the stops. This keeps the code fully
//! compatible with the existing renderer while still producing smooth
//! gradients on both Retina and non-Retina backing stores (the backing store
//! resolution is handled transparently by the context).

use crate::frameworks::core_graphics::cg_context::{
    CGContextFillRect, CGContextRef, CGContextRestoreGState, CGContextSaveGState,
    CGContextSetRGBFillColor,
};
use crate::frameworks::core_graphics::{CGFloat, CGPoint, CGRect, CGSize};
use crate::Environment;

/// A straight RGBA colour, components in the `0.0..=1.0` range.
pub type Rgba = (CGFloat, CGFloat, CGFloat, CGFloat);

#[inline]
fn lerp(a: CGFloat, b: CGFloat, t: CGFloat) -> CGFloat {
    a + (b - a) * t
}

#[inline]
fn lerp_rgba(a: Rgba, b: Rgba, t: CGFloat) -> Rgba {
    (
        lerp(a.0, b.0, t),
        lerp(a.1, b.1, t),
        lerp(a.2, b.2, t),
        lerp(a.3, b.3, t),
    )
}

/// Clamp a component to the representable colour range.
#[inline]
fn clamp01(x: CGFloat) -> CGFloat {
    x.clamp(0.0, 1.0)
}

/// Multiply the lightness of a colour by `factor` (values > 1 brighten,
/// < 1 darken). Alpha is preserved. Used to derive gloss stops from a single
/// bar tint colour.
#[inline]
pub fn scale_brightness(c: Rgba, factor: CGFloat) -> Rgba {
    (
        clamp01(c.0 * factor),
        clamp01(c.1 * factor),
        clamp01(c.2 * factor),
        c.3,
    )
}

/// Fill `rect` with a solid colour.
pub fn fill_solid(env: &mut Environment, ctx: CGContextRef, rect: CGRect, color: Rgba) {
    if ctx.is_null() || rect.size.width <= 0.0 || rect.size.height <= 0.0 {
        return;
    }
    CGContextSetRGBFillColor(env, ctx, color.0, color.1, color.2, color.3);
    CGContextFillRect(env, ctx, rect);
}

/// Fill `rect` with a smooth top-to-bottom linear gradient between `top` and
/// `bottom`. Rendered as a stack of 1pt strips (see the module docs).
pub fn fill_vertical_gradient(
    env: &mut Environment,
    ctx: CGContextRef,
    rect: CGRect,
    top: Rgba,
    bottom: Rgba,
) {
    if ctx.is_null() || rect.size.width <= 0.0 || rect.size.height <= 0.0 {
        return;
    }
    let steps = rect.size.height.ceil().max(1.0) as i32;
    for i in 0..steps {
        let t = if steps > 1 {
            i as CGFloat / (steps - 1) as CGFloat
        } else {
            0.0
        };
        let (r, g, b, a) = lerp_rgba(top, bottom, t);
        CGContextSetRGBFillColor(env, ctx, r, g, b, a);
        let strip = CGRect {
            origin: CGPoint {
                x: rect.origin.x,
                y: rect.origin.y + i as CGFloat,
            },
            size: CGSize {
                width: rect.size.width,
                // Slightly overlap so no seams appear between strips.
                height: 1.0,
            },
        };
        CGContextFillRect(env, ctx, strip);
    }
}

/// Draw a horizontal hairline (1pt tall by default) at `y` spanning the width
/// of `rect`. Used for the bright highlight / dark shadow edges of bars.
pub fn horizontal_line(
    env: &mut Environment,
    ctx: CGContextRef,
    rect: CGRect,
    y: CGFloat,
    color: Rgba,
    thickness: CGFloat,
) {
    fill_solid(
        env,
        ctx,
        CGRect {
            origin: CGPoint { x: rect.origin.x, y },
            size: CGSize {
                width: rect.size.width,
                height: thickness,
            },
        },
        color,
    );
}

/// The colour recipe for an iOS 5 bar (`UINavigationBar`, `UIToolbar`,
/// `UITabBar`). All values are hand-tuned to match iOS 5 screenshots.
#[derive(Clone, Copy)]
pub struct BarPalette {
    /// Bright highlight painted along the very top edge (1px).
    pub top_highlight: Rgba,
    /// Gradient stops for the upper (glossier) half of the bar.
    pub upper_top: Rgba,
    pub upper_bottom: Rgba,
    /// Gradient stops for the lower half of the bar.
    pub lower_top: Rgba,
    pub lower_bottom: Rgba,
    /// Dark shadow line painted along the very bottom edge (1px).
    pub bottom_shadow: Rgba,
}

impl BarPalette {
    /// The default blue-grey metallic tint used by `UINavigationBar` and
    /// `UIToolbar` on iOS 5.
    pub fn navigation_default() -> Self {
        BarPalette {
            top_highlight: (0.72, 0.76, 0.83, 1.0),
            upper_top: (0.56, 0.61, 0.70, 1.0),
            upper_bottom: (0.44, 0.51, 0.62, 1.0),
            lower_top: (0.40, 0.47, 0.59, 1.0),
            lower_bottom: (0.30, 0.37, 0.50, 1.0),
            bottom_shadow: (0.16, 0.20, 0.28, 1.0),
        }
    }

    /// `UIBarStyleBlack` bars (also the base for the `UITabBar`).
    pub fn black() -> Self {
        BarPalette {
            top_highlight: (0.34, 0.34, 0.36, 1.0),
            upper_top: (0.22, 0.22, 0.24, 1.0),
            upper_bottom: (0.13, 0.13, 0.15, 1.0),
            lower_top: (0.11, 0.11, 0.12, 1.0),
            lower_bottom: (0.02, 0.02, 0.03, 1.0),
            bottom_shadow: (0.0, 0.0, 0.0, 1.0),
        }
    }

    /// The light-grey gradient used behind a `UISearchBar` on iOS 5.
    pub fn search_bar() -> Self {
        BarPalette {
            top_highlight: (0.85, 0.86, 0.88, 1.0),
            upper_top: (0.74, 0.75, 0.78, 1.0),
            upper_bottom: (0.66, 0.68, 0.71, 1.0),
            lower_top: (0.62, 0.64, 0.67, 1.0),
            lower_bottom: (0.54, 0.56, 0.60, 1.0),
            bottom_shadow: (0.40, 0.42, 0.46, 1.0),
        }
    }

    /// The dark, glossy `UITabBar` background.
    pub fn tab_bar() -> Self {
        BarPalette {
            top_highlight: (0.40, 0.40, 0.42, 1.0),
            upper_top: (0.25, 0.25, 0.27, 1.0),
            upper_bottom: (0.15, 0.15, 0.16, 1.0),
            lower_top: (0.12, 0.12, 0.13, 1.0),
            lower_bottom: (0.03, 0.03, 0.04, 1.0),
            bottom_shadow: (0.0, 0.0, 0.0, 1.0),
        }
    }

    /// Derive a glossy palette from a single flat bar tint colour, mirroring
    /// how UIKit lightens/darkens `barTintColor`/`tintColor` to build the
    /// gradient.
    pub fn from_tint(tint: Rgba) -> Self {
        BarPalette {
            top_highlight: scale_brightness(tint, 1.45),
            upper_top: scale_brightness(tint, 1.18),
            upper_bottom: scale_brightness(tint, 1.02),
            lower_top: scale_brightness(tint, 0.92),
            lower_bottom: scale_brightness(tint, 0.72),
            bottom_shadow: scale_brightness(tint, 0.45),
        }
    }

    /// Apply an alpha multiplier to every stop (used for translucent bars).
    pub fn with_alpha(mut self, alpha: CGFloat) -> Self {
        for c in [
            &mut self.top_highlight,
            &mut self.upper_top,
            &mut self.upper_bottom,
            &mut self.lower_top,
            &mut self.lower_bottom,
            &mut self.bottom_shadow,
        ] {
            c.3 *= alpha;
        }
        self
    }
}

/// Render the full iOS 5 bar chrome (two-part gloss gradient plus top
/// highlight and bottom shadow lines) into `rect`.
pub fn draw_bar_background(
    env: &mut Environment,
    ctx: CGContextRef,
    rect: CGRect,
    palette: BarPalette,
) {
    if ctx.is_null() || rect.size.width <= 0.0 || rect.size.height <= 0.0 {
        return;
    }

    CGContextSaveGState(env, ctx);

    let mid = (rect.size.height / 2.0).floor();
    let upper = CGRect {
        origin: rect.origin,
        size: CGSize {
            width: rect.size.width,
            height: mid,
        },
    };
    let lower = CGRect {
        origin: CGPoint {
            x: rect.origin.x,
            y: rect.origin.y + mid,
        },
        size: CGSize {
            width: rect.size.width,
            height: rect.size.height - mid,
        },
    };

    fill_vertical_gradient(env, ctx, upper, palette.upper_top, palette.upper_bottom);
    fill_vertical_gradient(env, ctx, lower, palette.lower_top, palette.lower_bottom);

    // Bright highlight along the top edge.
    horizontal_line(env, ctx, rect, rect.origin.y, palette.top_highlight, 1.0);
    // Dark shadow along the bottom edge.
    horizontal_line(
        env,
        ctx,
        rect,
        rect.origin.y + rect.size.height - 1.0,
        palette.bottom_shadow,
        1.0,
    );

    CGContextRestoreGState(env, ctx);
}

/// Render an iOS 5 style glossy pill/button background inside `rect`. The
/// renderer cannot rasterise rounded corners into a bitmap, so callers that
/// need rounded corners should additionally set their layer's `cornerRadius`
/// (which the compositor rounds); this fills the glossy body.
pub fn draw_glossy_button(
    env: &mut Environment,
    ctx: CGContextRef,
    rect: CGRect,
    base: Rgba,
    highlighted: bool,
) {
    if ctx.is_null() || rect.size.width <= 0.0 || rect.size.height <= 0.0 {
        return;
    }

    CGContextSaveGState(env, ctx);

    let factor = if highlighted { 0.82 } else { 1.0 };
    let top = scale_brightness(base, 1.28 * factor);
    let upper_bottom = scale_brightness(base, 1.06 * factor);
    let lower_top = scale_brightness(base, 0.98 * factor);
    let bottom = scale_brightness(base, 0.80 * factor);

    let mid = (rect.size.height / 2.0).floor();
    let upper = CGRect {
        origin: rect.origin,
        size: CGSize {
            width: rect.size.width,
            height: mid,
        },
    };
    let lower = CGRect {
        origin: CGPoint {
            x: rect.origin.x,
            y: rect.origin.y + mid,
        },
        size: CGSize {
            width: rect.size.width,
            height: rect.size.height - mid,
        },
    };
    fill_vertical_gradient(env, ctx, upper, top, upper_bottom);
    fill_vertical_gradient(env, ctx, lower, lower_top, bottom);

    // Inner top highlight for the glass "shine".
    horizontal_line(
        env,
        ctx,
        rect,
        rect.origin.y,
        scale_brightness(base, 1.5 * factor),
        1.0,
    );

    CGContextRestoreGState(env, ctx);
}
