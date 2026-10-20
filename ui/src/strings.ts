// Every user-visible string lives here (BUILD.md section 11, Accessibility: English only,
// all strings in one file). Components import from this module and never inline text.

import type { AccountState, ErrorCode, OpKind, Role } from "./lib/ipc";

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
  couldNotDisplay: "Could not display this message.",
  saveSource: "Save source (.eml)",
  showPlainText: "Plain text",
  showFormatted: "Formatted",
  imagesBlocked: "Images from the web are blocked in this message.",
  loadImages: "Load images",
  alwaysLoadImages: "Always load from this sender",
  linkCheckTitle: "Check this link",
  linkMismatch: (shown: string, real: string) =>
    `This link shows ${shown} but goes to ${real}. Open it in your browser?`,
  openLink: "Open link",
  attachmentsLabel: "Attachments",
  unnamedAttachment: "Unnamed attachment",
  openAttachment: "Open",
  saveAttachment: "Save as",
  executableTitle: "Open a program file",
  executableWarning: (name: string) => `${name} can run programs on this computer. Open it anyway?`,
  openAnyway: "Open anyway",

  // Message actions (section 9)
  actionsLabel: "Message actions",
  archive: "Archive",
  delete: "Delete",
  star: "Star",
  unstar: "Remove star",
  markRead: "Mark read",
  markUnread: "Mark unread",
  moveTo: "Move to",
  moveToPlaceholder: "Move to…",
  deleteForever: "Delete permanently",
  deleteForeverConfirm: "Delete this message permanently? This cannot be undone.",
  archivedToast: "Archived.",
  trashedToast: "Moved to Trash.",
  deletedToast: "Deleted.",
  movedToast: (folder: string) => `Moved to ${folder}.`,
  undo: "Undo",
  opFailed: (what: string, why: string) => `Could not ${what}. ${why}`,
  markAllRead: "Mark all read",
  emptyFolder: (folder: string) => `Empty ${folder}`,
  emptyConfirm: (n: number, folder: string) =>
    `Delete all ${n.toLocaleString("en")} messages in ${folder} permanently? This cannot be undone.`,
  loadOlder: "Load older messages",

  // Folder settings (section 9, Folder list)
  foldersTitle: "Folders",
  folderName: "Folder",
  newFolder: "New folder",
  newFolderHint: "Full name, for example Projects/2026",
  create: "Create",
  rename: "Rename",
  deleteFolder: "Delete folder",
  deleteFolderConfirm: (name: string) => `Delete the folder ${name} and every message in it, on the server too?`,
  syncToggle: "Sync",
  watchToggle: "Watch",
  roleLabel: "Role",
  noRole: "None",

  // Status bar
  statusLabel: "Sync status",
  pending: (n: number) => (n === 1 ? "1 change pending" : `${n.toLocaleString("en")} changes pending`),
  backoffWarning: (account: string, error: string) => `${account}: ${error}`,
  openLog: "Open the log folder",
  linkTarget: "Link target",

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

/** What a failed op was doing, for the op-failed toast (SPEC_GAPS G58). */
export const opKindText: Record<OpKind, string> = {
  flags: "update the message",
  move: "move the message",
  expunge: "delete the message",
  folder_create: "create the folder",
  folder_rename: "rename the folder",
  folder_delete: "delete the folder",
};

/** Folder roles in words (folder settings). */
export const roleText: Record<Role, string> = {
  inbox: "Inbox",
  drafts: "Drafts",
  sent: "Sent",
  archive: "Archive",
  junk: "Junk",
  trash: "Trash",
  all: "All Mail",
};

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
