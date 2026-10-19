import { describe, expect, it } from "vitest";
import { MAX_MOTION_MS, easeOut, fade, parseDuration, pop, popCss } from "./motion";

describe("parseDuration", () => {
  it("reads ms and s", () => {
    expect(parseDuration("150ms")).toBe(150);
    expect(parseDuration(" 0.1s ")).toBe(100);
    expect(parseDuration("0ms")).toBe(0);
  });

  it("clamps to the 150 ms guardrail", () => {
    expect(parseDuration("400ms")).toBe(MAX_MOTION_MS);
    expect(parseDuration("2s")).toBe(MAX_MOTION_MS);
  });

  it("treats garbage and negatives as no motion", () => {
    expect(parseDuration("")).toBe(0);
    expect(parseDuration("fast")).toBe(0);
    expect(parseDuration("-5ms")).toBe(0);
  });
});

describe("transitions", () => {
  it("are instant without a document (tests, SSR)", () => {
    const node = {} as Element;
    expect(fade(node).duration).toBe(0);
    expect(pop(node).duration).toBe(0);
  });

  it("only animate opacity and transform", () => {
    const node = {} as Element;
    for (const cfg of [fade(node), pop(node)]) {
      const css = cfg.css?.(0.5, 0.5) ?? "";
      const props = css
        .split(";")
        .map((d) => d.split(":")[0]?.trim())
        .filter((p): p is string => Boolean(p));
      expect(props.every((p) => p === "opacity" || p === "transform")).toBe(true);
    }
  });

  it("pop scales from the start value to 1", () => {
    expect(popCss(0.96, 0)).toBe(0.96);
    expect(popCss(0.96, 1)).toBe(1);
    expect(popCss(1, 0.3)).toBe(1);
  });

  it("easeOut spans 0..1", () => {
    expect(easeOut(0)).toBe(0);
    expect(easeOut(1)).toBe(1);
    expect(easeOut(0.5)).toBeGreaterThan(0.5);
  });
});
