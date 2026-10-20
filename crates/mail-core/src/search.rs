//! `search.query` and `list.page` with a search scope (section 10): local-first full-text
//! search over everything cached, and Search server too on request.

use mail_imap::ImapConnector;
use mail_search::{Context, SERVER_CAP};
use mail_smtp::SmtpCheck;
use mail_store::{AccountId, Cursor};
use mail_sync::ServerSearch;
use serde::Deserialize;

use crate::core::Core;
use crate::error::AppError;
use crate::views::{CursorView, PageView, RowView};

/// `search.query` arguments (section 3).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SearchRequest {
    pub text: String,
    /// The current account; `None` widens the search to every account (section 10, Scope).
    #[serde(default)]
    pub account_id: Option<AccountId>,
    #[serde(default)]
    pub server_too: bool,
    #[serde(default)]
    pub cursor: Option<CursorView>,
    pub limit: u32,
}

impl<I, S, H> Core<I, S, H>
where
    I: ImapConnector + Clone + 'static,
    S: SmtpCheck + mail_smtp::SmtpSubmit + Clone + 'static,
    H: crate::core::Http,
{
    async fn search_context(&self, account: Option<AccountId>) -> Result<Context, AppError> {
        let mut accounts = Vec::new();
        let mut folders = Vec::new();
        for a in self.store.list_accounts().await? {
            folders.extend(self.store.list_folders(a.id).await?);
            accounts.push((a.id, a.config.email.clone()));
        }
        Ok(Context {
            accounts,
            folders,
            account,
            now: crate::accounts::now(),
        })
    }

    /// One page of local results, newest first, with the count of messages in scope whose
    /// body is not indexed yet (first page only).
    pub(crate) async fn search_page(
        &self,
        text: &str,
        account: Option<AccountId>,
        cursor: Option<Cursor>,
        limit: u32,
    ) -> Result<PageView, AppError> {
        let query = mail_search::parse(text);
        if query.is_empty() {
            return Ok(PageView {
                rows: Vec::new(),
                next: None,
                unindexed: None,
            });
        }
        let ctx = self.search_context(account).await?;
        let page = self
            .store
            .query_page(mail_search::local(&query, &ctx), cursor, limit)
            .await?;
        let unindexed = match cursor {
            None => Some(
                self.store
                    .query_count(mail_search::unindexed(&query, &ctx))
                    .await?,
            ),
            Some(_) => None,
        };
        Ok(PageView {
            rows: page.rows.iter().map(RowView::from).collect(),
            next: page.next.map(Into::into),
            unindexed,
        })
    }

    /// `search.query`. With `server_too`, each account in scope first searches its server
    /// (`UID SEARCH`, or `X-GM-RAW` on Gmail) and stores the hits it did not have, at most
    /// 500 in all; the local search then lists them with everything else. An account that
    /// is offline adds nothing.
    pub async fn search_query(&self, request: SearchRequest) -> Result<PageView, AppError> {
        let query = mail_search::parse(&request.text);
        if request.server_too && !query.is_empty() {
            let ctx = self.search_context(request.account_id).await?;
            let key = mail_search::server_key(&query, ctx.now);
            let raw = mail_search::gmail_raw(&request.text);
            let mut remaining = SERVER_CAP;
            for account in mail_search::accounts(&query, &ctx) {
                if remaining == 0 {
                    break;
                }
                let search = ServerSearch {
                    folders: mail_search::server_folders(&query, &ctx, account),
                    key: key.clone(),
                    gmail_raw: (!raw.trim().is_empty()).then(|| raw.clone()),
                    cap: remaining,
                };
                let rx = self
                    .actors
                    .lock()
                    .await
                    .get(&account)
                    .map(|h| h.server_search(search));
                if let Some(rx) = rx {
                    match rx.await {
                        Ok(Ok(n)) => remaining = remaining.saturating_sub(n),
                        Ok(Err(e)) => {
                            tracing::warn!(account_id = account, error = ?crate::core::sync_error(&e).code, "server search failed");
                        }
                        Err(_) => {}
                    }
                }
            }
        }
        self.search_page(
            &request.text,
            request.account_id,
            request.cursor.map(Into::into),
            request.limit.clamp(1, 500),
        )
        .await
    }
}
