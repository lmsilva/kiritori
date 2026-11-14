//! Arrange view: where each cutout goes on the canvas.
//!
//! Works in canvas pixels. Placements are stored as fractions of the canvas, so a layout made
//! on one display size works on another.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use super::style::*;
use super::{Effect, Geo, Layer};
use crate::engine::draw::{Align, Draw};
use crate::engine::render::measure;
use crate::model::geometry::{Rect, Size};
use crate::model::layout::{Cutout, Layout, Side};
use crate::shell::canvas::Mods;

/// Undo, redo, fill, auto-arrange and reset glide over this long.
pub const GLIDE: Duration = Duration::from_millis(180);
/// Scale percentages that resizing snaps to.
const SCALE_STOPS: [f64; 3] = [1.0, 1.5, 2.0];

#[derive(Clone, Debug)]
pub(super) enum PDrag {
    Move { n: u32, grab: (f64, f64), orig: Rect },
    Resize { n: u32, handle: usize, orig: Rect },
}

/// A guide line in canvas pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Guide {
    X(f64),
    Y(f64),
}

/// Placement rect in canvas pixels.
pub fn place_px(c: &Cutout, geo: &Geo) -> Rect {
    c.placement.rect.scale(geo.canvas)
}

/// Shown size as a share of the cutout's source size: 1.5 is 150%.
pub fn scale_of(c: &Cutout, px: &Rect, geo: &Geo) -> f64 {
    let s = c.pixel_size(geo.source_px);
    if s.w > 0.0 { px.w / s.w } else { 1.0 }
}

fn ease_out(t: f64) -> f64 {
    1.0 - (1.0 - t).powi(3)
}

impl Layer {
    /// Start a glide from the placements in `before` to whatever the layout now holds.
    pub fn start_glide(&mut self, before: &Layout) {
        let from: HashMap<u32, Rect> = before.cutouts.iter().map(|c| (c.number, c.placement.rect)).collect();
        self.glide = Some((Instant::now(), from));
    }

    pub fn animating(&self) -> bool {
        self.glide.as_ref().is_some_and(|(t, _)| t.elapsed() < GLIDE)
    }

    /// Where to draw a placement right now, part way through a glide.
    pub(super) fn shown(&self, n: u32, target: Rect) -> Rect {
        let Some((t, from)) = &self.glide else { return target };
        let Some(f) = from.get(&n) else { return target };
        let k = (t.elapsed().as_secs_f64() / GLIDE.as_secs_f64()).min(1.0);
        if k >= 1.0 {
            return target;
        }
        let e = ease_out(k);
        let mix = |a: f64, b: f64| a + (b - a) * e;
        Rect::new(mix(f.x, target.x), mix(f.y, target.y), mix(f.w, target.w), mix(f.h, target.h))
    }

    /// Topmost cutout whose placement is under a canvas point; hidden ones (ghosts) too.
    pub(super) fn placed_at(&self, x: f32, y: f32, layout: &Layout, geo: &Geo) -> Option<u32> {
        let (x, y) = (x as f64, y as f64);
        let order: Vec<&Cutout> = layout.by_z().into_iter().filter(|c| c.placement.display == geo.display && !self.parked.contains(&c.number)).collect();
        let hit = |c: &&&Cutout| place_px(c, geo).contains(x, y);
        order.iter().rev().filter(|c| c.placement.visible).find(hit).or_else(|| order.iter().rev().find(hit)).map(|c| c.number)
    }

    pub(super) fn arr_handles(&self, layout: &Layout, geo: &Geo) -> Vec<(f32, f32)> {
        let Some(c) = self.selected.and_then(|n| layout.cutout(n)).filter(|c| c.placement.display == geo.display) else { return Vec::new() };
        let r = place_px(c, geo);
        let (x, y, w, h) = (r.x as f32, r.y as f32, r.w as f32, r.h as f32);
        let (mx, my, rr, b) = (x + w / 2.0, y + h / 2.0, x + w, y + h);
        vec![(x, y), (mx, y), (rr, y), (rr, my), (rr, b), (mx, b), (x, b), (x, my)]
    }

