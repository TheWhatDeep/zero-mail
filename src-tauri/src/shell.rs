//! The OS side of section 11: the tray (Open, Compose, Quit), the unread count in the
//! title, tooltip and taskbar, notifications while the window is unfocused or hidden,
//! Keep running in tray, and autostart. Decisions stay in `mail-core`: it says which
//! messages notify and what is unread; this module only shows them.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use mail_core::logging::app_warning;
use mail_core::CoreEvent;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_notification::NotificationExt;

use crate::strings;

/// The tray icon's id.
const TRAY: &str = "main";
/// The Windows overlay dot: its size in pixels and the accent color (tokens.css
/// `--color-accent`, SPEC_GAPS G81).
const DOT_SIZE: u32 = 16;
const DOT_RGB: [u8; 3] = [0x00, 0x88, 0xb0];

/// What the window and tray know between events.
pub struct Shell {
    /// `app.keep_in_tray`: the close button hides the main window.
    keep_in_tray: AtomicBool,
}

impl Shell {
    pub fn new(keep_in_tray: bool) -> Self {
        Self {
            keep_in_tray: AtomicBool::new(keep_in_tray),
        }
    }
}

/// Builds the tray and follows the unread count.
pub fn setup(app: &tauri::App) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", strings::TRAY_OPEN, true, None::<&str>)?;
    let new_mail = MenuItem::with_id(app, "compose", strings::TRAY_COMPOSE, true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", strings::TRAY_QUIT, true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &new_mail, &quit])?;
    let mut tray = TrayIconBuilder::with_id(TRAY)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .tooltip(strings::tooltip(0))
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => show_main(app),
            "compose" => compose(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;

    let handle = app.handle().clone();
    let core = Arc::clone(&app.state::<crate::CoreState>().0);
    core.watch_unread(move |n| show_count(&handle, n));
    Ok(())
}

/// Brings the main window to the front (tray, a second launch, a notification click).
pub fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// Tray Compose: a new draft on the first enabled account; without one, the main window.
fn compose(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let core = Arc::clone(&app.state::<crate::CoreState>().0);
        let first = core
            .account_list()
            .await
            .ok()
            .and_then(|list| list.into_iter().find(|a| a.summary.enabled));
        let Some(account) = first else {
            show_main(&app);
            return;
        };
        match core.compose_new(account.summary.id, None).await {
            Ok(view) => {
                if let Err(e) = crate::open_compose(&app, &view) {
                    app_warning("compose window not opened", &e.message);
                }
            }
            Err(e) => app_warning("draft not created", &format!("{:?}", e.code)),
        }
    });
}

/// Every Core event passes here before the page gets it; `notify` shows the OS
/// notification while the main window is unfocused or hidden.
pub fn on_event(app: &AppHandle, event: &CoreEvent) {
    let CoreEvent::Notify {
        sender,
        subject,
        preview,
        count,
        account_name,
        ..
    } = event
    else {
        return;
    };
    let Some(main) = app.get_webview_window("main") else {
        return;
    };
    if main.is_focused().unwrap_or(false) && main.is_visible().unwrap_or(false) {
        return;
    }
    let (title, body) = if *count > 1 {
        (
            strings::APP_NAME.to_string(),
            strings::many_new(*count, account_name),
        )
    } else {
        let subject = if subject.trim().is_empty() {
            strings::NO_SUBJECT
        } else {
            subject.as_str()
        };
        let body = match preview {
            Some(p) => format!(
                "{subject}
{p}"
            ),
            None => subject.to_string(),
        };
        (sender.clone(), body)
    };
    if let Err(e) = app.notification().builder().title(title).body(body).show() {
        app_warning("notification not shown", &e.to_string());
    }
}

/// The unread count in the title, the tray tooltip and the taskbar (section 11; G81).
fn show_count(app: &AppHandle, unread: u32) {
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.set_title(&strings::title(unread));
        #[cfg(windows)]
        {
            let _ = main.set_overlay_icon((unread > 0).then(dot));
        }
        #[cfg(not(windows))]
        {
            let _ = main.set_badge_count((unread > 0).then_some(i64::from(unread)));
        }
    }
    if let Some(tray) = app.tray_by_id(TRAY) {
        let _ = tray.set_tooltip(Some(strings::tooltip(unread)));
    }
}

/// A solid accent dot with a soft edge, drawn in code (SPEC_GAPS G81).
#[cfg_attr(not(windows), allow(dead_code))]
fn dot() -> Image<'static> {
    let size = DOT_SIZE;
    let r = f64::from(size) / 2.0;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let dx = f64::from(x) + 0.5 - r;
            let dy = f64::from(y) + 0.5 - r;
            let d = (dx * dx + dy * dy).sqrt();
            // 1 inside, 0 outside, a one-pixel ramp at the edge.
            let alpha = (r - 0.5 - d + 0.5).clamp(0.0, 1.0);
            rgba.extend_from_slice(&DOT_RGB);
            rgba.push((alpha * 255.0).round() as u8);
        }
    }
    Image::new_owned(rgba, size, size)
}

/// The main window's close button: hide to the tray when Keep running in tray is on.
pub fn main_close_requested(window: &tauri::Window, api: &tauri::CloseRequestApi) {
    let keep = window
        .app_handle()
        .try_state::<Shell>()
        .is_some_and(|s| s.keep_in_tray.load(Ordering::Relaxed));
    if keep {
        api.prevent_close();
        let _ = window.hide();
    } else {
        window.app_handle().exit(0);
    }
}

/// A setting the shell applies itself changed (`settings.set`).
pub fn setting_changed(app: &AppHandle, key: &str, value: &mail_core::JsonValue) {
    let on = value.as_bool() == Some(true);
    if key == mail_core::settings::KEEP_IN_TRAY {
        if let Some(shell) = app.try_state::<Shell>() {
            shell.keep_in_tray.store(on, Ordering::Relaxed);
        }
    }
    if key == mail_core::settings::AUTOSTART {
        let launcher = app.autolaunch();
        let result = if on {
            launcher.enable()
        } else {
            launcher.disable()
        };
        if let Err(e) = result {
            app_warning("autostart not changed", &e.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dot_is_round_and_accent() {
        let img = dot();
        assert_eq!((img.width(), img.height()), (DOT_SIZE, DOT_SIZE));
        let px = |x: u32, y: u32| {
            let i = ((y * DOT_SIZE + x) * 4) as usize;
            img.rgba()[i..i + 4].to_vec()
        };
        assert_eq!(px(8, 8), vec![0x00, 0x88, 0xb0, 255]);
        assert_eq!(px(0, 0)[3], 0, "corners are clear");
    }
}
