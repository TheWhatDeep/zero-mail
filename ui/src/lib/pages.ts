// Keyset paging for the virtualized list (section 14: paginated by keyset on (date, id),
// never OFFSET). Page p starts after the cursor where page p-1 ended, so pages load in
// order. Only pages near the visible window are kept (the frontend holds only the visible
// window, section 14); their start cursors are kept, so an evicted page reloads directly.

import type { Cursor, Page, Row } from "./ipc";

export const PAGE_SIZE = 100;
/** Pages kept on each side of the visible window. */
export const KEEP_AROUND = 2;

export type Fetch = (cursor: Cursor | null, limit: number) => Promise<Page>;

export class PageCache {
  private pages = new Map<number, Row[]>();
  /** Start cursor of page p; page 0 starts at the top (null). */
  private starts = new Map<number, Cursor | null>([[0, null]]);
  private ended: number | null = null;
  /** Rows on the last page, once it has been seen. */
  private endCount: number | null = null;
  private loading = new Set<number>();
  private generation = 0;

  constructor(private readonly fetch: Fetch) {}

  /** The row at index i, if its page is loaded. */
  row(i: number): Row | undefined {
    return this.pages.get(Math.floor(i / PAGE_SIZE))?.[i % PAGE_SIZE];
  }

  /** Rows loaded from the top without a gap. */
  contiguousRows(): Row[] {
    const out: Row[] = [];
    for (let p = 0; this.pages.has(p); p += 1) out.push(...(this.pages.get(p) ?? []));
    return out;
  }

  loadedPages(): number[] {
    return [...this.pages.keys()].sort((a, b) => a - b);
  }

  /** True once the last page has been seen. */
  get complete(): boolean {
    return this.ended !== null && this.pages.has(this.ended);
  }

  /** How many rows the list has, as far as is known: exact once the last page has been
   * seen, else the rows reached so far plus one page, so the list can scroll on. For lists
   * whose length the folder counts cannot give (threads, All Inboxes, search). */
  estimatedTotal(): number {
    if (this.ended !== null && this.endCount !== null) return this.ended * PAGE_SIZE + this.endCount;
    const pages = [...this.pages.keys()];
    const reached = pages.length === 0 ? 0 : Math.max(...pages) * PAGE_SIZE + (this.pages.get(Math.max(...pages))?.length ?? 0);
    return reached + PAGE_SIZE;
  }

  /**
   * Loads every page the rows first..last need, in order, and evicts pages far from them.
   * Resolves true when something changed.
   */
  async ensure(first: number, last: number): Promise<boolean> {
    const from = Math.floor(Math.max(0, first) / PAGE_SIZE);
    const to = Math.floor(Math.max(0, last) / PAGE_SIZE);
    const generation = this.generation;
    let changed = false;
    for (let p = 0; p <= to; p += 1) {
      if (this.ended !== null && p > this.ended) break;
      // Pages before the window are walked only to learn the cursor of the next one.
      if (p < from && this.starts.has(p + 1)) continue;
      if (this.pages.has(p) || this.loading.has(p)) continue;
      const start = this.starts.get(p);
      if (start === undefined) break;
      this.loading.add(p);
      try {
        const page = await this.fetch(start, PAGE_SIZE);
        if (generation !== this.generation) return false;
        this.pages.set(p, page.rows);
        if (page.next) {
          this.starts.set(p + 1, page.next);
        } else {
          this.ended = p;
          this.endCount = page.rows.length;
        }
        changed = true;
      } finally {
        this.loading.delete(p);
      }
    }
    for (const p of [...this.pages.keys()]) {
      if (p < from - KEEP_AROUND || p > to + KEEP_AROUND) {
        this.pages.delete(p);
        changed = true;
      }
    }
    return changed;
  }

  /**
   * The store changed. Kept pages are reloaded from their start cursors. If page 0 now ends
   * at a different cursor (mail arrived at the top), later pages would leave a gap, so
   * they are dropped and reload in order as they are needed.
   */
  async refresh(): Promise<void> {
    this.generation += 1;
    const generation = this.generation;
    const kept = this.loadedPages();
    const first = await this.fetch(null, PAGE_SIZE);
    if (generation !== this.generation) return;
    const before = this.starts.get(1) ?? null;
    const after = first.next;
    const same =
      before === null ? after === null : after !== null && before.date_utc === after.date_utc && before.id === after.id;
    this.pages.set(0, first.rows);
    if (!same) {
      for (const p of [...this.pages.keys()]) if (p > 0) this.pages.delete(p);
      this.starts = new Map<number, Cursor | null>([[0, null]]);
    }
    if (after) {
      this.starts.set(1, after);
      if (!same) {
        this.ended = null;
        this.endCount = null;
      }
    } else {
      this.ended = 0;
      this.endCount = first.rows.length;
    }
    if (!same) return;
    for (const p of kept) {
      if (p === 0) continue;
      const start = this.starts.get(p);
      if (start === undefined) continue;
      const page = await this.fetch(start, PAGE_SIZE);
      if (generation !== this.generation) return;
      this.pages.set(p, page.rows);
      if (page.next) this.starts.set(p + 1, page.next);
      else {
        this.ended = p;
        this.endCount = page.rows.length;
      }
    }
  }
}
