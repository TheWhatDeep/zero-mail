// The minimal add-account form (SPEC_GAPS G43), as plain state and pure functions: resolve
// fills the server fields, Test reports both halves, Save needs both to pass unless the user
// picks Save anyway (section 5, step 4), and a certificate error offers its fingerprint to
// pin (SPEC_GAPS G34). Google sign-in takes the user's own client ID and secret instead of a
// password; the account is saved first and signed in second (SPEC_GAPS G70, G71).

import type { AccountConfig, AccountSummary, AppError, AuthKind, Candidate, Endpoint, Security, TestReport } from "./ipc";

export interface FormFields {
  name: string;
  email: string;
  imapHost: string;
  imapPort: string;
  imapSecurity: Security;
  smtpHost: string;
  smtpPort: string;
  smtpSecurity: Security;
  username: string;
  method: AuthKind;
  password: string;
  clientId: string;
  clientSecret: string;
  pin: string | null;
}

/** Defaults from section 5: IMAP 993 implicit TLS, SMTP 465 implicit TLS. */
export function emptyFields(email = ""): FormFields {
  return {
    name: "",
    email,
    imapHost: "",
    imapPort: "993",
    imapSecurity: "tls",
    smtpHost: "",
    smtpPort: "465",
    smtpSecurity: "tls",
    username: email,
    method: "password",
    password: "",
    clientId: "",
    clientSecret: "",
    pin: null,
  };
}

export function fromCandidate(fields: FormFields, c: Candidate): FormFields {
  return {
    ...fields,
    imapHost: c.imap.host,
    imapPort: String(c.imap.port),
    imapSecurity: c.imap.security,
    smtpHost: c.smtp.host,
    smtpPort: String(c.smtp.port),
    smtpSecurity: c.smtp.security,
    username: c.username,
    // Section 5, step 3: the login method the host offers first (OAuth for Google).
    method: c.auth_methods[0] ?? "password",
  };
}

export function fromSummary(a: AccountSummary): FormFields {
  return {
    name: a.name,
    email: a.email,
    imapHost: a.imap.host,
    imapPort: String(a.imap.port),
    imapSecurity: a.imap.security,
    smtpHost: a.smtp.host,
    smtpPort: String(a.smtp.port),
    smtpSecurity: a.smtp.security,
    username: a.username,
    method: a.auth_kind,
    password: "",
    clientId: a.oauth_client_id,
    clientSecret: "",
    pin: a.pinned_cert_sha256,
  };
}

/** Google's IMAP hosts (section 5, step 3); mirrors `auth_methods` in mail-auth. */
export function isGoogleHost(host: string): boolean {
  const h = host.trim().replace(/\.$/, "").toLowerCase();
  return ["gmail.com", "googlemail.com"].some((d) => h === d || h.endsWith(`.${d}`));
}

/** The login methods to offer: the found candidate's (checked against the server's
 * capabilities) while its IMAP host is in use, otherwise by the typed IMAP host. A Workspace
 * domain resolves to nothing and is typed in by hand (SPEC_GAPS G27). */
export function methodsFor(candidate: Candidate | null, imapHost: string): AuthKind[] {
  if (candidate && candidate.imap.host.toLowerCase() === imapHost.trim().toLowerCase()) {
    return candidate.auth_methods;
  }
  return isGoogleHost(imapHost) ? ["oauth_google", "password"] : ["password"];
}

/** The usual port for a security mode (section 5, Transport security). */
export function defaultPort(kind: "imap" | "smtp", security: Security): string {
  if (kind === "imap") return security === "tls" ? "993" : "143";
  return security === "tls" ? "465" : "587";
}

export type FieldError =
  | "email"
  | "imapHost"
  | "imapPort"
  | "smtpHost"
  | "smtpPort"
  | "username"
  | "password"
  | "clientId"
  | "clientSecret";

const EMAIL = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;
const HOST = /^[^\s/:?#]+$/;

function portOk(p: string): boolean {
  const n = Number(p);
  return Number.isInteger(n) && n >= 1 && n <= 65535;
}

/** Field problems; a new account needs its password (or Google client ID and secret), an
 * edit may keep the saved ones. */
export function validate(f: FormFields, editing: boolean): FieldError[] {
  const out: FieldError[] = [];
  if (!EMAIL.test(f.email.trim())) out.push("email");
  if (!HOST.test(f.imapHost.trim())) out.push("imapHost");
  if (!portOk(f.imapPort)) out.push("imapPort");
  if (!HOST.test(f.smtpHost.trim())) out.push("smtpHost");
  if (!portOk(f.smtpPort)) out.push("smtpPort");
  if (!f.username.trim()) out.push("username");
  if (f.method === "password") {
    if (!editing && !f.password) out.push("password");
  } else {
    if (!f.clientId.trim()) out.push("clientId");
    if (!editing && !f.clientSecret.trim()) out.push("clientSecret");
  }
  return out;
}

function endpoint(host: string, port: string, security: Security): Endpoint {
  return { host: host.trim(), port: Number(port), security };
}

export function toConfig(f: FormFields, saveAnyway: boolean): AccountConfig {
  return {
    name: f.name.trim(),
    email: f.email.trim(),
    imap: endpoint(f.imapHost, f.imapPort, f.imapSecurity),
    smtp: endpoint(f.smtpHost, f.smtpPort, f.smtpSecurity),
    username: f.username.trim(),
    auth_kind: f.method,
    password: f.method === "password" && f.password ? f.password : null,
    pinned_cert_sha256: f.pin,
    save_anyway: saveAnyway,
    oauth_client_id: f.method === "oauth_google" ? f.clientId.trim() : null,
    oauth_client_secret: f.method === "oauth_google" && f.clientSecret.trim() ? f.clientSecret.trim() : null,
  };
}

export function passed(r: TestReport | null): boolean {
  return r !== null && r.imap_error === null && r.smtp_error === null;
}

/** A certificate fingerprint the user could pin, from a test report or a probe error. */
export function certificateToPin(...errors: (AppError | null | undefined)[]): string | null {
  for (const e of errors) {
    if (e && e.code === "certificate" && /^[0-9a-f]{64}$/.test(e.message)) return e.message;
  }
  return null;
}

/** Groups a fingerprint in fours for reading aloud and comparing. */
export function groupFingerprint(hex: string): string {
  return (hex.match(/.{1,4}/g) ?? []).join(" ");
}
