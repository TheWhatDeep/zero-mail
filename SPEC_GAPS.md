# SPEC_GAPS.md

Gaps and conflicts between `docs/BUILD.md` (the spec) and the Dispatch design handoff, and what the owner decided.
A gap marked **Open** blocks the work it touches. **Resolved** gaps have been folded into `docs/BUILD.md` v0.7.

## G1. Build environment — Resolved 2026-10-19
A5 lists what the owner provides. On the build machine on 2026-10-19:
- Missing: the Rust toolchain (`rustup`, `cargo`), the MSVC C++ build tools, and Docker.
- Present: Node 24.14.0, npm 11.11.0, git 2.55, WebView2 155.

M0 is done when `cargo test` and `npm test` are green. The Rust half cannot run until these are installed.

## G2. App name — Resolved 2026-10-19
The owner chose **Zero Mail**.
- Display name: `Zero Mail`.
- Identifier: `com.digitalcircuitry.zeromail`, following the pattern of the spec's placeholder.
- Keychain service and app-data folder derive from the identifier.

## G3. Font — Resolved 2026-10-19
The owner chose to bundle Geist (400, 500, 600) as local files shipped with the app, under the SIL OFL 1.1.
- Nothing loads from the network.
- Fallback is the system stack.
- Spec section 2, section 11 "Look" and section 14 were amended.

## G4. Window glass — Resolved 2026-10-19
The owner chose an opaque look-alike: the design's glass tints are flattened onto solid surfaces.
- No `backdrop-filter`.
- No transparent window.
- No `window-vibrancy` crate.

## G5. Dark theme tokens — Resolved 2026-10-19
The owner chose to derive dark tokens from the zinc and accent ramps already in the design.
- Message bodies stay on white.
- The owner reviews the dark palette before M4 is tagged. Reviewed and approved 2026-10-20 (M4 screenshots, light and dark).

## G6. Design elements outside the spec — Resolved 2026-10-19: spec wins
Dropped:
- Calendar and Contacts top-bar buttons.
- The "From your mail" event card.
- The Important / Other segmented control.
- The Cmd+K command menu.
- "Remind me if no reply".
- Custom traffic lights. The app uses the native window frame.

Kept only where the spec has the feature:
- Labels in the sidebar, Gmail accounts only (18.1). They are styled with the design's LabelItem. Label dots use the neutral color, because the spec has no label colors.
- Starred in the sidebar has no spec view, so it is dropped.
- The Snoozed and Outbox views stay (section 11).

## G7. Keyboard shortcuts — Resolved 2026-10-19: spec wins
Shortcuts are exactly the section 11 table. The design's `H`, `U`, `L`, `V`, `G Z`, `Cmd+K` and `?` are not implemented.

## G8. Toasts and motion — Resolved 2026-10-19
- The Undo toast lasts 10 s, per spec section 9.
- Motion follows the owner's instruction: every interaction gets a minimal animation, using fades and pop-ins only, with no slides and no height animation.
- Implementation stays within guardrail 2:
  - Only `opacity` and `transform: scale()` transitions.
  - 150 ms or less.
  - Hover and selection tints fade in through an opacity overlay, never through a `background-color` transition.
  - Everything is disabled under `prefers-reduced-motion`.

## G9. List grouping — Resolved by the spec-wins rule
- One virtualized list (100 DOM rows or fewer, keyset pagination) inside one card.
- "Today" and "Earlier" appear as sticky section headers styled like the design's card header.
- The reading pane is right (420 px, per the design), bottom, or off (spec section 11).

## G10. Icons — Resolved
Phosphor (MIT) regular-weight path data is copied into `ui/src/lib/icons.ts` as inline SVG, with the license notice. No icon font and no runtime dependency. Added in M4.

## G11. Framework — No conflict
The design README suggests React + Tailwind. The spec fixes Svelte 5 with plain CSS variables, and the spec wins.

## G12. Dependency freshness (M0 check) — Resolved 2026-10-19
| Crate | Latest stable | Released | Advisories affecting latest | Result |
| --- | --- | --- | --- | --- |
| tauri | 2.12.4 | 2026-10-14 | none | pass |
| async-imap | 0.12.0 | 2026-09-30 | none | pass |
| rustls | 0.23.46 | 2026-10-14 | none (RUSTSEC-2026-0285, 0346–0349 patched in 0.23.46) | pass |
| lettre | 0.11.23 | 2026-08-03 | none (RUSTSEC-2026-0141 patched in 0.11.22) | pass |
| mail-parser | 0.11.9 | 2026-09-09 | none | pass |
| ammonia | 4.2.4 | 2026-10-17 | none (RUSTSEC-2026-0373 patched in 4.2.4) | pass |
| rusqlite | 0.40.2 | 2026-08-08 | none | pass |
| keyring | 4.2.0 | 2026-08-29 | none | pass |
| **oauth2** | 5.0.0 | **2025-01-21** | none | **fails the 12-month rule; owner waiver 2026-10-19, re-checked every milestone** |
| thiserror | 2.0.21 | 2026-09-23 | none | pass |
| tracing | 0.1.44 | 2025-12-18 | none | pass |
| tokio | 1.53.3 | 2026-10-13 | none | pass |
| tauri-plugin-notification, opener, dialog, single-instance, autostart, window-state | 2.7.0, 2.7.2, 2.8.3, 2.5.4, 2.7.0, 2.5.0 | 2026-09/10 | none | pass |

`scripts/check-deps.mjs` repeats this check in CI.

## G13. Glue crates — Resolved 2026-10-19
The owner approved adding these to the section 2 table:
- `serde` and `serde_json`, for Tauri IPC and JSON columns.
- `tracing-subscriber` and `tracing-appender`, for rotating log files.
- `reqwest` with rustls, as the HTTP client for the image proxy, the unsubscribe POST and OAuth.
- `uuid`, for Message-ID.
- `sha2`, for certificate pinning.
- `rand`, for the CSP nonce.
- `futures`, for async-imap streams.

Any other crate still needs the owner's approval.

Frontend tooling follows from the stack the spec names: `svelte`, `vite`, `@sveltejs/vite-plugin-svelte`, `typescript`, `svelte-check`, `vitest`, `@tauri-apps/api` and `@tauri-apps/cli`. "No `any`" is enforced by `ui/scripts/no-any.mjs` instead of adding ESLint.

## G14. Location of the fake ImapTransport — Resolved by interpretation (owner to ratify)
Section 16 places the fake "under tests/" in `mail-imap`. But the sync tests in `mail-sync` (T02, T07) also need it, and Rust cannot share a `tests/` file across crates.
- The fake is at `crates/mail-imap/src/fake.rs`.
- It compiles only under `cfg(test)` or the `fake` cargo feature.
- `mail-sync` enables that feature as a dev-dependency only, so it is never in a release build.

## G15. Stalwart version for the test container — Resolved by interpretation (owner to ratify)
Stalwart v0.16 (current line) dropped file-based configuration; accounts and listeners are created through its API in recovery mode.
- The test container pins **v0.15.5** (2026-02-14), the last release that reads a mounted `config.toml`. That makes the container fully declarative.
- Risk: the v0.15 line no longer gets fixes. It is acceptable for a throwaway, localhost-only, tmpfs-backed test server. It is never used with real mail.
- If v0.15.5 misbehaves, the fallback is v0.16.24 with a `stalwart-cli apply` bootstrap step.
- Stalwart does not advertise APPENDLIMIT. The APPENDLIMIT path (T16) is covered by the fake transport.

## G16. Design contrast below 4.5:1 — Resolved by the spec-wins rule (G6)
Section 11 requires text contrast of at least 4.5:1. Three design pairs miss it:
| Pair | Ratio | Fix (reuses an existing design value) |
| --- | --- | --- |
| Muted text `#71717a` on the selected row `#e9f8ff` | 4.45 | Use `#52525b` (text-muted-strong) on selected rows: 7.2 |
| Sidebar section label, white at .45 on the sidebar | 4.48 | Use .5, the sidebar meta alpha: 5.21 |
| Muted text `#71717a` on the main surface `#f4f4f6` | 4.40 | Never put muted text directly on the main surface. It sits on cards (4.79) or inputs. |

`ui/src/tokens.test.ts` checks every text and surface pair in both themes.

