#![forbid(unsafe_code)]
//! `outbox` (BUILD.md sections 3 and 7): the send queue, the undo window, and the hand-off
//! of the Sent copy and draft uploads to `ops` (SPEC_GAPS G66). May call `smtp`, `imap` and
//! `store`; never retries a message already handed to SMTP.
//!
//! | State | Next |
//! | --- | --- |
//! | `queued` | `sending` when `send_after_utc` passes; deleted by Undo |
//! | `sending` | `sent`, `retry_wait`, `failed` or `unknown` |
//! | `retry_wait` | `queued` again after 30 s, 2 min, 10 min, then 1 h; `failed` after 6 attempts |
//! | `sent` | terminal |
//! | `failed` | the user edits, retries or discards |
//! | `unknown` | a crash or a lost answer after DATA: never retried automatically |
//!
//! An item is claimed (`sending`, committed) before the SMTP session starts, so a crash
//! mid-send leaves `sending`, which the next start turns into `unknown` (T13).

use mail_smtp::{Envelope, SmtpConfig, SmtpLogin, SmtpSubmit, SubmitError};
use mail_store::{OutboxId, OutboxItem, OutboxState, Store, StoreError};
use serde_json::{json, Value};

/// Waits before the 2nd, 3rd, 4th and later attempts (section 7).
pub const RETRY_SECS: [i64; 4] = [30, 120, 600, 3600];
/// The sixth failed attempt is final (section 7).
pub const MAX_ATTEMPTS: u32 = 6;
/// The default undo window (section 7; the setting arrives with Settings in M10).
pub const UNDO_WINDOW_SECS: i64 = 10;

/// What happened to one item.
#[derive(Debug, Clone, PartialEq)]
pub struct Transition {
    pub item: OutboxItem,
    pub state: OutboxState,
}

/// The error a failed item keeps, as JSON text: `{code, message, addresses?}` with the
/// section 3 error codes, so the UI can say what went wrong.
pub fn error_json(err: &SubmitError) -> Value {
    match err {
        SubmitError::RecipientsRejected(addresses) => json!({
            "code": "recipient_rejected",
            "message": "recipients refused",
            "addresses": addresses,
        }),
        SubmitError::TooLarge => json!({ "code": "too_large", "message": "message too large" }),
        SubmitError::AuthFailed => {
            json!({ "code": "auth_failed", "message": "authentication failed" })
        }
        SubmitError::Permanent(e) => json!({ "code": "server_rejected", "message": e.to_string() }),
        SubmitError::Transient(e) => json!({ "code": "network", "message": e.to_string() }),
        SubmitError::Ambiguous => {
            json!({ "code": "network", "message": "the server's answer was lost" })
        }
    }
}

/// The section 7 transition for a submission result. `attempts` counts earlier failures.
pub fn next_state(
    result: &Result<(), SubmitError>,
    attempts: u32,
    now: i64,
) -> (OutboxState, Option<i64>, Option<String>) {
    match result {
        Ok(()) => (OutboxState::Sent, None, Some(String::new())),
        Err(SubmitError::Ambiguous) => (
            OutboxState::Unknown,
            None,
            Some(error_json(&SubmitError::Ambiguous).to_string()),
        ),
        Err(e @ SubmitError::Transient(_)) => {
            let failures = attempts + 1;
            if failures >= MAX_ATTEMPTS {
                (OutboxState::Failed, None, Some(error_json(e).to_string()))
            } else {
                let wait = RETRY_SECS[usize::try_from(failures - 1)
                    .unwrap_or(0)
                    .min(RETRY_SECS.len() - 1)];
                (
                    OutboxState::RetryWait,
                    Some(now + wait),
                    Some(error_json(e).to_string()),
                )
            }
        }
        Err(e) => (OutboxState::Failed, None, Some(error_json(e).to_string())),
    }
}

/// The envelope stored with an item.
pub fn envelope_of(item: &OutboxItem) -> Envelope {
    let e = &item.envelope["envelope"];
    Envelope {
        from: e["from"].as_str().unwrap_or_default().to_string(),
        to: e["to"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// Sends one due item: claims it (durably `sending`), runs one SMTP session and records
/// the outcome. `None` when it is not due any more (undone, discarded, claimed elsewhere).
pub async fn send_one<S: SmtpSubmit>(
    store: &Store,
    smtp: &S,
    config: &SmtpConfig,
    login: Result<SmtpLogin, SubmitError>,
    id: OutboxId,
    now: i64,
) -> Result<Option<Transition>, StoreError> {
    if !store.claim_outbox(id, now).await? {
        return Ok(None);
    }
    let item = store.get_outbox(id).await?;
    let result = match login {
        Ok(login) => {
            let raw = store.outbox_raw(id).await?;
            smtp.submit(config, &login, &envelope_of(&item), &raw).await
        }
        Err(e) => Err(e),
    };
    if let Err(e) = &result {
        tracing::warn!(outbox_id = id, error = %e, "send did not go through");
    }
    let (state, send_after, error) = next_state(&result, item.attempts, now);
    store
        .set_outbox(id, state, send_after, result.is_err(), error)
        .await?;
    let item = store.get_outbox(id).await?;
    Ok(Some(Transition { item, state }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mail_smtp::SmtpError;

    #[test]
    fn transitions_follow_section_7() {
        let transient = Err(SubmitError::Transient(SmtpError::Unavailable("451".into())));
        let waits: Vec<Option<i64>> = (0..5).map(|a| next_state(&transient, a, 1000).1).collect();
        assert_eq!(
            waits,
            vec![Some(1030), Some(1120), Some(1600), Some(4600), Some(4600)]
        );
        assert_eq!(next_state(&transient, 4, 0).0, OutboxState::RetryWait);
        assert_eq!(
            next_state(&transient, 5, 0).0,
            OutboxState::Failed,
            "the sixth failure"
        );
        assert_eq!(next_state(&Ok(()), 3, 0).0, OutboxState::Sent);
        assert_eq!(
            next_state(&Err(SubmitError::Ambiguous), 0, 0).0,
            OutboxState::Unknown
        );
        let rejected = next_state(
            &Err(SubmitError::RecipientsRejected(vec!["bad@x.test".into()])),
            0,
            0,
        );
        assert_eq!(rejected.0, OutboxState::Failed);
        let err: Value = serde_json::from_str(&rejected.2.unwrap()).unwrap();
        assert_eq!(err["code"], "recipient_rejected");
        assert_eq!(err["addresses"][0], "bad@x.test");
        assert_eq!(
            next_state(&Err(SubmitError::TooLarge), 0, 0).0,
            OutboxState::Failed
        );
        assert_eq!(
            next_state(&Err(SubmitError::AuthFailed), 0, 0).0,
            OutboxState::Failed
        );
    }
}
