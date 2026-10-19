<script lang="ts">
  import { initials } from "../lib/format";
  import type { Address } from "../lib/ipc";

  let { person, size = 28 }: { person: Address; size?: 28 | 32 } = $props();
</script>

<span class="avatar" class:large={size === 32} aria-hidden="true">{initials(person)}</span>

<style>
  .avatar {
    flex: none;
    display: inline-grid;
    place-items: center;
    width: var(--avatar-28);
    height: var(--avatar-28);
    border-radius: var(--radius-round);
    background: var(--color-avatar-bg);
    border: 1px solid var(--color-border);
    color: var(--color-ink-hover);
    font-size: var(--text-avatar-28);
    font-weight: var(--font-weight-semibold);
    line-height: 1;
  }

  .large {
    width: var(--avatar-32);
    height: var(--avatar-32);
    font-size: var(--text-avatar-32);
  }
</style>
