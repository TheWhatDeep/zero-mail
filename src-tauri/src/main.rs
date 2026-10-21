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
//! So are notification, single-instance and autostart (section 11): see `shell`.

mod shell;
mod strings;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mail_core::compose::{DraftContent, DraftView, IdentityView, OutboxView};
use mail_core::features::{RuleView, TemplateView};
use mail_core::search::SearchRequest;
use mail_core::unsubscribe::Outcome;
use mail_core::views::{
    AddressView, FlagChange, FolderView, ListRequest, MessageView, PageView, Role,
};
use mail_core::JsonValue;
use mail_core::{
    AccountConfig, AccountSummary, AccountView, AppError, Candidate, Core, CoreConfig, CoreEvent,
    ErrorCode, EventSink, HttpClient, ImapClient, Keychain, SmtpClient, TestReport,
};
use tauri::http::{header, Response, StatusCode};
use tauri::ipc::CapabilityBuilder;
use tauri::{AppHandle, Emitter, Listener, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

type AppCore = Core<ImapClient, SmtpClient, HttpClient>;

/// The core, shared by commands and the protocol handler.
struct CoreState(Arc<AppCore>);

/// Compose windows asked to close that wait for their page's last `draft.save`, so the
/// typing of the last 5 s is not lost (section 7, Drafts).
#[derive(Default)]
struct Closing(Mutex<HashSet<i64>>);

/// How long a closing compose window waits for its page to save.
const CLOSE_FLUSH: Duration = Duration::from_secs(3);

/// Run in a closing compose window: save now (Compose.svelte sets the hook).
const FLUSH_SCRIPT: &str = "window.__ZEROMAIL_FLUSH__ && window.__ZEROMAIL_FLUSH__();";

/// Takes a draft out of the closing set; true if it was there.
fn take_closing(app: &AppHandle, draft_id: i64) -> bool {
    app.try_state::<Closing>()
        .and_then(|c| c.0.lock().ok().map(|mut set| set.remove(&draft_id)))
        .unwrap_or(false)
}

/// Closes a compose window for good (no second flush).
fn destroy_compose(app: &AppHandle, draft_id: i64) {
    if let Some(w) = app.get_webview_window(&format!("compose-{draft_id}")) {
        let _ = w.destroy();
    }
}

/// A compose window's close button: ask the page to save, close once it has (or after
/// [`CLOSE_FLUSH`]).
fn close_requested(window: &tauri::Window, draft_id: i64, api: &tauri::CloseRequestApi) {
    let app = window.app_handle().clone();
    let fresh = app
        .try_state::<Closing>()
        .and_then(|c| c.0.lock().ok().map(|mut set| set.insert(draft_id)))
        .unwrap_or(false);
    if !fresh {
        return;
    }
    let Some(webview) = app.get_webview_window(window.label()) else {
        take_closing(&app, draft_id);
        return;
    };
    if webview.eval(FLUSH_SCRIPT).is_err() {
        take_closing(&app, draft_id);
        return;
    }
    api.prevent_close();
    std::thread::spawn(move || {
        std::thread::sleep(CLOSE_FLUSH);
        if take_closing(&app, draft_id) {
            destroy_compose(&app, draft_id);
        }
    });
}

/// Set by the perf harness: where to write the result.
const PERF_OUT_ENV: &str = "ZEROMAIL_PERF_OUT";
/// Set by the perf harness: also run the scroll benchmark.
const PERF_SCROLL_ENV: &str = "ZEROMAIL_PERF_SCROLL";
/// Set by T19's CI harness: open every message of the inbox in turn (SPEC_GAPS G68).
const PERF_WALK_ENV: &str = "ZEROMAIL_PERF_WALK";
/// Set by the perf harness and tests: a data directory instead of the OS app-data one.
const DATA_DIR_ENV: &str = "ZEROMAIL_DATA_DIR";
/// T35 harness only (tests/shell/single-instance.mjs): the running instance writes `ready`
/// when started and `focused` when a second launch hands over to it.
const INSTANCE_OUT_ENV: &str = "ZEROMAIL_INSTANCE_OUT";

/// Appends one line to the T35 harness file, when there is one.
fn instance_note(line: &str) {
    use std::io::Write as _;
    let Some(path) = std::env::var_os(INSTANCE_OUT_ENV) else {
        return;
    };
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path);
    if let Ok(mut f) = file {
        let _ = writeln!(f, "{line}");
    }
}

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

