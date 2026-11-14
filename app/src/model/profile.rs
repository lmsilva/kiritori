//! Profiles: one application, its match rule and its layouts. The JSON form is a public
//! interface, so field names here are part of the format.

use serde::{Deserialize, Serialize};

use super::layout::{DisplayRef, Layout};

pub const SCHEMA: u32 = 1;

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchRule {
    /// Executable file name, for example `game.exe`. Always required.
    pub exe: String,
    #[serde(default)]
    pub window_class: String,
    #[serde(default)]
    pub title_contains: String,
    /// Folder the executable must be in, matched as a suffix of the full path, for example
    /// `Sample Game\Binaries\Win64`. Used when the file name is
    /// generic, such as Unreal Engine's `*-Win64-Shipping.exe`. Empty: any folder.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub folder: String,
}

/// The facts Kiritori can read about a window, to test against a match rule.
#[derive(Clone, Debug, Default)]
pub struct WindowFacts {
    pub exe_path: String,
    pub window_class: String,
    pub title: String,
}

impl WindowFacts {
    pub fn exe_name(&self) -> &str {
        self.exe_path.rsplit(['\\', '/']).next().unwrap_or("")
    }

    pub fn folder(&self) -> &str {
        match self.exe_path.rfind(['\\', '/']) {
            Some(i) => &self.exe_path[..i],
            None => "",
        }
    }
}

/// Unreal Engine and similar build tools give many games the same executable name.
pub fn is_generic_exe(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.ends_with("-win64-shipping.exe") || n.ends_with("-win32-shipping.exe") || n == "game.exe" || n == "launcher.exe"
}

impl MatchRule {
    pub fn matches(&self, w: &WindowFacts) -> bool {
        if !w.exe_name().eq_ignore_ascii_case(&self.exe) {
            return false;
        }
        if !self.window_class.is_empty() && self.window_class != w.window_class {
            return false;
        }
        if !self.title_contains.is_empty() && !w.title.to_lowercase().contains(&self.title_contains.to_lowercase()) {
            return false;
        }
        if !self.folder.is_empty() {
            let have = w.folder().replace('/', "\\").to_lowercase();
            let want = self.folder.replace('/', "\\").trim_end_matches('\\').to_lowercase();
            if !have.ends_with(&want) {
                return false;
            }
        }
        true
    }

    /// A rule for a newly seen window: executable and class always, plus the last three
    /// folders of the path when the executable name is generic.
    pub fn for_window(w: &WindowFacts) -> Self {
        let folder = if is_generic_exe(w.exe_name()) {
            let parts: Vec<&str> = w.folder().split(['\\', '/']).filter(|p| !p.is_empty()).collect();
            parts[parts.len().saturating_sub(3)..].join("\\")
        } else {
            String::new()
        };
        MatchRule { exe: w.exe_name().to_string(), window_class: w.window_class.clone(), title_contains: String::new(), folder }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AutoApply {
    #[default]
    Auto,
    Ask,
    Off,
}

/// What the mouse and touch do over cutouts while the layer is closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Pointer {
    /// Pointing at a cutout points at the same spot in the app.
    #[default]
    Translate,
    /// Mouse movement passes to the app unchanged (games that read raw movement).
    Relative,
    /// Cutouts are display-only: clicks on Takeover displays do nothing.
    Off,
}

/// Remove the app window's title bar and frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FrameRemoval {
    #[default]
    Auto,
    Always,
    Never,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub schema: u32,
    pub name: String,
    #[serde(rename = "match")]
    pub match_rule: MatchRule,
    #[serde(default)]
    pub auto_apply: AutoApply,
    #[serde(default)]
    pub keep_when_unfocused: bool,
    #[serde(default)]
    pub remove_frame: FrameRemoval,
    #[serde(default, skip_serializing_if = "is_translate")]
    pub pointer: Pointer,
    /// The layout applied when the app appears: the most recently used one.
    pub default_layout: String,
    pub layouts: Vec<Layout>,
    /// The layout last used with each set of connected displays (with the touchscreen unplugged
    /// the game opens in its single-screen layout).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub layout_for_displays: std::collections::BTreeMap<String, String>,
}

impl Profile {
    /// A new profile for a window, with one empty layout. `taken` lists names already in use,
    /// for the "New template N" fallback.
    pub fn new_for(w: &WindowFacts, display: DisplayRef, taken: &[String]) -> Self {
        let name = default_name(w, taken);
        let layout = Layout::new("layout-1", "Layout 1", display);
        Profile {
            schema: SCHEMA,
            name,
            match_rule: MatchRule::for_window(w),
            auto_apply: AutoApply::Auto,
            keep_when_unfocused: false,
            remove_frame: FrameRemoval::Auto,
            pointer: Pointer::Translate,
            default_layout: layout.id.clone(),
            layouts: vec![layout],
            layout_for_displays: Default::default(),
        }
    }