    fn arr_handle_at(&self, x: f32, y: f32, layout: &Layout, geo: &Geo) -> Option<usize> {
        let reach = 9.0 * geo.scale;
        self.arr_handles(layout, geo).iter().position(|(hx, hy)| (hx - x).abs() <= reach && (hy - y).abs() <= reach)
    }

    pub(super) fn arr_down(&mut self, x: f32, y: f32, layout: &mut Layout, geo: &Geo) -> Effect {
        let p = (x as f64, y as f64);
        if let Some(n) = self.selected.filter(|n| layout.cutout(*n).is_some_and(|c| !c.placement.locked))
            && let Some(h) = self.arr_handle_at(x, y, layout, geo) {
                self.pdrag = Some(PDrag::Resize { n, handle: h, orig: place_px(layout.cutout(n).unwrap(), geo) });
                return Effect::None;
            }
        match self.placed_at(x, y, layout, geo) {
            Some(n) => {
                self.selected = Some(n);
                let c = layout.cutout(n).unwrap();
                if !c.placement.locked {
                    self.pdrag = Some(PDrag::Move { n, grab: p, orig: place_px(c, geo) });
                }
            }
            None => self.selected = None,
        }
        Effect::None
    }

    /// Guide targets: the canvas's edges and centre lines, and other visible cutouts' edges and
    /// centres on the same canvas.
    fn guide_targets(&self, layout: &Layout, geo: &Geo, except: u32) -> (Vec<f64>, Vec<f64>) {
        let (w, h) = (geo.canvas.w, geo.canvas.h);
        let mut xs = vec![0.0, w / 2.0, w];
        let mut ys = vec![0.0, h / 2.0, h];
        for c in layout.cutouts.iter().filter(|c| c.number != except && c.placement.visible && c.placement.display == geo.display) {
            let r = place_px(c, geo);
            xs.extend([r.x, r.x + r.w / 2.0, r.right()]);
            ys.extend([r.y, r.y + r.h / 2.0, r.bottom()]);
        }
        (xs, ys)
    }

    fn guide_reach(&self, geo: &Geo) -> f64 {
        10.0 * geo.scale as f64
    }

    /// Best snap of any of `edges` to `targets`: (offset to apply, target).
    fn best(edges: &[f64], targets: &[f64], reach: f64) -> Option<(f64, f64)> {
        edges
            .iter()
            .flat_map(|e| targets.iter().map(move |t| (t - e, *t)))
            .filter(|(d, _)| d.abs() <= reach)
            .min_by(|a, b| a.0.abs().total_cmp(&b.0.abs()))
    }

    pub(super) fn arr_move(&mut self, x: f32, y: f32, layout: &mut Layout, geo: &Geo, m: Mods) -> Effect {
        let p = (x as f64, y as f64);
        let snapping = self.snapping && !m.ctrl;
        self.guides.clear();
        match self.pdrag.clone() {
            Some(PDrag::Move { n, grab, orig }) => {
                let mut r = Rect::new(orig.x + p.0 - grab.0, orig.y + p.1 - grab.1, orig.w, orig.h);
                if snapping {
                    let (xs, ys) = self.guide_targets(layout, geo, n);
                    let reach = self.guide_reach(geo);
                    if let Some((d, t)) = Self::best(&[r.x, r.x + r.w / 2.0, r.right()], &xs, reach) {
                        r.x += d;
                        self.guides.push(Guide::X(t));
                    }
                    if let Some((d, t)) = Self::best(&[r.y, r.y + r.h / 2.0, r.bottom()], &ys, reach) {
                        r.y += d;
                        self.guides.push(Guide::Y(t));
                    }
                }
                layout.set_placement(n, r.unscale(geo.canvas));
            }
            Some(PDrag::Resize { n, handle, orig }) => {
                let src = layout.cutout(n).unwrap().pixel_size(geo.source_px);
                // Edge handles stretch that one side (width or height alone); corners keep the
                // shape. Shift swaps the two.
                let edge = matches!(handle, 1 | 3 | 5 | 7);
                let r = if edge != m.shift {
                    self.scale_label = None;
                    Self::stretch(orig, handle, p)
                } else {
                    self.locked_resize(orig, handle, p, src, snapping)
                };
                layout.set_placement(n, r.unscale(geo.canvas));
            }
            None => {}
        }
        Effect::None
    }

