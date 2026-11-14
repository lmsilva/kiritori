//! One line of editable text, as every text box in Kiritori behaves: a caret, a selection,
//! Home/End, word jumps, clipboard and undo. Positions are char indices. Drawing and the real
//! clipboard live with the callers; this is the model, so it can be tested on its own.

use crate::shell::canvas::Mods;

const VK_BACK: u16 = 0x08;
const VK_END: u16 = 0x23;
const VK_HOME: u16 = 0x24;
const VK_LEFT: u16 = 0x25;
const VK_UP: u16 = 0x26;
const VK_RIGHT: u16 = 0x27;
const VK_DOWN: u16 = 0x28;
const VK_INSERT: u16 = 0x2D;
const VK_DELETE: u16 = 0x2E;

/// What a key did, beyond changing the text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The key was for the text box (and may have changed it).
    Handled,
    /// Put this on the clipboard (copy, or cut: the text is already removed).
    Copy(String),
    /// Insert the clipboard's text with `insert`.
    Paste,
    /// Not a text-editing key: Enter, Escape, Tab and the like are the caller's.
    Ignored,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextEdit {
    chars: Vec<char>,
    caret: usize,
    /// The other end of the selection; equal to `caret` when nothing is selected.
    anchor: usize,
    max: usize,
    undo: Vec<(Vec<char>, usize, usize)>,
    redo: Vec<(Vec<char>, usize, usize)>,
    /// The last change was typing: more typing joins the same undo step.
    typing: bool,
}

