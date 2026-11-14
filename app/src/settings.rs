//! The Settings panel (Ctrl+, in the layer, or the tray): theme, start with Windows, the
//! auto-apply toast, the performance readout, the global shortcuts with a second binding each,
//! language, and the self-test.
//!
//! It shares the Profiles panel's window. It keeps only what it shows and the shortcut being
//! recorded; every change comes back as an [`Op`].

use crate::engine::draw::{Align, Draw, Px, Rgba};
use crate::engine::render::measure;
use crate::layer::style::*;
use crate::model::shortcut::{Bindings, Combo, Global};
use crate::shell::canvas::{Button, Input};
use crate::store::Theme;

#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    None,
    Close,
    Theme(Theme),
    StartWithWindows(bool),
    Toast(bool),
    Performance(bool),
    /// Record a new shortcut for this action and slot (hotkeys are paused meanwhile).
    StartRecording(Global, usize),
    StopRecording,
    /// Set (or clear, with None) an action's first or second shortcut.
    Shortcut(Global, usize, Option<Combo>),
    ResetShortcuts,
    About,
}

#[derive(Clone, Debug, PartialEq)]
enum Hit {
    Close,
    Theme(Theme),
    StartWithWindows,
    Toast,
    Performance,
    Slot(Global, usize),
    ResetShortcuts,
    About,
}

/// What the panel shows, from the app.
pub struct View<'a> {
    pub theme: Theme,
    pub start_with_windows: bool,
    pub toast: bool,
    pub performance: bool,
    pub bindings: &'a Bindings,
    /// Bindings Windows refused (another program holds them).
    pub taken: &'a [(Global, usize)],
    /// A shortcut that types a character on this keyboard (AltGr), with the character.
    pub types: &'a dyn Fn(Combo) -> Option<char>,
    pub self_test: &'a str,
}

#[derive(Default)]
pub struct SettingsPanel {
    hits: Vec<(Px, Hit)>,
    hover: Option<Hit>,
    /// The slot waiting for a new shortcut.
    pub recording: Option<(Global, usize)>,
    /// A line at the bottom: what just happened, or why a shortcut was refused.
    pub status: Option<String>,
}

fn inside(r: &Px, x: f32, y: f32) -> bool {
    x >= r.0 && x < r.0 + r.2 && y >= r.1 && y < r.1 + r.3
}

