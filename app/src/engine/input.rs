//! Mouse translation.
//!
//! A low-level mouse hook keeps Kiritori's own "virtual" pointer. While it is over a cutout,
//! the real cursor is parked on the matching point of the app window and hidden, and the
//! engine draws its own cursor. Clicks pass untouched, so the app receives them natively.
//! Clicks on the black part of a Takeover canvas are swallowed, because the hidden app window
//! underneath would otherwise receive them at an unrelated spot.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Foundation::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Magnification::*;
use windows::Win32::UI::WindowsAndMessaging::*;

#[derive(Clone, Debug)]
pub struct Map {
    /// Screen rect of the cutout as drawn: x, y, w, h.
    pub dst: (f64, f64, f64, f64),
    /// Screen rect of the same region on the app window.
    pub src: (f64, f64, f64, f64),
    /// The cutout's bounding box and shape, in source fractions: points in a hole of a shaped
    /// cutout are empty area.
    pub bounds: crate::model::geometry::Rect,
    pub shape: Vec<crate::model::layout::ShapeRect>,
    /// Rotation and mirroring of the placed cutout (`Cutout::orient`).
    pub orient: u8,
}

impl Map {
    fn hit(&self, v: (f64, f64)) -> bool {
        let (dx, dy, dw, dh) = self.dst;
        if !(v.0 >= dx && v.0 < dx + dw && v.1 >= dy && v.1 < dy + dh) {
            return false;
        }
        if self.shape.len() <= 1 {
            return true;
        }
        let (qx, qy) = crate::model::layout::unorient(self.orient, (v.0 - dx) / dw, (v.1 - dy) / dh);
        let fx = self.bounds.x + qx * self.bounds.w;
        let fy = self.bounds.y + qy * self.bounds.h;
        self.shape.iter().fold(false, |inside, r| match r.op {
            crate::model::layout::ShapeOp::Add => inside || r.rect.contains(fx, fy),
            crate::model::layout::ShapeOp::Subtract => inside && !r.rect.contains(fx, fy),
        })
    }

    fn to_source(&self, v: (f64, f64)) -> POINT {
        let (dx, dy, dw, dh) = self.dst;
        let (sx, sy, sw, sh) = self.src;
        let (fx, fy) = crate::model::layout::unorient(self.orient, ((v.0 - dx) / dw).clamp(0.0, 0.9999), ((v.1 - dy) / dh).clamp(0.0, 0.9999));
        let (fx, fy) = (fx.clamp(0.0, 0.9999), fy.clamp(0.0, 0.9999));
        POINT { x: (sx + fx * sw).floor() as i32, y: (sy + fy * sh).floor() as i32 }
    }
}

/// Marks input Kiritori sends itself (touch taps re-delivered at the mapped point), so the
/// hook lets it through untouched.
const OWN_INPUT: usize = 0x4B49_5254; // "KIRT": a tap Kiritori re-delivered

#[derive(Default)]
struct State {
    maps: Vec<Map>,
    /// Screen rects of Takeover canvases: clicks there outside any cutout are swallowed.
    takeover: Vec<RECT>,
    virt: (f64, f64),
    parked: Option<POINT>,
    /// Pointer Off: swallow every click on a Takeover display.
    swallow_all: bool,
    /// A drag that started on a cutout stays bound to it until the buttons are up.
    bound: Option<Map>,
    buttons_down: u32,
    /// A press Kiritori swallowed and delivered again itself: its release must follow it.
    redelivered: bool,
    desk: RECT,
    over_cutout: bool,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut g = STATE.lock().unwrap();
    f(g.get_or_insert_with(State::default))
}

/// The topmost cutout under a screen point, and the matching point on the app window.
fn translate(maps: &[Map], v: (f64, f64)) -> (POINT, Option<usize>) {
    match maps.iter().rposition(|m| m.hit(v)) {
        Some(i) => (maps[i].to_source(v), Some(i)),
        None => (POINT { x: v.0.round() as i32, y: v.1.round() as i32 }, None),
    }
}

fn is_down(msg: u32) -> bool {
    matches!(msg, WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN)
}

fn is_up(msg: u32) -> bool {
    matches!(msg, WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP | WM_XBUTTONUP)
}

/// `KIRITORI_DEBUG_INPUT=1` logs every button event the hook sees.
fn debug() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("KIRITORI_DEBUG_INPUT").is_ok_and(|v| v == "1"))
}

