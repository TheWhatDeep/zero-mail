# BUILD.md — Lightweight Desktop Email Client

Spec v0.15 (build-ready), amended 2026-10-20 with the owner decisions recorded in SPEC_GAPS.md. This file is the complete contract between Deep (owner) and Claude Code (builder). Part A tells the agent how to work. Part B is the specification. Where this file is silent, the agent stops and asks; it never improvises.

---

# PART A — INSTRUCTIONS FOR CLAUDE CODE

## A1. How to use this file

1. Read Part A, then Part B in full, before writing any code.
2. Copy section A3 into `CLAUDE.md` at the repo root on the first commit.
3. Build milestone by milestone (Part B, section 16). One milestone per session, except that M2 to M10 run in three sessions: M2–M4, M5–M7 and M8–M10 (SPEC_GAPS G28). Do not start M(N+1) until M(N) is tagged.
4. At the start of every session, re-read `CLAUDE.md`, `SPEC_GAPS.md` and the last milestone report in `docs/REPORTS.md`.
5. The owner runs the commands himself. When you need him to run something, give each shell command in its own fenced block, one command per block, never chained.

## A2. The owner's standing rules

- The spec is the architecture. Matching the architecture matters more than matching any feature list.
- Guardrails are absolute. If following the spec exactly seems wrong, write the problem to `SPEC_GAPS.md` and stop. Do not patch around it.
- No unauthorized visual choices. Tokens only.
- Budget-conscious: fewer dependencies, fewer moving parts. If two designs meet the spec, pick the one with less code.
- Tests are the definition of done. A milestone with red tests is not done.

## A3. CLAUDE.md (copy verbatim into the repo)

```markdown
# CLAUDE.md

This repo is built from docs/BUILD.md. Read it before anything else. The spec in Part B is the contract; Part A is how to work.

## Rules
- Architecture, schema, module boundaries and dependencies are fixed by the spec. Do not add a crate, table, column, module, command or event that is not in the spec. Write a SPEC_GAPS.md entry and stop instead.
- Rust: `#![forbid(unsafe_code)]`, no unwrap()/expect() outside tests, errors via thiserror, timeout on every network op, `cargo clippy -- -D warnings`, `cargo fmt`.
- TypeScript: strict, no `any`. Svelte 5 runes. Styling only through src/tokens.css variables. No UI kit, no icon font, no remotely loaded fonts (Geist is bundled locally).
- Security layers are mandatory from the first commit that touches them: ammonia sanitizer, sandboxed iframe without allow-same-origin, header CSP on mail-body://, secrets only in the OS keychain. No TODO placeholders for these.
- Automated tests run only against the Stalwart container and the fake ImapTransport. Never against a real mailbox.
- Each milestone ends with: all tests green, `docs/REPORTS.md` entry, git tag m<N>.
- Milestone report format: built / tests added / deviations (must be none) / gaps found.

## Commands
- Backend tests: `cargo test --workspace`
- Frontend tests: `cd ui && npm test`
- Lint: `cargo clippy --workspace -- -D warnings && cd ui && npm run lint`
- Integration server: `docker compose -f tests/docker/compose.yml up -d`
- Dev run: `npm run tauri dev`

## Layout
See BUILD.md Part B section 16, "Repository layout". Crate boundaries enforce module boundaries.
```

## A4. Per-milestone prompt

Use this exact shape when starting a milestone session:

```
Build milestone M<N> from docs/BUILD.md section 16.
Scope: <the "Builds" cell for M<N>>.
Done when: <the "Done when" cell for M<N>>.
Read CLAUDE.md, SPEC_GAPS.md and the last entry in docs/REPORTS.md first.
Do not touch anything outside this milestone's scope.
When done: run the full test suite, write the milestone report to docs/REPORTS.md, commit, tag m<N>, and stop.
```

## A5. Environment the owner provides

- Rust stable (1.80+), Node 20+, Docker, platform Tauri 2 prerequisites.
- A Google Cloud project with an Internal OAuth client of type Desktop app (for T31/T32 manual tests only).
- A Workspace app password for manual testing.
- The app name: **Zero Mail**, identifier `com.digitalcircuitry.zeromail` (decided 2026-10-19).

## A6. Definition of complete

The build is complete when M11 is tagged, every automated test in section 15 passes in CI on all three OSes, every manual test in `docs/MANUAL_TESTS.md` has been run by the owner, every section 14 budget is met on the reference dataset, and the owner has used the app as his daily Workspace client for one week with no lost mail and no duplicate sends.

---

# PART B — SPECIFICATION

## 1. Purpose and non-goals

A fast, small desktop email client for any IMAP/SMTP account, with offline reading and nothing phoning home. One person builds it, so every decision favors fewer moving parts.

**Principles.** When two conflict, the lower number wins.

1. The server is the source of truth. The local database is a cache plus a queue of pending actions. Deleting it loses only settings, unsent outbox items and drafts not yet uploaded.
2. Never lose a message. Sends and server-side changes are written to a durable queue before any network call.
3. Safe by default. Nothing in a message loads, runs or phones home unless the user allows it.
4. Generic IMAP/SMTP first. No provider API (Gmail API, Graph) anywhere. The login method is chosen by host (OAuth for Google); every other provider quirk is detected by server capability, never by domain name.
5. Lightweight is a feature with budgets (section 14). A feature that breaks a budget is redesigned or rejected.

**Non-goals.** Calendar and contacts sync, PGP/S-MIME, Exchange ActiveSync/MAPI/JMAP, Microsoft 365 and Outlook.com accounts (Microsoft requires OAuth for IMAP, and Microsoft OAuth is out of scope), mobile apps, running a mail server (the client treats a self-hosted server as just another account), AI features, plugins, telemetry or crash reporting, and any hosted backend.

**Done means:** the author uses it as the daily Workspace client for a week with no lost mail, no duplicate sends, and every budget in section 14 met.

## 2. Platform and stack

Tauri 2 desktop app for Windows, macOS and Linux. Rust does all network, storage and security work; the web frontend only displays state and sends commands.

| Layer | Choice | Rule |
| --- | --- | --- |
| Shell | Tauri 2.x | OS webview, no bundled browser. Capability file allows only the app's own commands. |
| Backend | Rust stable, tokio | All I/O async with timeouts. |
| IMAP | `async-imap` behind our own `ImapTransport` trait | Only the `imap` module imports the crate, so it can be swapped. |
| TLS | `rustls`, `rustls-platform-verifier` | No OpenSSL dependency. Certificates verify against the OS trust store; per-account pins on top (SPEC_GAPS G30). |
| SMTP and MIME build | `lettre` (tokio, rustls) | One crate for submit and message building. |
| MIME parse | `mail-parser` | Charset conversion included. |
| HTML sanitizer | `ammonia` | Allowlist-based. |
| Database | SQLite via `rusqlite` (bundled, WAL, FTS5) | Single writer task, pooled readers. |
| Secrets | `keyring` | OS keychain only. |
| OAuth | `oauth2` crate | PKCE, loopback redirect. Freshness waiver granted 2026-10-19 (SPEC_GAPS G12). |
| Errors, logs | `thiserror`, `tracing`, `tracing-subscriber`, `tracing-appender` | No other logging or error crates. |
| Serialization | `serde`, `serde_json` | IPC payloads and JSON columns. |
| HTTP | `reqwest` (`rustls-no-provider`, no default features) | Remote-image proxy, unsubscribe POST, OAuth token endpoint and autoconfig lookups only (SPEC_GAPS G29). |
| Small utilities | `uuid`, `sha2`, `rand`, `futures`, `socket2` | Message-ID, certificate pins, CSP nonces, async-imap streams, the 60 s TCP keepalive (SPEC_GAPS G31). |
| Tauri plugins | Official only: notification, opener, dialog, single-instance, autostart, window-state | No shell or broad fs plugin. |
| Frontend | Svelte 5, TypeScript strict, Vite, plain CSS variables | No component library, no icon font, no remotely loaded fonts. Geist (SIL OFL) ships as local font files. |
| Tests | `cargo test`, Vitest, Stalwart in Docker | Stalwart is the integration IMAP/SMTP server. |

Target OS: Windows 10/11, current and previous two macOS releases, and Linux with WebKitGTK 4.1. Until M11, builds and CI tests run on Windows only; the Stalwart integration job runs on Linux (SPEC_GAPS G24). Any dependency not in this table needs a spec change before code.

**Dependency freshness.** M0 checks every crate in this table: last release within 12 months, no open security advisory, exact version pinned in `Cargo.lock`. A crate that fails goes into `SPEC_GAPS.md` with a proposed replacement, and the build stops there. `async-imap` is the one to watch; the `ImapTransport` trait exists so it can be swapped without touching `sync`.

## 3. Architecture

Three layers: UI (Svelte, display only), Core (Rust, all logic) and the mail servers. The UI talks to Core through typed commands and events only. It never sees a password, token, socket or the database file.

```
UI (Svelte, display only)
   | commands            ^ store-changed events
   v                     |
