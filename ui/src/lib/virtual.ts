// Layout maths for the virtualized message list (section 11; SPEC_GAPS G9). One list in one
// card: a "Today" header, today's rows, an "Earlier" header, then the rest. Rows have one
// fixed height and only the visible window (plus overscan) exists in the DOM, at most 100
// rows (section 14).

export const MAX_DOM_ROWS = 100;

export interface Layout {
  rowHeight: number;
  headerHeight: number;
  /** Rows in the folder. */
  total: number;
  /** Rows dated today, from the top. `null` while the boundary is not loaded yet. */
  todayCount: number | null;
}

/** Index of the first non-today row: total when every row is today, Infinity if unknown. */
function boundary(l: Layout): number {
  return l.todayCount === null ? Number.POSITIVE_INFINITY : l.todayCount;
}

export function hasTodayHeader(l: Layout): boolean {
  return l.total > 0 && (l.todayCount === null || l.todayCount > 0);
}

export function hasEarlierHeader(l: Layout): boolean {
  return l.total > 0 && l.todayCount !== null && l.todayCount < l.total;
}

/** Top of row `i` in list coordinates. */
export function rowTop(i: number, l: Layout): number {
  let top = i * l.rowHeight;
  if (hasTodayHeader(l)) top += l.headerHeight;
  if (hasEarlierHeader(l) && i >= boundary(l)) top += l.headerHeight;
  return top;
}

export function totalHeight(l: Layout): number {
  if (l.total === 0) return 0;
  const headers = (hasTodayHeader(l) ? 1 : 0) + (hasEarlierHeader(l) ? 1 : 0);
  return l.total * l.rowHeight + headers * l.headerHeight;
}

/** Where the "Earlier" header sits, if it is shown. */
export function earlierHeaderTop(l: Layout): number | null {
  if (!hasEarlierHeader(l)) return null;
  return (hasTodayHeader(l) ? l.headerHeight : 0) + boundary(l) * l.rowHeight;
}

/** The row at a list y coordinate (clamped to the list). */
export function rowAt(y: number, l: Layout): number {
  if (l.total === 0) return 0;
  let rest = y - (hasTodayHeader(l) ? l.headerHeight : 0);
  const b = boundary(l);
  if (hasEarlierHeader(l) && rest >= b * l.rowHeight) rest -= l.headerHeight;
  const i = Math.floor(Math.max(0, rest) / l.rowHeight);
  return Math.min(l.total - 1, Math.max(0, i));
}

/** Rows to render for a viewport: first..last inclusive, capped at MAX_DOM_ROWS. */
export function visibleRange(scrollTop: number, viewport: number, l: Layout, overscan: number): { first: number; last: number } {
  if (l.total === 0) return { first: 0, last: -1 };
  const first = Math.max(0, rowAt(scrollTop, l) - overscan);
  let last = Math.min(l.total - 1, rowAt(scrollTop + viewport, l) + overscan);
  if (last - first + 1 > MAX_DOM_ROWS) last = first + MAX_DOM_ROWS - 1;
  return { first, last };
}

/** The scrollTop that brings row `i` fully into view, or null when it already is. */
export function scrollToReveal(i: number, scrollTop: number, viewport: number, l: Layout): number | null {
  const top = rowTop(i, l);
  const bottom = top + l.rowHeight;
  if (top < scrollTop) return top - (i === 0 && hasTodayHeader(l) ? l.headerHeight : 0);
  if (bottom > scrollTop + viewport) return bottom - viewport;
  return null;
}
