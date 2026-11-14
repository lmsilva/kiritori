//! Suggest cutouts (S) and Trim black (T), from the still frame's pixels.
//!
//! Game and simulator pictures are usually built from rectangular panels: a main view, a
//! gauge cluster, a map. Their borders are straight lines that run across most of the picture
//! (or of a panel), either a sharp change of colour or a dark gutter. Suggest finds those lines,
//! splits the picture along them, and splits each part again the same way, a few levels deep.
//! Parts that are almost entirely black are dropped, and every part loses its black edges.

use super::geometry::Rect;

/// A greyscale picture, row by row, 0–255.
pub struct Luma {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u8>,
}

impl Luma {
    /// From BGRA pixels, shrunk so the longer side is at most `max` by averaging.
    pub fn from_bgra(w: usize, h: usize, bgra: &[u8], max: usize) -> Luma {
        let f = w.max(h).div_ceil(max).max(1);
        let (ow, oh) = (w / f, h / f);
        let mut px = vec![0u8; ow * oh];
        for y in 0..oh {
            for x in 0..ow {
                let mut sum = 0u32;
                for dy in 0..f {
                    for dx in 0..f {
                        let i = ((y * f + dy) * w + x * f + dx) * 4;
                        let (b, g, r) = (bgra[i] as u32, bgra[i + 1] as u32, bgra[i + 2] as u32);
                        sum += (r * 54 + g * 183 + b * 19) >> 8;
                    }
                }
                px[y * ow + x] = (sum / (f * f) as u32) as u8;
            }
        }
        Luma { w: ow, h: oh, px }
    }

    fn at(&self, x: usize, y: usize) -> i32 {
        self.px[y * self.w + x] as i32
    }
}

/// Darker than this counts as black.
const BLACK: i32 = 28;
/// A change of brightness this large between neighbours is an edge.
const EDGE: i32 = 15;
/// A line must cover this share of the part it splits.
const COVER: f64 = 0.6;
/// Parts smaller than this share of the picture's side are not split further.
const MIN_PART: f64 = 0.08;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Part {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
}

impl Part {
    fn w(&self) -> usize {
        self.x1 - self.x0
    }
    fn h(&self) -> usize {
        self.y1 - self.y0
    }
}

fn dark_col(l: &Luma, x: usize, p: Part) -> bool {
    let n = (p.y0..p.y1).filter(|&y| l.at(x, y) < BLACK).count();
    n as f64 >= 0.95 * p.h() as f64
}

fn dark_row(l: &Luma, y: usize, p: Part) -> bool {
    let n = (p.x0..p.x1).filter(|&x| l.at(x, y) < BLACK).count();
    n as f64 >= 0.95 * p.w() as f64
}

/// How strongly a straight line splits the part between columns `at - 1` and `at` (vertical)
/// or rows: the share of its length where the three pixels on one side differ clearly in
/// brightness from the three on the other (averaging steps over texture and soft edges). The
/// edge of a dark gutter scores a little over 1, so it wins over the edge next to it.
fn line_score(l: &Luma, p: Part, vertical: bool, at: usize) -> f64 {
    let (lo, hi) = if vertical { (p.x0, p.x1) } else { (p.y0, p.y1) };
    if at < lo + 3 || at + 3 > hi {
        return 0.0;
    }
    let gutter = if vertical { dark_col(l, at, p) != dark_col(l, at - 1, p) } else { dark_row(l, at, p) != dark_row(l, at - 1, p) };
    if gutter {
        return 1.01;
    }
    let mean = |a: usize, b: usize, across: usize| -> i32 {
        (a..b).map(|i| if vertical { l.at(i, across) } else { l.at(across, i) }).sum::<i32>() / (b - a) as i32
    };
    let (from, to) = if vertical { (p.y0, p.y1) } else { (p.x0, p.x1) };
    let n = (from..to).filter(|&k| (mean(at, at + 3, k) - mean(at - 3, at, k)).abs() > EDGE).count();
    n as f64 / (to - from) as f64
}

/// The strongest line through the part that leaves both sides big enough, if it is strong
/// enough: (vertical, position, score).
fn best_line(l: &Luma, p: Part, cover: f64) -> Option<(bool, usize, f64)> {
    let min_w = ((l.w as f64 * MIN_PART) as usize).max(2);
    let min_h = ((l.h as f64 * MIN_PART) as usize).max(2);
    let mut best: Option<(bool, usize, f64)> = None;
    for x in p.x0 + min_w..=p.x1.saturating_sub(min_w) {
        let s = line_score(l, p, true, x);
        if best.is_none_or(|b| s > b.2) {
            best = Some((true, x, s));
        }
    }
    for y in p.y0 + min_h..=p.y1.saturating_sub(min_h) {
        let s = line_score(l, p, false, y);
        if best.is_none_or(|b| s > b.2) {
            best = Some((false, y, s));
        }
    }
    best.filter(|b| b.2 >= cover)
}