## G17. Node version
Spec A5 says Node 20+. The pinned Vite 8 plugin and Vitest 5 need Node 22.12 or newer. The build machine has 24.14, and CI uses Node 24. `package.json` `engines` says `>=22.12`.

## G18. AppError codes — Resolved 2026-10-19
Section 3 said `AppError.code` is "a fixed enum" but did not list its members. The owner approved these 20 codes, taken from the failure cases in sections 5, 7, 9 and 12. Section 3 was amended to match.

| Code | Meaning |
| --- | --- |
| `network` | Offline, unreachable, or the connection dropped |
| `timeout` | Connect or command timeout (section 12) |
| `certificate` | Invalid or changed certificate; pinning is offered |
| `tls` | TLS handshake failed, or STARTTLS was not offered |
| `auth_failed` | Wrong password or revoked OAuth; the account goes to `NeedsAttention` |
| `oauth` | The OAuth flow failed or was cancelled |
| `throttled` | Server throttling (`[LIMIT]`, `[UNAVAILABLE]`); 10 minute pause |
| `server_rejected` | Permanent IMAP `NO`/`BAD` or SMTP 5xx |
| `recipient_rejected` | SMTP 550 on `RCPT`; the message lists the addresses |
| `too_large` | Over the SMTP `SIZE` or IMAP `APPENDLIMIT` limit |
| `unsupported` | A capability or folder is missing (for example, no Archive) |
| `not_found` | The account, folder, message or draft is gone |
| `conflict` | Already exists, or a special-use folder delete |
| `invalid_input` | Bad address, server config or value |
| `keychain` | The OS keychain is unavailable or refused |
| `disk_full` | The write stopped and was rolled back |
| `db_corrupt` | The database was moved aside and is being rebuilt |
| `io` | Local file error (save, attachment cache) |
| `cancelled` | The user cancelled (dialog, undo window) |
| `internal` | A bug; details only in the log |

## G19. `\Deleted` has no bit in `messages.flags` — Resolved 2026-10-19
Invariant 8 says messages flagged `\Deleted` on the server are hidden in every view. Section 9 also leaves some messages "flagged and hidden" when the server has no UIDPLUS. But the section 4 `flags` column listed only bits 0–4, so the store had no way to know a row is `\Deleted`.
- The owner chose **bit 5 = `\Deleted` on the server**. Every list and search query filters `flags & 32 = 0`. No new column.
- Section 4 was amended to match.

## G20. FTS external-content source — Resolved by interpretation, ratified by the owner 2026-10-20
Section 10 indexes subject, from, to/cc and body text. Body text is in `bodies` and the headers are in `messages`. An FTS5 external-content table reads from one source, so:
- `message_fts` (columns `subject`, `sender`, `recipients`, `body`) uses the view `message_fts_content` (`messages LEFT JOIN bodies`) as its content. A view is not a table or a column. It is the only way to give FTS5 one source that spans the two tables.
- Triggers on `messages` and `bodies` keep the index in step, including cascaded deletes and upserts.
- `to_json`, `cc_json` and `reply_to_json` hold JSON arrays of `[name, addr]` pairs, with no keys. Only names and addresses reach the index, and FTS5's content consistency check works. That check fails when the view uses `json_each` or `json_tree`.

## G21. Nullability and "not set yet" values — Resolved by interpretation, ratified by the owner 2026-10-20
Section 4 says types are the builder's job but marks some columns `NULL`. M1 reads that literally:
- Every column the spec marks `NULL` is nullable, and every other column is `NOT NULL`.
- "Not set yet" is written as `''` or `0`. Examples: `uidvalidity` and `synced_at` stay 0 until a folder is first selected (0 is not a valid UIDVALIDITY); `oauth_client_id` is `''` for password accounts; `text_html` is `''` for plain-only mail; `last_error` is `''` when there is no error.
- The migration test checks this nullability against the spec column by column.

## G22. Two folder references without a foreign key — Resolved by interpretation, ratified by the owner 2026-10-20
- `pending_ops.folder_id` has no foreign key. A queued folder delete (M6) must outlive the local folder row it deletes optimistically, and a cascade would drop the op before it reached the server. The UIDVALIDITY purge drops the folder's ops explicitly (section 12).
- `snoozes.origin_folder_id` has no foreign key either, so that deleting the origin folder neither deletes the snooze nor is blocked by it. Where the message goes back to in that case is M9's question.
- Every other reference has a foreign key: `ON DELETE CASCADE` to its owner, or `NO ACTION` for `messages.thread_id` and `outbox.identity_id`.

## G23. `bcc:` search has no stored column — Resolved 2026-10-20: option (a), local Bcc lists
Section 10 lists `bcc:` (Sent and Drafts only), but `messages` has no Bcc column. Bcc is never written to the header (section 7), so the Sent copy on the server has none. Bcc exists only in `drafts.content_json` and `outbox.envelope_json`. To be decided before M9.

Options:
- (a) **Recommended:** `bcc:` matches locally, through the Bcc addresses this app knows.
  - Drafts are matched through `drafts.content_json`.
  - Sent messages are matched through the outbox history rows (`outbox.envelope_json`). Each row stores its Bcc list and Message-ID, and the Message-ID is matched to the Sent row.
  - Search server too sends IMAP `BCC` to Sent and Drafts, which finds copies other clients stored with a Bcc header.
  - No schema change. Mail sent from another client or another device, and Sent history older than the outbox rows, is not found.
- (b) `bcc:` runs only as a server search (IMAP `BCC` on Sent and Drafts). It is always empty locally. Our own Sent copies never match, because they carry no Bcc header.
- (c) Drop `bcc:` from section 10. It becomes a plain word, like unknown operators.
- (d) Add a `bcc_json` column to `messages`, filled for mail this app sent (schema change).

## G24. Windows-only builds until M11 — Resolved 2026-10-19
The owner has only Windows desktops.
- From M2, CI builds, tests and runs the startup harness on Windows only.
- The Stalwart integration job stays on GitHub's Linux runner, which is the only place its container runs.
- The format, audit and boundary checks stay on Linux. They are platform-neutral and do not build the app.
- Whether macOS and Linux are still targets for M11 (packaging for three OSes, A6) is decided before M11.

## G25. When tests run — Resolved 2026-10-19
- Every milestone still writes its automated tests and ends with them green (CLAUDE.md, guardrail 7).
- Manual tests (`docs/MANUAL_TESTS.md`) wait until M10 is done and M11 is starting.
- The exception is M8: the owner runs T31 and T32 himself, because they are M8's only done-when tests.

## G26. Autoconfig XML — Resolved 2026-10-19
The domain's autoconfig file and the Mozilla ISP database are XML, and no XML crate is approved. The owner chose a small hand-written reader for just the autoconfig format. No new dependency.

## G27. MX-host guess dropped — Resolved 2026-10-19
An MX lookup needs a DNS crate, and none is approved. DNS-over-HTTPS would contact a third party, which section 13 forbids. The owner dropped the step. Resolution is now: built-in table, the domain's autoconfig URL, the Mozilla ISP database, then the manual form. Section 5 was amended.

## G28. Milestones per session — Resolved 2026-10-19
The owner amended A1: M2 to M10 run in three sessions, M2–M4, M5–M7 and M8–M10. Each milestone inside a session still ends with tests green, a `docs/REPORTS.md` entry and its own `m<N>` tag before the next one starts. A session stops at any open spec gap.

G1 update (2026-10-19): the owner installed Rust (1.99.1, MSVC host) and the VS 2022 Build Tools. Docker is not installed locally, by the owner's choice; the Stalwart tests run in CI (GitHub's Linux runners), and the fake transport covers local runs.

## G29. Autoconfig: which module, and which HTTP client — Resolved 2026-10-19: proposal accepted
Section 5 resolves server settings through the domain's autoconfig URL and the Mozilla ISP database, and section 13 allows those hosts. But:
- No section 3 module owns server-settings resolution.
- Section 2 limits `reqwest` to the image proxy, the unsubscribe POST and the OAuth token endpoint ("only"). Autoconfig is not on that list.
- The workspace enables `reqwest`'s `rustls` feature. In reqwest 0.13 that feature pulls in `aws-lc-rs`, a second crypto library that needs a C build, next to the `ring` provider the workspace's `rustls` already uses. The `rustls-no-provider` feature of the same crate avoids this: the client is given a rustls config that uses `ring`.

