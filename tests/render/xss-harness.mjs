// T19 harness, webview half (BUILD.md section 15; SPEC_GAPS G56).
//
// Launches the app on the XSS fixture with WebView2's DevTools port open, attaches to the
// page and to every frame it creates, records every network request, and opens each corpus
// message in turn (keyboard navigation through the list, then the pointer over the body).
// Every payload that ran, loaded or navigated would reach http://beacon.invalid/<case>, so
// any request to that host, or to any host other than the app's own *.localhost protocols,
// fails the run. The run also fails unless every message was opened and its mail-body
// request was seen, so a harness that sees nothing cannot pass.
//
// Usage (Windows):
//   cargo run -p mail-store --example xss_fixture -- tests/fixtures/generated/xss tests/fixtures/xss/corpus.html
//   node tests/render/xss-harness.mjs --data tests/fixtures/generated/xss [--binary path]
import { spawn } from "node:child_process";
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = fileURLToPath(new URL("../..", import.meta.url));
const TIMEOUT_MS = 180_000;
const SETTLE_MS = 800;

const arg = (name, fallback) => {
  const i = process.argv.indexOf(name);
  return i >= 0 ? process.argv[i + 1] : fallback;
};
const exe = process.platform === "win32" ? "zero-mail.exe" : "zero-mail";
const binary = arg("--binary", join(ROOT, "target", "release", exe));
const data = arg("--data", null);
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

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const port = 9300 + Math.floor(Math.random() * 500);
const dir = mkdtempSync(join(tmpdir(), "zeromail-xss-"));
const appData = join(dir, "data");
cpSync(resolve(data), appData, { recursive: true });
const child = spawn(binary, [], {
  env: {
    ...process.env,
    ZEROMAIL_DATA_DIR: appData,
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port}`,
    // Its own WebView2 profile: a browser process left over from an earlier launch (the
    // perf runs) would otherwise be reused and ignore the DevTools argument.
    WEBVIEW2_USER_DATA_FOLDER: join(dir, "webview2"),
  },
  stdio: "ignore",
});
const deadline = setTimeout(() => {
  console.error(`T19 harness: no result within ${TIMEOUT_MS} ms`);
  if (process.env.GITHUB_ACTIONS) {
    console.log(`::error title=T19 harness::no result within ${TIMEOUT_MS} ms; ${requests.length} requests seen`);
  }
  child.kill();
  process.exit(1);
}, TIMEOUT_MS);

/** Every request any attached target made: { id, url, session }. */
const requests = [];
/** Requests the webview itself blocked (the CSP): request id -> reason. */
const blocked = new Map();
let ws;
let nextId = 1;
const pending = new Map();

function send(method, params = {}, sessionId) {
  const id = nextId++;
  ws.send(JSON.stringify({ id, method, params, sessionId }));
  return new Promise((res, rej) => pending.set(id, { res, rej, method }));
}

async function enable(sessionId) {
  await send("Network.enable", {}, sessionId);
  await send("Runtime.enable", {}, sessionId);
  await send("Target.setAutoAttach", { autoAttach: true, waitForDebuggerOnStart: true, flatten: true }, sessionId);
}

/** The DevTools port: the one asked for, or the one WebView2 wrote to its profile. */
function activePort() {
  const file = join(dir, "webview2", "EBWebView", "DevToolsActivePort");
  try {
    return Number(readFileSync(file, "utf8").split(/\r?\n/)[0]) || null;
  } catch {
    return null;
  }
}

async function connect() {
  for (let i = 0; i < 300; i++) {
    for (const p of new Set([port, activePort()].filter(Boolean))) {
      try {
        const version = await (await fetch(`http://127.0.0.1:${p}/json/version`)).json();
        return version.webSocketDebuggerUrl;
      } catch {
        // Not listening yet.
      }
    }
    await sleep(200);
  }
  // Why: is the app alive, and did WebView2 start with our profile and arguments?
  const list = (p) => {
    try {
      return readdirSync(p).slice(0, 20).join(",");
    } catch {
      return "missing";
    }
  };
  throw new Error(
    `the DevTools port never opened; app exit code ${child.exitCode}; ` +
      `profile [${list(join(dir, "webview2"))}]; EBWebView [${list(join(dir, "webview2", "EBWebView"))}]; ` +
      `active port ${activePort()}`,
  );
}

