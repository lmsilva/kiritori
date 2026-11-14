//! Small facts read from Windows: what a window is, which display it is on, the taskbar's
//! theme, and the one setting that lives outside the Kiritori folder ("Start with Windows").

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Registry::*;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

use crate::model::layout::{DisplayMode, DisplayRef};
use crate::model::profile::WindowFacts;

pub fn process_path(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let r = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(h);
        r.ok()?;
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

pub fn window_pid(hwnd: HWND) -> u32 {
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    pid
}

pub fn window_facts(hwnd: HWND) -> Option<WindowFacts> {
    unsafe {
        let mut class = [0u16; 256];
        let n = GetClassNameW(hwnd, &mut class);
        let mut title = [0u16; 512];
        let t = GetWindowTextW(hwnd, &mut title);
        Some(WindowFacts {
            exe_path: process_path(window_pid(hwnd))?,
            window_class: String::from_utf16_lossy(&class[..n.max(0) as usize]),
            title: String::from_utf16_lossy(&title[..t.max(0) as usize]),
        })
    }
}

/// Windows that are never a source: our own, the desktop, the taskbar and system popups.
pub fn is_candidate(hwnd: HWND, facts: &WindowFacts) -> bool {
    const SKIP_CLASSES: [&str; 8] = [
        "Progman",
        "WorkerW",
        "Shell_TrayWnd",
        "Shell_SecondaryTrayWnd",
        "NotifyIconOverflowWindow",
        "TopLevelWindowForOverflowXamlIsland",
        "XamlExplorerHostIslandWindow",
        "#32768", // menus
    ];
    unsafe {
        if hwnd.is_invalid() || !IsWindowVisible(hwnd).as_bool() {
            return false;
        }
    }
    window_pid(hwnd) != std::process::id() && !SKIP_CLASSES.contains(&facts.window_class.as_str())
}

/// The display a window is mostly on, as a layout display entry. The hardware id is the
/// monitor's device interface path, which stays the same when Windows renumbers displays.
/// Every connected display, in Windows' order.
pub fn monitors() -> Vec<crate::model::display::Monitor> {
    unsafe extern "system" fn cb(h: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        unsafe {
            let out = &mut *(data.0 as *mut Vec<HMONITOR>);
            out.push(h);
        }
        TRUE
    }
    let mut handles: Vec<HMONITOR> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(cb), LPARAM(&mut handles as *mut _ as isize));
    }
    let names = friendly_names();
    let mut out: Vec<crate::model::display::Monitor> = handles.into_iter().map(monitor_info).collect();
    for m in &mut out {
        if let Some(n) = names.iter().find(|(path, _)| path.eq_ignore_ascii_case(&m.id)).map(|(_, n)| n.clone()).filter(|n| !n.is_empty()) {
            m.name = n;
        }
    }
    out.sort_by_key(|m| m.number);
    out
}

/// Monitors' own names from their EDID ("Acer XV272U"), by device path.
fn friendly_names() -> Vec<(String, String)> {
    use windows::Win32::Devices::Display::*;
    let mut out = Vec::new();
    unsafe {
        let (mut np, mut nm) = (0u32, 0u32);
        if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut np, &mut nm) != ERROR_SUCCESS {
            return out;
        }
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); np as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); nm as usize];
        if QueryDisplayConfig(QDC_ONLY_ACTIVE_PATHS, &mut np, paths.as_mut_ptr(), &mut nm, modes.as_mut_ptr(), None) != ERROR_SUCCESS {
            return out;
        }
        for p in &paths[..np as usize] {
            let mut t = DISPLAYCONFIG_TARGET_DEVICE_NAME::default();
            t.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME;
            t.header.size = size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32;
            t.header.adapterId = p.targetInfo.adapterId;
            t.header.id = p.targetInfo.id;
            if DisplayConfigGetDeviceInfo(&mut t.header) == 0 {
                let w = |a: &[u16]| String::from_utf16_lossy(&a[..a.iter().position(|&c| c == 0).unwrap_or(a.len())]);
                out.push((w(&t.monitorDevicePath), w(&t.monitorFriendlyDeviceName)));
            }
        }
    }
    out
}

