//! Lucide icons (ISC licence, see assets/lucide/LICENSE), embedded as their SVG source and
//! turned into simple outlines the renderer strokes with Direct2D. Pinned to lucide-static
//! 0.469.0.
//!
//! Only what Lucide's 24×24 icons use is parsed: path (M L H V C S Q A Z, absolute and
//! relative), circle, line, polyline and rect (with rounded corners).

macro_rules! icons {
    ($($name:literal),* $(,)?) => {
        pub const ICONS: &[(&str, &str)] = &[$(($name, include_str!(concat!("../../assets/lucide/", $name, ".svg")))),*];
    };
}

icons!(
    "scissors", "layout-dashboard", "undo-2", "redo-2", "rotate-ccw", "refresh-cw", "grid-2x2", "eye", "eye-off", "lock", "lock-open",
    "maximize-2", "minimize-2", "wand-sparkles", "undo-dot", "magnet", "trash-2", "x", "panel-left", "layers", "keyboard", "settings",
    "app-window", "monitor-off", "video", "image", "pencil", "plus", "chevron-down", "star", "copy", "download", "upload", "history",
    "folder-open", "check", "rotate-cw", "flip-horizontal-2", "flip-vertical-2", "crop", "monitor", "gauge", "mouse-pointer-2", "sparkles",
    "square-dashed", "circle", "map", "send", "scan",
);

pub type Pt = (f32, f32);

#[derive(Clone, Debug, PartialEq)]
pub enum Seg {
    Line(Pt),
    Cubic(Pt, Pt, Pt),
    Quad(Pt, Pt),
    /// SVG arc: end point, radii, x-axis rotation (degrees), large-arc flag, sweep flag.
    Arc { to: Pt, r: Pt, rot: f32, large: bool, sweep: bool },
}

/// One continuous stroke: a start point and the segments that follow it.
#[derive(Clone, Debug, PartialEq)]
pub struct Figure {
    pub start: Pt,
    pub segs: Vec<Seg>,
    pub closed: bool,
}

pub fn source(name: &str) -> Option<&'static str> {
    ICONS.iter().find(|(n, _)| *n == name).map(|(_, s)| *s)
}

/// The icon as figures on its 24×24 grid.
pub fn figures(name: &str) -> Vec<Figure> {
    source(name).map(parse_svg).unwrap_or_default()
}

fn attr(tag: &str, name: &str) -> Option<String> {
    let key = format!(" {name}=\"");
    let i = tag.find(&key)? + key.len();
    let j = tag[i..].find('"')? + i;
    Some(tag[i..j].to_string())
}

fn num(tag: &str, name: &str) -> f32 {
    attr(tag, name).and_then(|v| v.trim().parse().ok()).unwrap_or(0.0)
}

pub fn parse_svg(svg: &str) -> Vec<Figure> {
    let mut out = Vec::new();
    let mut rest = svg;
    while let Some(i) = rest.find('<') {
        let Some(j) = rest[i..].find('>') else { break };
        let tag = &rest[i..i + j + 1];
        rest = &rest[i + j + 1..];
        let name: String = tag[1..].chars().take_while(|c| c.is_ascii_alphabetic()).collect();
        // Normalise whitespace so attribute lookups see " name=".
        let tag = tag.replace(['\n', '\r', '\t'], " ");
        match name.as_str() {
            "path" => out.extend(parse_path(&attr(&tag, "d").unwrap_or_default())),
            "circle" => {
                let (cx, cy, r) = (num(&tag, "cx"), num(&tag, "cy"), num(&tag, "r"));
                out.push(Figure {
                    start: (cx + r, cy),
                    segs: vec![
                        Seg::Arc { to: (cx - r, cy), r: (r, r), rot: 0.0, large: false, sweep: true },
                        Seg::Arc { to: (cx + r, cy), r: (r, r), rot: 0.0, large: false, sweep: true },
                    ],
                    closed: true,
                });
            }
            "line" => out.push(Figure { start: (num(&tag, "x1"), num(&tag, "y1")), segs: vec![Seg::Line((num(&tag, "x2"), num(&tag, "y2")))], closed: false }),
            "polyline" | "polygon" => {
                let n: Vec<f32> = numbers(&attr(&tag, "points").unwrap_or_default());
                let pts: Vec<Pt> = n.chunks(2).filter(|c| c.len() == 2).map(|c| (c[0], c[1])).collect();
                if let Some((first, more)) = pts.split_first() {
                    out.push(Figure { start: *first, segs: more.iter().map(|p| Seg::Line(*p)).collect(), closed: name == "polygon" });
                }
            }
            "rect" => {
                let (x, y, w, h) = (num(&tag, "x"), num(&tag, "y"), num(&tag, "width"), num(&tag, "height"));
                let rx = attr(&tag, "rx").and_then(|v| v.parse().ok()).unwrap_or(0.0f32).min(w / 2.0);
                let ry = attr(&tag, "ry").and_then(|v| v.parse().ok()).unwrap_or(rx).min(h / 2.0);
                let arc = |to: Pt| Seg::Arc { to, r: (rx, ry), rot: 0.0, large: false, sweep: true };
                let segs = if rx > 0.0 {
                    vec![
                        Seg::Line((x + w - rx, y)),
                        arc((x + w, y + ry)),
                        Seg::Line((x + w, y + h - ry)),
                        arc((x + w - rx, y + h)),
                        Seg::Line((x + rx, y + h)),
                        arc((x, y + h - ry)),
                        Seg::Line((x, y + ry)),
                        arc((x + rx, y)),
                    ]
                } else {
                    vec![Seg::Line((x + w, y)), Seg::Line((x + w, y + h)), Seg::Line((x, y + h))]
                };
                out.push(Figure { start: (x + rx, y), segs, closed: true });
            }
            _ => {}
        }
    }
    out
}

