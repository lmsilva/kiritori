//! Kiritori stand-in: a fake four-quadrant "game" used to test Kiritori without a real one.
//!
//! It draws a 2×2 picture (a synthetic test pattern, or any image given with --image), animates
//! each quadrant so a frozen picture is easy to spot, and marks and logs every click at the exact
//! source pixel it lands on. It can run framed, borderless or exclusive full screen.
//!
//! Keys while focused: 1 framed · 2 borderless · 3 exclusive full screen · C clear markers · Ctrl+Q quit.

#![windows_subsystem = "windows"]

use std::cell::RefCell;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::time::Instant;

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Security::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

const FORMAT: DXGI_FORMAT = DXGI_FORMAT_R8G8B8A8_UNORM;
const MAX_MARKERS: usize = 32;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Mode {
    Framed,
    Borderless,
    Fullscreen,
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Mode::Framed => "framed",
            Mode::Borderless => "borderless",
            Mode::Fullscreen => "fullscreen",
        }
    }
}

struct Args {
    width: u32,
    height: u32,
    mode: Mode,
    image: Option<String>,
    title: String,
    display: Option<usize>,
    /// `--timing file`: one line per frame, "frame qpc", taken just after Present, for
    /// measuring Kiritori's added lag (tools/latency).
    timing: Option<String>,
    /// `--plain`: the picture alone, without the moving blocks and frame counters (for
    /// screenshots).
    plain: bool,
}

fn parse_args() -> std::result::Result<Args, String> {
    let mut a = Args {
        width: 1920,
        height: 1080,
        mode: Mode::Framed,
        image: None,
        title: "Kiritori stand-in".into(),
        display: None,
        timing: None,
        plain: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut val = || it.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--size" => {
                let v = val()?;
                let (w, h) = v.split_once(['x', 'X']).ok_or("--size is WIDTHxHEIGHT")?;
                a.width = w.parse().map_err(|_| "bad width")?;
                a.height = h.parse().map_err(|_| "bad height")?;
            }
            "--mode" => {
                a.mode = match val()?.as_str() {
                    "framed" | "windowed" => Mode::Framed,
                    "borderless" => Mode::Borderless,
                    "fullscreen" => Mode::Fullscreen,
                    m => return Err(format!("unknown mode {m}")),
                }
            }
            "--image" => a.image = Some(val()?),
            "--title" => a.title = val()?,
            "--display" => a.display = Some(val()?.parse().map_err(|_| "bad display number")?),
            "--timing" => a.timing = Some(val()?),
            "--plain" => a.plain = true,
            "--help" | "-h" => {
                return Err("kiritori-standin [--size 1920x1080] [--mode framed|borderless|fullscreen] \
                            [--image path] [--title text] [--display N] [--timing file] [--plain]"
                    .into());
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(a)
}

// ---------------------------------------------------------------------------------------------
// Picture

/// RGBA pixels at the render size.
fn build_picture(args: &Args) -> std::result::Result<Vec<u8>, String> {
    let (w, h) = (args.width, args.height);
    if let Some(path) = &args.image {
        let img = image::open(path).map_err(|e| format!("cannot open {path}: {e}"))?;
        let img = img.resize_exact(w, h, image::imageops::FilterType::Triangle).to_rgba8();
        return Ok(img.into_raw());
    }
    // Synthetic test pattern: four tinted quadrants, a 40 px grid, 10 px ticks along each
    // quadrant's top and left edge, and a 1 px checkerboard patch to judge scaling sharpness.
    let tints: [[u8; 3]; 4] = [[40, 90, 160], [150, 60, 60], [50, 130, 70], [140, 110, 40]];
    let (qw, qh) = (w / 2, h / 2);
    let mut px = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let q = ((y >= qh) as usize) * 2 + (x >= qw) as usize;
            let (lx, ly) = (x % qw.max(1), y % qh.max(1));
            let base = tints[q];
            let mut c = [base[0] / 2, base[1] / 2, base[2] / 2];
            if lx % 40 == 0 || ly % 40 == 0 {
                c = base;
            }
            if (ly < 6 && lx % 10 == 0) || (lx < 6 && ly % 10 == 0) {
                c = [230, 230, 230];
            }
            if lx == 0 || ly == 0 || lx == qw - 1 || ly == qh - 1 {
                c = [255, 255, 255];
            }
            if (20..84).contains(&lx) && (ly + 84 >= qh && ly + 20 < qh) {
                let on = (x + y) % 2 == 0;
                c = if on { [255, 255, 255] } else { [0, 0, 0] };
            }
            let i = ((y * w + x) * 4) as usize;
            px[i..i + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
        }
    }
    Ok(px)
}

