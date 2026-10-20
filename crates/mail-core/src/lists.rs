//! `list.page` (section 3): one folder, All Inboxes (section 11), a thread (section 10,
//! rule 6) and search, with or without Group by conversation (rule 5).

use mail_imap::ImapConnector;
use mail_smtp::SmtpCheck;
use mail_store::{Folder, FolderId, SpecialUse};

use crate::core::Core;
use crate::error::AppError;
use crate::views::{ListRequest, PageView, RowView, Scope, ThreadInfo};

/// Trash, Junk and Drafts: left out of thread views unless one of them is open (rule 6).
fn hidden_from_threads(f: &Folder) -> bool {
    matches!(
        f.special_use,
        Some(SpecialUse::Trash | SpecialUse::Junk | SpecialUse::Drafts)
    )
}

impl<I, S, H> Core<I, S, H>
where
    I: ImapConnector + Clone + 'static,
    S: SmtpCheck + mail_smtp::SmtpSubmit + Clone + 'static,
    H: crate::core::Http,
{
    /// Every account's folders.
    async fn all_folders(&self) -> Result<Vec<Folder>, AppError> {
        let mut out = Vec::new();
        for a in self.store.list_accounts().await? {
            out.extend(self.store.list_folders(a.id).await?);
        }
        Ok(out)
    }

    /// The folders a thread's counts and view leave out, given the open folder.
    async fn thread_exclusions(&self, open: Option<FolderId>) -> Result<Vec<FolderId>, AppError> {
        Ok(self
            .all_folders()
            .await?
            .iter()
            .filter(|f| hidden_from_threads(f) && Some(f.id) != open)
            .map(|f| f.id)
            .collect())
    }

    /// `list.page`. A first page of a folder (no cursor) also tells the actor the folder is
    /// open, so it syncs now and every 5 minutes (section 6, step 9).
    pub async fn list_page(&self, request: ListRequest) -> Result<PageView, AppError> {
        let limit = request.limit.clamp(1, 500);
        let cursor = request.cursor.map(Into::into);
        let (folders, open) = match request.scope {
            Scope::Folder { folder_id } => {
                if request.cursor.is_none() {
                    let folder = self.store.get_folder(folder_id).await?;
                    self.with_actor(folder.account_id, |h| h.open_folder(Some(folder_id)))
                        .await;
                }
                (vec![folder_id], Some(folder_id))
            }
            Scope::Unified => {
                let accounts = self.store.list_accounts().await?;
                let mut inboxes = Vec::new();
                for a in accounts.iter().filter(|a| a.config.enabled) {
                    inboxes.extend(
                        self.store
                            .list_folders(a.id)
                            .await?
                            .into_iter()
                            .filter(|f| f.special_use == Some(SpecialUse::Inbox))
                            .map(|f| f.id),
                    );
                }
                (inboxes, None)
            }
            Scope::Thread {
                thread_id,
                folder_id,
            } => {
                let exclude = self.thread_exclusions(folder_id).await?;
                let rows = self
                    .store
                    .thread_messages(thread_id, exclude, folder_id)
                    .await?;
                return Ok(PageView {
                    rows: rows.iter().map(RowView::from).collect(),
                    next: None,
                    unindexed: None,
                });
            }
            Scope::Search { query, account_id } => {
                return self.search_page(&query, account_id, cursor, limit).await
            }
        };
        if request.group_by_thread {
            let exclude = self.thread_exclusions(open).await?;
            let page = self
                .store
                .threads_page(folders, exclude, cursor, limit)
                .await?;
            return Ok(PageView {
                rows: page
                    .rows
                    .iter()
                    .map(|t| RowView {
                        thread: Some(ThreadInfo {
                            messages: t.messages,
                            unread: t.unread,
                            participants: t.participants.clone(),
                        }),
                        ..RowView::from(&t.row)
                    })
                    .collect(),
                next: page.next.map(Into::into),
                unindexed: None,
            });
        }
        let page = match (open, folders.as_slice()) {
            (Some(_), [one]) => self.store.folder_page(*one, cursor, limit).await?,
            _ => self.store.folders_page(folders, cursor, limit).await?,
        };
        Ok(PageView {
            rows: page.rows.iter().map(RowView::from).collect(),
            next: page.next.map(Into::into),
            unindexed: None,
        })
    }
}
