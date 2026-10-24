// T35 (BUILD.md section 15): launch the app twice; the second launch focuses the first.
//
// Starts the release binary on an empty data directory and waits until it reports `ready`,
// then starts it again. The second process must exit on its own within the timeout, the
// first must say it was `focused`, and the first must still be running. The first is then
// stopped by its PID.
//
// Usage: node tests/shell/single-instance.mjs [--binary path]
import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = fileURLToPath(new URL("../..", import.meta.url));
const TIMEOUT_MS = 30_000;
const exe = process.platform === "win32" ? "zero-mail.exe" : "zero-mail";
const i = process.argv.indexOf("--binary");
const binary = i >= 0 ? process.argv[i + 1] : join(ROOT, "target", "release", exe);
if (!existsSync(binary)) {
  console.error(`Binary not found: ${binary}\nBuild it first: npx tauri build --no-bundle`);
  process.exit(1);
}

const dir = mkdtempSync(join(tmpdir(), "zeromail-t35-"));
const out = join(dir, "instance.txt");
const env = { ...process.env, ZEROMAIL_DATA_DIR: join(dir, "data"), ZEROMAIL_INSTANCE_OUT: out };
const lines = () => (existsSync(out) ? readFileSync(out, "utf8").split(/\r?\n/).filter(Boolean) : []);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function until(what, check) {
  const start = Date.now();
  while (!check()) {
    if (Date.now() - start > TIMEOUT_MS) throw new Error(`timed out: ${what}`);
    await sleep(100);
  }
}

/** What each process printed, for the failure report. */
const output = { first: "", second: "" };
function launch(which) {
  const child = spawn(binary, [], { env, stdio: ["ignore", "pipe", "pipe"] });
  const keep = (chunk) => (output[which] = (output[which] + chunk.toString()).slice(-2000));
  child.stdout.on("data", keep);
  child.stderr.on("data", keep);
  return child;
}

/** A failure, also as a GitHub annotation in CI (readable without signing in to the logs). */
function report(message) {
  console.error(message);
  if (process.env.GITHUB_ACTIONS) {
    console.log(`::error title=T35::${message.replace(/%/g, "%25").replace(/\r?\n/g, "%0A")}`);
  }
}

let first;
let failed = false;
let firstCode = null;
try {
  first = launch("first");
  let firstExited = false;
  first.on("exit", (code) => {
    firstExited = true;
    firstCode = code;
  });
  await until("the first instance is ready", () => lines().includes("ready") || firstExited);
  if (firstExited) throw new Error(`the first instance exited on its own (code ${firstCode})`);

  const second = launch("second");
  let secondExit = null;
  second.on("exit", (code) => (secondExit = code ?? -1));
  await until("the second launch exits", () => secondExit !== null);
  await until("the first instance is focused", () => lines().includes("focused"));
  await sleep(500);
  if (firstExited) throw new Error("the first instance stopped");
  if (lines().filter((l) => l === "ready").length !== 1) {
    throw new Error(`the second launch started a second app: ${lines().join(", ")}`);
  }
  console.log(`T35 passed: the second launch exited (code ${secondExit}) and focused the first`);
} catch (err) {
  report(
    `T35 failed: ${err.message}\nnotes: ${lines().join(", ") || "(none)"}\n` +
      `first instance output:\n${output.first || "(none)"}\nsecond instance output:\n${output.second || "(none)"}`,
  );
  failed = true;
} finally {
  if (first && first.exitCode === null) first.kill();
  await sleep(500);
  rmSync(dir, { recursive: true, force: true });
}
if (failed) process.exit(1);