/// Shrink a part while its outer rows and columns are black.
fn trim_part(l: &Luma, mut p: Part) -> Option<Part> {
    while p.w() > 1 && dark_col(l, p.x0, p) {
        p.x0 += 1;
    }
    while p.w() > 1 && dark_col(l, p.x1 - 1, p) {
        p.x1 -= 1;
    }
    while p.h() > 1 && dark_row(l, p.y0, p) {
        p.y0 += 1;
    }
    while p.h() > 1 && dark_row(l, p.y1 - 1, p) {
        p.y1 -= 1;
    }
    (p.w() > 1 && p.h() > 1).then_some(p)
}

/// Split a part, and return the line used. `hint` is the line its sibling was split along:
/// panels usually line up, so the same line is taken here if it shows at all.
fn split(l: &Luma, p: Part, depth: u32, hint: Option<(bool, usize)>, out: &mut Vec<Part>) -> Option<(bool, usize)> {
    let min_w = (l.w as f64 * MIN_PART) as usize;
    let min_h = (l.h as f64 * MIN_PART) as usize;
    // The first two splits (halves, then quarters) take a clear line; deeper ones only a
    // very clear one, so textures inside a panel do not cut it up.
    let cover = if depth >= 3 { COVER } else { 0.9 };
    let hinted = hint.filter(|_| depth >= 3).filter(|&(v, at)| {
        let (lo, hi, min) = if v { (p.x0, p.x1, min_w) } else { (p.y0, p.y1, min_h) };
        at >= lo + min.max(2) && at + min.max(2) <= hi && line_score(l, p, v, at) >= 0.4
    });
    if depth > 0
        && let Some((vertical, at)) = hinted.or_else(|| best_line(l, p, cover).map(|b| (b.0, b.1)))
    {
        let (a, b) = if vertical { (Part { x1: at, ..p }, Part { x0: at, ..p }) } else { (Part { y1: at, ..p }, Part { y0: at, ..p }) };
        let first = split(l, a, depth - 1, None, out);
        split(l, b, depth - 1, first, out);
        return Some((vertical, at));
    }
    // A part that is almost entirely black is not a panel.
    let dark = (p.y0..p.y1).flat_map(|y| (p.x0..p.x1).map(move |x| (x, y))).filter(|&(x, y)| l.at(x, y) < BLACK).count();
    if (dark as f64) < 0.9 * (p.w() * p.h()) as f64
        && let Some(t) = trim_part(l, p)
        && t.w() >= min_w.max(2)
        && t.h() >= min_h.max(2)
    {
        out.push(t);
    }
    None
}

fn to_rect(l: &Luma, p: Part) -> Rect {
    Rect::new(p.x0 as f64 / l.w as f64, p.y0 as f64 / l.h as f64, p.w() as f64 / l.w as f64, p.h() as f64 / l.h as f64)
}

/// Suggested cutouts, in source fractions, largest first.
pub fn suggest(l: &Luma) -> Vec<Rect> {
    let mut parts = Vec::new();
    split(l, Part { x0: 0, y0: 0, x1: l.w, y1: l.h }, 4, None, &mut parts);
    let mut rects: Vec<Rect> = parts.into_iter().map(|p| to_rect(l, p)).collect();
    rects.sort_by(|a, b| (b.w * b.h).total_cmp(&(a.w * a.h)));
    // One part that is the whole picture is no suggestion.
    if rects.len() == 1 && rects[0].w > 0.95 && rects[0].h > 0.95 {
        return Vec::new();
    }
    rects.truncate(9);
    rects
}

