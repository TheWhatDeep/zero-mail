<script lang="ts">
  // Banners (section 11, States): offline; an account that needs attention, with Sign in
  // again or Review certificate; storage trouble (section 12). Toasts for folder resets
  // (section 12; SPEC_GAPS G37).
  import Icon from "./Icon.svelte";
  import { app } from "../state/app.svelte";
  import { pop } from "../lib/motion";
  import { strings } from "../strings";

  const attention = $derived(app.accounts.filter((a) => a.state === "needs_attention"));
</script>

<div class="banners">
  {#if !app.online}
    <div class="banner" role="status" transition:pop>
      <span class="muted"><Icon name="wifiSlash" size="--icon-16" /></span>
      <span class="text">{strings.offline}</span>
    </div>
  {/if}
  {#each attention as a (a.id)}
    <div class="banner" role="alert" transition:pop>
      <span class="alert"><Icon name="warning" size="--icon-16" /></span>
      <span class="text">{strings.needsAttention(a.name || a.email)}</span>
      <button class="button tint press" onclick={() => (app.form = { mode: "edit", account: a })}>
        {a.last_error?.code === "certificate" ? strings.reviewCertificate : strings.signInAgain}
      </button>
    </div>
  {/each}
  {#if app.storage}
    <div class="banner" role="alert" transition:pop>
      <span class="alert"><Icon name="warningOctagon" size="--icon-16" /></span>
      <span class="text">{app.storage === "db_corrupt" ? strings.storageCorrupt : strings.storageFull}</span>
      <button class="button tint press" onclick={() => (app.storage = null)}>{strings.dismiss}</button>
    </div>
  {/if}
</div>

{#if app.toasts.length > 0}
  <div class="toasts">
    {#each app.toasts as t (t.id)}
      <div class="toast" role="status" transition:pop>
        <span>{t.text}</span>
        <button class="toast-button" onclick={() => app.dismissToast(t.id)} aria-label={strings.dismiss}>
          <Icon name="x" size="--icon-14" />
        </button>
      </div>
    {/each}
  </div>
{/if}

<style>
  .banners {
    display: flex;
    flex-direction: column;
    gap: var(--space-8);
  }

  .banners:empty {
    display: none;
  }

  .banner {
    display: flex;
    align-items: center;
    gap: var(--space-10);
    padding: var(--space-8) var(--space-8) var(--space-8) var(--space-14);
    background: var(--color-card);
    border: 1px solid var(--color-border-on-glass);
    border-radius: var(--radius-input);
    box-shadow: var(--shadow-control);
  }

  .muted {
    display: inline-flex;
    color: var(--color-text-muted);
  }

  .alert {
    display: inline-flex;
    color: var(--color-star);
  }

  .text {
    flex: 1;
  }

  .button {
    --tint: var(--color-button-hover);
    height: var(--button-height-small);
    padding: 0 var(--space-10);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-small-button);
    background: var(--color-secondary-bg);
    color: var(--color-secondary-fg);
    font-size: var(--text-meta);
    font-weight: var(--font-weight-medium);
  }

  .toasts {
    position: fixed;
    left: 50%;
    bottom: var(--space-26);
    transform: translateX(-50%);
    display: flex;
    flex-direction: column;
    gap: var(--space-8);
    z-index: 10;
  }

  .toast {
    display: flex;
    align-items: center;
    gap: var(--space-16);
    padding: var(--space-8) var(--space-8) var(--space-8) var(--space-18);
    border-radius: var(--radius-pill);
    background: var(--color-toast-bg);
    color: var(--color-toast-fg);
    box-shadow: var(--shadow-overlay);
  }

  .toast-button {
    display: inline-grid;
    place-items: center;
    width: var(--icon-button-28);
    height: var(--icon-button-28);
    border-radius: var(--radius-pill);
    border: 1px solid var(--color-toast-button-border);
    background: var(--color-toast-button-bg);
    color: inherit;
  }
</style>
