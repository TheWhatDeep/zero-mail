//! `message.unsubscribe` (section 18.7). The `List-Unsubscribe` header lists URIs in angle
//! brackets. Preference order: RFC 8058 one-click (an `https` URI plus
//! `List-Unsubscribe-Post: List-Unsubscribe=One-Click`), which Core POSTs with no cookies;
//! then `mailto:`, which becomes a prefilled draft; then `https:`, which the app opens in
//! the system browser. Nothing fires without the user's click (the UI confirms first).

use mail_imap::ImapConnector;
use mail_smtp::SmtpCheck;
use mail_store::MessageId;
use serde::Serialize;

use crate::compose::{DraftContent, DraftView};
use crate::core::Core;
use crate::error::{AppError, ErrorCode};
use crate::views::AddressView;

/// The RFC 8058 POST body.
pub const ONE_CLICK_BODY: &[u8] = b"List-Unsubscribe=One-Click";
/// Longest URI considered.
const MAX_URI: usize = 2048;

/// How a message unsubscribes, in preference order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    OneClick,
    Mailto,
    Https,
}

/// The usable targets of one message's headers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Targets {
    pub https: Option<String>,
    pub mailto: Option<String>,
    /// `List-Unsubscribe-Post` asks for one-click.
    pub one_click: bool,
}

impl Targets {
    /// Reads `List-Unsubscribe` and `List-Unsubscribe-Post`. Only `https` and `mailto`
    /// URIs count; the first of each kind is used.
    pub fn parse(header: &str, post: Option<&str>) -> Self {
        let mut out = Self::default();
        let mut rest = header;
        while let Some(start) = rest.find('<') {
            let Some(len) = rest[start + 1..].find('>') else {
                break;
            };
            let uri = rest[start + 1..start + 1 + len].trim();
            rest = &rest[start + 1 + len + 1..];
            if uri.len() > MAX_URI || uri.chars().any(|c| c.is_control() || c == ' ') {
                continue;
            }
            match mail_render::urls::scheme(uri).as_deref() {
                Some("https") if out.https.is_none() && uri.len() > "https://".len() => {
                    out.https = Some(uri.to_string());
                }
                Some("mailto") if out.mailto.is_none() && uri.len() > "mailto:".len() => {
                    out.mailto = Some(uri.to_string());
                }
                _ => {}
            }
        }
        out.one_click = post.is_some_and(|p| {
            p.split(',')
                .any(|v| v.trim().eq_ignore_ascii_case("List-Unsubscribe=One-Click"))
        });
        out
    }

    pub fn method(&self) -> Option<Method> {
        if self.one_click && self.https.is_some() {
            Some(Method::OneClick)
        } else if self.mailto.is_some() {
            Some(Method::Mailto)
        } else if self.https.is_some() {
            Some(Method::Https)
        } else {
            None
        }
    }
}

/// What `message.unsubscribe` did. For `mailto` the app opens the draft's compose window;
/// for `https` it opens `url` in the system browser.
#[derive(Debug, Clone, Serialize)]
pub struct Outcome {
    pub method: Method,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub draft: Option<DraftView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// A `mailto:` URI's address, subject and body (RFC 6068).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Mailto {
    pub to: Vec<String>,
    pub subject: String,
    pub body: String,
}

pub fn parse_mailto(uri: &str) -> Mailto {
    let rest = uri.get("mailto:".len()..).unwrap_or_default();
    let (to, query) = rest.split_once('?').unwrap_or((rest, ""));
    let decode = mail_render::urls::decode;
    let mut out = Mailto {
        to: decode(to)
            .split(',')
            .map(|a| a.trim().to_string())
            .filter(|a| a.contains('@'))
            .collect(),
        ..Mailto::default()
    };
    for pair in query.split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        match k.to_ascii_lowercase().as_str() {
            "subject" => out.subject = decode(v),
            "body" => out.body = decode(v),
            "to" => out.to.extend(
                decode(v)
                    .split(',')
                    .map(|a| a.trim().to_string())
                    .filter(|a| a.contains('@')),
            ),
            _ => {}
        }
    }
    out
}

