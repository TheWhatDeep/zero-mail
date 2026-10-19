# Milestone reports

Format (CLAUDE.md): built / tests added / deviations (must be none) / gaps found.

## M0: green locally, waiting for CI before tagging (2026-10-19)

Every M0 check passes on Windows 10 (Rust 1.99.1, MSVC Build Tools 2022). M0 is "done when" `cargo test` and `npm test` are green **in CI**. There is no GitHub repo yet, so `m0` is not tagged. The Stalwart container has not been run, because the owner chose not to install Docker locally; CI runs it.

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

### To finish M0
1. Create a GitHub repository and push `main`. CI then runs the full suite on all three OSes, plus the Stalwart smoke check and the startup harness.
2. When CI is green, tag `m0`.
