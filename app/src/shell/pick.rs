//! Pick a window: a cyan outline follows whichever window is under the pointer,
//! with its name; a click picks it and the layer opens on it; Esc or a right-click cancels.
//!
//! Built from plain windows so nothing is drawn over the user's apps except the outline:
//! an almost invisible click catcher on every display, four thin bars for the outline, a label
//! with the window's name, and a hint at the top of the display.

use std::cell::RefCell;

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

use super::system;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickInput {
    Move(i32, i32),
    Click(i32, i32),
    Cancel,
}

thread_local! {
    static INPUTS: RefCell<Vec<PickInput>> = const { RefCell::new(Vec::new()) };
    static LABELS: RefCell<Vec<(isize, String)>> = const { RefCell::new(Vec::new()) };
}

pub fn take_inputs() -> Vec<PickInput> {
    INPUTS.with(|q| std::mem::take(&mut *q.borrow_mut()))
}

fn push(i: PickInput) {
    INPUTS.with(|q| q.borrow_mut().push(i));
}

extern "system" fn catcher_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let screen = || unsafe {
        let mut p = POINT { x: (lp.0 & 0xFFFF) as i16 as i32, y: ((lp.0 >> 16) & 0xFFFF) as i16 as i32 };
        let _ = ClientToScreen(h, &mut p);
        (p.x, p.y)
    };
    match msg {
        WM_MOUSEMOVE => {
            let (x, y) = screen();
            push(PickInput::Move(x, y));
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let (x, y) = screen();
            push(PickInput::Click(x, y));
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            push(PickInput::Cancel);
            LRESULT(0)
        }
        WM_KEYDOWN if wp.0 == 0x1B => {
            push(PickInput::Cancel);
            LRESULT(0)
        }
        WM_SETCURSOR => {
            unsafe { SetCursor(LoadCursorW(None, IDC_HAND).ok()) };
            LRESULT(1)
        }
        _ => unsafe { DefWindowProcW(h, msg, wp, lp) },
    }
}

extern "system" fn bar_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(h, msg, wp, lp) }
}

/// Labels: dark glass with white text, drawn with GDI.
extern "system" fn label_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => unsafe {
            let mut ps = PAINTSTRUCT::default();
            let dc = BeginPaint(h, &mut ps);
            let mut r = RECT::default();
            let _ = GetClientRect(h, &mut r);
            let bg = CreateSolidBrush(COLORREF(0x00201E1C));
            FillRect(dc, &r, bg);
            let _ = DeleteObject(bg.into());
            let text = LABELS.with(|l| l.borrow().iter().find(|(k, _)| *k == h.0 as isize).map(|(_, t)| t.clone())).unwrap_or_default();
            let k = (GetDpiForWindow(h) as i32).max(96) / 96;
            let font = CreateFontW(-15 * k, 0, 0, 0, 500, 0, 0, 0, DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS, CLEARTYPE_QUALITY, 0, w!("Segoe UI Variable Text"));
            let old = SelectObject(dc, font.into());
            SetBkMode(dc, TRANSPARENT);
            SetTextColor(dc, COLORREF(0x00FFFFFF));
            let mut w: Vec<u16> = text.encode_utf16().collect();
            let _ = DrawTextW(dc, &mut w, &mut r, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS);
            SelectObject(dc, old);
            let _ = DeleteObject(font.into());
            let _ = EndPaint(h, &ps);
            LRESULT(0)
        },
        _ => unsafe { DefWindowProcW(h, msg, wp, lp) },
    }
}

fn register(class: PCWSTR, proc: WNDPROC, brush: HBRUSH) {
    unsafe {
        let instance = GetModuleHandleW(None).unwrap_or_default();
        let wc = WNDCLASSW { lpfnWndProc: proc, hInstance: instance.into(), lpszClassName: class, hbrBackground: brush, hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(), ..Default::default() };
        RegisterClassW(&wc);
    }
}

