// App state (Svelte 5 runes). It holds what the shell shows: accounts with their live
// state, each account's folder list, the selected folder and message, and banners. Message
// rows are not here: the list keeps only its visible window (section 14).

import { commands, on, toAppError, type AccountView, type AppError, type FolderView } from "../lib/ipc";
import { errorText, opKindText, strings } from "../strings";

export type FormMode = { mode: "add" } | { mode: "edit"; account: AccountView };

export interface Notice {
  id: number;
  text: string;
  /** The op the Undo button reverses (section 9: archive, delete and move). */
  undo?: number;
}

/** A folder reset toast lasts as long as the spec's other toasts (section 9: 10 s). */
export const TOAST_MS = 10_000;
/** Backoff longer than this shows a status bar warning (section 11, States). */
export const BACKOFF_WARNING_MS = 60_000;

class AppState {
  loaded = $state(false);
  accounts = $state<AccountView[]>([]);
  folders = $state<Record<number, FolderView[]>>({});
  selected = $state<{ account_id: number; folder_id: number } | null>(null);
  message = $state<number | null>(null);
  form = $state<FormMode | null>(null);
  online = $state(typeof navigator === "undefined" ? true : navigator.onLine);
  storage = $state<"disk_full" | "db_corrupt" | null>(null);
  toasts = $state<Notice[]>([]);
  /** When each account entered Backoff, for the one-minute warning. */
  backoffSince = $state<Record<number, number>>({});
  /** Bumped on store-changed per folder; the list and reader re-query. */
  changes = $state<Record<string, number>>({});
  startError = $state<AppError | null>(null);
  /** The real target of the link under the pointer in a message (section 11, status bar). */
  hoveredLink = $state<string | null>(null);
  /** Unfinished ops and outbox items (pending-count). */
  pending = $state(0);

  private unlisten: (() => void)[] = [];
  private nextToast = 1;

  folderOf(accountId: number, folderId: number): FolderView | undefined {
    return this.folders[accountId]?.find((f) => f.id === folderId);
  }

  get selectedFolder(): FolderView | undefined {
    const s = this.selected;
    return s ? this.folderOf(s.account_id, s.folder_id) : undefined;
  }

  changeCount(accountId: number, folderId: number | null): number {
    return this.changes[`${accountId}:${folderId ?? "*"}`] ?? 0;
  }

  async start(): Promise<void> {
    const offs = await Promise.all([
      on("store-changed", (p) => this.onStoreChanged(p.account_id, p.folder_id)),
      on("account-state", (p) => this.onAccountState(p.account_id, p.state, p.last_error)),
      on("folder-reset", (p) => {
        const name = this.folderOf(p.account_id, p.folder_id)?.name ?? "";
        this.toast(strings.folderReset(name));
      }),
      on("storage-warning", (p) => {
        this.storage = p.kind;
      }),
      on("pending-count", (p) => {
        this.pending = p.total;
      }),
      on("op-failed", (p) => {
        this.toast(strings.opFailed(opKindText[p.kind], errorText[p.error.code]));
      }),
    ]);
    this.unlisten = offs;
    if (typeof window !== "undefined") {
      const update = () => (this.online = navigator.onLine);
      window.addEventListener("online", update);
      window.addEventListener("offline", update);
      this.unlisten.push(() => {
        window.removeEventListener("online", update);
        window.removeEventListener("offline", update);
      });
    }
    try {
      await this.reloadAccounts();
    } catch (err) {
      this.startError = toAppError(err);
    }
    this.loaded = true;
  }

  stop(): void {
    for (const off of this.unlisten) off();
    this.unlisten = [];
  }

  async reloadAccounts(): Promise<void> {
    const accounts = await commands["account.list"]();
    this.accounts = accounts;
    for (const a of accounts) {
      if (a.state === "backoff") this.backoffSince[a.id] ??= Date.now();
      await this.reloadFolders(a.id);
    }
    if (!this.selected || !accounts.some((a) => a.id === this.selected?.account_id)) {
      this.selectFirstInbox();
    }
  }

  async reloadFolders(accountId: number): Promise<void> {
    try {
      this.folders[accountId] = await commands["folder.list"](accountId);
    } catch {
      this.folders[accountId] = [];
    }
    if (!this.selected) this.selectFirstInbox();
  }

  selectFirstInbox(): void {
    for (const a of this.accounts) {
      const inbox = this.folders[a.id]?.find((f) => f.role === "inbox") ?? this.folders[a.id]?.[0];
      if (inbox) {
        this.selectFolder(a.id, inbox.id);
        return;
      }
    }
    this.selected = null;
  }

  selectFolder(accountId: number, folderId: number): void {
    if (this.selected?.account_id === accountId && this.selected.folder_id === folderId) return;
    this.selected = { account_id: accountId, folder_id: folderId };
    this.message = null;
  }

  private onStoreChanged(accountId: number, folderId: number | null): void {
    const key = `${accountId}:${folderId ?? "*"}`;
    this.changes[key] = (this.changes[key] ?? 0) + 1;
    // Counts and the folder list follow every change.
    void this.reloadFolders(accountId);
  }

  private onAccountState(accountId: number, state: AccountView["state"], error: AppError | null): void {
    const a = this.accounts.find((x) => x.id === accountId);
    if (!a) {
      void this.reloadAccounts();
      return;
    }
    a.state = state;
    a.last_error = error;
    if (state === "backoff") {
      this.backoffSince[accountId] ??= Date.now();
    } else {
      delete this.backoffSince[accountId];
    }
    if (state === "idle" || state === "syncing") void this.reloadFolders(accountId);
  }

  toast(text: string, undo?: number): void {
    const id = this.nextToast++;
    this.toasts.push(undo === undefined ? { id, text } : { id, text, undo });
    setTimeout(() => this.dismissToast(id), TOAST_MS);
  }

  /** Undo from a toast: the op is cancelled or reversed (section 9). */
  async undo(toastId: number, opId: number): Promise<void> {
    this.dismissToast(toastId);
    try {
      await commands["message.undo"](opId);
    } catch (err) {
      this.toast(errorText[toAppError(err).code]);
    }
  }

  /** Runs an action; a failure becomes a toast with the error in words. */
  async act(action: () => Promise<unknown>): Promise<boolean> {
    try {
      await action();
      return true;
    } catch (err) {
      const e = toAppError(err);
      if (e.code !== "cancelled") this.toast(errorText[e.code]);
      return false;
    }
  }

  dismissToast(id: number): void {
    this.toasts = this.toasts.filter((t) => t.id !== id);
  }
}

export const app = new AppState();
