// Lane L04: one consistent, paused-aware story on every guide.
// Run: node --test src/data/paused-truth.test.ts (from docs/site)
// The "built site" group reads dist/ and is skipped, by name, until
// `npm run build` has produced it.
import { test } from "node:test";
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { testnet } from "./network-state.ts";
import { HOSTED_SUBSYSTEMS, lastRun, PUBLIC_HOSTS, recorded, runCaveats } from "./paused.ts";
import { release, wallet } from "./release.ts";

const site = new URL("../../", import.meta.url);
const repo = new URL("../../../../", import.meta.url);
const read = (p: string, base = site) => readFileSync(new URL(p, base), "utf8");
const authored = (f: string) => read(`authored/${f}`);

const GUIDES = ["start-mining.mdx", "help-run.mdx", "testnet.mdx", "validators.mdx", "wallet.mdx"];
const CHANNEL = "First channel: GitHub private vulnerability reporting, https://github.com/EricWijesinghe/Maya2C/security/advisories/new";
const S03 =
  "A transaction's signature is decided by its own frame: an untagged frame, which `l1-wallet` signs, is the ML-DSA-65 + SLH-DSA-SHA2-128s hybrid and both must verify; a suite-tagged frame (ML-DSA-87 among others) is opt-in. The governance parameter `crypto.default_signature_suite` (ML-DSA-87 at genesis) never decides what verifies, and no wallet reads it yet: `l1-wallet` signs the hybrid whatever it says.";
const STOP_REPORT = "reports/sessions/2026-10-28-testnet-stopped.md";

