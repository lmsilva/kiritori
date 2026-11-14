//! The Kiritori layer: the editing overlay opened with Ctrl+Alt+K.
//!
//! Cut view lives here and Arrange view in `arrange`. The layer keeps only editing state
//! (selection, the drag in progress, the name being typed); every change goes straight into the
//! layout, and the caller records it as an undoable step and saves.

/// A rectangle being drawn in Cut view: the shape operation (None for a new cutout) and its two
/// corners in source fractions.
pub type DrawingRect = (Option<ShapeOp>, (f64, f64), (f64, f64));

pub mod arrange;
pub mod chrome;
pub mod displays;
pub mod guide;
pub mod history;
pub mod keymap;
pub mod style;
pub mod view;

use std::collections::HashMap;
use std::time::{Duration, Instant};

use arrange::{Guide, PDrag};
use chrome::{HoverKey, LayoutNaming, ListHit, RowDrag};
use history::ResetChoice;
use guide::{GuideCard, GuideClick};

use crate::engine::draw::Px;
use crate::model::geometry::{Rect, Size};
use crate::model::layout::{Frame, Layout, Preset, ShapeOp, ShapeRect, Side, preset};
use crate::shell::canvas::{Button, Input, Mods};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Cut,
    Arrange,
}

/// Toolbar and keyboard actions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Cut,
    Arrange,
    Undo,
    Redo,
    /// Open the reset card (Reset arrangement or Start over).
    Reset,
    Retake,
    Grids,
    Hide,
    Delete,
    Snapping,
    Fill,
    AutoArrange,
    ResetPlacement,
    /// Arrange view: switch between the still frame (Edit) and the moving picture (Live).
    Live,
    /// Show or hide the cutout list.
    List,
    /// Cut view: suggest cutouts from the picture's panels (S).
    Suggest,
    /// Cut view: trim black edges from the selected cutout (T).
    Trim,
    /// The shortcut map (F1).
    Shortcuts,
    Profiles,
    Settings,
    Close,
}

/// What the caller should do after an input.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    None,
    /// The layout changed: record a step with this name (and merge key) and save.
    Edit(String, Option<String>),
    Undo,
    Redo,
    Close,
    /// Take a new still frame.
    Retake,
    /// Show the context menu at this canvas point, for this cutout if any.
    Menu(f32, f32, Option<u32>),
    /// Show the layouts menu (switch, rename, new) at this canvas point.
    LayoutMenu(f32, f32),
    /// Rename the active layout.
    RenameLayout(String),
    /// Undo this many steps at once (a row in the history list).
    UndoMany(usize),
    /// A click on the tutorial card.
    Guide(GuideClick),
    /// Open the Profiles panel (P).
    Profiles,
    /// D: send this cutout to the next display.
    SendToNext(u32),
    /// A click on a display in the display map: offer Takeover, Float, Not in use for map
    /// entry `.0`, at this canvas point.
    DisplayMenu(usize, f32, f32),
    /// A cutout row dropped on a display in the map.
    SendToMap(u32, usize),
    /// Suggest cutouts from the still frame (needs its pixels).
    Suggest,
    /// Trim black edges from this cutout (needs the still frame's pixels).
    Trim(u32),
    /// Open the Settings panel.
    Settings,
}

/// Geometry the layer needs, from the engine and the canvas.
#[derive(Clone, Debug)]
pub struct Geo {
    /// Key into the layout's `displays` for this canvas ("main" holds the source).
    pub display: String,
    /// This canvas carries the toolbar, list and hint strip (the pointer is on it).
    pub here: bool,
    /// Windows' number for the display, for its badge (0: none).
    pub number: u32,
    /// The canvas size in pixels.
    pub canvas: Size,
    /// The source's drawing area on the canvas, in canvas pixels.
    pub source: Rect,
    /// The source's drawing area size in source pixels.
    pub source_px: Size,
    /// Chrome scale (dpi / 96).
    pub scale: f32,
}

impl Geo {
    pub fn frame(&self) -> Frame {
        Frame { source_px: self.source_px, canvas_px: self.canvas, source_on_canvas: self.source.unscale(self.canvas) }
    }

    /// Canvas pixel → source pixel.
    fn to_src(&self, x: f32, y: f32) -> (f64, f64) {
        (
            (x as f64 - self.source.x) / self.source.w * self.source_px.w,
            (y as f64 - self.source.y) / self.source.h * self.source_px.h,
        )
    }

    /// Source fractions → canvas pixels.
    pub fn to_canvas(&self, r: Rect) -> Px {
        let p = self.source.sub(&r);
        (p.x as f32, p.y as f32, p.w as f32, p.h as f32)
    }

    /// Canvas pixels per source pixel.
    fn zoom(&self) -> f64 {
        self.source.w / self.source_px.w
    }
}

#[derive(Clone, Debug)]
enum Drag {
    /// Drawing a new cutout, or a rectangle to add to / subtract from the selected one.
    Draw { op: Option<ShapeOp>, start: (f64, f64), now: (f64, f64) },
    Move { n: u32, grab: (f64, f64), orig: Vec<ShapeRect> },
    /// `outside`: the press was outside the cutout; `press` where, in source pixels.
    Resize { n: u32, handle: usize, orig: Vec<ShapeRect>, outside: bool, press: (f64, f64) },
    /// A press on a toolbar button: a click if released there, but a drag that moves away
    /// draws (or grabs) on the picture underneath, because the toolbar sits over the top of
    /// a full-screen source.
    Button(Action, (f32, f32), Mods),
}

#[derive(Clone, Debug)]
pub struct Naming {
    pub n: u32,
    /// The name being typed; it starts all selected, so typing replaces it.
    pub typing: crate::textedit::Typing,
}

/// The longest cutout or layout name.
pub(crate) const NAME_MAX: usize = 40;

impl Naming {
    pub fn new(n: u32, name: &str) -> Self {
        Naming { n, typing: crate::textedit::Typing::new(name, NAME_MAX) }
    }

    #[cfg(test)]
    fn typing_rect(&self) -> Px {
        self.typing.drawn_rect().expect("drawn")
    }
}

/// A snap line in source pixels: vertical (x) or horizontal (y).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SnapLine {
    X(f64),
    Y(f64),
}

pub struct Layer {
    pub view: View,
    pub selected: Option<u32>,
    drag: Option<Drag>,
    pub naming: Option<Naming>,
    pub snapping: bool,
    pub pointer: (f32, f32),
    /// Still-frame generation for the engine.
    pub freeze: u64,
    pub snaps: Vec<SnapLine>,
    /// Toolbar hit areas from the last drawing.
    pub buttons: Vec<(Px, Action, bool)>,
    pub hover: Option<Action>,
    pub message: Option<(String, Instant)>,
    // Arrange view
    pdrag: Option<PDrag>,
    pub guides: Vec<Guide>,
    /// The scale stop a resize snapped to, for highlighting the size label.
    scale_label: Option<f64>,
    /// The cutout and side of the last auto-arrange, so pressing A again tries the other side.
    last_arrange: Option<(u32, Side)>,
    /// Glide from these placements, started at this time.
    glide: Option<(Instant, HashMap<u32, Rect>)>,
    /// Arrange view shows the moving picture.
    pub live: bool,
    /// Holding Space: chrome hidden, the result as it will look.
    pub peek: bool,
    // Chrome
    /// The cutout list is shown (L).
    pub list_open: bool,
    pub(crate) list_hits: Vec<(Px, ListHit)>,
    row_drag: Option<RowDrag>,
    last_row_click: Option<(u32, Instant)>,
    /// The layout name being typed in the readout.
    pub layout_naming: Option<LayoutNaming>,
    pub(crate) readout_hit: Option<Px>,
    hover_since: Option<(HoverKey, Instant)>,
    pulse: Option<(Action, Instant)>,
    /// The names of the steps Ctrl+Z and Ctrl+Y would undo and redo, for their tooltips.
    pub history_names: (Option<String>, Option<String>),
    // Reset and history
    /// When Ctrl+Shift+Backspace started being held.
    reset_held: Option<Instant>,
    pub reset_card: bool,
    reset_hits: Vec<(Px, ResetChoice)>,
    pub history_open: bool,
    /// The history list was opened by holding Undo, so releasing must not also undo.
    history_from_hold: bool,
    history_hits: Vec<(Px, usize)>,
    /// Every undoable step, newest first, for the history list.
    pub history_list: Vec<String>,
    /// A toolbar button being pressed, and since when.
    button_press: Option<(Action, Instant)>,
    /// The tutorial's current step, set by the app while the tutorial runs.
    pub guide: Option<GuideCard>,
    guide_hits: Vec<(Px, GuideClick)>,
    // Several displays
    /// When the layer opened, for the display number badges.
    pub opened: Instant,
    /// The display map is shown (M).
    pub map_open: bool,
    /// The displays for the map, set by the app before drawing.
    pub map: Vec<displays::MapDisplay>,
    map_hits: Vec<(Px, usize)>,
    /// Where the display map was drawn, so the tutorial card can sit above it.
    pub(crate) map_rect: Option<Px>,
    /// Cutouts whose display is not connected, set by the app.
    pub parked: Vec<u32>,
    /// The shortcut map is shown (F1 or ?).
    pub keymap_open: bool,
    /// Performance readout lines, set by the app when the readout is on.
    pub perf: Vec<String>,
    /// The panels drawn last time (toolbar, list, map, readout), for placing tags clear of them.
    pub(crate) chrome_rects: Vec<Px>,
    /// Where passing overlays (tooltips, cards) start in the last drawing.
    overlay_from: usize,
    keymap_hits: Vec<(Px, &'static str)>,
}

/// Snap distance in canvas pixels at 100% scaling.
const SNAP_PX: f64 = 10.0;
/// A drag shorter than this (source pixels) is a click, not a new cutout.
const MIN_CUT: f64 = 8.0;

impl Layer {
    pub fn new(freeze: u64) -> Self {
        Layer {
            view: View::Cut,
            selected: None,
            drag: None,
            naming: None,
            snapping: true,
            pointer: (0.0, 0.0),
            freeze,
            snaps: Vec::new(),
            buttons: Vec::new(),
            hover: None,
            message: None,
            pdrag: None,
            guides: Vec::new(),
            scale_label: None,
            last_arrange: None,
            glide: None,
            live: false,
            peek: false,
            list_open: true,
            list_hits: Vec::new(),
            row_drag: None,
            last_row_click: None,
            layout_naming: None,
            readout_hit: None,
            hover_since: None,
            pulse: None,
            history_names: (None, None),
            reset_held: None,
            reset_card: false,
            reset_hits: Vec::new(),
            history_open: false,
            history_from_hold: false,
            history_hits: Vec::new(),
            history_list: Vec::new(),
            button_press: None,
            guide: None,
            guide_hits: Vec::new(),
            opened: Instant::now(),
            map_open: true,
            map: Vec::new(),
            map_hits: Vec::new(),
            map_rect: None,
            parked: Vec::new(),
            keymap_open: false,
            perf: Vec::new(),
            chrome_rects: Vec::new(),
            overlay_from: usize::MAX,
            keymap_hits: Vec::new(),
        }
    }

