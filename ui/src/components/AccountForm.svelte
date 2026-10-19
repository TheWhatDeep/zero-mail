<script lang="ts">
  // The minimal add-account form (SPEC_GAPS G43; section 5 setup flow). Email first; resolve
  // fills the server fields (or the manual form when nothing is found); then the password,
  // Test connection, and Save once both halves pass, or Save anyway. A certificate the
  // system does not trust is shown with its fingerprint and can be pinned (section 5,
  // Certificates). Edit mode is the same form; it is how the user signs in again.
  import { untrack } from "svelte";
  import { pop } from "../lib/motion";
  import {
    certificateToPin,
    defaultPort,
    emptyFields,
    fromCandidate,
    fromSummary,
    groupFingerprint,
    passed,
    toConfig,
    validate,
    type FieldError,
    type FormFields,
  } from "../lib/accountForm";
  import { commands, toAppError, type AccountView, type AppError, type Candidate, type Security, type TestReport } from "../lib/ipc";
  import { app } from "../state/app.svelte";
  import { errorText, strings } from "../strings";

  let { account = null, cancellable }: { account?: AccountView | null; cancellable: boolean } = $props();

  // The form starts from the account it was opened with; later prop changes do not reset it.
  const initial = untrack(() => account);
  const editing = initial !== null;
  let step = $state<"email" | "details">(editing ? "details" : "email");
  let fields = $state<FormFields>(initial ? fromSummary(initial) : emptyFields());
  let candidate = $state<Candidate | null>(null);
  let found = $state<boolean | null>(null);
  let busy = $state<"resolve" | "test" | "save" | "remove" | null>(null);
  let report = $state<TestReport | null>(null);
  let error = $state<AppError | null>(initial?.last_error ?? null);
  let confirmRemove = $state(false);
  let showErrors = $state(false);

  const problems = $derived(validate(fields, editing));
  const pinOffer = $derived(
    certificateToPin(report?.imap_error, report?.smtp_error, candidate?.probe_error, error),
  );
  const oauthOnly = $derived(candidate !== null && candidate.auth_methods.length > 0 && !candidate.auth_methods.includes("password"));

  function invalid(f: FieldError): boolean {
    return showErrors && problems.includes(f);
  }

  async function resolve(): Promise<void> {
    showErrors = true;
    if (problems.includes("email")) return;
    busy = "resolve";
    error = null;
    try {
      const candidates = await commands["account.resolve"](fields.email);
      const first = candidates[0] ?? null;
      candidate = first;
      found = first !== null;
      fields = first ? fromCandidate({ ...fields, username: fields.email }, first) : { ...emptyFields(fields.email), name: fields.name };
    } catch (err) {
      error = toAppError(err);
      found = false;
    } finally {
      busy = null;
      showErrors = false;
      step = "details";
    }
  }

  async function test(): Promise<void> {
    showErrors = true;
    if (problems.length > 0) return;
    busy = "test";
    error = null;
    try {
      report = await commands["account.test"](toConfig(fields, false));
    } catch (err) {
      error = toAppError(err);
    } finally {
      busy = null;
    }
  }

  async function save(anyway: boolean): Promise<void> {
    showErrors = true;
    if (problems.length > 0) return;
    busy = "save";
    error = null;
    try {
      if (initial) {
        await commands["account.update"](initial.id, toConfig(fields, anyway));
      } else {
        await commands["account.add"](toConfig(fields, anyway));
      }
      app.form = null;
      await app.reloadAccounts();
    } catch (err) {
      error = toAppError(err);
    } finally {
      busy = null;
    }
  }

  async function remove(): Promise<void> {
    if (!initial) return;
    busy = "remove";
    try {
      await commands["account.remove"](initial.id);
      app.form = null;
      await app.reloadAccounts();
    } catch (err) {
      error = toAppError(err);
    } finally {
      busy = null;
    }
  }

  function trust(hex: string): void {
    fields.pin = hex;
    report = null;
    error = null;
    void test();
  }

  function security(kind: "imap" | "smtp", value: Security): void {
    if (kind === "imap") {
      fields.imapSecurity = value;
      fields.imapPort = defaultPort("imap", value);
    } else {
      fields.smtpSecurity = value;
      fields.smtpPort = defaultPort("smtp", value);
    }
  }

  function why(e: AppError): string {
    return errorText[e.code];
  }
</script>