fn monitor_info(mon: HMONITOR) -> crate::model::display::Monitor {
    use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
    unsafe {
        let mut mi = MONITORINFOEXW::default();
        mi.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        let _ = GetMonitorInfoW(mon, &mut mi as *mut _ as *mut MONITORINFO);
        let device = String::from_utf16_lossy(&mi.szDevice).trim_end_matches('\0').to_string();
        let mut dd = DISPLAY_DEVICEW { cb: size_of::<DISPLAY_DEVICEW>() as u32, ..Default::default() };
        let (id, name) = if EnumDisplayDevicesW(PCWSTR(mi.szDevice.as_ptr()), 0, &mut dd, EDD_GET_DEVICE_INTERFACE_NAME).as_bool() {
            (String::from_utf16_lossy(&dd.DeviceID).trim_end_matches('\0').to_string(), String::from_utf16_lossy(&dd.DeviceString).trim_end_matches('\0').to_string())
        } else {
            (device.clone(), String::new())
        };
        let number = device.trim_start_matches(r"\\.\DISPLAY").parse().unwrap_or(0);
        let mut dm = DEVMODEW { dmSize: size_of::<DEVMODEW>() as u16, ..Default::default() };
        let hz = if EnumDisplaySettingsW(PCWSTR(mi.szDevice.as_ptr()), ENUM_CURRENT_SETTINGS, &mut dm).as_bool() { dm.dmDisplayFrequency } else { 60 };
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        let r = mi.monitorInfo.rcMonitor;
        let generic = name.is_empty() || name.starts_with("Generic");
        crate::model::display::Monitor {
            id,
            number,
            name: if generic { format!("Display {number}") } else { name },
            rect: (r.left, r.top, r.right - r.left, r.bottom - r.top),
            scale: (dx as f32 / 96.0).max(1.0),
            hz,
        }
    }
}

pub fn display_for(hwnd: HWND) -> DisplayRef {
    unsafe {
        let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTOPRIMARY);
        let mut mi = MONITORINFOEXW::default();
        mi.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        let _ = GetMonitorInfoW(mon, &mut mi as *mut _ as *mut MONITORINFO);
        let mut dd = DISPLAY_DEVICEW { cb: size_of::<DISPLAY_DEVICEW>() as u32, ..Default::default() };
        let adapter = PCWSTR(mi.szDevice.as_ptr());
        let id = if EnumDisplayDevicesW(adapter, 0, &mut dd, EDD_GET_DEVICE_INTERFACE_NAME).as_bool() {
            String::from_utf16_lossy(&dd.DeviceID).trim_end_matches('\0').to_string()
        } else {
            String::from_utf16_lossy(&mi.szDevice).trim_end_matches('\0').to_string()
        };
        let number = String::from_utf16_lossy(&mi.szDevice).trim_end_matches('\0').trim_start_matches(r"\\.\DISPLAY").to_string();
        DisplayRef { hardware_id: id, label: format!("Display {number}"), mode: DisplayMode::Takeover }
    }
}

fn read_dword(key: HKEY, sub: PCWSTR, name: PCWSTR) -> Option<u32> {
    let mut v = 0u32;
    let mut len = 4u32;
    unsafe { RegGetValueW(key, sub, name, RRF_RT_REG_DWORD, None, Some(&mut v as *mut _ as *mut _), Some(&mut len)) }.ok().ok()?;
    Some(v)
}

