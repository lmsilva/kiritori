//! The tray icon, drawn in code at whatever size the taskbar asks for. An outlined
//! square with a smaller piece lifted out of its top-right
//! corner. State is shown by fill and badges, not by colour alone.

use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayState {
    Idle,
    Active,
    Editing,
    Bypassed,
    Problem,
}

type P = (f64, f64);

#[derive(Clone, Copy)]
enum Ink {
    /// The taskbar's text colour: white on a dark taskbar, near-black on a light one.
    Fg,
    /// Kiritori cyan, darker on a light taskbar for contrast.
    Accent,
}

enum Shape {
    /// Rounded rectangle, filled: x, y, w, h, radius.
    Round(f64, f64, f64, f64, f64),
    /// Rounded rectangle outline: x, y, w, h, radius, stroke width (centred on the edge).
    RoundLine(f64, f64, f64, f64, f64, f64),
    Line(P, P, f64),
    Fill(Vec<P>),
}

fn in_round(p: P, x: f64, y: f64, w: f64, h: f64, r: f64) -> bool {
    if p.0 < x || p.0 > x + w || p.1 < y || p.1 > y + h {
        return false;
    }
    let cx = p.0.clamp(x + r, x + w - r);
    let cy = p.1.clamp(y + r, y + h - r);
    (p.0 - cx).powi(2) + (p.1 - cy).powi(2) <= r * r
}

fn seg_dist(p: P, a: P, b: P) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    let t = if len2 == 0.0 { 0.0 } else { (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0) };
    ((p.0 - a.0 - t * dx).powi(2) + (p.1 - a.1 - t * dy).powi(2)).sqrt()
}

fn in_poly(p: P, poly: &[P]) -> bool {
    let mut c = false;
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + n - 1) % n]);
        if (a.1 > p.1) != (b.1 > p.1) && p.0 < (b.0 - a.0) * (p.1 - a.1) / (b.1 - a.1) + a.0 {
            c = !c;
        }
    }
    c
}

fn hit(s: &Shape, p: P) -> bool {
    match *s {
        Shape::Round(x, y, w, h, r) => in_round(p, x, y, w, h, r),
        Shape::RoundLine(x, y, w, h, r, sw) => {
            let o = sw / 2.0;
            in_round(p, x - o, y - o, w + sw, h + sw, r + o) && !in_round(p, x + o, y + o, w - sw, h - sw, (r - o).max(0.0))
        }
        Shape::Line(a, b, w) => seg_dist(p, a, b) <= w / 2.0,
        Shape::Fill(ref poly) => in_poly(p, poly),
    }
}

/// Layers bottom first, each (shape, ink, holes it cuts in everything below it).
fn drawing(state: TrayState) -> Vec<(Shape, Ink, Vec<Shape>)> {
    // On a 16×16 grid. The lifted piece cuts a 1-unit gap out of the square below it.
    let piece = (7.0, 1.0, 8.0, 8.0, 2.0);
    let gap = Shape::Round(piece.0 - 1.0, piece.1 - 1.0, piece.2 + 2.0, piece.3 + 2.0, piece.4 + 1.0);
    let mut layers = vec![(Shape::RoundLine(1.5, 4.5, 9.0, 9.0, 2.0, 1.3), Ink::Fg, Vec::new())];
    let filled = matches!(state, TrayState::Active | TrayState::Editing);
    if filled {
        layers.push((Shape::Round(piece.0, piece.1, piece.2, piece.3, piece.4), Ink::Accent, vec![gap]));
    } else {
        layers.push((Shape::RoundLine(piece.0 + 0.6, piece.1 + 0.6, piece.2 - 1.2, piece.3 - 1.2, piece.4 - 0.4, 1.2), Ink::Fg, vec![gap]));
    }
    match state {
        TrayState::Editing => {
            // Pencil, bottom-right, with a clear margin around it.
            layers.push((Shape::Line((10.5, 15.0), (15.2, 10.3), 2.0), Ink::Fg, vec![Shape::Line((10.5, 15.0), (15.2, 10.3), 4.0)]));
        }
        TrayState::Problem => {
            let tri = vec![(13.0, 9.2), (16.2, 15.6), (9.8, 15.6)];
            let halo = vec![(13.0, 7.4), (17.6, 16.6), (8.4, 16.6)];
            layers.push((Shape::Fill(tri), Ink::Fg, vec![Shape::Fill(halo)]));
        }
        TrayState::Bypassed => {
            layers.push((Shape::Line((1.0, 15.0), (15.0, 1.0), 1.4), Ink::Fg, vec![Shape::Line((1.0, 15.0), (15.0, 1.0), 3.4)]));
        }
        _ => {}
    }
    layers
}

