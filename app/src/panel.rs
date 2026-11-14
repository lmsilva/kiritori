//! The Profiles panel: every profile with its layout count and
//! auto-apply setting; for the selected one its name, match rule with a live "Matches now" line,
//! auto-apply, title bar removal, layouts (default, rename, duplicate, delete, new), export,
//! import, restore previous session and delete.
//!
//! The panel lives in its own window so it opens from the tray even when no app is in front.
//! It keeps only what it shows and what is being typed; every change comes back as an [`Op`]
//! for the app to carry out and save.

use crate::engine::draw::{Align, Draw, Px, Rgba};
use crate::engine::render::measure;
use crate::layer::style::*;
use crate::model::profile::{AutoApply, FrameRemoval, Profile};
use crate::shell::canvas::{Button, Input};
use crate::textedit::TextEdit;
use std::time::{Duration, Instant};

/// The panel's size in 96-dpi units.
pub const SIZE: (f32, f32) = (940.0, 800.0);

#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    None,
    Close,
    Rename(usize, String),
    TitleRule(usize, String),
    AutoApply(usize, AutoApply),
    Frame(usize, FrameRemoval),
    KeepUnfocused(usize, bool),
    Pointer(usize, crate::model::profile::Pointer),
    SetDefault(usize, String),
    RenameLayout(usize, String, String),
    DuplicateLayout(usize, String),
    DeleteLayout(usize, String),
    NewLayout(usize),
    DeleteProfile(usize),
    /// Make a profile for one of the open windows (the shell asks which).
    NewProfile,
    RestorePrevious(usize),
    Export(usize),
    Import,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Field {
    Name,
    Title,
    Layout(String),
}

#[derive(Clone, Debug, PartialEq)]
enum Hit {
    Close,
    Row(usize),
    Edit(Field),
    AutoApply(AutoApply),
    Frame(FrameRemoval),
    KeepUnfocused,
    Pointer(crate::model::profile::Pointer),
    Star(String),
    RenameLayout(String),
    Duplicate(String),
    DeleteLayout(String),
    NewLayout,
    Export,
    Import,
    Restore,
    DeleteProfile,
    NewProfile,
    /// The trash button on a row of the list.
    DeleteRow(usize),
}

/// What one profile looks like to the panel.
pub struct Entry<'a> {
    pub profile: &'a Profile,
    /// A previous-session backup exists.
    pub has_backup: bool,
}

#[derive(Default)]
pub struct ProfilesPanel {
    pub selected: usize,
    /// The field being typed in, and its text.
    pub editing: Option<(Field, TextEdit)>,
    /// The first character shown in the field being typed in (long text scrolls).
    edit_first: usize,
    /// Where the typed text is drawn, and its font size and weight, for mouse positioning.
    edit_text: Option<(Px, f32, bool)>,
    /// A drag selecting text in the field being typed in.
    text_drag: bool,
    /// The last click in the field being typed in, for double-click word selection.
    last_text_click: Option<(Instant, f32, f32)>,
    /// A delete waiting for a second click: the profile, or a layout id.
    confirm: Option<Hit>,
    hits: Vec<(Px, Hit)>,
    hover: Option<Hit>,
    /// Window titles the selected profile matches now.
    pub matches: Vec<String>,
    /// The first profile row shown, when there are more than fit.
    scroll: usize,
    /// A line at the bottom: what just happened ("Exported to …").
    pub status: Option<String>,
}

fn inside(r: &Px, x: f32, y: f32) -> bool {
    x >= r.0 && x < r.0 + r.2 && y >= r.1 && y < r.1 + r.3
}

/// `text`, cut with an ellipsis to fit `w` pixels.
fn fit(text: &str, size: f32, bold: bool, w: f32) -> String {
    if measure(text, size, bold) <= w {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut n = chars.len();
    while n > 0 {
        n -= 1;
        let s: String = chars[..n].iter().collect::<String>() + "…";
        if measure(&s, size, bold) <= w {
            return s;
        }
    }
    "…".into()
}

pub fn pointer_label(p: crate::model::profile::Pointer) -> &'static str {
    use crate::model::profile::Pointer;
    match p {
        Pointer::Translate => "Translate",
        Pointer::Relative => "Relative",
        Pointer::Off => "Off",
    }
}

pub fn auto_apply_label(a: AutoApply) -> &'static str {
    match a {
        AutoApply::Auto => "Automatically",
        AutoApply::Ask => "Ask",
        AutoApply::Off => "Off",
    }
}

/// A stable colour per profile for its initial badge (the app's own icon comes later).
fn badge(i: usize) -> Rgba {
    use crate::model::layout::CutoutColor;
    cutout(CutoutColor::ORDER[i % CutoutColor::ORDER.len()])
}

const ROW_H: f32 = 58.0;
/// The longest name or title rule.
const MAX_TEXT: usize = 60;

/// The font a field's text uses: the profile name is larger and bold.
fn field_font(f: &Field, k: f32) -> (f32, bool) {
    if *f == Field::Name { (16.0 * k, true) } else { (13.0 * k, false) }
}
const LIST_TOP: f32 = 72.0;
const LIST_W: f32 = 300.0;
/// Where the rows start, under the New profile button.
const ROWS_TOP: f32 = LIST_TOP + 52.0;

impl ProfilesPanel {
    pub fn new(selected: usize) -> Self {
        ProfilesPanel { selected, ..Default::default() }
    }

    fn rows_that_fit(&self) -> usize {
        ((SIZE.1 - ROWS_TOP - 20.0) / ROW_H) as usize
    }

    /// Select profile `i` of `count` (a new profile, say), scrolling it into view.
    pub fn select_index(&mut self, i: usize, count: usize) {
        self.select(i, count);
    }

    fn select(&mut self, i: usize, count: usize) {
        if count == 0 {
            return;
        }
        self.selected = i.min(count - 1);
        self.editing = None;
        self.confirm = None;
        let fit = self.rows_that_fit();
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + fit {
            self.scroll = self.selected + 1 - fit;
        }
    }

