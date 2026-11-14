//! Several displays: which connected display each of a layout's
//! displays is, which cutouts are parked because their display is missing, and the layout
//! operations that move cutouts between displays.
//!
//! The layout's `main` display is always the one holding the source window (it hides the
//! original picture); every other display is found by its hardware identity, because Windows
//! renumbers displays.

use std::collections::BTreeMap;

use super::geometry::{Rect, Size, fit};
use super::layout::{DisplayMode, DisplayRef, Frame, Layout, MAIN_DISPLAY};

/// A connected display, as Windows reports it.
#[derive(Clone, Debug, PartialEq)]
pub struct Monitor {
    /// Device interface path: stable across reboots and renumbering.
    pub id: String,
    /// Windows' number (\\.\DISPLAY3 → 3), shown as "Display 3".
    pub number: u32,
    /// The monitor's own name when Windows knows it ("DELL U2720Q"), else "Display 3".
    pub name: String,
    /// The whole display in screen pixels: x, y, w, h.
    pub rect: (i32, i32, i32, i32),
    /// Pixels per 96-dpi unit.
    pub scale: f32,
    pub hz: u32,
}

impl Monitor {
    pub fn label(&self) -> String {
        format!("Display {}", self.number)
    }

    pub fn size(&self) -> Size {
        Size::new(self.rect.2 as f64, self.rect.3 as f64)
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.rect.0 && x < self.rect.0 + self.rect.2 && y >= self.rect.1 && y < self.rect.1 + self.rect.3
    }

    /// The display as a layout stores it; its label is the monitor's own name when known, for
    /// messages such as `Display "Dell U2720Q" not connected`.
    pub fn as_ref(&self, mode: DisplayMode) -> DisplayRef {
        DisplayRef { hardware_id: self.id.clone(), label: self.name.clone(), mode }
    }
}

/// One of the layout's displays, found among the connected ones.
#[derive(Clone, Debug, PartialEq)]
pub struct Placed {
    /// Key into the layout's `displays`.
    pub key: String,
    /// Index into the monitor list.
    pub monitor: usize,
    pub mode: DisplayMode,
}

/// Where each of the layout's displays is now. `source` is the monitor holding the app window.
/// Returns the displays found and the keys of those not connected.
pub fn resolve(layout: &Layout, monitors: &[Monitor], source: usize) -> (Vec<Placed>, Vec<String>) {
    let mut found: Vec<Placed> = Vec::new();
    let mut missing = Vec::new();
    for (key, d) in &layout.displays {
        let at = if key == MAIN_DISPLAY { Some(source).filter(|s| *s < monitors.len()) } else { monitors.iter().position(|m| m.id == d.hardware_id) };
        match at {
            // A display already in use by another key (for example the source moved onto it)
            // cannot hold two canvases.
            Some(i) if !found.iter().any(|p| p.monitor == i) && !(key != MAIN_DISPLAY && i == source) => found.push(Placed { key: key.clone(), monitor: i, mode: d.mode }),
            _ => missing.push(key.clone()),
        }
    }
    // Main first, so it is drawn and focused first.
    found.sort_by_key(|p| p.key != MAIN_DISPLAY);
    (found, missing)
}

/// "DELL U2720Q, Display 2" style key for a set of connected displays, used to remember the
/// last layout per set.
pub fn set_key(monitors: &[Monitor]) -> String {
    let mut ids: Vec<&str> = monitors.iter().map(|m| m.id.as_str()).collect();
    ids.sort();
    ids.join("|")
}

/// The frame placement maths needs for a display that does not hold the source: the source
/// shown 1:1 in the middle (used only by Reset placement there).
pub fn frame_for(m: &Monitor, source_px: Size) -> Frame {
    let c = m.size();
    let sw = (source_px.w / c.w).min(1.0);
    let sh = (source_px.h / c.h).min(1.0);
    Frame { source_px, canvas_px: c, source_on_canvas: Rect::new((1.0 - sw) / 2.0, (1.0 - sh) / 2.0, sw, sh) }
}

impl Layout {
    /// The key for a connected display, adding it to the layout (in `mode`) if it is new.
    pub fn display_key(&mut self, m: &Monitor, is_source: bool, mode: DisplayMode) -> String {
        if is_source {
            return MAIN_DISPLAY.into();
        }
        if let Some((k, _)) = self.displays.iter().find(|(k, d)| *k != MAIN_DISPLAY && d.hardware_id == m.id) {
            return k.clone();
        }
        let taken: Vec<String> = self.displays.keys().cloned().collect();
        let key = (m.number..).chain(1..).map(|n| format!("display-{n}")).find(|k| !taken.contains(k)).unwrap();
        self.displays.insert(key.clone(), m.as_ref(mode));
        key
    }

