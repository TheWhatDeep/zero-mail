<script lang="ts">
  // The reading pane (section 11, right; design 3a reading card). The body comes from
  // mail-body:// in <iframe sandbox="allow-scripts"> without allow-same-origin (section 8,
  // step 4). The only script in it is Core's, with a nonce: it posts the content height,
  // link clicks and the hovered link. Links open only through decideLink (T21); remote
  // images load only after Load images (T20); attachments are fetched on Open or Save (T23).
  // A message deleted on the server keeps its body with a notice until closed (T04).
  // M9: Gmail label chips (18.1), Snooze (18.2), Mute (18.5) and Block sender (18.6). In a
  // thread view each expanded message is one of these cards (`messageId`, `embedded`).
  // M10: Unsubscribe after a confirmation (18.7), Print (18.8), image Preview (18.9), the
  // Reading settings, and the card's shortcut actions (section 11 table).
  import { onMount, untrack } from "svelte";
  import Avatar from "./Avatar.svelte";
  import Icon from "./Icon.svelte";
  import { addressList, formatSize, personName, readerDateText, subjectText } from "../lib/format";
  import { READ_DELAY_MS, deleteNeedsConfirm, moveTargets } from "../lib/actions";
  import { commands, toAppError, type AppError, type Attachment } from "../lib/ipc";
  import { mailtoAddress } from "../lib/compose";
  import { bodyUrl, decideLink, type LinkAction } from "../lib/links";
  import { platformUrl, usesHttpScheme } from "../lib/protocol";
  import { applyOpen, bodyArrived, emptyReader, type ReaderState } from "../lib/reader";
  import { canPreview } from "../lib/m10";
  import { markReadDelay } from "../lib/settings";
  import { blockRule, labelTargets } from "../lib/labels";
  import { PRESETS, epoch, fromLocalInput, presetTime, toLocalInput, type Preset } from "../lib/when";
  import { app } from "../state/app.svelte";
  import { errorText, presetText, strings } from "../strings";

  let { messageId = null, embedded = false }: { messageId?: number | null; embedded?: boolean } = $props();
  /** The message this card shows: its own, or the list's selection. */
  const current = $derived(messageId ?? app.message);

  let reader = $state<ReaderState>(emptyReader);
  /** Snooze: the date and time picker is open (section 18.2, pick a date and time). */
  let pickSnooze = $state(false);
  let snoozeAt = $state("");
  let frame: HTMLIFrameElement | undefined = $state();
  let frameHeight = $state(0);
  /** Reloads the frame when the body or its images switch changes. */
  let frameKey = $state(0);
  /** The per-message plain-text toggle (SPEC_GAPS G53). */
  let plain = $state(false);
  /** A link waiting for the user's confirmation (host mismatch). */
  let pendingLink = $state<Extract<LinkAction, { kind: "confirm" }> | null>(null);
  /** An executable attachment waiting for confirmation before Open. */
  let pendingOpen = $state<Attachment | null>(null);
  let actionError = $state<AppError | null>(null);
  /** A permanent delete waiting for confirmation (section 9). */
  let confirmDelete = $state(false);
  /** Unsubscribe waiting for confirmation (18.7: never without a click). */
  let confirmUnsubscribe = $state(false);
  /** The message the read timer already ran for: it marks read once per opening. */
  let autoReadFor: number | null = null;
  const http = typeof navigator !== "undefined" && usesHttpScheme(navigator.userAgent);

  async function open(id: number): Promise<void> {
    let next: ReaderState;
    try {
      const view = await commands["message.open"](id);
      next = applyOpen(reader, id, { ok: true, view });
    } catch (err) {
      next = applyOpen(reader, id, { ok: false, error: toAppError(err) });
    }
    if (current !== id) return;
    if (bodyArrived(reader, next)) frameKey += 1;
    reader = next;
  }

  // A newly selected message.
  $effect(() => {
    const id = current;
    pickSnooze = false;
    frameHeight = 0;
    // Show formatted messages off: plain text first (section 8, Body choice).
    plain = !untrack(() => app.ui.html);
    confirmUnsubscribe = false;
    pendingLink = null;
    pendingOpen = null;
    actionError = null;
    confirmDelete = false;
    autoReadFor = null;
    app.hoveredLink = null;
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
    if (n > 0 && current === s.view.id) void open(s.view.id);
  });

  // Read state (section 8): shown for 1.5 s (or at once, or never: the setting), an unread
  // message is marked read, once.
  $effect(() => {
    const s = reader;
    if (s.kind !== "open" || !s.view.unread || s.view.body_kind === "loading" || autoReadFor === s.view.id) return;
    const delay = markReadDelay(app.ui.markRead, READ_DELAY_MS);
    if (delay === null) return;
    const id = s.view.id;
    const timer = setTimeout(() => {
      if (current !== id || autoReadFor === id) return;
      autoReadFor = id;
      void app.act(() => commands["message.set_flags"]([id], { seen: true }));
    }, delay);
    return () => clearTimeout(timer);
  });

  // The open card's shortcut actions (section 11 table).
  $effect(() => {
    const v = reader.kind === "open" && !reader.deleted ? reader.view : null;
    if (!v) return;
    const id = v.id;
    app.readers.set(id, {
      reply: () => void run(() => commands["compose.reply"](id)),
      replyAll: () => void run(() => commands["compose.reply_all"](id)),
      forward: () => void run(() => commands["compose.forward"](id, false)),
      archive: () => {
        if (v.can_archive) void undoable(() => commands["message.archive"]([id]), strings.archivedToast);
      },
      delete: () => (deleteNeedsConfirm(v) ? (confirmDelete = true) : remove(id, false)),
      star: () => void app.act(() => commands["message.set_flags"]([id], { flagged: !v.flagged })),
      snooze: () => snoozePreset(id, "pick"),
      mute: () =>
        void app.act(() => (v.muted ? commands["message.unmute"](v.thread_id) : commands["message.mute"](v.thread_id))),
      toggleUnread: () => {
        autoReadFor = id;
        void app.act(() => commands["message.set_flags"]([id], { seen: v.unread }));
      },
    });
    return () => {
      app.readers.delete(id);
    };
  });

  /** Unsubscribe, after the confirmation (section 18.7). */
  async function unsubscribe(id: number): Promise<void> {
    confirmUnsubscribe = false;
    await run(async () => {
      const out = await commands["message.unsubscribe"](id);
      app.toast(
        out.method === "one_click"
          ? strings.unsubscribedToast
          : out.method === "mailto"
            ? strings.unsubscribeDraftToast
            : strings.unsubscribePageToast,
      );
    });
  }

  /** Archive, delete or move: the row leaves at once, with a 10 s Undo toast (section 9). */
  async function undoable(run: () => Promise<number>, text: string, undo = true): Promise<void> {
    let op: number | undefined;
    const ok = await app.act(async () => {
      op = await run();
    });
    if (ok && op !== undefined) {
      app.toast(text, undo ? op : undefined);
      app.message = null;
      app.thread = null;
    }
  }

  /** Snooze until `at` (section 18.2): the message leaves the list, with Undo. */
  function snooze(id: number, at: Date): void {
    pickSnooze = false;
    void undoable(() => commands["message.snooze"]([id], epoch(at)), strings.snoozedToast(readerDateText(epoch(at))));
  }

  function snoozePreset(id: number, value: string): void {
    if (value === "pick") {
      pickSnooze = true;
      snoozeAt = toLocalInput(presetTime("tomorrow", new Date()));
      return;
    }
    if ((PRESETS as readonly string[]).includes(value)) snooze(id, presetTime(value as Preset, new Date()));
  }

  function remove(id: number, permanent: boolean): void {
    confirmDelete = false;
    void undoable(() => commands["message.delete"]([id]), permanent ? strings.deletedToast : strings.trashedToast, !permanent);
  }

  /** Runs a command; a failure shows in the pane, a cancelled dialog shows nothing. */
  async function run(action: () => Promise<unknown>): Promise<void> {
    actionError = null;
    try {
      await action();
    } catch (err) {
      const e = toAppError(err);
      if (e.code !== "cancelled") actionError = e;
    }
  }

  function onLink(href: string, text: string): void {
    const action = decideLink(href, text);
    pendingLink = null;
    if (action.kind === "open") void run(() => commands["link.open"](action.href));
    else if (action.kind === "confirm") pendingLink = action;
    else if (action.kind === "compose" && reader.kind === "open") {
      const account = reader.view.account_id;
      void run(() => commands["compose.new"](account, mailtoAddress(action.href)));
    }
    // ignore: nothing happens.
  }

  async function allowImages(always: boolean): Promise<void> {
    const s = reader;
    if (s.kind !== "open") return;
    const id = s.view.id;
    await run(async () => {
      await (always ? commands["images.allow_sender"](id) : commands["images.allow_once"](id));
      await open(id);
      frameKey += 1;
    });
  }

  function openAttachment(a: Attachment): void {
    pendingOpen = null;
    if (a.executable) {
      pendingOpen = a;
      return;
    }
    void run(() => commands["attachment.open"](a.id));
  }

  onMount(() => {
    const onMessage = (e: MessageEvent) => {
      if (!frame || e.source !== frame.contentWindow) return;
      const data = e.data as { zeromail?: unknown; height?: unknown; href?: unknown; text?: unknown } | null;
      if (data?.zeromail === "height" && typeof data.height === "number" && Number.isFinite(data.height)) {
        frameHeight = Math.max(0, Math.ceil(data.height));
      } else if (data?.zeromail === "link" && typeof data.href === "string") {
        onLink(data.href, typeof data.text === "string" ? data.text : "");
      } else if (data?.zeromail === "hover") {
        app.hoveredLink = typeof data.href === "string" ? data.href : null;
      }
    };
    window.addEventListener("message", onMessage);
    return () => {
      window.removeEventListener("message", onMessage);
      app.hoveredLink = null;
    };
  });

  const view = $derived(reader.kind === "open" ? reader.view : null);
  const targets = $derived(view ? moveTargets(app.folders[view.account_id] ?? [], view.folder_id) : []);
  const labelChoices = $derived(
    view && view.labels ? labelTargets(app.folders[view.account_id] ?? [], view.labels) : [],
  );
  const snoozeDate = $derived(fromLocalInput(snoozeAt, new Date()));
