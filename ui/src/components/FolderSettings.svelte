<script lang="ts">
  // An account's folders in its settings (section 9, Folder list; section 5, step 5): every
  // folder, hidden ones too, with its role, the sync and watch toggles, rename and delete
  // (special-use folders cannot be deleted, INBOX cannot be renamed), and New folder. Each
  // change goes through Core and shows at once.
  import { folderEditable } from "../lib/actions";
  import { commands, type FolderView, type Role } from "../lib/ipc";
  import { app } from "../state/app.svelte";
  import { roleText, strings } from "../strings";

  let { accountId }: { accountId: number } = $props();

  const ROLES: Role[] = ["inbox", "drafts", "sent", "archive", "junk", "trash", "all"];

  let newName = $state("");
  let renaming = $state<number | null>(null);
  let renameTo = $state("");
  let confirmDelete = $state<number | null>(null);

  const folders = $derived(app.folders[accountId] ?? []);

  async function refresh(): Promise<void> {
    await app.reloadFolders(accountId);
  }

  async function run(action: () => Promise<unknown>): Promise<void> {
    if (await app.act(action)) await refresh();
  }

  function startRename(f: FolderView): void {
    renaming = f.id;
    renameTo = f.path;
    confirmDelete = null;
  }
</script>

<section class="card pop-in" aria-label={strings.foldersTitle}>
  <h2 class="title">{strings.foldersTitle}</h2>
  <table>
    <thead>
      <tr>
        <th scope="col">{strings.folderName}</th>
        <th scope="col">{strings.roleLabel}</th>
        <th scope="col">{strings.syncToggle}</th>
        <th scope="col">{strings.watchToggle}</th>
        <th scope="col"><span class="visually-hidden">{strings.actionsLabel}</span></th>
      </tr>
    </thead>
    <tbody>
      {#each folders as f (f.id)}
        {@const edit = folderEditable(f)}
        <tr>
          <td class="name">
            {#if renaming === f.id}
              <input
                aria-label={strings.rename}
                bind:value={renameTo}
                onkeydown={(e) => {
                  if (e.key === "Enter") {
                    renaming = null;
                    void run(() => commands["folder.rename"](accountId, f.path, renameTo));
                  } else if (e.key === "Escape") {
                    renaming = null;
                  }
                }}
              />
            {:else}
              {f.path}
            {/if}
          </td>
          <td>
            <select
              aria-label={`${strings.roleLabel}: ${f.path}`}
              value={f.role ?? ""}
              onchange={(e) => {
                const v = (e.currentTarget as HTMLSelectElement).value;
                void run(() => commands["folder.set_role"](accountId, f.path, v === "" ? null : (v as Role)));
              }}
            >
              <option value="">{strings.noRole}</option>
              {#each ROLES as r (r)}
                <option value={r}>{roleText[r]}</option>
              {/each}
            </select>
          </td>
          <td>
            <input
              type="checkbox"
              aria-label={`${strings.syncToggle}: ${f.path}`}
              checked={f.sync_enabled}
              onchange={(e) => run(() => commands["folder.set_sync"](accountId, f.path, (e.currentTarget as HTMLInputElement).checked))}
            />
          </td>
          <td>
            <input
              type="checkbox"
              aria-label={`${strings.watchToggle}: ${f.path}`}
              checked={f.watch_idle || f.role === "inbox"}
              disabled={f.role === "inbox"}
              onchange={(e) => run(() => commands["folder.set_watch"](accountId, f.path, (e.currentTarget as HTMLInputElement).checked))}
            />
          </td>
          <td class="actions">
            {#if confirmDelete === f.id}
              <span class="meta">{strings.deleteFolderConfirm(f.path)}</span>
              <button type="button" class="button tint press" onclick={() => (confirmDelete = null)}>{strings.cancel}</button>
              <button
                type="button"
                class="button tint press"
                onclick={() => {
                  confirmDelete = null;
                  void run(() => commands["folder.delete"](accountId, f.path));
                }}>{strings.deleteFolder}</button
              >
            {:else}
              {#if edit.rename}
                <button type="button" class="button tint press" onclick={() => startRename(f)}>{strings.rename}</button>
              {/if}
              {#if edit.remove}
                <button type="button" class="button tint press" onclick={() => ((confirmDelete = f.id), (renaming = null))}>
                  {strings.deleteFolder}
                </button>
              {/if}
            {/if}
          </td>
        </tr>
      {/each}
    </tbody>
  </table>
  <div class="new">
    <label class="field">
      <span>{strings.newFolder}</span>
      <input placeholder={strings.newFolderHint} bind:value={newName} />
    </label>
    <button
      type="button"
      class="button tint press"
      disabled={newName.trim() === ""}
      onclick={() => {
        const name = newName.trim();
        newName = "";
        void run(() => commands["folder.create"](accountId, name));
      }}>{strings.create}</button
    >
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

  .name {
    overflow-wrap: anywhere;
  }

  .actions {
    text-align: right;
    white-space: nowrap;
  }

  .meta {
    color: var(--color-text-muted-strong);
    white-space: normal;
  }

  input:not([type="checkbox"]),
  select {
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
    align-items: flex-end;
    gap: var(--space-8);
    margin-top: var(--space-14);
  }

  .field {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
    font-size: var(--text-meta);
    color: var(--color-text-muted-strong);
  }
</style>