impl<I, S, H> Core<I, S, H>
where
    I: ImapConnector + Clone + 'static,
    S: SmtpCheck + mail_smtp::SmtpSubmit + Clone + 'static,
    H: crate::core::Http,
{
    /// The message's unsubscribe targets, from its stored headers.
    pub(crate) async fn unsubscribe_targets(
        &self,
        message_id: MessageId,
    ) -> Result<Targets, AppError> {
        let (header, post) = self.store.unsubscribe_headers(message_id).await?;
        Ok(header
            .map(|h| Targets::parse(&h, post.as_deref()))
            .unwrap_or_default())
    }

    /// `message.unsubscribe`: takes the preferred path (section 18.7).
    pub async fn message_unsubscribe(&self, message_id: MessageId) -> Result<Outcome, AppError> {
        let message = self.store.get_message(message_id).await?;
        let targets = self.unsubscribe_targets(message_id).await?;
        let method = targets.method().ok_or_else(|| {
            AppError::new(
                ErrorCode::Unsupported,
                "the message has no unsubscribe link",
            )
        })?;
        match (method, &targets.https, &targets.mailto) {
            (Method::OneClick, Some(url), _) => {
                // No cookie store and no referrer: the POST carries only the RFC 8058 body.
                let reply = self
                    .deps
                    .logins
                    .http()
                    .post_form(url, ONE_CLICK_BODY.to_vec())
                    .await
                    .map_err(|e| AppError::new(ErrorCode::Network, e.0))?;
                if !(200..300).contains(&reply.status) {
                    return Err(AppError::new(
                        ErrorCode::ServerRejected,
                        format!("the list answered {}", reply.status),
                    ));
                }
                Ok(Outcome {
                    method,
                    draft: None,
                    url: None,
                })
            }
            (Method::Mailto, _, Some(uri)) => {
                let m = parse_mailto(uri);
                if m.to.is_empty() {
                    return Err(AppError::invalid("the unsubscribe address is not valid"));
                }
                let subject = if m.subject.trim().is_empty() {
                    "unsubscribe".to_string()
                } else {
                    m.subject
                };
                let content = DraftContent {
                    to: m
                        .to
                        .into_iter()
                        .map(|addr| AddressView {
                            name: String::new(),
                            addr,
                        })
                        .collect(),
                    subject,
                    html: if m.body.is_empty() {
                        String::new()
                    } else {
                        format!("<p>{}</p>", mail_render::escape(&m.body))
                    },
                    text: m.body,
                    mode: "new".into(),
                    ..DraftContent::default()
                };
                let draft = self.new_draft(message.account_id, content).await?;
                Ok(Outcome {
                    method,
                    draft: Some(draft),
                    url: None,
                })
            }
            (_, Some(url), _) => Ok(Outcome {
                method: Method::Https,
                draft: None,
                url: Some(url.clone()),
            }),
            _ => Err(AppError::new(
                ErrorCode::Unsupported,
                "the message has no unsubscribe link",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preference_order() {
        let both = "<mailto:u@list.org?subject=stop>, <https://list.org/u?id=1>";
        let t = Targets::parse(both, Some("List-Unsubscribe=One-Click"));
        assert_eq!(t.method(), Some(Method::OneClick));
        assert_eq!(t.https.as_deref(), Some("https://list.org/u?id=1"));
        assert_eq!(Targets::parse(both, None).method(), Some(Method::Mailto));
        let web = Targets::parse("<https://list.org/u>", None);
        assert_eq!(web.method(), Some(Method::Https));
        // One-click needs an https URI.
        let t = Targets::parse("<mailto:u@list.org>", Some("List-Unsubscribe=One-Click"));
        assert_eq!(t.method(), Some(Method::Mailto));
    }

    #[test]
    fn unsafe_and_odd_uris_are_ignored() {
        for h in [
            "<http://list.org/u>",
            "<javascript:alert(1)>",
            "<https://list.org/a b>",
            "https://list.org/no-brackets",
            "<https://",
            "",
        ] {
            assert_eq!(Targets::parse(h, None).method(), None, "{h}");
        }
    }

    #[test]
    fn mailto_fields() {
        let m = parse_mailto("mailto:u%40list.org?subject=Remove%20me&body=please&cc=x@y.z");
        assert_eq!(m.to, vec!["u@list.org".to_string()]);
        assert_eq!(m.subject, "Remove me");
        assert_eq!(m.body, "please");
        assert!(parse_mailto("mailto:nobody").to.is_empty());
    }
}