    /// Start typing in field `f` with `text`. A click at `x` in the field `field_rect` puts the
    /// caret there; without one (F2) the whole text is selected, so typing replaces it.
    fn start_editing(&mut self, f: Field, text: &str, click: Option<(f32, Px)>, k: f32) {
        let mut te = TextEdit::new(text, MAX_TEXT);
        if let Some((x, r)) = click {
            let (size, bold) = field_font(&f, k);
            te.set_caret(te.index_at(0, x - (r.0 + 10.0 * k), |s| measure(s, size, bold)), false);
            self.text_drag = true;
        }
        self.edit_first = 0;
        self.edit_text = None;
        self.editing = Some((f, te));
    }

    /// The char index under `x` in the field being typed in.
    fn text_index(&self, x: f32) -> Option<usize> {
        let ((_, te), (r, size, bold)) = (self.editing.as_ref()?, self.edit_text?);
        Some(te.index_at(self.edit_first, x - r.0, |s| measure(s, size, bold)))
    }

    /// Commit what is being typed.
    fn commit(&mut self) -> Op {
        let Some((field, te)) = self.editing.take() else { return Op::None };
        self.text_drag = false;
        let text = te.text();
        let i = self.selected;
        match field {
            Field::Name if !text.trim().is_empty() => Op::Rename(i, text.trim().to_string()),
            Field::Title => Op::TitleRule(i, text.trim().to_string()),
            Field::Layout(id) if !text.trim().is_empty() => Op::RenameLayout(i, id, text.trim().to_string()),
            _ => Op::None,
        }
    }

    /// Handle one input; `k` is the chrome scale, `entries` the profiles in order.
    pub fn input(&mut self, input: Input, k: f32, entries: &[Entry]) -> Op {
        let count = entries.len();
        match input {
            Input::Move(x, y) => {
                self.hover = self.hits.iter().find(|(r, _)| inside(r, x, y)).map(|(_, h)| h.clone());
                if self.text_drag
                    && let Some(i) = self.text_index(x)
                    && let Some((_, te)) = self.editing.as_mut()
                {
                    te.set_caret(i, true);
                }
                Op::None
            }
            Input::Up(Button::Left, ..) => {
                self.text_drag = false;
                Op::None
            }
            Input::Down(Button::Left, x, y, m) => {
                let hit_at = self.hits.iter().find(|(r, _)| inside(r, x, y)).cloned();
                let hit = hit_at.as_ref().map(|(_, h)| h.clone());
                // A click in the field being typed in moves the caret (Shift extends the
                // selection, a double-click selects a word, a drag selects).
                if matches!((&self.editing, &hit), (Some((f, _)), Some(Hit::Edit(g))) if f == g) {
                    let i = self.text_index(x);
                    let double = self.last_text_click.is_some_and(|(t, px, py)| t.elapsed() < Duration::from_millis(450) && (px - x).abs() < 4.0 && (py - y).abs() < 4.0);
                    if let (Some(i), Some((_, te))) = (i, self.editing.as_mut()) {
                        if double {
                            te.select_word_at(i);
                        } else {
                            te.set_caret(i, m.shift);
                            self.text_drag = true;
                        }
                    }
                    self.last_text_click = if double { None } else { Some((Instant::now(), x, y)) };
                    return Op::None;
                }
                // A click anywhere else finishes typing first.
                let typed = match (&self.editing, &hit) {
                    (Some(_), _) => self.commit(),
                    _ => Op::None,
                };
                let confirm = self.confirm.take();
                let op = match hit {
                    None => Op::None,
                    Some(Hit::Close) => Op::Close,
                    Some(Hit::Row(i)) => {
                        self.select(i, count);
                        Op::None
                    }
                    Some(Hit::Edit(f)) => {
                        if let Some(e) = entries.get(self.selected) {
                            let text = match &f {
                                Field::Name => e.profile.name.clone(),
                                Field::Title => e.profile.match_rule.title_contains.clone(),
                                Field::Layout(id) => e.profile.layout(id).map(|l| l.name.clone()).unwrap_or_default(),
                            };
                            self.start_editing(f, &text, hit_at.map(|(r, _)| (x, r)), k);
                            self.last_text_click = Some((Instant::now(), x, y));
                        }
                        Op::None
                    }
                    Some(Hit::RenameLayout(id)) => {
                        let name = entries.get(self.selected).and_then(|e| e.profile.layout(&id)).map(|l| l.name.clone()).unwrap_or_default();
                        self.start_editing(Field::Layout(id), &name, None, k);
                        Op::None
                    }
                    Some(Hit::AutoApply(a)) => Op::AutoApply(self.selected, a),
                    Some(Hit::Frame(f)) => Op::Frame(self.selected, f),
                    Some(Hit::KeepUnfocused) => Op::KeepUnfocused(self.selected, !entries.get(self.selected).is_some_and(|e| e.profile.keep_when_unfocused)),
                    Some(Hit::Star(id)) => Op::SetDefault(self.selected, id),
                    Some(Hit::Pointer(p)) => Op::Pointer(self.selected, p),
                    Some(Hit::Duplicate(id)) => Op::DuplicateLayout(self.selected, id),
                    Some(Hit::NewLayout) => Op::NewLayout(self.selected),
                    Some(Hit::Export) => Op::Export(self.selected),
                    Some(Hit::Import) => Op::Import,
                    Some(Hit::Restore) => Op::RestorePrevious(self.selected),
                    Some(Hit::NewProfile) => Op::NewProfile,
                    Some(h @ (Hit::DeleteLayout(_) | Hit::DeleteProfile | Hit::DeleteRow(_))) => {
                        // Deleting takes a second click on the same button.
                        if let Hit::DeleteRow(i) = h {
                            self.select(i, count);
                        }
                        if confirm.as_ref() == Some(&h) {
                            match h {
                                Hit::DeleteLayout(id) => Op::DeleteLayout(self.selected, id),
                                Hit::DeleteRow(i) => Op::DeleteProfile(i),
                                _ => Op::DeleteProfile(self.selected),
                            }
                        } else {
                            self.confirm = Some(h);
                            Op::None
                        }
                    }
                };
                if op == Op::None { typed } else { op }
            }
            Input::Key(vk, mods) => {
                if let Some((_, te)) = self.editing.as_mut() {
                    match vk {
                        0x0D => return self.commit(),
                        0x1B => {
                            self.editing = None;
                            self.text_drag = false;
                        }
                        _ => {
                            te.key_with_clipboard(vk, mods);
                        }
                    }
                    return Op::None;
                }
                // A delete waiting for its second press: Esc keeps the profile.
                if vk == 0x1B && self.confirm.take().is_some() {
                    return Op::None;
                }
                match vk {
                    0x1B | 0x50 => Op::Close, // Esc, P
                    0x2D => Op::NewProfile, // Insert
                    0x4E if mods.ctrl => Op::NewProfile,
                    0x2E if count > 0 => {
                        // Delete: once to ask, again to delete.
                        let h = Hit::DeleteRow(self.selected);
                        if self.confirm.as_ref() == Some(&h) {
                            self.confirm = None;
                            Op::DeleteProfile(self.selected)
                        } else {
                            self.confirm = Some(h);
                            Op::None
                        }
                    }
                    0x26 => {
                        self.select(self.selected.saturating_sub(1), count);
                        Op::None
                    }
                    0x28 => {
                        self.select(self.selected + 1, count);
                        Op::None
                    }
                    0x71 => {
                        // F2: rename
                        if let Some(e) = entries.get(self.selected) {
                            let name = e.profile.name.clone();
                            self.start_editing(Field::Name, &name, None, k);
                        }
                        Op::None
                    }
                    _ => Op::None,
                }
            }
            Input::Char(c) => {
                if let Some((_, te)) = self.editing.as_mut() {
                    te.char(c);
                }
                Op::None
            }
            Input::LostFocus => {
                self.confirm = None;
                Op::None
            }
            _ => Op::None,
        }
    }