/// The taskbar's theme, which can differ from the app theme.
pub fn taskbar_is_light() -> bool {
    read_dword(HKEY_CURRENT_USER, w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"), w!("SystemUsesLightTheme")) == Some(1)
}

/// The Windows app theme (Settings > Personalisation > Colours > app mode).
pub fn apps_use_light_theme() -> bool {
    read_dword(HKEY_CURRENT_USER, w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"), w!("AppsUseLightTheme")) != Some(0)
}

const RUN_KEY: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Run");

fn startup_command() -> String {
    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
    format!("\"{exe}\"")
}

/// The Save dialog for exporting a profile; the chosen path, or None if cancelled.
pub fn save_file_dialog(owner: HWND, suggested: &str) -> Option<String> {
    use windows::Win32::UI::Controls::Dialogs::*;
    let mut buf = [0u16; 1024];
    let s: Vec<u16> = suggested.encode_utf16().take(1000).collect();
    buf[..s.len()].copy_from_slice(&s);
    let mut ofn = OPENFILENAMEW {
        lStructSize: size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: owner,
        lpstrFilter: w!("Kiritori profile (*.json)\0*.json\0All files\0*.*\0\0"),
        lpstrFile: PWSTR(buf.as_mut_ptr()),
        nMaxFile: buf.len() as u32,
        lpstrTitle: w!("Export profile"),
        lpstrDefExt: w!("json"),
        Flags: OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
        ..Default::default()
    };
    unsafe { GetSaveFileNameW(&mut ofn) }.as_bool().then(|| String::from_utf16_lossy(&buf[..buf.iter().position(|&c| c == 0).unwrap_or(0)]))
}

/// The Open dialog for importing a profile.
pub fn open_file_dialog(owner: HWND) -> Option<String> {
    use windows::Win32::UI::Controls::Dialogs::*;
    let mut buf = [0u16; 1024];
    let mut ofn = OPENFILENAMEW {
        lStructSize: size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: owner,
        lpstrFilter: w!("Kiritori profile (*.json)\0*.json\0All files\0*.*\0\0"),
        lpstrFile: PWSTR(buf.as_mut_ptr()),
        nMaxFile: buf.len() as u32,
        lpstrTitle: w!("Import profile"),
        Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
        ..Default::default()
    };
    unsafe { GetOpenFileNameW(&mut ofn) }.as_bool().then(|| String::from_utf16_lossy(&buf[..buf.iter().position(|&c| c == 0).unwrap_or(0)]))
}

/// The character a shortcut types on the current keyboard layout, if any: Ctrl+Alt is AltGr
/// on many keyboards, so Ctrl+Alt+E types "€" on a German one.
pub fn types_character(c: crate::model::shortcut::Combo) -> Option<char> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyboardLayout, MapVirtualKeyExW, MAPVK_VK_TO_VSC_EX, ToUnicodeEx};
    if !(c.ctrl && c.alt) {
        return None;
    }
    unsafe {
        let hkl = GetKeyboardLayout(0);
        let mut state = [0u8; 256];
        state[0x11] = 0x80; // Ctrl
        state[0x12] = 0x80; // Alt
        if c.shift {
            state[0x10] = 0x80;
        }
        let sc = MapVirtualKeyExW(c.vk, MAPVK_VK_TO_VSC_EX, Some(hkl));
        let mut buf = [0u16; 8];
        // Flag 4: do not change the keyboard state (no dead-key side effects).
        let n = ToUnicodeEx(c.vk, sc, &state, &mut buf, 4, Some(hkl));
        if n >= 1 {
            char::from_u32(buf[0] as u32).filter(|ch| !ch.is_control())
        } else {
            None
        }
    }
}

/// Open app windows (not Kiritori's), front to back, with their facts: what New profile offers.
pub fn app_windows() -> Vec<(isize, WindowFacts)> {
    unsafe extern "system" fn cb(h: HWND, data: LPARAM) -> BOOL {
        unsafe {
            let out = &mut *(data.0 as *mut Vec<(isize, WindowFacts)>);
            if IsWindowVisible(h).as_bool() && window_pid(h) != std::process::id() && let Some(f) = window_facts(h) && is_candidate(h, &f) {
                out.push((h.0 as isize, f));
            }
        }
        TRUE
    }
    let mut out: Vec<(isize, WindowFacts)> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(cb), LPARAM(&mut out as *mut _ as isize));
    }
    out
}

/// Open windows a match rule matches now, front to back.
pub fn matching_windows(rule: &crate::model::profile::MatchRule) -> Vec<isize> {
    struct S<'a> {
        rule: &'a crate::model::profile::MatchRule,
        out: Vec<isize>,
    }
    unsafe extern "system" fn cb(h: HWND, data: LPARAM) -> BOOL {
        unsafe {
            let s = &mut *(data.0 as *mut S);
            if IsWindowVisible(h).as_bool() && window_pid(h) != std::process::id() && let Some(f) = window_facts(h) && is_candidate(h, &f) && s.rule.matches(&f) {
                s.out.push(h.0 as isize);
            }
        }
        TRUE
    }
    let mut s = S { rule, out: Vec::new() };
    unsafe {
        let _ = EnumWindows(Some(cb), LPARAM(&mut s as *mut _ as isize));
    }
    s.out
}