    /// Free resize (an edge handle, or a corner with Shift): the dragged edges follow the pointer.
    fn stretch(o: Rect, handle: usize, p: (f64, f64)) -> Rect {
        let (mut l, mut t, mut r, mut b) = (o.x, o.y, o.right(), o.bottom());
        if matches!(handle, 0 | 6 | 7) {
            l = p.0;
        }
        if matches!(handle, 2..=4) {
            r = p.0;
        }
        if matches!(handle, 0..=2) {
            t = p.1;
        }
        if matches!(handle, 4..=6) {
            b = p.1;
        }
        let (l, r) = (l.min(r), l.max(r).max(l.min(r) + 16.0));
        let (t, b) = (t.min(b), t.max(b).max(t.min(b) + 16.0));
        Rect::new(l, t, r - l, b - t)
    }

    /// Aspect-locked resize from the handle opposite the one dragged; snaps to 100%, 150% and
    /// 200% of the source size.
    fn locked_resize(&mut self, o: Rect, handle: usize, p: (f64, f64), src: Size, snapping: bool) -> Rect {
        let aspect = o.w / o.h.max(1.0);
        let mut w = match handle {
            0 | 6 => (o.right() - p.0).max(16.0),
            2 | 4 => (p.0 - o.x).max(16.0),
            7 => (o.right() - p.0).max(16.0),
            3 => (p.0 - o.x).max(16.0),
            1 => (o.bottom() - p.1).max(16.0) * aspect,
            _ => (p.1 - o.y).max(16.0) * aspect,
        };
        if matches!(handle, 0 | 2 | 4 | 6) {
            // Corners follow whichever axis the pointer has moved further along.
            let h_based = match handle {
                0 | 2 => (o.bottom() - p.1).max(16.0) * aspect,
                _ => (p.1 - o.y).max(16.0) * aspect,
            };
            w = w.max(h_based);
        }
        self.scale_label = None;
        if snapping && src.w > 0.0 {
            let pct = w / src.w;
            if let Some(stop) = SCALE_STOPS.iter().find(|s| (pct - *s).abs() < 0.03) {
                w = src.w * stop;
                self.scale_label = Some(*stop);
            }
        }
        let h = w / aspect;
        let (cx, cy) = (o.x + o.w / 2.0, o.y + o.h / 2.0);
        let x = match handle {
            0 | 6 | 7 => o.right() - w,
            1 | 5 => cx - w / 2.0,
            _ => o.x,
        };
        let y = match handle {
            0..=2 => o.bottom() - h,
            3 | 7 => cy - h / 2.0,
            _ => o.y,
        };
        Rect::new(x, y, w, h)
    }

    pub(super) fn arr_up(&mut self, layout: &Layout, geo: &Geo) -> Effect {
        self.guides.clear();
        self.scale_label = None;
        match self.pdrag.take() {
            Some(PDrag::Move { n, orig, .. }) => {
                let c = layout.cutout(n).unwrap();
                if place_px(c, geo) == orig {
                    return Effect::None;
                }
                Effect::Edit(format!("Move {}", c.name), None)
            }
            Some(PDrag::Resize { n, .. }) => Effect::Edit(format!("Resize {}", layout.cutout(n).unwrap().name), None),
            None => Effect::None,
        }
    }

    /// The cutout being moved in Arrange view, if any (for dragging across displays).
    pub fn arr_dragging_number(&self) -> Option<u32> {
        match self.pdrag {
            Some(PDrag::Move { n, .. }) => Some(n),
            _ => None,
        }
    }

    pub(super) fn arr_dragging(&self) -> Option<u32> {
        match &self.pdrag {
            Some(PDrag::Move { n, .. }) | Some(PDrag::Resize { n, .. }) => Some(*n),
            None => None,
        }
    }

