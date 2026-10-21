// M10 page logic: keyboard shortcuts (T34), settings, the notification click (SPEC_GAPS
// G80), print (T44: order and blocked images) and the image preview (18.9).

import { describe, expect, it } from "vitest";
import { CLICK_WINDOW_MS, NotifyClicks, canPreview, previewUrl, printBodyUrl, printOrder } from "./m10";
import { DEFAULTS, markReadDelay, rootAttributes, signatureHtml, withValue } from "./settings";
import { G_WINDOW_MS, SHORTCUT_TABLE, ShortcutDispatcher, isTyping, type KeyPress, type ShortcutAction } from "./shortcuts";

const press = (key: string, more: Partial<KeyPress> = {}): KeyPress => ({
  key,
  ctrl: false,
  meta: false,
  shift: false,
  alt: false,
  typing: false,
  ...more,
});

describe("shortcuts (T34)", () => {
  it("maps every key of the section 11 table", () => {
    const d = new ShortcutDispatcher();
    const cases: [KeyPress, ShortcutAction][] = [
      [press("j"), "next"],
      [press("k"), "previous"],
      [press("Enter"), "open"],
      [press("o"), "open"],
      [press("Escape"), "close"],
      [press("c"), "compose"],
      [press("r"), "reply"],
      [press("a"), "replyAll"],
      [press("f"), "forward"],
      [press("e"), "archive"],
      [press("#", { shift: true }), "delete"],
      [press("Delete"), "delete"],
      [press("s"), "star"],
      [press("b"), "snooze"],
      [press("m"), "mute"],
      [press("U", { shift: true }), "toggleUnread"],
      [press("/"), "search"],
      [press("Enter", { ctrl: true }), "send"],
      [press("Enter", { meta: true }), "send"],
      [press("p", { ctrl: true }), "print"],
      [press("P", { meta: true }), "print"],
      [press(",", { ctrl: true }), "settings"],
    ];
    for (const [k, action] of cases) expect(d.handle(k, 0), JSON.stringify(k)).toBe(action);
  });

  it("g then i, d or s within the window", () => {
    const d = new ShortcutDispatcher();
    expect(d.handle(press("g"), 0)).toBeNull();
    expect(d.handle(press("i"), 100)).toBe("goInbox");
    d.handle(press("g"), 1000);
    expect(d.handle(press("d"), 1200)).toBe("goDrafts");
    d.handle(press("g"), 2000);
    expect(d.handle(press("s"), 2100)).toBe("goSent");
    // Too late: s is Star again.
    d.handle(press("g"), 5000);
    expect(d.handle(press("s"), 5000 + G_WINDOW_MS + 1)).toBe("star");
    // g only counts once.
    d.handle(press("g"), 9000);
    d.handle(press("x"), 9001);
    expect(d.handle(press("i"), 9002)).toBeNull();
  });

  it("single keys do nothing while typing; Ctrl/Cmd and Esc still act", () => {
    const d = new ShortcutDispatcher();
    for (const key of ["j", "k", "e", "#", "Delete", "s", "/", "c", "r", "g", "Enter"]) {
      expect(d.handle(press(key, { typing: true }), 0), key).toBeNull();
    }
    expect(d.handle(press("U", { shift: true, typing: true }), 0)).toBeNull();
    expect(d.handle(press("Enter", { ctrl: true, typing: true }), 0)).toBe("send");
    expect(d.handle(press("Escape", { typing: true }), 0)).toBe("close");
    // g typed in a field does not start a sequence.
    d.handle(press("g", { typing: true }), 0);
    expect(d.handle(press("i"), 10)).toBeNull();
  });

  it("ignores other combinations", () => {
    const d = new ShortcutDispatcher();
    expect(d.handle(press("J", { shift: true }), 0)).toBeNull();
    expect(d.handle(press("e", { alt: true }), 0)).toBeNull();
    expect(d.handle(press("p", { ctrl: true, shift: true }), 0)).toBeNull();
    expect(d.handle(press("x"), 0)).toBeNull();
  });

  it("knows what takes text", () => {
    expect(isTyping({ tagName: "INPUT", type: "text" })).toBe(true);
    expect(isTyping({ tagName: "INPUT", type: "search" })).toBe(true);
    expect(isTyping({ tagName: "INPUT", type: "checkbox" })).toBe(false);
    expect(isTyping({ tagName: "TEXTAREA" })).toBe(true);
    expect(isTyping({ tagName: "SELECT" })).toBe(true);
    expect(isTyping({ tagName: "DIV", isContentEditable: true })).toBe(true);
    expect(isTyping({ tagName: "DIV", isContentEditable: false })).toBe(false);
    expect(isTyping(null)).toBe(false);
  });

  it("the Shortcuts screen lists every action (j / k on one row)", () => {
    const listed = new Set(SHORTCUT_TABLE.map((s) => s.action));
    const all: ShortcutAction[] = [
      "next", "open", "close", "compose", "reply", "replyAll", "forward", "archive", "delete", "star",
      "snooze", "mute", "toggleUnread", "search", "goInbox", "goDrafts", "goSent", "send", "print", "settings",
    ];
    expect([...listed].sort()).toEqual([...all].sort());
  });
});

