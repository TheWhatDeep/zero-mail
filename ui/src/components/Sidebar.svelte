<script lang="ts">
  // Account and folder sidebar (section 11, left; design 3a SidebarItem and Badge). Special
  // folders come first in the section 9 order, then the rest as a tree (folder.list
  // returns that order). Items of later milestones are not shown (SPEC_GAPS G45).
  import Icon from "./Icon.svelte";
  import type { IconName } from "../lib/icons";
  import { visibleFolders } from "../lib/actions";
  import { commands, type FolderView, type Role } from "../lib/ipc";
  import { app, needsAction } from "../state/app.svelte";
  import { strings } from "../strings";

  const ROLE_ICON: Record<Role, IconName> = {
    inbox: "tray",
    drafts: "fileText",
    sent: "paperPlaneTilt",
    archive: "archive",
    junk: "warningOctagon",
    trash: "trash",
    all: "folderSimple",
  };

  function icon(f: FolderView): IconName {
    return f.role ? ROLE_ICON[f.role] : "folderSimple";
  }

  function active(f: FolderView): boolean {
    return (
      app.outboxAccount === null &&
      !app.unified &&
      !app.search &&
      app.selected?.account_id === f.account_id &&
      app.selected.folder_id === f.id
    );
  }

  /** All Inboxes counts every enabled account's INBOX (section 11). */
  const unifiedUnread = $derived(
    app.accounts
      .filter((a) => a.enabled)
      .reduce((n, a) => n + (app.folders[a.id]?.find((f) => f.role === "inbox")?.unread ?? 0), 0),
  );

  /** Compose from the account in view (section 11, Compose; SPEC_GAPS G63). */
  function compose(): void {
    const account = app.outboxAccount ?? app.selected?.account_id ?? app.accounts[0]?.id;
    if (account !== undefined) void app.act(() => commands["compose.new"](account));
  }
</script>

<nav class="sidebar" aria-label={strings.sidebarLabel}>
  <div class="compose">
    <button class="item tint" onclick={compose}>
      <span class="icon"><Icon name="plus" size="--icon-17" /></span>
      <span class="name">{strings.compose}</span>
    </button>
  </div>
  <div class="accounts">
    <ul class="top">
      <li>
        <button
          class="item tint"
          class:is-active={app.unified && !app.search && app.outboxAccount === null}
          aria-current={app.unified ? "page" : undefined}
          onclick={() => app.openUnified()}
        >
          <span class="icon"><Icon name="tray" size="--icon-17" /></span>
          <span class="name">{strings.allInboxes}</span>
          {#if unifiedUnread > 0}
            <span class="badge" class:solid={app.unified}>{unifiedUnread.toLocaleString("en")}</span>
          {/if}
        </button>
      </li>
    </ul>
    {#each app.accounts as account (account.id)}
      {@const attention = needsAction(app.outbox[account.id] ?? [])}
      {@const open = app.outboxAccount === account.id}
      <section class="account">
        <button
          class="label tint"
          title={account.email}
          onclick={() => (app.form = { mode: "edit", account })}
        >
          {account.name || account.email}
        </button>
        <ul>
          {#each visibleFolders(app.folders[account.id] ?? []) as folder (folder.id)}
            <li>
              <button
                class="item tint"
                class:is-active={active(folder)}
                aria-current={active(folder) ? "page" : undefined}
                style:padding-left={`calc(var(--space-20) + ${folder.depth} * var(--space-16))`}
                onclick={() => app.selectFolder(account.id, folder.id)}
              >
                <span class="icon"><Icon name={icon(folder)} size="--icon-17" /></span>
                <span class="name">{folder.name}</span>
                {#if folder.unread > 0}
                  <span class="badge" class:solid={active(folder)}>{folder.unread.toLocaleString("en")}</span>
                {/if}
              </button>
            </li>
          {/each}
          <li>
            <button
              class="item tint"
              class:is-active={open}
              aria-current={open ? "page" : undefined}
              onclick={() => app.openOutbox(account.id)}
            >
              <span class="icon"><Icon name="paperPlaneTilt" size="--icon-17" /></span>
              <span class="name">{strings.outbox}</span>
              {#if attention > 0}
                <span class="badge alert" title={strings.needsAction(attention)}>{attention.toLocaleString("en")}</span>
              {/if}
            </button>
          </li>
        </ul>
      </section>
    {/each}
  </div>
  <footer class="footer">
    <button class="item tint" onclick={() => (app.form = { mode: "add" })}>
      <span class="icon"><Icon name="plus" size="--icon-17" /></span>
      <span class="name">{strings.addAccount}</span>
    </button>
  </footer>
</nav>

<style>
  .sidebar {
    height: 100%;
    min-height: 0;
    display: flex;
    flex-direction: column;
    background: var(--color-sidebar);
    box-shadow: inset 0 0 0 1px var(--color-sidebar-outline);
    color: var(--color-sidebar-text);
  }

  .compose {
    padding: var(--space-14) 0 0;
  }

  .accounts {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding-top: var(--space-14);
  }

  .account + .account {
    margin-top: var(--space-22);
  }

  .label {
    --tint: var(--color-sidebar-hover);
    display: block;
    width: 100%;
    padding: 0 var(--space-20) var(--space-6);
    border: 0;
    background: transparent;
    text-align: left;
    font-size: var(--text-tag);
    font-weight: var(--font-weight-medium);
    text-transform: uppercase;
    letter-spacing: var(--tracking-label);
    color: var(--color-sidebar-label);
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }

  .item {
    --tint: var(--color-sidebar-hover);
    width: 100%;
    display: flex;
    align-items: center;
    gap: var(--space-12);
    padding: var(--sidebar-row-pad-y) var(--space-20);
    border: 0;
    background: transparent;
    color: inherit;
    text-align: left;
  }

  .item.is-active {
    --tint: var(--color-sidebar-active-row);
    color: var(--color-sidebar-text-strong);
    font-weight: var(--font-weight-medium);
    box-shadow:
      inset 0 1px 0 var(--color-sidebar-active-inset),
      inset 0 -1px 0 var(--color-sidebar-active-inset);
  }

  .icon {
    display: inline-flex;
    opacity: var(--color-sidebar-icon-opacity);
  }

  .name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .badge {
    min-width: var(--badge-min-width);
    padding: 0 var(--space-6);
    border-radius: var(--radius-pill);
    border: 1px solid var(--color-sidebar-badge-border);
    font-size: var(--text-tag);
    text-align: center;
    color: var(--color-sidebar-badge-text);
  }

  .badge.alert {
    border-color: var(--color-status-attention);
    color: var(--color-status-attention);
    font-weight: var(--font-weight-semibold);
  }

  .badge.solid {
    border-color: transparent;
    background: var(--color-sidebar-badge-solid);
    font-weight: var(--font-weight-semibold);
    color: var(--color-sidebar-text-strong);
  }

  .footer {
    padding: var(--space-8) 0;
    border-top: 1px solid var(--color-sidebar-rule);
  }
</style>