    /// Draw the panel filling a window of `w`×`h` pixels at chrome scale `k`.
    pub fn draw(&mut self, entries: &[Entry], k: f32, w: f32, h: f32) -> Vec<Draw> {
        let mut d = Vec::new();
        self.hits.clear();
        let s = |v: f32| v * k;
        let solid = |mut c: Rgba| {
            c[3] = 1.0;
            c
        };
        d.push(Draw::Fill { r: (0.0, 0.0, w, h), color: solid(glass()), radius: 0.0 });
        d.push(Draw::Stroke { r: (0.5, 0.5, w - 1.0, h - 1.0), color: hairline(), width: 1.0, radius: 0.0, dashed: false });
        d.push(Draw::Text { r: (s(24.0), s(16.0), s(400.0), s(36.0)), text: "Profiles".into(), size: s(20.0), color: fg(), bold: true, align: Align::Left });
        let hint = "↑↓ choose · Ctrl+N new · Del delete · F2 rename · Esc close";
        d.push(Draw::Text { r: (w - s(600.0), s(16.0), s(530.0), s(36.0)), text: hint.into(), size: s(12.0), color: fg_muted(), bold: false, align: Align::Right });
        let close = (w - s(52.0), s(16.0), s(36.0), s(36.0));
        self.button_bg(&mut d, close, &Hit::Close, s(8.0));
        d.push(Draw::Icon { name: "x", r: (close.0 + s(8.0), close.1 + s(8.0), s(20.0), s(20.0)), color: fg(), stroke: s(2.0) });
        self.hits.push((close, Hit::Close));

        // Profile list.
        d.push(Draw::Line { a: (s(LIST_W + 16.0), s(LIST_TOP)), b: (s(LIST_W + 16.0), h - s(48.0)), color: hairline(), width: 1.0 });
        // New profile: the first thing in the list column.
        let new_r = (s(12.0), s(LIST_TOP), s(LIST_W - 4.0), s(40.0));
        self.text_button(&mut d, new_r, "New profile…", Some("plus"), Hit::NewProfile, k, fg());
        if entries.is_empty() {
            let y0 = ROWS_TOP + 8.0;
            d.push(Draw::Text { r: (s(24.0), s(y0), s(LIST_W - 16.0), s(30.0)), text: "No profiles yet".into(), size: s(15.0), color: fg(), bold: true, align: Align::Left });
            for (j, line) in ["New profile… makes one for any open", "window. Or press Ctrl+Alt+K over an", "app, or import a profile file."].iter().enumerate() {
                d.push(Draw::Text { r: (s(24.0), s(y0 + 32.0 + 22.0 * j as f32), s(LIST_W - 16.0), s(22.0)), text: (*line).into(), size: s(13.0), color: fg_muted(), bold: false, align: Align::Left });
            }
            let imp = (s(24.0), s(y0 + 116.0), s(150.0), s(36.0));
            self.text_button(&mut d, imp, "Import…", Some("upload"), Hit::Import, k, fg());
            return d;
        }
        self.selected = self.selected.min(entries.len() - 1);
        let fit_rows = self.rows_that_fit();
        for (row, (i, e)) in entries.iter().enumerate().skip(self.scroll).take(fit_rows).enumerate() {
            let r = (s(12.0), s(ROWS_TOP) + row as f32 * s(ROW_H), s(LIST_W - 4.0), s(ROW_H - 4.0));
            let hovered = matches!(self.hover, Some(Hit::Row(j) | Hit::DeleteRow(j)) if j == i);
            if i == self.selected {
                d.push(Draw::Fill { r, color: accent_fill(), radius: s(8.0) });
            } else if hovered {
                d.push(Draw::Fill { r, color: glass_hover(), radius: s(8.0) });
            }
            // The row's own delete button: a trash can, then "Delete?" once pressed.
            let asking = self.confirm == Some(Hit::DeleteRow(i));
            let trash_w = if asking { measure("Delete?", s(13.0), true) + s(44.0) } else { s(34.0) };
            let show_trash = asking || i == self.selected || hovered;
            if show_trash {
                let t = (r.0 + r.2 - trash_w - s(10.0), r.1 + (r.3 - s(34.0)) / 2.0, trash_w, s(34.0));
                if asking {
                    d.push(Draw::Fill { r: t, color: danger(), radius: s(8.0) });
                    d.push(Draw::Icon { name: "trash-2", r: (t.0 + s(10.0), t.1 + s(8.0), s(18.0), s(18.0)), color: [1.0; 4], stroke: s(1.8) });
                    d.push(Draw::Text { r: (t.0 + s(34.0), t.1, t.2 - s(38.0), t.3), text: "Delete?".into(), size: s(13.0), color: [1.0; 4], bold: true, align: Align::Left });
                } else {
                    self.button_bg(&mut d, t, &Hit::DeleteRow(i), s(8.0));
                    let c = if self.hover == Some(Hit::DeleteRow(i)) { danger() } else { fg_muted() };
                    d.push(Draw::Icon { name: "trash-2", r: (t.0 + s(8.0), t.1 + s(8.0), s(18.0), s(18.0)), color: c, stroke: s(1.8) });
                }
                self.hits.push((t, Hit::DeleteRow(i)));
            }
            let b = (r.0 + s(10.0), r.1 + s(11.0), s(32.0), s(32.0));
            d.push(Draw::Fill { r: b, color: badge(i), radius: s(16.0) });
            let initial: String = e.profile.name.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().collect()).unwrap_or_else(|| "?".into());
            d.push(Draw::Text { r: b, text: initial, size: s(15.0), color: ON_COLOR, bold: true, align: Align::Center });
            let tx = b.0 + s(44.0);
            let tw = r.0 + r.2 - tx - s(8.0) - if show_trash { trash_w + s(10.0) } else { 0.0 };
            d.push(Draw::Text { r: (tx, r.1 + s(6.0), tw, s(24.0)), text: fit(&e.profile.name, s(15.0), true, tw), size: s(15.0), color: fg(), bold: true, align: Align::Left });
            let n = e.profile.layouts.len();
            let sub = format!("{n} layout{} · {}", if n == 1 { "" } else { "s" }, auto_apply_label(e.profile.auto_apply));
            d.push(Draw::Text { r: (tx, r.1 + s(28.0), tw, s(20.0)), text: sub, size: s(12.0), color: fg_muted(), bold: false, align: Align::Left });
            self.hits.push((r, Hit::Row(i)));
        }
        if entries.len() > fit_rows {
            let more = format!("{} of {} · ↑↓ for more", fit_rows.min(entries.len() - self.scroll), entries.len());
            d.push(Draw::Text { r: (s(24.0), h - s(28.0), s(LIST_W - 24.0), s(20.0)), text: more, size: s(11.0), color: fg_dim(), bold: false, align: Align::Left });
        }