    pub fn layout(&self, id: &str) -> Option<&Layout> {
        self.layouts.iter().find(|l| l.id == id)
    }

    pub fn layout_mut(&mut self, id: &str) -> Option<&mut Layout> {
        self.layouts.iter_mut().find(|l| l.id == id)
    }

    /// The layout to apply: the default, or the first if the default is missing.
    pub fn active(&self) -> Option<&Layout> {
        self.layout(&self.default_layout).or(self.layouts.first())
    }

    pub fn active_id(&self) -> Option<String> {
        self.active().map(|l| l.id.clone())
    }

    /// Rename a layout; its id follows the new name so files stay readable.
    pub fn rename_layout(&mut self, id: &str, name: &str) -> Option<String> {
        let name = name.trim();
        if name.is_empty() {
            return None;
        }
        let taken: Vec<String> = self.layouts.iter().filter(|l| l.id != id).map(|l| l.id.clone()).collect();
        let new_id = unique(&slug(name, "layout"), &taken);
        let was_default = self.default_layout == id;
        let l = self.layout_mut(id)?;
        l.name = name.to_string();
        l.id = new_id.clone();
        if was_default {
            self.default_layout = new_id.clone();
        }
        Some(new_id)
    }

    /// Copy a layout under a new name ("Single screen copy"); returns the copy's id.
    pub fn duplicate_layout(&mut self, id: &str) -> Option<String> {
        let src = self.layout(id)?.clone();
        let names: Vec<String> = self.layouts.iter().map(|l| l.name.clone()).collect();
        let base = format!("{} copy", src.name);
        let name = if names.contains(&base) { (2..).map(|n| format!("{base} {n}")).find(|n| !names.contains(n)).unwrap() } else { base };
        let ids: Vec<String> = self.layouts.iter().map(|l| l.id.clone()).collect();
        let new_id = unique(&slug(&name, "layout"), &ids);
        let mut copy = src;
        copy.id = new_id.clone();
        copy.name = name;
        let at = self.layouts.iter().position(|l| l.id == id).map(|i| i + 1).unwrap_or(self.layouts.len());
        self.layouts.insert(at, copy);
        Some(new_id)
    }

    /// Delete a layout. The last one cannot go (a profile always has a layout); if it was the
    /// default, the first remaining one becomes the default.
    pub fn delete_layout(&mut self, id: &str) -> bool {
        if self.layouts.len() <= 1 || self.layout(id).is_none() {
            return false;
        }
        self.layouts.retain(|l| l.id != id);
        if self.default_layout == id {
            self.default_layout = self.layouts[0].id.clone();
        }
        true
    }

    /// The next layout in order, for Ctrl+Alt+L.
    pub fn next_layout_id(&self) -> Option<String> {
        let i = self.layouts.iter().position(|l| l.id == self.default_layout).unwrap_or(0);
        self.layouts.get((i + 1) % self.layouts.len().max(1)).map(|l| l.id.clone())
    }
}

fn is_translate(p: &Pointer) -> bool {
    *p == Pointer::Translate
}

fn default_name(w: &WindowFacts, taken: &[String]) -> String {
    let title = w.title.trim();
    if !title.is_empty() && title.chars().any(|c| c.is_alphanumeric()) {
        return shorten(title, 40);
    }
    (1..).map(|n| format!("New template {n}")).find(|n| !taken.contains(n)).unwrap()
}

/// At most `max` characters, cut at a word boundary when there is one, with an ellipsis.
/// Window titles can be long (a browser's is the page title).
fn shorten(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max - 1).collect();
    let cut = match cut.rfind(' ') {
        Some(i) if i > max / 2 => cut[..i].to_string(),
        _ => cut,
    };
    format!("{}…", cut.trim_end_matches([' ', '-', '–', ':', '·']))
}

/// File-name-safe id: lowercase ASCII letters, digits and dashes. Falls back to `fallback`
/// when nothing is left (for example a Japanese-only title).
pub fn slug(name: &str, fallback: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() { fallback.to_string() } else { out.chars().take(60).collect() }
}

