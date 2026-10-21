//! The few English strings the OS shows outside the page: the tray menu and tooltip, the
//! window title and the coalesced notification (section 11). Every page string lives in
//! `ui/src/strings.ts`; these are its OS-side counterpart (SPEC_GAPS G82).

pub const APP_NAME: &str = "Zero Mail";
pub const TRAY_OPEN: &str = "Open";
pub const TRAY_COMPOSE: &str = "Compose";
pub const TRAY_QUIT: &str = "Quit";
/// Matches `strings.noSubject` in the page.
pub const NO_SUBJECT: &str = "(no subject)";

/// The main window title: the unread count when there is any.
pub fn title(unread: u32) -> String {
    match unread {
        0 => APP_NAME.to_string(),
        n => format!("{APP_NAME} ({n})"),
    }
}

/// The tray tooltip.
pub fn tooltip(unread: u32) -> String {
    match unread {
        0 => APP_NAME.to_string(),
        1 => format!("{APP_NAME}: 1 unread"),
        n => format!("{APP_NAME}: {n} unread"),
    }
}

/// A coalesced notification (section 11).
pub fn many_new(count: u32, account: &str) -> String {
    format!("{count} new messages in {account}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts() {
        assert_eq!(title(0), "Zero Mail");
        assert_eq!(title(3), "Zero Mail (3)");
        assert_eq!(tooltip(1), "Zero Mail: 1 unread");
        assert_eq!(many_new(5, "Work"), "5 new messages in Work");
    }
}