    /// Start typing a new name for the active layout in the readout.
    pub fn rename_layout(&mut self, current: &str) {
        self.layout_naming = Some(LayoutNaming { typing: crate::textedit::Typing::new(current, NAME_MAX) });
    }

    /// Run an action from its shortcut: the same as clicking it, plus a pulse on its button.
    fn key_act(&mut self, a: Action, layout: &mut Layout, geo: &Geo) -> Effect {
        self.pulse = Some((a, Instant::now()));
        self.act(a, layout, geo)
    }

    /// A drag that changes the layout as it goes is in progress (not a toolbar press).
    pub fn dragging(&self) -> bool {
        self.pdrag.is_some() || matches!(self.drag, Some(Drag::Move { .. }) | Some(Drag::Resize { .. }))
    }

    /// The engine should show the moving picture rather than the still frame.
    pub fn wants_live(&self) -> bool {
        self.view == View::Arrange && self.live
    }

    pub fn say(&mut self, text: &str) {
        self.message = Some((text.to_string(), Instant::now()));
    }

    pub fn message(&self) -> Option<&str> {
        self.message.as_ref().filter(|(_, t)| t.elapsed() < Duration::from_secs(4)).map(|(m, _)| m.as_str())
    }

    pub fn drawing_rect(&self) -> Option<DrawingRect> {
        match &self.drag {
            Some(Drag::Draw { op, start, now }) => Some((*op, *start, *now)),
            _ => None,
        }
    }

    /// Show the loupe while placing edges.
    pub fn loupe(&self) -> bool {
        matches!(self.drag, Some(Drag::Draw { .. }) | Some(Drag::Resize { .. }))
    }

    pub fn handle(&mut self, input: Input, layout: &mut Layout, geo: &Geo) -> Effect {
        match input {
            Input::Move(x, y) if self.naming.as_mut().is_some_and(|n| n.typing.mouse_move(x)) || self.layout_naming.as_mut().is_some_and(|n| n.typing.mouse_move(x)) => {
                // Selecting text in the name being typed.
                self.pointer = (x, y);
                Effect::None
            }
            Input::Move(x, y) => {
                self.pointer = (x, y);
                self.hover = self.button_at(x, y);
                self.update_hover(x, y);
                if self.row_drag.is_some() {
                    self.list_move(y, layout, geo.scale);
                    return Effect::None;
                }
                let m = crate::shell::canvas::mods();
                // A toolbar press that moves away becomes a press on the picture at that spot.
                if let Some(Drag::Button(_, p0, m0)) = self.drag.clone()
                    && ((x - p0.0).powi(2) + (y - p0.1).powi(2)).sqrt() > 8.0 * geo.scale
                {
                    self.drag = None;
                    self.button_press = None;
                    if self.view == View::Arrange {
                        self.arr_down(p0.0, p0.1, layout, geo);
                    } else {
                        let saved = std::mem::take(&mut self.buttons);
                        self.on_down(p0.0, p0.1, m0, layout, geo);
                        self.buttons = saved;
                    }
                }
                if self.view == View::Arrange { self.arr_move(x, y, layout, geo, m) } else { self.on_move(x, y, layout, geo, m) }
            }
            Input::Down(Button::Left, x, y, m) => {
                self.pointer = (x, y);
                // A click on the name being typed places the caret there.
                if self.naming.as_mut().is_some_and(|n| n.typing.mouse_down(x, y, m.shift)) || self.layout_naming.as_mut().is_some_and(|n| n.typing.mouse_down(x, y, m.shift)) {
                    return Effect::None;
                }
                if self.keymap_open {
                    self.keymap_open = false;
                    return Effect::None;
                }
                if self.reset_card {
                    return self.reset_card_down(x, y, layout, geo);
                }
                if self.history_open {
                    return self.history_down(x, y);
                }
                if let Some(c) = self.guide_hit(x, y) {
                    return self.guide_down(c);
                }
                if self.view == View::Arrange
                    && let Some(i) = self.map_hit(x, y)
                {
                    return Effect::DisplayMenu(i, x, y);
                }
                if let Some(a) = self.button_at(x, y) {
                    self.button_press = Some((a, Instant::now()));
                }
                // A click anywhere but the readout confirms a layout name being typed.
                if self.layout_naming.is_some() && !self.over_readout(x, y) {
                    return self.layout_naming_key(0x0D, Mods::default());
                }
                if self.button_at(x, y).is_none() {
                    if self.over_readout(x, y) {
                        let r = self.readout_hit.unwrap();
                        return Effect::LayoutMenu(r.0, r.1);
                    }
                    if let Some(hit) = self.list_hit(x, y) {
                        if let Some(fx) = self.finish_naming(layout)
                            && fx != Effect::None {
                                return fx;
                            }
                        return self.list_down(hit, y, layout);
                    }
                }
                if self.view == View::Arrange {
                    if let Some(a) = self.button_at(x, y) {
                        self.drag = Some(Drag::Button(a, (x, y), m));
                        return Effect::None;
                    }
                    if let Some(fx) = self.finish_naming(layout) {
                        return fx;
                    }
                    return self.arr_down(x, y, layout, geo);
                }
                self.on_down(x, y, m, layout, geo)
            }
            Input::Down(Button::Right, x, y, _) => {
                // On the cutout list the menu is for the row under the pointer; on any other
                // panel it is for no cutout. Only on the picture itself does the cutout under
                // the pointer count (the list and toolbar sit over the picture).
                let on_panel = self.button_at(x, y).is_some() || self.chrome_rects.iter().any(|r| x >= r.0 && x < r.0 + r.2 && y >= r.1 && y < r.1 + r.3);
                let n = match self.list_hit(x, y) {
                    Some(ListHit::Row(n) | ListHit::Eye(n) | ListHit::Lock(n)) => Some(n),
                    None if on_panel => None,
                    None if self.view == View::Arrange => self.placed_at(x, y, layout, geo),
                    None => self.cutout_at(x, y, layout, geo),
                };
                // The menu acts on the selection, so it must be this cutout, or none.
                self.selected = n;
                Effect::Menu(x, y, n)
            }
            Input::Up(Button::Left, ..) if self.naming.as_mut().is_some_and(|n| n.typing.mouse_up()) || self.layout_naming.as_mut().is_some_and(|n| n.typing.mouse_up()) => Effect::None,
            Input::Up(Button::Left, x, y) => {
                self.button_press = None;
                if self.history_from_hold {
                    // Releasing the long press on Undo leaves the list open; it does not undo.
                    self.history_from_hold = false;
                    self.drag = None;
                    return Effect::None;
                }
                if self.row_drag.is_some() {
                    // A row dropped on the display map sends that cutout there.
                    if let Some(i) = self.map_hit(x, y)
                        && let Some(d) = self.row_drag.take()
                    {
                        return Effect::SendToMap(d.n, i);
                    }
                    return self.list_up(layout);
                }
                if self.view == View::Arrange && self.pdrag.is_some() {
                    return self.arr_up(layout, geo);
                }
                self.on_up(x, y, layout, geo)
            }
            Input::Up(..) => Effect::None,
            Input::Key(vk, m) if self.keymap_open && self.naming.is_none() => {
                if matches!(vk, 0x1B | 0x70) || (vk == 0xBF && m.shift) {
                    self.keymap_open = false;
                }
                Effect::None
            }
            Input::Key(0x70, _) if self.naming.is_none() && self.layout_naming.is_none() => {
                self.keymap_open = true;
                self.pulse = Some((Action::Shortcuts, Instant::now()));
                Effect::None
            }
            Input::Key(0xBF, m) if m.shift && self.naming.is_none() && self.layout_naming.is_none() => {
                self.keymap_open = true;
                Effect::None
            }
            Input::Key(vk, m) if self.layout_naming.is_some() => self.layout_naming_key(vk, m),
            Input::Key(vk, _) if self.reset_card => match vk {
                0x1B => self.choose_reset(ResetChoice::Cancel, layout, geo),
                0x31 => self.choose_reset(ResetChoice::Arrangement, layout, geo),
                0x32 => self.choose_reset(ResetChoice::StartOver, layout, geo),
                _ => Effect::None,
            },
            Input::Key(0x1B, _) if self.history_open => {
                self.history_open = false;
                Effect::None
            }
            Input::Key(0x08, m) if m.ctrl && m.shift => {
                // Hold Ctrl+Shift+Backspace for one second.
                if self.reset_held.is_none() {
                    self.reset_held = Some(Instant::now());
                }
                Effect::None
            }
            Input::Key(vk, m) => self.on_key(vk, m, layout, geo),
            Input::KeyUp(0x20) => {
                self.peek = false;
                Effect::None
            }
            Input::KeyUp(0x08 | 0x10 | 0x11) => {
                // Letting go of Backspace, Shift or Ctrl before the ring fills cancels the reset.
                self.reset_held = None;
                Effect::None
            }
            Input::KeyUp(_) => Effect::None,
            Input::Char(c) if self.layout_naming.is_some() => {
                self.layout_naming_char(c);
                Effect::None
            }
            Input::Char(c) => self.on_char(c, layout),
            Input::LostFocus => {
                self.reset_held = None;
                self.button_press = None;
                self.drag = None;
                self.pdrag = None;
                self.row_drag = None;
                self.snaps.clear();
                self.guides.clear();
                self.peek = false;
                Effect::None
            }
        }
    }

