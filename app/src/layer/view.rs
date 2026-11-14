//! Drawing the layer: Cut view's scrim, cutouts, tags, handles, guides and loupe, plus the
//! clickable toolbar, hint strip and readout.

use super::style::*;
use super::{Action, Geo, Layer, SnapLine, View};
use crate::engine::draw::{Align, Draw, Px, Rgba};
use crate::engine::render::measure;
use crate::strings::t;
use crate::model::geometry::Rect;
use crate::model::layout::{Cutout, Filter, Layout, ShapeOp, number_label};

fn text(r: Px, s: &str, size: f32, color: Rgba, bold: bool, align: Align) -> Draw {
    Draw::Text { r, text: s.to_string(), size, color, bold, align }
}

/// A keycap: the key's name in a small rounded box. Returns its width.
fn keycap(out: &mut Vec<Draw>, x: f32, cy: f32, key: &str, k: f32, enabled: bool) -> f32 {
    let size = 11.0 * k;
    let w = measure(key, size, true) + 10.0 * k;
    let h = 18.0 * k;
    out.push(Draw::Fill { r: (x, cy - h / 2.0, w, h), color: keycap_fill(), radius: 4.0 * k });
    out.push(text((x, cy - h / 2.0, w, h), key, size, if enabled { fg() } else { fg_dim() }, true, Align::Center));
    w
}

/// The Lucide icon for a toolbar action, given its current label.
fn icon_of(a: Action, label: &str) -> &'static str {
    match a {
        Action::Cut => "scissors",
        Action::Arrange => "layout-dashboard",
        Action::Undo => "undo-2",
        Action::Redo => "redo-2",
        Action::Reset => "rotate-ccw",
        Action::Retake => "refresh-cw",
        Action::Grids => "grid-2x2",
        Action::Hide if label == "Show" => "eye",
        Action::Hide => "eye-off",
        Action::Delete => "trash-2",
        Action::Snapping => "magnet",
        Action::Fill if label == "Restore" => "minimize-2",
        Action::Fill => "maximize-2",
        Action::AutoArrange => "wand-sparkles",
        Action::ResetPlacement => "undo-dot",
        Action::Live if label == "Live" => "video",
        Action::Live => "image",
        Action::List => "panel-left",
        Action::Suggest => "sparkles",
        Action::Trim => "crop",
        Action::Shortcuts => "keyboard",
        Action::Profiles => "app-window",
        Action::Settings => "settings",
        Action::Close => "x",
    }
}

impl Layer {
    /// Everything drawn while the layer is open on this canvas.
    pub fn draw(&mut self, layout: &Layout, geo: &Geo, title: &str) -> Vec<Draw> {
        self.advance_holds();
        let mut out = Vec::new();
        let picture_end;
        self.overlay_from = usize::MAX;
        self.map_rect = None;
        if self.view == View::Arrange {
            self.draw_arrange(&mut out, layout, geo);
            picture_end = out.len();
            if !self.peek {
                self.draw_map(&mut out, geo);
                self.draw_chrome(&mut out, layout, geo, title);
            }
        } else {
            self.draw_cut(&mut out, layout, geo);
            picture_end = out.len();
            self.draw_chrome(&mut out, layout, geo, title);
        }
        self.draw_badge(&mut out, geo);
        // Remember where the panels are, so cutout tags keep clear of them next time.
        let overlay = self.overlay_from.clamp(picture_end, out.len());
        self.chrome_rects = out[picture_end..overlay]
            .iter()
            .filter_map(|d| match d {
                Draw::Fill { r, radius, .. } if *radius > 0.0 && r.2 > 40.0 && r.3 > 20.0 => Some(*r),
                _ => None,
            })
            .collect();
        out
    }

    /// Another display while the layer is open: its cutouts as placed, with outlines, tags and
    /// handles, and its number badge. The toolbar and list stay on the display with the pointer.
    pub fn draw_side(&mut self, layout: &Layout, geo: &Geo) -> Vec<Draw> {
        let mut out = Vec::new();
        // No panels on this display: tags go where they belong.
        let panels = std::mem::take(&mut self.chrome_rects);
        self.draw_arrange(&mut out, layout, geo);
        self.chrome_rects = panels;
        self.draw_badge(&mut out, geo);
        out
    }