        // The selected profile.
        let e = &entries[self.selected];
        let p = e.profile;
        let x0 = s(LIST_W + 40.0);
        let fw = w - x0 - s(28.0);
        let right = x0 + fw;
        // One grid, as in Settings: a label column, then every control at the same place.
        let cx = x0 + s(150.0);
        let cw = right - cx;
        let row_h = s(40.0);
        let mut y = s(LIST_TOP);
        let heading = |d: &mut Vec<Draw>, y: f32, t: &str| {
            d.push(Draw::Text { r: (x0, y, fw, s(20.0)), text: t.to_uppercase(), size: s(11.0), color: fg_muted(), bold: true, align: Align::Left });
            d.push(Draw::Line { a: (x0, y + s(24.0)), b: (right, y + s(24.0)), color: hairline(), width: 1.0 });
        };
        let row_label = |d: &mut Vec<Draw>, y: f32, h: f32, t: &str| {
            d.push(Draw::Text { r: (x0, y, s(146.0), h), text: t.into(), size: s(13.0), color: fg(), bold: false, align: Align::Left });
        };

        heading(&mut d, y, "Profile");
        y += s(34.0);
        row_label(&mut d, y, row_h, "Name");
        self.field(&mut d, (cx, y + s(3.0), cw, s(34.0)), Field::Name, &p.name, "", k, true);
        y += row_h + s(12.0);

        heading(&mut d, y, "Match rule");
        y += s(34.0);
        let rule = &p.match_rule;
        let line_h = s(30.0);
        let kv = |d: &mut Vec<Draw>, y: f32, key: &str, val: &str| {
            d.push(Draw::Text { r: (x0, y, s(146.0), line_h), text: key.into(), size: s(13.0), color: fg(), bold: false, align: Align::Left });
            d.push(Draw::Text { r: (cx + s(10.0), y, cw - s(10.0), line_h), text: fit(val, s(13.0), false, cw - s(10.0)), size: s(13.0), color: fg_muted(), bold: false, align: Align::Left });
        };
        kv(&mut d, y, "App", &rule.exe);
        y += line_h;
        kv(&mut d, y, "Window class", if rule.window_class.is_empty() { "any" } else { &rule.window_class });
        y += line_h;
        if !rule.folder.is_empty() {
            kv(&mut d, y, "Folder", &format!("…\\{}", rule.folder));
            y += line_h;
        }
        row_label(&mut d, y, row_h, "Title contains");
        self.field(&mut d, (cx, y + s(4.0), cw, s(32.0)), Field::Title, &rule.title_contains, "any title (click to narrow it down)", k, false);
        y += row_h;
        let (dot, line) = match self.matches.as_slice() {
            [] => (fg_dim(), "No open window matches".to_string()),
            [one] => ([0.3, 0.85, 0.4, 1.0], format!("Matches now: {one}")),
            [one, rest @ ..] => ([0.3, 0.85, 0.4, 1.0], format!("Matches now: {one} and {} more", rest.len())),
        };
        d.push(Draw::Fill { r: (cx + s(10.0), y + (line_h - s(8.0)) / 2.0, s(8.0), s(8.0)), color: dot, radius: s(4.0) });
        d.push(Draw::Text { r: (cx + s(26.0), y, cw - s(26.0), line_h), text: fit(&line, s(12.0), false, cw - s(26.0)), size: s(12.0), color: fg_muted(), bold: false, align: Align::Left });
        y += line_h + s(12.0);

