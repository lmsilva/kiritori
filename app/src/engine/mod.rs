//! The engine: captures the source window and draws each canvas's item list, on its own thread
//! so the tray, shortcuts and layer stay responsive.
//!
//! The shell owns the canvas windows and decides what is drawn (`Frame`); the engine turns
//! that into pixels at each display's own refresh rate and reports facts about the source back
//! (`SourceInfo`). If the process dies, the windows die with it and the original picture is
//! back.

pub mod draw;
pub mod fsr;
pub mod icons;
pub mod input;
pub mod render;

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::log;
use draw::{Draw, EngineCaps, Frame, SourceInfo, Stats};
use render::{Capture, Gpu, Target};

/// What the engine reports back to the shell.
#[derive(Debug)]
pub enum Report {
    /// Something the user needs to know; None clears it.
    Problem(Option<String>),
}

struct Shared {
    frame: Frame,
    generation: u64,
    stop: bool,
    info: SourceInfo,
    caps: Option<EngineCaps>,
    snapshot_wanted: bool,
    snapshot: Option<draw::Snapshot>,
    stats: Stats,
}

pub struct Engine {
    shared: Arc<Mutex<Shared>>,
    wake: HANDLE,
    thread: Option<std::thread::JoinHandle<()>>,
}

struct SendHandle(HANDLE);
unsafe impl Send for SendHandle {}

impl Engine {
    pub fn start(reports: Sender<Report>, notify: impl Fn() + Send + 'static) -> Self {
        let shared = Arc::new(Mutex::new(Shared { frame: Frame::default(), generation: 0, stop: false, info: SourceInfo::default(), caps: None, snapshot_wanted: false, snapshot: None, stats: Stats::default() }));
        let wake = unsafe { CreateEventW(None, false, false, None) }.unwrap_or_default();
        input::start();
        let s2 = shared.clone();
        let w2 = SendHandle(wake);
        let thread = std::thread::Builder::new()
            .name("kiritori-engine".into())
            .spawn(move || {
                let w2 = w2;
                if let Err(e) = run(s2, w2.0, reports, notify) {
                    log!("engine stopped: {e}");
                }
            })
            .ok();
        Engine { shared, wake, thread }
    }

    /// Hand the engine what to draw. Cheap when nothing changed.
    pub fn set(&self, f: Frame) {
        let mut g = self.shared.lock().unwrap();
        if g.frame != f {
            g.frame = f;
            g.generation += 1;
            unsafe {
                let _ = SetEvent(self.wake);
            }
        }
    }

    pub fn info(&self) -> SourceInfo {
        self.shared.lock().unwrap().info
    }

    /// Ask for the picture's pixels; they arrive through `take_snapshot`.
    pub fn request_snapshot(&self) {
        self.shared.lock().unwrap().snapshot_wanted = true;
        unsafe {
            let _ = SetEvent(self.wake);
        }
    }

    /// The performance readout's numbers.
    pub fn stats(&self) -> Stats {
        self.shared.lock().unwrap().stats.clone()
    }

    pub fn take_snapshot(&self) -> Option<draw::Snapshot> {
        self.shared.lock().unwrap().snapshot.take()
    }

    /// The graphics card's capabilities, once the engine has started.
    pub fn caps(&self) -> Option<EngineCaps> {
        self.shared.lock().unwrap().caps.clone()
    }

    /// Stop drawing and capturing, before quitting.
    pub fn stop(mut self) {
        self.shared.lock().unwrap().stop = true;
        unsafe {
            let _ = SetEvent(self.wake);
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        input::release();
    }
}

fn is_elevated(pid: u32) -> Option<bool> {
    use windows::Win32::Security::*;
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut tok = HANDLE::default();
        let ok = OpenProcessToken(h, TOKEN_QUERY, &mut tok).is_ok();
        let _ = CloseHandle(h);
        if !ok {
            return None;
        }
        let mut e = TOKEN_ELEVATION::default();
        let mut len = 0;
        let r = GetTokenInformation(tok, TokenElevation, Some(&mut e as *mut _ as *mut _), size_of::<TOKEN_ELEVATION>() as u32, &mut len);
        let _ = CloseHandle(tok);
        r.ok()?;
        Some(e.TokenIsElevated != 0)
    }
}

