//! Cutouts, placements and layouts, and the pure operations on them.
//!
//! Vocabulary: a cutout is a named, numbered region of the source;
//! its shape lives in source fractions, its placement in canvas fractions; a layout is a set of
//! cutouts plus their placements.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::geometry::{Rect, Size, fit};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShapeOp {
    Add,
    Subtract,
}

/// One entry of a cutout's shape. Entries apply in order, each adding to or subtracting from
/// the shape built so far.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShapeRect {
    pub op: ShapeOp,
    #[serde(flatten)]
    pub rect: Rect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CutoutColor {
    Cyan,
    Amber,
    Magenta,
    Lime,
    Orange,
    Violet,
    Red,
    Teal,
}

impl CutoutColor {
    /// Assigned in this order.
    pub const ORDER: [CutoutColor; 8] = [
        CutoutColor::Cyan,
        CutoutColor::Amber,
        CutoutColor::Magenta,
        CutoutColor::Lime,
        CutoutColor::Orange,
        CutoutColor::Violet,
        CutoutColor::Red,
        CutoutColor::Teal,
    ];
}

/// How a cutout is scaled when shown at a different size.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Filter {
    #[default]
    Auto,
    Smooth,
    Sharp,
    Enhanced,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Placement {
    /// Key into the layout's `displays`.
    pub display: String,
    #[serde(flatten)]
    pub rect: Rect,
    pub visible: bool,
    pub locked: bool,
    pub z: i32,
    #[serde(default)]
    pub filter: Filter,
    /// Clockwise quarter turns, 0–3.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rotation: u8,
    #[serde(default, skip_serializing_if = "is_false")]
    pub flip: bool,
    /// Where the cutout was before Fill, so a second Fill can restore it. Not saved.
    #[serde(skip)]
    pub before_fill: Option<Rect>,
}

fn is_zero(v: &u8) -> bool {
    *v == 0
}

