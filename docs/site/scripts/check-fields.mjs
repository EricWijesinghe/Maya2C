// Fails the build when a status on the fields page disagrees with features.toml.
//
// Every part in src/data/fields.ts names a [[subsystem]] id and a tag. The tag
// must match what the ledger row says, using the same reading of the schema
// that docs/architecture-vision.md uses:
//   SHIPPED   class REAL, status working or verified
//   RESEARCH  class RESEARCH, status not planned
//   SIM       class SIM
//   PLANNED   status planned
// A mismatch is a claim the tree does not back, so the build stops.
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "../../..");
const siteRoot = resolve(here, "..");

const toml = readFileSync(resolve(root, "features.toml"), "utf8");
const fieldOf = (block, key) => {
  const m = block.match(new RegExp(`^${key}\\s*=\\s*"([^"]*)"`, "m"));
  return m ? m[1] : null;
};
const ledger = new Map(
  toml
    .split(/^\[\[subsystem\]\]/m)
    .slice(1)
    .map((b) => [fieldOf(b, "id"), { class: fieldOf(b, "class"), status: fieldOf(b, "status") }]),
);

const consistent = {
  SHIPPED: (s) => s.class === "REAL" && (s.status === "working" || s.status === "verified"),
  RESEARCH: (s) => s.class === "RESEARCH" && s.status !== "planned",
  SIM: (s) => s.class === "SIM",
  PLANNED: (s) => s.status === "planned",
};

const src = readFileSync(resolve(siteRoot, "src/data/fields.ts"), "utf8");
const parts = [...src.matchAll(/ledger: "([^"]+)",\s*tag: "([A-Z]+)"/g)].map((m) => ({ id: m[1], tag: m[2] }));
const fieldIds = [...src.matchAll(/^ {4}id: "([a-z]+)"/gm)].map((m) => m[1]);

const problems = [];
if (parts.length === 0) problems.push("no parts parsed from src/data/fields.ts");
if (fieldIds.length !== 11) problems.push(`expected 11 fields (the owner's list), found ${fieldIds.length}`);
if (new Set(fieldIds).size !== fieldIds.length) problems.push("duplicate field id");
for (const p of parts) {
  if (!(p.tag in consistent)) {
    problems.push(`${p.id}: unknown tag ${p.tag}`);
    continue;
  }
  const row = ledger.get(p.id);
  if (!row) {
    problems.push(`${p.id}: no [[subsystem]] row in features.toml`);
  } else if (!consistent[p.tag](row)) {
    problems.push(`${p.id}: tagged ${p.tag} but features.toml says class=${row.class} status=${row.status}`);
  }
}

if (problems.length) {
  console.error(`check-fields: ${problems.length} problem(s)`);
  for (const p of problems) console.error(`  - ${p}`);
  process.exit(1);
}
console.log(`check-fields: ${parts.length} parts across ${fieldIds.length} fields match features.toml`);
