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
    "account_set_enabled",
    "folder_list",
    "folder_refresh",
    "list_page",
    "message_open",
];

fn main() {
    let attributes = tauri_build::Attributes::new()
        .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS));
    if let Err(err) = tauri_build::try_build(attributes) {
        eprintln!("tauri build failed: {err:#}");
        std::process::exit(1);
    }
}
