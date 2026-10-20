// The typed mirror of the command and event contract (BUILD.md section 3). The UI talks to
// Core only through these functions. Command names keep the spec's dots here; Tauri sees
// them with underscores (src-tauri/src/main.rs). Commands are added as their milestones
// build them.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/** The fixed error codes (SPEC_GAPS G18). */
export type ErrorCode =
  | "network"
  | "timeout"
  | "certificate"
  | "tls"
  | "auth_failed"
  | "oauth"
  | "throttled"
  | "server_rejected"
  | "recipient_rejected"
  | "too_large"
  | "unsupported"
  | "not_found"
  | "conflict"
  | "invalid_input"
  | "keychain"
  | "disk_full"
  | "db_corrupt"
  | "io"
  | "cancelled"
  | "internal";

/** For `certificate`, `message` is the presented certificate's SHA-256 (SPEC_GAPS G34). */
export interface AppError {
  code: ErrorCode;
  message: string;
  account_id?: number;
}

export type Security = "tls" | "starttls";
export type AuthKind = "password" | "oauth_google";

export interface Endpoint {
  host: string;
  port: number;
  security: Security;
}

export interface AccountSummary {
  id: number;
  name: string;
  email: string;
  imap: Endpoint;
  smtp: Endpoint;
  username: string;
  auth_kind: AuthKind;
  /** The Google client ID (not a secret); empty for password accounts. */
  oauth_client_id: string;
  pinned_cert_sha256: string | null;
  enabled: boolean;
  sync_days: number;
}

export type AccountState = "disabled" | "connecting" | "syncing" | "idle" | "backoff" | "needs_attention";

export interface AccountView extends AccountSummary {
  state: AccountState;
  last_error: AppError | null;
}

export interface Candidate {
  imap: Endpoint;
  smtp: Endpoint;
  username: string;
  source: "built_in" | "autoconfig" | "isp_db";
  auth_methods: AuthKind[];
  probe_error: AppError | null;
}

export interface TestReport {
  imap_error: AppError | null;
  smtp_error: AppError | null;
  folders: number;
}

/** The password and the Google client secret go to Core once and are never sent back. */
export interface AccountConfig {
  name: string;
  email: string;
  imap: Endpoint;
  smtp: Endpoint;
  username: string;
  auth_kind: AuthKind;
  password: string | null;
  pinned_cert_sha256: string | null;
  save_anyway: boolean;
  /** Google sign-in only (SPEC_GAPS G70); null keeps the saved value on update. */
  oauth_client_id?: string | null;
  oauth_client_secret?: string | null;
}

export type Role = "inbox" | "drafts" | "sent" | "archive" | "junk" | "trash" | "all";

export interface FolderView {
  id: number;
  account_id: number;
  path: string;
  name: string;
  depth: number;
  role: Role | null;
  unread: number;
  total: number;
  sync_enabled: boolean;
  watch_idle: boolean;
  /** A full sync has completed once. */
  synced: boolean;
  /** Gmail's Important and Starred while their sync is off (SPEC_GAPS G61). */
  hidden: boolean;
}

export interface Cursor {
  date_utc: number;
  id: number;
}

export type Scope =
  | { kind: "folder"; folder_id: number }
  | { kind: "unified" }
  | { kind: "search"; query: string }
  | { kind: "thread"; thread_id: number };

export interface ListRequest {
  scope: Scope;
  cursor: Cursor | null;
  limit: number;
  group_by_thread: boolean;
}

export interface Address {
  name: string;
  addr: string;
}

export interface Row {
  id: number;
  account_id: number;
  folder_id: number;
  thread_id: number;
  subject: string;
  from: Address;
  to: Address[];
  date_utc: number;
  unread: boolean;
  flagged: boolean;
  has_attach: boolean;
  preview: string;
}

export interface Page {
  rows: Row[];
  next: Cursor | null;
}

/** HTML or plain text from mail-body; loading while it is fetched; unreadable when the
 * server could not deliver it (Could not display this message, section 12). */
export type BodyKind = "html" | "plain" | "loading" | "unreadable";

export type RemoteImages = "none" | "blocked" | "allowed";

export interface Attachment {
  id: number;
  filename: string;
  mime: string;
  size: number;
  /** Open asks first (section 8). */
  executable: boolean;
}

export interface MessageView {
  id: number;
  account_id: number;
  folder_id: number;
  subject: string;
  from: Address;
  to: Address[];
  cc: Address[];
  reply_to: Address[];
  date_utc: number;
  unread: boolean;
  flagged: boolean;
  body_kind: BodyKind;
  /** A text part was cut at 1 MB (section 12, Very large message). */
  truncated: boolean;
  /** The plain-text toggle applies (SPEC_GAPS G53). */
  has_html: boolean;
  body_url: string;
  attachments: Attachment[];
  remote_images: RemoteImages;
  /** The account has an archive folder (section 9). */
  can_archive: boolean;
  /** Delete here is permanent and asks first (section 9). */
  delete_is_permanent: boolean;
}

