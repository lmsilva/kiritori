//! System presence: the hidden shell window, tray icon and menu, global shortcuts,
//! foreground-app detection and single-instance handling.
//!
//! The window procedure and the Windows callbacks only queue events; the main loop handles
//! them with the app borrowed, so nothing re-enters the app while a menu is open.

pub mod canvas;
pub mod icon;
pub mod pick;
pub mod system;

use std::cell::RefCell;

use windows::Win32::Foundation::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

use crate::app::{App, AppWindow};
use crate::strings::t;
use crate::log;

pub const CLASS: PCWSTR = w!("KiritoriShell");
const WM_TRAY: u32 = WM_APP + 1;
/// Sent by a second launch: open the tray menu.
pub const WM_SECOND_LAUNCH: u32 = WM_APP + 2;
/// Sent by `kiritori --quit`.
pub const WM_QUIT_REQUEST: u32 = WM_APP + 3;
/// Sent by `kiritori --pick`: start Pick a window.
pub const WM_PICK_REQUEST: u32 = WM_APP + 4;
const TRAY_ID: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hotkey {
    Layer = 1,
    Bypass = 2,
    NextLayout = 3,
    Emergency = 4,
    Quit = 5,
    Cutout1 = 11,
}

#[derive(Debug)]
enum Event {
    TrayClick,
    SecondLaunch,
    QuitRequest,
    PickRequest,
    /// Arguments from the command line: a second launch's, or this launch's own.
    Command(Vec<String>),
    Hotkey(i32),
    Foreground(isize),
    TaskbarCreated,
    SettingsChanged,
    DisplaysChanged,
}

thread_local! {
    static EVENTS: RefCell<Vec<Event>> = const { RefCell::new(Vec::new()) };
    static TASKBAR_CREATED: RefCell<u32> = const { RefCell::new(0) };
    static SHELL_HWND: RefCell<isize> = const { RefCell::new(0) };
}

