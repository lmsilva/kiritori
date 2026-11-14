//! Self-test on launch: a portable tool gets copied between very
//! different PCs, so Kiritori checks what works on this one and says so. Written to
//! `logs\self-test.txt` and shown by "About Kiritori" in the tray.

use windows::Win32::System::Registry::*;
use windows::core::*;

use crate::engine::draw::EngineCaps;

pub struct Line {
    pub label: &'static str,
    pub value: String,
    pub ok: bool,
}

fn reg_string(name: PCWSTR) -> Option<String> {
    let mut buf = [0u16; 256];
    let mut len = (buf.len() * 2) as u32;
    unsafe {
        RegGetValueW(HKEY_LOCAL_MACHINE, w!(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion"), name, RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as *mut _), Some(&mut len)).ok().ok()?;
    }
    Some(String::from_utf16_lossy(&buf[..(len as usize / 2).saturating_sub(1)]))
}

/// Windows' build number (0 if it cannot be read).
pub fn windows_build() -> u32 {
    reg_string(w!("CurrentBuildNumber")).and_then(|b| b.parse().ok()).unwrap_or(0)
}

fn windows_version() -> (String, bool) {
    let build = windows_build();
    let display = reg_string(w!("DisplayVersion")).unwrap_or_default();
    // Windows 11 is build 22000 and later; earlier builds cannot hide the capture border.
    let name = if build >= 22000 { "Windows 11" } else { "Windows 10 or earlier" };
    (format!("{name} {display} (build {build})"), build >= 22000)
}

pub fn run(folder: &str, writable: bool, caps: Option<&EngineCaps>, failed_shortcuts: &[String], elevated: bool) -> Vec<Line> {
    let (win, win_ok) = windows_version();
    let capture = windows::Graphics::Capture::GraphicsCaptureSession::IsSupported().unwrap_or(false);
    let mut lines = vec![
        Line { label: "Kiritori", value: env!("CARGO_PKG_VERSION").into(), ok: true },
        Line { label: "Folder", value: format!("{folder} ({})", if writable { "saving" } else { "read-only: not saving" }), ok: writable },
        Line { label: "Windows", value: win, ok: win_ok },
        Line { label: "Window capture", value: if capture { "available".into() } else { "not available".into() }, ok: capture },
    ];
    match caps {
        Some(c) => {
            lines.push(Line { label: "Graphics card", value: format!("{} (Direct3D {})", c.adapter, c.feature_level), ok: true });
            lines.push(Line {
                label: "Enhanced scaling",
                value: if c.enhanced { "FSR 1 ready".into() } else { "unavailable: Smooth is used instead".into() },
                ok: c.enhanced,
            });
        }
        None => lines.push(Line { label: "Graphics card", value: "could not start Direct3D 11".into(), ok: false }),
    }
    lines.push(Line {
        label: "Shortcuts",
        value: if failed_shortcuts.is_empty() { "all registered".into() } else { format!("taken by another program: {}", failed_shortcuts.join(", ")) },
        ok: failed_shortcuts.is_empty(),
    });
    lines.push(Line { label: "Running as administrator", value: if elevated { "yes".into() } else { "no".into() }, ok: true });
    lines
}

pub fn text(lines: &[Line]) -> String {
    lines.iter().map(|l| format!("{} {}: {}", if l.ok { "✓" } else { "✗" }, l.label, l.value)).collect::<Vec<_>>().join("\n")
}
