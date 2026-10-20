<script lang="ts">
  // An account's Outbox view (section 11, Left and States; SPEC_GAPS G69): what has not gone
  // yet. Failed and `unknown` items carry Retry, Edit and Discard; an `unknown` one tells
  // the user to check Sent first, since it is never retried automatically (section 7).
  // A scheduled send has Cancel (back to its draft, SPEC_GAPS G77) and Send now; sent items
  // stay as history, marked Sent late when they went after their time (section 18.3).
  import Icon from "./Icon.svelte";
  import { commands, type OutboxView } from "../lib/ipc";
  import { app, sentHistory, unsent } from "../state/app.svelte";
  import { errorText, outboxStateText, strings } from "../strings";

  let { accountId }: { accountId: number } = $props();

  const items = $derived(unsent(app.outbox[accountId] ?? []));
  const history = $derived(sentHistory(app.outbox[accountId] ?? []));

  /** Scheduled with Send later: due after the undo window (section 18.3). */
  function scheduled(o: OutboxView): boolean {
    return o.state === "queued" && o.send_after_utc - o.created_at > 30;
  }

  function stuck(o: OutboxView): boolean {
    return o.state === "failed" || o.state === "unknown";
  }

  function when(utc: number): string {
    return new Date(utc * 1000).toLocaleString();
  }
</script>

<section class="outbox" aria-label={strings.outbox}>
  {#if items.length === 0}
    <p class="empty">{strings.outboxEmpty}</p>
  {:else}
    <ul>
      {#each items as o (o.id)}
        <li class="row" class:stuck={stuck(o)}>
          <div class="text">
            <span class="subject">{o.subject || strings.noSubject}</span>
            <span class="meta">{strings.toPrefix} {o.to.join(", ") || strings.noRecipients}</span>
            <span class="meta state">
              {#if stuck(o)}<span class="icon"><Icon name="warning" size="--icon-14" /></span>{/if}
              {[
                outboxStateText[o.state],
                o.state === "unknown" ? strings.checkSent : null,
                o.error && o.state !== "unknown" ? errorText[o.error.code] : null,
                o.error?.addresses && o.error.addresses.length > 0 ? o.error.addresses.join(", ") : null,
                o.state === "queued" || o.state === "retry_wait" ? strings.scheduledFor(when(o.send_after_utc)) : null,
              ]
                .filter((x) => x !== null)
                .join(" · ")}
            </span>
          </div>
          <div class="actions">
            {#if stuck(o)}
              <button class="small tint press" onclick={() => app.act(() => commands["outbox.retry"](o.id))}>{strings.retry}</button>
            {/if}
            {#if scheduled(o)}
              <button class="small tint press" onclick={() => app.act(() => commands["outbox.retry"](o.id))}>{strings.sendNow}</button>
              <button class="small tint press" onclick={() => app.act(() => commands["outbox.undo"](o.id))}>{strings.cancelScheduled}</button>
            {:else if stuck(o) || o.state === "queued"}
              <button class="small tint press" onclick={() => app.act(() => commands["outbox.undo"](o.id))}>{strings.edit}</button>
            {/if}
            {#if o.state !== "sending"}
              <button class="small tint press" onclick={() => app.act(() => commands["outbox.discard"](o.id))}>{strings.discard}</button>
            {/if}
          </div>
        </li>
      {/each}
    </ul>
  {/if}
  {#if history.length > 0}
    <h2 class="heading">{strings.sentHistory}</h2>
    <ul>
      {#each history as o (o.id)}
        <li class="row">
          <div class="text">
            <span class="subject">{o.subject || strings.noSubject}</span>
            <span class="meta">
              {[strings.toPrefix + " " + (o.to.join(", ") || strings.noRecipients), o.late ? strings.sentLate : null]
                .filter((x) => x !== null)
                .join(" · ")}
            </span>
          </div>
        </li>
      {/each}
    </ul>
  {/if}
</section>

<style>
  .outbox {
    min-height: 0;
    overflow-y: auto;
    background: var(--color-card);
    border: 1px solid var(--color-border-on-glass);
    border-radius: var(--radius-card);
    box-shadow: var(--shadow-control);
  }

  .heading {
    margin: 0;
    padding: var(--space-12) var(--space-18) var(--space-4);
    font-size: var(--text-meta);
    font-weight: var(--font-weight-semibold);
    color: var(--color-text-muted-strong);
  }

  .empty {
    margin: 0;
    padding: var(--space-20);
    color: var(--color-text-muted);
  }

  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }

  .row {
    display: flex;
    align-items: center;
    gap: var(--space-12);
    padding: var(--space-12) var(--space-18);
    border-bottom: 1px solid var(--color-divider-row);
  }

  .text {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
  }

  .subject {
    font-weight: var(--font-weight-medium);
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .meta {
    font-size: var(--text-meta);
    color: var(--color-text-muted);
    overflow-wrap: anywhere;
  }

  .icon {
    display: inline-flex;
    vertical-align: middle;
    margin-right: var(--space-4);
  }

  .stuck .state {
    color: var(--color-status-attention);
  }

  .actions {
    display: flex;
    gap: var(--space-6);
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
