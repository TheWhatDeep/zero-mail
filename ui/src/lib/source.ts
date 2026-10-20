// What the message list shows (section 11, middle pane): a folder, All Inboxes, or a
// search, each with or without Group by conversation (section 10, rule 5).

import type { FolderView, Scope } from "./ipc";

export interface ListSource {
  /** A new key starts a fresh list from the top. */
  key: string;
  scope: Scope;
  group: boolean;
  /** The folder listed, for its bar (Mark all read, Empty, Load older) and its role. */
  folder: FolderView | null;
  /** The row count when the folder counts give it; null: it grows as pages load. */
  total: number | null;
  /** The search text, for the Search server too button. */
  searchText: string | null;
  /** The account a search covers; null: every account. */
  searchAccount: number | null;
}

export interface SourceInput {
  folder: FolderView | null;
  unified: boolean;
  search: { text: string; allAccounts: boolean } | null;
  grouped: boolean;
}

export function listSource(input: SourceInput): ListSource | null {
  const { folder, unified, search, grouped } = input;
  if (search && search.text.trim() !== "") {
    const account = search.allAccounts ? null : (folder?.account_id ?? null);
    return {
      key: `search:${account ?? "all"}:${search.text}`,
      scope: { kind: "search", query: search.text, account_id: account },
      group: false,
      folder: null,
      total: null,
      searchText: search.text,
      searchAccount: account,
    };
  }
  if (unified) {
    return {
      key: `unified:${grouped}`,
      scope: { kind: "unified" },
      group: grouped,
      folder: null,
      total: null,
      searchText: null,
      searchAccount: null,
    };
  }
  if (!folder) return null;
  return {
    key: `folder:${folder.id}:${grouped}`,
    scope: { kind: "folder", folder_id: folder.id },
    group: grouped,
    folder,
    total: grouped ? null : folder.total,
    searchText: null,
    searchAccount: null,
  };
}
