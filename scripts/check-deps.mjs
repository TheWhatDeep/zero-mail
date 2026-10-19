// M0 dependency check (BUILD.md sections 2 and 16, SPEC_GAPS G12/G13).
//   1. Boundaries: each crate depends only on the internal crates section 3 allows, and every
//      third-party dependency comes from [workspace.dependencies] (the approved list).
//   2. Exclusivity: async-imap is imported only by mail-imap.
//   3. Freshness (with --online): every approved crate's latest release is within 12 months,
//      unless waived below. Advisories are checked separately by `cargo audit` in CI.
// Usage: node scripts/check-deps.mjs [--online]
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = fileURLToPath(new URL("..", import.meta.url));

const ALLOWED_INTERNAL = {
  "mail-store": [],
  "mail-secrets": [],
  "mail-imap": [],
  "mail-smtp": [],
  "mail-auth": ["mail-secrets"],
  "mail-sync": ["mail-imap", "mail-store", "mail-auth"],
  "mail-outbox": ["mail-smtp", "mail-imap", "mail-store"],
  "mail-render": ["mail-store"],
  "mail-search": ["mail-store", "mail-imap"],
  "mail-core": [
    "mail-store", "mail-secrets", "mail-imap", "mail-smtp", "mail-auth",
    "mail-sync", "mail-outbox", "mail-render", "mail-search",
  ],
  "zero-mail": ["mail-core"],
};
const EXCLUSIVE = { "async-imap": ["mail-imap"] };
// Waivers granted by the owner: crate -> reason (SPEC_GAPS G12).
const FRESHNESS_WAIVERS = { oauth2: "owner waiver 2026-10-19, SPEC_GAPS G12" };

const errors = [];

/** Returns { section -> [dependency names] } for the dependency tables of a Cargo.toml. */
function deps(toml) {
  const out = {};
  let section = null;
  for (const raw of toml.split(/\r?\n/)) {
    const line = raw.replace(/#.*$/, "").trim();
    const header = /^\[([^\]]+)\]$/.exec(line);
    if (header) {
      section = header[1];
      continue;
    }
    if (!section || !/dependencies$/.test(section) || !line) continue;
    const name = /^([A-Za-z0-9_-]+)(\.workspace)?\s*=/.exec(line);
    if (name) (out[section] ??= []).push({ name: name[1], line });
  }
  return out;
}

const workspace = readFileSync(join(ROOT, "Cargo.toml"), "utf8");
const approved = new Set((deps(workspace)["workspace.dependencies"] ?? []).map((d) => d.name));
const members = [...workspace.matchAll(/^\s*"((?:crates\/[a-z-]+)|src-tauri)",/gm)].map((m) => m[1]);

for (const member of members) {
  const toml = readFileSync(join(ROOT, member, "Cargo.toml"), "utf8");
  const pkg = /^name\s*=\s*"([^"]+)"/m.exec(toml)?.[1];
  const allowed = ALLOWED_INTERNAL[pkg];
  if (!allowed) {
    errors.push(`${member}: crate ${pkg} is not in the section 3 module table`);
    continue;
  }
  for (const [section, list] of Object.entries(deps(toml))) {
    for (const { name, line } of list) {
      if (!approved.has(name)) {
        errors.push(`${pkg} [${section}]: ${name} is not an approved dependency`);
      } else if (!/workspace\s*=\s*true|\.workspace\s*=\s*true/.test(line)) {
        errors.push(`${pkg} [${section}]: ${name} must use the workspace version`);
      }
      const internal = name in ALLOWED_INTERNAL;
      // Dev-dependencies may re-list an allowed crate with test features, nothing else.
      if (internal && !allowed.includes(name)) {
        errors.push(`${pkg} [${section}]: may not depend on ${name} (section 3)`);
      }
      if (EXCLUSIVE[name] && !EXCLUSIVE[name].includes(pkg)) {
        errors.push(`${pkg}: only ${EXCLUSIVE[name].join(", ")} may import ${name}`);
      }
    }
  }
}

if (process.argv.includes("--online")) {
  const thirdParty = [...approved].filter((n) => !(n in ALLOWED_INTERNAL));
  const yearMs = 365 * 24 * 3600 * 1000;
  for (const name of thirdParty) {
    const res = await fetch(`https://crates.io/api/v1/crates/${name}`, {
      headers: { "User-Agent": "zero-mail-dependency-check" },
      signal: AbortSignal.timeout(15_000),
    });
    if (!res.ok) {
      errors.push(`${name}: crates.io lookup failed (${res.status})`);
      continue;
    }
    const { crate, versions } = await res.json();
    const latest = versions.find((v) => v.num === crate.max_stable_version);
    const age = Date.now() - Date.parse(latest?.created_at ?? 0);
    const status = age <= yearMs ? "ok" : FRESHNESS_WAIVERS[name] ? `waived (${FRESHNESS_WAIVERS[name]})` : "STALE";
    console.log(`${name.padEnd(30)} ${crate.max_stable_version.padEnd(10)} ${latest?.created_at.slice(0, 10)} ${status}`);
    if (status === "STALE") errors.push(`${name}: last release over 12 months ago; add to SPEC_GAPS with a replacement`);
  }
}

if (errors.length > 0) {
  console.error(errors.join("\n"));
  process.exit(1);
}
console.log(`check-deps: ${members.length} crates within section 3 boundaries`);