/// Mouse needs Kiritori elevated when the app is.
///
/// The problem text for an app that runs as administrator (the shell offers a restart).
pub const ELEVATED: &str = "This app runs as administrator. Restart Kiritori as administrator to control it.";

fn elevation_problem(target: HWND) -> Option<String> {
    let pid = crate::shell::system::window_pid(target);
    let ours = is_elevated(std::process::id()).unwrap_or(false);
    (is_elevated(pid) != Some(false) && !ours).then(|| ELEVATED.to_string())
}

fn client_screen(hwnd: HWND) -> (i32, i32, i32, i32) {
    unsafe {
        let mut cr = RECT::default();
        let _ = GetClientRect(hwnd, &mut cr);
        let mut tl = POINT { x: 0, y: 0 };
        let _ = ClientToScreen(hwnd, &mut tl);
        (tl.x, tl.y, cr.right, cr.bottom)
    }
}

/// Whether any cutout is drawn smaller than it is in the source, so the frame needs its smaller
/// mip levels. `src`: the source texture size.
fn shrinks_source(frame: &Frame, src: Option<(u32, u32)>) -> bool {
    let Some((w, h)) = src else { return true };
    frame.surfaces.iter().flat_map(|s| &s.draws).any(|d| match d {
        Draw::Source { dst, src: r, .. } => {
            let (sw, sh) = (r.w * w as f64, r.h * h as f64);
            (dst.2 as f64) * (dst.3 as f64) < sw * sh * 0.98
        }
        _ => false,
    })
}

/// Hand the input hook the screen-to-app map for the current layout.
fn set_pointer_maps(frame: &Frame, target: Option<HWND>, client: (i32, i32, i32, i32)) {
    use crate::model::profile::Pointer;
    if !frame.mouse || target.is_none() {
        input::set_maps(Vec::new(), Vec::new(), false);
        return;
    }
    let (cx, cy, cw, ch) = client;
    let mut maps = Vec::new();
    let mut takeover = Vec::new();
    for s in &frame.surfaces {
        for d in &s.draws {
            if let Draw::Source { orient, dst, src, shape, input: true, .. } = d {
                maps.push(input::Map {
                    dst: (s.screen.0 as f64 + dst.0 as f64, s.screen.1 as f64 + dst.1 as f64, dst.2 as f64, dst.3 as f64),
                    src: (cx as f64 + src.x * cw as f64, cy as f64 + src.y * ch as f64, src.w * cw as f64, src.h * ch as f64),
                    bounds: *src,
                    shape: shape.clone(),
                    orient: *orient,
                });
            }
        }
        if s.takeover {
            takeover.push(RECT { left: s.screen.0, top: s.screen.1, right: s.screen.0 + s.screen.2, bottom: s.screen.1 + s.screen.3 });
        }
    }
    match frame.pointer {
        Pointer::Translate => input::set_maps(maps, takeover, false),
        // Display-only: every click on a Takeover display is swallowed.
        Pointer::Off => input::set_maps(Vec::new(), takeover, true),
        Pointer::Relative => input::set_maps(Vec::new(), Vec::new(), false),
    }
}

