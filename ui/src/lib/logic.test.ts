// Tests for the M4 UI logic modules. The components stay thin; what they decide is here.

import { describe, expect, it } from "vitest";
import {
  certificateToPin,
  defaultPort,
  emptyFields,
  fromCandidate,
  groupFingerprint,
  isGoogleHost,
  methodsFor,
  passed,
  toConfig,
  validate,
} from "./accountForm";
import { addressList, formatSize, initials, isToday, nameWithAddress, rowDate, senderLabel, subjectText } from "./format";
import { isAppError, toAppError, type Candidate, type Cursor, type MessageView, type Page, type Row } from "./ipc";
import { PAGE_SIZE, PageCache } from "./pages";
import { platformUrl, usesHttpScheme } from "./protocol";
import { applyOpen, bodyArrived, emptyReader, touches } from "./reader";
import {
  MAX_DOM_ROWS,
  earlierHeaderTop,
  hasEarlierHeader,
  hasTodayHeader,
  rowAt,
  rowTop,
  scrollToReveal,
  totalHeight,
  visibleRange,
  type Layout,
} from "./virtual";

function row(id: number, date: number, extra: Partial<Row> = {}): Row {
  return {
    id,
    account_id: 1,
    folder_id: 1,
    thread_id: id,
    subject: `S${id}`,
    from: { name: "Alice Example", addr: "alice@example.test" },
    to: [{ name: "Bob", addr: "bob@example.test" }],
    date_utc: date,
    unread: true,
    flagged: false,
    has_attach: false,
    preview: "",
    thread: null,
    ...extra,
  };
}

describe("format", () => {
  const now = new Date(2026, 9, 20, 15, 30);
  const at = (d: Date) => Math.floor(d.getTime() / 1000);

  it("dates: time today, Yesterday, day this year, full date otherwise", () => {
    expect(rowDate(at(new Date(2026, 9, 20, 9, 5)), now)).toBe("9:05 AM");
    expect(rowDate(at(new Date(2026, 9, 19, 23, 59)), now)).toBe("Yesterday");
    expect(rowDate(at(new Date(2026, 2, 3, 12)), now)).toBe("Mar 3");
    expect(rowDate(at(new Date(2025, 11, 31, 12)), now)).toBe("Dec 31, 2025");
    expect(isToday(at(new Date(2026, 9, 20, 0, 0)), now)).toBe(true);
    expect(isToday(at(new Date(2026, 9, 19, 23, 59)), now)).toBe(false);
  });

  it("people, initials and the address next to the name", () => {
    expect(initials({ name: "Jürgen Müller-Lüdenscheidt", addr: "j@x" })).toBe("JM");
    expect(initials({ name: "", addr: "zed@example.test" })).toBe("Z");
    expect(initials({ name: "\"Ops\" (bot)", addr: "o@x" })).toBe("OB");
    expect(nameWithAddress({ name: "PayPal", addr: "security@evil.test" })).toBe("PayPal <security@evil.test>");
    expect(nameWithAddress({ name: "", addr: "a@b" })).toBe("a@b");
    expect(nameWithAddress({ name: "", addr: "" })).toBe("(unknown sender)");
    expect(addressList([{ name: "A", addr: "a@x" }, { name: "", addr: "b@x" }])).toBe("A <a@x>, b@x");
    expect(subjectText("  ")).toBe("(no subject)");
  });

  it("Sent and Drafts show recipients; other folders the sender", () => {
    const r = row(1, 0);
    expect(senderLabel(r, "inbox")).toBe("Alice Example");
    expect(senderLabel(r, "sent")).toBe("To: Bob");
    expect(senderLabel({ ...r, to: [] }, "drafts")).toBe("(no recipients)");
    expect(senderLabel({ ...r, from: { name: "", addr: "" } }, null)).toBe("(unknown sender)");
  });

  it("sizes", () => {
    expect(formatSize(512)).toBe("512 B");
    expect(formatSize(1536)).toBe("1.5 KB");
    expect(formatSize(20 * 1024 * 1024)).toBe("20 MB");
  });
});