    /// Fill (F): largest size that fits, centred; again to restore.
    pub(super) fn fill(&mut self, layout: &mut Layout, geo: &Geo) -> Effect {
        let Some(n) = self.selected.filter(|n| layout.cutout(*n).is_some()) else {
            self.say("Select a cutout first (click it or press its number)");
            return Effect::None;
        };
        let restoring = layout.cutout(n).unwrap().placement.before_fill.is_some();
        layout.toggle_fill(n, &geo.frame());
        let name = layout.cutout(n).unwrap().name.clone();
        Effect::Edit(format!("{} {name}", if restoring { "Restore" } else { "Fill" }), None)
    }

    /// Auto-arrange (A): the selected cutout, or cutout 1, gets the largest share. Pressing A
    /// again tries the other side.
    pub(super) fn auto_arrange(&mut self, layout: &mut Layout, geo: &Geo) -> Effect {
        // Never focus a hidden cutout (it is often still selected right after pressing H):
        // fall back to the lowest-numbered visible one.
        let visible = |n: &u32| layout.cutout(*n).is_some_and(|c| c.placement.visible);
        let focus = self.selected.filter(visible).or_else(|| layout.cutouts.iter().map(|c| c.number).filter(visible).min());
        let Some(focus) = focus else {
            self.say("Nothing to arrange yet: cut something in Cut view first");
            return Effect::None;
        };
        let side = match self.last_arrange {
            Some((n, s)) if n == focus => Some(if s == Side::Below { Side::Beside } else { Side::Below }),
            _ => None,
        };
        let used = layout.auto_arrange(focus, &geo.frame(), side);
        self.last_arrange = used.map(|s| (focus, s));
        Effect::Edit("Auto-arrange".into(), None)
    }

    pub(super) fn reset_placement(&mut self, layout: &mut Layout, geo: &Geo) -> Effect {
        let Some(n) = self.selected.filter(|n| layout.cutout(*n).is_some()) else { return Effect::None };
        layout.reset_placement(n, &geo.frame());
        Effect::Edit(format!("Reset placement of {}", layout.cutout(n).unwrap().name), None)
    }

    /// Arrow keys in Arrange view: move by 1 px (Shift 10). Ctrl resizes keeping the shape;
    /// Alt stretches: Left and Right change the width alone, Up and Down the height alone.
    pub(super) fn arr_nudge(&mut self, vk: u16, m: Mods, layout: &mut Layout, geo: &Geo) -> Effect {
        let Some(n) = self.selected.filter(|n| layout.cutout(*n).is_some_and(|c| !c.placement.locked)) else { return Effect::None };
        let step = if m.shift { 10.0 } else { 1.0 };
        let (dx, dy) = match vk {
            0x25 => (-step, 0.0),
            0x26 => (0.0, -step),
            0x27 => (step, 0.0),
            _ => (0.0, step),
        };
        let c = layout.cutout(n).unwrap();
        let name = c.name.clone();
        let r = place_px(c, geo);
        let new = if m.alt {
            Rect::new(r.x, r.y, (r.w + dx).max(16.0), (r.h + dy).max(16.0))
        } else if m.ctrl {
            let w = (r.w + dx + dy).max(16.0);
            Rect::new(r.x, r.y, w, w * r.h / r.w)
        } else {
            Rect::new(r.x + dx, r.y + dy, r.w, r.h)
        };
        layout.set_placement(n, new.unscale(geo.canvas));
        if m.ctrl || m.alt {
            Effect::Edit(format!("Resize {name}"), Some(format!("presize-{n}")))
        } else {
            Effect::Edit(format!("Move {name}"), Some(format!("pmove-{n}")))
        }
    }