describe("settings", () => {
  it("takes only values that fit", () => {
    expect(withValue(DEFAULTS, "theme", "dark").theme).toBe("dark");
    expect(withValue(DEFAULTS, "theme", "blue").theme).toBe("system");
    expect(withValue(DEFAULTS, "readingPane", "bottom").readingPane).toBe("bottom");
    expect(withValue(DEFAULTS, "groupByThread", false).groupByThread).toBe(false);
    expect(withValue(DEFAULTS, "groupByThread", "no").groupByThread).toBe(true);
    expect(withValue(DEFAULTS, "html", 1).html).toBe(true);
  });

  it("theme and density as root attributes", () => {
    expect(rootAttributes(DEFAULTS)).toEqual({ theme: null, density: null });
    expect(rootAttributes({ ...DEFAULTS, theme: "dark", density: "compact" })).toEqual({ theme: "dark", density: "compact" });
  });

  it("mark as read: after the delay, at once, or never", () => {
    expect(markReadDelay("delay", 1500)).toBe(1500);
    expect(markReadDelay("immediately", 1500)).toBe(0);
    expect(markReadDelay("never", 1500)).toBeNull();
  });

  it("a signature's HTML part is escaped, line by line", () => {
    expect(signatureHtml("Ann <a@x>\n\n-- & co")).toBe("<div>Ann &lt;a@x&gt;</div><div><br></div><div>-- &amp; co</div>");
  });
});

describe("notification click (G80)", () => {
  it("opens the newest background notification once, within 10 s", () => {
    const c = new NotifyClicks();
    c.shown({ account_id: 1, message_id: 5, count: 1, at: 1000 }, false);
    c.shown({ account_id: 1, message_id: 6, count: 1, at: 2000 }, false);
    expect(c.focused(3000)).toEqual({ kind: "message", account_id: 1, message_id: 6 });
    expect(c.focused(3001)).toBeNull();
  });

  it("a coalesced one opens the INBOX; stale or foreground ones nothing", () => {
    const c = new NotifyClicks();
    c.shown({ account_id: 2, message_id: 9, count: 4, at: 0 }, false);
    expect(c.focused(100)).toEqual({ kind: "inbox", account_id: 2 });
    c.shown({ account_id: 2, message_id: 9, count: 1, at: 0 }, false);
    expect(c.focused(CLICK_WINDOW_MS + 1)).toBeNull();
    c.shown({ account_id: 2, message_id: 9, count: 1, at: 0 }, true);
    expect(c.focused(10)).toBeNull();
  });
});

describe("print (T44) and preview (18.9)", () => {
  it("a thread prints oldest first", () => {
    const views = [
      { id: 3, date_utc: 300 },
      { id: 1, date_utc: 100 },
      { id: 2, date_utc: 200 },
    ];
    expect(printOrder(views).map((v) => v.id)).toEqual([1, 2, 3]);
  });

  it("blocked images stay blocked in print; allowed ones load", () => {
    const blocked = { body_url: "mail-body://localhost/7?images=0", remote_images: "blocked" as const };
    expect(printBodyUrl(blocked)).toBe("mail-body://localhost/7?images=0");
    const tampered = { body_url: "mail-body://localhost/7?images=1", remote_images: "blocked" as const };
    expect(printBodyUrl(tampered)).toBe("mail-body://localhost/7?images=0");
    const allowed = { body_url: "mail-body://localhost/7?images=1", remote_images: "allowed" as const };
    expect(printBodyUrl(allowed)).toBe("mail-body://localhost/7?images=1");
  });

  it("images up to 20 MB preview in the lightbox; PDFs open externally", () => {
    expect(canPreview({ mime: "image/jpeg", size: 1000 })).toBe(true);
    expect(canPreview({ mime: "IMAGE/PNG", size: 20 * 1024 * 1024 })).toBe(true);
    expect(canPreview({ mime: "image/png", size: 20 * 1024 * 1024 + 1 })).toBe(false);
    expect(canPreview({ mime: "application/pdf", size: 1000 })).toBe(false);
    expect(previewUrl(4, 9)).toBe("mail-cid://localhost/4/attachment/9");
  });
});
