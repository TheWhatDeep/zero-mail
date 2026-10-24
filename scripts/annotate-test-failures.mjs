// CI only: turns each failed Rust test in a `cargo test` log into one GitHub error
// annotation with its whole panic message. Annotations can be read without signing in to
// the logs, and GitHub keeps only 10 per step, so one per failure keeps them all visible.
// Usage: node scripts/annotate-test-failures.mjs test.log
import { readFileSync } from "node:fs";

const log = readFileSync(process.argv[2] ?? "test.log", "utf8").replace(/\x1b\[[0-9;]*m/g, "");
const lines = log.split(/\r?\n/);
const sections = [];
let current = null;
for (const line of lines) {
  const head = /^---- (\S+) stdout ----$/.exec(line);
  if (head) {
    current = { name: head[1], body: [] };
    sections.push(current);
  } else if (current && (line === "failures:" || line.startsWith("test result:"))) {
    current = null;
  } else if (current) {
    current.body.push(line);
  }
}
// GitHub's escaping for annotation text: %, CR and LF.
const esc = (s) => s.replace(/%/g, "%25").replace(/\r/g, "%0D").replace(/\n/g, "%0A");
for (const s of sections.slice(0, 9)) {
  const text = s.body.join("\n").trim().split("\n").slice(0, 30).join("\n");
  console.log(`::error title=${esc(s.name)}::${esc(text)}`);
}
const failed = lines.filter((l) => / FAILED$/.test(l) && l.startsWith("test "));
console.log(`::error title=cargo test::${esc(`${failed.length} failed:\n${failed.join("\n")}`)}`);