    fn button_at(&self, x: f32, y: f32) -> Option<Action> {
        self.buttons.iter().find(|(r, _, _)| x >= r.0 && x < r.0 + r.2 && y >= r.1 && y < r.1 + r.3).map(|b| b.1)
    }

    /// Topmost cutout under a canvas point (Cut view: source position).
    fn cutout_at(&self, x: f32, y: f32, layout: &Layout, geo: &Geo) -> Option<u32> {
        let (sx, sy) = geo.to_src(x, y);
        let f = (sx / geo.source_px.w, sy / geo.source_px.h);
        layout.by_z().into_iter().rev().find(|c| c.contains(f.0, f.1)).map(|c| c.number)
    }

    /// The selected cutout's handles, in canvas pixels: 0 TL, 1 T, 2 TR, 3 R, 4 BR, 5 B, 6 BL, 7 L.
    pub fn handles(&self, layout: &Layout, geo: &Geo) -> Vec<(f32, f32)> {
        if self.view == View::Arrange {
            return self.arr_handles(layout, geo);
        }
        let Some(c) = self.selected.and_then(|n| layout.cutout(n)) else { return Vec::new() };
        let (x, y, w, h) = geo.to_canvas(c.bounds());
        let (mx, my, r, b) = (x + w / 2.0, y + h / 2.0, x + w, y + h);
        vec![(x, y), (mx, y), (r, y), (r, my), (r, b), (mx, b), (x, b), (x, my)]
    }

    /// A handle under the pointer. Easy to grab from inside the cutout; from outside only when
    /// right on it, so a drag that starts just outside a corner draws a new cutout instead
    /// (neighbouring quadrants share corners).
    fn handle_at(&self, x: f32, y: f32, layout: &Layout, geo: &Geo) -> Option<usize> {
        let c = self.selected.and_then(|n| layout.cutout(n))?;
        let (bx, by, bw, bh) = geo.to_canvas(c.bounds());
        let inside = x >= bx && x <= bx + bw && y >= by && y <= by + bh;
        let reach = if inside { 9.0 } else { 4.0 } * geo.scale;
        self.handles(layout, geo).iter().position(|(hx, hy)| (hx - x).abs() <= reach && (hy - y).abs() <= reach)
    }

    fn on_down(&mut self, x: f32, y: f32, m: Mods, layout: &mut Layout, geo: &Geo) -> Effect {
        if let Some(a) = self.button_at(x, y) {
            self.drag = Some(Drag::Button(a, (x, y), m));
            return Effect::None;
        }
        if let Some(fx) = self.finish_naming(layout) {
            // Clicking elsewhere confirms a name being typed.
            self.drag = None;
            return fx;
        }
        let p = geo.to_src(x, y);
        let p = self.clamp_src(p, geo);
        let selected_unlocked = self.selected.filter(|n| layout.cutout(*n).is_some_and(|c| !c.placement.locked));

        if let Some(n) = selected_unlocked {
            if m.shift || m.alt {
                let op = if m.alt { ShapeOp::Subtract } else { ShapeOp::Add };
                let s = self.snap_point(p, layout, geo, None, m.ctrl);
                self.drag = Some(Drag::Draw { op: Some(op), start: s, now: s });
                return Effect::None;
            }
            if let Some(h) = self.handle_at(x, y, layout, geo) {
                let c = layout.cutout(n).unwrap();
                let b = c.bounds().scale(geo.source_px);
                let outside = !(p.0 >= b.x && p.0 <= b.right() && p.1 >= b.y && p.1 <= b.bottom());
                self.drag = Some(Drag::Resize { n, handle: h, orig: c.rects.clone(), outside, press: p });
                return Effect::None;
            }
        }
        if let Some(n) = self.cutout_at(x, y, layout, geo) {
            self.selected = Some(n);
            if !layout.cutout(n).unwrap().placement.locked {
                self.drag = Some(Drag::Move { n, grab: p, orig: layout.cutout(n).unwrap().rects.clone() });
            }
            return Effect::None;
        }
        let s = self.snap_point(p, layout, geo, None, m.ctrl);
        self.drag = Some(Drag::Draw { op: None, start: s, now: s });
        Effect::None
    }

    fn clamp_src(&self, p: (f64, f64), geo: &Geo) -> (f64, f64) {
        (p.0.clamp(0.0, geo.source_px.w), p.1.clamp(0.0, geo.source_px.h))
    }

    /// Snap targets in source pixels: the source's edges, halves and thirds, and the edges of
    /// other cutouts.
    fn targets(&self, layout: &Layout, geo: &Geo, except: Option<u32>) -> (Vec<f64>, Vec<f64>) {
        let (w, h) = (geo.source_px.w, geo.source_px.h);
        let mut xs = vec![0.0, w / 3.0, w / 2.0, 2.0 * w / 3.0, w];
        let mut ys = vec![0.0, h / 3.0, h / 2.0, 2.0 * h / 3.0, h];
        for c in &layout.cutouts {
            if Some(c.number) == except {
                continue;
            }
            let b = c.bounds().scale(geo.source_px);
            xs.extend([b.x, b.right()]);
            ys.extend([b.y, b.bottom()]);
        }
        (xs, ys)
    }

    fn reach(&self, geo: &Geo) -> f64 {
        SNAP_PX * geo.scale as f64 / geo.zoom()
    }

    fn snap_one(v: f64, targets: &[f64], reach: f64) -> Option<f64> {
        targets.iter().copied().filter(|t| (t - v).abs() <= reach).min_by(|a, b| (a - v).abs().total_cmp(&(b - v).abs()))
    }

    fn snap_point(&mut self, p: (f64, f64), layout: &Layout, geo: &Geo, except: Option<u32>, suspend: bool) -> (f64, f64) {
        self.snaps.clear();
        if !self.snapping || suspend {
            return p;
        }
        let (xs, ys) = self.targets(layout, geo, except);
        let r = self.reach(geo);
        let mut out = p;
        if let Some(x) = Self::snap_one(p.0, &xs, r) {
            out.0 = x;
            self.snaps.push(SnapLine::X(x));
        }
        if let Some(y) = Self::snap_one(p.1, &ys, r) {
            out.1 = y;
            self.snaps.push(SnapLine::Y(y));
        }
        out
    }

    fn on_move(&mut self, x: f32, y: f32, layout: &mut Layout, geo: &Geo, m: Mods) -> Effect {
        let p = self.clamp_src(geo.to_src(x, y), geo);
        match self.drag.clone() {
            Some(Drag::Draw { op, start, .. }) => {
                let now = self.snap_point(p, layout, geo, None, m.ctrl);
                self.drag = Some(Drag::Draw { op, start, now });
            }
            Some(Drag::Move { n, grab, orig }) => {
                let c = layout.cutout_mut(n).unwrap();
                c.rects = orig.clone();
                let b = c.bounds().scale(geo.source_px);
                let (mut dx, mut dy) = (p.0 - grab.0, p.1 - grab.1);
                // Snap whichever edge of the moved cut is closest to a target.
                self.snaps.clear();
                if self.snapping && !m.ctrl {
                    let (xs, ys) = self.targets(layout, geo, Some(n));
                    let r = self.reach(geo);
                    let best = |edges: [f64; 2], ts: &[f64]| {
                        edges.iter().filter_map(|e| Self::snap_one(*e, ts, r).map(|t| (t - e, t))).min_by(|a, b| a.0.abs().total_cmp(&b.0.abs()))
                    };
                    if let Some((d, t)) = best([b.x + dx, b.right() + dx], &xs) {
                        dx += d;
                        self.snaps.push(SnapLine::X(t));
                    }
                    if let Some((d, t)) = best([b.y + dy, b.bottom() + dy], &ys) {
                        dy += d;
                        self.snaps.push(SnapLine::Y(t));
                    }
                }
                layout.move_cut(n, dx / geo.source_px.w, dy / geo.source_px.h);
            }
            Some(Drag::Resize { n, handle, orig, outside, press }) => {
                let q = self.snap_point(p, layout, geo, Some(n), m.ctrl);
                let old = Self::bounds_of(&orig).scale(geo.source_px);
                // Started just outside a corner and heading across the far edge: the user is
                // drawing the neighbouring cutout, not resizing this one.
                let crosses = (matches!(handle, 0 | 6 | 7) && q.0 >= old.right())
                    || (matches!(handle, 2..=4) && q.0 <= old.x)
                    || (matches!(handle, 0..=2) && q.1 >= old.bottom())
                    || (matches!(handle, 4..=6) && q.1 <= old.y);
                if outside && crosses {
                    layout.cutout_mut(n).unwrap().rects = orig;
                    let start = self.snap_point(press, layout, geo, None, m.ctrl);
                    let now = self.snap_point(p, layout, geo, None, m.ctrl);
                    self.drag = Some(Drag::Draw { op: None, start, now });
                    return Effect::None;
                }
                let (mut l, mut t, mut r, mut b) = (old.x, old.y, old.right(), old.bottom());
                if matches!(handle, 0 | 6 | 7) {
                    l = q.0;
                }
                if matches!(handle, 2..=4) {
                    r = q.0;
                }
                if matches!(handle, 0..=2) {
                    t = q.1;
                }
                if matches!(handle, 4..=6) {
                    b = q.1;
                }
                // Dragging an edge past the opposite one flips the cut rather than squashing it.
                let (l, r) = (l.min(r), l.max(r).max(l.min(r) + 1.0));
                let (t, b) = (t.min(b), t.max(b).max(t.min(b) + 1.0));
                let new = Rect::new(l, t, r - l, b - t).unscale(geo.source_px);
                layout.cutout_mut(n).unwrap().rects = Self::refit(&orig, new);
            }
            _ => {}
        }
        Effect::None
    }

