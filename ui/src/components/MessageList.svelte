<script lang="ts">
  // The message list (section 11, middle pane; SPEC_GAPS G9). Virtualized: only the visible
  // window plus overscan is in the DOM, at most 100 rows (section 14). Rows load in keyset
  // pages of 100 and pages far from the window are dropped. One card with sticky "Today"
  // and "Earlier" headers. An ARIA listbox: arrow keys, Home and End move the selection.
  import { onMount, untrack } from "svelte";
  import MessageRow from "./MessageRow.svelte";
  import { isToday } from "../lib/format";
  import { commands, type FolderView, type Row } from "../lib/ipc";
  import { PageCache } from "../lib/pages";
  import { answerScrollRequests, markInboxReady } from "../lib/perf";
  import {
    earlierHeaderTop,
    hasEarlierHeader,
    hasTodayHeader,
    rowTop,
    scrollToReveal,
    totalHeight,
    visibleRange,
    type Layout,
  } from "../lib/virtual";
  import { app } from "../state/app.svelte";
  import { strings } from "../strings";

  let { folder }: { folder: FolderView } = $props();

  const OVERSCAN = 8;

  let scroller: HTMLDivElement | undefined = $state();
  let scrollTop = $state(0);
  let viewport = $state(0);
  let rowHeight = $state(50);
  let headerHeight = $state(41);
  /** Bumped whenever the page cache changes, so the window re-reads it. */
  let version = $state(0);
  let todayCount = $state<number | null>(null);
  let selectedIndex = $state<number | null>(null);
  let now = $state(new Date());
  let cache: PageCache | null = null;
  let cacheFolder = -1;

  const layout: Layout = $derived({ rowHeight, headerHeight, total: folder.total, todayCount });
  const range = $derived(visibleRange(scrollTop, viewport, layout, OVERSCAN));
  const indices = $derived(range.last >= range.first ? Array.from({ length: range.last - range.first + 1 }, (_, k) => range.first + k) : []);
  const earlierTop = $derived(earlierHeaderTop(layout));
  const stickyEarlier = $derived(earlierTop !== null && scrollTop >= earlierTop);
  const skeleton = $derived(folder.total === 0 && !folder.synced);

  function cssPx(name: string, fallback: number): number {
    const v = Number.parseFloat(getComputedStyle(document.documentElement).getPropertyValue(name));
    return Number.isFinite(v) && v > 0 ? v : fallback;
  }

  function fetchPage(folderId: number) {
    return (cursor: Parameters<typeof commands["list.page"]>[0]["cursor"], limit: number) =>
      commands["list.page"]({ scope: { kind: "folder", folder_id: folderId }, cursor, limit, group_by_thread: false });
  }

  /** The Today/Earlier boundary, once the rows around it are loaded. */
  function updateBoundary(): void {
    if (!cache) return;
    const rows = cache.contiguousRows();
    const at = rows.findIndex((r) => !isToday(r.date_utc, now));
    if (at >= 0) todayCount = at;
    else if (cache.complete) todayCount = rows.length;
  }

  async function load(): Promise<void> {
    const c = cache;
    if (!c) return;
    const changed = await c.ensure(range.first, Math.max(range.last, range.first));
    if (c !== cache) return;
    if (changed) {
      updateBoundary();
      version += 1;
    }
    if (c.row(0)) markInboxReady();
  }

  // A new folder: a fresh cache from the top.
  $effect(() => {
    const id = folder.id;
    if (id === cacheFolder) return;
    cacheFolder = id;
    cache = new PageCache(fetchPage(id));
    todayCount = null;
    selectedIndex = null;
    now = new Date();
    untrack(() => {
      if (scroller) scroller.scrollTop = 0;
      scrollTop = 0;
      version += 1;
      void load();
    });
  });

  // Rows needed by the window.
  $effect(() => {
    void range.first;
    void range.last;
    void load();
  });

  // A store change in this folder: reload kept pages (section 3, One data path).
  $effect(() => {
    const n = app.changeCount(folder.account_id, folder.id);
    if (n === 0 || !cache) return;
    const c = cache;
    now = new Date();
    void c.refresh().then(async () => {
      if (c !== cache) return;
      todayCount = null;
      updateBoundary();
      version += 1;
      await load();
    });
  });

  // Keep the selected index in step with the selected message.
  $effect(() => {
    void version;
    const id = app.message;
    if (id === null) {
      selectedIndex = null;
    } else if (cache && selectedIndex !== null && cache.row(selectedIndex)?.id !== id) {
      const found = indices.find((i) => cache?.row(i)?.id === id);
      selectedIndex = found ?? null;
    }
  });

  function rowAt(i: number): Row | undefined {
    void version;
    return cache?.row(i);
  }

  function select(row: Row, index: number): void {
    selectedIndex = index;
    app.message = row.id;
  }

  async function move(to: number): Promise<void> {
    if (!cache || folder.total === 0 || !scroller) return;
    const i = Math.max(0, Math.min(folder.total - 1, to));
    await cache.ensure(i, i);
    version += 1;
    const r = cache.row(i);
    if (!r) return;
    select(r, i);
    const target = scrollToReveal(i, scroller.scrollTop, viewport, layout);
    if (target !== null) scroller.scrollTop = target;
  }

  function onkeydown(e: KeyboardEvent): void {
    const keys: Record<string, number> = { ArrowDown: 1, ArrowUp: -1 };
    if (e.key in keys) {
      e.preventDefault();
      void move((selectedIndex ?? -1) + (keys[e.key] ?? 0));
    } else if (e.key === "Home") {
      e.preventDefault();
      void move(0);
    } else if (e.key === "End") {
      e.preventDefault();
      void move(folder.total - 1);
    }
  }

  onMount(() => {
    rowHeight = cssPx("--row-height", 50);
    headerHeight = cssPx("--list-header-height", 41);
    answerScrollRequests(() => scroller ?? null);
    if (!scroller) return;
    const el = scroller;
    const resize = new ResizeObserver(() => (viewport = el.clientHeight));
    resize.observe(el);
    viewport = el.clientHeight;
    return () => resize.disconnect();
  });