test("every paused-sensitive guide renders the one PausedNote", () => {
  for (const g of GUIDES) {
    const src = authored(g);
    assert.match(src, /import PausedNote from ["']\.\.\/\.\.\/\.\.\/components\/PausedNote\.astro["']/, g);
    assert.match(src, /<PausedNote\b/, g);
  }
});

test("the recorded response is the stop report's own line, not retyped", () => {
  assert.ok(read(STOP_REPORT, repo).includes(recorded.body), "recorded.body must appear verbatim in the stop report");
  assert.equal(JSON.parse(recorded.body).height, testnet.last.height);
});

test("the run timeline on /experience/ is the stop report's own dates", () => {
  const report = read(STOP_REPORT, repo);
  assert.ok(report.includes(`Genesis 1792511653 = ${lastRun.genesisAt.replace(" UTC", ":13 UTC")}`), "genesis time");
  assert.ok(report.includes("shows a halt from 07:36:32 UTC: that is this deliberate stop"), "the stop shows as a halt");
});

test("/experience/ shows rest by form, never by greying the brand colour", () => {
  const src = read("src/pages/experience.astro");
  assert.doesNotMatch(src, /saturate\(|grayscale\(/);
  assert.doesNotMatch(src, /--xp-glow:\s*transparent/, "light mode gets a designed shadow, not no effect");
  assert.match(src, /prefers-reduced-motion[\s\S]*is-paused::after[^}]*animation: none/, "the breathing halo stops under reduced motion");
});

test("the 7-day figure carries both recorded caveats and the halt definition", () => {
  const report = read(STOP_REPORT, repo);
  assert.match(report, /71% of stake/);
  assert.match(report, /gap over 60 s/);
  assert.match(read("docs/mainnet-v1-plan.md", repo), /pre-mining build/);
  for (const [name, text] of [
    ["README.md", read("README.md", repo)],
    ["faq.md", authored("faq.md")],
    ["mainnet-gates.ts", read("src/data/mainnet-gates.ts")],
  ]) {
    const flat = text.replace(/\s*\n>?\s*/g, " ");
    assert.match(flat, /71% of stake/, name);
    assert.match(flat, /pre-mining build/, name);
    assert.match(flat, /gap over 60 s/, name);
  }
  assert.match(runCaveats.stake, /71%/);
});

test("mining.md says earn only inside its no-profit disclaimer", () => {
  const src = authored("mining.md");
  const disclaimer = src.match(/:::caution\[No profit promised\][\s\S]*?:::/)?.[0] ?? "";
  assert.ok(disclaimer, "the disclaimer exists");
  assert.equal((src.replace(disclaimer, "").match(/\bearn(s|ing)?\b/gi) ?? []).length, 0);
});

test("no guide says there is no fee market", () => {
  for (const f of ["contracts.md", "wallet.mdx", "transactions.mdx", "faq.md", "api.md"]) {
    assert.doesNotMatch(authored(f).replace(/\s+/g, " "), /no fee market/i, f);
  }
  assert.match(authored("contracts.md").replace(/\s+/g, " "), /base fees per byte where genesis sets `bft\.fees`/);
});

test("api.md's supply example prints no total", () => {
  assert.doesNotMatch(authored("api.md"), /"total":"\d/);
});

test("the security channel is stated identically in all three places", () => {
  const txt = read("public/.well-known/security.txt");
  for (const [name, text] of [["SECURITY.md", read("SECURITY.md", repo)], ["break-it.md", authored("break-it.md")], ["security.txt", txt]]) {
    assert.ok(text.includes(CHANNEL), `${name} names the first channel`);
  }
  // RFC 9116: Contact and Expires are required; Expires is in the future and under a year out.
  const fields = txt.split("\n").filter((l) => l && !l.startsWith("#"));
  assert.equal(fields.find((l) => l.startsWith("Contact:")), "Contact: https://github.com/EricWijesinghe/Maya2C/security/advisories/new");
  const expires = Date.parse(fields.find((l) => l.startsWith("Expires:"))!.slice(8).trim());
  assert.ok(expires > Date.now() && expires - Date.now() < 366 * 86400e3, "Expires within a year");
  assert.match(read("SECURITY.md", repo), /Encrypted channel[^|]*\| \*\*None exists yet\.\*\*/);
});

test("README tagline and no novelty sentence", () => {
  const readme = read("README.md", repo);
  assert.match(readme, /post-quantum layer-1 blockchain/);
  assert.doesNotMatch(readme, /not the first/i);
});

test("S-03: both docs carry the same sentence on what decides a signature", () => {
  assert.ok(authored("pq-comparison.md").includes(S03), "pq-comparison.md");
  assert.ok(read("docs/architecture-vision.md", repo).includes(S03), "architecture-vision.md");
});

test("download links name a tag, never /releases/latest", () => {
  for (const f of ["src/data/release.ts", "src/data/ecosystem-apps.ts", "src/components/GetWallet.astro", "authored/wallet.mdx", "src/pages/experience.astro"]) {
    assert.doesNotMatch(read(f), /releases\/latest/, f);
  }
  if (release.published) {
    for (const [os, name] of Object.entries(wallet)) assert.ok(name, `published release lists the ${os} asset`);
  }
});

// ---- the built site -------------------------------------------------------
const dist = new URL("dist/", site);
const built = existsSync(dist);
const PAGES = ["guides/start-mining", "guides/help-run", "guides/testnet", "guides/validators", "guides/wallet", "experience", "health"];
const page = (p: string) => readFileSync(new URL(`${p}/index.html`, dist), "utf8");
const text = (html: string) =>
  html.replace(/<(script|style)\b[\s\S]*?<\/\1>/gi, " ").replace(/<[^>]+>/g, " ").replace(/&nbsp;|&#160;/g, " ").replace(/\s+/g, " ");
const skip = built ? false : "no dist/: run `npm run build` first";

/** The label of the Starlight tab whose panel contains `at`, or "". */
function tabLabel(html: string, at: number): string {
  const before = html.slice(0, at);
  const open = [...before.matchAll(/<div[^>]*role="tabpanel"[^>]*>/g)].pop();
  if (!open) return "";
  const labelledby = open[0].match(/aria-labelledby="([^"]+)"/)?.[1];
  if (!labelledby) return "";
  const tab = html.match(new RegExp(`<a[^>]*id="${labelledby}"[^>]*>([\\s\\S]*?)</a>`));
  return tab ? text(tab[1]) : "";
}

test("built: no enabled form or copy button reaches a stopped host without the paused label", { skip }, () => {
  if (!testnet.paused) return;
  // Counted, so a markup change that hides every control fails instead of passing empty.
  const seen = { forms: 0, hostCopies: 0, pickerCopies: 0 };
  for (const p of PAGES) {
    const html = page(p);
    for (const form of html.match(/<form\b[\s\S]*?<\/form>/g) ?? []) {
      seen.forms++;
      assert.match(form, /<fieldset[^>]*\bdisabled\b/, `${p}: a form is not disabled`);
      assert.match(text(form), /Paused since/, `${p}: a disabled form says why`);
    }
    for (const m of html.matchAll(/<button\b[^>]*data-code="([^"]*)"[^>]*>/g)) {
      const code = m[1];
      if (!PUBLIC_HOSTS.some((h) => code.includes(h))) continue;
      seen.hostCopies++;
      assert.match(tabLabel(html, m.index!), /paused/i, `${p}: a copy button for ${code.slice(0, 60)} has no paused label`);
    }
    for (const b of html.match(/<button\b[^>]*class="ip-copy\b[^"]*"[^>]*>/g) ?? []) {
      seen.pickerCopies++;
      assert.match(b, /\bdisabled\b/, `${p}: an install-picker Copy is enabled`);
    }
  }
  assert.ok(seen.forms > 0 && seen.hostCopies > 0 && seen.pickerCopies > 0, `checked nothing: ${JSON.stringify(seen)}`);
});

test("built: every height or block count is at most the last real height, or an example", { skip }, () => {
  const max = testnet.last.height;
  for (const p of PAGES) {
    const t = text(page(p));
    for (const m of t.matchAll(/\b(?:block|blocks|height|tip)\b([^.;:()]{0,40})/gi)) {
      for (const n of m[1].match(/\d{1,3}(?:,\d{3})+|\d{4,}/g) ?? []) {
        const v = Number(n.replace(/,/g, ""));
        if (v <= max) continue;
        assert.match(t.slice(m.index!, m.index! + 90), /example/i, `${p}: "${m[0].trim()}" is above ${max} and not labelled`);
      }
    }
  }
});

test("built: a hosted service's status chip is not green while paused", { skip }, () => {
  if (!testnet.paused) return;
  const html = page("guides/testnet");
  const chips = html.match(/<a[^>]*data-paused-chip[^>]*>[\s\S]*?<\/a>/g) ?? [];
  assert.equal(chips.length, 2, "faucet and chain status");
  for (const c of chips) {
    assert.match(c, /ui-pill--warn/);
    assert.match(text(c), /offline until next genesis/);
  }
  assert.ok(HOSTED_SUBSYSTEMS.includes("testnet-faucet"));
});

test("built: /experience/ opens with a lede and one action that works today", { skip }, () => {
  const html = page("experience");
  assert.match(html, /class="xp-lede/);
  if (testnet.paused) assert.match(html, /class="xp-cta[^"]*" href="\/guides\/quickstart\/"/);
});

test("built: the wallet guide's download buttons are off until the release exists", { skip }, () => {
  const html = page("guides/wallet");
  assert.match(html, /data-get-wallet/);
  assert.doesNotMatch(html, /releases\/latest/);
  if (!release.published) {
    assert.doesNotMatch(html, /releases\/download\//);
    assert.match(text(html), /Preview build not released yet/);
    assert.match(html, /href="\/guides\/quickstart\/#1-install-the-tools-and-build"/, "the unreleased card offers the build-from-source path");
    assert.doesNotMatch(html, /owner's approval/, "no process talk on a public page");
  }
});