/** message.set_flags: a field left out is unchanged. */
export interface FlagChange {
  seen?: boolean;
  flagged?: boolean;
}

export interface DraftAttachment {
  id: string;
  name: string;
  mime: string;
  size: number;
}

/** What the composer quotes; it formats the date in local time (section 7). */
export interface Quote {
  date_utc: number;
  from: Address;
  to: Address[];
  subject: string;
  text: string;
  forwarded: boolean;
}

/** A draft's content (drafts.content_json). */
export interface DraftContent {
  identity_id: number | null;
  to: Address[];
  cc: Address[];
  bcc: Address[];
  subject: string;
  html: string;
  text: string;
  plain_only: boolean;
  attachments: DraftAttachment[];
  forward_attachment: number | null;
  in_reply_to: string | null;
  references: string[];
  original: number | null;
  mode: "new" | "reply" | "reply_all" | "forward" | string;
  quote: Quote | null;
  message_id: string;
}

export interface IdentityView {
  id: number | null;
  account_id: number;
  name: string;
  address: string;
  reply_to: string | null;
  is_default: boolean;
}

/** What a compose window opens with (SPEC_GAPS G63). */
export interface DraftView {
  draft_id: number;
  account_id: number;
  content: DraftContent;
  identities: IdentityView[];
  signature_html: string;
  signature_text: string;
}

export type OutboxStateName = "queued" | "sending" | "retry_wait" | "sent" | "failed" | "unknown";

/** One outbox.list row (SPEC_GAPS G69). */
export interface OutboxView {
  id: number;
  account_id: number;
  subject: string;
  to: string[];
  state: OutboxStateName;
  error: { code: ErrorCode; message: string; addresses?: string[] } | null;
  send_after_utc: number;
  created_at: number;
  draft_id: number | null;
}

/** pending_ops kinds, as op-failed reports them. */
export type OpKind =
  | "flags"
  | "move"
  | "expunge"
  | "folder_create"
  | "folder_rename"
  | "folder_delete"
  | "append"
  | "draft_replace"
  | "draft_delete";

const ERROR_CODES: readonly ErrorCode[] = [
  "network",
  "timeout",
  "certificate",
  "tls",
  "auth_failed",
  "oauth",
  "throttled",
  "server_rejected",
  "recipient_rejected",
  "too_large",
  "unsupported",
  "not_found",
  "conflict",
  "invalid_input",
  "keychain",
  "disk_full",
  "db_corrupt",
  "io",
  "cancelled",
  "internal",
];

export function isAppError(value: unknown): value is AppError {
  if (typeof value !== "object" || value === null) return false;
  const v = value as { code?: unknown; message?: unknown };
  return typeof v.code === "string" && (ERROR_CODES as readonly string[]).includes(v.code) && typeof v.message === "string";
}

/** Every failure reaches the UI as an AppError; anything else is an internal error. */
export function toAppError(value: unknown): AppError {
  return isAppError(value) ? value : { code: "internal", message: "" };
}

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(command, args);
  } catch (err) {
    throw toAppError(err);
  }
}