/// Queue an event and wake the main loop. Tray clicks and foreground changes arrive as *sent*
/// messages and callbacks, which GetMessage handles internally without returning; without a
/// posted message the queue would sit until some unrelated message came along.
fn push(e: Event) {
    EVENTS.with(|q| q.borrow_mut().push(e));
    let h = SHELL_HWND.with(|h| *h.borrow());
    if h != 0 {
        unsafe {
            let _ = PostMessageW(Some(HWND(h as *mut _)), WM_NULL, WPARAM(0), LPARAM(0));
        }
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let taskbar_created = TASKBAR_CREATED.with(|t| *t.borrow());
    match msg {
        WM_TRAY => {
            // NOTIFYICON_VERSION_4: the event is in the low word of lParam.
            let ev = (lp.0 & 0xFFFF) as u32;
            if ev == WM_CONTEXTMENU || ev == NIN_SELECT || ev == NIN_SELECT + 1 /* NIN_KEYSELECT */ {
                push(Event::TrayClick);
            }
            LRESULT(0)
        }
        WM_SECOND_LAUNCH => {
            push(Event::SecondLaunch);
            LRESULT(0)
        }
        WM_QUIT_REQUEST => {
            push(Event::QuitRequest);
            LRESULT(0)
        }
        WM_PICK_REQUEST => {
            push(Event::PickRequest);
            LRESULT(0)
        }
        WM_COPYDATA => {
            let cds = unsafe { &*(lp.0 as *const COPYDATASTRUCT) };
            if cds.dwData == COPYDATA_ARGS && !cds.lpData.is_null() {
                let words = unsafe { std::slice::from_raw_parts(cds.lpData as *const u16, cds.cbData as usize / 2) };
                let text = String::from_utf16_lossy(words);
                let args: Vec<String> = if text.is_empty() { Vec::new() } else { text.split('\0').map(String::from).collect() };
                push(Event::Command(args));
                return LRESULT(1);
            }
            LRESULT(0)
        }
        WM_HOTKEY => {
            push(Event::Hotkey(wp.0 as i32));
            LRESULT(0)
        }
        WM_DISPLAYCHANGE => {
            push(Event::DisplaysChanged);
            LRESULT(0)
        }
        WM_SETTINGCHANGE => {
            push(Event::SettingsChanged);
            LRESULT(0)
        }
        m if m == taskbar_created && m != 0 => {
            push(Event::TaskbarCreated);
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

unsafe extern "system" fn on_win_event(_: HWINEVENTHOOK, event: u32, hwnd: HWND, id_object: i32, _: i32, _: u32, _: u32) {
    if event == EVENT_SYSTEM_FOREGROUND && id_object == OBJID_WINDOW.0 {
        push(Event::Foreground(hwnd.0 as isize));
    }
}

fn copy_wide(dst: &mut [u16], s: &str) {
    let w: Vec<u16> = s.encode_utf16().take(dst.len() - 1).collect();
    dst[..w.len()].copy_from_slice(&w);
    dst[w.len()] = 0;
}

struct Shell {
    hwnd: HWND,
    icon: HICON,
    shown_state: Option<(icon::TrayState, bool)>,
    failed_hotkeys: Vec<String>,
    /// Hotkey ids registered now.
    registered: Vec<i32>,
    /// Hotkeys are paused while a new shortcut is recorded in Settings.
    hotkeys_paused: bool,
}

impl Shell {
    fn tray_data(&self) -> NOTIFYICONDATAW {
        NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: TRAY_ID,
            ..Default::default()
        }
    }

    fn add_tray(&mut self, app: &App) {
        self.shown_state = None;
        self.refresh_icon(app);
        let mut d = self.tray_data();
        d.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP | NIF_SHOWTIP;
        d.uCallbackMessage = WM_TRAY;
        d.hIcon = self.icon;
        copy_wide(&mut d.szTip, &app.tooltip());
        unsafe {
            if !Shell_NotifyIconW(NIM_ADD, &d).as_bool() {
                log!("tray icon could not be added");
            }
            d.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            let _ = Shell_NotifyIconW(NIM_SETVERSION, &d);
        }
    }

    /// Redraw the icon only when the state or the taskbar theme changed; always update the tooltip.
    fn refresh_icon(&mut self, app: &App) {
        let key = (app.tray_state(), system::taskbar_is_light());
        if self.shown_state != Some(key) {
            if let Ok(i) = icon::make(key.0, key.1) {
                if !self.icon.is_invalid() {
                    unsafe {
                        let _ = DestroyIcon(self.icon);
                    }
                }
                self.icon = i;
            }
            self.shown_state = Some(key);
        }
        let mut d = self.tray_data();
        d.uFlags = NIF_ICON | NIF_TIP | NIF_SHOWTIP;
        d.hIcon = self.icon;
        copy_wide(&mut d.szTip, &app.tooltip());
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &d);
        }
    }

    fn notify(&self, title: &str, text: &str) {
        let mut d = self.tray_data();
        d.uFlags = NIF_INFO;
        copy_wide(&mut d.szInfoTitle, title);
        copy_wide(&mut d.szInfo, text);
        d.dwInfoFlags = NIIF_NONE | NIIF_RESPECT_QUIET_TIME;
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &d);
        }
    }

    fn remove_tray(&self) {
        let d = self.tray_data();
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &d);
        }
    }

    /// Register the global shortcuts from the settings. Ids:
    /// 100 + action × 10 + slot; the cutout row 200 + slot × 10 + digit.
    fn register_hotkeys(&mut self, app: &mut App) {
        use crate::model::shortcut::Global;
        self.unregister_hotkeys();
        let b = app.bindings();
        app.taken.clear();
        for (gi, g) in Global::ALL.iter().enumerate() {
            for (slot, combo) in b.get(*g).iter().enumerate() {
                let Some(c) = combo else { continue };
                let mut mods = MOD_NOREPEAT;
                if c.ctrl {
                    mods |= MOD_CONTROL;
                }
                if c.alt {
                    mods |= MOD_ALT;
                }
                if c.shift {
                    mods |= MOD_SHIFT;
                }
                let keys: Vec<(i32, u32)> = if *g == Global::Cutouts {
                    (1..=9).map(|d| (200 + slot as i32 * 10 + d, '0' as u32 + d as u32)).collect()
                } else {
                    vec![(100 + gi as i32 * 10 + slot as i32, c.vk)]
                };
                let mut ok = true;
                for (id, vk) in keys {
                    if unsafe { RegisterHotKey(Some(self.hwnd), id, mods, vk) }.is_err() {
                        ok = false;
                    } else {
                        self.registered.push(id);
                    }
                }
                if !ok {
                    log!("shortcut {c} ({}) is taken by another program", g.label());
                    self.failed_hotkeys.push(c.to_string());
                    app.taken.push((*g, slot));
                }
            }
        }
    }

    fn unregister_hotkeys(&mut self) {
        for id in self.registered.drain(..) {
            unsafe {
                let _ = UnregisterHotKey(Some(self.hwnd), id);
            }
        }
        self.failed_hotkeys.clear();
    }

    /// About Kiritori, with the self-test.
    fn about(&self, app: &App) {
        let text = format!(
            "Kiritori {}: cut, move and reshape regions of any screen.\nMIT licence. Icons: Lucide (ISC). Enhanced scaling: AMD FidelityFX FSR 1 (MIT).\n\nSelf-test on this PC:\n{}",
            env!("CARGO_PKG_VERSION"),
            app.self_test.clone().unwrap_or_else(|| "still running".into())
        );
        let w: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
        unsafe {
            MessageBoxW(Some(self.hwnd), PCWSTR(w.as_ptr()), w!("About Kiritori"), MB_OK | MB_ICONINFORMATION);
        }
    }

    /// Left- and right-click open the same menu.
    fn show_menu(&mut self, app: &mut App) {
        const LAYER: usize = 1;
        const BYPASS: usize = 2;
        const PROFILES: usize = 3;
        const SHORTCUTS: usize = 4;
        const SETTINGS: usize = 5;
        const STARTUP: usize = 6;
        const QUIT: usize = 7;
        const TUTORIAL: usize = 8;
        const PICK: usize = 9;
        const ABOUT: usize = 10;
        const LAYOUT0: usize = 100;
        unsafe {
            let menu = CreatePopupMenu().unwrap();
            let add = |m: HMENU, flags: MENU_ITEM_FLAGS, id: usize, text: &str| {
                let w: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
                let _ = AppendMenuW(m, flags, id, PCWSTR(w.as_ptr()));
            };
            if let Some(p) = &app.problem {
                add(menu, MF_GRAYED, 0, &format!("⚠ {p}"));
                add(menu, MF_SEPARATOR, 0, "");
            }
            if !app.store.writable() {
                add(menu, MF_GRAYED, 0, t("tray.not_saving"));
                add(menu, MF_SEPARATOR, 0, "");
            }
            let b = app.bindings();
            let keys = |g: crate::model::shortcut::Global| b.get(g)[0].map(|c| format!("\t{c}")).unwrap_or_default();
            use crate::model::shortcut::Global;
            let layer_label = format!("{}{}", if app.layer_open() { t("tray.close_layer") } else { t("tray.open_layer") }, keys(Global::Layer));
            add(menu, MF_STRING, LAYER, &layer_label);
            add(menu, MF_STRING, PICK, t("tray.pick"));
            add(menu, MF_SEPARATOR, 0, "");
            add(menu, MF_GRAYED, 0, &app.current_label().unwrap_or_else(|| t("tray.no_app").into()));
            let sub = CreatePopupMenu().unwrap();
            if let Some(i) = app.active {
                let p = &app.profiles[i].profile;
                for (k, l) in p.layouts.iter().enumerate() {
                    let flags = if p.default_layout == l.id { MF_STRING | MF_CHECKED } else { MF_STRING };
                    add(sub, flags, LAYOUT0 + k, &l.name);
                }
            }
            let layouts_flags = if app.active.is_some() { MF_POPUP } else { MF_POPUP | MF_GRAYED };
            add(menu, layouts_flags, sub.0 as usize, &format!("{}{}", t("tray.layouts"), keys(Global::NextLayout)));
            add(menu, if app.bypass { MF_STRING | MF_CHECKED } else { MF_STRING }, BYPASS, &format!("{}{}", t("tray.show_original"), keys(Global::Bypass)));
            add(menu, MF_SEPARATOR, 0, "");
            // Panels that need the layer are dimmed when it cannot open, never removed.
            add(menu, MF_STRING, PROFILES, t("tray.profiles"));
            add(menu, MF_STRING, SHORTCUTS, &format!("{}\tF1", t("tray.shortcuts")));
            add(menu, MF_STRING, SETTINGS, t("tray.settings"));
            add(menu, MF_STRING, TUTORIAL, t("tray.tutorial"));
            add(menu, MF_STRING, ABOUT, t("tray.about"));
            let startup = system::starts_with_windows();
            add(menu, if startup { MF_STRING | MF_CHECKED } else { MF_STRING }, STARTUP, t("tray.start_with_windows"));
            add(menu, MF_SEPARATOR, 0, "");
            add(menu, MF_STRING, QUIT, &format!("{}{}", t("tray.quit"), keys(Global::Quit)));

            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            let _ = SetForegroundWindow(self.hwnd);
            let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN, pt.x, pt.y, Some(0), self.hwnd, None).0 as usize;
            let _ = PostMessageW(Some(self.hwnd), WM_NULL, WPARAM(0), LPARAM(0));
            let _ = DestroyMenu(menu);
            match cmd {
                LAYER => app.toggle_layer(),
                BYPASS => app.toggle_bypass(),
                STARTUP => app.set_start_with_windows(!startup),
                TUTORIAL => app.start_tutorial(),
                PICK => app.wants_pick = true,
                PROFILES => app.open_profiles(),
                SETTINGS => app.open_settings(),
                SHORTCUTS => app.open_shortcuts(),
                ABOUT => self.about(app),
                QUIT => {
                    log!("quit from tray menu");
                    PostQuitMessage(0);
                }
                c if c >= LAYOUT0 => {
                    if let Some(i) = app.active
                        && let Some(id) = app.profiles[i].profile.layouts.get(c - LAYOUT0).map(|l| l.id.clone()) {
                            app.set_layout(i, &id);
                        }
                }
                _ => {}
            }
        }
    }

    fn handle(&mut self, app: &mut App, e: Event) {
        match e {
            Event::TrayClick => self.show_menu(app),
            Event::SecondLaunch => {
                log!("second launch: opening the tray menu");
                self.show_menu(app);
            }
            Event::QuitRequest => {
                log!("quit requested from the command line");
                unsafe { PostQuitMessage(0) };
            }
            Event::PickRequest => app.wants_pick = true,
            Event::Command(args) => {
                log!("command line: {}", if args.is_empty() { "(none)".to_string() } else { args.join(" ") });
                match crate::cli::parse(&args) {
                    Ok(crate::cli::Command::Menu) => self.show_menu(app),
                    Ok(crate::cli::Command::Quit) => unsafe { PostQuitMessage(0) },
                    Ok(crate::cli::Command::Pick) => app.wants_pick = true,
                    Ok(crate::cli::Command::Profiles) => app.open_profiles(),
                    Ok(c) => app.command(c),
                    Err(e) => app.notices.push(("Kiritori did not understand the command line".into(), e)),
                }
            }
            Event::Hotkey(id) => {
                use crate::model::shortcut::Global;
                match id {
                    200.. => app.toggle_cutout((id % 10) as u32),
                    100.. => match Global::ALL.get(((id - 100) / 10) as usize) {
                        Some(Global::Layer) => app.toggle_layer(),
                        Some(Global::Bypass) => app.toggle_bypass(),
                        Some(Global::NextLayout) => app.next_layout(),
                        Some(Global::Emergency) => app.emergency_off(),
                        Some(Global::Quit) => {
                            log!("quit from its shortcut");
                            unsafe { PostQuitMessage(0) };
                        }
                        _ => {}
                    },
                    _ => {}
                }
            }
            Event::Foreground(h) => {
                let hwnd = HWND(h as *mut _);
                let w = system::window_facts(hwnd)
                    .filter(|f| system::is_candidate(hwnd, f))
                    .map(|facts| AppWindow { hwnd: h, facts, display: system::display_for(hwnd) });
                app.on_foreground(w);
            }
            Event::TaskbarCreated => {
                log!("taskbar restarted: adding the tray icon again");
                self.add_tray(app);
            }
            Event::SettingsChanged => app.refresh_theme(), // the tray icon is re-checked below
            Event::DisplaysChanged => app.displays_changed(system::monitors()),
        }
    }
}