    /// Cut view's picture: the source under a scrim, cutouts as clear windows, outlines, tags,
    /// handles, the rectangle being drawn, snap lines and the loupe.
    fn draw_cut(&mut self, out: &mut Vec<Draw>, layout: &Layout, geo: &Geo) {
        let k = geo.scale;
        let s = geo.source;
        let src_px = (s.x as f32, s.y as f32, s.w as f32, s.h as f32);

        // The source at its real position, under a 55% scrim.
        out.push(Draw::Source { orient: 0, dst: src_px, src: Rect::new(0.0, 0.0, 1.0, 1.0), shape: Vec::new(), filter: Filter::Smooth, dim: SCRIM_DIM, alpha: 1.0, circle: false, input: false });
        // Each visible cutout is a clear window through the scrim.
        for c in layout.by_z() {
            if c.placement.visible {
                out.push(Draw::Source { orient: 0, dst: geo.to_canvas(c.bounds()), src: c.bounds(), shape: c.rects.clone(), filter: Filter::Smooth, dim: 1.0, alpha: 1.0, circle: false, input: false });
            }
        }

        // Outlines and tags.
        for c in layout.by_z() {
            let col = cutout(c.color);
            let sel = self.selected == Some(c.number);
            let hidden = !c.placement.visible;
            let w = if sel { 3.0 } else { 2.0 } * k;
            for r in c.rects.iter().filter(|r| r.op == ShapeOp::Add) {
                let p = geo.to_canvas(r.rect);
                out.push(Draw::Stroke { r: p, color: HALO, width: w + 2.0 * k, radius: 0.0, dashed: false });
                out.push(Draw::Stroke { r: p, color: col, width: w, radius: 0.0, dashed: hidden });
            }
            for r in c.rects.iter().filter(|r| r.op == ShapeOp::Subtract) {
                let p = geo.to_canvas(r.rect);
                out.push(Draw::Stroke { r: p, color: col, width: 1.0 * k, radius: 0.0, dashed: true });
            }
            self.draw_tag(out, c, geo.to_canvas(c.bounds()), geo);

            if sel && !c.placement.locked {
                for (hx, hy) in self.handles(layout, geo) {
                    let hs = 10.0 * k;
                    out.push(Draw::Fill { r: (hx - hs / 2.0 - k, hy - hs / 2.0 - k, hs + 2.0 * k, hs + 2.0 * k), color: HALO, radius: 2.0 * k });
                    out.push(Draw::Fill { r: (hx - hs / 2.0, hy - hs / 2.0, hs, hs), color: col, radius: 2.0 * k });
                }
            }
        }

        // The rectangle being drawn, with its size.
        if let Some((op, a, b)) = self.drawing_rect() {
            let r = Rect::new(a.0.min(b.0), a.1.min(b.1), (b.0 - a.0).abs(), (b.1 - a.1).abs());
            let p = geo.to_canvas(r.unscale(geo.source_px));
            let col = match op {
                None => cutout(layout.next_color()),
                Some(ShapeOp::Add) => accent(),
                Some(ShapeOp::Subtract) => danger(),
            };
            if op != Some(ShapeOp::Subtract) {
                out.push(Draw::Source { orient: 0, dst: p, src: r.unscale(geo.source_px), shape: Vec::new(), filter: Filter::Smooth, dim: 1.0, alpha: 1.0, circle: false, input: false });
            }
            out.push(Draw::Stroke { r: p, color: HALO, width: 4.0 * k, radius: 0.0, dashed: false });
            out.push(Draw::Stroke { r: p, color: col, width: 2.0 * k, radius: 0.0, dashed: op == Some(ShapeOp::Subtract) });
            let label = format!("{} × {}", r.w.round(), r.h.round());
            let lw = measure(&label, 13.0 * k, true) + 16.0 * k;
            let (lx, ly) = (self.pointer.0 + 16.0 * k, self.pointer.1 + 16.0 * k);
            out.push(Draw::Fill { r: (lx, ly, lw, 24.0 * k), color: glass(), radius: 6.0 * k });
            out.push(text((lx, ly, lw, 24.0 * k), &label, 13.0 * k, fg(), true, Align::Center));
        }

        // Snap lines across the source.
        for sl in &self.snaps {
            match *sl {
                SnapLine::X(x) => {
                    let cx = (s.x + x / geo.source_px.w * s.w) as f32;
                    out.push(Draw::Line { a: (cx, s.y as f32), b: (cx, (s.y + s.h) as f32), color: SNAP, width: 1.0 * k });
                }
                SnapLine::Y(y) => {
                    let cy = (s.y + y / geo.source_px.h * s.h) as f32;
                    out.push(Draw::Line { a: (s.x as f32, cy), b: ((s.x + s.w) as f32, cy), color: SNAP, width: 1.0 * k });
                }
            }
        }

        // Loupe: an 8× circular magnifier next to the pointer while edges are being placed.
        if self.loupe() {
            let d = 200.0 * k;
            let (px, py) = self.pointer;
            let mut lx = px + 28.0 * k;
            let mut ly = py + 28.0 * k;
            if lx + d > geo.canvas.w as f32 {
                lx = px - 28.0 * k - d;
            }
            if ly + d > geo.canvas.h as f32 {
                ly = py - 28.0 * k - d;
            }
            let span = d / 8.0; // canvas pixels shown across the loupe
            let fx = ((px as f64 - s.x) / s.w, (py as f64 - s.y) / s.h);
            let fw = span as f64 / s.w;
            let fh = span as f64 / s.h;
            let region = Rect::new(fx.0 - fw / 2.0, fx.1 - fh / 2.0, fw, fh);
            out.push(Draw::Fill { r: (lx - 3.0 * k, ly - 3.0 * k, d + 6.0 * k, d + 6.0 * k), color: HALO, radius: d / 2.0 + 3.0 * k });
            out.push(Draw::Source { orient: 0, dst: (lx, ly, d, d), src: region, shape: Vec::new(), filter: Filter::Sharp, dim: 1.0, alpha: 1.0, circle: true, input: false });
            out.push(Draw::Ellipse { r: (lx, ly, d, d), color: fg(), width: 2.0 * k });
            let (cx, cy) = (lx + d / 2.0, ly + d / 2.0);
            out.push(Draw::Line { a: (cx - 14.0 * k, cy), b: (cx + 14.0 * k, cy), color: accent(), width: 1.0 * k });
            out.push(Draw::Line { a: (cx, cy - 14.0 * k), b: (cx, cy + 14.0 * k), color: accent(), width: 1.0 * k });
        }

    }