/// `base`, or `base-2`, `base-3`… whichever is not taken.
pub fn unique(base: &str, taken: &[String]) -> String {
    if !taken.iter().any(|t| t == base) {
        return base.to_string();
    }
    (2..).map(|n| format!("{base}-{n}")).find(|c| !taken.contains(c)).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::layout::DisplayMode;

    #[test]
    fn layouts_duplicate_and_delete() {
        let d = DisplayRef { hardware_id: "x".into(), label: "Display 1".into(), mode: DisplayMode::Takeover };
        let mut p = Profile::new_for(&sample(), d, &[]);
        let first = p.layouts[0].id.clone();
        let copy = p.duplicate_layout(&first).unwrap();
        assert_eq!(p.layouts[1].name, "Layout 1 copy");
        let copy2 = p.duplicate_layout(&first).unwrap();
        assert_eq!(p.layout(&copy2).unwrap().name, "Layout 1 copy 2");
        assert_eq!(p.layouts.len(), 3);
        assert_ne!(copy, copy2);
        assert!(p.delete_layout(&first));
        assert_eq!(p.default_layout, p.layouts[0].id, "the default moves to the first remaining layout");
        assert!(p.delete_layout(&copy));
        assert!(!p.delete_layout(&copy2), "the last layout stays");
        assert!(!p.delete_layout("nope"));
    }

    fn sample() -> WindowFacts {
        WindowFacts {
            exe_path: r"D:\Games\Sample Game\Binaries\Win64\SampleGame-Win64-Shipping.exe".into(),
            window_class: "UnrealWindow".into(),
            title: "SampleGame  ".into(),
        }
    }

    fn display() -> DisplayRef {
        DisplayRef { hardware_id: "x".into(), label: "Display 1".into(), mode: DisplayMode::Takeover }
    }

    #[test]
    fn generic_unreal_exe_also_matches_on_folder() {
        let rule = MatchRule::for_window(&sample());
        assert_eq!(rule.exe, "SampleGame-Win64-Shipping.exe");
        assert_eq!(rule.folder, r"Sample Game\Binaries\Win64");
        assert!(rule.matches(&sample()));

        let mut other_game = sample();
        other_game.exe_path = r"D:\Other\Sample Game\Binaries\Win64\SampleGame-Win64-Shipping.exe".into();
        assert!(rule.matches(&other_game), "same last three folders still match; a moved game keeps working");
        other_game.exe_path = r"D:\Other\Game\Binaries\Win64\SampleGame-Win64-Shipping.exe".into();
        assert!(!rule.matches(&other_game));
    }

    #[test]
    fn plain_exe_ignores_folder_and_case() {
        let w = WindowFacts { exe_path: r"C:\x\kiritori-standin.exe".into(), window_class: "KiritoriStandIn".into(), title: "Kiritori stand-in".into() };
        let rule = MatchRule::for_window(&w);
        assert!(rule.folder.is_empty());
        let moved = WindowFacts { exe_path: r"D:\elsewhere\KIRITORI-STANDIN.EXE".into(), ..w.clone() };
        assert!(rule.matches(&moved));
        let other_class = WindowFacts { window_class: "Launcher".into(), ..w };
        assert!(!rule.matches(&other_class));
    }

    #[test]
    fn title_contains_is_case_insensitive() {
        let mut rule = MatchRule::for_window(&sample());
        rule.title_contains = "samplegame".into();
        assert!(rule.matches(&sample()));
        rule.title_contains = "outrun".into();
        assert!(!rule.matches(&sample()));
    }

    #[test]
    fn default_names() {
        let p = Profile::new_for(&sample(), display(), &[]);
        assert_eq!(p.name, "SampleGame");
        let blank = WindowFacts { title: "  ".into(), ..sample() };
        let taken = vec!["New template 1".to_string()];
        assert_eq!(Profile::new_for(&blank, display(), &taken).name, "New template 2");
    }

    #[test]
    fn long_titles_are_shortened_at_a_word() {
        let w = WindowFacts { title: "The Hidden Curse of Something Long: Why It Feels Like This - YouTube - Google Chrome".into(), ..sample() };
        let name = Profile::new_for(&w, display(), &[]).name;
        assert_eq!(name, "The Hidden Curse of Something Long…");
        assert!(name.chars().count() <= 40);
    }

    #[test]
    fn rename_layout_moves_id_and_default() {
        let mut p = Profile::new_for(&sample(), display(), &[]);
        let id = p.rename_layout("layout-1", "Single screen").unwrap();
        assert_eq!(id, "single-screen");
        assert_eq!(p.default_layout, "single-screen");
        assert_eq!(p.active().unwrap().name, "Single screen");
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("Sample Game!! (5.80)", "x"), "sample-game-5-80");
        assert_eq!(slug("電車でGO", "profile"), "go");
        assert_eq!(slug("電車", "profile"), "profile");
        assert_eq!(unique("single-screen", &["single-screen".into(), "single-screen-2".into()]), "single-screen-3");
    }
}