Proposal:
- Autoconfig lives in `mail-auth`. Auth already owns choosing the login method by host (section 5, step 3), and it will hold `reqwest` for the OAuth token endpoint in M8.
- Account CRUD (`account.*`) lives in `mail-core`. It is the only crate that may call `store`, `secrets`, `imap` and `auth` together.
- Section 2's `reqwest` row adds autoconfig lookups.
- Lookups use HTTPS only: `https://autoconfig.<domain>/mail/config-v1.1.xml`, then `https://<domain>/.well-known/autoconfig/mail/config-v1.1.xml`, then `https://autoconfig.thunderbird.net/v1.1/<domain>`. No email address is sent, only the domain.
- `reqwest` switches from the `rustls` feature to `rustls-no-provider`, so only `ring` is linked.

## G30. Trusted roots for certificate verification — Resolved 2026-10-19: `rustls-platform-verifier`
Section 5 requires certificate verification to be always on. `rustls` verifies against a root store, and no approved crate provides one.

Proposal: add `rustls-platform-verifier`. It is already in the build as a dependency of `reqwest`'s rustls support. It verifies with the OS trust store (Windows CryptoAPI, macOS Security framework, Linux system roots), so a root the user or their company installed works as it does in a browser.
- The alternative is `webpki-roots`, a bundled copy of Mozilla's root list. It goes stale between releases and ignores roots installed on the machine.
- Pinning (section 5) is our own code on top of the verifier. With a pin set, only a leaf certificate whose SHA-256 matches is accepted. Without a pin, the platform verifier decides, and a failure reports the leaf certificate's SHA-256 so the UI can offer to pin it.

## G31. TCP keepalive at 60 s needs `socket2` — Resolved 2026-10-19: `socket2` approved
Section 12 requires TCP keepalive at 60 s on IDLE connections. tokio can switch keepalive on, but it cannot set the interval. The OS default is 2 hours on Windows and Linux. Setting the interval needs `socket2`, which is not approved. It is already in the build as a dependency of tokio, so it adds no code size.

Proposal: approve `socket2` for this one use in `mail-imap`. The alternative is OS-default keepalive. Then the 30 min 30 s IDLE read timeout is the only dead-connection check.

## G32. The SMTP half of Test connection in M2 — Resolved 2026-10-19: option (a)
Section 5, step 4: Test connection is IMAP login plus `LIST`, and SMTP `EHLO` plus `AUTH`; the account saves only if both pass. M2 builds account CRUD, but `smtp` is M7.

Options:
- (a) M2 adds one function to `mail-smtp`: connect, `EHLO`, `AUTH`, `QUIT`, using `lettre`. Section 5 is met from M2 on. M7 builds submission on top of it.
- (b) M2 tests IMAP only and records the SMTP half as deferred to M7.

## G33. T09 at M2, before sync actors exist — Resolved 2026-10-19: split as proposed
T09 is in M2's done-when: "Revoke the app password: the account goes to `NeedsAttention` with zero retries; other accounts keep syncing". Two problems:
- Account states and actors arrive in M3.
- The Stalwart test container's accounts are fixed in `config.toml`. A password cannot be revoked while it runs.

Proposal:
- **M2:**
  - On the fake transport, the password is revoked while the session is connected, and the next login gets `auth_failed`.
  - On Stalwart, an account is saved with a password that no longer matches.
  - In both, exactly one login attempt is made, no retry follows, the error class is `NeedsAttention`, and a second account on the same server still logs in and lists.
- **M3:** T09 runs again with actors: the account state becomes `NeedsAttention`, no reconnect timer starts, and the other account's actor keeps syncing.

## G34. One certificate pin per account, and how the fingerprint reaches the UI — Resolved by interpretation, ratified by the owner 2026-10-20
Section 4 has one `accounts.pinned_cert_sha256` column, and section 5 says the user pins "the server certificate" per account. M2 reads this as follows:
- The one pin applies to both the IMAP and the SMTP connection. The Stalwart test container uses one certificate for both, and so do most self-hosted servers.
- Limit: a self-hosted pair whose IMAP and SMTP servers present two different self-signed certificates cannot be pinned. A second column would be a schema change.
- `AppError` is fixed as `{ code, message, account_id? }`. For code `certificate`, `message` holds the presented leaf certificate's SHA-256 in lowercase hex (64 characters). That is what the UI shows under Review certificate and offers to pin.

## G35. What `body_state` text and full mean — Resolved 2026-10-20
Section 4 lists `none | text | full` and does not define them. The owner chose:
- `none`: no body is cached.
- `full`: every text/plain and text/html part of the message was fetched whole.
- `text`: the parts were fetched, but at least one was cut at the 1 MB cap. Section 8 shows such a part as Message too large.

## G36. `thread_id` before M9 — Resolved 2026-10-20
`messages.thread_id` is `NOT NULL`, but threading arrives in M9. The owner chose:
- Until M9, sync writes one thread per message.
- On Gmail, messages that share `X-GM-THRID` share one thread. That is already M9's rule.
- M9's union-find merges the single-message threads.

## G37. The UIDVALIDITY toast needs an event — Resolved 2026-10-20: event `folder-reset` added
Section 12 says a UIDVALIDITY change shows a toast, but no section 3 event carried it. The owner added the event `folder-reset {account_id, folder_id}`. It fires once, after the purge commits. The UI (M4) shows the toast. Section 3 was amended.

## G38. T04 split between M3 and M4 — Resolved 2026-10-20
- **M3:** the expunged row disappears locally.
- **M4:** the open reading pane keeps the body and shows This message was deleted. The plain-text reader arrives in M4.

## G39. Which messages the prefetcher skips — Resolved by interpretation, ratified by the owner 2026-10-20
Section 6, step 8 prefetches "text and HTML parts (each up to 1 MB)". Section 12 says a very large message gets "no body prefetch" but gives no size. M3 reads the two together:
- The prefetcher takes the folder's newest 200 messages. The cap counts messages that already have a body.
- It fetches each message's body text/plain and text/html parts whole.
- A message whose body text part is over 1 MB is skipped. Its `body_state` stays `none`. Opening it later fetches a part capped at 1 MB, which gives `body_state` `text` (G35).
- The message's total size does not matter. A small text part with a 20 MB attachment is prefetched; the attachment is never fetched (invariant 3).
- HTML-only mail gets generated plain text, through `mail-parser`'s HTML-to-text, which is stored as `text_plain`. Previews and search then work (section 8).

## G40. Small sync event and state details — Resolved by interpretation, ratified by the owner 2026-10-20
- **Throttle pause:** the 10-minute throttle pause shows as state `Backoff`, with `last_error` `throttled`. Section 6 has no separate paused state.
- **Folder list changes:** `store-changed` with no `folder_id` (null) means the account's folder list changed. A folder was added or purged by `LIST`.
- **Account removed:** an actor whose account row is gone stops.
- **Missing credentials:** an account with no keychain password goes to `NeedsAttention`.
- **Charsets:** `mail-parser`'s `full_encoding` feature is on, so Shift_JIS, ISO-2022-JP and GB2312 decode (section 2: "charset conversion included"; T22). It brings `encoding_rs` in as a transitive dependency. Nothing was added to the approved list.

## G41. What "real mail" means for M4's done-when — Resolved 2026-10-20
M4's done-when is "Real mail visible in the UI at 60 fps scroll". Automated tests may not use a real mailbox, and Stalwart runs only on Linux CI (G24). The owner chose:
- **Automated:** the perf harness scrolls a 50,000-message store fixture in the Windows release build. It checks 60 fps and at most 100 DOM rows.
- **By hand, once at M4:** the owner adds his Workspace account with an app password and looks at his own mail. This is a check, not a section 15 test, so G25 does not delay it.

## G42. How plain text reaches the M4 reader — Resolved 2026-10-20
Core serves the plain text from `mail-body://` from M4 on:
- The text is HTML-escaped, with nothing else added.
- The response carries the section 8 CSP header with a per-response nonce.
- The text is shown in the sandboxed iframe without `allow-same-origin`.

M5 adds the HTML pipeline (ammonia, URL rewriting, linkify) to the same protocol. The command contract is unchanged.