pub fn run(mut app: App, args: Vec<String>) -> Result<()> {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let instance = GetModuleHandleW(None)?;
        let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: instance.into(), lpszClassName: CLASS, ..Default::default() };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(WS_EX_TOOLWINDOW, CLASS, w!("Kiritori"), WS_POPUP, 0, 0, 0, 0, None, None, Some(instance.into()), None)?;
        SHELL_HWND.with(|h| *h.borrow_mut() = hwnd.0 as isize);
        let tc = RegisterWindowMessageW(w!("TaskbarCreated"));
        TASKBAR_CREATED.with(|t| *t.borrow_mut() = tc);
        // Let a second launch and the taskbar reach us even when we run as administrator.
        for m in [WM_SECOND_LAUNCH, WM_QUIT_REQUEST, WM_PICK_REQUEST, WM_COPYDATA, tc] {
            let _ = ChangeWindowMessageFilterEx(hwnd, m, MSGFLT_ALLOW, None);
        }

        let mut shell = Shell { hwnd, icon: HICON::default(), shown_state: None, failed_hotkeys: Vec::new(), registered: Vec::new(), hotkeys_paused: false };
        shell.register_hotkeys(&mut app);
        if !shell.failed_hotkeys.is_empty() {
            app.problem = Some(format!("{} used by another program", shell.failed_hotkeys.join(", ")));
        }
        app.refresh_theme();
        shell.add_tray(&app);

        let hook = SetWinEventHook(EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND, None, Some(on_win_event), 0, 0, WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS);
        push(Event::Foreground(GetForegroundWindow().0 as isize));
        if !args.is_empty() {
            push(Event::Command(args));
        }

        let (tx, rx) = std::sync::mpsc::channel();
        let wake_shell = hwnd.0 as isize;
        let engine = crate::engine::Engine::start(tx, move || {
            let _ = PostMessageW(Some(HWND(wake_shell as *mut _)), WM_NULL, WPARAM(0), LPARAM(0));
        });

        if !app.settings.first_run_done {
            shell.notify("Kiritori is running", "Press Ctrl+Alt+K over any app to start cutting.");
            app.settings.first_run_done = true;
            let _ = app.store.save_settings(&app.settings);
        }
        if !app.store.writable() {
            shell.notify("Kiritori is not saving", "Its folder is read-only. Changes are kept until you quit. Move the Kiritori folder somewhere writable to keep them.");
        }
        log!("running; tray icon added");

        let mut canvas = Canvases::default();
        let mut panel: Option<canvas::Canvas> = None;
        let mut toast: Option<canvas::Canvas> = None;
        let mut timer_on = false;
        let mut picker: Option<pick::Picker> = None;
        let mut msg = MSG::default();
        loop {
            let r = GetMessageW(&mut msg, None, 0, 0);
            if r.0 <= 0 {
                break;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
            loop {
                let events: Vec<Event> = EVENTS.with(|q| std::mem::take(&mut *q.borrow_mut()));
                if events.is_empty() {
                    break;
                }
                for e in events {
                    shell.handle(&mut app, e);
                }
            }
            for r in rx.try_iter() {
                match r {
                    crate::engine::Report::Problem(p) => {
                        if let Some(text) = &p {
                            shell.notify("Kiritori", text);
                        }
                        app.elevation_card = (p.as_deref() == Some(crate::engine::ELEVATED)).then(std::time::Instant::now);
                        app.engine_problem = p;
                    }
                }
            }
            // Shortcuts: paused while Settings records a new one, registered again when changed.
            let pause = app.recording_shortcut();
            if pause != shell.hotkeys_paused {
                shell.hotkeys_paused = pause;
                if pause {
                    shell.unregister_hotkeys();
                } else {
                    app.hotkeys_dirty = true;
                }
            }
            if app.hotkeys_dirty && !shell.hotkeys_paused {
                app.hotkeys_dirty = false;
                shell.register_hotkeys(&mut app);
                app.problem = (!shell.failed_hotkeys.is_empty()).then(|| format!("{} used by another program", shell.failed_hotkeys.join(", ")));
            }
            if std::mem::take(&mut app.show_about) {
                shell.about(&app);
            }
            if app.wants_restart_admin {
                app.wants_restart_admin = false;
                if system::restart_as_admin() {
                    log!("restarting as administrator");
                } else {
                    shell.notify("Kiritori is still running normally", "Windows did not start the administrator copy (the prompt may have been declined).");
                }
            }
            // Pick a window: start it when asked; follow the pointer; a click opens the layer.
            if app.wants_pick {
                app.wants_pick = false;
                if app.layer_open() {
                    app.toggle_layer();
                }
                log!("pick a window: started");
                picker = pick::Picker::start();
            }
            if let Some(p) = picker.as_mut() {
                for input in pick::take_inputs() {
                    match input {
                        pick::PickInput::Move(x, y) => p.hover(x, y),
                        pick::PickInput::Cancel => {
                            log!("pick a window: cancelled");
                            picker = None;
                            break;
                        }
                        pick::PickInput::Click(x, y) => {
                            picker = None;
                            match pick::window_at(x, y) {
                                Some(h) => {
                                    let facts = system::window_facts(h);
                                    if let Some(facts) = facts {
                                        let w = AppWindow { hwnd: h.0 as isize, facts, display: system::display_for(h) };
                                        app.open_on(w);
                                    }
                                }
                                None => log!("pick a window: nothing there"),
                            }
                            break;
                        }
                    }
                }
            } else {
                pick::take_inputs();
            }
            app.picking = picker.is_some();
            // Self-test once the engine reports the graphics card.
            if app.self_test.is_none()
                && let Some(caps) = engine.caps()
            {
                let failed: Vec<String> = shell.failed_hotkeys.iter().map(|s| s.to_string()).collect();
                let lines = crate::selftest::run(&app.store.root().display().to_string(), app.store.writable(), Some(&caps), &failed, crate::shell::system::is_elevated());
                let text = crate::selftest::text(&lines);
                for l in text.lines() {
                    log!("self-test {l}");
                }
                let _ = std::fs::write(app.store.logs_dir().join("self-test.txt"), &text);
                app.self_test = Some(text);
            }
            compose(&mut app, &mut canvas, &mut panel, &mut toast, &engine, &shell);
            // While cutouts glide, wake about 60 times a second to redraw.
            let want_timer = app.animating();
            if want_timer != timer_on {
                if want_timer {
                    SetTimer(Some(shell.hwnd), 7, 15, None);
                } else {
                    let _ = KillTimer(Some(shell.hwnd), 7);
                }
                timer_on = want_timer;
            }
            for (title, text) in std::mem::take(&mut app.notices) {
                shell.notify(&title, &text);
            }
            shell.refresh_icon(&app);
        }

        // Quit. Window frames go back first: they are the one thing that must not be left
        // behind. If any later step stalls (Windows not answering, say), a watchdog ends the
        // process after a few seconds, so quitting always quits.
        let step = std::sync::Arc::new(std::sync::Mutex::new("restoring window frames"));
        {
            let step = step.clone();
            let marker = app.store.marker_path();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(5));
                let at = step.lock().map(|s| *s).unwrap_or("quitting");
                log!("exit forced: quitting stalled while {at}");
                // Frames still out: keep the marker, so the next start puts them back.
                if at != "restoring window frames" {
                    let _ = std::fs::remove_file(marker);
                }
                std::process::exit(0);
            });
        }
        let at = |s: &'static str| {
            if let Ok(mut g) = step.lock() {
                *g = s;
            }
        };
        app.restore_frames();
        at("stopping the engine");
        engine.stop();
        at("closing the windows");
        drop(panel);
        drop(toast);
        drop(canvas);
        at("removing the tray icon");
        let _ = UnhookWinEvent(hook);
        shell.remove_tray();
        app.store.remove_marker();
        log!("exit");
    }
    Ok(())
}