/// Titles of the open windows a match rule matches now, for the Profiles panel.
pub fn matching_titles(rule: &crate::model::profile::MatchRule) -> Vec<String> {
    struct S<'a> {
        rule: &'a crate::model::profile::MatchRule,
        out: Vec<String>,
    }
    unsafe extern "system" fn cb(h: HWND, data: LPARAM) -> BOOL {
        unsafe {
            let s = &mut *(data.0 as *mut S);
            if IsWindowVisible(h).as_bool()
                && window_pid(h) != std::process::id()
                && let Some(f) = window_facts(h)
                && is_candidate(h, &f)
                && s.rule.matches(&f)
            {
                let t = f.title.trim();
                s.out.push(if t.is_empty() { f.exe_name().to_string() } else { t.to_string() });
            }
        }
        TRUE
    }
    let mut s = S { rule, out: Vec::new() };
    unsafe {
        let _ = EnumWindows(Some(cb), LPARAM(&mut s as *mut _ as isize));
    }
    s.out
}

/// Start an administrator copy of Kiritori that takes over from this one (`--replace`).
/// Returns false if Windows did not start it (for example the prompt was declined).
pub fn restart_as_admin() -> bool {
    use windows::Win32::UI::Shell::ShellExecuteW;
    let Ok(exe) = std::env::current_exe() else { return false };
    let exe: Vec<u16> = exe.as_os_str().to_string_lossy().encode_utf16().chain(Some(0)).collect();
    let r = unsafe { ShellExecuteW(None, w!("runas"), PCWSTR(exe.as_ptr()), w!("--replace"), None, SW_SHOWNORMAL) };
    r.0 as isize > 32
}

/// Whether Kiritori itself runs as administrator.
pub fn is_elevated() -> bool {
    use windows::Win32::Security::*;
    unsafe {
        let mut tok = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut tok).is_err() {
            return false;
        }
        let mut e = TOKEN_ELEVATION::default();
        let mut len = 0;
        let r = GetTokenInformation(tok, TokenElevation, Some(&mut e as *mut _ as *mut _), size_of::<TOKEN_ELEVATION>() as u32, &mut len);
        let _ = CloseHandle(tok);
        r.is_ok() && e.TokenIsElevated != 0
    }
}

/// Whether the Run entry exists and points at this copy of Kiritori.
pub fn starts_with_windows() -> bool {
    let mut buf = [0u16; 1024];
    let mut len = (buf.len() * 2) as u32;
    let ok = unsafe { RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, w!("Kiritori"), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as *mut _), Some(&mut len)) };
    ok.is_ok() && String::from_utf16_lossy(&buf[..(len as usize / 2).saturating_sub(1)]) == startup_command()
}

/// The one write outside the Kiritori folder; removed again when turned off.
pub fn set_start_with_windows(on: bool) -> Result<()> {
    unsafe {
        let mut key = HKEY::default();
        RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, None, KEY_SET_VALUE, &mut key).ok()?;
        let r = if on {
            let v: Vec<u16> = startup_command().encode_utf16().chain(Some(0)).collect();
            let bytes = std::slice::from_raw_parts(v.as_ptr() as *const u8, v.len() * 2);
            RegSetValueExW(key, w!("Kiritori"), None, REG_SZ, Some(bytes)).ok()
        } else {
            match RegDeleteValueW(key, w!("Kiritori")) {
                e if e == ERROR_FILE_NOT_FOUND => Ok(()),
                e => e.ok(),
            }
        };
        let _ = RegCloseKey(key);
        r
    }
}

/// A window's frame as it was before Kiritori removed it, so it can be put back.
#[derive(Clone, Copy, Debug)]
pub struct SavedFrame {
    style: isize,
    ex_style: isize,
    rect: RECT,
}

impl SavedFrame {
    pub fn record(&self, hwnd: isize) -> crate::store::FrameRecord {
        crate::store::FrameRecord {
            hwnd: hwnd as i64,
            style: self.style as i64,
            ex_style: self.ex_style as i64,
            rect: [self.rect.left, self.rect.top, self.rect.right, self.rect.bottom],
        }
    }

