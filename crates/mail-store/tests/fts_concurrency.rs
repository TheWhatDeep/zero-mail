//! Full-text reads on pooled readers while sync writes bodies (M3 prefetch runs alongside
//! UI queries). Found in M3: `PRAGMA integrity_check` on a pooled reader reported FTS5
//! corruption during concurrent index writes, so the check runs on the writer. Searches on
//! readers stay correct throughout.
#![cfg(test)]

mod common;

use common::{message, seeded, TempDir};
use mail_store::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn searches_and_integrity_checks_stay_clean_during_body_writes() {
    let dir = TempDir::new("fts-concurrency");
    let store = Store::open(&dir.db()).unwrap();
    let (_, inbox, thread) = seeded(&store).await;
    let ids = store
        .insert_messages(
            inbox,
            (1..=400)
                .map(|i| message(i, i64::from(i), thread))
                .collect(),
        )
        .await
        .unwrap();

    let writer = {
        let store = store.clone();
        tokio::spawn(async move {
            for (n, id) in ids.into_iter().enumerate() {
                let body = Body {
                    text_plain: format!("budget report number {n} with words"),
                    text_html: String::new(),
                    fetched_at: 1,
                };
                store
                    .store_body(id, body, BodyState::Full, format!("budget {n}"))
                    .await
                    .unwrap();
            }
        })
    };
    let mut readers = Vec::new();
    for _ in 0..4 {
        let store = store.clone();
        readers.push(tokio::spawn(async move {
            for i in 0..200 {
                store.search_page("budget".into(), None, 20).await.unwrap();
                if i % 20 == 0 {
                    store.integrity_check().await.unwrap();
                }
            }
        }));
    }
    writer.await.unwrap();
    for r in readers {
        r.await.unwrap();
    }
    let hits = store
        .search_page("budget".into(), None, 1000)
        .await
        .unwrap();
    assert_eq!(hits.rows.len(), 400);
    store.integrity_check().await.unwrap();
}