    fn bounds_of(rects: &[ShapeRect]) -> Rect {
        rects.iter().filter(|r| r.op == ShapeOp::Add).fold(Rect::default(), |a, r| a.union(&r.rect))
    }

    /// Scale every rect of a shape from its old bounding box to a new one.
    fn refit(orig: &[ShapeRect], new: Rect) -> Vec<ShapeRect> {
        let old = Self::bounds_of(orig);
        orig.iter()
            .map(|r| {
                let fx = |v: f64| new.x + (v - old.x) / old.w * new.w;
                let fy = |v: f64| new.y + (v - old.y) / old.h * new.h;
                let x = fx(r.rect.x);
                let y = fy(r.rect.y);
                ShapeRect { op: r.op, rect: Rect::new(x, y, fx(r.rect.right()) - x, fy(r.rect.bottom()) - y) }
            })
            .collect()
    }

    fn on_up(&mut self, x: f32, y: f32, layout: &mut Layout, geo: &Geo) -> Effect {
        let drag = self.drag.take();
        self.snaps.clear();
        match drag {
            Some(Drag::Button(a, ..)) => {
                if self.button_at(x, y) == Some(a) && self.buttons.iter().any(|b| b.1 == a && b.2) {
                    self.act(a, layout, geo)
                } else {
                    Effect::None
                }
            }
            Some(Drag::Draw { op, start, now }) => {
                let r = Rect::new(start.0.min(now.0), start.1.min(now.1), (now.0 - start.0).abs(), (now.1 - start.1).abs());
                if r.w < MIN_CUT || r.h < MIN_CUT {
                    if op.is_none() {
                        self.selected = None;
                    }
                    return Effect::None;
                }
                let f = r.unscale(geo.source_px);
                match op {
                    None => {
                        let n = layout.add_cutout(f, &geo.frame());
                        self.selected = Some(n);
                        let name = layout.cutout(n).unwrap().name.clone();
                        self.naming = Some(Naming::new(n, &name));
                        Effect::Edit(format!("New cutout {n}"), None)
                    }
                    Some(op) => {
                        let n = self.selected.unwrap();
                        layout.edit_shape(n, op, f);
                        let name = layout.cutout(n).unwrap().name.clone();
                        Effect::Edit(format!("{} {name}", if op == ShapeOp::Add { "Add rectangle to" } else { "Subtract rectangle from" }), None)
                    }
                }
            }
            Some(Drag::Move { n, .. }) => Effect::Edit(format!("Move cut {}", name_of(layout, n)), None),
            Some(Drag::Resize { n, .. }) => Effect::Edit(format!("Resize cut {}", name_of(layout, n)), None),
            None => Effect::None,
        }
    }

    /// Run a toolbar or keyboard action.
    pub fn act(&mut self, a: Action, layout: &mut Layout, _geo: &Geo) -> Effect {
        match a {
            Action::Cut => {
                self.view = View::Cut;
                self.pdrag = None;
                self.guides.clear();
                Effect::None
            }
            Action::Arrange => {
                self.view = View::Arrange;
                self.drag = None;
                self.snaps.clear();
                Effect::None
            }
            Action::Fill => self.fill(layout, _geo),
            Action::AutoArrange => self.auto_arrange(layout, _geo),
            Action::ResetPlacement => self.reset_placement(layout, _geo),
            Action::List => {
                self.list_open = !self.list_open;
                Effect::None
            }
            Action::Live => {
                self.live = !self.live;
                self.say(if self.live { "Live: the moving picture" } else { "Edit: the still frame" });
                Effect::None
            }
            Action::Undo => Effect::Undo,
            Action::Reset => {
                self.reset_card = true;
                self.reset_held = None;
                Effect::None
            }
            Action::Redo => Effect::Redo,
            Action::Retake => {
                self.say("New still frame taken");
                Effect::Retake
            }
            Action::Grids => {
                let (x, y) = self.buttons.iter().find(|b| b.1 == Action::Grids).map(|b| (b.0.0, b.0.1 + b.0.3)).unwrap_or(self.pointer);
                Effect::Menu(x, y, None)
            }
            Action::Hide => match self.selected {
                Some(n) if layout.cutout(n).is_some() => {
                    layout.toggle_visible(n);
                    let c = layout.cutout(n).unwrap();
                    Effect::Edit(format!("{} {}", if c.placement.visible { "Show" } else { "Hide" }, c.name), None)
                }
                _ => Effect::None,
            },
            Action::Delete => match self.selected.take() {
                Some(n) if layout.cutout(n).is_some() => {
                    let name = name_of(layout, n);
                    layout.delete(n);
                    self.naming = None;
                    Effect::Edit(format!("Delete {name}"), None)
                }
                _ => Effect::None,
            },
            Action::Snapping => {
                self.snapping = !self.snapping;
                self.say(if self.snapping { "Snapping on" } else { "Snapping off (hold Ctrl to suspend it while dragging)" });
                Effect::None
            }
            Action::Suggest => Effect::Suggest,
            Action::Trim => match self.selected.filter(|n| layout.cutout(*n).is_some()) {
                Some(n) => Effect::Trim(n),
                None => {
                    self.say("Select a cutout to trim its black edges");
                    Effect::None
                }
            },
            Action::Shortcuts => {
                self.keymap_open = !self.keymap_open;
                Effect::None
            }
            Action::Profiles => Effect::Profiles,
            Action::Settings => Effect::Settings,
            Action::Close => Effect::Close,
        }
    }

    /// Choose how a cutout is scaled.
    pub fn set_filter(&mut self, n: u32, f: crate::model::layout::Filter, layout: &mut Layout) -> Effect {
        let Some(c) = layout.cutout_mut(n) else { return Effect::None };
        if c.placement.filter == f {
            return Effect::None;
        }
        c.placement.filter = f;
        let word = match f {
            crate::model::layout::Filter::Auto => "Auto",
            crate::model::layout::Filter::Smooth => "Smooth",
            crate::model::layout::Filter::Sharp => "Sharp",
            crate::model::layout::Filter::Enhanced => "Enhanced",
        };
        Effect::Edit(format!("Scaling of {}: {word}", c.name), None)
    }

    /// Add a ready-made grid of cutouts.
    pub fn add_preset(&mut self, p: Preset, layout: &mut Layout, geo: &Geo) -> Effect {
        let mut last = None;
        for (name, r) in preset(p) {
            let n = layout.add_cutout(r, &geo.frame());
            layout.rename(n, name);
            last = Some(n);
        }
        self.selected = last;
        Effect::Edit(format!("Add grid: {}", p.label()), None)
    }

    fn finish_naming(&mut self, layout: &mut Layout) -> Option<Effect> {
        let nm = self.naming.take()?;
        let old = name_of(layout, nm.n);
        let text = nm.typing.text().trim().to_string();
        if !text.is_empty() && text != old {
            layout.rename(nm.n, &text);
            return Some(Effect::Edit(format!("Rename {old} to {text}"), None));
        }
        Some(Effect::None)
    }

    fn on_char(&mut self, c: char, _layout: &mut Layout) -> Effect {
        if let Some(nm) = self.naming.as_mut() {
            nm.typing.edit.char(c);
        }
        Effect::None
    }

