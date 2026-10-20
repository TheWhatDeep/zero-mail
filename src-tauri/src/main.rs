#![forbid(unsafe_code)]
// No console window in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! `app` (BUILD.md section 3): Tauri commands, events, windows, protocols, tray and
//! notifications. Holds no business logic; everything goes through `mail-core`.
//!
//! Commands are the section 3 names with `.` written as `_` (Tauri command names are Rust
//! identifiers); `ui/src/lib/ipc.ts` maps them back. The capability file allows exactly
//! these commands plus listening to events (src-tauri/capabilities/default.json).
//!
//! Perf mode (`ZEROMAIL_PERF_OUT`, tests/perf/startup.mjs only) adds one runtime capability
//! so the page can report its marks, and writes them to that file.
//!
//! The opener and dialog plugins are used from Rust only: `link.open`, `attachment.open`
//! and `log.open` hand a checked URL or path to the OS, and Save as opens the native dialog
//! when the page passes no path (SPEC_GAPS G52, G54). The page has no plugin permission.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use mail_core::views::{FolderView, ListRequest, MessageView, PageView};
use mail_core::{
    AccountConfig, AccountSummary, AccountView, AppError, Candidate, Core, CoreConfig, CoreEvent,
    ErrorCode, EventSink, HttpClient, ImapClient, Keychain, SmtpClient, TestReport,
};
use tauri::http::{header, Response, StatusCode};
use tauri::ipc::CapabilityBuilder;
use tauri::{AppHandle, Emitter, Listener, Manager, RunEvent};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

type AppCore = Core<ImapClient, SmtpClient, HttpClient>;

/// The core, shared by commands and the protocol handler.
struct CoreState(Arc<AppCore>);

/// Set by the perf harness: where to write the result.
const PERF_OUT_ENV: &str = "ZEROMAIL_PERF_OUT";
/// Set by the perf harness: also run the scroll benchmark.
const PERF_SCROLL_ENV: &str = "ZEROMAIL_PERF_SCROLL";
/// Set by T19's CI harness: open every message of the inbox in turn (SPEC_GAPS G68).
const PERF_WALK_ENV: &str = "ZEROMAIL_PERF_WALK";
/// Set by the perf harness and tests: a data directory instead of the OS app-data one.
const DATA_DIR_ENV: &str = "ZEROMAIL_DATA_DIR";

#[tauri::command(rename_all = "snake_case")]
async fn account_list(core: tauri::State<'_, CoreState>) -> Result<Vec<AccountView>, AppError> {
    core.0.account_list().await
}

#[tauri::command(rename_all = "snake_case")]
async fn account_resolve(
    core: tauri::State<'_, CoreState>,
    email: String,
) -> Result<Vec<Candidate>, AppError> {
    core.0.account_resolve(&email).await
}

#[tauri::command(rename_all = "snake_case")]
async fn account_test(
    core: tauri::State<'_, CoreState>,
    config: AccountConfig,
) -> Result<TestReport, AppError> {
    core.0.account_test(&config).await
}

#[tauri::command(rename_all = "snake_case")]
async fn account_add(
    core: tauri::State<'_, CoreState>,
    config: AccountConfig,
) -> Result<AccountSummary, AppError> {
    core.0.account_add(config).await
}

#[tauri::command(rename_all = "snake_case")]
async fn account_update(
    core: tauri::State<'_, CoreState>,
    account_id: i64,
    config: AccountConfig,
) -> Result<AccountSummary, AppError> {
    core.0.account_update(account_id, config).await
}

#[tauri::command(rename_all = "snake_case")]
async fn account_remove(
    core: tauri::State<'_, CoreState>,
    account_id: i64,
) -> Result<(), AppError> {
    core.0.account_remove(account_id).await
}

#[tauri::command(rename_all = "snake_case")]
async fn account_set_enabled(
    core: tauri::State<'_, CoreState>,
    account_id: i64,
    enabled: bool,
) -> Result<(), AppError> {
    core.0.account_set_enabled(account_id, enabled).await
}