/// Keep the canvas, the layer's input and the engine's frame in step with the app.
/// Runs after every message, so the layer redraws as the pointer moves.
/// The Profiles panel's window: opened, fed its input and drawn while `app.panel` is set.
fn panel_surface(app: &mut App, panel: &mut Option<canvas::Canvas>, near: Option<HWND>) -> Option<crate::engine::draw::Surface> {
    if app.settings_panel.is_some() {
        // A Profiles window is the wrong size for Settings.
        if panel.as_ref().is_some_and(|p| (p.screen.2 as f32 / p.scale() - crate::settings::SIZE.0).abs() > 2.0) {
            *panel = None;
        }
        if panel.is_none() {
            let wa = canvas::work_area_of(near);
            let k = canvas::scale_at(wa.0 + wa.2 / 2, wa.1 + wa.3 / 2);
            let (w, h) = ((crate::settings::SIZE.0 * k) as i32, (crate::settings::SIZE.1 * k) as i32);
            let rect = (wa.0 + (wa.2 - w) / 2, wa.1 + (wa.3 - h) / 2, w, h);
            *panel = canvas::Canvas::new_panel(rect).map_err(|e| log!("panel window failed: {e}")).ok();
        }
        let p = panel.as_ref()?;
        for input in canvas::take_panel_inputs() {
            app.settings_input(input);
        }
        if app.settings_panel.is_none() && app.panel.is_none() {
            *panel = None;
            return None;
        }
        if app.settings_panel.is_some() {
            let draws = app.settings_draws(p.scale(), p.screen.2 as f32, p.screen.3 as f32);
            return Some(crate::engine::draw::Surface { hwnd: p.id(), screen: p.screen, draws, takeover: false, transparent: false });
        }
    }
    if app.panel.is_none() {
        if panel.take().is_some() {
            canvas::take_panel_inputs();
        }
        return None;
    }
    if panel.as_ref().is_some_and(|p| (p.screen.2 as f32 / p.scale() - crate::panel::SIZE.0).abs() > 2.0) {
        *panel = None;
    }
    if panel.is_none() {
        let wa = canvas::work_area_of(near);
        let k = canvas::scale_at(wa.0 + wa.2 / 2, wa.1 + wa.3 / 2);
        let (w, h) = ((crate::panel::SIZE.0 * k) as i32, (crate::panel::SIZE.1 * k) as i32);
        let rect = (wa.0 + (wa.2 - w) / 2, wa.1 + (wa.3 - h) / 2, w, h);
        *panel = canvas::Canvas::new_panel(rect).map_err(|e| log!("panel window failed: {e}")).ok();
    }
    let p = panel.as_ref()?;
    let k = p.scale();
    for input in canvas::take_panel_inputs() {
        app.panel_input(input, k);
    }
    // Export and Import: a file dialog over the panel.
    match app.dialog.take() {
        Some(crate::app::Dialog::Export(i)) => {
            if let Some(stem) = app.profiles.get(i).map(|p| p.stem.clone())
                && let Some(path) = system::save_file_dialog(p.hwnd, &format!("{stem}.json"))
            {
                let _ = app.export_to(i, &path);
            }
        }
        Some(crate::app::Dialog::Import) => {
            if let Some(path) = system::open_file_dialog(p.hwnd) {
                let _ = app.import_from(&path);
            }
        }
        Some(crate::app::Dialog::NewProfile) => {
            if let Some(h) = new_profile_menu(p.hwnd)
                && let Some(facts) = system::window_facts(h)
            {
                app.new_profile_for(&facts, system::display_for(h));
            }
        }
        None => {}
    }
    if app.panel.is_none() {
        *panel = None;
        return None;
    }
    let draws = app.panel_draws(k, p.screen.2 as f32, p.screen.3 as f32);
    Some(crate::engine::draw::Surface { hwnd: p.id(), screen: p.screen, draws, takeover: false, transparent: false })
}

