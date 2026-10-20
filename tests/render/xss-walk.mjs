// T19 harness, webview half, CI-safe (BUILD.md section 15; SPEC_GAPS G56, G68).
//
// Runs a local HTTP "beacon" server and launches the release app on the XSS fixture whose
// beacon host points at it. In perf mode with ZEROMAIL_PERF_WALK the page opens every corpus
// message in turn, lets its body load and settle, and reports what it opened. Anything that
// left the webview (a script that ran, a navigation, an unrewritten URL, an image the proxy
// fetched) would reach the beacon server, so the run fails on any request to it. It also
// fails unless every corpus message was opened, and the server proves it is listening with
// one control request of the harness's own.
//
// Usage (Windows):
//   cargo run -p mail-store --example xss_fixture -- tests/fixtures/generated/xss-walk tests/fixtures/xss/corpus.html http://127.0.0.1:48619
//   node tests/render/xss-walk.mjs --data tests/fixtures/generated/xss-walk --beacon-port 48619 [--binary path]
import { spawn } from "node:child_process";
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = fileURLToPath(new URL("../..", import.meta.url));
const TIMEOUT_MS = 180_000;

const arg = (name, fallback) => {
  const i = process.argv.indexOf(name);
  return i >= 0 ? process.argv[i + 1] : fallback;
};
const exe = process.platform === "win32" ? "zero-mail.exe" : "zero-mail";
const binary = arg("--binary", join(ROOT, "target", "release", exe));
const data = arg("--data", null);
const port = Number(arg("--beacon-port", "48619"));
const corpus = readFileSync(join(ROOT, "tests", "fixtures", "xss", "corpus.html"), "utf8");
const cases = corpus
  .split("<!-- case: ")
  .slice(1)
  .map((c) => c.split(" -->")[0].trim());

if (!existsSync(binary)) {
  console.error(`Binary not found: ${binary}`);
  process.exit(1);
}
if (!data || !existsSync(join(resolve(data), "mail.db"))) {
  console.error("The XSS fixture is required: --data <dir> (see the usage at the top)");
  process.exit(1);
}

const hits = [];
const server = createServer((req, res) => {
  hits.push(`${req.method} ${req.url}`);
  res.writeHead(204);
  res.end();
});
await new Promise((r) => server.listen(port, "127.0.0.1", r));

// Control: the server sees a request the harness makes itself.
await fetch(`http://127.0.0.1:${port}/control`).catch(() => {});
const controlSeen = hits.includes("GET /control");

const dir = mkdtempSync(join(tmpdir(), "zeromail-xss-walk-"));
const appData = join(dir, "data");
cpSync(resolve(data), appData, { recursive: true });
const out = join(dir, "result.json");
const child = spawn(binary, [], {
  env: { ...process.env, ZEROMAIL_PERF_OUT: out, ZEROMAIL_PERF_WALK: "1", ZEROMAIL_DATA_DIR: appData },
  stdio: "ignore",
});
const exited = await new Promise((r) => {
  const timer = setTimeout(() => {
    child.kill();
    r("timeout");
  }, TIMEOUT_MS);
  child.on("exit", () => {
    clearTimeout(timer);
    r("exit");
  });
});
// Anything still in flight from the webview arrives now or never.
await new Promise((r) => setTimeout(r, 1000));
server.close();

let opened = [];
try {
  opened = JSON.parse(readFileSync(out, "utf8")).walk.opened.map((s) => s.replace(/^XSS: /, ""));
} catch {
  // No result: reported below.
}
try {
  rmSync(dir, { recursive: true, force: true, maxRetries: 20, retryDelay: 250 });
} catch {
  console.error(`could not remove ${dir}`);
}

const leaks = hits.filter((h) => h !== "GET /control");
const missing = cases.filter((c) => !opened.includes(c));
const report = { date: new Date().toISOString(), cases: cases.length, exited, control: controlSeen, opened: opened.length, missing, leaks };
mkdirSync(join(ROOT, "tests", "render", "results"), { recursive: true });
writeFileSync(join(ROOT, "tests", "render", "results", "xss-walk.json"), JSON.stringify(report, null, 2));
console.log(JSON.stringify(report, null, 2));
if (!controlSeen || exited !== "exit" || missing.length > 0 || leaks.length > 0) {
  // A CI annotation is readable without signing in to the logs.
  if (process.env.GITHUB_ACTIONS) {
    console.log(`::error title=T19 walk::${JSON.stringify({ ...report, missing: missing.slice(0, 10), leaks: leaks.slice(0, 10) })}`);
  }
  console.error("T19 walk FAILED");
  process.exit(1);
}
console.log(`T19 walk passed: ${opened.length} messages opened, no request left the webview.`);
