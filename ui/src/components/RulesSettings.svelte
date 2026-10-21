<script lang="ts">
  // Settings, Rules (section 18.6): the ordered list with Enabled, edit, up and down,
  // remove, and Run now; a form for a new or edited rule. Block sender rules show here too.
  import { onMount } from "svelte";
  import { commands, type RuleAction, type RuleField, type RuleView } from "../lib/ipc";
  import { app } from "../state/app.svelte";
  import { strings } from "../strings";
  import { GMAIL_LINKS } from "../lib/settings";

  let { openLink, gmail }: { openLink: (url: string) => void; gmail: boolean } = $props();

  const FIELDS: RuleField[] = ["from", "to", "cc", "subject", "list_id", "has_attachment", "body"];
  const ACTIONS: RuleAction["kind"][] = ["move", "label", "star", "mark_read", "mute", "delete", "never_notify"];

  let rules = $state<RuleView[]>([]);
  let editing = $state<RuleView | null>(null);

  const blank = (): RuleView => ({
    id: 0,
    account_id: null,
    name: "",
    enabled: true,
    conditions: { match: "all", items: [{ field: "from", value: "" }] },
    actions: [{ kind: "star" }],
    stop_processing: false,
  });

  async function run(action: () => Promise<RuleView[]>): Promise<boolean> {
    let list: RuleView[] = [];
    const ok = await app.act(async () => (list = await action()));
    if (ok) rules = list;
    return ok;
  }

  function actionOf(kind: RuleAction["kind"]): RuleAction {
    return kind === "move" || kind === "label" ? { kind, folder: "" } : { kind };
  }

  function move(i: number, by: number): void {
    const ids = rules.map((r) => r.id);
    const j = i + by;
    if (j < 0 || j >= ids.length) return;
    [ids[i], ids[j]] = [ids[j] as number, ids[i] as number];
    void run(() => commands["rules.reorder"](ids));
  }

  async function save(): Promise<void> {
    if (!editing) return;
    const rule = $state.snapshot(editing);
    if (await run(() => commands["rules.save"](rule))) editing = null;
  }

  onMount(() => {
    void run(() => commands["rules.list"]());
  });
</script>

