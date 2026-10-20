<script lang="ts">
  // The search box (section 10): local full-text search over everything cached, in the
  // current account or every account. The list waits 150 ms after the last keystroke, and
  // a newer query replaces the one still running.
  import Icon from "./Icon.svelte";
  import { app } from "../state/app.svelte";
  import { strings } from "../strings";

  let text = $state(app.search?.text ?? "");
  let all = $state(app.search?.allAccounts ?? false);

  // Another view (a folder, All Inboxes) clears the box.
  $effect(() => {
    if (app.search === null) text = "";
  });

  function update(): void {
    app.setSearch(text, all);
  }
</script>

<div class="search" role="search">
  <span class="icon"><Icon name="magnifyingGlass" size="--icon-16" /></span>
  <input
    id="search-box"
    type="search"
    placeholder={strings.searchPlaceholder}
    aria-label={strings.searchLabel}
    bind:value={text}
    oninput={update}
    onkeydown={(e) => {
      if (e.key === "Escape") {
        text = "";
        update();
      }
    }}
  />
  {#if app.accounts.length > 1}
    <label class="all">
      <input type="checkbox" bind:checked={all} onchange={update} />
      <span>{strings.searchAllAccounts}</span>
    </label>
  {/if}
</div>

<style>
  .search {
    display: flex;
    align-items: center;
    gap: var(--space-8);
    min-width: 0;
    height: var(--control-height);
    padding: 0 var(--space-10);
    border: 1px solid var(--color-border-input);
    border-radius: var(--radius-input);
    background: var(--color-input-bg);
  }

  .icon {
    display: inline-flex;
    color: var(--color-text-muted);
  }

  input[type="search"] {
    flex: 1;
    min-width: 0;
    border: 0;
    outline: 0;
    background: transparent;
    color: var(--color-ink);
    font: inherit;
  }

  .all {
    display: flex;
    align-items: center;
    gap: var(--space-4);
    font-size: var(--text-meta);
    color: var(--color-text-muted-strong);
    white-space: nowrap;
  }
</style>
