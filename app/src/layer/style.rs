//! Colour tokens and sizes for the layer, defined in one place (no component
//! hard-codes a colour). Chrome follows the light or dark theme; anything that belongs to the
//! picture (scrim, halos, cutout colours, the black canvas) is the same in both.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::engine::draw::Rgba;
use crate::model::layout::CutoutColor;

static LIGHT: AtomicBool = AtomicBool::new(false);

/// Switch the chrome to the light or dark theme.
pub fn set_light(on: bool) {
    LIGHT.store(on, Ordering::Relaxed);
}

pub fn light() -> bool {
    LIGHT.load(Ordering::Relaxed)
}

fn pick(dark: Rgba, light_: Rgba) -> Rgba {
    if light() { light_ } else { dark }
}

/// Glass surface: near-black at about 80% in dark, near-white at about 88% in light. (No
/// background blur yet; the extra opacity keeps text readable over busy pictures.)
pub fn glass() -> Rgba {
    pick([0.07, 0.07, 0.08, 0.86], [0.97, 0.97, 0.98, 0.9])
}
pub fn glass_hover() -> Rgba {
    pick([0.22, 0.22, 0.25, 0.95], [0.86, 0.87, 0.9, 0.97])
}
pub fn hairline() -> Rgba {
    pick([1.0, 1.0, 1.0, 0.14], [0.0, 0.0, 0.0, 0.12])
}
/// Text and icons.
pub fn fg() -> Rgba {
    pick([1.0, 1.0, 1.0, 0.92], [0.06, 0.06, 0.07, 0.9])
}
pub fn fg_dim() -> Rgba {
    pick([1.0, 1.0, 1.0, 0.35], [0.0, 0.0, 0.0, 0.32])
}
pub fn fg_muted() -> Rgba {
    pick([1.0, 1.0, 1.0, 0.62], [0.0, 0.0, 0.0, 0.6])
}
pub fn keycap_fill() -> Rgba {
    pick([1.0, 1.0, 1.0, 0.12], [0.0, 0.0, 0.0, 0.07])
}
pub fn accent() -> Rgba {
    pick([0.13, 0.83, 0.93, 1.0], [0.03, 0.57, 0.7, 1.0])
}
pub fn accent_fill() -> Rgba {
    pick([0.13, 0.83, 0.93, 0.28], [0.03, 0.57, 0.7, 0.2])
}
pub fn danger() -> Rgba {
    pick([0.94, 0.3, 0.3, 1.0], [0.8, 0.15, 0.15, 1.0])
}

/// A 1 px contrasting halo under outlines, so they read over any picture in either theme.
pub const HALO: Rgba = [0.0, 0.0, 0.0, 0.7];
pub const SNAP: Rgba = [1.0, 1.0, 1.0, 0.9];
/// Dark text for use on a cutout colour (the number chip).
pub const ON_COLOR: Rgba = [0.05, 0.05, 0.06, 1.0];

/// Brightness of the source outside cutouts in Cut view: a 55% black scrim.
pub const SCRIM_DIM: f32 = 0.45;

pub fn cutout(c: CutoutColor) -> Rgba {
    match c {
        CutoutColor::Cyan => [0.13, 0.83, 0.93, 1.0],
        CutoutColor::Amber => [0.98, 0.75, 0.14, 1.0],
        CutoutColor::Magenta => [0.93, 0.33, 0.66, 1.0],
        CutoutColor::Lime => [0.55, 0.85, 0.15, 1.0],
        CutoutColor::Orange => [0.98, 0.5, 0.12, 1.0],
        CutoutColor::Violet => [0.62, 0.45, 0.98, 1.0],
        CutoutColor::Red => [0.95, 0.3, 0.3, 1.0],
        CutoutColor::Teal => [0.1, 0.78, 0.68, 1.0],
    }
}