/// The "Ask first" toast: top right of the app's display, until it times out or is answered.
fn ask_surface(app: &mut App, toast: &mut Option<canvas::Canvas>) -> Option<crate::engine::draw::Surface> {
    let hwnd = match (app.asking, app.elevation_card, app.target()) {
        (Some((_, h, _)), _, _) => h,
        (None, Some(_), Some(t)) => t,
        _ => {
            if toast.take().is_some() {
                canvas::take_toast_inputs();
            }
            return None;
        }
    };
    if toast.is_none() {
        let wa = canvas::work_area_of(Some(HWND(hwnd as *mut _)));
        let k = canvas::scale_at(wa.0 + wa.2 / 2, wa.1 + wa.3 / 2);
        let (w, h) = ((crate::app::ASK_SIZE.0 * k) as i32, (crate::app::ASK_SIZE.1 * k) as i32);
        let rect = (wa.0 + wa.2 - w - (24.0 * k) as i32, wa.1 + (24.0 * k) as i32, w, h);
        *toast = canvas::Canvas::new_toast(rect).map_err(|e| log!("ask toast failed: {e}")).ok();
    }
    let t = toast.as_ref()?;
    for input in canvas::take_toast_inputs() {
        app.ask_input(input);
    }
    match app.ask_draws(t.scale(), t.screen.2 as f32, t.screen.3 as f32) {
        Some(draws) => Some(crate::engine::draw::Surface { hwnd: t.id(), screen: t.screen, draws, takeover: false, transparent: false }),
        None => {
            *toast = None;
            None
        }
    }
}

/// The canvases: one per display the layout uses, plus "Not in use" cards while the layer is
/// open.
#[derive(Default)]
pub struct Canvases {
    /// Layout key, monitor index, window.
    displays: Vec<(String, usize, canvas::Canvas)>,
    /// Monitor index, card window.
    cards: Vec<(usize, canvas::Canvas)>,
}

impl Canvases {
    fn by_hwnd(&self, h: isize) -> Option<usize> {
        self.displays.iter().position(|(_, _, c)| c.id() == h)
    }
    fn of_key(&self, key: &str) -> Option<usize> {
        self.displays.iter().position(|(k, _, _)| k == key)
    }
    fn first_hwnd(&self) -> Option<HWND> {
        self.displays.first().map(|(_, _, c)| c.hwnd)
    }
}

/// Which connected display a screen point is on.
fn monitor_at(app: &App, x: i32, y: i32) -> Option<usize> {
    app.monitors.iter().position(|m| m.contains(x, y))
}

