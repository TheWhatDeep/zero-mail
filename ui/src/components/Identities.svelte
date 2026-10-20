<script lang="ts">
  // An account's identities in its settings (section 6; SPEC_GAPS G64): display name,
  // address, optional Reply-To; one default, never zero. Extra send-as addresses are typed
  // in by hand. Signatures arrive with Settings (M10).
  import { onMount } from "svelte";
  import { isValidAddress } from "../lib/compose";
  import { commands, type IdentityView } from "../lib/ipc";
  import { app } from "../state/app.svelte";
  import { strings } from "../strings";

  let { accountId }: { accountId: number } = $props();

  let identities = $state<IdentityView[]>([]);
  let name = $state("");
  let address = $state("");
  let replyTo = $state("");

  async function run(action: () => Promise<IdentityView[]>): Promise<void> {
    let list: IdentityView[] = [];
    if (await app.act(async () => (list = await action()))) identities = list;
  }

  function save(i: IdentityView): void {
    void run(() => commands["identity.save"]({ ...$state.snapshot(i), reply_to: i.reply_to?.trim() || null }));
  }

  function add(): void {
    const identity: IdentityView = {
      id: null,
      account_id: accountId,
      name: name.trim(),
      address: address.trim(),
      reply_to: replyTo.trim() || null,
      is_default: false,
    };
    name = "";
    address = "";
    replyTo = "";
    void run(() => commands["identity.save"](identity));
  }

  const canAdd = $derived(isValidAddress(address) && (replyTo.trim() === "" || isValidAddress(replyTo)));

  onMount(() => {
    void run(() => commands["identity.list"](accountId));
  });
</script>

<section class="card pop-in" aria-label={strings.identities}>
  <h2 class="title">{strings.identities}</h2>
  <table>
    <thead>
      <tr>
        <th>{strings.identityName}</th>
        <th>{strings.identityAddress}</th>
        <th>{strings.identityReplyTo}</th>
        <th></th>
      </tr>
    </thead>
    <tbody>
      {#each identities as i (i.id)}
        <tr>
          <td><input aria-label={`${strings.identityName}: ${i.address}`} bind:value={i.name} onchange={() => save(i)} /></td>
          <td class="address">{i.address}</td>
          <td>
            <input
              aria-label={`${strings.identityReplyTo}: ${i.address}`}
              value={i.reply_to ?? ""}
              onchange={(e) => {
                const v = (e.currentTarget as HTMLInputElement).value.trim();
                if (v === "" || isValidAddress(v)) save({ ...i, reply_to: v || null });
              }}
            />
          </td>
          <td class="actions">
            {#if i.is_default}
              <span class="meta">{strings.isDefault}</span>
            {:else}
              <button type="button" class="button tint press" onclick={() => save({ ...i, is_default: true })}>{strings.makeDefault}</button>
              <button
                type="button"
                class="button tint press"
                aria-label={strings.removeIdentity(i.address)}
                onclick={() => i.id !== null && run(() => commands["identity.delete"](i.id ?? 0))}>{strings.delete}</button
              >
            {/if}
          </td>
        </tr>
      {/each}
    </tbody>
  </table>
  <div class="new">
    <input aria-label={strings.identityName} placeholder={strings.identityName} bind:value={name} />
    <input aria-label={strings.identityAddress} placeholder={strings.identityAddress} bind:value={address} />
    <input aria-label={strings.identityReplyTo} placeholder={strings.identityReplyTo} bind:value={replyTo} />
    <button type="button" class="button tint press" disabled={!canAdd} onclick={add}>{strings.addIdentity}</button>
  </div>
</section>

<style>
  .card {
    width: 100%;
    max-width: var(--form-width);
    margin-top: var(--space-16);
    padding: var(--space-20);
    background: var(--color-card);
    border: 1px solid var(--color-border-on-glass);
    border-radius: var(--radius-card);
    box-shadow: var(--shadow-control);
    user-select: text;
  }

  .title {
    margin: 0 0 var(--space-12);
    font-size: var(--text-base);
    font-weight: var(--font-weight-semibold);
  }

  table {
    width: 100%;
    border-collapse: collapse;
    font-size: var(--text-meta);
  }

  th {
    text-align: left;
    font-weight: var(--font-weight-medium);
    color: var(--color-text-muted);
    padding: var(--space-6) var(--space-4);
    border-bottom: 1px solid var(--color-divider);
  }

  td {
    padding: var(--space-6) var(--space-4);
    border-bottom: 1px solid var(--color-divider-row);
    vertical-align: middle;
  }

  .address {
    overflow-wrap: anywhere;
  }

  .actions {
    text-align: right;
    white-space: nowrap;
  }

  .meta {
    color: var(--color-text-muted-strong);
  }

  input {
    width: 100%;
    min-width: 0;
    height: var(--button-height-small);
    padding: 0 var(--space-8);
    border: 1px solid var(--color-border-input);
    border-radius: var(--radius-small-button);
    background: var(--color-input-bg);
  }

  .button {
    --tint: var(--color-button-hover);
    height: var(--button-height-small);
    padding: 0 var(--space-10);
    margin-left: var(--space-4);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-small-button);
    background: var(--color-secondary-bg);
    color: var(--color-secondary-fg);
    font-weight: var(--font-weight-medium);
  }

  .button:disabled {
    opacity: var(--disabled-opacity);
  }

  .new {
    display: flex;
    align-items: center;
    gap: var(--space-8);
    margin-top: var(--space-14);
  }
</style>