</script>

<section class="card" aria-label={strings.listLabel}>
  <div
    class="scroller"
    bind:this={scroller}
    onscroll={() => (scrollTop = scroller?.scrollTop ?? 0)}
    role="listbox"
    tabindex="0"
    aria-label={folder.name}
    aria-activedescendant={app.message !== null && selectedIndex !== null ? `row-${app.message}` : undefined}
    {onkeydown}
  >
    {#if skeleton}
      <div class="skeleton" aria-label={strings.loadingRows}>
        {#each Array.from({ length: 8 }, (_, k) => k) as k (k)}
          <div class="skeleton-row" aria-hidden="true"><span></span><span></span><span></span></div>
        {/each}
      </div>
    {:else if folder.total === 0}
      <p class="empty">{strings.noMessages}</p>
    {:else}
      <div class="spacer" style:height={`${totalHeight(layout)}px`}>
        {#if hasTodayHeader(layout) || hasEarlierHeader(layout)}
          <div class="header sticky" aria-hidden="true">
            {#if stickyEarlier || !hasTodayHeader(layout)}
              <span class="title">{strings.earlier}</span>
              {#if todayCount !== null}<span class="count">{strings.messages(folder.total - todayCount)}</span>{/if}
            {:else}
              <span class="title">{strings.today}</span>
              {#if todayCount !== null}<span class="count">{strings.messages(todayCount)}</span>{/if}
            {/if}
          </div>
        {/if}
        {#if earlierTop !== null && hasTodayHeader(layout)}
          <div class="header inline" style:transform={`translateY(${earlierTop}px)`} aria-hidden="true">
            <span class="title">{strings.earlier}</span>
            {#if todayCount !== null}<span class="count">{strings.messages(folder.total - todayCount)}</span>{/if}
          </div>
        {/if}
        {#each indices as i (i)}
          {@const r = rowAt(i)}
          {#if r}
            <MessageRow
              row={r}
              role={folder.role}
              selected={app.message === r.id}
              top={rowTop(i, layout)}
              {now}
              onselect={(row) => select(row, i)}
            />
          {:else}
            <div class="skeleton-row placed" style:transform={`translateY(${rowTop(i, layout)}px)`} aria-hidden="true">
              <span></span><span></span><span></span>
            </div>
          {/if}
        {/each}
      </div>
    {/if}
  </div>
</section>

<style>
  .card {
    min-height: 0;
    display: flex;
    flex-direction: column;
    background: var(--color-card);
    border: 1px solid var(--color-border-on-glass);
    border-radius: var(--radius-card);
    box-shadow: var(--shadow-control);
    overflow: hidden;
  }

  .scroller {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    position: relative;
    contain: strict;
  }

  .scroller:focus-visible {
    outline-offset: calc(var(--focus-ring-width) * -1);
  }

  .spacer {
    position: relative;
  }

  .header {
    height: var(--list-header-height);
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: var(--space-12);
    padding: var(--space-12) var(--space-16);
    background: var(--color-card);
    border-bottom: 1px solid var(--color-divider);
  }

  .sticky {
    position: sticky;
    top: 0;
    z-index: 2;
  }

  .inline {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    z-index: 1;
  }

  .title {
    font-weight: var(--font-weight-semibold);
  }

  .count {
    font-size: var(--text-meta);
    color: var(--color-text-muted);
  }

  .empty {
    margin: 0;
    padding: var(--space-26) var(--space-16);
    text-align: center;
    color: var(--color-text-muted);
  }

  .skeleton-row {
    height: var(--row-height);
    display: grid;
    grid-template-columns: var(--avatar-28) var(--sender-width) minmax(0, 1fr);
    align-items: center;
    gap: var(--space-12);
    padding: 0 var(--space-16) 0 var(--space-26);
    border-bottom: 1px solid var(--color-divider-row);
  }

  .placed {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
  }

  .skeleton-row span {
    height: var(--skeleton-line);
    border-radius: var(--radius-pill);
    background: var(--color-skeleton);
  }

  .skeleton-row span:first-child {
    height: var(--avatar-28);
    border-radius: var(--radius-round);
  }
</style>