    /// Toolbar, hint strip, readout and the passing message, for either view.
    fn draw_chrome(&mut self, out: &mut Vec<Draw>, layout: &Layout, geo: &Geo, title: &str) {
        let k = geo.scale;
        self.draw_list(out, layout, geo);
        self.draw_toolbar(out, layout, geo);
        self.draw_hints(out, layout, geo);
        self.draw_readout(out, layout, geo, title);
        if let Some(m) = self.message() {
            let size = 14.0 * k;
            let w = measure(m, size, false) + 32.0 * k;
            let r = ((geo.canvas.w as f32 - w) / 2.0, 80.0 * k, w, 34.0 * k);
            out.push(Draw::Fill { r, color: glass(), radius: 17.0 * k });
            out.push(text(r, m, size, fg(), false, Align::Center));
        }
        self.draw_guide(out, geo);
        // Everything after this is a passing overlay (tooltip, cards), not a panel to avoid.
        self.overlay_from = out.len();
        self.draw_reset_ring(out, geo);
        self.draw_history(out, geo);
        if !self.history_open && !self.reset_card {
            self.draw_tooltip(out, layout, geo);
        }
        self.draw_reset_card(out, geo);
        self.draw_keymap(out, geo);
    }

    /// A cutout's tag: number in a filled circle, then the name (or the name being typed).
    /// Above the cutout, or inside it when there is no room under the toolbar.
    pub(super) fn draw_tag(&self, out: &mut Vec<Draw>, c: &Cutout, b: crate::engine::draw::Px, geo: &Geo) {
        let k = geo.scale;
        let col = cutout(c.color);
        let hidden = !c.placement.visible;
        let naming = self.naming.as_ref().filter(|n| n.n == c.number);
        let label = match naming {
            Some(n) => n.typing.text() + " ",
            None if hidden => format!("{}  (hidden)", c.name),
            None if c.placement.locked => format!("{}  (locked)", c.name),
            None => c.name.clone(),
        };
        let size = 13.0 * k;
        let h = 26.0 * k;
        let tw = measure(&label, size, false) + 16.0 * k;
        let toolbar_bottom = 70.0 * k;
        // Above the cutout, else just inside it; and never under a panel (list, toolbar, map,
        // readout): slide right past it, or try the other spot.
        let clear = |x: f32, y: f32| !self.chrome_rects.iter().any(|r| x < r.0 + r.2 && x + h + tw > r.0 && y < r.1 + r.3 && y + h > r.1);
        let above = b.1 - h - 4.0 * k;
        let spots = [(b.0, above), (b.0 + 6.0 * k, b.1 + 6.0 * k)];
        let (mut tx, mut ty) = if above >= toolbar_bottom { spots[0] } else { spots[1] };
        if !clear(tx, ty) {
            // Other places, in order: the other default spot; just below the panel in the way;
            // just right of it. All stay over the cutout.
            let mut cands: Vec<(f32, f32)> = spots.iter().copied().filter(|s| s.1 >= toolbar_bottom || s.1 > b.1).collect();
            for r in self.chrome_rects.iter().filter(|r| tx < r.0 + r.2 && tx + h + tw > r.0 && ty < r.1 + r.3 && ty + h > r.1) {
                cands.push((tx, r.1 + r.3 + 6.0 * k));
                cands.push((r.0 + r.2 + 8.0 * k, ty));
            }
            let inside = |x: f32, y: f32| x >= b.0 - 1.0 && x + h + tw <= b.0 + b.2 + 1.0 && y >= b.1 - h - 6.0 * k && y + h <= b.1 + b.3;
            if let Some(c) = cands.into_iter().find(|&(x, y)| clear(x, y) && (inside(x, y) || y < b.1)) {
                (tx, ty) = c;
            }
        }
        out.push(Draw::Fill { r: (tx, ty, h + tw, h), color: if naming.is_some() { glass_hover() } else { glass() }, radius: h / 2.0 });
        if naming.is_some() {
            out.push(Draw::Stroke { r: (tx, ty, h + tw, h), color: col, width: 1.5 * k, radius: h / 2.0, dashed: false });
        }
        out.push(Draw::Fill { r: (tx + 3.0 * k, ty + 3.0 * k, h - 6.0 * k, h - 6.0 * k), color: col, radius: (h - 6.0 * k) / 2.0 });
        out.push(text((tx + 3.0 * k, ty + 3.0 * k, h - 6.0 * k, h - 6.0 * k), &number_label(c.number), 12.0 * k, ON_COLOR, true, Align::Center));
        match naming {
            Some(n) => out.extend(n.typing.draws((tx + h + 8.0 * k, ty, tw - 8.0 * k, h), size, false, fg(), accent_fill(), fg(), 1.5 * k)),
            None => out.push(text((tx + h + 8.0 * k, ty, tw, h), &label, size, if hidden { fg_muted() } else { fg() }, false, Align::Left)),
        }
    }