/// `account.oauth_start`: Core listens on the loopback redirect; the consent page opens in
/// the system browser. The result arrives as `oauth-done` (section 3).
#[tauri::command(rename_all = "snake_case")]
async fn account_oauth_start(
    app: AppHandle,
    core: tauri::State<'_, CoreState>,
    account_id: i64,
) -> Result<(), AppError> {
    let url = core.0.account_oauth_start(account_id).await?;
    app.opener().open_url(url, None::<&str>).map_err(os_error)
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

#[tauri::command(rename_all = "snake_case")]
async fn images_list_allowed(
    core: tauri::State<'_, CoreState>,
    account_id: i64,
) -> Result<Vec<String>, AppError> {
    core.0.images_list_allowed(account_id).await
}

#[tauri::command(rename_all = "snake_case")]
async fn images_disallow_sender(
    core: tauri::State<'_, CoreState>,
    account_id: i64,
    address: String,
) -> Result<(), AppError> {
    core.0.images_disallow_sender(account_id, &address).await
}

#[tauri::command(rename_all = "snake_case")]
async fn storage_clear_body_cache(core: tauri::State<'_, CoreState>) -> Result<(), AppError> {
    core.0.storage_clear_body_cache().await
}

#[tauri::command(rename_all = "snake_case")]
async fn settings_get(
    core: tauri::State<'_, CoreState>,
    key: String,
) -> Result<JsonValue, AppError> {
    core.0.settings_get(&key).await
}

/// Keep running in tray and autostart also change the shell at once.
#[tauri::command(rename_all = "snake_case")]
async fn settings_set(
    app: AppHandle,
    core: tauri::State<'_, CoreState>,
    key: String,
    value: JsonValue,
) -> Result<JsonValue, AppError> {
    let stored = core.0.settings_set(&key, value).await?;
    shell::setting_changed(&app, &key, &stored);
    Ok(stored)
}

/// `message.unsubscribe` (section 18.7): a mailto draft opens its compose window, an https
/// page opens in the system browser; one-click was already POSTed by Core.
#[tauri::command(rename_all = "snake_case")]
async fn message_unsubscribe(
    app: AppHandle,
    core: tauri::State<'_, CoreState>,
    message_id: i64,
) -> Result<Outcome, AppError> {
    let outcome = core.0.message_unsubscribe(message_id).await?;
    if let Some(draft) = &outcome.draft {
        open_compose(&app, draft)?;
    }
    if let Some(url) = &outcome.url {
        let url = core.0.link_check(url)?;
        app.opener().open_url(url, None::<&str>).map_err(os_error)?;
    }
    Ok(outcome)
}

/// `message.print` (section 18.8): the page has laid out its print view; this opens the
/// webview's print dialog over it.
#[tauri::command(rename_all = "snake_case")]
async fn message_print(
    app: AppHandle,
    core: tauri::State<'_, CoreState>,
    message_id: Option<i64>,
    thread_id: Option<i64>,
) -> Result<(), AppError> {
    core.0.message_print(message_id, thread_id).await?;
    let main = app
        .get_webview_window("main")
        .ok_or_else(|| AppError::new(ErrorCode::Internal, "no main window"))?;
    main.print().map_err(os_error)
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

#[tauri::command(rename_all = "snake_case")]
async fn message_set_flags(
    core: tauri::State<'_, CoreState>,
    message_ids: Vec<i64>,
    flags: FlagChange,
) -> Result<i64, AppError> {
    core.0.message_set_flags(&message_ids, flags).await
}

#[tauri::command(rename_all = "snake_case")]
async fn message_move(
    core: tauri::State<'_, CoreState>,
    message_ids: Vec<i64>,
    folder_id: i64,
) -> Result<i64, AppError> {
    core.0.message_move(&message_ids, folder_id).await
}

#[tauri::command(rename_all = "snake_case")]
async fn message_delete(
    core: tauri::State<'_, CoreState>,
    message_ids: Vec<i64>,
    folder_id: Option<i64>,
) -> Result<i64, AppError> {
    core.0.message_delete(&message_ids, folder_id).await
}

#[tauri::command(rename_all = "snake_case")]
async fn message_archive(
    core: tauri::State<'_, CoreState>,
    message_ids: Vec<i64>,
) -> Result<i64, AppError> {
    core.0.message_archive(&message_ids).await
}

#[tauri::command(rename_all = "snake_case")]
async fn message_mark_all_read(
    core: tauri::State<'_, CoreState>,
    folder_id: i64,
) -> Result<i64, AppError> {
    core.0.message_mark_all_read(folder_id).await
}

#[tauri::command(rename_all = "snake_case")]
async fn message_undo(core: tauri::State<'_, CoreState>, op_id: i64) -> Result<(), AppError> {
    core.0.message_undo(op_id).await
}

#[tauri::command(rename_all = "snake_case")]
async fn folder_create(
    core: tauri::State<'_, CoreState>,
    account_id: i64,
    path: String,
) -> Result<(), AppError> {
    core.0.folder_create(account_id, &path).await
}

#[tauri::command(rename_all = "snake_case")]
async fn folder_rename(
    core: tauri::State<'_, CoreState>,
    account_id: i64,
    path: String,
    value: String,
) -> Result<(), AppError> {
    core.0.folder_rename(account_id, &path, &value).await
}

#[tauri::command(rename_all = "snake_case")]
async fn folder_delete(
    core: tauri::State<'_, CoreState>,
    account_id: i64,
    path: String,
) -> Result<(), AppError> {
    core.0.folder_delete(account_id, &path).await
}

#[tauri::command(rename_all = "snake_case")]
async fn folder_set_role(
    core: tauri::State<'_, CoreState>,
    account_id: i64,
    path: String,
    value: Option<Role>,
) -> Result<(), AppError> {
    core.0.folder_set_role(account_id, &path, value).await
}

#[tauri::command(rename_all = "snake_case")]
async fn folder_set_sync(
    core: tauri::State<'_, CoreState>,
    account_id: i64,
    path: String,
    value: bool,
) -> Result<(), AppError> {
    core.0.folder_set_sync(account_id, &path, value).await
}

#[tauri::command(rename_all = "snake_case")]
async fn folder_set_watch(
    core: tauri::State<'_, CoreState>,
    account_id: i64,
    path: String,
    value: bool,
) -> Result<(), AppError> {
    core.0.folder_set_watch(account_id, &path, value).await
}

#[tauri::command(rename_all = "snake_case")]
async fn folder_extend_window(
    core: tauri::State<'_, CoreState>,
    account_id: i64,
    folder_id: i64,
) -> Result<(), AppError> {
    core.0.folder_extend_window(account_id, folder_id).await
}

/// Opens (or focuses) a draft's compose window (section 11; SPEC_GAPS G63). The draft
/// reaches the page through a start-up script, so the page needs no command to fetch it.
fn open_compose(app: &AppHandle, view: &DraftView) -> Result<(), AppError> {
    let label = format!("compose-{}", view.draft_id);
    if let Some(w) = app.get_webview_window(&label) {
        let _ = w.set_focus();
        return Ok(());
    }
    WebviewWindowBuilder::new(
        app,
        &label,
        WebviewUrl::App(format!("index.html#compose/{}", view.draft_id).into()),
    )
    .title("Zero Mail")
    .inner_size(760.0, 680.0)
    .min_inner_size(560.0, 420.0)
    .initialization_script(format!("window.__ZEROMAIL_DRAFT__ = {};", view.to_json()))
    .build()
    .map(|_| ())
    .map_err(os_error)
}

/// The draft a compose window belongs to.
fn compose_draft(label: &str) -> Option<i64> {
    label.strip_prefix("compose-")?.parse().ok()
}

#[tauri::command(rename_all = "snake_case")]
async fn compose_new(
    app: AppHandle,
    core: tauri::State<'_, CoreState>,
    account_id: i64,
    to: Option<String>,
) -> Result<DraftView, AppError> {
    let view = core.0.compose_new(account_id, to).await?;
    open_compose(&app, &view)?;
    Ok(view)
}

#[tauri::command(rename_all = "snake_case")]
async fn compose_reply(
    app: AppHandle,
    core: tauri::State<'_, CoreState>,
    message_id: i64,
) -> Result<DraftView, AppError> {
    let view = core.0.compose_reply(message_id, false).await?;
    open_compose(&app, &view)?;
    Ok(view)
}

#[tauri::command(rename_all = "snake_case")]
async fn compose_reply_all(
    app: AppHandle,
    core: tauri::State<'_, CoreState>,
    message_id: i64,
) -> Result<DraftView, AppError> {
    let view = core.0.compose_reply(message_id, true).await?;
    open_compose(&app, &view)?;
    Ok(view)
}

#[tauri::command(rename_all = "snake_case")]
async fn compose_forward(
    app: AppHandle,
    core: tauri::State<'_, CoreState>,
    message_id: i64,
    as_attachment: bool,
) -> Result<DraftView, AppError> {
    let view = core.0.compose_forward(message_id, as_attachment).await?;
    open_compose(&app, &view)?;
    Ok(view)
}

#[tauri::command(rename_all = "snake_case")]
async fn draft_save(
    app: AppHandle,
    core: tauri::State<'_, CoreState>,
    draft_id: i64,
    content: DraftContent,
) -> Result<(), AppError> {
    let saved = core.0.draft_save(draft_id, content).await;
    if take_closing(&app, draft_id) {
        destroy_compose(&app, draft_id);
    }
    saved
}

#[tauri::command(rename_all = "snake_case")]
async fn draft_discard(
    window: tauri::WebviewWindow,
    core: tauri::State<'_, CoreState>,
    draft_id: i64,
) -> Result<(), AppError> {
    core.0.draft_discard(draft_id).await?;
    if compose_draft(window.label()) == Some(draft_id) {
        let _ = window.destroy();
    }
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
async fn draft_add_attachment(
    app: AppHandle,
    core: tauri::State<'_, CoreState>,
    draft_id: i64,
    path: Option<String>,
) -> Result<DraftContent, AppError> {
    let path = match path {
        Some(p) => PathBuf::from(p),
        // The native Open dialog from Rust (as for Save, SPEC_GAPS G54).
        None => {
            let app = app.clone();
            let chosen = tauri::async_runtime::spawn_blocking(move || {
                app.dialog().file().blocking_pick_file()
            })
            .await
            .map_err(os_error)?;
            match chosen {
                Some(p) => p.into_path().map_err(os_error)?,
                None => return Err(AppError::new(ErrorCode::Cancelled, "no file chosen")),
            }
        }
    };
    core.0.draft_add_attachment(draft_id, path).await
}

#[tauri::command(rename_all = "snake_case")]
async fn draft_remove_attachment(
    core: tauri::State<'_, CoreState>,
    draft_id: i64,
    attachment_id: String,
) -> Result<DraftContent, AppError> {
    core.0
        .draft_remove_attachment(draft_id, &attachment_id)
        .await
}

#[tauri::command(rename_all = "snake_case")]
async fn draft_send(
    window: tauri::WebviewWindow,
    core: tauri::State<'_, CoreState>,
    draft_id: i64,
    identity_id: i64,
    send_at: Option<i64>,
) -> Result<i64, AppError> {
    let id = core.0.draft_send(draft_id, identity_id, send_at).await?;
    if compose_draft(window.label()) == Some(draft_id) {
        let _ = window.destroy();
    }
    Ok(id)
}

#[tauri::command(rename_all = "snake_case")]
async fn outbox_undo(
    app: AppHandle,
    core: tauri::State<'_, CoreState>,
    outbox_id: i64,
) -> Result<(), AppError> {
    if let Some(draft_id) = core.0.outbox_undo(outbox_id).await? {
        let view = core.0.draft_view(draft_id).await?;
        open_compose(&app, &view)?;
    }
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
async fn outbox_retry(core: tauri::State<'_, CoreState>, outbox_id: i64) -> Result<(), AppError> {
    core.0.outbox_retry(outbox_id).await
}

#[tauri::command(rename_all = "snake_case")]
async fn outbox_discard(core: tauri::State<'_, CoreState>, outbox_id: i64) -> Result<(), AppError> {
    core.0.outbox_discard(outbox_id).await
}

#[tauri::command(rename_all = "snake_case")]
async fn outbox_list(
    core: tauri::State<'_, CoreState>,
    account_id: i64,
) -> Result<Vec<OutboxView>, AppError> {
    core.0.outbox_list(account_id).await
}

#[tauri::command(rename_all = "snake_case")]
async fn identity_list(
    core: tauri::State<'_, CoreState>,
    account_id: i64,
) -> Result<Vec<IdentityView>, AppError> {
    core.0.identity_list(account_id).await
}

#[tauri::command(rename_all = "snake_case")]
async fn identity_save(
    core: tauri::State<'_, CoreState>,
    identity: IdentityView,
) -> Result<Vec<IdentityView>, AppError> {
    core.0.identity_save(identity).await
}

#[tauri::command(rename_all = "snake_case")]
async fn identity_delete(
    core: tauri::State<'_, CoreState>,
    identity_id: i64,
) -> Result<Vec<IdentityView>, AppError> {
    core.0.identity_delete(identity_id).await
}

#[tauri::command(rename_all = "snake_case")]
async fn contacts_suggest(
    core: tauri::State<'_, CoreState>,
    prefix: String,
    account_id: i64,
) -> Result<Vec<AddressView>, AppError> {
    core.0.contacts_suggest(&prefix, account_id).await
}

#[tauri::command(rename_all = "snake_case")]
async fn search_query(
    core: tauri::State<'_, CoreState>,
    request: SearchRequest,
) -> Result<PageView, AppError> {
    core.0.search_query(request).await
}

#[tauri::command(rename_all = "snake_case")]
async fn message_snooze(
    core: tauri::State<'_, CoreState>,
    message_ids: Vec<i64>,
    due_utc: i64,
) -> Result<i64, AppError> {
    core.0.message_snooze(&message_ids, due_utc).await
}

#[tauri::command(rename_all = "snake_case")]
async fn message_unsnooze(
    core: tauri::State<'_, CoreState>,
    message_ids: Vec<i64>,
) -> Result<(), AppError> {
    core.0.message_unsnooze(&message_ids).await
}

#[tauri::command(rename_all = "snake_case")]
async fn message_mute(core: tauri::State<'_, CoreState>, thread_id: i64) -> Result<(), AppError> {
    core.0.message_mute(thread_id).await
}

#[tauri::command(rename_all = "snake_case")]
async fn message_unmute(core: tauri::State<'_, CoreState>, thread_id: i64) -> Result<(), AppError> {
    core.0.message_unmute(thread_id).await
}

#[tauri::command(rename_all = "snake_case")]
async fn message_add_label(
    core: tauri::State<'_, CoreState>,
    message_ids: Vec<i64>,
    folder_id: i64,
) -> Result<i64, AppError> {
    core.0.message_add_label(&message_ids, folder_id).await
}

#[tauri::command(rename_all = "snake_case")]
async fn message_remove_label(
    core: tauri::State<'_, CoreState>,
    message_ids: Vec<i64>,
    folder_id: i64,
) -> Result<i64, AppError> {
    core.0.message_remove_label(&message_ids, folder_id).await
}

#[tauri::command(rename_all = "snake_case")]
async fn rules_list(core: tauri::State<'_, CoreState>) -> Result<Vec<RuleView>, AppError> {
    core.0.rules_list().await
}

#[tauri::command(rename_all = "snake_case")]
async fn rules_save(
    core: tauri::State<'_, CoreState>,
    rule: RuleView,
) -> Result<Vec<RuleView>, AppError> {
    core.0.rules_save(rule).await
}

#[tauri::command(rename_all = "snake_case")]
async fn rules_delete(
    core: tauri::State<'_, CoreState>,
    id: i64,
) -> Result<Vec<RuleView>, AppError> {
    core.0.rules_delete(id).await
}

#[tauri::command(rename_all = "snake_case")]
async fn rules_reorder(
    core: tauri::State<'_, CoreState>,
    ids: Vec<i64>,
) -> Result<Vec<RuleView>, AppError> {
    core.0.rules_reorder(ids).await
}

#[tauri::command(rename_all = "snake_case")]
async fn rules_run_now(core: tauri::State<'_, CoreState>) -> Result<Vec<RuleView>, AppError> {
    core.0.rules_run_now().await
}

#[tauri::command(rename_all = "snake_case")]
async fn templates_list(core: tauri::State<'_, CoreState>) -> Result<Vec<TemplateView>, AppError> {
    core.0.templates_list().await
}

#[tauri::command(rename_all = "snake_case")]
async fn templates_save(
    core: tauri::State<'_, CoreState>,
    template: TemplateView,
) -> Result<Vec<TemplateView>, AppError> {
    core.0.templates_save(template).await
}

#[tauri::command(rename_all = "snake_case")]
async fn templates_delete(
    core: tauri::State<'_, CoreState>,
    id: i64,
) -> Result<Vec<TemplateView>, AppError> {
    core.0.templates_delete(id).await
}

#[tauri::command(rename_all = "snake_case")]
async fn draft_apply_template(
    core: tauri::State<'_, CoreState>,
    draft_id: i64,
    template_id: i64,
) -> Result<TemplateView, AppError> {
    core.0.draft_apply_template(draft_id, template_id).await
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
        // First, so a second launch focuses this one and goes no further (section 11, T35).
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            shell::show_main(app);
            instance_note("focused");
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .setup(move |app| {
            let dir = data_dir(app)?;
            mail_core::logging::init(&dir).map_err(os_error)?;
            let handle = app.handle().clone();
            let sink: EventSink = Arc::new(move |event: CoreEvent| {
                shell::on_event(&handle, &event);
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
            let keep_in_tray = tauri::async_runtime::block_on(
                core.settings_get(mail_core::settings::KEEP_IN_TRAY),
            )?;
            app.manage(CoreState(core));
            app.manage(Closing::default());
            app.manage(shell::Shell::new(keep_in_tray.as_bool() == Some(true)));
            shell::setup(app)?;
            perf_hooks(app, started)?;
            instance_note("ready");
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
            account_oauth_start,
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
            message_set_flags,
            message_move,
            message_delete,
            message_archive,
            message_mark_all_read,
            message_undo,
            folder_create,
            folder_rename,
            folder_delete,
            folder_set_role,
            folder_set_sync,
            folder_set_watch,
            folder_extend_window,
            compose_new,
            compose_reply,
            compose_reply_all,
            compose_forward,
            draft_save,
            draft_discard,
            draft_add_attachment,
            draft_remove_attachment,
            draft_send,
            outbox_undo,
            outbox_retry,
            outbox_discard,
            outbox_list,
            identity_list,
            identity_save,
            identity_delete,
            contacts_suggest,
            search_query,
            message_snooze,
            message_unsnooze,
            message_mute,
            message_unmute,
            message_add_label,
            message_remove_label,
            rules_list,
            rules_save,
            rules_delete,
            rules_reorder,
            rules_run_now,
            templates_list,
            templates_save,
            templates_delete,
            draft_apply_template,
            images_list_allowed,
            images_disallow_sender,
            storage_clear_body_cache,
            settings_get,
            settings_set,
            message_unsubscribe,
            message_print,
        ])
        // A compose window that closes uploads its draft now (section 7, Drafts).
        .on_window_event(|window, event| {
            if let (tauri::WindowEvent::CloseRequested { api, .. }, "main") =
                (event, window.label())
            {
                shell::main_close_requested(window, api);
            }
            if let (tauri::WindowEvent::CloseRequested { api, .. }, Some(draft_id)) =
                (event, compose_draft(window.label()))
            {
                close_requested(window, draft_id, api);
            }
            if let (tauri::WindowEvent::Destroyed, Some(draft_id)) =
                (event, compose_draft(window.label()))
            {
                take_closing(window.app_handle(), draft_id);
                if let Some(core) = window.app_handle().try_state::<CoreState>() {
                    let core = Arc::clone(&core.0);
                    tauri::async_runtime::spawn(async move {
                        let _ = core.draft_close(draft_id).await;
                    });
                }
            }
        })
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