## G43. A minimal add-account form in M4 — Resolved 2026-10-20
The form asks for the email, runs resolve, shows the server fields to edit, and takes the password. It has Test, Save, and Save anyway after a failed test. It also offers to pin a certificate it shows. It uses the `account.*` commands. M10 turns it into onboarding.

## G44. Which startup the budget measures — Resolved 2026-10-20
The 1.5 s startup budget is measured with the app cold and the OS warm:
- The app is not running, but the OS and WebView2 files are cached.
- The result is the median of 5 launches, after one discarded warm-up launch.
- The end point is an interactive cached inbox: list rows are rendered.

CI enforces the budget from M4.

## G45. UI items of later milestones are hidden until built — Resolved 2026-10-20
Until their milestones:
- All Inboxes (M9), Snoozed (M9), Outbox (M7) and Group by conversation (M9) are not shown.
- The reading-pane position setting (M10) is not shown, and the pane sits on the right.
- Search (M9), Compose and reply (M7), and message actions (M6) are not shown.
- The list shows one row per message.

## G46. No command listed the accounts — Resolved 2026-10-20: `account.list` added
At startup the UI needs every account and its current state. The owner added `account.list`. It takes no arguments and returns every account summary with its current state and last error. After that, `account-state` events keep the UI current. Section 3 was amended.

## G47. Phosphor icon source — Resolved 2026-10-20
The owner approved downloading the regular-weight SVGs (and `star-fill`) of `@phosphor-icons/core` 2.1.1 (MIT) from unpkg.com. Only their path data and the MIT notice are copied into `ui/src/lib/icons.ts` (G10). Nothing is a dependency.

## G48. When the database integrity check runs — Resolved 2026-10-20
Section 12 moves a database that fails its integrity check aside as `*.corrupt` and rebuilds it. The owner chose:
- **At open:** if SQLite reports that the file is not a database or is corrupt, the file is moved aside and a fresh database is created at once.
- **After startup:** a `PRAGMA quick_check` runs in the background, after the inbox is showing. If it fails, `storage-warning` (`db_corrupt`) fires, and the move-aside and rebuild happen at the next start.
- **Either way:** the user is warned that outbox items in the old file may be lost.

## G49. M4 wiring details — Resolved by interpretation, ratified by the owner 2026-10-20
- **Capability file:** section 13.3 says the capability file lists only the app's own commands. It lists exactly the eleven M4 commands, plus `core:event:allow-listen` and `core:event:allow-unlisten`, without which the UI cannot receive the section 3 events. Nothing else is allowed.
- **Perf mode:** in the harness only (`ZEROMAIL_PERF_OUT` set), the app adds one runtime capability, `core:event:allow-emit`, so the page can report its startup and scroll marks. The shipped capability file never allows it, so the page's reports outside perf mode fail silently.
- **Data directory override:** the harness and tests set `ZEROMAIL_DATA_DIR`. Without it the app uses the OS app-data directory.
- **Command names:** Tauri commands are Rust identifiers, so `account.list` is invoked as `account_list`. `ui/src/lib/ipc.ts` keeps the section 3 names.
- **Folder open:** `list.page` without a cursor tells the account's actor which folder is open. It then syncs at once and every 5 minutes (section 6, step 9). No new command.
- **Message open:** `message.open` on an uncached body queues the fetch ahead of all other work and returns `body_kind: loading`. `store-changed` follows when the body is stored. A text part over 1 MB is cut there (`body_state` `text`, G35).
- **IDLE catch-up:** each IDLE connection queues one sync of its folder right after its SELECT. A change landing between the worker's sync and that SELECT would otherwise not be seen until the next change.

## G50. Visual values added in M4 — Resolved by interpretation, ratified by the owner 2026-10-20
Every value lives in `tokens.css`. New colors reuse existing palette values through alias tokens:
- **Account status dots:** up to date uses `--color-accent`. Connecting or syncing uses `--color-accent-400`. Waiting to retry uses `--color-text-muted`. Needs attention uses `--color-accent-2`. Off is a hollow ring in `--color-border-control`.
- **Skeleton rows:** `--color-divider-row`.
- **Banners:** use the card style with a muted or magenta icon. There is no new fill color.
- **Layout values the design implied:**
  - Row height 50 px (40 px compact), list header 41 px, status bar 28 px.
  - Sender and date columns 120 and 64 px, badge minimum width 24 px.
  - Form width 560 px, port and security fields 88 and 120 px.
  - Disabled opacity 0.5.
- **Monospace font:** certificate fingerprints use the system monospace stack (`--font-mono`), with nothing bundled.
- **Folder-reset toast:** shows for 10 s, the spec's only toast duration (section 9).

## G51. Two features no milestone named — Resolved 2026-10-20: the owner delegated the choice
- **Load older:** `folder.extend_window` adds 90 days per click (sections 3 and 12). It goes in **M6**, next to the other folder commands and the folder `ops`. Sync already keeps older cached rows, so the command only has to fetch the extra window and add a Load older control at the end of the list.
- **Logging:** section 13.8 asks for rotating log files (5 files of 2 MB, level info), with no message content or credentials at any level. It also asks for a debug switch with IMAP and SMTP traces that redact `AUTH`, `LOGIN` and message literals. `log.open` opens the folder. This goes in **M5**, early, so later milestones have logs to debug with. `tracing-appender` rotates by time, not size, so M5 adds a small size-rotating writer of our own on `tracing-subscriber`. No new crate. The UI switch for the debug setting arrives with Settings in M10.

## G52. How a clicked link reaches the system browser — Resolved 2026-10-20: option (a), `link.open`
Section 8 says `http`, `https` and `tel` links open in the system browser, never in-app. Section 3 has no command for it, and section 13.3 lets the page call only the app's own commands, so the page may not use the opener plugin itself.

Options:
- (a) **Recommended:** add the command `link.open {url}`. Core accepts `http`, `https` and `tel` only, and the app opens the URL through the official opener plugin from Rust. The capability file stays commands-only.
- (b) Grant the page the opener plugin's `allow-open-url` permission, scoped to those schemes. This adds a plugin permission to the capability file.

## G53. The per-message plain-text toggle on `mail-body` — Resolved 2026-10-20: option (a), `plain=1`
Section 8 has a per-message toggle from HTML to plain text, but the `mail-body` URL carries only `images=0|1`.

Options:
- (a) **Recommended:** add a `plain=1` query parameter: `mail-body://<message>?images=0|1&plain=0|1`. `message.open` returns the HTML URL; the toggle swaps the parameter.
- (b) No toggle until Settings (M10).

## G54. Save dialogs, and `message.save_source` in M5 — Resolved 2026-10-20: option (a)
`attachment.save` takes `path?` and `message.save_source` takes `path`. A path comes from a native Save dialog, but section 13.3 does not let the page call the dialog plugin. Section 12 also asks for Save source (`.eml`) on Could not display this message and a download option on Message too large, which are M5's render cases.

Options:
- (a) **Recommended:** with no path, the app opens the native Save dialog from Rust (official dialog plugin), with the sanitized file name as the default. `message.save_source`'s `path` becomes optional like `attachment.save`'s. M5 builds `message.save_source` for the two section 12 cases.
- (b) Grant the page the dialog plugin's `allow-save` permission.

## G55. Attachment file names on Windows — Resolved by interpretation (owner to ratify)
Section 8 strips path separators and control characters, prefixes Windows reserved names and caps the name at 200 characters. On Windows `:` also separates a drive or an NTFS stream, and `* ? " < > |` cannot be in a file name, so a save would fail or write a hidden stream. M5 strips those seven characters as well, on every OS. A name that ends up empty, `.` or `..` becomes `attachment`.

## G56. How T19 counts outbound requests — Resolved 2026-10-20: option (a), Rust tests and a WebView2 harness
T19 passes when "no script runs; harness records zero outbound requests". No harness watches the webview's network yet.

Options:
- (a) **Recommended:** both: Rust tests run the XSS corpus through the sanitizer and URL rewriting and check that nothing fetchable or executable survives; and a Windows CI harness launches the release app on a corpus database with WebView2's DevTools protocol, opens every corpus message, and fails on any network request outside the app's own protocols. Every corpus payload that would run tries to fetch a beacon URL, so zero requests also proves no script ran.
- (b) The Rust tests only.