    /// The toolbar: every button shows its name and its shortcut.
    fn draw_toolbar(&mut self, out: &mut Vec<Draw>, layout: &Layout, geo: &Geo) {
        let k = geo.scale;
        let sel = self.selected.and_then(|n| layout.cutout(n));
        let hide_label = if sel.is_some_and(|c| !c.placement.visible) { t("tool.show") } else { t("tool.hide") };
        let cut = self.view == View::Cut;
        let filled = sel.is_some_and(|c| c.placement.before_fill.is_some());
        let mut items: Vec<(Action, &str, &str, bool, bool)> = vec![
            (Action::Cut, t("tool.cut"), "Tab", true, cut),
            (Action::Arrange, t("tool.arrange"), "Tab", true, !cut),
            (Action::Undo, t("tool.undo"), "Ctrl+Z", true, false),
            (Action::Redo, t("tool.redo"), "Ctrl+Y", true, false),
            (Action::Reset, t("tool.reset"), "", !layout.cutouts.is_empty(), self.reset_card),
        ];
        if cut {
            items.extend([
                (Action::Retake, t("tool.retake"), "F5", true, false),
                (Action::Grids, t("tool.grids"), "", true, false),
                (Action::Suggest, t("tool.suggest"), "S", true, false),
                (Action::Trim, t("tool.trim"), "T", sel.is_some(), false),
                (Action::Hide, hide_label, "H", sel.is_some(), false),
                (Action::Delete, t("tool.delete"), "Del", sel.is_some(), false),
                (Action::Snapping, t("tool.snap"), "G", true, self.snapping),
                (Action::List, t("tool.list"), "L", true, self.list_open),
            ]);
        } else {
            items.extend([
                (Action::Fill, if filled { t("tool.restore") } else { t("tool.fill") }, "F", sel.is_some(), false),
                (Action::AutoArrange, t("tool.auto_arrange"), "A", !layout.cutouts.is_empty(), false),
                (Action::Hide, hide_label, "H", sel.is_some(), false),
                (Action::ResetPlacement, t("tool.reset_placement"), "R", sel.is_some(), false),
                (Action::Live, if self.live { t("tool.live") } else { t("tool.edit") }, "V", true, self.live),
                (Action::Snapping, t("tool.snap"), "G", true, self.snapping),
                (Action::List, t("tool.list"), "L", true, self.list_open),
            ]);
        }
        let size = 13.0 * k;
        let h = 36.0 * k;
        let pad = 10.0 * k;
        let gap = 4.0 * k;
        // Room for the right-hand group (Profiles, Shortcuts, Settings, Close).
        let right_w = 3.0 * (h + gap) + measure("Close", size, false) + 2.0 * pad + 24.0 * k + measure("Esc", 11.0 * k, true) + 18.0 * k + 6.0 * gap + 32.0 * k;
        // The toolbar is centred if it fits; otherwise it moves left, clear of the right group.
        let room = geo.canvas.w as f32 - right_w - 32.0 * k;
        let width_of = |compact: bool| -> Vec<f32> {
            items
                .iter()
                .map(|(_, l, key, _, _)| {
                    let label = if compact { 0.0 } else { measure(l, size, false) + 24.0 * k };
                    let cap = if key.is_empty() { 0.0 } else { measure(key, 11.0 * k, true) + 18.0 * k };
                    if compact { 2.0 * pad + 18.0 * k + cap } else { label + 2.0 * pad + cap }
                })
                .collect()
        };
        let full = width_of(false);
        let compact = full.iter().sum::<f32>() + gap * (items.len() as f32 + 1.0) + 12.0 * k > room;
        // On a narrow display, buttons show icon and key only; the tooltip names them.
        let widths = if compact { width_of(true) } else { full };
        let total: f32 = widths.iter().sum::<f32>() + gap * (items.len() as f32 + 1.0) + 12.0 * k;
        let mut x = ((geo.canvas.w as f32 - total) / 2.0).min(geo.canvas.w as f32 - right_w - total).max(16.0 * k);
        let y = 16.0 * k;
        out.push(Draw::Fill { r: (x, y, total, h + 2.0 * gap), color: glass(), radius: 12.0 * k });
        out.push(Draw::Stroke { r: (x, y, total, h + 2.0 * gap), color: hairline(), width: 1.0, radius: 12.0 * k, dashed: false });
        x += gap;
        self.buttons.clear();
        for (i, ((a, label, key, enabled, on), w)) in items.iter().zip(&widths).enumerate() {
            if i == 2 || i == 5 {
                x += 12.0 * k / 2.0; // group spacing: view switch | history | tools
            }
            let r = (x, y + gap, *w, h);
            let hover = self.hover == Some(*a) && *enabled;
            if *on {
                out.push(Draw::Fill { r, color: accent_fill(), radius: 8.0 * k });
            } else if hover {
                out.push(Draw::Fill { r, color: glass_hover(), radius: 8.0 * k });
            }
            let pulse = self.pulse_alpha(*a);
            if pulse > 0.0 {
                let mut c = accent();
                c[3] = 0.55 * pulse;
                out.push(Draw::Fill { r, color: c, radius: 8.0 * k });
            }
            let warn = *a == Action::Delete && hover;
            let tc = if !enabled { fg_dim() } else if warn { danger() } else { fg() };
            let is = 18.0 * k;
            out.push(Draw::Icon { name: icon_of(*a, label), r: (x + pad, r.1 + (h - is) / 2.0, is, is), color: tc, stroke: 1.5 * k });
            if compact {
                if !key.is_empty() {
                    keycap(out, x + pad + 22.0 * k, r.1 + h / 2.0, key, k, *enabled);
                }
            } else {
                let lx = x + pad + 24.0 * k;
                let lw = measure(label, size, false);
                out.push(text((lx, r.1, lw + 2.0, h), label, size, tc, *on, Align::Left));
                if !key.is_empty() {
                    keycap(out, lx + lw + 8.0 * k, r.1 + h / 2.0, key, k, *enabled);
                }
            }
            self.buttons.push((r, *a, *enabled));
            x += w + gap;
        }
        // Profiles, Shortcuts, Settings and Close, top right.
        let label = t("tool.close");
        let w = measure(label, size, false) + 2.0 * pad + 24.0 * k + measure("Esc", 11.0 * k, true) + 18.0 * k;
        let r = (geo.canvas.w as f32 - w - 16.0 * k, y + gap, w, h);
        let group_x = r.0 - 3.0 * (h + gap) - gap;
        out.push(Draw::Fill { r: (group_x - gap, y, r.0 + w + gap - group_x + gap, h + 2.0 * gap), color: glass(), radius: 12.0 * k });
        out.push(Draw::Stroke { r: (group_x - gap, y, r.0 + w + gap - group_x + gap, h + 2.0 * gap), color: hairline(), width: 1.0, radius: 12.0 * k, dashed: false });
        for (i, a) in [Action::Profiles, Action::Shortcuts, Action::Settings].into_iter().enumerate() {
            let br = (group_x + i as f32 * (h + gap), y + gap, h, h);
            let on = a == Action::Shortcuts && self.keymap_open;
            if on {
                out.push(Draw::Fill { r: br, color: accent_fill(), radius: 8.0 * k });
            } else if self.hover == Some(a) {
                out.push(Draw::Fill { r: br, color: glass_hover(), radius: 8.0 * k });
            }
            let pulse = self.pulse_alpha(a);
            if pulse > 0.0 {
                let mut c = accent();
                c[3] = 0.55 * pulse;
                out.push(Draw::Fill { r: br, color: c, radius: 8.0 * k });
            }
            let is = 18.0 * k;
            out.push(Draw::Icon { name: icon_of(a, ""), r: (br.0 + (h - is) / 2.0, br.1 + (h - is) / 2.0, is, is), color: fg(), stroke: 1.5 * k });
            self.buttons.push((br, a, true));
        }
        if self.hover == Some(Action::Close) {
            out.push(Draw::Fill { r, color: glass_hover(), radius: 8.0 * k });
        }
        let is = 18.0 * k;
        out.push(Draw::Icon { name: "x", r: (r.0 + pad, r.1 + (h - is) / 2.0, is, is), color: fg(), stroke: 1.5 * k });
        let lw = measure(label, size, false);
        out.push(text((r.0 + pad + 24.0 * k, r.1, lw + 2.0, h), label, size, fg(), false, Align::Left));
        keycap(out, r.0 + pad + 24.0 * k + lw + 8.0 * k, r.1 + h / 2.0, "Esc", k, true);
        self.buttons.push((r, Action::Close, true));
    }