// ---------------------------------------------------------------------------------------------
// Events from the window procedure

enum Event {
    Click { down: bool, button: &'static str, cx: i32, cy: i32, origin: &'static str },
    Key { vk: u16, ctrl: bool },
    Close,
}

thread_local! {
    static EVENTS: RefCell<Vec<Event>> = const { RefCell::new(Vec::new()) };
}

fn push(e: Event) {
    EVENTS.with(|v| v.borrow_mut().push(e));
}

fn input_origin() -> &'static str {
    // Mouse messages synthesised from touch or pen carry a signature in the extra info.
    let extra = unsafe { GetMessageExtraInfo() }.0 as usize;
    if extra & 0xFFFF_FF00 == 0xFF51_5700 {
        if extra & 0x80 != 0 { "touch" } else { "pen" }
    } else {
        "mouse"
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let x = (lp.0 & 0xFFFF) as i16 as i32;
    let y = ((lp.0 >> 16) & 0xFFFF) as i16 as i32;
    let click = |down, button| {
        push(Event::Click { down, button, cx: x, cy: y, origin: input_origin() });
        LRESULT(0)
    };
    match msg {
        WM_LBUTTONDOWN => click(true, "left"),
        WM_LBUTTONUP => click(false, "left"),
        WM_RBUTTONDOWN => click(true, "right"),
        WM_RBUTTONUP => click(false, "right"),
        WM_MBUTTONDOWN => click(true, "middle"),
        WM_MBUTTONUP => click(false, "middle"),
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
            push(Event::Key { vk: wp.0 as u16, ctrl });
            if msg == WM_SYSKEYDOWN {
                unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
            } else {
                LRESULT(0)
            }
        }
        WM_CLOSE => {
            push(Event::Close);
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

// ---------------------------------------------------------------------------------------------
// Helpers

fn is_elevated() -> bool {
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elev = TOKEN_ELEVATION::default();
        let mut len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elev as *mut _ as *mut _),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
        .is_ok();
        let _ = CloseHandle(token);
        ok && elev.TokenIsElevated != 0
    }
}

fn monitors() -> Vec<RECT> {
    unsafe extern "system" fn cb(_: HMONITOR, _: HDC, r: *mut RECT, data: LPARAM) -> BOOL {
        unsafe { (*(data.0 as *mut Vec<RECT>)).push(*r) };
        TRUE
    }
    let mut v: Vec<RECT> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(cb), LPARAM(&mut v as *mut _ as isize));
    }
    v
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

struct Logger(Option<std::fs::File>);

impl Logger {
    fn new() -> Self {
        let path = std::env::current_exe()
            .ok()
            .map(|p| p.with_file_name("kiritori-standin.log"));
        Logger(path.and_then(|p| OpenOptions::new().create(true).append(true).open(p).ok()))
    }
    fn line(&mut self, t: f64, s: &str) {
        let l = format!("{t:10.3}  {s}\n");
        if let Some(f) = &mut self.0 {
            let _ = f.write_all(l.as_bytes());
        }
        eprint!("{l}");
    }
}

// ---------------------------------------------------------------------------------------------
// App

struct Marker {
    x: i32,
    y: i32,
    held: bool,
}

