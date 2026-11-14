//! The tutorial card, drawn in the layer. The tutorial's progress lives in the
//! app (it survives closing the layer); the layer only shows the current step and reports
//! clicks on Skip step and End tutorial.

use super::style::*;
use super::{Action, Effect, Geo, Layer};
use crate::engine::draw::{Align, Draw, Px};
use crate::engine::render::measure;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuideClick {
    SkipStep,
    End,
}

/// What the card shows for the current step.
#[derive(Clone, Debug, PartialEq)]
pub struct GuideCard {
    pub index: usize,
    pub total: usize,
    pub title: &'static str,
    pub body: &'static str,
    pub key: &'static str,
    pub target: Option<Action>,
}

fn inside(r: &Px, x: f32, y: f32) -> bool {
    x >= r.0 && x < r.0 + r.2 && y >= r.1 && y < r.1 + r.3
}

impl Layer {
    pub(super) fn guide_hit(&self, x: f32, y: f32) -> Option<GuideClick> {
        self.guide_hits.iter().find(|(r, _)| inside(r, x, y)).map(|(_, c)| *c)
    }

    pub(super) fn guide_down(&mut self, c: GuideClick) -> Effect {
        Effect::Guide(c)
    }

    /// The card at the bottom left, and a ring around the toolbar button the step uses.
    pub(super) fn draw_guide(&mut self, out: &mut Vec<Draw>, geo: &Geo) {
        self.guide_hits.clear();
        let Some(card) = self.guide.clone() else { return };
        let k = geo.scale;

        // Ring the button this step uses, so it can be clicked instead of pressing the key.
        if let Some((r, _, _)) = card.target.and_then(|t| self.buttons.iter().find(|b| b.1 == t)) {
            let pad = 4.0 * k;
            out.push(Draw::Stroke { r: (r.0 - pad, r.1 - pad, r.2 + 2.0 * pad, r.3 + 2.0 * pad), color: accent(), width: 2.5 * k, radius: 10.0 * k, dashed: false });
        }

        let w = 440.0 * k;
        let h = 176.0 * k;
        let x = 16.0 * k;
        // Bottom left, above the display map when it shows.
        let floor = self.map_rect.map(|r| r.1 - 12.0 * k).unwrap_or(geo.canvas.h as f32 - 76.0 * k);
        let y = floor - h;
        out.push(Draw::Fill { r: (x, y, w, h), color: glass_hover(), radius: 14.0 * k });
        out.push(Draw::Stroke { r: (x, y, w, h), color: accent(), width: 1.5 * k, radius: 14.0 * k, dashed: false });

        // Header: "Getting started · 2 of 7" and progress dots.
        out.push(Draw::Text { r: (x + 18.0 * k, y + 10.0 * k, w, 24.0 * k), text: format!("Getting started · {} of {}", card.index + 1, card.total), size: 12.0 * k, color: fg_muted(), bold: true, align: Align::Left });
        for i in 0..card.total {
            let d = 8.0 * k;
            let dx = x + w - 18.0 * k - (card.total - i) as f32 * (d + 6.0 * k);
            let col = if i < card.index { accent() } else if i == card.index { fg() } else { keycap_fill() };
            out.push(Draw::Fill { r: (dx, y + 18.0 * k, d, d), color: col, radius: d / 2.0 });
        }

        // The step: what to do, and how.
        out.push(Draw::Text { r: (x + 18.0 * k, y + 38.0 * k, w - 36.0 * k, 30.0 * k), text: card.title.into(), size: 16.0 * k, color: fg(), bold: true, align: Align::Left });
        out.push(Draw::Text { r: (x + 18.0 * k, y + 68.0 * k, w - 36.0 * k, 24.0 * k), text: card.body.into(), size: 12.5 * k, color: fg_muted(), bold: false, align: Align::Left });
        let kw = measure(card.key, 12.0 * k, true) + 14.0 * k;
        out.push(Draw::Text { r: (x + 18.0 * k, y + 100.0 * k, 60.0 * k, 26.0 * k), text: "Press".into(), size: 12.5 * k, color: fg_muted(), bold: false, align: Align::Left });
        let kx = x + 18.0 * k + measure("Press", 12.5 * k, false) + 8.0 * k;
        out.push(Draw::Fill { r: (kx, y + 102.0 * k, kw, 22.0 * k), color: keycap_fill(), radius: 5.0 * k });
        out.push(Draw::Text { r: (kx, y + 102.0 * k, kw, 22.0 * k), text: card.key.into(), size: 12.0 * k, color: fg(), bold: true, align: Align::Center });
        if card.target.is_some() {
            out.push(Draw::Text { r: (kx + kw + 8.0 * k, y + 100.0 * k, w, 26.0 * k), text: "or click the ringed button".into(), size: 12.5 * k, color: fg_muted(), bold: false, align: Align::Left });
        }

        // Skip step · End tutorial.
        let mut bx = x + w - 18.0 * k;
        for (label, click) in [("End tutorial", GuideClick::End), ("Skip step", GuideClick::SkipStep)] {
            let bw = measure(label, 12.5 * k, false) + 24.0 * k;
            bx -= bw;
            let r = (bx, y + h - 40.0 * k, bw, 28.0 * k);
            let hover = inside(&r, self.pointer.0, self.pointer.1);
            out.push(Draw::Fill { r, color: if hover { accent_fill() } else { glass() }, radius: 7.0 * k });
            out.push(Draw::Text { r, text: label.into(), size: 12.5 * k, color: fg(), bold: false, align: Align::Center });
            self.guide_hits.push((r, click));
            bx -= 8.0 * k;
        }
    }
}
