//! A plain-text log in the Kiritori folder's `logs\`. Kept small: when it passes 1 MB it is
//! renamed to `kiritori.old.log` and a new one is started.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

static LOG: Mutex<Option<(PathBuf, File)>> = Mutex::new(None);
const MAX: u64 = 1024 * 1024;

pub fn init(dir: &Path) {
    let path = dir.join("kiritori.log");
    if fs::metadata(&path).map(|m| m.len() > MAX).unwrap_or(false) {
        let _ = fs::rename(&path, dir.join("kiritori.old.log"));
    }
    if let Ok(f) = OpenOptions::new().create(true).append(true).open(&path) {
        *LOG.lock().unwrap() = Some((path, f));
    }
}

/// UTC time of day with milliseconds; enough to line events up.
fn stamp() -> String {
    let d = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let s = d.as_secs();
    format!("{:02}:{:02}:{:02}.{:03}Z", (s / 3600) % 24, (s / 60) % 60, s % 60, d.subsec_millis())
}

pub fn write(msg: &str) {
    let line = format!("{} {msg}\n", stamp());
    if cfg!(debug_assertions) {
        eprint!("{line}");
    }
    if let Some((_, f)) = LOG.lock().unwrap().as_mut() {
        let _ = f.write_all(line.as_bytes());
    }
}

#[macro_export]
macro_rules! log {
    ($($t:tt)*) => { $crate::log::write(&format!($($t)*)) };
}
