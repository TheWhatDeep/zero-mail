<script lang="ts">
  // The shell (section 11; design 3a with SPEC_GAPS G4, G6, G9, G45): sidebar | list and
  // reading pane, banners above, status bar below. First run shows the welcome step, then
  // only the add-account form. M10: Settings, the reading pane right, bottom or off, the
  // keyboard shortcuts (inactive while typing), the print view and the image lightbox.
  import { onMount } from "svelte";
  import AccountForm from "./components/AccountForm.svelte";
  import Banners from "./components/Banners.svelte";
  import Lightbox from "./components/Lightbox.svelte";
  import MessageList from "./components/MessageList.svelte";
  import Outbox from "./components/Outbox.svelte";
  import PrintView from "./components/PrintView.svelte";
  import ReadingPane from "./components/ReadingPane.svelte";
  import SearchBox from "./components/SearchBox.svelte";
  import Settings from "./components/Settings.svelte";
  import Sidebar from "./components/Sidebar.svelte";
  import ThreadView from "./components/ThreadView.svelte";
  import Welcome from "./components/Welcome.svelte";
  import { ShortcutDispatcher, isTyping } from "./lib/shortcuts";
  import { listSource } from "./lib/source";
  import StatusBar from "./components/StatusBar.svelte";
  import { app } from "./state/app.svelte";
  import { errorText, strings } from "./strings";

  const shortcuts = new ShortcutDispatcher();

  function onkeydown(e: KeyboardEvent): void {
    if (!app.loaded || app.accounts.length === 0 || e.defaultPrevented || e.isComposing) return;
    const action = shortcuts.handle(
      {
        key: e.key,
        ctrl: e.ctrlKey,
        meta: e.metaKey,
        shift: e.shiftKey,
        alt: e.altKey,
        typing: isTyping(e.target as HTMLElement | null),
      },
      Date.now(),
    );
    if (!action) return;
    // While a form is open, only Esc and Settings act.
    if (app.form && action !== "close" && action !== "settings") return;
    e.preventDefault();
    app.shortcut(action);
  }

  onMount(() => {
    void app.start();
    window.addEventListener("keydown", onkeydown);
    return () => {
      window.removeEventListener("keydown", onkeydown);
      app.stop();
    };
  });

  const folder = $derived(app.selectedFolder);
  const source = $derived(listSource({ folder: folder ?? null, unified: app.unified, search: app.search, grouped: app.grouped }));
  const title = $derived(app.search ? strings.searchResults : app.unified ? strings.allInboxes : (folder?.name ?? ""));
</script>

{#if !app.loaded}
  <main class="blank" aria-label={strings.appName}></main>
{:else if app.startError}
  <main class="blank fade-in"><p class="muted">{errorText[app.startError.code]}</p></main>
{:else if app.accounts.length === 0}
  <main class="first-run fade-in">
    {#if app.welcomed}<AccountForm cancellable={false} />{:else}<Welcome />{/if}
  </main>
{:else}
  <div class="shell screen fade-in">
    <Sidebar />
    <main class="main">
      <Banners />
      {#if app.form}
        <AccountForm account={app.form.mode === "edit" ? app.form.account : null} cancellable={true} />
      {:else if app.settingsOpen}
        <Settings />
      {:else if app.outboxAccount !== null}
        <h1 class="title">{strings.outbox}</h1>
        <Outbox accountId={app.outboxAccount} />
      {:else if app.ui.readingPane === "off" && app.fullView && app.message !== null}
        <div class="head">
          <button class="small tint press" onclick={() => (app.fullView = false)}>{strings.backToList}</button>
        </div>
        <div class="full">
          {#if app.thread}<ThreadView thread={app.thread} />{:else}<ReadingPane />{/if}
        </div>
      {:else}
        <div class="head">
          <h1 class="title">{title}</h1>
          <SearchBox />
        </div>
        <div class="panes" class:bottom={app.ui.readingPane === "bottom"} class:off={app.ui.readingPane === "off"}>
          {#if source}
            <MessageList {source} />
          {:else}
            <div></div>
          {/if}
          {#if app.ui.readingPane !== "off"}
            {#if app.thread}
              <ThreadView thread={app.thread} />
            {:else}
              <ReadingPane />
            {/if}
          {/if}
        </div>
      {/if}
    </main>
    <div class="status"><StatusBar /></div>
  </div>
  {#if app.lightbox}
    <Lightbox
      messageId={app.lightbox.message_id}
      attachmentId={app.lightbox.attachment.id}
      name={app.lightbox.attachment.filename || strings.unnamedAttachment}
    />
  {/if}
  {#if app.printing}
    {#key app.printing}<PrintView job={app.printing} />{/key}
  {/if}
{/if}

<style>
  .blank,
  .first-run {
    height: 100vh;
    background: var(--color-surface-main);
  }

  .blank {
    display: grid;
    place-items: center;
  }

  .muted {
    color: var(--color-text-muted);
  }

  .shell {
    height: 100vh;
    display: grid;
    grid-template-columns: var(--sidebar-width) minmax(0, 1fr);
    grid-template-rows: minmax(0, 1fr) var(--status-bar-height);
    background: var(--color-surface-main);
  }

  .main {
    min-height: 0;
    display: grid;
    grid-template-rows: auto auto minmax(0, 1fr);
    gap: var(--space-14);
    padding: var(--space-18) var(--space-20) var(--space-20);
  }

  .head {
    display: grid;
    grid-template-columns: minmax(0, 1fr) var(--reading-width);
    align-items: center;
    gap: var(--space-18);
  }

  .title {
    margin: 0;
    font-size: var(--text-pane-title);
    font-weight: var(--font-weight-semibold);
    letter-spacing: var(--tracking-title);
  }

  .panes {
    min-height: 0;
    display: grid;
    grid-template-columns: minmax(0, 1fr) var(--reading-width);
    gap: var(--space-18);
  }

  .panes.bottom {
    grid-template-columns: minmax(0, 1fr);
    grid-template-rows: minmax(0, 2fr) minmax(0, 3fr);
  }

  .panes.off {
    grid-template-columns: minmax(0, 1fr);
  }

  .full {
    min-height: 0;
    display: grid;
  }

  .small {
    --tint: var(--color-button-hover);
    justify-self: start;
    height: var(--button-height-small);
    padding: 0 var(--space-10);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-small-button);
    background: var(--color-secondary-bg);
    color: var(--color-secondary-fg);
    font-size: var(--text-meta);
    font-weight: var(--font-weight-medium);
  }

  /* On paper only the print view shows (section 18.8). */
  @media print {
    .screen {
      display: none;
    }
  }

  .status {
    grid-column: 1 / -1;
  }
</style>