/// Premultiplied BGRA pixels for a size×size icon.
pub fn pixels(state: TrayState, size: u32, light_taskbar: bool) -> Vec<u8> {
    let fg: (f64, f64, f64) = if light_taskbar { (26.0, 26.0, 26.0) } else { (255.0, 255.0, 255.0) };
    let accent: (f64, f64, f64) = if light_taskbar { (8.0, 145.0, 178.0) } else { (34.0, 211.0, 238.0) };
    let layers = drawing(state);
    // The "!" inside the warning badge.
    let bang = [Shape::Line((13.0, 11.4), (13.0, 13.3), 1.1), Shape::Line((13.0, 14.6), (13.0, 14.7), 1.1)];
    let scale = 16.0 / size as f64;
    let ss = 4;
    let mut out = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let (mut r, mut g, mut b, mut cov) = (0.0, 0.0, 0.0, 0.0);
            for sy in 0..ss {
                for sx in 0..ss {
                    let p = ((x as f64 + (sx as f64 + 0.5) / ss as f64) * scale, (y as f64 + (sy as f64 + 0.5) / ss as f64) * scale);
                    // Topmost layer wins; a layer's holes erase what is below it.
                    let mut ink = None;
                    for (shape, k, holes) in &layers {
                        if holes.iter().any(|h| hit(h, p)) {
                            ink = None;
                        }
                        if hit(shape, p) {
                            ink = Some(*k);
                        }
                    }
                    if state == TrayState::Problem && bang.iter().any(|s| hit(s, p)) {
                        ink = None;
                    }
                    if let Some(k) = ink {
                        let c = match k {
                            Ink::Fg => fg,
                            Ink::Accent => accent,
                        };
                        r += c.0;
                        g += c.1;
                        b += c.2;
                        cov += 1.0;
                    }
                }
            }
            let n = (ss * ss) as f64;
            let i = ((y * size + x) * 4) as usize;
            out[i] = (b / n).round() as u8;
            out[i + 1] = (g / n).round() as u8;
            out[i + 2] = (r / n).round() as u8;
            out[i + 3] = (cov / n * 255.0).round() as u8;
        }
    }
    out
}

/// The tile behind the exe icon: the dark taskbar the tray glyph was designed on.
#[cfg_attr(not(test), allow(dead_code))]
const TILE: (f64, f64, f64) = (32.0, 32.0, 32.0);
/// Sizes Windows asks an exe icon for (Explorer views, Alt+Tab, the taskbar, at any scale).
// Used by the test that keeps assets/kiritori.ico (embedded by build.rs) in step.
#[cfg_attr(not(test), allow(dead_code))]
pub const APP_ICON_SIZES: [u32; 9] = [16, 20, 24, 32, 40, 48, 64, 128, 256];

/// The exe's icon: the tray icon in its active state (white outline, cyan piece, as on a
/// dark taskbar) on a dark rounded tile, so it reads on light and dark backgrounds alike.
/// Straight (not premultiplied) RGBA.
#[cfg_attr(not(test), allow(dead_code))]
pub fn app_icon_rgba(size: u32) -> Vec<u8> {
    let layers = drawing(TrayState::Active);
    let (fg, accent) = ((255.0, 255.0, 255.0), (34.0, 211.0, 238.0));
    // On the 16-unit grid: the tile fills it; the glyph sits inside a 1.5-unit margin.
    let tile = Shape::Round(0.25, 0.25, 15.5, 15.5, 3.5);
    let (inset, glyph) = (1.75, 12.5);
    let scale = 16.0 / size as f64;
    let ss = 4;
    let mut out = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let (mut r, mut g, mut b, mut cov) = (0.0, 0.0, 0.0, 0.0);
            for sy in 0..ss {
                for sx in 0..ss {
                    let p = ((x as f64 + (sx as f64 + 0.5) / ss as f64) * scale, (y as f64 + (sy as f64 + 0.5) / ss as f64) * scale);
                    if !hit(&tile, p) {
                        continue;
                    }
                    let q = ((p.0 - inset) * 16.0 / glyph, (p.1 - inset) * 16.0 / glyph);
                    let mut ink = None;
                    for (shape, k, holes) in &layers {
                        if holes.iter().any(|h| hit(h, q)) {
                            ink = None;
                        }
                        if hit(shape, q) {
                            ink = Some(*k);
                        }
                    }
                    let c = match ink {
                        Some(Ink::Fg) => fg,
                        Some(Ink::Accent) => accent,
                        None => TILE,
                    };
                    r += c.0;
                    g += c.1;
                    b += c.2;
                    cov += 1.0;
                }
            }
            let i = ((y * size + x) * 4) as usize;
            if cov > 0.0 {
                out[i] = (r / cov).round() as u8;
                out[i + 1] = (g / cov).round() as u8;
                out[i + 2] = (b / cov).round() as u8;
                out[i + 3] = (cov / (ss * ss) as f64 * 255.0).round() as u8;
            }
        }
    }
    out
}

