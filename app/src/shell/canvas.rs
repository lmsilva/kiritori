//! Canvas windows, owned by the shell thread so the layer's input arrives where the app state
//! lives. The engine draws into them from its own thread.
//!
//! Two modes: passive (the live result: click-through, never focused) and
//! interactive (the layer is open: takes the mouse and keyboard).

use std::cell::RefCell;

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

pub fn mods() -> Mods {
    unsafe {
        Mods {
            ctrl: GetKeyState(VK_CONTROL.0 as i32) < 0,
            shift: GetKeyState(VK_SHIFT.0 as i32) < 0,
            alt: GetKeyState(VK_MENU.0 as i32) < 0,
        }
    }
}

/// Input on a canvas, in that canvas's pixels.
#[derive(Clone, Debug)]
pub enum Input {
    Move(f32, f32),
    Down(Button, f32, f32, Mods),
    Up(Button, f32, f32),
    Key(u16, Mods),
    KeyUp(u16),
    Char(char),
    LostFocus,
}

thread_local! {
    /// Input on the display canvases and cards, with the window it arrived on.
    static INPUTS: RefCell<Vec<(isize, Input)>> = const { RefCell::new(Vec::new()) };
    /// "Not in use" cards: clickable, never focused.
    static CARDS: RefCell<Vec<isize>> = const { RefCell::new(Vec::new()) };
    static PANEL_INPUTS: RefCell<Vec<Input>> = const { RefCell::new(Vec::new()) };
    static INTERACTIVE: RefCell<bool> = const { RefCell::new(false) };
    /// The panel window (Profiles), whose input goes to its own queue.
    static PANEL: RefCell<isize> = const { RefCell::new(0) };
    /// The Ask toast: clickable, never focused.
    static TOAST: RefCell<isize> = const { RefCell::new(0) };
    static TOAST_INPUTS: RefCell<Vec<Input>> = const { RefCell::new(Vec::new()) };
}

/// Test mode (`KIRITORI_NO_FOCUS=1`): Kiritori's windows never take the keyboard focus, so
/// automated checks can drive them with posted messages while someone keeps working.
pub fn no_focus() -> bool {
    static NO: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *NO.get_or_init(|| std::env::var("KIRITORI_NO_FOCUS").is_ok_and(|v| v == "1"))
}

fn take_focus(h: HWND) {
    if no_focus() {
        return;
    }
    unsafe {
        if GetForegroundWindow() != h {
            let _ = SetForegroundWindow(h);
        }
        let _ = SetFocus(Some(h));
    }
}

/// Input on the Ask toast.
pub fn take_toast_inputs() -> Vec<Input> {
    TOAST_INPUTS.with(|q| std::mem::take(&mut *q.borrow_mut()))
}

pub fn take_inputs() -> Vec<(isize, Input)> {
    INPUTS.with(|q| std::mem::take(&mut *q.borrow_mut()))
}

/// Input on the panel window.
pub fn take_panel_inputs() -> Vec<Input> {
    PANEL_INPUTS.with(|q| std::mem::take(&mut *q.borrow_mut()))
}

extern "system" fn proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let x = (lp.0 & 0xFFFF) as i16 as f32;
    let y = ((lp.0 >> 16) & 0xFFFF) as i16 as f32;
    let is_panel = PANEL.with(|p| *p.borrow()) == h.0 as isize;
    let is_toast = TOAST.with(|p| *p.borrow()) == h.0 as isize;
    let is_card = CARDS.with(|c| c.borrow().contains(&(h.0 as isize)));
    let push = |i: Input| {
        if is_panel {
            PANEL_INPUTS.with(|q| q.borrow_mut().push(i));
        } else if is_toast {
            TOAST_INPUTS.with(|q| q.borrow_mut().push(i));
        } else {
            INPUTS.with(|q| q.borrow_mut().push((h.0 as isize, i)));
        }
    };
    let interactive = is_panel || (!is_toast && !is_card && INTERACTIVE.with(|i| *i.borrow()));
    match msg {
        WM_MOUSEACTIVATE if !interactive => LRESULT(MA_NOACTIVATE as isize),
        WM_SETCURSOR if is_toast || is_card => {
            unsafe { SetCursor(LoadCursorW(None, IDC_HAND).ok()) };
            LRESULT(1)
        }
        WM_MOUSEMOVE => {
            push(Input::Move(x, y));
            LRESULT(0)
        }
        WM_LBUTTONDOWN | WM_RBUTTONDOWN => {
            unsafe { SetCapture(h) };
            push(Input::Down(if msg == WM_LBUTTONDOWN { Button::Left } else { Button::Right }, x, y, mods()));
            LRESULT(0)
        }
        WM_LBUTTONUP | WM_RBUTTONUP => {
            unsafe {
                let _ = ReleaseCapture();
            }
            push(Input::Up(if msg == WM_LBUTTONUP { Button::Left } else { Button::Right }, x, y));
            LRESULT(0)
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            push(Input::Key(wp.0 as u16, mods()));
            LRESULT(0)
        }
        WM_KEYUP | WM_SYSKEYUP => {
            push(Input::KeyUp(wp.0 as u16));
            LRESULT(0)
        }
        WM_CHAR => {
            if let Some(c) = char::from_u32(wp.0 as u32) {
                push(Input::Char(c));
            }
            LRESULT(0)
        }
        WM_KILLFOCUS => {
            push(Input::LostFocus);
            LRESULT(0)
        }
        WM_SETCURSOR if interactive => {
            unsafe { SetCursor(LoadCursorW(None, if is_panel { IDC_ARROW } else { IDC_CROSS }).ok()) };
            LRESULT(1)
        }
        _ => unsafe { DefWindowProcW(h, msg, wp, lp) },
    }
}