struct App {
    args: Args,
    hwnd: HWND,
    mode: Mode,
    ctx: ID3D11DeviceContext1,
    swap: IDXGISwapChain1,
    device: ID3D11Device,
    picture: ID3D11Texture2D,
    markers: Vec<Marker>,
    frame: u64,
    start: Instant,
    log: Logger,
    last_click: String,
    elevated: bool,
    timing: Option<std::fs::File>,
}

impl App {
    fn target_monitor(&self) -> RECT {
        let mons = monitors();
        let pick = self.args.display.and_then(|n| mons.get(n.saturating_sub(1)).copied());
        pick.unwrap_or_else(|| unsafe {
            let m = MonitorFromWindow(self.hwnd, MONITOR_DEFAULTTOPRIMARY);
            let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
            let _ = GetMonitorInfoW(m, &mut mi);
            mi.rcMonitor
        })
    }

    fn apply_mode(&mut self, mode: Mode) -> Result<()> {
        unsafe {
            if self.mode == Mode::Fullscreen && mode != Mode::Fullscreen {
                self.swap.SetFullscreenState(false, None)?;
            }
            let (w, h) = (self.args.width as i32, self.args.height as i32);
            let mon = self.target_monitor();
            match mode {
                Mode::Framed => {
                    let style = WS_OVERLAPPEDWINDOW | WS_VISIBLE;
                    SetWindowLongPtrW(self.hwnd, GWL_STYLE, style.0 as isize);
                    let mut r = RECT { left: 0, top: 0, right: w, bottom: h };
                    AdjustWindowRectEx(&mut r, style, false, WINDOW_EX_STYLE(0))?;
                    SetWindowPos(
                        self.hwnd,
                        None,
                        mon.left + 40,
                        mon.top + 40,
                        r.right - r.left,
                        r.bottom - r.top,
                        SWP_FRAMECHANGED | SWP_NOZORDER | SWP_SHOWWINDOW,
                    )?;
                }
                Mode::Borderless | Mode::Fullscreen => {
                    SetWindowLongPtrW(self.hwnd, GWL_STYLE, (WS_POPUP | WS_VISIBLE).0 as isize);
                    SetWindowPos(
                        self.hwnd,
                        None,
                        mon.left,
                        mon.top,
                        w,
                        h,
                        SWP_FRAMECHANGED | SWP_NOZORDER | SWP_SHOWWINDOW,
                    )?;
                }
            }
            if mode == Mode::Fullscreen {
                self.swap.SetFullscreenState(true, None)?;
            }
            self.swap.ResizeBuffers(
                0,
                self.args.width,
                self.args.height,
                DXGI_FORMAT_UNKNOWN,
                DXGI_SWAP_CHAIN_FLAG_ALLOW_MODE_SWITCH,
            )?;
        }
        self.mode = mode;
        let t = self.now();
        self.log.line(t, &format!("mode {}", mode.name()));
        Ok(())
    }

    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    /// Client point → source pixel. The swap chain stretches the render size over the client area.
    fn to_source(&self, cx: i32, cy: i32) -> (i32, i32) {
        let mut r = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut r);
        }
        let (cw, ch) = ((r.right - r.left).max(1), (r.bottom - r.top).max(1));
        (
            (cx as i64 * self.args.width as i64 / cw as i64) as i32,
            (cy as i64 * self.args.height as i64 / ch as i64) as i32,
        )
    }

    fn quadrant(&self, x: i32, y: i32) -> &'static str {
        let right = x >= self.args.width as i32 / 2;
        let bottom = y >= self.args.height as i32 / 2;
        match (bottom, right) {
            (false, false) => "TL",
            (false, true) => "TR",
            (true, false) => "BL",
            (true, true) => "BR",
        }
    }

    fn handle(&mut self, e: Event) -> bool {
        match e {
            Event::Close => return false,
            Event::Click { down, button, cx, cy, origin } => {
                let (sx, sy) = self.to_source(cx, cy);
                let q = self.quadrant(sx, sy);
                let t = self.now();
                let kind = if down { "down" } else { "up  " };
                self.log.line(
                    t,
                    &format!("{button:6} {kind} source {sx},{sy} {q}  client {cx},{cy}  via {origin}"),
                );
                if down {
                    self.markers.push(Marker { x: sx, y: sy, held: true });
                    if self.markers.len() > MAX_MARKERS {
                        self.markers.remove(0);
                    }
                    self.last_click = format!("{sx},{sy} {q} ({origin})");
                } else {
                    for m in &mut self.markers {
                        m.held = false;
                    }
                }
            }
            Event::Key { vk, ctrl } => {
                let t = self.now();
                self.log.line(t, &format!("key 0x{vk:02X}{}", if ctrl { " +ctrl" } else { "" }));
                let target = match vk {
                    0x31 => Some(Mode::Framed),
                    0x32 => Some(Mode::Borderless),
                    0x33 => Some(Mode::Fullscreen),
                    _ => None,
                };
                if let Some(m) = target
                    && let Err(err) = self.apply_mode(m) {
                        self.log.line(t, &format!("mode change failed: {err}"));
                    }
                if vk == 0x43 {
                    self.markers.clear();
                }
                if vk == 0x51 && ctrl {
                    return false;
                }
            }
        }
        true
    }

    fn render(&mut self) -> Result<()> {
        unsafe {
            let back: ID3D11Texture2D = self.swap.GetBuffer(0)?;
            self.ctx.CopyResource(&back, &self.picture);
            let mut rtv = None;
            self.device.CreateRenderTargetView(&back, None, Some(&mut rtv))?;
            let rtv = rtv.unwrap();

            let (w, h) = (self.args.width as i32, self.args.height as i32);
            let (qw, qh) = (w / 2, h / 2);
            let t = self.now();
            let fill = |rects: &[RECT], c: [f32; 4]| self.ctx.ClearView(&rtv, &c, Some(rects));

            for q in (0..4).filter(|_| !self.args.plain) {
                let (ox, oy) = ((q % 2) * qw, (q / 2) * qh);
                // A block sweeping across each quadrant, at a different speed per quadrant.
                let speed = 0.25 + 0.1 * q as f64;
                let p = ((t * speed).fract() * (qw - 60) as f64) as i32;
                let by = oy + qh / 2 - 15;
                fill(&[RECT { left: ox + p, top: by, right: ox + p + 60, bottom: by + 30 }], [1.0, 1.0, 1.0, 1.0]);
                // Frame counter in binary: 16 cells, lit = 1.
                let mut cells = Vec::new();
                for bit in 0..16 {
                    if self.frame >> bit & 1 == 1 {
                        let x = ox + 20 + (15 - bit) * 14;
                        cells.push(RECT { left: x, top: oy + 20, right: x + 10, bottom: oy + 30 });
                    }
                }
                if !cells.is_empty() {
                    fill(&cells, [1.0, 1.0, 0.2, 1.0]);
                }
            }

            for m in &self.markers {
                let c = if m.held { [1.0, 0.2, 0.2, 1.0] } else { [1.0, 0.9, 0.0, 1.0] };
                fill(
                    &[
                        RECT { left: m.x - 12, top: m.y, right: m.x + 13, bottom: m.y + 1 },
                        RECT { left: m.x, top: m.y - 12, right: m.x + 1, bottom: m.y + 13 },
                    ],
                    c,
                );
                if m.held {
                    fill(&[RECT { left: m.x - 4, top: m.y - 4, right: m.x + 5, bottom: m.y + 5 }], c);
                }
            }
            drop(rtv);
            drop(back);

            let hr = self.swap.Present(1, DXGI_PRESENT(0));
            if hr.is_err() && hr != DXGI_STATUS_OCCLUDED {
                hr.ok()?;
            }
            if let Some(f) = self.timing.as_mut() {
                let mut q = 0i64;
                let _ = windows::Win32::System::Performance::QueryPerformanceCounter(&mut q);
                let _ = writeln!(f, "{} {q}", self.frame & 0xFFFF);
            }
        }
        self.frame += 1;
        if self.frame.is_multiple_of(30) {
            let title = format!(
                "{} · {} · {}×{} · elevated: {} · last click {}",
                self.args.title,
                self.mode.name(),
                self.args.width,
                self.args.height,
                if self.elevated { "yes" } else { "no" },
                if self.last_click.is_empty() { "none" } else { &self.last_click }
            );
            unsafe {
                let _ = SetWindowTextW(self.hwnd, PCWSTR(wide(&title).as_ptr()));
            }
        }
        Ok(())
    }
}