impl TextEdit {
    /// Start editing `text`, at most `max` characters, with the whole text selected (so typing
    /// replaces it, and the arrow keys or a click keep it).
    pub fn new(text: &str, max: usize) -> Self {
        let chars: Vec<char> = text.chars().take(max).collect();
        let n = chars.len();
        TextEdit { chars, caret: n, anchor: 0, max, undo: Vec::new(), redo: Vec::new(), typing: false }
    }

    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }

    pub fn len(&self) -> usize {
        self.chars.len()
    }

    pub fn caret(&self) -> usize {
        self.caret
    }

    /// The selected range, start before end; empty when start == end.
    pub fn selection(&self) -> (usize, usize) {
        (self.caret.min(self.anchor), self.caret.max(self.anchor))
    }

    pub fn has_selection(&self) -> bool {
        self.caret != self.anchor
    }

    /// The text before char index `i` (for measuring where the caret or a selection edge is).
    pub fn prefix(&self, i: usize) -> String {
        self.chars[..i.min(self.chars.len())].iter().collect()
    }

    pub fn select_all(&mut self) {
        self.anchor = 0;
        self.caret = self.chars.len();
    }

    /// Put the caret at `i`; with `extend`, keep the selection's other end where it is.
    pub fn set_caret(&mut self, i: usize, extend: bool) {
        self.caret = i.min(self.chars.len());
        if !extend {
            self.anchor = self.caret;
        }
        self.typing = false;
    }

    /// Select the word around char index `i` (double-click).
    pub fn select_word_at(&mut self, i: usize) {
        let i = i.min(self.chars.len());
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        let at_word = self.chars.get(i).is_some_and(|c| is_word(*c)) || (i > 0 && self.chars.get(i - 1).is_some_and(|c| is_word(*c)));
        if !at_word {
            self.anchor = i;
            self.caret = (i + 1).min(self.chars.len());
            return;
        }
        let mut a = i;
        while a > 0 && is_word(self.chars[a - 1]) {
            a -= 1;
        }
        let mut b = i;
        while b < self.chars.len() && is_word(self.chars[b]) {
            b += 1;
        }
        self.anchor = a;
        self.caret = b;
        self.typing = false;
    }

    fn snapshot(&mut self, typing: bool) {
        if !(typing && self.typing) {
            self.undo.push((self.chars.clone(), self.caret, self.anchor));
            if self.undo.len() > 100 {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
        self.typing = typing;
    }

    fn delete_selection(&mut self) -> bool {
        let (a, b) = self.selection();
        if a == b {
            return false;
        }
        self.chars.drain(a..b);
        self.caret = a;
        self.anchor = a;
        true
    }

    /// Replace the selection with `s` (paste, or a typed character). Line breaks and tabs
    /// become spaces, other control characters are dropped, and the text stops at `max`.
    pub fn insert(&mut self, s: &str) {
        let clean: Vec<char> = s.chars().map(|c| if c == '\n' || c == '\t' { ' ' } else { c }).filter(|c| !c.is_control()).collect();
        let (a, b) = self.selection();
        let room = self.max.saturating_sub(self.chars.len() - (b - a));
        let clean: Vec<char> = clean.into_iter().take(room).collect();
        if clean.is_empty() && a == b {
            return;
        }
        self.snapshot(clean.len() == 1 && a == b && !clean[0].is_whitespace());
        self.delete_selection();
        let at = self.caret;
        self.chars.splice(at..at, clean.iter().copied());
        self.caret = at + clean.len();
        self.anchor = self.caret;
    }

    /// A typed character (WM_CHAR). Control characters, which Windows also sends for
    /// Backspace and Ctrl+letter combinations, are left to `key`.
    pub fn char(&mut self, c: char) -> bool {
        if c.is_control() {
            return false;
        }
        self.insert(&c.to_string());
        true
    }

    fn word_left(&self, from: usize) -> usize {
        let mut i = from;
        while i > 0 && !self.chars[i - 1].is_alphanumeric() {
            i -= 1;
        }
        while i > 0 && self.chars[i - 1].is_alphanumeric() {
            i -= 1;
        }
        i
    }

    fn word_right(&self, from: usize) -> usize {
        let n = self.chars.len();
        let mut i = from;
        while i < n && self.chars[i].is_alphanumeric() {
            i += 1;
        }
        while i < n && !self.chars[i].is_alphanumeric() {
            i += 1;
        }
        i
    }

    fn restore(&mut self, s: (Vec<char>, usize, usize)) {
        self.chars = s.0;
        self.caret = s.1.min(self.chars.len());
        self.anchor = s.2.min(self.chars.len());
        self.typing = false;
    }

    /// A key press (WM_KEYDOWN).
    pub fn key(&mut self, vk: u16, m: Mods) -> Outcome {
        let n = self.chars.len();
        let letter = |c: u8| vk == c as u16 && m.ctrl && !m.alt;
        match vk {
            VK_LEFT | VK_RIGHT if !m.alt => {
                let left = vk == VK_LEFT;
                let (a, b) = self.selection();
                let to = if m.ctrl {
                    if left { self.word_left(self.caret) } else { self.word_right(self.caret) }
                } else if self.has_selection() && !m.shift {
                    // Collapse to the side moved towards.
                    if left { a } else { b }
                } else if left {
                    self.caret.saturating_sub(1)
                } else {
                    (self.caret + 1).min(n)
                };
                self.set_caret(to, m.shift);
                Outcome::Handled
            }
            VK_HOME | VK_UP if !m.alt => {
                self.set_caret(0, m.shift);
                Outcome::Handled
            }
            VK_END | VK_DOWN if !m.alt => {
                self.set_caret(n, m.shift);
                Outcome::Handled
            }
            VK_BACK | VK_DELETE if !(vk == VK_DELETE && m.shift) => {
                if self.has_selection() {
                    self.snapshot(false);
                    self.delete_selection();
                    return Outcome::Handled;
                }
                let back = vk == VK_BACK;
                let (a, b) = match (back, m.ctrl) {
                    (true, false) => (self.caret.saturating_sub(1), self.caret),
                    (true, true) => (self.word_left(self.caret), self.caret),
                    (false, false) => (self.caret, (self.caret + 1).min(n)),
                    (false, true) => (self.caret, self.word_right(self.caret)),
                };
                if a < b {
                    self.snapshot(false);
                    self.chars.drain(a..b);
                    self.caret = a;
                    self.anchor = a;
                }
                Outcome::Handled
            }
            _ if letter(b'A') => {
                self.select_all();
                Outcome::Handled
            }
            _ if letter(b'C') || (vk == VK_INSERT && m.ctrl && !m.shift) => {
                let (a, b) = self.selection();
                if a == b { Outcome::Handled } else { Outcome::Copy(self.chars[a..b].iter().collect()) }
            }
            _ if letter(b'X') || (vk == VK_DELETE && m.shift && !m.ctrl) => {
                let (a, b) = self.selection();
                if a == b {
                    return Outcome::Handled;
                }
                let cut: String = self.chars[a..b].iter().collect();
                self.snapshot(false);
                self.delete_selection();
                Outcome::Copy(cut)
            }
            _ if letter(b'V') || (vk == VK_INSERT && m.shift && !m.ctrl) => Outcome::Paste,
            _ if letter(b'Z') && !m.shift => {
                if let Some(s) = self.undo.pop() {
                    self.redo.push((self.chars.clone(), self.caret, self.anchor));
                    self.restore(s);
                }
                Outcome::Handled
            }
            _ if letter(b'Y') || (letter(b'Z') && m.shift) => {
                if let Some(s) = self.redo.pop() {
                    self.undo.push((self.chars.clone(), self.caret, self.anchor));
                    self.restore(s);
                }
                Outcome::Handled
            }
            // Other Ctrl combinations must not reach the caller's shortcuts while typing,
            // but Enter, Escape and Tab are the caller's to act on.
            0x0D | 0x1B | 0x09 => Outcome::Ignored,
            _ => Outcome::Handled,
        }
    }

    /// `key`, with copy and paste going through the Windows clipboard.
    pub fn key_with_clipboard(&mut self, vk: u16, m: Mods) -> Outcome {
        let out = self.key(vk, m);
        match &out {
            Outcome::Copy(s) => crate::shell::system::set_clipboard_text(s),
            Outcome::Paste => {
                if let Some(s) = crate::shell::system::clipboard_text() {
                    self.insert(&s);
                }
            }
            _ => {}
        }
        out
    }

    fn slice(&self, a: usize, b: usize) -> String {
        self.chars[a.min(self.chars.len())..b.min(self.chars.len())].iter().collect()
    }

    /// The char index nearest to `x` pixels from where char `first` is drawn, using `width`
    /// to measure (DirectWrite in the app, a fixed width in tests).
    pub fn index_at(&self, first: usize, x: f32, width: impl Fn(&str) -> f32) -> usize {
        let mut prev = 0.0;
        for i in first + 1..=self.chars.len() {
            let w = width(&self.slice(first, i));
            if x < (prev + w) / 2.0 {
                return i - 1;
            }
            prev = w;
        }
        self.chars.len()
    }

    /// The first char to draw in a box `room` pixels wide so the caret stays inside it,
    /// starting from the current `first`: scrolls only as far as needed, and back when the
    /// text gets shorter.
    pub fn first_visible(&self, first: usize, room: f32, width: impl Fn(&str) -> f32) -> usize {
        let mut f = first.min(self.caret);
        while f < self.caret && width(&self.slice(f, self.caret)) > room {
            f += 1;
        }
        while f > 0 && width(&self.slice(f - 1, self.chars.len())) <= room {
            f -= 1;
        }
        f
    }

    /// The chars from `first` that fit in `room` pixels: the end index.
    fn visible_end(&self, first: usize, room: f32, width: impl Fn(&str) -> f32) -> usize {
        let mut e = self.chars.len();
        while e > first && width(&self.slice(first, e)) > room {
            e -= 1;
        }
        e
    }

    /// Draw the text being edited in `r`, scrolled from char `first` (see `first_visible`):
    /// the selection behind it, the text, and the caret. Returns the draws.
    #[allow(clippy::too_many_arguments)]
    pub fn draws(&self, first: usize, r: crate::engine::draw::Px, size: f32, bold: bool, color: crate::engine::draw::Rgba, selection: crate::engine::draw::Rgba, caret: crate::engine::draw::Rgba, caret_w: f32) -> Vec<crate::engine::draw::Draw> {
        use crate::engine::draw::{Align, Draw};
        let w = |s: &str| crate::engine::render::measure(s, size, bold);
        let end = self.visible_end(first, r.2, w);
        let x_of = |i: usize| r.0 + w(&self.slice(first, i.clamp(first, end)));
        let pad = (r.3 - size * 1.45).max(0.0) / 2.0;
        let mut out = Vec::new();
        let (a, b) = self.selection();
        if a != b {
            let (x0, x1) = (x_of(a), x_of(b));
            if x1 > x0 {
                out.push(Draw::Fill { r: (x0, r.1 + pad, x1 - x0, r.3 - 2.0 * pad), color: selection, radius: 2.0 });
            }
        }
        out.push(Draw::Text { r: (r.0, r.1, r.2 + size, r.3), text: self.slice(first, end), size, color, bold, align: Align::Left });
        let cx = x_of(self.caret);
        out.push(Draw::Line { a: (cx, r.1 + pad), b: (cx, r.1 + r.3 - pad), color: caret, width: caret_w });
        out
    }
}

/// A text box being typed in on a canvas: the text, how far it is scrolled, where it was last
/// drawn (so the mouse can place the caret) and a drag selecting text. Drawing takes `&self`,
/// so the drawn place and the scroll are kept in cells.
#[derive(Clone, Debug)]
pub struct Typing {
    pub edit: TextEdit,
    first: std::cell::Cell<usize>,
    drawn: std::cell::Cell<Option<(crate::engine::draw::Px, f32, bool)>>,
    drag: bool,
    last_click: Option<(std::time::Instant, f32, f32)>,
}

impl Typing {
    /// Start typing over `text` (all of it selected), at most `max` characters.
    pub fn new(text: &str, max: usize) -> Self {
        Typing { edit: TextEdit::new(text, max), first: std::cell::Cell::new(0), drawn: std::cell::Cell::new(None), drag: false, last_click: None }
    }

    pub fn text(&self) -> String {
        self.edit.text()
    }

    /// Where the text was last drawn.
    #[cfg(test)]
    pub fn drawn_rect(&self) -> Option<crate::engine::draw::Px> {
        self.drawn.get().map(|(r, ..)| r)
    }

    fn index_at(&self, x: f32) -> Option<usize> {
        let (r, size, bold) = self.drawn.get()?;
        Some(self.edit.index_at(self.first.get(), x - r.0, |s| crate::engine::render::measure(s, size, bold)))
    }

    /// A press at `x`, `y`: inside the text, it places the caret (Shift extends the selection,
    /// a double-click selects a word, a drag selects). False when it was not on the text.
    pub fn mouse_down(&mut self, x: f32, y: f32, shift: bool) -> bool {
        let Some((r, ..)) = self.drawn.get() else { return false };
        // A little slack around the text, so a click just past its end still counts.
        if !(x >= r.0 - 6.0 && x < r.0 + r.2 + 12.0 && y >= r.1 && y < r.1 + r.3) {
            return false;
        }
        let Some(i) = self.index_at(x) else { return false };
        let double = self.last_click.is_some_and(|(t, px, py)| t.elapsed() < std::time::Duration::from_millis(450) && (px - x).abs() < 4.0 && (py - y).abs() < 4.0);
        if double {
            self.edit.select_word_at(i);
            self.last_click = None;
        } else {
            self.edit.set_caret(i, shift);
            self.drag = true;
            self.last_click = Some((std::time::Instant::now(), x, y));
        }
        true
    }

    /// The pointer moved: while dragging, extend the selection. True when it was dragging.
    pub fn mouse_move(&mut self, x: f32) -> bool {
        if !self.drag {
            return false;
        }
        if let Some(i) = self.index_at(x) {
            self.edit.set_caret(i, true);
        }
        true
    }

    /// The button came up: ends a drag. True when there was one.
    pub fn mouse_up(&mut self) -> bool {
        std::mem::take(&mut self.drag)
    }

    /// Draw in `r` (selection, text, caret), scrolled so the caret is in view.
    #[allow(clippy::too_many_arguments)]
    pub fn draws(&self, r: crate::engine::draw::Px, size: f32, bold: bool, color: crate::engine::draw::Rgba, selection: crate::engine::draw::Rgba, caret: crate::engine::draw::Rgba, caret_w: f32) -> Vec<crate::engine::draw::Draw> {
        let first = self.edit.first_visible(self.first.get(), r.2, |s| crate::engine::render::measure(s, size, bold));
        self.first.set(first);
        self.drawn.set(Some((r, size, bold)));
        self.edit.draws(first, r, size, bold, color, selection, caret, caret_w)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m() -> Mods {
        Mods::default()
    }
    fn ctrl() -> Mods {
        Mods { ctrl: true, ..Mods::default() }
    }
    fn shift() -> Mods {
        Mods { shift: true, ..Mods::default() }
    }
    fn typed(t: &mut TextEdit, s: &str) {
        for c in s.chars() {
            t.char(c);
        }
    }

    #[test]
    fn starts_all_selected_and_typing_replaces_it() {
        let mut t = TextEdit::new("Main view", 40);
        assert_eq!(t.selection(), (0, 9));
        typed(&mut t, "Cab");
        assert_eq!(t.text(), "Cab");
        assert_eq!(t.caret(), 3);
    }

    #[test]
    fn arrows_home_end_and_inserting_in_the_middle() {
        let mut t = TextEdit::new("Main view", 40);
        assert_eq!(t.key(VK_LEFT, m()), Outcome::Handled);
        assert_eq!((t.caret(), t.has_selection()), (0, false), "Left collapses the selection to its start");
        t.key(VK_END, m());
        assert_eq!(t.caret(), 9);
        t.key(VK_HOME, m());
        t.key(VK_RIGHT, m());
        t.key(VK_RIGHT, m());
        typed(&mut t, "X");
        assert_eq!(t.text(), "MaXin view");
        t.key(VK_LEFT, ctrl());
        assert_eq!(t.caret(), 0, "Ctrl+Left goes to the start of the word");
        t.key(VK_RIGHT, ctrl());
        assert_eq!(t.caret(), 6, "Ctrl+Right goes to the start of the next word");
    }

    #[test]
    fn shift_selects_and_typing_or_backspace_replaces_the_selection() {
        let mut t = TextEdit::new("Main view", 40);
        t.key(VK_END, m());
        t.key(VK_LEFT, Mods { ctrl: true, shift: true, alt: false });
        assert_eq!(t.selection(), (5, 9));
        typed(&mut t, "cab");
        assert_eq!(t.text(), "Main cab");
        t.key(VK_HOME, shift());
        assert_eq!(t.selection(), (0, 8));
        t.key(VK_BACK, m());
        assert_eq!(t.text(), "");
    }

    #[test]
    fn backspace_and_delete_by_char_and_by_word() {
        let mut t = TextEdit::new("one two three", 40);
        t.key(VK_END, m());
        t.key(VK_BACK, m());
        assert_eq!(t.text(), "one two thre");
        t.key(VK_BACK, ctrl());
        assert_eq!(t.text(), "one two ");
        t.key(VK_HOME, m());
        t.key(VK_DELETE, m());
        assert_eq!(t.text(), "ne two ");
        t.key(VK_DELETE, ctrl());
        assert_eq!(t.text(), "two ");
    }

    #[test]
    fn copy_cut_paste() {
        let mut t = TextEdit::new("Main view", 40);
        assert_eq!(t.key(b'C' as u16, ctrl()), Outcome::Copy("Main view".into()));
        t.key(VK_HOME, m());
        t.key(VK_END, shift());
        t.key(VK_HOME, m());
        t.key(VK_RIGHT, shift());
        t.key(VK_RIGHT, shift());
        assert_eq!(t.key(b'X' as u16, ctrl()), Outcome::Copy("Ma".into()));
        assert_eq!(t.text(), "in view");
        assert_eq!(t.key(b'V' as u16, ctrl()), Outcome::Paste);
        t.insert("Ma");
        assert_eq!(t.text(), "Main view");
        assert_eq!(t.key(VK_INSERT, shift()), Outcome::Paste, "Shift+Insert pastes too");
        assert_eq!(t.key(VK_INSERT, ctrl()), Outcome::Handled, "Ctrl+Insert with nothing selected copies nothing");
        assert_eq!(t.key(b'A' as u16, ctrl()), Outcome::Handled);
        assert_eq!(t.key(VK_DELETE, shift()), Outcome::Copy("Main view".into()), "Shift+Delete cuts");
        assert_eq!(t.text(), "");
    }

    #[test]
    fn paste_flattens_lines_and_respects_the_limit() {
        let mut t = TextEdit::new("", 10);
        t.insert("line one\r\nline two");
        assert_eq!(t.text(), "line one l");
        assert_eq!(t.caret(), 10);
        t.insert("more");
        assert_eq!(t.text(), "line one l", "full: nothing more fits");
        typed(&mut t, "z");
        assert_eq!(t.len(), 10);
    }

    #[test]
    fn undo_and_redo_group_typing() {
        let mut t = TextEdit::new("Main", 40);
        t.key(VK_END, m());
        typed(&mut t, " view");
        assert_eq!(t.text(), "Main view");
        t.key(b'Z' as u16, ctrl());
        assert_eq!(t.text(), "Main ", "a typed word undoes as one step");
        t.key(b'Z' as u16, ctrl());
        assert_eq!(t.text(), "Main");
        t.key(b'Y' as u16, ctrl());
        assert_eq!(t.text(), "Main ");
        t.key(b'Z' as u16, Mods { ctrl: true, shift: true, alt: false });
        assert_eq!(t.text(), "Main view", "Ctrl+Shift+Z redoes");
    }

    #[test]
    fn control_chars_and_caller_keys_are_left_alone() {
        let mut t = TextEdit::new("ab", 40);
        t.key(VK_END, m());
        // What WM_CHAR sends for Backspace, Ctrl+A, Ctrl+C, Ctrl+V, Ctrl+Backspace.
        for c in ['\u{8}', '\u{1}', '\u{3}', '\u{16}', '\u{7f}', '\r', '\u{1b}'] {
            assert!(!t.char(c));
        }
        assert_eq!(t.text(), "ab");
        for vk in [0x0D, 0x1B, 0x09] {
            assert_eq!(t.key(vk, m()), Outcome::Ignored);
        }
        assert_eq!(t.key(b'H' as u16, m()), Outcome::Handled, "letters never reach the caller's shortcuts");
    }

    #[test]
    fn mouse_positions_words_and_scrolling() {
        let w = |s: &str| s.chars().count() as f32 * 10.0;
        let mut t = TextEdit::new("one two three", 40);
        assert_eq!(t.index_at(0, 0.0, w), 0);
        assert_eq!(t.index_at(0, 14.0, w), 1);
        assert_eq!(t.index_at(0, 16.0, w), 2);
        assert_eq!(t.index_at(0, 999.0, w), 13);
        assert_eq!(t.index_at(5, 16.0, w), 7, "scrolled: measured from the first char shown");
        t.select_word_at(5);
        assert_eq!(t.selection(), (4, 7));
        t.set_caret(2, false);
        t.set_caret(9, true);
        assert_eq!(t.selection(), (2, 9));
        // 13 chars of 10 px in a 50 px box: with the caret at the end, chars 8.. are shown.
        t.set_caret(13, false);
        assert_eq!(t.first_visible(0, 50.0, w), 8);
        t.set_caret(0, false);
        assert_eq!(t.first_visible(8, 50.0, w), 0);
        t.set_caret(10, false);
        assert_eq!(t.first_visible(6, 50.0, w), 6, "visible caret: no jump");
        // Text that fits is never scrolled.
        let short = TextEdit::new("abc", 40);
        assert_eq!(short.first_visible(2, 50.0, w), 0);
    }
}
