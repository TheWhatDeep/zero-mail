// The settings the page reads (section 11, Settings; SPEC_GAPS G82). Core keeps the
// registry with each key's type and default (crates/mail-core/src/settings.rs); this is the
// page's view of the keys it applies itself. The page applies theme and density as
// attributes on the root element, which tokens.css reads.

import type { SettingValue } from "./ipc";

export type Theme = "system" | "light" | "dark";
export type Density = "comfortable" | "compact";
export type ReadingPanePosition = "right" | "bottom" | "off";
export type MarkRead = "delay" | "immediately" | "never";

export interface UiSettings {
  theme: Theme;
  density: Density;
  readingPane: ReadingPanePosition;
  groupByThread: boolean;
  markRead: MarkRead;
  html: boolean;
}

/** Core's defaults, so the page looks right before the first answer. */
export const DEFAULTS: UiSettings = {
  theme: "system",
  density: "comfortable",
  readingPane: "right",
  groupByThread: true,
  markRead: "delay",
  html: true,
};

/** Each page setting and its key in Core's registry. */
export const KEYS: Record<keyof UiSettings, string> = {
  theme: "ui.theme",
  density: "ui.density",
  readingPane: "ui.reading_pane",
  groupByThread: "ui.group_by_thread",
  markRead: "reading.mark_read",
  html: "reading.html",
};

/** Keys the Settings screens change that Core or the app applies. */
export const CORE_KEYS = {
  undoSeconds: "compose.undo_seconds",
  notifyPreview: "notify.preview",
  keepInTray: "app.keep_in_tray",
  autostart: "app.autostart",
  onlineLookups: "privacy.online_lookups",
  debugLogging: "debug_logging",
  subjectMerge: "threads.subject_merge",
  snoozeFolder: "snooze.folder",
  snoozeEndOnReply: "snooze.end_on_reply",
} as const;

export const notifyAccountKey = (accountId: number): string => `notify.accounts.${accountId}`;

const CHOICES: Partial<Record<keyof UiSettings, readonly string[]>> = {
  theme: ["system", "light", "dark"],
  density: ["comfortable", "compact"],
  readingPane: ["right", "bottom", "off"],
  markRead: ["delay", "immediately", "never"],
};

/** Takes Core's answer for one key, keeping the default when it does not fit. */
export function withValue(current: UiSettings, field: keyof UiSettings, value: SettingValue): UiSettings {
  const choices = CHOICES[field];
  if (choices) {
    return typeof value === "string" && choices.includes(value) ? { ...current, [field]: value } : current;
  }
  return typeof value === "boolean" ? { ...current, [field]: value } : current;
}

/** The root element's attributes: `data-theme` absent follows the system (tokens.css). */
export function rootAttributes(s: UiSettings): { theme: string | null; density: string | null } {
  return {
    theme: s.theme === "system" ? null : s.theme,
    density: s.density === "compact" ? "compact" : null,
  };
}

/** How long a shown message waits before it is marked read; null: never (section 8). */
export function markReadDelay(mode: MarkRead, delayMs: number): number | null {
  if (mode === "never") return null;
  return mode === "immediately" ? 0 : delayMs;
}

/** The undo-send window's allowed values in seconds (section 7: 0 to 30). */
export const UNDO_CHOICES = [0, 5, 10, 20, 30] as const;

/** About: where new versions are published (section 11; updates are manual, 13.1). */
export const RELEASES_URL = "https://github.com/TheWhatDeep/zero-mail/releases";

/** Gmail's own pages for what runs on the server (section 18 table: Settings links them). */
export const GMAIL_LINKS = {
  filters: "https://mail.google.com/mail/u/0/#settings/filters",
  vacation: "https://mail.google.com/mail/u/0/#settings/general",
  forwarding: "https://mail.google.com/mail/u/0/#settings/fwdandpop",
  spam: "https://mail.google.com/mail/u/0/#spam",
  security: "https://myaccount.google.com/signinoptions/two-step-verification",
  storage: "https://one.google.com/storage",
} as const;

/** Settings screens, in the section 11 order. */
export const SECTIONS = [
  "accounts",
  "identities",
  "reading",
  "composing",
  "notifications",
  "privacy",
  "rules",
  "templates",
  "sync",
  "shortcuts",
  "about",
] as const;
export type Section = (typeof SECTIONS)[number];

/** A plain-text signature as the HTML part: escaped, one line per line. */
export function signatureHtml(text: string): string {
  const esc = text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
  return esc
    .split(/\r?\n/)
    .map((line) => `<div>${line === "" ? "<br>" : line}</div>`)
    .join("");
}
