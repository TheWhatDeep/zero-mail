//! `settings.get` and `settings.set` (section 3): one registry of the keys the Settings
//! screens change, each with its type, allowed values and default (section 11, Settings;
//! SPEC_GAPS G82). A key outside the registry is `invalid_input`; so is a value of the
//! wrong type or out of range. `get` answers the default for a key never set.

use mail_imap::ImapConnector;
use mail_smtp::SmtpCheck;
use serde_json::Value;

use crate::core::Core;
use crate::error::AppError;

/// Setting key prefix: notifications on or off per account, `notify.accounts.<id>`.
pub const NOTIFY_ACCOUNT_PREFIX: &str = "notify.accounts.";
/// Setting key: the body preview in notifications (off by default, section 11).
pub const NOTIFY_PREVIEW: &str = "notify.preview";
/// Setting key: Keep running in tray when the window closes (default on, section 11).
pub const KEEP_IN_TRAY: &str = "app.keep_in_tray";
/// Setting key: start with the OS (default off, section 11).
pub const AUTOSTART: &str = "app.autostart";
/// Setting key: the undo-send window in seconds, 0 to 30 (section 7).
pub const UNDO_SECONDS: &str = "compose.undo_seconds";

/// What a key holds.
#[derive(Debug, Clone, Copy)]
enum Kind {
    Bool(bool),
    /// One of the listed strings; the first is the default.
    Choice(&'static [&'static str]),
    /// An integer in a range, with its default.
    Int {
        min: i64,
        max: i64,
        default: i64,
    },
    /// A non-empty string of at most 200 characters, with its default.
    Text(&'static str),
}

/// The registry (SPEC_GAPS G82).
const KEYS: &[(&str, Kind)] = &[
    // Reading (sections 8 and 11).
    ("ui.theme", Kind::Choice(&["system", "light", "dark"])),
    ("ui.density", Kind::Choice(&["comfortable", "compact"])),
    ("ui.reading_pane", Kind::Choice(&["right", "bottom", "off"])),
    ("ui.group_by_thread", Kind::Bool(true)),
    (
        "reading.mark_read",
        Kind::Choice(&["delay", "immediately", "never"]),
    ),
    ("reading.html", Kind::Bool(true)),
    // Composing (section 7).
    (
        UNDO_SECONDS,
        Kind::Int {
            min: 0,
            max: 30,
            default: 10,
        },
    ),
    // Notifications and background (section 11).
    (NOTIFY_PREVIEW, Kind::Bool(false)),
    (KEEP_IN_TRAY, Kind::Bool(true)),
    (AUTOSTART, Kind::Bool(false)),
    // Privacy and logging (sections 5 and 13).
    (crate::accounts::ONLINE_LOOKUPS, Kind::Bool(true)),
    (crate::logging::DEBUG_SETTING, Kind::Bool(false)),
    // Threads and snooze (sections 10 and 18.2).
    (mail_store::SUBJECT_MERGE_SETTING, Kind::Bool(false)),
    (
        crate::features::SNOOZE_FOLDER_SETTING,
        Kind::Text("Snoozed"),
    ),
    (
        crate::features::SNOOZE_END_ON_REPLY_SETTING,
        Kind::Bool(true),
    ),
];

fn kind_of(key: &str) -> Option<Kind> {
    if let Some(id) = key.strip_prefix(NOTIFY_ACCOUNT_PREFIX) {
        return id.parse::<i64>().is_ok().then_some(Kind::Bool(true));
    }
    KEYS.iter().find(|(k, _)| *k == key).map(|(_, kind)| *kind)
}

fn default_of(kind: Kind) -> Value {
    match kind {
        Kind::Bool(b) => Value::Bool(b),
        Kind::Choice(list) => Value::from(list.first().copied().unwrap_or_default()),
        Kind::Int { default, .. } => Value::from(default),
        Kind::Text(t) => Value::from(t),
    }
}

/// The value, checked against its key's type; `None` when it does not fit.
fn validate(kind: Kind, value: &Value) -> Option<Value> {
    match (kind, value) {
        (Kind::Bool(_), Value::Bool(_)) => Some(value.clone()),
        (Kind::Choice(list), Value::String(s)) => list.contains(&s.as_str()).then(|| value.clone()),
        (Kind::Int { min, max, .. }, Value::Number(n)) => n
            .as_i64()
            .filter(|v| (min..=max).contains(v))
            .map(Value::from),
        (Kind::Text(_), Value::String(s)) => {
            let t = s.trim();
            (!t.is_empty() && t.chars().count() <= 200 && !t.chars().any(char::is_control))
                .then(|| Value::from(t))
        }
        _ => None,
    }
}

