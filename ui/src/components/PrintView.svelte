<script lang="ts">
  // The print view (section 18.8): a header block (from, to, cc, date, subject) above each
  // sanitized body, a thread expanded and oldest first. Bodies come from mail-body in the
  // same sandboxed iframe as the reader, with images only where already allowed (T44). Laid
  // out off screen; once every body has its height, the app opens the print dialog. On
  // paper only this view shows.
  import { onMount } from "svelte";
  import { addressList, personName, readerDateText, subjectText } from "../lib/format";
  import { commands, type MessageView } from "../lib/ipc";
  import { printBodyUrl, printOrder } from "../lib/m10";
  import { platformUrl, usesHttpScheme } from "../lib/protocol";
  import { app, type PrintJob } from "../state/app.svelte";
  import { strings } from "../strings";

  let { job }: { job: PrintJob } = $props();

  /** A body still downloading is waited for this long. */
  const BODY_WAIT_MS = 8000;
  /** The print dialog opens after this long even if a body never reports its height. */
  const LAYOUT_WAIT_MS = 5000;

  let views = $state<MessageView[]>([]);
  let heights = $state<Record<number, number>>({});
  let frames: (HTMLIFrameElement | undefined)[] = $state([]);
  const http = typeof navigator !== "undefined" && usesHttpScheme(navigator.userAgent);

  async function openAll(ids: number[]): Promise<MessageView[]> {
    const start = Date.now();
    for (;;) {
      const all = await Promise.all(ids.map((id) => commands["message.open"](id)));
      if (!all.some((v) => v.body_kind === "loading") || Date.now() - start > BODY_WAIT_MS) return all;
      await new Promise((r) => setTimeout(r, 250));
    }
  }

  async function ids(): Promise<number[]> {
    if ("message_id" in job) return [job.message_id];
    const page = await commands["list.page"]({
      scope: { kind: "thread", thread_id: job.thread_id, folder_id: app.thread?.folder_id ?? null },
      cursor: null,
      limit: 500,
      group_by_thread: false,
    });
    return page.rows.map((r) => r.id);
  }

  async function run(): Promise<void> {
    const ok = await app.act(async () => {
      views = printOrder(await openAll(await ids()));
    });
    if (!ok) {
      app.printing = null;
      return;
    }
    const start = Date.now();
    while (Object.keys(heights).length < views.length && Date.now() - start < LAYOUT_WAIT_MS) {
      await new Promise((r) => setTimeout(r, 100));
    }
    await app.act(() => commands["message.print"](job));
  }

  onMount(() => {
    const onMessage = (e: MessageEvent) => {
      const i = frames.findIndex((f) => f !== undefined && e.source === f.contentWindow);
      const data = e.data as { zeromail?: unknown; height?: unknown } | null;
      const view = views[i];
      if (view && data?.zeromail === "height" && typeof data.height === "number" && Number.isFinite(data.height)) {
        heights[view.id] = Math.max(0, Math.ceil(data.height));
      }
    };
    const done = () => (app.printing = null);
    window.addEventListener("message", onMessage);
    window.addEventListener("afterprint", done);
    void run();
    return () => {
      window.removeEventListener("message", onMessage);
      window.removeEventListener("afterprint", done);
    };
  });
</script>

<div class="print-view" aria-label={strings.printLabel}>
  {#each views as v, i (v.id)}
    <article class="message">
      <h1 class="subject">{subjectText(v.subject)}</h1>
      <dl class="headers">
        <dt>{strings.from}</dt>
        <dd>{personName(v.from)}{v.from.addr && v.from.addr !== personName(v.from) ? ` <${v.from.addr}>` : ""}</dd>
        {#if v.to.length > 0}<dt>{strings.to}</dt><dd>{addressList(v.to)}</dd>{/if}
        {#if v.cc.length > 0}<dt>{strings.cc}</dt><dd>{addressList(v.cc)}</dd>{/if}
        <dt>{strings.date}</dt>
        <dd>{readerDateText(v.date_utc)}</dd>
      </dl>
      <iframe
        bind:this={frames[i]}
        title={strings.bodyFrameTitle}
        sandbox="allow-scripts"
        referrerpolicy="no-referrer"
        src={platformUrl(printBodyUrl(v), http)}
        style:height={`${heights[v.id] ?? 0}px`}
      ></iframe>
    </article>
  {/each}
</div>

<style>
  /* Off screen but laid out, so each body reports its height. */
  .print-view {
    position: fixed;
    top: 0;
    left: calc(-2 * var(--print-width));
    width: var(--print-width);
    background: var(--color-message-canvas);
    color: var(--color-ink);
  }

  .message {
    display: flex;
    flex-direction: column;
    gap: var(--space-8);
    padding-bottom: var(--space-20);
    break-inside: auto;
  }

  .message + .message {
    border-top: 1px solid var(--color-border);
    padding-top: var(--space-20);
  }

  .subject {
    margin: 0;
    font-size: var(--text-subject);
    font-weight: var(--font-weight-semibold);
  }

  .headers {
    display: grid;
    grid-template-columns: max-content minmax(0, 1fr);
    gap: var(--space-4) var(--space-12);
    margin: 0;
    font-size: var(--text-meta);
  }

  dt {
    color: var(--color-text-muted);
  }

  dd {
    margin: 0;
    overflow-wrap: anywhere;
  }

  iframe {
    display: block;
    width: 100%;
    border: 0;
    background: var(--color-message-canvas);
  }

  @media print {
    .print-view {
      position: static;
      width: auto;
    }
  }
</style>