fn popup(class: PCWSTR, ex: WINDOW_EX_STYLE, r: RECT) -> Option<HWND> {
    unsafe {
        let instance = GetModuleHandleW(None).ok()?;
        CreateWindowExW(ex | WS_EX_TOPMOST | WS_EX_TOOLWINDOW, class, w!(""), WS_POPUP, r.left, r.top, r.right - r.left, r.bottom - r.top, None, None, Some(instance.into()), None).ok()
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

fn cloaked(h: HWND) -> bool {
    let mut c = 0u32;
    unsafe { DwmGetWindowAttribute(h, DWMWA_CLOAKED, &mut c as *mut _ as *mut _, 4) }.is_ok() && c != 0
}

/// The visible bounds of a window, without its invisible resize border.
pub fn bounds(h: HWND) -> RECT {
    let mut r = RECT::default();
    unsafe {
        if DwmGetWindowAttribute(h, DWMWA_EXTENDED_FRAME_BOUNDS, &mut r as *mut _ as *mut _, size_of::<RECT>() as u32).is_err() {
            let _ = GetWindowRect(h, &mut r);
        }
    }
    r
}

/// The topmost app window under a screen point, skipping Kiritori's own windows, hidden and
/// cloaked windows, the desktop and the taskbar.
pub fn window_at(x: i32, y: i32) -> Option<HWND> {
    struct S {
        x: i32,
        y: i32,
        found: Option<HWND>,
    }
    unsafe extern "system" fn cb(h: HWND, data: LPARAM) -> BOOL {
        unsafe {
            let s = &mut *(data.0 as *mut S);
            if !IsWindowVisible(h).as_bool() || IsIconic(h).as_bool() || cloaked(h) || system::window_pid(h) == std::process::id() {
                return TRUE;
            }
            let ex = GetWindowLongPtrW(h, GWL_EXSTYLE) as u32;
            if ex & WS_EX_TOOLWINDOW.0 != 0 {
                return TRUE;
            }
            let r = bounds(h);
            if s.x >= r.left && s.x < r.right && s.y >= r.top && s.y < r.bottom
                && let Some(f) = system::window_facts(h)
                && system::is_candidate(h, &f)
            {
                s.found = Some(h);
                return FALSE; // EnumWindows goes top to bottom: the first hit is in front
            }
        }
        TRUE
    }
    let mut s = S { x, y, found: None };
    unsafe {
        let _ = EnumWindows(Some(cb), LPARAM(&mut s as *mut _ as isize));
    }
    s.found
}

pub struct Picker {
    catchers: Vec<HWND>,
    bars: [HWND; 4],
    label: HWND,
    hint: HWND,
    hovered: Option<HWND>,
    /// The display the hint is on, so it only moves when the pointer changes display.
    hint_on: Option<RECT>,
}

const CATCHER: PCWSTR = w!("KiritoriPickCatcher");
const BAR: PCWSTR = w!("KiritoriPickBar");
const LABEL: PCWSTR = w!("KiritoriPickLabel");

impl Picker {
    pub fn start() -> Option<Self> {
        unsafe {
            register(CATCHER, Some(catcher_proc), CreateSolidBrush(COLORREF(0)));
            // Kiritori cyan (0x22D3EE) as a COLORREF (0x00BBGGRR).
            register(BAR, Some(bar_proc), CreateSolidBrush(COLORREF(0x00EED322)));
            register(LABEL, Some(label_proc), HBRUSH::default());
            let mut catchers = Vec::new();
            for m in monitors() {
                let h = popup(CATCHER, WS_EX_LAYERED, m)?;
                // Almost invisible, but still receives the mouse.
                SetLayeredWindowAttributes(h, COLORREF(0), 1, LWA_ALPHA).ok()?;
                let _ = ShowWindow(h, SW_SHOW);
                catchers.push(h);
            }
            let none = RECT::default();
            let mk = |c| popup(c, WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE, none);
            let bars = [mk(BAR)?, mk(BAR)?, mk(BAR)?, mk(BAR)?];
            for b in bars {
                SetLayeredWindowAttributes(b, COLORREF(0), 255, LWA_ALPHA).ok()?;
            }
            let label = mk(LABEL)?;
            let hint = mk(LABEL)?;
            for l in [label, hint] {
                SetLayeredWindowAttributes(l, COLORREF(0), 235, LWA_ALPHA).ok()?;
            }
            if let Some(c) = catchers.first() {
                let _ = SetForegroundWindow(*c);
                let _ = SetFocus(Some(*c));
            }
            Self::set_label(hint, "Click the app you want to cut  ·  Esc or right-click cancels");
            let mut p = Picker { catchers, bars, label, hint, hovered: None, hint_on: None };
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            p.hover(pt.x, pt.y);
            Some(p)
        }
    }

    fn set_label(h: HWND, text: &str) {
        LABELS.with(|l| {
            let mut l = l.borrow_mut();
            l.retain(|(k, _)| *k != h.0 as isize);
            l.push((h.0 as isize, text.to_string()));
        });
        unsafe {
            let _ = InvalidateRect(Some(h), None, true);
        }
    }

    /// Move the outline to the window under the pointer and name it.
    pub fn hover(&mut self, x: i32, y: i32) {
        let k = unsafe { GetDpiForWindow(self.catchers[0]) as i32 / 96 }.max(1);
        // The hint sits at the top of the display the pointer is on.
        if let Some(m) = monitors().into_iter().find(|m| x >= m.left && x < m.right && y >= m.top && y < m.bottom)
            && self.hint_on != Some(m)
        {
            self.hint_on = Some(m);
            let (w, h) = (520 * k, 40 * k);
            let cx = (m.left + m.right) / 2;
            unsafe {
                let _ = SetWindowPos(self.hint, Some(HWND_TOPMOST), cx - w / 2, m.top + 24 * k, w, h, SWP_NOACTIVATE | SWP_SHOWWINDOW);
            }
        }
        let target = window_at(x, y);
        if target == self.hovered {
            return;
        }
        self.hovered = target;
        unsafe {
            match target {
                Some(t) => {
                    let r = bounds(t);
                    let th = 4 * k;
                    let rects = [
                        (r.left - th, r.top - th, r.right - r.left + 2 * th, th),
                        (r.left - th, r.bottom, r.right - r.left + 2 * th, th),
                        (r.left - th, r.top, th, r.bottom - r.top),
                        (r.right, r.top, th, r.bottom - r.top),
                    ];
                    for (b, (bx, by, bw, bh)) in self.bars.iter().zip(rects) {
                        let _ = SetWindowPos(*b, Some(HWND_TOPMOST), bx, by, bw, bh, SWP_NOACTIVATE | SWP_SHOWWINDOW);
                    }
                    let facts = system::window_facts(t);
                    let name = facts.as_ref().map(|f| {
                        let title = f.title.trim();
                        if title.is_empty() { f.exe_name().to_string() } else { format!("{title}  ·  {}", f.exe_name()) }
                    });
                    Self::set_label(self.label, &name.unwrap_or_default());
                    let (lw, lh) = (480 * k, 36 * k);
                    let ly = if r.top - lh - 8 * k > 0 { r.top - lh - 8 * k } else { r.top + 8 * k };
                    let _ = SetWindowPos(self.label, Some(HWND_TOPMOST), r.left, ly, lw.min((r.right - r.left).max(240 * k)), lh, SWP_NOACTIVATE | SWP_SHOWWINDOW);
                }
                None => {
                    for b in self.bars {
                        let _ = ShowWindow(b, SW_HIDE);
                    }
                    let _ = ShowWindow(self.label, SW_HIDE);
                }
            }
        }
    }
}

impl Drop for Picker {
    fn drop(&mut self) {
        unsafe {
            for h in self.catchers.iter().chain(self.bars.iter()).chain([self.label, self.hint].iter()) {
                let _ = DestroyWindow(*h);
            }
        }
        LABELS.with(|l| l.borrow_mut().clear());
    }
}
