<script lang="ts">
  // Settings (section 11): Accounts, Identities and signatures, Reading, Composing,
  // Notifications, Privacy (remote image allowlist), Rules, Templates, Sync and storage,
  // Shortcuts (read-only) and About. Every value goes through settings.set, whose registry
  // in Core checks it (SPEC_GAPS G82).
  import { onMount } from "svelte";
  import FolderSettings from "./FolderSettings.svelte";
  import Identities from "./Identities.svelte";
  import RulesSettings from "./RulesSettings.svelte";
  import TemplatesSettings from "./TemplatesSettings.svelte";
  import { isGoogleHost } from "../lib/accountForm";
  import { commands, type AccountView, type SettingValue } from "../lib/ipc";
  import {
    CORE_KEYS,
    GMAIL_LINKS,
    RELEASES_URL,
    SECTIONS,
    UNDO_CHOICES,
    notifyAccountKey,
    signatureHtml,
    type Section,
  } from "../lib/settings";
  import { SHORTCUT_TABLE } from "../lib/shortcuts";
  import { app } from "../state/app.svelte";
  import { shortcutText, stateText, strings } from "../strings";

  const TITLES: Record<Section, string> = {
    accounts: strings.sectionAccounts,
    identities: strings.sectionIdentities,
    reading: strings.sectionReading,
    composing: strings.sectionComposing,
    notifications: strings.sectionNotifications,
    privacy: strings.sectionPrivacy,
    rules: strings.sectionRules,
    templates: strings.sectionTemplates,
    sync: strings.sectionSync,
    shortcuts: strings.sectionShortcuts,
    about: strings.sectionAbout,
  };

  let section = $state<Section>("accounts");
  /** Core-applied settings, by key. */
  let values = $state<Record<string, SettingValue>>({});
  /** Each account's allowlisted senders (Privacy). */
  let allowed = $state<Record<number, string[]>>({});
  /** Signature drafts by account. */
  let signatures = $state<Record<number, string>>({});
  let snoozeFolder = $state("");

  const keys = $derived([...Object.values(CORE_KEYS), ...app.accounts.map((a) => notifyAccountKey(a.id))]);

  async function load(): Promise<void> {
    for (const key of keys) {
      try {
        values[key] = await commands["settings.get"](key);
      } catch {
        // Left out: the control shows its default.
      }
    }
    snoozeFolder = String(values[CORE_KEYS.snoozeFolder] ?? "");
    for (const a of app.accounts) {
      signatures[a.id] ??= a.signature_text;
      try {
        allowed[a.id] = await commands["images.list_allowed"](a.id);
      } catch {
        allowed[a.id] = [];
      }
    }
  }

  async function set(key: string, value: SettingValue): Promise<void> {
    await app.act(async () => {
      values[key] = await commands["settings.set"](key, value);
    });
  }

  const on = (key: string, fallback: boolean): boolean => {
    const v = values[key];
    return typeof v === "boolean" ? v : fallback;
  };

  async function disallow(accountId: number, address: string): Promise<void> {
    if (await app.act(() => commands["images.disallow_sender"](accountId, address))) {
      allowed[accountId] = (allowed[accountId] ?? []).filter((a) => a !== address);
    }
  }

  /** The signature through account.update, keeping every other setting (no new test). */
  async function saveSignature(a: AccountView): Promise<void> {
    const text = signatures[a.id] ?? "";
    const ok = await app.act(() =>
      commands["account.update"](a.id, {
        name: a.name,
        email: a.email,
        imap: a.imap,
        smtp: a.smtp,
        username: a.username,
        auth_kind: a.auth_kind,
        password: null,
        pinned_cert_sha256: a.pinned_cert_sha256,
        save_anyway: true,
        oauth_client_id: null,
        oauth_client_secret: null,
        signature_text: text,
        signature_html: signatureHtml(text),
      }),
    );
    if (ok) {
      app.toast(strings.signatureSaved);
      await app.reloadAccounts();
    }
  }

  async function clearCache(): Promise<void> {
    if (await app.act(() => commands["storage.clear_body_cache"]())) app.toast(strings.clearBodyCacheDone);
  }

  const isGmail = (a: AccountView) => a.auth_kind === "oauth_google" || isGoogleHost(a.imap.host);
  const open = (url: string) => void app.act(() => commands["link.open"](url));

  onMount(() => {
    void load();
  });
</script>