    /// Arrange view's picture: the canvas exactly as it will look, plus outlines and tags.
    pub(super) fn draw_arrange(&self, out: &mut Vec<Draw>, layout: &Layout, geo: &Geo) {
        let k = geo.scale;
        let here: Vec<&Cutout> = layout.by_z().into_iter().filter(|c| c.placement.display == geo.display && !self.parked.contains(&c.number)).collect();
        // Hidden cutouts are faint ghosts under the visible ones, never over them.
        let mut ordered: Vec<&&Cutout> = here.iter().filter(|c| !c.placement.visible).collect();
        ordered.extend(here.iter().filter(|c| c.placement.visible));
        for c in ordered {
            let r = self.shown(c.number, c.placement.rect).scale(geo.canvas);
            let dst = (r.x as f32, r.y as f32, r.w as f32, r.h as f32);
            if c.placement.visible {
                out.push(Draw::Source { orient: c.orient(), dst, src: c.bounds(), shape: c.rects.clone(), filter: c.placement.filter, dim: 1.0, alpha: 1.0, circle: false, input: false });
            } else if !self.peek {
                out.push(Draw::Source { orient: c.orient(), dst, src: c.bounds(), shape: c.rects.clone(), filter: c.placement.filter, dim: 1.0, alpha: 0.25, circle: false, input: false });
            }
        }
        if self.peek {
            return;
        }
        for c in here {
            let r = self.shown(c.number, c.placement.rect).scale(geo.canvas);
            let dst = (r.x as f32, r.y as f32, r.w as f32, r.h as f32);
            let col = cutout(c.color);
            let sel = self.selected == Some(c.number);
            let w = if sel { 3.0 } else { 2.0 } * k;
            out.push(Draw::Stroke { r: dst, color: HALO, width: w + 2.0 * k, radius: 0.0, dashed: false });
            out.push(Draw::Stroke { r: dst, color: col, width: w, radius: 0.0, dashed: !c.placement.visible });
            self.draw_tag(out, c, dst, geo);
            if sel && !c.placement.locked && !self.animating() {
                for (hx, hy) in self.arr_handles(layout, geo) {
                    let hs = 10.0 * k;
                    out.push(Draw::Fill { r: (hx - hs / 2.0 - k, hy - hs / 2.0 - k, hs + 2.0 * k, hs + 2.0 * k), color: HALO, radius: 2.0 * k });
                    out.push(Draw::Fill { r: (hx - hs / 2.0, hy - hs / 2.0, hs, hs), color: col, radius: 2.0 * k });
                }
            }
        }
        for g in &self.guides {
            match *g {
                Guide::X(x) => out.push(Draw::Line { a: (x as f32, 0.0), b: (x as f32, geo.canvas.h as f32), color: SNAP, width: 1.0 * k }),
                Guide::Y(y) => out.push(Draw::Line { a: (0.0, y as f32), b: (geo.canvas.w as f32, y as f32), color: SNAP, width: 1.0 * k }),
            }
        }
        // Live size label while dragging: "1920 × 1080 · 150%".
        if let Some(c) = self.arr_dragging().and_then(|n| layout.cutout(n)).filter(|c| c.placement.display == geo.display) {
            let r = place_px(c, geo);
            let pct = (scale_of(c, &r, geo) * 100.0).round();
            let label = format!("{} × {} · {pct}%", r.w.round(), r.h.round());
            let size = 13.0 * k;
            let lw = measure(&label, size, true) + 16.0 * k;
            let (mut lx, mut ly) = (self.pointer.0 + 16.0 * k, self.pointer.1 + 16.0 * k);
            // Keep clear of the hint strip and the screen edges.
            if ly + 24.0 * k > geo.canvas.h as f32 - 80.0 * k {
                ly = self.pointer.1 - 40.0 * k;
            }
            if lx + lw > geo.canvas.w as f32 - 8.0 * k {
                lx = self.pointer.0 - 16.0 * k - lw;
            }
            out.push(Draw::Fill { r: (lx, ly, lw, 24.0 * k), color: if self.scale_label.is_some() { accent_fill() } else { glass() }, radius: 6.0 * k });
            out.push(Draw::Text { r: (lx, ly, lw, 24.0 * k), text: label, size, color: fg(), bold: true, align: Align::Center });
        }
    }
}
