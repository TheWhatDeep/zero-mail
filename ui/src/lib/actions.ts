// Mailbox actions in the UI (section 9). Core applies each locally at once and queues it;
// the UI only decides what to offer and what to say.

import type { FolderView, MessageView } from "./ipc";

/** A message shown this long is marked read (section 8, Read state; SPEC_GAPS G62). */
export const READ_DELAY_MS = 1500;

/** Archive, delete and move show the 10 s Undo toast (section 9). */
export const UNDOABLE = new Set(["archive", "delete", "move"]);

/** Folders a message can move to: the account's others, hidden ones left out. */
export function moveTargets(folders: FolderView[], currentId: number): FolderView[] {
  return folders.filter((f) => f.id !== currentId && !f.hidden);
}

/** The sidebar lists every folder but Gmail's hidden ones (SPEC_GAPS G61). */
export function visibleFolders(folders: FolderView[]): FolderView[] {
  return folders.filter((f) => !f.hidden);
}

/** Empty is offered in Trash and Junk (section 9, Empty Trash). */
export function canEmpty(folder: FolderView): boolean {
  return (folder.role === "trash" || folder.role === "junk") && folder.total > 0;
}

/** Delete asks first only when it is permanent (section 9). */
export function deleteNeedsConfirm(view: Pick<MessageView, "delete_is_permanent">): boolean {
  return view.delete_is_permanent;
}

/** Special-use folders cannot be deleted, and INBOX cannot be renamed (section 9). */
export function folderEditable(folder: FolderView): { rename: boolean; remove: boolean } {
  return { rename: folder.role !== "inbox", remove: folder.role === null };
}