Core (Rust) — higher rows call lower rows, never the reverse
   app      : commands, events, windows, tray, notifications
   sync | ops | outbox | render | search
   auth | imap | smtp | store | secrets
   |      |      |      |       |
   v      v      v      v       v
 mail servers + OAuth   SQLite  OS keychain
```

| Module | Owns | May call | Must never |
| --- | --- | --- | --- |
| `store` | Schema, migrations, queries, FTS | nothing | Contain protocol or UI logic |
| `secrets` | Keychain read and write | nothing | Return a secret to the UI or a log |
| `imap` | Transport trait, command wrappers, capability detection, quirks | nothing | Touch the database |
| `smtp` | Submission, outgoing MIME build | nothing | Touch the database |
| `auth` | Password and XOAUTH2 login, token refresh, OAuth flow, server-settings resolution (autoconfig, SPEC_GAPS G29) | `secrets` | Persist an access token |
| `sync` | Per-account actors, IDLE, backoff, applying server state to the store | `imap`, `store`, `auth` | Render or sanitize HTML |
| `ops` | Pending-op queue: flags, moves, deletes, folder changes | `imap`, `store` | Block the UI |
| `outbox` | Send queue, undo window, Sent copy, draft upload | `smtp`, `imap`, `store` | Retry a message already handed to SMTP |
| `render` | MIME to display model, sanitizer, `mail-body`, `mail-cid`, `mail-remote` protocols | `store` | Fetch from the network except through the remote-image proxy |
| `search` | Query parser, FTS5 queries, server search | `store`, `imap` | Write to the store |
| `app` | Tauri commands, events, windows, tray, notifications | all of the above | Hold business logic |

**Process model.** One tokio runtime. One `AccountActor` per account owns that account's IMAP connections (section 6). Actors talk through channels. The store has one writer task and a pool of read connections.

**One data path.** Server to `sync` to `store`, then a `store-changed(account, folder)` event, then the UI re-queries. The UI never reads data straight from a sync result. For user actions: UI command, `ops` writes the local change and a queue row, UI refreshes from the store (optimistic), `ops` pushes to the server, and on permanent failure reverts and emits an event.

**Command and event contract.** This is the whole UI/Core surface. Adding a command is a spec change. Every command returns `Result<T, AppError>` where `AppError` is `{ code, message, account_id? }` and `code` is a fixed enum; the UI never sees a raw error string. The codes are: `network`, `timeout`, `certificate`, `tls`, `auth_failed`, `oauth`, `throttled`, `server_rejected`, `recipient_rejected`, `too_large`, `unsupported`, `not_found`, `conflict`, `invalid_input`, `keychain`, `disk_full`, `db_corrupt`, `io`, `cancelled`, `internal` (meanings in SPEC_GAPS G18).

| Command | Arguments | Returns |
| --- | --- | --- |
| `account.resolve` | email | server config candidates and offered auth methods |
| `account.test`, `account.add`, `account.update`, `account.remove` | config, secret handled in Core | account summary |
| `account.oauth_start` | account_id | opens the browser; completion arrives as an event |
| `account.set_enabled` | account_id, bool | none |
| `account.list` | none | every account summary with its current state and last error (SPEC_GAPS G46) |
| `folder.list` | account_id | folder tree with counts and roles |
| `folder.create`, `folder.rename`, `folder.delete`, `folder.set_role`, `folder.set_sync`, `folder.set_watch` | account_id, path, value | none |
| `folder.refresh` | account_id, folder_id | none; progress arrives as events |
| `folder.extend_window` | account_id, folder_id | none; loads 90 more days |
| `list.page` | scope (folder, unified, search, thread), cursor `(date, id)`, limit, group_by_thread | rows plus next cursor |
| `message.open` | message_id | display model: headers, body kind, `mail-body` URL, attachment list, remote image state |
| `message.set_flags`, `message.move`, `message.delete`, `message.archive`, `message.mark_all_read` | message_ids or folder_id, value | op_id for Undo |
| `message.undo` | op_id | none |
| `message.save_source` | message_id, path? (none: the app shows a native Save dialog, SPEC_GAPS G54) | none |
| `message.snooze`, `message.unsnooze` | message_ids, due_utc | op_id |
| `message.mute`, `message.unmute` | thread_id | none |
| `message.add_label`, `message.remove_label` | message_ids, folder_id | op_id |
| `message.unsubscribe` | message_id | method used |
| `message.print` | message_id or thread_id | none |
| `attachment.open`, `attachment.save` | attachment_id, path? (none: native Save dialog, G54) | none |
| `link.open` | url (`http`, `https` or `tel` only) | none; opens the system browser or handler (SPEC_GAPS G52) |
| `images.allow_once`, `images.allow_sender` | message_id | none |
| `compose.new`, `compose.reply`, `compose.reply_all`, `compose.forward` | message_id?, mode | draft_id and prefilled draft; the app opens the draft's compose window (SPEC_GAPS G63) |
| `identity.list`, `identity.save`, `identity.delete` | account_id, identity, identity_id | identities; one default per account, never zero (SPEC_GAPS G64) |
| `draft.save`, `draft.discard`, `draft.add_attachment`, `draft.remove_attachment`, `draft.apply_template` | draft_id, content | none |
| `draft.send` | draft_id, identity_id, optional send_at | outbox_id |
| `outbox.undo`, `outbox.retry`, `outbox.discard` | outbox_id | none |
| `rules.list`, `rules.save`, `rules.delete`, `rules.reorder`, `rules.run_now` | rule or ids | list |
| `templates.list`, `templates.save`, `templates.delete` | template | list |
| `contacts.suggest` | prefix, account_id | up to 10 matches |
| `search.query` | text, scope, server_too | same shape as `list.page` |
| `settings.get`, `settings.set` | key, value | value |
| `log.open` | none | opens the log folder |

| Event | Payload | Fired when |
| --- | --- | --- |
| `store-changed` | account_id, folder_id | after any committed write; debounced 100 ms |
| `account-state` | account_id, state (section 6), last_error | on every state change |
| `sync-progress` | account_id, folder_id, fetched, total | during a first sync or window extension |
| `pending-count` | total queued ops and outbox items | on change |
| `outbox-state` | outbox_id, state | on every outbox transition |
| `oauth-done` | account_id, ok, error? | when the loopback redirect completes |
| `notify` | account_id, message_id, sender, subject | for a new message that qualifies (section 11) |
| `snooze-due` | account_id, message_id | when a snooze fires |
| `storage-warning` | kind (disk_full, db_corrupt) | once per incident |
| `folder-reset` | account_id, folder_id | once, after a UIDVALIDITY purge commits (section 12 toast, SPEC_GAPS G37) |
| `op-failed` | account_id, op_id, kind, error | once, after a permanently failed op is reverted (section 9, SPEC_GAPS G58) |

## 4. Data model

SQLite in WAL mode with foreign keys on and forward-only migrations (`PRAGMA user_version`). Messages are keyed by folder and UID, so a Gmail message under two labels is two rows. Column lists below are the contract; types and indexes are the builder's job.

```sql
accounts(id, name, email, imap_host, imap_port, imap_security, smtp_host, smtp_port, smtp_security,
  username, auth_kind /* password | oauth_google */, oauth_client_id,
  pinned_cert_sha256 NULL, sync_days DEFAULT 90, save_sent_copy /* auto | on | off */,
  signature_html, signature_text, enabled, created_at)
identities(id, account_id, name, address, reply_to NULL, is_default)
folders(id, account_id, path, delimiter, special_use NULL /* inbox sent drafts trash junk archive all */,
  uidvalidity, uidnext, highestmodseq NULL, sync_enabled, watch_idle, unread, total, synced_at,
  UNIQUE(account_id, path))
messages(id, account_id, folder_id, uid, gm_msgid NULL, gm_thrid NULL, message_id, in_reply_to NULL, refs NULL,
  gm_labels_json NULL, thread_id, subject, from_name, from_addr, to_json, cc_json, reply_to_json,
  date_utc, internal_utc, size, flags /* bit0 seen, 1 flagged, 2 answered, 3 draft, 4 forwarded, 5 deleted or pending move/delete (hidden, SPEC_GAPS G19, G59) */,
  has_attach, preview, body_state /* none | text (a part cut at 1 MB) | full (every text part whole), SPEC_GAPS G35 */, UNIQUE(folder_id, uid))
bodies(message_id, text_plain, text_html, fetched_at)
attachments(id, message_id, part_id, filename, mime, size, content_id NULL, is_inline)
threads(id, account_id, last_date_utc, subject_norm)
outbox(id, account_id, identity_id, raw_mime, envelope_json, state, send_after_utc, attempts, last_error,
  created_at)
