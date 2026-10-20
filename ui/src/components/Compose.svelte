<script lang="ts">
  // The compose window (section 11, Compose; SPEC_GAPS G63): From (identity picker), To, Cc,
  // Bcc, Subject, a rich text body or plain text, attachments by button or drag and drop.
  // The draft is saved locally 5 s after typing stops; Core uploads it 30 s after the last
  // change and when the window closes (section 7). Send warns about an empty subject or a
  // body that mentions attachments when there are none; red chips block it. Send later
  // schedules the send (section 18.3); Insert template puts a template's body at the cursor
  // and fills an empty subject; Save as template keeps this draft as one (section 18.4).
  import { onMount, tick, untrack } from "svelte";
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import Icon from "./Icon.svelte";
  import RecipientField from "./RecipientField.svelte";
  import { firstBody, insertAt, invalidRecipients, sendWarnings, type Block } from "../lib/compose";
  import { formatSize, readerDateText } from "../lib/format";
  import {
    commands,
    toAppError,
    type Address,
    type AppError,
    type DraftContent,
    type DraftView,
    type TemplateView,
  } from "../lib/ipc";
  import { PRESETS, epoch, fromLocalInput, presetTime, toLocalInput, type Preset } from "../lib/when";
  import { errorText, presetText, strings } from "../strings";

  let { view }: { view: DraftView } = $props();

  /** Typing stops this long before the draft is saved (section 7). */
  const AUTOSAVE_MS = 5000;

  // The window owns its copy; it starts from the draft it was opened with.
  const initial = untrack(() => view);
  let content = $state<DraftContent>(structuredClone(initial.content));
  let identityId = $state<number | null>(initial.content.identity_id ?? initial.identities[0]?.id ?? null);
  let showCc = $state(initial.content.cc.length > 0);
  let showBcc = $state(initial.content.bcc.length > 0);
  let editor: HTMLDivElement | undefined = $state();
  let error = $state<AppError | null>(null);
  let notice = $state<string | null>(null);
  let confirming = $state<string[] | null>(null);
  let busy = $state(false);
  let linkOpen = $state(false);
  /** Send later: the date and time picker is open (section 18.3). */
  let pickLater = $state(false);
  let laterAt = $state("");
  let templates = $state<TemplateView[]>([]);
  /** Save as template: the name being typed. */
  let templateName = $state<string | null>(null);
  /** Where the cursor was in the editor, for Insert template (focus moves to the picker). */
  let savedRange: Range | null = null;
  let textarea: HTMLTextAreaElement | undefined = $state();
  let linkUrl = $state("");
  let timer: ReturnType<typeof setTimeout> | undefined;

  function render(blocks: Block[]): void {
    if (!editor) return;
    editor.replaceChildren();
    for (const b of blocks) {
      const el = document.createElement(b.kind === "quote" ? "blockquote" : "div");
      const lines = b.text.replace(/\r\n/g, "\n").split("\n");
      lines.forEach((line, i) => {
        if (i > 0) el.appendChild(document.createElement("br"));
        if (line) el.appendChild(document.createTextNode(line));
      });
      if (b.kind === "line" && b.text === "") el.appendChild(document.createElement("br"));
      editor.appendChild(el);
    }
  }

  /** The plain part from the editor: quoted lines get `> ` and the signature line is `-- `
   * again (the browser trims its trailing space). */
  function plainText(root: HTMLElement): string {
    const lines: string[] = [];
    let inline = "";
    const block = (text: string): string[] => text.replace(/\n$/, "").split("\n");
    for (const n of Array.from(root.childNodes)) {
      if (n instanceof HTMLElement && n.tagName === "BR") {
        lines.push(inline);
        inline = "";
      } else if (n instanceof HTMLElement && getComputedStyle(n).display !== "inline") {
        if (inline) lines.push(inline);
        inline = "";
        const quoted = n.tagName === "BLOCKQUOTE";
        for (const l of block(n.innerText)) lines.push(quoted ? (l ? `> ${l}` : ">") : l === "--" ? "-- " : l);
      } else {
        inline += n.textContent ?? "";
      }
    }
    if (inline) lines.push(inline);
    return lines.join("\n");
  }

  function readEditor(): void {
    if (content.plain_only || !editor) return;
    content.html = editor.innerHTML;
    content.text = plainText(editor);
  }

  async function save(): Promise<void> {
    clearTimeout(timer);
    readEditor();
    content.identity_id = identityId;
    content.quote = null;
    try {
      await commands["draft.save"](initial.draft_id, $state.snapshot(content));
    } catch (err) {
      error = toAppError(err);
    }
  }

  function changed(): void {
    clearTimeout(timer);
    timer = setTimeout(() => void save(), AUTOSAVE_MS);
  }

  function setRecipients(field: "to" | "cc" | "bcc", list: Address[]): void {
    content[field] = list;
    changed();
  }

  function format(command: string, arg?: string): void {
    editor?.focus();
    document.execCommand(command, false, arg);
    changed();
  }

  function insertLink(): void {
    const url = linkUrl.trim();
    linkOpen = false;
    linkUrl = "";
    if (/^(https?:|mailto:)/i.test(url)) format("createLink", url);
  }

  function togglePlain(): void {
    readEditor();
    content.plain_only = !content.plain_only;
    if (!content.plain_only) {
      void tick().then(() => render(content.text.split("\n").map((t) => ({ kind: "line", text: t }))));
    }
    changed();
  }

  async function attach(path: string | null): Promise<void> {
    error = null;
    try {
      content.attachments = (await commands["draft.add_attachment"](initial.draft_id, path)).attachments;
    } catch (err) {
      const e = toAppError(err);
      if (e.code !== "cancelled") error = e;
    }
  }

  async function detach(id: string): Promise<void> {
    try {
      content.attachments = (await commands["draft.remove_attachment"](initial.draft_id, id)).attachments;
    } catch (err) {
      error = toAppError(err);
    }
  }

  function keepCursor(): void {
    const sel = window.getSelection();
    if (editor && sel && sel.rangeCount > 0 && editor.contains(sel.getRangeAt(0).commonAncestorContainer)) {
      savedRange = sel.getRangeAt(0).cloneRange();
    }
  }

  /** Insert template (section 18.4, T41): the body at the cursor; an empty subject is filled. */
  async function insertTemplate(id: number): Promise<void> {
    error = null;
    try {
      const t = await commands["draft.apply_template"](initial.draft_id, id);
      if (content.subject.trim() === "") content.subject = t.subject;
      if (content.plain_only) {
        const start = textarea?.selectionStart ?? content.text.length;
        const end = textarea?.selectionEnd ?? start;
        const next = insertAt(content.text, start, end, t.body_text);
        content.text = next.text;
        await tick();
        textarea?.setSelectionRange(next.cursor, next.cursor);
      } else if (editor) {
        editor.focus();
        const sel = window.getSelection();
        if (sel && savedRange) {
          sel.removeAllRanges();
          sel.addRange(savedRange);
        }
        document.execCommand("insertHTML", false, t.body_html);
      }
      changed();
    } catch (err) {
      error = toAppError(err);
    }
  }

  /** Save as template (section 18.4): this draft's subject and bodies under a name. */
  async function saveTemplate(): Promise<void> {
    const name = (templateName ?? "").trim();
    if (!name) return;
    readEditor();
    try {
      templates = await commands["templates.save"]({
        id: 0,
        name,
        subject: content.subject,
        body_html: content.plain_only ? "" : content.html,
        body_text: content.text,
      });
      templateName = null;
      notice = strings.templateSaved;
    } catch (err) {
      error = toAppError(err);
    }
  }

  function laterPreset(value: string): void {
    if (value === "pick") {
      pickLater = true;
      laterAt = toLocalInput(presetTime("tomorrow", new Date()));
      return;
    }
    if ((PRESETS as readonly string[]).includes(value)) void send(false, presetTime(value as Preset, new Date()));
  }

  async function send(confirmed = false, at: Date | null = null): Promise<void> {
    error = null;
    notice = null;
    laterFor = at;
    readEditor();
    if (content.to.length + content.cc.length + content.bcc.length === 0) {
      notice = strings.addRecipient;
      return;
    }
    if (invalidRecipients(content).length > 0) {
      notice = strings.invalidRecipients;
      return;
    }
    const w = sendWarnings(content);
    const asks: string[] = [];
    if (w.emptySubject) asks.push(strings.emptySubjectWarning);
    if (w.missingAttachment) asks.push(strings.missingAttachmentWarning);
    if (!confirmed && asks.length > 0) {
      confirming = asks;
      return;
    }
    confirming = null;
    if (identityId === null) return;
    busy = true;
    try {
      await save();
      await commands["draft.send"](initial.draft_id, identityId, at ? epoch(at) : null);
      // The app closes this window; the main window shows Undo.
    } catch (err) {
      error = toAppError(err);
    } finally {
      busy = false;
    }
  }

  /** The time a confirmed Send later goes ahead with. */
  let laterFor: Date | null = null;
  const laterDate = $derived(fromLocalInput(laterAt, new Date()));

  onMount(() => {
    void commands["templates.list"]().then(
      (t) => (templates = t),
      () => (templates = []),
    );
    if (content.plain_only) {
      // Plain text lives in content.text.
    } else if (content.html.trim() !== "") {
      if (editor) editor.innerHTML = content.html;
    } else {
      const words = {
        wrote: strings.quoteWrote,
        forwardedHeader: strings.forwardedHeader,
        from: strings.forwardFrom,
        date: strings.forwardDate,
        subject: strings.forwardSubject,
        to: strings.forwardTo,
      };
      render(firstBody(content.quote, initial.signature_text, words, (utc) => new Date(utc * 1000).toLocaleString()));
    }
    const keys = (e: KeyboardEvent) => {
      if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
        e.preventDefault();
        void send();
      }
    };
    window.addEventListener("keydown", keys);
    // The app runs this when the window's close button is pressed, then closes it once the
    // save has landed (section 7, Drafts: nothing typed is lost).
    window.__ZEROMAIL_FLUSH__ = () => void save();
    // Files dropped on this window only (its webview has drag and drop on, G63).
    const dropped = getCurrentWebview().onDragDropEvent((e) => {
      if (e.payload.type === "drop") for (const p of e.payload.paths) void attach(p);
    });
    return () => {
      window.removeEventListener("keydown", keys);
      void dropped.then((off) => off());
      clearTimeout(timer);
    };
  });

  const identities = initial.identities;