fn fail(msg: &str) -> ! {
    unsafe {
        MessageBoxW(None, PCWSTR(wide(msg).as_ptr()), w!("Kiritori stand-in"), MB_ICONERROR);
    }
    std::process::exit(1);
}

fn main() {
    let args = parse_args().unwrap_or_else(|e| fail(&e));
    let pixels = build_picture(&args).unwrap_or_else(|e| fail(&e));
    if let Err(e) = run(args, pixels) {
        fail(&format!("{e}"));
    }
}

fn run(args: Args, pixels: Vec<u8>) -> Result<()> {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let instance = GetModuleHandleW(None)?;
        let class = w!("KiritoriStandIn");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            lpszClassName: class,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            ..Default::default()
        };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            PCWSTR(wide(&args.title).as_ptr()),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            args.width as i32,
            args.height as i32,
            None,
            None,
            Some(instance.into()),
            None,
        )?;

        let mut device = None;
        let mut ctx = None;
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&[D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0]),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut ctx),
        )?;
        let device: ID3D11Device = device.unwrap();
        let ctx: ID3D11DeviceContext1 = ctx.unwrap().cast()?;

        let factory: IDXGIFactory2 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))?;
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: args.width,
            Height: args.height,
            Format: FORMAT,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            Flags: DXGI_SWAP_CHAIN_FLAG_ALLOW_MODE_SWITCH.0 as u32,
            ..Default::default()
        };
        let swap = factory.CreateSwapChainForHwnd(&device, hwnd, &desc, None, None)?;

        let tex_desc = D3D11_TEXTURE2D_DESC {
            Width: args.width,
            Height: args.height,
            MipLevels: 1,
            ArraySize: 1,
            Format: FORMAT,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_IMMUTABLE,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            ..Default::default()
        };
        let init = D3D11_SUBRESOURCE_DATA {
            pSysMem: pixels.as_ptr() as *const _,
            SysMemPitch: args.width * 4,
            SysMemSlicePitch: 0,
        };
        let mut picture = None;
        device.CreateTexture2D(&tex_desc, Some(&init), Some(&mut picture))?;

        let mode = args.mode;
        let mut app = App {
            args,
            hwnd,
            mode: Mode::Framed,
            ctx,
            swap,
            device,
            picture: picture.unwrap(),
            markers: Vec::new(),
            frame: 0,
            start: Instant::now(),
            log: Logger::new(),
            last_click: String::new(),
            timing: None,
            elevated: is_elevated(),
        };
        app.timing = app.args.timing.as_ref().and_then(|p| std::fs::File::create(p).ok());
        let pid = GetCurrentProcessId();
        let t = app.now();
        app.log.line(
            t,
            &format!(
                "start pid {pid} size {}x{} elevated {} image {}",
                app.args.width,
                app.args.height,
                app.elevated,
                app.args.image.as_deref().unwrap_or("synthetic")
            ),
        );
        app.apply_mode(mode)?;

        'outer: loop {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_QUIT {
                    break 'outer;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            let events: Vec<Event> = EVENTS.with(|v| std::mem::take(&mut *v.borrow_mut()));
            for e in events {
                if !app.handle(e) {
                    break 'outer;
                }
            }
            app.render()?;
        }
        if app.mode == Mode::Fullscreen {
            let _ = app.swap.SetFullscreenState(false, None);
        }
        let t = app.now();
        app.log.line(t, "exit");
        let _ = DestroyWindow(hwnd);
    }
    Ok(())
}
