//! Reset and the history list.
//!
//! Reset opens a small card with two choices, either from the toolbar button or by holding
//! Ctrl+Shift+Backspace for one second while a ring fills (holding prevents accidents).
//! Pressing and holding the Undo button opens the list of steps, newest first; clicking a row
//! jumps back to that point.

use std::time::Duration;

use super::style::*;
use super::{Action, Effect, Geo, Layer, View};
use crate::engine::draw::{Align, Draw, Px};
use crate::engine::render::measure;
use crate::model::layout::Layout;

pub const HOLD_TO_RESET: Duration = Duration::from_secs(1);
/// Holding the Undo button this long opens the history list instead of undoing once.
pub const HOLD_FOR_HISTORY: Duration = Duration::from_millis(450);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResetChoice {
    Arrangement,
    StartOver,
    Cancel,
}

fn inside(r: &Px, x: f32, y: f32) -> bool {
    x >= r.0 && x < r.0 + r.2 && y >= r.1 && y < r.1 + r.3
}

impl Layer {
    /// Progress of the Ctrl+Shift+Backspace hold, 0–1, while it is held.
    pub fn reset_hold(&self) -> Option<f32> {
        self.reset_held.map(|t| (t.elapsed().as_secs_f32() / HOLD_TO_RESET.as_secs_f32()).min(1.0))
    }

    /// Called on every tick: a completed hold opens the card; a long press on Undo opens the
    /// history list.
    pub(super) fn advance_holds(&mut self) {
        if self.reset_held.is_some_and(|t| t.elapsed() >= HOLD_TO_RESET) {
            self.reset_held = None;
            self.reset_card = true;
        }
        if let Some((Action::Undo, t)) = self.button_press
            && t.elapsed() >= HOLD_FOR_HISTORY
            && !self.history_open
        {
            self.history_open = true;
            self.history_from_hold = true;
        }
    }

    pub(super) fn reset_card_down(&mut self, x: f32, y: f32, layout: &mut Layout, geo: &Geo) -> Effect {
        let choice = self.reset_hits.iter().find(|(r, _)| inside(r, x, y)).map(|(_, c)| *c);
        match choice {
            Some(c) => self.choose_reset(c, layout, geo),
            None => Effect::None, // clicks outside the card do nothing while it is open
        }
    }

    pub(super) fn choose_reset(&mut self, c: ResetChoice, layout: &mut Layout, geo: &Geo) -> Effect {
        self.reset_card = false;
        match c {
            ResetChoice::Cancel => Effect::None,
            ResetChoice::Arrangement => {
                layout.reset_arrangement(&geo.frame());
                self.say("Every cutout is back at its source position. Ctrl+Z undoes this.");
                Effect::Edit("Reset arrangement".into(), None)
            }
            ResetChoice::StartOver => {
                if layout.cutouts.is_empty() {
                    return Effect::None;
                }
                layout.start_over();
                self.selected = None;
                self.naming = None;
                self.view = View::Cut;
                self.say("All cutouts removed. Ctrl+Z brings them back.");
                Effect::Edit("Start over".into(), None)
            }
        }
    }

    pub(super) fn history_down(&mut self, x: f32, y: f32) -> Effect {
        let row = self.history_hits.iter().find(|(r, _)| inside(r, x, y)).map(|(_, i)| *i);
        self.history_open = false;
        match row {
            Some(i) => Effect::UndoMany(i + 1),
            None => Effect::None,
        }
    }

