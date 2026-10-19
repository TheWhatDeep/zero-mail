<script lang="ts">
  // M0 shell: an empty window on the main surface. The three-pane UI arrives in M4.
  import { strings } from "./strings";
</script>

<main class="shell fade-in" aria-label={strings.appName}></main>

<style>
  .shell {
    display: grid;
    grid-template-columns: var(--sidebar-width) 1fr;
    height: 100vh;
    background: var(--color-surface-main);
  }
</style>
