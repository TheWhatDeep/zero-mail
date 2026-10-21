// Keyboard shortcuts (section 11 table, T34). A pure dispatcher: it turns a key press into
// an action and knows nothing of the page. Single keys are inactive while typing in a field;
// Ctrl/Cmd combinations stay active (they type nothing, and Ctrl+Enter sends from the body).

export type ShortcutAction =
  | "next"
  | "previous"
  | "open"
  | "close"
  | "compose"
  | "reply"
  | "replyAll"
  | "forward"
  | "archive"
  | "delete"
  | "star"
  | "snooze"
  | "mute"
  | "toggleUnread"
  | "search"
  | "goInbox"
  | "goDrafts"
  | "goSent"
  | "send"
  | "print"
  | "settings";

export interface KeyPress {
  key: string;
  ctrl: boolean;
  meta: boolean;
  shift: boolean;
  alt: boolean;
  /** The focus is in a text field, a select or an editable area. */
  typing: boolean;
}

/** `g` then a key, within this long (section 11: "g then i, d, s"). */
export const G_WINDOW_MS = 1500;

const SINGLE: Record<string, ShortcutAction> = {
  j: "next",
  k: "previous",
  Enter: "open",
  o: "open",
  Escape: "close",
  c: "compose",
  r: "reply",
  a: "replyAll",
  f: "forward",
  e: "archive",
  "#": "delete",
  Delete: "delete",
  s: "star",
  b: "snooze",
  m: "mute",
  "/": "search",
};

const AFTER_G: Record<string, ShortcutAction> = { i: "goInbox", d: "goDrafts", s: "goSent" };

const WITH_MOD: Record<string, ShortcutAction> = { Enter: "send", p: "print", ",": "settings" };

export class ShortcutDispatcher {
  private gAt: number | null = null;

  /** The action for a key press at `now` (ms), or null. */
  handle(k: KeyPress, now: number): ShortcutAction | null {
    if (k.ctrl || k.meta) {
      this.gAt = null;
      if (k.alt || k.shift) return null;
      return WITH_MOD[k.key.length === 1 ? k.key.toLowerCase() : k.key] ?? null;
    }
    // Escape leaves a field too: it closes what is open.
    if (k.typing) return k.key === "Escape" && !k.alt ? "close" : null;
    if (k.alt) return null;
    const g = this.gAt;
    this.gAt = null;
    if (g !== null && now - g <= G_WINDOW_MS && !k.shift) {
      const go = AFTER_G[k.key];
      if (go) return go;
    }
    if (k.key === "U" && k.shift) return "toggleUnread";
    if (k.key === "g" && !k.shift) {
      this.gAt = now;
      return null;
    }
    // `#` needs Shift on most layouts; letters must be lower case.
    if (k.shift && k.key !== "#") return null;
    return SINGLE[k.key] ?? null;
  }
}

/** Whether a key press lands in something that takes text. */
export function isTyping(target: { tagName?: string; isContentEditable?: boolean; type?: string } | null): boolean {
  if (!target) return false;
  if (target.isContentEditable) return true;
  const tag = (target.tagName ?? "").toUpperCase();
  if (tag === "TEXTAREA" || tag === "SELECT") return true;
  if (tag !== "INPUT") return false;
  const type = (target.type ?? "text").toLowerCase();
  return !["checkbox", "radio", "button", "submit", "reset", "range", "color", "file"].includes(type);
}

/** The Shortcuts screen (read-only, section 11). Keys as shown, then the action. */
export const SHORTCUT_TABLE: readonly { keys: string; action: ShortcutAction }[] = [
  { keys: "j / k", action: "next" },
  { keys: "Enter or o", action: "open" },
  { keys: "Esc", action: "close" },
  { keys: "c", action: "compose" },
  { keys: "r", action: "reply" },
  { keys: "a", action: "replyAll" },
  { keys: "f", action: "forward" },
  { keys: "e", action: "archive" },
  { keys: "# or Delete", action: "delete" },
  { keys: "s", action: "star" },
  { keys: "b", action: "snooze" },
  { keys: "m", action: "mute" },
  { keys: "Shift+U", action: "toggleUnread" },
  { keys: "/", action: "search" },
  { keys: "g then i", action: "goInbox" },
  { keys: "g then d", action: "goDrafts" },
  { keys: "g then s", action: "goSent" },
  { keys: "Ctrl/Cmd+Enter", action: "send" },
  { keys: "Ctrl/Cmd+P", action: "print" },
  { keys: "Ctrl/Cmd+,", action: "settings" },
];