</script>

<section class="card" class:embedded aria-label={strings.readerLabel}>
  {#if reader.kind === "empty"}
    <p class="placeholder">{strings.noMessageSelected}</p>
  {:else if reader.kind === "error"}
    <p class="placeholder">{strings.couldNotOpen} {errorText[reader.error.code]}</p>
  {:else if view}
    <div class="body">
      {#if reader.kind === "open" && reader.deleted}
        <p class="notice" role="status"><Icon name="trash" size="--icon-14" /> {strings.deleted}</p>
      {/if}
      {#if !(reader.kind === "open" && reader.deleted)}
        <div class="toolbar" role="toolbar" aria-label={strings.actionsLabel}>
          <button class="small tint press" onclick={() => run(() => commands["compose.reply"](view.id))}>{strings.reply}</button>
          <button class="small tint press" onclick={() => run(() => commands["compose.reply_all"](view.id))}>{strings.replyAll}</button>
          <button class="small tint press" onclick={() => run(() => commands["compose.forward"](view.id, false))}>{strings.forward}</button>
          <button class="small tint press" onclick={() => run(() => commands["compose.forward"](view.id, true))}>
            {strings.forwardAsAttachment}
          </button>
          {#if view.can_archive}
            <button class="small tint press" onclick={() => undoable(() => commands["message.archive"]([view.id]), strings.archivedToast)}>
              <Icon name="archive" size="--icon-14" />{strings.archive}
            </button>
          {/if}
          <button
            class="small tint press"
            onclick={() => (deleteNeedsConfirm(view) ? (confirmDelete = true) : remove(view.id, false))}
          >
            <Icon name="trash" size="--icon-14" />{strings.delete}
          </button>
          <button
            class="small tint press"
            aria-pressed={view.flagged}
            onclick={() => app.act(() => commands["message.set_flags"]([view.id], { flagged: !view.flagged }))}
          >
            <Icon name={view.flagged ? "starFill" : "star"} size="--icon-14" />{view.flagged ? strings.unstar : strings.star}
          </button>
          <button
            class="small tint press"
            onclick={() => {
              autoReadFor = view.id;
              void app.act(() => commands["message.set_flags"]([view.id], { seen: view.unread }));
            }}
          >
            {view.unread ? strings.markRead : strings.markUnread}
          </button>
          <select
            class="small move"
            aria-label={strings.snooze}
            value=""
            onchange={(e) => {
              const v = (e.currentTarget as HTMLSelectElement).value;
              (e.currentTarget as HTMLSelectElement).value = "";
              snoozePreset(view.id, v);
            }}
          >
            <option value="" disabled>{strings.snooze}</option>
            {#each PRESETS as p (p)}
              <option value={p}>{presetText[p]}</option>
            {/each}
            <option value="pick">{strings.pickDateTime}</option>
          </select>
          {#if view.snoozed_until !== null}
            <button class="small tint press" onclick={() => app.act(() => commands["message.unsnooze"]([view.id]))}>
              {strings.unsnooze}
            </button>
          {/if}
          <button
            class="small tint press"
            aria-pressed={view.muted}
            onclick={() =>
              app.act(() => (view.muted ? commands["message.unmute"](view.thread_id) : commands["message.mute"](view.thread_id)))}
          >
            {view.muted ? strings.unmute : strings.mute}
          </button>
          {#if view.unsubscribe !== null}
            <button class="small tint press" onclick={() => (confirmUnsubscribe = true)}>{strings.unsubscribe}</button>
          {/if}
          <button class="small tint press" onclick={() => (app.printing = { message_id: view.id })}>{strings.print}</button>
          {#if view.from.addr}
            <button
              class="small tint press"
              onclick={() => app.act(async () => {
                await commands["rules.save"](blockRule(view.from.addr));
                app.toast(strings.blockedToast(view.from.addr));
              })}
            >
              {strings.blockSender}
            </button>
          {/if}
          {#if labelChoices.length > 0}
            <select
              class="small move"
              aria-label={strings.addLabel}
              value=""
              onchange={(e) => {
                const to = Number((e.currentTarget as HTMLSelectElement).value);
                (e.currentTarget as HTMLSelectElement).value = "";
                if (to) void undoable(() => commands["message.add_label"]([view.id], to), strings.labelAddedToast, true);
              }}
            >
              <option value="" disabled>{strings.addLabel}</option>
              {#each labelChoices as f (f.id)}
                <option value={f.id}>{f.path}</option>
              {/each}
            </select>
          {/if}
          {#if targets.length > 0}
            <select
              class="small move"
              aria-label={strings.moveTo}
              value=""
              onchange={(e) => {
                const to = Number((e.currentTarget as HTMLSelectElement).value);
                const name = targets.find((f) => f.id === to)?.name ?? "";
                (e.currentTarget as HTMLSelectElement).value = "";
                if (to) void undoable(() => commands["message.move"]([view.id], to), strings.movedToast(name));
              }}
            >
              <option value="" disabled>{strings.moveToPlaceholder}</option>
              {#each targets as f (f.id)}
                <option value={f.id}>{f.path}</option>
              {/each}
            </select>
          {/if}
        </div>
        {#if pickSnooze}
          <div class="notice confirm" role="dialog" aria-label={strings.pickDateTime}>
            <input class="when" type="datetime-local" bind:value={snoozeAt} aria-label={strings.pickDateTime} />
            <span class="actions">
              <button
                class="small tint press"
                disabled={snoozeDate === null}
                onclick={() => {
                  if (snoozeDate) snooze(view.id, snoozeDate);
                }}>{strings.snooze}</button
              >
              <button class="small tint press" onclick={() => (pickSnooze = false)}>{strings.cancel}</button>
            </span>
          </div>
        {/if}
        {#if confirmUnsubscribe && view.unsubscribe !== null}
          <div class="notice confirm" role="alertdialog" aria-label={strings.unsubscribe}>
            <span class="text">{strings.unsubscribeConfirm[view.unsubscribe]}</span>
            <span class="actions">
              <button class="small tint press" onclick={() => unsubscribe(view.id)}>{strings.unsubscribe}</button>
              <button class="small tint press" onclick={() => (confirmUnsubscribe = false)}>{strings.cancel}</button>
            </span>
          </div>
        {/if}
        {#if confirmDelete}
          <div class="notice confirm" role="alertdialog" aria-label={strings.deleteForever}>
            <span class="text">{strings.deleteForeverConfirm}</span>
            <span class="actions">
              <button class="small tint press" onclick={() => remove(view.id, true)}>{strings.deleteForever}</button>
              <button class="small tint press" onclick={() => (confirmDelete = false)}>{strings.cancel}</button>
            </span>
          </div>
        {/if}
      {/if}
      <h2 class="subject">{subjectText(view.subject)}</h2>
      {#if view.labels || view.important || view.snoozed_until !== null || view.muted}
        <ul class="chips" aria-label={strings.labelsLabel}>
          {#if view.important}<li class="chip">{strings.important}</li>{/if}
          {#if view.snoozed_until !== null}<li class="chip">{strings.snoozedUntil(readerDateText(view.snoozed_until))}</li>{/if}
          {#if view.muted}<li class="chip">{strings.muted}</li>{/if}
          {#each view.labels ?? [] as l (l.folder_id)}
            <li class="chip">
              {l.name}
              {#if (view.labels?.length ?? 0) > 1}
                <button
                  class="chip-x"
                  aria-label={strings.removeLabel(l.name)}
                  onclick={() => undoable(() => commands["message.remove_label"]([view.id], l.folder_id), strings.labelRemovedToast, true)}
                >
                  <Icon name="x" size="--icon-14" />
                </button>
              {/if}
            </li>
          {/each}
        </ul>
      {/if}
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
        <div class="side">
          <time class="meta-small" datetime={new Date(view.date_utc * 1000).toISOString()}>{readerDateText(view.date_utc)}</time>
          {#if view.has_html && view.body_kind === "html"}
            <button class="small tint press" onclick={() => ((plain = !plain), (frameKey += 1), (frameHeight = 0))}>
              {plain ? strings.showFormatted : strings.showPlainText}
            </button>
          {/if}
        </div>
      </div>

      {#if pendingLink}
        <div class="notice confirm" role="alertdialog" aria-label={strings.linkCheckTitle}>
          <span class="text">{strings.linkMismatch(pendingLink.shownHost, pendingLink.realHost)}</span>
          <span class="actions">
            <button
              class="small tint press"
              onclick={() => {
                const href = pendingLink?.href;
                pendingLink = null;
                if (href) void run(() => commands["link.open"](href));
              }}>{strings.openLink}</button
            >
            <button class="small tint press" onclick={() => (pendingLink = null)}>{strings.cancel}</button>
          </span>
        </div>
      {/if}
      {#if actionError}
        <p class="notice" role="alert">{errorText[actionError.code]}</p>
      {/if}

      {#if view.body_kind === "loading"}
        <p class="meta" role="status">{strings.loadingBody}</p>
      {:else if view.body_kind === "unreadable"}
        <div class="notice" role="status">
          <span class="text">{strings.couldNotDisplay}</span>
          <button class="small tint press" onclick={() => run(() => commands["message.save_source"](view.id))}>
            {strings.saveSource}
          </button>
        </div>
      {:else}
        {#if view.truncated}
          <div class="notice" role="status">
            <span class="text">{strings.truncated}</span>
            <button class="small tint press" onclick={() => run(() => commands["message.save_source"](view.id))}>
              {strings.saveSource}
            </button>
          </div>
        {/if}
        {#if view.remote_images === "blocked" && !plain}
          <div class="notice" role="status">
            <span class="text with-icon"><Icon name="lockSimple" size="--icon-14" />{strings.imagesBlocked}</span>
            <span class="actions">
              <button class="small tint press" onclick={() => allowImages(false)}>{strings.loadImages}</button>
              <button class="small tint press" onclick={() => allowImages(true)}>{strings.alwaysLoadImages}</button>
            </span>
          </div>
        {/if}
        <div class="canvas">
          {#key `${view.id}:${frameKey}`}
            <iframe
              bind:this={frame}
              title={strings.bodyFrameTitle}
              sandbox="allow-scripts"
              referrerpolicy="no-referrer"
              src={platformUrl(bodyUrl(view.body_url, plain), http)}
              style:height={`${frameHeight}px`}
            ></iframe>
          {/key}
        </div>
      {/if}

      {#if view.attachments.length > 0}
        <ul class="attachments" aria-label={strings.attachmentsLabel}>
          {#each view.attachments as a (a.id)}
            <li class="attachment">
              <span class="file"><Icon name="fileText" size="--icon-16" /></span>
              <span class="file-name" title={a.filename}>{a.filename || strings.unnamedAttachment}</span>
              <span class="meta-small">{formatSize(a.size)}</span>
              {#if canPreview(a)}
                <button class="small tint press" onclick={() => (app.lightbox = { message_id: view.id, attachment: a })}>
                  {strings.preview}
                </button>
              {/if}
              <button class="small tint press" onclick={() => openAttachment(a)}>{strings.openAttachment}</button>
              <button class="small tint press" onclick={() => run(() => commands["attachment.save"](a.id))}>
                {strings.saveAttachment}
              </button>
            </li>
          {/each}
        </ul>
        {#if pendingOpen}
          <div class="notice confirm" role="alertdialog" aria-label={strings.executableTitle}>
            <span class="text">{strings.executableWarning(pendingOpen.filename)}</span>
            <span class="actions">
              <button
                class="small tint press"
                onclick={() => {
                  const id = pendingOpen?.id;
                  pendingOpen = null;
                  if (id !== undefined) void run(() => commands["attachment.open"](id));
                }}>{strings.openAnyway}</button
              >
              <button class="small tint press" onclick={() => (pendingOpen = null)}>{strings.cancel}</button>
            </span>
          </div>
        {/if}
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

  .card.embedded {
    min-height: auto;
    overflow: visible;
  }

  .embedded .body {
    overflow-y: visible;
  }

  .placeholder {
    margin: auto;
    color: var(--color-text-muted);
  }

  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-6);
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .chip {
    display: inline-flex;
    align-items: center;
    gap: var(--space-4);
    height: var(--button-height-small);
    padding: 0 var(--space-10);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-pill);
    font-size: var(--text-meta);
    color: var(--color-text-muted-strong);
  }

  .chip-x {
    display: inline-flex;
    padding: 0;
    border: 0;
    background: transparent;
    color: var(--color-text-muted);
  }

  .when {
    height: var(--button-height-small);
    border: 1px solid var(--color-border-input);
    border-radius: var(--radius-small-button);
    background: var(--color-input-bg);
    color: var(--color-ink);
    padding: 0 var(--space-6);
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

  .side {
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    gap: var(--space-6);
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
    flex-wrap: wrap;
    gap: var(--space-8);
    padding: var(--space-8) var(--space-12);
    border-radius: var(--radius-input);
    background: var(--color-surface-sunken);
    border: 1px solid var(--color-divider);
    color: var(--color-text-muted-strong);
  }

  .notice .text {
    flex: 1 1 100%;
    min-width: 0;
    overflow-wrap: break-word;
  }

  .with-icon {
    display: inline-flex;
    align-items: center;
    gap: var(--space-8);
  }

  .actions {
    display: flex;
    gap: var(--space-6);
  }

  .toolbar {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-6);
  }

  .toolbar .small {
    display: inline-flex;
    align-items: center;
    gap: var(--space-6);
  }

  .move {
    max-width: 100%;
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

  .attachments {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--space-6);
  }

  .attachment {
    display: flex;
    align-items: center;
    gap: var(--space-8);
    padding: var(--space-6) var(--space-6) var(--space-6) var(--space-10);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-input);
  }

  .file {
    display: inline-flex;
    color: var(--color-file-icon);
  }

  .file-name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
</style>
