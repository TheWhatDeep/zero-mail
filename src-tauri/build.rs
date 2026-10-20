#![forbid(unsafe_code)]

/// The app's own commands (BUILD.md section 3; `.` written as `_`). Each gets an
/// `allow-<name>` permission, and capabilities/default.json lists exactly these.
const COMMANDS: &[&str] = &[
    "account_list",
    "account_resolve",
    "account_test",
    "account_add",
    "account_update",
    "account_remove",
    "account_oauth_start",
    "account_set_enabled",
    "folder_list",
    "folder_refresh",
    "list_page",
    "message_open",
    "images_allow_once",
    "images_allow_sender",
    "attachment_open",
    "attachment_save",
    "message_save_source",
    "link_open",
    "log_open",
    "message_set_flags",
    "message_move",
    "message_delete",
    "message_archive",
    "message_mark_all_read",
    "message_undo",
    "folder_create",
    "folder_rename",
    "folder_delete",
    "folder_set_role",
    "folder_set_sync",
    "folder_set_watch",
    "folder_extend_window",
    "compose_new",
    "compose_reply",
    "compose_reply_all",
    "compose_forward",
    "draft_save",
    "draft_discard",
    "draft_add_attachment",
    "draft_remove_attachment",
    "draft_send",
    "outbox_undo",
    "outbox_retry",
    "outbox_discard",
    "outbox_list",
    "identity_list",
    "identity_save",
    "identity_delete",
    "contacts_suggest",
];

fn main() {
    let attributes = tauri_build::Attributes::new()
        .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS));
    if let Err(err) = tauri_build::try_build(attributes) {
        eprintln!("tauri build failed: {err:#}");
        std::process::exit(1);
    }
}