#[tauri::command(rename_all = "snake_case")]
async fn folder_list(
    core: tauri::State<'_, CoreState>,
    account_id: i64,
) -> Result<Vec<FolderView>, AppError> {
    core.0.folder_list(account_id).await
}

#[tauri::command(rename_all = "snake_case")]
async fn folder_refresh(
    core: tauri::State<'_, CoreState>,
    account_id: i64,
    folder_id: i64,
) -> Result<(), AppError> {
    core.0.folder_refresh(account_id, folder_id).await
}

#[tauri::command(rename_all = "snake_case")]
async fn list_page(
    core: tauri::State<'_, CoreState>,
    request: ListRequest,
) -> Result<PageView, AppError> {
    core.0.list_page(request).await
}

#[tauri::command(rename_all = "snake_case")]
async fn message_open(
    core: tauri::State<'_, CoreState>,
    message_id: i64,
) -> Result<MessageView, AppError> {
    core.0.message_open(message_id).await
}

#[tauri::command(rename_all = "snake_case")]
async fn images_allow_once(
    core: tauri::State<'_, CoreState>,
    message_id: i64,
) -> Result<(), AppError> {
    core.0.images_allow_once(message_id).await
}

#[tauri::command(rename_all = "snake_case")]
async fn images_allow_sender(
    core: tauri::State<'_, CoreState>,
    message_id: i64,
) -> Result<(), AppError> {
    core.0.images_allow_sender(message_id).await
}

fn os_error(e: impl std::fmt::Display) -> AppError {
    AppError::new(ErrorCode::Io, e.to_string())
}

/// The native Save dialog, offering `name` (SPEC_GAPS G54). Closing it is `cancelled`.
async fn save_dialog(app: &AppHandle, name: String) -> Result<PathBuf, AppError> {
    let app = app.clone();
    let chosen = tauri::async_runtime::spawn_blocking(move || {
        app.dialog().file().set_file_name(name).blocking_save_file()
    })
    .await
    .map_err(os_error)?;
    match chosen {
        Some(path) => path.into_path().map_err(os_error),
        None => Err(AppError::new(ErrorCode::Cancelled, "save cancelled")),
    }
}

#[tauri::command(rename_all = "snake_case")]
async fn attachment_open(
    app: AppHandle,
    core: tauri::State<'_, CoreState>,
    attachment_id: i64,
) -> Result<(), AppError> {
    let path = core.0.attachment_open(attachment_id).await?;
    app.opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(os_error)
}

#[tauri::command(rename_all = "snake_case")]
async fn attachment_save(
    app: AppHandle,
    core: tauri::State<'_, CoreState>,
    attachment_id: i64,
    path: Option<String>,
) -> Result<(), AppError> {
    let path = match path {
        Some(p) => PathBuf::from(p),
        None => {
            let name = core.0.attachment_file_name(attachment_id).await?;
            save_dialog(&app, name).await?
        }
    };
    core.0.attachment_save(attachment_id, path).await
}

#[tauri::command(rename_all = "snake_case")]
async fn message_save_source(
    app: AppHandle,
    core: tauri::State<'_, CoreState>,
    message_id: i64,
    path: Option<String>,
) -> Result<(), AppError> {
    let path = match path {
        Some(p) => PathBuf::from(p),
        None => {
            let name = core.0.source_file_name(message_id).await?;
            save_dialog(&app, name).await?
        }
    };
    core.0.message_save_source(message_id, path).await
}

#[tauri::command(rename_all = "snake_case")]
async fn link_open(
    app: AppHandle,
    core: tauri::State<'_, CoreState>,
    url: String,
) -> Result<(), AppError> {
    let url = core.0.link_check(&url)?;
    app.opener().open_url(url, None::<&str>).map_err(os_error)
}

#[tauri::command(rename_all = "snake_case")]
async fn log_open(app: AppHandle, core: tauri::State<'_, CoreState>) -> Result<(), AppError> {
    let dir = core.0.log_dir();
    std::fs::create_dir_all(&dir).map_err(os_error)?;
    app.opener()
        .open_path(dir.to_string_lossy(), None::<&str>)
        .map_err(os_error)
}

