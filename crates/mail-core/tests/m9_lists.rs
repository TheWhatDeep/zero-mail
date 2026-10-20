//! M9 on the fakes: Group by conversation and the thread view (section 10, rules 5 and 6),
//! All Inboxes (section 11), T25's unified, search and thread views (SPEC_GAPS G60), T42,
//! and Search server too adding uncached hits (T30's server half; the 100,000-message
//! timing is in mail-search's tests).
#![cfg(test)]

mod common;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::{config, FakeHttp, TempDir};
use mail_core::search::SearchRequest;
use mail_core::views::{ListRequest, RowView, Scope};
use mail_core::{Core, CoreConfig, CoreEvent, EventSink, StateName};
use mail_imap::fake::{FakeMessage, FakeServer};
use mail_imap::{Flag, SpecialUse};
use mail_secrets::{MemoryStore, SecretStore};
use mail_smtp::fake::FakeSmtp;

const USER: &str = "alice@test.local";
const PASS: &str = "alice-pw";
const DAY: i64 = 86_400;

type TestCore = Core<FakeServer, FakeSmtp, FakeHttp>;

struct Setup {
    _dir: TempDir,
    imap: FakeServer,
    events: Arc<Mutex<Vec<CoreEvent>>>,
    core: Arc<TestCore>,
    account: i64,
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// A raw message. `extra` holds header lines; `attach` adds a PDF attachment; `pad` makes
/// the text body that many bytes long.
struct Mail<'a> {
    id: &'a str,
    from: &'a str,
    subject: &'a str,
    age_days: i64,
    extra: &'a str,
    attach: bool,
    pad: usize,
}

impl Default for Mail<'_> {
    fn default() -> Self {
        Self {
            id: "x@t",
            from: "Bob <bob@test.local>",
            subject: "Hello",
            age_days: 1,
            extra: "",
            attach: false,
            pad: 0,
        }
    }
}

fn mail(m: Mail<'_>) -> FakeMessage {
    let text = format!("Body of {}{}", m.subject, "x".repeat(m.pad));
    let head = format!(
        "Message-ID: <{}>\r\nFrom: {}\r\nTo: {USER}\r\nSubject: {}\r\n{}MIME-Version: 1.0\r\n",
        m.id, m.from, m.subject, m.extra
    );
    let raw = if m.attach {
        format!(
            "{head}Content-Type: multipart/mixed; boundary=\"b1\"\r\n\r\n--b1\r\n\
             Content-Type: text/plain; charset=utf-8\r\n\r\n{text}\r\n--b1\r\n\
             Content-Type: application/pdf; name=\"plan.pdf\"\r\n\
             Content-Disposition: attachment; filename=\"plan.pdf\"\r\n\
             Content-Transfer-Encoding: base64\r\n\r\nJVBERi0xLjQK\r\n--b1--\r\n"
        )
    } else {
        format!("{head}Content-Type: text/plain; charset=utf-8\r\n\r\n{text}\r\n")
    };
    FakeMessage::mime(raw, now() - m.age_days * DAY - 60)
}

const FULL: &[&str] = &["IMAP4rev1", "IDLE", "MOVE", "UIDPLUS", "SPECIAL-USE"];

async fn start(imap: FakeServer) -> Setup {
    let dir = TempDir::new("m9");
    let smtp = FakeSmtp::default();
    smtp.add_user(USER, PASS);
    smtp.add_user("bob@test.local", "bob-pw");
    let events: Arc<Mutex<Vec<CoreEvent>>> = Arc::default();
    let log = Arc::clone(&events);
    let sink: EventSink = Arc::new(move |e| log.lock().unwrap().push(e));
    let core = Core::start(
        CoreConfig {
            data_dir: dir.path(),
            secrets: Arc::new(MemoryStore::default()) as Arc<dyn SecretStore>,
            imap: imap.clone(),
            smtp,
            http: FakeHttp::default(),
        },
        sink,
    )
    .await
    .unwrap();
    let account = core.account_add(config(USER, USER, PASS)).await.unwrap().id;
    let s = Setup {
        _dir: dir,
        imap,
        events,
        core,
        account,
    };
    s.until_idle(account).await;
    s
}