describe("virtual list layout", () => {
  const l: Layout = { rowHeight: 50, headerHeight: 40, total: 50_000, todayCount: 3 };

  it("places Today and Earlier headers around today's rows", () => {
    expect(hasTodayHeader(l) && hasEarlierHeader(l)).toBe(true);
    expect(rowTop(0, l)).toBe(40);
    expect(rowTop(2, l)).toBe(140);
    expect(earlierHeaderTop(l)).toBe(190);
    expect(rowTop(3, l)).toBe(230);
    expect(totalHeight(l)).toBe(50_000 * 50 + 80);
    expect(rowAt(230, l)).toBe(3);
    expect(rowAt(229, l)).toBe(2);
    expect(rowAt(10_000_000, l)).toBe(49_999);
  });

  it("no today rows: only Earlier; boundary unknown: only Today", () => {
    const none: Layout = { ...l, todayCount: 0 };
    expect(hasTodayHeader(none)).toBe(false);
    expect(rowTop(0, none)).toBe(40);
    expect(earlierHeaderTop(none)).toBe(0);
    const unknown: Layout = { ...l, todayCount: null };
    expect(hasEarlierHeader(unknown)).toBe(false);
    expect(rowTop(1000, unknown)).toBe(40 + 1000 * 50);
    expect(totalHeight({ ...l, total: 0 })).toBe(0);
  });

  it("renders at most 100 rows for any viewport", () => {
    const r = visibleRange(1_000_000, 700, l, 8);
    // 14 visible rows plus 8 of overscan on each side.
    expect(r.last - r.first + 1).toBe(31);
    const huge = visibleRange(0, 100_000, l, 8);
    expect(huge.last - huge.first + 1).toBe(MAX_DOM_ROWS);
    expect(visibleRange(0, 700, { ...l, total: 0 }, 8)).toEqual({ first: 0, last: -1 });
  });

  it("scrolls just enough to reveal a row", () => {
    expect(scrollToReveal(5, 0, 700, l)).toBeNull();
    expect(scrollToReveal(20, 0, 700, l)).toBe(rowTop(20, l) + 50 - 700);
    expect(scrollToReveal(0, 500, 700, l)).toBe(0);
  });
});

describe("keyset page cache", () => {
  /** A folder of n rows, newest first, served by cursor like the store. */
  function folder(n: number, base = 1_000_000) {
    let rows = Array.from({ length: n }, (_, i) => row(n - i, base - i));
    const calls: (Cursor | null)[] = [];
    const fetch = async (cursor: Cursor | null, limit: number): Promise<Page> => {
      calls.push(cursor);
      const start = cursor === null ? 0 : rows.findIndex((r) => r.date_utc < cursor.date_utc || (r.date_utc === cursor.date_utc && r.id < cursor.id));
      const from = start < 0 ? rows.length : start;
      const page = rows.slice(from, from + limit);
      const more = from + limit < rows.length;
      const last = page[page.length - 1];
      return { rows: page, next: more && last ? { date_utc: last.date_utc, id: last.id } : null, unindexed: null };
    };
    return {
      fetch,
      calls,
      prepend: (r: Row) => {
        rows = [r, ...rows];
      },
    };
  }

  it("loads pages in order, never by offset, and keeps only pages near the window", async () => {
    const f = folder(1_000);
    const cache = new PageCache(f.fetch);
    await cache.ensure(0, 30);
    expect(cache.loadedPages()).toEqual([0]);
    expect(cache.row(0)?.id).toBe(1_000);
    await cache.ensure(750, 780);
    expect(cache.row(750)?.id).toBe(250);
    // Pages 0-6 were walked for their cursors; only 5-7 stay (window 7, keep 2 around).
    expect(cache.loadedPages()).toEqual([5, 6, 7]);
    expect(cache.complete).toBe(false);
    await cache.ensure(990, 999);
    expect(cache.complete).toBe(true);
    // Back to the top: page 0 reloads straight from its known start.
    const before = f.calls.length;
    await cache.ensure(0, 10);
    expect(f.calls.length).toBe(before + 1);
    expect(cache.row(0)?.id).toBe(1_000);
  });

  it("refresh keeps pages in place unless mail arrived above them", async () => {
    const f = folder(250);
    const cache = new PageCache(f.fetch);
    await cache.ensure(0, 249);
    expect(cache.loadedPages()).toEqual([0, 1, 2]);
    await cache.refresh();
    expect(cache.loadedPages()).toEqual([0, 1, 2]);
    f.prepend(row(9_999, 2_000_000));
    await cache.refresh();
    expect(cache.row(0)?.id).toBe(9_999);
    expect(cache.loadedPages()).toEqual([0]);
    await cache.ensure(0, 250);
    expect(cache.row(PAGE_SIZE)?.id).toBe(151);
    expect(cache.contiguousRows().map((r) => r.id).slice(0, 3)).toEqual([9_999, 250, 249]);
  });
});

