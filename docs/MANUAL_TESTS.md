# Manual tests

The owner runs these by hand against a real Workspace account. They are never automated (BUILD.md section 15).
Record each run as: date, build tag, OS, result, notes.

| ID | Test | Starting state | Steps | Pass condition |
| --- | --- | --- | --- | --- |
| T31 | OAuth sign-in, sync and send | Workspace account with IMAP on; Internal OAuth client of type Desktop app; its client ID and client secret at hand (SPEC_GAPS G70) | Add the account with Sign in with Google, the client ID and the secret; sign in through the browser; wait for sync; send a message to yourself; leave the app running for over 1 hour; then send again | Works; the second send needs no sign-in (token refreshed automatically) |
| T32 | Revoked OAuth access | T31 passed; a second account (for example the app-password account) also added | Revoke the app at myaccount.google.com/permissions; quit Zero Mail and start it again (a connection that is already signed in may stay open until it reconnects) | Account shows `NeedsAttention` with Sign in again; other accounts keep syncing; signing in again restores everything with no lost mail, drafts or outbox items |

## Run log

| Date | Tag | OS | Test | Result | Notes |
| --- | --- | --- | --- | --- | --- |
| 2026-10-20 | m8 | Windows 10 | T31 | Pass | Workspace account, Internal Desktop-app client; release build b9595f0 |
| 2026-10-20 | m8 | Windows 10 | T32 | Pass | Revoked at myaccount.google.com, restarted; signed in again with nothing lost |
