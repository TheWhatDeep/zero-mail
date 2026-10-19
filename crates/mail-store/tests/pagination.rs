//! M1 "keyset pagination test on a 50,000-row fixture" (section 14: the list is paginated
//! by keyset on `(date, id)`, never OFFSET).
#![cfg(test)]

mod common;

use common::*;
use mail_store::*;
use std::collections::HashSet;

const ROWS: u32 = 50_000;
const PAGE: u32 = 100;

/// Deterministic shuffle (an LCG over a full cycle), so insertion order differs from date
/// order and ids do not track dates.
fn shuffled(n: u32) -> Vec<u32> {
    let mut v: Vec<u32> = (0..n).collect();
    let mut state: u64 = 0x2545_f491_4f6c_dd1d;
    for i in (1..v.len()).rev() {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let j = (state >> 33) as usize % (i + 1);
        v.swap(i, j);
    }
    v
}

/// Seven messages share each timestamp, so the id tiebreak is exercised on every page.
fn date_of(n: u32) -> i64 {
    1_700_000_000 + i64::from(n / 7) * 60
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn keyset_pagination_over_50k_rows() {
    let dir = TempDir::new("pagination");
    let store = Store::open(&dir.db()).unwrap();
    let (account, inbox, thread) = seeded(&store).await;
    let other = store
        .insert_folder(folder(account, "Archive"))
        .await
        .unwrap();

    // Fixture: 50,000 rows in INBOX, every 97th flagged \Deleted (hidden), plus 1,000 rows
    // in another folder that must never appear. Inserted in sync-sized batches.
    let order = shuffled(ROWS);
    let mut hidden = 0;
    for chunk in order.chunks(1_000) {
        let batch = chunk
            .iter()
            .map(|&n| {
                let mut m = message(n + 1, date_of(n), thread);
                if n % 97 == 0 {
                    m.flags = Flags(Flags::DELETED);
                }
                m
            })
            .collect::<Vec<_>>();
        hidden += batch
            .iter()
            .filter(|m| m.flags.contains(Flags::DELETED))
            .count();
        store.insert_messages(inbox, batch).await.unwrap();
    }
    let decoys = (1..=1_000)
        .map(|uid| message(uid, date_of(uid), thread))
        .collect();
    store.insert_messages(other, decoys).await.unwrap();

    // Expected order straight from the table.
    let conn = raw(&dir.db());
    let expected: Vec<(i64, i64)> = conn
        .prepare(
            "SELECT date_utc, id FROM messages WHERE folder_id = ?1 AND (flags & 32) = 0
             ORDER BY date_utc DESC, id DESC",
        )
        .unwrap()
        .query_map([inbox], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(expected.len(), ROWS as usize - hidden);

    // Walk every page.
    let mut seen = Vec::with_capacity(expected.len());
    let mut cursor = None;
    let mut pages = 0;
    loop {
        let page = store.folder_page(inbox, cursor, PAGE).await.unwrap();
        pages += 1;
        assert!(page.rows.len() <= PAGE as usize);
        assert!(page.rows.iter().all(|r| r.folder_id == inbox));
        assert!(page.rows.iter().all(|r| !r.flags.contains(Flags::DELETED)));
        seen.extend(page.rows.iter().map(|r| (r.date_utc, r.id)));
        match page.next {
            Some(next) => {
                assert_eq!(
                    page.rows.len(),
                    PAGE as usize,
                    "only a full page has a successor"
                );
                let last = page.rows.last().unwrap();
                assert_eq!(
                    next,
                    Cursor {
                        date_utc: last.date_utc,
                        id: last.id
                    }
                );
                cursor = Some(next);
            }
            None => break,
        }
    }

    assert_eq!(
        seen, expected,
        "every visible row exactly once, newest first by (date, id)"
    );
    assert!(seen.windows(2).all(|w| w[0] > w[1]), "strictly descending");
    assert_eq!(
        seen.iter().collect::<HashSet<_>>().len(),
        seen.len(),
        "no duplicates"
    );
    assert_eq!(pages, expected.len().div_ceil(PAGE as usize));
}

#[tokio::test]
async fn a_page_boundary_inside_a_run_of_equal_dates_loses_nothing() {
    let dir = TempDir::new("ties");
    let store = Store::open(&dir.db()).unwrap();
    let (_, inbox, thread) = seeded(&store).await;
    let batch = (1..=25).map(|uid| message(uid, 1_000, thread)).collect();
    let mut ids = store.insert_messages(inbox, batch).await.unwrap();
    ids.reverse();

    for limit in [1, 2, 3, 7, 24, 25, 26] {
        let mut seen = Vec::new();
        let mut cursor = None;
        loop {
            let page = store.folder_page(inbox, cursor, limit).await.unwrap();
            seen.extend(page.rows.iter().map(|r| r.id));
            match page.next {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        assert_eq!(seen, ids, "limit {limit}");
    }
}

#[tokio::test]
async fn rows_inserted_above_the_cursor_do_not_shift_later_pages() {
    // The reason for keyset over OFFSET: new mail arriving while the user scrolls neither
    // duplicates nor skips rows further down.
    let dir = TempDir::new("stable");
    let store = Store::open(&dir.db()).unwrap();
    let (_, inbox, thread) = seeded(&store).await;
    let batch = (1..=30)
        .map(|uid| message(uid, i64::from(uid), thread))
        .collect();
    store.insert_messages(inbox, batch).await.unwrap();

    let first = store.folder_page(inbox, None, 10).await.unwrap();
    let newer = (31..=40)
        .map(|uid| message(uid, i64::from(uid), thread))
        .collect();
    store.insert_messages(inbox, newer).await.unwrap();
    let second = store.folder_page(inbox, first.next, 10).await.unwrap();

    let dates: Vec<i64> = second.rows.iter().map(|r| r.date_utc).collect();
    assert_eq!(dates, (11..=20).rev().collect::<Vec<i64>>());
}