pending_ops(id, account_id, folder_id, uids_json, kind, args_json, state, attempts, last_error, created_at)
drafts(id, account_id, imap_uid NULL, content_json, updated_at, uploaded_at NULL)
contacts(account_id, address, name, last_used_utc, use_count)
remote_image_allow(account_id, sender_addr)
snoozes(message_id, account_id, due_utc, origin_folder_id)
muted_threads(account_id, thread_id)
rules(id, account_id NULL /* all */, position, name, enabled, conditions_json, actions_json, stop_processing)
rules_applied(message_id, rule_id)
templates(id, name, subject, body_html, body_text, updated_at)
settings(key, value_json)
message_fts  -- FTS5 over subject, from, to/cc, body text; external content
```

**Invariants**

1. `(folder_id, uid)` is unique. A UIDVALIDITY change deletes every message, body and attachment row of that folder in one transaction.
2. `flags` is what the user sees: server flags with pending ops laid on top. After every server pull, sync recomputes that overlay.
3. Raw message source is not stored. Only parsed text and HTML parts and attachment metadata are. Attachment bytes are fetched on demand into an on-disk cache (LRU, 500 MB default cap).
4. HTML is stored unsanitized and sanitized at every render, so sanitizer fixes apply to old mail.
5. No secret is ever in the database. Accounts reference keychain entries by account id.
6. All times are UTC epoch seconds; the UI converts to local time.
7. Deleting a folder cascades to its messages, bodies and attachments. Headers are never auto-evicted; a Clear body cache action drops bodies and attachment files only.
8. Messages flagged `\Deleted` on the server are hidden in every view.

## 5. Accounts and authentication

An account is a server config plus a login method. Nothing else differs between Workspace, Fastmail or a self-hosted server.

**Setup flow**

1. The user enters an email address.
2. Server settings resolve in this order: built-in table (Google, Fastmail, iCloud, Yahoo), the domain's own autoconfig URL, the Mozilla ISP database, then the manual form (SPEC_GAPS G27). Autoconfig XML is read by a small hand-written reader (G26). Online lookups use HTTPS only and send only the domain, and a setting turns them off (G29).
3. The app reads `CAPABILITY` and offers a login method: OAuth for Google hosts, password for everything else.
4. Test connection: IMAP login plus `LIST`, SMTP `EHLO` plus `AUTH`. No message is sent. The account saves only if both pass, unless the user picks Save anyway.
5. Folder roles come from SPECIAL-USE, then name heuristics. The user can override each.

**Transport security.** IMAP on 993 with implicit TLS (default) or 143 with required STARTTLS. SMTP on 465 implicit TLS or 587 with required STARTTLS. If STARTTLS is not offered the connection aborts; the app never continues in plaintext and never offers a no-encryption option. TLS 1.2 or newer, certificate verification always on.

| Method | Used for | How |
| --- | --- | --- |
| App password | Any server | `AUTHENTICATE PLAIN` over TLS (fallback `LOGIN`). Password in keychain. |
| XOAUTH2, Google | Workspace | Desktop-app client type, PKCE, loopback redirect to `127.0.0.1` on a random port, scope `https://mail.google.com/`. |

**Bring-your-own OAuth client.** The app ships no client IDs or secrets. The user creates an OAuth client in their own Google Cloud project (set to Internal, so no Google verification) and pastes the client ID during setup. Consequence: personal gmail.com accounts use app passwords.

**Tokens.** Access tokens live in memory only and refresh 5 minutes before expiry. The refresh token lives in the keychain. On `invalid_grant` the account goes to `NeedsAttention` (section 6): connections stop, a banner says Sign in again, and nothing retries. Passwords sit under keychain key `account:<id>:password`.

**Certificates.** An invalid or changed certificate is a hard failure that shows its details. The user may pin the server certificate's SHA-256 per account, which is how self-signed or self-hosted servers work. A pinned certificate that later changes is a hard failure again.

**Accounts and identities.** Unlimited accounts, each independent; one failing account never blocks another. Every account has at least one identity (display name, address, optional reply-to). Extra send-as addresses are typed in by hand. Removing an account deletes its rows, keychain entries and cached files, never anything on the server, and asks for confirmation.

## 6. Sync engine

Each account runs one sync actor. It pushes queued local changes first, then makes the cache match the server for enabled folders, then waits for news via IDLE or a timer.

**Account states.** Disabled → Connecting (TLS, then login) → Syncing (push, then pull) → Idle (waiting in IDLE). Connecting or Syncing or Idle → Backoff on any network error, then back to Connecting when the timer fires. Connecting → NeedsAttention on a bad login or certificate; only the user's fix returns it to Connecting. Any state → Disabled when the user turns the account off. Network and server trouble loops through Backoff on a timer; only a bad login or certificate stops the loop and waits for the user.

**Connections per account.** One IDLE connection on INBOX and one worker connection for everything else. The user may mark up to 3 more folders as watched, each with its own IDLE connection. Hard cap: 5 connections per account.

**Folder order.** On first sync and after every reconnect: INBOX, then Drafts, Sent and Archive, then the remaining enabled folders alphabetically. INBOX headers are visible before any other folder is touched. Only one folder syncs at a time per account; the worker queue (below) interleaves user requests.

**Capabilities.** Plain IMAP4rev1 must work with no extensions. These are used only when the server advertises them: IDLE, MOVE, UIDPLUS, CONDSTORE, QRESYNC, SPECIAL-USE, LIST-STATUS, APPENDLIMIT, and `X-GM-EXT-1` (Gmail quirks, section 9). Without IDLE the actor polls INBOX every 2 minutes.

**Folder sync procedure**

1. `SELECT` the folder. Read UIDVALIDITY, UIDNEXT, EXISTS and HIGHESTMODSEQ.
2. If UIDVALIDITY differs from the stored value, purge the folder's cache (invariant 1), store the new value and treat this as a first sync.
3. Push first: run this folder's `pending_ops`. Only then pull.
4. New mail: `UID FETCH` UIDs above the highest cached one for `UID FLAGS INTERNALDATE RFC822.SIZE BODYSTRUCTURE` plus selected headers via `BODY.PEEK`, in batches of 100. First sync limits to the last `sync_days` (default 90) via `UID SEARCH SINCE`, newest first. Each batch is one database transaction, so a kill between batches is safe.
5. Flags: with CONDSTORE, fetch flags `CHANGEDSINCE` the stored modseq. Without it, fetch flags for all cached UIDs in batches. Then re-lay pending ops over the result.
6. Deletions: with QRESYNC use `VANISHED`. Otherwise `UID SEARCH ALL` and delete local rows whose UID is gone.
7. Update folder counters and emit `store-changed`.
8. Body prefetch: at lowest priority, fetch text and HTML parts (each up to 1 MB) for the newest 200 messages per folder. Opening an unfetched message jumps the queue.
9. Wait: the INBOX connection sits in IDLE and re-issues it every 25 minutes. Any `EXISTS`, `EXPUNGE` or `FETCH` notice ends IDLE and reruns steps 1 to 7. Other folders sync when opened, on manual refresh, and every 5 minutes while open in the UI.

**Worker queue priority.** 1: user-initiated fetches and actions. 2: pending ops. 3: folder sync. 4: prefetch.

