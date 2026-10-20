// Compose rules in the UI (section 7). Core builds and sends; the page parses recipients,
// warns before sending, and lays out the first body: an empty line for the cursor, the
// signature, then the quote (SPEC_GAPS G67: the original's plain text, never its HTML).

import type { Address, DraftContent, Quote } from "./ipc";

/** A mailbox per RFC 5322's common forms: `addr`, `Name <addr>`, `"Name" <addr>`. */
const ADDR = /^[^\s@<>()",;:\\[\]]+@[^\s@<>()",;:\\[\]]+\.[^\s@<>()",;:\\[\]]{2,}$/;

export function isValidAddress(addr: string): boolean {
  return ADDR.test(addr.trim());
}

/** Splits typed or pasted text into addresses, at commas, semicolons and line breaks outside
 * quotes and angle brackets. */
export function parseAddresses(text: string): Address[] {
  const parts: string[] = [];
  let current = "";
  let quoted = false;
  let angle = false;
  for (const c of text) {
    if (c === '"') quoted = !quoted;
    else if (c === "<" && !quoted) angle = true;
    else if (c === ">" && !quoted) angle = false;
    if ((c === "," || c === ";" || c === "\n") && !quoted && !angle) {
      parts.push(current);
      current = "";
    } else {
      current += c;
    }
  }
  parts.push(current);
  return parts
    .map((p) => p.trim())
    .filter((p) => p.length > 0)
    .map((p) => {
      const m = /^(.*?)\s*<([^<>]*)>$/.exec(p);
      if (m) return { name: (m[1] ?? "").trim().replace(/^"(.*)"$/, "$1").trim(), addr: (m[2] ?? "").trim() };
      return { name: "", addr: p };
    });
}

/** Every recipient field parses (invalid chips block sending, section 7). */
export function invalidRecipients(content: Pick<DraftContent, "to" | "cc" | "bcc">): Address[] {
  return [...content.to, ...content.cc, ...content.bcc].filter((a) => !isValidAddress(a.addr));
}

export interface SendWarnings {
  emptySubject: boolean;
  /** The body says attach or attached and nothing is attached (section 7). */
  missingAttachment: boolean;
}

export function sendWarnings(content: Pick<DraftContent, "subject" | "text" | "attachments" | "forward_attachment">): SendWarnings {
  const mentions = /\battach(ed|ment|ments)?\b/i.test(withoutQuote(content.text));
  return {
    emptySubject: content.subject.trim() === "",
    missingAttachment: mentions && content.attachments.length === 0 && content.forward_attachment === null,
  };
}

/** The text the user wrote: quoted lines (`>`) and everything after the signature line go. */
export function withoutQuote(text: string): string {
  const out: string[] = [];
  for (const line of text.split(/\r?\n/)) {
    if (/^--\s*$/.test(line)) break;
    if (line.trimStart().startsWith(">")) continue;
    out.push(line);
  }
  return out.join("\n");
}

/** One block of the first body: a paragraph, or the quote. */
export type Block = { kind: "line"; text: string } | { kind: "quote"; text: string };

export interface QuoteText {
  wrote: (date: string, name: string, addr: string) => string;
  forwardedHeader: string;
  from: string;
  date: string;
  subject: string;
  to: string;
}

function person(a: Address): string {
  return a.name ? `${a.name} <${a.addr}>` : a.addr;
}

/** The first body (section 7): the cursor line, the signature between reply and quote,
 * then `On <local date>, <name> <address> wrote:` and the quoted text, or the forwarded
 * header block and the original text. */
export function firstBody(quote: Quote | null, signature: string, words: QuoteText, localDate: (utc: number) => string): Block[] {
  const blocks: Block[] = [{ kind: "line", text: "" }];
  if (signature.trim()) {
    blocks.push({ kind: "line", text: "-- " });
    for (const line of signature.replace(/\r\n/g, "\n").split("\n")) blocks.push({ kind: "line", text: line });
  }
  if (!quote) return blocks;
  blocks.push({ kind: "line", text: "" });
  const date = localDate(quote.date_utc);
  if (quote.forwarded) {
    blocks.push({ kind: "line", text: words.forwardedHeader });
    blocks.push({ kind: "line", text: `${words.from} ${person(quote.from)}` });
    blocks.push({ kind: "line", text: `${words.date} ${date}` });
    blocks.push({ kind: "line", text: `${words.subject} ${quote.subject}` });
    if (quote.to.length > 0) blocks.push({ kind: "line", text: `${words.to} ${quote.to.map(person).join(", ")}` });
    blocks.push({ kind: "line", text: "" });
    blocks.push({ kind: "quote", text: quote.text });
  } else {
    blocks.push({ kind: "line", text: words.wrote(date, quote.from.name, quote.from.addr) });
    blocks.push({ kind: "quote", text: quote.text });
  }
  return blocks;
}

/** The address a `mailto:` link names (section 8, Links), or null when it names none. */
export function mailtoAddress(href: string): string | null {
  if (!/^mailto:/i.test(href)) return null;
  const path = href.slice("mailto:".length).split("?")[0] ?? "";
  let decoded: string;
  try {
    decoded = decodeURIComponent(path);
  } catch {
    return null;
  }
  const first = parseAddresses(decoded)[0];
  return first ? first.addr : null;
}

/** Turns typed text into a chip, unless it is empty. */
export function chipsFrom(text: string, existing: Address[]): Address[] {
  const added = parseAddresses(text).filter(
    (a) => !existing.some((e) => e.addr.toLowerCase() === a.addr.toLowerCase()),
  );
  return [...existing, ...added];
}
