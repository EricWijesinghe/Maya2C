// Emits public/health.json for the 3D health view from sources already in the
// repo, so the page never shows a number nobody can check (Standing Order 1).
//
//   - The reality ledger (features.toml): one node per [[subsystem]], with its
//     tier, class and status. These counts are exact and current every build.
//   - The launch gates (src/data/launch-gates.ts): how many are passed.
//   - A measured test count, with the date and command it came from. This is
//     the one figure not parsed live; CI overwrites it with a fresh count on
//     every push (that is what "live from CI" means). Until then it is labelled
//     with its as-of date so no one mistakes it for today's.
//
// Run by `npm run build` before astro; safe to run standalone.
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "../../..");
const siteRoot = resolve(here, "..");

const read = (p) => readFileSync(p, "utf8");

// --- reality ledger -------------------------------------------------------
const toml = read(resolve(root, "features.toml"));
const blocks = toml.split(/^\[\[subsystem\]\]/m).slice(1);
const field = (block, key) => {
  const m = block.match(new RegExp(`^${key}\\s*=\\s*"([^"]*)"`, "m"));
  return m ? m[1] : null;
};
const subsystems = blocks.map((b) => ({
  name: field(b, "name"),
  domain: field(b, "domain"),
  tier: field(b, "tier"),
  class: field(b, "class"),
  status: field(b, "status"),
})).filter((s) => s.name && s.status && s.tier);

const tally = (key) =>
  subsystems.reduce((acc, s) => ((acc[s[key]] = (acc[s[key]] || 0) + 1), acc), {});

// --- launch gates ---------------------------------------------------------
const gatesSrc = read(resolve(siteRoot, "src/data/launch-gates.ts"));
const gatesTotal = (gatesSrc.match(/\bstate:\s*"(done|next|later)"/g) || []).length;
const gatesPassed = (gatesSrc.match(/\bstate:\s*"done"/g) || []).length;

// --- measured tests (CI overwrites this block on every push) --------------
const tests = {
  passed: 2793,
  failed: 0,
  as_of: "2026-09-26",
  source: "cargo test --workspace --profile ci --no-fail-fast, 4 vCPU CI VM",
  note: "Replaced by a fresh count when this runs in CI.",
};

const health = {
  generated: new Date().toISOString(),
  ledger: {
    total: subsystems.length,
    by_status: tally("status"),
    by_tier: tally("tier"),
    by_class: tally("class"),
  },
  gates: { passed: gatesPassed, total: gatesTotal },
  tests,
  subsystems,
};

const out = resolve(siteRoot, "public/health.json");
writeFileSync(out, JSON.stringify(health));
console.log(
  `gen-health: ${subsystems.length} subsystems, ` +
    `${health.ledger.by_status.verified || 0} verified, ` +
    `${gatesPassed}/${gatesTotal} gates -> public/health.json`,
);
