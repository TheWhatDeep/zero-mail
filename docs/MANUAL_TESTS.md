# Manual tests

The owner runs these by hand against a real Workspace account. They are never automated (BUILD.md section 15).
Record each run as: date, build tag, OS, result, notes.

| ID | Test | Starting state | Steps | Pass condition |
| --- | --- | --- | --- | --- |
| T31 | OAuth sign-in, sync and send | Workspace account; Internal OAuth client of type Desktop app; client ID at hand | Add the account with the client ID, sign in through the browser, wait for sync, send a message to yourself, leave the app running for over 1 hour, then send again | Works; the second send needs no sign-in (token refreshed automatically) |
| T32 | Revoked OAuth access | T31 passed | Revoke the app at myaccount.google.com/permissions, then wait for the next sync | Account shows `NeedsAttention` with Sign in again; other accounts keep syncing; signing in again restores everything with no lost mail, drafts or outbox items |

## Run log

| Date | Tag | OS | Test | Result | Notes |
| --- | --- | --- | --- | --- | --- |