/// Send a button event at a screen point, marked as Kiritori's own.
fn send_button(msg: u32, at: POINT, data: u32) {
    use windows::Win32::UI::Input::KeyboardAndMouse::*;
    let flag = match msg {
        WM_LBUTTONDOWN => MOUSEEVENTF_LEFTDOWN,
        WM_LBUTTONUP => MOUSEEVENTF_LEFTUP,
        WM_RBUTTONDOWN => MOUSEEVENTF_RIGHTDOWN,
        WM_RBUTTONUP => MOUSEEVENTF_RIGHTUP,
        WM_MBUTTONDOWN => MOUSEEVENTF_MIDDLEDOWN,
        WM_MBUTTONUP => MOUSEEVENTF_MIDDLEUP,
        WM_XBUTTONDOWN => MOUSEEVENTF_XDOWN,
        WM_XBUTTONUP => MOUSEEVENTF_XUP,
        _ => return,
    };
    unsafe {
        let (vx, vy) = (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN));
        let (vw, vh) = (GetSystemMetrics(SM_CXVIRTUALSCREEN).max(1), GetSystemMetrics(SM_CYVIRTUALSCREEN).max(1));
        let nx = ((at.x - vx) as i64 * 65535 / (vw - 1).max(1) as i64) as i32;
        let ny = ((at.y - vy) as i64 * 65535 / (vh - 1).max(1) as i64) as i32;
        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT { dx: nx, dy: ny, mouseData: data >> 16, dwFlags: flag | MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK, time: 0, dwExtraInfo: OWN_INPUT },
            },
        };
        SendInput(&[input], size_of::<INPUT>() as i32);
    }
}

/// The engine's wake event: Kiritori draws its own pointer into the picture, so a move over a
/// cutout needs a redraw straight away.
static WAKE: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

pub fn set_wake(h: HANDLE) {
    WAKE.store(h.0 as isize, Ordering::Release);
}

fn wake_engine() {
    let h = WAKE.load(Ordering::Acquire);
    if h != 0 {
        unsafe {
            let _ = windows::Win32::System::Threading::SetEvent(HANDLE(h as *mut _));
        }
    }
}

/// The Magnification API is ready (it hides the real cursor over cutouts).
static MAG_READY: AtomicBool = AtomicBool::new(false);
static CURSOR_HIDDEN: AtomicBool = AtomicBool::new(false);

/// Hide or show the real cursor. Never call it with `STATE` locked: if Windows is slow to
/// answer (a screen-snipping overlay, for one), the lock would be held and everything that
/// needs it (the hook, the engine, quitting) would wait too.
fn show_cursor(hide: bool) {
    if MAG_READY.load(Ordering::Acquire) && CURSOR_HIDDEN.swap(hide, Ordering::AcqRel) != hide {
        unsafe {
            let _ = MagShowSystemCursor(!hide);
        }
    }
}

fn in_takeover(st: &State, v: (f64, f64)) -> bool {
    st.takeover.iter().any(|r| v.0 >= r.left as f64 && v.0 < r.right as f64 && v.1 >= r.top as f64 && v.1 < r.bottom as f64)
}

