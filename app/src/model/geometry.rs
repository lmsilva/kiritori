//! Rectangles and sizes. Cut shapes use fractions of the source's drawing area (0–1);
//! placements use fractions of a display's canvas (0–1); pixel sizes are only used to keep
//! aspect ratios right.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Size {
    pub w: f64,
    pub h: f64,
}

impl Size {
    pub const fn new(w: f64, h: f64) -> Self {
        Size { w, h }
    }
}

impl Rect {
    pub const fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Rect { x, y, w, h }
    }

    pub fn right(&self) -> f64 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f64 {
        self.y + self.h
    }

    pub fn is_empty(&self) -> bool {
        self.w <= 0.0 || self.h <= 0.0
    }

    pub fn contains(&self, px: f64, py: f64) -> bool {
        px >= self.x && px < self.right() && py >= self.y && py < self.bottom()
    }

    pub fn union(&self, o: &Rect) -> Rect {
        if self.is_empty() {
            return *o;
        }
        if o.is_empty() {
            return *self;
        }
        let x = self.x.min(o.x);
        let y = self.y.min(o.y);
        Rect::new(x, y, self.right().max(o.right()) - x, self.bottom().max(o.bottom()) - y)
    }

    pub fn intersect(&self, o: &Rect) -> Rect {
        let x = self.x.max(o.x);
        let y = self.y.max(o.y);
        let r = self.right().min(o.right());
        let b = self.bottom().min(o.bottom());
        if r <= x || b <= y { Rect::default() } else { Rect::new(x, y, r - x, b - y) }
    }

    /// Multiply by a size: fractions to pixels.
    pub fn scale(&self, s: Size) -> Rect {
        Rect::new(self.x * s.w, self.y * s.h, self.w * s.w, self.h * s.h)
    }

    /// Divide by a size: pixels to fractions.
    pub fn unscale(&self, s: Size) -> Rect {
        Rect::new(self.x / s.w, self.y / s.h, self.w / s.w, self.h / s.h)
    }

    /// Map a rect given in fractions of `self` into the space `self` lives in.
    pub fn sub(&self, f: &Rect) -> Rect {
        Rect::new(self.x + f.x * self.w, self.y + f.y * self.h, f.w * self.w, f.h * self.h)
    }

    pub fn size(&self) -> Size {
        Size::new(self.w, self.h)
    }

    /// Keep the rect inside 0–1 on both axes, shrinking it if needed.
    pub fn clamp_unit(&self) -> Rect {
        self.intersect(&Rect::new(0.0, 0.0, 1.0, 1.0))
    }

    /// Round each edge to `places` decimal places, so saved files stay readable.
    pub fn rounded(&self, places: i32) -> Rect {
        let f = 10f64.powi(places);
        let r = |v: f64| (v * f).round() / f;
        Rect::new(r(self.x), r(self.y), r(self.w), r(self.h))
    }
}

/// The largest rect with `content`'s aspect ratio that fits inside `area`, centred in it.
pub fn fit(content: Size, area: Rect) -> Rect {
    if content.w <= 0.0 || content.h <= 0.0 || area.is_empty() {
        return Rect::new(area.x, area.y, 0.0, 0.0);
    }
    let s = (area.w / content.w).min(area.h / content.h);
    let (w, h) = (content.w * s, content.h * s);
    Rect::new(area.x + (area.w - w) / 2.0, area.y + (area.h - h) / 2.0, w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_letterboxes_and_centres() {
        let r = fit(Size::new(1280.0, 720.0), Rect::new(0.0, 0.0, 2560.0, 1080.0));
        assert_eq!(r, Rect::new(320.0, 0.0, 1920.0, 1080.0));
    }

    #[test]
    fn union_and_intersect() {
        let a = Rect::new(0.0, 0.0, 2.0, 2.0);
        let b = Rect::new(1.0, 1.0, 2.0, 2.0);
        assert_eq!(a.union(&b), Rect::new(0.0, 0.0, 3.0, 3.0));
        assert_eq!(a.intersect(&b), Rect::new(1.0, 1.0, 1.0, 1.0));
        assert!(a.intersect(&Rect::new(5.0, 5.0, 1.0, 1.0)).is_empty());
    }

    #[test]
    fn scale_and_sub() {
        let px = Rect::new(0.5, 0.0, 0.5, 0.5).scale(Size::new(2560.0, 1440.0));
        assert_eq!(px, Rect::new(1280.0, 0.0, 1280.0, 720.0));
        let inner = Rect::new(100.0, 50.0, 200.0, 100.0).sub(&Rect::new(0.5, 0.5, 0.5, 0.5));
        assert_eq!(inner, Rect::new(200.0, 100.0, 100.0, 50.0));
    }
}