<div class="wrap">
  <form class="card pop-in" onsubmit={(e) => e.preventDefault()}>
    <h1 class="title">{editing ? strings.editAccountTitle : strings.addAccountTitle}</h1>

    {#if step === "email"}
      <label class="field">
        <span>{strings.emailLabel}</span>
        <!-- svelte-ignore a11y_autofocus -->
        <input type="email" autocomplete="email" autofocus bind:value={fields.email} aria-invalid={invalid("email")} />
      </label>
      <label class="field">
        <span>{strings.nameLabel}</span>
        <input type="text" autocomplete="name" bind:value={fields.name} />
      </label>
      <div class="actions">
        {#if cancellable}
          <button type="button" class="button secondary tint press" onclick={() => (app.form = null)}>{strings.cancel}</button>
        {/if}
        <button type="submit" class="button primary press" disabled={busy !== null} onclick={resolve}>
          {busy === "resolve" ? strings.lookingUp : strings.continue}
        </button>
      </div>
    {:else}
      {#if found === true}<p class="hint">{strings.foundHint}</p>{/if}
      {#if found === false}<p class="hint">{strings.manualHint}</p>{/if}
      {#if oauthOnly}<p class="hint">{strings.oauthLater}</p>{/if}

      <div class="row2">
        <label class="field">
          <span>{strings.emailLabel}</span>
          <input type="email" bind:value={fields.email} aria-invalid={invalid("email")} disabled={editing} />
        </label>
        <label class="field">
          <span>{strings.nameLabel}</span>
          <input type="text" bind:value={fields.name} />
        </label>
      </div>

      {#each [["imap", strings.incoming], ["smtp", strings.outgoing]] as const as [kind, heading] (kind)}
        <fieldset class="server">
          <legend>{heading}</legend>
          <label class="field host">
            <span>{strings.hostLabel}</span>
            {#if kind === "imap"}
              <input type="text" spellcheck="false" bind:value={fields.imapHost} aria-invalid={invalid("imapHost")} />
            {:else}
              <input type="text" spellcheck="false" bind:value={fields.smtpHost} aria-invalid={invalid("smtpHost")} />
            {/if}
          </label>
          <label class="field port">
            <span>{strings.portLabel}</span>
            {#if kind === "imap"}
              <input type="text" inputmode="numeric" bind:value={fields.imapPort} aria-invalid={invalid("imapPort")} />
            {:else}
              <input type="text" inputmode="numeric" bind:value={fields.smtpPort} aria-invalid={invalid("smtpPort")} />
            {/if}
          </label>
          <label class="field security">
            <span>{strings.securityLabel}</span>
            <select
              value={kind === "imap" ? fields.imapSecurity : fields.smtpSecurity}
              onchange={(e) => security(kind, (e.currentTarget as HTMLSelectElement).value as Security)}
            >
              <option value="tls">{strings.securityTls}</option>
              <option value="starttls">{strings.securityStartTls}</option>
            </select>
          </label>
        </fieldset>
      {/each}

      <div class="row2">
        <label class="field">
          <span>{strings.usernameLabel}</span>
          <input type="text" spellcheck="false" autocomplete="username" bind:value={fields.username} aria-invalid={invalid("username")} />
        </label>
        <label class="field">
          <span>{strings.passwordLabel}</span>
          <input
            type="password"
            autocomplete={editing ? "current-password" : "new-password"}
            placeholder={editing ? strings.passwordKeep : ""}
            bind:value={fields.password}
            aria-invalid={invalid("password")}
          />
        </label>
      </div>

      {#if pinOffer && fields.pin !== pinOffer}
        <div class="certificate" role="alert" transition:pop>
          <strong>{strings.certificateTitle}</strong>
          <p>{strings.certificateBody}</p>
          <div class="fingerprint">
            <span class="meta">{strings.certificateFingerprint}</span>
            <code>{groupFingerprint(pinOffer)}</code>
          </div>
          <button type="button" class="button secondary tint press" onclick={() => trust(pinOffer)}>{strings.trustCertificate}</button>
        </div>
      {:else if fields.pin}
        <p class="hint">{strings.pinned}: <code>{groupFingerprint(fields.pin)}</code></p>
      {/if}

      {#if report}
        <ul class="results" role="status">
          <li class:fail={report.imap_error !== null}>
            {report.imap_error ? strings.imapFailed(why(report.imap_error)) : strings.imapOk(report.folders)}
          </li>
          <li class:fail={report.smtp_error !== null}>
            {report.smtp_error ? strings.smtpFailed(why(report.smtp_error)) : strings.smtpOk}
          </li>
        </ul>
      {/if}
      {#if error && error.code !== "certificate"}
        <p class="error" role="alert">{why(error)}</p>
      {/if}
      {#if showErrors && problems.length > 0}
        <p class="error" role="alert">{strings.fieldInvalid}</p>
      {/if}

      <div class="actions">
        {#if editing}
          {#if confirmRemove}
            <span class="meta">{strings.removeConfirm}</span>
            <button type="button" class="button secondary tint press" onclick={() => (confirmRemove = false)}>{strings.cancel}</button>
            <button type="button" class="button primary press" disabled={busy !== null} onclick={remove}>{strings.removeAccount}</button>
          {:else}
            <button type="button" class="button secondary tint press" onclick={() => (confirmRemove = true)}>{strings.removeAccount}</button>
          {/if}
        {/if}
        <span class="grow"></span>
        {#if cancellable}
          <button type="button" class="button secondary tint press" onclick={() => (app.form = null)}>{strings.cancel}</button>
        {/if}
        <button type="button" class="button secondary tint press" disabled={busy !== null} onclick={test}>
          {busy === "test" ? strings.testing : strings.test}
        </button>
        {#if report && !passed(report)}
          <button type="button" class="button secondary tint press" disabled={busy !== null} onclick={() => save(true)}>
            {strings.saveAnyway}
          </button>
        {/if}
        <button type="submit" class="button primary press" disabled={busy !== null || !passed(report)} onclick={() => save(false)}>
          {strings.save}
        </button>
      </div>
    {/if}
  </form>
</div>

<style>
  .wrap {
    height: 100%;
    overflow-y: auto;
    display: grid;
    place-items: start center;
    padding: var(--space-26) var(--space-20);
  }

  .card {
    width: 100%;
    max-width: var(--form-width);
    display: flex;
    flex-direction: column;
    gap: var(--space-14);
    padding: var(--space-20);
    background: var(--color-card);
    border: 1px solid var(--color-border-on-glass);
    border-radius: var(--radius-card);
    box-shadow: var(--shadow-control);
    user-select: text;
  }

  .title {
    margin: 0;
    font-size: var(--text-pane-title);
    font-weight: var(--font-weight-semibold);
    letter-spacing: var(--tracking-title);
  }

  .hint,
  .meta {
    margin: 0;
    font-size: var(--text-meta);
    color: var(--color-text-muted);
  }

  .row2 {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: var(--space-12);
  }

  .server {
    margin: 0;
    padding: var(--space-12);
    display: grid;
    grid-template-columns: minmax(0, 1fr) var(--port-width) var(--security-width);
    gap: var(--space-10);
    border: 1px solid var(--color-divider);
    border-radius: var(--radius-input);
  }

  legend {
    padding: 0 var(--space-4);
    font-weight: var(--font-weight-medium);
  }

  .field {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
    font-size: var(--text-meta);
    color: var(--color-text-muted-strong);
  }

  input,
  select {
    height: var(--button-height);
    padding: 0 var(--space-10);
    border: 1px solid var(--color-border-input);
    border-radius: var(--radius-button);
    background: var(--color-input-bg);
    color: var(--color-ink);
    font-size: var(--text-base);
  }

  input[aria-invalid="true"] {
    border-color: var(--color-star);
  }

  input:disabled {
    color: var(--color-text-muted);
  }

  .certificate {
    display: flex;
    flex-direction: column;
    gap: var(--space-8);
    padding: var(--space-12);
    border-radius: var(--radius-input);
    background: var(--color-surface-sunken);
    border: 1px solid var(--color-divider);
  }

  .certificate p {
    margin: 0;
    color: var(--color-text-body);
  }

  .fingerprint {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }

  code {
    font-family: var(--font-mono);
    font-size: var(--text-meta);
    overflow-wrap: anywhere;
  }

  .results {
    margin: 0;
    padding: 0;
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
  }

  .fail,
  .error {
    color: var(--color-star);
  }

  .error {
    margin: 0;
  }

  .actions {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: var(--space-8);
  }

  .grow {
    flex: 1;
  }

  .button {
    height: var(--button-height);
    padding: 0 var(--space-14);
    border-radius: var(--radius-button);
    font-weight: var(--font-weight-medium);
  }

  .primary {
    border: 0;
    background: var(--color-primary-bg);
    color: var(--color-primary-fg);
  }

  .primary:hover:not(:disabled) {
    background: var(--color-primary-bg-hover);
  }

  .primary:disabled {
    opacity: var(--disabled-opacity);
  }

  .secondary {
    --tint: var(--color-button-hover);
    border: 1px solid var(--color-border);
    background: var(--color-secondary-bg);
    color: var(--color-secondary-fg);
  }
</style>