let page;
async function evaluate(expression) {
  const r = await send("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true }, page);
  return r.result?.value;
}

async function key(code, keyCode) {
  for (const type of ["keyDown", "keyUp"]) {
    await send("Input.dispatchKeyEvent", { type, key: code, code, windowsVirtualKeyCode: keyCode }, page);
  }
}

const report = { date: new Date().toISOString(), cases: cases.length, opened: [], body_requests: 0, violations: [] };
try {
  ws = new WebSocket(await connect());
  await new Promise((res, rej) => {
    ws.onopen = res;
    ws.onerror = rej;
  });
  ws.onmessage = async (event) => {
    const msg = JSON.parse(event.data);
    if (msg.id && pending.has(msg.id)) {
      const p = pending.get(msg.id);
      pending.delete(msg.id);
      if (msg.error) p.rej(new Error(`${p.method}: ${msg.error.message}`));
      else p.res(msg.result);
      return;
    }
    if (msg.method === "Network.requestWillBeSent") {
      requests.push({ id: msg.params.requestId, url: msg.params.request.url, session: msg.sessionId ?? null });
    } else if (msg.method === "Network.loadingFailed" && msg.params.blockedReason) {
      blocked.set(msg.params.requestId, msg.params.blockedReason);
    } else if (msg.method === "Target.attachedToTarget") {
      const child = msg.params.sessionId;
      try {
        await enable(child);
      } catch {
        // A frame that went away before it was enabled.
      }
      await send("Runtime.runIfWaitingForDebugger", {}, child).catch(() => {});
    }
  };

  let target;
  for (let i = 0; i < 100 && !target; i++) {
    const { targetInfos } = await send("Target.getTargets");
    target = targetInfos.find((t) => t.type === "page" && /localhost/.test(t.url));
    if (!target) await sleep(200);
  }
  if (!target) throw new Error("the app page never appeared");
  ({ sessionId: page } = await send("Target.attachToTarget", { targetId: target.targetId, flatten: true }));
  await enable(page);

  // Wait for the list, then open the first message.
  for (let i = 0; i < 150; i++) {
    if ((await evaluate(`document.querySelectorAll('[role="option"]').length`)) > 0) break;
    await sleep(200);
  }
  const href = await evaluate(`location.href`);
  if (!/localhost/.test(href ?? "")) throw new Error(`the app page did not load: ${href}`);
  await evaluate(`document.querySelector('[role="option"]').click()`);

  const seen = new Set();
  let last = null;
  for (let step = 0; step < cases.length * 2 && seen.size < cases.length; step++) {
    let subject = null;
    // The next message is open once the subject changed and its frame is there.
    for (let i = 0; i < 150; i++) {
      subject = await evaluate(`document.querySelector('h2')?.textContent ?? null`);
      const framed = await evaluate(`!!document.querySelector('iframe')`);
      if (subject && subject !== last && framed) break;
      await sleep(100);
    }
    if (subject === last) break;
    last = subject;
    await sleep(SETTLE_MS);
    const rect = await evaluate(
      `(() => { const r = document.querySelector('iframe')?.getBoundingClientRect(); return r ? [r.x + r.width / 2, r.y + Math.min(r.height / 2, 200)] : null; })()`,
    );
    if (rect) {
      await send("Input.dispatchMouseEvent", { type: "mouseMoved", x: rect[0], y: rect[1] }, page);
      await sleep(200);
    }
    if (subject?.startsWith("XSS: ")) seen.add(subject.slice(5));
    // Back to the list, then the next message.
    await evaluate(`document.querySelector('[role="listbox"]')?.focus()`);
    await key("ArrowDown", 40);
  }
  await sleep(SETTLE_MS);
  report.opened = [...seen];
  report.body_requests = requests.filter((r) => /mail-body/.test(r.url)).length;
  for (const r of requests) {
    let url;
    try {
      url = new URL(r.url);
    } catch {
      continue;
    }
    // Any attempt on the beacon host itself, even one the CSP stopped, means a payload ran
    // or an unrewritten URL survived.
    if (url.hostname === "beacon.invalid") {
      report.violations.push(r.url);
      continue;
    }
    if (blocked.has(r.id)) {
      report.blocked = (report.blocked ?? 0) + 1;
      continue;
    }
    const outbound = ["http:", "https:", "ws:", "wss:", "ftp:"].includes(url.protocol);
    const ours = url.hostname === "localhost" || url.hostname.endsWith(".localhost");
    // Images are never allowed in this run, so a mail-remote request the CSP let through
    // would reach the network through the proxy.
    const proxied = url.hostname === "mail-remote.localhost" || url.protocol === "mail-remote:";
    if (proxied || (outbound && !ours)) report.violations.push(r.url);
  }
} catch (err) {
  report.error = String(err);
} finally {
  clearTimeout(deadline);
  ws?.close();
  child.kill();
  await sleep(500);
  rmSync(dir, { recursive: true, force: true });
}

mkdirSync(join(ROOT, "tests", "render", "results"), { recursive: true });
writeFileSync(join(ROOT, "tests", "render", "results", "xss.json"), JSON.stringify({ ...report, requests: requests.map((r) => r.url) }, null, 2));
const missing = cases.filter((c) => !report.opened.includes(c));
console.log(JSON.stringify({ ...report, missing }, null, 2));
const failed = report.error || report.violations.length > 0 || missing.length > 0 || report.body_requests < cases.length;
if (failed) {
  const why = {
    error: report.error,
    violations: report.violations.slice(0, 5),
    missing: missing.slice(0, 10),
    opened: report.opened.length,
    body_requests: report.body_requests,
    requests: requests.length,
  };
  // A CI annotation is readable without signing in to the logs.
  if (process.env.GITHUB_ACTIONS) console.log(`::error title=T19 harness::${JSON.stringify(why)}`);
  console.error("T19 harness FAILED");
  process.exit(1);
}
console.log(`T19 harness passed: ${cases.length} messages opened, ${requests.length} requests, none outbound.`);
process.exit(0);
