<script lang="ts">
  // Settings, Templates (section 18.4): rename a template or change its subject, see its
  // text, or remove it. Templates are made from a draft with Save as template, so their
  // formatting stays as composed (SPEC_GAPS G82).
  import { onMount } from "svelte";
  import { commands, type TemplateView } from "../lib/ipc";
  import { app } from "../state/app.svelte";
  import { strings } from "../strings";

  let templates = $state<TemplateView[]>([]);

  async function run(action: () => Promise<TemplateView[]>): Promise<void> {
    let list: TemplateView[] = [];
    if (await app.act(async () => (list = await action()))) templates = list;
  }

  onMount(() => {
    void run(() => commands["templates.list"]());
  });
</script>

<p class="muted">{strings.templateEditNote}</p>
{#if templates.length === 0}
  <p class="muted">{strings.noTemplates}</p>
{:else}
  {#each templates as t (t.id)}
    <div class="group">
      <label class="field"><span>{strings.templateName}</span><input bind:value={t.name} /></label>
      <label class="field"><span>{strings.templateSubject}</span><input bind:value={t.subject} /></label>
      <div class="field">
        <span>{strings.templateBody}</span>
        <pre class="text">{t.body_text}</pre>
      </div>
      <div class="row">
        <button class="small tint press" onclick={() => run(() => commands["templates.save"]($state.snapshot(t)))}>{strings.save}</button>
        <button class="small tint press" onclick={() => run(() => commands["templates.delete"](t.id))}>{strings.remove}</button>
      </div>
    </div>
  {/each}
{/if}

<style>
  .muted {
    margin: 0;
    color: var(--color-text-muted);
    font-size: var(--text-meta);
  }

  .group {
    display: flex;
    flex-direction: column;
    gap: var(--space-8);
    padding-bottom: var(--space-14);
    border-bottom: 1px solid var(--color-divider);
  }

  .field {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
    max-width: var(--reading-width);
  }

  .text {
    margin: 0;
    max-height: var(--template-preview-height);
    overflow: auto;
    padding: var(--space-8);
    border-radius: var(--radius-small-button);
    background: var(--color-surface-sunken);
    font: inherit;
    white-space: pre-wrap;
  }

  .row {
    display: flex;
    gap: var(--space-8);
  }

  input {
    padding: var(--space-6) var(--space-8);
    border: 1px solid var(--color-border-input);
    border-radius: var(--radius-small-button);
    background: var(--color-input-bg);
    color: var(--color-ink);
    font: inherit;
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