    /// The reset card: two choices, each with what it does, and Cancel.
    pub(super) fn draw_reset_card(&mut self, out: &mut Vec<Draw>, geo: &Geo) {
        self.reset_hits.clear();
        if !self.reset_card {
            return;
        }
        let k = geo.scale;
        let (w, h) = (560.0 * k, 300.0 * k);
        let x = (geo.canvas.w as f32 - w) / 2.0;
        let y = (geo.canvas.h as f32 - h) / 2.0;
        out.push(Draw::Fill { r: (0.0, 0.0, geo.canvas.w as f32, geo.canvas.h as f32), color: [0.0, 0.0, 0.0, 0.35], radius: 0.0 });
        out.push(Draw::Fill { r: (x, y, w, h), color: glass_hover(), radius: 14.0 * k });
        out.push(Draw::Stroke { r: (x, y, w, h), color: hairline(), width: 1.0, radius: 14.0 * k, dashed: false });
        out.push(Draw::Icon { name: "rotate-ccw", r: (x + 22.0 * k, y + 20.0 * k, 22.0 * k, 22.0 * k), color: fg(), stroke: 1.8 * k });
        out.push(Draw::Text { r: (x + 54.0 * k, y + 14.0 * k, w, 34.0 * k), text: "Reset".into(), size: 17.0 * k, color: fg(), bold: true, align: Align::Left });
        let choices = [
            (ResetChoice::Arrangement, "rotate-ccw", "Reset arrangement", "Keep every cutout. Each goes back to its source position at 100%, shown.", "1"),
            (ResetChoice::StartOver, "trash-2", "Start over", "Delete every cutout in this layout. The picture shows untouched.", "2"),
        ];
        for (i, (c, icon, title, body, key)) in choices.iter().enumerate() {
            let r = (x + 16.0 * k, y + 62.0 * k + i as f32 * 84.0 * k, w - 32.0 * k, 74.0 * k);
            let hover = inside(&r, self.pointer.0, self.pointer.1);
            out.push(Draw::Fill { r, color: if hover { accent_fill() } else { glass() }, radius: 10.0 * k });
            let col = if *c == ResetChoice::StartOver && hover { danger() } else { fg() };
            out.push(Draw::Icon { name: icon, r: (r.0 + 16.0 * k, r.1 + 14.0 * k, 20.0 * k, 20.0 * k), color: col, stroke: 1.6 * k });
            out.push(Draw::Text { r: (r.0 + 48.0 * k, r.1 + 8.0 * k, r.2, 30.0 * k), text: title.to_string(), size: 15.0 * k, color: col, bold: true, align: Align::Left });
            out.push(Draw::Text { r: (r.0 + 48.0 * k, r.1 + 38.0 * k, r.2 - 60.0 * k, 26.0 * k), text: body.to_string(), size: 12.5 * k, color: fg_muted(), bold: false, align: Align::Left });
            let kw = measure(key, 11.0 * k, true) + 10.0 * k;
            out.push(Draw::Fill { r: (r.0 + r.2 - kw - 14.0 * k, r.1 + 12.0 * k, kw, 18.0 * k), color: keycap_fill(), radius: 4.0 * k });
            out.push(Draw::Text { r: (r.0 + r.2 - kw - 14.0 * k, r.1 + 12.0 * k, kw, 18.0 * k), text: key.to_string(), size: 11.0 * k, color: fg(), bold: true, align: Align::Center });
            self.reset_hits.push((r, *c));
        }
        let cancel = (x + w - 150.0 * k, y + h - 44.0 * k, 134.0 * k, 32.0 * k);
        let hover = inside(&cancel, self.pointer.0, self.pointer.1);
        out.push(Draw::Fill { r: cancel, color: if hover { accent_fill() } else { glass() }, radius: 8.0 * k });
        out.push(Draw::Text { r: (cancel.0 + 14.0 * k, cancel.1, 70.0 * k, cancel.3), text: "Cancel".into(), size: 13.0 * k, color: fg(), bold: false, align: Align::Left });
        let kw = measure("Esc", 11.0 * k, true) + 10.0 * k;
        out.push(Draw::Fill { r: (cancel.0 + cancel.2 - kw - 10.0 * k, cancel.1 + 7.0 * k, kw, 18.0 * k), color: keycap_fill(), radius: 4.0 * k });
        out.push(Draw::Text { r: (cancel.0 + cancel.2 - kw - 10.0 * k, cancel.1 + 7.0 * k, kw, 18.0 * k), text: "Esc".into(), size: 11.0 * k, color: fg(), bold: true, align: Align::Center });
        self.reset_hits.push((cancel, ResetChoice::Cancel));
    }

