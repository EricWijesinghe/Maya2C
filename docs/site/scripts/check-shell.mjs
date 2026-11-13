// Fails the build when the site shell (L02) regresses in a way a reader sees
// before anything else: the wrong theme on first paint, a browser bar in the
// wrong colour, a breadcrumb that 404s, a 300 KB logo, a split wordmark, a
// colour written by hand instead of a token. Reads dist/ and src/ only.
//
//   node scripts/check-shell.mjs          after `astro build`
import { readFileSync, readdirSync, statSync, existsSync } from "node:fs";
import { join, relative, sep, extname } from "node:path";
import { fileURLToPath } from "node:url";

const SITE = "https://maya2c.dev";
const root = fileURLToPath(new URL("../", import.meta.url));
const dist = join(root, "dist");
const src = join(root, "src");
const problems = [];
const fail = (m) => problems.push(m);
const read = (p) => readFileSync(p, "utf8");
const walk = (d, keep) =>
  readdirSync(d).flatMap((f) => {
    const p = join(d, f);
    return statSync(p).isDirectory() ? (["node_modules", "dist", ".astro"].includes(f) ? [] : walk(p, keep)) : keep(p) ? [p] : [];
  });

// --- 1. Theme before paint, and the browser bar per theme -------------------
// The two --bg values the theme-color metas must carry, from the generated tokens.
const tokens = read(join(src, "styles", "tokens.css"));
const bgOf = (sel) => tokens.match(new RegExp(`${sel.replace(/[[\]"]/g, "\\$&")}\\s*\\{[^}]*?--bg:\\s*(#[0-9a-f]{6})`, "i"))?.[1]?.toLowerCase();
const bg = { light: bgOf('[data-theme="light"]'), dark: bgOf('[data-theme="dark"]') };
if (!bg.light || !bg.dark) fail(`tokens.css: could not read --bg for both themes (${JSON.stringify(bg)})`);

for (const page of ["index.html", "guides/faq/index.html", "404.html"]) {
  const file = join(dist, page);
  if (!existsSync(file)) { fail(`${page}: not built`); continue; }
  const html = read(file);
  const head = html.slice(0, html.indexOf("</head>"));
  const theme = [...head.matchAll(/<script(?![^>]*\bsrc=)[^>]*>([\s\S]*?)<\/script>/gi)].find((m) => m[1].includes("starlight-theme") && m[1].includes("prefers-color-scheme"));
  const sheet = head.search(/<link\b[^>]*rel="stylesheet"/i);
  if (!theme) fail(`${page}: no inline theme script in <head>`);
  else if (sheet !== -1 && theme.index > sheet) fail(`${page}: theme script comes after the first stylesheet (first paint can flash)`);
  const metas = [...head.matchAll(/<meta\b[^>]*name="theme-color"[^>]*>/gi)].map((m) => m[0]);
  if (metas.length !== 2) fail(`${page}: expected two theme-color metas, found ${metas.length}`);
  for (const scheme of ["light", "dark"]) {
    const m = metas.find((t) => t.includes(`(prefers-color-scheme: ${scheme})`));
    const content = m?.match(/content="([^"]+)"/)?.[1]?.toLowerCase();
    if (!m) fail(`${page}: no theme-color meta for ${scheme}`);
    else if (content !== bg[scheme]) fail(`${page}: ${scheme} theme-color ${content} is not the ${scheme} --bg token ${bg[scheme]}`);
  }
}

// An installed app's window chrome: the manifest carries the dark default's --bg.
const manifest = JSON.parse(read(join(root, "public", "site.webmanifest")));
for (const k of ["theme_color", "background_color"]) {
  if (manifest[k]?.toLowerCase() !== bg.dark) fail(`site.webmanifest: ${k} ${manifest[k]} is not the dark --bg token ${bg.dark}`);
}