## G57. Small M5 details — Resolved by interpretation (owner to ratify)
- **Remote-image limit:** the `mail-remote` proxy allows at most 4 requests at a time in total. That meets "at most 4 per message", and the URL stays `mail-remote://<encoded url>` as section 8 has it.
- **Images switch:** `mail-body` honours `images=1` only for a message the user allowed (Load images for this session, or a sender in `remote_image_allow`). Otherwise it answers as `images=0`.
- **`mailto` links before M7:** compose arrives in M7 (G45), so until then a `mailto` click does nothing.
- **Link host check:** the visible text "looks like a URL" when it is a bare host, a host with a path, or an `http(s)` URL. The hosts differ when they are not equal after lower-casing and removing one leading `www.`.
- **Where files go:** logs in `<app data>/logs` (`zeromail.log` plus 4 older files of 2 MB); the attachment cache in `<app data>/attachments` (LRU, 500 MB); opened attachments in `<app data>/tmp/<random>/`, which is emptied at quit and at the next start.
- **Debug traces:** the switch is the setting `debug_logging` (default off), read at start. The UI for it comes in M10 (G51). IMAP traces redact `LOGIN` and `AUTHENTICATE` arguments, SASL responses, every literal and every quoted string in `FETCH` responses. SMTP traces redact `AUTH` lines and everything between `DATA` and the final `.`.

## G58. How the user hears that a queued action failed — Resolved 2026-10-20: option (a), event `op-failed`
Section 3 says that on permanent failure `ops` "reverts and emits an event", and section 9 says the user is told. No section 3 event carries it.

Options:
- (a) **Recommended:** add the event `op-failed {account_id, op_id, kind, error}`, fired once after the revert commits. The UI shows a 10 s toast such as Could not move 3 messages, with the error text.
- (b) Revert silently; `store-changed` alone brings the row back.

## G59. Hiding a moved or deleted message before the server confirms — Resolved 2026-10-20: option (a), the bit 5 overlay
A move, archive or delete must leave the list at once (T06), but the server owns the row until it acknowledges. Invariant 2 says `flags` is the server's flags with pending ops laid on top.

Options:
- (a) **Recommended:** a pending move or delete lays bit 5 (hidden, G19) on its rows, exactly as a pending star lays bit 1. Every view already hides bit 5. Success: the server expunges the source row and the next sync drops it. Failure: the bit comes off and the row is back with its flags. No schema change.
- (b) Delete the source rows locally and re-fetch their headers if the server refuses.

## G60. T25 before unified inbox, search and threads — Resolved 2026-10-20: option (a), split between M6 and M9
T25 is in M6's done-when: "Two rows, one entry in unified, search and thread views; Archive removes only the INBOX label." Unified inbox, search and thread views arrive in M9.

Options:
- (a) **Recommended:** split as G38 did. **M6:** on the Gmail fake, a message under two labels is two rows sharing one `gm_msgid`, and Archive from INBOX leaves the other label's row and its `gm_msgid` alone. **M9:** one entry in unified, search and thread views.
- (b) Keep T25 whole in M6 and build the counting rules for views that do not exist yet.

## G61. Hiding Gmail's Important and Starred folders — Resolved 2026-10-20: option (a), sync off and hidden
Section 9: on Gmail, `Important` and `Starred` "are hidden by default". `folders` has no hidden column.

Options:
- (a) **Recommended:** on accounts with `X-GM-EXT-1`, folders listed with `\Important` or `\Flagged` start with sync off, like All Mail, and the sidebar omits them while their sync is off. The account's folder settings list every folder, so turning sync on shows them.
- (b) Add a `hidden` column (schema change).

## G62. Small M6 details — Resolved by interpretation (owner to ratify)
- **Undo after the server acted:** a finished move, archive or delete keeps its `pending_ops` row as `done`, with the `COPYUID` result in `args_json`, for the 10 s Undo window; done rows are not counted as pending and are removed after it. Undo of a queued op deletes the row and lifts its overlay.
- **One op_id for a selection:** a command over messages in several folders queues one op per folder; they share the first op's id as a group, and `message.undo` reverses the group.
- **Archive on Gmail** moves to the `\All` folder (`[Gmail]/All Mail`); elsewhere to the `\Archive` folder; with neither, Archive is hidden.
- **Delete:** the UI asks before a permanent delete (in Trash or Junk, or no Trash folder; Empty Trash shows the count). Core performs what it is asked.
- **Load older (`folder.extend_window`, G51):** the window's start is the folder's oldest cached INTERNALDATE. A click fetches the 90 days before it; when that finds nothing, it fetches the 90 days before the next older message on the server, so a gap never blocks it.
- **Read state:** a message shown for 1.5 s is marked `\Seen` through `message.set_flags` (section 8). The setting for immediately or never comes with Settings in M10.
- **Multi-select:** commands take many messages and batch 500 UIDs per command (section 9). The M6 list keeps single selection; no milestone names a multi-select list UI.
- **Finding a message again without COPYUID:** Undo searches the destination with `UID SEARCH HEADER Message-ID <id>` (RFC 3501) rather than waiting for a resync of a folder that may not be synced. `SearchKey` gains `Header`.
- **Folder ops and LIST:** folder ops are pushed before the session's first `LIST`, and a `LIST` never drops a folder with an unacknowledged create or rename, nor brings back one with an unacknowledged delete. The local change and its op are written in one transaction.
- **Flag pulls:** a server flag pull and the re-laying of pending overlays happen in one transaction, so a reader never sees a local star or read state flicker away.
- **Which folders G61 hides:** `folders` keeps no LIST attributes, so the sidebar hides a folder whose sync is off, which has no role, and whose path is under Gmail's system container (`[Gmail]/` or `[Google Mail]/`). On Gmail those are exactly Important and Starred.
- **Undo of a star or read change:** `message.set_flags` also returns an op_id; Undo of a finished one queues the opposite change. No toast offers it (section 9 lists archive, delete and move), but the command accepts it.

## G63. Who opens a compose window — Resolved 2026-10-20: option (a), the app opens it from Rust
Section 11 gives each draft its own window. Section 13.3 lets the page call only the app's own commands, so the page may not create windows itself.

Options:
- (a) **Recommended:** `compose.new`, `compose.reply`, `compose.reply_all` and `compose.forward` create the draft and the app opens its window from Rust (label `compose-<draft_id>`, the same UI on a compose route). The capability file covers `main` and `compose-*` windows with the same command list. Closing the window uploads the draft (section 7).
- (b) Compose opens inside the main window (an overlay or a pane) in M7; separate windows come later.

## G64. Identities and signatures have no commands — Resolved 2026-10-20: option (a), `identity.*` commands
M7 builds identities: section 5 says every account has at least one identity and extra send-as addresses are typed in by hand, and section 11's compose has an identity picker. Section 3 has no command to list, add or remove identities, and none that edits `accounts.signature_html` and `signature_text`.

Options:
- (a) **Recommended:** add `identity.list {account_id}`, `identity.save {identity}` (add or edit; one default per account) and `identity.delete {identity_id}` (never the last one). Signatures become fields of `account.update`'s config. The Settings screens that edit them arrive in M10; M7 shows the picker and a small identities list in the account settings.
- (b) Read identities only through the compose commands' response, and edit them through `settings.set` from M10.

## G65. The size check when the server cannot be asked — Resolved 2026-10-20: option (a), check when reachable
Section 7: before queueing, check the size against SMTP `SIZE` and IMAP `APPENDLIMIT` and block with a clear message. Neither limit is stored, and offline neither server can be asked. Compose and send must work offline (section 6).

Options:
- (a) **Recommended:** `draft.send` asks the SMTP server (`EHLO`, then `QUIT`, under the normal timeout) and uses the IMAP `APPENDLIMIT` the account's actor last saw. Over a limit: `too_large`, nothing queued. Server unreachable: the message is queued, and a later `552` makes it `failed` with `too_large`.
- (b) Never queue without a successful size check: offline sends are refused.

## G66. How the Sent copy and draft uploads reach IMAP — Resolved 2026-10-20: option (a), through `ops`
Section 7: the outbox APPENDs the Sent copy and uploads drafts; "a failed APPEND is retried as an ops job". Section 6 caps an account at 5 IMAP connections, all owned by its actor, and `outbox` may not call `auth` for credentials.