**Retry and throttling.** Backoff starts at 2 s, doubles to a 5 minute cap, with plus or minus 20% jitter, and resets after a connection stays healthy for 60 s. A throttle response (such as Gmail's command or bandwidth limit message, `[LIMIT]`, `[UNAVAILABLE]`) pauses the account for a fixed 10 minutes.

**Offline.** Cached mail stays readable. Flag, move, delete, compose and send all work locally and queue. The UI shows how many changes are pending.

**Conflicts.** The server wins on whether a message exists and on its flags once a pending op is acknowledged. Local pending ops win until acknowledged. An op whose target vanished from the server is dropped silently.

## 7. Send pipeline

Sending is: save to outbox, wait out the undo window, submit via SMTP, upload a Sent copy. A message is persisted before any network call and is never sent twice automatically.

| State | Meaning | Next |
| --- | --- | --- |
| `queued` | In the outbox until `send_after_utc` (undo window, default 10 s, range 0 to 30 s; or a scheduled time, section 18.3) | `sending` at the timer; deleted on undo, which restores the draft |
| `sending` | SMTP session in progress | `sent`, `retry_wait`, `failed`, or `unknown` |
| `retry_wait` | Transient failure (network, 4xx) | `queued` after backoff of 30 s, 2 min, 10 min, 1 h; `failed` after 6 attempts |
| `sent` | Server accepted the message (250 after DATA) | Terminal. Sent copy handled as below. |
| `failed` | Permanent error (5xx, rejected recipient, oversize, auth) | User edits, retries or discards |
| `unknown` | App closed or crashed while `sending` | Never retried automatically. The user is told to check Sent, then resend or discard. |

**Submission**

- One SMTP session per message. XOAUTH2 tokens are refreshed first.
- Envelope: `MAIL FROM` is the identity address; `RCPT TO` is To, Cc and Bcc. The Bcc header is never written.
- If any recipient is rejected permanently, abort before `DATA`, mark `failed` and list the bad addresses. No partial sends.
- Before queueing, check size against the SMTP `SIZE` limit and IMAP `APPENDLIMIT` and block with a clear message. When the SMTP server cannot be reached, the message is queued and a later `552` makes it `failed` (SPEC_GAPS G65).
- Headers set by the client: Message-ID (`<uuid@from-domain>`), Date, From, To, Cc, Reply-To if set, Subject, In-Reply-To and References for replies, MIME-Version, Content-Type. No X-Mailer.
- Body: `multipart/alternative` (plain and HTML) from the rich text composer, or plain only when the user toggles it. Attachments go in `multipart/mixed`, inline images in `multipart/related`. Non-ASCII headers use RFC 2047, body text UTF-8 quoted-printable, attachments base64.

**Sent copy.** After SMTP accepts, `APPEND` the raw message to the Sent folder with `\Seen`. The APPEND, like every draft upload, is an `ops` job the account's actor pushes (SPEC_GAPS G66). The `save_sent_copy` setting resolves as: `auto` means off when the server advertises `X-GM-EXT-1` (Gmail files its own copy) and on otherwise; `on` and `off` override. A failed APPEND is retried as an `ops` job and never causes a resend. After a reply or forward, set `\Answered` or `$Forwarded` on the original through `ops`.

**Drafts.** Autosave to the local `drafts` table 5 s after typing stops. Upload to the IMAP Drafts folder 30 s after the last change and on window close: `APPEND` the new version with `\Draft`, then delete the previously uploaded UID. Sending or discarding deletes the uploaded draft. If the same draft changes elsewhere, last write wins with no merge. Attached files are copied into the app data directory at attach time, so a moved or deleted original cannot break a later send; the copies are removed when the draft is sent or discarded.

**Compose rules**

- Reply goes to Reply-To if present, else From. Reply-all adds To and Cc minus every identity of the account, deduplicated case-insensitively.
- Subject prefixes `Re:` and `Fwd:` never stack.
- Quote format: `On <local date>, <name> <address> wrote:` then a blockquote. The cursor sits above the quote and the signature goes between reply and quote.
- Forward is inline by default, with a forward-as-attachment option (`message/rfc822`).
- Warn before sending with an empty subject, or when the body says attach or attached and nothing is attached.
- Recipient fields parse RFC 5322 addresses and show invalid ones as red chips that block sending.

## 8. Message parsing and rendering

Raw mail is parsed in Rust, reduced to a small display model, and shown in a locked-down iframe. The webview never receives unsanitized HTML, and nothing loads from the network unless the user allows it.

**Parsing**

- `mail-parser` builds the MIME tree and decodes headers and charsets. Unknown or invalid charset: try UTF-8, then Windows-1252, then lossy UTF-8. Parsing never panics.
- Body choice: HTML if present and HTML view is on (default), else plain text. A per-message toggle switches to plain text. HTML-only mail gets generated plain text for list previews and search.
- A Date header that is invalid or more than 24 h in the future is ignored; sort by INTERNALDATE.
- Preview: first 140 characters of plain text, whitespace collapsed, quoted lines (`>`) and anything after `-- ` dropped.

**HTML pipeline.** All steps are required, in this order.

1. **Sanitize** with an `ammonia` allowlist. Keep structural, text, table, `img`, `a` and `style` elements with safe attributes. Remove `script`, `iframe`, `frame`, `object`, `embed`, `applet`, `form` and form controls, `base`, `meta`, `link`, `svg`, `math`, `audio`, `video`, every `on*` attribute, and `javascript:`, `vbscript:` and non-image `data:` URLs.
2. **Rewrite URLs.** `cid:` becomes `mail-cid://<message>/<cid>`. Every `http(s)` image or CSS `url()` becomes `mail-remote://<encoded url>`. Each `a href` moves to `data-href` and `href` becomes `#`.
3. **Serve** the result from the `mail-body://<message>?images=0|1&plain=0|1` custom protocol (`plain=1` is the per-message plain-text toggle, SPEC_GAPS G53) with a `Content-Security-Policy` response header (not a meta tag): `default-src 'none'; style-src 'unsafe-inline'; img-src mail-cid:` plus `mail-remote:` only when `images=1`; `script-src 'nonce-<random per response>'`.
4. **Display** in `<iframe sandbox="allow-scripts" src="mail-body://...">` without `allow-same-origin`. The only script is ours, carrying the nonce. It handles link clicks and posts the content height to the parent for auto-sizing. The body always sits on a white background, in light and dark themes. No color inversion.

Because remote URLs are always rewritten and the CSP decides whether they load, blocking and allowing are one switch: `images=0` or `images=1`.

**Remote images.** Blocked by default with a banner: Load images, or Always load from this sender (stored in `remote_image_allow`). When allowed, Core fetches through the `mail-remote` proxy with no cookies, no referrer, a generic user agent, a 10 s timeout, a 5 MB cap, `image/*` responses only, and at most 4 concurrent requests per message.

**Links.** The iframe script posts the clicked `href` to the parent. The parent allows only `http`, `https`, `mailto` and `tel`. `mailto` opens compose; the rest open in the system browser through `link.open` (SPEC_GAPS G52), never in-app. If the visible link text looks like a URL whose host differs from the real `href` host, show a confirmation with both. Hovering shows the real target in the status bar.

**Plain text.** Linkify `http(s)` and `mailto` only. Collapse quoted blocks deeper than level 1 behind a toggle. Honor `format=flowed`.

**Attachments**

- Listed from `BODYSTRUCTURE` metadata (name, size, type) without downloading. Inline `cid:` images are served by Core, images only.
- Open: fetch `BODY.PEEK[part]` into a randomly named temp subfolder, then open with the OS default app. Executable-type extensions (`.exe .msi .bat .cmd .com .scr .ps1 .vbs .js .jar .lnk .app`) require a confirmation. Save as is always available. Nothing opens automatically.
- Filenames are sanitized: path separators and control characters stripped, Windows reserved names (such as `CON`) prefixed, 200 character maximum.
- `text/calendar`, S/MIME signatures and `winmail.dat` appear as plain attachments with no processing.

**Read state.** A message is marked `\Seen` after it has been displayed 1.5 s (setting: immediately, after delay, or never). `Disposition-Notification-To` is ignored; read receipts are never sent.

## 9. Mailbox operations

Every action applies locally at once, is queued durably in `pending_ops`, and is pushed to the server by the sync actor. If the server rejects it permanently, the local change is reverted and the user is told.

| Action | IMAP implementation | Fallback |
| --- | --- | --- |
| Mark read or unread, star | `UID STORE` `+FLAGS.SILENT` or `-FLAGS.SILENT` with `\Seen` or `\Flagged` | none needed |
| Move (also used for Archive, Delete, Spam) | `UID MOVE` | `UID COPY`, `STORE +FLAGS (\Deleted)`, `UID EXPUNGE` (UIDPLUS). Without UIDPLUS: never a bare `EXPUNGE` unless `UID SEARCH DELETED` shows no other flagged messages; otherwise leave it flagged and hidden. |
| Delete | Move to Trash. Already in Trash or Junk, or no Trash folder: permanent delete after confirmation (`\Deleted` plus `UID EXPUNGE`). | same as Move |
| Archive | Move to the `\Archive` folder. No such folder: the action is hidden for that account. | same as Move |
| Mark all read | `UID STORE` over the folder's UIDs, 500 per command | none needed |
| Create, rename, delete folder | `CREATE`, `RENAME`, `DELETE` with modified UTF-7 names. Deleting asks for confirmation. Special-use folders cannot be deleted. | none needed |
| Empty Trash | Permanent delete of everything in Trash, after confirmation showing the count | same as Delete |

Multi-select actions batch UID sets of at most 500 per command.

**Undo.** Archive, delete and move show a 10 s Undo toast. If the op is still queued, Undo cancels it. If already sent, Undo moves the message back using the `COPYUID` response; without it, the app resyncs and finds the message again by Message-ID.

**Gmail behavior** (when the server advertises `X-GM-EXT-1`)

- `[Gmail]/All Mail` has sync turned off by default, because it duplicates every message. `Important` and `Starred` virtual folders are hidden by default: they start with sync off and the sidebar omits them while it is off (SPEC_GAPS G61).
- Header fetches also request `X-GM-MSGID`, `X-GM-THRID` and `X-GM-LABELS`. A message under two labels is two rows sharing one `gm_msgid`. Unified inbox, search and counts count it once.
- Archive moves to `[Gmail]/All Mail`, which removes only the INBOX label. Delete moves to `[Gmail]/Trash`. Spam moves to `[Gmail]/Spam`. `\Flagged` is Starred.

**Folder list.** A tree with unread counts. Special folders are pinned at the top in the order Inbox, Drafts, Sent, Archive, Junk, Trash; other folders follow alphabetically. Each folder has a sync on/off toggle and a watch (IDLE) toggle.

## 10. Threading and search

Threads are built locally per account, across folders. Search is local-first full text over everything cached, with server search only when asked.

**Threading**

1. On Gmail, a thread is `X-GM-THRID`. Nothing else is computed.
2. Elsewhere, union-find over Message-ID, In-Reply-To and References: two messages share a thread if they share any ID in that set. Merging two threads relabels the smaller one.
3. No subject-based merging by default. A setting (default off) adds grouping by normalized subject within 14 days. A wrong merge costs more than a split thread.
4. A message with no Message-ID gets a synthetic one, `<synth:<account>:<folder>:<uid>@local>`.
5. The list shows one row per thread (toggle Group by conversation, default on): latest date in the current folder, up to 3 participant names, unread count, message count. Rows sort by that date.
6. The thread view includes the thread's messages from all folders of the account except Trash, Junk and Drafts (unless that is the current folder). Oldest first; unread messages and the latest are expanded, the rest collapsed to one line.

**Search**

- Index: FTS5 over subject, from, to and cc, and body text, with tokenizer `unicode61 remove_diacritics 2` and prefix matching on the last token. Only messages with downloaded bodies have body text indexed; the UI shows how many are not, with a Search server too button.
- Syntax: words (AND), `"phrase"`, `from:`, `to:`, `cc:`, `bcc:` (Sent and Drafts only), `subject:`, `in:<folder>`, `label:` (alias of `in:`), `is:unread|read|starred|snoozed|muted`, `has:attachment`, `filename:`, `before:YYYY-MM-DD`, `after:YYYY-MM-DD`, `older_than:` and `newer_than:` with `d`, `m` or `y`, `larger:` and `smaller:` with `k` or `m`, `account:<email>`, `-term` negation, `OR` between terms. Unknown operators are treated as plain words.
- Scope: the current account, all folders except Trash and Junk. A toggle widens it to all accounts.
- Server search: IMAP `UID SEARCH` (`TEXT`, `FROM`, `SUBJECT`, `CC`, `BCC`, `SINCE`, `BEFORE`, `UNSEEN`, `FLAGGED`, `LARGER`, `SMALLER`, `NOT`, `OR`) on the selected folders, or `X-GM-RAW` on Gmail; the rest runs locally. Hits not in the cache are header-fetched into the store, marked as outside the sync window, then listed. Results cap at 500.
- Typing debounces 150 ms and cancels any query still running.

**Contacts autocomplete.** From the `contacts` table, filled from From, To and Cc of mail the user sends or receives. Sent addresses weigh more. Ranked by use count, then recency. No address-book sync.

## 11. UI

Three panes, keyboard-first, no animation that costs frames. The window title and tray show the unread count.

**Layout**

- **Left:** account and folder sidebar. The top item, All Inboxes, is the unified inbox. Below it: Snoozed and Outbox views per account.
- **Middle:** message list, virtualized so only visible rows exist in the DOM. Sorted newest first. A row shows sender (recipients in Sent and Drafts), subject, 140 character preview, date, and indicators for unread, star, attachment and thread count.
- **Right:** reading pane, positioned right (default), bottom, or off (list full width, message opens in its own view).
- **Compose:** a separate window per draft, so several can be open at once. Fields: From (identity picker), To, Cc, Bcc (chip inputs with autocomplete), Subject, body, attachments by button or drag and drop. Buttons: Send, Send later, Insert template, Save as template.
- **Settings:** Accounts, Identities and signatures, Reading, Composing, Notifications, Privacy (remote image allowlist), Rules, Templates, Sync and storage, Shortcuts (read-only), About (version, releases link).
- **Status bar:** one sync dot per account, pending-op count, hovered link target.

**States**

| State | Behavior |
| --- | --- |
| First run, no accounts | Only the add-account form. |
| Folder loading, cache empty | Skeleton rows, never a bare spinner. |
| Empty folder | No messages. |
| Offline | Banner: Offline, changes will sync later. All actions still work. |
| `NeedsAttention` | Banner on that account with Sign in again or Review certificate. Other accounts unaffected. |
| Backoff longer than 1 minute | Status bar warning with the last error; clicking opens the log. |
| Send failed or `unknown` | Per-account Outbox folder lists these with Retry, Edit, Discard and a sidebar badge. |
| Remote images blocked | Banner above the message body. |

**Keyboard shortcuts** (fixed in the first build, inactive while typing in a field)

| Keys | Action |
| --- | --- |
| `j` / `k` | Next / previous message |
| `Enter` or `o` | Open |
| `Esc` | Close or deselect |
| `c` | Compose |
| `r` / `a` / `f` | Reply / reply all / forward |
| `e` | Archive |
| `#` or `Delete` | Delete |
| `s` | Star |
| `b` | Snooze |
| `m` | Mute |
| `Shift+U` | Toggle unread |
| `/` | Focus search |
| `g` then `i`, `d`, `s` | Go to Inbox, Drafts, Sent |
| `Ctrl/Cmd+Enter` | Send |
| `Ctrl/Cmd+P` | Print |
| `Ctrl/Cmd+,` | Settings |

**Notifications.** For new unread mail in INBOX or watched folders, when the window is unfocused or hidden. Never during a folder's first sync, and only for messages whose INTERNALDATE is within the last 30 minutes. One notification per message; more than 3 within 10 s coalesce into N new messages in <account>. Content is sender and subject; body preview is off by default. Click focuses the app on that message. On or off per account. Muted threads and rules with Never notify suppress it.

**Background.** Setting Keep running in tray when the window closes, default on. Tray menu: Open, Compose, Quit. Unread badge on the dock or taskbar where the OS supports it. Autostart is off by default. A second launch focuses the running instance.

**Look.** Light, dark or system theme via CSS variables in one `tokens.css` (visual language: the Dispatch design handoff, screen 3a, with glass flattened to opaque surfaces). Comfortable or compact density. Geist bundled locally, system stack as fallback. Message bodies stay on white. Inline SVG icons (Phosphor regular, MIT). Motion: fades and scale pop-ins only, 150 ms or less, none under `prefers-reduced-motion`.

**Accessibility.** Everything reachable by keyboard, visible focus ring, list exposed as a listbox, `prefers-reduced-motion` respected, text contrast at least 4.5:1. English only; all strings live in one file.

## 12. Errors and edge cases

Every failure ends one of three ways: retried silently, shown as a banner the user can act on, or reverted with a message. Nothing fails silently and nothing loops forever.

| Situation | Required behavior |
| --- | --- |
| Network drops mid-sync | The batch in flight rolls back. The actor goes to Backoff and resumes from the last committed batch. |
| Server sends BYE or closes the socket | Reconnect once immediately, then Backoff. |
| Dead IDLE connection (NAT timeout) | TCP keepalive at 60 s, read timeout of 30 min 30 s on IDLE, re-IDLE every 25 min. |
| Command timeout | 60 s for normal commands, 5 min for large fetches. Then drop the connection and go to Backoff. |
| Wrong password or revoked OAuth | `NeedsAttention`. No retries. Banner. |
| Invalid or changed TLS certificate | `NeedsAttention` with certificate details and the pin option. |
| Throttled by the server | Pause 10 min and show an info banner. |
| UIDVALIDITY changed | Purge that folder's cache, resync, drop its pending ops, show a toast. |
| Folder deleted or renamed on server | Next `LIST` marks it gone and purges it. Renames are treated as delete plus new; headers refetch cheaply. |
| Open message deleted on server | The reading pane keeps the body and shows This message was deleted until closed. |
| Missing Message-ID, Date, From or Subject | Synthetic ID, INTERNALDATE, `(unknown sender)`, `(no subject)`. |
| Malformed MIME or unreadable message | Best-effort render, never a panic. On total failure show Could not display this message with Save source (`.eml`). |
| Very large message | No body prefetch. Text parts over 1 MB are not displayed inline; show Message too large with a download option. |
| Folder with 100,000+ messages | Sync window applies. Load older extends it by 90 days per click. |
| Duplicate send risk | No automatic retry from `sending`; crashes land in `unknown` (section 7). |
| Disk full | Stop writes, show a banner, roll back the transaction. Never corrupt the database. |
| Database fails to open or integrity check fails | Move it aside as `*.corrupt`, rebuild from the server, and warn that outbox items in it may be lost. |
| Second app instance | Focus the first instance and exit. |
| System clock wrong | Only date display is affected. Sync does not depend on local time. Snooze and Send later use UTC stored at creation. |

## 13. Security and privacy

The app talks only to servers the user configured. It collects nothing, loads nothing from a message without permission, and keeps secrets in the OS keychain.

1. **Network allowlist.** The user's IMAP and SMTP hosts, Google's OAuth endpoints, autoconfig hosts (domain only, can be disabled), remote images after the user allows them, and List-Unsubscribe targets after the user clicks. Nothing else. No telemetry, no crash reporting, no update checks; updates are manual (assumption 12).
2. **Secrets.** Only in the OS keychain. Never in the database, config files, logs or IPC events. Access tokens exist only in memory.
3. **IPC and frontend.** The Tauri capability file lists only the app's own commands. The main window's CSP is `default-src 'self'` plus the app's custom protocols, with no `unsafe-eval` and no remote origins.
4. **Hostile content, three layers, all required.** The `ammonia` sanitizer, the sandboxed iframe without `allow-same-origin`, and the CSP sent as a response header by `mail-body`. The sanitizer is tested against an XSS corpus (T19).
5. **Attachments.** Never opened automatically. Risky extensions confirm first. Temp files go in a randomly named subfolder with user-only permissions, deleted at quit and at next startup.
6. **No read receipts.** `Disposition-Notification-To` is ignored.
7. **Transport.** TLS only, STARTTLS mandatory where used, TLS 1.2 or newer, certificate verification always on, per-account pinning for self-signed servers.
8. **Logging.** `tracing` to rotating files (5 files of 2 MB) in the app data directory, level info. No message content, subjects or credentials at any level. A debug switch enables IMAP and SMTP traces, off by default, redacting `AUTH` and `LOGIN` lines and message literals.
9. **Local data.** The database and caches live in the OS app-data directory with user-only permissions. The database is not encrypted at rest; OS disk encryption is assumed (assumption 11).
10. **Phishing aids.** The sender's address is always shown next to the display name. Link targets show on hover, and display-text mismatches ask for confirmation.
11. **Dependencies.** `cargo audit` and `npm audit` run in CI. Lockfiles are committed. No dependency is added outside section 2.

## 14. Performance budgets

These are the lightweight contract, measured on a mid-range laptop with 3 accounts and 50,000 cached messages. The numbers are proposed targets: M0 builds the measurement harness, and the first measured baseline may revise them. After that, a missed budget blocks the build.

| Metric | Budget |
| --- | --- |
| Installer size | 15 MB or less |
| Cold start to an interactive cached inbox | 1.5 s or less |
| Idle CPU, accounts idle, window hidden | Under 1% |
| Idle memory, all processes combined | 250 MB or less |
| Open a cached message to first paint | 100 ms or less |
| Scrolling a 50,000-message list | 60 fps, 100 DOM rows or fewer |
| Local search over 100,000 messages | First results in 300 ms or less |
| New mail after an IDLE notice | Visible in 2 s plus server latency |
| Rules engine | 1 ms per message average |

**Rules that protect the budgets**

- The list is virtualized and paginated by keyset on `(date, id)`, never `OFFSET`.
- First sync fetches headers only. Bodies are lazy, plus the prefetch cap in section 6.
- The frontend holds only the visible window and the selection. No state library caches the whole mailbox.
- `store-changed` events are debounced 100 ms and carry only account and folder.
- No remotely loaded fonts, no icon font, no UI kit, no animation library.

## 15. Acceptance tests

Each test passes or fails from a stated starting state. Automated tests run against a Stalwart container. Tests marked manual run on a real Workspace account and are listed in `docs/MANUAL_TESTS.md`.

Stalwart does not speak Gmail's dialect, so tests that need `X-GM-EXT-1` (T11 second half, T25, T36) and the capability-fallback tests (T26) run against a fake `ImapTransport` that replays scripted responses. The fake also powers unit tests of the sync procedure (T02, T07) without a server.

| ID | Area | Test | Pass condition |
| --- | --- | --- | --- |
| T01 | Sync | New account with 300 inbox messages | Headers for the last 90 days appear, app is usable before sync ends, no bodies beyond the prefetch cap. |
| T02 | Sync | Change a folder's UIDVALIDITY on the server | That folder's cache is purged and repopulated, no stale rows. |
| T03 | Sync | Mark read on another client; star locally | Both sides match after one sync, within 2 s with IDLE. |
| T04 | Sync | Expunge a message from another client | Row disappears locally; an open reading pane shows the deleted notice. |
| T05 | Sync | Deliver mail to INBOX by SMTP | Appears in the list within 2 s; exactly one notification fires. |
| T06 | Sync | Go offline, star, move and delete 3 messages, reconnect | UI changes are instant; server matches after reconnect; pending count returns to 0. |
| T07 | Sync | Kill the process between batches | Restart resumes, no duplicate rows, integrity check passes. |
| T08 | Sync | Block the server port | Retries follow 2 s doubling to 5 min; UI shows the state; recovers when unblocked. |
| T09 | Auth | Revoke the app password | Account goes to `NeedsAttention` with zero retries; other accounts keep syncing. |
| T10 | Send | Send with To, Cc and Bcc | Each recipient gets one copy; no Bcc header anywhere; Bcc address invisible to others. |
| T11 | Send | Send on a non-Gmail server, then on one advertising `X-GM-EXT-1` | Exactly one Sent copy with `\Seen` on the first; none appended on the second. |
| T12 | Send | Send with a 10 s window; Undo at 5 s | Nothing reaches SMTP; the draft is restored. |
| T13 | Send | Kill the process after DATA, before marking sent | On restart the state is `unknown`, nothing resends, the user is prompted. |
| T14 | Send | SMTP returns 451 then recovers | `retry_wait` follows the backoff schedule; exactly one delivery. |
| T15 | Send | One recipient rejected with 550 | No DATA sent; state `failed` lists the bad address. |
| T16 | Send | Attachment over the server's SIZE limit | Blocked before queueing with a clear message. |
| T17 | Send | Reply-all on a message that includes the user's own alias | Own addresses excluded, no duplicates, In-Reply-To and References correct. |
| T18 | Send | Type a draft, wait 30 s, open Drafts on another client, edit again | One draft present at all times; old UID removed after each upload. |
| T19 | Render | XSS corpus: scripts, event handlers, `javascript:` URLs, SVG payloads, CSS `@import`, meta refresh, forms, `base` | No script runs; harness records zero outbound requests (Rust corpus tests plus a webview walk against a local beacon server in CI, SPEC_GAPS G56, G68). |
| T20 | Render | Message with a remote image and a tracking pixel | Nothing fetched until Load images; then fetched via the proxy without cookies or referrer; Always load persists. |
| T21 | Render | Link text `paypal.com` pointing at another host | Confirmation shows both; cancelling keeps the user in the app. |
| T22 | Render | Mail in UTF-8, ISO-8859-1, Windows-1252, ISO-2022-JP, GB2312, Shift_JIS and a bogus charset label | All readable or best-effort with no panic. |
| T23 | Render | 20 MB attachment; filenames `../x` and `CON.txt` | Fetched only on demand; saved names sanitized; executable types confirm. |
| T24 | Render | HTML mail with `cid:` images | Shown via the cid protocol; no `data:` URIs in the DOM. |
| T25 | Gmail | Message under two labels | Two rows, one entry in unified, search and thread views; Archive removes only the INBOX label. |
| T26 | Ops | Server without MOVE; server without UIDPLUS | Same result as the MOVE path; no unrelated `\Deleted` messages expunged. |
| T27 | Ops | Create, rename and delete a folder named `Café/日本語` | Round-trips and matches the server. |
| T28 | Ops | Archive, then Undo within 10 s | Message returns to its folder with flags intact. |
| T29 | Threads | Fixture with out-of-order replies, a missing middle message, and a subject-only match | Header-linked messages form one thread; the subject-only match stays separate. |
| T30 | Search | 100,000-message fixture; `from:alice has:attachment after:2026-01-01 budget` | Correct results in 300 ms or less; Search server too adds uncached hits. |
| T31 | OAuth | Manual: Workspace internal client, sign in, sync, send | Works; token refresh after 1 h needs no user action. |
| T32 | OAuth | Manual: revoke the app in the Google account page | `NeedsAttention`; signing in again restores everything with no data loss. |
| T33 | Perf | Run the harness on the reference dataset | Every section 14 budget met. |
| T34 | UI | Triage using only the keyboard: open, reply, archive, star, search | All reachable; shortcuts inactive while typing. |
| T35 | UI | Launch the app twice | The second launch focuses the first. |
| T36 | Labels | On the Gmail fake, add then remove a label | COPY then targeted EXPUNGE; one gm_msgid throughout; label UI hidden on Stalwart. |
| T37 | Snooze | Snooze for 1 minute; kill and restart the app before it fires | Message returns to its origin folder unread within 10 s of restart; one notification. |
| T38 | Send later | Schedule three sends 2 minutes out; cancel one, let one fire, restart before the third | Cancelled never reaches SMTP; fired delivers once; third sends after restart, marked late. |
| T39 | Mute | Mute a thread, deliver a reply | Reply archived, no notification; `$Muted` set where PERMANENTFLAGS allows. |
| T40 | Rules | Three rules, stop processing on the second; deliver 1,000 matches; undo one move | Third rule never runs on the second's matches; under 1 ms per message; the undone message is not re-filed. |
| T41 | Templates | Save a draft as a template; insert it into a draft that already has a subject | Body inserted at the cursor; existing subject untouched. |
| T42 | Search | Query `larger:1m older_than:30d -newsletter from:a OR from:b` | Correct set locally and with Search server too. |
| T43 | Unsubscribe | Messages with one-click, mailto and https List-Unsubscribe headers | Each takes the right path; the POST carries no cookies; nothing fires without a click. |
| T44 | Preview, print | Open a JPEG and a PDF attachment; print a 3-message thread with blocked images | JPEG in the lightbox, PDF externally; print shows all three, no remote fetch. |

## 16. Build guardrails and order

This section is addressed to the AI agent that builds the app. Follow the spec exactly. Where it is silent, stop and ask; do not improvise.

**Guardrails**

1. The architecture is fixed (sections 3 and 4). Do not add modules, tables, columns or dependencies that are not in the spec. Change the spec first, then the code.
2. Do not make visual style choices. Use the tokens in `tokens.css`, defined once. No UI kit, no icon font (inline SVG only), no animation beyond 150 ms opacity or transform transitions.
3. No security shortcuts. The sanitizer, the sandboxed iframe, the header CSP and keychain-only secrets are mandatory from the first commit that touches them. No TODOs in their place.
4. Rust: `#![forbid(unsafe_code)]`, no `unwrap()` or `expect()` outside tests, errors through `thiserror`, a timeout on every network operation, `clippy -D warnings`, `rustfmt`. TypeScript: `strict`, no `any`.
5. Every test in section 15 not marked manual gets an automated test. Manual tests go in `docs/MANUAL_TESTS.md`.
6. Automated tests run only against the container server and the fake transport, never against a real mailbox.
7. Each milestone ends with tests green and a git commit tagged `m<N>`. If a milestone exposes a gap or error in the spec, write it to `SPEC_GAPS.md` and stop. Do not patch around it.
8. After each milestone report: what was built, tests added, deviations from the spec (there should be none), gaps found.

**Repository layout.** One Cargo workspace. Module boundaries from section 3 are crate boundaries, so a forbidden call fails to compile instead of waiting for review.

```text
/
  Cargo.toml              workspace
  CLAUDE.md               from Part A, section A3
  SPEC_GAPS.md
  crates/
    mail-store/           section 3 `store`
    mail-secrets/         `secrets`
    mail-imap/            `imap`, ImapTransport trait, fake transport under tests/
    mail-smtp/            `smtp`
    mail-auth/            `auth`
    mail-sync/            `sync` and `ops`
    mail-outbox/          `outbox`
    mail-render/          `render`
    mail-search/          `search`
    mail-core/            wires the crates into one API; the only crate `app` imports
  src-tauri/              `app`: Tauri commands, events, windows, protocols
  ui/                     Svelte app; src/lib/ipc.ts is the typed mirror of the command table
    src/tokens.css
    src/strings.ts
  tests/
    fixtures/             .eml corpora, XSS corpus, 100k-message generator
    docker/               Stalwart compose file
  docs/
    BUILD.md              this document
    MANUAL_TESTS.md
    REPORTS.md            one entry per milestone
```

Dependency direction is enforced in `Cargo.toml`: `mail-sync` may depend on `mail-imap`, `mail-store` and `mail-auth` and nothing else, and so on down the table in section 3. `src-tauri` depends only on `mail-core`.

**Build order.** These are internal checkpoints, not releases. Nothing ships until M11 passes.

| M | Builds | Done when |
| --- | --- | --- |
| M0 | Repo, CI, Tauri shell, Stalwart test container, fake IMAP transport, perf harness, `tokens.css`, dependency freshness check | `cargo test` and `npm test` green in CI; harness measures startup |
| M1 | `store`: schema, migrations, query API, FTS, writer and read pool | Migration test passes; keyset pagination test passes on a 50,000-row fixture |
| M2 | `imap`, `secrets`, `auth` (password), account CRUD, autoconfig, SMTP connection test (G32) | Add an account against the container; `LIST` works; T09 at the login level (G33) |
| M3 | `sync`: actors, folder order, folder procedure, IDLE, backoff | T01, T02, T04 (row part, G38), T05 (list part), T07, T08, T09 with actors (G33) |
| M4 | UI shell, virtualized list, plain-text reader (via `mail-body`, G42), sidebar, status bar, minimal add-account form (G43) | 60 fps scroll on the 50,000-message fixture and the owner's own mail seen once (G41); T04 reading-pane part (G38); startup budget enforced (G44) |
| M5 | `render`: parser, sanitizer, protocols, attachments; logging and `log.open` (SPEC_GAPS G51); `link.open` and `message.save_source` (G52, G54) | T19 to T24 |
| M6 | `ops`: flags, move, delete, archive, folders, undo, Gmail quirks; `folder.extend_window` (G51) | T03, T06, T25 (rows and Archive, G60), T26 to T28 |
| M7 | `smtp`, `outbox`, compose window, drafts, identities, contacts | T10 to T18 |
| M8 | OAuth: Google XOAUTH2 end to end | T31, T32 |
| M9 | Threading, search, unified inbox, labels, snooze, send later, mute, templates, rules | T25 (unified, search and thread views, G60), T29, T30, T36 to T42 |
| M10 | Notifications, tray, shortcuts, settings, onboarding, single instance, unsubscribe, print, image preview | T05 (notification part), T34, T35, T43, T44 |
| M11 | Error injection, performance pass, accessibility pass, packaging for three OSes | T33 and the full suite green |

## 17. Assumptions and open question

Thirteen assumptions fill gaps the owner had not answered. Status is tracked in the Claude Doc; "Proposed" means not yet ratified. The builder treats every assumption as decided unless `SPEC_GAPS.md` says otherwise.

| # | Assumption |
| --- | --- |
| 1 | Target OSes: Windows, macOS and Linux. |
| 2 | Frontend: Svelte 5 with TypeScript. |
| 3 | Distribution: personal use, bring-your-own Google OAuth client ID, no public release. |
| 4 | Initial sync window of 90 days; older mail loads on demand. |
| 5 | Raw message source is not stored; only parsed parts, with attachments fetched on demand. |
| 6 | Threading by headers only; Gmail uses `X-GM-THRID`. |
| 7 | Default send is HTML plus a plain-text alternative, with a 10 s undo window. |
| 8 | The app keeps running in the tray when the window closes. |
| 9 | Remote images are blocked by default, and mail always renders on a white canvas. |
| 10 | English only. |
| 11 | No database encryption at rest; OS disk encryption (BitLocker, FileVault, LUKS) is assumed. SQLCipher would add a C build and complicate FTS5 for little gain on a single-user machine. |
| 12 | Updates are manual: an About-screen link opens the releases page in the browser. No background update checks. |
| 13 | Scale target: up to 5 accounts and 200,000 cached messages in total. The 90-day window and the section 14 budgets are sized for this. |

**Resolved question.** The app is called **Zero Mail**; identifier `com.digitalcircuitry.zeromail`. The keychain service name and the app-data folder derive from the identifier.

## 18. Gmail parity

Gmail is a service plus a client. This app targets parity with the client half: every Gmail feature below is either already in this spec, added in this section, left to the server (the app links to the provider's settings page), or rejected because it is proprietary, needs a backend, or conflicts with section 1.

| Gmail feature | Verdict | Where |
| --- | --- | --- |
| Conversation view, archive, nested labels as folders, star, mark read, undo send, keyboard shortcuts, dark mode, offline reading, multiple accounts, signatures, recipient autocomplete, drafts sync | Already in | sections 7 to 11 |
| Report spam, not spam | Already in | section 9: move to or from Junk; Gmail learns from the move |
| Several labels on one message, Important marker | Added | 18.1 |
| Snooze | Added | 18.2 |
| Schedule send | Added | 18.3 |
| Templates (canned responses) | Added | 18.4 |
| Mute conversation | Added | 18.5 |
| Filters, block sender | Added as client-side rules, with a stated limit | 18.6 |
| Unsubscribe | Added | 18.7 |
| Print | Added | 18.8 |
| Search operators | Extended | section 10 |
| Attachment preview | Added for images | 18.9 |
| Spam and malware scanning, server filters, vacation responder, forwarding, 2-step verification, storage quota | Server-side | Settings shows a link to the provider's page for each |
| Confidential mode, Smart Compose, summaries, nudges, translate, Drive attachments, Meet, Chat, Tasks, Spaces, category tabs, colored stars | Rejected | Proprietary, needs Google's backend, not exposed over IMAP, or an AI feature (section 1) |

**Security and stability parity.** Gmail's web client scans attachments and links server-side. This app cannot scan for malware; it relies on the mail server for that and keeps its own three-layer rendering defense (section 13). Where Gmail is weaker, this app is stronger: remote images are blocked by default for every sender, nothing loads from a message without consent, and mail stays readable offline with queued actions. Stability parity means the acceptance suite (section 15) stays green and the crash path never loses a queued send (section 7).

### 18.1 Labels and Important (Gmail only)

- Shown only on accounts advertising `X-GM-EXT-1`. Elsewhere a message lives in one folder and the label UI is hidden.
- Header fetches add `X-GM-LABELS`, stored in `messages.gm_labels_json`. The message view shows one chip per label.
- Add label: `UID COPY` to the label's folder. Gmail deduplicates by `gm_msgid`, so this adds a label without a second copy. Remove label: in that label's folder, `UID STORE +FLAGS (\Deleted)` then `UID EXPUNGE` of that UID only. Both go through `ops` with the usual undo toast.
- `\Important` shows as a read-only badge. The app never sets or clears it.

### 18.2 Snooze

- Snooze moves the message to a `Snoozed` folder the app creates on first use (configurable) and records `(message, due_utc, origin_folder)` in `snoozes`. At `due_utc` the message moves back to its origin folder, is marked unread, and a notification fires. Due times are per device; the move itself is visible to other clients.
- Presets: later today (+3 h), tomorrow 08:00, this weekend (Sat 08:00), next week (Mon 08:00), pick a date and time. Presets use local time.
- If the app was closed when a snooze came due, it fires within 10 s of the next start. Unsnooze is available from the Snoozed view. A reply arriving to a snoozed thread ends the snooze early (setting, default on).

### 18.3 Schedule send

- Compose offers Send later, which sets `outbox.send_after_utc`. The message sits in `queued` and appears in the Outbox view with Edit, Cancel and Send now. The outbox is durable (section 7), so a restart keeps the schedule.
- The app must be running at the time, including in the tray. The picker says so. A scheduled message missed while the app was closed sends within 10 s of the next start and is marked Sent late in the Outbox history.

### 18.4 Templates

- `templates` holds name, subject, HTML and plain bodies. Compose has Insert template and Save as template. Inserting puts the body at the cursor and fills an empty subject. Templates are local, not synced.

### 18.5 Mute

- Mute records the thread in `muted_threads`. New messages in a muted thread are archived on arrival by the rules engine and never notify. Unmute stops this; nothing is un-archived. On servers whose `PERMANENTFLAGS` include `\*`, messages also get the `$Muted` keyword so another install of this app agrees.

### 18.6 Rules and Block sender

- An ordered list of rules, each with conditions (any or all of: from, to, cc, subject, list-id, has attachment, body contains) and actions (move, add label on Gmail, star, mark read, mute, delete, never notify, stop processing). Stored in `rules` as JSON. A rule applies to one account or all.
- Rules run on new INBOX and watched-folder messages after each sync, once per message (`rules_applied` prevents re-runs after Undo). Body conditions wait until the body is prefetched. Budget: 1 ms per message average.
- Stated limit, shown in the UI: rules run only while the app is running and only on this device. For Gmail, the Rules screen links to Gmail's own filters and says they run server-side.
- Block sender is a one-click preset rule: from this address, move to Trash, stop processing. Unblock deletes the rule.

### 18.7 Unsubscribe

- When `List-Unsubscribe` is present, an Unsubscribe button shows in the header. Preference order: RFC 8058 one-click (`List-Unsubscribe-Post`; Core sends the POST with no cookies after a confirmation), `mailto:` (opens compose prefilled), `https:` (opens the system browser). Never fires without a click.

### 18.8 Print

- Print renders a header block (from, to, cc, date, subject) above the sanitized body and calls the webview's print dialog. Remote images print only if already allowed for that message. Threads print expanded, oldest first.

### 18.9 Attachment preview

- Image attachments (`image/*` up to 20 MB) open in an in-app lightbox served over the `mail-cid` protocol, with Save and Open externally. Every other type opens externally as in section 8. In-app PDF is rejected: WebKitGTK has no built-in viewer, so it would need a bundled one.

## 19. Changelog

| Version | Date | Change |
| --- | --- | --- |
| v0.15 | 2026-10-20 | Owner decisions (SPEC_GAPS G63 to G66, G68): compose windows opened by the app; `identity.list`, `identity.save`, `identity.delete`; the size check when reachable; Sent copy and draft uploads through `ops`; T19's webview half runs as a walk against a local beacon server. |
| v0.14 | 2026-10-20 | Owner decisions (SPEC_GAPS G58 to G61): event `op-failed`; pending moves and deletes hide rows through bit 5; T25 split between M6 and M9; Gmail Important and Starred start with sync off and hidden. |
| v0.13 | 2026-10-20 | Owner decisions (SPEC_GAPS G52 to G54, G56): command `link.open`; `plain=1` on `mail-body`; native Save dialogs from Rust, `message.save_source` path optional and built in M5; T19 harness. |
| v0.12 | 2026-10-20 | Owner ratified G20 to G22, G34, G39, G40, G49 and G50. Logging and `log.open` scheduled in M5, `folder.extend_window` in M6 (SPEC_GAPS G51). |
| v0.11 | 2026-10-20 | Owner decisions (SPEC_GAPS G41 to G48): M4 done-when (fixture at 60 fps and the owner's own mail once); plain text through `mail-body` from M4; minimal add-account form in M4; startup budget is app cold, OS warm; later-milestone UI hidden until built; `account.list` added; Phosphor icon source; background integrity check. |
| v0.10 | 2026-10-20 | Owner decisions (SPEC_GAPS G35 to G38): `body_state` meanings; one thread per message until M9 (Gmail by X-GM-THRID); event `folder-reset` for the UIDVALIDITY toast; T04 split between M3 (row) and M4 (reading pane). |
| v0.9 | 2026-10-19 | Owner decisions (SPEC_GAPS G29 to G33): autoconfig in `auth` over `reqwest` (HTTPS, domain only, `ring` provider); `rustls-platform-verifier` for trusted roots; `socket2` for the 60 s keepalive; M2 includes the SMTP half of Test connection; T09 split between M2 (login level) and M3 (actors). |
| v0.8 | 2026-10-19 | Owner decisions (SPEC_GAPS G18, G19, G24 to G28): AppError code list; `\Deleted` is flags bit 5; Windows-only builds until M11 with Stalwart on Linux; manual tests wait until M10 is done, except T31 and T32 at M8; hand-written autoconfig XML reader; MX-host guess dropped; M2 to M10 in three sessions. |
| v0.7 | 2026-10-19 | Owner decisions (SPEC_GAPS G2 to G14): name Zero Mail; Geist bundled locally; opaque surfaces instead of window vibrancy; dark tokens derived from the zinc ramp; design extras outside the spec dropped and spec shortcuts kept; Undo toast 10 s; motion limited to fades and pop-ins; glue crates added to section 2; oauth2 freshness waiver; fake transport behind a cargo feature. |
| v0.6 | 2026-10-18 | Build-ready. Section 18 schema and commands merged into sections 3 and 4. Scale resolved as assumption 13. App name is the one open question, with a placeholder identifier so M0 can start. BUILD.md exported with Part A for Claude Code. |
| v0.5 | 2026-10-17 | Section 18, Gmail parity: feature map with verdicts; labels, snooze, schedule send, templates, mute, rules and block sender, unsubscribe, print, extended search operators, image preview; tests T36 to T44; M9 and M10 scope updated. |
| v0.4 | 2026-10-17 | Assumptions became a ratification table; encryption-at-rest and update questions resolved into assumptions 11 and 12. Dependency freshness check, Google Desktop-app client type, attachment copying for drafts. |
| v0.3 | 2026-10-17 | Dropped Microsoft OAuth; Microsoft 365 and Outlook.com are non-goals. First-sync folder order. |
| v0.2 | 2026-10-17 | Command and event contract, repository layout with crate-enforced boundaries, fake IMAP transport for Gmail and fallback tests. Principle 4 reconciled with host-based OAuth. |
| v0.1 | 2026-10-16 | First full draft. |