</script>

<main class="compose">
  <header class="fields">
    <div class="field">
      <label class="label" for="from">{strings.composeFrom}</label>
      <select id="from" bind:value={identityId} onchange={changed}>
        {#each identities as i (i.id)}
          <option value={i.id}>{i.name ? `${i.name} <${i.address}>` : i.address}</option>
        {/each}
      </select>
      <span class="toggles">
        {#if !showCc}<button class="link" onclick={() => (showCc = true)}>{strings.cc}</button>{/if}
        {#if !showBcc}<button class="link" onclick={() => (showBcc = true)}>{strings.bcc}</button>{/if}
      </span>
    </div>
    <RecipientField label={strings.to} value={content.to} accountId={initial.account_id} onchange={(l) => setRecipients("to", l)} />
    {#if showCc}
      <RecipientField label={strings.cc} value={content.cc} accountId={initial.account_id} onchange={(l) => setRecipients("cc", l)} />
    {/if}
    {#if showBcc}
      <RecipientField label={strings.bcc} value={content.bcc} accountId={initial.account_id} onchange={(l) => setRecipients("bcc", l)} />
    {/if}
    <div class="field">
      <label class="label" for="subject">{strings.subject}</label>
      <input id="subject" class="subject" bind:value={content.subject} oninput={changed} />
    </div>
  </header>

  <div class="toolbar" role="toolbar" aria-label={strings.formatting}>
    {#if !content.plain_only}
      <button class="tool tint" aria-label={strings.bold} onclick={() => format("bold")}><b>B</b></button>
      <button class="tool tint" aria-label={strings.italic} onclick={() => format("italic")}><i>I</i></button>
      <button class="tool tint" aria-label={strings.underline} onclick={() => format("underline")}><u>U</u></button>
      <button class="tool tint" aria-label={strings.bulletList} onclick={() => format("insertUnorderedList")}>•</button>
      <button class="tool tint" aria-label={strings.numberedList} onclick={() => format("insertOrderedList")}>1.</button>
      <button class="tool tint" aria-label={strings.link} onclick={() => (linkOpen = !linkOpen)}>{strings.link}</button>
      {#if linkOpen}
        <input class="link-input" placeholder="https://" bind:value={linkUrl} onkeydown={(e) => e.key === "Enter" && insertLink()} />
      {/if}
    {/if}
    <span class="grow"></span>
    <button class="tool tint" aria-pressed={content.plain_only} onclick={togglePlain}>{strings.plainTextOnly}</button>
  </div>

  {#if content.plain_only}
    <textarea class="body" aria-label={strings.messageBody} bind:this={textarea} bind:value={content.text} oninput={changed}></textarea>
  {:else}
    <div
      class="body editor"
      bind:this={editor}
      contenteditable="true"
      role="textbox"
      aria-multiline="true"
      aria-label={strings.messageBody}
      tabindex="0"
      oninput={changed}
      onblur={keepCursor}
      onkeyup={keepCursor}
      onmouseup={keepCursor}
    ></div>
  {/if}

  {#if content.attachments.length > 0 || content.forward_attachment !== null}
    <ul class="attachments" aria-label={strings.attachmentsLabel}>
      {#if content.forward_attachment !== null}
        <li class="attachment"><Icon name="fileText" size="--icon-16" /><span class="file-name">{strings.forwardedMessage}</span></li>
      {/if}
      {#each content.attachments as a (a.id)}
        <li class="attachment">
          <Icon name="paperclip" size="--icon-16" />
          <span class="file-name" title={a.name}>{a.name}</span>
          <span class="meta">{formatSize(a.size)}</span>
          <button class="small tint press" aria-label={strings.removeAttachment(a.name)} onclick={() => detach(a.id)}>
            <Icon name="x" size="--icon-12" />
          </button>
        </li>
      {/each}
    </ul>
  {/if}

  {#if confirming}
    <div class="notice" role="alertdialog" aria-label={strings.sendAnyway}>
      <span class="text">{confirming.join(" ")}</span>
      <button class="small tint press" onclick={() => send(true, laterFor)}>{strings.sendAnyway}</button>
      <button class="small tint press" onclick={() => (confirming = null)}>{strings.cancel}</button>
    </div>
  {/if}
  {#if pickLater}
    <div class="notice" role="dialog" aria-label={strings.sendLater}>
      <input class="when" type="datetime-local" bind:value={laterAt} aria-label={strings.pickDateTime} />
      <span class="text">{laterDate ? readerDateText(epoch(laterDate)) : ""}</span>
      <button
        class="small tint press"
        disabled={laterDate === null || busy}
        onclick={() => {
          pickLater = false;
          if (laterDate) void send(false, laterDate);
        }}>{strings.sendLater}</button
      >
      <button class="small tint press" onclick={() => (pickLater = false)}>{strings.cancel}</button>
      <span class="text">{strings.sendLaterNote}</span>
    </div>
  {/if}
  {#if templateName !== null}
    <div class="notice" role="dialog" aria-label={strings.saveAsTemplate}>
      <input class="when" placeholder={strings.templateName} bind:value={templateName} onkeydown={(e) => e.key === "Enter" && saveTemplate()} />
      <button class="small tint press" disabled={templateName.trim() === ""} onclick={saveTemplate}>{strings.saveAsTemplate}</button>
      <button class="small tint press" onclick={() => (templateName = null)}>{strings.cancel}</button>
    </div>
  {/if}
  {#if notice}<p class="notice" role="alert">{notice}</p>{/if}
  {#if error}<p class="notice" role="alert">{errorText[error.code]}</p>{/if}

  <footer class="actions">
    <button class="primary press" disabled={busy || identityId === null} onclick={() => send()}>
      <Icon name="paperPlaneTilt" size="--icon-14" />{strings.send}
    </button>
    <select
      class="small"
      aria-label={strings.sendLater}
      title={strings.sendLaterNote}
      value=""
      disabled={busy || identityId === null}
      onchange={(e) => {
        const v = (e.currentTarget as HTMLSelectElement).value;
        (e.currentTarget as HTMLSelectElement).value = "";
        laterPreset(v);
      }}
    >
      <option value="" disabled>{strings.sendLater}</option>
      {#each PRESETS as p (p)}
        <option value={p}>{presetText[p]}</option>
      {/each}
      <option value="pick">{strings.pickDateTime}</option>
    </select>
    <button class="small tint press" onclick={() => attach(null)}><Icon name="paperclip" size="--icon-14" />{strings.attach}</button>
    <select
      class="small"
      aria-label={strings.insertTemplate}
      value=""
      onchange={(e) => {
        const id = Number((e.currentTarget as HTMLSelectElement).value);
        (e.currentTarget as HTMLSelectElement).value = "";
        if (id) void insertTemplate(id);
      }}
    >
      <option value="" disabled>{templates.length > 0 ? strings.insertTemplate : strings.noTemplates}</option>
      {#each templates as t (t.id)}
        <option value={t.id}>{t.name}</option>
      {/each}
    </select>
    <button class="small tint press" onclick={() => (templateName = content.subject)}>{strings.saveAsTemplate}</button>
    <span class="grow"></span>
    <button class="small tint press" onclick={() => commands["draft.discard"](initial.draft_id).catch((e) => (error = toAppError(e)))}>
      <Icon name="trash" size="--icon-14" />{strings.discard}
    </button>
  </footer>
</main>

<style>
  .compose {
    height: 100vh;
    display: grid;
    grid-template-rows: auto auto minmax(0, 1fr) auto auto auto;
    gap: var(--space-8);
    padding: var(--space-14) var(--space-18);
    background: var(--color-card);
    color: var(--color-ink);
  }

  .fields {
    display: flex;
    flex-direction: column;
  }

  .field {
    display: grid;
    grid-template-columns: var(--date-width) minmax(0, 1fr) auto;
    align-items: center;
    gap: var(--space-8);
    padding: var(--space-6) 0;
    border-bottom: 1px solid var(--color-divider);
  }

  .label {
    font-size: var(--text-meta);
    color: var(--color-text-muted);
  }

  select,
  .subject,
  .link-input {
    height: var(--button-height-small);
    border: 1px solid var(--color-border-input);
    border-radius: var(--radius-small-button);
    background: var(--color-input-bg);
    padding: 0 var(--space-8);
  }

  .subject {
    grid-column: 2 / 4;
  }

  .toggles {
    display: flex;
    gap: var(--space-8);
  }

  .link {
    border: 0;
    background: transparent;
    color: var(--color-text-muted-strong);
    font-size: var(--text-meta);
    cursor: pointer;
  }

  .toolbar {
    display: flex;
    align-items: center;
    gap: var(--space-4);
  }

  .tool {
    --tint: var(--color-button-hover);
    min-width: var(--icon-button-28);
    height: var(--icon-button-28);
    padding: 0 var(--space-8);
    border: 1px solid transparent;
    border-radius: var(--radius-small-button);
    background: transparent;
  }

  .tool[aria-pressed="true"] {
    border-color: var(--color-border);
  }

  .grow {
    flex: 1;
  }

  .body {
    min-height: 0;
    overflow-y: auto;
    padding: var(--space-12);
    border: 1px solid var(--color-border-input);
    border-radius: var(--radius-input);
    background: var(--color-input-bg);
    line-height: var(--leading-body);
    user-select: text;
  }

  textarea.body {
    resize: none;
    font: inherit;
  }

  .editor :global(blockquote) {
    margin: 0 0 0 var(--space-8);
    padding-left: var(--space-12);
    border-left: 2px solid var(--color-border);
    color: var(--color-text-muted-strong);
  }

  .attachments {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-6);
  }

  .attachment {
    display: inline-flex;
    align-items: center;
    gap: var(--space-6);
    padding: var(--space-4) var(--space-4) var(--space-4) var(--space-10);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-input);
    color: var(--color-file-icon);
  }

  .file-name {
    color: var(--color-ink);
    max-width: var(--form-width);
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .meta {
    font-size: var(--text-meta-small);
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

  .notice {
    margin: 0;
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-8);
    padding: var(--space-8) var(--space-12);
    border-radius: var(--radius-input);
    background: var(--color-surface-sunken);
    border: 1px solid var(--color-divider);
    color: var(--color-text-muted-strong);
  }

  .notice .text {
    flex: 1 1 100%;
  }

  .actions {
    display: flex;
    align-items: center;
    gap: var(--space-8);
  }

  .primary {
    display: inline-flex;
    align-items: center;
    gap: var(--space-6);
    height: var(--button-height);
    padding: 0 var(--space-16);
    border: 0;
    border-radius: var(--radius-button);
    background: var(--color-primary-bg);
    color: var(--color-primary-fg);
    font-weight: var(--font-weight-medium);
  }

  .primary:disabled {
    opacity: var(--disabled-opacity);
  }

  .small {
    --tint: var(--color-button-hover);
    display: inline-flex;
    align-items: center;
    gap: var(--space-6);
    height: var(--button-height-small);
    padding: 0 var(--space-10);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-small-button);
    background: var(--color-secondary-bg);
    color: var(--color-secondary-fg);
    font-size: var(--text-meta);
    font-weight: var(--font-weight-medium);
  }
</style>