<p class="muted">{strings.rulesNote}</p>
{#if gmail}
  <button class="link" onclick={() => openLink(GMAIL_LINKS.filters)}>{strings.gmailFilters}</button>
{/if}

{#if editing}
  {@const r = editing}
  <div class="form">
    <label class="field"><span>{strings.ruleName}</span><input bind:value={r.name} /></label>
    <label class="field">
      <span>{strings.ruleAccount}</span>
      <select
        value={r.account_id === null ? "" : String(r.account_id)}
        onchange={(e) => {
          const v = (e.currentTarget as HTMLSelectElement).value;
          r.account_id = v === "" ? null : Number(v);
        }}
      >
        <option value="">{strings.ruleAllAccounts}</option>
        {#each app.accounts as a (a.id)}<option value={String(a.id)}>{a.name || a.email}</option>{/each}
      </select>
    </label>
    <label class="field">
      <span>{strings.ruleMatch}</span>
      <select bind:value={r.conditions.match}>
        {#each Object.entries(strings.ruleMatchChoices) as [v, t] (v)}<option value={v}>{t}</option>{/each}
      </select>
    </label>
    <fieldset>
      <legend>{strings.ruleConditions}</legend>
      {#each r.conditions.items as c, i (i)}
        <div class="row">
          <select bind:value={c.field}>
            {#each FIELDS as f (f)}<option value={f}>{strings.ruleFieldText[f]}</option>{/each}
          </select>
          {#if c.field !== "has_attachment"}
            <input aria-label={strings.ruleValue} placeholder={strings.ruleValue} bind:value={c.value} />
          {/if}
          <button class="small tint press" onclick={() => r.conditions.items.splice(i, 1)}>{strings.remove}</button>
        </div>
      {/each}
      <button class="small tint press" onclick={() => r.conditions.items.push({ field: "subject", value: "" })}>{strings.addCondition}</button>
    </fieldset>
    <fieldset>
      <legend>{strings.ruleActions}</legend>
      {#each r.actions as a, i (i)}
        <div class="row">
          <select
            value={a.kind}
            onchange={(e) => (r.actions[i] = actionOf((e.currentTarget as HTMLSelectElement).value as RuleAction["kind"]))}
          >
            {#each ACTIONS as k (k)}<option value={k}>{strings.ruleActionText[k]}</option>{/each}
          </select>
          {#if a.kind === "move" || a.kind === "label"}
            <input aria-label={strings.ruleFolder} placeholder={strings.ruleFolder} bind:value={a.folder} />
          {/if}
          <button class="small tint press" onclick={() => r.actions.splice(i, 1)}>{strings.remove}</button>
        </div>
      {/each}
      <button class="small tint press" onclick={() => r.actions.push({ kind: "star" })}>{strings.addAction}</button>
    </fieldset>
    <label class="check"><input type="checkbox" bind:checked={r.stop_processing} />{strings.ruleStop}</label>
    <label class="check"><input type="checkbox" bind:checked={r.enabled} />{strings.ruleEnabled}</label>
    <div class="row">
      <button class="small tint press" onclick={save}>{strings.save}</button>
      <button class="small tint press" onclick={() => (editing = null)}>{strings.cancel}</button>
    </div>
  </div>
{:else}
  <div class="row">
    <button class="small tint press" onclick={() => (editing = blank())}>{strings.newRule}</button>
    <button class="small tint press" onclick={() => run(() => commands["rules.run_now"]())}>{strings.runNow}</button>
  </div>
  {#if rules.length === 0}
    <p class="muted">{strings.noRules}</p>
  {:else}
    <ol class="list">
      {#each rules as rule, i (rule.id)}
        <li class="row">
          <input
            type="checkbox"
            aria-label={strings.ruleEnabled}
            checked={rule.enabled}
            onchange={(e) => run(() => commands["rules.save"]({ ...$state.snapshot(rule), enabled: (e.currentTarget as HTMLInputElement).checked }))}
          />
          <span class="grow">{rule.name}</span>
          <button class="small tint press" onclick={() => (editing = $state.snapshot(rule))}>{strings.edit}</button>
          <button class="small tint press" disabled={i === 0} onclick={() => move(i, -1)}>{strings.moveUp}</button>
          <button class="small tint press" disabled={i === rules.length - 1} onclick={() => move(i, 1)}>{strings.moveDown}</button>
          <button class="small tint press" onclick={() => run(() => commands["rules.delete"](rule.id))}>{strings.remove}</button>
        </li>
      {/each}
    </ol>
  {/if}
{/if}

<style>
  .muted {
    margin: 0;
    color: var(--color-text-muted);
    font-size: var(--text-meta);
  }

  .form,
  fieldset {
    display: flex;
    flex-direction: column;
    gap: var(--space-8);
  }

  fieldset {
    margin: 0;
    padding: var(--space-10) var(--space-12);
    border: 1px solid var(--color-divider);
    border-radius: var(--radius-input);
  }

  .field {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
    max-width: var(--reading-width);
  }

  .row {
    display: flex;
    align-items: center;
    gap: var(--space-8);
  }

  .grow {
    flex: 1;
    min-width: 0;
  }

  .check {
    display: flex;
    align-items: center;
    gap: var(--space-8);
  }

  .list {
    margin: 0;
    padding: 0;
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: var(--space-6);
  }

  .link {
    align-self: flex-start;
    padding: 0;
    border: 0;
    background: transparent;
    color: var(--color-accent-700);
    text-decoration: underline;
  }

  input:not([type="checkbox"]),
  select {
    padding: var(--space-6) var(--space-8);
    border: 1px solid var(--color-border-input);
    border-radius: var(--radius-small-button);
    background: var(--color-input-bg);
    color: var(--color-ink);
    font: inherit;
  }

  .small {
    --tint: var(--color-button-hover);
    align-self: flex-start;
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
</style>
