<script lang="ts">
  // The shell (section 11; design 3a with SPEC_GAPS G4, G6, G9, G45): sidebar | list and
  // reading pane, banners above, status bar below. First run shows only the add-account form.
  import { onMount } from "svelte";
  import AccountForm from "./components/AccountForm.svelte";
  import Banners from "./components/Banners.svelte";
  import MessageList from "./components/MessageList.svelte";
  import Outbox from "./components/Outbox.svelte";
  import ReadingPane from "./components/ReadingPane.svelte";
  import Sidebar from "./components/Sidebar.svelte";
  import StatusBar from "./components/StatusBar.svelte";
  import { app } from "./state/app.svelte";
  import { errorText, strings } from "./strings";

  onMount(() => {
    void app.start();
    return () => app.stop();
  });

  const folder = $derived(app.selectedFolder);
</script>

{#if !app.loaded}
  <main class="blank" aria-label={strings.appName}></main>
{:else if app.startError}
  <main class="blank fade-in"><p class="muted">{errorText[app.startError.code]}</p></main>
{:else if app.accounts.length === 0}
  <main class="first-run fade-in"><AccountForm cancellable={false} /></main>
{:else}
  <div class="shell fade-in">
    <Sidebar />
    <main class="main">
      <Banners />
      {#if app.form}
        <AccountForm account={app.form.mode === "edit" ? app.form.account : null} cancellable={true} />
      {:else if app.outboxAccount !== null}
        <h1 class="title">{strings.outbox}</h1>
        <Outbox accountId={app.outboxAccount} />
      {:else}
        <h1 class="title">{folder?.name ?? ""}</h1>
        <div class="panes">
          {#if folder}
            <MessageList {folder} />
          {:else}
            <div></div>
          {/if}
          <ReadingPane />
        </div>
      {/if}
    </main>
    <div class="status"><StatusBar /></div>
  </div>
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

  .status {
    grid-column: 1 / -1;
  }
</style>
