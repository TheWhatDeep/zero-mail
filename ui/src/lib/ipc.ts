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

/** The password goes to Core once and is never sent back. */
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

export type BodyKind = "plain" | "plain_truncated" | "loading";

export interface Attachment {
  part_id: string;
  filename: string;
  mime: string;
  size: number;
  is_inline: boolean;
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
  body_url: string;
  attachments: Attachment[];
  remote_images: "none" | "blocked" | "allowed";
}

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
  "account.set_enabled": (account_id: number, enabled: boolean) =>
    call<null>("account_set_enabled", { account_id, enabled }),
  "folder.list": (account_id: number) => call<FolderView[]>("folder_list", { account_id }),
  "folder.refresh": (account_id: number, folder_id: number) =>
    call<null>("folder_refresh", { account_id, folder_id }),
  "list.page": (request: ListRequest) => call<Page>("list_page", { request }),
  "message.open": (message_id: number) => call<MessageView>("message_open", { message_id }),
} as const;

/** Event payloads (section 3, event table). */
export interface EventMap {
  "store-changed": { account_id: number; folder_id: number | null };
  "account-state": { account_id: number; state: AccountState; last_error: AppError | null };
  "sync-progress": { account_id: number; folder_id: number; fetched: number; total: number };
  "folder-reset": { account_id: number; folder_id: number };
  "storage-warning": { kind: "disk_full" | "db_corrupt" };
}

export function on<K extends keyof EventMap>(name: K, handler: (payload: EventMap[K]) => void): Promise<UnlistenFn> {
  return listen<EventMap[K]>(name, (event) => handler(event.payload));
}
