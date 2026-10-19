// Svelte transitions for the two allowed motions (SPEC_GAPS G8): fade and pop.
// Durations come from the --motion-* tokens, so prefers-reduced-motion (which sets them to
// 0 ms in tokens.css) disables every transition without extra code here.

import type { TransitionConfig } from "svelte/transition";

/** Hard ceiling from BUILD.md guardrail 2. */
export const MAX_MOTION_MS = 150;

/** Parses a CSS time value ("150ms", "0.1s") and clamps it to [0, MAX_MOTION_MS]. */
export function parseDuration(value: string): number {
  const v = value.trim();
  let ms: number;
  if (v.endsWith("ms")) {
    ms = Number.parseFloat(v);
  } else if (v.endsWith("s")) {
    ms = Number.parseFloat(v) * 1000;
  } else {
    ms = Number.NaN;
  }
  if (!Number.isFinite(ms) || ms < 0) return 0;
  return Math.min(ms, MAX_MOTION_MS);
}

function token(name: string): string {
  if (typeof document === "undefined") return "";
  return getComputedStyle(document.documentElement).getPropertyValue(name);
}

/** Current motion duration in ms, honoring reduced motion through the tokens. */
export function motionDuration(fast = false): number {
  return parseDuration(token(fast ? "--motion-duration-fast" : "--motion-duration"));
}

function popScale(): number {
  const s = Number.parseFloat(token("--motion-pop-scale"));
  return Number.isFinite(s) && s > 0 && s <= 1 ? s : 1;
}

/** Decelerating curve matching --motion-ease-out closely enough for 150 ms. */
export function easeOut(t: number): number {
  return 1 - (1 - t) ** 3;
}

/** Opacity fade. */
export function fade(_node: Element, opts: { fast?: boolean } = {}): TransitionConfig {
  return {
    duration: motionDuration(opts.fast ?? false),
    easing: easeOut,
    css: (t) => `opacity: ${t}`,
  };
}

/** Pop-in: opacity plus a small scale-up. Used for menus, dialogs, toasts and popovers. */
export function pop(_node: Element, opts: { fast?: boolean } = {}): TransitionConfig {
  const from = popScale();
  return {
    duration: motionDuration(opts.fast ?? false),
    easing: easeOut,
    css: (t) => `opacity: ${t}; transform: scale(${popCss(from, t)})`,
  };
}

/** Scale at progress t, from `from` to 1. Exported for tests. */
export function popCss(from: number, t: number): number {
  return Number((from + (1 - from) * t).toFixed(4));
}