fn compose(app: &mut App, cv: &mut Canvases, panel: &mut Option<canvas::Canvas>, toast: &mut Option<canvas::Canvas>, engine: &crate::engine::Engine, shell: &Shell) {
    use crate::engine::draw::{Frame, Surface};
    use crate::layer::{Effect, Geo, View};
    use crate::model::geometry::{Rect, Size};
    use crate::model::layout::{DisplayMode, MAIN_DISPLAY};

    if app.monitors.is_empty() {
        app.displays_changed(system::monitors());
    }
    let near = app.target().map(|t| HWND(t as *mut _)).or(cv.first_hwnd());
    let panel_was_open = panel.is_some();
    let panel_surface = panel_surface(app, panel, near);
    let ask = ask_surface(app, toast);
    // After the panel and the toast: either can change what is shown (Apply, auto-apply).
    let target = app.target();
    let Some(t) = target else {
        cv.displays.clear();
        cv.cards.clear();
        canvas::take_inputs();
        engine.set(Frame { surfaces: panel_surface.into_iter().chain(ask).collect(), ..Frame::default() });
        return;
    };

    // The source's display, and the layout's displays among those connected.
    let info = engine.info();
    if info.ready {
        app.source_size = (info.client.2, info.client.3);
        app.check_source_shape(info.client.2, info.client.3);
    }
    // Before the first captured frame the client rectangle is still empty: going by its
    // centre would always pick the display at 0,0. Ask Windows which display the window is on.
    let (cx, cy) = (info.client.0 + info.client.2 / 2, info.client.1 + info.client.3 / 2);
    let at_centre = if info.ready && info.client.2 > 0 && info.client.3 > 0 { monitor_at(app, cx, cy) } else { None };
    app.source_monitor = at_centre.or_else(|| {
        let r = canvas::monitor_of(HWND(t as *mut _));
        app.monitors.iter().position(|m| m.rect == r)
    }).unwrap_or(0);
    let layer_open = app.layer_open();
    let cut_view = app.layer.as_ref().is_some_and(|l| l.view == View::Cut);
    if layer_open {
        app.refresh_displays();
    } else {
        app.refresh_displays_quiet();
    }
    let (placed, _) = app.resolve_displays();
    if cut_view || !placed.iter().any(|p| p.key == app.here) {
        app.here = MAIN_DISPLAY.into();
    }

    // One canvas per display in use: black for Takeover, see-through for Float.
    cv.displays.retain(|(k, m, c)| placed.iter().any(|p| &p.key == k && p.monitor == *m && (p.mode == DisplayMode::Float) == c.float && app.monitors.get(*m).is_some_and(|mon| mon.rect == c.screen)));
    for p in &placed {
        if cv.of_key(&p.key).is_none() {
            let Some(m) = app.monitors.get(p.monitor) else { continue };
            let made = if p.mode == DisplayMode::Float { canvas::Canvas::new_float(m.rect) } else { canvas::Canvas::new(m.rect) };
            match made {
                Ok(c) => cv.displays.push((p.key.clone(), p.monitor, c)),
                Err(e) => log!("canvas window failed: {e}"),
            }
        }
    }
    // "Not in use" cards on the other displays while the layer is open.
    let unused: Vec<usize> = if layer_open { (0..app.monitors.len()).filter(|i| !placed.iter().any(|p| p.monitor == *i)).collect() } else { Vec::new() };
    cv.cards.retain(|(m, _)| unused.contains(m));
    for m in &unused {
        if !cv.cards.iter().any(|(c, _)| c == m) {
            let mon = &app.monitors[*m];
            let (w, h) = ((560.0 * mon.scale) as i32, (104.0 * mon.scale) as i32);
            let rect = (mon.rect.0 + (mon.rect.2 - w) / 2, mon.rect.1 + (mon.rect.3 - h) / 2, w, h);
            if let Ok(c) = canvas::Canvas::new_card(rect) {
                cv.cards.push((*m, c));
            }
        }
    }
    if cv.displays.is_empty() {
        return;
    }

    crate::layer::style::set_light(app.light);
    let geo_for = |app: &App, key: &str, m: usize, c: &canvas::Canvas| -> Geo {
        let screen = c.screen;
        let number = if layer_open { app.monitors.get(m).map(|m| m.number).unwrap_or(0) } else { 0 };
        Geo {
            display: key.to_string(),
            here: key == app.here,
            number,
            canvas: Size::new(screen.2 as f64, screen.3 as f64),
            source: Rect::new((info.client.0 - screen.0) as f64, (info.client.1 - screen.1) as f64, info.client.2.max(1) as f64, info.client.3.max(1) as f64),
            source_px: Size::new(info.client.2.max(1) as f64, info.client.3.max(1) as f64),
            scale: c.scale(),
        }
    };

    // While the layer is open every canvas takes the mouse; the one under the pointer has the
    // keyboard and the toolbar.
    if layer_open {
        for (_, _, c) in cv.displays.iter_mut() {
            c.set_interactive(true);
        }
        if panel_was_open && panel.is_none() && let Some(i) = cv.of_key(&app.here) {
            cv.displays[i].2.focus();
        }
    }
    for (hwnd, input) in canvas::take_inputs() {
        if !app.layer_open() || !info.ready {
            continue;
        }
        // A "Not in use" card: choose how to use that display.
        if let Some((m, card)) = cv.cards.iter().find(|(_, c)| c.id() == hwnd) {
            if let canvas::Input::Down(_, x, y, _) = input {
                let (m, sx, sy) = (*m, card.screen.0 + x as i32, card.screen.1 + y as i32);
                let main = cv.of_key(MAIN_DISPLAY).unwrap_or(0);
                let geo = geo_for(app, &cv.displays[main].0, cv.displays[main].1, &cv.displays[main].2);
                if let Some(mode) = mode_menu(&cv.displays[main].2, sx, sy, None, false) {
                    app.set_monitor_mode(m, mode, &geo);
                }
            }
            continue;
        }
        let Some(from) = cv.by_hwnd(hwnd) else { continue };
        let (key, m) = (cv.displays[from].0.clone(), cv.displays[from].1);
        let screen = cv.displays[from].2.screen;
        let cut_view = app.layer.as_ref().is_some_and(|l| l.view == View::Cut);
        let dragging = app.layer.as_ref().is_some_and(|l| l.dragging());
        // Cut view happens on the source's display.
        if cut_view && key != MAIN_DISPLAY {
            continue;
        }
        match &input {
            canvas::Input::Move(x, y) => {
                let (sx, sy) = (screen.0 + *x as i32, screen.1 + *y as i32);
                if dragging {
                    // A cutout dragged off its display shows where it would land.
                    let over = monitor_at(app, sx, sy).filter(|o| *o != m);
                    app.drag_over = over.map(|o| {
                        let r = app.monitors[o].rect;
                        (o, ((sx - r.0) as f32, (sy - r.1) as f32))
                    });
                } else if key != app.here {
                    app.here = key.clone();
                    cv.displays[from].2.focus();
                }
            }
            canvas::Input::Up(canvas::Button::Left, x, y) => {
                let (sx, sy) = (screen.0 + *x as i32, screen.1 + *y as i32);
                let moving = app.layer.as_ref().and_then(|l| l.arr_dragging_number());
                if let (Some(n), Some(o)) = (moving, monitor_at(app, sx, sy).filter(|o| *o != m)) {
                    // Dropped on another display: the drag becomes a send (one undo step).
                    app.drag_over = None;
                    let geo = geo_for(app, &key, m, &cv.displays[from].2);
                    app.layer_input(canvas::Input::LostFocus, &geo);
                    let r = app.monitors[o].rect;
                    let at = Some(((sx - r.0) as f64 / r.2 as f64, (sy - r.1) as f64 / r.3 as f64));
                    send_or_ask(app, &cv.displays[from].2, n, o, at, sx, sy, &geo);
                    continue;
                }
                app.drag_over = None;
            }
            _ => {}
        }
        // Keys act on the selected cutout's display; pointer input on the canvas it arrived on.
        let is_key = matches!(input, canvas::Input::Key(..) | canvas::Input::KeyUp(_) | canvas::Input::Char(_));
        let at = if is_key {
            let sel_key = app.layer.as_ref().and_then(|l| l.selected).and_then(|n| app.active_layout()?.cutout(n).map(|c| c.placement.display.clone()));
            sel_key.and_then(|k| cv.of_key(&k)).filter(|_| !cut_view).unwrap_or(from)
        } else {
            from
        };
        let mut geo = geo_for(app, &cv.displays[at].0, cv.displays[at].1, &cv.displays[at].2);
        if is_key {
            geo.here = true;
        }
        let c = &cv.displays[from].2;
        match app.layer_input(input, &geo) {
            Effect::Close => app.toggle_layer(),
            Effect::Menu(x, y, n) => layer_menu(app, c, &geo, x, y, n),
            Effect::LayoutMenu(x, y) => layouts_menu(app, c, x, y),
            Effect::Profiles => app.open_profiles(),
            Effect::Settings => app.open_settings(),
            fx @ (Effect::Suggest | Effect::Trim(_)) => {
                app.pixel_job = Some(fx);
                engine.request_snapshot();
            }
            Effect::SendToNext(n) => {
                if let Some(o) = app.next_monitor_for(n) {
                    let (px, py) = (c.screen.0 + (app.layer.as_ref().map(|l| l.pointer.0).unwrap_or(0.0)) as i32, c.screen.1 + (app.layer.as_ref().map(|l| l.pointer.1).unwrap_or(0.0)) as i32);
                    send_or_ask(app, c, n, o, None, px, py, &geo);
                }
            }
            Effect::SendToMap(n, o) => {
                let (px, py) = (c.screen.0 + (app.layer.as_ref().map(|l| l.pointer.0).unwrap_or(0.0)) as i32, c.screen.1 + (app.layer.as_ref().map(|l| l.pointer.1).unwrap_or(0.0)) as i32);
                send_or_ask(app, c, n, o, None, px, py, &geo);
            }
            Effect::DisplayMenu(o, x, y) => {
                let current = app.key_of_monitor(o).and_then(|_| {
                    let (placed, _) = app.resolve_displays();
                    placed.iter().find(|p| p.monitor == o).map(|p| p.mode)
                });
                let is_source = o == app.source_monitor;
                if let Some(mode) = mode_menu(c, c.screen.0 + x as i32, c.screen.1 + y as i32, Some(current), is_source) {
                    app.set_monitor_mode(o, mode, &geo);
                }
            }
            _ => {}
        }
    }
    // Suggest and Trim, once the picture's pixels arrive.
    if app.pixel_job.is_some()
        && let Some(snap) = engine.take_snapshot()
        && let Some(i) = cv.of_key(MAIN_DISPLAY)
    {
        let geo = geo_for(app, MAIN_DISPLAY, cv.displays[i].1, &cv.displays[i].2);
        app.run_pixel_job(snap, &geo);
    }
    // A send chosen from the right-click menu.
    if let Some(crate::layer::Effect::SendToNext(n)) = app.queued.take()
        && let Some(o) = app.next_monitor_for(n)
        && let Some(i) = cv.of_key(&app.here).or(cv.of_key(MAIN_DISPLAY))
    {
        let c = &cv.displays[i].2;
        let geo = geo_for(app, &cv.displays[i].0, cv.displays[i].1, c);
        let (px, py) = (c.screen.0 + c.screen.2 / 2, c.screen.1 + c.screen.3 / 2);
        send_or_ask(app, c, n, o, None, px, py, &geo);
    }
    if !app.layer_open() {
        app.drag_over = None;
        let ours = |h: HWND| cv.displays.iter().any(|(_, _, c)| c.hwnd == h);
        if ours(unsafe { GetForegroundWindow() }) {
            unsafe {
                let _ = SetForegroundWindow(HWND(t as *mut _));
            }
        }
        for (_, _, c) in cv.displays.iter_mut() {
            c.set_interactive(false);
        }
    }

    // The performance readout (Settings), in the layer's readout.
    if app.settings.performance && let Some(l) = app.layer.as_mut() {
        let st = engine.stats();
        let mut lines = vec![format!("Capture {:.0} fps · added delay {:.1} ms{}", st.capture_fps, st.delay_ms, st.gpu_ms.map(|g| format!(" · GPU {g:.2} ms")).unwrap_or_default())];
        let outs: Vec<String> = cv
            .displays
            .iter()
            .filter_map(|(_, m, c)| st.outputs.iter().find(|(h, _)| *h == c.id()).map(|(_, f)| format!("{} {:.0} fps", app.monitors.get(*m).map(|m| m.label()).unwrap_or_default(), f)))
            .collect();
        if !outs.is_empty() {
            lines.push(format!("Output: {}", outs.join(" · ")));
        }
        l.perf = lines;
    } else if let Some(l) = app.layer.as_mut() {
        l.perf.clear();
    }
    // Draw every display.
    let layer_open = app.layer_open();
    let mut surfaces = Vec::new();
    for i in 0..cv.displays.len() {
        let (key, m) = (cv.displays[i].0.clone(), cv.displays[i].1);
        let geo = geo_for(app, &key, m, &cv.displays[i].2);
        let draws = if layer_open {
            Some(if !info.ready {
                Vec::new()
            } else if key == app.here {
                app.layer_draws(&geo)
            } else {
                app.layer_side_draws(&geo, m)
            })
        } else {
            app.live_draws(&geo)
        };
        let c = &mut cv.displays[i].2;
        c.set_visible(draws.is_some());
        if let Some(d) = draws {
            surfaces.push(Surface { hwnd: c.id(), screen: c.screen, draws: d, takeover: !c.float, transparent: c.float });
        }
    }
    for (m, c) in &cv.cards {
        let k = c.scale();
        surfaces.push(Surface { hwnd: c.id(), screen: c.screen, draws: app.card_draws(*m, k, c.screen.2 as f32, c.screen.3 as f32), takeover: false, transparent: false });
    }
    let panel_open = panel_surface.is_some();
    surfaces.extend(panel_surface);
    surfaces.extend(ask);
    let _ = shell;
    // While a panel is open the mouse is for the panel, not the app.
    let pointer = app.active.map(|i| app.profiles[i].profile.pointer).unwrap_or_default();
    engine.set(Frame { target, surfaces, freeze: app.freeze_generation(), mouse: !app.layer_open() && !panel_open, pointer });
}

