//! The app's state and the commands that change it. The shell turns tray clicks, global
//! shortcuts and foreground changes into calls here; the engine and layer (later slices) read
//! the state to decide what to draw.

use std::collections::HashMap;
use std::time::Instant;

use crate::engine::draw::Draw;
use crate::layer::{Effect, Geo, Layer};
use crate::log;
use crate::model::history::History;
use crate::model::layout::{DisplayRef, Layout, Preset};
use crate::shell::canvas::Input;
use crate::model::profile::{AutoApply, FrameRemoval, Profile, WindowFacts};
use crate::shell::icon::TrayState;
use crate::shell::system::{self, SavedFrame};
use crate::store::{Settings, Store, StoredProfile};

#[derive(Clone, Debug)]
pub struct AppWindow {
    pub hwnd: isize,
    pub facts: WindowFacts,
    pub display: DisplayRef,
}

pub struct App {
    pub store: Store,
    pub settings: Settings,
    pub profiles: Vec<StoredProfile>,
    /// The most recent foreground window that could be a source.
    pub last_app: Option<AppWindow>,
    /// Index into `profiles` of the profile being shown, if any.
    pub active: Option<usize>,
    /// The editing layer, while open.
    pub layer: Option<Layer>,
    pub bypass: bool,
    /// Something the user must know about (a shortcut that could not be registered, a
    /// capture failure). Shown as the tray's problem badge and in the menu.
    pub problem: Option<String>,
    /// A problem reported by the engine (capture failed, game runs as administrator).
    pub engine_problem: Option<String>,
    /// Emergency off: nothing captured or drawn until Ctrl+Alt+K or Ctrl+Alt+B.
    pub suspended: bool,
    /// Windows whose frame Kiritori removed, to put back on quit or emergency off.
    pub frames: Vec<(isize, SavedFrame)>,
    /// The window the "layout applied" toast was last shown for. Returning to the same window
    /// shows the canvas again with no toast.
    toasted: Option<isize>,
    /// The auto-apply toast on the canvas, and when it started.
    toast: Option<(String, Instant)>,
    /// Undo history per layout, keyed "profile-stem/layout-id", kept while Kiritori runs.
    histories: HashMap<String, History>,
    /// Still-frame counter for the engine.
    freeze: u64,
    /// The layout as it was when the current drag began. Drags change the layout as they go,
    /// so the undo step must start from here; Esc puts it back.
    drag_before: Option<Layout>,
    /// The layer's chrome uses the light theme.
    pub light: bool,
    /// A profile the command line asked for (`--apply`, `--launch`): it applies even when its
    /// auto-apply is Ask or Off, until Kiritori quits.
    pub forced: Option<usize>,
    /// The self-test results, once run.
    pub self_test: Option<String>,
    /// The shell should start Pick a window.
    pub wants_pick: bool,
    /// Pick a window is running: Kiritori's own display steps aside so real windows show.
    pub picking: bool,
    /// The first-run tutorial, while it runs.
    pub tutorial: Option<crate::tutorial::Tutorial>,
    /// Notifications for the shell to show: (title, text).
    pub notices: Vec<(String, String)>,
    /// The Profiles panel, while open.
    pub panel: Option<crate::panel::ProfilesPanel>,
    /// The Settings panel, while open (it shares the panel window).
    pub settings_panel: Option<crate::settings::SettingsPanel>,
    /// Shortcuts Windows refused because another program holds them, set by the shell.
    pub taken: Vec<(crate::model::shortcut::Global, usize)>,
    /// The shortcuts changed: the shell registers them again.
    pub hotkeys_dirty: bool,
    /// The shell should show the self-test (About).
    pub show_about: bool,
    /// For the panel: which profiles have a previous-session backup, and when the "Matches
    /// now" line was last refreshed.
    panel_backups: Vec<bool>,
    panel_checked: Option<Instant>,
    /// A file dialog the shell should show over the panel.
    pub dialog: Option<Dialog>,
    /// "Ask first": the profile whose layout waits for Apply or Ctrl+Alt+K, for this window,
    /// and since when its toast shows.
    pub asking: Option<(usize, isize, Instant)>,
    ask_hits: Vec<(crate::engine::draw::Px, bool)>,
    /// Windows whose "Ask first" layout the user accepted; they apply without asking again.
    approved: Vec<isize>,
    // Several displays
    /// The connected displays, refreshed when Windows reports a change.
    pub monitors: Vec<crate::model::display::Monitor>,
    /// Index into `monitors` of the display holding the app window.
    pub source_monitor: usize,
    /// The display (layout key) that carries the layer's toolbar: the one under the pointer.
    pub here: String,
    /// A cutout being dragged over another display: that display and the pointer on it.
    pub drag_over: Option<(usize, (f32, f32))>,
    /// A send waiting for the user to choose Takeover or Float for a display not in use.
    pub pending_send: Option<(u32, usize, Option<(f64, f64)>)>,
    /// The parked-cutouts toast was shown for this layout and these missing displays.
    parked_note: Option<(String, Vec<String>)>,
    /// An effect from a menu, for the shell to carry out on its next redraw.
    pub queued: Option<Effect>,
    /// Suggest or Trim, waiting for the picture's pixels from the engine.
    pub pixel_job: Option<Effect>,
    /// The source's drawing area in pixels, as last reported by the engine.
    pub source_size: (i32, i32),
    /// "This app runs as administrator" card: shown since, until answered.
    pub elevation_card: Option<Instant>,
    /// The user chose Restart as administrator; the shell relaunches Kiritori elevated.
    pub wants_restart_admin: bool,
    /// The "Source shape changed" toast was shown for this layout at this size.
    shape_note: Option<(String, i32, i32)>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Dialog {
    Export(usize),
    Import,
    /// Choose an open window to make a profile for.
    NewProfile,
}

/// The Ask toast shows this long; Ctrl+Alt+K still applies after it goes.
const ASK_SHOW: std::time::Duration = std::time::Duration::from_secs(12);
/// The Ask toast's size in 96-dpi units.
pub const ASK_SIZE: (f32, f32) = (560.0, 64.0);

/// The auto-apply toast shows for 2.5 seconds, then fades over half a second.
const TOAST_SHOW: std::time::Duration = std::time::Duration::from_millis(2500);
const TOAST_TOTAL: std::time::Duration = std::time::Duration::from_millis(3000);

/// A small glass pill at the top right of the canvas; it never takes focus.
fn toast_draws(text: &str, age: std::time::Duration, geo: &Geo) -> Vec<Draw> {
    use crate::engine::draw::Align;
    use crate::layer::style::{fg, glass, hairline};
    if age >= TOAST_TOTAL {
        return Vec::new();
    }
    let fade = if age <= TOAST_SHOW { 1.0 } else { 1.0 - (age - TOAST_SHOW).as_secs_f32() / (TOAST_TOTAL - TOAST_SHOW).as_secs_f32() };
    let k = geo.scale;
    let size = 14.0 * k;
    let w = crate::engine::render::measure(text, size, false) + 36.0 * k;
    let h = 38.0 * k;
    let r = (geo.canvas.w as f32 - w - 24.0 * k, 24.0 * k, w, h);
    let a = |mut c: crate::engine::draw::Rgba| {
        c[3] *= fade;
        c
    };
    vec![
        Draw::Fill { r, color: a(glass()), radius: h / 2.0 },
        Draw::Stroke { r, color: a(hairline()), width: 1.0, radius: h / 2.0, dashed: false },
        Draw::Text { r, text: text.to_string(), size, color: a(fg()), bold: false, align: Align::Center },
    ]
}

impl App {
    pub fn new(store: Store) -> Self {
        let settings = store.load_settings();
        let report = store.load_profiles();
        for (path, why) in &report.problems {
            log!("could not read {}: {why}", path.display());
        }
        log!("loaded {} profiles from {}", report.profiles.len(), store.root().display());
        App {
            store,
            settings,
            profiles: report.profiles,
            last_app: None,
            active: None,
            layer: None,
            bypass: false,
            problem: None,
            engine_problem: None,
            suspended: false,
            frames: Vec::new(),
            toasted: None,
            toast: None,
            histories: HashMap::new(),
            freeze: 0,
            drag_before: None,
            light: false,
            wants_pick: false,
            picking: false,
            self_test: None,
            forced: None,
            panel: None,
            settings_panel: None,
            taken: Vec::new(),
            hotkeys_dirty: false,
            show_about: false,
            panel_backups: Vec::new(),
            panel_checked: None,
            dialog: None,
            asking: None,
            ask_hits: Vec::new(),
            approved: Vec::new(),
            monitors: Vec::new(),
            source_monitor: 0,
            here: crate::model::layout::MAIN_DISPLAY.into(),
            drag_over: None,
            pending_send: None,
            parked_note: None,
            elevation_card: None,
            wants_restart_admin: false,
            source_size: (0, 0),
            queued: None,
            pixel_job: None,
            shape_note: None,
            tutorial: None,
            notices: Vec::new(),
        }
    }

    /// Follow the theme setting: System (the Windows app theme), Light or Dark.
    pub fn refresh_theme(&mut self) {
        let light = match self.settings.theme {
            crate::store::Theme::System => system::apps_use_light_theme(),
            crate::store::Theme::Light => true,
            crate::store::Theme::Dark => false,
        };
        if light != self.light {
            log!("theme: {}", if light { "light" } else { "dark" });
        }
        self.light = light;
    }

    pub fn tray_state(&self) -> TrayState {
        if self.problem.is_some() || self.engine_problem.is_some() {
            TrayState::Problem
        } else if self.bypass {
            TrayState::Bypassed
        } else if self.layer.is_some() {
            TrayState::Editing
        } else if self.active.is_some() {
            TrayState::Active
        } else {
            TrayState::Idle
        }
    }

    /// "Sample Game · Single screen", for the tooltip and the menu header.
    pub fn current_label(&self) -> Option<String> {
        let p = &self.profiles[self.active?].profile;
        Some(match p.active() {
            Some(l) => format!("{} · {}", p.name, l.name),
            None => p.name.clone(),
        })
    }

    pub fn tooltip(&self) -> String {
        let state = match self.tray_state() {
            TrayState::Idle => "idle",
            TrayState::Active => "active",
            TrayState::Editing => "editing",
            TrayState::Bypassed => "showing original",
            TrayState::Problem => "needs attention",
        };
        let mut t = match self.current_label() {
            Some(l) => format!("Kiritori — {l} ({state})"),
            None => format!("Kiritori — {state}"),
        };
        if !self.store.writable() {
            t.push_str(" · not saving");
        }
        t.chars().take(127).collect()
    }

    fn find_profile(&self, facts: &WindowFacts) -> Option<usize> {
        self.profiles.iter().position(|p| p.profile.match_rule.matches(facts))
    }

