//! Kiritori: cut, move and reshape regions of any screen.
//!
//! Modules: `model` (pure data and logic), `store` (files in the Kiritori folder), `shell` (tray,
//! shortcuts, app detection, windows), `engine` (capture and drawing, on its own thread) and
//! `layer` (the editing overlay).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![allow(dead_code)] // Parts of the model wait for the engine and layer.

mod app;
mod cli;
mod engine;
mod layer;
mod log;
mod model;
mod panel;
mod shell;
mod selftest;
mod settings;
mod store;
mod strings;
mod textedit;
mod tutorial;

use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
use windows::Win32::System::Threading::CreateMutexW;
use windows::core::w;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if cli::wants_help(&args) {
        shell::info(&format!("Kiritori {}: cut, move and reshape regions of any screen.

{}", env!("CARGO_PKG_VERSION"), cli::USAGE));
        return;
    }
    let command = match cli::parse(&args) {
        Ok(c) => c,
        Err(e) => {
            shell::message(&format!("{e}.\n\n{}", cli::USAGE));
            std::process::exit(2);
        }
    };

    // One copy per Windows session. A second launch hands its arguments to the running copy
    // (no arguments: open its tray menu).
    // Windows 11 is required: switching off the capture border arrived in build 20348.
    let build = selftest::windows_build();
    if build != 0 && build < 20348 {
        shell::message("Kiritori needs Windows 11.");
        std::process::exit(3);
    }

    let mut _mutex = unsafe { CreateMutexW(None, true, w!("Local\\Kiritori.SingleInstance")) };
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        if command == cli::Command::Replace {
            // Restart as administrator: ask the running copy to quit, then take its place.
            shell::forward_to_running(&["--quit".to_string()]);
            let mut ok = false;
            for _ in 0..100 {
                std::thread::sleep(std::time::Duration::from_millis(100));
                _mutex = unsafe { CreateMutexW(None, true, w!("Local\\Kiritori.SingleInstance")) };
                if unsafe { GetLastError() } != ERROR_ALREADY_EXISTS {
                    ok = true;
                    break;
                }
            }
            if !ok {
                return;
            }
        } else {
            shell::forward_to_running(&args);
            return;
        }
    }
    if command == cli::Command::Quit {
        return; // nothing running to quit
    }

    let store = match store::Store::beside_exe() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Kiritori cannot find its own folder: {e}");
            std::process::exit(1);
        }
    };
    log::init(&store.logs_dir());
    strings::init(store.root(), "en");
    log!("Kiritori {} starting in {}{}", env!("CARGO_PKG_VERSION"), store.root().display(), if store.writable() { "" } else { " (read-only: not saving)" });

    // Panics go to the log; the process then exits and its canvases go with it.
    std::panic::set_hook(Box::new(|info| {
        crate::log::write(&format!("panic: {info}"));
    }));

    let previous = store.read_marker();
    let mut app = app::App::new(store);
    if let Some(m) = previous {
        app.recover_from(m);
    }
    app.write_marker();
    if let Err(e) = shell::run(app, if matches!(command, cli::Command::Menu | cli::Command::Replace) { Vec::new() } else { args }) {
        log!("fatal: {e}");
        std::process::exit(1);
    }
}
