//! The layer on several displays: the display map at the bottom
//! left of Arrange view, the number badge each display shows for two seconds, and what other
//! displays show while the layer is open.

use std::time::Duration;

use super::style::*;
use super::{Geo, Layer};
use crate::engine::draw::{Align, Draw, Px, Rgba};
use crate::model::geometry::Rect;
use crate::model::layout::DisplayMode;

/// The number badges show this long after the layer opens.
pub const BADGE_TIME: Duration = Duration::from_secs(2);

/// One display in the map, as the app sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct MapDisplay {
    pub number: u32,
    /// The display in screen pixels.
    pub rect: (i32, i32, i32, i32),
    /// Takeover or Float, or None for Not in use.
    pub mode: Option<DisplayMode>,
    /// The pointer is on it: it carries the toolbar.
    pub here: bool,
    /// Its cutouts: placement (display fractions) and colour.
    pub cutouts: Vec<(Rect, Rgba)>,
}

pub fn mode_icon(mode: Option<DisplayMode>) -> &'static str {
    match mode {
        Some(DisplayMode::Takeover) => "monitor",
        Some(DisplayMode::Float) => "app-window",
        None => "monitor-off",
    }
}

pub fn mode_label(mode: Option<DisplayMode>) -> &'static str {
    match mode {
        Some(DisplayMode::Takeover) => "Takeover",
        Some(DisplayMode::Float) => "Float",
        None => "Not in use",
    }
}

impl Layer {
    pub fn badges_showing(&self) -> bool {
        self.opened.elapsed() < BADGE_TIME
    }

    /// A large display number in the top-left corner, for the first two seconds.
    pub(super) fn draw_badge(&self, out: &mut Vec<Draw>, geo: &Geo) {
        if !self.badges_showing() || geo.number == 0 {
            return;
        }
        let k = geo.scale;
        let r = (24.0 * k, 24.0 * k, 120.0 * k, 120.0 * k);
        out.push(Draw::Fill { r, color: glass(), radius: 24.0 * k });
        out.push(Draw::Stroke { r, color: if geo.here { accent() } else { hairline() }, width: 2.0 * k, radius: 24.0 * k, dashed: false });
        out.push(Draw::Text { r, text: geo.number.to_string(), size: 72.0 * k, color: fg(), bold: true, align: Align::Center });
    }

    /// The display map: a miniature of the Windows display arrangement, each display numbered,
    /// with its mode icon and its cutouts. Clicking one offers Takeover, Float, Not in use.
    pub(super) fn draw_map(&mut self, out: &mut Vec<Draw>, geo: &Geo) {
        self.map_hits.clear();
        self.map_rect = None;
        if !self.map_open || self.map.len() < 2 {
            return;
        }
        let k = geo.scale;
        let (x0, y0) = self.map.iter().fold((i32::MAX, i32::MAX), |a, d| (a.0.min(d.rect.0), a.1.min(d.rect.1)));
        let (x1, y1) = self.map.iter().fold((i32::MIN, i32::MIN), |a, d| (a.0.max(d.rect.0 + d.rect.2), a.1.max(d.rect.1 + d.rect.3)));
        let (bw, bh) = (300.0 * k, 150.0 * k);
        let s = (bw / (x1 - x0) as f32).min(bh / (y1 - y0) as f32);
        let (mw, mh) = ((x1 - x0) as f32 * s, (y1 - y0) as f32 * s);
        let pad = 12.0 * k;
        let title_h = 24.0 * k;
        let card = (16.0 * k, geo.canvas.h as f32 - 16.0 * k - mh - 2.0 * pad - title_h, mw + 2.0 * pad, mh + 2.0 * pad + title_h);
        self.map_rect = Some(card);
        out.push(Draw::Fill { r: card, color: glass(), radius: 12.0 * k });
        out.push(Draw::Stroke { r: card, color: hairline(), width: 1.0, radius: 12.0 * k, dashed: false });
        out.push(Draw::Text { r: (card.0 + pad, card.1 + 4.0 * k, card.2 - 2.0 * pad, title_h), text: "Displays · M".into(), size: 11.0 * k, color: fg_muted(), bold: true, align: Align::Left });
        let ox = card.0 + pad;
        let oy = card.1 + pad + title_h - 4.0 * k;
        for (i, d) in self.map.iter().enumerate() {
            let r: Px = (ox + (d.rect.0 - x0) as f32 * s + 2.0 * k, oy + (d.rect.1 - y0) as f32 * s + 2.0 * k, d.rect.2 as f32 * s - 4.0 * k, d.rect.3 as f32 * s - 4.0 * k);
            let used = d.mode.is_some();
            out.push(Draw::Fill { r, color: if used { [0.0, 0.0, 0.0, 0.85] } else { keycap_fill() }, radius: 4.0 * k });
            for (c, col) in &d.cutouts {
                let cr = (r.0 + c.x as f32 * r.2, r.1 + c.y as f32 * r.3, (c.w as f32 * r.2).max(2.0), (c.h as f32 * r.3).max(2.0));
                let mut fill = *col;
                fill[3] = 0.55;
                out.push(Draw::Fill { r: cr, color: fill, radius: 1.0 });
                out.push(Draw::Stroke { r: cr, color: *col, width: 1.0, radius: 1.0, dashed: false });
            }
            out.push(Draw::Stroke { r, color: if d.here { accent() } else { hairline() }, width: if d.here { 2.0 * k } else { 1.0 }, radius: 4.0 * k, dashed: !used });
            let ts = (r.3 * 0.4).clamp(10.0 * k, 22.0 * k);
            out.push(Draw::Text { r: (r.0 + 6.0 * k, r.1 + 2.0 * k, r.2, ts * 1.4), text: d.number.to_string(), size: ts, color: if used { [1.0, 1.0, 1.0, 0.95] } else { fg_muted() }, bold: true, align: Align::Left });
            let is = (r.3 * 0.25).clamp(10.0 * k, 16.0 * k);
            out.push(Draw::Icon { name: mode_icon(d.mode), r: (r.0 + r.2 - is - 6.0 * k, r.1 + 6.0 * k, is, is), color: if used { [1.0, 1.0, 1.0, 0.8] } else { fg_muted() }, stroke: 1.6 * k });
            self.map_hits.push((r, i));
        }
    }

    /// The display map miniature under a canvas point.
    pub(super) fn map_hit(&self, x: f32, y: f32) -> Option<usize> {
        self.map_hits.iter().find(|(r, _)| x >= r.0 && x < r.0 + r.2 && y >= r.1 && y < r.1 + r.3).map(|(_, i)| *i)
    }
}
