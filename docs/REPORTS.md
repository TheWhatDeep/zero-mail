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

Scope: `store` (`crates/mail-store`): schema, migrations, query API, FTS, writer and read pool. Both done-when tests pass: the migration test and keyset pagination on a 50,000-row fixture. Every check below passes locally on Windows 10 (Rust 1.99.1) and in CI on Windows, macOS and Linux ([run 39228812671](https://github.com/TheWhatDeep/zero-mail/actions/runs/39228812671)): boundaries and freshness, fmt, `cargo audit` (covering the new `rusqlite` and `libsqlite3-sys`), `npm audit`, lint, clippy, tests on three OSes, Stalwart integration, and the startup harness.

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

## M2: done, tagged `m2` (2026-10-19)

Scope: `imap`, `secrets`, `auth` (password), account CRUD, autoconfig, and the SMTP connection test (G32). Done when: an account is added against the Stalwart container, `LIST` works, and T09 passes at the login level (G33). Every check below passes locally on Windows 10 (Rust 1.99.1). CI is green on Windows, with Stalwart on Linux ([run 39259264139](https://github.com/TheWhatDeep/zero-mail/actions/runs/39259264139)). The Stalwart-backed tests ran only in CI, because Docker is not installed locally.

### Built
- **`mail-imap`, the real transport (`ImapClient` / `ImapSession`):**
  - **Connection:** TCP with a 60 s keepalive (G31). TLS 1.2+ through rustls on `ring`, verified by the OS trust store (G30). With a per-account pin, only an exact SHA-256 match of the leaf certificate is accepted. A certificate failure reports the presented fingerprint.
  - **STARTTLS:** required on 143. It aborts before anything else is sent when STARTTLS is not offered. Capabilities seen before TLS are discarded.
  - **Login:** `AUTHENTICATE PLAIN`, falling back to `LOGIN` unless `LOGINDISABLED`. XOAUTH2 is wired for M8. Exactly one attempt: a rejected login is never retried.
  - **Commands:** every `ImapTransport` method, each under the section 12 timeouts. A timeout or a broken stream poisons the connection.
    - `LIST` adds `RETURN (SPECIAL-USE STATUS ...)` where advertised.
    - `SELECT` adds `(CONDSTORE)` where advertised. `ENABLE QRESYNC` runs after login when advertised.
    - Header fetch covers `UID FLAGS INTERNALDATE RFC822.SIZE BODYSTRUCTURE`, the selected headers, `MODSEQ`, and the `X-GM-*` attributes.
    - Flag fetch uses `CHANGEDSINCE` and `VANISHED`.
    - The rest: part fetch, `STORE`, `MOVE`/`COPY` with `COPYUID`, `UID EXPUNGE`, `APPEND` (literal) with `APPENDUID`, `CREATE`/`RENAME`/`DELETE`, raw `IDLE` with re-issue and read deadlines, `NOOP`, `LOGOUT`.
  - **Search strings:** non-ASCII strings are sent as literals with `CHARSET UTF-8`.
  - **Error mapping:** throttle responses (`[LIMIT]`, `[UNAVAILABLE]`, bandwidth messages) become `Throttled`, including on BYE. `[NONEXISTENT]` and `[TRYCREATE]` become `NoSuchMailbox`.
  - **Folder names:** modified UTF-7 in both directions.
  - **Folder roles:** SPECIAL-USE first, then name heuristics, at most one folder per role (section 5, step 5).
  - **Probe:** `ImapConnector::probe` reads the pre-login capabilities so setup can offer a login method. The fake implements it too.
- **`mail-secrets`:**
  - `SecretStore` trait. `Keychain` (the `keyring` crate, service `com.digitalcircuitry.zeromail`) uses keys `account:<id>:password` and `account:<id>:refresh_token`.
  - `Secret` never prints.
  - `MemoryStore` behind the `fake` feature, for tests and for the Linux CI runner, which has no Secret Service.
- **`mail-smtp`:** `SmtpCheck::check_login` (G32) runs connect, `EHLO`, `AUTH PLAIN`/`LOGIN` and `QUIT`, and sends no message.
  - TLS is ours: the same verifier and pin as IMAP, kept as a copy because the crates may not depend on each other. lettre speaks SMTP over the encrypted stream.
  - STARTTLS is required on 587. The plaintext part is only greeting, `EHLO` and `STARTTLS`, and data arriving before the handshake is refused.
  - The EHLO name is `[127.0.0.1]`, so the host name never leaves the machine.
  - `FakeSmtp` behind the `fake` feature.
- **`mail-auth`:**
  - **Resolution order:** the built-in table (Google, Fastmail, iCloud, Yahoo), then `https://autoconfig.<domain>/mail/config-v1.1.xml`, then `https://<domain>/.well-known/autoconfig/mail/config-v1.1.xml`, then the Mozilla ISP database. An empty result means the manual form.
  - **Lookups:** HTTPS only, domain only, and switched off by the `privacy.online_lookups` setting.
  - **XML:** a hand-written reader (G26). It refuses DOCTYPE internal subsets and unknown entities and caps depth, and the document size is capped at 256 KB. Plaintext and CRAM-only servers are dropped.
  - **HTTP client:** `reqwest` with `rustls-no-provider` on `ring` (G29), with no cookies, no referrer, no proxy, a generic user agent and a 10 s timeout. `aws-lc-rs` is no longer in the build.
  - **Login methods:** `auth_methods` offers OAuth and an app password for Google hosts, and a password everywhere else.
  - **Passwords:** `password_login` and `store_password` read and write the keychain.
- **`mail-store`:** `add_account`, which writes the account and its default identity in one transaction (section 5: every account has at least one identity). Also `get_account`, `list_accounts`, `update_account`, `set_account_enabled`, `list_identities`, `get_setting` and `set_setting`.
- **`mail-core`:**
  - `AppError { code, message, account_id? }` with the 20 G18 codes, serialized as the snake-case strings G18 lists, plus mappings from every module error.
  - `Accounts` implements `resolve`, `test`, `add`, `update`, `remove`, `set_enabled`, `list` and `check_login`. Saving requires both halves of Test connection to pass, unless Save anyway is set.
  - The password goes only to the keychain. A keychain failure undoes the account insert.
  - Remove deletes the account's rows and keychain entries and touches nothing on the server.
- **CI:** the Linux integration job now builds and runs `cargo test -p mail-core --test stalwart -- --ignored`. These tests are `#[ignore]`d everywhere else.
- **Dependencies:** `rustls-platform-verifier` and `socket2` were added to `[workspace.dependencies]` (G30, G31). `reqwest` switched to `rustls-no-provider` (G29). `check-deps --online` passes.

### Tests added (55 local, plus 3 in CI; workspace total 104 local)
- `mail-imap` (+20):
  - Fingerprint parsing, and an unreachable port reporting `Network`.
  - Modified UTF-7: the RFC 3501 examples, round trips, malformed input.
  - INTERNALDATE parsing, throttle text, UIDPLUS uid-sets.
  - BODYSTRUCTURE: single part and nested multipart with part ids, content id, disposition and filename.
  - Folder-role heuristics.
  - Command building: UTF-7 quoting, search literals, the APPEND literal. SASL PLAIN and XOAUTH2 responses. Status and error mapping.
  - Against a scripted plaintext server: STARTTLS not offered aborts with nothing sent; capabilities are asked for when the greeting lacks them; a plaintext server on an implicit-TLS port fails TLS.
- `mail-secrets` (4): key format, redaction, the memory store, and a real Windows Credential Manager round trip.
- `mail-smtp` (4): missing STARTTLS aborts before any AUTH, refused STARTTLS aborts, a plaintext server on 465 fails TLS, and a refused port is transient.
- `mail-auth` (12):
  - The Google host rule.
  - Address validation.
  - The keychain password round trip.
  - The built-in table.
  - The XML reader: entities, CDATA and comments are handled; malformed or hostile input is rejected.
  - Resolution: order and URLs, with only the domain sent; offline mode sends nothing; usable servers only; garbage and oversize documents.
  - The HTTP client builds and refuses `http://`.
- `mail-store` (3): account plus identity in one transaction, update, enable, list and NotFound, the identity cascade, and settings as JSON.
- `mail-core` (12):
  - The AppError contract: shape, all 20 codes, the certificate fingerprint.
  - Add, list and `check_login`, with no password in anything the UI receives.
  - A failing IMAP or SMTP half blocks saving, and Save anyway skips the test.
  - Update replaces settings and password, or keeps the saved password.
  - Remove deletes rows and keychain entries with zero server calls.
  - Invalid input is rejected before any connection; OAuth returns `unsupported` until M8.
  - Pin normalization.
  - Resolve: login methods from the host and capabilities, a certificate to pin, and the offline setting.
  - **T09 (login level):** after revocation, `auth_failed` with the account id, exactly one `Connect` and no `LIST`, and the other account still logs in.
- **Stalwart, CI only (3):**
  - Unpinned Test connection reports the self-signed certificate on both halves with one fingerprint. Pinned, it passes over implicit TLS and STARTTLS for IMAP and SMTP.
  - Add the account; `check_login` returns at least INBOX; a different pin is a hard failure.
  - The real transport probes, lists with roles, selects INBOX, NOOPs and logs out.
  - T09: a stale password gives `auth_failed` on both halves and on `check_login`, while another account keeps working.
- **Checks run here:**
  - `npm --prefix ui run build`.
  - `cargo fmt --all -- --check`.
  - `cargo clippy --workspace --all-targets -- -D warnings`.
  - `cargo test --workspace`: 104 passed, 3 ignored.
  - `npm test`: 18 passed. `npm run lint`: 0 errors, 0 warnings.
  - `node scripts/check-deps.mjs --online`: every crate fresh except the waived `oauth2`.

### Deviations
None from spec v0.9. v0.9 records the owner's decisions G29–G33.

### Gaps found
- **G29–G33:** resolved by the owner (see above).
- **G34** (interpretation, owner to ratify): one pin covers IMAP and SMTP. For `certificate` errors, `message` is the fingerprint.
- **Consequence of G27, for the owner to know:** a Google Workspace address on a custom domain resolves through the domain's autoconfig file, if it has one, else the manual form. The built-in table matches only gmail.com and googlemail.com, and the MX guess that would have found Google is dropped.
- **Not in M2's scope:**
  - The Tauri command layer (`account.*` handlers, `ipc.ts`, opening the store at startup) is wired with the first UI that calls it, in M4.
  - Folder rows are written by `sync` in M3. `account.remove` has no cached files to delete yet; the attachment cache arrives in M5.

### Next
M3 (`sync`).

## M3: done, tagged `m3` (2026-10-20)

Scope: `sync`: actors, folder order, folder procedure, IDLE, backoff. Done when: T01, T02, T04 (row part, G38), T05 (list part), T07 and T08 pass, plus T09 with actors (G33). Every check below passes locally on Windows 10. CI is green on Windows and in the Stalwart job ([run 39312899452](https://github.com/TheWhatDeep/zero-mail/actions/runs/39312899452)).

### Built
- **`mail-sync`, the account actor (`spawn_account`, `AccountHandle`):**
  - **One actor per account** with the section 6 states: Disabled, Connecting, Syncing, Idle, Backoff and NeedsAttention.
  - **Connections:** one worker connection runs a priority queue: user requests, then pending ops (M6), then folder sync, then prefetch, deduplicated. INBOX gets an IDLE connection, and so does each watched folder, up to 3, for at most 5 connections. The IDLE connections open once INBOX headers are in.
  - **IDLE:** re-issued every 25 minutes, and any news queues that folder's sync. Without IDLE, INBOX is polled every 2 minutes.
  - **Folders on request:** `refresh` (priority 1). A folder open in the UI syncs at once and every 5 minutes.
  - **Retries:**
    - Backoff starts at 2 s and doubles to 5 minutes with ±20% jitter. It resets once a connection has stayed healthy for 60 s.
    - A throttle response pauses the account for a fixed 10 minutes (shown as Backoff, G40).
    - After BYE or a closed socket, the actor reconnects once at once, then backs off.
    - A bad login, a bad certificate or missing credentials go to NeedsAttention with no timer. Only `retry` or an account change brings the account back.
    - Disabling the account closes every connection. An actor whose account was removed stops.
  - **Events:** `SyncEvent` carries `StoreChanged` (no folder means the folder list changed), `AccountState` (with the error and the wait), `SyncProgress` and `FolderReset` (G37). Core debounces and forwards them in M4.
- **Folder list:**
  - `LIST`, then roles: SPECIAL-USE first, then name heuristics.
  - New paths are added. Vanished paths are purged with their messages (section 12).
  - Known folders keep their switches.
  - On `X-GM-EXT-1`, All Mail starts with sync off.
- **Folder order:** INBOX, then Drafts, Sent and Archive, then the rest alphabetically. INBOX is complete before any other folder is selected.
- **Folder procedure, steps 1–7:**
  - **UIDVALIDITY:** the first value is recorded before anything is cached. A change purges the folder, fires `folder-reset`, and starts a first sync.
  - **New mail:** the first sync takes `UID SEARCH SINCE` for the last `sync_days`, minus what is cached. That rule also resumes a killed first sync. Later syncs take `UID SEARCH UID n:*`, filtered for the RFC quirk. Headers come newest first, 100 per batch, one transaction per batch, with `store-changed` and `sync-progress` after each.
  - **Flags and deletions:**
    - With CONDSTORE, a single `UID FETCH 1:max (FLAGS) (CHANGEDSINCE m VANISHED)` covers the folder, and with QRESYNC its VANISHED list replaces `SEARCH ALL`.
    - Without CONDSTORE, flags are fetched in batches of 500 and deletions come from `UID SEARCH ALL`.
  - **Step 7:** counters and server state, then `store-changed`.
  - **Step 3** (push pending ops) has nothing to do until `ops` arrives in M6.
- **Headers to rows:**
  - `mail-parser` decodes RFC 2047 subjects and names, address lists and groups, Date, Message-ID, In-Reply-To and References. IDs are stored bare.
  - A missing Message-ID gets a synthetic `synth:<account>:<folder>:<uid>@local`. A bad Date, a missing one, or one more than 24 h ahead falls back to INTERNALDATE.
  - Flags map to bits, including `$Forwarded`.
  - Attachment metadata comes from BODYSTRUCTURE. Inline parts are marked and do not set `has_attach`.
  - Threads: one per message, or shared by `X-GM-THRID` (G36).
- **Body prefetch (step 8):**
  - Takes the newest 200 messages per folder, at the lowest priority, one message per job.
  - Fetches the text/plain and text/html parts whole. A body text part over 1 MB is skipped (G39).
  - Transfer and charset decoding follow section 8's fallback: UTF-8, then Windows-1252, then lossy. HTML-only mail gets generated plain text.
  - Previews take the first 140 characters, with quotes and the signature dropped.
  - `body_state` becomes `full` (G35).
- **`mail-store`:** queries for the folder list, the first UIDVALIDITY, cached UIDs, threaded inserts, flags, finishing a folder sync, prefetch candidates and bodies with previews. Migration V2 adds an index on `(account_id, gm_thrid)`. `integrity_check` runs on the writer (see Gaps).
- **`mail-imap`:**
  - `uid_fetch_changes` sends `CHANGEDSINCE` over `1:max`.
  - `SearchKey::UidFrom` sends `UID n:*`. The fake mimics the RFC quirk.
  - The fake gained `fail_after(op, n, error)`.
- **`mail-auth`:** `LoginSource` and `KeychainLogins`, so sync gets credentials through `auth` (section 3).
- **Dependencies:** `mail-parser` has the `full_encoding` feature on (G40). `mail-sync` uses `mail-parser`, `rand` (jitter) and `tracing`. No new approved crate.
- **CI:** the Stalwart job also runs `cargo test -p mail-sync --test stalwart -- --ignored`.

### Tests added (33 local, plus 2 in CI; workspace total 137 local)
- **T01** (`tests/sync.rs`): 300 INBOX messages, 250 of them in the window.
  - Exactly 250 headers, newest first.
  - Progress events at 100, 200 and 250, and INBOX rows announced before the account is Idle.
  - Every INBOX header fetch happens before any other folder is selected. The order is INBOX, Drafts, Sent, Archive, then the rest.
  - Bodies only for the newest 200 (UIDs 101–300), with clean previews and `body_state = full`.
- **T02:** a server-side UIDVALIDITY change fires `FolderReset`. The cache then holds exactly the renumbered UIDs, with UIDVALIDITY and counters updated.
- **T04 (row part):** an expunge elsewhere removes the row within 2 s, with QRESYNC (VANISHED) and on plain IMAP (SEARCH ALL).
- **T05 (list part):** delivered mail is in the list within 2 s through IDLE.
- **T07:** the second header batch never completes, and the process is killed during the backoff. Exactly one batch was committed. After a restart on a reopened store: 350 rows, 350 distinct UIDs, the exact server UID set, and the integrity and FTS content checks pass.
- **T08** (`tests/backoff.rs`, paused tokio time):
  - With the port blocked, the announced delays are 2, 4, 8, … 256, 300, 300 s, each within ±20%. Each actual reconnect gap matches its announced delay within 5 ms. The UI sees Backoff with the network error.
  - The account recovers to Idle once unblocked.
  - After 61 s healthy, a new failure starts again at 2 s.
  - Throttled: exactly 10 minutes.
  - BYE: one immediate reconnect, then backoff.
- **T09 with actors:** after revocation and a BYE, Alice goes to NeedsAttention with `auth_failed` and exactly one login, and none over the next 3 s. Bob keeps syncing new mail. Retry brings Alice back once the password is fixed.
- **Procedure:**
  - Flags set elsewhere are pulled with and without CONDSTORE, and `\Deleted` rows are hidden and uncounted.
  - A QRESYNC resync of 600 cached messages is one flag command and no `SEARCH ALL`.
  - A folder deleted on the server is purged on the next LIST. Gmail All Mail starts unsynced.
  - With 4 watched folders, IDLE connections are capped: 5 connections in all.
  - `refresh` and `open_folder` sync on request.
  - The enabled switch works. A missing keychain password means NeedsAttention.
- **Unit tests:** backoff schedule and jitter bounds; header decoding with fallbacks; body and attachment part selection; charsets (base64, QP, ISO-8859-1, a bogus label falling back to UTF-8 and Windows-1252, Shift_JIS); previews; folder order; the queue's priorities and deduplication.
- **`mail-store`:** 5 sync-query tests, and `fts_concurrency` (searches and integrity checks during concurrent body writes).
- **`mail-imap`:** `uid_fetch_changes` on the fake; `fail_after`.
- **Stalwart, CI only:** T05 (SMTP delivery to port 25, listed within 2 s plus server latency) and T04 (`UID EXPUNGE` from a second real connection removes the row).
- **Checks run here:**
  - `npm --prefix ui run build`.
  - `cargo fmt --all -- --check`.
  - `cargo clippy --workspace --all-targets -- -D warnings`.
  - `cargo test --workspace`: 137 passed, 5 ignored. The sync suites ran several times in a row with no flakes.
  - `npm test`: 18 passed. `npm run lint`: 0 errors, 0 warnings.
  - `node scripts/check-deps.mjs`.

### Deviations
None from spec v0.10. v0.10 records the owner's decisions G35–G38.

### Gaps found
- **G35–G38:** resolved by the owner (see above).
- **G39 and G40** (interpretations, owner to ratify):
  - G39: the prefetch skip rule.
  - G40: throttle shown as Backoff; a `store-changed` with no folder for folder-list changes; `full_encoding`.
- **Found in testing:** `PRAGMA integrity_check` on a pooled read-only connection reported `fts5: corruption found` while the writer was updating the FTS index. A check on another connection at the same moment said `ok`, and 800 concurrent searches on the readers never failed. The check now runs on the writer, and `tests/fts_concurrency.rs` covers this.
- **Not in M3's scope:**
  - Pending ops (step 3 and the overlay in step 5) arrive in M6.
  - The core and app wiring of actors and events arrives in M4.
- **Not scheduled:** no milestone row names `folder.extend_window` (Load older, sections 3 and 12). Sync's window logic already allows it, because older cached rows stay put. The owner should pick its milestone.

### Next
M4 (UI shell).