    pub fn from_record(r: &crate::store::FrameRecord) -> (isize, Self) {
        (r.hwnd as isize, SavedFrame { style: r.style as isize, ex_style: r.ex_style as isize, rect: RECT { left: r.rect[0], top: r.rect[1], right: r.rect[2], bottom: r.rect[3] } })
    }
}

/// Whether Windows reports a title bar or sizing frame on the window.
pub fn has_frame(hwnd: HWND) -> bool {
    let style = unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) } as u32;
    style & (WS_CAPTION.0 | WS_THICKFRAME.0) != 0
}

/// Remove the title bar and frame, keeping the drawing area exactly where it was on screen
///. Returns what is needed to restore it, or None if Windows refused (for example,
/// the app runs as administrator and Kiritori does not).
pub fn remove_frame(hwnd: HWND) -> Option<SavedFrame> {
    unsafe {
        if !has_frame(hwnd) || IsZoomed(hwnd).as_bool() {
            return None;
        }
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
        let ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let mut rect = RECT::default();
        GetWindowRect(hwnd, &mut rect).ok()?;
        let mut client = RECT::default();
        GetClientRect(hwnd, &mut client).ok()?;
        let mut tl = POINT { x: 0, y: 0 };
        let _ = ClientToScreen(hwnd, &mut tl);

        let strip = (WS_CAPTION.0 | WS_THICKFRAME.0 | WS_SYSMENU.0 | WS_MINIMIZEBOX.0 | WS_MAXIMIZEBOX.0) as isize;
        let strip_ex = (WS_EX_DLGMODALFRAME.0 | WS_EX_CLIENTEDGE.0 | WS_EX_STATICEDGE.0 | WS_EX_WINDOWEDGE.0) as isize;
        SetLastError(WIN32_ERROR(0));
        if SetWindowLongPtrW(hwnd, GWL_STYLE, style & !strip) == 0 && GetLastError() != WIN32_ERROR(0) {
            return None;
        }
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex_style & !strip_ex);
        let _ = SetWindowPos(hwnd, None, tl.x, tl.y, client.right, client.bottom, SWP_FRAMECHANGED | SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOOWNERZORDER);
        Some(SavedFrame { style, ex_style, rect })
    }
}

pub fn restore_frame(hwnd: HWND, saved: SavedFrame) {
    unsafe {
        if !IsWindow(Some(hwnd)).as_bool() {
            return;
        }
        SetWindowLongPtrW(hwnd, GWL_STYLE, saved.style);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, saved.ex_style);
        let r = saved.rect;
        let _ = SetWindowPos(hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_FRAMECHANGED | SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOOWNERZORDER);
    }
}

const CF_UNICODETEXT: u32 = 13;

/// The clipboard's text, if it holds any. Text boxes paste it.
pub fn clipboard_text() -> Option<String> {
    use windows::Win32::System::DataExchange::*;
    use windows::Win32::System::Memory::*;
    unsafe {
        OpenClipboard(None).ok()?;
        let text = (|| {
            let h = GetClipboardData(CF_UNICODETEXT).ok()?;
            let g = HGLOBAL(h.0);
            let p = GlobalLock(g) as *const u16;
            if p.is_null() {
                return None;
            }
            let mut n = 0;
            while *p.add(n) != 0 {
                n += 1;
            }
            let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, n));
            let _ = GlobalUnlock(g);
            Some(s)
        })();
        let _ = CloseClipboard();
        text
    }
}

/// Put `text` on the clipboard (copy and cut in text boxes).
pub fn set_clipboard_text(text: &str) {
    use windows::Win32::System::DataExchange::*;
    use windows::Win32::System::Memory::*;
    let w: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    unsafe {
        if OpenClipboard(None).is_err() {
            return;
        }
        let _ = EmptyClipboard();
        if let Ok(g) = GlobalAlloc(GMEM_MOVEABLE, w.len() * 2) {
            let p = GlobalLock(g) as *mut u16;
            if !p.is_null() {
                std::ptr::copy_nonoverlapping(w.as_ptr(), p, w.len());
                let _ = GlobalUnlock(g);
                // The clipboard owns the memory once this succeeds.
                if SetClipboardData(CF_UNICODETEXT, Some(HANDLE(g.0))).is_err() {
                    let _ = GlobalFree(Some(g));
                }
            }
        }
        let _ = CloseClipboard();
    }
}
