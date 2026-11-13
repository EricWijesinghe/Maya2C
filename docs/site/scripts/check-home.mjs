// Fails the build when the built homepage (dist/index.html) breaks a promise
// the L03 redesign made (2026-11-14):
//   1. the title is the one people search with;
//   2. while the testnet is paused, no "Start mining" button and no "live"
//      describing the network; once it runs, "Start mining" is back;
//   3. every node of the transaction graphic and of the timeline is a link
//      with an accessible name (its text or aria-label) to a page (and #anchor) that exists in dist/;
//   4. every figure on the page traces to network-state.ts, progress.json,
//      health.json or a fixed technical name; a new, unsourced number fails;
//   5. every link-preview card is 1200x630, at most 150 KB, and none says
//      "Public testnet".
// `--list` prints each figure with its source, for the lane report.
import { readFileSync, existsSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import sharp from "sharp";
import { testnet } from "../src/data/network-state.ts";

const site = fileURLToPath(new URL("..", import.meta.url));
const dist = join(site, "dist");
const list = process.argv.includes("--list");
const TITLE = "Maya2C: post-quantum layer-1 blockchain";
const OG_MAX_BYTES = 150 * 1024;
const fails = [];
const fail = (m) => fails.push(m);

const html = readFileSync(join(dist, "index.html"), "utf8");
const decode = (s) => s.replace(/&nbsp;/g, " ").replace(/&amp;/g, "&").replace(/&quot;/g, '"').replace(/&#39;|&#x27;/g, "'").replace(/&lt;/g, "<").replace(/&gt;/g, ">");
const attr = (tag, name) => tag.match(new RegExp(`\\s${name}="([^"]*)"`, "i"))?.[1] ?? null;
const textOf = (h) => decode(h.replace(/<(script|style|pre|code)\b[\s\S]*?<\/\1>/gi, " ").replace(/<[^>]+>/g, " ")).replace(/\s+/g, " ");

// 1. Title.
const title = decode(html.match(/<title>([^<]*)<\/title>/)?.[1] ?? "");
if (title !== TITLE) fail(`title is "${title}", want "${TITLE}"`);

// 2. Paused state.
// The page's own content, hero to footnote; the shared header and footer have their own checks.
const from = html.search(/<section\b[^>]*class="hero\b/);
const to = html.indexOf("</p>", html.indexOf('class="maya-footnote'));
const main = from >= 0 && to > from ? html.slice(from, to) : "";
if (!main) fail("homepage content (hero to footnote) not found in dist/index.html");
const primaries = [...main.matchAll(/<a\b[^>]*class="[^"]*ui-btn--primary[^"]*"[^>]*>([\s\S]*?)<\/a>/g)].map((m) => textOf(m[1]).trim());
if (testnet.paused) {
  if (primaries.some((p) => /start mining/i.test(p))) fail("paused, but a primary button says Start mining");
  const live = textOf(main).match(/[^.]{0,40}\blive\b[^.]{0,40}/gi);
  if (live) fail(`paused, but the page says "live": ${live.join(" | ")}`);
} else if (!primaries.some((p) => /start mining/i.test(p))) {
  fail("testnet running, but no primary Start mining button");
}

// 3. Graphic and timeline links.
const fileFor = (path) => {
  const clean = decodeURI(path);
  return [join(dist, clean, "index.html"), join(dist, clean)].find((f) => existsSync(f) && f.endsWith(".html")) ?? null;
};
const region = (re, name) => {
  const m = main.match(re);
  if (!m) fail(`${name}: not found on the page`);
  return m?.[0] ?? "";
};
for (const [name, block] of [
  ["transaction graphic", region(/<figure\b[^>]*data-txp[\s\S]*?<\/figure>/, "transaction graphic")],
  ["timeline", region(/<ol\b[^>]*data-timeline[\s\S]*?<\/ol>/, "timeline")],
]) {
  const links = [...block.matchAll(/(<a\b[^>]*>)([\s\S]*?)<\/a>/g)];
  if (links.length < 4) fail(`${name}: ${links.length} links, want every node linked`);
  for (const [, a, inner] of links) {
    // The accessible name: aria-label, else the link's own text (SVG <text> included).
    const href = attr(a, "href"), label = attr(a, "aria-label") ?? textOf(inner);
    if (!label?.trim()) fail(`${name}: link to ${href} has no accessible name`);
    if (!href?.startsWith("/")) { fail(`${name}: ${href} is not a page on this site`); continue; }
    const [path, frag] = href.split("#");
    const file = fileFor(path);
    if (!file) fail(`${name}: ${href} has no page in dist/`);
    else if (frag && !readFileSync(file, "utf8").includes(`id="${frag}"`)) fail(`${name}: ${href} has no #${frag}`);
  }
}

// 4. Every figure has a source.
const progress = JSON.parse(readFileSync(join(dist, "progress.json"), "utf8"));
const health = JSON.parse(readFileSync(join(dist, "health.json"), "utf8"));
const run = testnet.last;
const sources = new Map();
const add = (src, ...vals) => vals.forEach((v) => v != null && sources.set(String(v), src));
add("network-state.ts testnet.last (stop report)", run.height.toLocaleString("en-US"), run.from, run.to, run.halts, `${Math.floor(run.uptime * 100)}%`, `${run.haltSeconds}`, ...run.at.split(" "), ...run.run.match(/\d+/g), `${parseInt(run.run, 10)}-day`);
add("network-state.ts testnet.since", testnet.since);
add("progress.json gates", progress.gates?.met, progress.gates?.total);
add("progress.json proofs", progress.proofs?.kani_harnesses?.toLocaleString("en-US"), progress.proofs?.conformance_vectors?.toLocaleString("en-US"));
if (progress.finality) add("progress.json finality (committed bench report)", progress.finality.p50_ms.toLocaleString("en-US"), progress.finality.validators);
add("health.json ledger (features.toml)", health.ledger?.by_status?.verified ?? 0, health.ledger?.total);
// Names that contain digits but state no measurement.
add("technical name", "ML-DSA-65", "ML-KEM-1024", "ML-KEM-768", "layer-1", "Maya2C", "SHA256SUMS", "Apache-2.0", "ChaCha20-Poly1305", "Argon2id", "maya-testnet-1", "127.0.0.1:8545", "127.0.0.1:8080", "204", "205", "p50", "status.maya2c.dev");
add("NIST publication year of FIPS 204/205 (/guides/signatures/)", "2024");
const tokens = (textOf(main).match(/[^\s()]*\d[^\s()]*/g) ?? []).map((t) => t.replace(/^[^\w]+|[.,;:!?]+$/g, ""));
const seen = new Map();
for (const t of tokens) {
  const src = sources.get(t);
  if (src) seen.set(t, src);
  else fail(`figure "${t}" has no source: add it to a data file, or render MISSING`);
}
if (list) for (const [t, src] of seen) console.log(`  ${t.padEnd(24)} ${src}`);

// 5. Link-preview cards.
const pages = JSON.parse(readFileSync(join(site, "src/data/og-pages.json"), "utf8"));
for (const e of pages) {
  const rel = e.card ?? `og/pages/${e.slug.replace(/\//g, "-")}.png`;
  const f = join(dist, rel);
  if (!existsSync(f)) { fail(`og: ${rel} missing`); continue; }
  const meta = await sharp(f).metadata();
  const bytes = readFileSync(f).length;
  if (meta.width !== 1200 || meta.height !== 630) fail(`og: ${rel} is ${meta.width}x${meta.height}`);
  if (bytes > OG_MAX_BYTES) fail(`og: ${rel} is ${bytes} bytes, over ${OG_MAX_BYTES}`);
  if (/public testnet/i.test(`${e.title} ${e.line}`)) fail(`og: ${rel} says "Public testnet"`);
  if (list) console.log(`  og ${rel.padEnd(40)} ${meta.width}x${meta.height} ${bytes} bytes`);
}

if (fails.length) {
  for (const m of fails) console.error(`check-home: ${m}`);
  process.exit(1);
}
console.log(`check-home: title, ${testnet.paused ? "paused" : "running"} doors, graphic and timeline links, ${seen.size} sourced figures, ${pages.length} cards`);