        heading(&mut d, y, "Behaviour");
        y += s(34.0);
        let seg_y = |y: f32| y + (row_h - s(32.0)) / 2.0;
        row_label(&mut d, y, row_h, "Apply the layout");
        let opts = [(AutoApply::Auto, "Automatically"), (AutoApply::Ask, "Ask first"), (AutoApply::Off, "Off")];
        self.segmented(&mut d, (cx, seg_y(y)), &opts.map(|(a, t)| (Hit::AutoApply(a), t, p.auto_apply == a)), k);
        y += row_h;
        row_label(&mut d, y, row_h, "Title bar");
        let opts = [(FrameRemoval::Auto, "Remove if it has one"), (FrameRemoval::Never, "Leave it")];
        self.segmented(&mut d, (cx, seg_y(y)), &opts.map(|(f, t)| (Hit::Frame(f), t, p.remove_frame == f || (f == FrameRemoval::Auto && p.remove_frame == FrameRemoval::Always))), k);
        y += row_h;
        row_label(&mut d, y, row_h, "Pointer");
        {
            use crate::model::profile::Pointer;
            let opts = [(Pointer::Translate, "Translate"), (Pointer::Relative, "Relative"), (Pointer::Off, "Off")];
            self.segmented(&mut d, (cx, seg_y(y)), &opts.map(|(v, t)| (Hit::Pointer(v), t, p.pointer == v)), k);
        }
        y += row_h;
        row_label(&mut d, y, row_h, "Not in front");
        let cb = (cx, y + (row_h - s(20.0)) / 2.0, s(20.0), s(20.0));
        if p.keep_when_unfocused {
            d.push(Draw::Fill { r: cb, color: accent(), radius: s(5.0) });
            d.push(Draw::Icon { name: "check", r: (cb.0 + s(3.0), cb.1 + s(3.0), s(14.0), s(14.0)), color: ON_COLOR, stroke: s(2.5) });
        } else {
            d.push(Draw::Stroke { r: cb, color: fg_muted(), width: s(1.5), radius: s(5.0), dashed: false });
        }
        let kt = "Keep showing the layout";
        d.push(Draw::Text { r: (cx + s(32.0), y, cw - s(32.0), row_h), text: kt.into(), size: s(13.0), color: fg_muted(), bold: false, align: Align::Left });
        self.hits.push(((cx - s(4.0), y + s(4.0), measure(kt, s(13.0), false) + s(44.0), row_h - s(8.0)), Hit::KeepUnfocused));
        y += row_h + s(12.0);

        heading(&mut d, y, "Layouts");
        let nl_w = measure("New layout", s(13.0), false) + s(52.0);
        let nl = (right - nl_w, y - s(8.0), nl_w, s(30.0));
        self.text_button(&mut d, nl, "New layout", Some("plus"), Hit::NewLayout, k, fg());
        y += s(32.0);
        let active = p.active_id();
        let bottom = h - s(100.0);
        for l in &p.layouts {
            if y + s(34.0) > bottom {
                break;
            }
            let row = (x0, y, fw, s(34.0));
            if self.hover.as_ref().is_some_and(|h| matches!(h, Hit::Star(id) | Hit::RenameLayout(id) | Hit::Duplicate(id) | Hit::DeleteLayout(id) if *id == l.id)) {
                d.push(Draw::Fill { r: row, color: glass_hover(), radius: s(6.0) });
            }
            let is_default = active.as_deref() == Some(l.id.as_str());
            let star = (x0 + s(4.0), y + s(5.0), s(24.0), s(24.0));
            d.push(Draw::Icon { name: "star", r: (star.0 + s(2.0), star.1 + s(2.0), s(20.0), s(20.0)), color: if is_default { [0.98, 0.75, 0.14, 1.0] } else { fg_dim() }, stroke: s(if is_default { 2.4 } else { 1.6 }) });
            self.hits.push((star, Hit::Star(l.id.clone())));
            let editing_this = matches!(&self.editing, Some((Field::Layout(id), _)) if *id == l.id);
            let nx = x0 + s(36.0);
            let nw = fw - s(36.0) - s(3.0 * 36.0 + 8.0);
            if editing_this {
                self.field(&mut d, (nx, y + s(1.0), nw, s(32.0)), Field::Layout(l.id.clone()), &l.name, "", k, false);
            } else {
                let suffix = if is_default { "  · default" } else { "" };
                d.push(Draw::Text { r: (nx, y, nw, s(34.0)), text: fit(&format!("{}{suffix}", l.name), s(14.0), false, nw), size: s(14.0), color: fg(), bold: false, align: Align::Left });
            }
            let mut bx = x0 + fw - s(3.0 * 36.0);
            for (icon, hit) in [("pencil", Hit::RenameLayout(l.id.clone())), ("copy", Hit::Duplicate(l.id.clone())), ("trash-2", Hit::DeleteLayout(l.id.clone()))] {
                let br = (bx, y + s(1.0), s(32.0), s(32.0));
                let confirming = self.confirm.as_ref() == Some(&hit);
                let disabled = matches!(hit, Hit::DeleteLayout(_)) && p.layouts.len() == 1;
                if confirming {
                    d.push(Draw::Fill { r: br, color: danger(), radius: s(6.0) });
                } else {
                    self.button_bg(&mut d, br, &hit, s(6.0));
                }
                let color = if confirming { [1.0, 1.0, 1.0, 1.0] } else if disabled { fg_dim() } else { fg_muted() };
                d.push(Draw::Icon { name: icon, r: (br.0 + s(7.0), br.1 + s(7.0), s(18.0), s(18.0)), color, stroke: s(1.8) });
                if !disabled {
                    self.hits.push((br, hit));
                }
                bx += s(36.0);
            }
            y += s(36.0);
        }
        if matches!(self.confirm, Some(Hit::DeleteLayout(_))) {
            self.status = Some("Click the red button again to delete the layout".into());
        }