unsafe extern "system" fn hook(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if code >= 0 {
            let info = &*(lp.0 as *const MSLLHOOKSTRUCT);
            let msg = wp.0 as u32;
            if debug() && msg != WM_MOUSEMOVE {
                crate::log!("input: msg {msg:#x} at {},{} flags {:#x} extra {:#x}", info.pt.x, info.pt.y, info.flags, info.dwExtraInfo);
            }
            let mut guard = STATE.lock().unwrap();
            if info.dwExtraInfo == OWN_INPUT {
                // A tap Kiritori re-delivered itself: let it through.
            } else if let Some(st) = guard.as_mut()
                && st.swallow_all
                && msg != WM_MOUSEMOVE
                && in_takeover(st, (info.pt.x as f64, info.pt.y as f64))
            {
                // Pointer Off: the cutouts are display-only.
                return LRESULT(1);
            } else if let Some(st) = guard.as_mut() && !st.maps.is_empty() {
                // Touch and pen arrive as mouse input marked with this signature.
                let from_touch_or_pen = info.dwExtraInfo & 0xFFFF_FF00 == 0xFF51_5700;
                if msg == WM_MOUSEMOVE {
                    let pt = info.pt;
                    // A physical mouse moves by an amount, so add it to the virtual pointer.
                    // Touch, pen and software input (remote desktop, automation, test tools)
                    // say "be here", so take the position as given.
                    let injected = info.flags & LLMHF_INJECTED != 0;
                    match st.parked {
                        Some(p) if !from_touch_or_pen && !injected => {
                            st.virt.0 += (pt.x - p.x) as f64;
                            st.virt.1 += (pt.y - p.y) as f64;
                        }
                        _ => st.virt = (pt.x as f64, pt.y as f64),
                    }
                    let d = st.desk;
                    st.virt.0 = st.virt.0.clamp(d.left as f64, (d.right - 1) as f64);
                    st.virt.1 = st.virt.1.clamp(d.top as f64, (d.bottom - 1) as f64);
                    // A drag that began on a cutout stays on it, clamped to its edge.
                    let (target, inside) = match &st.bound {
                        Some(m) => {
                            let (dx, dy, dw, dh) = m.dst;
                            st.virt = (st.virt.0.clamp(dx, dx + dw - 1.0), st.virt.1.clamp(dy, dy + dh - 1.0));
                            (m.to_source(st.virt), true)
                        }
                        None => {
                            let (t, i) = translate(&st.maps, st.virt);
                            (t, i.is_some())
                        }
                    };
                    let was_inside = st.over_cutout;
                    st.over_cutout = inside;
                    st.parked = Some(target);
                    let moved = target.x != pt.x || target.y != pt.y;
                    drop(guard);
                    show_cursor(inside);
                    if inside || was_inside {
                        wake_engine();
                    }
                    if moved {
                        let _ = SetCursorPos(target.x, target.y);
                        return LRESULT(1);
                    }
                    return CallNextHookEx(None, code, wp, lp);
                } else if is_up(msg) && st.redelivered {
                    // The release of a press Kiritori delivered itself goes where the cursor is
                    // parked now (the end of the drag), never to what is under the finger.
                    st.redelivered = false;
                    st.bound = None;
                    let at = st.parked.unwrap_or(info.pt);
                    let data = info.mouseData;
                    drop(guard);
                    send_button(msg, at, data);
                    return LRESULT(1);
                } else if (is_down(msg) || is_up(msg)) && st.parked.is_some_and(|p| p.x != info.pt.x || p.y != info.pt.y) {
                    // Touch and pen move the cursor first, and that move is mapped like any
                    // other, so their press arrives at the parked point and passes natively.
                    // A press somewhere else came from software that says "here" without
                    // moving first: deliver it at the mapped point on the app instead.
                    let v = (info.pt.x as f64, info.pt.y as f64);
                    let (target, hit) = translate(&st.maps, v);
                    if hit.is_some() || st.bound.is_some() {
                        let target = match (&st.bound, hit) {
                            (Some(m), None) => m.to_source(v),
                            _ => target,
                        };
                        if is_down(msg) {
                            st.bound = hit.map(|i| st.maps[i].clone());
                            st.redelivered = true;
                        } else {
                            st.bound = None;
                        }
                        // The cursor now rests on the target: later moves and the release are
                        // measured from there.
                        st.parked = Some(target);
                        let data = info.mouseData;
                        drop(guard);
                        let _ = SetCursorPos(target.x, target.y);
                        send_button(msg, target, data);
                        return LRESULT(1);
                    }
                    if in_takeover(st, v) {
                        return LRESULT(1);
                    }
                } else if !st.over_cutout && st.bound.is_none() && in_takeover(st, st.virt) {
                    // A click or wheel on black canvas: nothing is there, and the app underneath
                    // must not receive it.
                    return LRESULT(1);
                } else if is_down(msg) {
                    st.buttons_down += 1;
                    if st.bound.is_none() {
                        st.bound = translate(&st.maps, st.virt).1.map(|i| st.maps[i].clone());
                    }
                } else if is_up(msg) {
                    st.buttons_down = st.buttons_down.saturating_sub(1);
                    if st.buttons_down == 0 {
                        st.bound = None;
                    }
                }
            }
        }
        CallNextHookEx(None, code, wp, lp)
    }
}

/// Start the hook on its own thread, so a busy render loop never delays the pointer.
pub fn start() {
    with(|st| {
        MAG_READY.store(unsafe { MagInitialize() }.as_bool(), Ordering::Release);
        st.desk = unsafe {
            RECT {
                left: GetSystemMetrics(SM_XVIRTUALSCREEN),
                top: GetSystemMetrics(SM_YVIRTUALSCREEN),
                right: GetSystemMetrics(SM_XVIRTUALSCREEN) + GetSystemMetrics(SM_CXVIRTUALSCREEN),
                bottom: GetSystemMetrics(SM_YVIRTUALSCREEN) + GetSystemMetrics(SM_CYVIRTUALSCREEN),
            }
        };
    });
    std::thread::Builder::new()
        .name("kiritori-mouse".into())
        .spawn(|| unsafe {
            let _ = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            let Ok(h) = SetWindowsHookExW(WH_MOUSE_LL, Some(hook), GetModuleHandleW(None).ok().map(|m| m.into()), 0) else {
                crate::log!("mouse hook could not be installed: mouse translation is off");
                return;
            };
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                DispatchMessageW(&msg);
            }
            let _ = UnhookWindowsHookEx(h);
        })
        .ok();
}

