// What a click on a link in a message does (section 8, Links). The frame reports the real
// target and the visible text; only http, https, mailto and tel are allowed. mailto opens
// compose (M7; until then nothing happens, SPEC_GAPS G57), the rest go to the system browser
// through link.open. When the visible text looks like a URL whose host differs from the
// real one, the user confirms first and sees both (T21).

export type LinkAction =
  | { kind: "ignore" }
  | { kind: "compose"; href: string }
  | { kind: "open"; href: string }
  | { kind: "confirm"; href: string; shownHost: string; realHost: string };

const ALLOWED = new Set(["http:", "https:", "mailto:", "tel:"]);

/** A host as compared: lower case, without one leading "www.". */
export function comparableHost(host: string): string {
  const h = host.toLowerCase().replace(/\.$/, "");
  return h.startsWith("www.") ? h.slice(4) : h;
}

/** The host the visible text names, when it looks like a URL: an http(s) URL, a bare host,
 * or a host with a path (SPEC_GAPS G57). */
export function hostInText(text: string): string | null {
  const t = text.trim();
  if (/^https?:\/\//i.test(t)) {
    try {
      return new URL(t).hostname || null;
    } catch {
      return null;
    }
  }
  const m = /^((?:[a-z0-9](?:[a-z0-9-]*[a-z0-9])?\.)+[a-z]{2,63})(?:[/:?#]\S*)?$/i.exec(t);
  return m?.[1] ?? null;
}

export function decideLink(href: string, text: string): LinkAction {
  let url: URL;
  try {
    url = new URL(href.trim());
  } catch {
    return { kind: "ignore" };
  }
  if (!ALLOWED.has(url.protocol)) return { kind: "ignore" };
  if (url.protocol === "mailto:") return { kind: "compose", href: url.href };
  if (url.protocol === "tel:") return { kind: "open", href: url.href };
  const shown = hostInText(text);
  if (shown && comparableHost(shown) !== comparableHost(url.hostname)) {
    return { kind: "confirm", href: url.href, shownHost: shown.toLowerCase(), realHost: url.hostname };
  }
  return { kind: "open", href: url.href };
}

/** The mail-body URL for the reader, with the plain-text toggle (SPEC_GAPS G53). */
export function bodyUrl(canonical: string, plain: boolean): string {
  return plain ? `${canonical}&plain=1` : canonical;
}