    /// The ring that fills around the Reset button while Ctrl+Shift+Backspace is held.
    pub(super) fn draw_reset_ring(&self, out: &mut Vec<Draw>, geo: &Geo) {
        let Some(p) = self.reset_hold() else { return };
        let Some((r, _, _)) = self.buttons.iter().find(|b| b.1 == Action::Reset) else { return };
        let k = geo.scale;
        let (cx, cy) = (r.0 + 10.0 * k + 9.0 * k, r.1 + r.3 / 2.0);
        let rad = 15.0 * k;
        let steps = 48;
        let n = (p * steps as f32) as usize;
        for i in 0..n {
            let a0 = -std::f32::consts::FRAC_PI_2 + i as f32 / steps as f32 * std::f32::consts::TAU;
            let a1 = -std::f32::consts::FRAC_PI_2 + (i + 1) as f32 / steps as f32 * std::f32::consts::TAU;
            out.push(Draw::Line { a: (cx + rad * a0.cos(), cy + rad * a0.sin()), b: (cx + rad * a1.cos(), cy + rad * a1.sin()), color: danger(), width: 3.0 * k });
        }
    }

    /// The history list under the Undo button: every step, newest first.
    pub(super) fn draw_history(&mut self, out: &mut Vec<Draw>, geo: &Geo) {
        self.history_hits.clear();
        if !self.history_open {
            return;
        }
        let k = geo.scale;
        let Some((ur, _, _)) = self.buttons.iter().find(|b| b.1 == Action::Undo).cloned() else { return };
        let row_h = 32.0 * k;
        let names = self.history_list.clone();
        let shown = names.len().min(14);
        let w = names.iter().take(shown).map(|n| measure(n, 13.0 * k, false)).fold(220.0 * k, f32::max) + 60.0 * k;
        let h = 36.0 * k + (shown.max(1) as f32) * row_h + 8.0 * k;
        let (x, y) = (ur.0, ur.1 + ur.3 + 8.0 * k);
        out.push(Draw::Fill { r: (x, y, w, h), color: glass_hover(), radius: 12.0 * k });
        out.push(Draw::Stroke { r: (x, y, w, h), color: hairline(), width: 1.0, radius: 12.0 * k, dashed: false });
        out.push(Draw::Text { r: (x + 14.0 * k, y + 4.0 * k, w, 28.0 * k), text: "History · click a step to go back to before it".into(), size: 12.0 * k, color: fg_muted(), bold: true, align: Align::Left });
        if names.is_empty() {
            out.push(Draw::Text { r: (x + 14.0 * k, y + 36.0 * k, w, row_h), text: "Nothing to undo yet".into(), size: 13.0 * k, color: fg_dim(), bold: false, align: Align::Left });
            return;
        }
        for (i, name) in names.iter().take(shown).enumerate() {
            let r = (x + 6.0 * k, y + 36.0 * k + i as f32 * row_h, w - 12.0 * k, row_h - 2.0 * k);
            if inside(&r, self.pointer.0, self.pointer.1) {
                out.push(Draw::Fill { r, color: accent_fill(), radius: 6.0 * k });
            }
            out.push(Draw::Icon { name: "undo-2", r: (r.0 + 8.0 * k, r.1 + 8.0 * k, 14.0 * k, 14.0 * k), color: fg_muted(), stroke: 1.4 * k });
            out.push(Draw::Text { r: (r.0 + 30.0 * k, r.1, r.2, r.3), text: name.clone(), size: 13.0 * k, color: fg(), bold: false, align: Align::Left });
            self.history_hits.push((r, i));
        }
    }
}
