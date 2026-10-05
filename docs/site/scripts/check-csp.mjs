// Fails the build if any page carries an executable inline <script> whose
// SHA-256 is not in that page's Content-Security-Policy. Such a script would
// be silently blocked in browsers (a broken feature), and allowing it by
// loosening the policy would undo the protection added after the
// 2026-10-06 injection incident. `--print` lists the missing hashes, for
// astro.config.mjs (security.csp.scriptDirective.hashes).
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";
import { createHash } from "node:crypto";

const dist = new URL("../dist/", import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, "$1");
const print = process.argv.includes("--print");
const walk = (d) => readdirSync(d).flatMap((f) => {
  const p = join(d, f);
  return statSync(p).isDirectory() ? walk(p) : p.endsWith(".html") ? [p] : [];
});

const missing = new Map();
let pages = 0;
for (const file of walk(dist)) {
  const html = readFileSync(file, "utf8");
  const meta = html.match(/content-security-policy" content="([^"]+)"/i);
  if (!meta) continue;
  pages++;
  const allowed = new Set([...meta[1].matchAll(/'sha256-([^']+)'/g)].map((m) => m[1]));
  for (const [, attrs, body] of html.matchAll(/<script([^>]*)>([\s\S]*?)<\/script>/g)) {
    if (/\bsrc=/.test(attrs) || /application\/(ld\+)?json/.test(attrs)) continue;
    const h = createHash("sha256").update(body, "utf8").digest("base64");
    if (!allowed.has(h)) missing.set(h, (missing.get(h) ?? []).concat(file.slice(dist.length)));
  }
}
if (print) {
  for (const h of missing.keys()) console.log(`"sha256-${h}",`);
} else if (missing.size) {
  console.error(`check-csp: ${missing.size} inline script(s) without a CSP hash, e.g.:`);
  for (const [h, files] of [...missing].slice(0, 5)) console.error(`  sha256-${h}  (${files.length} pages, first ${files[0]})`);
  console.error("Add them to security.csp.scriptDirective.hashes after reviewing the script.");
  process.exit(1);
} else {
  console.log(`check-csp: ${pages} pages, every inline script hashed`);
}
