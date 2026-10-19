// Display formatting for list rows and the reader. All times arrive as UTC epoch seconds
// and are shown in local time (section 4, invariant 6). English only (section 11).

import type { Address, Role, Row } from "./ipc";
import { strings } from "../strings";

const DAY_MS = 24 * 60 * 60 * 1000;

/** Local midnight of the day containing `now`. */
export function startOfDay(now: Date): Date {
  return new Date(now.getFullYear(), now.getMonth(), now.getDate());
}

export function isToday(epochSeconds: number, now: Date): boolean {
  return epochSeconds * 1000 >= startOfDay(now).getTime();
}

const time = new Intl.DateTimeFormat("en", { hour: "numeric", minute: "2-digit" });
const monthDay = new Intl.DateTimeFormat("en", { month: "short", day: "numeric" });
const fullDate = new Intl.DateTimeFormat("en", { month: "short", day: "numeric", year: "numeric" });
const readerDate = new Intl.DateTimeFormat("en", {
  weekday: "short",
  month: "short",
  day: "numeric",
  year: "numeric",
  hour: "numeric",
  minute: "2-digit",
});

/** The list row date: a time today, "Yesterday", a day this year, else a full date. */
export function rowDate(epochSeconds: number, now: Date): string {
  const at = new Date(epochSeconds * 1000);
  const today = startOfDay(now).getTime();
  if (at.getTime() >= today) return time.format(at);
  if (at.getTime() >= today - DAY_MS) return strings.yesterday;
  if (at.getFullYear() === now.getFullYear()) return monthDay.format(at);
  return fullDate.format(at);
}

export function readerDateText(epochSeconds: number): string {
  return readerDate.format(new Date(epochSeconds * 1000));
}

/** Up to two initials from a display name, else the first letter of the address. */
export function initials(a: Address): string {
  const words = a.name
    .replace(/["'()<>[\]]/g, " ")
    .split(/\s+/)
    .filter((w) => /\p{L}|\p{N}/u.test(w));
  const letters = words.slice(0, 2).map((w) => [...w][0] ?? "");
  const out = letters.join("") || [...a.addr][0] || "";
  return out.toLocaleUpperCase("en");
}

/** A person as text: the display name, else the address. */
export function personName(a: Address): string {
  return a.name.trim() || a.addr.trim() || strings.unknownSender;
}

/** The list's sender column: recipients in Sent and Drafts (section 11), else the sender. */
export function senderLabel(row: Row, role: Role | null): string {
  if (role === "sent" || role === "drafts") {
    const names = row.to.map(personName);
    return names.length > 0 ? `${strings.toPrefix} ${names.join(", ")}` : strings.noRecipients;
  }
  return personName(row.from);
}

/** `Name <address>`: the address is always shown next to the name (section 13, item 10). */
export function nameWithAddress(a: Address): string {
  const name = a.name.trim();
  const addr = a.addr.trim();
  if (name && addr && name !== addr) return `${name} <${addr}>`;
  return name || addr || strings.unknownSender;
}

export function addressList(list: Address[]): string {
  return list.map(nameWithAddress).join(", ");
}

export function subjectText(subject: string): string {
  return subject.trim() || strings.noSubject;
}

export function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB"];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value < 10 ? value.toFixed(1) : Math.round(value)} ${units[unit] ?? "GB"}`;
}