Options:
- (a) **Recommended:** both always go through `ops`: the outbox queues a `pending_ops` row (`append` with the outbox id, or `draft_replace` with the draft id and the UID to remove) and the actor pushes it on its worker connection. Retries, offline and the connection cap are then the existing ops machinery. `outbox` itself only speaks SMTP.
- (b) The outbox opens its own short-lived IMAP connection per APPEND with credentials Core hands it, falling back to an ops job when that fails.

## G67. Small M7 details — Resolved by interpretation (owner to ratify)
- **Quoting:** replies and inline forwards quote the original's plain text, HTML-escaped, in a `<blockquote>`, under `On <local date>, <name> <address> wrote:` (or the forwarded header block). The original's HTML never enters the compose window, so no hostile markup reaches an unsandboxed page.
- **Rich text:** the composer is a `contenteditable` with bold, italic, underline, lists and links, no UI kit. Its HTML goes through the same `ammonia` allowlist before the message is built.
- **Contacts:** every From, To and Cc of synced mail counts 1 use; every recipient of a sent message counts 3 (section 10: sent addresses weigh more). Suggestions rank by use count, then recency, at most 10.
- **Undo window:** 10 s until the setting arrives with Settings (M10). Send later (section 18.3) is M9.
- **Retry schedule:** `retry_wait` waits 30 s, 2 min, 10 min, then 1 h for every later attempt, and the sixth failed attempt is `failed` (section 7).
- **Outgoing HTML:** the composer's HTML goes through the incoming allowlist (`mail_render::sanitize_outgoing`), but links and `http(s)` images keep their real URLs for the recipient; the reader's `mail-remote`/`data-href` rewrite applies only to display.
- **Command arguments:** `compose.new` takes the account and an optional address (a `mailto:` link or the Compose button). `draft.add_attachment` takes an optional path (none: the native Open dialog, as Save does in G54) and, like `draft.remove_attachment`, returns the draft's attachment list, since the page cannot know the new attachment's id, name and size otherwise.
- **Outbox actions:** Edit is `outbox.undo` (a queued, failed or `unknown` item goes back to its draft and the compose window reopens). The Outbox view lists unsent items; sent rows stay as history for section 18.3.
- **Plain part:** built from the editor with quoted lines prefixed `> ` and the `-- ` signature line; the attach warning ignores both.
- **Closing a compose window:** the app asks the page to save first and closes once that save lands (3 s at most), so the last 5 s of typing are not lost. No command, event or permission is added: the app runs the page's save hook.
- **Draft uploads** carry the typed message and attachments but not a forward-as-attachment `.eml`; the original stays on the server and is attached again at send.
- **Signatures:** the account's plain signature is placed by the composer; editing it is part of Settings (M10).
- **Test hook:** `Core::set_timing` shortens the undo window, upload delay and outbox tick in tests only; the app never calls it.

## G68. The T19 WebView2 harness cannot open DevTools on CI — Resolved 2026-10-20: option (a), a CI-safe walk with a local beacon server
The G56 harness passes on the build machine (44 corpus messages opened, zero outbound requests), but on GitHub's `windows-2025` runner WebView2 never opens the DevTools port: not with `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`, not with its own profile folder, not with WebView2's per-app registry override. The app runs and WebView2 starts; no policy is set on the runner.

Options:
- (a) **Recommended:** a CI-safe harness without DevTools. In the harness only (an environment variable, like perf mode, G49), the page walks every message of the open folder and reports which ones it opened. The corpus beacon host becomes a local HTTP server that the harness runs; anything that leaves the webview (a script, a navigation, an unrewritten URL, a proxied image) reaches it and fails the run. The DevTools harness stays as a local tool.
- (b) Keep the DevTools harness, run it on the build machine at each milestone and record the result in the report; CI runs the Rust half of T19 only.

## G69. Nothing lists the outbox — Resolved 2026-10-20: option (a), `outbox.list`
Section 11: a per-account Outbox folder lists failed and `unknown` sends with Retry, Edit and Discard, with a sidebar badge; section 18.3 adds scheduled sends. `list.page` scopes are folder, unified, search and thread, and outbox rows are not messages (they have a state, an error and a send time). No command returns them.

Options:
- (a) **Recommended:** add `outbox.list {account_id}`, returning each item's id, subject, recipients, state, error, send time and creation time. `outbox-state` events keep it current, and the sidebar badge counts failed and `unknown` items.
- (b) Add a scope `outbox` to `list.page`, with rows that carry the outbox fields.

## G70. Google's Desktop-app client needs its client secret — Resolved 2026-10-20: option (a), keychain
Section 5 says the user pastes the client ID, and the app ships no client IDs or secrets. But Google's token endpoint refuses a Desktop-app client that sends only the ID, even with PKCE: the reply is `invalid_request`, "client_secret is missing". Google's Credentials page shows a client secret for every Desktop client. Google's documentation says that for installed apps this secret "is obviously not treated as a secret". The schema has `oauth_client_id` and no column for the secret.

Options:
- (a) **Recommended:** the form also takes the client secret. It goes to the keychain as `account:<id>:oauth_client_secret`, a third `SecretKind`, and never into the database. It is sent only to Google's token endpoint. The ID stays in `accounts.oauth_client_id`. No schema change.
- (b) Store the secret in a new `accounts` column (schema change; a secret in the database breaks invariant 5).
- (c) Client ID only. Sign-in would fail at Google for Desktop clients, so T31 cannot pass.

## G71. Signing in with Google when the account does not exist yet — Resolved 2026-10-20: option (a), save then sign in
`account.oauth_start` takes an `account_id`, so the account exists before the browser opens. Section 5, step 4 saves an account only after Test connection passes, but Test needs a token, which needs the sign-in.

Options:
- (a) **Recommended:** save first, then sign in.
  - `account.add` with `auth_kind: oauth_google` saves the account with its client ID and secret and runs no test. Its actor waits in `NeedsAttention` (`auth_failed`, no refresh token) and makes no connection.
  - The form then calls `account.oauth_start`, and the app opens the system browser. On the loopback redirect Core exchanges the code and stores the refresh token in the keychain. It then runs Test connection (IMAP XOAUTH2 plus `LIST`, SMTP XOAUTH2) and fires `oauth-done {ok, error?}`. On success the actor connects.
  - Cancelling the new-account form before a successful sign-in removes that account.
  - Sign in again (the `NeedsAttention` banner) is `account.oauth_start` on the existing account. Nothing is deleted, so no data is lost (T32).
- (b) Sign in before saving. `account.oauth_start` would need a config instead of an account_id (a command contract change).

## G72. Small M8 details — Resolved by interpretation (owner to ratify)
- **When tokens refresh:** a login uses the access token in memory while more than 5 minutes of it remain. Otherwise the login refreshes it first (section 5). A connection already signed in stays open; Google decides how long. A restart always refreshes, because access tokens are never stored.
- **Refresh failures:**
  - A refused grant or client (`invalid_grant`, `invalid_client` and the other OAuth error codes) is `auth_failed`. The account goes to `NeedsAttention`, with no retry (T32).
  - Google out of reach, a 5xx or `temporarily_unavailable` is `network`. The account goes to Backoff, and a send waits in `retry_wait` instead of failing (the M7 hand-over question).
- **The consent request:** scope `https://mail.google.com/`, PKCE S256, `access_type=offline` and `prompt=consent` (Google returns a refresh token only then), and `login_hint` with the account's address. The client secret travels in the token request body, never in the browser URL.
- **The loopback listener:** `127.0.0.1` on a random port, waiting at most 5 minutes. It accepts only a `GET` that carries the request's `state`; anything else gets a 404 and is ignored. The browser page it returns is a single sentence with `Content-Security-Policy: default-src 'none'`. A second `account.oauth_start` for the same account closes the first listener.
- **After the exchange:** the refresh token is saved and the actor reconnects, even if the Test connection that follows fails. `oauth-done` then carries the test's error.
- **Editing a Google account:** the edit saves without a test, because the next sign-in tests it. Empty client fields keep the saved ID and secret. The sign-in method cannot change on an existing account.
- **The setup form:** when the host offers both, a Sign-in method choice, Google first (section 5, step 3). A Workspace domain resolves to nothing (no autoconfig, and no MX guess, G27), so the manual form offers the choice by the typed IMAP host, as `auth_methods` does: Google hosts get both methods. Google sign-in shows the client ID and secret fields in place of the password, and Save and sign in with Google in place of Test and Save.
- **Fakes:** the fake IMAP and SMTP servers record each login's mechanism (`password` or `xoauth2`, never the secret), so tests can tell XOAUTH2 was used.

