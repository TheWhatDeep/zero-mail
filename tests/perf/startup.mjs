// Startup perf harness (BUILD.md M0 and section 14: cold start to interactive, 1.5 s budget).
// Launches the release binary N times with ZEROMAIL_PERF_OUT set. The app writes its own
// startup time when the main page finishes loading, then exits (src-tauri/src/main.rs).
// M4 moves the measurement point to "cached inbox interactive".
//
// Usage: node tests/perf/startup.mjs [--runs 5] [--binary path] [--enforce]
// --enforce fails the run when the median exceeds the budget. Section 14 lets the first
// measured baseline revise budgets, so CI enforces only after the owner ratifies one.
import { spawn } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = fileURLToPath(new URL("../..", import.meta.url));
const BUDGET_MS = 1500;
const TIMEOUT_MS = 30_000;

const arg = (name, fallback) => {
  const i = process.argv.indexOf(name);
  return i >= 0 ? process.argv[i + 1] : fallback;
};
const runs = Number(arg("--runs", "5"));
const exe = process.platform === "win32" ? "zero-mail.exe" : "zero-mail";
const binary = arg("--binary", join(ROOT, "target", "release", exe));
const enforce = process.argv.includes("--enforce");

if (!existsSync(binary)) {
  console.error(`Binary not found: ${binary}\nBuild it first: npm run tauri build -- --no-bundle`);
  process.exit(1);
}

function once() {
  return new Promise((resolvePromise, reject) => {
    const dir = mkdtempSync(join(tmpdir(), "zeromail-perf-"));
    const out = join(dir, "startup.json");
    const t0 = performance.now();
    const child = spawn(binary, [], {
      env: { ...process.env, ZEROMAIL_PERF_OUT: out },
      stdio: "ignore",
    });
    const timer = setTimeout(() => {
      child.kill();
      reject(new Error(`no startup signal within ${TIMEOUT_MS} ms`));
    }, TIMEOUT_MS);
    child.on("error", reject);
    child.on("exit", () => {
      clearTimeout(timer);
      const wall = performance.now() - t0;
      try {
        const { startup_ms } = JSON.parse(readFileSync(out, "utf8"));
        resolvePromise({ app_ms: startup_ms, wall_ms: Math.round(wall) });
      } catch (err) {
        reject(new Error(`app exited without a result: ${err.message}`));
      } finally {
        rmSync(dir, { recursive: true, force: true });
      }
    });
  });
}

const samples = [];
for (let i = 0; i < runs; i++) samples.push(await once());
const median = (xs) => [...xs].sort((a, b) => a - b)[Math.floor(xs.length / 2)];
const result = {
  date: new Date().toISOString(),
  platform: `${process.platform}-${process.arch}`,
  runs,
  median_app_ms: median(samples.map((s) => s.app_ms)),
  median_wall_ms: median(samples.map((s) => s.wall_ms)),
  budget_ms: BUDGET_MS,
  samples,
};
mkdirSync(join(ROOT, "tests", "perf", "results"), { recursive: true });
writeFileSync(join(ROOT, "tests", "perf", "results", "startup.json"), JSON.stringify(result, null, 2));
console.log(JSON.stringify(result, null, 2));
if (enforce && result.median_wall_ms > BUDGET_MS) {
  console.error(`Startup median ${result.median_wall_ms} ms exceeds the ${BUDGET_MS} ms budget`);
  process.exit(1);
}
