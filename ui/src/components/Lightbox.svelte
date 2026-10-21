<script lang="ts">
  // The image lightbox (section 18.9): an image attachment up to 20 MB, served by mail-cid
  // (`<message>/attachment/<id>`), with Save and Open externally. Esc closes (app.close).
  import { onMount } from "svelte";
  import { commands } from "../lib/ipc";
  import { previewUrl } from "../lib/m10";
  import { platformUrl, usesHttpScheme } from "../lib/protocol";
  import { app } from "../state/app.svelte";
  import { strings } from "../strings";

  let { messageId, attachmentId, name }: { messageId: number; attachmentId: number; name: string } = $props();

  let closeButton: HTMLButtonElement | undefined = $state();
  const http = typeof navigator !== "undefined" && usesHttpScheme(navigator.userAgent);
  const src = $derived(platformUrl(previewUrl(messageId, attachmentId), http));

  onMount(() => closeButton?.focus());
</script>

<div class="scrim fade-in" role="dialog" aria-modal="true" aria-label={strings.previewLabel(name)}>
  <div class="bar">
    <span class="name" title={name}>{name}</span>
    <button class="small tint press" onclick={() => app.act(() => commands["attachment.save"](attachmentId))}>
      {strings.saveAttachment}
    </button>
    <button class="small tint press" onclick={() => app.act(() => commands["attachment.open"](attachmentId))}>
      {strings.openExternally}
    </button>
    <button class="small tint press" bind:this={closeButton} onclick={() => (app.lightbox = null)}>{strings.close}</button>
  </div>
  <!-- The image is the message's own attachment; mail-cid serves image/* only. -->
  <img {src} alt={name} />
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 50;
    display: grid;
    grid-template-rows: auto minmax(0, 1fr);
    gap: var(--space-12);
    padding: var(--space-20);
    background: var(--color-toast-bg);
  }

  .bar {
    display: flex;
    align-items: center;
    gap: var(--space-8);
  }

  .name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
    color: var(--color-toast-fg);
    font-weight: var(--font-weight-medium);
  }

  img {
    justify-self: center;
    align-self: center;
    max-width: 100%;
    max-height: 100%;
    object-fit: contain;
    background: var(--color-message-canvas);
    border-radius: var(--radius-track);
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
  }
</style>
