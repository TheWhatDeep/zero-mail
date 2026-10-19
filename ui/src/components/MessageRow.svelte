<script lang="ts">
  // One list row (section 11; design 3a MessageRow): unread dot, avatar, sender (recipients
  // in Sent and Drafts), subject with preview, star and attachment indicators, date.
  import Avatar from "./Avatar.svelte";
  import Icon from "./Icon.svelte";
  import { rowDate, senderLabel, subjectText } from "../lib/format";
  import type { Role, Row } from "../lib/ipc";
  import { strings } from "../strings";

  let {
    row,
    role,
    selected,
    top,
    now,
    onselect,
  }: { row: Row; role: Role | null; selected: boolean; top: number; now: Date; onselect: (row: Row) => void } =
    $props();

  const sent = $derived(role === "sent" || role === "drafts");
</script>

<!-- Keys are handled by the listbox (MessageList), as in any ARIA listbox. -->
<!-- svelte-ignore a11y_click_events_have_key_events -->
<div
  id={`row-${row.id}`}
  class="row tint"
  class:unread={row.unread}
  role="option"
  aria-selected={selected}
  tabindex="-1"
  style:transform={`translateY(${top}px)`}
  onclick={() => onselect(row)}
>
  <span class="dot" aria-hidden="true"></span>
  {#if row.unread}<span class="visually-hidden">{strings.unread}</span>{/if}
  <Avatar person={sent ? (row.to[0] ?? row.from) : row.from} />
  <span class="sender">{senderLabel(row, role)}</span>
  <span class="text">
    <span class="subject">{subjectText(row.subject)}</span>
    {#if row.preview}<span class="preview"> — {row.preview}</span>{/if}
  </span>
  <span class="marks">
    {#if row.flagged}<span class="star"><Icon name="starFill" size="--icon-14" label={strings.starred} /></span>{/if}
    {#if row.has_attach}<Icon name="paperclip" size="--icon-14" label={strings.hasAttachment} />{/if}
  </span>
  <span class="date">{rowDate(row.date_utc, now)}</span>
</div>

<style>
  .row {
    --tint: var(--color-row-hover);
    position: absolute;
    left: 0;
    right: 0;
    top: 0;
    height: var(--row-height);
    display: grid;
    grid-template-columns: var(--dot-unread) var(--avatar-28) var(--sender-width) minmax(0, 1fr) auto var(--date-width);
    align-items: center;
    gap: var(--space-12);
    padding: 0 var(--space-16) 0 var(--space-12);
    border-bottom: 1px solid var(--color-divider-row);
    cursor: default;
    color: var(--color-ink);
  }

  .row[aria-selected="true"] {
    --tint: var(--color-selected-row);
  }

  .dot {
    width: var(--dot-unread);
    height: var(--dot-unread);
    border-radius: var(--radius-round);
  }

  .unread .dot {
    background: var(--color-unread-dot);
  }

  .sender {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
    color: var(--color-text-muted-strong);
  }

  .unread .sender,
  .unread .subject {
    color: var(--color-ink);
    font-weight: var(--font-weight-semibold);
  }

  .text {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .preview {
    color: var(--color-text-muted);
  }

  .row[aria-selected="true"] .preview,
  .row[aria-selected="true"] .date,
  .row[aria-selected="true"] .sender {
    color: var(--color-text-muted-on-selected);
  }

  .marks {
    display: flex;
    gap: var(--space-6);
    color: var(--color-icon);
  }

  .star {
    color: var(--color-star);
  }

  .date {
    text-align: right;
    font-size: var(--text-meta);
    color: var(--color-text-muted);
    white-space: nowrap;
  }
</style>
