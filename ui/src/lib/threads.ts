// The thread view (section 10, rule 6): oldest first; unread messages and the latest are
// expanded, the rest collapsed to one line.

import type { Row } from "./ipc";

/** The messages shown expanded at first. */
export function initiallyExpanded(rows: readonly Row[]): Set<number> {
  const out = new Set<number>();
  for (const r of rows) if (r.unread) out.add(r.id);
  const latest = rows[rows.length - 1];
  if (latest) out.add(latest.id);
  return out;
}

/** Up to 3 participants, for a thread row (rule 5). */
export function participantsText(names: readonly string[]): string {
  return names.slice(0, 3).join(", ");
}
