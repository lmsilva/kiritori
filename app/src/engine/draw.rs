//! What the engine draws: a list of items per display, built by the shell from the app's state.
//! Coordinates are pixels on that display's canvas, origin at its top-left.

use crate::model::geometry::Rect;
use crate::model::layout::{Filter, ShapeRect};

/// x, y, w, h in canvas pixels.
pub type Px = (f32, f32, f32, f32);
/// Straight (not premultiplied) red, green, blue, alpha, 0–1.
pub type Rgba = [f32; 4];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Draw {
    /// Part of the source picture. `src` is the region in source fractions, drawn into `dst`;
    /// `shape` limits it to an add/subtract shape (empty: the whole region).
    Source {
        /// Rotation and mirroring (`Cutout::orient`): 0 draws the region as it is.
        orient: u8,
        dst: Px,
        src: Rect,
        shape: Vec<ShapeRect>,
        filter: Filter,
        /// Brightness, 1 normal. Cut view's scrim draws the source at 0.45.
        dim: f32,
        alpha: f32,
        /// Clip to the circle inscribed in `dst` (the loupe).
        circle: bool,
        /// Mouse on this item is translated to the source (the live result, not the layer).
        input: bool,
    },
    Fill { r: Px, color: Rgba, radius: f32 },
    Stroke { r: Px, color: Rgba, width: f32, radius: f32, dashed: bool },
    Line { a: (f32, f32), b: (f32, f32), color: Rgba, width: f32 },
    Ellipse { r: Px, color: Rgba, width: f32 },
    Text { r: Px, text: String, size: f32, color: Rgba, bold: bool, align: Align },
    /// A Lucide icon drawn into the square `r`, with a stroke this many pixels wide.
    Icon { name: &'static str, r: Px, color: Rgba, stroke: f32 },
}

/// One display's canvas window and what goes on it.
#[derive(Clone, Debug, PartialEq)]
pub struct Surface {
    pub hwnd: isize,
    /// The display's rectangle in screen pixels.
    pub screen: (i32, i32, i32, i32),
    pub draws: Vec<Draw>,
    /// Black outside the drawn items (Takeover); clicks there are swallowed.
    pub takeover: bool,
    /// See-through outside the drawn items (Float): the desktop shows between cutouts. The
    /// window must have been made for it (`Canvas::new_float`).
    pub transparent: bool,
}

/// Everything the engine needs to know.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Frame {
    /// The window being captured.
    pub target: Option<isize>,
    pub surfaces: Vec<Surface>,
    /// 0: show the live picture. Any other value: show a still frame; a new value takes a new
    /// still (Cut view opens frozen; F5 retakes).
    pub freeze: u64,
    /// Translate the mouse onto `Source` items marked `input` (off while the layer is open).
    pub mouse: bool,
    /// The profile's Pointer setting, while `mouse` is on.
    pub pointer: crate::model::profile::Pointer,
}

/// The performance readout, over the last two seconds.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stats {
    /// Frames a second arriving from the app.
    pub capture_fps: f32,
    /// Frames a second shown on each canvas window.
    pub outputs: Vec<(isize, f32)>,
    /// How old the newest captured frame is when it is shown, on average: Kiritori's added
    /// delay.
    pub delay_ms: f32,
    /// Time the graphics card spends drawing one canvas.
    pub gpu_ms: Option<f32>,
}

/// The source's drawing area as pixels, for Suggest and Trim.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub w: usize,
    pub h: usize,
    /// BGRA, top row first.
    pub bgra: Vec<u8>,
}

/// What the graphics card can do, for the self-test.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EngineCaps {
    pub adapter: String,
    pub feature_level: String,
    /// FSR 1 compiled: Enhanced scaling is available.
    pub enhanced: bool,
}

/// Facts about the source, reported back by the engine for the layer's geometry.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SourceInfo {
    /// The source's drawing area in screen pixels.
    pub client: (i32, i32, i32, i32),
    /// A frame has been captured.
    pub ready: bool,
    pub fps: f32,
}
