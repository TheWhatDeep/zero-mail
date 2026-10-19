//! M5 on the fakes: T19 (Core half), T20, T22, T23 and T24, plus the plain-text toggle,
//! format=flowed, Could not display this message and Save source.
#![cfg(test)]

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{config, FakeHttp, TempDir};
use mail_core::views::{BodyKind, MessageView, RemoteImages};
use mail_core::{Core, CoreConfig, CoreEvent, ErrorCode, EventSink, StateName};
use mail_imap::fake::{FakeMessage, FakeServer, Op};
use mail_imap::BodyPart;
use mail_secrets::{MemoryStore, SecretStore};
use mail_smtp::fake::FakeSmtp;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const USER: &str = "alice@test.local";
const PASS: &str = "alice-pw";

type TestCore = Core<FakeServer, FakeSmtp, FakeHttp>;

struct Setup {
    dir: TempDir,
    imap: FakeServer,
    secrets: Arc<dyn SecretStore>,
    events: Arc<Mutex<Vec<CoreEvent>>>,
    core: Arc<TestCore>,
    account: i64,
}

async fn start(dir: TempDir, imap: FakeServer, add: bool) -> Setup {
    restart(dir, imap, Arc::new(MemoryStore::default()), add).await
}

async fn restart(
    dir: TempDir,
    imap: FakeServer,
    secrets: Arc<dyn SecretStore>,
    add: bool,
) -> Setup {
    let smtp = FakeSmtp::default();
    smtp.add_user(USER, PASS);
    let events: Arc<Mutex<Vec<CoreEvent>>> = Arc::default();
    let log = Arc::clone(&events);
    let sink: EventSink = Arc::new(move |e| log.lock().unwrap().push(e));
    let core = Core::start(
        CoreConfig {
            data_dir: dir.path(),
            secrets: Arc::clone(&secrets),
            imap: imap.clone(),
            smtp,
            http: FakeHttp::default(),
        },
        sink,
    )
    .await
    .unwrap();
    let account = if add {
        core.account_add(config(USER, USER, PASS)).await.unwrap().id
    } else {
        core.account_list().await.unwrap()[0].summary.id
    };
    let s = Setup {
        dir,
        imap,
        secrets,
        events,
        core,
        account,
    };
    until(&s, |e| {
        matches!(e, CoreEvent::AccountState { account_id, state: StateName::Idle, .. } if *account_id == account)
    })
    .await;
    s
}