/// All numbers in a string, SVG style: "8.12-1.5.5" is 8.12, -1.5, 0.5.
fn numbers(s: &str) -> Vec<f32> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let flush = |cur: &mut String, out: &mut Vec<f32>| {
        if let Ok(v) = cur.parse() {
            out.push(v);
        }
        cur.clear();
    };
    for c in s.chars() {
        match c {
            '-' if !cur.is_empty() && !cur.ends_with('e') => {
                flush(&mut cur, &mut out);
                cur.push(c);
            }
            '.' if cur.contains('.') => {
                flush(&mut cur, &mut out);
                cur.push(c);
            }
            c if c.is_ascii_digit() || c == '.' || c == '-' || c == 'e' => cur.push(c),
            _ => flush(&mut cur, &mut out),
        }
    }
    flush(&mut cur, &mut out);
    out
}

/// Tokens of a path: commands and numbers. Arc flags may be packed ("a1 1 0 01 2 2").
fn tokens(d: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut i = 0;
    let b: Vec<char> = d.chars().collect();
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_alphabetic() && c != 'e' {
            out.push(Tok::Cmd(c));
            i += 1;
        } else if c.is_ascii_digit() || c == '.' || c == '-' {
            let mut j = i + 1;
            let mut dot = c == '.';
            while j < b.len() {
                let d = b[j];
                if d.is_ascii_digit() {
                    j += 1;
                } else if d == '.' && !dot {
                    dot = true;
                    j += 1;
                } else {
                    break;
                }
            }
            let s: String = b[i..j].iter().collect();
            out.push(Tok::Num(s.parse().unwrap_or(0.0), s));
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

#[derive(Clone, Debug)]
enum Tok {
    Cmd(char),
    Num(f32, String),
}

pub fn parse_path(d: &str) -> Vec<Figure> {
    let toks = tokens(d);
    let mut figs: Vec<Figure> = Vec::new();
    let mut i = 0;
    let mut cmd = 'M';
    let mut cur: Pt = (0.0, 0.0);
    let mut start: Pt = (0.0, 0.0);
    let mut last_ctrl: Option<Pt> = None;
    let mut fig: Option<Figure> = None;
    let next = |i: &mut usize| -> Option<f32> {
        match toks.get(*i) {
            Some(Tok::Num(v, _)) => {
                *i += 1;
                Some(*v)
            }
            _ => None,
        }
    };
    // A flag may be one digit of a packed run: "01" is 0 then 1.
    let flag = |toks: &Vec<Tok>, i: &mut usize, pending: &mut Option<String>| -> Option<bool> {
        if let Some(p) = pending.as_mut() {
            let c = p.remove(0);
            if p.is_empty() {
                *pending = None;
            }
            return Some(c == '1');
        }
        match toks.get(*i) {
            Some(Tok::Num(_, s)) => {
                *i += 1;
                let mut s = s.clone();
                let c = s.remove(0);
                if !s.is_empty() {
                    *pending = Some(s);
                }
                Some(c == '1')
            }
            _ => None,
        }
    };
    let mut pending: Option<String> = None;
    while i < toks.len() || pending.is_some() {
        if pending.is_none()
            && let Some(Tok::Cmd(c)) = toks.get(i)
        {
            cmd = *c;
            i += 1;
            if cmd == 'z' || cmd == 'Z' {
                if let Some(mut f) = fig.take() {
                    f.closed = true;
                    figs.push(f);
                }
                cur = start;
                continue;
            }
        }
        let rel = cmd.is_ascii_lowercase();
        let base = if rel { cur } else { (0.0, 0.0) };
        let before = i;
        match cmd.to_ascii_uppercase() {
            'M' => {
                let (Some(x), Some(y)) = (next(&mut i), next(&mut i)) else { break };
                if let Some(f) = fig.take() {
                    figs.push(f);
                }
                cur = (base.0 + x, base.1 + y);
                start = cur;
                fig = Some(Figure { start: cur, segs: Vec::new(), closed: false });
                // Further pairs after M are lines.
                cmd = if rel { 'l' } else { 'L' };
            }
            'L' => {
                let (Some(x), Some(y)) = (next(&mut i), next(&mut i)) else { break };
                cur = (base.0 + x, base.1 + y);
                fig.get_or_insert(Figure { start, segs: Vec::new(), closed: false }).segs.push(Seg::Line(cur));
            }
            'H' => {
                let Some(x) = next(&mut i) else { break };
                cur = (if rel { cur.0 + x } else { x }, cur.1);
                fig.get_or_insert(Figure { start, segs: Vec::new(), closed: false }).segs.push(Seg::Line(cur));
            }
            'V' => {
                let Some(y) = next(&mut i) else { break };
                cur = (cur.0, if rel { cur.1 + y } else { y });
                fig.get_or_insert(Figure { start, segs: Vec::new(), closed: false }).segs.push(Seg::Line(cur));
            }
            'C' => {
                let v: Vec<f32> = (0..6).filter_map(|_| next(&mut i)).collect();
                if v.len() < 6 {
                    break;
                }
                let c1 = (base.0 + v[0], base.1 + v[1]);
                let c2 = (base.0 + v[2], base.1 + v[3]);
                cur = (base.0 + v[4], base.1 + v[5]);
                last_ctrl = Some(c2);
                fig.get_or_insert(Figure { start, segs: Vec::new(), closed: false }).segs.push(Seg::Cubic(c1, c2, cur));
                continue;
            }
            'S' => {
                let v: Vec<f32> = (0..4).filter_map(|_| next(&mut i)).collect();
                if v.len() < 4 {
                    break;
                }
                let c1 = match last_ctrl {
                    Some(c) => (2.0 * cur.0 - c.0, 2.0 * cur.1 - c.1),
                    None => cur,
                };
                let c2 = (base.0 + v[0], base.1 + v[1]);
                cur = (base.0 + v[2], base.1 + v[3]);
                last_ctrl = Some(c2);
                fig.get_or_insert(Figure { start, segs: Vec::new(), closed: false }).segs.push(Seg::Cubic(c1, c2, cur));
                continue;
            }
            'Q' => {
                let v: Vec<f32> = (0..4).filter_map(|_| next(&mut i)).collect();
                if v.len() < 4 {
                    break;
                }
                let c = (base.0 + v[0], base.1 + v[1]);
                cur = (base.0 + v[2], base.1 + v[3]);
                fig.get_or_insert(Figure { start, segs: Vec::new(), closed: false }).segs.push(Seg::Quad(c, cur));
            }
            'A' => {
                let (Some(rx), Some(ry), Some(rot)) = (next(&mut i), next(&mut i), next(&mut i)) else { break };
                let (Some(large), Some(sweep)) = (flag(&toks, &mut i, &mut pending), flag(&toks, &mut i, &mut pending)) else { break };
                let (x, y) = if let Some(p) = pending.take() {
                    // The end x was packed after the flags ("01.5 2").
                    (p.parse().unwrap_or(0.0), next(&mut i).unwrap_or(0.0))
                } else {
                    let (Some(x), Some(y)) = (next(&mut i), next(&mut i)) else { break };
                    (x, y)
                };
                cur = (base.0 + x, base.1 + y);
                fig.get_or_insert(Figure { start, segs: Vec::new(), closed: false }).segs.push(Seg::Arc { to: cur, r: (rx, ry), rot, large, sweep });
            }
            _ => i += 1,
        }
        last_ctrl = None;
        if i == before && pending.is_none() {
            i += 1; // never loop forever on bad input
        }
    }
    if let Some(f) = fig {
        figs.push(f);
    }
    figs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_parses_into_something() {
        for (name, svg) in ICONS {
            let f = parse_svg(svg);
            assert!(!f.is_empty(), "{name} has no figures");
            for fig in &f {
                for p in std::iter::once(fig.start).chain(fig.segs.iter().map(|s| match s {
                    Seg::Line(p) | Seg::Quad(_, p) | Seg::Cubic(_, _, p) | Seg::Arc { to: p, .. } => *p,
                })) {
                    assert!((-1.0..=25.0).contains(&p.0) && (-1.0..=25.0).contains(&p.1), "{name} point {p:?} off the 24x24 grid");
                }
            }
        }
    }

    #[test]
    fn scissors_has_two_circles_and_three_lines() {
        let f = figures("scissors");
        assert_eq!(f.len(), 5);
        assert_eq!(f[0].start, (9.0, 6.0));
        assert_eq!(f[1].segs, vec![Seg::Line((12.0, 12.0))]);
    }

    #[test]
    fn relative_commands_and_packed_numbers() {
        let f = parse_path("M3 7v6h6 m4-2.5.5 2");
        assert_eq!(f[0].segs, vec![Seg::Line((3.0, 13.0)), Seg::Line((9.0, 13.0))]);
        assert_eq!(f[1].start, (13.0, 10.5));
        assert_eq!(f[1].segs, vec![Seg::Line((13.5, 12.5))]);
    }

    #[test]
    fn packed_arc_flags() {
        let f = parse_path("M10 10a2 2 0 01-2 2");
        assert_eq!(f[0].segs, vec![Seg::Arc { to: (8.0, 12.0), r: (2.0, 2.0), rot: 0.0, large: false, sweep: true }]);
    }

    #[test]
    fn closepath_closes() {
        let f = parse_path("M1 1 L5 1 L5 5 Z");
        assert!(f[0].closed);
        assert_eq!(f[0].segs.len(), 2);
    }
}