fn run(shared: Arc<Mutex<Shared>>, wake: HANDLE, reports: Sender<Report>, notify: impl Fn()) -> windows::core::Result<()> {
    let report = |r: Report| {
        let _ = reports.send(r);
        notify();
    };
    // Scheduled like a game's own threads, so a busy system never makes the copy wait.
    unsafe {
        let mut task = 0u32;
        let _ = AvSetMmThreadCharacteristicsW(windows::core::w!("Games"), &mut task);
    }
    let gpu = Gpu::new()?;
    shared.lock().unwrap().caps = Some(gpu.caps());
    notify();
    // Signalled by the capture the moment a new frame is ready.
    let arrived = unsafe { CreateEventW(None, false, false, None) }?;
    // Kiritori's own pointer moving wakes the engine too: it is drawn into the picture.
    input::set_wake(wake);
    let mut capture: Option<Capture> = None;
    let mut timer = render::GpuTimer::new(&gpu).ok();
    let mut presents: HashMap<isize, u32> = HashMap::new();
    let (mut delay_sum, mut delay_n) = (0f64, 0u32);
    let mut targets: HashMap<isize, Target> = HashMap::new();
    let mut frame = Frame::default();
    let mut seen = u64::MAX;
    let mut frozen_at = 0u64;
    let mut problem: Option<String> = None;
    let mut rate_at = Instant::now();
    let mut fps = 0.0f32;
    // Canvases whose picture is out of date, and those whose present slot is already ours.
    let mut dirty: HashSet<isize> = HashSet::new();
    let mut ready: HashSet<isize> = HashSet::new();
    let mut last_cursor: Option<(i32, i32)> = None;
    let mut maps_for: Option<(u64, (i32, i32, i32, i32))> = None;

    loop {
        {
            let g = shared.lock().unwrap();
            if g.stop {
                break;
            }
            if g.generation != seen {
                seen = g.generation;
                frame = g.frame.clone();
                dirty.extend(frame.surfaces.iter().map(|s| s.hwnd));
            }
        }

        // Capture follows the target window.
        let target = frame.target.map(|h| HWND(h as *mut _)).filter(|h| unsafe { IsWindow(Some(*h)) }.as_bool());
        if capture.as_ref().map(|c| c.hwnd) != target {
            capture = None;
            frozen_at = 0;
            let mut new_problem = None;
            if let Some(h) = target {
                match Capture::start(&gpu, h, arrived) {
                    Ok(c) => {
                        log!("capturing window {:?}", h.0);
                        capture = Some(c);
                        new_problem = elevation_problem(h);
                    }
                    Err(e) => {
                        log!("capture failed: {e}");
                        new_problem = Some(format!("Kiritori cannot capture this window ({})", e.message()));
                    }
                }
            }
            if new_problem != problem {
                problem = new_problem.clone();
                report(Report::Problem(new_problem));
            }
        }
        if let Some(c) = capture.as_mut() {
            if c.update(&gpu, shrinks_source(&frame, c.source_size()))? && frame.freeze == 0 {
                // Only canvases that show the picture: panels and cards stay as they are.
                dirty.extend(frame.surfaces.iter().filter(|s| s.draws.iter().any(|d| matches!(d, Draw::Source { .. }))).map(|s| s.hwnd));
            }
            if frame.freeze != 0 && frame.freeze != frozen_at {
                c.freeze(&gpu)?;
                frozen_at = frame.freeze;
                dirty.extend(frame.surfaces.iter().map(|s| s.hwnd));
            }
        }
        if std::mem::take(&mut shared.lock().unwrap().snapshot_wanted) {
            match capture.as_ref().map(|c| c.snapshot(&gpu)) {
                Some(Ok(s)) => {
                    shared.lock().unwrap().snapshot = Some(s);
                    notify();
                }
                Some(Err(e)) => log!("snapshot failed: {e}"),
                None => {}
            }
        }
        let client = target.map(client_screen).unwrap_or_default();
        {
            let ready_src = capture.as_ref().and_then(|c| c.source(false)).is_some();
            shared.lock().unwrap().info = SourceInfo { client, ready: ready_src, fps };
        }

        // Swap chains follow the shell's canvas windows.
        targets.retain(|h, _| frame.surfaces.iter().any(|s| s.hwnd == *h));
        ready.retain(|h| targets.contains_key(h));
        for s in &frame.surfaces {
            let size = ((s.screen.2).max(1) as u32, (s.screen.3).max(1) as u32);
            if targets.get(&s.hwnd).is_none_or(|t| t.size != size || t.transparent != s.transparent) {
                targets.remove(&s.hwnd);
                ready.remove(&s.hwnd);
                match Target::new(&gpu, s.hwnd, size, s.transparent) {
                    Ok(t) => {
                        targets.insert(s.hwnd, t);
                        dirty.insert(s.hwnd);
                    }
                    Err(e) => log!("canvas swap chain failed: {e}"),
                }
            }
        }

        // Mouse translation onto the live result: rebuilt only when the layout or the source
        // window changed, not on every wake.
        if maps_for != Some((seen, client)) {
            maps_for = Some((seen, client));
            set_pointer_maps(&frame, target, client);
        }

        // Kiritori's own pointer: redraw when it moved.
        let cursor_at = if frame.mouse && frame.pointer == crate::model::profile::Pointer::Translate { input::own_cursor() } else { None };
        if cursor_at != last_cursor {
            // The canvases it left and the ones it is on now.
            let on = |p: Option<(i32, i32)>, s: &draw::Surface| p.is_some_and(|(x, y)| x >= s.screen.0 && y >= s.screen.1 && x < s.screen.0 + s.screen.2 && y < s.screen.1 + s.screen.3);
            dirty.extend(frame.surfaces.iter().filter(|s| on(cursor_at, s) || on(last_cursor, s)).map(|s| s.hwnd));
            last_cursor = cursor_at;
        }

        if targets.is_empty() {
            unsafe {
                let _ = WaitForMultipleObjects(&[arrived, wake], false, 100);
            }
            continue;
        }

        // Draw every out-of-date canvas whose present slot is free, with the newest frame.
        let order: Vec<isize> = frame.surfaces.iter().map(|s| s.hwnd).filter(|h| targets.contains_key(h)).collect();
        for (i, h) in order.iter().enumerate() {
            if !dirty.contains(h) {
                continue;
            }
            let slot = ready.remove(h) || unsafe { WaitForSingleObject(targets[h].waitable, 0) } == WAIT_OBJECT_0;
            if !slot {
                continue;
            }
            dirty.remove(h);
            let s = frame.surfaces.iter().find(|s| s.hwnd == *h).unwrap();
            let src = capture.as_ref().and_then(|c| c.source(frame.freeze != 0));
            let cursor = cursor_at.map(|(x, y)| (x - s.screen.0, y - s.screen.1)).filter(|(x, y)| *x >= 0 && *y >= 0 && *x < s.screen.2 && *y < s.screen.3);
            // Time the first canvas on the graphics card, now and then.
            let timed = i == 0 && timer.as_mut().is_some_and(|t| t.begin(&gpu));
            match targets.get_mut(h).unwrap().render(&gpu, &s.draws, src, cursor) {
                Ok(()) => {
                    *presents.entry(*h).or_default() += 1;
                    if let Some(at) = capture.as_ref().and_then(|c| c.arrived) {
                        delay_sum += at.elapsed().as_secs_f64() * 1000.0;
                        delay_n += 1;
                    }
                }
                Err(e) => {
                    log!("draw failed: {e}");
                    targets.remove(h);
                }
            }
            if timed && let Some(t) = timer.as_mut() {
                t.end(&gpu);
            }
        }

        // Sleep until there is something new to draw, or a busy canvas frees its slot.
        let waiting: Vec<isize> = order.iter().copied().filter(|h| dirty.contains(h) && targets.contains_key(h)).collect();
        let mut handles = vec![arrived, wake];
        handles.extend(waiting.iter().map(|h| targets[h].waitable));
        let r = unsafe { WaitForMultipleObjects(&handles, false, 100) };
        let i = r.0.wrapping_sub(WAIT_OBJECT_0.0) as usize;
        if (2..handles.len()).contains(&i) {
            // Waiting took this canvas's slot: it is ours to use next round.
            ready.insert(waiting[i - 2]);
        }

        if rate_at.elapsed() >= Duration::from_secs(2) {
            let secs = rate_at.elapsed().as_secs_f32();
            if let Some(c) = capture.as_mut() {
                fps = c.frames as f32 / secs;
                c.frames = 0;
            }
            let stats = Stats {
                capture_fps: fps,
                outputs: presents.drain().map(|(h, n)| (h, n as f32 / secs)).collect(),
                delay_ms: if delay_n > 0 { (delay_sum / delay_n as f64) as f32 } else { 0.0 },
                gpu_ms: timer.as_ref().and_then(|t| t.last_ms),
            };
            (delay_sum, delay_n) = (0.0, 0);
            shared.lock().unwrap().stats = stats;
            rate_at = Instant::now();
        }
    }
    drop(targets);
    drop(capture);
    log!("engine stopped");
    Ok(())
}
