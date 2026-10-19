# Milestone reports

Format (CLAUDE.md): built / tests added / deviations (must be none) / gaps found.

## M0: done, tagged `m0` (2026-10-19)

Every M0 check passes locally on Windows 10 (Rust 1.99.1, MSVC Build Tools 2022) and in CI on Windows, macOS and Linux ([run 39222979439](https://github.com/TheWhatDeep/zero-mail/actions/runs/39222979439)). CI covered boundaries and freshness, fmt, `cargo audit`, `npm audit`, lint, clippy, Rust and UI tests on three OSes, the Stalwart container smoke check, and the startup harness on three OSes (results are uploaded as run artifacts). Docker is not installed locally, by the owner choice; the Stalwart checks run in CI.

### Built
- **Repo:** `CLAUDE.md` (A3), `SPEC_GAPS.md`, `docs/BUILD.md` amended to v0.7 with the owner's decisions, `docs/MANUAL_TESTS.md` (T31, T32), `.gitignore`, `rustfmt.toml`, `clippy.toml`. Committed on `main`.
- **Cargo workspace:**
  - Ten module crates plus `src-tauri`.
  - The internal dependency edges follow the section 3 table exactly.
  - Workspace lints: `unsafe_code = forbid`, and `unwrap_used`, `expect_used`, `panic`, `todo` and `dbg_macro` denied. Every crate root has `#![forbid(unsafe_code)]`.
  - Release profile: LTO, strip, one codegen unit.
- **`mail-imap`:**
  - Types: capabilities, special-use roles, flags, `UidSet` (range-compressed, chunked at 500), BODYSTRUCTURE, search keys and dates.
  - `ImapError`, with the classes from section 12.
  - The `ImapConnector` and `ImapTransport` traits.
  - The section 12 timeout constants.
  - The fake server (`fake` feature, SPEC_GAPS G14). It honors advertised capabilities (MOVE, UIDPLUS, CONDSTORE, QRESYNC, SPECIAL-USE, LIST-STATUS, IDLE, X-GM-EXT-1). It supports scripted external changes, a UIDVALIDITY reset, password revocation, queued failures for any operation, and a call log.
- **Tauri shell (`src-tauri`):**
  - Identifier `com.digitalcircuitry.zeromail`, window 1320×800.
  - Production CSP: `default-src 'self'`, no `unsafe-eval`, no `unsafe-inline`, no remote origins, plus the `mail-body` and `mail-cid` protocols.
  - The capability file grants nothing.
  - `freezePrototype` is on.
  - The perf hook is active only under `ZEROMAIL_PERF_OUT`.
  - Placeholder app icon generated from `src-tauri/app-icon.svg`.
- **UI:**
  - Svelte 5 + Vite 8 + TypeScript 6 in strict mode.
  - `tokens.css` holds the 3a design tokens, the glass flattened to opaque (G4), dark tokens derived from the same ramps (G5), compact density, and motion tokens.
  - Geist 400/500/600 is bundled under the OFL (G3).
  - `app.css` holds the motion primitives. Hover and selection tints fade through an opacity overlay, presses scale, and entrances fade or pop (G8).
  - `lib/motion.ts` holds Svelte `fade` and `pop` transitions.
  - Also added: `strings.ts`, and `scripts/no-any.mjs` (a "no `any`" check).
- **Test infrastructure:**
  - Stalwart v0.15.5 compose file and declarative config with three test users (G15), plus a smoke check.
  - `tests/perf/startup.mjs`, the startup harness.
  - `scripts/check-deps.mjs`: section 3 boundaries, the approved dependency list, async-imap exclusivity, and 12-month freshness with the oauth2 waiver.
- **CI** (`.github/workflows/ci.yml`):
  - Boundaries, freshness, fmt, `cargo audit` and `npm audit`.
  - Lint, tests and clippy on Windows, macOS and Linux.
  - Stalwart integration.
  - Startup harness on all three OSes.
  - Actions are pinned to full commit SHAs.

### Tests added
- **`mail-imap` (Rust, 19 tests, passing):** capability parsing, UID set compression and chunking, flags, epoch-to-date conversion, special-use roles, error classes, secret redaction. The fake has tests for:
  - login and revocation
  - select, search and fetch
  - CONDSTORE/QRESYNC deltas
  - MOVE and UIDPLUS fallbacks
  - targeted and bare expunge
  - UIDVALIDITY renumbering
  - APPEND and folder commands (`Café/日本語`)
  - LIST roles
  - Gmail de-duplication and labels
  - scripted failures
  - IDLE wake and timeout
  - header unfolding
- **UI (Vitest, 18 tests, passing):**
  - Motion parsing, the 150 ms clamp, and opacity/transform-only transitions.
  - Guardrail tests: no literal colors outside `tokens.css`; every `var()` defined; no `backdrop-filter` or remote resources; transitions and keyframes limited to opacity and transform; no literal durations; reduced motion sets 0 ms; the two dark blocks are identical; the message canvas stays white; WCAG 4.5:1 contrast for every text and surface pair in both themes.
  - A mutation check confirmed these tests fail on violations.
- **Checks run here:**
  - `npm test`: 18/18 pass.
  - `npm run lint` (svelte-check plus no-any): 0 errors, 0 warnings.
  - `npm run build`: no `data:` URIs in the output.
  - `node scripts/check-deps.mjs --online`: pass. A deliberate boundary violation was rejected.
  - `npm audit`: 0 vulnerabilities.
  - `cargo build --workspace --all-targets`: no errors and no warnings on the first compile.
  - `cargo fmt --all`, then `cargo clippy --workspace --all-targets -- -D warnings`: clean.
  - `cargo test --workspace`: 19/19 pass.
  - `npx tauri build --no-bundle`: `zero-mail.exe` is 7.8 MB.
  - **Startup baseline** (`tests/perf/startup.mjs`, 5 runs, Windows 10, page load finished): median **736 ms** wall clock and 607 ms in-app, against the 1,500 ms budget. The first, fully cold run took 1,805 ms because WebView2 had to start cold. The owner should decide whether the budget means warm or cold start before the harness is enforced in M4.

### Deviations
None from spec v0.7. Every departure from v0.6 is an owner decision in SPEC_GAPS (G2–G18).

### Gaps found
- G1: resolved locally for Rust and MSVC. Docker is replaced by CI by owner choice.
- G14 and G15: resolved by interpretation, awaiting owner ratification.
- G16: contrast fixes.
- G17: Node 22.12+.
- G18 (open, blocks M2): the AppError code list.

### Next
M1 (`store`). M2 is blocked on G18 (the AppError code list).

## M1: done, tagged `m1` (2026-10-19)

Scope: `store` (`crates/mail-store`): schema, migrations, query API, FTS, writer and read pool. Both done-when tests pass: the migration test and keyset pagination on a 50,000-row fixture. Every check below passes locally on Windows 10 (Rust 1.99.1). CI result is added once the run is green.

### Built
- **Schema (migration v1):**
  - All 18 section 4 tables, with exactly the spec's columns in the spec's order, plus `message_fts`.
  - `STRICT` tables. Every column the spec marks `NULL` is nullable and every other column is `NOT NULL` (G21).
  - `CHECK` constraints on the enumerated values the spec gives (`auth_kind`, `save_sent_copy`, `special_use`, `body_state`, outbox `state`, security mode), on booleans, on UID ranges, and `json_valid` on every JSON column.
  - Times are `INTEGER` UTC epoch seconds (invariant 6). No column holds a secret (invariant 5).
- **Foreign keys:**
  - On for the writer connection.
  - `ON DELETE CASCADE` from each owner (account, folder, message, thread, rule), so deleting a folder or an account removes its messages, bodies and attachments (invariant 7).
  - `pending_ops.folder_id` and `snoozes.origin_folder_id` have no foreign key (G22).
- **Migrations:**
  - Forward-only through `PRAGMA user_version`.
  - Each migration runs in one immediate transaction together with its version bump.
  - A database from a newer build is refused with `TooNew` and never downgraded.
- **FTS:**
  - `message_fts` is FTS5 over `subject`, `sender`, `recipients` and `body`, with tokenizer `unicode61 remove_diacritics 2`.
  - External content comes from the view `message_fts_content` (`messages LEFT JOIN bodies`, G20).
  - Triggers keep the index exact through inserts, header updates, body upserts and deletes, and cascaded deletes.
  - Address lists are stored as JSON `[name, addr]` pairs, so only names and addresses reach the index.
- **Indexes:** `messages(folder_id, date_utc DESC, id DESC)` for keyset pagination, plus an index on every foreign-key child column.
- **Connections:**
  - WAL mode, checked at open. `synchronous = FULL`, so a committed outbox or pending-op row survives power loss (principle 2). 5 s busy timeout.
  - On Unix the database file gets mode 0600 (section 13, item 9).
- **Writer:**
  - One `store-writer` thread owns the only read-write connection. Writes arrive over a bounded tokio channel and run in order.
  - Each API write is one immediate transaction that rolls back on any error, including `SQLITE_FULL`, which maps to `StoreError::DiskFull` (section 12).
- **Read pool:**
  - `READERS = 4` read-only, `query_only` connections behind a semaphore. Queries run in `spawn_blocking`.
  - A loan guard returns the connection to the pool even when the caller abandons the read, as cancelled searches will in M9.
  - Dropping the last `Store` handle drains the writer and closes every connection.
- **Query API** (async methods on `Store`; no SQL or connection leaves the crate):
  - Writes: `insert_account`, `delete_account`, `insert_folder`, `delete_folder`, `insert_thread`, `insert_messages` (a batch with attachment metadata, one transaction), `delete_messages` (by UID), `put_body` (upsert plus `body_state`).
  - `purge_folder`: the UIDVALIDITY purge (invariant 1, section 12). In one transaction it deletes the folder's messages, bodies, attachments and pending ops, stores the new UIDVALIDITY, and resets `uidnext`, `highestmodseq`, the counters and `synced_at` so the next pull is a first sync.
  - Reads: `folder_page` and `search_page` (FTS5 match). Both are keyset pages on `(date, id)`, newest first, and return a `next` cursor only when another row exists. No query uses an offset.
  - Every list and search hides `\Deleted` messages, which are bit 5 of `flags` (invariant 8, G19).
- `StoreError` (thiserror): `DiskFull`, `Corrupt` (also for a file that is not a database), `TooNew`, `NotWal`, `NotFound`, `Invalid`, `Closed`, `Sqlite`, `Json`, `Io`.
- **Dependencies added to `mail-store`:** `rusqlite` (bundled), `serde`, `serde_json`, `thiserror` and `tokio` (`sync`, `rt`), all from `[workspace.dependencies]`.

### Tests added (30, all passing)
- **Migrations** (`src/schema.rs`, 5):
  - A fresh database reaches the latest version.
  - Re-running is a no-op that keeps data.
  - Only unapplied migrations run.
  - A failing migration rolls back entirely, DDL and version.
  - A newer database is refused.
- **Queries** (`src/queries.rs`, 2):
  - `EXPLAIN QUERY PLAN` of the folder page is an index seek on `messages_folder_date` with no temp B-tree sort.
  - A source guard: no query uses an offset.
- **Migration test** (`tests/migrations.rs`, 6):
  - A fresh file database matches section 4 exactly: table set, column names and order, nullability per the spec's `NULL` marks, time columns `INTEGER`, WAL on, and FTS5 external content with the specified tokenizer.
  - No column name looks like a secret.
  - Reopening keeps data and the version.
  - A newer version is refused and not downgraded.
  - Foreign keys are enforced.
  - A garbage file reports `Corrupt`.
- **Invariants** (`tests/invariants.rs`, 14):
  - The UIDVALIDITY purge empties only its folder (messages, bodies, attachments, ops) and resets its sync state.
  - An injected failure in the purge's last step leaves everything in place.
  - Folder and account deletes cascade.
  - `\Deleted` rows are hidden from lists and search.
  - A duplicate UID rolls back the whole batch.
  - Delete by UID works.
  - FTS: diacritics removed, prefix match, address search, body indexing, re-indexing on body update and header update, and body-cache clearing. FTS5's integrity check with content verification (`rank = 1`) passes after every case.
  - List rows carry the right columns. A zero limit is rejected.
  - 16 concurrent readers run against the writer and only ever see whole batches.
  - Abandoned reads return their connections.
  - Commits survive reopening, and `PRAGMA integrity_check` passes.
- **Keyset pagination** (`tests/pagination.rs`, 3):
  - **The 50,000-row fixture:** shuffled insertion order, seven rows per timestamp, every 97th row `\Deleted`, and 1,000 decoy rows in another folder. Walking 100-row pages returns every visible row exactly once, strictly descending by `(date, id)`, identical to a direct `ORDER BY`. The page count is exact, and only full pages have a cursor.
  - Page boundaries inside a run of equal dates lose nothing, for limits 1 to 26.
  - Mail arriving above the cursor does not shift later pages.
- **Checks run here:**
  - `npm --prefix ui run build`.
  - `cargo fmt --all -- --check`.
  - `cargo clippy --workspace --all-targets -- -D warnings`: clean.
  - `cargo test --workspace`: 49 passed (19 `mail-imap`, 30 `mail-store`). The 50k test takes about 14 s in a debug build.
  - `npm test`: 18 passed. `npm run lint`: 0 errors, 0 warnings.
  - `node scripts/check-deps.mjs`: 11 crates within boundaries.

### Deviations
None from spec v0.7. The section 4 amendment for G19 (bit 5 = `\Deleted`) is the owner's decision.

### Gaps found
- **G19** (resolved by the owner): `\Deleted` is bit 5 of `flags`.
- **G20, G21, G22** (resolved by interpretation, owner to ratify):
  - G20: the FTS content view and the `[name, addr]` JSON format.
  - G21: nullability and the "not set yet" values.
  - G22: two folder references without a foreign key.
- **G23** (open, blocks only the `bcc:` operator in M9): Bcc is not stored on `messages`.
- **Not in M1's scope:** the section 12 open path (integrity check, move aside as `*.corrupt`, rebuild) is not wired. The store reports `Corrupt` and `TooNew`; whoever opens it at startup acts on them.

### Next
M2. It is still blocked on G18 (the AppError code list).