/// `mail-body://localhost/<message>?images=0|1&plain=0|1` (section 8, step 3; SPEC_GAPS
/// G53): the message id and the query. The CSP is a response header, never a meta tag.
fn mail_body_request(uri: &tauri::http::Uri) -> Option<(i64, String)> {
    let id = uri.path().trim_matches('/').parse().ok()?;
    Some((id, uri.query().unwrap_or_default().to_string()))
}

fn respond(status: u16, content_type: &str, csp: &str, body: Vec<u8>) -> Response<Vec<u8>> {
    let built = Response::builder()
        .status(StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR))
        .header(header::CONTENT_TYPE, content_type)
        .header("Content-Security-Policy", csp)
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(header::REFERRER_POLICY, "no-referrer")
        .header(header::CACHE_CONTROL, "no-store")
        .body(body);
    built.unwrap_or_else(|_| Response::new(Vec::new()))
}

fn reply(r: mail_core::BodyResponse) -> Response<Vec<u8>> {
    respond(r.status, &r.content_type, &r.csp, r.body)
}

fn data_dir(app: &tauri::App) -> Result<std::path::PathBuf, AppError> {
    match std::env::var_os(DATA_DIR_ENV) {
        Some(dir) => Ok(dir.into()),
        None => app
            .path()
            .app_data_dir()
            .map_err(|e| AppError::new(ErrorCode::Io, e.to_string())),
    }
}

/// Perf mode: the page reports when the cached inbox is interactive, and on request the
/// scroll benchmark; both go to the file the harness reads, then the app exits.
fn perf_hooks(app: &tauri::App, started: Instant) -> tauri::Result<()> {
    let Some(out) = std::env::var_os(PERF_OUT_ENV) else {
        return Ok(());
    };
    app.add_capability(
        CapabilityBuilder::new("perf")
            .window("main")
            .permission("core:event:allow-emit"),
    )?;
    let scroll = std::env::var_os(PERF_SCROLL_ENV).is_some();
    let walk = std::env::var_os(PERF_WALK_ENV).is_some();
    let startup = Arc::new(std::sync::Mutex::new(0u128));
    let handle = app.handle().clone();
    let (out_ready, startup_ready) = (out.clone(), Arc::clone(&startup));
    app.listen("zeromail-perf-ready", move |event| {
        let ms = started.elapsed().as_millis();
        if let Ok(mut s) = startup_ready.lock() {
            *s = ms;
        }
        if walk {
            let _ = handle.emit("zeromail-perf-walk", ());
        } else if scroll {
            let _ = handle.emit("zeromail-perf-scroll", ());
        } else {
            let json = format!("{{\"startup_ms\":{ms},\"ui\":{}}}\n", event.payload());
            if std::fs::write(&out_ready, json).is_err() {
                eprintln!("perf: could not write the result");
            }
            handle.exit(0);
        }
    });
    let (handle, out_walk) = (app.handle().clone(), out.clone());
    app.listen("zeromail-perf-walk-result", move |event| {
        let json = format!("{{\"walk\":{}}}\n", event.payload());
        if std::fs::write(&out_walk, json).is_err() {
            eprintln!("perf: could not write the result");
        }
        handle.exit(0);
    });
    let handle = app.handle().clone();
    app.listen("zeromail-perf-scroll-result", move |event| {
        let ms = startup.lock().map(|s| *s).unwrap_or_default();
        let json = format!("{{\"startup_ms\":{ms},\"scroll\":{}}}\n", event.payload());
        if std::fs::write(&out, json).is_err() {
            eprintln!("perf: could not write the result");
        }
        handle.exit(0);
    });
    Ok(())
}