pub fn make(state: TrayState, light_taskbar: bool) -> windows::core::Result<HICON> {
    let size = unsafe { GetSystemMetrics(SM_CXSMICON) }.max(16) as u32;
    let px = pixels(state, size, light_taskbar);
    unsafe {
        let bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size as i32,
                biHeight: -(size as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let color = CreateDIBSection(None, &bi, DIB_RGB_COLORS, &mut bits, None, 0)?;
        std::ptr::copy_nonoverlapping(px.as_ptr(), bits as *mut u8, px.len());
        let mask = CreateBitmap(size as i32, size as i32, 1, 1, None);
        let info = ICONINFO { fIcon: true.into(), xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let icon = CreateIconIndirect(&info);
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
        icon
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn coverage(state: TrayState) -> usize {
        pixels(state, 32, false).chunks(4).filter(|p| p[3] > 128).count()
    }

    fn cyan(state: TrayState) -> usize {
        pixels(state, 32, false).chunks(4).filter(|p| p[3] > 200 && p[0] > 200 && p[2] < 80).count()
    }

    #[test]
    fn states_differ_by_shape() {
        let idle = coverage(TrayState::Idle);
        assert!(idle > 50, "outline is drawn");
        assert!(cyan(TrayState::Idle) == 0, "idle has no fill");
        assert!(cyan(TrayState::Active) > 100, "active piece is filled");
        assert_ne!(coverage(TrayState::Editing), coverage(TrayState::Active), "editing has a badge");
        assert_ne!(coverage(TrayState::Bypassed), idle, "bypassed has a slash");
        assert_ne!(coverage(TrayState::Problem), idle, "problem has a badge");
    }
}

#[cfg(test)]
mod app_icon {
    use super::*;

    fn ico_bytes() -> Vec<u8> {
        use image::codecs::ico::{IcoEncoder, IcoFrame};
        let frames: Vec<IcoFrame> = APP_ICON_SIZES
            .iter()
            .map(|&s| IcoFrame::as_png(&app_icon_rgba(s), s, s, image::ExtendedColorType::Rgba8).unwrap())
            .collect();
        let mut out = Vec::new();
        IcoEncoder::new(&mut out).encode_images(&frames).unwrap();
        out
    }

    /// The exe's icon file (embedded by build.rs) is drawn by this code. If the drawing
    /// changes, regenerate it: `KIRITORI_WRITE_ICON=1 cargo test -p kiritori app_icon`.
    #[test]
    fn the_icon_file_matches_the_drawing() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/kiritori.ico");
        let want = ico_bytes();
        if std::env::var("KIRITORI_WRITE_ICON").is_ok_and(|v| v == "1") {
            std::fs::write(path, &want).unwrap();
        }
        let have = std::fs::read(path).expect("assets/kiritori.ico exists");
        assert!(have == want, "assets/kiritori.ico is out of date: run with KIRITORI_WRITE_ICON=1");
    }

    #[test]
    fn tile_is_opaque_and_glyph_is_drawn() {
        let px = app_icon_rgba(64);
        let at = |x: usize, y: usize| &px[(y * 64 + x) * 4..(y * 64 + x) * 4 + 4];
        assert_eq!(at(32, 32)[3], 255, "the middle is solid");
        assert_eq!(at(0, 0)[3], 0, "the rounded corner is clear");
        assert!(px.chunks(4).any(|p| p[0] < 80 && p[1] > 180 && p[2] > 200), "the cyan piece is there");
        assert!(px.chunks(4).any(|p| p[0] > 240 && p[1] > 240 && p[2] > 240), "the white outline is there");
    }
}

#[cfg(test)]
mod preview {
    /// `cargo test -p kiritori dump_icons -- --ignored` writes every state as raw BGRA files to
    /// KIRITORI_ICON_DIR, for checking the design by eye.
    #[test]
    #[ignore]
    fn dump_icons() {
        use super::*;
        let dir = std::env::var("KIRITORI_ICON_DIR").expect("set KIRITORI_ICON_DIR");
        for (name, st) in [("idle", TrayState::Idle), ("active", TrayState::Active), ("editing", TrayState::Editing), ("bypassed", TrayState::Bypassed), ("problem", TrayState::Problem)] {
            for (theme, light) in [("dark", false), ("light", true)] {
                for size in [16u32, 32, 64] {
                    std::fs::write(format!("{dir}/{name}-{theme}-{size}.bgra"), pixels(st, size, light)).unwrap();
                }
            }
        }
    }
}