/// `r` (source fractions) without its black edges, or None if it is all black.
pub fn trim(l: &Luma, r: Rect) -> Option<Rect> {
    let x0 = ((r.x * l.w as f64).floor() as usize).min(l.w.saturating_sub(1));
    let y0 = ((r.y * l.h as f64).floor() as usize).min(l.h.saturating_sub(1));
    let x1 = ((r.right() * l.w as f64).ceil() as usize).clamp(x0 + 1, l.w);
    let y1 = ((r.bottom() * l.h as f64).ceil() as usize).clamp(y0 + 1, l.h);
    let whole = Part { x0, y0, x1, y1 };
    let t = trim_part(l, whole)?;
    if t == whole {
        return Some(r);
    }
    // Keep the original edge where nothing was trimmed, so a trim never grows a cutout.
    let mut out = to_rect(l, t);
    if t.x0 == x0 {
        out.w += out.x - r.x;
        out.x = r.x;
    }
    if t.y0 == y0 {
        out.h += out.y - r.y;
        out.y = r.y;
    }
    if t.x1 == x1 {
        out.w = r.right() - out.x;
    }
    if t.y1 == y1 {
        out.h = r.bottom() - out.y;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 4-panel picture like the README example's: four textured panels split by 4 px black gutters,
    /// with a black band at the bottom of the lower-left panel.
    fn quadrants() -> Luma {
        let (w, h) = (320, 180);
        let mut px = vec![0u8; w * h];
        for y in 0..h {
            for x in 0..w {
                let gutter = (158..162).contains(&x) || (88..92).contains(&y);
                let band = x < 158 && y >= 160;
                px[y * w + x] = if gutter || band {
                    5
                } else {
                    let panel = (x >= 160) as usize + 2 * (y >= 90) as usize;
                    (60 + panel * 40 + ((x * 7 + y * 3) % 23)) as u8
                };
            }
        }
        Luma { w, h, px }
    }

    fn near(a: f64, b: f64) -> bool {
        (a - b).abs() < 0.02
    }

    #[test]
    fn four_panels_are_found() {
        let l = quadrants();
        let s = suggest(&l);
        assert_eq!(s.len(), 4, "{s:?}");
        // Top right: x 0.5–1, y 0–0.5 (gutters excluded).
        assert!(s.iter().any(|r| near(r.x, 0.506) && near(r.y, 0.0) && near(r.right(), 1.0) && near(r.bottom(), 0.489)), "{s:?}");
        // Bottom left lost its black band to trimming.
        assert!(s.iter().any(|r| near(r.x, 0.0) && near(r.y, 0.511) && near(r.bottom(), 0.889)), "{s:?}");
    }

    #[test]
    fn trimming_removes_black_edges_only() {
        let l = quadrants();
        let t = trim(&l, Rect::new(0.0, 0.5, 0.5, 0.5)).unwrap();
        assert!(near(t.y, 0.511) && near(t.bottom(), 0.889) && near(t.x, 0.0), "{t:?}");
        // Nothing black at the edges: unchanged.
        let r = Rect::new(0.6, 0.1, 0.2, 0.2);
        assert_eq!(trim(&l, r), Some(r));
        // All black: nothing left.
        let blank = Luma { w: 10, h: 10, px: vec![0; 100] };
        assert_eq!(trim(&blank, Rect::new(0.0, 0.0, 1.0, 1.0)), None);
    }

    /// The README example picture: its four quadrants must come out as separate cutouts,
    /// and no cutout may straddle the middle lines.
    #[test]
    fn sample_quadrants() {
        let img = image::open(concat!(env!("CARGO_MANIFEST_DIR"), "/../docs/images/sample-quadrants.webp")).unwrap().to_rgba8();
        let (w, h) = (img.width() as usize, img.height() as usize);
        let bgra: Vec<u8> = img.pixels().flat_map(|p| [p[2], p[1], p[0], 255]).collect();
        let l = Luma::from_bgra(w, h, &bgra, 640);
        let s = suggest(&l);
        assert!(s.len() >= 4, "{s:?}");
        let (mx, my) = (1000.0 / 2000.0, 565.0 / 1127.0);
        for r in &s {
            assert!(r.right() <= mx + 0.02 || r.x >= mx - 0.02, "straddles the vertical middle: {r:?}");
            assert!(r.bottom() <= my + 0.02 || r.y >= my - 0.02, "straddles the horizontal middle: {r:?}");
        }
        // The driver's view (top right) is found whole.
        assert!(s.iter().any(|r| (r.x - mx).abs() < 0.03 && r.y < 0.03 && r.right() > 0.97 && (r.bottom() - my).abs() < 0.03), "{s:?}");
    }

    #[test]
    fn a_plain_picture_has_no_suggestions() {
        let l = Luma { w: 100, h: 60, px: (0..6000).map(|i| (100 + (i * 13) % 17) as u8).collect() };
        assert!(suggest(&l).is_empty());
    }

    #[test]
    fn bgra_is_shrunk_and_greyed() {
        let bgra: Vec<u8> = (0..(8 * 4)).flat_map(|i| if i % 2 == 0 { [255, 255, 255, 255] } else { [0, 0, 0, 255] }).collect();
        let l = Luma::from_bgra(8, 4, &bgra, 4);
        assert_eq!((l.w, l.h), (4, 2));
        assert!(l.px.iter().all(|&v| (120..=135).contains(&v)), "{:?}", l.px);
    }
}
