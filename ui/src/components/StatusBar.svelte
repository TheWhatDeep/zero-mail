<script lang="ts">
  // Status bar (section 11): one sync dot per account, the pending-change count when there
  // is one (ops arrive in M6), a warning when an account has been backing off for more than
  // a minute, with its last error, which opens the log (section 11, States), and the real
  // target of the link under the pointer (section 8, Links).
  import { onMount } from "svelte";
  import { commands, type AccountState } from "../lib/ipc";
  import { BACKOFF_WARNING_MS, app } from "../state/app.svelte";
  import { errorText, stateText, strings } from "../strings";

  let now = $state(Date.now());

  const DOT: Record<AccountState, string> = {
    idle: "idle",
    syncing: "busy",
    connecting: "busy",
    backoff: "waiting",
    needs_attention: "attention",
    disabled: "off",
  };

  const warnings = $derived(
    app.accounts.filter((a) => {
      const since = app.backoffSince[a.id];
      return a.state === "backoff" && since !== undefined && now - since > BACKOFF_WARNING_MS;
    }),
  );

  onMount(() => {
    const t = setInterval(() => (now = Date.now()), 5_000);
    return () => clearInterval(t);
  });
</script>

<footer class="status" aria-label={strings.statusLabel}>
  <ul class="dots">
    {#each app.accounts as a (a.id)}
      <li class="account" title={`${a.email}: ${stateText[a.state]}`}>
        <span class={`dot ${DOT[a.state]}`} aria-hidden="true"></span>
        <span class="visually-hidden">{a.email}: {stateText[a.state]}</span>
      </li>
    {/each}
  </ul>
  {#each warnings as a (a.id)}
    <button class="warning" title={strings.openLog} onclick={() => void commands["log.open"]().catch(() => {})}>
      {strings.backoffWarning(a.name || a.email, a.last_error ? errorText[a.last_error.code] : stateText.backoff)}
    </button>
  {/each}
  {#if app.pending > 0}
    <span class="pending" role="status">{strings.pending(app.pending)}</span>
  {/if}
  {#if app.hoveredLink}
    <span class="link" aria-label={strings.linkTarget}>{app.hoveredLink}</span>
  {/if}
</footer>

<style>
  .status {
    height: var(--status-bar-height);
    display: flex;
    align-items: center;
    gap: var(--space-16);
    padding: 0 var(--space-16);
    border-top: 1px solid var(--color-border-on-glass);
    background: var(--color-surface-main);
    font-size: var(--text-meta-small);
    color: var(--color-text-muted-strong);
  }

  .dots {
    display: flex;
    gap: var(--space-8);
    list-style: none;
    margin: 0;
    padding: 0;
  }

  .account {
    display: inline-flex;
  }

  .dot {
    width: var(--dot-status);
    height: var(--dot-status);
    border-radius: var(--radius-round);
  }

  .idle {
    background: var(--color-status-idle);
  }

  .busy {
    background: var(--color-status-busy);
  }

  .waiting {
    background: var(--color-status-waiting);
  }

  .attention {
    background: var(--color-status-attention);
  }

  .off {
    border: 1px solid var(--color-status-off);
  }

  .warning {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
    padding: 0;
    border: 0;
    background: none;
    font-size: inherit;
    color: inherit;
    cursor: pointer;
  }

  .pending {
    white-space: nowrap;
  }

  .link {
    margin-left: auto;
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
</style>