/// Send a cutout to display `o`; a display not in use first asks Takeover or Float.
#[allow(clippy::too_many_arguments)]
fn send_or_ask(app: &mut App, c: &canvas::Canvas, n: u32, o: usize, at: Option<(f64, f64)>, sx: i32, sy: i32, geo: &crate::layer::Geo) {
    if app.send_to_monitor(n, o, at, None, geo) {
        return;
    }
    if let Some((n, o, at)) = app.pending_send.take()
        && let Some(Some(mode)) = mode_menu(c, sx, sy, None, false)
    {
        app.send_to_monitor(n, o, at, Some(mode), geo);
    }
}

/// Takeover, Float, Not in use. `current`: Some(mode) for a display in use, Some(None) for one
/// not in use, None to ask without a current choice (a display about to be used). Returns
/// Some(choice) or None if dismissed.
fn mode_menu(c: &canvas::Canvas, sx: i32, sy: i32, current: Option<Option<crate::model::layout::DisplayMode>>, is_source: bool) -> Option<Option<crate::model::layout::DisplayMode>> {
    use crate::model::layout::DisplayMode;
    const TAKEOVER: usize = 1;
    const FLOAT: usize = 2;
    const NONE: usize = 3;
    unsafe {
        let menu = CreatePopupMenu().ok()?;
        let add = |flags: MENU_ITEM_FLAGS, id: usize, text: &str| {
            let w: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
            let _ = AppendMenuW(menu, flags, id, PCWSTR(w.as_ptr()));
        };
        let check = |m: Option<DisplayMode>| if current == Some(m) { MF_STRING | MF_CHECKED } else { MF_STRING };
        add(check(Some(DisplayMode::Takeover)), TAKEOVER, "Takeover — black, only cutouts");
        add(check(Some(DisplayMode::Float)), FLOAT, "Float — cutouts over the desktop");
        if current.is_some() {
            add(if is_source { MF_GRAYED } else { check(None) }, NONE, "Not in use");
        }
        let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, sx, sy, Some(0), c.hwnd, None).0 as usize;
        let _ = DestroyMenu(menu);
        match cmd {
            TAKEOVER => Some(Some(DisplayMode::Takeover)),
            FLOAT => Some(Some(DisplayMode::Float)),
            NONE => Some(None),
            _ => None,
        }
    }
}

