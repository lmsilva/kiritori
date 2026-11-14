//! The shortcut map (F1 or ?): a keyboard drawn over the layer with every key Kiritori uses lit
//! and coloured by group — global white, views cyan, cutouts amber, arrange lime, history
//! magenta. Hovering a key names its action; holding Ctrl or Shift redraws the map for that
//! modifier.

use super::style::*;
use super::{Geo, Layer};
use crate::engine::draw::{Align, Draw, Px, Rgba};
use crate::engine::render::measure;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Global,
    View,
    Cutout,
    Arrange,
    History,
}

impl Group {
    pub fn color(self) -> Rgba {
        // Darker shades in the light theme, so keys and dots stand out on white.
        let light = super::style::light();
        match self {
            Group::Global if light => [0.2, 0.2, 0.24, 1.0],
            Group::Global => [0.95, 0.95, 0.97, 1.0],
            Group::View if light => [0.02, 0.5, 0.64, 1.0],
            Group::View => [0.13, 0.83, 0.93, 1.0],
            Group::Cutout if light => [0.78, 0.5, 0.0, 1.0],
            Group::Cutout => [0.98, 0.75, 0.14, 1.0],
            Group::Arrange if light => [0.3, 0.56, 0.05, 1.0],
            Group::Arrange => [0.55, 0.85, 0.15, 1.0],
            Group::History if light => [0.78, 0.18, 0.5, 1.0],
            Group::History => [0.93, 0.33, 0.66, 1.0],
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Group::Global => "Global",
            Group::View => "Views",
            Group::Cutout => "Cutouts",
            Group::Arrange => "Arrange",
            Group::History => "History",
        }
    }
}