describe("reader", () => {
  const view = (id: number, extra: Partial<MessageView> = {}): MessageView => ({
    id,
    account_id: 1,
    folder_id: 2,
    subject: "s",
    from: { name: "", addr: "a@x" },
    to: [],
    cc: [],
    reply_to: [],
    date_utc: 0,
    unread: false,
    flagged: false,
    body_kind: "plain",
    truncated: false,
    has_html: false,
    can_archive: true,
    delete_is_permanent: false,
    labels: null,
    important: false,
    thread_id: id,
    muted: false,
    snoozed_until: null,
    unsubscribe: null,
    body_url: `mail-body://localhost/${id}?images=0`,
    attachments: [],
    remote_images: "none",
    ...extra,
  });

  it("T04: a message deleted on the server keeps its body with the deleted notice", () => {
    const open = applyOpen(emptyReader, 5, { ok: true, view: view(5) });
    expect(open).toEqual({ kind: "open", view: view(5), deleted: false });
    const gone = applyOpen(open, 5, { ok: false, error: { code: "not_found", message: "" } });
    expect(gone).toEqual({ kind: "open", view: view(5), deleted: true });
    // Opening something else that fails is an error, not a deleted notice.
    expect(applyOpen(open, 6, { ok: false, error: { code: "not_found", message: "" } }).kind).toBe("error");
  });

  it("asks again only for changes in its folder and reloads when the body arrives", () => {
    const loading = applyOpen(emptyReader, 5, { ok: true, view: view(5, { body_kind: "loading" }) });
    expect(touches(loading, 1, 2)).toBe(true);
    expect(touches(loading, 1, null)).toBe(true);
    expect(touches(loading, 1, 3)).toBe(false);
    expect(touches(loading, 9, 2)).toBe(false);
    const arrived = applyOpen(loading, 5, { ok: true, view: view(5) });
    expect(bodyArrived(loading, arrived)).toBe(true);
    expect(bodyArrived(arrived, arrived)).toBe(false);
  });
});

describe("protocol URLs", () => {
  it("maps custom schemes to http://<scheme>.localhost on Windows", () => {
    expect(platformUrl("mail-body://localhost/42?images=0", true)).toBe("http://mail-body.localhost/42?images=0");
    expect(platformUrl("mail-body://localhost/42?images=0", false)).toBe("mail-body://localhost/42?images=0");
    expect(usesHttpScheme("Mozilla/5.0 (Windows NT 10.0; Win64; x64) Edg/141")).toBe(true);
    expect(usesHttpScheme("Mozilla/5.0 (Macintosh)")).toBe(false);
  });
});

describe("errors", () => {
  it("only known codes count as AppError", () => {
    expect(isAppError({ code: "auth_failed", message: "x" })).toBe(true);
    expect(isAppError({ code: "nope", message: "x" })).toBe(false);
    expect(toAppError("raw string")).toEqual({ code: "internal", message: "" });
  });
});

