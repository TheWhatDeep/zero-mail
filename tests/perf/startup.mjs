// Perf harness (BUILD.md section 14; SPEC_GAPS G41, G44).
//
// Startup: launches the release binary on a copy of a fixture data directory and measures
// cold start (app not running, OS warm) to an interactive cached inbox: the first list rows
// on screen, reported by the page. One warm-up launch is discarded, then the median of
// --runs launches is compared with the 1.5 s budget.
// Scroll (--scroll): one more launch scrolls the 50,000-message list for 300 frames and
// reports frame rate and the most list rows ever in the DOM (budget: 60 fps, 100 rows).
//
// Usage:
//   cargo run --release -p mail-store --example fixture -- tests/fixtures/generated/perf 50000
//   node tests/perf/startup.mjs --data tests/fixtures/generated/perf [--runs 5] [--scroll]
//     [--binary path] [--enforce] [--enforce-scroll]
import { spawn } from "node:child_process";
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = fileURLToPath(new URL("../..", import.meta.url));
const BUDGET_MS = 1500;
const FPS_BUDGET = 60;
const FPS_TOLERANCE = 0.95;
const DOM_ROWS_BUDGET = 100;
const TIMEOUT_MS = 60_000;

const arg = (name, fallback) => {
  const i = process.argv.indexOf(name);
  return i >= 0 ? process.argv[i + 1] : fallback;
};
const has = (name) => process.argv.includes(name);
const runs = Number(arg("--runs", "5"));
const exe = process.platform === "win32" ? "zero-mail.exe" : "zero-mail";
const binary = arg("--binary", join(ROOT, "target", "release", exe));
const data = arg("--data", null);

if (!existsSync(binary)) {
  console.error(`Binary not found: ${binary}\nBuild it first: npx tauri build --no-bundle`);
  process.exit(1);
}
if (!data || !existsSync(join(resolve(data), "mail.db"))) {
  console.error("A fixture data directory is required: --data <dir> (see the usage at the top)");
  process.exit(1);
}

function once({ scroll }) {
  return new Promise((resolvePromise, reject) => {
    const dir = mkdtempSync(join(tmpdir(), "zeromail-perf-"));
    const appData = join(dir, "data");
    cpSync(resolve(data), appData, { recursive: true });
    const out = join(dir, "result.json");
    const t0 = performance.now();
    const env = { ...process.env, ZEROMAIL_PERF_OUT: out, ZEROMAIL_DATA_DIR: appData };
    if (scroll) env.ZEROMAIL_PERF_SCROLL = "1";
    const child = spawn(binary, [], { env, stdio: "ignore" });
    const timer = setTimeout(() => {
      child.kill();
      reject(new Error(`no result within ${TIMEOUT_MS} ms`));
    }, TIMEOUT_MS);
    child.on("error", reject);
    child.on("exit", () => {
      clearTimeout(timer);
      const wall = Math.round(performance.now() - t0);
      try {
        const result = JSON.parse(readFileSync(out, "utf8"));
        resolvePromise({ ...result, wall_ms: wall });
      } catch (err) {
        reject(new Error(`app exited without a result: ${err.message}`));
      } finally {
        rmSync(dir, { recursive: true, force: true });
      }
    });
  });
}

const median = (xs) => [...xs].sort((a, b) => a - b)[Math.floor(xs.length / 2)];

/** A failure, also as a GitHub annotation in CI (readable without signing in to the logs). */
function report(message) {
  console.error(message);
  if (process.env.GITHUB_ACTIONS) {
    console.log(`::error title=startup harness::${message.replace(/%/g, "%25").replace(/\r?\n/g, "%0A")}`);
  }
}

/** A launch that fails says which one it was. */
async function launch(what, opts) {
  try {
    return await once(opts);
  } catch (err) {
    report(`${what}: ${err.message}`);
    process.exit(1);
  }
}

const warmup = await launch("warm-up launch", { scroll: false });
const samples = [];
for (let i = 0; i < runs; i++) samples.push(await launch(`launch ${i + 1} of ${runs}`, { scroll: false }));
const result = {
  date: new Date().toISOString(),
  platform: `${process.platform}-${process.arch}`,
  measure: "cold app, warm OS: launch to the first cached inbox rows (SPEC_GAPS G44)",
  runs,
  warmup_discarded: { startup_ms: warmup.startup_ms, wall_ms: warmup.wall_ms, phases: warmup.phases },
  median_startup_ms: median(samples.map((s) => s.startup_ms)),
  median_wall_ms: median(samples.map((s) => s.wall_ms)),
  budget_ms: BUDGET_MS,
  samples: samples.map((s) => ({ startup_ms: s.startup_ms, wall_ms: s.wall_ms, ui_ms: s.ui?.ui_ms, phases: s.phases })),
};
if (has("--scroll")) {
  const s = await launch("scroll launch", { scroll: true });
  result.scroll = { ...s.scroll, fps_budget: FPS_BUDGET, dom_rows_budget: DOM_ROWS_BUDGET };
}

mkdirSync(join(ROOT, "tests", "perf", "results"), { recursive: true });
writeFileSync(join(ROOT, "tests", "perf", "results", "startup.json"), JSON.stringify(result, null, 2));
console.log(JSON.stringify(result, null, 2));

let failed = false;
if (has("--enforce") && result.median_startup_ms > BUDGET_MS) {
  report(
    `Startup median ${result.median_startup_ms} ms exceeds the ${BUDGET_MS} ms budget; warm-up ${JSON.stringify(result.warmup_discarded)}; samples ${JSON.stringify(result.samples)}`,
  );
  failed = true;
}
if (result.scroll && result.scroll.max_dom_rows > DOM_ROWS_BUDGET) {
  report(`${result.scroll.max_dom_rows} list rows were in the DOM; the budget is ${DOM_ROWS_BUDGET}`);
  failed = true;
}
if (has("--enforce-scroll") && result.scroll && result.scroll.fps < FPS_BUDGET * FPS_TOLERANCE) {
  report(`Scrolling ran at ${result.scroll.fps} fps; the budget is ${FPS_BUDGET}`);
  failed = true;
}
if (failed) process.exit(1);
