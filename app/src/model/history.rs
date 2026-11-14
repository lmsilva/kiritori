//! Undo and redo. Each step keeps the whole layout before and after, so every step, including
//! both resets, is exactly reversible.

use std::time::{Duration, Instant};

use super::layout::Layout;

pub const DEPTH: usize = 200;
/// Arrow-key nudges on the same cutout within this time are one step.
pub const MERGE_WINDOW: Duration = Duration::from_secs(1);

#[derive(Clone, Debug)]
pub struct Step {
    pub name: String,
    pub before: Layout,
    pub after: Layout,
    /// Steps with the same key, close together in time, merge into one (nudges).
    merge_key: Option<String>,
    at: Instant,
}

#[derive(Clone, Debug, Default)]
pub struct History {
    undo: Vec<Step>,
    redo: Vec<Step>,
}

impl History {
    /// Record an edit. Does nothing if the layout did not actually change.
    pub fn record(&mut self, name: &str, before: Layout, after: Layout, merge_key: Option<String>, now: Instant) {
        if before == after {
            return;
        }
        self.redo.clear();
        if let (Some(key), Some(last)) = (&merge_key, self.undo.last_mut())
            && last.merge_key.as_ref() == Some(key) && now.duration_since(last.at) <= MERGE_WINDOW {
                last.after = after;
                last.at = now;
                return;
            }
        self.undo.push(Step { name: name.into(), before, after, merge_key, at: now });
        if self.undo.len() > DEPTH {
            self.undo.remove(0);
        }
    }

    /// The layout to go back to, and the step's name.
    pub fn undo(&mut self) -> Option<(String, Layout)> {
        let step = self.undo.pop()?;
        let out = (step.name.clone(), step.before.clone());
        self.redo.push(step);
        Some(out)
    }

    pub fn redo(&mut self) -> Option<(String, Layout)> {
        let step = self.redo.pop()?;
        let out = (step.name.clone(), step.after.clone());
        self.undo.push(step);
        Some(out)
    }

    /// For the tooltip: "Undo: Move Controls".
    pub fn undo_name(&self) -> Option<&str> {
        self.undo.last().map(|s| s.name.as_str())
    }

    pub fn redo_name(&self) -> Option<&str> {
        self.redo.last().map(|s| s.name.as_str())
    }

    /// Newest first, for the history list.
    pub fn names(&self) -> Vec<&str> {
        self.undo.iter().rev().map(|s| s.name.as_str()).collect()
    }

    pub fn len(&self) -> usize {
        self.undo.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::geometry::{Rect, Size};
    use crate::model::layout::{DisplayMode, DisplayRef, Frame};

    fn base() -> (Layout, Frame) {
        let f = Frame {
            source_px: Size::new(1920.0, 1080.0),
            canvas_px: Size::new(1920.0, 1080.0),
            source_on_canvas: Rect::new(0.0, 0.0, 1.0, 1.0),
        };
        let d = DisplayRef { hardware_id: "x".into(), label: "Display 1".into(), mode: DisplayMode::Takeover };
        (Layout::new("l", "L", d), f)
    }

    #[test]
    fn undo_and_redo_walk_the_steps() {
        let (l0, f) = base();
        let mut h = History::default();
        let t = Instant::now();
        let mut l1 = l0.clone();
        l1.add_cutout(Rect::new(0.0, 0.0, 0.5, 0.5), &f);
        h.record("New cutout", l0.clone(), l1.clone(), None, t);
        let mut l2 = l1.clone();
        l2.auto_arrange(1, &f, None);
        h.record("Auto-arrange", l1.clone(), l2.clone(), None, t);

        assert_eq!(h.undo_name(), Some("Auto-arrange"));
        assert_eq!(h.undo().unwrap().1, l1);
        assert_eq!(h.undo().unwrap().1, l0);
        assert!(h.undo().is_none());
        assert_eq!(h.redo_name(), Some("New cutout"));
        assert_eq!(h.redo().unwrap().1, l1);
        assert_eq!(h.redo().unwrap().1, l2);
    }

    #[test]
    fn a_new_edit_after_undo_discards_redo() {
        let (l0, f) = base();
        let mut h = History::default();
        let t = Instant::now();
        let mut l1 = l0.clone();
        l1.add_cutout(Rect::new(0.0, 0.0, 0.5, 0.5), &f);
        h.record("New cutout", l0.clone(), l1.clone(), None, t);
        h.undo();
        let mut l2 = l0.clone();
        l2.add_cutout(Rect::new(0.5, 0.5, 0.5, 0.5), &f);
        h.record("New cutout", l0, l2, None, t);
        assert!(h.redo().is_none());
    }

    #[test]
    fn nudges_within_a_second_merge() {
        let (mut l, f) = base();
        l.add_cutout(Rect::new(0.1, 0.1, 0.2, 0.2), &f);
        let mut h = History::default();
        let t = Instant::now();
        let mut prev = l.clone();
        for i in 0..5 {
            let mut next = prev.clone();
            next.move_cut(1, 0.001, 0.0);
            h.record("Nudge Cutout 1", prev.clone(), next.clone(), Some("nudge-cut-1".into()), t + Duration::from_millis(300 * i));
            prev = next;
        }
        assert_eq!(h.len(), 1);
        // A pause longer than a second starts a new step.
        let after_five = prev.clone();
        let mut next = prev.clone();
        next.move_cut(1, 0.001, 0.0);
        h.record("Nudge Cutout 1", prev, next, Some("nudge-cut-1".into()), t + Duration::from_millis(3000));
        assert_eq!(h.len(), 2);
        // Undo once: back to after the five merged nudges. Undo again: back to the start.
        assert_eq!(h.undo().unwrap().1, after_five);
        assert_eq!(h.undo().unwrap().1, l);
    }

    #[test]
    fn no_change_records_nothing_and_depth_is_capped() {
        let (l, f) = base();
        let mut h = History::default();
        let t = Instant::now();
        h.record("Nothing", l.clone(), l.clone(), None, t);
        assert_eq!(h.len(), 0);
        let mut prev = l;
        for i in 0..(DEPTH + 10) {
            let mut next = prev.clone();
            next.add_cutout(Rect::new(0.0, 0.0, 0.01, 0.01), &f);
            next.delete(1);
            next.add_cutout(Rect::new(i as f64 / 1000.0, 0.0, 0.01, 0.01), &f);
            h.record("Step", prev.clone(), next.clone(), None, t);
            prev = next;
        }
        assert_eq!(h.len(), DEPTH);
    }
}