/// The stored value if it still fits its key, else the default.
fn effective(kind: Kind, stored: Option<Value>) -> Value {
    stored
        .and_then(|v| validate(kind, &v))
        .unwrap_or_else(|| default_of(kind))
}

impl<I, S, H> Core<I, S, H>
where
    I: ImapConnector + Clone + 'static,
    S: SmtpCheck + mail_smtp::SmtpSubmit + Clone + 'static,
    H: crate::core::Http,
{
    /// `settings.get`: the key's value, or its default when never set.
    pub async fn settings_get(&self, key: &str) -> Result<Value, AppError> {
        let kind = kind_of(key).ok_or_else(|| AppError::invalid("unknown setting"))?;
        Ok(effective(kind, self.store.get_setting(key).await?))
    }

    /// `settings.set`: validates and stores the value, applies debug logging at once, and
    /// returns the stored value. Subject grouping applies to mail threaded from then on
    /// (SPEC_GAPS G82).
    pub async fn settings_set(&self, key: &str, value: Value) -> Result<Value, AppError> {
        let kind = kind_of(key).ok_or_else(|| AppError::invalid("unknown setting"))?;
        let value = validate(kind, &value).ok_or_else(|| AppError::invalid("bad value"))?;
        self.store.set_setting(key, value.clone()).await?;
        if key == crate::logging::DEBUG_SETTING {
            crate::logging::set_debug(value == Value::Bool(true));
        }
        Ok(value)
    }

    /// A boolean setting through the registry's default.
    pub(crate) async fn setting_on(&self, key: &str) -> bool {
        matches!(self.settings_get(key).await, Ok(Value::Bool(true)))
    }

    /// The undo-send window: the setting when set (0 to 30 s), else the timing in use.
    pub(crate) async fn undo_seconds(&self) -> i64 {
        let fallback = i64::try_from(self.timing().undo_window.as_secs()).unwrap_or(10);
        let stored = self.store.get_setting(UNDO_SECONDS).await.ok().flatten();
        match (kind_of(UNDO_SECONDS), stored) {
            (Some(kind), Some(v)) => validate(kind, &v)
                .and_then(|v| v.as_i64())
                .unwrap_or(fallback),
            _ => fallback,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn keys_validate_by_type_and_range() {
        let theme = kind_of("ui.theme").unwrap();
        assert_eq!(validate(theme, &json!("dark")), Some(json!("dark")));
        assert_eq!(validate(theme, &json!("blue")), None);
        assert_eq!(validate(theme, &json!(true)), None);
        let undo = kind_of(UNDO_SECONDS).unwrap();
        assert_eq!(validate(undo, &json!(0)), Some(json!(0)));
        assert_eq!(validate(undo, &json!(30)), Some(json!(30)));
        assert_eq!(validate(undo, &json!(31)), None);
        assert_eq!(validate(undo, &json!(-1)), None);
        assert_eq!(validate(undo, &json!(2.5)), None);
        let folder = kind_of(crate::features::SNOOZE_FOLDER_SETTING).unwrap();
        assert_eq!(validate(folder, &json!("  Later ")), Some(json!("Later")));
        assert_eq!(validate(folder, &json!(" ")), None);
        assert_eq!(validate(folder, &json!("a\nb")), None);
        assert!(kind_of("notify.accounts.7").is_some());
        assert!(kind_of("notify.accounts.x").is_none());
        assert!(kind_of("nope").is_none());
    }

    #[test]
    fn defaults_and_stale_values() {
        let kind = kind_of(KEEP_IN_TRAY).unwrap();
        assert_eq!(effective(kind, None), json!(true));
        assert_eq!(effective(kind, Some(json!("yes"))), json!(true));
        assert_eq!(effective(kind, Some(json!(false))), json!(false));
        assert_eq!(
            effective(kind_of("ui.reading_pane").unwrap(), None),
            json!("right")
        );
        assert_eq!(
            effective(kind_of("reading.mark_read").unwrap(), None),
            json!("delay")
        );
        assert_eq!(
            effective(kind_of(NOTIFY_PREVIEW).unwrap(), None),
            json!(false)
        );
        assert_eq!(effective(kind_of(AUTOSTART).unwrap(), None), json!(false));
    }
}