pub struct Canvas {
    pub hwnd: HWND,
    /// The display it covers, in screen pixels: x, y, w, h.
    pub screen: (i32, i32, i32, i32),
    visible: bool,
    interactive: bool,
    /// A panel window rather than a display's canvas.
    panel: bool,
    toast: bool,
    card: bool,
    /// See-through (Float): made with WS_EX_NOREDIRECTIONBITMAP for DirectComposition.
    pub float: bool,
}

const PASSIVE_EX: WINDOW_EX_STYLE = WINDOW_EX_STYLE(WS_EX_TOPMOST.0 | WS_EX_NOACTIVATE.0 | WS_EX_TOOLWINDOW.0 | WS_EX_LAYERED.0 | WS_EX_TRANSPARENT.0);
const INTERACTIVE_EX: WINDOW_EX_STYLE = WINDOW_EX_STYLE(WS_EX_TOPMOST.0 | WS_EX_TOOLWINDOW.0 | WS_EX_LAYERED.0);

impl Canvas {
    pub fn new(screen: (i32, i32, i32, i32)) -> Result<Self> {
        unsafe {
            let instance = GetModuleHandleW(None)?;
            let class = w!("KiritoriCanvas");
            let wc = WNDCLASSW { lpfnWndProc: Some(proc), hInstance: instance.into(), lpszClassName: class, hCursor: LoadCursorW(None, IDC_ARROW)?, ..Default::default() };
            RegisterClassW(&wc);
            let hwnd = CreateWindowExW(PASSIVE_EX, class, w!("Kiritori"), WS_POPUP, screen.0, screen.1, screen.2, screen.3, None, None, Some(instance.into()), None)?;
            SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA)?;
            Ok(Canvas { hwnd, screen, visible: false, interactive: false, panel: false, toast: false, card: false, float: false })
        }
    }

    /// A Float display's canvas: the desktop shows wherever nothing is drawn.
    pub fn new_float(screen: (i32, i32, i32, i32)) -> Result<Self> {
        unsafe {
            let instance = GetModuleHandleW(None)?;
            let class = w!("KiritoriCanvas");
            let wc = WNDCLASSW { lpfnWndProc: Some(proc), hInstance: instance.into(), lpszClassName: class, hCursor: LoadCursorW(None, IDC_ARROW)?, ..Default::default() };
            RegisterClassW(&wc);
            let ex = WINDOW_EX_STYLE(PASSIVE_EX.0 | WS_EX_NOREDIRECTIONBITMAP.0);
            let hwnd = CreateWindowExW(ex, class, w!("Kiritori"), WS_POPUP, screen.0, screen.1, screen.2, screen.3, None, None, Some(instance.into()), None)?;
            Ok(Canvas { hwnd, screen, visible: false, interactive: false, panel: false, toast: false, card: false, float: true })
        }
    }

    /// A "Not in use" card in the middle of a display: clickable, never focused.
    pub fn new_card(rect: (i32, i32, i32, i32)) -> Result<Self> {
        let mut c = Self::new(rect)?;
        c.card = true;
        CARDS.with(|cs| cs.borrow_mut().push(c.hwnd.0 as isize));
        unsafe {
            SetWindowLongPtrW(c.hwnd, GWL_EXSTYLE, (WS_EX_TOPMOST.0 | WS_EX_NOACTIVATE.0 | WS_EX_TOOLWINDOW.0 | WS_EX_LAYERED.0) as isize);
            let _ = SetWindowPos(c.hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_FRAMECHANGED | SWP_NOACTIVATE);
            let _ = ShowWindow(c.hwnd, SW_SHOWNOACTIVATE);
        }
        c.visible = true;
        Ok(c)
    }

    /// Give this canvas the keyboard (the pointer moved onto its display).
    pub fn focus(&self) {
        take_focus(self.hwnd);
    }

    /// The Ask toast at `rect`: on top and clickable, but it never takes the keyboard from the
    /// app in front.
    pub fn new_toast(rect: (i32, i32, i32, i32)) -> Result<Self> {
        let mut c = Self::new(rect)?;
        c.toast = true;
        TOAST.with(|p| *p.borrow_mut() = c.hwnd.0 as isize);
        unsafe {
            SetWindowLongPtrW(c.hwnd, GWL_EXSTYLE, (WS_EX_TOPMOST.0 | WS_EX_NOACTIVATE.0 | WS_EX_TOOLWINDOW.0 | WS_EX_LAYERED.0) as isize);
            let _ = SetWindowPos(c.hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_FRAMECHANGED | SWP_NOACTIVATE);
            let _ = ShowWindow(c.hwnd, SW_SHOWNOACTIVATE);
        }
        c.visible = true;
        Ok(c)
    }

    /// A panel window (Profiles) at `rect` in screen pixels: shown, on top and focused.
    pub fn new_panel(rect: (i32, i32, i32, i32)) -> Result<Self> {
        let mut c = Self::new(rect)?;
        c.panel = true;
        PANEL.with(|p| *p.borrow_mut() = c.hwnd.0 as isize);
        unsafe {
            SetWindowLongPtrW(c.hwnd, GWL_EXSTYLE, (WS_EX_TOPMOST.0 | WS_EX_TOOLWINDOW.0 | WS_EX_LAYERED.0) as isize);
            let _ = SetWindowPos(c.hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_FRAMECHANGED);
            let _ = ShowWindow(c.hwnd, if no_focus() { SW_SHOWNOACTIVATE } else { SW_SHOW });
        }
        take_focus(c.hwnd);
        c.visible = true;
        c.interactive = true;
        Ok(c)
    }

    pub fn id(&self) -> isize {
        self.hwnd.0 as isize
    }

    /// Pixels per 96-dpi unit on this display, for sizing chrome.
    pub fn scale(&self) -> f32 {
        (unsafe { GetDpiForWindow(self.hwnd) } as f32 / 96.0).max(1.0)
    }

    pub fn set_visible(&mut self, on: bool) {
        if self.visible != on {
            unsafe {
                let _ = ShowWindow(self.hwnd, if on { SW_SHOWNOACTIVATE } else { SW_HIDE });
            }
            self.visible = on;
        }
    }

    pub fn visible(&self) -> bool {
        self.visible
    }

    /// Interactive while the layer is open: takes focus, mouse and keyboard.
    pub fn set_interactive(&mut self, on: bool) {
        if self.interactive == on || self.panel || self.toast || self.card {
            return;
        }
        self.interactive = on;
        INTERACTIVE.with(|i| *i.borrow_mut() = on);
        unsafe {
            let float = if self.float { WS_EX_NOREDIRECTIONBITMAP.0 } else { 0 };
            SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, ((if on { INTERACTIVE_EX } else { PASSIVE_EX }).0 | float) as isize);
            let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_FRAMECHANGED | SWP_NOACTIVATE);
            if on {
                self.set_visible(true);
                take_focus(self.hwnd);
            }
        }
    }
}

