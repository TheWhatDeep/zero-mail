// Every user-visible string lives here (BUILD.md section 11, Accessibility: English only,
// all strings in one file). Components import from this module and never inline text.

import type { AccountState, ErrorCode } from "./lib/ipc";

export const strings = {
  appName: "Zero Mail",

  // Sidebar
  sidebarLabel: "Accounts and folders",
  addAccount: "Add account",
  inbox: "Inbox",

  // List
  listLabel: "Messages",
  today: "Today",
  earlier: "Earlier",
  messages: (n: number) => (n === 1 ? "1 message" : `${n.toLocaleString("en")} messages`),
  noMessages: "No messages",
  loadingRows: "Loading messages",
  yesterday: "Yesterday",
  toPrefix: "To:",
  noRecipients: "(no recipients)",
  unknownSender: "(unknown sender)",
  noSubject: "(no subject)",
  unread: "Unread",
  starred: "Starred",
  hasAttachment: "Has attachments",
  refresh: "Refresh",

  // Reader
  readerLabel: "Reading pane",
  noMessageSelected: "No message selected",
  from: "From",
  to: "To",
  cc: "Cc",
  replyTo: "Reply-To",
  date: "Date",
  previous: "Previous message",
  next: "Next message",
  position: (i: number, n: number) => `${i.toLocaleString("en")} of ${n.toLocaleString("en")}`,
  loadingBody: "Loading message",
  truncated: "Message too large: only the first 1 MB is shown.",
  deleted: "This message was deleted.",
  bodyFrameTitle: "Message body",
  couldNotOpen: "Could not open this message.",

  // Status bar
  statusLabel: "Sync status",
  pending: (n: number) => (n === 1 ? "1 change pending" : `${n} changes pending`),
  backoffWarning: (account: string, error: string) => `${account}: ${error}`,

  // Banners
  offline: "Offline, changes will sync later.",
  needsAttention: (account: string) => `${account} needs your attention.`,
  signInAgain: "Sign in again",
  reviewCertificate: "Review certificate",
  storageCorrupt: "The local database was damaged and is being rebuilt from the server. Unsent outbox items in it may be lost.",
  storageFull: "The disk is full. Changes are paused until there is space.",
  folderReset: (folder: string) => `${folder} changed on the server and is being downloaded again.`,
  dismiss: "Dismiss",

  // Account form
  addAccountTitle: "Add an account",
  editAccountTitle: "Account settings",
  emailLabel: "Email address",
  continue: "Continue",
  lookingUp: "Looking up server settings",
  nameLabel: "Your name",
  usernameLabel: "Username",
  passwordLabel: "App password",
  passwordKeep: "Leave empty to keep the saved password",
  incoming: "Incoming mail (IMAP)",
  outgoing: "Outgoing mail (SMTP)",
  hostLabel: "Server",
  portLabel: "Port",
  securityLabel: "Security",
  securityTls: "TLS",
  securityStartTls: "STARTTLS",
  manualHint: "No settings were found for this address. Enter your provider's server details.",
  foundHint: "Server settings found. Check them, then enter your app password.",
  test: "Test connection",
  testing: "Testing",
  save: "Save",
  saveAnyway: "Save anyway",
  cancel: "Cancel",
  removeAccount: "Remove account",
  removeConfirm: "Remove this account from Zero Mail? Nothing on the server is deleted.",
  imapOk: (folders: number) => `IMAP: signed in, ${folders} folders.`,
  smtpOk: "SMTP: signed in.",
  imapFailed: (why: string) => `IMAP: ${why}`,
  smtpFailed: (why: string) => `SMTP: ${why}`,
  certificateTitle: "The server's certificate is not trusted",
  certificateBody:
    "Trust it only if you run this server yourself and the fingerprint matches the one the server shows. A changed certificate will be refused again.",
  certificateFingerprint: "SHA-256 fingerprint",
  trustCertificate: "Trust this certificate",
  pinned: "Certificate trusted for this account",
  oauthLater: "Google sign-in arrives in a later version; use an app password for now.",
  fieldInvalid: "Check this field",
} as const;

/** The account states in words (status bar and tooltips). */
export const stateText: Record<AccountState, string> = {
  disabled: "Off",
  connecting: "Connecting",
  syncing: "Syncing",
  idle: "Up to date",
  backoff: "Waiting to retry",
  needs_attention: "Needs attention",
};

/** The UI never shows a raw error string (section 3); it shows these. */
export const errorText: Record<ErrorCode, string> = {
  network: "The server could not be reached.",
  timeout: "The server did not answer in time.",
  certificate: "The server's certificate is not trusted.",
  tls: "A secure connection could not be set up.",
  auth_failed: "The username or password was not accepted.",
  oauth: "Sign-in did not complete.",
  throttled: "The server asked to slow down; retrying in 10 minutes.",
  server_rejected: "The server refused the request.",
  recipient_rejected: "A recipient was refused.",
  too_large: "This is too large for the server.",
  unsupported: "The server does not support this.",
  not_found: "It is no longer there.",
  conflict: "That already exists.",
  invalid_input: "Some details are not valid.",
  keychain: "The system keychain could not be used.",
  disk_full: "The disk is full.",
  db_corrupt: "The local database is being rebuilt.",
  io: "A local file could not be read or written.",
  cancelled: "Cancelled.",
  internal: "Something went wrong.",
};

export type Strings = typeof strings;