/// Replace the cutout map. An empty map turns translation off and shows the real cursor.
/// `swallow_all`: Pointer Off, every click on a Takeover display is swallowed.
pub fn set_maps(maps: Vec<Map>, takeover: Vec<RECT>, swallow_all: bool) {
    let off = maps.is_empty();
    with(|st| {
        if off {
            st.parked = None;
            st.over_cutout = false;
            st.bound = None;
            st.buttons_down = 0;
            st.redelivered = false;
        }
        st.maps = maps;
        st.takeover = takeover;
        st.swallow_all = swallow_all;
    });
    if off {
        show_cursor(false);
    }
}

/// Where to draw Kiritori's own cursor, if the pointer is over a cutout.
pub fn own_cursor() -> Option<(i32, i32)> {
    with(|st| st.over_cutout.then_some((st.virt.0 as i32, st.virt.1 as i32)))
}

/// Show the real cursor again (quit, emergency off).
pub fn release() {
    // Quitting must never wait on the hook: take the lock only if it comes free soon.
    for _ in 0..50 {
        if let Ok(mut g) = STATE.try_lock() {
            if let Some(st) = g.as_mut() {
                st.maps.clear();
                st.takeover.clear();
                st.swallow_all = false;
                st.parked = None;
                st.bound = None;
            }
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    if MAG_READY.swap(false, Ordering::AcqRel) {
        CURSOR_HIDDEN.store(false, Ordering::Release);
        unsafe {
            let _ = MagShowSystemCursor(true);
            let _ = MagUninitialize();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::geometry::Rect;
    use crate::model::layout::{ShapeOp, ShapeRect};

    fn map(dst: (f64, f64, f64, f64), shape: Vec<ShapeRect>) -> Map {
        Map { dst, src: (1000.0, 0.0, 500.0, 500.0), bounds: Rect::new(0.5, 0.0, 0.5, 0.5), shape, orient: 0 }
    }

    #[test]
    fn points_map_back_and_holes_are_empty() {
        let plain = map((0.0, 0.0, 1000.0, 1000.0), vec![ShapeRect { op: ShapeOp::Add, rect: Rect::new(0.5, 0.0, 0.5, 0.5) }]);
        let (p, hit) = translate(std::slice::from_ref(&plain), (500.0, 250.0));
        assert_eq!((p.x, p.y, hit), (1250, 125, Some(0)));
        assert_eq!(translate(std::slice::from_ref(&plain), (1500.0, 10.0)).1, None);
        // A ring: the middle quarter of the cutout is cut away.
        let ring = map(
            (0.0, 0.0, 1000.0, 1000.0),
            vec![ShapeRect { op: ShapeOp::Add, rect: Rect::new(0.5, 0.0, 0.5, 0.5) }, ShapeRect { op: ShapeOp::Subtract, rect: Rect::new(0.625, 0.125, 0.25, 0.25) }],
        );
        assert!(ring.hit((100.0, 100.0)));
        assert!(!ring.hit((500.0, 500.0)), "the hole is empty area");
        // The topmost of two overlapping cutouts wins.
        let top = map((400.0, 400.0, 200.0, 200.0), vec![ShapeRect { op: ShapeOp::Add, rect: Rect::new(0.5, 0.0, 0.5, 0.5) }]);
        assert_eq!(translate(&[plain.clone(), top], (500.0, 500.0)).1, Some(1));
        // Turned a quarter clockwise: the top-right corner of the placement is the top-left of
        // the source region.
        let turned = Map { orient: 1, ..plain.clone() };
        let p = turned.to_source((999.0, 0.0));
        assert_eq!((p.x, p.y), (1000, 0));
        // Mirrored: left and right swap.
        let mirrored = Map { orient: 4, ..plain.clone() };
        assert_eq!(mirrored.to_source((0.0, 0.0)).x, 1499);
        // Clamped at the edge for a drag bound to a cutout.
        let p = plain.to_source((5000.0, -50.0));
        assert!(p.x <= 1499 && p.y >= 0);
    }
}
