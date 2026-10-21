<script lang="ts">
  // Onboarding (section 11): the first run starts with this welcome step, then the
  // add-account form.
  import { onMount } from "svelte";
  import { app } from "../state/app.svelte";
  import { strings } from "../strings";

  let start: HTMLButtonElement | undefined = $state();
  onMount(() => start?.focus());
</script>

<section class="welcome" aria-labelledby="welcome-title">
  <h1 id="welcome-title">{strings.welcomeTitle}</h1>
  <p>{strings.welcomeBody}</p>
  <p class="muted">{strings.welcomePrivacy}</p>
  <button class="primary press" bind:this={start} onclick={() => (app.welcomed = true)}>{strings.getStarted}</button>
</section>

<style>
  .welcome {
    max-width: var(--reading-width);
    margin: 0 auto;
    padding-top: var(--space-26);
    display: flex;
    flex-direction: column;
    gap: var(--space-14);
  }

  h1 {
    margin: 0;
    font-size: var(--text-pane-title);
    font-weight: var(--font-weight-semibold);
    letter-spacing: var(--tracking-title);
  }

  p {
    margin: 0;
    line-height: var(--leading-body);
    color: var(--color-text-body);
  }

  .muted {
    color: var(--color-text-muted);
  }

  .primary {
    align-self: flex-start;
    height: var(--button-height);
    padding: 0 var(--space-16);
    border: 0;
    border-radius: var(--radius-button);
    background: var(--color-primary-bg);
    color: var(--color-primary-fg);
    font-weight: var(--font-weight-medium);
  }
</style>