## G73. Zero Mail becomes a public app — Resolved 2026-10-20: public app, new M11
The owner wants a public app: anyone enters their address and uses it, whatever their mail server. The spec assumes personal use: assumption 3 says no public release and a bring-your-own Google client. These parts change:
- **Sign-in per provider:**
  - Generic IMAP/SMTP with an app password works today (Fastmail, iCloud, Yahoo, self-hosted, and Google with an app password).
  - Google sign-in for everyone needs one Zero Mail client shipped in the app. That breaks section 5's "ships no client IDs or secrets". The mail scope is restricted, so Google must verify the app: privacy policy, verified domain and possibly a yearly third-party security assessment. Until then, External testing mode allows 100 named users with weekly re-sign-in.
  - Microsoft 365 and Outlook.com require OAuth for IMAP and SMTP. They are a non-goal (section 1, v0.3). "Regardless of server" brings them back: an Azure app registration and Microsoft OAuth endpoints (section 13 network allowlist).
- **Server discovery:** custom domains hosted by Google or Microsoft (for example circuitry.digital) find no settings, because the MX guess was dropped (G27). A public app needs it back, through a DNS crate or a lookup service.
- **Distribution (M11):** signed installers (Windows code signing, Apple notarization), a releases page, and updates (assumption 12: manual).
- **What stays:** the architecture, the local-only data, no telemetry, and the security layers.

The owner decided:
- **Google:** Zero Mail ships one Google client and goes through Google's verification. Until approval: testing mode for up to 100 users, plus app passwords. The bring-your-own client stays available.
- **Microsoft:** Microsoft 365 and Outlook.com come back through Microsoft OAuth, in a new milestone that reuses M8's OAuth machinery.
- **Order:** M8 is tagged after T31 and T32 with the owner's own client; the token code is the same for a shipped client. M9 and M10 are built as specified. A new **M11, Public release**, holds the shipped Google client, Microsoft OAuth, MX discovery for custom domains, and the update path. Its scope is written into the spec before it starts. Packaging moves to **M12**.
- **Earlier references:** "M11" in entries written before v0.18 (G24, G25, G27) means the packaging milestone, now M12. G24's question about macOS and Linux targets, and G25's manual test round, are decided at M12.

## G74. Finding a thread through References needs an index — Resolved 2026-10-20: option (a), `message_refs`
Section 10 threads messages by union-find: two messages share a thread if they share any ID in their Message-ID, In-Reply-To and References. T29 needs out-of-order replies and a missing middle message to join. Example: C (References A and B, In-Reply-To B) arrives before A, and B never arrives. When A arrives, the only link is that C's References contain A.
- `refs` is a space-separated text column, so "which stored messages reference this ID" is a full scan of the account per new message. With 100,000 messages a first sync would scan billions of rows.

Options:
- (a) **Recommended:** a new table `message_refs(account_id, ref, message_id)`, indexed on `(account_id, ref)`, holding each message's own ID, In-Reply-To and References. Rows cascade with their message. Each new message then costs a few index lookups. One table, written by the store in the same transaction as the message.
- (b) No schema change. Sync keeps an in-memory map from every ID to its thread, built from the store when the actor starts (a background read of the account; tens of MB at 100,000 messages).
- (c) A `LIKE` scan of `refs` per new message (no schema change; too slow for the section 14 budgets).

## G75. Server-search hits "marked as outside the sync window" — Resolved 2026-10-20: option (a), `folders.window_start`
Section 10: hits not in the cache "are header-fetched into the store, marked as outside the sync window, then listed". No column holds that mark. It matters because Load older starts at the folder's oldest cached message (G62). Without a mark, a 5-year-old hit moves the window start back 5 years, and Load older skips everything between.

Options:
- (a) **Recommended:** a new column `folders.window_start` (UTC seconds, 0 until the first sync). The first sync sets it to now minus `sync_days`, and Load older moves it back 90 days per click. A stored message older than it is outside the sync window. It is listed and searchable but not counted by Load older. One column, no per-message mark.
- (b) No schema change: keep each folder's window start in `settings` under a per-folder key.
- (c) A per-message mark: a flags bit 6. The `flags` CHECK allows bits 0 to 5 only, so this means a table rebuild.

## G76. Rules need List-Id, which is not stored — Resolved 2026-10-20: option (a), `messages.list_id`
Section 18.6 rule conditions include `list-id`. Sync already fetches the List-Id header (with List-Unsubscribe and List-Unsubscribe-Post) but stores none of them. Rules with a body condition wait for the body prefetch, so the header must still be known later.

Options:
- (a) **Recommended:** a new nullable column `messages.list_id`, filled at header fetch. The List-Unsubscribe headers are M10's question (section 18.7).
- (b) Evaluate `list-id` only while the headers are in memory during sync. A rule that combines `list-id` with a body condition fetches the List-Id header again when the body arrives.
- (c) Drop `list-id` from the conditions.

## G77. M9 details — Resolved by interpretation (owner to ratify)
- **Where rules run:** section 3 has no rules module. Sync reports new rows after each folder sync (not a first sync) and stored bodies, as internal events. A task in `mail-core` runs rules, mute and the snooze timer on them. Every action is an ordinary command (`ops`), with the usual Undo. Rules run on INBOX and watched folders only.
- **Not new mail:** a message this app itself moves into INBOX or a watched folder is not new mail there, for an hour. That covers Undo, a move back by hand, and the return of a snooze. Rules do not run on it again (T40), and in M10 it will not notify.
- **Rules:**
  - A rule's folders are named by path, so a rule for every account files into the folder of that path in each one.
  - Conditions match case-insensitively, as "contains".
  - A rule that needs the body waits until the prefetch stores it, and so do the rules after it.
  - Block sender is a preset rule: from the address, delete, stop. Unblock deletes it.
- **Snooze:**
  - The `Snoozed` folder is created on first use; its path is the setting `snooze.folder`. Folder ops are now pushed before a folder's message ops, so a first snooze never moves to a folder the server does not have yet.
  - After the move, its executor header-fetches the copy in `Snoozed` and points the snooze at it, before the source row goes.
  - At the due time, `-\Seen` and a move back to the origin folder (INBOX when the origin is gone, G22), then `snooze-due`. The OS notification is M10's (T37), as G38 split T04.
  - A reply in a snoozed thread ends the snooze early (setting `snooze.end_on_reply`, default on).
  - Undo of a snooze also removes the snooze.
- **Undo race:** an op can start between Undo's look and its cancel. Undo then waits once for it to finish and reverses it, instead of answering "being sent".
- **Mute:** `$Muted` is set where the folder's PERMANENTFLAGS include `\*`; the executor selects the folder again to read them. A message that arrives with `$Muted` (another install) mutes its thread here. Muting archives only new messages; what is in INBOX stays (section 18.5).
- **Send later (18.3):**
  - Cancel is `outbox.undo` (back to the draft).
  - Send now is `outbox.retry` on a queued item.
  - An item sent more than 60 s after its time is marked late, through a `sent_at` in its envelope; `outbox.list` returns `late`.
  - The Outbox view shows a Sent history (the newest 50) with Sent late.
- **Templates:** `draft.apply_template {draft_id, template_id}` fills an empty subject, saves the draft, and returns the template with its HTML sanitized. The composer inserts that body at the cursor (T41). Templates are stored sanitized.
- **Search:**
  - `before:` and `after:` dates are UTC midnights.
  - `from:`, `subject:` and words use FTS5 column filters, with prefix matching on the last word. `to:` and `cc:` narrow by FTS, then match the address lists.
  - Server search sends what IMAP can express and narrows the rest locally. On Gmail the query goes as `X-GM-RAW` to All Mail, without `account:` terms.
  - Search lists messages, not threads.
