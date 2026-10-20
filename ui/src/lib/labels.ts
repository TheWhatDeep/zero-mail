// Gmail labels (section 18.1) and Block sender (section 18.6) in the UI.

import type { FolderView, LabelView, RuleView } from "./ipc";

/** Labels a message can get: the account's folders without a role (Gmail's own system
 * folders have one or are hidden), minus the labels it has. */
export function labelTargets(folders: readonly FolderView[], labels: readonly LabelView[]): FolderView[] {
  const has = new Set(labels.map((l) => l.folder_id));
  return folders.filter((f) => f.role === null && !f.hidden && !has.has(f.id) && !f.path.startsWith("[Gmail]/"));
}

/** Block sender: from this address, move to Trash, stop processing. Unblock deletes it. */
export function blockRule(address: string): RuleView {
  const addr = address.trim();
  return {
    id: 0,
    account_id: null,
    name: `Block ${addr}`,
    enabled: true,
    conditions: { match: "all", items: [{ field: "from", value: addr }] },
    actions: [{ kind: "delete" }],
    stop_processing: true,
  };
}
