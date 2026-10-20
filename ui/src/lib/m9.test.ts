// M9 UI logic: snooze and Send later presets, the thread view's expanded messages, list
// sources, labels, Block sender, Insert template at the cursor, and the Outbox history.

import { describe, expect, it } from "vitest";
import { insertAt } from "./compose";
import type { FolderView, OutboxView, Page, Row } from "./ipc";
import { blockRule, labelTargets } from "./labels";
import { PAGE_SIZE, PageCache } from "./pages";
import { listSource } from "./source";
import { initiallyExpanded, participantsText } from "./threads";
import { fromLocalInput, presetTime, toLocalInput } from "./when";
import { sentHistory } from "../state/app.svelte";

function row(id: number, unread: boolean): Row {
  return {
    id,
    account_id: 1,
    folder_id: 1,
    thread_id: 1,
    subject: "s",
    from: { name: "", addr: "a@x" },
    to: [],
    date_utc: id,
    unread,
    flagged: false,
    has_attach: false,
    preview: "",
    thread: null,
  };
}

function folder(id: number, extra: Partial<FolderView> = {}): FolderView {
  return {
    id,
    account_id: 1,
    path: `F${id}`,
    name: `F${id}`,
    depth: 0,
    role: null,
    unread: 0,
    total: 10,
    sync_enabled: true,
    watch_idle: false,
    synced: true,
    hidden: false,
    ...extra,
  };
}

describe("snooze and send later presets (18.2)", () => {
  // Wednesday 2026-10-21, 14:30 local.
  const wed = new Date(2026, 9, 21, 14, 30);
  it("later today, tomorrow, this weekend and next week, in local time", () => {
    expect(presetTime("later_today", wed).getTime()).toBe(wed.getTime() + 3 * 3600 * 1000);
    expect(presetTime("tomorrow", wed)).toEqual(new Date(2026, 9, 22, 8, 0));
    expect(presetTime("weekend", wed)).toEqual(new Date(2026, 9, 24, 8, 0));
    expect(presetTime("next_week", wed)).toEqual(new Date(2026, 9, 26, 8, 0));
    // On a Saturday "this weekend" is the next one; on a Monday "next week" is a week on.
    expect(presetTime("weekend", new Date(2026, 9, 24, 9, 0))).toEqual(new Date(2026, 9, 31, 8, 0));
    expect(presetTime("next_week", new Date(2026, 9, 26, 9, 0))).toEqual(new Date(2026, 10, 2, 8, 0));
  });

  it("a picked time must be valid and in the future", () => {
    expect(fromLocalInput("2026-10-22T09:15", wed)).toEqual(new Date(2026, 9, 22, 9, 15));
    expect(fromLocalInput("2026-10-20T09:15", wed)).toBeNull();
    expect(fromLocalInput("tomorrow", wed)).toBeNull();
    expect(toLocalInput(new Date(2026, 0, 5, 7, 3))).toBe("2026-01-05T07:03");
  });
});

describe("the thread view (section 10, rule 6)", () => {
  it("expands unread messages and the latest; the rest are one line", () => {
    const rows = [row(1, false), row(2, true), row(3, false), row(4, false)];
    expect([...initiallyExpanded(rows)].sort()).toEqual([2, 4]);
    expect([...initiallyExpanded([])]).toEqual([]);
    expect(participantsText(["Ann", "Bob", "Cy", "Dee"])).toBe("Ann, Bob, Cy");
  });
});

describe("list sources", () => {
  it("a folder, grouped or not; All Inboxes; a search", () => {
    const f = folder(7, { total: 42 });
    expect(listSource({ folder: f, unified: false, search: null, grouped: false })).toMatchObject({
      scope: { kind: "folder", folder_id: 7 },
      group: false,
      total: 42,
    });
    // Threads: the folder count is not the row count.
    expect(listSource({ folder: f, unified: false, search: null, grouped: true })?.total).toBeNull();
    expect(listSource({ folder: f, unified: true, search: null, grouped: true })).toMatchObject({
      scope: { kind: "unified" },
      group: true,
      folder: null,
    });
    const s = listSource({ folder: f, unified: false, search: { text: "budget", allAccounts: false }, grouped: true });
    expect(s).toMatchObject({ scope: { kind: "search", query: "budget", account_id: 1 }, group: false, searchText: "budget" });
    expect(listSource({ folder: f, unified: false, search: { text: "x", allAccounts: true }, grouped: true })?.searchAccount).toBeNull();
    // An empty search shows the folder again.
    expect(listSource({ folder: f, unified: false, search: { text: "  ", allAccounts: false }, grouped: false })?.scope.kind).toBe("folder");
    expect(listSource({ folder: null, unified: false, search: null, grouped: true })).toBeNull();
  });

  it("a list without a known length grows a page at a time", async () => {
    const all = Array.from({ length: 250 }, (_, i) => row(1000 - i, false));
    const fetch = async (cursor: { date_utc: number; id: number } | null, limit: number): Promise<Page> => {
      const from = cursor === null ? 0 : all.findIndex((r) => r.id < cursor.id);
      const rows = all.slice(from, from + limit);
      const last = rows[rows.length - 1];
      return { rows, next: from + limit < all.length && last ? { date_utc: last.date_utc, id: last.id } : null, unindexed: null };
    };
    const cache = new PageCache(fetch);
    expect(cache.estimatedTotal()).toBe(PAGE_SIZE);
    await cache.ensure(0, 10);
    expect(cache.estimatedTotal()).toBe(2 * PAGE_SIZE);
    await cache.ensure(0, 249);
    expect(cache.estimatedTotal()).toBe(250);
  });
});

describe("labels and Block sender (18.1, 18.6)", () => {
  it("offers the account's own folders as labels, minus the ones it has", () => {
    const folders = [
      folder(1, { role: "inbox" }),
      folder(2, { path: "Work" }),
      folder(3, { path: "Travel" }),
      folder(4, { path: "[Gmail]/Important", hidden: true }),
      folder(5, { path: "[Gmail]/Spam" }),
    ];
    const targets = labelTargets(folders, [{ folder_id: 2, name: "Work" }]).map((f) => f.path);
    expect(targets).toEqual(["Travel"]);
  });

  it("Block sender is a rule: from the address, to Trash, stop", () => {
    expect(blockRule(" spam@x.test ")).toEqual({
      id: 0,
      account_id: null,
      name: "Block spam@x.test",
      enabled: true,
      conditions: { match: "all", items: [{ field: "from", value: "spam@x.test" }] },
      actions: [{ kind: "delete" }],
      stop_processing: true,
    });
  });
});

describe("templates and the outbox history (18.3, 18.4)", () => {
  it("Insert template puts the body at the cursor (T41)", () => {
    expect(insertAt("Hello world", 6, 6, "dear ")).toEqual({ text: "Hello dear world", cursor: 11 });
    expect(insertAt("Hello world", 6, 11, "you")).toEqual({ text: "Hello you", cursor: 9 });
    expect(insertAt("x", 50, 60, "y")).toEqual({ text: "xy", cursor: 2 });
  });

  it("sent items are history, newest first, with Sent late", () => {
    const item = (id: number, state: OutboxView["state"], late = false): OutboxView => ({
      id,
      account_id: 1,
      subject: "s",
      to: [],
      state,
      error: null,
      send_after_utc: id,
      created_at: 0,
      draft_id: null,
      late,
    });
    const history = sentHistory([item(1, "sent"), item(2, "queued"), item(3, "sent", true)]);
    expect(history.map((o) => [o.id, o.late])).toEqual([
      [3, true],
      [1, false],
    ]);
  });
});