impl SettingsPanel {
    pub fn input(&mut self, input: Input, v: &View) -> Op {
        match input {
            Input::Move(x, y) => {
                self.hover = self.hits.iter().find(|(r, _)| inside(r, x, y)).map(|(_, h)| h.clone());
                Op::None
            }
            Input::Down(Button::Left, x, y, _) => {
                let hit = self.hits.iter().find(|(r, _)| inside(r, x, y)).map(|(_, h)| h.clone());
                if self.recording.is_some() {
                    self.recording = None;
                    return match hit {
                        Some(Hit::Slot(g, i)) => {
                            self.recording = Some((g, i));
                            Op::StartRecording(g, i)
                        }
                        _ => Op::StopRecording,
                    };
                }
                match hit {
                    None => Op::None,
                    Some(Hit::Close) => Op::Close,
                    Some(Hit::Theme(t)) => Op::Theme(t),
                    Some(Hit::StartWithWindows) => Op::StartWithWindows(!v.start_with_windows),
                    Some(Hit::Toast) => Op::Toast(!v.toast),
                    Some(Hit::Performance) => Op::Performance(!v.performance),
                    Some(Hit::Slot(g, i)) => {
                        self.recording = Some((g, i));
                        self.status = Some("Press the new shortcut · Esc cancels · Backspace clears".into());
                        Op::StartRecording(g, i)
                    }
                    Some(Hit::ResetShortcuts) => Op::ResetShortcuts,
                    Some(Hit::About) => Op::About,
                }
            }
            Input::Key(vk, m) => {
                let Some((g, slot)) = self.recording else {
                    return if vk == 0x1B { Op::Close } else { Op::None };
                };
                match vk {
                    0x1B => {
                        self.recording = None;
                        self.status = None;
                        Op::StopRecording
                    }
                    0x08 if !m.ctrl && !m.alt => {
                        self.recording = None;
                        self.status = Some(if slot == 0 { "Back to the default".into() } else { "Second shortcut cleared".into() });
                        Op::Shortcut(g, slot, if slot == 0 { Some(g.default_combo()) } else { None })
                    }
                    // Modifiers on their own: keep waiting for the key.
                    0x10 | 0x11 | 0x12 | 0x5B | 0x5C | 0xA0..=0xA5 => Op::None,
                    _ => {
                        let mut c = Combo::new(m.ctrl, m.alt, m.shift, vk as u32);
                        if g == Global::Cutouts {
                            // The cutout row is a set of modifiers for the digits 1–9.
                            c.vk = '1' as u32;
                        }
                        if !c.usable() {
                            self.status = Some(format!("{c} needs Ctrl or Alt, so it does not take a key from other programs"));
                            return Op::None;
                        }
                        if let Some(other) = v.bindings.clash(c, (g, slot)) {
                            self.status = Some(format!("{c} is already used for \"{}\"", other.label()));
                            return Op::None;
                        }
                        self.recording = None;
                        self.status = match (v.types)(c) {
                            Some(ch) => Some(format!("Note: {c} types \"{ch}\" on this keyboard (AltGr), so that character cannot be typed while Kiritori runs")),
                            None => Some(format!("{}: {c}", g.label())),
                        };
                        Op::Shortcut(g, slot, Some(c))
                    }
                }
            }
            Input::LostFocus => {
                if self.recording.take().is_some() {
                    return Op::StopRecording;
                }
                Op::None
            }
            _ => Op::None,
        }
    }