- **Thread view:** `list.page` with scope thread takes the open folder. Every expanded message is a full reading card; collapsed ones are one line and expand on a click.
- **New indexes and test speed:** indexes added with v3: `messages(account_id, gm_msgid)`, `messages(account_id, date_utc, id)` and `threads(account_id, subject_norm)` (indexes are the builder's job, section 4). Test builds compile SQLite's C code optimized, so the T30 timing measures SQLite's real speed.
- **Settings without a screen:** subject grouping (`threads.subject_merge`, default off), Group by conversation (on), and the snooze settings. Their switches come with Settings in M10, and so do the Rules and Templates management screens (section 11, Settings).

## G78. Unsubscribe needs List-Unsubscribe, which is not stored — Resolved 2026-10-21: option (a), two columns
Section 18.7: an Unsubscribe button shows in the header when `List-Unsubscribe` is present, preferring RFC 8058 one-click (`List-Unsubscribe-Post`). Sync already fetches both headers but stores neither (G76 left them to M10).

Options:
- (a) **Recommended:** two nullable columns, `messages.list_unsubscribe` and `messages.list_unsubscribe_post`, filled at header fetch. The button shows offline too, and `message.unsubscribe` reads them.
- (b) Fetch the headers when a message opens (a round trip per open). No button offline, and none until the fetch answers.

## G79. The Privacy and Sync and storage screens need commands — Resolved 2026-10-21: option (a), three commands
Section 11 Settings has Privacy (the remote image allowlist) and Sync and storage. Invariant 7 asks for a Clear body cache action. No command lists or removes allowlisted senders, and none clears the body cache.

Options:
- (a) **Recommended:** three commands. `images.list_allowed {account_id}` returns the senders, `images.disallow_sender {account_id, address}` removes one, and `storage.clear_body_cache {}` drops bodies and attachment files, never headers.
- (b) Pseudo-keys through `settings.get` and `settings.set`; no new commands, but the settings table does not hold these.
- (c) Privacy lists nothing, and Clear body cache is left out.

## G80. A notification click cannot open its message — Resolved 2026-10-21: option (a), focus and open the newest
Section 11: "Click focuses the app on that message." The official notification plugin, the only one allowed, reports clicks on mobile only. On Windows a click on an installed app's toast starts the app again; with single instance (section 11), that focuses the running window.

Options:
- (a) **Recommended:** a click focuses the app (single instance). When the window comes to the front within 10 s of a notification, the app opens the newest notified message. A coalesced "N new messages" notification opens that account's INBOX.
- (b) A click only focuses the app.
- (c) Add a crate that reports Windows toast clicks (outside the section 2 table).

## G81. A taskbar unread count on Windows — Resolved 2026-10-21: option (a), an overlay dot
Section 11: "Unread badge on the dock or taskbar where the OS supports it." Tauri's badge count covers macOS and Linux, not Windows. Windows has a taskbar overlay icon, but it is an image, and drawing digits needs an image crate. The window title and tray tooltip show the count either way (section 11).

Options:
- (a) **Recommended:** on Windows, a small solid dot overlay, drawn in code in the accent color, while anything is unread. The number is in the title and the tray tooltip. macOS and Linux use the badge count (M12).
- (b) No taskbar badge on Windows; the title and tray only.
- (c) A numbered overlay from bundled digit images (new image assets).

## G82. M10 details — Resolved by interpretation (owner to ratify)
- **Settings registry:** `settings.get` and `settings.set` accept only the keys Core lists, each with a type, allowed values and default: `ui.theme` (system, light, dark), `ui.density` (comfortable, compact), `ui.reading_pane` (right, bottom, off), `ui.group_by_thread` (on), `reading.mark_read` (delay, immediately, never), `reading.html` (on), `compose.undo_seconds` (0 to 30, default 10), `notify.preview` (off), `notify.accounts.<id>` (on), `app.keep_in_tray` (on), `app.autostart` (off), `privacy.online_lookups` (on), `debug_logging` (off, applied at once), `threads.subject_merge` (off), `snooze.folder` (Snoozed), `snooze.end_on_reply` (on). Anything else is `invalid_input`; `get` answers the default for a key never set. Subject grouping applies to mail threaded after the change.
- **The `notify` payload** carries, beyond the section 3 fields, `preview` (only with the preview setting on), `count` and `account_name`: a count above 1 is the coalesced "N new messages in <account>", and `message_id` is then the newest of them.
- **Which messages notify:** new mail in INBOX or a watched folder (never a first sync, never a message this app moved there, G77), after mute and the rules: still unread and visible, no Never notify rule, INTERNALDATE within 30 minutes, notifications on for the account. A message whose rules wait for its body notifies once they ran. With the preview on, a notification waits up to 5 s for the body.
- **Coalescing:** per account, the first 3 in a 10 s window show one by one. A batch that would pass 3 shows at once as one "N new messages"; what arrives later in that window waits for the window's end and shows as one more. A steady stream shows one notification per 10 s.
- **Where notifications show:** Core decides and emits `notify`; the app shows the OS notification only while the main window is unfocused or hidden. The few strings the OS shows outside the page (tray menu, tooltip, window title, the coalesced text) live in `src-tauri/src/strings.rs`.
- **Notification click (G80):** the page remembers the newest notification that arrived while it was in the background; when the window comes to the front within 10 s, it opens that message, or the account's INBOX for a coalesced one, once.
- **Unread count:** unread INBOX messages of enabled accounts, each Gmail message once. Title `Zero Mail (N)`, tooltip `Zero Mail: N unread`, the Windows dot (G81); macOS and Linux get the badge count through the same call, checked at M12.
- **Tray and closing:** a left click on the tray icon opens the window; Compose opens a draft on the first enabled account. With Keep running in tray off, closing the main window quits. Autostart is applied when the setting changes.
- **Shortcuts:** single keys do nothing while the focus is in a text field, a select or an editable area; Ctrl/Cmd combinations and Esc stay active (Ctrl+Enter sends from the body). `g` waits 1.5 s for its second key. With the reading pane on, a selected message already shows, so Enter and `o` open a message in its own view only with the pane off. Esc closes the lightbox, then Settings, then the full view, then deselects.
- **Reading pane off:** a click (or Enter, `o`) opens the message in its own view, with Back to the list; Esc returns.
- **Print (18.8):** the page lays out a print view off screen (a header block with subject, from, to, cc and date above each body, a thread oldest first) and calls `message.print`, which checks the message or thread exists; the app then opens the webview's print dialog. Bodies come from `mail-body` with `images=1` only where images were already allowed. On paper only the print view shows.
- **Image preview (18.9):** the lightbox loads `mail-cid://localhost/<message>/attachment/<id>`. Core serves it only for an attachment of that message, `image/*` and at most 20 MB, with the same `default-src 'none'` response CSP as inline images.
- **Unsubscribe (18.7):** the first `https:` and the first `mailto:` URI in `List-Unsubscribe` count; one-click needs an `https:` URI and `List-Unsubscribe-Post: List-Unsubscribe=One-Click`. The POST goes through the approved HTTP client (HTTPS only, no cookie store, no referrer, 3 redirects at most); a 2xx answer is success, anything else `server_rejected`. A `mailto:` draft takes the URI's subject and body; with no subject it is `unsubscribe`. An `https:` page opens in the system browser through the same check as `link.open`. The reader shows Unsubscribe only when a method exists, and asks first.
- **Settings screens:** Accounts lists each account with On and Edit; for a Gmail account it links Gmail's own filter, vacation, forwarding, spam, 2-Step Verification and storage pages (section 18 table, server-side). Signatures are edited as plain text; the HTML part is that text escaped, saved through `account.update` without a new connection test. Templates can be renamed and their subject changed; the body is made in compose (Save as template) and shown read-only. The sync window length is not editable (`account.update` has no `sync_days`); Load older messages extends it. Debug logging and Open log are under Sync and storage. The sidebar's Settings item has no icon: no fitting icon is among the bundled Phosphor paths.
- **Onboarding:** with no account, a welcome step comes before the add-account form.
- **Clear body cache** also deletes every cached attachment file and sends `store-changed` for each account.
- **T35 in CI:** `tests/shell/single-instance.mjs` starts the release build twice in CI's Windows job. The running instance writes `ready` and `focused` to the file named by `ZEROMAIL_INSTANCE_OUT`, a test-only hook like the perf harness's.
- **Store API:** the message record now carries its list preview, for the notification preview.

