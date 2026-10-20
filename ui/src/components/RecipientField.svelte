<script lang="ts">
  // A recipient field (section 11, Compose): chips with autocomplete from contacts.suggest
  // (section 10, up to 10, typing debounced). Text turns into chips at a comma, semicolon,
  // Enter or when the field loses focus; an address that does not parse is a red chip,
  // which blocks sending (section 7).
  import Icon from "./Icon.svelte";
  import { chipsFrom, isValidAddress } from "../lib/compose";
  import { commands, type Address } from "../lib/ipc";
  import { strings } from "../strings";

  let {
    label,
    value,
    accountId,
    onchange,
  }: { label: string; value: Address[]; accountId: number; onchange: (list: Address[]) => void } = $props();

  let text = $state("");
  let suggestions = $state<Address[]>([]);
  let active = $state(-1);
  let timer: ReturnType<typeof setTimeout> | undefined;
  const id = `field-${Math.random().toString(36).slice(2)}`;

  function commit(): void {
    if (text.trim() === "") return;
    onchange(chipsFrom(text, value));
    text = "";
    suggestions = [];
  }

  function pick(a: Address): void {
    if (!value.some((v) => v.addr.toLowerCase() === a.addr.toLowerCase())) onchange([...value, a]);
    text = "";
    suggestions = [];
    active = -1;
  }

  function oninput(): void {
    if (/[,;]/.test(text)) {
      commit();
      return;
    }
    clearTimeout(timer);
    const prefix = text.trim();
    if (prefix.length === 0) {
      suggestions = [];
      return;
    }
    timer = setTimeout(() => {
      commands["contacts.suggest"](prefix, accountId)
        .then((list) => {
          if (text.trim() === prefix) {
            suggestions = list.filter((a) => !value.some((v) => v.addr.toLowerCase() === a.addr.toLowerCase()));
            active = suggestions.length > 0 ? 0 : -1;
          }
        })
        .catch(() => (suggestions = []));
    }, 150);
  }

  function onkeydown(e: KeyboardEvent): void {
    if (e.key === "ArrowDown" && suggestions.length > 0) {
      e.preventDefault();
      active = (active + 1) % suggestions.length;
    } else if (e.key === "ArrowUp" && suggestions.length > 0) {
      e.preventDefault();
      active = (active - 1 + suggestions.length) % suggestions.length;
    } else if (e.key === "Enter") {
      const s = suggestions[active];
      if (s) {
        e.preventDefault();
        pick(s);
      } else if (text.trim() !== "") {
        e.preventDefault();
        commit();
      }
    } else if (e.key === "Escape") {
      suggestions = [];
    } else if (e.key === "Backspace" && text === "" && value.length > 0) {
      onchange(value.slice(0, -1));
    }
  }
</script>

<div class="field">
  <label class="label" for={id}>{label}</label>
  <div class="box">
    {#each value as a, i (a.addr + i)}
      <span class="chip" class:invalid={!isValidAddress(a.addr)} title={a.addr}>
        <span class="chip-text">{a.name || a.addr}</span>
        <button class="chip-remove" aria-label={strings.removeRecipient(a.addr)} onclick={() => onchange(value.filter((_, k) => k !== i))}>
          <Icon name="x" size="--icon-12" />
        </button>
      </span>
    {/each}
    <input
      {id}
      autocomplete="off"
      role="combobox"
      aria-expanded={suggestions.length > 0}
      aria-controls={`${id}-list`}
      bind:value={text}
      {oninput}
      {onkeydown}
      onblur={() => setTimeout(commit, 120)}
    />
  </div>
  {#if suggestions.length > 0}
    <ul class="suggestions" id={`${id}-list`} role="listbox" aria-label={label}>
      {#each suggestions as s, i (s.addr)}
        <li role="option" aria-selected={i === active}>
          <button class="suggestion tint" class:is-active={i === active} onmousedown={(e) => (e.preventDefault(), pick(s))}>
            <span class="name">{s.name || s.addr}</span>
            {#if s.name}<span class="addr">{s.addr}</span>{/if}
          </button>
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .field {
    position: relative;
    display: grid;
    grid-template-columns: var(--date-width) minmax(0, 1fr);
    align-items: start;
    gap: var(--space-8);
    padding: var(--space-6) 0;
    border-bottom: 1px solid var(--color-divider);
  }

  .label {
    padding-top: var(--space-6);
    font-size: var(--text-meta);
    color: var(--color-text-muted);
  }

  .box {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-6);
    align-items: center;
    min-height: var(--button-height-small);
  }

  .chip {
    display: inline-flex;
    align-items: center;
    gap: var(--space-4);
    height: var(--button-height-small);
    padding: 0 var(--space-4) 0 var(--space-10);
    border-radius: var(--radius-pill);
    background: var(--color-tag-bg);
    border: 1px solid var(--color-tag-border);
    color: var(--color-tag-fg);
    font-size: var(--text-meta);
  }

  .chip.invalid {
    border-color: var(--color-accent-2);
    color: var(--color-accent-2);
  }

  .chip-remove {
    display: inline-grid;
    place-items: center;
    width: var(--icon-button-28);
    height: var(--icon-button-28);
    border: 0;
    background: transparent;
    color: inherit;
  }

  input {
    flex: 1;
    min-width: var(--sender-width);
    height: var(--button-height-small);
    border: 0;
    background: transparent;
    outline: none;
  }

  .suggestions {
    position: absolute;
    left: var(--date-width);
    top: 100%;
    z-index: 5;
    min-width: var(--form-width);
    max-width: 100%;
    margin: 0;
    padding: var(--space-4);
    list-style: none;
    background: var(--color-card);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-input);
    box-shadow: var(--shadow-overlay);
  }

  .suggestion {
    --tint: var(--color-button-hover);
    width: 100%;
    display: flex;
    gap: var(--space-8);
    padding: var(--space-6) var(--space-8);
    border: 0;
    border-radius: var(--radius-small-button);
    background: transparent;
    text-align: left;
  }

  .addr {
    color: var(--color-text-muted);
  }
</style>