async fn wait_for(what: &str, mut check: impl AsyncFnMut() -> bool) {
    let start = Instant::now();
    loop {
        if check().await {
            return;
        }
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "timed out: {what}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

impl Setup {
    async fn until_idle(&self, account: i64) {
        let events = Arc::clone(&self.events);
        wait_for("idle", async || {
            events.lock().unwrap().iter().any(|e| {
                matches!(e, CoreEvent::AccountState { account_id, state: StateName::Idle, .. } if *account_id == account)
            })
        })
        .await;
    }

    async fn folder(&self, path: &str) -> i64 {
        self.core
            .folder_list(self.account)
            .await
            .unwrap()
            .iter()
            .find(|f| f.path == path)
            .unwrap()
            .id
    }

    async fn page(&self, scope: Scope, group: bool) -> Vec<RowView> {
        self.core
            .list_page(ListRequest {
                scope,
                cursor: None,
                limit: 500,
                group_by_thread: group,
            })
            .await
            .unwrap()
            .rows
    }

    async fn folder_rows(&self, path: &str, group: bool) -> Vec<RowView> {
        let folder_id = self.folder(path).await;
        self.page(Scope::Folder { folder_id }, group).await
    }

    /// Waits until every listed folder holds `n` cached rows.
    async fn wait_rows(&self, path: &str, n: usize) {
        let start = Instant::now();
        loop {
            let rows = self.folder_rows(path, false).await;
            if rows.len() == n {
                return;
            }
            assert!(
                start.elapsed() < Duration::from_secs(20),
                "{path}: {:?}",
                subjects(&rows)
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    async fn search(&self, text: &str, server_too: bool) -> Vec<String> {
        let mut subjects: Vec<String> = self
            .core
            .search_query(SearchRequest {
                text: text.into(),
                account_id: Some(self.account),
                server_too,
                cursor: None,
                limit: 500,
            })
            .await
            .unwrap()
            .rows
            .into_iter()
            .map(|r| r.subject)
            .collect();
        subjects.sort();
        subjects
    }
}

fn subjects(rows: &[RowView]) -> Vec<&str> {
    rows.iter().map(|r| r.subject.as_str()).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn threads_group_in_a_folder_and_the_thread_view_spans_folders() {
    let imap = FakeServer::new(FULL);
    imap.add_user(USER, PASS);
    imap.add_folder("Sent", Some(SpecialUse::Sent));
    imap.add_folder("Trash", Some(SpecialUse::Trash));
    let deliver = |folder: &str, m: Mail<'_>| imap.deliver(folder, mail(m)).unwrap();
    deliver(
        "INBOX",
        Mail {
            id: "root@t",
            subject: "Plan",
            age_days: 5,
            ..Mail::default()
        },
    );
    deliver(
        "Sent",
        Mail {
            id: "mine@t",
            from: USER,
            subject: "Re: Plan",
            age_days: 4,
            extra: "In-Reply-To: <root@t>\r\nReferences: <root@t>\r\n",
            ..Mail::default()
        },
    );
    deliver(
        "INBOX",
        Mail {
            id: "carol@t",
            from: "Carol <carol@test.local>",
            subject: "Re: Plan",
            age_days: 3,
            extra: "In-Reply-To: <mine@t>\r\nReferences: <root@t> <mine@t>\r\n",
            ..Mail::default()
        },
    );
    deliver(
        "Trash",
        Mail {
            id: "binned@t",
            from: "Dave <dave@test.local>",
            subject: "Re: Plan",
            age_days: 2,
            extra: "References: <root@t>\r\n",
            ..Mail::default()
        },
    );
    deliver(
        "INBOX",
        Mail {
            id: "other@t",
            subject: "Unrelated",
            age_days: 4,
            ..Mail::default()
        },
    );
    let s = start(imap).await;
    s.wait_rows("INBOX", 3).await;
    s.wait_rows("Sent", 1).await;
    s.wait_rows("Trash", 1).await;
    // Mark the root read on the server and let the flags arrive.
    s.imap.set_flags("INBOX", 1, vec![Flag::Seen]).unwrap();
    wait_for("seen", async || {
        s.folder_rows("INBOX", false)
            .await
            .iter()
            .any(|r| r.subject == "Plan" && !r.unread)
    })
    .await;

    // Group by conversation: one row per thread, by its latest message in INBOX.
    let rows = s.folder_rows("INBOX", true).await;
    assert_eq!(subjects(&rows), vec!["Re: Plan", "Unrelated"]);
    let t = rows[0].thread.clone().unwrap();
    assert_eq!(t.messages, 3, "INBOX twice and Sent; Trash is left out");
    assert_eq!(t.unread, 2);
    assert_eq!(t.participants, vec!["Bob", USER, "Carol"]);
    assert_eq!(rows[1].thread.as_ref().unwrap().messages, 1);

    // The thread view: every folder but Trash, Junk and Drafts, oldest first.
    let thread_id = rows[0].thread_id;
    let inbox = s.folder("INBOX").await;
    let view = s
        .page(
            Scope::Thread {
                thread_id,
                folder_id: Some(inbox),
            },
            false,
        )
        .await;
    assert_eq!(subjects(&view), vec!["Plan", "Re: Plan", "Re: Plan"]);
    assert!(view.windows(2).all(|w| w[0].date_utc <= w[1].date_utc));
    // Opened from Trash, Trash is included.
    let trash = s.folder("Trash").await;
    let from_trash = s
        .page(
            Scope::Thread {
                thread_id,
                folder_id: Some(trash),
            },
            false,
        )
        .await;
    assert_eq!(from_trash.len(), 4);
    assert_eq!(
        s.folder_rows("Trash", true).await[0]
            .thread
            .as_ref()
            .unwrap()
            .messages,
        4
    );
    s.core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn all_inboxes_lists_every_account_s_inbox_newest_first() {
    let imap = FakeServer::new(FULL);
    imap.add_user(USER, PASS);
    imap.add_user("bob@test.local", "bob-pw");
    imap.add_folder("Archive", Some(SpecialUse::Archive));
    for (i, age) in [3, 1, 2].iter().enumerate() {
        imap.deliver(
            "INBOX",
            mail(Mail {
                id: &format!("u{i}@t"),
                subject: &format!("Inbox {i}"),
                age_days: *age,
                ..Mail::default()
            }),
        )
        .unwrap();
    }
    imap.deliver(
        "Archive",
        mail(Mail {
            id: "arch@t",
            subject: "Archived",
            ..Mail::default()
        }),
    )
    .unwrap();
    let s = start(imap).await;
    // A second account on the same fake server sees the same INBOX.
    let bob = s
        .core
        .account_add(config("bob@test.local", "bob@test.local", "bob-pw"))
        .await
        .unwrap()
        .id;
    s.until_idle(bob).await;
    s.wait_rows("Archive", 1).await;
    wait_for("both inboxes", async || {
        s.page(Scope::Unified, false).await.len() == 6
    })
    .await;
    let rows = s.page(Scope::Unified, false).await;
    assert!(rows.iter().all(|r| r.subject.starts_with("Inbox")));
    assert!(rows.windows(2).all(|w| w[0].date_utc >= w[1].date_utc));
    assert_eq!(rows.iter().filter(|r| r.account_id == bob).count(), 3);
    assert_eq!(rows[0].subject, "Inbox 1");
    // Grouped: one thread per message per account.
    assert_eq!(s.page(Scope::Unified, true).await.len(), 6);
    // A disabled account leaves All Inboxes.
    s.core.account_set_enabled(bob, false).await.unwrap();
    assert_eq!(s.page(Scope::Unified, false).await.len(), 3);
    s.core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t25_one_entry_in_unified_search_and_thread_views() {
    let imap = FakeServer::new(&[
        "IMAP4rev1",
        "IDLE",
        "MOVE",
        "UIDPLUS",
        "SPECIAL-USE",
        "X-GM-EXT-1",
    ]);
    imap.add_user(USER, PASS);
    imap.add_folder("[Gmail]/All Mail", Some(SpecialUse::All));
    imap.add_folder("Work", None);
    let m = mail(Mail {
        id: "labels@t",
        subject: "Two labels",
        ..Mail::default()
    })
    .with_gmail_ids(77, 77);
    for folder in ["[Gmail]/All Mail", "INBOX", "Work"] {
        imap.deliver(folder, m.clone()).unwrap();
    }
    let s = start(imap).await;
    s.wait_rows("INBOX", 1).await;
    s.wait_rows("Work", 1).await;

    assert_eq!(s.page(Scope::Unified, false).await.len(), 1);
    assert_eq!(s.page(Scope::Unified, true).await.len(), 1);
    assert_eq!(s.search("labels", false).await, vec!["Two labels"]);
    let row = &s.folder_rows("Work", true).await[0];
    assert_eq!(row.thread.as_ref().unwrap().messages, 1, "counted once");
    let view = s
        .page(
            Scope::Thread {
                thread_id: row.thread_id,
                folder_id: Some(s.folder("Work").await),
            },
            false,
        )
        .await;
    assert_eq!(view.len(), 1);
    assert_eq!(
        view[0].folder_id,
        s.folder("Work").await,
        "the open folder's row"
    );
    s.core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t42_and_search_server_too_adds_uncached_hits() {
    let imap = FakeServer::new(FULL);
    imap.add_user(USER, PASS);
    imap.add_folder("Trash", Some(SpecialUse::Trash));
    let big = 1_200_000;
    let put =
        |folder: &str, id: &str, from: &str, subject: &str, age: i64, pad: usize, attach: bool| {
            imap.deliver(
                folder,
                mail(Mail {
                    id,
                    from,
                    subject,
                    age_days: age,
                    pad,
                    attach,
                    ..Mail::default()
                }),
            )
            .unwrap();
        };
    // Outside the window, and older UIDs: on the server only, until Search server too
    // fetches them.
    put(
        "INBOX",
        "8@t",
        "a@x.test",
        "big ancient from a",
        200,
        big,
        false,
    );
    put(
        "INBOX",
        "9@t",
        "alice@x.test",
        "ancient budget",
        200,
        10,
        true,
    );
    // Within the 90-day sync window: cached by the first sync.
    put("INBOX", "1@t", "a@x.test", "big old from a", 40, big, false);
    put("INBOX", "2@t", "b@x.test", "big old from b", 45, big, false);
    put(
        "INBOX",
        "3@t",
        "b@x.test",
        "big old newsletter",
        50,
        big,
        false,
    );
    put("INBOX", "4@t", "c@x.test", "big old from c", 40, big, false);
    put(
        "INBOX",
        "5@t",
        "a@x.test",
        "small old from a",
        40,
        10,
        false,
    );
    put("INBOX", "6@t", "b@x.test", "big new from b", 10, big, false);
    put("Trash", "7@t", "a@x.test", "big old binned", 40, big, false);
    let s = start(imap).await;
    s.wait_rows("INBOX", 6).await;

    let t42 = "larger:1m older_than:30d -newsletter from:a OR from:b";
    assert_eq!(
        s.search(t42, false).await,
        vec!["big old from a", "big old from b"]
    );
    assert_eq!(
        s.search(t42, true).await,
        vec!["big ancient from a", "big old from a", "big old from b"],
        "the uncached hit is fetched; Trash stays out"
    );
    // T30's query: has:attachment is not sent to the server; the hit is narrowed locally.
    assert_eq!(
        s.search("from:alice has:attachment budget", true).await,
        vec!["ancient budget"]
    );
    assert_eq!(s.folder_rows("INBOX", false).await.len(), 8);
    s.core.shutdown().await;
}