fn is_false(v: &bool) -> bool {
    !*v
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Cutout {
    /// 1–9, then 10 onwards (shown as letters). Stable for the cutout's life.
    pub number: u32,
    pub name: String,
    pub color: CutoutColor,
    pub rects: Vec<ShapeRect>,
    pub placement: Placement,
}

impl Cutout {
    /// Bounding box of the shape, in source fractions.
    pub fn bounds(&self) -> Rect {
        self.rects
            .iter()
            .filter(|r| r.op == ShapeOp::Add)
            .fold(Rect::default(), |acc, r| acc.union(&r.rect))
    }

    /// Whether a source point (fractions) is inside the shape.
    pub fn contains(&self, x: f64, y: f64) -> bool {
        self.rects.iter().fold(false, |inside, r| match r.op {
            ShapeOp::Add => inside || r.rect.contains(x, y),
            ShapeOp::Subtract => inside && !r.rect.contains(x, y),
        })
    }

    /// Size of the shape's bounding box in source pixels, as shown: a quarter turn swaps width
    /// and height.
    pub fn pixel_size(&self, source_px: Size) -> Size {
        let s = self.bounds().scale(source_px).size();
        if self.placement.rotation % 2 == 1 { Size::new(s.h, s.w) } else { s }
    }

    /// Rotation (0–3 clockwise quarter turns) and mirroring, packed for drawing: bits 0–1 the
    /// turns, bit 2 the mirror.
    pub fn orient(&self) -> u8 {
        (self.placement.rotation % 4) | if self.placement.flip { 4 } else { 0 }
    }

    /// The label shown in tags and lists: 1–9, then A, B, C…
    pub fn label(&self) -> String {
        number_label(self.number)
    }
}

/// A point in a placed cutout (fractions of its placement, 0–1) back to the same point in the
/// unrotated, unmirrored shape (fractions of its bounding box). The shader does the same.
pub fn unorient(orient: u8, x: f64, y: f64) -> (f64, f64) {
    let (x, y) = match orient & 3 {
        1 => (y, 1.0 - x),
        2 => (1.0 - x, 1.0 - y),
        3 => (1.0 - y, x),
        _ => (x, y),
    };
    if orient & 4 != 0 { (1.0 - x, y) } else { (x, y) }
}

pub fn number_label(n: u32) -> String {
    if n <= 9 {
        n.to_string()
    } else {
        char::from_u32('A' as u32 + (n - 10) % 26).unwrap_or('?').to_string()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DisplayMode {
    Takeover,
    Float,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DisplayRef {
    pub hardware_id: String,
    pub label: String,
    pub mode: DisplayMode,
}

/// The facts about the running source and its display that placement maths needs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    /// Size of the source's drawing area in pixels.
    pub source_px: Size,
    /// Size of the display's canvas in pixels.
    pub canvas_px: Size,
    /// Where the source's drawing area sits on that canvas, in canvas fractions.
    pub source_on_canvas: Rect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Below,
    Beside,
}

pub const MAIN_DISPLAY: &str = "main";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Layout {
    pub id: String,
    pub name: String,
    pub source_aspect: [u32; 2],
    pub displays: BTreeMap<String, DisplayRef>,
    pub cutouts: Vec<Cutout>,
}

impl Layout {
    pub fn new(id: &str, name: &str, display: DisplayRef) -> Self {
        let mut displays = BTreeMap::new();
        displays.insert(MAIN_DISPLAY.to_string(), display);
        Layout { id: id.into(), name: name.into(), source_aspect: [16, 9], displays, cutouts: Vec::new() }
    }

    pub fn cutout(&self, n: u32) -> Option<&Cutout> {
        self.cutouts.iter().find(|c| c.number == n)
    }

    pub fn cutout_mut(&mut self, n: u32) -> Option<&mut Cutout> {
        self.cutouts.iter_mut().find(|c| c.number == n)
    }

    /// Lowest number not in use.
    pub fn next_number(&self) -> u32 {
        (1..).find(|n| self.cutout(*n).is_none()).unwrap()
    }

    /// First colour in the order above that no cutout uses; cycles when all eight are taken.
    pub fn next_color(&self) -> CutoutColor {
        CutoutColor::ORDER
            .iter()
            .copied()
            .find(|c| self.cutouts.iter().all(|k| k.color != *c))
            .unwrap_or(CutoutColor::ORDER[self.cutouts.len() % 8])
    }

    fn top_z(&self) -> i32 {
        self.cutouts.iter().map(|c| c.placement.z).max().unwrap_or(0)
    }

    /// Placement that shows a shape exactly where it is in the source, at 100%.
    pub fn source_placement(bounds: Rect, frame: &Frame) -> Rect {
        frame.source_on_canvas.sub(&bounds)
    }

    /// Make a new cutout from a source rect. It starts visible at its source position.
    pub fn add_cutout(&mut self, rect: Rect, frame: &Frame) -> u32 {
        let number = self.next_number();
        let rect = rect.clamp_unit();
        let cutout = Cutout {
            number,
            name: format!("Cutout {number}"),
            color: self.next_color(),
            rects: vec![ShapeRect { op: ShapeOp::Add, rect }],
            placement: Placement {
                display: MAIN_DISPLAY.into(),
                rect: Self::source_placement(rect, frame),
                visible: true,
                locked: false,
                z: self.top_z() + 1,
                filter: Filter::Auto,
                rotation: 0,
                flip: false,
                before_fill: None,
            },
        };
        self.cutouts.push(cutout);
        number
    }

    pub fn delete(&mut self, n: u32) -> bool {
        let before = self.cutouts.len();
        self.cutouts.retain(|c| c.number != n);
        self.cutouts.len() != before
    }

    pub fn rename(&mut self, n: u32, name: &str) {
        if let Some(c) = self.cutout_mut(n) {
            let name = name.trim();
            if !name.is_empty() {
                c.name = name.to_string();
            }
        }
    }

    pub fn toggle_visible(&mut self, n: u32) {
        if let Some(c) = self.cutout_mut(n) {
            c.placement.visible = !c.placement.visible;
        }
    }

    pub fn toggle_locked(&mut self, n: u32) {
        if let Some(c) = self.cutout_mut(n) {
            c.placement.locked = !c.placement.locked;
        }
    }

    /// Add a rectangle to, or subtract one from, a cutout's shape.
    pub fn edit_shape(&mut self, n: u32, op: ShapeOp, rect: Rect) {
        if let Some(c) = self.cutout_mut(n) {
            c.rects.push(ShapeRect { op, rect: rect.clamp_unit() });
        }
    }

    /// Replace a single-rectangle shape (the common case: resize or move a cut).
    pub fn set_cut(&mut self, n: u32, rect: Rect) {
        if let Some(c) = self.cutout_mut(n) {
            c.rects = vec![ShapeRect { op: ShapeOp::Add, rect: rect.clamp_unit() }];
        }
    }

    /// Trim a single-rectangle cutout to `t` (source fractions inside its current cut). The
    /// part that stays keeps its place and size on the canvas.
    pub fn trim_to(&mut self, n: u32, t: Rect, frame: &Frame) -> bool {
        let Some(c) = self.cutout_mut(n) else { return false };
        if c.rects.len() != 1 {
            return false;
        }
        let b = c.bounds();
        let (sw, sh) = (frame.source_px.w, frame.source_px.h);
        let (cw, ch) = (frame.canvas_px.w, frame.canvas_px.h);
        let p = c.placement.rect;
        let zx = p.w * cw / (b.w * sw).max(1e-9);
        let zy = p.h * ch / (b.h * sh).max(1e-9);
        c.placement.rect = Rect::new((p.x * cw + (t.x - b.x) * sw * zx) / cw, (p.y * ch + (t.y - b.y) * sh * zy) / ch, t.w * sw * zx / cw, t.h * sh * zy / ch);
        c.placement.before_fill = None;
        c.rects = vec![ShapeRect { op: ShapeOp::Add, rect: t.clamp_unit() }];
        true
    }

    /// Move a cut's whole shape by a source-fraction offset, keeping it inside the source.
    pub fn move_cut(&mut self, n: u32, dx: f64, dy: f64) {
        if let Some(c) = self.cutout_mut(n) {
            let b = c.bounds();
            let dx = dx.clamp(-b.x, 1.0 - b.right());
            let dy = dy.clamp(-b.y, 1.0 - b.bottom());
            for r in &mut c.rects {
                r.rect.x += dx;
                r.rect.y += dy;
            }
        }
    }

    pub fn set_placement(&mut self, n: u32, rect: Rect) {
        if let Some(c) = self.cutout_mut(n) {
            c.placement.rect = rect;
            c.placement.before_fill = None;
        }
    }

    /// Back to its source position at 100%.
    pub fn reset_placement(&mut self, n: u32, frame: &Frame) {
        if let Some(c) = self.cutout_mut(n) {
            c.placement.rect = Self::source_placement(c.bounds(), frame);
            c.placement.before_fill = None;
            c.placement.rotation = 0;
            c.placement.flip = false;
        }
    }

    /// Q and E: turn a cutout a quarter turn left or right about its centre; its placement
    /// swaps width and height, kept on the canvas.
    pub fn rotate(&mut self, n: u32, clockwise: bool, canvas: Size) {
        if let Some(c) = self.cutout_mut(n) {
            let r = c.placement.rect.scale(canvas);
            let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
            let (w, h) = (r.h.min(canvas.w), r.w.min(canvas.h));
            let x = (cx - w / 2.0).clamp(0.0, (canvas.w - w).max(0.0));
            let y = (cy - h / 2.0).clamp(0.0, (canvas.h - h).max(0.0));
            c.placement.rect = Rect::new(x, y, w, h).unscale(canvas);
            c.placement.rotation = (c.placement.rotation + if clockwise { 1 } else { 3 }) % 4;
            c.placement.before_fill = None;
        }
    }

    /// Mirror a cutout left to right.
    pub fn mirror(&mut self, n: u32) {
        if let Some(c) = self.cutout_mut(n) {
            c.placement.flip = !c.placement.flip;
        }
    }

    /// Largest size that fits the canvas, centred. A second call restores the previous rect.
    pub fn toggle_fill(&mut self, n: u32, frame: &Frame) {
        if let Some(c) = self.cutout_mut(n) {
            if let Some(prev) = c.placement.before_fill.take() {
                c.placement.rect = prev;
                return;
            }
            let px = fit(c.pixel_size(frame.source_px), Rect::new(0.0, 0.0, frame.canvas_px.w, frame.canvas_px.h));
            c.placement.before_fill = Some(c.placement.rect);
            c.placement.rect = px.unscale(frame.canvas_px);
        }
    }

    /// Auto-arrange the visible cutouts on the focus's display: the focus gets the largest
    /// share, the rest go in a strip below or beside it, whichever leaves the focus larger
    /// (below on a tie). `side` forces one side, for "press A again to try the other side".
    /// Returns the side used.
    pub fn auto_arrange(&mut self, focus: u32, frame: &Frame, side: Option<Side>) -> Option<Side> {
        let display = self.cutout(focus)?.placement.display.clone();
        let mut others: Vec<u32> = self
            .cutouts
            .iter()
            .filter(|c| c.number != focus && c.placement.visible && c.placement.display == display)
            .map(|c| c.number)
            .collect();
        others.sort();
        let (cw, ch) = (frame.canvas_px.w, frame.canvas_px.h);
        let fsize = self.cutout(focus)?.pixel_size(frame.source_px);

        if others.is_empty() {
            let r = fit(fsize, Rect::new(0.0, 0.0, cw, ch));
            self.set_placement(focus, r.unscale(frame.canvas_px));
            return Some(Side::Below);
        }

        // Focus pinned to the top (below) or left (beside) edge of its 75% share.
        let below = {
            let r = fit(fsize, Rect::new(0.0, 0.0, cw, ch * 0.75));
            Rect::new(r.x, 0.0, r.w, r.h)
        };
        let beside = {
            let r = fit(fsize, Rect::new(0.0, 0.0, cw * 0.75, ch));
            Rect::new(0.0, r.y, r.w, r.h)
        };
        let chosen = side.unwrap_or(if beside.w * beside.h > below.w * below.h + 0.5 { Side::Beside } else { Side::Below });
        let f = if chosen == Side::Below { below } else { beside };
        self.set_placement(focus, f.unscale(frame.canvas_px));

        let n = others.len() as f64;
        for (i, num) in others.iter().enumerate() {
            let size = self.cutout(*num).unwrap().pixel_size(frame.source_px);
            let r = match chosen {
                Side::Below => {
                    let band = Rect::new(0.0, f.bottom(), cw, ch - f.bottom());
                    let cell = Rect::new(band.w / n * i as f64, band.y, band.w / n, band.h);
                    let r = fit(size, cell);
                    Rect::new(r.x, cell.y, r.w, r.h)
                }
                Side::Beside => {
                    let band = Rect::new(f.right(), 0.0, cw - f.right(), ch);
                    let cell = Rect::new(band.x, band.h / n * i as f64, band.w, band.h / n);
                    let r = fit(size, cell);
                    Rect::new(cell.x, r.y, r.w, r.h)
                }
            };
            self.set_placement(*num, r.unscale(frame.canvas_px));
        }
        Some(chosen)
    }

    /// Cutouts in drawing order, bottom first.
    pub fn by_z(&self) -> Vec<&Cutout> {
        let mut v: Vec<&Cutout> = self.cutouts.iter().collect();
        v.sort_by_key(|c| (c.placement.z, c.number));
        v
    }

    /// Swap stacking order with the neighbour above (`up`) or below.
    pub fn restack(&mut self, n: u32, up: bool) {
        let order: Vec<u32> = self.by_z().iter().map(|c| c.number).collect();
        let Some(i) = order.iter().position(|k| *k == n) else { return };
        let j = if up { i + 1 } else { i.wrapping_sub(1) };
        let Some(&other) = order.get(j) else { return };
        let (za, zb) = (self.cutout(n).unwrap().placement.z, self.cutout(other).unwrap().placement.z);
        let (za, zb) = if za == zb { (za, za + if up { -1 } else { 1 }) } else { (za, zb) };
        self.cutout_mut(n).unwrap().placement.z = zb;
        self.cutout_mut(other).unwrap().placement.z = za;
    }

    /// Front-to-back order, as the cutout list shows it.
    pub fn front_to_back(&self) -> Vec<u32> {
        self.by_z().iter().rev().map(|c| c.number).collect()
    }

    /// Restack to match a front-to-back list of numbers (dragging rows in the cutout list).
    /// Numbers not in the list keep their relative order behind the listed ones.
    pub fn set_order(&mut self, front_to_back: &[u32]) {
        let mut order: Vec<u32> = front_to_back.iter().copied().filter(|n| self.cutout(*n).is_some()).collect();
        for n in self.front_to_back() {
            if !order.contains(&n) {
                order.push(n);
            }
        }
        let count = order.len() as i32;
        for (i, n) in order.iter().enumerate() {
            self.cutout_mut(*n).unwrap().placement.z = count - i as i32;
        }
    }

    /// Reset arrangement: every cutout back at its source position, 100%, shown.
    pub fn reset_arrangement(&mut self, frame: &Frame) {
        let numbers: Vec<u32> = self.cutouts.iter().map(|c| c.number).collect();
        for n in numbers {
            self.reset_placement(n, frame);
            let c = self.cutout_mut(n).unwrap();
            c.placement.visible = true;
            c.placement.display = MAIN_DISPLAY.into();
        }
    }

    /// Start over: no cutouts.
    pub fn start_over(&mut self) {
        self.cutouts.clear();
    }

    /// Round stored numbers so files stay readable and diffs stay small.
    pub fn tidy(&mut self) {
        for c in &mut self.cutouts {
            for r in &mut c.rects {
                r.rect = r.rect.rounded(5);
            }
            c.placement.rect = c.placement.rect.rounded(5);
        }
    }
}

/// Ready-made grids for Cut view. Each entry is (name, source rect).
pub fn preset(kind: Preset) -> Vec<(&'static str, Rect)> {
    let h = 0.5;
    let t = 1.0 / 3.0;
    match kind {
        Preset::SideBySide => vec![("Left", Rect::new(0.0, 0.0, h, 1.0)), ("Right", Rect::new(h, 0.0, h, 1.0))],
        Preset::Stacked => vec![("Top", Rect::new(0.0, 0.0, 1.0, h)), ("Bottom", Rect::new(0.0, h, 1.0, h))],
        Preset::Quadrants => vec![
            ("Top left", Rect::new(0.0, 0.0, h, h)),
            ("Top right", Rect::new(h, 0.0, h, h)),
            ("Bottom left", Rect::new(0.0, h, h, h)),
            ("Bottom right", Rect::new(h, h, h, h)),
        ],
        Preset::ThreeColumns => vec![
            ("Left", Rect::new(0.0, 0.0, t, 1.0)),
            ("Middle", Rect::new(t, 0.0, t, 1.0)),
            ("Right", Rect::new(2.0 * t, 0.0, t, 1.0)),
        ],
        Preset::ThreeRows => vec![
            ("Top", Rect::new(0.0, 0.0, 1.0, t)),
            ("Middle", Rect::new(0.0, t, 1.0, t)),
            ("Bottom", Rect::new(0.0, 2.0 * t, 1.0, t)),
        ],
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    SideBySide,
    Stacked,
    Quadrants,
    ThreeColumns,
    ThreeRows,
}

impl Preset {
    pub const ALL: [Preset; 5] = [Preset::SideBySide, Preset::Stacked, Preset::Quadrants, Preset::ThreeColumns, Preset::ThreeRows];

    pub fn label(self) -> &'static str {
        match self {
            Preset::SideBySide => "Two side by side",
            Preset::Stacked => "Two stacked",
            Preset::Quadrants => "Four quadrants",
            Preset::ThreeColumns => "Three columns",
            Preset::ThreeRows => "Three rows",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn display() -> DisplayRef {
        DisplayRef { hardware_id: "test".into(), label: "Display 1".into(), mode: DisplayMode::Takeover }
    }

    /// The README example: a 2560×1440 borderless source on a 2560×1440 display.
    fn sample_frame() -> Frame {
        Frame {
            source_px: Size::new(2560.0, 1440.0),
            canvas_px: Size::new(2560.0, 1440.0),
            source_on_canvas: Rect::new(0.0, 0.0, 1.0, 1.0),
        }
    }

    fn px(r: Rect, s: Size) -> (i64, i64, i64, i64) {
        let p = r.scale(s);
        (p.x.round() as i64, p.y.round() as i64, p.w.round() as i64, p.h.round() as i64)
    }

    fn sample_layout() -> (Layout, Frame) {
        let f = sample_frame();
        let mut l = Layout::new("layout-1", "Layout 1", display());
        let s = f.source_px;
        let main = l.add_cutout(Rect::new(1280.0, 0.0, 1280.0, 720.0).unscale(s), &f);
        l.rename(main, "Main view");
        let controls = l.add_cutout(Rect::new(1296.0, 736.0, 1250.0, 500.0).unscale(s), &f);
        l.rename(controls, "Controls");
        let curve = l.add_cutout(Rect::new(0.0, 0.0, 1280.0, 720.0).unscale(s), &f);
        l.rename(curve, "Graph");
        l.toggle_visible(curve);
        let score = l.add_cutout(Rect::new(0.0, 720.0, 1280.0, 720.0).unscale(s), &f);
        l.rename(score, "Score");
        l.toggle_visible(score);
        (l, f)
    }

    #[test]
    fn numbering_names_and_colours_follow_the_order() {
        let (l, _) = sample_layout();
        let tags: Vec<_> = l.cutouts.iter().map(|c| (c.number, c.name.as_str(), c.color, c.placement.visible)).collect();
        assert_eq!(
            tags,
            vec![
                (1, "Main view", CutoutColor::Cyan, true),
                (2, "Controls", CutoutColor::Amber, true),
                (3, "Graph", CutoutColor::Magenta, false),
                (4, "Score", CutoutColor::Lime, false),
            ]
        );
    }

    #[test]
    fn new_cutouts_start_at_their_source_position() {
        let (l, f) = sample_layout();
        assert_eq!(px(l.cutout(1).unwrap().placement.rect, f.canvas_px), (1280, 0, 1280, 720));
        assert_eq!(px(l.cutout(2).unwrap().placement.rect, f.canvas_px), (1296, 736, 1250, 500));
    }

    #[test]
    fn trimming_keeps_the_rest_where_it_was() {
        let mut l = Layout::new("l", "L", display());
        let f = Frame { source_px: Size::new(1000.0, 1000.0), canvas_px: Size::new(2000.0, 2000.0), source_on_canvas: Rect::new(0.0, 0.0, 1.0, 1.0) };
        l.add_cutout(Rect::new(0.0, 0.0, 0.5, 0.5), &f);
        l.cutout_mut(1).unwrap().placement.rect = Rect::new(0.0, 0.0, 0.5, 0.5); // 2x zoom
        assert!(l.trim_to(1, Rect::new(0.1, 0.0, 0.4, 0.5), &f));
        let p = l.cutout(1).unwrap().placement.rect;
        assert!((p.x - 0.1).abs() < 1e-9 && (p.w - 0.4).abs() < 1e-9 && p.y.abs() < 1e-9 && (p.h - 0.5).abs() < 1e-9, "{p:?}");
        assert_eq!(l.cutout(1).unwrap().bounds(), Rect::new(0.1, 0.0, 0.4, 0.5));
    }

    #[test]
    fn source_placement_follows_a_framed_window() {
        // A 1920×1080 drawing area at (40, 98) on a 3840×2160 display.
        let canvas = Size::new(3840.0, 2160.0);
        let f = Frame {
            source_px: Size::new(1920.0, 1080.0),
            canvas_px: canvas,
            source_on_canvas: Rect::new(40.0, 98.0, 1920.0, 1080.0).unscale(canvas),
        };
        let p = Layout::source_placement(Rect::new(0.5, 0.5, 0.5, 0.5), &f);
        assert_eq!(px(p, canvas), (1000, 638, 960, 540));
    }

    #[test]
    fn auto_arrange_stacks_the_focus_over_a_strip() {
        let (mut l, f) = sample_layout();
        assert_eq!(l.auto_arrange(1, &f, None), Some(Side::Below));
        // Main view 1920×1080 (150%) centred on the top edge; Controls 900×360 (72%) beneath it.
        assert_eq!(px(l.cutout(1).unwrap().placement.rect, f.canvas_px), (320, 0, 1920, 1080));
        assert_eq!(px(l.cutout(2).unwrap().placement.rect, f.canvas_px), (830, 1080, 900, 360));
        // And the same as the fractions in the example profile file.
        let r = l.cutout(2).unwrap().placement.rect.rounded(5);
        assert_eq!(r, Rect::new(0.32422, 0.75, 0.35156, 0.25));
        // Hidden cutouts are left where they were.
        assert_eq!(px(l.cutout(3).unwrap().placement.rect, f.canvas_px), (0, 0, 1280, 720));
    }

    #[test]
    fn auto_arrange_again_tries_the_other_side() {
        let (mut l, f) = sample_layout();
        l.auto_arrange(1, &f, None);
        assert_eq!(l.auto_arrange(1, &f, Some(Side::Beside)), Some(Side::Beside));
        let main = px(l.cutout(1).unwrap().placement.rect, f.canvas_px);
        let controls = px(l.cutout(2).unwrap().placement.rect, f.canvas_px);
        assert_eq!(main, (0, 180, 1920, 1080));
        assert_eq!(controls.0, 1920); // strip starts at the focus's right edge
        assert!(controls.0 + controls.2 <= 2560);
    }

    #[test]
    fn fill_then_fill_again_restores() {
        let (mut l, f) = sample_layout();
        let before = l.cutout(2).unwrap().placement.rect;
        l.toggle_fill(2, &f);
        assert_eq!(px(l.cutout(2).unwrap().placement.rect, f.canvas_px), (0, 208, 2560, 1024));
        l.toggle_fill(2, &f);
        assert_eq!(l.cutout(2).unwrap().placement.rect, before);
    }

    #[test]
    fn numbers_are_reused_lowest_first_and_labels_go_to_letters() {
        let (mut l, f) = sample_layout();
        l.delete(2);
        assert_eq!(l.add_cutout(Rect::new(0.0, 0.0, 0.1, 0.1), &f), 2);
        assert_eq!(number_label(9), "9");
        assert_eq!(number_label(10), "A");
        assert_eq!(number_label(11), "B");
    }

    #[test]
    fn shapes_add_and_subtract_in_order() {
        let f = sample_frame();
        let mut l = Layout::new("l", "L", display());
        let n = l.add_cutout(Rect::new(0.0, 0.0, 0.5, 0.5), &f);
        l.edit_shape(n, ShapeOp::Add, Rect::new(0.0, 0.5, 0.25, 0.25)); // an L
        l.edit_shape(n, ShapeOp::Subtract, Rect::new(0.4, 0.0, 0.1, 0.1)); // a notch
        let c = l.cutout(n).unwrap();
        assert!(c.contains(0.1, 0.6));
        assert!(!c.contains(0.4, 0.6));
        assert!(!c.contains(0.45, 0.05));
        assert!(c.contains(0.3, 0.05));
        assert_eq!(c.bounds(), Rect::new(0.0, 0.0, 0.5, 0.75));
    }

    #[test]
    fn move_cut_stays_inside_the_source() {
        let f = sample_frame();
        let mut l = Layout::new("l", "L", display());
        let n = l.add_cutout(Rect::new(0.5, 0.5, 0.5, 0.5), &f);
        l.move_cut(n, 0.2, -0.1);
        assert_eq!(l.cutout(n).unwrap().bounds().rounded(6), Rect::new(0.5, 0.4, 0.5, 0.5));
    }

    #[test]
    fn restack_swaps_neighbours() {
        let (mut l, _) = sample_layout();
        l.restack(1, true);
        let order: Vec<u32> = l.by_z().iter().map(|c| c.number).collect();
        assert_eq!(order, vec![2, 1, 3, 4]);
        l.restack(1, false);
        let order: Vec<u32> = l.by_z().iter().map(|c| c.number).collect();
        assert_eq!(order, vec![1, 2, 3, 4]);
    }

    #[test]
    fn set_order_restacks_front_to_back() {
        let (mut l, _) = sample_layout();
        assert_eq!(l.front_to_back(), vec![4, 3, 2, 1]);
        l.set_order(&[2, 1]);
        assert_eq!(l.front_to_back(), vec![2, 1, 4, 3]);
        l.set_order(&[1, 2, 3, 4]);
        assert_eq!(l.front_to_back(), vec![1, 2, 3, 4]);
    }

    #[test]
    fn reset_arrangement_and_start_over() {
        let (mut l, f) = sample_layout();
        l.auto_arrange(1, &f, None);
        l.reset_arrangement(&f);
        assert!(l.cutouts.iter().all(|c| c.placement.visible));
        assert_eq!(px(l.cutout(1).unwrap().placement.rect, f.canvas_px), (1280, 0, 1280, 720));
        l.start_over();
        assert!(l.cutouts.is_empty());
    }

    #[test]
    fn presets_cover_the_source() {
        for p in Preset::ALL {
            let area: f64 = preset(p).iter().map(|(_, r)| r.w * r.h).sum();
            assert!((area - 1.0).abs() < 1e-9, "{p:?}");
        }
    }
}