    /// The keys that matter right now.
    fn draw_hints(&self, out: &mut Vec<Draw>, layout: &Layout, geo: &Geo) {
        let k = geo.scale;
        let arrange = self.view == View::Arrange;
        let hints: Vec<(&str, String)> = if arrange && self.naming.is_none() {
            if self.selected.is_some() {
                vec![
                    ("Drag", "move".into()),
                    ("Corner", "resize".into()),
                    ("F", "fill".into()),
                    ("A", "auto-arrange".into()),
                    ("H", "hide".into()),
                    ("Arrows", "nudge".into()),
                    ("Ctrl+Z", "undo".into()),
                ]
            } else {
                vec![("Click", "select".into()), ("1–9", "select".into()), ("A", "auto-arrange".into()), ("Space", "peek".into()), ("Tab", "cut".into()), ("Esc", "close".into())]
            }
        } else if let Some(n) = &self.naming {
            vec![("Type", "a name".into()), ("Enter", "confirm".into()), ("Esc", format!("keep \"{}\"", layout.cutout(n.n).map(|c| c.name.as_str()).unwrap_or("")))]
        } else if self.selected.is_some() {
            vec![
                ("Drag", "move".into()),
                ("Arrows", "nudge".into()),
                ("Shift+drag", "add".into()),
                ("Alt+drag", "subtract".into()),
                ("H", "hide".into()),
                ("Enter", "rename".into()),
                ("Esc", "deselect".into()),
            ]
        } else {
            vec![("Drag", "new cutout".into()), ("1–9", "select".into()), ("F5", "retake".into()), ("Right-click", "grids".into()), ("Tab", "arrange".into()), ("Esc", "close".into())]
        };
        let size = 13.0 * k;
        let gap = 18.0 * k;
        let parts: Vec<(f32, f32)> = hints.iter().map(|(key, l)| (measure(key, 11.0 * k, true) + 10.0 * k, measure(l, size, false))).collect();
        let total: f32 = parts.iter().map(|(a, b)| a + 6.0 * k + b).sum::<f32>() + gap * (parts.len() as f32 - 1.0) + 32.0 * k;
        let h = 36.0 * k;
        let mut x = (geo.canvas.w as f32 - total) / 2.0;
        let y = geo.canvas.h as f32 - h - 20.0 * k;
        out.push(Draw::Fill { r: (x, y, total, h), color: glass(), radius: h / 2.0 });
        x += 16.0 * k;
        for ((key, label), (kw, lw)) in hints.iter().zip(parts) {
            keycap(out, x, y + h / 2.0, key, k, true);
            x += kw + 6.0 * k;
            out.push(text((x, y, lw + 2.0, h), label, size, fg_muted(), false, Align::Left));
            x += lw + gap;
        }
    }