// --- 2. Structured data: crumbs that resolve, a free offer, no 404 article ---
const fileFor = (urlPath) => {
  const clean = decodeURI(urlPath.split(/[?#]/)[0]);
  const c = clean.endsWith("/") ? [join(dist, clean, "index.html")] : [join(dist, clean), join(dist, clean, "index.html"), join(dist, `${clean}.html`)];
  return c.find((p) => existsSync(p) && statSync(p).isFile()) ?? null;
};
const pages = walk(dist, (p) => p.endsWith(".html"));
let crumbs = 0;
for (const file of pages) {
  const rel = relative(dist, file).split(sep).join("/");
  const html = read(file);
  for (const [, body] of html.matchAll(/<script\b[^>]*type="application\/ld\+json"[^>]*>([\s\S]*?)<\/script>/gi)) {
    let nodes;
    try { const d = JSON.parse(body); nodes = d["@graph"] ?? [d]; } catch { continue; } // check-seo reports parse errors
    for (const n of nodes.filter((x) => x["@type"] === "BreadcrumbList")) {
      if (rel === "404.html") fail("404.html: carries a BreadcrumbList");
      for (const item of n.itemListElement ?? []) {
        crumbs++;
        if (!String(item.item).startsWith(`${SITE}/`) || !fileFor(new URL(item.item).pathname)) fail(`${rel}: breadcrumb "${item.name}" points at ${item.item}, which is not built`);
      }
    }
    for (const n of nodes.filter((x) => x["@type"] === "SoftwareApplication")) {
      if (n.offers?.price !== "0") fail(`${rel}: SoftwareApplication without a free Offer (price "0")`);
    }
  }
}
const nf = existsSync(join(dist, "404.html")) ? read(join(dist, "404.html")) : "";
if (!/name="robots"[^>]*noindex/.test(nf)) fail("404.html: missing or not noindex");

// --- 3. Header mark: the SVG orb on every page, under 30 KB --------------------
const LOGO = "/brand/orb-sm.svg";
const LOGO_MAX = 30 * 1024;
const logoBytes = existsSync(join(dist, LOGO)) ? statSync(join(dist, LOGO)).size : -1;
if (logoBytes < 0 || logoBytes > LOGO_MAX) fail(`${LOGO}: ${logoBytes} bytes, expected 0 < size <= ${LOGO_MAX}`);
for (const file of pages) {
  const html = read(file);
  const brand = html.match(/<a\b[^>]*class="brand[^"]*"[^>]*>([\s\S]*?)<\/a>/i)?.[1];
  if (brand && !brand.includes(`src="${LOGO}"`)) fail(`${relative(dist, file)}: header mark is not ${LOGO}`);
}

// --- 4. Sitemap lastmod: from commit dates, never on live pages ----------------
const sitemap = existsSync(join(dist, "sitemap-0.xml")) ? read(join(dist, "sitemap-0.xml")) : "";
const entry = (path) => sitemap.match(new RegExp(`<url><loc>${SITE}${path}</loc>(.*?)</url>`))?.[1];
if (!entry("/guides/faq/")?.includes("<lastmod>")) fail("sitemap: /guides/faq/ has no lastmod");
for (const live of ["/stats/", "/health/"]) {
  const e = entry(live);
  if (e === undefined) fail(`sitemap: ${live} missing`);
  else if (e.includes("<lastmod>")) fail(`sitemap: ${live} is rebuilt from live data and must not carry a lastmod`);
}

// --- 5. Source rules: one-word wordmark, nav, colour literals ------------------
const COMPONENTS = join(src, "components");
const header = read(join(COMPONENTS, "Header.astro"));
if (/built by eric/i.test(header)) fail("Header.astro: 'Built by Eric' belongs in the footer only");
for (const f of walk(src, (p) => /\.(astro|ts|mdx?|css)$/.test(p))) {
  const text = read(f);
  if (/Maya 2C/.test(text)) fail(`${relative(root, f)}: "Maya 2C" splits the one-word brand`);
  if (/Maya<\/span>\s*<span[^>]*>2C|Maya\s*<span[^>]*>2C/.test(text)) fail(`${relative(root, f)}: a span splits the wordmark`);
}
// Port of `cargo xtask design-tokens --literals` (lane L01): every hex or
// functional colour written by hand outside tokens.css. Only the platform's
// theme-color meta may carry one, because the browser reads it before CSS.
const HEX = /(?:^|[^&\w#])(#(?:[0-9a-fA-F]{8}|[0-9a-fA-F]{6}|[0-9a-fA-F]{3,4}))\b/g;
const FUNC = /(?:^|[^\w-])((?:rgba?|hsla?|hwb|oklch|oklab|lab|lch)\(\s*[\d.])/gi;
const TOKENISED = [
  join(src, "styles"),
  ...["Head", "Header", "Footer", "SiteTitle", "Maintenance", "Banner", "CommandPalette", "Support", "DeliveryStates", "RoadmapTimeline", "ThemeSelect", "MenuPanel", "Sidebar"].map((n) => join(COMPONENTS, `${n}.astro`)),
  join(COMPONENTS, "math"),
  join(src, "pages", "learn", "signatures-live.astro"),
  join(src, "pages", "404.astro"),
];
let literals = 0;
for (const target of TOKENISED) {
  const files = statSync(target).isDirectory() ? walk(target, (p) => [".css", ".astro", ".ts"].includes(extname(p))) : [target];
  for (const f of files.filter((p) => !p.endsWith("tokens.css"))) {
    read(f).split(/\r?\n/).forEach((line, i) => {
      if (line.includes('name="theme-color"')) return;
      for (const re of [HEX, FUNC]) for (const m of line.matchAll(re)) { literals++; fail(`${relative(root, f)}:${i + 1}: colour literal ${m[1]}; name a token`); }
    });
  }
}

if (problems.length) {
  console.error(`check-shell: ${problems.length} problem(s)`);
  for (const p of problems.slice(0, 40)) console.error(`  ${p}`);
  process.exit(1);
}
console.log(`check-shell: theme script before CSS and two theme-color metas on 3 pages; ${crumbs} breadcrumb items resolve across ${pages.length} pages; header mark ${logoBytes} B; sitemap lastmod ok; 0 colour literals in ${TOKENISED.length} shell targets`);