impl Drop for Canvas {
    fn drop(&mut self) {
        if self.panel {
            PANEL.with(|p| *p.borrow_mut() = 0);
        } else if self.toast {
            TOAST.with(|p| *p.borrow_mut() = 0);
        } else if self.card {
            CARDS.with(|cs| cs.borrow_mut().retain(|h| *h != self.hwnd.0 as isize));
        } else {
            INTERACTIVE.with(|i| *i.borrow_mut() = false);
        }
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

/// The work area (the display minus the taskbar) of the display a window is on, or of the
/// primary display.
pub fn work_area_of(hwnd: Option<HWND>) -> (i32, i32, i32, i32) {
    unsafe {
        let m = match hwnd {
            Some(h) => MonitorFromWindow(h, MONITOR_DEFAULTTOPRIMARY),
            None => MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY),
        };
        let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(m, &mut mi);
        let r = mi.rcWork;
        (r.left, r.top, r.right - r.left, r.bottom - r.top)
    }
}

/// Pixels per 96-dpi unit at a screen point.
pub fn scale_at(x: i32, y: i32) -> f32 {
    use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
    unsafe {
        let m = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(m, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        (dx as f32 / 96.0).max(1.0)
    }
}

/// The display a window is on, in screen pixels.
pub fn monitor_of(hwnd: HWND) -> (i32, i32, i32, i32) {
    unsafe {
        let m = MonitorFromWindow(hwnd, MONITOR_DEFAULTTOPRIMARY);
        let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(m, &mut mi);
        let r = mi.rcMonitor;
        (r.left, r.top, r.right - r.left, r.bottom - r.top)
    }
}
