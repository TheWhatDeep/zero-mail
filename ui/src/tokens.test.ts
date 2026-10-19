// Guards for BUILD.md guardrail 2 and section 11. They read the source files directly.
//   - Visual values live only in tokens.css; every var(--x) used is defined there.
//   - Motion is opacity/transform only and never exceeds 150 ms.
//   - No backdrop-filter (SPEC_GAPS G4), no remote fonts or imports (G3, section 13).
//   - The explicit and system dark blocks are identical (G5).
//   - Text contrast is at least 4.5:1 in both themes (section 11, G16).

import { readdirSync, readFileSync, statSync } from "node:fs";
import { extname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const SRC = fileURLToPath(new URL(".", import.meta.url));
const tokensCss = readFileSync(join(SRC, "tokens.css"), "utf8");

function sources(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) return sources(path);
    return [".css", ".svelte", ".ts"].includes(extname(path)) ? [path] : [];
  });
}

/** CSS text of a file: whole .css files, <style> blocks of .svelte files. */
function cssOf(path: string): string {
  const text = readFileSync(path, "utf8");
  if (path.endsWith(".css")) return text;
  if (path.endsWith(".svelte")) {
    return [...text.matchAll(/<style[^>]*>([\s\S]*?)<\/style>/g)].map((m) => m[1] ?? "").join("\n");
  }
  return "";
}