    /// A new window came to the front.
    pub fn on_foreground(&mut self, w: Option<AppWindow>) {
        let Some(w) = w else {
            // Our own windows or the desktop: nothing changes while the layer is open.
            return;
        };
        // Another app in front (a screenshot tool, alt-tab): the layer cannot keep the keyboard,
        // so close it rather than leave it waiting, unseen, for keys it will never get.
        if self.layer.is_some() && self.last_app.as_ref().is_some_and(|l| l.hwnd != w.hwnd) {
            self.close_layer(&format!(" ({} came to the front)", w.facts.exe_name()));
        }
        let matched = self.find_profile(&w.facts);
        // "Keep showing when not focused": another window in front leaves the layout up while
        // the app is still open.
        if matched.is_none()
            && let (Some(i), Some(last)) = (self.active, self.last_app.as_ref())
            && self.profiles[i].profile.keep_when_unfocused
            && unsafe { windows::Win32::UI::WindowsAndMessaging::IsWindow(Some(windows::Win32::Foundation::HWND(last.hwnd as *mut _))) }.as_bool()
        {
            return;
        }
        let was = self.active;
        if self.asking.is_some_and(|(_, h, _)| h != w.hwnd) {
            self.asking = None;
        }
        self.active = match matched {
            Some(i) if self.forced == Some(i) => Some(i),
            Some(i) => match self.profiles[i].profile.auto_apply {
                AutoApply::Auto => Some(i),
                AutoApply::Ask if self.approved.contains(&w.hwnd) => Some(i),
                AutoApply::Ask => {
                    if self.asking.is_none() && !self.suspended {
                        log!("foreground: {} matches \"{}\", which asks first", w.facts.exe_name(), self.profiles[i].profile.name);
                        self.asking = Some((i, w.hwnd, Instant::now()));
                    }
                    None
                }
                AutoApply::Off => None,
            },
            None => None,
        };
        if self.active != was {
            match self.active {
                Some(i) => {
                    self.recall_layout(i);
                    log!("foreground: {} matches profile \"{}\"", w.facts.exe_name(), self.profiles[i].profile.name);
                    if self.settings.show_apply_toast && self.toasted != Some(w.hwnd) && let Some(l) = self.profiles[i].profile.active() {
                        let text = format!("Kiritori · {} · Ctrl+Alt+K to edit", l.name);
                        log!("toast: {text}");
                        self.toast = Some((text, Instant::now()));
                        self.toasted = Some(w.hwnd);
                    }
                }
                None => log!("foreground: {} has no active profile", w.facts.exe_name()),
            }
        }
        self.last_app = Some(w);
        self.apply_frame_removal();
    }

    /// Remove the active app's window frame if its profile asks for it.
    fn apply_frame_removal(&mut self) {
        let (Some(i), Some(w)) = (self.active, self.last_app.as_ref()) else { return };
        let mode = self.profiles[i].profile.remove_frame;
        if mode == FrameRemoval::Never || self.suspended || self.frames.iter().any(|(h, _)| *h == w.hwnd) {
            return;
        }
        let hwnd = windows::Win32::Foundation::HWND(w.hwnd as *mut _);
        if !system::has_frame(hwnd) {
            return;
        }
        match system::remove_frame(hwnd) {
            Some(saved) => {
                log!("removed the window frame of {}", w.facts.exe_name());
                self.frames.push((w.hwnd, saved));
                self.write_marker();
            }
            None => log!("could not remove the window frame of {} (it may run as administrator)", w.facts.exe_name()),
        }
    }

    /// Put back every frame Kiritori removed.
    pub fn restore_frames(&mut self) {
        for (h, saved) in self.frames.drain(..) {
            system::restore_frame(windows::Win32::Foundation::HWND(h as *mut _), saved);
            log!("restored a window frame");
        }
        self.write_marker();
    }

    /// Keep the run marker up to date with the frames Kiritori has removed, so a crash can be
    /// cleaned up at the next start.
    pub fn write_marker(&self) {
        let frames = self.frames.iter().map(|(h, f)| f.record(*h)).collect();
        self.store.write_marker(&crate::store::RunMarker { pid: std::process::id(), frames });
    }

    /// The last run did not exit cleanly: put back title bars it removed, pause auto-apply and
    /// say why.
    pub fn recover_from(&mut self, m: crate::store::RunMarker) {
        let mut restored = 0;
        for r in &m.frames {
            let (h, saved) = system::SavedFrame::from_record(r);
            let hwnd = windows::Win32::Foundation::HWND(h as *mut _);
            if unsafe { windows::Win32::UI::WindowsAndMessaging::IsWindow(Some(hwnd)) }.as_bool() && !system::has_frame(hwnd) {
                system::restore_frame(hwnd, saved);
                restored += 1;
            }
        }
        self.suspended = true;
        log!("last run (pid {}) did not exit cleanly: auto-apply paused, {restored} window frame(s) put back", m.pid);
        self.notices.push((
            "Kiritori closed unexpectedly last time".into(),
            "Auto-apply is paused for now. Press Ctrl+Alt+B or Ctrl+Alt+K to carry on.".into(),
        ));
    }

    /// The window to capture: the active app, unless switched off.
    pub fn target(&self) -> Option<isize> {
        if self.suspended || self.active.is_none() {
            return None;
        }
        // A minimised app: canvases go away and capture pauses until it is restored.
        let w = self.last_app.as_ref()?;
        if unsafe { windows::Win32::UI::WindowsAndMessaging::IsIconic(windows::Win32::Foundation::HWND(w.hwnd as *mut _)) }.as_bool() {
            return None;
        }
        Some(w.hwnd)
    }

    /// The source's drawing area is `w`×`h`: if its shape differs from the layout's, say so
    /// once. The layout applies anyway, being stored as fractions.
    pub fn check_source_shape(&mut self, w: i32, h: i32) {
        let Some(l) = self.active_layout() else { return };
        if w <= 0 || h <= 0 || l.cutouts.is_empty() {
            return;
        }
        let [aw, ah] = l.source_aspect;
        let stored = aw as f64 / ah.max(1) as f64;
        let now = w as f64 / h as f64;
        let id = l.id.clone();
        if (stored - now).abs() / stored > 0.01 {
            if self.shape_note.as_ref().is_none_or(|(i, sw, sh)| *i != id || *sw != w || *sh != h) {
                log!("source shape changed: layout made at {aw}:{ah}, the app is now {w}×{h}");
                self.toast = Some(("Source shape changed — check your cutouts".into(), Instant::now()));
                self.shape_note = Some((id, w, h));
            }
        }
    }

    /// The aspect ratio of `w`×`h` in lowest terms, for `sourceAspect`.
    pub fn aspect_of(w: i32, h: i32) -> [u32; 2] {
        fn gcd(a: u32, b: u32) -> u32 {
            if b == 0 { a } else { gcd(b, a % b) }
        }
        let (w, h) = (w.max(1) as u32, h.max(1) as u32);
        let g = gcd(w, h).max(1);
        [w / g, h / g]
    }

    pub fn layer_open(&self) -> bool {
        self.layer.is_some()
    }

    pub fn active_layout(&self) -> Option<&Layout> {
        self.profiles[self.active?].profile.active()
    }

    /// The live result on this canvas, or None when nothing should be shown.
    pub fn live_draws(&self, geo: &Geo) -> Option<Vec<Draw>> {
        if self.bypass || self.picking || self.target().is_none() {
            return None;
        }
        let layout = self.active_layout()?;
        if layout.cutouts.is_empty() {
            return None;
        }
        let (cw, ch) = (geo.canvas.w as f32, geo.canvas.h as f32);
        let (_, missing) = self.resolve_displays();
        let mut draws: Vec<Draw> = layout
            .by_z()
            .into_iter()
            .filter(|c| c.placement.visible && c.placement.display == geo.display && !missing.contains(&c.placement.display))
            .map(|c| {
                let p = c.placement.rect;
                Draw::Source {
                    orient: c.orient(),
                    dst: (p.x as f32 * cw, p.y as f32 * ch, p.w as f32 * cw, p.h as f32 * ch),
                    src: c.bounds(),
                    shape: c.rects.clone(),
                    filter: c.placement.filter,
                    dim: 1.0,
                    alpha: 1.0,
                    circle: false,
                    input: true,
                }
            })
            .collect();
        if let Some((text, at)) = &self.toast
            && geo.display == crate::model::layout::MAIN_DISPLAY
        {
            draws.extend(toast_draws(text, at.elapsed(), geo));
        }
        Some(draws)
    }

    /// The active layout's displays among the connected ones, and the keys of those missing.
    pub fn resolve_displays(&self) -> (Vec<crate::model::display::Placed>, Vec<String>) {
        use crate::model::display::{Placed, resolve};
        use crate::model::layout::{DisplayMode, MAIN_DISPLAY};
        let main = || vec![Placed { key: MAIN_DISPLAY.into(), monitor: self.source_monitor, mode: DisplayMode::Takeover }];
        if self.monitors.is_empty() {
            return (main(), Vec::new());
        }
        match self.active_layout() {
            Some(l) => resolve(l, &self.monitors, self.source_monitor.min(self.monitors.len() - 1)),
            None => (main(), Vec::new()),
        }
    }

    /// Canvas sizes of the active layout's connected displays, by key.
    fn display_sizes(&self) -> std::collections::BTreeMap<String, crate::model::geometry::Size> {
        let (placed, _) = self.resolve_displays();
        placed.iter().filter_map(|p| self.monitors.get(p.monitor).map(|m| (p.key.clone(), m.size()))).collect()
    }

