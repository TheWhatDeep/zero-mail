// Times for Snooze (section 18.2) and Send later (section 18.3), in local time. Core stores
// UTC seconds (section 12: snooze and send later use UTC stored at creation).

export type Preset = "later_today" | "tomorrow" | "weekend" | "next_week";

export const PRESETS: readonly Preset[] = ["later_today", "tomorrow", "weekend", "next_week"];

function at8(d: Date): Date {
  const out = new Date(d);
  out.setHours(8, 0, 0, 0);
  return out;
}

/** Later today (+3 h), tomorrow 08:00, this weekend (Saturday 08:00), next week (Monday
 * 08:00), from `now` in local time. On a Saturday, "this weekend" is the next Saturday. */
export function presetTime(p: Preset, now: Date): Date {
  switch (p) {
    case "later_today":
      return new Date(now.getTime() + 3 * 60 * 60 * 1000);
    case "tomorrow": {
      const d = new Date(now);
      d.setDate(d.getDate() + 1);
      return at8(d);
    }
    case "weekend": {
      const d = new Date(now);
      const days = (6 - d.getDay() + 7) % 7 || 7;
      d.setDate(d.getDate() + days);
      return at8(d);
    }
    case "next_week": {
      const d = new Date(now);
      const days = (1 - d.getDay() + 7) % 7 || 7;
      d.setDate(d.getDate() + days);
      return at8(d);
    }
  }
}

/** UTC seconds. */
export function epoch(d: Date): number {
  return Math.floor(d.getTime() / 1000);
}

/** An `<input type="datetime-local">` value (local time) as a Date; null when invalid or not
 * in the future. */
export function fromLocalInput(value: string, now: Date): Date | null {
  if (!/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}$/.test(value)) return null;
  const d = new Date(value);
  return Number.isNaN(d.getTime()) || d.getTime() <= now.getTime() ? null : d;
}

/** The value an `<input type="datetime-local">` takes for `d`, in local time. */
export function toLocalInput(d: Date): string {
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`;
}
