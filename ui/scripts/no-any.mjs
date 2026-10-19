// Fails when TypeScript or Svelte source uses the `any` type (CLAUDE.md: strict, no `any`).
// A dependency-free stand-in for ESLint's no-explicit-any (SPEC_GAPS G13).
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, extname } from "node:path";

const root = process.argv[2] ?? "src";
const pattern = /(?::\s*any\b|\bas\s+any\b|<any>|\bany\[\]|Array<any>|Record<[^>]*\bany\b)/;
const offenders = [];

function walk(dir) {
  for (const name of readdirSync(dir)) {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) {
      walk(path);
    } else if ([".ts", ".svelte"].includes(extname(path))) {
      readFileSync(path, "utf8")
        .split("\n")
        .forEach((line, i) => {
          const code = line.replace(/\/\/.*$/, "");
          if (pattern.test(code)) offenders.push(`${path}:${i + 1}: ${line.trim()}`);
        });
    }
  }
}

walk(root);
if (offenders.length > 0) {
  console.error("Explicit `any` is not allowed:\n" + offenders.join("\n"));
  process.exit(1);
}
console.log(`no-any: ${root} clean`);