    /// Before drawing the layer: the display map, parked cutouts, and the toast for parked
    /// cutouts once per change.
    pub fn refresh_displays(&mut self) {
        use crate::layer::displays::MapDisplay;
        let (placed, missing) = self.resolve_displays();
        let Some(layout) = self.active_layout().cloned() else { return };
        let parked = layout.parked(&missing);
        let note = (layout.id.clone(), missing.clone());
        if parked.is_empty() {
            self.parked_note = None;
        } else if self.parked_note.as_ref() != Some(&note) {
            let names: Vec<String> = missing.iter().filter_map(|k| layout.displays.get(k)).map(|d| format!("\"{}\"", d.label)).collect();
            let text = format!("Display {} not connected · {} cutout{} parked", names.join(", "), parked.len(), if parked.len() == 1 { "" } else { "s" });
            log!("{text}");
            self.toast = Some((text, Instant::now()));
            self.parked_note = Some(note);
        }
        let here = self.here.clone();
        let map: Vec<MapDisplay> = self
            .monitors
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let p = placed.iter().find(|p| p.monitor == i);
                let cutouts = p
                    .map(|p| layout.cutouts.iter().filter(|c| c.placement.display == p.key && c.placement.visible).map(|c| (c.placement.rect, crate::layer::style::cutout(c.color))).collect())
                    .unwrap_or_default();
                MapDisplay { number: m.number, rect: m.rect, mode: p.map(|p| p.mode), here: p.is_some_and(|p| p.key == here), cutouts }
            })
            .collect();
        if let Some(l) = self.layer.as_mut() {
            l.map = map;
            l.parked = parked;
        }
    }

    /// Outside the layer: the parked-cutouts toast only.
    pub fn refresh_displays_quiet(&mut self) {
        self.refresh_displays();
    }

    /// Another display's drawing while the layer is open; a cutout dragged over it shows
    /// where it would land.
    pub fn layer_side_draws(&mut self, geo: &Geo, monitor: usize) -> Vec<Draw> {
        use crate::engine::draw::Align;
        use crate::layer::style::*;
        let Some(layout) = self.active_layout().cloned() else { return Vec::new() };
        let drag = self.layer.as_ref().and_then(|l| l.arr_dragging_number());
        let Some(l) = self.layer.as_mut() else { return Vec::new() };
        let mut out = l.draw_side(&layout, geo);
        if let (Some((m, (x, y))), Some(n)) = (self.drag_over, drag)
            && m == monitor
            && let Some(c) = layout.cutout(n)
        {
            let k = geo.scale;
            let src = c.pixel_size(geo.source_px);
            let (w, h) = ((src.w as f32).min(geo.canvas.w as f32), (src.h as f32).min(geo.canvas.h as f32));
            let r = (x - w / 2.0, y - h / 2.0, w, h);
            out.push(Draw::Fill { r, color: accent_fill(), radius: 0.0 });
            out.push(Draw::Stroke { r, color: cutout(c.color), width: 3.0 * k, radius: 0.0, dashed: true });
            out.push(Draw::Text { r, text: format!("Release to send {} here", c.name), size: 18.0 * k, color: fg(), bold: true, align: Align::Center });
        }
        out
    }

    /// A display not in use, while the layer is open: a glass card in its centre.
    pub fn card_draws(&self, monitor: usize, k: f32, w: f32, h: f32) -> Vec<Draw> {
        use crate::engine::draw::Align;
        use crate::layer::style::*;
        let Some(m) = self.monitors.get(monitor) else { return Vec::new() };
        let mut bg = glass();
        bg[3] = 1.0;
        let over = self.drag_over.is_some_and(|(d, _)| d == monitor);
        vec![
            Draw::Fill { r: (0.0, 0.0, w, h), color: bg, radius: 0.0 },
            Draw::Stroke { r: (1.0, 1.0, w - 2.0, h - 2.0), color: if over { accent() } else { hairline() }, width: if over { 3.0 * k } else { 1.0 }, radius: 0.0, dashed: false },
            Draw::Text { r: (24.0 * k, 16.0 * k, 80.0 * k, h - 32.0 * k), text: m.number.to_string(), size: 56.0 * k, color: fg(), bold: true, align: Align::Center },
            Draw::Text { r: (120.0 * k, 22.0 * k, w - 140.0 * k, 30.0 * k), text: format!("{} · Not in use", m.label()), size: 18.0 * k, color: fg(), bold: true, align: Align::Left },
            Draw::Text {
                r: (120.0 * k, 56.0 * k, w - 140.0 * k, 26.0 * k),
                text: if over { "Release to send the cutout here".into() } else { "Click to use, or drag a cutout here".into() },
                size: 14.0 * k,
                color: fg_muted(),
                bold: false,
                align: Align::Left,
            },
        ]
    }

    /// The layout key a connected display has in the active layout, if it is in use.
    pub fn key_of_monitor(&self, m: usize) -> Option<String> {
        let (placed, _) = self.resolve_displays();
        placed.iter().find(|p| p.monitor == m).map(|p| p.key.clone())
    }

    /// Send cutout `n` to connected display `m`, centred on `at` (display fractions) or in the
    /// middle. A display not in use needs Takeover or Float first: returns false and leaves
    /// the question in `pending_send`.
    pub fn send_to_monitor(&mut self, n: u32, m: usize, at: Option<(f64, f64)>, mode: Option<crate::model::layout::DisplayMode>, geo: &Geo) -> bool {
        let Some(mon) = self.monitors.get(m).cloned() else { return false };
        let is_source = m == self.source_monitor;
        if self.key_of_monitor(m).is_none() && !is_source && mode.is_none() {
            self.pending_send = Some((n, m, at));
            return false;
        }
        let sizes = self.display_sizes();
        let mode = mode.unwrap_or(crate::model::layout::DisplayMode::Takeover);
        self.edit(geo, |layer, layout, _| {
            let Some(c) = layout.cutout(n) else { return Effect::None };
            let name = c.name.clone();
            let was = c.placement.display.clone();
            let from = sizes.get(&was).copied().unwrap_or(mon.size());
            let key = layout.display_key(&mon, is_source, mode);
            if was == key && at.is_none() {
                return Effect::None;
            }
            layout.send_to(n, &key, from, mon.size(), at);
            layer.selected = Some(n);
            layer.say(&format!("{name} → {}", mon.label()));
            Effect::Edit(format!("Send {name} to {}", mon.label()), None)
        });
        true
    }

    /// Suggest or Trim, now that the picture's pixels have arrived.
    pub fn run_pixel_job(&mut self, snap: crate::engine::draw::Snapshot, geo: &Geo) {
        use crate::model::suggest::{Luma, suggest, trim};
        let Some(job) = self.pixel_job.take() else { return };
        let luma = Luma::from_bgra(snap.w, snap.h, &snap.bgra, 640);
        match job {
            Effect::Suggest => {
                let found = suggest(&luma);
                self.edit(geo, |layer, layout, geo| {
                    // Leave out panels an existing cutout already covers.
                    let fresh: Vec<_> = found
                        .iter()
                        .filter(|r| !layout.cutouts.iter().any(|c| {
                            let i = c.bounds().intersect(r);
                            i.w * i.h > 0.5 * r.w * r.h
                        }))
                        .copied()
                        .collect();
                    if fresh.is_empty() {
                        layer.say(if found.is_empty() { "No panels found in this picture" } else { "Every panel already has a cutout" });
                        return Effect::None;
                    }
                    let f = geo.frame();
                    let mut first = None;
                    for r in &fresh {
                        let n = layout.add_cutout(*r, &f);
                        first.get_or_insert(n);
                    }
                    layer.selected = first;
                    layer.say(&format!("{} cutout{} suggested · rename them with Enter, delete with Del", fresh.len(), if fresh.len() == 1 { "" } else { "s" }));
                    Effect::Edit(format!("Suggest {} cutout{}", fresh.len(), if fresh.len() == 1 { "" } else { "s" }), None)
                });
            }
            Effect::Trim(n) => {
                self.edit(geo, |layer, layout, geo| {
                    let Some(c) = layout.cutout(n) else { return Effect::None };
                    let b = c.bounds();
                    let name = c.name.clone();
                    match trim(&luma, b) {
                        Some(t) if t != b => {
                            if !layout.trim_to(n, t, &geo.frame()) {
                                layer.say("Trim works on rectangular cutouts");
                                return Effect::None;
                            }
                            layer.say(&format!("Trimmed {name}"));
                            Effect::Edit(format!("Trim {name}"), None)
                        }
                        Some(_) => {
                            layer.say(&format!("{name} has no black edges"));
                            Effect::None
                        }
                        None => {
                            layer.say(&format!("{name} is all black"));
                            Effect::None
                        }
                    }
                });
            }
            _ => {}
        }
    }

    /// Settings.
    pub fn open_settings(&mut self) {
        self.panel = None;
        if self.settings_panel.is_none() {
            self.settings_panel = Some(crate::settings::SettingsPanel::default());
            log!("settings opened");
        }
    }

    /// Shortcuts from the tray: the shortcut map over the layer when there is an app to edit;
    /// otherwise Settings, which lists the global shortcuts.
    pub fn open_shortcuts(&mut self) {
        if self.last_app.is_some() {
            if self.layer.is_none() {
                self.toggle_layer();
            }
            if let Some(l) = self.layer.as_mut() {
                l.keymap_open = true;
                return;
            }
        }
        self.open_settings();
    }

    /// The global shortcuts as they are set now.
    pub fn bindings(&self) -> crate::model::shortcut::Bindings {
        crate::model::shortcut::Bindings::from_settings(&self.settings.shortcuts)
    }

    fn settings_view_parts(&self) -> (crate::store::Theme, bool, bool, bool, String) {
        let s = &self.settings;
        (s.theme, s.start_with_windows, s.show_apply_toast, s.performance, self.self_test.clone().unwrap_or_default())
    }

    pub fn settings_draws(&mut self, k: f32, w: f32, h: f32) -> Vec<Draw> {
        let b = self.bindings();
        let (theme, sw, toast, perf, test) = self.settings_view_parts();
        let taken = self.taken.clone();
        let types = |c: crate::model::shortcut::Combo| system::types_character(c);
        let v = crate::settings::View { theme, start_with_windows: sw, toast, performance: perf, bindings: &b, taken: &taken, types: &types, self_test: &test };
        match self.settings_panel.as_mut() {
            Some(p) => p.draw(&v, k, w, h),
            None => Vec::new(),
        }
    }

    pub fn settings_input(&mut self, input: Input) {
        use crate::settings::Op;
        let b = self.bindings();
        let (theme, sw, toast, perf, test) = self.settings_view_parts();
        let taken = self.taken.clone();
        let types = |c: crate::model::shortcut::Combo| system::types_character(c);
        let v = crate::settings::View { theme, start_with_windows: sw, toast, performance: perf, bindings: &b, taken: &taken, types: &types, self_test: &test };
        let Some(p) = self.settings_panel.as_mut() else { return };
        let op = p.input(input, &v);
        match op {
            Op::None | Op::StartRecording(..) | Op::StopRecording => {}
            Op::Close => {
                self.settings_panel = None;
                log!("settings closed");
            }
            Op::Theme(t) => {
                self.settings.theme = t;
                self.save_settings();
                self.refresh_theme();
            }
            Op::StartWithWindows(on) => self.set_start_with_windows(on),
            Op::Toast(on) => {
                self.settings.show_apply_toast = on;
                self.save_settings();
            }
            Op::Performance(on) => {
                self.settings.performance = on;
                self.save_settings();
            }
            Op::Shortcut(g, slot, c) => {
                let mut b = self.bindings();
                let mut slots = b.get(g);
                slots[slot] = c;
                if slots[0].is_none() {
                    slots = [slots[1].or(Some(g.default_combo())), None];
                }
                b.0.insert(g, slots);
                self.settings.shortcuts = b.to_settings();
                self.save_settings();
                self.hotkeys_dirty = true;
                log!("shortcut for {}: {}", g.label(), slots.iter().flatten().map(|c| c.to_string()).collect::<Vec<_>>().join(" or "));
            }
            Op::ResetShortcuts => {
                self.settings.shortcuts.clear();
                self.save_settings();
                self.hotkeys_dirty = true;
                if let Some(p) = self.settings_panel.as_mut() {
                    p.status = Some("Shortcuts are back to their defaults".into());
                }
            }
            Op::About => self.show_about = true,
        }
    }

    /// Whether a shortcut is being recorded (the shell pauses its hotkeys meanwhile).
    pub fn recording_shortcut(&self) -> bool {
        self.settings_panel.as_ref().is_some_and(|p| p.recording.is_some())
    }

    fn save_settings(&mut self) {
        if let Err(e) = self.store.save_settings(&self.settings) {
            log!("could not save settings: {e}");
        }
    }

    /// Mirror the selected cutout (right-click menu).
    pub fn mirror_selected(&mut self, geo: &Geo) {
        self.edit(geo, |layer, layout, _| {
            let Some(n) = layer.selected.filter(|n| layout.cutout(*n).is_some()) else { return Effect::None };
            layout.mirror(n);
            Effect::Edit(format!("Mirror {}", layout.cutout(n).unwrap().name), None)
        });
    }

    /// D: the next connected display after the one the cutout is on, cycling.
    pub fn next_monitor_for(&self, n: u32) -> Option<usize> {
        let layout = self.active_layout()?;
        let key = &layout.cutout(n)?.placement.display;
        let (placed, _) = self.resolve_displays();
        let now = placed.iter().find(|p| &p.key == key).map(|p| p.monitor).unwrap_or(self.source_monitor);
        (self.monitors.len() > 1).then(|| (now + 1) % self.monitors.len())
    }

    /// Takeover, Float or Not in use for connected display `m` (the display map, or a card).
    pub fn set_monitor_mode(&mut self, m: usize, mode: Option<crate::model::layout::DisplayMode>, geo: &Geo) {
        let Some(mon) = self.monitors.get(m).cloned() else { return };
        let is_source = m == self.source_monitor;
        let key = self.key_of_monitor(m);
        let sizes = self.display_sizes();
        if is_source && mode.is_none() {
            if let Some(l) = self.layer.as_mut() {
                l.say("The display with the app stays in use; choose Float to see the app there");
            }
            return;
        }
        let label = crate::layer::displays::mode_label(mode);
        self.edit(geo, |layer, layout, _| {
            let changed = match (key, mode) {
                (None, Some(md)) => {
                    layout.display_key(&mon, is_source, md);
                    true
                }
                (Some(k), md) => layout.set_display_mode(&k, md, &sizes),
                (None, None) => false,
            };
            if !changed {
                return Effect::None;
            }
            layer.say(&format!("{}: {label}", mon.label()));
            Effect::Edit(format!("{} {label}", mon.label()), None)
        });
    }

    /// The displays changed (plugged, unplugged, rearranged): refresh, and use the layout last
    /// used with this set of displays.
    pub fn displays_changed(&mut self, monitors: Vec<crate::model::display::Monitor>) {
        log!("displays: {}", monitors.iter().map(|m| format!("{} {}×{}@{}Hz at {},{} ×{}", m.label(), m.rect.2, m.rect.3, m.hz, m.rect.0, m.rect.1, m.scale)).collect::<Vec<_>>().join(", "));
        for m in &monitors {
            log!("  {}: {} ({})", m.label(), m.name, m.id);
        }
        self.monitors = monitors;
        self.source_monitor = self.source_monitor.min(self.monitors.len().saturating_sub(1));
        if let Some(i) = self.active {
            self.recall_layout(i);
        }
    }

    /// Use the layout last used with the connected set of displays, if there is one.
    fn recall_layout(&mut self, i: usize) {
        if self.monitors.is_empty() || self.layer.is_some() {
            return;
        }
        let set = crate::model::display::set_key(&self.monitors);
        let p = &mut self.profiles[i].profile;
        if let Some(id) = p.layout_for_displays.get(&set).cloned()
            && id != p.default_layout
            && p.layout(&id).is_some()
        {
            log!("layout {} was last used with these displays", p.layout(&id).map(|l| l.name.as_str()).unwrap_or("?"));
            p.default_layout = id;
        }
    }

    /// The layer's drawing on this canvas.
    pub fn layer_draws(&mut self, geo: &Geo) -> Vec<Draw> {
        let Some(i) = self.active else { return Vec::new() };
        let title = self.profiles[i].profile.name.clone();
        let Some(layout) = self.profiles[i].profile.active().cloned() else { return Vec::new() };
        let key = format!("{}/{}", self.profiles[i].stem, layout.id);
        let names = self.histories.get(&key).map(|h| (h.undo_name().map(String::from), h.redo_name().map(String::from))).unwrap_or_default();
        let list: Vec<String> = self.histories.get(&key).map(|h| h.names().into_iter().map(String::from).collect()).unwrap_or_default();
        let card = self.tutorial.as_ref().and_then(|t| {
            let s = t.step()?;
            Some(crate::layer::guide::GuideCard { index: t.index, total: crate::tutorial::Step::ALL.len(), title: s.title(), body: s.body(), key: s.key(), target: s.target() })
        });
        if let Some(l) = self.layer.as_mut() {
            l.history_names = names;
            l.history_list = list;
            l.guide = card;
        }
        match self.layer.as_mut() {
            Some(l) => l.draw(&layout, geo, &title),
            None => Vec::new(),
        }
    }

    /// 0 shows the moving picture; otherwise the still frame of that generation.
    pub fn freeze_generation(&self) -> u64 {
        match &self.layer {
            Some(l) if !l.wants_live() => l.freeze,
            _ => 0,
        }
    }

    /// Something time-based is on screen (a glide, tooltip, pulse or the toast): redraw on a timer.
    /// Open the Profiles panel (P in the layer, or the tray), on the active profile.
    pub fn open_profiles(&mut self) {
        self.settings_panel = None;
        if self.panel.is_none() {
            self.panel = Some(crate::panel::ProfilesPanel::new(self.active.unwrap_or(0)));
            self.panel_checked = None;
            self.refresh_backups();
            log!("profiles panel opened");
        }
    }

    pub fn close_profiles(&mut self) {
        if self.panel.take().is_some() {
            log!("profiles panel closed");
        }
    }

    fn refresh_backups(&mut self) {
        self.panel_backups = self.profiles.iter().map(|p| self.store.load_backup(&p.stem).is_some()).collect();
    }

    /// The panel's drawing, at chrome scale `k` in a window of `w`×`h` pixels. Refreshes the
    /// "Matches now" line once a second.
    pub fn panel_draws(&mut self, k: f32, w: f32, h: f32) -> Vec<Draw> {
        let Some(panel) = self.panel.as_mut() else { return Vec::new() };
        if self.panel_checked.is_none_or(|t| t.elapsed() >= std::time::Duration::from_secs(1)) {
            panel.matches = self.profiles.get(panel.selected).map(|p| system::matching_titles(&p.profile.match_rule)).unwrap_or_default();
            self.panel_checked = Some(Instant::now());
        }
        let entries: Vec<crate::panel::Entry> =
            self.profiles.iter().enumerate().map(|(i, p)| crate::panel::Entry { profile: &p.profile, has_backup: self.panel_backups.get(i).copied().unwrap_or(false) }).collect();
        panel.draw(&entries, k, w, h)
    }

    /// Input on the panel window.
    pub fn panel_input(&mut self, input: Input, k: f32) {
        let Some(panel) = self.panel.as_mut() else { return };
        let before = panel.selected;
        let entries: Vec<crate::panel::Entry> =
            self.profiles.iter().enumerate().map(|(i, p)| crate::panel::Entry { profile: &p.profile, has_backup: self.panel_backups.get(i).copied().unwrap_or(false) }).collect();
        let op = panel.input(input, k, &entries);
        if panel.selected != before {
            self.panel_checked = None;
        }
        self.panel_op(op);
    }

    fn panel_say(&mut self, text: String) {
        log!("profiles panel: {text}");
        if let Some(p) = self.panel.as_mut() {
            p.status = Some(text);
        }
    }

    /// Carry out a change made in the Profiles panel, and save it.
    pub fn panel_op(&mut self, op: crate::panel::Op) {
        use crate::panel::Op;
        match op {
            Op::None => {}
            Op::Close => self.close_profiles(),
            Op::Rename(i, name) => {
                let old = std::mem::replace(&mut self.profiles[i].profile.name, name.clone());
                self.save(i);
                self.panel_say(format!("Renamed \"{old}\" to \"{name}\""));
            }
            Op::TitleRule(i, t) => {
                self.profiles[i].profile.match_rule.title_contains = t.clone();
                self.save(i);
                self.panel_checked = None;
                self.panel_say(if t.is_empty() { "Matches any window title".into() } else { format!("Matches only titles containing \"{t}\"") });
                self.reevaluate();
            }
            Op::AutoApply(i, a) => {
                self.profiles[i].profile.auto_apply = a;
                self.save(i);
                self.panel_say(format!("Apply the layout: {}", crate::panel::auto_apply_label(a)));
                self.reevaluate();
            }
            Op::Frame(i, f) => {
                self.profiles[i].profile.remove_frame = f;
                self.save(i);
                if f == FrameRemoval::Never {
                    self.restore_frames();
                } else {
                    self.apply_frame_removal();
                }
                self.panel_say(if f == FrameRemoval::Never { "The title bar is left alone".into() } else { "The title bar is removed when the app has one".to_string() });
            }
            Op::KeepUnfocused(i, on) => {
                self.profiles[i].profile.keep_when_unfocused = on;
                self.save(i);
                self.panel_say(if on { "Keeps showing when the app is not in front".into() } else { "Hides when the app is not in front".to_string() });
            }
            Op::SetDefault(i, id) => {
                self.set_layout(i, &id);
                let name = self.profiles[i].profile.active().map(|l| l.name.clone()).unwrap_or_default();
                self.panel_say(format!("Default layout: {name}"));
            }
            Op::RenameLayout(i, id, name) => {
                if self.profiles[i].profile.rename_layout(&id, &name).is_some() {
                    self.save(i);
                    self.panel_say(format!("Layout renamed to \"{name}\""));
                }
            }
            Op::DuplicateLayout(i, id) => {
                if let Some(new) = self.profiles[i].profile.duplicate_layout(&id) {
                    self.save(i);
                    let name = self.profiles[i].profile.layout(&new).map(|l| l.name.clone()).unwrap_or_default();
                    self.panel_say(format!("Made \"{name}\""));
                }
            }
            Op::DeleteLayout(i, id) => {
                if self.active == Some(i) && self.profiles[i].profile.active_id().as_deref() == Some(id.as_str()) {
                    self.layer = None; // the layer was editing the layout that goes
                }
                let name = self.profiles[i].profile.layout(&id).map(|l| l.name.clone()).unwrap_or_default();
                if self.profiles[i].profile.delete_layout(&id) {
                    self.save(i);
                    self.panel_say(format!("Deleted the layout \"{name}\""));
                }
            }
            Op::NewLayout(i) => {
                let was = self.active;
                self.active = Some(i);
                self.new_layout();
                if was != Some(i) {
                    self.active = was;
                }
                let name = self.profiles[i].profile.active().map(|l| l.name.clone()).unwrap_or_default();
                self.panel_say(format!("New layout \"{name}\", a copy of the default, is now the default"));
            }
            Op::DeleteProfile(i) => self.delete_profile(i),
            Op::RestorePrevious(i) => match self.store.load_backup(&self.profiles[i].stem) {
                Some(p) => {
                    if self.active == Some(i) {
                        self.layer = None;
                    }
                    self.profiles[i].profile = p;
                    self.save(i);
                    self.histories.retain(|k, _| !k.starts_with(&format!("{}/", self.profiles[i].stem)));
                    self.panel_say("Restored the profile as it was at the start of the last session".into());
                }
                None => self.panel_say("There is no previous session to restore".into()),
            },
            Op::Pointer(i, p) => {
                self.profiles[i].profile.pointer = p;
                self.save(i);
                self.panel_say(format!("Pointer: {}", crate::panel::pointer_label(p)));
            }
            Op::Export(i) => self.export_profile(i),
            Op::Import => self.import_profile(),
            Op::NewProfile => self.dialog = Some(Dialog::NewProfile),
        }
    }

    /// Make a profile for an open window, chosen from the Profiles panel, and select it. If one
    /// already matches the window, select that one instead.
    pub fn new_profile_for(&mut self, facts: &WindowFacts, display: crate::model::layout::DisplayRef) -> usize {
        if let Some(i) = self.find_profile(facts) {
            self.select_in_panel(i);
            self.panel_say(format!("\"{}\" already has a profile: \"{}\"", facts.exe_name(), self.profiles[i].profile.name));
            return i;
        }
        let taken: Vec<String> = self.profiles.iter().map(|p| p.profile.name.clone()).collect();
        let profile = Profile::new_for(facts, display, &taken);
        let stem = self.store.new_stem(&profile.name);
        log!("new profile \"{}\" for {} ({stem}.json), from the Profiles panel", profile.name, facts.exe_name());
        let name = profile.name.clone();
        self.profiles.push(StoredProfile { stem, profile });
        let i = self.profiles.len() - 1;
        self.save(i);
        self.refresh_backups();
        self.select_in_panel(i);
        self.panel_say(format!("Made the profile \"{name}\". Bring {} to the front and press Ctrl+Alt+K to cut it out.", facts.exe_name()));
        i
    }

    fn select_in_panel(&mut self, i: usize) {
        let n = self.profiles.len();
        if let Some(p) = self.panel.as_mut() {
            p.select_index(i, n);
        }
        self.panel_checked = None;
    }

    /// After a match rule or auto-apply change, apply (or stop applying) to the app in front.
    fn reevaluate(&mut self) {
        if self.layer.is_some() {
            return;
        }
        if let Some(w) = self.last_app.clone() {
            self.toasted = Some(w.hwnd); // no toast for a change the user just made
            let keep = self.active;
            self.active = None;
            let _ = keep;
            self.on_foreground(Some(w));
        }
    }

    /// Delete a profile and its file; indices of the others shift down.
    pub fn delete_profile(&mut self, i: usize) {
        if i >= self.profiles.len() {
            return;
        }
        if self.active == Some(i) {
            self.layer = None;
            self.restore_frames();
        }
        let gone = self.profiles.remove(i);
        let _ = self.store.delete_profile(&gone.stem);
        self.histories.retain(|k, _| !k.starts_with(&format!("{}/", gone.stem)));
        let shift = |x: Option<usize>| match x {
            Some(j) if j == i => None,
            Some(j) if j > i => Some(j - 1),
            other => other,
        };
        self.active = shift(self.active);
        self.forced = shift(self.forced);
        self.refresh_backups();
        if let Some(p) = self.panel.as_mut() {
            p.selected = p.selected.min(self.profiles.len().saturating_sub(1));
        }
        self.panel_checked = None;
        log!("deleted profile \"{}\" ({}.json; a copy is in backups)", gone.profile.name, gone.stem);
        self.panel_say(format!("Deleted \"{}\". A copy is in backups\\{}.deleted.json: Import… brings it back.", gone.profile.name, gone.stem));
    }

    /// Ask the shell for the Save dialog.
    pub fn export_profile(&mut self, i: usize) {
        self.dialog = Some(Dialog::Export(i));
    }

    /// Ask the shell for the Open dialog.
    pub fn import_profile(&mut self) {
        self.dialog = Some(Dialog::Import);
    }

    /// Write profile `i` to a file to share.
    pub fn export_to(&mut self, i: usize, path: &str) -> Result<(), String> {
        let p = self.profiles.get(i).ok_or("no such profile")?;
        match Store::export_profile(&p.profile, std::path::Path::new(path)) {
            Ok(()) => {
                let name = p.profile.name.clone();
                self.panel_say(format!("Exported \"{name}\" to {path}"));
                Ok(())
            }
            Err(e) => {
                let text = format!("Could not export to {path}: {e}");
                self.panel_say(text.clone());
                Err(text)
            }
        }
    }

    /// Add a profile from a shared file. A name already in use gets " (2)", " (3)"…; the
    /// file is saved under a new name, so nothing is overwritten.
    pub fn import_from(&mut self, path: &str) -> Result<usize, String> {
        let mut p = match Store::read_profile_file(std::path::Path::new(path)) {
            Ok(p) => p,
            Err(e) => {
                let text = format!("Could not import {path}: {e}");
                self.panel_say(text.clone());
                return Err(text);
            }
        };
        let names: Vec<String> = self.profiles.iter().map(|p| p.profile.name.clone()).collect();
        if names.contains(&p.name) {
            let base = p.name.clone();
            p.name = (2..).map(|n| format!("{base} ({n})")).find(|n| !names.contains(n)).unwrap();
        }
        let stem = self.store.new_stem(&p.name);
        let name = p.name.clone();
        self.profiles.push(StoredProfile { stem: stem.clone(), profile: p });
        let i = self.profiles.len() - 1;
        self.save(i);
        self.refresh_backups();
        if let Some(panel) = self.panel.as_mut() {
            panel.selected = i;
        }
        self.panel_checked = None;
        log!("imported \"{name}\" from {path} ({stem}.json)");
        self.panel_say(format!("Imported \"{name}\""));
        self.reevaluate();
        Ok(i)
    }

    pub fn animating(&mut self) -> bool {
        if self.panel.is_some() || self.settings_panel.is_some() || self.asking.is_some_and(|(_, _, t)| t.elapsed() < ASK_SHOW) || self.elevation_card.is_some() {
            return true;
        }
        if self.toast.as_ref().is_some_and(|(_, at)| at.elapsed() >= TOAST_TOTAL) {
            self.toast = None;
        }
        self.toast.is_some() || self.layer.as_ref().is_some_and(|l| l.needs_tick())
    }

    /// The active profile's layouts, as (id, name, is active).
    pub fn layouts(&self) -> Vec<(String, String, bool)> {
        let Some(i) = self.active else { return Vec::new() };
        let p = &self.profiles[i].profile;
        let active = p.active_id();
        p.layouts.iter().map(|l| (l.id.clone(), l.name.clone(), Some(&l.id) == active.as_ref())).collect()
    }

    /// Switch the active profile to another layout (from the readout or the tray).
    pub fn switch_layout(&mut self, id: &str) {
        let Some(i) = self.active else { return };
        self.set_layout(i, id);
        if let Some(l) = self.layer.as_mut() {
            l.selected = None;
            l.naming = None;
            let name = self.profiles[i].profile.active().map(|l| l.name.clone()).unwrap_or_default();
            l.say(&format!("Layout: {name}"));
        }
    }

    /// New layout: a copy of the current one, which becomes active.
    pub fn new_layout(&mut self) {
        let Some(i) = self.active else { return };
        let p = &mut self.profiles[i].profile;
        let Some(current) = p.active().cloned() else { return };
        let names: Vec<String> = p.layouts.iter().map(|l| l.name.clone()).collect();
        let name = (1..).map(|n| format!("Layout {n}")).find(|n| !names.contains(n)).unwrap();
        let ids: Vec<String> = p.layouts.iter().map(|l| l.id.clone()).collect();
        let id = crate::model::profile::unique(&crate::model::profile::slug(&name, "layout"), &ids);
        let mut copy = current.clone();
        copy.id = id.clone();
        copy.name = name.clone();
        p.layouts.push(copy);
        p.default_layout = id;
        log!("new layout {name} (a copy of {})", current.name);
        self.save(i);
        if let Some(l) = self.layer.as_mut() {
            l.selected = None;
            l.say(&format!("New layout: {name}, a copy of {}", current.name));
        }
    }

    /// Start renaming the active layout in the readout.
    pub fn begin_rename_layout(&mut self) {
        let name = self.active_layout().map(|l| l.name.clone()).unwrap_or_default();
        if let Some(l) = self.layer.as_mut() {
            l.rename_layout(&name);
        }
    }

    /// Run something that may edit the active layout, then record it and save.
    pub fn edit(&mut self, geo: &Geo, f: impl FnOnce(&mut Layer, &mut Layout, &Geo) -> Effect) -> Effect {
        let Some(i) = self.active else { return Effect::None };
        let Some(id) = self.profiles[i].profile.active_id() else { return Effect::None };
        let Some(layer) = self.layer.as_mut() else { return Effect::None };
        let layout = self.profiles[i].profile.layout_mut(&id).unwrap();
        let mut before = layout.clone();
        let was_dragging = layer.dragging();
        let fx = f(layer, layout, geo);
        if !was_dragging && layer.dragging() {
            self.drag_before = Some(before.clone());
        }
        if was_dragging && !layer.dragging() {
            match (&fx, self.drag_before.take()) {
                // The drag finished: the step runs from where the drag began.
                (Effect::Edit(..), Some(start)) => before = start,
                // Cancelled (Esc, lost focus) or no change: put the layout back as it was.
                (_, Some(start)) => *layout = start,
                _ => {}
            }
        }
        if matches!(fx, Effect::Edit(..)) && self.source_size.0 > 0 {
            let aspect = Self::aspect_of(self.source_size.0, self.source_size.1);
            if let Some(l) = self.profiles[i].profile.layout_mut(&id) {
                l.source_aspect = aspect;
            }
        }
        let key = format!("{}/{}", self.profiles[i].stem, id);
        // In Arrange view, whole-layout moves glide so the user can follow what moved.
        let glides = match &fx {
            Effect::Undo | Effect::Redo | Effect::UndoMany(_) => true,
            Effect::Edit(name, _) => ["Auto-arrange", "Fill", "Restore", "Reset placement", "Reset arrangement"].iter().any(|p| name.starts_with(p)),
            _ => false,
        };
        match &fx {
            Effect::Edit(name, merge) => {
                let after = self.profiles[i].profile.layout(&id).unwrap().clone();
                self.histories.entry(key).or_default().record(name, before.clone(), after, merge.clone(), Instant::now());
                log!("edit: {name}");
                self.save(i);
            }
            Effect::Undo | Effect::Redo => {
                let h = self.histories.entry(key).or_default();
                let step = if fx == Effect::Undo { h.undo() } else { h.redo() };
                match step {
                    Some((name, l)) => {
                        *self.profiles[i].profile.layout_mut(&id).unwrap() = l;
                        let word = if fx == Effect::Undo { "Undo" } else { "Redo" };
                        log!("{word}: {name}");
                        if let Some(layer) = self.layer.as_mut() {
                            layer.say(&format!("{word}: {name}"));
                        }
                        self.save(i);
                    }
                    None => {
                        if let Some(layer) = self.layer.as_mut() {
                            layer.say(if fx == Effect::Undo { "Nothing to undo" } else { "Nothing to redo" });
                        }
                    }
                }
            }
            Effect::UndoMany(n) => {
                let h = self.histories.entry(key).or_default();
                let mut last = None;
                for _ in 0..*n {
                    match h.undo() {
                        Some(step) => last = Some(step),
                        None => break,
                    }
                }
                if let Some((name, l)) = last {
                    *self.profiles[i].profile.layout_mut(&id).unwrap() = l;
                    log!("history: back to before {name} ({n} steps)");
                    if let Some(layer) = self.layer.as_mut() {
                        layer.say(&format!("Back to before \u{201c}{name}\u{201d} · Ctrl+Y to redo"));
                    }
                    self.save(i);
                }
            }
            Effect::RenameLayout(name) => {
                let old = self.profiles[i].stem.clone() + "/" + &id;
                if let Some(new_id) = self.profiles[i].profile.rename_layout(&id, name) {
                    // History follows the layout to its new id.
                    if let Some(h) = self.histories.remove(&old) {
                        self.histories.insert(format!("{}/{}", self.profiles[i].stem, new_id), h);
                    }
                    log!("layout renamed to {name}");
                    self.save(i);
                    if let Some(layer) = self.layer.as_mut() {
                        layer.say(&format!("Layout renamed to {name}"));
                    }
                }
            }
            Effect::Retake => {
                self.freeze += 1;
                if let Some(layer) = self.layer.as_mut() {
                    layer.freeze = self.freeze;
                }
            }
            _ => {}
        }
        if glides && let Some(layer) = self.layer.as_mut() && layer.view == crate::layer::View::Arrange {
            layer.start_glide(&before);
        }
        self.observe_tutorial(&fx);
        fx
    }

    /// Advance the tutorial from what just happened, and handle its card's buttons.
    fn observe_tutorial(&mut self, fx: &Effect) {
        use crate::tutorial::Seen;
        let Some(t) = self.tutorial.as_mut() else { return };
        match fx {
            Effect::Guide(crate::layer::guide::GuideClick::SkipStep) => t.skip(),
            Effect::Guide(crate::layer::guide::GuideClick::End) => {
                self.end_tutorial("Tutorial ended. Replay it any time from the tray menu.");
                return;
            }
            Effect::Edit(name, _) => {
                t.observe(&Seen::Edit(name));
            }
            _ => {}
        }
        if let Some(l) = &self.layer {
            t.observe(&Seen::Layer { view: l.view, naming: l.naming.is_some(), peek: l.peek });
        }
        if t.finished() {
            self.end_tutorial("You're set up. Replay the tutorial any time from the tray menu.");
        }
    }

    fn end_tutorial(&mut self, message: &str) {
        self.tutorial = None;
        if let Some(l) = self.layer.as_mut() {
            l.guide = None;
        }
        if !self.settings.layer_hints_done {
            self.settings.layer_hints_done = true;
            let _ = self.store.save_settings(&self.settings);
        }
        log!("tutorial: {message}");
        self.notices.push(("Kiritori".into(), message.into()));
    }

    /// Start (or restart) the tutorial; opens the layer if it can.
    pub fn start_tutorial(&mut self) {
        self.tutorial = Some(crate::tutorial::Tutorial::default());
        log!("tutorial started");
        if self.layer.is_none() {
            self.toggle_layer();
        }
    }

    pub fn layer_input(&mut self, input: Input, geo: &Geo) -> Effect {
        self.edit(geo, |l, layout, g| l.handle(input, layout, g))
    }

    pub fn add_preset(&mut self, p: Preset, geo: &Geo) -> Effect {
        self.edit(geo, |l, layout, g| l.add_preset(p, layout, g))
    }

    pub fn set_filter(&mut self, n: u32, f: crate::model::layout::Filter, geo: &Geo) -> Effect {
        self.edit(geo, |l, layout, _| l.set_filter(n, f, layout))
    }

    pub fn layer_action(&mut self, a: crate::layer::Action, geo: &Geo) -> Effect {
        self.edit(geo, |l, layout, g| l.act(a, layout, g))
    }

    pub fn save(&mut self, i: usize) {
        // Remember the layout in use with this set of displays.
        if !self.monitors.is_empty() {
            let set = crate::model::display::set_key(&self.monitors);
            let p = &mut self.profiles[i].profile;
            let id = p.default_layout.clone();
            p.layout_for_displays.insert(set, id);
        }
        let sp = &self.profiles[i];
        match self.store.save_profile(&sp.stem, &sp.profile) {
            Ok(true) => {}
            Ok(false) => log!("not saving {}: folder is read-only", sp.stem),
            Err(e) => {
                log!("could not save {}: {e}", sp.stem);
                self.notices.push(("Kiritori could not save".into(), format!("{}: {e}", sp.profile.name)));
            }
        }
    }

    /// "Ask first": the user said yes (Apply on the toast, or Ctrl+Alt+K).
    pub fn accept_ask(&mut self) {
        let Some((i, hwnd, _)) = self.asking.take() else { return };
        self.approved.push(hwnd);
        self.toasted = Some(hwnd);
        self.active = Some(i);
        self.suspended = false;
        self.bypass = false;
        log!("applied \"{}\" after asking", self.profiles[i].profile.name);
        self.apply_frame_removal();
    }

    /// The Ask toast's drawing, while it shows, at chrome scale `k` in `w`×`h` pixels.
    pub fn ask_draws(&mut self, k: f32, w: f32, h: f32) -> Option<Vec<Draw>> {
        use crate::engine::draw::Align;
        use crate::layer::style::*;
        if self.asking.is_none() && self.elevation_card.is_some() {
            // "This app runs as administrator": offer the restart.
            let s = |v: f32| v * k;
            let mut solid = glass();
            solid[3] = 1.0;
            let close = (w - s(44.0), (h - s(32.0)) / 2.0, s(32.0), s(32.0));
            let restart = (close.0 - s(8.0) - s(220.0), (h - s(40.0)) / 2.0, s(220.0), s(40.0));
            let tw = restart.0 - s(20.0);
            let app_name = self.active.map(|i| self.profiles[i].profile.name.clone()).unwrap_or_else(|| "This app".into());
            let d = vec![
                Draw::Fill { r: (0.0, 0.0, w, h), color: solid, radius: 0.0 },
                Draw::Stroke { r: (0.5, 0.5, w - 1.0, h - 1.0), color: danger(), width: s(1.5), radius: 0.0, dashed: false },
                Draw::Text { r: (s(18.0), s(10.0), tw, s(24.0)), text: format!("{app_name} runs as administrator"), size: s(14.0), color: fg(), bold: true, align: Align::Left },
                Draw::Text { r: (s(18.0), s(32.0), tw, s(22.0)), text: "Restart Kiritori as administrator to control it.".into(), size: s(13.0), color: fg_muted(), bold: false, align: Align::Left },
                Draw::Fill { r: restart, color: accent(), radius: s(8.0) },
                Draw::Text { r: restart, text: "Restart as administrator".into(), size: s(13.0), color: ON_COLOR, bold: true, align: Align::Center },
                Draw::Icon { name: "x", r: (close.0 + s(7.0), close.1 + s(7.0), s(18.0), s(18.0)), color: fg_muted(), stroke: s(2.0) },
            ];
            self.ask_hits = vec![(restart, true), (close, false)];
            return Some(d);
        }
        let (i, _, since) = self.asking?;
        if since.elapsed() >= ASK_SHOW {
            return None;
        }
        let s = |v: f32| v * k;
        let p = &self.profiles[i].profile;
        let layout = p.active().map(|l| l.name.clone()).unwrap_or_default();
        let mut solid = glass();
        solid[3] = 1.0;
        let mut d = vec![
            Draw::Fill { r: (0.0, 0.0, w, h), color: solid, radius: 0.0 },
            Draw::Stroke { r: (0.5, 0.5, w - 1.0, h - 1.0), color: hairline(), width: 1.0, radius: 0.0, dashed: false },
        ];
        let close = (w - s(44.0), (h - s(32.0)) / 2.0, s(32.0), s(32.0));
        let apply = (close.0 - s(8.0) - s(196.0), (h - s(40.0)) / 2.0, s(196.0), s(40.0));
        let text_w = apply.0 - s(20.0);
        d.push(Draw::Text { r: (s(18.0), s(10.0), text_w, s(24.0)), text: format!("Kiritori · {}", p.name), size: s(14.0), color: fg(), bold: true, align: Align::Left });
        d.push(Draw::Text { r: (s(18.0), s(32.0), text_w, s(22.0)), text: format!("Apply \"{layout}\"?"), size: s(13.0), color: fg_muted(), bold: false, align: Align::Left });
        d.push(Draw::Fill { r: apply, color: accent(), radius: s(8.0) });
        d.push(Draw::Text { r: (apply.0 + s(14.0), apply.1, s(60.0), apply.3), text: "Apply".into(), size: s(14.0), color: ON_COLOR, bold: true, align: Align::Left });
        d.push(Draw::Text { r: (apply.0, apply.1, apply.2 - s(12.0), apply.3), text: "Ctrl+Alt+K".into(), size: s(12.0), color: ON_COLOR, bold: false, align: Align::Right });
        d.push(Draw::Icon { name: "x", r: (close.0 + s(7.0), close.1 + s(7.0), s(18.0), s(18.0)), color: fg_muted(), stroke: s(2.0) });
        self.ask_hits = vec![(apply, true), (close, false)];
        Some(d)
    }

    /// A click on the Ask toast: Apply, or close (the layout stays off for now).
    pub fn ask_input(&mut self, input: Input) {
        if let Input::Down(_, x, y, _) = input
            && let Some((_, apply)) = self.ask_hits.iter().find(|(r, _)| x >= r.0 && x < r.0 + r.2 && y >= r.1 && y < r.1 + r.3).copied()
        {
            if self.asking.is_none() && self.elevation_card.is_some() {
                self.elevation_card = None;
                if apply {
                    log!("restart as administrator: chosen");
                    self.wants_restart_admin = true;
                }
                return;
            }
            if apply {
                self.accept_ask();
            } else if let Some(a) = self.asking.as_mut() {
                a.2 = Instant::now() - ASK_SHOW; // hide the toast; Ctrl+Alt+K still applies
                log!("ask toast dismissed");
            }
        }
    }

    /// Close the layer if it is open (edits are already in the layout). `why` goes in the log.
    fn close_layer(&mut self, why: &str) -> bool {
        if self.layer.take().is_none() {
            return false;
        }
        log!("layer closed{why}");
        if let Some(t) = self.tutorial.as_mut() {
            t.observe(&crate::tutorial::Seen::LayerClosed);
            if t.finished() {
                self.end_tutorial("You're set up. Replay the tutorial any time from the tray menu.");
            }
        }
        true
    }

    /// Ctrl+Alt+K: open the layer over the most recent app, creating its profile if needed.
    pub fn toggle_layer(&mut self) {
        // "Ask first": Ctrl+Alt+K is the yes.
        if self.layer.is_none() && self.asking.is_some_and(|(_, h, _)| self.last_app.as_ref().is_some_and(|w| w.hwnd == h)) {
            self.accept_ask();
            return;
        }
        if self.close_layer("") {
            return;
        }
        let Some(w) = self.last_app.clone() else {
            // No app yet: let the user point at one.
            log!("layer: no app yet, starting Pick a window");
            self.wants_pick = true;
            return;
        };
        let i = match self.find_profile(&w.facts) {
            Some(i) => i,
            None => {
                let taken: Vec<String> = self.profiles.iter().map(|p| p.profile.name.clone()).collect();
                let profile = Profile::new_for(&w.facts, w.display.clone(), &taken);
                let stem = self.store.new_stem(&profile.name);
                log!("new profile \"{}\" for {} ({stem}.json)", profile.name, w.facts.exe_name());
                self.profiles.push(StoredProfile { stem, profile });
                let i = self.profiles.len() - 1;
                self.save(i);
                i
            }
        };
        self.active = Some(i);
        self.bypass = false;
        self.suspended = false;
        self.freeze += 1;
        self.layer = Some(Layer::new(self.freeze));
        if !self.settings.layer_hints_done && self.tutorial.is_none() {
            self.tutorial = Some(crate::tutorial::Tutorial::default());
            log!("tutorial started (first time the layer opens)");
        }
        self.apply_frame_removal();
        log!("layer open over {} (profile \"{}\")", w.facts.exe_name(), self.profiles[i].profile.name);
    }

    /// A window was picked: open the layer on it, creating its profile if needed.
    pub fn open_on(&mut self, w: AppWindow) {
        log!("picked {} ({})", w.facts.exe_name(), w.facts.title.trim());
        self.layer = None;
        self.active = self.profiles.iter().position(|p| p.profile.match_rule.matches(&w.facts));
        self.last_app = Some(w);
        self.toggle_layer();
    }

    /// Ctrl+Alt+B.
    pub fn toggle_bypass(&mut self) {
        self.bypass = !self.bypass;
        if self.suspended {
            self.suspended = false;
            self.bypass = false;
            self.apply_frame_removal();
            log!("resumed after emergency off");
            return;
        }
        log!("show original {}", if self.bypass { "on" } else { "off" });
    }

    /// Ctrl+Alt+L: the active profile's next layout becomes its default.
    pub fn next_layout(&mut self) {
        let Some(i) = self.active else { return };
        if let Some(id) = self.profiles[i].profile.next_layout_id() {
            self.set_layout(i, &id);
        }
    }

    pub fn set_layout(&mut self, i: usize, id: &str) {
        let p = &mut self.profiles[i].profile;
        if p.default_layout != id && p.layout(id).is_some() {
            p.default_layout = id.to_string();
            log!("layout: {}", p.active().map(|l| l.name.as_str()).unwrap_or("?"));
            self.save(i);
        }
    }

    /// Ctrl+Alt+1…9: hide or show a cutout without opening the layer.
    pub fn toggle_cutout(&mut self, n: u32) {
        let Some(i) = self.active else { return };
        let p = &mut self.profiles[i].profile;
        let id = p.active_id().unwrap_or_default();
        if let Some(l) = p.layout_mut(&id) && l.cutout(n).is_some() {
            l.toggle_visible(n);
            log!("cutout {n} {}", if l.cutout(n).unwrap().placement.visible { "shown" } else { "hidden" });
            self.save(i);
        }
    }

    /// Use profile `i` (and a layout by name) because the command line asked; false if the
    /// layout was not found (already reported).
    fn choose_from_command(&mut self, i: usize, layout: Option<String>) -> bool {
        if let Some(name) = layout {
            match crate::cli::find_layout(&self.profiles[i].profile, &name) {
                Some(id) => self.set_layout(i, &id),
                None => {
                    let p = &self.profiles[i].profile;
                    let have: Vec<String> = p.layouts.iter().map(|l| l.name.clone()).collect();
                    let text = format!("\"{}\" has no layout called \"{name}\". It has: {}.", p.name, have.join(", "));
                    self.command_failed("No such layout", text);
                    return false;
                }
            }
        }
        self.forced = Some(i);
        self.suspended = false;
        self.bypass = false;
        // The app may already be in front; if not, an open window of it is used, so a frontend
        // can apply a layout right after starting the game.
        let w = self.last_app.clone().filter(|w| self.profiles[i].profile.match_rule.matches(&w.facts)).or_else(|| {
            let h = system::matching_windows(&self.profiles[i].profile.match_rule).into_iter().next()?;
            let hwnd = windows::Win32::Foundation::HWND(h as *mut _);
            Some(AppWindow { hwnd: h, facts: system::window_facts(hwnd)?, display: system::display_for(hwnd) })
        });
        if let Some(w) = w {
            if self.settings.show_apply_toast && let Some(l) = self.profiles[i].profile.active() {
                self.toast = Some((format!("Kiritori · {} · Ctrl+Alt+K to edit", l.name), Instant::now()));
                self.toasted = Some(w.hwnd);
            }
            self.last_app = Some(w);
            self.active = Some(i);
            self.apply_frame_removal();
        }
        true
    }

    fn command_failed(&mut self, title: &str, text: String) {
        log!("command line: {title}: {text}");
        self.notices.push((title.to_string(), text));
    }

    /// `--apply`, `--launch`, `--bypass` from the command line.
    pub fn command(&mut self, c: crate::cli::Command) {
        use crate::cli::{self, Command};
        match c {
            Command::Apply { profile, layout } => match cli::find_profile(&self.profiles, &profile) {
                Some(i) => {
                    if self.choose_from_command(i, layout) {
                        let p = &self.profiles[i].profile;
                        log!("command line: applying \"{}\" · {}", p.name, p.active().map(|l| l.name.as_str()).unwrap_or("?"));
                    }
                }
                None => {
                    let have: Vec<String> = self.profiles.iter().map(|p| p.profile.name.clone()).collect();
                    let list = if have.is_empty() { "none yet".to_string() } else { have.join(", ") };
                    self.command_failed("No such profile", format!("There is no profile called \"{profile}\". Profiles: {list}."));
                }
            },
            Command::Launch { exe, layout, args } => {
                match cli::profile_for_exe(&self.profiles, &exe) {
                    Some(i) => {
                        if !self.choose_from_command(i, layout) {
                            return;
                        }
                        log!("command line: launching {exe} with \"{}\"", self.profiles[i].profile.name);
                    }
                    None if layout.is_some() => {
                        self.command_failed("No profile for this app", format!("No profile matches {exe}, so --layout cannot be used. Start it once and press Ctrl+Alt+K to make one."));
                        return;
                    }
                    None => log!("command line: launching {exe} (no profile matches it yet)"),
                }
                let mut cmd = std::process::Command::new(&exe);
                cmd.args(&args);
                if let Some(d) = std::path::Path::new(&exe).parent().filter(|d| !d.as_os_str().is_empty()) {
                    cmd.current_dir(d);
                }
                if let Err(e) = cmd.spawn() {
                    self.command_failed("Could not start the app", format!("{exe}: {e}"));
                }
            }
            Command::Bypass(None) => self.toggle_bypass(),
            Command::Bypass(Some(on)) => {
                if self.suspended {
                    self.toggle_bypass(); // resumes
                }
                self.bypass = on;
                log!("show original {} (command line)", if on { "on" } else { "off" });
            }
            Command::Profiles => self.open_profiles(),
            Command::Settings => self.open_settings(),
            Command::Edit => {
                if self.layer.is_none() {
                    self.toggle_layer();
                }
            }
            Command::Import(path) => {
                if let Err(e) = self.import_from(&path) {
                    self.command_failed("Could not import the profile", e);
                }
            }
            Command::Export { profile, file } => match cli::find_profile(&self.profiles, &profile) {
                Some(i) => {
                    if let Err(e) = self.export_to(i, &file) {
                        self.command_failed("Could not export the profile", e);
                    } else {
                        log!("command line: exported \"{}\" to {file}", self.profiles[i].profile.name);
                    }
                }
                None => self.command_failed("No such profile", format!("There is no profile called \"{profile}\".")),
            },
            Command::Menu | Command::Pick | Command::Quit | Command::Replace => {} // the shell handles these
        }
    }

    /// Ctrl+Alt+Shift+K: close the layer, remove the canvas, stop capturing.
    pub fn emergency_off(&mut self) {
        self.layer = None;
        self.suspended = true;
        self.restore_frames();
        log!("emergency off: canvas removed, capture stopped; Ctrl+Alt+K or Ctrl+Alt+B resumes");
    }

    pub fn set_start_with_windows(&mut self, on: bool) {
        match crate::shell::system::set_start_with_windows(on) {
            Ok(()) => {
                self.settings.start_with_windows = on;
                let _ = self.store.save_settings(&self.settings);
                log!("start with Windows {}", if on { "on" } else { "off" });
            }
            Err(e) => {
                log!("start with Windows failed: {e}");
                self.notices.push(("Kiritori".into(), format!("Could not change Start with Windows: {e}")));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::geometry::{Rect, Size};
    use crate::model::layout::{DisplayMode, DisplayRef, Frame};
    use crate::shell::canvas::{Button, Mods};

    fn app_with_cutout(tag: &str) -> (App, Geo) {
        let root = std::env::temp_dir().join(format!("kiritori-app-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut app = App::new(Store::open(&root));
        let w = WindowFacts { exe_path: r"C:\g\game.exe".into(), window_class: "G".into(), title: "Game".into() };
        let d = DisplayRef { hardware_id: "x".into(), label: "Display 1".into(), mode: DisplayMode::Takeover };
        let mut p = Profile::new_for(&w, d, &[]);
        let geo = Geo { display: "main".into(), here: true, number: 0, canvas: Size::new(3840.0, 2160.0), source: Rect::new(0.0, 0.0, 3840.0, 2160.0), source_px: Size::new(1920.0, 1080.0), scale: 2.0 };
        let f: Frame = geo.frame();
        p.layouts[0].add_cutout(Rect::new(0.5, 0.0, 0.5, 0.5), &f);
        app.profiles.push(StoredProfile { stem: "game".into(), profile: p });
        app.active = Some(0);
        let mut layer = Layer::new(1);
        layer.view = crate::layer::View::Arrange;
        app.layer = Some(layer);
        (app, geo)
    }

    fn placement(app: &App) -> Rect {
        app.active_layout().unwrap().cutout(1).unwrap().placement.rect.scale(Size::new(3840.0, 2160.0))
    }

    #[test]
    fn another_app_in_front_closes_the_layer() {
        let (mut app, _) = app_with_cutout("front");
        let facts = WindowFacts { exe_path: r"C:\g\game.exe".into(), window_class: "G".into(), title: "Game".into() };
        let d = DisplayRef { hardware_id: "x".into(), label: "Display 1".into(), mode: DisplayMode::Takeover };
        app.last_app = Some(AppWindow { hwnd: 77, facts: facts.clone(), display: d.clone() });
        // Kiritori's own windows (None) and the same app leave the layer open.
        app.on_foreground(None);
        app.on_foreground(Some(AppWindow { hwnd: 77, facts: facts.clone(), display: d.clone() }));
        assert!(app.layer_open());
        // A screenshot tool in front closes it; the cutout is kept.
        let snip = WindowFacts { exe_path: r"C:\Windows\SnippingTool.exe".into(), window_class: "S".into(), title: "Snipping Tool".into() };
        app.on_foreground(Some(AppWindow { hwnd: 90, facts: snip, display: d.clone() }));
        assert!(!app.layer_open());
        app.on_foreground(Some(AppWindow { hwnd: 77, facts, display: d }));
        assert_eq!(app.active, Some(0), "back in the app, the layout applies");
        assert!(!app.layer_open());
        assert_eq!(app.active_layout().unwrap().cutouts.len(), 1);
    }

    #[test]
    fn imports_never_overwrite_and_ask_waits_for_a_yes() {
        let (mut app, _) = app_with_cutout("import");
        app.layer = None;
        app.active = None;
        let file = std::env::temp_dir().join(format!("kiritori-import-{}.json", std::process::id()));
        Store::export_profile(&app.profiles[0].profile, &file).unwrap();
        let i = app.import_from(file.to_str().unwrap()).unwrap();
        assert_eq!(app.profiles[i].profile.name, "Game (2)");
        assert_ne!(app.profiles[i].stem, app.profiles[0].stem);
        let j = app.import_from(file.to_str().unwrap()).unwrap();
        assert_eq!(app.profiles[j].profile.name, "Game (3)");
        assert!(app.import_from("C:\\nowhere\\nothing.json").is_err());
        let _ = std::fs::remove_file(&file);

        // Ask first: nothing applies until Ctrl+Alt+K.
        app.profiles.truncate(1);
        app.profiles[0].profile.auto_apply = AutoApply::Ask;
        let facts = WindowFacts { exe_path: r"C:\g\game.exe".into(), window_class: "G".into(), title: "Game".into() };
        let d = DisplayRef { hardware_id: "x".into(), label: "Display 1".into(), mode: DisplayMode::Takeover };
        app.on_foreground(Some(AppWindow { hwnd: 77, facts: facts.clone(), display: d.clone() }));
        assert_eq!(app.active, None);
        assert!(app.asking.is_some());
        app.toggle_layer();
        assert_eq!(app.active, Some(0), "Ctrl+Alt+K applies");
        assert!(app.layer.is_none(), "and does not open the layer");
        // Coming back to the same window applies without asking again; another window asks.
        app.on_foreground(Some(AppWindow { hwnd: 78, facts: WindowFacts { exe_path: r"C:\x\other.exe".into(), ..facts.clone() }, display: d.clone() }));
        app.on_foreground(Some(AppWindow { hwnd: 77, facts: facts.clone(), display: d.clone() }));
        assert_eq!(app.active, Some(0));
        app.on_foreground(Some(AppWindow { hwnd: 79, facts: facts.clone(), display: d.clone() }));
        assert_eq!(app.active, None);
        assert!(app.asking.is_some_and(|(_, h, _)| h == 79));
        // Off never applies by itself.
        app.asking = None;
        app.profiles[0].profile.auto_apply = AutoApply::Off;
        app.on_foreground(Some(AppWindow { hwnd: 80, facts, display: d }));
        assert_eq!(app.active, None);
        assert!(app.asking.is_none());
    }

    #[test]
    fn each_set_of_displays_remembers_its_layout() {
        use crate::model::display::Monitor;
        let (mut app, _) = app_with_cutout("sets");
        app.layer = None;
        let mon = |id: &str, n: u32, x: i32| Monitor { id: id.into(), number: n, name: format!("Display {n}"), rect: (x, 0, 1920, 1080), scale: 1.0, hz: 60 };
        let one = vec![mon("A", 1, 0)];
        let two = vec![mon("A", 1, 0), mon("B", 2, 1920)];
        app.displays_changed(one.clone());
        let cab = app.profiles[0].profile.default_layout.clone();
        app.save(0);
        // With the touchscreen plugged in, the user makes and uses "Two screens".
        app.displays_changed(two.clone());
        app.new_layout();
        let two_screens = app.profiles[0].profile.default_layout.clone();
        assert_ne!(cab, two_screens);
        // Unplugged: back to the first layout. Plugged in again: Two screens.
        app.displays_changed(one);
        assert_eq!(app.profiles[0].profile.default_layout, cab);
        app.displays_changed(two);
        assert_eq!(app.profiles[0].profile.default_layout, two_screens);
    }

    #[test]
    fn a_new_source_shape_is_reported_once() {
        let (mut app, _) = app_with_cutout("shape");
        assert_eq!(App::aspect_of(1920, 1080), [16, 9]);
        assert_eq!(App::aspect_of(2560, 1080), [64, 27]);
        app.check_source_shape(3840, 2160);
        assert!(app.toast.is_none(), "same shape, bigger: nothing to say");
        app.check_source_shape(1080, 1920);
        assert!(app.toast.as_ref().is_some_and(|(t, _)| t.starts_with("Source shape changed")));
        app.toast = None;
        app.check_source_shape(1080, 1920);
        assert!(app.toast.is_none(), "only once for the same size");
    }

    #[test]
    fn a_drag_is_one_undoable_step() {
        let (mut app, geo) = app_with_cutout("drag");
        let start = placement(&app);
        let off = Mods { ctrl: true, ..Default::default() };
        app.layer_input(Input::Down(Button::Left, 2880.0, 540.0, off), &geo);
        app.layer_input(Input::Move(2380.0, 840.0), &geo);
        app.layer_input(Input::Move(1880.0, 1040.0), &geo);
        let fx = app.layer_input(Input::Up(Button::Left, 1880.0, 1040.0), &geo);
        assert!(matches!(fx, Effect::Edit(..)), "{fx:?}");
        assert_ne!(placement(&app), start);
        app.layer_input(Input::Key(0x5A, Mods { ctrl: true, ..Default::default() }), &geo);
        assert_eq!(placement(&app), start, "Ctrl+Z puts it back where the drag began");
        app.layer_input(Input::Key(0x59, Mods { ctrl: true, ..Default::default() }), &geo);
        assert_ne!(placement(&app), start, "Ctrl+Y moves it again");
    }

    #[test]
    fn undo_many_goes_back_several_steps_and_start_over_is_undoable() {
        let (mut app, geo) = app_with_cutout("many");
        let start = app.active_layout().unwrap().clone();
        app.layer_input(Input::Key(0x41, Mods::default()), &geo); // auto-arrange
        app.layer_action(crate::layer::Action::Reset, &geo);
        app.layer_input(Input::Key(0x32, Mods::default()), &geo); // start over
        assert!(app.active_layout().unwrap().cutouts.is_empty());
        app.edit(&geo, |_, _, _| Effect::UndoMany(2));
        assert_eq!(app.active_layout().unwrap().cutouts, start.cutouts, "both steps undone");
    }

    #[test]
    fn tutorial_skip_and_end_from_the_card() {
        use crate::layer::guide::GuideClick;
        let (mut app, geo) = app_with_cutout("guide");
        app.tutorial = Some(crate::tutorial::Tutorial::default());
        app.edit(&geo, |_, _, _| Effect::Guide(GuideClick::SkipStep));
        assert_eq!(app.tutorial.as_ref().unwrap().step(), Some(crate::tutorial::Step::Name));
        app.layer_draws(&geo);
        assert_eq!(app.layer.as_ref().unwrap().guide.as_ref().unwrap().title, "Give it a name");
        app.edit(&geo, |_, _, _| Effect::Guide(GuideClick::End));
        assert!(app.tutorial.is_none());
        assert!(app.settings.layer_hints_done, "remembered, so it does not start again");
    }

    #[test]
    fn the_toast_shows_then_fades_then_goes() {
        let geo = Geo { display: "main".into(), here: true, number: 0, canvas: Size::new(3840.0, 2160.0), source: Rect::new(0.0, 0.0, 3840.0, 2160.0), source_px: Size::new(1920.0, 1080.0), scale: 2.0 };
        let shown = toast_draws("Kiritori · Single screen · Ctrl+Alt+K to edit", std::time::Duration::from_millis(100), &geo);
        assert_eq!(shown.len(), 3);
        let alpha = |d: &[Draw]| match &d[0] {
            Draw::Fill { color, .. } => color[3],
            _ => 0.0,
        };
        let fading = toast_draws("x", std::time::Duration::from_millis(2750), &geo);
        assert!(alpha(&fading) < alpha(&shown) && alpha(&fading) > 0.0);
        assert!(toast_draws("x", std::time::Duration::from_millis(3100), &geo).is_empty());
        // It sits at the top right.
        if let Draw::Fill { r, .. } = &shown[0] {
            assert!(r.0 + r.2 <= 3840.0 - 40.0 && r.1 < 100.0);
        }
    }

    #[test]
    fn escape_during_a_drag_puts_it_back() {
        let (mut app, geo) = app_with_cutout("esc");
        let start = placement(&app);
        let off = Mods { ctrl: true, ..Default::default() };
        app.layer_input(Input::Down(Button::Left, 2880.0, 540.0, off), &geo);
        app.layer_input(Input::Move(1880.0, 1040.0), &geo);
        assert_ne!(placement(&app), start);
        app.layer_input(Input::Key(0x1B, Mods::default()), &geo);
        assert_eq!(placement(&app), start);
    }
}
