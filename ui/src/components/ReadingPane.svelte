<script lang="ts">
  // The reading pane (section 11, right; design 3a reading card). The body is plain text
  // from mail-body:// in <iframe sandbox="allow-scripts"> without allow-same-origin
  // (section 8, step 4; SPEC_GAPS G42). The only script in it is Core's, with a nonce; it
  // posts the content height, which sizes the frame. A message deleted on the server keeps
  // its body with a notice until it is closed (section 12; T04).
  import { onMount } from "svelte";
  import Avatar from "./Avatar.svelte";
  import Icon from "./Icon.svelte";
  import { addressList, personName, readerDateText, subjectText } from "../lib/format";
  import { commands, toAppError } from "../lib/ipc";
  import { platformUrl, usesHttpScheme } from "../lib/protocol";
  import { applyOpen, bodyArrived, emptyReader, type ReaderState } from "../lib/reader";
  import { app } from "../state/app.svelte";
  import { errorText, strings } from "../strings";

  let reader = $state<ReaderState>(emptyReader);
  let frame: HTMLIFrameElement | undefined = $state();
  let frameHeight = $state(0);
  /** Reloads the frame once a body that was loading has arrived. */
  let frameKey = $state(0);
  const http = typeof navigator !== "undefined" && usesHttpScheme(navigator.userAgent);

  async function open(id: number): Promise<void> {
    let next: ReaderState;
    try {
      const view = await commands["message.open"](id);
      next = applyOpen(reader, id, { ok: true, view });
    } catch (err) {
      next = applyOpen(reader, id, { ok: false, error: toAppError(err) });
    }
    if (app.message !== id) return;
    if (bodyArrived(reader, next)) frameKey += 1;
    reader = next;
  }

  // A newly selected message.
  $effect(() => {
    const id = app.message;
    frameHeight = 0;
    if (id === null) {
      reader = emptyReader;
      return;
    }
    void open(id);
  });

  // A store change where the open message lives: ask again (section 3, One data path).
  $effect(() => {
    const s = reader;
    if (s.kind !== "open") return;
    const n = app.changeCount(s.view.account_id, s.view.folder_id);
    if (n > 0 && app.message === s.view.id) void open(s.view.id);
  });

  onMount(() => {
    const onMessage = (e: MessageEvent) => {
      if (!frame || e.source !== frame.contentWindow) return;
      const data = e.data as { zeromail?: unknown; height?: unknown } | null;
      if (data?.zeromail === "height" && typeof data.height === "number" && Number.isFinite(data.height)) {
        frameHeight = Math.max(0, Math.ceil(data.height));
      }
    };
    window.addEventListener("message", onMessage);
    return () => window.removeEventListener("message", onMessage);
  });

  const view = $derived(reader.kind === "open" ? reader.view : null);
</script>

<section class="card" aria-label={strings.readerLabel}>
  {#if reader.kind === "empty"}
    <p class="placeholder">{strings.noMessageSelected}</p>
  {:else if reader.kind === "error"}
    <p class="placeholder">{strings.couldNotOpen} {errorText[reader.error.code]}</p>
  {:else if view}
    <div class="body">
      {#if reader.kind === "open" && reader.deleted}
        <p class="notice" role="status"><Icon name="trash" size="--icon-14" /> {strings.deleted}</p>
      {/if}
      <h2 class="subject">{subjectText(view.subject)}</h2>
      <div class="sender">
        <Avatar person={view.from} size={32} />
        <div class="who">
          <div class="from">
            <span class="name">{personName(view.from)}</span>
            {#if view.from.addr && view.from.addr !== personName(view.from)}
              <span class="addr">&lt;{view.from.addr}&gt;</span>
            {/if}
          </div>
          {#if view.to.length > 0}<div class="meta-small">{strings.to} {addressList(view.to)}</div>{/if}
          {#if view.cc.length > 0}<div class="meta-small">{strings.cc} {addressList(view.cc)}</div>{/if}
        </div>
        <time class="meta-small" datetime={new Date(view.date_utc * 1000).toISOString()}>{readerDateText(view.date_utc)}</time>
      </div>
      {#if view.body_kind === "loading"}
        <p class="meta" role="status">{strings.loadingBody}</p>
      {:else}
        {#if view.body_kind === "plain_truncated"}
          <p class="notice" role="status">{strings.truncated}</p>
        {/if}
        <div class="canvas">
          {#key `${view.id}:${frameKey}`}
            <iframe
              bind:this={frame}
              title={strings.bodyFrameTitle}
              sandbox="allow-scripts"
              referrerpolicy="no-referrer"
              src={platformUrl(view.body_url, http)}
              style:height={`${frameHeight}px`}
            ></iframe>
          {/key}
        </div>
      {/if}
    </div>
  {/if}
</section>

<style>
  .card {
    min-height: 0;
    display: flex;
    flex-direction: column;
    background: var(--color-card-reading);
    border: 1px solid var(--color-border-on-glass);
    border-radius: var(--radius-card);
    box-shadow: var(--shadow-control);
    overflow: hidden;
  }

  .placeholder {
    margin: auto;
    color: var(--color-text-muted);
  }






  .body {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: var(--space-14);
    padding: var(--space-18) var(--space-20);
    user-select: text;
  }

  .subject {
    margin: 0;
    font-size: var(--text-subject);
    line-height: var(--leading-subject);
    font-weight: var(--font-weight-semibold);
    letter-spacing: var(--tracking-title);
    text-wrap: pretty;
  }

  .sender {
    display: flex;
    align-items: flex-start;
    gap: var(--space-10);
  }

  .who {
    flex: 1;
    min-width: 0;
  }

  .from {
    display: flex;
    flex-wrap: wrap;
    column-gap: var(--space-6);
  }

  .name {
    font-weight: var(--font-weight-semibold);
  }

  .addr {
    color: var(--color-text-muted);
    overflow-wrap: anywhere;
  }

  .meta {
    margin: 0;
    font-size: var(--text-meta);
    color: var(--color-text-muted);
  }

  .meta-small {
    font-size: var(--text-meta-small);
    color: var(--color-text-muted);
    overflow-wrap: anywhere;
  }

  .notice {
    margin: 0;
    display: flex;
    align-items: center;
    gap: var(--space-8);
    padding: var(--space-8) var(--space-12);
    border-radius: var(--radius-input);
    background: var(--color-surface-sunken);
    border: 1px solid var(--color-divider);
    color: var(--color-text-muted-strong);
  }

  .canvas {
    background: var(--color-message-canvas);
    border-radius: var(--radius-track);
  }

  iframe {
    display: block;
    width: 100%;
    border: 0;
    background: var(--color-message-canvas);
  }
</style>
