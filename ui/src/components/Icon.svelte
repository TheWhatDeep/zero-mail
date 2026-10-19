<script lang="ts">
  // Inline SVG icon (Phosphor, SPEC_GAPS G10). Decorative unless it has a label.
  import { ICONS, type IconName } from "../lib/icons";

  let {
    name,
    size = "--icon-16",
    label = null,
  }: { name: IconName; size?: "--icon-17" | "--icon-16" | "--icon-15" | "--icon-14" | "--icon-12"; label?: string | null } =
    $props();
</script>

<svg
  class="icon"
  viewBox="0 0 256 256"
  fill="currentColor"
  style:width={`var(${size})`}
  style:height={`var(${size})`}
  role={label ? "img" : undefined}
  aria-label={label ?? undefined}
  aria-hidden={label ? undefined : "true"}
  focusable="false"
>
  {#each ICONS[name] as d (d)}
    <path {d} />
  {/each}
</svg>

<style>
  .icon {
    flex: none;
    display: block;
  }
</style>
