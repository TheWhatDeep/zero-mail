// Mailbox action rules in the UI (section 9; SPEC_GAPS G61, G62).

import { describe, expect, it } from "vitest";
import { READ_DELAY_MS, canEmpty, deleteNeedsConfirm, folderEditable, moveTargets, visibleFolders } from "./actions";
import type { FolderView, Role } from "./ipc";

function folder(id: number, path: string, role: Role | null = null, extra: Partial<FolderView> = {}): FolderView {
  return {
    id,
    account_id: 1,
    path,
    name: path,
    depth: 0,
    role,
    unread: 0,
    total: 3,
    sync_enabled: true,
    watch_idle: false,
    synced: true,
    hidden: false,
    ...extra,
  };
}

describe("mailbox actions", () => {
  const folders = [
    folder(1, "INBOX", "inbox"),
    folder(2, "Trash", "trash"),
    folder(3, "Work"),
    folder(4, "[Gmail]/Starred", null, { hidden: true, sync_enabled: false }),
  ];

  it("offers every other visible folder as a move target", () => {
    expect(moveTargets(folders, 1).map((f) => f.id)).toEqual([2, 3]);
    expect(visibleFolders(folders).map((f) => f.id)).toEqual([1, 2, 3]);
  });

  it("asks before a permanent delete and offers Empty in Trash and Junk", () => {
    expect(deleteNeedsConfirm({ delete_is_permanent: true })).toBe(true);
    expect(deleteNeedsConfirm({ delete_is_permanent: false })).toBe(false);
    expect(canEmpty(folders[1] as FolderView)).toBe(true);
    expect(canEmpty(folder(5, "Junk", "junk", { total: 0 }))).toBe(false);
    expect(canEmpty(folders[2] as FolderView)).toBe(false);
  });

  it("keeps special folders from being deleted and INBOX from being renamed", () => {
    expect(folderEditable(folders[0] as FolderView)).toEqual({ rename: false, remove: false });
    expect(folderEditable(folders[1] as FolderView)).toEqual({ rename: true, remove: false });
    expect(folderEditable(folders[2] as FolderView)).toEqual({ rename: true, remove: true });
  });

  it("marks read after 1.5 s", () => {
    expect(READ_DELAY_MS).toBe(1500);
  });
});
