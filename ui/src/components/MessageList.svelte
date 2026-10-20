<script lang="ts">
  // The message list (section 11, middle pane; SPEC_GAPS G9). Virtualized: only the visible
  // window plus overscan is in the DOM, at most 100 rows (section 14). Rows load in keyset
  // pages of 100 and pages far from the window are dropped. One card with sticky "Today"
  // and "Earlier" headers. An ARIA listbox: arrow keys, Home and End move the selection.
  // The source is a folder, All Inboxes or a search, with or without Group by conversation
  // (section 10, rule 5); a list the folder counts cannot size grows as its pages load.
  import { onMount, untrack } from "svelte";
  import MessageRow from "./MessageRow.svelte";
  import { canEmpty } from "../lib/actions";
  import { isToday } from "../lib/format";
  import { commands, type Row } from "../lib/ipc";
  import { PageCache } from "../lib/pages";
  import type { ListSource } from "../lib/source";
  import { answerScrollRequests, answerWalkRequests, markInboxReady } from "../lib/perf";
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

  let { source }: { source: ListSource } = $props();
  const folder = $derived(source.folder);

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
  let cacheKey = "";
  /** The cache a new source is waiting to get (the search debounce). */
  let pending: ReturnType<typeof setTimeout> | undefined;
  /** Empty Trash waiting for confirmation (section 9). */
  let confirmEmpty = $state(false);
  /** Search: messages in scope whose body is not indexed (section 10). */
  let unindexed = $state<number | null>(null);
  let searchingServer = $state(false);

  /** Rows in the list: the folder count, or what the loaded pages show. */
  const total = $derived.by(() => {
    void version;
    return source.total ?? cache?.estimatedTotal() ?? 0;
  });
  const empty = $derived.by(() => {
    void version;
    return source.total === 0 || (source.total === null && cache !== null && cache.complete && cache.estimatedTotal() === 0);
  });
  const layout: Layout = $derived({ rowHeight, headerHeight, total, todayCount });
  const range = $derived(visibleRange(scrollTop, viewport, layout, OVERSCAN));
  const indices = $derived(range.last >= range.first ? Array.from({ length: range.last - range.first + 1 }, (_, k) => range.first + k) : []);
  const earlierTop = $derived(earlierHeaderTop(layout));
  const stickyEarlier = $derived(earlierTop !== null && scrollTop >= earlierTop);
  const skeleton = $derived(folder !== null && !source.group && folder.total === 0 && !folder.synced);

  function cssPx(name: string, fallback: number): number {
    const v = Number.parseFloat(getComputedStyle(document.documentElement).getPropertyValue(name));
    return Number.isFinite(v) && v > 0 ? v : fallback;
  }

  function fetchPage(s: ListSource) {
    return async (cursor: Parameters<typeof commands["list.page"]>[0]["cursor"], limit: number) => {
      const page = await commands["list.page"]({ scope: s.scope, cursor, limit, group_by_thread: s.group });
      if (cursor === null && page.unindexed !== null) unindexed = page.unindexed;
      return page;
    };
  }

  /** Search server too (section 10): hits not cached are fetched, then the list reloads. */
  async function searchServer(): Promise<void> {
    const text = source.searchText;
    if (text === null || !cache) return;
    searchingServer = true;
    const c = cache;
    await app.act(() =>
      commands["search.query"]({ text, account_id: source.searchAccount, server_too: true, cursor: null, limit: 100 }),
    );
    searchingServer = false;
    if (c !== cache) return;
    await c.refresh();
    version += 1;
    await load();
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

  // A new source: a fresh cache from the top. A search waits 150 ms after the last
  // keystroke; a newer one makes the older one's answer stale (section 10).
  $effect(() => {
    const s = source;
    if (s.key === cacheKey) return;
    cacheKey = s.key;
    cache = null;
    unindexed = null;
    todayCount = null;
    selectedIndex = null;
    confirmEmpty = false;
    version += 1;
    now = new Date();
    const delay = s.searchText !== null ? 150 : 0;
    clearTimeout(pending);
    pending = setTimeout(() => {
      if (cacheKey !== s.key) return;
      cache = new PageCache(fetchPage(s));
      untrack(() => {
        if (scroller) scroller.scrollTop = 0;
        scrollTop = 0;
        version += 1;
        void load();
      });
    }, delay);
  });

  // Rows needed by the window.
  $effect(() => {
    void range.first;
    void range.last;
    void load();
  });

  // A store change in this folder (any folder, for All Inboxes and search): reload kept
  // pages (section 3, One data path).
  $effect(() => {
    const f = folder;
    const n = f && !source.group ? app.changeCount(f.account_id, f.id) : app.changeTotal;
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
    app.selectRow(row);
  }

  async function move(to: number): Promise<void> {
    if (!cache || total === 0 || !scroller) return;
    const i = Math.max(0, Math.min(total - 1, to));
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
      void move(total - 1);
    }
  }

  onMount(() => {
    rowHeight = cssPx("--row-height", 50);
    headerHeight = cssPx("--list-header-height", 41);
    answerScrollRequests(() => scroller ?? null);
    answerWalkRequests(
      async (i) => {
        await move(i);
        return cache?.row(i)?.subject ?? null;
      },
      () => total,
    );
    if (!scroller) return;
    const el = scroller;
    const resize = new ResizeObserver(() => (viewport = el.clientHeight));
    resize.observe(el);
    viewport = el.clientHeight;
    return () => {
      resize.disconnect();
      clearTimeout(pending);
    };
  });
</script>

<section class="card" aria-label={strings.listLabel}>
  <div class="bar">
    {#if source.searchText !== null}
      <span class="meta">
        {unindexed !== null && unindexed > 0 ? strings.notIndexed(unindexed) : ""}
      </span>
      <button class="small tint press" disabled={searchingServer} onclick={searchServer}>
        {searchingServer ? strings.searchingServer : strings.searchServerToo}
      </button>
    {:else}
      <label class="toggle">
        <input type="checkbox" bind:checked={app.grouped} />
        <span>{strings.groupByConversation}</span>
      </label>
    {/if}
    <span class="grow"></span>
    {#if folder}
      {#if confirmEmpty}
        <span class="confirm" role="alertdialog" aria-label={strings.emptyFolder(folder.name)}>
          <span class="text">{strings.emptyConfirm(folder.total, folder.name)}</span>
          <button
            class="small tint press"
            onclick={() => {
              confirmEmpty = false;
              void app.act(() => commands["message.delete"]([], folder.id));
            }}>{strings.deleteForever}</button
          >
          <button class="small tint press" onclick={() => (confirmEmpty = false)}>{strings.cancel}</button>
        </span>
      {:else}
        {#if folder.unread > 0}
          <button class="small tint press" onclick={() => app.act(() => commands["message.mark_all_read"](folder.id))}>
            {strings.markAllRead}
          </button>
        {/if}
        {#if canEmpty(folder)}
          <button class="small tint press" onclick={() => (confirmEmpty = true)}>{strings.emptyFolder(folder.name)}</button>
        {/if}
      {/if}
    {/if}
  </div>
  <div
    class="scroller"
    bind:this={scroller}
    onscroll={() => (scrollTop = scroller?.scrollTop ?? 0)}
    role="listbox"
    tabindex="0"
    aria-label={folder?.name ?? strings.listLabel}
    aria-activedescendant={app.message !== null && selectedIndex !== null ? `row-${app.message}` : undefined}
    {onkeydown}
  >
    {#if skeleton}
      <div class="skeleton" aria-label={strings.loadingRows}>
        {#each Array.from({ length: 8 }, (_, k) => k) as k (k)}
          <div class="skeleton-row" aria-hidden="true"><span></span><span></span><span></span></div>
        {/each}
      </div>
    {:else if empty}
      <p class="empty">{source.searchText !== null ? strings.noResults : strings.noMessages}</p>
    {:else}
      <div class="spacer" style:height={`${totalHeight(layout)}px`}>
        {#if hasTodayHeader(layout) || hasEarlierHeader(layout)}
          <div class="header sticky" aria-hidden="true">
            {#if stickyEarlier || !hasTodayHeader(layout)}
              <span class="title">{strings.earlier}</span>
              {#if todayCount !== null && source.total !== null}<span class="count">{strings.messages(total - todayCount)}</span>{/if}
            {:else}
              <span class="title">{strings.today}</span>
              {#if todayCount !== null && source.total !== null}<span class="count">{strings.messages(todayCount)}</span>{/if}
            {/if}
          </div>
        {/if}
        {#if earlierTop !== null && hasTodayHeader(layout)}
          <div class="header inline" style:transform={`translateY(${earlierTop}px)`} aria-hidden="true">
            <span class="title">{strings.earlier}</span>
            {#if todayCount !== null && source.total !== null}<span class="count">{strings.messages(total - todayCount)}</span>{/if}
          </div>
        {/if}
        {#each indices as i (i)}
          {@const r = rowAt(i)}
          {#if r}
            <MessageRow
              row={r}
              role={folder?.role ?? null}
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
    {#if folder && folder.synced && source.searchText === null}
      <div class="more">
        <button class="small tint press" onclick={() => app.act(() => commands["folder.extend_window"](folder.account_id, folder.id))}>
          {strings.loadOlder}
        </button>
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

  .bar {
    display: flex;
    justify-content: flex-end;
    align-items: center;
    gap: var(--space-6);
    padding: var(--space-6) var(--space-8);
    border-bottom: 1px solid var(--color-divider);
  }

  .grow {
    flex: 1;
  }

  .meta {
    font-size: var(--text-meta);
    color: var(--color-text-muted);
  }

  .toggle {
    display: flex;
    align-items: center;
    gap: var(--space-6);
    font-size: var(--text-meta);
    color: var(--color-text-muted-strong);
  }

  .confirm {
    display: flex;
    align-items: center;
    gap: var(--space-6);
    min-width: 0;
  }

  .confirm .text {
    font-size: var(--text-meta);
    color: var(--color-text-muted-strong);
  }

  .small {
    --tint: var(--color-button-hover);
    height: var(--button-height-small);
    padding: 0 var(--space-10);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-small-button);
    background: var(--color-secondary-bg);
    color: var(--color-secondary-fg);
    font-size: var(--text-meta);
    font-weight: var(--font-weight-medium);
    white-space: nowrap;
  }

  .more {
    display: flex;
    justify-content: center;
    padding: var(--space-12);
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
