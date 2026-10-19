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
- The owner reviews the dark palette before M4 is tagged.

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

## G18. AppError codes — Open (blocks M2)
Section 3 says `AppError.code` is "a fixed enum" but does not list its members. A proposed list goes to the owner at the start of M2, before the first command is added.

## G19. `\Deleted` has no bit in `messages.flags` — Resolved 2026-10-19
Invariant 8 says messages flagged `\Deleted` on the server are hidden in every view. Section 9 also leaves some messages "flagged and hidden" when the server has no UIDPLUS. But the section 4 `flags` column listed only bits 0–4, so the store had no way to know a row is `\Deleted`.
- The owner chose **bit 5 = `\Deleted` on the server**. Every list and search query filters `flags & 32 = 0`. No new column.
- Section 4 was amended to match.

## G20. FTS external-content source — Resolved by interpretation (owner to ratify)
Section 10 indexes subject, from, to/cc and body text. Body text is in `bodies` and the headers are in `messages`. An FTS5 external-content table reads from one source, so:
- `message_fts` (columns `subject`, `sender`, `recipients`, `body`) uses the view `message_fts_content` (`messages LEFT JOIN bodies`) as its content. A view is not a table or a column. It is the only way to give FTS5 one source that spans the two tables.
- Triggers on `messages` and `bodies` keep the index in step, including cascaded deletes and upserts.
- `to_json`, `cc_json` and `reply_to_json` hold JSON arrays of `[name, addr]` pairs, with no keys. Only names and addresses reach the index, and FTS5's content consistency check works. That check fails when the view uses `json_each` or `json_tree`.

## G21. Nullability and "not set yet" values — Resolved by interpretation (owner to ratify)
Section 4 says types are the builder's job but marks some columns `NULL`. M1 reads that literally:
- Every column the spec marks `NULL` is nullable, and every other column is `NOT NULL`.
- "Not set yet" is written as `''` or `0`. Examples: `uidvalidity` and `synced_at` stay 0 until a folder is first selected (0 is not a valid UIDVALIDITY); `oauth_client_id` is `''` for password accounts; `text_html` is `''` for plain-only mail; `last_error` is `''` when there is no error.
- The migration test checks this nullability against the spec column by column.

## G22. Two folder references without a foreign key — Resolved by interpretation (owner to ratify)
- `pending_ops.folder_id` has no foreign key. A queued folder delete (M6) must outlive the local folder row it deletes optimistically, and a cascade would drop the op before it reached the server. The UIDVALIDITY purge drops the folder's ops explicitly (section 12).
- `snoozes.origin_folder_id` has no foreign key either, so that deleting the origin folder neither deletes the snooze nor is blocked by it. Where the message goes back to in that case is M9's question.
- Every other reference has a foreign key: `ON DELETE CASCADE` to its owner, or `NO ACTION` for `messages.thread_id` and `outbox.identity_id`.

## G23. `bcc:` search has no stored column — Open (blocks the `bcc:` operator in M9 only)
Section 10 lists `bcc:` (Sent and Drafts only), but `messages` has no Bcc column. Bcc is never written to the header (section 7), so the Sent copy on the server has none. Bcc exists only in `drafts.content_json` and `outbox.envelope_json`. To be decided before M9.

G1 update (2026-10-19): the owner installed Rust (1.99.1, MSVC host) and the VS 2022 Build Tools. Docker is not installed locally, by the owner's choice; the Stalwart tests run in CI (GitHub's Linux runners), and the fake transport covers local runs.
