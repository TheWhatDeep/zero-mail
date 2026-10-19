// The minimal add-account form (SPEC_GAPS G43), as plain state and pure functions: resolve
// fills the server fields, Test reports both halves, Save needs both to pass unless the user
// picks Save anyway (section 5, step 4), and a certificate error offers its fingerprint to
// pin (SPEC_GAPS G34).

import type { AccountConfig, AccountSummary, AppError, Candidate, Endpoint, Security, TestReport } from "./ipc";

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
  password: string;
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
    password: "",
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
    password: "",
    pin: a.pinned_cert_sha256,
  };
}

/** The usual port for a security mode (section 5, Transport security). */
export function defaultPort(kind: "imap" | "smtp", security: Security): string {
  if (kind === "imap") return security === "tls" ? "993" : "143";
  return security === "tls" ? "465" : "587";
}

export type FieldError = "email" | "imapHost" | "imapPort" | "smtpHost" | "smtpPort" | "username" | "password";

const EMAIL = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;
const HOST = /^[^\s/:?#]+$/;

function portOk(p: string): boolean {
  const n = Number(p);
  return Number.isInteger(n) && n >= 1 && n <= 65535;
}

/** Field problems; a new account needs a password, an edit may keep the saved one. */
export function validate(f: FormFields, editing: boolean): FieldError[] {
  const out: FieldError[] = [];
  if (!EMAIL.test(f.email.trim())) out.push("email");
  if (!HOST.test(f.imapHost.trim())) out.push("imapHost");
  if (!portOk(f.imapPort)) out.push("imapPort");
  if (!HOST.test(f.smtpHost.trim())) out.push("smtpHost");
  if (!portOk(f.smtpPort)) out.push("smtpPort");
  if (!f.username.trim()) out.push("username");
  if (!editing && !f.password) out.push("password");
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
    auth_kind: "password",
    password: f.password ? f.password : null,
    pinned_cert_sha256: f.pin,
    save_anyway: saveAnyway,
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