    pub fn draw(&mut self, v: &View, k: f32, w: f32, h: f32) -> Vec<Draw> {
        let s = |x: f32| x * k;
        self.hits.clear();
        let mut d = Vec::new();
        let mut bg = glass();
        bg[3] = 1.0;
        d.push(Draw::Fill { r: (0.0, 0.0, w, h), color: bg, radius: 0.0 });
        d.push(Draw::Stroke { r: (0.5, 0.5, w - 1.0, h - 1.0), color: hairline(), width: 1.0, radius: 0.0, dashed: false });
        d.push(Draw::Text { r: (s(PAD), s(18.0), s(400.0), s(32.0)), text: "Settings".into(), size: s(20.0), color: fg(), bold: true, align: Align::Left });
        d.push(Draw::Text { r: (w - s(PAD + 52.0 + 240.0), s(18.0), s(240.0), s(32.0)), text: "Esc to close".into(), size: s(12.0), color: fg_muted(), bold: false, align: Align::Right });
        let close = (w - s(PAD + 36.0), s(16.0), s(36.0), s(36.0));
        self.hover_bg(&mut d, close, &Hit::Close, s(8.0));
        d.push(Draw::Icon { name: "x", r: (close.0 + s(8.0), close.1 + s(8.0), s(20.0), s(20.0)), color: fg(), stroke: s(2.0) });
        self.hits.push((close, Hit::Close));

        // One grid: a label column, then every control starting at the same place.
        let x0 = s(PAD);
        let cx = x0 + s(LABEL_W);
        let right = w - s(PAD);
        let row_h = s(ROW);
        let mut y = s(68.0);
        let heading = |d: &mut Vec<Draw>, y: f32, t: &str| {
            d.push(Draw::Text { r: (x0, y, right - x0, s(20.0)), text: t.to_uppercase(), size: s(11.0), color: fg_muted(), bold: true, align: Align::Left });
            d.push(Draw::Line { a: (x0, y + s(24.0)), b: (right, y + s(24.0)), color: hairline(), width: 1.0 });
        };
        let row_label = |d: &mut Vec<Draw>, y: f32, t: &str| {
            d.push(Draw::Text { r: (x0, y, s(LABEL_W - 16.0), row_h), text: t.into(), size: s(13.0), color: fg(), bold: false, align: Align::Left });
        };

        heading(&mut d, y, "Appearance");
        y += s(34.0);
        row_label(&mut d, y, "Theme");
        let mut x = cx;
        for (t, text) in [(Theme::System, "Follow Windows"), (Theme::Light, "Light"), (Theme::Dark, "Dark")] {
            let on = v.theme == t;
            let bw = measure(text, s(13.0), true) + s(28.0);
            let r = (x, y + (row_h - s(32.0)) / 2.0, bw, s(32.0));
            if on {
                d.push(Draw::Fill { r, color: accent_fill(), radius: s(8.0) });
                d.push(Draw::Stroke { r, color: accent(), width: s(1.5), radius: s(8.0), dashed: false });
            } else {
                self.hover_bg(&mut d, r, &Hit::Theme(t), s(8.0));
                d.push(Draw::Stroke { r, color: hairline(), width: 1.0, radius: s(8.0), dashed: false });
            }
            d.push(Draw::Text { r, text: text.into(), size: s(13.0), color: fg(), bold: on, align: Align::Center });
            self.hits.push((r, Hit::Theme(t)));
            x += bw + s(8.0);
        }
        y += row_h + s(14.0);

        heading(&mut d, y, "Behaviour");
        y += s(34.0);
        for (i, (hit, on, label, text)) in [
            (Hit::StartWithWindows, v.start_with_windows, "Start with Windows", "Start Kiritori when you sign in (the only setting that touches the system)"),
            (Hit::Toast, v.toast, "Applied note", "Show a short note when a layout is applied"),
            (Hit::Performance, v.performance, "Performance", "Show capture rate, delay and GPU time in the layer"),
        ]
        .into_iter()
        .enumerate()
        {
            let ry = y + i as f32 * row_h;
            row_label(&mut d, ry, label);
            self.checkbox(&mut d, (cx, ry), row_h, on, text, hit, k);
        }
        y += 3.0 * row_h + s(14.0);

        heading(&mut d, y, "Global shortcuts · they work at any time, even over a full-screen game");
        let reset = "Reset to defaults";
        let rw = measure(reset, s(12.0), false) + s(24.0);
        let rr = (right - rw, y - s(4.0), rw, s(26.0));
        self.hover_bg(&mut d, rr, &Hit::ResetShortcuts, s(6.0));
        d.push(Draw::Text { r: rr, text: reset.into(), size: s(12.0), color: accent(), bold: false, align: Align::Center });
        self.hits.push((rr, Hit::ResetShortcuts));
        y += s(32.0);
        let slot_w = s(SLOT_W);
        let gap = s(12.0);
        let note_x = cx + 2.0 * (slot_w + gap);
        for (i, t) in ["Shortcut", "Second shortcut"].iter().enumerate() {
            d.push(Draw::Text { r: (cx + i as f32 * (slot_w + gap), y, slot_w, s(18.0)), text: t.to_string(), size: s(11.0), color: fg_muted(), bold: false, align: Align::Center });
        }
        y += s(22.0);
        for g in Global::ALL {
            row_label(&mut d, y, g.label());
            let slots = v.bindings.get(g);
            for (i, combo) in slots.iter().enumerate() {
                let r = (cx + i as f32 * (slot_w + gap), y + (row_h - s(32.0)) / 2.0, slot_w, s(32.0));
                let rec = self.recording == Some((g, i));
                let taken = v.taken.contains(&(g, i));
                if rec {
                    d.push(Draw::Fill { r, color: accent_fill(), radius: s(6.0) });
                    d.push(Draw::Stroke { r, color: accent(), width: s(1.5), radius: s(6.0), dashed: false });
                } else {
                    self.hover_bg(&mut d, r, &Hit::Slot(g, i), s(6.0));
                    d.push(Draw::Stroke { r, color: if taken { danger() } else { hairline() }, width: if taken { s(1.5) } else { 1.0 }, radius: s(6.0), dashed: combo.is_none() });
                }
                let text = if rec {
                    "Press keys…".to_string()
                } else {
                    match combo {
                        Some(c) if g == Global::Cutouts => format!("{}1–9", c.to_string().trim_end_matches('1')),
                        Some(c) => c.to_string(),
                        None => "+ Add".to_string(),
                    }
                };
                let color = if taken { danger() } else if combo.is_none() && !rec { fg_muted() } else { fg() };
                d.push(Draw::Text { r, text, size: s(13.0), color, bold: combo.is_some() && !rec, align: Align::Center });
                self.hits.push((r, Hit::Slot(g, i)));
            }
            let note = if slots.iter().enumerate().any(|(i, _)| v.taken.contains(&(g, i))) {
                Some(("Taken by another program".to_string(), danger()))
            } else {
                slots.iter().flatten().find_map(|c| (v.types)(*c).map(|ch| (format!("Types \"{ch}\" on this keyboard"), WARN)))
            };
            if let Some((text, color)) = note {
                d.push(Draw::Text { r: (note_x, y, right - note_x, row_h), text, size: s(12.0), color, bold: false, align: Align::Left });
            }
            y += row_h;
        }
        y += s(14.0);

        heading(&mut d, y, "About");
        y += s(34.0);
        row_label(&mut d, y, "Language");
        d.push(Draw::Text { r: (cx, y, right - cx, row_h), text: "English · more languages will follow".into(), size: s(13.0), color: fg_muted(), bold: false, align: Align::Left });
        y += row_h;
        row_label(&mut d, y, "This PC");
        let problems = v.self_test.lines().filter(|l| l.starts_with('✗')).count();
        let summary = if v.self_test.is_empty() {
            "Self-test still running".to_string()
        } else if problems == 0 {
            "Self-test: everything works".to_string()
        } else {
            format!("Self-test: {problems} problem{}", if problems == 1 { "" } else { "s" })
        };
        let sw = measure(&summary, s(13.0), false);
        d.push(Draw::Text { r: (cx, y, sw + 4.0, row_h), text: summary, size: s(13.0), color: if problems == 0 { fg_muted() } else { danger() }, bold: false, align: Align::Left });
        let ab = "Details…";
        let aw = measure(ab, s(13.0), false) + s(28.0);
        let ar = (cx + sw + s(16.0), y + (row_h - s(32.0)) / 2.0, aw, s(32.0));
        self.hover_bg(&mut d, ar, &Hit::About, s(8.0));
        d.push(Draw::Stroke { r: ar, color: hairline(), width: 1.0, radius: s(8.0), dashed: false });
        d.push(Draw::Text { r: ar, text: ab.into(), size: s(13.0), color: fg(), bold: false, align: Align::Center });
        self.hits.push((ar, Hit::About));

        // The status line sits in its own band at the bottom.
        d.push(Draw::Line { a: (x0, h - s(44.0)), b: (right, h - s(44.0)), color: hairline(), width: 1.0 });
        let status = self.status.clone().unwrap_or_else(|| "Click a shortcut to change it.".into());
        d.push(Draw::Text { r: (x0, h - s(40.0), right - x0, s(32.0)), text: status, size: s(12.0), color: fg_muted(), bold: false, align: Align::Left });
        d
    }

