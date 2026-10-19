// Reading-pane state. After any store change in the open message's folder the reader asks
// again; a message that no longer exists keeps its body on screen with the deleted notice
// until it is closed (section 12; T04's reading-pane part, SPEC_GAPS G38).

import type { AppError, MessageView } from "./ipc";

export type ReaderState =
  | { kind: "empty" }
  | { kind: "open"; view: MessageView; deleted: boolean }
  | { kind: "error"; error: AppError };

export const emptyReader: ReaderState = { kind: "empty" };

/** The result of opening, or of asking again for, message `id`. */
export function applyOpen(
  state: ReaderState,
  id: number,
  result: { ok: true; view: MessageView } | { ok: false; error: AppError },
): ReaderState {
  const current = state.kind === "open" && state.view.id === id ? state : null;
  if (result.ok) return { kind: "open", view: result.view, deleted: false };
  if (result.error.code === "not_found" && current) return { ...current, deleted: true };
  return { kind: "error", error: result.error };
}

/** Whether a store change concerns what the reader shows. */
export function touches(state: ReaderState, accountId: number, folderId: number | null): boolean {
  if (state.kind !== "open" || state.view.account_id !== accountId) return false;
  return folderId === null || folderId === state.view.folder_id;
}

/** The body frame reloads when a body that was loading has arrived. */
export function bodyArrived(before: ReaderState, after: ReaderState): boolean {
  return (
    before.kind === "open" &&
    after.kind === "open" &&
    before.view.id === after.view.id &&
    before.view.body_kind === "loading" &&
    after.view.body_kind !== "loading"
  );
}