/// What a key does with the given modifier held, if anything.
pub fn binding(key: &str, ctrl: bool, shift: bool) -> Option<(Group, &'static str, Option<&'static str>)> {
    use Group::*;
    let b = |g, label, icon| Some((g, label, icon));
    match (ctrl, shift, key) {
        (false, false, "Tab") => b(View, "Switch Cut / Arrange", Some("layout-dashboard")),
        (false, false, "Esc") => b(View, "Cancel, deselect, close", Some("x")),
        (false, false, "Space") => b(View, "Peek (hold)", Some("eye")),
        (false, false, "F1") => b(View, "Shortcut map", Some("keyboard")),
        (false, false, "F5") => b(View, "Retake still frame (Cut)", Some("refresh-cw")),
        (false, false, "L") => b(View, "Cutout list", Some("panel-left")),
        (false, false, "P") => b(View, "Profiles", Some("app-window")),
        (false, false, "M") => b(View, "Display map", Some("monitor")),
        (false, false, "V") => b(View, "Edit / Live (Arrange)", Some("video")),
        (false, false, "G") => b(View, "Snapping", Some("magnet")),
        (false, false, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9") => b(Cutout, "Select cutout by number", None),
        (false, false, "Enter" | "F2") => b(Cutout, "Rename", Some("pencil")),
        (false, false, "H") => b(Cutout, "Hide / show", Some("eye-off")),
        (false, false, "K") => b(Cutout, "Lock / unlock", Some("lock")),
        (false, false, "Del") => b(Cutout, "Delete cutout", Some("trash-2")),
        (false, false, "S") => b(Cutout, "Suggest cutouts (Cut)", Some("sparkles")),
        (false, false, "T") => b(Cutout, "Trim black edges (Cut)", Some("crop")),
        (false, false, "F") => b(Arrange, "Fill / restore", Some("maximize-2")),
        (false, false, "A") => b(Arrange, "Auto-arrange", Some("wand-sparkles")),
        (false, false, "R") => b(Arrange, "Reset placement", Some("undo-dot")),
        (false, false, "Q") => b(Arrange, "Rotate left", Some("rotate-ccw")),
        (false, false, "E") => b(Arrange, "Rotate right", Some("rotate-cw")),
        (false, false, "D") => b(Arrange, "Send to next display", Some("send")),
        (false, false, "[") => b(Arrange, "Send backward", None),
        (false, false, "]") => b(Arrange, "Bring forward", None),
        (false, false, "←" | "→" | "↑" | "↓") => b(Arrange, "Move 1 px", None),
        (false, true, "←" | "→" | "↑" | "↓") => b(Arrange, "Move 10 px", None),
        (false, true, "/") => b(View, "Shortcut map (?)", Some("keyboard")),
        (true, false, "Z") => b(History, "Undo", Some("undo-2")),
        (true, false, "Y") => b(History, "Redo", Some("redo-2")),
        (true, true, "Z") => b(History, "Redo", Some("redo-2")),
        (true, true, "Bksp") => b(History, "Reset… (hold)", Some("rotate-ccw")),
        (true, false, "D") => b(Cutout, "Duplicate cutout", Some("copy")),
        (true, false, "←" | "→" | "↑" | "↓") => b(Cutout, "Resize 1 px", None),
        (true, false, ",") => b(View, "Settings", Some("settings")),
        _ => None,
    }
}

/// The global shortcuts, shown in their own row.
pub const GLOBAL: [(&str, &str); 6] = [
    ("Ctrl+Alt+K", "Show or hide the layer"),
    ("Ctrl+Alt+B", "Show original"),
    ("Ctrl+Alt+L", "Next layout"),
    ("Ctrl+Alt+1–9", "Hide or show a cutout"),
    ("Ctrl+Alt+Shift+K", "Emergency off"),
    ("Ctrl+Alt+Shift+Q", "Quit Kiritori"),
];

/// Rows of the keyboard: (label, width in key units).
/// Rows of the keyboard: (label, width in key units); an empty label is a gap. The main block
/// is 15 units wide; the arrow keys sit in their own cluster to the right, as on a real one.
fn rows() -> Vec<Vec<(&'static str, f32)>> {
    vec![
        vec![("Esc", 1.0), ("", 1.0), ("F1", 1.0), ("F2", 1.0), ("F3", 1.0), ("F4", 1.0), ("", 0.5), ("F5", 1.0), ("F6", 1.0), ("F7", 1.0), ("F8", 1.0), ("", 0.5), ("F9", 1.0), ("F10", 1.0), ("F11", 1.0), ("F12", 1.0)],
        vec![("`", 1.0), ("1", 1.0), ("2", 1.0), ("3", 1.0), ("4", 1.0), ("5", 1.0), ("6", 1.0), ("7", 1.0), ("8", 1.0), ("9", 1.0), ("0", 1.0), ("-", 1.0), ("=", 1.0), ("Bksp", 2.0)],
        vec![("Tab", 1.5), ("Q", 1.0), ("W", 1.0), ("E", 1.0), ("R", 1.0), ("T", 1.0), ("Y", 1.0), ("U", 1.0), ("I", 1.0), ("O", 1.0), ("P", 1.0), ("[", 1.0), ("]", 1.0), ("\\", 1.5)],
        vec![("Caps", 1.75), ("A", 1.0), ("S", 1.0), ("D", 1.0), ("F", 1.0), ("G", 1.0), ("H", 1.0), ("J", 1.0), ("K", 1.0), ("L", 1.0), (";", 1.0), ("'", 1.0), ("Enter", 2.25)],
        vec![("Shift", 2.25), ("Z", 1.0), ("X", 1.0), ("C", 1.0), ("V", 1.0), ("B", 1.0), ("N", 1.0), ("M", 1.0), (",", 1.0), (".", 1.0), ("/", 1.0), ("Shift", 2.75), ("", 1.5), ("↑", 1.0)],
        vec![("Ctrl", 1.5), ("Win", 1.25), ("Alt", 1.25), ("Space", 6.5), ("Alt", 1.25), ("Del", 1.25), ("Ctrl", 2.0), ("", 0.5), ("←", 1.0), ("↓", 1.0), ("→", 1.0)],
    ]
}

/// The keyboard's width in key units (the widest row).
fn units_wide() -> f32 {
    rows().iter().map(|r| r.iter().map(|(_, u)| u).sum::<f32>()).fold(0.0, f32::max)
}

impl Layer {
    pub(super) fn draw_keymap(&mut self, out: &mut Vec<Draw>, geo: &Geo) {
        self.keymap_hits.clear();
        if !self.keymap_open {
            return;
        }
        let k = geo.scale;
        let m = crate::shell::canvas::mods();
        let (ctrl, shift) = (m.ctrl, m.shift);
        let gap = 5.0 * k;
        // Keys as large as fit the display, at most 52 px at 100%.
        let units = units_wide();
        let unit = ((geo.canvas.w as f32 - 2.0 * 24.0 * k - 64.0 * k) / units - gap).min(52.0 * k);
        let kb_w = units * (unit + gap) - gap;
        let pad = 24.0 * k;
        let global_h = 92.0 * k;
        let legend_h = 64.0 * k;
        let w = kb_w + 2.0 * pad;
        let h = pad + 30.0 * k + global_h + 6.0 * (unit + gap) + legend_h + pad;
        let x0 = (geo.canvas.w as f32 - w) / 2.0;
        let y0 = ((geo.canvas.h as f32 - h) / 2.0).max(70.0 * k);
        let mut bg = glass();
        bg[3] = 0.96;
        out.push(Draw::Fill { r: (x0, y0, w, h), color: bg, radius: 16.0 * k });
        out.push(Draw::Stroke { r: (x0, y0, w, h), color: hairline(), width: 1.0, radius: 16.0 * k, dashed: false });
        let held = match (ctrl, shift) {
            (true, true) => " · with Ctrl+Shift",
            (true, false) => " · with Ctrl",
            (false, true) => " · with Shift",
            _ => " · hold Ctrl or Shift to see more",
        };
        out.push(Draw::Text { r: (x0 + pad, y0 + pad - 6.0 * k, w - 2.0 * pad, 30.0 * k), text: format!("Shortcuts{held}"), size: 18.0 * k, color: fg(), bold: true, align: Align::Left });
        out.push(Draw::Text { r: (x0 + pad, y0 + pad - 6.0 * k, w - 2.0 * pad, 30.0 * k), text: "F1 or Esc to close".into(), size: 12.0 * k, color: fg_muted(), bold: false, align: Align::Right });

        // Global shortcuts: they work at any time, even over a full-screen game.
        let gy = y0 + pad + 32.0 * k;
        let gw = (w - 2.0 * pad - 5.0 * gap) / 6.0;
        for (i, (keys, what)) in GLOBAL.iter().enumerate() {
            let r = (x0 + pad + i as f32 * (gw + gap), gy, gw, global_h - 12.0 * k);
            out.push(Draw::Fill { r, color: keycap_fill(), radius: 8.0 * k });
            out.push(Draw::Stroke { r, color: Group::Global.color(), width: 1.0, radius: 8.0 * k, dashed: false });
            out.push(Draw::Text { r: (r.0, r.1 + 8.0 * k, r.2, 26.0 * k), text: keys.to_string(), size: 13.0 * k, color: fg(), bold: true, align: Align::Center });
            out.push(Draw::Text { r: (r.0, r.1 + 38.0 * k, r.2, 24.0 * k), text: what.to_string(), size: 12.0 * k, color: fg_muted(), bold: false, align: Align::Center });
        }

        // The keyboard.
        let ky = gy + global_h;
        let mut hovered: Option<(Group, &'static str)> = None;
        for (ri, row) in rows().iter().enumerate() {
            let mut x = x0 + pad;
            let y = ky + ri as f32 * (unit + gap);
            for (label, units) in row {
                let kw = units * (unit + gap) - gap;
                if label.is_empty() {
                    x += kw + gap;
                    continue;
                }
                let r: Px = (x, y, kw, unit);
                let held_key = (*label == "Ctrl" && ctrl) || (*label == "Shift" && shift);
                let b = binding(label, ctrl, shift);
                match b {
                    Some((g, _, _)) => {
                        let mut f = g.color();
                        f[3] = 0.22;
                        out.push(Draw::Fill { r, color: f, radius: 6.0 * k });
                        out.push(Draw::Stroke { r, color: g.color(), width: 1.5 * k, radius: 6.0 * k, dashed: false });
                    }
                    None => {
                        out.push(Draw::Fill { r, color: if held_key { accent_fill() } else { keycap_fill() }, radius: 6.0 * k });
                    }
                }
                let lit = b.is_some();
                out.push(Draw::Text { r: (r.0 + 6.0 * k, r.1 + 4.0 * k, r.2 - 8.0 * k, 18.0 * k), text: label.to_string(), size: 12.0 * k, color: if lit || held_key { fg() } else { fg_dim() }, bold: lit, align: Align::Left });
                if let Some((g, what, icon)) = b {
                    if let Some(i) = icon {
                        let is = 18.0 * k;
                        out.push(Draw::Icon { name: i, r: (r.0 + r.2 - is - 6.0 * k, r.1 + r.3 - is - 6.0 * k, is, is), color: g.color(), stroke: 1.6 * k });
                    }
                    let (px, py) = self.pointer;
                    if px >= r.0 && px < r.0 + r.2 && py >= r.1 && py < r.1 + r.3 {
                        hovered = Some((g, what));
                        out.push(Draw::Stroke { r, color: fg(), width: 2.0 * k, radius: 6.0 * k, dashed: false });
                    }
                    self.keymap_hits.push((r, what));
                }
                x += kw + gap;
            }
        }

        // Legend, or what the key under the pointer does.
        let ly = ky + 6.0 * (unit + gap) + 10.0 * k;
        match hovered {
            Some((g, what)) => {
                out.push(Draw::Fill { r: (x0 + pad, ly + 8.0 * k, 14.0 * k, 14.0 * k), color: g.color(), radius: 7.0 * k });
                out.push(Draw::Text { r: (x0 + pad + 22.0 * k, ly, w - 2.0 * pad, 30.0 * k), text: format!("{} · {what}", g.label()), size: 15.0 * k, color: fg(), bold: true, align: Align::Left });
            }
            None => {
                let mut x = x0 + pad;
                for g in [Group::Global, Group::View, Group::Cutout, Group::Arrange, Group::History] {
                    out.push(Draw::Fill { r: (x, ly + 8.0 * k, 14.0 * k, 14.0 * k), color: g.color(), radius: 7.0 * k });
                    let tw = measure(g.label(), 13.0 * k, false);
                    out.push(Draw::Text { r: (x + 20.0 * k, ly, tw + 4.0, 30.0 * k), text: g.label().into(), size: 13.0 * k, color: fg(), bold: false, align: Align::Left });
                    x += tw + 48.0 * k;
                }
                out.push(Draw::Text { r: (x, ly, x0 + w - pad - x, 30.0 * k), text: "Point at a key to see what it does".into(), size: 13.0 * k, color: fg_muted(), bold: false, align: Align::Right });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_documented_key_is_on_the_map() {
        // Spec, "Keyboard shortcuts": each of these keys must light up.
        for key in ["Tab", "Esc", "1", "Enter", "F2", "H", "K", "Del", "G", "L", "P", "F1", "Space", "S", "T", "F", "A", "R", "Q", "E", "D", "M", "[", "]", "←"] {
            assert!(binding(key, false, false).is_some(), "{key} is not on the map");
            assert!(rows().iter().flatten().any(|(l, _)| *l == key), "{key} has no key on the keyboard");
        }
        assert_eq!(binding("Z", true, false).map(|b| b.1), Some("Undo"));
        assert_eq!(binding("Z", true, true).map(|b| b.1), Some("Redo"));
        assert!(binding("Z", false, false).is_none());
    }
}
