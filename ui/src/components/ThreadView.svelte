<script lang="ts">
  // The thread view (section 10, rule 6): the thread's messages from every folder of the
  // account except Trash, Junk and Drafts (unless one of them is open), oldest first.
  // Unread messages and the latest are expanded; the rest are one line each, and a click
  // expands them. Each expanded message is a reading card.
  import ReadingPane from "./ReadingPane.svelte";
  import { personName, rowDate, subjectText } from "../lib/format";
  import { commands, type Row } from "../lib/ipc";
  import { initiallyExpanded } from "../lib/threads";
  import type { ThreadView } from "../state/app.svelte";
  import { app } from "../state/app.svelte";
  import { strings } from "../strings";

  let { thread }: { thread: ThreadView } = $props();

  let rows = $state<Row[]>([]);
  let expanded = $state<Set<number>>(new Set());
  let loadedFor = "";
  let now = $state(new Date());

  async function load(t: ThreadView, fresh: boolean): Promise<void> {
    const key = `${t.thread_id}:${t.folder_id}`;
    const page = await commands["list.page"]({
      scope: { kind: "thread", thread_id: t.thread_id, folder_id: t.folder_id },
      cursor: null,
      limit: 500,
      group_by_thread: false,
    });
    if (`${thread.thread_id}:${thread.folder_id}` !== key) return;
    const known = new Set(rows.map((r) => r.id));
    const first = initiallyExpanded(page.rows);
    const next = new Set(fresh ? first : expanded);
    // New messages in an open thread follow the same rule.
    for (const r of page.rows) if (!known.has(r.id) && first.has(r.id)) next.add(r.id);
    rows = page.rows;
    expanded = next;
    now = new Date();
  }

  // A new thread.
  $effect(() => {
    const t = thread;
    const key = `${t.thread_id}:${t.folder_id}`;
    if (key === loadedFor) return;
    loadedFor = key;
    rows = [];
    expanded = new Set();
    void load(t, true);
  });

  // Any store change can add or remove one of its messages (across folders).
  $effect(() => {
    if (app.changeTotal === 0) return;
    void load(thread, false);
  });

  function expand(id: number): void {
    expanded = new Set([...expanded, id]);
  }
</script>

<section class="thread" aria-label={strings.threadLabel}>
  {#if rows.length > 0}
    <h2 class="subject">{subjectText(rows[rows.length - 1]?.subject ?? "")}</h2>
  {/if}
  {#each rows as r (r.id)}
    {#if expanded.has(r.id)}
      <ReadingPane messageId={r.id} embedded={true} />
    {:else}
      <button class="collapsed tint press" class:unread={r.unread} aria-label={strings.expandMessage} onclick={() => expand(r.id)}>
        <span class="who">{personName(r.from)}</span>
        <span class="preview">{r.preview}</span>
        <span class="date">{rowDate(r.date_utc, now)}</span>
      </button>
    {/if}
  {/each}
</section>

<style>
  .thread {
    min-height: 0;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: var(--space-10);
  }

  .subject {
    margin: 0;
    font-size: var(--text-subject);
    line-height: var(--leading-subject);
    font-weight: var(--font-weight-semibold);
    letter-spacing: var(--tracking-title);
  }

  .collapsed {
    --tint: var(--color-row-hover);
    display: grid;
    grid-template-columns: var(--sender-width) minmax(0, 1fr) var(--date-width);
    align-items: center;
    gap: var(--space-12);
    height: var(--row-height);
    padding: 0 var(--space-16);
    background: var(--color-card);
    border: 1px solid var(--color-border-on-glass);
    border-radius: var(--radius-card);
    text-align: left;
    color: var(--color-text-body);
  }

  .collapsed.unread .who {
    font-weight: var(--font-weight-semibold);
  }

  .who,
  .preview {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .preview,
  .date {
    font-size: var(--text-meta);
    color: var(--color-text-muted);
  }

  .date {
    text-align: right;
  }
</style>