fn main() {
    let started = Instant::now();
    let built = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(move |app| {
            let dir = data_dir(app)?;
            mail_core::logging::init(&dir).map_err(os_error)?;
            let handle = app.handle().clone();
            let sink: EventSink = Arc::new(move |event: CoreEvent| {
                let _ = handle.emit(event.name(), &event);
            });
            let core = tauri::async_runtime::block_on(Core::start(
                CoreConfig {
                    data_dir: dir,
                    secrets: Arc::new(Keychain::new()),
                    imap: ImapClient,
                    smtp: SmtpClient,
                    http: HttpClient::new()?,
                },
                sink,
            ))?;
            app.manage(CoreState(core));
            perf_hooks(app, started)?;
            Ok(())
        })
        .register_asynchronous_uri_scheme_protocol("mail-body", |ctx, request, responder| {
            let app = ctx.app_handle().clone();
            let target = mail_body_request(request.uri());
            tauri::async_runtime::spawn(async move {
                let Some((id, query)) = target else {
                    responder.respond(reply(mail_core::empty_response(400)));
                    return;
                };
                let core = Arc::clone(&app.state::<CoreState>().0);
                match core.mail_body(id, &query).await {
                    Ok(r) => responder.respond(reply(r)),
                    Err(_) => responder.respond(reply(mail_core::empty_response(500))),
                }
            });
        })
        // Inline images of a message (section 8, Attachments).
        .register_asynchronous_uri_scheme_protocol("mail-cid", |ctx, request, responder| {
            let app = ctx.app_handle().clone();
            let path = request.uri().path().to_string();
            tauri::async_runtime::spawn(async move {
                let core = Arc::clone(&app.state::<CoreState>().0);
                responder.respond(reply(core.mail_cid(&path).await));
            });
        })
        // Remote images through the proxy, only once the CSP admits them (section 8).
        .register_asynchronous_uri_scheme_protocol("mail-remote", |ctx, request, responder| {
            let app = ctx.app_handle().clone();
            let path = request.uri().path().to_string();
            tauri::async_runtime::spawn(async move {
                let core = Arc::clone(&app.state::<CoreState>().0);
                responder.respond(reply(core.mail_remote(&path).await));
            });
        })
        .invoke_handler(tauri::generate_handler![
            account_list,
            account_resolve,
            account_test,
            account_add,
            account_update,
            account_remove,
            account_set_enabled,
            folder_list,
            folder_refresh,
            list_page,
            message_open,
            images_allow_once,
            images_allow_sender,
            attachment_open,
            attachment_save,
            message_save_source,
            link_open,
            log_open,
        ])
        .build(tauri::generate_context!());

    let app = match built {
        Ok(app) => app,
        Err(err) => {
            eprintln!("Zero Mail failed to start: {err}");
            std::process::exit(1);
        }
    };
    app.run(|handle, event| {
        if let RunEvent::Exit = event {
            if let Some(core) = handle.try_state::<CoreState>() {
                let core = Arc::clone(&core.0);
                tauri::async_runtime::block_on(core.shutdown());
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mail_body_urls_parse_on_every_platform_form() {
        for url in [
            "mail-body://localhost/42?images=0",
            "http://mail-body.localhost/42?images=0",
        ] {
            let uri: tauri::http::Uri = url.parse().unwrap();
            assert_eq!(mail_body_request(&uri), Some((42, "images=0".to_string())));
        }
        let uri: tauri::http::Uri = "mail-body://localhost/7?images=1&plain=1".parse().unwrap();
        assert_eq!(
            mail_body_request(&uri),
            Some((7, "images=1&plain=1".to_string()))
        );
        let uri: tauri::http::Uri = "mail-body://localhost/../etc".parse().unwrap();
        assert_eq!(mail_body_request(&uri), None);
    }

    #[test]
    fn responses_carry_the_csp_header() {
        let r = respond(
            200,
            "text/html; charset=utf-8",
            "default-src 'none'",
            b"x".to_vec(),
        );
        assert_eq!(r.headers()["Content-Security-Policy"], "default-src 'none'");
        assert_eq!(
            r.headers()[header::CONTENT_TYPE],
            "text/html; charset=utf-8"
        );
        assert_eq!(r.headers()[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert_eq!(r.status(), 200);
    }
}