async fn until(s: &Setup, mut pred: impl FnMut(&CoreEvent) -> bool) {
    let start = std::time::Instant::now();
    loop {
        if s.events.lock().unwrap().iter().any(&mut pred) {
            return;
        }
        assert!(start.elapsed() < Duration::from_secs(30), "event not seen");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn server() -> FakeServer {
    let s = FakeServer::new(&["IMAP4rev1", "IDLE", "SPECIAL-USE", "UIDPLUS"]);
    s.add_user(USER, PASS);
    s
}

/// The INBOX message with this Message-ID, once synced.
async fn message_id(s: &Setup, mid: &str) -> i64 {
    let folders = s.core.folder_list(s.account).await.unwrap();
    let inbox = folders[0].id;
    let start = std::time::Instant::now();
    loop {
        let page = s
            .core
            .list_page(mail_core::views::ListRequest {
                scope: mail_core::views::Scope::Folder { folder_id: inbox },
                cursor: None,
                limit: 500,
                group_by_thread: false,
            })
            .await
            .unwrap();
        // Rows carry no Message-ID; the subject is the test's key.
        if let Some(row) = page.rows.iter().find(|r| r.subject == mid) {
            return row.id;
        }
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "{mid} not synced"
        );
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
}

/// Opens until the body is no longer loading.
async fn open(s: &Setup, id: i64) -> MessageView {
    let start = std::time::Instant::now();
    loop {
        let view = s.core.message_open(id).await.unwrap();
        if view.body_kind != BodyKind::Loading {
            return view;
        }
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "body never arrived"
        );
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
}

async fn body_html(s: &Setup, id: i64, query: &str) -> (String, String) {
    let r = s.core.mail_body(id, query).await.unwrap();
    assert_eq!(r.status, 200);
    (String::from_utf8(r.body).unwrap(), r.csp)
}

/// A message with `subject` as its key; `body` is everything after the headers given.
fn mime(subject: &str, from: &str, headers: &str, body: &[u8]) -> FakeMessage {
    let mut raw = format!(
        "Message-ID: <{}@t>\r\nFrom: {from}\r\nTo: {USER}\r\nSubject: {subject}\r\nMIME-Version: 1.0\r\n{headers}\r\n",
        subject.replace(' ', "-")
    )
    .into_bytes();
    raw.extend_from_slice(body);
    FakeMessage::mime(raw, now() - 60)
}

fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len() * 4 / 3 + bytes.len() / 57 * 2 + 4);
    for (i, chunk) in bytes.chunks(3).enumerate() {
        if i > 0 && i % 19 == 0 {
            out.push_str("\r\n");
        }
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for k in 0..4 {
            if k <= chunk.len() {
                out.push(char::from(T[((n >> (18 - 6 * k)) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t22_every_charset_is_readable_and_nothing_panics() {
    let imap = server();
    let cases: Vec<(&str, &str, Vec<u8>, &str)> = vec![
        (
            "utf8",
            "utf-8",
            "Grüße 日本".as_bytes().to_vec(),
            "Grüße 日本",
        ),
        (
            "latin1",
            "iso-8859-1",
            b"Caf\xe9 cr\xe8me".to_vec(),
            "Café crème",
        ),
        (
            "cp1252",
            "windows-1252",
            b"\x93quoted\x94 \x80".to_vec(),
            "\u{201c}quoted\u{201d} €",
        ),
        (
            "jis",
            "iso-2022-jp",
            b"\x1b$B$3$s$K$A$O\x1b(B".to_vec(),
            "こんにちは",
        ),
        ("gb2312", "gb2312", b"\xd6\xd0\xce\xc4".to_vec(), "中文"),
        ("sjis", "shift_jis", b"\x93\xfa\x96{".to_vec(), "日本"),
        (
            "bogus-utf8",
            "x-bogus",
            "naïve".as_bytes().to_vec(),
            "naïve",
        ),
        ("bogus-8bit", "x-bogus", b"na\xefve".to_vec(), "naïve"),
    ];
    for (subject, charset, bytes, _) in &cases {
        imap.deliver(
            "INBOX",
            mime(
                subject,
                "Bob <bob@test.local>",
                &format!("Content-Type: text/plain; charset={charset}\r\nContent-Transfer-Encoding: 8bit\r\n"),
                bytes,
            ),
        )
        .unwrap();
    }
    // An encoded-word subject in ISO-2022-JP.
    imap.deliver(
        "INBOX",
        FakeMessage::mime(
            b"Message-ID: <jis-subject@t>\r\nFrom: b@test.local\r\nSubject: =?ISO-2022-JP?B?GyRCJDMkcyRLJEEkTxsoQg==?=\r\n\r\nx\r\n".to_vec(),
            now() - 60,
        ),
    )
    .unwrap();
    // Malformed MIME: a multipart without its closing delimiter and a junk part header.
    imap.deliver(
        "INBOX",
        mime(
            "broken",
            "b@test.local",
            "Content-Type: multipart/mixed; boundary=zz\r\n",
            b"--zz\r\nContent-Type: text/plain; charset=\"\r\n\r\nstill readable\r\n--zz\r\nContent-Type: \r\n\r\n\xff\xfe",
        ),
    )
    .unwrap();
    let s = start(TempDir::new("t22"), imap, true).await;
    for (subject, _, _, expected) in &cases {
        let id = message_id(&s, subject).await;
        let view = open(&s, id).await;
        assert_eq!(view.body_kind, BodyKind::Plain, "{subject}");
        let (html, _) = body_html(&s, id, "images=0").await;
        assert!(html.contains(expected), "{subject}: {html}");
    }
    let _ = message_id(&s, "こんにちは").await;
    let broken = message_id(&s, "broken").await;
    let view = open(&s, broken).await;
    assert_eq!(view.body_kind, BodyKind::Plain);
    let (html, _) = body_html(&s, broken, "images=0").await;
    assert!(html.contains("still readable"), "{html}");
    s.core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t23_attachments_are_fetched_on_demand_with_safe_names() {
    let big: Vec<u8> = (0..20 * 1024 * 1024u32).map(|i| (i % 251) as u8).collect();
    let body = format!(
        "--b0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nSee attached.\r\n\
         --b0\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=\"../x\"\r\n\
         Content-Transfer-Encoding: base64\r\n\r\n{}\r\n\
         --b0\r\nContent-Type: text/plain\r\nContent-Disposition: attachment; filename=\"CON.txt\"\r\n\r\ndevice\r\n\
         --b0\r\nContent-Type: application/octet-stream; name=\"=?UTF-8?B?c2V0dXAuZXhl?=\"\r\n\
         Content-Disposition: attachment\r\nContent-Transfer-Encoding: base64\r\n\r\nTVo=\r\n--b0--\r\n",
        base64(&big)
    );
    let imap = server();
    imap.deliver(
        "INBOX",
        mime(
            "files",
            "Bob <bob@test.local>",
            "Content-Type: multipart/mixed; boundary=b0\r\n",
            body.as_bytes(),
        ),
    )
    .unwrap();
    let s = start(TempDir::new("t23"), imap, true).await;
    let id = message_id(&s, "files").await;
    let view = open(&s, id).await;
    let parts = |s: &Setup| -> Vec<String> {
        s.imap
            .calls()
            .into_iter()
            .filter(|c| c.op == Op::FetchPart)
            .map(|c| c.arg)
            .collect()
    };
    assert!(
        !parts(&s).contains(&"2".to_string()),
        "the 20 MB part is never prefetched"
    );
    let names: Vec<(&str, bool)> = view
        .attachments
        .iter()
        .map(|a| (a.filename.as_str(), a.executable))
        .collect();
    assert_eq!(
        names,
        vec![("../x", false), ("CON.txt", false), ("setup.exe", true)]
    );
    let (big_id, con_id, exe_id) = (
        view.attachments[0].id,
        view.attachments[1].id,
        view.attachments[2].id,
    );
    assert_eq!(s.core.attachment_file_name(big_id).await.unwrap(), "..x");
    assert_eq!(
        s.core.attachment_file_name(con_id).await.unwrap(),
        "_CON.txt"
    );

    let target = s.dir.path().join("saved.bin");
    s.core
        .attachment_save(big_id, target.clone())
        .await
        .unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), big, "decoded bytes");
    let fetched = |s: &Setup| parts(s).iter().filter(|p| *p == "2").count();
    assert_eq!(fetched(&s), 1, "fetched on demand, once");
    s.core
        .attachment_save(big_id, target.clone())
        .await
        .unwrap();
    assert_eq!(fetched(&s), 1, "the second save comes from the cache");

    let opened = s.core.attachment_open(exe_id).await.unwrap();
    assert_eq!(std::fs::read(&opened).unwrap(), b"MZ");
    assert_eq!(opened.file_name().unwrap(), "setup.exe");
    let tmp = s.dir.path().join("tmp");
    assert!(
        opened.starts_with(&tmp) && opened.parent() != Some(tmp.as_path()),
        "a random subfolder"
    );
    s.core.shutdown().await;
    assert!(!tmp.exists(), "temp files are deleted at quit");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t24_cid_images_are_served_by_core() {
    let png = b"\x89PNG\r\n\x1a\nfakepng";
    let body = format!(
        "--r\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<p>Logo:</p><img src=\"cid:logo@x\" alt=\"logo\">\r\n\
         --r\r\nContent-Type: image/png\r\nContent-ID: <logo@x>\r\nContent-Disposition: inline\r\n\
         Content-Transfer-Encoding: base64\r\n\r\n{}\r\n\
         --r\r\nContent-Type: text/html\r\nContent-ID: <page@x>\r\n\r\n<script>no</script>\r\n--r--\r\n",
        base64(png)
    );
    let imap = server();
    imap.deliver(
        "INBOX",
        mime(
            "cid",
            "Bob <bob@test.local>",
            "Content-Type: multipart/related; boundary=r\r\n",
            body.as_bytes(),
        ),
    )
    .unwrap();
    let s = start(TempDir::new("t24"), imap, true).await;
    let id = message_id(&s, "cid").await;
    let view = open(&s, id).await;
    assert_eq!(view.body_kind, BodyKind::Html);
    assert_eq!(view.remote_images, RemoteImages::None);
    assert!(view.attachments.is_empty(), "inline images are not listed");
    let (html, csp) = body_html(&s, id, "images=0").await;
    assert!(!html.contains("data:"), "no data: URIs: {html}");
    let marker = "mail-cid";
    let at = html.find(marker).expect("a mail-cid URL");
    let url_start = html[..at].rfind('"').unwrap() + 1;
    let url_end = at + html[at..].find('"').unwrap();
    let url = &html[url_start..url_end];
    let path = url.split_once(".localhost").map_or_else(
        || url.trim_start_matches("mail-cid://localhost"),
        |(_, p)| p,
    );
    assert_eq!(path, format!("/{id}/logo%40x"));
    assert!(csp.contains("img-src mail-cid:"));
    let image = s.core.mail_cid(path).await;
    assert_eq!(
        (image.status, image.content_type.as_str()),
        (200, "image/png")
    );
    assert_eq!(image.body, png);
    // Only images: a cid naming an HTML part is not served.
    assert_eq!(
        s.core.mail_cid(&format!("/{id}/page%40x")).await.status,
        404
    );
    assert_eq!(s.core.mail_cid("/999999/logo%40x").await.status, 404);
    assert_eq!(s.core.mail_cid("/nonsense").await.status, 400);
    s.core.shutdown().await;
}

/// A local image server that records each request head.
async fn image_server() -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = Arc::clone(&seen);
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let mut buf = vec![0; 8192];
            let n = sock.read(&mut buf).await.unwrap_or(0);
            log.lock()
                .unwrap()
                .push(String::from_utf8_lossy(&buf[..n]).into_owned());
            let _ = sock
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: image/gif\r\nContent-Length: 3\r\nSet-Cookie: track=1\r\nConnection: close\r\n\r\nGIF")
                .await;
            let _ = sock.shutdown().await;
        }
    });
    (base, seen)
}

/// What the webview would load: every `mail-remote` URL in the document, as request paths.
fn remote_paths(html: &str) -> Vec<String> {
    html.split('"')
        .filter(|v| v.contains("mail-remote"))
        .map(|v| {
            v.split_once(".localhost")
                .map_or_else(
                    || v.trim_start_matches("mail-remote://localhost"),
                    |(_, p)| p,
                )
                .to_string()
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t20_remote_images_load_only_after_consent_through_the_proxy() {
    let (base, seen) = image_server().await;
    let html = format!(
        "<p>News</p><img src=\"{base}/logo.png\"><img src=\"{base}/pixel.gif?u=42\" width=\"1\" height=\"1\">"
    );
    let imap = server();
    for (subject, from) in [
        ("news", "News <news@shop.test>"),
        ("more", "News <NEWS@shop.test>"),
        ("other", "Eve <eve@x.test>"),
    ] {
        imap.deliver(
            "INBOX",
            mime(
                subject,
                from,
                "Content-Type: text/html; charset=utf-8\r\n",
                html.as_bytes(),
            ),
        )
        .unwrap();
    }
    let dir = TempDir::new("t20");
    let s = start(dir, imap, true).await;
    let id = message_id(&s, "news").await;
    let view = open(&s, id).await;
    assert_eq!(view.body_kind, BodyKind::Html);
    assert_eq!(view.remote_images, RemoteImages::Blocked);
    assert!(view.body_url.ends_with("?images=0"));
    let (doc, csp) = body_html(&s, id, "images=0").await;
    assert!(!csp.contains("mail-remote"));
    assert_eq!(remote_paths(&doc).len(), 2, "rewritten, not loaded: {doc}");
    // Asking for images=1 without consent changes nothing.
    let (_, csp) = body_html(&s, id, "images=1").await;
    assert!(!csp.contains("mail-remote"));
    assert!(
        seen.lock().unwrap().is_empty(),
        "nothing fetched before Load images"
    );

    // Load images: this message, this session.
    s.core.images_allow_once(id).await.unwrap();
    let view = s.core.message_open(id).await.unwrap();
    assert_eq!(view.remote_images, RemoteImages::Allowed);
    assert!(view.body_url.ends_with("?images=1"));
    let (doc, csp) = body_html(&s, id, "images=1").await;
    assert!(csp.contains("mail-remote:"));
    for path in remote_paths(&doc) {
        let r = s.core.mail_remote(&path).await;
        assert_eq!(
            (r.status, r.content_type.as_str(), r.body.as_slice()),
            (200, "image/gif", b"GIF".as_slice())
        );
    }
    let heads = seen.lock().unwrap().clone();
    assert_eq!(heads.len(), 2);
    for head in &heads {
        let lower = head.to_ascii_lowercase();
        assert!(
            !lower.contains("\r\ncookie:") && !lower.contains("\r\nreferer:"),
            "{head}"
        );
    }
    assert!(heads[1].starts_with("GET /pixel.gif?u=42 "), "{}", heads[1]);
    let more = message_id(&s, "more").await;
    assert_eq!(
        s.core.message_open(more).await.unwrap().remote_images,
        RemoteImages::Blocked
    );

    // Always load from this sender persists across restarts; allow-once does not.
    s.core.images_allow_sender(more).await.unwrap();
    s.core.shutdown().await;
    let (dir, imap, secrets) = (s.dir, s.imap, s.secrets);
    drop(s.core);
    tokio::time::sleep(Duration::from_millis(200)).await;
    let again = restart(dir, imap, secrets, false).await;
    for subject in ["news", "more"] {
        let id = message_id(&again, subject).await;
        assert_eq!(
            again.core.message_open(id).await.unwrap().remote_images,
            RemoteImages::Allowed,
            "{subject}"
        );
    }
    let other = message_id(&again, "other").await;
    assert_eq!(
        again.core.message_open(other).await.unwrap().remote_images,
        RemoteImages::Blocked
    );
    again.core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn html_toggle_flowed_text_links_and_the_xss_corpus() {
    let corpus = include_str!("../../../tests/fixtures/xss/corpus.html");
    let imap = server();
    imap.deliver(
        "INBOX",
        mime(
            "both",
            "b@test.local",
            "Content-Type: multipart/alternative; boundary=a\r\n",
            b"--a\r\nContent-Type: text/plain; charset=utf-8; format=flowed\r\n\r\nSoft \r\nwrapped, see https://x.test/p.\r\n> one\r\n>> two\r\n\
              --a\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<p><a href=\"https://evil.test/login\">paypal.com</a></p>\r\n--a--\r\n",
        ),
    )
    .unwrap();
    imap.deliver(
        "INBOX",
        mime(
            "corpus",
            "b@test.local",
            "Content-Type: text/html; charset=utf-8\r\n",
            corpus.as_bytes(),
        ),
    )
    .unwrap();
    let s = start(TempDir::new("toggle"), imap, true).await;
    let id = message_id(&s, "both").await;
    let view = open(&s, id).await;
    assert_eq!(view.body_kind, BodyKind::Html);
    assert!(view.has_html && !view.truncated);
    let (html, _) = body_html(&s, id, "images=0").await;
    assert!(
        html.contains("<a href=\"#\" data-href=\"https://evil.test/login\">paypal.com</a>"),
        "{html}"
    );
    let (plain, _) = body_html(&s, id, "images=0&plain=1").await;
    assert!(
        plain.contains(
            "Soft wrapped, see <a href=\"#\" data-href=\"https://x.test/p\">https://x.test/p</a>."
        ),
        "{plain}"
    );
    assert!(
        plain.contains("<details class=\"quote\"><summary>"),
        "{plain}"
    );

    // T19, Core half: the corpus is served with only our script.
    let corpus_id = message_id(&s, "corpus").await;
    open(&s, corpus_id).await;
    let (doc, csp) = body_html(&s, corpus_id, "images=1").await;
    assert_eq!(doc.matches("<script").count(), 1);
    assert!(!csp.contains("mail-remote"), "images=1 needs consent");
    assert!(!doc.to_ascii_lowercase().contains("javascript:"));

    // link.open hands only http, https and tel to the OS.
    assert_eq!(
        s.core.link_check(" https://x.test/a ").unwrap(),
        "https://x.test/a"
    );
    assert!(s.core.link_check("tel:+15551234").is_ok());
    for bad in [
        "javascript:alert(1)",
        "file:///c:/x",
        "mailto:a@b",
        "https:",
        "nonsense",
    ] {
        assert_eq!(
            s.core.link_check(bad).unwrap_err().code,
            ErrorCode::InvalidInput,
            "{bad}"
        );
    }
    s.core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_undeliverable_body_is_unreadable_and_its_source_can_be_saved() {
    let imap = server();
    let raw =
        b"Message-ID: <odd@t>\r\nFrom: b@test.local\r\nSubject: odd\r\n\r\nraw text\r\n".to_vec();
    // BODYSTRUCTURE names a part the message does not have.
    let structure = BodyPart {
        part_id: String::new(),
        mime: "multipart/mixed".into(),
        children: vec![BodyPart {
            part_id: "1.1".into(),
            ..BodyPart::text_plain(10)
        }],
        ..BodyPart::text_plain(0)
    };
    imap.deliver(
        "INBOX",
        FakeMessage::new(raw.clone(), now() - 60).with_body(structure),
    )
    .unwrap();
    let s = start(TempDir::new("odd"), imap, true).await;
    let id = message_id(&s, "odd").await;
    let view = open(&s, id).await;
    assert_eq!(view.body_kind, BodyKind::Unreadable);
    let target = s.dir.path().join("odd.eml");
    s.core
        .message_save_source(id, target.clone())
        .await
        .unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), raw);
    assert_eq!(s.core.source_file_name(id).await.unwrap(), "odd.eml");

    // Offline: Save source reports the network.
    s.core.account_set_enabled(s.account, false).await.unwrap();
    until(&s, |e| {
        matches!(
            e,
            CoreEvent::AccountState {
                state: StateName::Disabled,
                ..
            }
        )
    })
    .await;
    let err = s.core.message_save_source(id, target).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::Network);
    s.core.shutdown().await;
}
