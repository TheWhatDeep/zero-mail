# Manual tests

The owner runs these by hand against a real Workspace account. They are never automated (BUILD.md section 15).
Record each run as: date, build tag, OS, result, notes.

| ID | Test | Starting state | Steps | Pass condition |
| --- | --- | --- | --- | --- |
| T31 | OAuth sign-in, sync and send | Workspace account with IMAP on; Internal OAuth client of type Desktop app; its client ID and client secret at hand (SPEC_GAPS G70) | Add the account with Sign in with Google, the client ID and the secret; sign in through the browser; wait for sync; send a message to yourself; leave the app running for over 1 hour; then send again | Works; the second send needs no sign-in (token refreshed automatically) |
| T32 | Revoked OAuth access | T31 passed; a second account (for example the app-password account) also added | Revoke the app at myaccount.google.com/permissions; quit Zero Mail and start it again (a connection that is already signed in may stay open until it reconnects) | Account shows `NeedsAttention` with Sign in again; other accounts keep syncing; signing in again restores everything with no lost mail, drafts or outbox items |

## Manual round after M10 (SPEC_GAPS G25)

These complete automated tests on the real app: the OS shows them, so no test can see them. Run them on a release build (`npx tauri build --no-bundle`, then `target/release/zero-mail.exe`) with your own accounts, before M11 starts.

| ID | Test | Starting state | Steps | Pass condition |
| --- | --- | --- | --- | --- |
| M10-1 | New mail notification (T05) | An account syncing; Zero Mail minimized or another window in front | Send yourself one message from another account or device | Exactly one Windows notification with the sender and subject (no preview); none while Zero Mail is in front |
| M10-2 | Notification click (G80) | M10-1 just fired | Click the notification within 10 s | Zero Mail comes to the front with that message open |
| M10-3 | Coalescing | Zero Mail in the background | Send yourself 5 messages within a few seconds | At most 3 single notifications and one "N new messages in <account>"; clicking that opens the Inbox |
| M10-4 | Tray and Keep running in tray | Default settings | Close the window with X; use the tray icon's menu: Open, Compose, then Quit | X hides to the tray and mail keeps syncing; Open shows the window; Compose opens a new draft; Quit ends the app. With the setting off in Settings, X quits |
| M10-5 | Unread count | Some unread mail in Inbox | Look at the window title, hover the tray icon, look at the taskbar button; then read one message | Title `Zero Mail (N)`, tooltip `Zero Mail: N unread`, a dot on the taskbar button; all follow the change; the dot goes when nothing is unread |
| M10-6 | Single instance (T35) | Zero Mail running | Start Zero Mail again from the Start menu or the exe | No second window; the running one comes to the front |
| M10-7 | Autostart | Settings, Notifications | Turn on Start Zero Mail when you sign in; sign out of Windows and back in; then turn it off and repeat | Starts with Windows only while the setting is on |
| M10-8 | Keyboard triage (T34) | A folder with mail | Using only the keyboard: j/k, Enter, r (then close the draft), e, s, Shift+U, /, type a search, Esc, g then i, Ctrl+, | Each acts as the Shortcuts screen says; letters typed in the search box or a compose field type text instead |
| M10-9 | Print (T44) | A 3-message conversation with remote images blocked | Open it, press Ctrl+P, look at the preview, cancel | All three messages, oldest first, each with its header block; no images from the web |
| M10-10 | Image preview (T44) | A message with a JPEG and a PDF attachment | Preview the JPEG; Esc; Open the PDF | The JPEG shows in the lightbox with Save and Open externally; the PDF opens in its own app |
| M10-11 | Unsubscribe (T43) | A newsletter you do not want | Click Unsubscribe, read the question, confirm | One-click lists confirm with a toast; mailto lists open a draft to send; others open the browser. Nothing happens before you confirm |
| M10-12 | Settings | Any | Switch theme, density and the reading pane (right, bottom, off); change Undo send to 30 s and send a message | Each applies at once and survives a restart; the Undo toast lasts 30 s |

## Run log

| Date | Tag | OS | Test | Result | Notes |
| --- | --- | --- | --- | --- | --- |
| 2026-10-20 | m8 | Windows 10 | T31 | Pass | Workspace account, Internal Desktop-app client; release build b9595f0 |
| 2026-10-20 | m8 | Windows 10 | T32 | Pass | Revoked at myaccount.google.com, restarted; signed in again with nothing lost |
