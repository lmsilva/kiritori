//! The layer's panels: the cutout list, the clickable readout for
//! layouts, tooltips and the pulse that ties a shortcut to its button.

use std::time::{Duration, Instant};

use super::style::*;
use super::{Action, Effect, Geo, Layer};
use crate::engine::draw::{Align, Draw, Px};
use crate::engine::render::measure;
use crate::model::layout::{Layout, number_label};

/// Tooltips appear after the pointer rests this long.
pub const TOOLTIP_DELAY: Duration = Duration::from_millis(300);
pub const PULSE: Duration = Duration::from_millis(350);
const DOUBLE_CLICK: Duration = Duration::from_millis(450);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListHit {
    Row(u32),
    Eye(u32),
    Lock(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HoverKey {
    Button(Action),
    List(ListHit),
    Readout,
}

#[derive(Clone, Debug)]
pub struct RowDrag {
    pub n: u32,
    pub from_y: f32,
    pub moved: bool,
    /// Index in the front-to-back list where the row would land.
    pub to: usize,
}

/// The layout name being typed in the readout; it starts all selected.
#[derive(Clone, Debug)]
pub struct LayoutNaming {
    pub typing: crate::textedit::Typing,
}

fn inside(r: &Px, x: f32, y: f32) -> bool {
    x >= r.0 && x < r.0 + r.2 && y >= r.1 && y < r.1 + r.3
}

fn text(r: Px, s: &str, size: f32, color: crate::engine::draw::Rgba, bold: bool, align: Align) -> Draw {
    Draw::Text { r, text: s.to_string(), size, color, bold, align }
}

impl Layer {
    pub(super) fn list_hit(&self, x: f32, y: f32) -> Option<ListHit> {
        // Buttons first: they sit on top of their row.
        self.list_hits.iter().filter(|(_, h)| !matches!(h, ListHit::Row(_))).find(|(r, _)| inside(r, x, y)).or_else(|| self.list_hits.iter().find(|(r, _)| inside(r, x, y))).map(|(_, h)| *h)
    }

    pub(super) fn over_readout(&self, x: f32, y: f32) -> bool {
        self.readout_hit.as_ref().is_some_and(|r| inside(r, x, y))
    }

    /// Track what the pointer rests on, for tooltips.
    pub(super) fn update_hover(&mut self, x: f32, y: f32) {
        let key = if let Some(a) = self.button_at(x, y) {
            Some(HoverKey::Button(a))
        } else if let Some(h) = self.list_hit(x, y) {
            Some(HoverKey::List(h))
        } else if self.over_readout(x, y) {
            Some(HoverKey::Readout)
        } else {
            None
        };
        if self.hover_since.as_ref().map(|h| h.0) != key {
            self.hover_since = key.map(|k| (k, Instant::now()));
        }
    }

    /// Something time-based is on screen (a glide, a pending tooltip, a pulse): redraw soon.
    pub fn needs_tick(&self) -> bool {
        self.animating()
            || self.badges_showing()
            || self.reset_held.is_some()
            || self.button_press.is_some_and(|(a, _)| a == Action::Undo) && !self.history_open
            || self.hover_since.as_ref().is_some_and(|(_, t)| t.elapsed() < TOOLTIP_DELAY + Duration::from_millis(50))
            || self.pulse.as_ref().is_some_and(|(_, t)| t.elapsed() < PULSE)
    }

    pub(super) fn list_down(&mut self, hit: ListHit, y: f32, layout: &mut Layout) -> Effect {
        match hit {
            ListHit::Eye(n) => {
                layout.toggle_visible(n);
                let c = layout.cutout(n).unwrap();
                Effect::Edit(format!("{} {}", if c.placement.visible { "Show" } else { "Hide" }, c.name), None)
            }
            ListHit::Lock(n) => {
                layout.toggle_locked(n);
                let c = layout.cutout(n).unwrap();
                Effect::Edit(format!("{} {}", if c.placement.locked { "Lock" } else { "Unlock" }, c.name), None)
            }
            ListHit::Row(n) => {
                let double = self.last_row_click.as_ref().is_some_and(|(m, t)| *m == n && t.elapsed() < DOUBLE_CLICK);
                self.selected = Some(n);
                if double {
                    self.last_row_click = None;
                    let name = layout.cutout(n).map(|c| c.name.clone()).unwrap_or_default();
                    self.naming = Some(super::Naming::new(n, &name));
                    return Effect::None;
                }
                self.last_row_click = Some((n, Instant::now()));
                let to = layout.front_to_back().iter().position(|k| *k == n).unwrap_or(0);
                self.row_drag = Some(RowDrag { n, from_y: y, moved: false, to });
                Effect::None
            }
        }
    }

    pub(super) fn list_move(&mut self, y: f32, layout: &Layout, k: f32) {
        let row_h = 38.0 * k;
        let Some(d) = self.row_drag.as_mut() else { return };
        if (y - d.from_y).abs() > 6.0 * k {
            d.moved = true;
        }
        let order = layout.front_to_back();
        let from = order.iter().position(|n| *n == d.n).unwrap_or(0) as f32;
        d.to = (from + ((y - d.from_y) / row_h).round()).clamp(0.0, (order.len() - 1) as f32) as usize;
    }

    pub(super) fn list_up(&mut self, layout: &mut Layout) -> Effect {
        let Some(d) = self.row_drag.take() else { return Effect::None };
        if !d.moved {
            return Effect::None;
        }
        let mut order = layout.front_to_back();
        let Some(from) = order.iter().position(|n| *n == d.n) else { return Effect::None };
        if from == d.to {
            return Effect::None;
        }
        let n = order.remove(from);
        order.insert(d.to, n);
        layout.set_order(&order);
        let name = layout.cutout(n).map(|c| c.name.clone()).unwrap_or_default();
        Effect::Edit(format!("Restack {name}"), None)
    }

    /// Keys and characters while the layout name is being typed in the readout.
    pub(super) fn layout_naming_key(&mut self, vk: u16, m: crate::shell::canvas::Mods) -> Effect {
        match vk {
            0x0D => match self.layout_naming.take() {
                Some(n) if !n.typing.text().trim().is_empty() => Effect::RenameLayout(n.typing.text().trim().to_string()),
                _ => Effect::None,
            },
            0x1B => {
                self.layout_naming = None;
                Effect::None
            }
            _ => {
                if let Some(n) = self.layout_naming.as_mut() {
                    n.typing.edit.key_with_clipboard(vk, m);
                }
                Effect::None
            }
        }
    }

    pub(super) fn layout_naming_char(&mut self, c: char) {
        if let Some(n) = self.layout_naming.as_mut() {
            n.typing.edit.char(c);
        }
    }

    /// The cutout list, front first. One row per cutout: number chip, name, eye, lock.
    pub(super) fn draw_list(&mut self, out: &mut Vec<Draw>, layout: &Layout, geo: &Geo) {
        self.list_hits.clear();
        if !self.list_open {
            return;
        }
        let k = geo.scale;
        let (x, y0, w) = (16.0 * k, 92.0 * k, 280.0 * k);
        let row_h = 38.0 * k;
        let head = 34.0 * k;
        let order = layout.front_to_back();
        let rows = order.len().max(1) as f32;
        let h = head + rows * row_h + 10.0 * k;
        out.push(Draw::Fill { r: (x, y0, w, h), color: glass(), radius: 12.0 * k });
        out.push(Draw::Stroke { r: (x, y0, w, h), color: hairline(), width: 1.0, radius: 12.0 * k, dashed: false });
        out.push(Draw::Icon { name: "layers", r: (x + 12.0 * k, y0 + 9.0 * k, 16.0 * k, 16.0 * k), color: fg_muted(), stroke: 1.5 * k });
        out.push(text((x + 34.0 * k, y0, w, head), &format!("Cutouts · {}", order.len()), 12.0 * k, fg_muted(), true, Align::Left));
        let size = 13.0 * k;
        // Hint for the list toggle key, top right of the panel.
        let kw = measure("L", 11.0 * k, true) + 10.0 * k;
        out.push(Draw::Fill { r: (x + w - kw - 10.0 * k, y0 + 8.0 * k, kw, 18.0 * k), color: keycap_fill(), radius: 4.0 * k });
        out.push(text((x + w - kw - 10.0 * k, y0 + 8.0 * k, kw, 18.0 * k), "L", 11.0 * k, fg_muted(), true, Align::Center));
        if order.is_empty() {
            out.push(text((x + 14.0 * k, y0 + head, w - 28.0 * k, row_h), "Drag on the picture to cut", size, fg_dim(), false, Align::Left));
            return;
        }
        let dragging = self.row_drag.as_ref().filter(|d| d.moved).cloned();
        for (i, n) in order.iter().enumerate() {
            let c = layout.cutout(*n).unwrap();
            let ry = y0 + head + i as f32 * row_h;
            let row = (x + 6.0 * k, ry, w - 12.0 * k, row_h - 4.0 * k);
            let hovered = self.hover_since.as_ref().is_some_and(|(h, _)| matches!(h, super::chrome::HoverKey::List(ListHit::Row(m)) if m == n));
            if self.selected == Some(*n) {
                out.push(Draw::Fill { r: row, color: accent_fill(), radius: 8.0 * k });
            } else if hovered {
                out.push(Draw::Fill { r: row, color: glass_hover(), radius: 8.0 * k });
            }
            if dragging.as_ref().is_some_and(|d| d.n == *n) {
                out.push(Draw::Stroke { r: row, color: accent(), width: 1.5 * k, radius: 8.0 * k, dashed: true });
            }
            let visible = c.placement.visible;
            let chip = 22.0 * k;
            let cx = row.0 + 8.0 * k;
            let cy = ry + (row.3 - chip) / 2.0;
            out.push(Draw::Fill { r: (cx, cy, chip, chip), color: cutout(c.color), radius: chip / 2.0 });
            out.push(text((cx, cy, chip, chip), &number_label(c.number), 12.0 * k, ON_COLOR, true, Align::Center));
            out.push(text((cx + chip + 10.0 * k, ry, w - 120.0 * k, row.3), &c.name, size, if visible { fg() } else { fg_dim() }, false, Align::Left));
            let ib = 26.0 * k;
            let eye = (row.0 + row.2 - 2.0 * ib - 8.0 * k, ry + (row.3 - ib) / 2.0, ib, ib);
            let lock = (row.0 + row.2 - ib - 4.0 * k, eye.1, ib, ib);
            for (r, hit, icon, on) in [
                (eye, ListHit::Eye(*n), if visible { "eye" } else { "eye-off" }, !visible),
                (lock, ListHit::Lock(*n), if c.placement.locked { "lock" } else { "lock-open" }, c.placement.locked),
            ] {
                let hover = self.hover_since.as_ref().is_some_and(|(h, _)| *h == super::chrome::HoverKey::List(hit));
                if on {
                    out.push(Draw::Fill { r, color: accent_fill(), radius: 6.0 * k });
                } else if hover {
                    out.push(Draw::Fill { r, color: glass_hover(), radius: 6.0 * k });
                }
                let is = 16.0 * k;
                out.push(Draw::Icon { name: icon, r: (r.0 + (r.2 - is) / 2.0, r.1 + (r.3 - is) / 2.0, is, is), color: if on || hover { fg() } else { fg_muted() }, stroke: 1.5 * k });
                self.list_hits.push((r, hit));
            }
            self.list_hits.push((row, ListHit::Row(*n)));
        }
        // Where a dragged row would land.
        if let Some(d) = dragging {
            let ly = y0 + head + d.to as f32 * row_h + if d.to > order.iter().position(|m| *m == d.n).unwrap_or(0) { row_h - 4.0 * k } else { -2.0 * k };
            out.push(Draw::Line { a: (x + 10.0 * k, ly), b: (x + w - 10.0 * k, ly), color: accent(), width: 2.0 * k });
        }
    }

    /// A tooltip for whatever the pointer has rested on for 300 ms.
    pub(super) fn draw_tooltip(&self, out: &mut Vec<Draw>, layout: &Layout, geo: &Geo) {
        let Some((key, since)) = &self.hover_since else { return };
        if since.elapsed() < TOOLTIP_DELAY || self.dragging() || self.row_drag.as_ref().is_some_and(|d| d.moved) {
            return;
        }
        let name = |n: u32| layout.cutout(n).map(|c| c.name.clone()).unwrap_or_default();
        let (label, key_cap): (String, &str) = match key {
            HoverKey::Button(Action::Undo) => (self.history_names.0.as_ref().map(|s| format!("Undo: {s}")).unwrap_or("Nothing to undo".into()), "Ctrl+Z"),
            HoverKey::Button(Action::Redo) => (self.history_names.1.as_ref().map(|s| format!("Redo: {s}")).unwrap_or("Nothing to redo".into()), "Ctrl+Y"),
            HoverKey::Button(Action::Grids) => ("Add a ready-made grid of cutouts".into(), ""),
            HoverKey::Button(Action::Live) => ("Switch between the still frame and the moving picture".into(), "V"),
            HoverKey::Button(Action::Close) => ("Close the layer; everything is already saved".into(), "Esc"),
            HoverKey::Button(Action::Profiles) => ("Profiles".into(), "P"),
            HoverKey::Button(Action::Shortcuts) => ("All shortcuts".into(), "F1"),
            HoverKey::Button(Action::Settings) => ("Settings".into(), "Ctrl+,"),
            HoverKey::Button(Action::Suggest) => ("Suggest cutouts from the picture's panels".into(), "S"),
            HoverKey::Button(Action::Trim) => ("Trim black edges from the selected cutout".into(), "T"),
            HoverKey::List(ListHit::Eye(n)) => (format!("{} {}", if layout.cutout(*n).is_some_and(|c| c.placement.visible) { "Hide" } else { "Show" }, name(*n)), "H"),
            HoverKey::List(ListHit::Lock(n)) => (format!("{} {}", if layout.cutout(*n).is_some_and(|c| c.placement.locked) { "Unlock" } else { "Lock" }, name(*n)), "K"),
            HoverKey::List(ListHit::Row(_)) => ("Click to select · double-click to rename · drag to restack".into(), ""),
            HoverKey::Readout => ("Click to switch, rename or add a layout".into(), ""),
            HoverKey::Button(_) => return,
        };
        let k = geo.scale;
        let size = 13.0 * k;
        let tw = measure(&label, size, false);
        let kw = if key_cap.is_empty() { 0.0 } else { measure(key_cap, 11.0 * k, true) + 18.0 * k };
        let w = tw + kw + 24.0 * k;
        let h = 30.0 * k;
        let (px, py) = self.pointer;
        let mut x = px + 14.0 * k;
        let mut y = py + 22.0 * k;
        if x + w > geo.canvas.w as f32 - 8.0 * k {
            x = px - w - 8.0 * k;
        }
        if y + h > geo.canvas.h as f32 - 8.0 * k {
            y = py - h - 10.0 * k;
        }
        out.push(Draw::Fill { r: (x, y, w, h), color: glass_hover(), radius: 8.0 * k });
        out.push(Draw::Stroke { r: (x, y, w, h), color: hairline(), width: 1.0, radius: 8.0 * k, dashed: false });
        out.push(text((x + 12.0 * k, y, tw + 2.0, h), &label, size, fg(), false, Align::Left));
        if !key_cap.is_empty() {
            let cw = measure(key_cap, 11.0 * k, true) + 10.0 * k;
            let cx = x + 12.0 * k + tw + 8.0 * k;
            out.push(Draw::Fill { r: (cx, y + (h - 18.0 * k) / 2.0, cw, 18.0 * k), color: keycap_fill(), radius: 4.0 * k });
            out.push(text((cx, y + (h - 18.0 * k) / 2.0, cw, 18.0 * k), key_cap, 11.0 * k, fg(), true, Align::Center));
        }
    }

    /// The button an action belongs to pulses once when its shortcut is used.
    pub(super) fn pulse_alpha(&self, a: Action) -> f32 {
        match &self.pulse {
            Some((p, t)) if *p == a && t.elapsed() < PULSE => 1.0 - t.elapsed().as_secs_f32() / PULSE.as_secs_f32(),
            _ => 0.0,
        }
    }
}
