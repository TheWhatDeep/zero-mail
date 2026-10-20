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

## M4: done, tagged `m4` (2026-10-20)

Scope: UI shell, virtualized list, plain-text reader (through `mail-body`, G42), sidebar, status bar, and a minimal add-account form (G43). Done when:
- 60 fps scroll on the 50,000-message fixture, plus the owner's own mail seen once (G41).
- T04's reading-pane part passes (G38).
- The startup budget is enforced (G44).

Every check below passes locally on Windows 10. CI is green on Windows and in the Stalwart job ([run 39320230240](https://github.com/TheWhatDeep/zero-mail/actions/runs/39320230240)). The startup harness there enforces the startup budget and the 100-row cap. The owner approved the dark palette (G5), and on 2026-10-20 he added his Workspace account in the release build: real mail appeared, opened as plain text and scrolled smoothly (G41).

### Built
- **`mail-core::Core`, the facade `app` calls:**
  - **Open path (section 12, G48):**
    - A file SQLite rejects is moved aside as `mail.db.corrupt-<time>`, a fresh database is created, and `storage-warning` (`db_corrupt`) fires.
    - A `quick_check` runs 10 s after start, on the writer. A failure writes a marker, and the next start moves the database aside.
  - **Actors:** one actor per account at start. They are started, reconfigured and stopped by `account.add`, `account.update` (also the fix for NeedsAttention), `account.set_enabled` and `account.remove`.
  - **Event pump:** `store-changed` is coalesced per (account, folder) into 100 ms windows, so a long sync updates the list at most 10 times a second. `account-state` (whose latest value `account.list` returns), `sync-progress` and `folder-reset` are forwarded as they come.
  - **Commands:** `account.list` (G46), `account.resolve`, `account.test`, `account.add`, `account.update`, `account.remove`, `account.set_enabled`, `folder.list`, `folder.refresh`, `list.page` and `message.open`.
    - `folder.list` returns the tree in section 9 order.
    - `list.page` serves folder scope with keyset cursors. Its first page marks the folder open. Other scopes and `group_by_thread` return `unsupported` until M9.
    - `message.open` returns the display model. An uncached body is fetched ahead of the queue.
- **`mail-render`:** the `mail-body` plain-text document (G42). The text is HTML-escaped with nothing added and sits on the white canvas. The response carries the section 8 CSP header with a fresh 128-bit nonce. The only script is the nonce-carrying one that posts the content height.
- **`app` (`src-tauri`):**
  - The eleven commands, with `.` written as `_`.
  - The `mail-body` protocol: the CSP is a response header (never a meta tag), plus `nosniff`, `no-referrer` and `no-store`.
  - Events go to the UI.
  - **Capabilities:** the capability file allows exactly these commands plus event listen (G49). The build generates one permission per command.
  - **Perf hooks:** a runtime capability in perf mode only (G49).
- **`mail-sync`:**
  - The open-time fetch caps a text part at 1 MB, which gives `body_state` `text`.
  - Each IDLE connection triggers one catch-up sync right after its SELECT. This closed a gap the test suite exposed (G49).
- **`mail-store`:** `get_message`, `get_body`, `list_attachments` and `quick_check`. The perf fixture generator is `cargo run --release -p mail-store --example fixture -- <dir> 50000`.
- **UI (Svelte 5, design 3a within G4, G6, G9 and G45):**
  - **Typed IPC:** `ipc.ts` mirrors the commands built so far, the 20 error codes and the event payloads.
  - **Shell:** sidebar on the left; the list and a 420 px reading pane in the main area, with banners above and the status bar below. First run shows only the add-account form.
  - **Sidebar:** accounts with their folders. Special folders come first, the rest form a tree, and unread badges show. Add account sits at the bottom, and the account name opens its settings.
  - **Message list:**
    - An ARIA listbox (arrows, Home, End), virtualized with at most 100 DOM rows; 31 is the most seen.
    - Rows load in keyset pages of 100. Pages more than 2 away from the window are dropped, and their cursors are kept.
    - Sticky Today and Earlier headers with counts.
    - Skeleton rows while a folder has never synced, and No messages when it is empty.
    - Each row shows: an unread dot and bold text; the sender, or the recipients in Sent and Drafts; subject and preview; star and attachment marks; and a local-time date.
  - **Reading pane:** the subject; the sender's name with the address beside it (section 13, item 10); To and Cc; the date. The plain text sits in the sandboxed frame without `allow-same-origin`, sized from its height messages. It shows Loading while the body is fetched and Message too large when the text was cut.
  - **T04:** a message deleted on the server keeps its body, with This message was deleted.
  - **Status bar:** one dot per account. A warning appears when an account has been in backoff for more than a minute, with its error.
  - **Banners and toasts:** Offline; an account that needs attention, with Sign in again or Review certificate; storage warnings. A folder reset shows a 10 s toast.
  - **Add-account form:** email, then resolve, then editable server fields and the app password. Test reports both halves. Save needs both to pass, and Save anyway appears after a failure. An untrusted certificate is shown with its fingerprint, grouped in fours, and can be trusted. In edit mode the password is optional and Remove account asks for confirmation.
  - **Icons:** Phosphor path data (G10, G47).
  - **Strings and tokens:** all strings are in `strings.ts`. Every value is in `tokens.css` (G50).
- **Perf harness** (`tests/perf/startup.mjs`):
  - Runs on a copy of the fixture: one discarded warm-up, then the median of 5 cold-app launches to painted inbox rows.
  - `--scroll` runs a 300-frame scroll that measures frame rate, p95 and the most DOM rows.
  - CI generates the fixture, enforces startup and DOM rows, and records the frame rate (G41).

### Tests added (34 local; workspace total 149 Rust and 35 UI)
- **`mail-core`:**
  - The pump coalesces `store-changed` into 100 ms windows, on paused time. State events are forwarded and remembered.
  - Folder tree order and depth. `list.page` request parsing.
  - **Core on the fakes:** add, actor Idle, `account.list` flattened with its state, `folder.list` roles, two keyset pages, `message.open` through prefetch, and `mail-body` escaped, with the CSP and a 404 for a missing body. Unified scope returns `unsupported`. T04's Core half: the expunged message is `not_found`. Remove.
  - A corrupt file is moved aside with a warning, and a rebuild marker is honoured.
- **`mail-render`:** hostile text is escaped and exactly one nonce script is present; the CSP follows section 8; nonces are fresh.
- **`mail-sync`:** an uncached message beyond the prefetch cap is fetched on open, and a 1.5 MB part is cut to exactly 1 MB with `body_state = text`. The QRESYNC test now checks one flag command per sync.
- **`app`:** `mail-body` URLs parse in both platform forms and reject paths. Responses carry the CSP header.
- **UI (17):**
  - Row dates. Initials. The address beside the name. Recipients in Sent and Drafts. Sizes.
  - Virtual layout: both headers, an unknown boundary, the 100-row cap, reveal-on-select.
  - **Keyset page cache:** ordered loading, eviction, a direct reload, refresh in place, and a gap-free reset when mail arrives on top.
  - **Reader (T04):** the deleted notice keeps the body; it re-asks only for its folder; it reloads when the body arrives.
  - Protocol URL mapping; AppError checks.
  - **Account form:** filling from a candidate, validation, the password sent once, the certificate to pin, both halves needed.
- **Perf on this machine** (owner's Windows 10, 144 Hz display, release build, fixture):

  | Measure | Result | Budget |
  | --- | --- | --- |
  | Startup, median of 5 | 732 ms (warm-up discarded: 2,214 ms) | 1,500 ms |
  | Scroll frame rate | 137.6 fps | 60 fps |
  | Scroll p95 frame time | 7.1 ms | |
  | Most list rows in the DOM | 31 | 100 |

- **Checks run here:**
  - `npm --prefix ui run build`.
  - `cargo fmt --all -- --check`.
  - `cargo clippy --workspace --all-targets -- -D warnings`.
  - `cargo test --workspace`: 149 passed, 5 ignored. The full suite ran twice with no flakes.
  - `npm test`: 35 passed. `npm run lint`: 0 errors, 0 warnings.
  - `node scripts/check-deps.mjs`.

### Deviations
None from spec v0.11. v0.11 records the owner's decisions G41–G48.

### Gaps found
- **G41–G48:** resolved by the owner (see above).
- **G49 and G50** (interpretations, owner to ratify):
  - G49: the event-listen permission, the perf-only capability, the data-dir override, folder-open through `list.page`, and the IDLE catch-up.
  - G50: the new visual values.
- **Size to watch:** the release exe is 16.2 MB, up from 7.8 MB at M0. The new network, TLS, MIME and charset dependencies account for the growth. The 15 MB budget is for the compressed installer, measured at M11.
- **Not scheduled:**
  - `folder.extend_window` (Load older; see M3).
  - Logging setup (section 13, item 8: 5 rotating files of 2 MB). `tracing-appender` rotates by time, not size, so size-based rotation needs a small writer of our own. No milestone names it.
- **Read state:** marking a message `\Seen` after 1.5 s needs `message.set_flags` and `ops`, which come in M6. Until then, opening a message does not mark it read.

### Next
Stop: the M5–M7 session starts next.

## M5: done, tagged `m5` (2026-10-20)

Scope: `render` (parser, sanitizer, protocols, attachments), logging and `log.open` (G51), and `link.open` and `message.save_source` (G52, G54). Done when: T19 to T24.

Every check below passes locally on Windows 10. CI is green on Windows, in the Stalwart job and in the startup harness with the T19 walk ([run 39360788410](https://github.com/TheWhatDeep/zero-mail/actions/runs/39360788410)).

### Built
- **`render` (`mail-render`), the HTML pipeline of section 8, every step at every render (invariant 4):**
  - **Sanitize:** an `ammonia` allowlist of structural, text, table, `img`, `a` and `style` elements with presentational attributes, `style` and `class`. Removed with their content: `script`, `iframe`, `frame`, `object`, `embed`, `applet`, `svg`, `math`, `audio`, `video`, form controls, `noscript`, `template` and `title`. Removed, content kept: `form` and anything not allowed. `base`, `meta` and `link` disappear. Every `on*` attribute, `id` and `name` go. Only `http`, `https`, `mailto` and `tel` links survive; images keep `cid:`, `http(s)` and non-SVG `data:image/` (which the CSP never loads).
  - **Rewrite URLs:** `cid:` becomes `mail-cid://<message>/<cid>`; every `http(s)` image, `background` attribute and CSS `url()` becomes `mail-remote://<encoded url>`; each `a href` moves to `data-href` with `href="#"`. CSS in `style` attributes and `<style>` elements: `@import` rules are removed, URL strings in `image-set()` and `@font-face` are rewritten, CSS escapes that spell names (`\75 rl(`, `@\69mport`) are resolved first, and `expression()`, `behavior` and `-moz-binding` are broken.
  - **Serve:** `mail-body://<message>?images=0|1&plain=0|1` with the section 8 CSP as a response header and a fresh nonce. `images=1` counts only for a message the user allowed (G57). URLs are written in the platform's form (`http://mail-cid.localhost/...` on Windows).
  - **The frame's one script** posts the content height, link clicks (target and visible text) and the hovered link to the parent. It prevents every navigation and form submit inside the frame.
  - **Plain text:** escaped, `http(s)` and `mailto` linkified, quoted blocks deeper than level 1 collapsed behind a `<details>` toggle.
  - **Remote-image proxy:** no cookies, no referrer, a generic user agent, a 10 s timeout, a 5 MB cap, `image/*` only, 4 requests at a time (G57).
  - **Attachments:** names decoded (RFC 2047 here, RFC 2231 when BODYSTRUCTURE is read), sanitized for saving (G55), the executable-type check, the on-disk byte cache (LRU, 500 MB, keyed by account, folder, UIDVALIDITY, UID and part), and the temp folders for Open (a random subfolder per file, deleted at quit and at the next start).
- **Parser (`imap`, `sync`):** `format=flowed` (with `delsp`) is undone when a text part is decoded; BODYSTRUCTURE carries `format` and `delsp` and decodes RFC 2231 file names. The actor fetches one part, transfer-decoded, or the raw source on demand at user priority and answers the caller; offline, the caller gets `network`. A body the server refuses permanently becomes Could not display this message.
- **Logging (G51, G57):** `tracing` to `<app data>/logs`, five files of 2 MB rotated by our own writer, level info. The setting `debug_logging` adds the IMAP and SMTP traces, which redact `LOGIN` and `AUTHENTICATE` arguments, SASL exchanges, every IMAP literal, quoted strings in `FETCH` responses, `AUTH` lines and the message between `DATA` and `.`. A connection traces only when the switch was on when it opened.
- **`mail-core`:** `message.open` returns `html`, `plain`, `loading` or `unreadable`, plus `truncated`, `has_html`, remote image state and attachments with ids and the executable flag. New: `images.allow_once` (this session), `images.allow_sender` (`remote_image_allow`), `attachment.open`, `attachment.save`, `message.save_source`, `link.open` (only `http`, `https`, `tel`) and `log.open`; the `mail-cid` and `mail-remote` protocol answers. Removing an account also drops its cached attachment files.
- **`app`:** the seven commands, the `mail-cid` and `mail-remote` protocols, logging at start. The opener and dialog plugins are used from Rust only: Save as with no path opens the native dialog with the sanitized name, and closing it is `cancelled` (G52, G54). The capability file lists the commands only.
- **UI:**
  - **Reading pane:** the HTML or plain body in the sandboxed frame; a Plain text / Formatted toggle when there is HTML; the remote-images notice with Load images and Always load from this sender; Message too large and Could not display this message, both with Save source (.eml).
  - **Links (T21):** only `http`, `https`, `mailto` and `tel`. When the visible text names another host, an inline confirmation shows both hosts; Cancel keeps the user in the app. `mailto` does nothing until compose (M7, G57).
  - **Attachments:** name, size, Open and Save as. Executable types ask first.
  - **Status bar:** the hovered link's real target; the backoff warning opens the log folder.
- **T19 harnesses (G56, G68):**
  - **CI (`tests/render/xss-walk.mjs`):** WebView2 on GitHub's runner never opens its DevTools port, so CI uses a walk instead (G68). In perf mode with `ZEROMAIL_PERF_WALK` the page opens every corpus message in turn and reports what it opened. The fixture points the corpus beacon at a local HTTP server the harness runs; any request reaching it fails the run, and a control request proves it listens.
  - **Local (`tests/render/xss-harness.mjs`):** the DevTools harness, which also records CSP-blocked attempts, stays as a tool for the build machine.

### Tests added (35 local; workspace total 179 Rust (6 container tests ignored locally) and 40 UI)
- **`mail-render`:** sanitizer structure and URL rewriting, style blocks, the link mark, CSS rewriting (escapes, imports, strings, legacy), plain text (linkify, quote collapse), the CSP, nonces, only images served as images, file names (G55), executable types, encoded names, the LRU cache, temp folders, the proxy (no cookie or referrer across two requests, image only, http only, the size cap).
- **T19 (Rust half):** 44 corpus cases, each in both URL forms: no forbidden element or attribute, no event handler, no `javascript:`, `vbscript:`, `@import`, `expression(` or foreign URL left in markup or CSS, and exactly one script in the served document.
- **T19 (webview half):**
  - **Walk (CI and here):** 44 of 44 corpus messages opened in the release build; nothing reached the beacon server.
  - **DevTools (here):** 44 messages opened through keyboard navigation with the pointer moved over each body, 44 `mail-body` loads seen across the page and its frames, 7 `mail-remote` loads stopped by the CSP, and zero requests to any other host. Any attempt on the beacon host, even one the CSP stopped, would fail the run.
- **`mail-core` on the fakes:** T20 (nothing fetched before consent, `images=1` ignored without it, the proxy sends no cookie or referrer, allow-once is per session, Always load persists across a restart and matches the sender case-insensitively); T22 (UTF-8, ISO-8859-1, Windows-1252, ISO-2022-JP body and subject, GB2312, Shift_JIS, a bogus label in UTF-8 and in 8-bit, and broken MIME); T23 (the 20 MB part never prefetched, fetched once on Save and then from the cache, `../x` and `CON.txt` saved as `..x` and `_CON.txt`, the executable flag, Open into a random temp folder, temp deleted at quit); T24 (served through `mail-cid`, no `data:` URI, non-image and unknown cids refused); the plain toggle, format=flowed and linkify end to end; Could not display and Save source, and `network` offline; `link.open`'s scheme check.
- **Stalwart:** an APPENDed multipart message through the real transport: sanitized HTML, the cid image, the base64 attachment saved, and Save source.
- **`mail-imap`:** multipart parts in the fake, `format=flowed` and RFC 2231 in BODYSTRUCTURE, IMAP trace redaction. **`mail-smtp`:** SMTP trace redaction. **`mail-sync`:** format=flowed, transfer decoding. **`mail-core`:** log rotation keeps 5 files of at most 2 MB; the debug filter adds only the traces.
- **UI:** link decisions (T21), host parsing and the body URL toggle.

### Deviations
None from spec v0.15. v0.13 records the owner's decisions G52–G54 and G56; v0.15 records G68.

### Gaps found
- **G52–G54, G56:** resolved by the owner (see above).
- **G68:** resolved by the owner: T19's webview half runs in CI as a walk against a local beacon server. Found because the DevTools harness passed here and could not start on the runner.
- **G55 and G57** (interpretations, owner to ratify): Windows-safe file names; the proxy's overall limit of 4; `images=1` only with consent; `mailto` inert until M7; the link host rule; where logs, cache and temp files live; the debug setting and what the traces redact.
- **For later (not in M5's scope):** the proxy fetches whatever host an allowed image names, as a browser would, including the local network; blocking private addresses would be a spec change. Read state after 1.5 s needs `message.set_flags` (M6).

## M6: done, tagged `m6` (2026-10-20)

Scope: `ops` (flags, move, delete, archive, folders, undo, Gmail quirks) and `folder.extend_window` (G51). Done when: T03, T06, T25 (rows and Archive; the views part moves to M9, G60), T26, T27 and T28.

Every check below passes locally on Windows 10. CI is green on Windows, in the Stalwart job (with the new M6 container test) and in the startup harness ([run 39367163002](https://github.com/TheWhatDeep/zero-mail/actions/runs/39367163002), on the M6 commit plus the docs-only spec v0.16).

### Built
- **The pending-op queue (`mail-store`):** each command writes its local change and its `pending_ops` row in one transaction. The local change is an overlay on the rows' flags: a star or read change sets its bit; a pending move, archive or delete sets bit 5, which every view already hides (G59). The flags each row had are saved with the op, so a cancel or a refusal restores exactly the overlay's bits. A server flag pull lays unfinished overlays back on in the same transaction (invariant 2), so a local change never flickers away. Finished moves stay as `done` with their COPYUID for the 10 s Undo window (G62). Ops of one command share an Undo group.
- **`ops` (`mail-sync`):**
  - Flags: `UID STORE +FLAGS.SILENT` / `-FLAGS.SILENT`, 500 UIDs per command.
  - Move, Archive, Delete to Trash: `UID MOVE`; without MOVE, `UID COPY`, `\Deleted` and `UID EXPUNGE` of exactly those UIDs; without UIDPLUS a bare `EXPUNGE` only when `UID SEARCH DELETED` shows no other flagged message, else the source stays flagged and hidden (section 9, T26).
  - Permanent delete and Empty Trash: `\Deleted` and the same expunge rule.
  - Targets that vanished from the server are dropped silently. A permanent refusal reverts the overlay and fires `op-failed` once (G58); a transient error leaves the op queued.
  - Folder create, rename and delete with modified UTF-7 names (T27). The local change and its op are one transaction; folder ops are pushed before the session's first `LIST`, and a `LIST` never drops a folder with an unacknowledged create or rename nor revives one with an unacknowledged delete (G62).
  - The actor pushes ops ahead of folder syncs (worker priority 2), at every connect (section 6: push first), and in step 3 of each folder sync; ops left `running` by a crash run again.
  - Load older (`folder.extend_window`, G51): 90 days before the oldest cached message; an empty stretch jumps to the next older message (G62).
  - Gmail (section 9, G61): All Mail, Important and Starred start with sync off; Important and Starred are hidden in the sidebar while off. Archive moves to All Mail, removing only the INBOX label (T25).
- **Undo (section 9):** a queued op is cancelled locally; a finished move goes back with COPYUID, or by `UID SEARCH HEADER Message-ID` without it (G62); a flag change is reversed; a permanent delete cannot be undone.
- **`mail-core`:** `message.set_flags`, `message.move`, `message.delete` (messages, or a folder for Empty Trash), `message.archive`, `message.mark_all_read`, `message.undo`, `folder.create`, `folder.rename`, `folder.delete` (special-use folders refused), `folder.set_role` (one folder per role), `folder.set_sync`, `folder.set_watch` (at most 3), `folder.extend_window`; events `pending-count` and `op-failed`. `message.open` says whether Archive exists and whether Delete is permanent; `folder.list` marks hidden folders.
- **`app`:** the thirteen commands, in the capability file.
- **UI:**
  - **Reading pane toolbar:** Archive (only with an archive folder), Delete (asks first when permanent), Star, Mark read/unread, Move to. Archive, Delete and Move show the 10 s Undo toast.
  - **Read state:** a message shown 1.5 s is marked read, once per opening (section 8).
  - **List:** Mark all read; Empty Trash or Junk with the count, asking first; Load older at the end.
  - **Status bar:** the pending count. **Toasts:** Undo, and the op-failed message.
  - **Account settings, Folders:** every folder with its role, sync and watch toggles, Rename and Delete where allowed, and New folder.
  - The remote-images notice now wraps its buttons below the text (M5 polish found in a visual check).

### Tests added (16 local Rust and 4 UI; workspace total 194 Rust (7 container tests ignored locally) and 44 UI)
- **`mail-core` on the fakes:**
  - **T03:** another client's read and a local star meet within 2 s with IDLE; pending returns to 0.
  - **T06:** offline star, move and delete of 3 messages: the list changes at once, nothing reaches the server, pending is 3; after reconnect the server matches and pending is 0.
  - **T25 (M6 part):** on the Gmail fake a message under INBOX and Work is two rows with one Message-ID; Archive moves to All Mail, Work keeps its row and All Mail holds one copy; Important and Starred start hidden and sync on shows them.
  - **T26:** without MOVE (COPY, `\Deleted`, UID EXPUNGE, no bare EXPUNGE); without UIDPLUS with another `\Deleted` message present (nothing expunged, the source hidden); without UIDPLUS alone (one bare EXPUNGE).
  - **T27:** `Café/日本語` created, renamed and deleted; the folder list matches the server; special-use delete, INBOX rename and duplicates refused.
  - **T28:** Archive then Undo after the server acted, with UIDPLUS (COPYUID) and without (Message-ID), flags intact; and Undo while queued (nothing reaches the server).
  - A refused move is reverted and reported once as `op-failed` with `server_rejected`.
  - Load older: 90 days per click, and a gap is jumped.
  - Mark all read, Empty Trash, role override, the watch limit, sync off.
- **`mail-store`:** overlays laid, laid again after a pull, lifted on cancel and failure; Undo groups; done ops keep their result and are purged; pending folder ops win over `LIST`.
- **Stalwart:** `Café/日本語 …` created, renamed and deleted through the real transport, and Delete to Trash then Undo with the star intact.
- **UI:** move targets, hidden folders, the permanent-delete rule, Empty, folder edit rules, the read delay.

### Deviations
None from spec v0.15. v0.14 and v0.15 record the owner's decisions G58–G61 and G63–G66, G68.

### Gaps found
- **G58–G61:** resolved by the owner (see above).
- **G62** (interpretations, owner to ratify): Undo's done rows and groups, Gmail Archive target, delete confirmation, Load older's window, read state, single selection, the Message-ID search, folder ops and LIST, flag pulls in one transaction, which folders G61 hides, and Undo of flag changes.
- **Races found and fixed by the tests:** a `LIST` in flight could drop a folder created a moment before; a flag pull could briefly hide a fresh star; Undo by Message-ID could run before the destination synced. Each now has a test.

## M7: done, tagged `m7` (2026-10-20)

Scope: `smtp`, `outbox`, compose window, drafts, identities, contacts. Done when: T10 to T18.

Every check below passes locally on Windows 10. CI is green on Windows, in the Stalwart job (with the new M7 container test) and in the startup and T19 harness ([run 39371601137](https://github.com/TheWhatDeep/zero-mail/actions/runs/39371601137)).

### Built
- **`smtp`:**
  - The message builder (section 7) writes these headers: Message-ID `<uuid@from-domain>`, Date, From, To, Cc, Reply-To, Subject, In-Reply-To and References, MIME-Version and Content-Type. It never writes a Bcc header or X-Mailer.
  - The body is `multipart/alternative` (plain and HTML), or plain only. Attachments go in `multipart/mixed`, and a forwarded `.eml` as `message/rfc822`. Text is UTF-8 quoted-printable, attachments base64, and non-ASCII headers RFC 2047.
  - The envelope is To, Cc and Bcc, deduplicated.
  - Submission sends `RCPT` for every recipient and stops before `DATA` if any recipient is refused (T15).
  - An error after `DATA` is reported as ambiguous, never as safe to retry.
  - `SIZE` is read from EHLO.
- **`outbox`:** the section 7 state machine.
  - States: `queued`, then `sending` (claimed durably), then `sent`, `retry_wait`, `failed` or `unknown`.
  - Retry waits are 30 s, 2 min, 10 min, then 1 h. The sixth failure becomes `failed` (G67).
  - Items left in `sending` become `unknown` at start-up and are never retried automatically (T13).
  - After a send, Core queues the Sent copy as an `ops` APPEND with `\Seen`. With `save_sent_copy: auto` there is no copy on `X-GM-EXT-1` servers (T11, G66). Core also sets `\Answered` or `$Forwarded` on the original, removes the draft, and weights the recipients 3 in `contacts`.
- **Drafts:**
  - Saved locally 5 s after typing stops.
  - Uploaded 30 s after the last change and when the window closes: an `ops` APPEND with `\Draft`, then the old UID is deleted (T18).
  - Sending or discarding removes the draft.
- **`mail-core`:**
  - Compose commands: `compose.new`, `compose.reply`, `compose.reply_all` (own identities excluded, deduplicated case-insensitively, T17), `compose.forward` (inline or as attachment).
  - Draft commands: `draft.save`, `draft.discard`, `draft.add_attachment`, `draft.remove_attachment`, `draft.send`.
  - Before queueing, `draft.send` checks SIZE and APPENDLIMIT when the server is reachable (G65, T16).
  - Outbox commands: `outbox.undo`, `outbox.retry`, `outbox.discard`, `outbox.list` (G69).
  - Identity commands: `identity.list`, `identity.save`, `identity.delete` (one default, never zero; G64).
  - `contacts.suggest`: synced From, To and Cc count 1 use, sent recipients count 3. Suggestions rank by use, then recency, at most 10.
  - Event `outbox-state`. `pending-count` now includes the outbox.
  - The composer's HTML goes through the `ammonia` allowlist before building. Links and web images keep their real URLs (G67).
- **`app`:**
  - The 17 commands, added to the capability file.
  - Compose windows (`compose-<draft>`, G63) are opened by the app with the draft in a start-up script.
  - Send and Discard close the window; Edit and Undo reopen it.
  - Closing a window flushes the last edit before it closes (G67).
- **UI:**
  - **Compose window:**
    - From picker, and To/Cc/Bcc chips with autocomplete; red chips block sending.
    - Subject, and a rich-text body (bold, italic, underline, lists, link) with a plain-text toggle.
    - Attachments by button or drag and drop.
    - The first body has the cursor line, then the signature, then the quote: `On <local date>, <name> <address> wrote:` with the original's plain text, escaped.
    - Send on Ctrl/Cmd+Enter, with warnings for an empty subject and for a missing attachment.
  - **Main window:**
    - Compose button. Reply, Reply all, Forward and Forward as attachment in the reader. `mailto:` links open compose (G57).
    - Sending toast with Undo.
    - Per-account Outbox view with Retry, Edit and Discard, and a sidebar badge for failed and `unknown` items.
    - Identities card in account settings.

### Tests added (18 local Rust, 1 container, 11 UI; workspace total 212 Rust (8 container tests ignored locally) and 55 UI)
- **`mail-core` on the fakes (`tests/send.rs`):**
  - **T10:** To, Cc and Bcc each get one copy; no Bcc header.
  - **T11:** one Sent copy with `\Seen`; none on the Gmail fake.
  - **T12:** Undo at 5 s of a 10 s window; nothing reaches SMTP and the draft is back.
  - **T13:** a crash after `DATA` restarts as `unknown` and is never resent.
  - **T15:** a 550 recipient fails before `DATA`, and `failed` lists the address.
  - **T16:** an attachment over SIZE is refused before queueing.
  - **T17:** reply-all with the user's alias in Cc: own addresses out, no duplicates, In-Reply-To and References right.
  - **T18:** one draft on the server at all times; the old UID goes after each upload.
  - A sent draft leaves no Drafts copy. Identities keep a default.
- **`mail-outbox`, T14:** 451 then recovery follows the retry schedule and delivers once.
- **`mail-smtp`:** headers and structure (section 7), plain only, attachments and forwards, SIZE parsing.
- **`mail-render`:** the outgoing sanitizer keeps real URLs and strips scripts and handlers.
- **Stalwart (CI):** Alice sends To Bob, Cc Carol, Bcc herself. Each mailbox gets one copy, there is no Bcc header, and the Bcc address is invisible. Alice's Sent holds exactly one copy with `\Seen` (T10, T11 first half).
- **UI:** address parsing and red chips, chip deduplication, both send warnings (quote and signature ignored), the first body for replies and forwards, `mailto:` parsing, and the outbox badge and filter.
- **Visual check:** the compose window and the Outbox view in a browser with stubbed IPC. This found the quote missing its `> ` markers in the plain part and the trimmed `-- `; both are fixed.

### Deviations
None from spec v0.16.

### Gaps found
- **G63–G66, G69:** resolved by the owner.
- **G67** (interpretations, owner to ratify): quoting, rich text, contacts, the undo window, the retry schedule, outgoing HTML, the arguments of `compose.new` and `draft.add_attachment`, Edit as `outbox.undo`, the plain part, the close flush, forward-as-attachment in draft uploads, signatures until M10, and the timing test hook.
- **Found by review:** the composer's HTML first went through the display sanitizer, which would have sent `href="#"` links and `mail-remote://` images. It now has its own outgoing variant, with a test.
