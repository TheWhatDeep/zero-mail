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