    /// App, layout, source size, and the selected cutout's size.
    fn draw_readout(&mut self, out: &mut Vec<Draw>, layout: &Layout, geo: &Geo, title: &str) {
        let k = geo.scale;
        let layout_label = match &self.layout_naming {
            Some(n) => format!("{title} · {}  ", n.typing.text()),
            None => format!("{title} · {}  ▾", layout.name),
        };
        let mut lines = vec![(layout_label, true)];
        let picture = if self.wants_live() { "live picture, V for still" } else if self.view == View::Arrange { "still frame, V for live" } else { "still frame, F5 to retake" };
        lines.push((format!("{} × {} · {picture}", geo.source_px.w, geo.source_px.h), false));
        if let Some(c) = self.selected.and_then(|n| layout.cutout(n)) {
            if self.view == View::Arrange {
                let r = super::arrange::place_px(c, geo);
                let pct = (super::arrange::scale_of(c, &r, geo) * 100.0).round();
                lines.push((format!("{} {} · {} × {} · {pct}% at {}, {}", number_label(c.number), c.name, r.w.round(), r.h.round(), r.x.round(), r.y.round()), false));
            } else {
                let b = c.bounds().scale(geo.source_px);
                lines.push((format!("{} {} · {} × {} at {}, {}", number_label(c.number), c.name, b.w.round(), b.h.round(), b.x.round(), b.y.round()), false));
            }
        }
        for p in &self.perf {
            lines.push((p.clone(), false));
        }
        let size = 12.0 * k;
        let w = lines.iter().map(|(l, b)| measure(l, size, *b)).fold(0.0, f32::max) + 28.0 * k;
        let lh = 20.0 * k;
        let h = lh * lines.len() as f32 + 16.0 * k;
        let x = geo.canvas.w as f32 - w - 16.0 * k;
        let y = geo.canvas.h as f32 - h - 16.0 * k;
        out.push(Draw::Fill { r: (x, y, w, h), color: glass(), radius: 12.0 * k });
        // The layout line is a button: switch, rename or add a layout.
        let hit = (x + 6.0 * k, y + 6.0 * k, w - 12.0 * k, lh + 4.0 * k);
        self.readout_hit = Some(hit);
        let hovered = self.hover_since.as_ref().is_some_and(|(h, _)| *h == super::chrome::HoverKey::Readout);
        if self.layout_naming.is_some() {
            out.push(Draw::Stroke { r: hit, color: accent(), width: 1.5 * k, radius: 6.0 * k, dashed: false });
        } else if hovered {
            out.push(Draw::Fill { r: hit, color: glass_hover(), radius: 6.0 * k });
        }
        for (i, (l, b)) in lines.iter().enumerate() {
            let r = (x + 14.0 * k, y + 8.0 * k + lh * i as f32, w, lh);
            match (&self.layout_naming, i) {
                (Some(n), 0) => {
                    // The app's name, then the layout name being typed, with caret and selection.
                    let lead = format!("{title} · ");
                    let lw = measure(&lead, size, true);
                    out.push(text(r, &lead, size, fg(), true, Align::Left));
                    out.extend(n.typing.draws((r.0 + lw, r.1, w - lw - 28.0 * k, lh), size, true, fg(), accent_fill(), accent(), 1.5 * k));
                }
                _ => out.push(text(r, l, size, if *b { fg() } else { fg_muted() }, *b, Align::Left)),
            }
        }
    }
}