/// Right-click in the layer: actions for the cutout under the pointer, and the preset grids.
fn layer_menu(app: &mut App, c: &canvas::Canvas, geo: &crate::layer::Geo, x: f32, y: f32, n: Option<u32>) {
    use crate::layer::Action;
    use crate::model::layout::Preset;
    const RENAME: usize = 1;
    const HIDE: usize = 2;
    const DELETE: usize = 3;
    const RETAKE: usize = 4;
    const ROTATE_L: usize = 5;
    const ROTATE_R: usize = 6;
    const MIRROR: usize = 7;
    const DUPLICATE: usize = 8;
    const SEND_NEXT: usize = 9;
    const SCALE0: usize = 50;
    const GRID0: usize = 100;
    use crate::model::layout::Filter;
    const FILTERS: [(Filter, &str); 4] = [
        (Filter::Auto, "Auto (Enhanced when enlarged)"),
        (Filter::Smooth, "Smooth"),
        (Filter::Sharp, "Sharp (pixel art, crisp panels)"),
        (Filter::Enhanced, "Enhanced (FSR 1)"),
    ];
    unsafe {
        let menu = CreatePopupMenu().unwrap();
        let add = |m: HMENU, flags: MENU_ITEM_FLAGS, id: usize, text: &str| {
            let w: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
            let _ = AppendMenuW(m, flags, id, PCWSTR(w.as_ptr()));
        };
        if let Some(cut) = n.and_then(|n| app.active_layout()?.cutout(n).cloned()) {
            add(menu, MF_GRAYED, 0, &format!("{} {}", crate::model::layout::number_label(cut.number), cut.name));
            add(menu, MF_STRING, RENAME, "Rename\tEnter");
            add(menu, MF_STRING, HIDE, if cut.placement.visible { "Hide\tH" } else { "Show\tH" });
            add(menu, MF_STRING, DELETE, "Delete cutout\tDel");
          if app.layer.as_ref().is_some_and(|l| l.view == crate::layer::View::Arrange) {
              add(menu, MF_STRING, ROTATE_L, "Rotate left\tQ");
              add(menu, MF_STRING, ROTATE_R, "Rotate right\tE");
              add(menu, if cut.placement.flip { MF_STRING | MF_CHECKED } else { MF_STRING }, MIRROR, "Mirror");
              add(menu, MF_STRING, DUPLICATE, "Duplicate\tCtrl+D");
              add(menu, MF_STRING, SEND_NEXT, "Send to next display\tD");
          }
            let scaling = CreatePopupMenu().unwrap();
            for (k, (f, label)) in FILTERS.iter().enumerate() {
                add(scaling, if cut.placement.filter == *f { MF_STRING | MF_CHECKED } else { MF_STRING }, SCALE0 + k, label);
            }
            add(menu, MF_POPUP, scaling.0 as usize, "Scaling");
            add(menu, MF_SEPARATOR, 0, "");
        }
        let grids = CreatePopupMenu().unwrap();
        for (i, p) in Preset::ALL.iter().enumerate() {
            add(grids, MF_STRING, GRID0 + i, p.label());
        }
        add(menu, MF_POPUP, grids.0 as usize, "Add a grid of cutouts");
        add(menu, MF_STRING, RETAKE, "Retake still frame\tF5");
        let sx = c.screen.0 + x as i32;
        let sy = c.screen.1 + y as i32;
        let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, sx, sy, Some(0), c.hwnd, None).0 as usize;
        let _ = DestroyMenu(menu);
        match cmd {
            RENAME => {
                app.layer_input(canvas::Input::Key(0x0D, canvas::Mods::default()), geo);
            }
            HIDE => {
                app.layer_action(Action::Hide, geo);
            }
            ROTATE_L => {
                app.layer_input(canvas::Input::Key(0x51, canvas::Mods::default()), geo);
            }
            ROTATE_R => {
                app.layer_input(canvas::Input::Key(0x45, canvas::Mods::default()), geo);
            }
            DUPLICATE => {
                app.layer_input(canvas::Input::Key(0x44, canvas::Mods { ctrl: true, ..Default::default() }), geo);
            }
            SEND_NEXT => {
                // Sending needs the display canvases: the next redraw does it.
                app.queued = app.layer.as_ref().and_then(|l| l.selected).map(crate::layer::Effect::SendToNext);
            }
            MIRROR => app.mirror_selected(geo),
            DELETE => {
                app.layer_action(Action::Delete, geo);
            }
            RETAKE => {
                app.layer_action(Action::Retake, geo);
            }
            k if (SCALE0..SCALE0 + FILTERS.len()).contains(&k) => {
                if let Some(n) = n {
                    app.set_filter(n, FILTERS[k - SCALE0].0, geo);
                }
            }
            k if k >= GRID0 => {
                if let Some(p) = Preset::ALL.get(k - GRID0) {
                    app.add_preset(*p, geo);
                }
            }
            _ => {}
        }
    }
}

/// New profile: a menu of the open app windows, at the pointer. Returns the one chosen.
fn new_profile_menu(owner: HWND) -> Option<HWND> {
    let windows = system::app_windows();
    unsafe {
        let menu = CreatePopupMenu().ok()?;
        let add = |flags: MENU_ITEM_FLAGS, id: usize, text: &str| {
            let w: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
            let _ = AppendMenuW(menu, flags, id, PCWSTR(w.as_ptr()));
        };
        add(MF_GRAYED, 0, "Make a profile for…");
        add(MF_SEPARATOR, 0, "");
        if windows.is_empty() {
            add(MF_GRAYED, 0, "No open app windows");
        }
        for (k, (_, f)) in windows.iter().enumerate().take(30) {
            let title: String = f.title.trim().chars().take(60).collect();
            let label = if title.is_empty() { f.exe_name().to_string() } else { format!("{title}\t{}", f.exe_name()) };
            add(MF_STRING, k + 1, &label);
        }
        let mut pt = windows::Win32::Foundation::POINT::default();
        let _ = GetCursorPos(&mut pt);
        let _ = SetForegroundWindow(owner);
        let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, Some(0), owner, None).0 as usize;
        let _ = DestroyMenu(menu);
        (cmd >= 1).then(|| windows.get(cmd - 1).map(|(h, _)| HWND(*h as *mut _))).flatten()
    }
}

/// The readout's menu: switch to another layout, rename this one, or start a new one.
fn layouts_menu(app: &mut App, c: &canvas::Canvas, x: f32, y: f32) {
    const RENAME: usize = 1;
    const NEW: usize = 2;
    const LAYOUT0: usize = 100;
    let layouts = app.layouts();
    let current = layouts.iter().find(|l| l.2).map(|l| l.1.clone()).unwrap_or_default();
    unsafe {
        let menu = CreatePopupMenu().unwrap();
        let add = |flags: MENU_ITEM_FLAGS, id: usize, text: &str| {
            let w: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
            let _ = AppendMenuW(menu, flags, id, PCWSTR(w.as_ptr()));
        };
        add(MF_GRAYED, 0, "Layouts");
        for (k, (_, name, active)) in layouts.iter().enumerate() {
            add(if *active { MF_STRING | MF_CHECKED } else { MF_STRING }, LAYOUT0 + k, name);
        }
        add(MF_SEPARATOR, 0, "");
        add(MF_STRING, RENAME, &format!("Rename \u{201c}{current}\u{201d}…"));
        add(MF_STRING, NEW, &format!("New layout (a copy of \u{201c}{current}\u{201d})"));
        let sx = c.screen.0 + x as i32;
        let sy = c.screen.1 + y as i32;
        let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN, sx, sy, Some(0), c.hwnd, None).0 as usize;
        let _ = DestroyMenu(menu);
        match cmd {
            RENAME => app.begin_rename_layout(),
            NEW => app.new_layout(),
            k if k >= LAYOUT0 => {
                if let Some((id, _, _)) = layouts.get(k - LAYOUT0) {
                    app.switch_layout(id);
                }
            }
            _ => {}
        }
    }
}

/// Marks a WM_COPYDATA as Kiritori's command line ("KIRI").
const COPYDATA_ARGS: usize = 0x4B49_5249;

/// If Kiritori is already running, hand it these arguments (NUL-separated UTF-16) and return
/// true.
pub fn forward_to_running(args: &[String]) -> bool {
    unsafe {
        let Ok(hwnd) = FindWindowW(CLASS, None) else { return false };
        let words: Vec<u16> = args.join("\0").encode_utf16().collect();
        let cds = COPYDATASTRUCT { dwData: COPYDATA_ARGS, cbData: (words.len() * 2) as u32, lpData: words.as_ptr() as *mut _ };
        let mut result = 0usize;
        SendMessageTimeoutW(hwnd, WM_COPYDATA, WPARAM(0), LPARAM(&cds as *const _ as isize), SMTO_ABORTIFHUNG, 5000, Some(&mut result)).0 != 0
    }
}

/// A message box for command-line mistakes, when there is no tray icon to speak through.
/// An information box (the command-line help).
pub fn info(text: &str) {
    let w: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    unsafe {
        MessageBoxW(None, PCWSTR(w.as_ptr()), w!("Kiritori"), MB_OK | MB_ICONINFORMATION);
    }
}

pub fn message(text: &str) {
    let w: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    unsafe {
        MessageBoxW(None, PCWSTR(w.as_ptr()), w!("Kiritori"), MB_OK | MB_ICONWARNING);
    }
}