const stripComments = (css: string) => css.replace(/\/\*[\s\S]*?\*\//g, "");

const styleFiles = sources(SRC).filter((p) => !p.endsWith("tokens.css") && !p.endsWith(".test.ts"));

/** Declarations of one selector block, e.g. ':root' or ':root[data-theme="dark"]'. */
function block(css: string, selectorStart: string): Map<string, string> {
  const start = css.indexOf(selectorStart);
  expect(start, `missing block ${selectorStart}`).toBeGreaterThanOrEqual(0);
  const open = start + selectorStart.lastIndexOf("{");
  const close = css.indexOf("}", open);
  const body = stripComments(css.slice(open + 1, close));
  const out = new Map<string, string>();
  for (const decl of body.split(";")) {
    const idx = decl.indexOf(":");
    if (idx < 0) continue;
    out.set(decl.slice(0, idx).trim(), decl.slice(idx + 1).trim());
  }
  return out;
}

const light = block(tokensCss, ":root {");
const darkExplicit = block(tokensCss, ':root[data-theme="dark"] {');
const darkSystem = block(tokensCss, ':root:not([data-theme="light"]) {');
const dark = new Map([...light, ...darkExplicit]);

describe("tokens are the only source of visual values", () => {
  it("no literal colors outside tokens.css", () => {
    const literal = /#[0-9a-fA-F]{3,8}\b|\brgba?\(|\bhsla?\(/;
    for (const file of styleFiles) {
      const css = stripComments(cssOf(file));
      expect(literal.test(css), `${file} contains a literal color`).toBe(false);
    }
  });

  it("every var(--x) used is defined in tokens.css", () => {
    const defined = new Set([...tokensCss.matchAll(/(--[a-z0-9-]+)\s*:/g)].map((m) => m[1]));
    for (const file of styleFiles) {
      const text = readFileSync(file, "utf8");
      for (const m of text.matchAll(/var\((--[a-z0-9-]+)/g)) {
        // --tint is a local hook set by components to a token value (app.css .tint).
        if (m[1] === "--tint") continue;
        expect(defined.has(m[1] ?? ""), `${file} uses undefined ${m[1]}`).toBe(true);
      }
    }
  });

  it("no backdrop-filter, remote fonts or remote imports anywhere", () => {
    for (const file of [...styleFiles, join(SRC, "tokens.css")]) {
      const css = stripComments(cssOf(file));
      expect(css.includes("backdrop-filter"), `${file}: backdrop-filter`).toBe(false);
      expect(/@import|url\(\s*["']?(https?:)?\/\//.test(css), `${file}: remote resource`).toBe(false);
    }
  });
});

describe("motion stays within guardrail 2 and SPEC_GAPS G8", () => {
  it("transitions name only opacity or transform", () => {
    for (const file of [...styleFiles, join(SRC, "tokens.css")]) {
      const css = stripComments(cssOf(file));
      for (const m of css.matchAll(/transition(?:-property)?\s*:\s*([^;}]+)/g)) {
        const props = (m[1] ?? "").split(",").map((part) => part.trim().split(/\s+/)[0]);
        for (const p of props) {
          expect(["opacity", "transform", "none"], `${file}: transition on ${p}`).toContain(p);
        }
      }
    }
  });

  it("keyframes animate only opacity and transform", () => {
    for (const file of [...styleFiles, join(SRC, "tokens.css")]) {
      const css = stripComments(cssOf(file));
      for (const m of css.matchAll(/@keyframes\s+[\w-]+\s*\{([\s\S]*?\})\s*\}/g)) {
        const props = [...(m[1] ?? "").matchAll(/([a-z-]+)\s*:/g)].map((p) => p[1]);
        for (const p of props) expect(["opacity", "transform"], `${file}: keyframe ${p}`).toContain(p);
      }
    }
  });

  it("durations are tokens of at most 150 ms, and zero under reduced motion", () => {
    const ms = (v: string | undefined) => (v?.endsWith("ms") ? Number.parseFloat(v) : Number.NaN);
    expect(ms(light.get("--motion-duration"))).toBeLessThanOrEqual(150);
    expect(ms(light.get("--motion-duration-fast"))).toBeLessThanOrEqual(150);
    const reduced = block(tokensCss, "@media (prefers-reduced-motion: reduce) {\n  :root {");
    expect(reduced.get("--motion-duration")).toBe("0ms");
    expect(reduced.get("--motion-duration-fast")).toBe("0ms");
    for (const file of styleFiles) {
      const css = stripComments(cssOf(file));
      expect(/\b\d+(\.\d+)?m?s\b/.test(css.replace(/var\([^)]*\)/g, "")), `${file}: literal duration`).toBe(
        false,
      );
    }
  });
});

describe("dark theme", () => {
  it("explicit and system dark blocks are identical", () => {
    expect([...darkSystem.entries()].sort()).toEqual([...darkExplicit.entries()].sort());
  });

  it("every dark override names an existing light token", () => {
    for (const key of darkExplicit.keys()) {
      if (key === "color-scheme") continue;
      expect(light.has(key), `dark-only token ${key}`).toBe(true);
    }
  });

  it("message canvas stays white", () => {
    expect(light.get("--color-message-canvas")).toBe("#ffffff");
    expect(darkExplicit.has("--color-message-canvas")).toBe(false);
  });
});

type Rgb = [number, number, number];

function color(theme: Map<string, string>, token: string, over?: Rgb): Rgb {
  const value = theme.get(token);
  expect(value, `token ${token}`).toBeDefined();
  const v = value ?? "";
  if (v.startsWith("#")) {
    const h = v.slice(1);
    return [0, 2, 4].map((i) => Number.parseInt(h.slice(i, i + 2), 16)) as Rgb;
  }
  const m = /rgba?\(([^)]+)\)/.exec(v);
  const parts = (m?.[1] ?? "").split(",").map((p) => Number.parseFloat(p));
  const [r = 0, g = 0, b = 0, a = 1] = parts;
  const bg = over ?? [255, 255, 255];
  return [r, g, b].map((c, i) => Math.round(c * a + (bg[i] ?? 0) * (1 - a))) as Rgb;
}

function contrast(a: Rgb, b: Rgb): number {
  const lum = (c: Rgb) => {
    const [r, g, bl] = c.map((v) => {
      const s = v / 255;
      return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
    }) as Rgb;
    return 0.2126 * r + 0.7152 * g + 0.0722 * bl;
  };
  const [hi, lo] = [lum(a), lum(b)].sort((x, y) => y - x) as [number, number];
  return (hi + 0.05) / (lo + 0.05);
}

describe("text contrast is at least 4.5:1 (section 11)", () => {
  const pairs: [string, string][] = [
    ["--color-ink", "--color-card"],
    ["--color-ink", "--color-surface-main"],
    ["--color-ink", "--color-selected-row"],
    ["--color-ink", "--color-row-hover"],
    ["--color-text-body", "--color-card"],
    ["--color-text-muted-strong", "--color-card"],
    ["--color-text-muted", "--color-card"],
    ["--color-text-muted", "--color-row-hover"],
    ["--color-text-muted", "--color-input-bg"],
    ["--color-text-muted", "--color-surface-sunken"],
    ["--color-text-muted-on-selected", "--color-selected-row"],
    ["--color-primary-fg", "--color-primary-bg"],
    ["--color-primary-fg", "--color-primary-bg-hover"],
    ["--color-secondary-fg", "--color-secondary-bg"],
    ["--color-tag-fg", "--color-tag-bg"],
    ["--color-highlight-fg", "--color-highlight-bg"],
  ];
  const sidebarText = [
    "--color-sidebar-text",
    "--color-sidebar-label",
    "--color-sidebar-meta",
    "--color-sidebar-badge-text",
  ];

  for (const [name, theme] of [
    ["light", light],
    ["dark", dark],
  ] as const) {
    it(`${name} theme`, () => {
      for (const [fg, bg] of pairs) {
        const ratio = contrast(color(theme, fg), color(theme, bg));
        expect(ratio, `${name}: ${fg} on ${bg} = ${ratio.toFixed(2)}`).toBeGreaterThanOrEqual(4.5);
      }
      const sidebar = color(theme, "--color-sidebar");
      for (const fg of sidebarText) {
        const ratio = contrast(color(theme, fg, sidebar), sidebar);
        expect(ratio, `${name}: ${fg} on sidebar = ${ratio.toFixed(2)}`).toBeGreaterThanOrEqual(4.5);
      }
    });
  }
});
