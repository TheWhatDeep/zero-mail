// Perf marks for tests/perf/startup.mjs (section 14). The page reports when the cached inbox
// is interactive and, when the harness asks, runs a scroll benchmark over the list. Outside
// perf mode the app grants no permission to emit events, so the reports fail silently and
// nobody asks for a benchmark.

import { emit, listen } from "@tauri-apps/api/event";

let reported = false;

/** Once: the first list rows are on screen. */
export function markInboxReady(): void {
  if (reported) return;
  reported = true;
  // After the rows are painted: the frame after the next one.
  requestAnimationFrame(() =>
    requestAnimationFrame(() => {
      emit("zeromail-perf-ready", { ui_ms: Math.round(performance.now()) }).catch(() => {});
    }),
  );
}

export interface ScrollResult {
  frames: number;
  fps: number;
  p95_ms: number;
  max_ms: number;
  max_dom_rows: number;
  scrolled_px: number;
}

/** Frame timing statistics from requestAnimationFrame timestamps. Exported for tests. */
export function frameStats(stamps: number[]): { fps: number; p95_ms: number; max_ms: number } {
  const gaps = stamps.slice(1).map((t, i) => t - (stamps[i] ?? t));
  if (gaps.length === 0) return { fps: 0, p95_ms: 0, max_ms: 0 };
  const sorted = [...gaps].sort((a, b) => a - b);
  const p95 = sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * 0.95))] ?? 0;
  const total = (stamps[stamps.length - 1] ?? 0) - (stamps[0] ?? 0);
  return {
    fps: total > 0 ? Math.round((gaps.length / total) * 1000 * 10) / 10 : 0,
    p95_ms: Math.round(p95 * 10) / 10,
    max_ms: Math.round((sorted[sorted.length - 1] ?? 0) * 10) / 10,
  };
}

/** Scrolls `scroller` a fixed step per frame and measures. */
export function scrollBenchmark(scroller: HTMLElement, frames: number, step: number): Promise<ScrollResult> {
  return new Promise((resolve) => {
    const stamps: number[] = [];
    let maxRows = 0;
    const start = scroller.scrollTop;
    const tick = (t: number) => {
      stamps.push(t);
      maxRows = Math.max(maxRows, scroller.querySelectorAll('[role="option"]').length);
      if (stamps.length > frames) {
        resolve({
          frames,
          ...frameStats(stamps),
          max_dom_rows: maxRows,
          scrolled_px: Math.round(scroller.scrollTop - start),
        });
        return;
      }
      scroller.scrollTop += step;
      requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
  });
}

/** Waits for the reading pane's body frame of the message titled `subject` to load. */
function frameLoaded(subject: string, timeoutMs: number): Promise<void> {
  return new Promise((resolve) => {
    const start = performance.now();
    const check = () => {
      const heading = document.querySelector("h2")?.textContent ?? "";
      const frame = document.querySelector("iframe");
      if (heading === subject && frame) {
        frame.addEventListener("load", () => resolve(), { once: true });
        setTimeout(resolve, 1500);
        return;
      }
      if (performance.now() - start > timeoutMs) {
        resolve();
        return;
      }
      setTimeout(check, 50);
    };
    check();
  });
}

/** T19's CI harness (SPEC_GAPS G68): when asked, opens every message of the folder in turn,
 * lets each body load and settle, and reports the subjects it opened. */
export function answerWalkRequests(open: (index: number) => Promise<string | null>, count: () => number): void {
  listen("zeromail-perf-walk", async () => {
    const opened: string[] = [];
    for (let i = 0; i < count(); i++) {
      const subject = await open(i);
      if (subject === null) continue;
      await frameLoaded(subject, 5000);
      await new Promise((r) => setTimeout(r, 400));
      opened.push(subject);
    }
    emit("zeromail-perf-walk-result", { opened }).catch(() => {});
  }).catch(() => {});
}

/** When the harness asks, benchmark the scroller `find` returns and report back. */
export function answerScrollRequests(find: () => HTMLElement | null): void {
  listen("zeromail-perf-scroll", () => {
    const scroller = find();
    if (!scroller) return;
    void scrollBenchmark(scroller, 300, 120).then((result) =>
      emit("zeromail-perf-scroll-result", result).catch(() => {}),
    );
  }).catch(() => {});
}