    fn on_key(&mut self, vk: u16, m: Mods, layout: &mut Layout, geo: &Geo) -> Effect {
        const VK_TAB: u16 = 0x09;
        const VK_RETURN: u16 = 0x0D;
        const VK_ESCAPE: u16 = 0x1B;
        const VK_DELETE: u16 = 0x2E;
        const VK_F2: u16 = 0x71;
        const VK_F5: u16 = 0x74;
        const VK_LEFT: u16 = 0x25;
        const VK_DOWN: u16 = 0x28;

        // While typing a name, keys edit the text except Enter and Escape; none of them
        // reaches the layer's shortcuts.
        if let Some(nm) = self.naming.as_mut() {
            match vk {
                VK_RETURN => return self.finish_naming(layout).unwrap_or(Effect::None),
                VK_ESCAPE => {
                    let n = nm.n;
                    self.naming = None;
                    self.say(&format!("Kept the name \"{}\"", name_of(layout, n)));
                    return Effect::None;
                }
                _ => {
                    nm.typing.edit.key_with_clipboard(vk, m);
                    return Effect::None;
                }
            }
        }
        if self.view == View::Arrange {
            match vk {
                0x20 => {
                    self.peek = true;
                    return Effect::None;
                }
                0x46 if !m.ctrl => return self.key_act(Action::Fill, layout, geo),
                0x41 if !m.ctrl => return self.key_act(Action::AutoArrange, layout, geo),
                0x52 if !m.ctrl => return self.key_act(Action::ResetPlacement, layout, geo),
                0x56 if !m.ctrl => return self.key_act(Action::Live, layout, geo),
                0x51 | 0x45 if !m.ctrl => {
                    // Q and E: a quarter turn left or right.
                    let Some(n) = self.selected.filter(|n| layout.cutout(*n).is_some_and(|c| !c.placement.locked && c.placement.display == geo.display)) else { return Effect::None };
                    let cw = vk == 0x45;
                    layout.rotate(n, cw, geo.canvas);
                    return Effect::Edit(format!("Rotate {} {}", name_of(layout, n), if cw { "right" } else { "left" }), None);
                }
                0xDD | 0xDB => {
                    let up = vk == 0xDD;
                    return match self.selected.filter(|n| layout.cutout(*n).is_some()) {
                        Some(n) => {
                            layout.restack(n, up);
                            Effect::Edit(format!("{} {}", if up { "Bring forward" } else { "Send backward" }, name_of(layout, n)), None)
                        }
                        None => Effect::None,
                    };
                }
                k @ VK_LEFT..=VK_DOWN => return self.arr_nudge(k, m, layout, geo),
                VK_F5 => return Effect::None,
                _ => {}
            }
        }
        match vk {
            VK_ESCAPE => {
                if self.drag.is_some() || self.pdrag.is_some() {
                    self.drag = None;
                    self.pdrag = None;
                    self.snaps.clear();
                    self.guides.clear();
                    Effect::None
                } else if self.selected.take().is_some() {
                    Effect::None
                } else {
                    Effect::Close
                }
            }
            VK_TAB => self.key_act(if self.view == View::Cut { Action::Arrange } else { Action::Cut }, layout, geo),
            VK_F5 => self.key_act(Action::Retake, layout, geo),
            0x4C if !m.ctrl => self.key_act(Action::List, layout, geo),
            0x50 if !m.ctrl => Effect::Profiles,
            0xBC if m.ctrl => Effect::Settings,
            0x53 if !m.ctrl && self.view == View::Cut => self.key_act(Action::Suggest, layout, geo),
            0x54 if !m.ctrl && self.view == View::Cut => self.key_act(Action::Trim, layout, geo),
            0x4D if !m.ctrl => {
                self.map_open = !self.map_open;
                Effect::None
            }
            0x44 if m.ctrl => match self.selected.filter(|n| layout.cutout(*n).is_some()) {
                Some(n) => {
                    let new = layout.duplicate(n).unwrap();
                    self.selected = Some(new);
                    Effect::Edit(format!("Duplicate {}", name_of(layout, n)), None)
                }
                None => Effect::None,
            },
            0x44 if self.view == View::Arrange => match self.selected.filter(|n| layout.cutout(*n).is_some() && !self.parked.contains(n)) {
                Some(n) => Effect::SendToNext(n),
                None => Effect::None,
            },
            VK_RETURN | VK_F2 => {
                if let Some(n) = self.selected.filter(|n| layout.cutout(*n).is_some()) {
                    self.naming = Some(Naming::new(n, &name_of(layout, n)));
                }
                Effect::None
            }
            VK_DELETE => self.key_act(Action::Delete, layout, geo),
            0x5A if m.ctrl && m.shift => self.key_act(Action::Redo, layout, geo),
            0x5A if m.ctrl => self.key_act(Action::Undo, layout, geo),
            0x59 if m.ctrl => self.key_act(Action::Redo, layout, geo),
            0x48 if !m.ctrl => self.key_act(Action::Hide, layout, geo),
            0x47 if !m.ctrl => self.key_act(Action::Snapping, layout, geo),
            0x4B if !m.ctrl => match self.selected {
                Some(n) if layout.cutout(n).is_some() => {
                    layout.toggle_locked(n);
                    let c = layout.cutout(n).unwrap();
                    Effect::Edit(format!("{} {}", if c.placement.locked { "Lock" } else { "Unlock" }, c.name), None)
                }
                _ => Effect::None,
            },
            k @ 0x31..=0x39 if !m.ctrl => {
                let n = (k - 0x30) as u32;
                if layout.cutout(n).is_some() {
                    self.selected = Some(n);
                }
                Effect::None
            }
            k @ VK_LEFT..=VK_DOWN => {
                let Some(n) = self.selected.filter(|n| layout.cutout(*n).is_some_and(|c| !c.placement.locked)) else { return Effect::None };
                let step = if m.shift { 10.0 } else { 1.0 };
                let (dx, dy) = match k {
                    0x25 => (-step, 0.0),
                    0x26 => (0.0, -step),
                    0x27 => (step, 0.0),
                    _ => (0.0, step),
                };
                let name = name_of(layout, n);
                if m.ctrl {
                    // Grow or shrink from the bottom-right corner.
                    let orig = layout.cutout(n).unwrap().rects.clone();
                    let b = Self::bounds_of(&orig).scale(geo.source_px);
                    let new = Rect::new(b.x, b.y, (b.w + dx).max(1.0), (b.h + dy).max(1.0)).unscale(geo.source_px);
                    layout.cutout_mut(n).unwrap().rects = Self::refit(&orig, new.clamp_unit());
                    Effect::Edit(format!("Resize cut {name}"), Some(format!("nudge-size-{n}")))
                } else {
                    layout.move_cut(n, dx / geo.source_px.w, dy / geo.source_px.h);
                    Effect::Edit(format!("Nudge {name}"), Some(format!("nudge-{n}")))
                }
            }
            _ => Effect::None,
        }
    }
}

fn name_of(layout: &Layout, n: u32) -> String {
    layout.cutout(n).map(|c| c.name.clone()).unwrap_or_else(|| format!("Cutout {n}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::layout::{DisplayMode, DisplayRef};

    fn setup() -> (Layer, Layout, Geo) {
        let d = DisplayRef { hardware_id: "x".into(), label: "Display 1".into(), mode: DisplayMode::Takeover };
        // A 1920×1080 source shown at (0,0) on a 3840×2160 canvas: 2 canvas px per source px.
        let geo = Geo { display: "main".into(), here: true, number: 0, canvas: Size::new(3840.0, 2160.0), source: Rect::new(0.0, 0.0, 3840.0, 2160.0), source_px: Size::new(1920.0, 1080.0), scale: 2.0 };
        (Layer::new(1), Layout::new("l", "L", d), geo)
    }

    fn drag(l: &mut Layer, layout: &mut Layout, geo: &Geo, from: (f32, f32), to: (f32, f32), m: Mods) -> Effect {
        l.handle(Input::Down(Button::Left, from.0, from.1, m), layout, geo);
        l.on_move(to.0, to.1, layout, geo, m);
        l.handle(Input::Up(Button::Left, to.0, to.1), layout, geo)
    }

    #[test]
    fn a_rough_drag_snaps_to_the_top_right_quadrant() {
        // At stand-in size: drag from near the centre to near the top
        // right corner; the edges snap to the centre lines and the source edges.
        let (mut l, mut layout, geo) = setup();
        let fx = drag(&mut l, &mut layout, &geo, (1928.0, 1074.0), (3836.0, 6.0), Mods::default());
        assert_eq!(fx, Effect::Edit("New cutout 1".into(), None));
        let b = layout.cutout(1).unwrap().bounds().scale(geo.source_px);
        assert_eq!((b.x, b.y, b.w, b.h), (960.0, 0.0, 960.0, 540.0));
        assert!(l.naming.is_some(), "the tag becomes a name field");
    }

    #[test]
    fn typing_replaces_the_default_name_and_enter_confirms() {
        let (mut l, mut layout, geo) = setup();
        drag(&mut l, &mut layout, &geo, (1928.0, 1074.0), (3836.0, 6.0), Mods::default());
        for c in "Main view".chars() {
            l.handle(Input::Char(c), &mut layout, &geo);
        }
        let fx = l.handle(Input::Key(0x0D, Mods::default()), &mut layout, &geo);
        assert_eq!(fx, Effect::Edit("Rename Cutout 1 to Main view".into(), None));
        assert_eq!(layout.cutout(1).unwrap().name, "Main view");
    }

    #[test]
    fn a_name_edits_like_a_text_box_and_no_shortcut_fires() {
        let (mut l, mut layout, geo) = setup();
        drag(&mut l, &mut layout, &geo, (1928.0, 1074.0), (3836.0, 6.0), Mods::default());
        let none = Mods::default();
        // Keys that are shortcuts outside a text box only edit the name here.
        for (vk, m) in [(0x25, none), (0x24, none), (0x2E, none), (0x48, none), (0x09, none), (0x5A, Mods { ctrl: true, ..none })] {
            assert_eq!(l.handle(Input::Key(vk, m), &mut layout, &geo), Effect::None);
        }
        assert!(layout.cutout(1).is_some(), "Delete removed a character, not the cutout");
        assert!(layout.cutout(1).unwrap().placement.visible, "H did not hide it");
        assert_eq!(l.naming.as_ref().unwrap().typing.text(), "Cutout 1", "Ctrl+Z undid the Delete");
        // Ctrl+A, typing, Home, Ctrl+Shift+Right and typing again.
        l.handle(Input::Key(0x41, Mods { ctrl: true, ..none }), &mut layout, &geo);
        for c in "Main view".chars() {
            l.handle(Input::Char(c), &mut layout, &geo);
        }
        l.handle(Input::Key(0x24, none), &mut layout, &geo);
        l.handle(Input::Key(0x27, Mods { ctrl: true, shift: true, ..none }), &mut layout, &geo);
        for c in "Side ".chars() {
            l.handle(Input::Char(c), &mut layout, &geo);
        }
        let fx = l.handle(Input::Key(0x0D, none), &mut layout, &geo);
        assert_eq!(fx, Effect::Edit("Rename Cutout 1 to Side view".into(), None));
    }

    #[test]
    fn a_click_on_the_name_being_typed_places_the_caret() {
        let (mut l, mut layout, geo) = setup();
        drag(&mut l, &mut layout, &geo, (1928.0, 1074.0), (3836.0, 6.0), Mods::default());
        for c in "Main view".chars() {
            l.handle(Input::Char(c), &mut layout, &geo);
        }
        l.draw(&layout, &geo, "t");
        let nm = l.naming.as_ref().unwrap();
        let (x, y) = (nm.typing_rect().0 + 1.0, nm.typing_rect().1 + 4.0);
        assert_eq!(l.handle(Input::Down(Button::Left, x, y, Mods::default()), &mut layout, &geo), Effect::None);
        l.handle(Input::Up(Button::Left, x, y), &mut layout, &geo);
        assert!(l.naming.is_some(), "still typing");
        assert_eq!(l.naming.as_ref().unwrap().typing.edit.caret(), 0);
    }

    #[test]
    fn escape_keeps_the_default_name() {
        let (mut l, mut layout, geo) = setup();
        drag(&mut l, &mut layout, &geo, (100.0, 100.0), (500.0, 400.0), Mods::default());
        l.handle(Input::Char('x'), &mut layout, &geo);
        l.handle(Input::Key(0x1B, Mods::default()), &mut layout, &geo);
        assert_eq!(layout.cutout(1).unwrap().name, "Cutout 1");
        assert!(l.naming.is_none());
    }

    #[test]
    fn ctrl_suspends_snapping() {
        let (mut l, mut layout, geo) = setup();
        let ctrl = Mods { ctrl: true, ..Default::default() };
        drag(&mut l, &mut layout, &geo, (1928.0, 1074.0), (3836.0, 6.0), ctrl);
        let b = layout.cutout(1).unwrap().bounds().scale(geo.source_px);
        assert_eq!((b.x, b.y), (964.0, 3.0));
    }

    #[test]
    fn small_drags_are_clicks() {
        let (mut l, mut layout, geo) = setup();
        let fx = drag(&mut l, &mut layout, &geo, (300.0, 300.0), (306.0, 330.0), Mods::default());
        assert_eq!(fx, Effect::None);
        assert!(layout.cutouts.is_empty());
    }

    #[test]
    fn nudge_hide_delete_and_select_by_number() {
        let (mut l, mut layout, geo) = setup();
        drag(&mut l, &mut layout, &geo, (100.0, 100.0), (900.0, 700.0), Mods::default());
        l.handle(Input::Key(0x0D, Mods::default()), &mut layout, &geo); // confirm default name
        l.handle(Input::Key(0x1B, Mods::default()), &mut layout, &geo); // deselect
        l.handle(Input::Key(0x31, Mods::default()), &mut layout, &geo); // 1
        assert_eq!(l.selected, Some(1));
        let before = layout.cutout(1).unwrap().bounds().scale(geo.source_px);
        let fx = l.handle(Input::Key(0x27, Mods { shift: true, ..Default::default() }), &mut layout, &geo);
        assert_eq!(fx, Effect::Edit("Nudge Cutout 1".into(), Some("nudge-1".into())));
        let after = layout.cutout(1).unwrap().bounds().scale(geo.source_px);
        assert!((after.x - before.x - 10.0).abs() < 1e-6);
        let fx = l.handle(Input::Key(0x48, Mods::default()), &mut layout, &geo);
        assert_eq!(fx, Effect::Edit("Hide Cutout 1".into(), None));
        assert!(!layout.cutout(1).unwrap().placement.visible);
        let fx = l.handle(Input::Key(0x2E, Mods::default()), &mut layout, &geo);
        assert_eq!(fx, Effect::Edit("Delete Cutout 1".into(), None));
        assert!(layout.cutouts.is_empty());
    }

    #[test]
    fn handles_resize_and_drag_moves() {
        let (mut l, mut layout, geo) = setup();
        let off = Mods { ctrl: true, ..Default::default() }; // no snapping, for exact numbers
        drag(&mut l, &mut layout, &geo, (100.0, 100.0), (900.0, 700.0), off);
        l.handle(Input::Key(0x0D, Mods::default()), &mut layout, &geo);
        // Bottom-right handle at canvas (900, 700): drag it to (1100, 900).
        l.handle(Input::Down(Button::Left, 900.0, 700.0, off), &mut layout, &geo);
        l.on_move(1100.0, 900.0, &mut layout, &geo, off);
        assert_eq!(l.handle(Input::Up(Button::Left, 1100.0, 900.0), &mut layout, &geo), Effect::Edit("Resize cut Cutout 1".into(), None));
        let b = layout.cutout(1).unwrap().bounds().scale(geo.source_px).rounded(6);
        assert_eq!((b.x, b.y, b.w, b.h), (50.0, 50.0, 500.0, 400.0));
        // Drag from inside: moves the whole cut.
        l.handle(Input::Down(Button::Left, 400.0, 400.0, off), &mut layout, &geo);
        l.on_move(600.0, 500.0, &mut layout, &geo, off);
        assert_eq!(l.handle(Input::Up(Button::Left, 600.0, 500.0), &mut layout, &geo), Effect::Edit("Move cut Cutout 1".into(), None));
        let b = layout.cutout(1).unwrap().bounds().scale(geo.source_px).rounded(6);
        assert_eq!((b.x, b.y), (150.0, 100.0));
    }

    #[test]
    fn shift_and_alt_drags_build_shapes() {
        let (mut l, mut layout, geo) = setup();
        let off = Mods { ctrl: true, ..Default::default() };
        drag(&mut l, &mut layout, &geo, (100.0, 100.0), (900.0, 700.0), off);
        l.handle(Input::Key(0x0D, Mods::default()), &mut layout, &geo);
        let fx = drag(&mut l, &mut layout, &geo, (100.0, 700.0), (500.0, 1000.0), Mods { shift: true, ctrl: true, alt: false });
        assert_eq!(fx, Effect::Edit("Add rectangle to Cutout 1".into(), None));
        let fx = drag(&mut l, &mut layout, &geo, (800.0, 100.0), (900.0, 200.0), Mods { alt: true, ctrl: true, shift: false });
        assert_eq!(fx, Effect::Edit("Subtract rectangle from Cutout 1".into(), None));
        assert_eq!(layout.cutout(1).unwrap().rects.len(), 3);
    }

    #[test]
    fn a_drag_starting_just_outside_a_shared_corner_draws_a_new_cutout() {
        // As in the README example: the graph (top left) is selected and hidden; the Score
        // drag starts just below its bottom-left corner.
        let (mut l, mut layout, geo) = setup();
        drag(&mut l, &mut layout, &geo, (6.0, 6.0), (1916.0, 1076.0), Mods::default());
        l.handle(Input::Key(0x0D, Mods::default()), &mut layout, &geo);
        assert_eq!(l.selected, Some(1));
        // 6 canvas pixels right of and below the corner, as in the live run at 200% scaling.
        let fx = drag(&mut l, &mut layout, &geo, (6.0, 1086.0), (1916.0, 2154.0), Mods::default());
        assert_eq!(fx, Effect::Edit("New cutout 2".into(), None));
        let b = layout.cutout(1).unwrap().bounds().scale(geo.source_px).rounded(6);
        assert_eq!((b.x, b.y, b.w, b.h), (0.0, 0.0, 960.0, 540.0), "the first cutout is untouched");
    }

    #[test]
    fn resizing_past_the_opposite_edge_flips() {
        let (mut l, mut layout, geo) = setup();
        let off = Mods { ctrl: true, ..Default::default() };
        drag(&mut l, &mut layout, &geo, (100.0, 100.0), (900.0, 700.0), off);
        l.handle(Input::Key(0x0D, Mods::default()), &mut layout, &geo);
        // Drag the left handle (100, 400) to the right, past the right edge (900).
        drag(&mut l, &mut layout, &geo, (100.0, 400.0), (1300.0, 400.0), off);
        let b = layout.cutout(1).unwrap().bounds().scale(geo.source_px).rounded(6);
        assert_eq!((b.x, b.w), (450.0, 200.0));
    }

    /// Two cutouts like the README example, at stand-in size, then Tab to Arrange.
    fn arranged() -> (Layer, Layout, Geo) {
        let (mut l, mut layout, geo) = setup();
        let off = Mods { ctrl: true, ..Default::default() };
        drag(&mut l, &mut layout, &geo, (1920.0, 0.0), (3840.0, 1080.0), off); // 960x540 at 960,0
        l.handle(Input::Key(0x0D, Mods::default()), &mut layout, &geo);
        drag(&mut l, &mut layout, &geo, (1920.0, 1164.0), (3840.0, 2160.0), off); // 960x498 at 960,582
        l.handle(Input::Key(0x0D, Mods::default()), &mut layout, &geo);
        l.handle(Input::Key(0x1B, Mods::default()), &mut layout, &geo); // deselect
        l.handle(Input::Key(0x09, Mods::default()), &mut layout, &geo); // Tab
        assert_eq!(l.view, View::Arrange);
        (l, layout, geo)
    }

    fn placed(layout: &Layout, n: u32, geo: &Geo) -> (i64, i64, i64, i64) {
        let r = layout.cutout(n).unwrap().placement.rect.scale(geo.canvas);
        (r.x.round() as i64, r.y.round() as i64, r.w.round() as i64, r.h.round() as i64)
    }

    #[test]
    fn arrange_starts_at_source_positions_and_tab_goes_back() {
        let (mut l, mut layout, geo) = arranged();
        // The source fills the canvas at 2 canvas px per source px, so 100% of the source is 200%.
        assert_eq!(placed(&layout, 1, &geo), (1920, 0, 1920, 1080));
        l.handle(Input::Key(0x09, Mods::default()), &mut layout, &geo);
        assert_eq!(l.view, View::Cut);
    }

    #[test]
    fn dragging_moves_and_snaps_to_the_canvas_centre() {
        let (mut l, mut layout, geo) = arranged();
        // Drag Main view left by 1000 px without snapping, then nudge its centre near the
        // canvas centre with snapping on.
        let off = Mods { ctrl: true, ..Default::default() };
        l.handle(Input::Down(Button::Left, 2880.0, 540.0, off), &mut layout, &geo);
        l.arr_move(1880.0, 540.0, &mut layout, &geo, off);
        assert_eq!(l.handle(Input::Up(Button::Left, 1880.0, 540.0), &mut layout, &geo), Effect::Edit("Move Cutout 1".into(), None));
        assert_eq!(placed(&layout, 1, &geo), (920, 0, 1920, 1080));
        // Its centre is at 1880; moving it 30 px right puts it 10 px from the canvas centre.
        l.handle(Input::Down(Button::Left, 1880.0, 540.0, Mods::default()), &mut layout, &geo);
        l.arr_move(1910.0, 540.0, &mut layout, &geo, Mods::default());
        assert!(l.guides.contains(&arrange::Guide::X(1920.0)));
        l.handle(Input::Up(Button::Left, 1910.0, 540.0), &mut layout, &geo);
        assert_eq!(placed(&layout, 1, &geo).0, 960, "centre snapped to the canvas centre");
    }

    #[test]
    fn corner_resize_keeps_the_shape_and_snaps_to_150_percent() {
        let (mut l, mut layout, geo) = arranged();
        l.handle(Input::Key(0x31, Mods::default()), &mut layout, &geo); // select 1
        // Bottom-right handle at (3840, 1080). 150% of 960 px is 1440 px wide.
        l.handle(Input::Down(Button::Left, 3838.0, 1078.0, Mods::default()), &mut layout, &geo);
        l.arr_move(1920.0 + 1452.0, 815.0, &mut layout, &geo, Mods::default());
        assert_eq!(l.handle(Input::Up(Button::Left, 3372.0, 900.0), &mut layout, &geo), Effect::Edit("Resize Cutout 1".into(), None));
        assert_eq!(placed(&layout, 1, &geo), (1920, 0, 1440, 810));
    }

    #[test]
    fn edge_handles_stretch_one_side_only() {
        let (mut l, mut layout, geo) = arranged();
        l.handle(Input::Key(0x31, Mods::default()), &mut layout, &geo); // select 1
        let (x, y, w, h) = placed(&layout, 1, &geo);
        // Right edge: the width alone follows the pointer.
        l.handle(Input::Down(Button::Left, (x + w) as f32 - 2.0, (y + h / 2) as f32, Mods::default()), &mut layout, &geo);
        l.arr_move(3400.0, 700.0, &mut layout, &geo, Mods::default());
        assert_eq!(l.handle(Input::Up(Button::Left, 3400.0, 700.0), &mut layout, &geo), Effect::Edit("Resize Cutout 1".into(), None));
        assert_eq!(placed(&layout, 1, &geo), (x, y, 3400 - x, h));
        // Bottom edge: the height alone.
        let (x, y, w, _) = placed(&layout, 1, &geo);
        l.handle(Input::Down(Button::Left, (x + w / 2) as f32, (y + h) as f32 - 2.0, Mods::default()), &mut layout, &geo);
        l.arr_move(100.0, 800.0, &mut layout, &geo, Mods::default());
        l.handle(Input::Up(Button::Left, 100.0, 800.0), &mut layout, &geo);
        assert_eq!(placed(&layout, 1, &geo), (x, y, w, 800));
        // Shift on an edge keeps the shape instead.
        let (x, y, w, h) = placed(&layout, 1, &geo);
        let shift = Mods { shift: true, ..Mods::default() };
        l.handle(Input::Down(Button::Left, (x + w) as f32 - 2.0, (y + h / 2) as f32, shift), &mut layout, &geo);
        l.arr_move((x + w / 2) as f32, 700.0, &mut layout, &geo, shift);
        l.handle(Input::Up(Button::Left, 0.0, 0.0), &mut layout, &geo);
        let (_, _, w2, h2) = placed(&layout, 1, &geo);
        assert!((w2 as f64 / h2 as f64 - w as f64 / h as f64).abs() < 0.01, "kept {w}×{h}'s shape: {w2}×{h2}");
    }

    #[test]
    fn alt_arrows_stretch_width_or_height() {
        let (mut l, mut layout, geo) = arranged();
        l.handle(Input::Key(0x31, Mods::default()), &mut layout, &geo); // select 1
        let (x, y, w, h) = placed(&layout, 1, &geo);
        let alt = Mods { alt: true, ..Mods::default() };
        let alt10 = Mods { alt: true, shift: true, ..Mods::default() };
        assert_eq!(l.handle(Input::Key(0x27, alt10), &mut layout, &geo), Effect::Edit("Resize Cutout 1".into(), Some("presize-1".into())));
        assert_eq!(placed(&layout, 1, &geo), (x, y, w + 10, h), "Alt+Shift+Right: 10 px wider, same height");
        l.handle(Input::Key(0x26, alt), &mut layout, &geo);
        assert_eq!(placed(&layout, 1, &geo), (x, y, w + 10, h - 1), "Alt+Up: 1 px shorter, same width");
    }

    #[test]
    fn fill_and_restore() {
        let (mut l, mut layout, geo) = arranged();
        l.handle(Input::Key(0x32, Mods::default()), &mut layout, &geo); // select 2
        let before = placed(&layout, 2, &geo);
        assert_eq!(l.handle(Input::Key(0x46, Mods::default()), &mut layout, &geo), Effect::Edit("Fill Cutout 2".into(), None));
        let filled = placed(&layout, 2, &geo);
        assert_eq!((filled.0, filled.2), (0, 3840), "fills the width");
        assert_eq!(l.handle(Input::Key(0x46, Mods::default()), &mut layout, &geo), Effect::Edit("Restore Cutout 2".into(), None));
        assert_eq!(placed(&layout, 2, &geo), before);
    }

    #[test]
    fn auto_arrange_then_again_for_the_other_side() {
        let (mut l, mut layout, geo) = arranged();
        assert_eq!(l.handle(Input::Key(0x41, Mods::default()), &mut layout, &geo), Effect::Edit("Auto-arrange".into(), None));
        assert_eq!(placed(&layout, 1, &geo), (480, 0, 2880, 1620));
        let below = placed(&layout, 2, &geo);
        assert_eq!(below.1, 1620, "Controls directly beneath");
        l.handle(Input::Key(0x41, Mods::default()), &mut layout, &geo);
        assert_eq!(placed(&layout, 1, &geo), (0, 270, 2880, 1620));
        assert_eq!(placed(&layout, 2, &geo).0, 2880, "Controls beside it");
    }

    #[test]
    fn space_peeks_while_held() {
        let (mut l, mut layout, geo) = arranged();
        l.handle(Input::Key(0x20, Mods::default()), &mut layout, &geo);
        assert!(l.peek);
        let draws = l.draw(&layout, &geo, "t");
        assert!(draws.iter().all(|d| matches!(d, crate::engine::draw::Draw::Source { .. })), "only the picture while peeking");
        l.handle(Input::KeyUp(0x20), &mut layout, &geo);
        assert!(!l.peek);
    }

    #[test]
    fn a_glide_starts_from_the_old_placement() {
        let (mut l, mut layout, geo) = arranged();
        let before = layout.clone();
        l.handle(Input::Key(0x41, Mods::default()), &mut layout, &geo);
        l.start_glide(&before);
        assert!(l.animating());
        let target = layout.cutout(1).unwrap().placement.rect;
        let shown = l.shown(1, target);
        let start = before.cutout(1).unwrap().placement.rect;
        assert!((shown.x - start.x).abs() < (target.x - start.x).abs() + 1e-9, "not yet at the target");
    }

    #[test]
    fn live_toggle_and_freeze() {
        let (mut l, mut layout, geo) = arranged();
        assert!(!l.wants_live(), "Arrange opens on the still frame");
        l.handle(Input::Key(0x56, Mods::default()), &mut layout, &geo);
        assert!(l.wants_live());
    }

    fn hit_rect(l: &Layer, want: ListHit) -> Px {
        l.list_hits.iter().find(|(_, h)| *h == want).map(|(r, _)| *r).expect("list row drawn")
    }

    fn centre(r: Px) -> (f32, f32) {
        (r.0 + r.2 / 2.0, r.1 + r.3 / 2.0)
    }

    #[test]
    fn list_eye_toggles_and_is_undoable_as_hide() {
        let (mut l, mut layout, geo) = arranged();
        l.draw(&layout, &geo, "t");
        let (x, y) = centre(hit_rect(&l, ListHit::Eye(1)));
        let fx = l.handle(Input::Down(Button::Left, x, y, Mods::default()), &mut layout, &geo);
        assert_eq!(fx, Effect::Edit("Hide Cutout 1".into(), None));
        assert!(!layout.cutout(1).unwrap().placement.visible);
    }

    #[test]
    fn right_click_on_a_list_row_is_for_that_row() {
        let (mut l, mut layout, geo) = arranged();
        l.draw(&layout, &geo, "t");
        for want in [2, 1, 2] {
            let (x, y) = centre(hit_rect(&l, ListHit::Row(want)));
            let fx = l.handle(Input::Down(Button::Right, x, y, Mods::default()), &mut layout, &geo);
            assert_eq!(fx, Effect::Menu(x, y, Some(want)));
            assert_eq!(l.selected, Some(want), "the menu acts on the row that was right-clicked");
        }
        // The menu's Delete then removes exactly that cutout.
        let fx = l.handle(Input::Key(0x2E, Mods::default()), &mut layout, &geo);
        assert_eq!(fx, Effect::Edit("Delete Cutout 2".into(), None));
        assert!(layout.cutout(1).is_some() && layout.cutout(2).is_none());
    }

    #[test]
    fn right_click_on_a_panel_is_for_no_cutout() {
        let (mut l, mut layout, geo) = arranged();
        l.draw(&layout, &geo, "t");
        l.selected = Some(1);
        let r = *l.chrome_rects.first().expect("a panel is drawn");
        let (x, y) = (r.0 + 2.0, r.1 + 2.0);
        if l.list_hit(x, y).is_none() {
            assert_eq!(l.handle(Input::Down(Button::Right, x, y, Mods::default()), &mut layout, &geo), Effect::Menu(x, y, None));
            assert_eq!(l.selected, None);
        }
    }

    #[test]
    fn list_row_double_click_renames() {
        let (mut l, mut layout, geo) = arranged();
        l.draw(&layout, &geo, "t");
        let (x, y) = centre(hit_rect(&l, ListHit::Row(2)));
        for _ in 0..2 {
            l.handle(Input::Down(Button::Left, x, y, Mods::default()), &mut layout, &geo);
            l.handle(Input::Up(Button::Left, x, y), &mut layout, &geo);
        }
        assert_eq!(l.selected, Some(2));
        assert_eq!(l.naming.as_ref().map(|n| n.n), Some(2));
    }

    #[test]
    fn dragging_a_row_restacks() {
        let (mut l, mut layout, geo) = arranged();
        assert_eq!(layout.front_to_back(), vec![2, 1]);
        l.draw(&layout, &geo, "t");
        let (x, y) = centre(hit_rect(&l, ListHit::Row(2)));
        l.handle(Input::Down(Button::Left, x, y, Mods::default()), &mut layout, &geo);
        l.handle(Input::Move(x, y + 38.0 * geo.scale), &mut layout, &geo);
        let fx = l.handle(Input::Up(Button::Left, x, y + 38.0 * geo.scale), &mut layout, &geo);
        assert_eq!(fx, Effect::Edit("Restack Cutout 2".into(), None));
        assert_eq!(layout.front_to_back(), vec![1, 2]);
    }

    #[test]
    fn l_toggles_the_list_and_pulses_its_button() {
        let (mut l, mut layout, geo) = arranged();
        assert!(l.list_open);
        l.handle(Input::Key(0x4C, Mods::default()), &mut layout, &geo);
        assert!(!l.list_open);
        assert!(l.pulse_alpha(Action::List) > 0.0, "the List button pulses");
        l.draw(&layout, &geo, "t");
        assert!(l.list_hits.is_empty(), "no rows while hidden");
    }

    #[test]
    fn clicking_the_readout_opens_the_layouts_menu_and_typing_renames() {
        let (mut l, mut layout, geo) = arranged();
        l.draw(&layout, &geo, "t");
        let r = l.readout_hit.unwrap();
        let (x, y) = centre(r);
        assert_eq!(l.handle(Input::Down(Button::Left, x, y, Mods::default()), &mut layout, &geo), Effect::LayoutMenu(r.0, r.1));
        l.rename_layout("Layout 1");
        for c in "Single screen".chars() {
            l.handle(Input::Char(c), &mut layout, &geo);
        }
        // Arrow keys and Backspace edit the layout name too.
        l.handle(Input::Key(0x25, Mods { ctrl: true, ..Mods::default() }), &mut layout, &geo);
        l.handle(Input::Key(0x08, Mods::default()), &mut layout, &geo);
        assert_eq!(l.handle(Input::Key(0x0D, Mods::default()), &mut layout, &geo), Effect::RenameLayout("Singlescreen".into()));
        assert!(l.layout_naming.is_none());
    }

    #[test]
    fn toolbar_buttons_have_icons() {
        let (mut l, layout, geo) = arranged();
        let draws = l.draw(&layout, &geo, "t");
        let icons: Vec<&str> = draws.iter().filter_map(|d| if let crate::engine::draw::Draw::Icon { name, .. } = d { Some(*name) } else { None }).collect();
        for want in ["scissors", "layout-dashboard", "undo-2", "redo-2", "wand-sparkles", "maximize-2", "panel-left", "x", "eye", "lock-open"] {
            assert!(icons.contains(&want), "missing {want}: {icons:?}");
        }
        for name in icons {
            assert!(crate::engine::icons::source(name).is_some(), "{name} is not embedded");
        }
    }

    #[test]
    fn reset_card_resets_the_arrangement() {
        let (mut l, mut layout, geo) = arranged();
        l.handle(Input::Key(0x41, Mods::default()), &mut layout, &geo); // auto-arrange
        assert_ne!(placed(&layout, 1, &geo), (1920, 0, 1920, 1080));
        assert_eq!(l.act(Action::Reset, &mut layout, &geo), Effect::None);
        assert!(l.reset_card);
        let fx = l.handle(Input::Key(0x31, Mods::default()), &mut layout, &geo);
        assert_eq!(fx, Effect::Edit("Reset arrangement".into(), None));
        assert!(!l.reset_card);
        assert_eq!(placed(&layout, 1, &geo), (1920, 0, 1920, 1080), "back at its source position");
    }

    #[test]
    fn start_over_removes_everything_and_goes_to_cut_view() {
        let (mut l, mut layout, geo) = arranged();
        l.act(Action::Reset, &mut layout, &geo);
        let fx = l.handle(Input::Key(0x32, Mods::default()), &mut layout, &geo);
        assert_eq!(fx, Effect::Edit("Start over".into(), None));
        assert!(layout.cutouts.is_empty());
        assert_eq!(l.view, View::Cut);
    }

    #[test]
    fn escape_cancels_the_reset_card() {
        let (mut l, mut layout, geo) = arranged();
        l.act(Action::Reset, &mut layout, &geo);
        assert_eq!(l.handle(Input::Key(0x1B, Mods::default()), &mut layout, &geo), Effect::None);
        assert!(!l.reset_card);
        assert_eq!(layout.cutouts.len(), 2);
    }

    #[test]
    fn holding_ctrl_shift_backspace_for_a_second_opens_the_card() {
        let (mut l, mut layout, geo) = arranged();
        let cs = Mods { ctrl: true, shift: true, alt: false };
        l.handle(Input::Key(0x08, cs), &mut layout, &geo);
        assert!(l.reset_hold().is_some_and(|p| p < 0.5), "the ring starts filling");
        assert!(l.needs_tick());
        // Let go early: nothing happens.
        l.handle(Input::KeyUp(0x08), &mut layout, &geo);
        assert!(l.reset_hold().is_none());
        // Held for the full second (simulated).
        l.handle(Input::Key(0x08, cs), &mut layout, &geo);
        l.reset_held = Some(Instant::now() - Duration::from_millis(1100));
        l.draw(&layout, &geo, "t");
        assert!(l.reset_card);
    }

    #[test]
    fn holding_undo_opens_the_history_and_a_row_jumps_back() {
        let (mut l, mut layout, geo) = arranged();
        l.history_list = vec!["Auto-arrange".into(), "New cutout 2".into(), "New cutout 1".into()];
        l.draw(&layout, &geo, "t");
        let undo = l.buttons.iter().find(|b| b.1 == Action::Undo).unwrap().0;
        let (x, y) = (undo.0 + undo.2 / 2.0, undo.1 + undo.3 / 2.0);
        l.handle(Input::Down(Button::Left, x, y, Mods::default()), &mut layout, &geo);
        l.button_press = Some((Action::Undo, Instant::now() - Duration::from_millis(600)));
        l.draw(&layout, &geo, "t");
        assert!(l.history_open);
        assert_eq!(l.handle(Input::Up(Button::Left, x, y), &mut layout, &geo), Effect::None, "releasing does not also undo");
        l.draw(&layout, &geo, "t");
        let row = l.history_hits.iter().find(|(_, i)| *i == 1).unwrap().0;
        let fx = l.handle(Input::Down(Button::Left, row.0 + 10.0, row.1 + 5.0, Mods::default()), &mut layout, &geo);
        assert_eq!(fx, Effect::UndoMany(2));
        assert!(!l.history_open);
    }

    #[test]
    fn a_short_click_on_undo_still_undoes() {
        let (mut l, mut layout, geo) = arranged();
        l.draw(&layout, &geo, "t");
        let undo = l.buttons.iter().find(|b| b.1 == Action::Undo).unwrap().0;
        let (x, y) = (undo.0 + undo.2 / 2.0, undo.1 + undo.3 / 2.0);
        l.handle(Input::Down(Button::Left, x, y, Mods::default()), &mut layout, &geo);
        assert_eq!(l.handle(Input::Up(Button::Left, x, y), &mut layout, &geo), Effect::Undo);
    }

    #[test]
    fn scaling_is_an_undoable_choice() {
        use crate::model::layout::Filter;
        let (mut l, mut layout, _) = arranged();
        assert_eq!(layout.cutout(1).unwrap().placement.filter, Filter::Auto, "new cutouts start on Auto");
        assert_eq!(l.set_filter(1, Filter::Sharp, &mut layout), Effect::Edit("Scaling of Cutout 1: Sharp".into(), None));
        assert_eq!(l.set_filter(1, Filter::Sharp, &mut layout), Effect::None, "no step when nothing changes");
    }

    #[test]
    fn a_drag_starting_on_a_toolbar_button_draws_on_the_picture() {
        let (mut l, mut layout, geo) = setup();
        l.draw(&layout, &geo, "t");
        let b = l.buttons.iter().find(|b| b.1 == Action::Undo).unwrap().0;
        let start = (b.0 + b.2 / 2.0, b.1 + b.3 / 2.0);
        let off = Mods { ctrl: true, ..Default::default() };
        l.handle(Input::Down(Button::Left, start.0, start.1, off), &mut layout, &geo);
        l.handle(Input::Move(start.0 + 400.0, start.1 + 300.0), &mut layout, &geo);
        let fx = l.handle(Input::Up(Button::Left, start.0 + 400.0, start.1 + 300.0), &mut layout, &geo);
        assert_eq!(fx, Effect::Edit("New cutout 1".into(), None), "drew instead of pressing Undo");
        let c = layout.cutout(1).unwrap().bounds().scale(geo.source_px);
        assert!((c.x - start.0 as f64 / 2.0).abs() < 1.0 && (c.y - start.1 as f64 / 2.0).abs() < 1.0, "starts where the press was: {c:?}");
    }

    #[test]
    fn a_click_on_a_toolbar_button_still_presses_it() {
        let (mut l, mut layout, geo) = setup();
        l.draw(&layout, &geo, "t");
        let b = l.buttons.iter().find(|b| b.1 == Action::Undo).unwrap().0;
        let (x, y) = (b.0 + b.2 / 2.0, b.1 + b.3 / 2.0);
        l.handle(Input::Down(Button::Left, x, y, Mods::default()), &mut layout, &geo);
        l.handle(Input::Move(x + 2.0, y + 1.0), &mut layout, &geo);
        assert_eq!(l.handle(Input::Up(Button::Left, x + 2.0, y + 1.0), &mut layout, &geo), Effect::Undo);
    }

    #[test]
    fn auto_arrange_never_focuses_a_hidden_cutout() {
        let (mut l, mut layout, geo) = arranged();
        l.handle(Input::Key(0x32, Mods::default()), &mut layout, &geo); // select 2
        l.handle(Input::Key(0x48, Mods::default()), &mut layout, &geo); // hide it; still selected
        let hidden_before = placed(&layout, 2, &geo);
        l.handle(Input::Key(0x41, Mods::default()), &mut layout, &geo); // A
        // Cutout 1 is the only visible one, so it gets the whole canvas; the hidden one stays put.
        assert_eq!(placed(&layout, 1, &geo), (0, 0, 3840, 2160));
        assert_eq!(placed(&layout, 2, &geo), hidden_before);
    }

    #[test]
    fn presets_add_named_cutouts() {
        let (mut l, mut layout, geo) = setup();
        let fx = l.add_preset(Preset::Quadrants, &mut layout, &geo);
        assert_eq!(fx, Effect::Edit("Add grid: Four quadrants".into(), None));
        let names: Vec<&str> = layout.cutouts.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["Top left", "Top right", "Bottom left", "Bottom right"]);
    }
}