/** The commands built so far, by their section 3 names. */
export const commands = {
  "account.list": () => call<AccountView[]>("account_list"),
  "account.resolve": (email: string) => call<Candidate[]>("account_resolve", { email }),
  "account.test": (config: AccountConfig) => call<TestReport>("account_test", { config }),
  "account.add": (config: AccountConfig) => call<AccountSummary>("account_add", { config }),
  "account.update": (account_id: number, config: AccountConfig) =>
    call<AccountSummary>("account_update", { account_id, config }),
  "account.remove": (account_id: number) => call<null>("account_remove", { account_id }),
  /** Opens Google's consent page in the browser; the result arrives as oauth-done. */
  "account.oauth_start": (account_id: number) => call<null>("account_oauth_start", { account_id }),
  "account.set_enabled": (account_id: number, enabled: boolean) =>
    call<null>("account_set_enabled", { account_id, enabled }),
  "folder.list": (account_id: number) => call<FolderView[]>("folder_list", { account_id }),
  "folder.refresh": (account_id: number, folder_id: number) =>
    call<null>("folder_refresh", { account_id, folder_id }),
  "list.page": (request: ListRequest) => call<Page>("list_page", { request }),
  "message.open": (message_id: number) => call<MessageView>("message_open", { message_id }),
  "message.save_source": (message_id: number) => call<null>("message_save_source", { message_id, path: null }),
  "images.allow_once": (message_id: number) => call<null>("images_allow_once", { message_id }),
  "images.allow_sender": (message_id: number) => call<null>("images_allow_sender", { message_id }),
  "attachment.open": (attachment_id: number) => call<null>("attachment_open", { attachment_id }),
  /** With no path the app shows the native Save dialog (SPEC_GAPS G54). */
  "attachment.save": (attachment_id: number) => call<null>("attachment_save", { attachment_id, path: null }),
  "link.open": (url: string) => call<null>("link_open", { url }),
  "log.open": () => call<null>("log_open"),
  "message.set_flags": (message_ids: number[], flags: FlagChange) =>
    call<number>("message_set_flags", { message_ids, flags }),
  "message.move": (message_ids: number[], folder_id: number) => call<number>("message_move", { message_ids, folder_id }),
  /** Messages, or with folder_id and no messages, everything in the folder (Empty Trash). */
  "message.delete": (message_ids: number[], folder_id: number | null = null) =>
    call<number>("message_delete", { message_ids, folder_id }),
  "message.archive": (message_ids: number[]) => call<number>("message_archive", { message_ids }),
  "message.mark_all_read": (folder_id: number) => call<number>("message_mark_all_read", { folder_id }),
  "message.undo": (op_id: number) => call<null>("message_undo", { op_id }),
  "folder.create": (account_id: number, path: string) => call<null>("folder_create", { account_id, path }),
  "folder.rename": (account_id: number, path: string, value: string) =>
    call<null>("folder_rename", { account_id, path, value }),
  "folder.delete": (account_id: number, path: string) => call<null>("folder_delete", { account_id, path }),
  "folder.set_role": (account_id: number, path: string, value: Role | null) =>
    call<null>("folder_set_role", { account_id, path, value }),
  "folder.set_sync": (account_id: number, path: string, value: boolean) =>
    call<null>("folder_set_sync", { account_id, path, value }),
  "folder.set_watch": (account_id: number, path: string, value: boolean) =>
    call<null>("folder_set_watch", { account_id, path, value }),
  "folder.extend_window": (account_id: number, folder_id: number) =>
    call<null>("folder_extend_window", { account_id, folder_id }),
  /** Opens the compose window (SPEC_GAPS G63). */
  "compose.new": (account_id: number, to: string | null = null) => call<DraftView>("compose_new", { account_id, to }),
  "compose.reply": (message_id: number) => call<DraftView>("compose_reply", { message_id }),
  "compose.reply_all": (message_id: number) => call<DraftView>("compose_reply_all", { message_id }),
  "compose.forward": (message_id: number, as_attachment: boolean) =>
    call<DraftView>("compose_forward", { message_id, as_attachment }),
  "draft.save": (draft_id: number, content: DraftContent) => call<null>("draft_save", { draft_id, content }),
  "draft.discard": (draft_id: number) => call<null>("draft_discard", { draft_id }),
  /** With no path the app shows the native Open dialog. */
  "draft.add_attachment": (draft_id: number, path: string | null = null) =>
    call<DraftContent>("draft_add_attachment", { draft_id, path }),
  "draft.remove_attachment": (draft_id: number, attachment_id: string) =>
    call<DraftContent>("draft_remove_attachment", { draft_id, attachment_id }),
  "draft.send": (draft_id: number, identity_id: number, send_at: number | null = null) =>
    call<number>("draft_send", { draft_id, identity_id, send_at }),
  "outbox.undo": (outbox_id: number) => call<null>("outbox_undo", { outbox_id }),
  "outbox.retry": (outbox_id: number) => call<null>("outbox_retry", { outbox_id }),
  "outbox.discard": (outbox_id: number) => call<null>("outbox_discard", { outbox_id }),
  "outbox.list": (account_id: number) => call<OutboxView[]>("outbox_list", { account_id }),
  "identity.list": (account_id: number) => call<IdentityView[]>("identity_list", { account_id }),
  "identity.save": (identity: IdentityView) => call<IdentityView[]>("identity_save", { identity }),
  "identity.delete": (identity_id: number) => call<IdentityView[]>("identity_delete", { identity_id }),
  "contacts.suggest": (prefix: string, account_id: number) =>
    call<Address[]>("contacts_suggest", { prefix, account_id }),
} as const;

/** Event payloads (section 3, event table). */
export interface EventMap {
  "store-changed": { account_id: number; folder_id: number | null };
  "account-state": { account_id: number; state: AccountState; last_error: AppError | null };
  "sync-progress": { account_id: number; folder_id: number; fetched: number; total: number };
  "folder-reset": { account_id: number; folder_id: number };
  "storage-warning": { kind: "disk_full" | "db_corrupt" };
  "pending-count": { total: number };
  "op-failed": { account_id: number; op_id: number; kind: OpKind; error: AppError };
  "oauth-done": { account_id: number; ok: boolean; error: AppError | null };
  "outbox-state": {
    outbox_id: number;
    account_id: number;
    state: OutboxStateName | "undone" | "discarded";
  };
}

export function on<K extends keyof EventMap>(name: K, handler: (payload: EventMap[K]) => void): Promise<UnlistenFn> {
  return listen<EventMap[K]>(name, (event) => handler(event.payload));
}