    /// Move a cutout to another display, centred on `at` (fractions of the new display), at the
    /// same size in pixels, shrunk to fit if needed.
    pub fn send_to(&mut self, n: u32, key: &str, from: Size, to: Size, at: Option<(f64, f64)>) -> bool {
        if !self.displays.contains_key(key) {
            return false;
        }
        let Some(c) = self.cutout_mut(n) else { return false };
        let px = c.placement.rect.scale(from);
        let fitted = fit(px.size(), Rect::new(0.0, 0.0, to.w, to.h));
        let (w, h) = if px.w <= to.w && px.h <= to.h { (px.w, px.h) } else { (fitted.w, fitted.h) };
        let (cx, cy) = at.map(|(x, y)| (x * to.w, y * to.h)).unwrap_or((to.w / 2.0, to.h / 2.0));
        let x = (cx - w / 2.0).clamp(0.0, (to.w - w).max(0.0));
        let y = (cy - h / 2.0).clamp(0.0, (to.h - h).max(0.0));
        c.placement.display = key.to_string();
        c.placement.rect = Rect::new(x, y, w, h).unscale(to);
        c.placement.before_fill = None;
        true
    }

    /// Duplicate a cutout (Ctrl+D): same shape, a new number and colour, placed a little
    /// down and to the right, on top. Returns the new number.
    pub fn duplicate(&mut self, n: u32) -> Option<u32> {
        let src = self.cutout(n)?.clone();
        let number = self.next_number();
        let color = self.next_color();
        let z = self.cutouts.iter().map(|c| c.placement.z).max().unwrap_or(0) + 1;
        let mut copy = src;
        copy.number = number;
        copy.color = color;
        copy.name = format!("{} copy", copy.name);
        copy.placement.z = z;
        copy.placement.before_fill = None;
        let r = copy.placement.rect;
        copy.placement.rect = Rect::new((r.x + 0.02).min(1.0 - r.w).max(0.0), (r.y + 0.02).min(1.0 - r.h).max(0.0), r.w, r.h);
        self.cutouts.push(copy);
        Some(number)
    }

    /// Set a display's mode. `None` is "Not in use": the display leaves the layout and its
    /// cutouts move to the main display, centred. The main display cannot be released.
    pub fn set_display_mode(&mut self, key: &str, mode: Option<DisplayMode>, sizes: &BTreeMap<String, Size>) -> bool {
        match mode {
            Some(m) => match self.displays.get_mut(key) {
                Some(d) if d.mode != m => {
                    d.mode = m;
                    true
                }
                _ => false,
            },
            None if key == MAIN_DISPLAY || !self.displays.contains_key(key) => false,
            None => {
                let from = sizes.get(key).copied().unwrap_or(Size::new(1920.0, 1080.0));
                let to = sizes.get(MAIN_DISPLAY).copied().unwrap_or(Size::new(1920.0, 1080.0));
                let on: Vec<u32> = self.cutouts.iter().filter(|c| c.placement.display == key).map(|c| c.number).collect();
                for n in on {
                    self.send_to(n, MAIN_DISPLAY, from, to, None);
                }
                self.displays.remove(key);
                true
            }
        }
    }

