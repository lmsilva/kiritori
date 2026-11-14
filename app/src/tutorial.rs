//! The first-run tutorial: one card per step of the basic loop, each naming
//! its button and shortcut and completing when the user actually does it. Skippable, resumable
//! across layer sessions, replayable from the tray.

use crate::layer::{Action, View};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Draw,
    Name,
    Hide,
    Arrange,
    AutoArrange,
    Peek,
    Close,
}

impl Step {
    pub const ALL: [Step; 7] = [Step::Draw, Step::Name, Step::Hide, Step::Arrange, Step::AutoArrange, Step::Peek, Step::Close];

    pub fn title(self) -> &'static str {
        match self {
            Step::Draw => "Cut out a part you want to keep",
            Step::Name => "Give it a name",
            Step::Hide => "Cut a part you don't want, then hide it",
            Step::Arrange => "Switch to Arrange",
            Step::AutoArrange => "Let Kiritori arrange it",
            Step::Peek => "Peek at the result",
            Step::Close => "Close the layer",
        }
    }

    pub fn body(self) -> &'static str {
        match self {
            Step::Draw => "Drag across it on the picture. Edges snap to the halves and thirds.",
            Step::Name => "Type a name and press Enter, or press Esc to keep the one it has.",
            Step::Hide => "Hidden parts stay in the list and come back with H. Skip this if you need everything.",
            Step::Arrange => "Arrange shows the screen as it will look. Tab switches back to Cut any time.",
            Step::AutoArrange => "Your first cutout gets the most room. Drag or resize afterwards if you like.",
            Step::Peek => "Hold Space to hide every control and see the result.",
            Step::Close => "Everything is already saved. Ctrl+Alt+K brings the layer back any time.",
        }
    }

    /// The toolbar button to ring, if the step has one.
    pub fn target(self) -> Option<Action> {
        match self {
            Step::Hide => Some(Action::Hide),
            Step::Arrange => Some(Action::Arrange),
            Step::AutoArrange => Some(Action::AutoArrange),
            Step::Close => Some(Action::Close),
            _ => None,
        }
    }

    /// The shortcut shown on the card.
    pub fn key(self) -> &'static str {
        match self {
            Step::Draw => "Drag",
            Step::Name => "Enter",
            Step::Hide => "H",
            Step::Arrange => "Tab",
            Step::AutoArrange => "A",
            Step::Peek => "hold Space",
            Step::Close => "Esc",
        }
    }
}

/// What happened, as far as the tutorial cares.
#[derive(Clone, Debug, PartialEq)]
pub enum Seen<'a> {
    /// An undoable step with this name.
    Edit(&'a str),
    /// The layer's state after an input.
    Layer { view: View, naming: bool, peek: bool },
    LayerClosed,
}

#[derive(Clone, Debug, Default)]
pub struct Tutorial {
    /// Index into `Step::ALL`; equal to its length when finished.
    pub index: usize,
    /// A name was being typed during the Name step; it completes when typing ends.
    saw_naming: bool,
}

impl Tutorial {
    pub fn step(&self) -> Option<Step> {
        Step::ALL.get(self.index).copied()
    }

    pub fn finished(&self) -> bool {
        self.index >= Step::ALL.len()
    }

    pub fn skip(&mut self) {
        self.index = (self.index + 1).min(Step::ALL.len());
        self.saw_naming = false;
    }

    /// Advance if what happened completes the current step. Returns true if it advanced.
    pub fn observe(&mut self, seen: &Seen) -> bool {
        let Some(step) = self.step() else { return false };
        if step == Step::Name && matches!(seen, Seen::Layer { naming: true, .. }) {
            self.saw_naming = true;
        }
        let done = match (step, seen) {
            (Step::Draw, Seen::Edit(n)) => n.starts_with("New cutout") || n.starts_with("Add grid"),
            // Naming ends with Enter, Esc or a click elsewhere; any of them completes the step,
            // but only once a name was actually being typed (not merely "not typing").
            (Step::Name, Seen::Layer { naming: false, .. }) => self.saw_naming,
            (Step::Name, Seen::Edit(n)) => n.starts_with("Rename"),
            (Step::Hide, Seen::Edit(n)) => n.starts_with("Hide"),
            (Step::Arrange, Seen::Layer { view: View::Arrange, .. }) => true,
            (Step::AutoArrange, Seen::Edit(n)) => ["Auto-arrange", "Fill", "Move", "Resize"].iter().any(|p| n.starts_with(p)),
            (Step::Peek, Seen::Layer { peek: true, .. }) => true,
            (Step::Close, Seen::LayerClosed) => true,
            _ => false,
        };
        if done {
            self.index += 1;
            self.saw_naming = false;
        }
        done
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer(view: View, naming: bool, peek: bool) -> Seen<'static> {
        Seen::Layer { view, naming, peek }
    }

    #[test]
    fn the_basic_loop_walks_every_step() {
        let mut t = Tutorial::default();
        assert_eq!(t.step(), Some(Step::Draw));
        assert!(!t.observe(&layer(View::Cut, false, false)), "nothing yet");
        assert!(t.observe(&Seen::Edit("New cutout 1")));
        assert!(!t.observe(&layer(View::Cut, true, false)), "still typing the name");
        assert!(t.observe(&layer(View::Cut, false, false)), "name confirmed");
        assert!(t.observe(&Seen::Edit("Hide Score")));
        assert!(t.observe(&layer(View::Arrange, false, false)));
        assert!(t.observe(&Seen::Edit("Auto-arrange")));
        assert!(t.observe(&layer(View::Arrange, false, true)));
        assert!(!t.finished());
        assert!(t.observe(&Seen::LayerClosed));
        assert!(t.finished());
        assert_eq!(t.step(), None);
    }

    #[test]
    fn steps_only_complete_in_order_and_can_be_skipped() {
        let mut t = Tutorial::default();
        assert!(!t.observe(&Seen::Edit("Hide Score")), "hiding before drawing does not count");
        t.skip();
        t.skip();
        assert_eq!(t.step(), Some(Step::Hide));
        t.skip();
        assert_eq!(t.step(), Some(Step::Arrange));
    }

    #[test]
    fn the_name_step_waits_for_a_name_to_be_typed() {
        let mut t = Tutorial { index: 1, ..Default::default() };
        assert!(!t.observe(&layer(View::Cut, false, false)), "not typing is not the same as finished typing");
        assert!(!t.observe(&layer(View::Cut, true, false)));
        assert!(t.observe(&layer(View::Cut, false, false)));
        let mut t = Tutorial { index: 1, ..Default::default() };
        assert!(t.observe(&Seen::Edit("Rename Cutout 1 to Main view")), "a rename from the list counts too");
    }

    #[test]
    fn dragging_also_counts_as_arranging() {
        let mut t = Tutorial { index: 4, ..Default::default() };
        assert!(t.observe(&Seen::Edit("Move Controls")));
    }

    #[test]
    fn every_step_says_what_to_press() {
        for s in Step::ALL {
            assert!(!s.title().is_empty() && !s.body().is_empty() && !s.key().is_empty());
        }
    }
}