        // Bottom row.
        let by = h - s(92.0);
        let mut bx = x0;
        for (t, icon, hit, enabled) in [
            ("Export…", "download", Hit::Export, true),
            ("Import…", "upload", Hit::Import, true),
            ("Restore previous session", "history", Hit::Restore, e.has_backup),
        ] {
            let bw = measure(t, s(13.0), false) + s(46.0);
            let r = (bx, by, bw, s(36.0));
            if enabled {
                self.text_button(&mut d, r, t, Some(icon), hit, k, fg());
            } else {
                d.push(Draw::Stroke { r, color: hairline(), width: 1.0, radius: s(8.0), dashed: false });
                d.push(Draw::Icon { name: icon, r: (r.0 + s(12.0), r.1 + s(9.0), s(18.0), s(18.0)), color: fg_dim(), stroke: s(1.8) });
                d.push(Draw::Text { r: (r.0 + s(36.0), r.1, r.2 - s(40.0), r.3), text: t.into(), size: s(13.0), color: fg_dim(), bold: false, align: Align::Left });
            }
            bx += bw + s(10.0);
        }
        let confirming = self.confirm == Some(Hit::DeleteProfile);
        let t = if confirming { "Click again to delete" } else { "Delete profile" };
        let bw = measure(t, s(13.0), false) + s(46.0);
        let r = (w - s(28.0) - bw, by, bw, s(36.0));
        if confirming {
            d.push(Draw::Fill { r, color: danger(), radius: s(8.0) });
            d.push(Draw::Icon { name: "trash-2", r: (r.0 + s(12.0), r.1 + s(9.0), s(18.0), s(18.0)), color: [1.0; 4], stroke: s(1.8) });
            d.push(Draw::Text { r: (r.0 + s(36.0), r.1, r.2 - s(40.0), r.3), text: t.into(), size: s(13.0), color: [1.0; 4], bold: true, align: Align::Left });
            self.hits.push((r, Hit::DeleteProfile));
        } else {
            self.text_button(&mut d, r, t, Some("trash-2"), Hit::DeleteProfile, k, danger());
        }
        // What just happened, in its own band at the bottom.
        d.push(Draw::Line { a: (x0, h - s(44.0)), b: (right, h - s(44.0)), color: hairline(), width: 1.0 });
        let pending = match &self.confirm {
            Some(Hit::DeleteRow(i)) => entries.get(*i).map(|e| e.profile),
            Some(Hit::DeleteProfile) => Some(p),
            _ => None,
        };
        let st = match pending {
            Some(q) => {
                let n = q.layouts.len();
                format!("Delete \"{}\" ({n} layout{})? Click Delete again or press Del. Esc keeps it.", q.name, if n == 1 { "" } else { "s" })
            }
            None => self.status.clone().unwrap_or_else(|| "Changes are saved as you make them.".into()),
        };
        d.push(Draw::Text { r: (x0, h - s(40.0), fw, s(32.0)), text: fit(&st, s(12.0), pending.is_some(), fw), size: s(12.0), color: if pending.is_some() { danger() } else { fg_muted() }, bold: pending.is_some(), align: Align::Left });
        d
    }

    fn button_bg(&self, d: &mut Vec<Draw>, r: Px, hit: &Hit, radius: f32) {
        if self.hover.as_ref() == Some(hit) {
            d.push(Draw::Fill { r, color: glass_hover(), radius });
        }
    }

    fn text_button(&mut self, d: &mut Vec<Draw>, r: Px, text: &str, icon: Option<&'static str>, hit: Hit, k: f32, color: Rgba) {
        let s = |v: f32| v * k;
        self.button_bg(d, r, &hit, s(8.0));
        d.push(Draw::Stroke { r, color: hairline(), width: 1.0, radius: s(8.0), dashed: false });
        let mut tx = r.0 + s(14.0);
        if let Some(i) = icon {
            d.push(Draw::Icon { name: i, r: (r.0 + s(12.0), r.1 + (r.3 - s(18.0)) / 2.0, s(18.0), s(18.0)), color, stroke: s(1.8) });
            tx = r.0 + s(36.0);
        }
        d.push(Draw::Text { r: (tx, r.1, r.0 + r.2 - tx - s(4.0), r.3), text: text.into(), size: s(13.0), color, bold: false, align: Align::Left });
        self.hits.push((r, hit));
    }

    fn segmented(&mut self, d: &mut Vec<Draw>, at: (f32, f32), opts: &[(Hit, &str, bool)], k: f32) {
        let s = |v: f32| v * k;
        let mut x = at.0;
        for (hit, text, on) in opts {
            let w = measure(text, s(13.0), *on) + s(28.0);
            let r = (x, at.1, w, s(32.0));
            if *on {
                d.push(Draw::Fill { r, color: accent_fill(), radius: s(8.0) });
                d.push(Draw::Stroke { r, color: accent(), width: s(1.5), radius: s(8.0), dashed: false });
            } else {
                self.button_bg(d, r, hit, s(8.0));
                d.push(Draw::Stroke { r, color: hairline(), width: 1.0, radius: s(8.0), dashed: false });
            }
            d.push(Draw::Text { r, text: text.to_string(), size: s(13.0), color: fg(), bold: *on, align: Align::Center });
            self.hits.push((r, hit.clone()));
            x += w + s(6.0);
        }
    }

    /// A text field: shows `value` (or `placeholder` dimmed), or the text being typed with a
    /// caret when it is the field being edited.
    #[allow(clippy::too_many_arguments)]
    fn field(&mut self, d: &mut Vec<Draw>, r: Px, f: Field, value: &str, placeholder: &str, k: f32, big: bool) {
        let s = |v: f32| v * k;
        let (size, _) = field_font(&f, k);
        let typing = match &self.editing {
            Some((g, t)) if *g == f => Some(t.clone()),
            _ => None,
        };
        let hit = Hit::Edit(f);
        if typing.is_some() {
            d.push(Draw::Fill { r, color: keycap_fill(), radius: s(6.0) });
            d.push(Draw::Stroke { r, color: accent(), width: s(1.5), radius: s(6.0), dashed: false });
        } else {
            self.button_bg(d, r, &hit, s(6.0));
            d.push(Draw::Stroke { r, color: hairline(), width: 1.0, radius: s(6.0), dashed: false });
        }
        let tr = (r.0 + s(10.0), r.1, r.2 - s(20.0), r.3);
        match typing {
            Some(t) => {
                // Scroll long text so the caret stays in view; draw the selection and caret.
                let room = tr.2 - s(4.0);
                self.edit_first = t.first_visible(self.edit_first, room, |x| measure(x, size, big));
                let text_r = (tr.0, tr.1, room, tr.3);
                self.edit_text = Some((text_r, size, big));
                d.extend(t.draws(self.edit_first, text_r, size, big, fg(), accent_fill(), accent(), s(1.5)));
            }
            None if value.is_empty() => d.push(Draw::Text { r: tr, text: fit(placeholder, size, false, tr.2), size, color: fg_dim(), bold: false, align: Align::Left }),
            None => {
                d.push(Draw::Text { r: tr, text: fit(value, size, big, tr.2 - s(24.0)), size, color: fg(), bold: big, align: Align::Left });
                d.push(Draw::Icon { name: "pencil", r: (r.0 + r.2 - s(26.0), r.1 + (r.3 - s(14.0)) / 2.0, s(14.0), s(14.0)), color: fg_dim(), stroke: s(1.6) });
            }
        }
        self.hits.push((r, hit));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::layout::{DisplayMode, DisplayRef};
    use crate::model::profile::WindowFacts;
    use crate::shell::canvas::Mods;

    fn profile(title: &str) -> Profile {
        let f = WindowFacts { exe_path: format!(r"C:\Games\{title}.exe"), window_class: "W".into(), title: title.into() };
        Profile::new_for(&f, DisplayRef { hardware_id: "x".into(), label: "Display 1".into(), mode: DisplayMode::Takeover }, &[])
    }

    fn center(p: &ProfilesPanel, want: impl Fn(&Hit) -> bool) -> (f32, f32) {
        let (r, _) = p.hits.iter().find(|(_, h)| want(h)).expect("hit area");
        (r.0 + r.2 / 2.0, r.1 + r.3 / 2.0)
    }

    fn click(p: &mut ProfilesPanel, at: (f32, f32), e: &[Entry]) -> Op {
        p.input(Input::Down(Button::Left, at.0, at.1, Mods::default()), 1.0, e)
    }

    #[test]
    fn rows_select_and_ops_come_back() {
        let mut a = profile("Alpha");
        a.duplicate_layout(&a.layouts[0].id.clone());
        let b = profile("Beta");
        let entries = [Entry { profile: &a, has_backup: true }, Entry { profile: &b, has_backup: false }];
        let mut p = ProfilesPanel::new(0);
        p.draw(&entries, 1.0, SIZE.0, SIZE.1);
        let row1 = center(&p, |h| *h == Hit::Row(1));
        assert_eq!(click(&mut p, row1, &entries), Op::None);
        assert_eq!(p.selected, 1);
        p.input(Input::Key(0x26, Mods::default()), 1.0, &entries);
        assert_eq!(p.selected, 0);
        p.draw(&entries, 1.0, SIZE.0, SIZE.1);
        let off = center(&p, |h| *h == Hit::AutoApply(AutoApply::Off));
        assert_eq!(click(&mut p, off, &entries), Op::AutoApply(0, AutoApply::Off));
        let second = a.layouts[1].id.clone();
        let star = center(&p, |h| *h == Hit::Star(second.clone()));
        assert_eq!(click(&mut p, star, &entries), Op::SetDefault(0, second.clone()));
        let dup = center(&p, |h| *h == Hit::Duplicate(second.clone()));
        assert_eq!(click(&mut p, dup, &entries), Op::DuplicateLayout(0, second));
        assert_eq!(p.input(Input::Key(0x1B, Mods::default()), 1.0, &entries), Op::Close);
    }

    #[test]
    fn deleting_needs_a_second_click() {
        let a = profile("Alpha");
        let entries = [Entry { profile: &a, has_backup: false }];
        let mut p = ProfilesPanel::new(0);
        p.draw(&entries, 1.0, SIZE.0, SIZE.1);
        let del = center(&p, |h| *h == Hit::DeleteProfile);
        assert_eq!(click(&mut p, del, &entries), Op::None);
        p.draw(&entries, 1.0, SIZE.0, SIZE.1);
        let del = center(&p, |h| *h == Hit::DeleteProfile);
        assert_eq!(click(&mut p, del, &entries), Op::DeleteProfile(0));
        // Clicking elsewhere in between cancels.
        p.draw(&entries, 1.0, SIZE.0, SIZE.1);
        click(&mut p, del, &entries);
        click(&mut p, (5.0, 5.0), &entries);
        p.draw(&entries, 1.0, SIZE.0, SIZE.1);
        let del = center(&p, |h| *h == Hit::DeleteProfile);
        assert_eq!(click(&mut p, del, &entries), Op::None);
        // The only layout has no delete button.
        assert!(!p.hits.iter().any(|(_, h)| matches!(h, Hit::DeleteLayout(_))));
        // No backup: Restore previous session is not clickable.
        assert!(!p.hits.iter().any(|(_, h)| *h == Hit::Restore));
    }

    fn status_text(d: &[Draw]) -> String {
        d.iter().filter_map(|x| if let Draw::Text { text, .. } = x { Some(text.clone()) } else { None }).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn new_profile_from_the_button_and_the_keyboard() {
        let a = profile("Alpha");
        let entries = [Entry { profile: &a, has_backup: false }];
        let mut p = ProfilesPanel::new(0);
        p.draw(&entries, 1.0, SIZE.0, SIZE.1);
        let new = center(&p, |h| *h == Hit::NewProfile);
        assert_eq!(click(&mut p, new, &entries), Op::NewProfile);
        assert_eq!(p.input(Input::Key(0x4E, Mods { ctrl: true, ..Mods::default() }), 1.0, &entries), Op::NewProfile);
        assert_eq!(p.input(Input::Key(0x2D, Mods::default()), 1.0, &entries), Op::NewProfile);
        // With no profiles at all, the button is still there.
        let mut empty = ProfilesPanel::new(0);
        empty.draw(&[], 1.0, SIZE.0, SIZE.1);
        let new = center(&empty, |h| *h == Hit::NewProfile);
        assert_eq!(click(&mut empty, new, &[]), Op::NewProfile);
    }

    #[test]
    fn a_rows_trash_button_deletes_that_profile_after_a_second_click() {
        let a = profile("Alpha");
        let b = profile("Beta");
        let entries = [Entry { profile: &a, has_backup: false }, Entry { profile: &b, has_backup: false }];
        let mut p = ProfilesPanel::new(0);
        // Hovering the second row shows its trash button.
        p.draw(&entries, 1.0, SIZE.0, SIZE.1);
        let row1 = center(&p, |h| *h == Hit::Row(1));
        p.input(Input::Move(row1.0, row1.1), 1.0, &entries);
        let d = p.draw(&entries, 1.0, SIZE.0, SIZE.1);
        assert!(!status_text(&d).contains("Delete \"Beta\""));
        let trash = center(&p, |h| *h == Hit::DeleteRow(1));
        assert_eq!(click(&mut p, trash, &entries), Op::None, "the first click only asks");
        assert_eq!(p.selected, 1);
        let d = p.draw(&entries, 1.0, SIZE.0, SIZE.1);
        assert!(status_text(&d).contains("Delete \"Beta\" (1 layout)?"), "the status band says what will go");
        assert!(status_text(&d).contains("Delete?"));
        let trash = center(&p, |h| *h == Hit::DeleteRow(1));
        assert_eq!(click(&mut p, trash, &entries), Op::DeleteProfile(1));
    }

    #[test]
    fn delete_key_asks_then_deletes_and_esc_keeps_the_profile() {
        let a = profile("Alpha");
        let entries = [Entry { profile: &a, has_backup: false }];
        let mut p = ProfilesPanel::new(0);
        p.draw(&entries, 1.0, SIZE.0, SIZE.1);
        let key = |p: &mut ProfilesPanel, vk: u16| p.input(Input::Key(vk, Mods::default()), 1.0, &entries);
        assert_eq!(key(&mut p, 0x2E), Op::None);
        assert_eq!(key(&mut p, 0x1B), Op::None, "Esc cancels the delete instead of closing");
        assert_eq!(key(&mut p, 0x2E), Op::None);
        assert_eq!(key(&mut p, 0x2E), Op::DeleteProfile(0));
        assert_eq!(key(&mut p, 0x1B), Op::Close);
    }

    #[test]
    fn the_name_field_edits_like_a_text_box() {
        let a = profile("Alpha");
        let entries = [Entry { profile: &a, has_backup: false }];
        let mut p = ProfilesPanel::new(0);
        p.draw(&entries, 1.0, SIZE.0, SIZE.1);
        let key = |p: &mut ProfilesPanel, vk: u16, m: Mods| p.input(Input::Key(vk, m), 1.0, &entries);
        // F2 selects the whole name: typing replaces it.
        key(&mut p, 0x71, Mods::default());
        for c in "Sample".chars() {
            p.input(Input::Char(c), 1.0, &entries);
        }
        // Home, then insert at the start; End, then Shift+Left selects the last letter.
        key(&mut p, 0x24, Mods::default());
        p.input(Input::Char('X'), 1.0, &entries);
        key(&mut p, 0x23, Mods::default());
        key(&mut p, 0x25, Mods { shift: true, ..Default::default() });
        p.input(Input::Char('o'), 1.0, &entries);
        // Ctrl+Left jumps a word; Delete removes the char after the caret.
        key(&mut p, 0x25, Mods { ctrl: true, ..Default::default() });
        key(&mut p, 0x2E, Mods::default());
        assert_eq!(p.editing.as_ref().map(|(_, t)| t.text()), Some("Samplo".into()));
        // Ctrl+Z undoes the delete.
        key(&mut p, 0x5A, Mods { ctrl: true, ..Default::default() });
        assert_eq!(p.editing.as_ref().map(|(_, t)| t.text()), Some("XSamplo".into()));
        // A click in the field moves the caret instead of finishing typing.
        p.draw(&entries, 1.0, SIZE.0, SIZE.1);
        let (r, ..) = p.edit_text.expect("the typed text is drawn");
        assert_eq!(click(&mut p, (r.0 + 1.0, r.1 + r.3 / 2.0), &entries), Op::None);
        p.input(Input::Up(Button::Left, r.0 + 1.0, r.1 + r.3 / 2.0), 1.0, &entries);
        assert_eq!(p.editing.as_ref().map(|(_, t)| t.caret()), Some(0));
        // A drag to the far end selects everything; typing replaces the selection.
        click(&mut p, (r.0 + 1.0, r.1 + r.3 / 2.0), &entries);
        p.input(Input::Move(r.0 + r.2 - 1.0, r.1 + r.3 / 2.0), 1.0, &entries);
        p.input(Input::Up(Button::Left, r.0 + r.2 - 1.0, r.1 + r.3 / 2.0), 1.0, &entries);
        assert_eq!(p.editing.as_ref().map(|(_, t)| t.selection()), Some((0, 7)));
        p.input(Input::Char('Z'), 1.0, &entries);
        assert_eq!(key(&mut p, 0x0D, Mods::default()), Op::Rename(0, "Z".into()));
    }

    #[test]
    fn typing_renames_and_narrows_the_match() {
        let a = profile("Alpha");
        let entries = [Entry { profile: &a, has_backup: false }];
        let mut p = ProfilesPanel::new(0);
        p.draw(&entries, 1.0, SIZE.0, SIZE.1);
        let name = center(&p, |h| *h == Hit::Edit(Field::Name));
        click(&mut p, name, &entries);
        p.input(Input::Key(0x08, Mods { ctrl: true, ..Default::default() }), 1.0, &entries);
        for c in "Sample".chars() {
            p.input(Input::Char(c), 1.0, &entries);
        }
        assert_eq!(p.input(Input::Key(0x0D, Mods::default()), 1.0, &entries), Op::Rename(0, "Sample".into()));
        p.draw(&entries, 1.0, SIZE.0, SIZE.1);
        let title = center(&p, |h| *h == Hit::Edit(Field::Title));
        click(&mut p, title, &entries);
        for c in "TG4".chars() {
            p.input(Input::Char(c), 1.0, &entries);
        }
        // Clicking somewhere else commits.
        assert_eq!(click(&mut p, (5.0, 5.0), &entries), Op::TitleRule(0, "TG4".into()));
        // Esc cancels typing without closing the panel.
        p.draw(&entries, 1.0, SIZE.0, SIZE.1);
        click(&mut p, name, &entries);
        assert_eq!(p.input(Input::Key(0x1B, Mods::default()), 1.0, &entries), Op::None);
        assert!(p.editing.is_none());
    }
}