describe("account form", () => {
  const candidate: Candidate = {
    imap: { host: "imap.example.test", port: 993, security: "tls" },
    smtp: { host: "smtp.example.test", port: 587, security: "starttls" },
    username: "me",
    source: "autoconfig",
    auth_methods: ["password"],
    probe_error: { code: "certificate", message: "ab".repeat(32) },
  };

  it("fills from a candidate and builds the config with the password once", () => {
    const f = { ...fromCandidate(emptyFields("me@example.test"), candidate), password: "pw" };
    expect(validate(f, false)).toEqual([]);
    const c = toConfig(f, false);
    expect(c.imap).toEqual({ host: "imap.example.test", port: 993, security: "tls" });
    expect(c.smtp.security).toBe("starttls");
    expect(c.username).toBe("me");
    expect(c.password).toBe("pw");
    expect(c.auth_kind).toBe("password");
    expect(toConfig({ ...f, password: "" }, true)).toMatchObject({ password: null, save_anyway: true });
  });

  it("validates fields; an edit may keep the saved password", () => {
    const bad = { ...emptyFields("not an address"), imapPort: "70000", imapHost: "a b" };
    expect(validate(bad, false)).toEqual(["email", "imapHost", "imapPort", "smtpHost", "password"]);
    const edit = { ...fromCandidate(emptyFields("me@example.test"), candidate) };
    expect(validate(edit, true)).toEqual([]);
    expect(defaultPort("imap", "starttls")).toBe("143");
    expect(defaultPort("smtp", "tls")).toBe("465");
  });

  it("Google sign-in takes the client ID and secret instead of a password (G70)", () => {
    const google: Candidate = { ...candidate, auth_methods: ["oauth_google", "password"], probe_error: null };
    const f = fromCandidate(emptyFields("me@workspace.test"), google);
    expect(f.method).toBe("oauth_google");
    expect(validate(f, false)).toEqual(["clientId", "clientSecret"]);
    const filled = { ...f, clientId: " id.apps.googleusercontent.com ", clientSecret: " s3cret ", password: "ignored" };
    expect(validate(filled, false)).toEqual([]);
    expect(toConfig(filled, false)).toMatchObject({
      auth_kind: "oauth_google",
      password: null,
      oauth_client_id: "id.apps.googleusercontent.com",
      oauth_client_secret: "s3cret",
    });
    // An edit keeps the saved secret; the ID stays required.
    expect(validate({ ...filled, clientSecret: "" }, true)).toEqual([]);
    expect(toConfig({ ...filled, clientSecret: "" }, false).oauth_client_secret).toBeNull();
    expect(validate({ ...filled, clientId: "" }, true)).toEqual(["clientId"]);
    // A password account sends no client fields.
    const pw = { ...filled, method: "password" as const, password: "pw" };
    expect(toConfig(pw, false)).toMatchObject({ oauth_client_id: null, oauth_client_secret: null, password: "pw" });
  });

  it("offers Google sign-in by the typed IMAP host when nothing was found", () => {
    // A Workspace domain resolves to nothing; the user types Google's hosts by hand.
    expect(methodsFor(null, "imap.gmail.com")).toEqual(["oauth_google", "password"]);
    expect(methodsFor(null, " IMAP.GoogleMail.com. ")).toEqual(["oauth_google", "password"]);
    expect(methodsFor(null, "imap.fastmail.com")).toEqual(["password"]);
    expect(methodsFor(null, "")).toEqual(["password"]);
    expect(isGoogleHost("notgmail.com")).toBe(false);
    // A found candidate's capability-checked list wins while its host is kept.
    const noXoauth: Candidate = { ...candidate, imap: { ...candidate.imap, host: "imap.gmail.com" }, auth_methods: ["password"] };
    expect(methodsFor(noXoauth, "imap.gmail.com")).toEqual(["password"]);
    expect(methodsFor(candidate, "imap.gmail.com")).toEqual(["oauth_google", "password"]);
  });

  it("offers a certificate fingerprint to pin and needs both halves to pass", () => {
    expect(certificateToPin(null, candidate.probe_error)).toBe("ab".repeat(32));
    expect(certificateToPin({ code: "network", message: "x" })).toBeNull();
    expect(certificateToPin({ code: "certificate", message: "not hex" })).toBeNull();
    expect(groupFingerprint("abcd1234ef")).toBe("abcd 1234 ef");
    expect(passed({ imap_error: null, smtp_error: null, folders: 3 })).toBe(true);
    expect(passed({ imap_error: null, smtp_error: { code: "auth_failed", message: "" }, folders: 3 })).toBe(false);
    expect(passed(null)).toBe(false);
  });
});