    /// Cutouts whose display is in `missing`: parked, not drawn.
    pub fn parked(&self, missing: &[String]) -> Vec<u32> {
        self.cutouts.iter().filter(|c| missing.contains(&c.placement.display)).map(|c| c.number).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mon(id: &str, number: u32, x: i32, w: i32, h: i32) -> Monitor {
        Monitor { id: id.into(), number, name: format!("Display {number}"), rect: (x, 0, w, h), scale: 1.0, hz: 60 }
    }

    fn layout() -> Layout {
        let mut l = Layout::new("l", "L", DisplayRef { hardware_id: "A".into(), label: "Display 1".into(), mode: DisplayMode::Takeover });
        let f = Frame { source_px: Size::new(1920.0, 1080.0), canvas_px: Size::new(2560.0, 1440.0), source_on_canvas: Rect::new(0.125, 0.125, 0.75, 0.75) };
        l.add_cutout(Rect::new(0.5, 0.0, 0.5, 0.5), &f);
        l.add_cutout(Rect::new(0.5, 0.5, 0.5, 0.5), &f);
        l
    }

    #[test]
    fn displays_are_found_by_hardware_id_and_main_follows_the_source() {
        let mut l = layout();
        let ms = vec![mon("A", 1, 0, 2560, 1440), mon("B", 2, 2560, 1920, 1080)];
        let key = l.display_key(&ms[1], false, DisplayMode::Takeover);
        assert_eq!(key, "display-2");
        assert_eq!(l.display_key(&ms[1], false, DisplayMode::Float), "display-2", "the same display keeps its key");
        let (found, missing) = resolve(&l, &ms, 0);
        assert_eq!(found.iter().map(|p| (p.key.as_str(), p.monitor)).collect::<Vec<_>>(), vec![("main", 0), ("display-2", 1)]);
        assert!(missing.is_empty());
        // Windows renumbers: B is now first in the list. It is still found.
        let swapped = vec![mon("B", 1, 0, 1920, 1080), mon("A", 2, 1920, 2560, 1440)];
        let (found, _) = resolve(&l, &swapped, 1);
        assert_eq!(found.iter().map(|p| (p.key.as_str(), p.monitor)).collect::<Vec<_>>(), vec![("main", 1), ("display-2", 0)]);
        // B unplugged: its cutouts are parked.
        l.send_to(2, "display-2", Size::new(2560.0, 1440.0), Size::new(1920.0, 1080.0), None);
        let (found, missing) = resolve(&l, &ms[..1], 0);
        assert_eq!(found.len(), 1);
        assert_eq!(missing, vec!["display-2".to_string()]);
        assert_eq!(l.parked(&missing), vec![2]);
        // The source moved onto B: B cannot also be display-2.
        let (found, missing) = resolve(&l, &ms, 1);
        assert_eq!(found.iter().map(|p| p.key.as_str()).collect::<Vec<_>>(), vec!["main"]);
        assert_eq!(missing, vec!["display-2".to_string()]);
    }

    #[test]
    fn sending_keeps_the_pixel_size_and_shrinks_to_fit() {
        let mut l = layout();
        let big = Size::new(3840.0, 2160.0);
        let small = Size::new(1920.0, 1080.0);
        l.cutout_mut(1).unwrap().placement.rect = Rect::new(0.0, 0.0, 0.25, 0.25); // 960×540 on 4K
        l.display_key(&mon("B", 2, 3840, 1920, 1080), false, DisplayMode::Takeover);
        assert!(l.send_to(1, "display-2", big, small, None));
        let p = &l.cutout(1).unwrap().placement;
        assert_eq!(p.display, "display-2");
        let px = p.rect.scale(small);
        assert!((px.w - 960.0).abs() < 1e-6 && (px.h - 540.0).abs() < 1e-6, "same pixel size: {px:?}");
        assert!((px.x - 480.0).abs() < 1e-6 && (px.y - 270.0).abs() < 1e-6, "centred: {px:?}");
        // Too big for the new display: shrunk to fit, aspect kept.
        l.cutout_mut(2).unwrap().placement.rect = Rect::new(0.0, 0.0, 1.0, 1.0);
        l.send_to(2, "display-2", big, small, Some((0.9, 0.9)));
        let px = l.cutout(2).unwrap().placement.rect.scale(small);
        assert!((px.w - 1920.0).abs() < 1e-6 && (px.h - 1080.0).abs() < 1e-6);
        // Dropped near an edge: stays inside.
        l.cutout_mut(1).unwrap().placement.rect = Rect::new(0.0, 0.0, 0.25, 0.25);
        l.send_to(1, "display-2", small, small, Some((0.99, 0.99)));
        let r = l.cutout(1).unwrap().placement.rect;
        assert!(r.x + r.w <= 1.0 + 1e-9 && r.y + r.h <= 1.0 + 1e-9);
        assert!(!l.send_to(1, "nowhere", small, small, None));
    }

    #[test]
    fn duplicate_and_release_a_display() {
        let mut l = layout();
        let n = l.duplicate(1).unwrap();
        assert_eq!(n, 3);
        let c = l.cutout(3).unwrap();
        assert_eq!(c.rects, l.cutout(1).unwrap().rects);
        assert_ne!(c.color, l.cutout(1).unwrap().color);
        assert!(c.placement.z > l.cutout(2).unwrap().placement.z);
        let key = l.display_key(&mon("B", 2, 2560, 1920, 1080), false, DisplayMode::Takeover);
        let mut sizes = BTreeMap::new();
        sizes.insert("main".to_string(), Size::new(2560.0, 1440.0));
        sizes.insert(key.clone(), Size::new(1920.0, 1080.0));
        l.send_to(3, &key, Size::new(2560.0, 1440.0), Size::new(1920.0, 1080.0), None);
        assert!(l.set_display_mode(&key, Some(DisplayMode::Float), &sizes));
        assert_eq!(l.displays[&key].mode, DisplayMode::Float);
        assert!(l.set_display_mode(&key, None, &sizes));
        assert!(!l.displays.contains_key(&key));
        assert_eq!(l.cutout(3).unwrap().placement.display, "main", "its cutouts come back to the main display");
        assert!(!l.set_display_mode("main", None, &sizes), "the main display stays");
    }

    #[test]
    fn set_keys_ignore_order() {
        let a = vec![mon("A", 1, 0, 10, 10), mon("B", 2, 10, 10, 10)];
        let b = vec![mon("B", 1, 0, 10, 10), mon("A", 2, 10, 10, 10)];
        assert_eq!(set_key(&a), set_key(&b));
        assert_ne!(set_key(&a), set_key(&a[..1]));
    }
}
