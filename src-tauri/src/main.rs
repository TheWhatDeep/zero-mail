#![forbid(unsafe_code)]
// No console window in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! `app` (BUILD.md section 3): Tauri commands, events, windows, tray and notifications.
//! Holds no business logic; everything goes through `mail-core`.
//!
//! M0 shell: one window. The startup perf hook below is active only when the harness sets
//! `ZEROMAIL_PERF_OUT` (tests/perf/startup.mjs).

use std::time::Instant;

use tauri::webview::PageLoadEvent;
use tauri::Manager;

/// Environment variable naming the file the perf harness reads.
const PERF_OUT_ENV: &str = "ZEROMAIL_PERF_OUT";

fn main() {
    let started = Instant::now();
    let perf_out = std::env::var_os(PERF_OUT_ENV);

    let result = tauri::Builder::default()
        .on_page_load(move |webview, payload| {
            if payload.event() != PageLoadEvent::Finished {
                return;
            }
            if let Some(path) = &perf_out {
                let ms = started.elapsed().as_millis();
                if std::fs::write(path, format!("{{\"startup_ms\":{ms}}}\n")).is_err() {
                    eprintln!("perf: could not write the startup result");
                }
                webview.app_handle().exit(0);
            }
        })
        .run(tauri::generate_context!());

    if let Err(err) = result {
        eprintln!("Zero Mail failed to start: {err}");
        std::process::exit(1);
    }
}