<section class="settings fade-in" aria-label={strings.settings}>
  <nav class="nav" aria-label={strings.settingsSections}>
    <h1 class="title">{strings.settings}</h1>
    <ul>
      {#each SECTIONS as s (s)}
        <li>
          <button class="item tint" class:is-active={section === s} aria-current={section === s ? "page" : undefined} onclick={() => (section = s)}>
            {TITLES[s]}
          </button>
        </li>
      {/each}
    </ul>
    <button class="small tint press" onclick={() => (app.settingsOpen = false)}>{strings.close}</button>
  </nav>

  <div class="content">
    <h2 class="heading">{TITLES[section]}</h2>

    {#if section === "accounts"}
      {#each app.accounts as a (a.id)}
        <div class="group">
          <div class="row">
            <span class="grow"><strong>{a.name || a.email}</strong> <span class="muted">{a.email} · {stateText[a.state]}</span></span>
            <label class="check">
              <input type="checkbox" checked={a.enabled} onchange={(e) => app.act(() => commands["account.set_enabled"](a.id, (e.currentTarget as HTMLInputElement).checked))} />
              {strings.accountEnabled}
            </label>
            <button class="small tint press" onclick={() => ((app.settingsOpen = false), (app.form = { mode: "edit", account: a }))}>{strings.editAccount}</button>
          </div>
          {#if isGmail(a)}
            <p class="muted">{strings.providerSettings}: {strings.providerSettingsNote}</p>
            <div class="links">
              {#each Object.entries(GMAIL_LINKS) as [k, url] (k)}
                <button class="link" onclick={() => open(url)}>{strings.providerLinks[k as keyof typeof GMAIL_LINKS]}</button>
              {/each}
            </div>
          {/if}
        </div>
      {/each}
      <button class="small tint press" onclick={() => ((app.settingsOpen = false), (app.form = { mode: "add" }))}>{strings.addAccount}</button>
    {:else if section === "identities"}
      {#each app.accounts as a (a.id)}
        <div class="group">
          <h3 class="sub">{a.name || a.email}</h3>
          <Identities accountId={a.id} />
          <label class="field">
            <span>{strings.signature}</span>
            <textarea rows="4" bind:value={signatures[a.id]}></textarea>
            <span class="muted">{strings.signatureHint}</span>
          </label>
          <button class="small tint press" onclick={() => saveSignature(a)}>{strings.save}</button>
        </div>
      {/each}
    {:else if section === "reading"}
      <label class="field">
        <span>{strings.theme}</span>
        <select aria-label={strings.theme} value={app.ui.theme} onchange={(e) => app.setUi("theme", (e.currentTarget as HTMLSelectElement).value as typeof app.ui.theme)}>
          {#each Object.entries(strings.themeChoices) as [v, t] (v)}<option value={v}>{t}</option>{/each}
        </select>
      </label>
      <label class="field">
        <span>{strings.density}</span>
        <select aria-label={strings.density} value={app.ui.density} onchange={(e) => app.setUi("density", (e.currentTarget as HTMLSelectElement).value as typeof app.ui.density)}>
          {#each Object.entries(strings.densityChoices) as [v, t] (v)}<option value={v}>{t}</option>{/each}
        </select>
      </label>
      <label class="field">
        <span>{strings.readingPane}</span>
        <select aria-label={strings.readingPane} value={app.ui.readingPane} onchange={(e) => app.setUi("readingPane", (e.currentTarget as HTMLSelectElement).value as typeof app.ui.readingPane)}>
          {#each Object.entries(strings.readingPaneChoices) as [v, t] (v)}<option value={v}>{t}</option>{/each}
        </select>
      </label>
      <label class="field">
        <span>{strings.markReadSetting}</span>
        <select aria-label={strings.markReadSetting} value={app.ui.markRead} onchange={(e) => app.setUi("markRead", (e.currentTarget as HTMLSelectElement).value as typeof app.ui.markRead)}>
          {#each Object.entries(strings.markReadChoices) as [v, t] (v)}<option value={v}>{t}</option>{/each}
        </select>
      </label>
      <label class="check">
        <input type="checkbox" checked={app.ui.html} onchange={(e) => app.setUi("html", (e.currentTarget as HTMLInputElement).checked)} />
        {strings.htmlSetting}
      </label>
      <label class="check">
        <input type="checkbox" checked={app.ui.groupByThread} onchange={(e) => app.setUi("groupByThread", (e.currentTarget as HTMLInputElement).checked)} />
        {strings.groupByConversation}
      </label>
    {:else if section === "composing"}
      <label class="field">
        <span>{strings.undoSend}</span>
        <select
          aria-label={strings.undoSend}
          value={String(values[CORE_KEYS.undoSeconds] ?? 10)}
          onchange={(e) => set(CORE_KEYS.undoSeconds, Number((e.currentTarget as HTMLSelectElement).value))}
        >
          {#each UNDO_CHOICES as n (n)}<option value={String(n)}>{strings.undoSeconds(n)}</option>{/each}
        </select>
      </label>
    {:else if section === "notifications"}
      <p class="muted">{strings.notifyNote}</p>
      {#each app.accounts as a (a.id)}
        <label class="check">
          <input type="checkbox" checked={on(notifyAccountKey(a.id), true)} onchange={(e) => set(notifyAccountKey(a.id), (e.currentTarget as HTMLInputElement).checked)} />
          {strings.notifyAccount(a.name || a.email)}
        </label>
      {/each}
      <label class="check">
        <input type="checkbox" checked={on(CORE_KEYS.notifyPreview, false)} onchange={(e) => set(CORE_KEYS.notifyPreview, (e.currentTarget as HTMLInputElement).checked)} />
        {strings.notifyPreview}
      </label>
      <label class="check">
        <input type="checkbox" checked={on(CORE_KEYS.keepInTray, true)} onchange={(e) => set(CORE_KEYS.keepInTray, (e.currentTarget as HTMLInputElement).checked)} />
        {strings.keepInTray}
      </label>
      <label class="check">
        <input type="checkbox" checked={on(CORE_KEYS.autostart, false)} onchange={(e) => set(CORE_KEYS.autostart, (e.currentTarget as HTMLInputElement).checked)} />
        {strings.autostart}
      </label>
    {:else if section === "privacy"}
      <label class="check">
        <input type="checkbox" checked={on(CORE_KEYS.onlineLookups, true)} onchange={(e) => set(CORE_KEYS.onlineLookups, (e.currentTarget as HTMLInputElement).checked)} />
        {strings.onlineLookups}
      </label>
      <p class="muted">{strings.onlineLookupsHint}</p>
      {#each app.accounts as a (a.id)}
        <div class="group">
          <h3 class="sub">{strings.imagesAllowed(a.name || a.email)}</h3>
          {#if (allowed[a.id] ?? []).length === 0}
            <p class="muted">{strings.noAllowedSenders}</p>
          {:else}
            <ul class="list">
              {#each allowed[a.id] ?? [] as addr (addr)}
                <li class="row">
                  <span class="grow">{addr}</span>
                  <button class="small tint press" aria-label={strings.removeSender(addr)} onclick={() => disallow(a.id, addr)}>{strings.remove}</button>
                </li>
              {/each}
            </ul>
          {/if}
        </div>
      {/each}
    {:else if section === "rules"}
      <RulesSettings openLink={open} gmail={app.accounts.some(isGmail)} />
    {:else if section === "templates"}
      <TemplatesSettings />
    {:else if section === "sync"}
      {#each app.accounts as a (a.id)}
        <div class="group">
          <h3 class="sub">{a.name || a.email}</h3>
          <FolderSettings accountId={a.id} />
        </div>
      {/each}
      <label class="check">
        <input type="checkbox" checked={on(CORE_KEYS.subjectMerge, false)} onchange={(e) => set(CORE_KEYS.subjectMerge, (e.currentTarget as HTMLInputElement).checked)} />
        {strings.subjectMerge}
      </label>
      <p class="muted">{strings.subjectMergeHint}</p>
      <label class="field">
        <span>{strings.snoozeFolder}</span>
        <input bind:value={snoozeFolder} onchange={() => set(CORE_KEYS.snoozeFolder, snoozeFolder)} />
      </label>
      <label class="check">
        <input type="checkbox" checked={on(CORE_KEYS.snoozeEndOnReply, true)} onchange={(e) => set(CORE_KEYS.snoozeEndOnReply, (e.currentTarget as HTMLInputElement).checked)} />
        {strings.snoozeEndOnReply}
      </label>
      <div class="group">
        <button class="small tint press" onclick={clearCache}>{strings.clearBodyCache}</button>
        <p class="muted">{strings.clearBodyCacheHint}</p>
      </div>
      <label class="check">
        <input type="checkbox" checked={on(CORE_KEYS.debugLogging, false)} onchange={(e) => set(CORE_KEYS.debugLogging, (e.currentTarget as HTMLInputElement).checked)} />
        {strings.debugLogging}
      </label>
      <button class="small tint press" onclick={() => app.act(() => commands["log.open"]())}>{strings.openLog}</button>
    {:else if section === "shortcuts"}
      <p class="muted">{strings.shortcutsNote}</p>
      <table class="keys">
        <tbody>
          {#each SHORTCUT_TABLE.filter((s) => s.action !== "previous") as s (s.keys)}
            <tr><td><kbd>{s.keys}</kbd></td><td>{shortcutText[s.action]}</td></tr>
          {/each}
        </tbody>
      </table>
    {:else if section === "about"}
      <p><strong>{strings.appName}</strong> · {strings.version(__APP_VERSION__)}</p>
      <p class="muted">{strings.aboutNote}</p>
      <button class="link" onclick={() => open(RELEASES_URL)}>{strings.releases}</button>
    {/if}
  </div>
</section>

<style>
  .settings {
    /* The head and panes rows of the shell (App.svelte). */
    grid-row: span 2;
    min-height: 0;
    display: grid;
    grid-template-columns: var(--settings-nav-width) minmax(0, 1fr);
    gap: var(--space-18);
  }

  .nav {
    display: flex;
    flex-direction: column;
    gap: var(--space-10);
  }

  .nav ul {
    margin: 0;
    padding: 0;
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }

  .title {
    margin: 0;
    font-size: var(--text-pane-title);
    font-weight: var(--font-weight-semibold);
    letter-spacing: var(--tracking-title);
  }

  .item {
    --tint: var(--color-button-hover);
    width: 100%;
    text-align: left;
    padding: var(--space-6) var(--space-10);
    border: 0;
    border-radius: var(--radius-small-button);
    background: transparent;
    color: var(--color-text-body);
  }

  .item.is-active {
    background: var(--color-selected-row);
    color: var(--color-ink);
    font-weight: var(--font-weight-medium);
  }

  .content {
    min-height: 0;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: var(--space-14);
    padding: var(--space-18) var(--space-20);
    background: var(--color-card);
    border: 1px solid var(--color-border-on-glass);
    border-radius: var(--radius-card);
  }

  .heading {
    margin: 0;
    font-size: var(--text-subject);
    font-weight: var(--font-weight-semibold);
  }

  .sub {
    margin: 0;
    font-size: var(--text-base);
    font-weight: var(--font-weight-semibold);
  }

  .group {
    display: flex;
    flex-direction: column;
    gap: var(--space-8);
    padding-bottom: var(--space-14);
    border-bottom: 1px solid var(--color-divider);
  }

  .row {
    display: flex;
    align-items: center;
    gap: var(--space-10);
  }

  .grow {
    flex: 1;
    min-width: 0;
    overflow-wrap: anywhere;
  }

  .muted {
    margin: 0;
    color: var(--color-text-muted);
    font-size: var(--text-meta);
  }

  .field {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
    max-width: var(--reading-width);
  }

  .check {
    display: flex;
    align-items: center;
    gap: var(--space-8);
  }

  .list {
    margin: 0;
    padding: 0;
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: var(--space-6);
  }

  .links {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-10);
  }

  .link {
    padding: 0;
    border: 0;
    background: transparent;
    color: var(--color-accent-700);
    text-decoration: underline;
    align-self: flex-start;
  }

  input:not([type="checkbox"]),
  select,
  textarea {
    padding: var(--space-6) var(--space-8);
    border: 1px solid var(--color-border-input);
    border-radius: var(--radius-small-button);
    background: var(--color-input-bg);
    color: var(--color-ink);
    font: inherit;
  }

  .keys td {
    padding: var(--space-4) var(--space-12) var(--space-4) 0;
  }

  kbd {
    font-family: var(--font-mono);
    font-size: var(--text-meta);
  }

  .small {
    --tint: var(--color-button-hover);
    align-self: flex-start;
    height: var(--button-height-small);
    padding: 0 var(--space-10);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-small-button);
    background: var(--color-secondary-bg);
    color: var(--color-secondary-fg);
    font-size: var(--text-meta);
    font-weight: var(--font-weight-medium);
    white-space: nowrap;
  }
</style>
