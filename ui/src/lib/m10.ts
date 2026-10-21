// M10 helpers the components share: the notification click (SPEC_GAPS G80), print (18.8),
// the image lightbox (18.9) and unsubscribe (18.7). Pure, so they are tested without a page.

import type { Attachment, MessageView, UnsubscribeMethod } from "./ipc";

/** A notification counts as clicked when the window comes to the front this soon (G80). */
export const CLICK_WINDOW_MS = 10_000;

export interface Notified {
  account_id: number;
  message_id: number;
  count: number;
  at: number;
}

export type NotifyTarget = { kind: "message"; account_id: number; message_id: number } | { kind: "inbox"; account_id: number };

/**
 * Remembers the newest notification shown while the window was in the background. When
 * the window comes to the front within [`CLICK_WINDOW_MS`], it opens that message, or the
 * account's INBOX for a coalesced one (SPEC_GAPS G80). Each notification opens once.
 */
export class NotifyClicks {
  private last: Notified | null = null;

  shown(n: Notified, windowFocused: boolean): void {
    if (!windowFocused) this.last = n;
  }

  focused(now: number): NotifyTarget | null {
    const n = this.last;
    this.last = null;
    if (!n || now - n.at > CLICK_WINDOW_MS) return null;
    return n.count > 1
      ? { kind: "inbox", account_id: n.account_id }
      : { kind: "message", account_id: n.account_id, message_id: n.message_id };
  }
}

/** Print (section 18.8): a thread prints expanded, oldest first. */
export function printOrder<T extends Pick<MessageView, "id" | "date_utc">>(views: T[]): T[] {
  return [...views].sort((a, b) => a.date_utc - b.date_utc || a.id - b.id);
}

/**
 * The body a print shows: images only if already allowed for that message (18.8), so a
 * blocked message prints with `images=0` and nothing is fetched (T44).
 */
export function printBodyUrl(view: Pick<MessageView, "body_url" | "remote_images">): string {
  const allowed = view.remote_images === "allowed";
  return view.body_url.replace(/([?&])images=[01]/, `$1images=${allowed ? 1 : 0}`);
}

/** The lightbox's limit (18.9). */
export const PREVIEW_MAX_BYTES = 20 * 1024 * 1024;

/** Image attachments up to 20 MB open in the lightbox; the rest open externally (18.9). */
export function canPreview(a: Pick<Attachment, "mime" | "size">): boolean {
  return a.mime.toLowerCase().startsWith("image/") && a.size <= PREVIEW_MAX_BYTES;
}

/** The lightbox image, served by mail-cid (section 18.9). */
export function previewUrl(messageId: number, attachmentId: number): string {
  return `mail-cid://localhost/${messageId}/attachment/${attachmentId}`;
}

/** What Unsubscribe asks before it acts (18.7: never without a click). */
export type UnsubscribeText = Record<UnsubscribeMethod, string>;