    fn hover_bg(&self, d: &mut Vec<Draw>, r: Px, hit: &Hit, radius: f32) {
        if self.hover.as_ref() == Some(hit) {
            d.push(Draw::Fill { r, color: glass_hover(), radius });
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn checkbox(&mut self, d: &mut Vec<Draw>, at: (f32, f32), row_h: f32, on: bool, text: &str, hit: Hit, k: f32) {
        let s = |x: f32| x * k;
        let cb = (at.0, at.1 + (row_h - s(20.0)) / 2.0, s(20.0), s(20.0));
        if on {
            d.push(Draw::Fill { r: cb, color: accent(), radius: s(5.0) });
            d.push(Draw::Icon { name: "check", r: (cb.0 + s(3.0), cb.1 + s(3.0), s(14.0), s(14.0)), color: ON_COLOR, stroke: s(2.5) });
        } else {
            d.push(Draw::Stroke { r: cb, color: fg_muted(), width: s(1.5), radius: s(5.0), dashed: false });
        }
        let tw = measure(text, s(13.0), false);
        d.push(Draw::Text { r: (at.0 + s(32.0), at.1, tw + 4.0, row_h), text: text.into(), size: s(13.0), color: fg_muted(), bold: false, align: Align::Left });
        self.hits.push(((at.0 - s(4.0), at.1 + s(4.0), tw + s(40.0), row_h - s(8.0)), hit));
    }

}

/// The window's size in 96-dpi units.
pub const SIZE: (f32, f32) = (900.0, 800.0);
const PAD: f32 = 28.0;
const LABEL_W: f32 = 210.0;
const ROW: f32 = 40.0;
const SLOT_W: f32 = 180.0;

/// Colour of a warning line.
pub const WARN: Rgba = [0.98, 0.75, 0.14, 1.0];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::canvas::Mods;
    use std::collections::BTreeMap;

    fn view<'a>(b: &'a Bindings, types: &'a dyn Fn(Combo) -> Option<char>) -> View<'a> {
        View { theme: Theme::System, start_with_windows: false, toast: true, performance: false, bindings: b, taken: &[], types, self_test: "✓ ok" }
    }

    fn click_slot(p: &mut SettingsPanel, v: &View, g: Global, i: usize) -> Op {
        let (r, _) = p.hits.iter().find(|(_, h)| *h == Hit::Slot(g, i)).unwrap().clone();
        p.input(Input::Down(Button::Left, r.0 + 2.0, r.1 + 2.0, Mods::default()), v)
    }

    #[test]
    fn recording_a_shortcut() {
        let b = Bindings::from_settings(&BTreeMap::new());
        let none = |_| None;
        let v = view(&b, &none);
        let mut p = SettingsPanel::default();
        p.draw(&v, 1.0, 940.0, 680.0);
        assert_eq!(click_slot(&mut p, &v, Global::Layer, 1), Op::StartRecording(Global::Layer, 1));
        let ctrl_alt = Mods { ctrl: true, alt: true, shift: false };
        // Modifiers alone keep waiting.
        assert_eq!(p.input(Input::Key(0x11, ctrl_alt), &v), Op::None);
        // A clash is refused and explained.
        assert_eq!(p.input(Input::Key('B' as u16, ctrl_alt), &v), Op::None);
        assert!(p.status.as_ref().unwrap().contains("Show original"));
        // A bare key is refused.
        assert_eq!(p.input(Input::Key('J' as u16, Mods::default()), &v), Op::None);
        assert_eq!(p.input(Input::Key('J' as u16, ctrl_alt), &v), Op::Shortcut(Global::Layer, 1, Combo::parse("Ctrl+Alt+J")));
        assert!(p.recording.is_none());
        // Backspace clears a second shortcut; Esc cancels.
        click_slot(&mut p, &v, Global::Layer, 1);
        assert_eq!(p.input(Input::Key(0x08, Mods::default()), &v), Op::Shortcut(Global::Layer, 1, None));
        click_slot(&mut p, &v, Global::Bypass, 0);
        assert_eq!(p.input(Input::Key(0x1B, Mods::default()), &v), Op::StopRecording);
    }

    #[test]
    fn altgr_characters_are_flagged() {
        let b = Bindings::from_settings(&BTreeMap::new());
        let euro = |c: Combo| (c.vk == 'E' as u32 && c.ctrl && c.alt).then_some('€');
        let v = view(&b, &euro);
        let mut p = SettingsPanel::default();
        p.draw(&v, 1.0, 940.0, 680.0);
        click_slot(&mut p, &v, Global::NextLayout, 0);
        let op = p.input(Input::Key('E' as u16, Mods { ctrl: true, alt: true, shift: false }), &v);
        assert_eq!(op, Op::Shortcut(Global::NextLayout, 0, Combo::parse("Ctrl+Alt+E")));
        assert!(p.status.as_ref().unwrap().contains('€'));
    }
}
