// Section 7 compose rules on the page: recipient parsing and red chips, the send warnings,
// the first body (cursor, signature, then the quote), and mailto links.

import { describe, expect, it } from "vitest";
import {
  chipsFrom,
  firstBody,
  invalidRecipients,
  isValidAddress,
  mailtoAddress,
  parseAddresses,
  sendWarnings,
  type QuoteText,
} from "./compose";
import type { OutboxView } from "./ipc";
import { needsAction, unsent } from "../state/app.svelte";

const words: QuoteText = {
  wrote: (date, name, addr) => `On ${date}, ${name} <${addr}> wrote:`,
  forwardedHeader: "-- Forwarded --",
  from: "From:",
  date: "Date:",
  subject: "Subject:",
  to: "To:",
};

describe("recipients", () => {
  it("parses the common RFC 5322 forms at commas, semicolons and line breaks", () => {
    expect(parseAddresses('bob@example.test, "Doe, Jane" <jane@example.test>; Carol <carol@example.test>\nx@y.test')).toEqual([
      { name: "", addr: "bob@example.test" },
      { name: "Doe, Jane", addr: "jane@example.test" },
      { name: "Carol", addr: "carol@example.test" },
      { name: "", addr: "x@y.test" },
    ]);
  });

  it("marks addresses that do not parse, which blocks sending", () => {
    expect(isValidAddress("bob@example.test")).toBe(true);
    expect(isValidAddress("bob@")).toBe(false);
    expect(isValidAddress("bob example.test")).toBe(false);
    expect(isValidAddress("bob@localhost")).toBe(false);
    const content = { to: [{ name: "", addr: "ok@example.test" }], cc: [{ name: "", addr: "nope" }], bcc: [] };
    expect(invalidRecipients(content)).toEqual([{ name: "", addr: "nope" }]);
  });

  it("adds chips without duplicates, case-insensitively", () => {
    const existing = [{ name: "", addr: "Bob@Example.test" }];
    expect(chipsFrom("bob@example.test, carol@example.test", existing)).toEqual([
      { name: "", addr: "Bob@Example.test" },
      { name: "", addr: "carol@example.test" },
    ]);
  });
});

describe("send warnings", () => {
  const base = { subject: "Hi", text: "Hello", attachments: [], forward_attachment: null };

  it("warns about an empty subject", () => {
    expect(sendWarnings({ ...base, subject: "  " }).emptySubject).toBe(true);
    expect(sendWarnings(base).emptySubject).toBe(false);
  });

  it("warns when the body says attach or attached and nothing is attached", () => {
    expect(sendWarnings({ ...base, text: "See the attached file." }).missingAttachment).toBe(true);
    expect(sendWarnings({ ...base, text: "I will attach it." }).missingAttachment).toBe(true);
    const one = [{ id: "a", name: "a.pdf", mime: "application/pdf", size: 1 }];
    expect(sendWarnings({ ...base, text: "Attached.", attachments: one }).missingAttachment).toBe(false);
    expect(sendWarnings({ ...base, text: "Attached.", forward_attachment: 7 }).missingAttachment).toBe(false);
  });

  it("ignores the quote and the signature", () => {
    expect(sendWarnings({ ...base, text: "Thanks\n> see attached\n" }).missingAttachment).toBe(false);
    expect(sendWarnings({ ...base, text: "Thanks\n-- \nAttachments welcome" }).missingAttachment).toBe(false);
  });
});

describe("the first body", () => {
  const quote = {
    date_utc: 0,
    from: { name: "Bob", addr: "bob@example.test" },
    to: [{ name: "", addr: "me@example.test" }],
    subject: "Plans",
    text: "<b>not markup</b>\nsecond line",
    forwarded: false,
  };

  it("puts the cursor line first, the signature between reply and quote", () => {
    const blocks = firstBody(quote, "Me\nPhone", words, () => "DATE");
    expect(blocks).toEqual([
      { kind: "line", text: "" },
      { kind: "line", text: "-- " },
      { kind: "line", text: "Me" },
      { kind: "line", text: "Phone" },
      { kind: "line", text: "" },
      { kind: "line", text: "On DATE, Bob <bob@example.test> wrote:" },
      { kind: "quote", text: "<b>not markup</b>\nsecond line" },
    ]);
  });

  it("writes the forwarded header block for a forward", () => {
    const blocks = firstBody({ ...quote, forwarded: true }, "", words, () => "DATE");
    expect(blocks.map((b) => b.text)).toEqual([
      "",
      "",
      "-- Forwarded --",
      "From: Bob <bob@example.test>",
      "Date: DATE",
      "Subject: Plans",
      "To: me@example.test",
      "",
      "<b>not markup</b>\nsecond line",
    ]);
  });

  it("is only the cursor line for a new message without a signature", () => {
    expect(firstBody(null, "  ", words, () => "")).toEqual([{ kind: "line", text: "" }]);
  });
});

describe("mailto links", () => {
  it("open compose to the address they name", () => {
    expect(mailtoAddress("mailto:bob@example.test?subject=Hi")).toBe("bob@example.test");
    expect(mailtoAddress("mailto:Bob%20%3Cbob@example.test%3E")).toBe("bob@example.test");
    expect(mailtoAddress("mailto:")).toBeNull();
    expect(mailtoAddress("https://example.test")).toBeNull();
    expect(mailtoAddress("mailto:%E0%A4%A")).toBeNull();
  });
});

describe("the outbox view", () => {
  const item = (id: number, state: OutboxView["state"]): OutboxView => ({
    id,
    account_id: 1,
    subject: "s",
    to: [],
    state,
    error: null,
    send_after_utc: 0,
    created_at: 0,
    draft_id: null,
    late: false,
  });

  it("badges failed and unknown items and hides sent history", () => {
    const list = [item(1, "failed"), item(2, "unknown"), item(3, "sent"), item(4, "queued"), item(5, "retry_wait")];
    expect(needsAction(list)).toBe(2);
    expect(unsent(list).map((o) => o.id)).toEqual([1, 2, 4, 5]);
  });
});
