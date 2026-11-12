// Fails the build when a built page breaks a search-engine basic that a crawler
// would otherwise silently punish: a missing or duplicated <title>, a missing
// description, a canonical that is absent or points elsewhere, JSON-LD that does
// not parse, or an internal link or image that goes nowhere. Reads dist/ only,
// so it checks what is actually served. `--verbose` lists every page checked.
import { readFileSync, readdirSync, statSync, existsSync } from "node:fs";
import { join, relative, sep } from "node:path";
import { fileURLToPath } from "node:url";

const SITE = "https://maya2c.dev";
const dist = fileURLToPath(new URL("../dist/", import.meta.url));
const verbose = process.argv.includes("--verbose");

const walk = (d) =>
  readdirSync(d).flatMap((f) => {
    const p = join(d, f);
    return statSync(p).isDirectory() ? walk(p) : p.endsWith(".html") ? [p] : [];
  });

// Decode the few entities Astro emits inside attribute values and text.
const decode = (s) =>
  s
    .replace(/&amp;/g, "&")
    .replace(/&quot;/g, '"')
    .replace(/&#39;|&#x27;/g, "'")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .trim();

const attr = (tag, name) => {
  const m = tag.match(new RegExp(`\\s${name}="([^"]*)"`, "i"));
  return m ? decode(m[1]) : null;
};

// Route a URL path to the file Astro would have written for it.
const fileFor = (urlPath) => {
  const clean = decodeURI(urlPath.split(/[?#]/)[0]);
  const candidates = clean.endsWith("/")
    ? [join(dist, clean, "index.html")]
    : [join(dist, clean), join(dist, clean, "index.html"), join(dist, `${clean}.html`)];
  return candidates.find((c) => existsSync(c) && statSync(c).isFile()) ?? null;
};

const pageUrl = (file) => {
  const rel = relative(dist, file).split(sep).join("/");
  if (rel === "404.html") return null; // served for any unknown path; no canonical by design
  if (rel === "index.html") return `${SITE}/`;
  return rel.endsWith("/index.html")
    ? `${SITE}/${rel.slice(0, -"index.html".length)}`
    : `${SITE}/${rel.slice(0, -".html".length)}/`;
};

const problems = [];
const titles = new Map();
const files = walk(dist);

for (const file of files) {
  const rel = relative(dist, file).split(sep).join("/");
  const html = readFileSync(file, "utf8");
  const headEnd = html.indexOf("</head>");
  const head = headEnd === -1 ? html : html.slice(0, headEnd);
  const fail = (msg) => problems.push(`${rel}: ${msg}`);

  // Title and description. Starlight's <meta name="description"> is the one a crawler reads.
  const title = decode(html.match(/<title>([\s\S]*?)<\/title>/i)?.[1] ?? "");
  if (!title) fail("missing or empty <title>");
  else titles.set(title, [...(titles.get(title) ?? []), rel]);

  // A page that opts out of the index (the Go vanity import) needs no snippet or canonical.
  const noindex = /<meta\b[^>]*name="robots"[^>]*content="[^"]*noindex/i.test(head);

  const desc = [...head.matchAll(/<meta\b[^>]*>/gi)]
    .map((m) => m[0])
    .find((t) => /\bname="description"/i.test(t));
  if (!noindex && (!desc || !attr(desc, "content"))) fail("missing or empty meta description");

  // One h1 per page: the headline a crawler reads as the topic.
  const h1s = (html.match(/<h1[\s>]/gi) ?? []).length;
  if (!noindex && h1s !== 1) fail(`expected one <h1>, found ${h1s}`);
  // No skipped levels (h1 then h3): an outline a screen reader or crawler reads top-down.
  let prevLevel = 0;
  for (const [, lvl] of html.matchAll(/<h([1-6])[\s>]/gi)) {
    if (prevLevel && +lvl > prevLevel + 1) {
      fail(`heading skips from h${prevLevel} to h${lvl}`);
      break;
    }
    prevLevel = +lvl;
  }

  // Every image needs an alt attribute (empty is allowed for decoration).
  for (const [tag] of html.matchAll(/<img\b[^>]*>/gi)) {
    if (!/\salt(=|\s|>|\/)/i.test(tag)) fail(`<img> without alt: ${tag.slice(0, 80)}`);
  }

  // Link-preview image: must be served from this site and exist in the build.
  const ogImage = [...head.matchAll(/<meta\b[^>]*>/gi)]
    .map((m) => m[0])
    .find((t) => /\bproperty="og:image"/i.test(t));
  const ogSrc = ogImage ? attr(ogImage, "content") : null;
  if (!noindex && !ogSrc) fail("missing og:image");
  else if (ogSrc && (!ogSrc.startsWith(`${SITE}/`) || !fileFor(ogSrc.slice(SITE.length)))) fail(`og:image not served: ${ogSrc}`);

  // Canonical: present on every indexable page and pointing at this page's own URL.
  const expected = pageUrl(file);
  if (expected && !noindex) {
    const canon = [...head.matchAll(/<link\b[^>]*>/gi)]
      .map((m) => m[0])
      .find((t) => /\brel="canonical"/i.test(t));
    const href = canon ? attr(canon, "href") : null;
    if (!href) fail("missing canonical link");
    else if (href !== expected) fail(`canonical ${href} does not match page URL ${expected}`);
  }

  // JSON-LD must parse; a malformed block is ignored by search engines without any error.
  const ld = [];
  for (const [, body] of html.matchAll(/<script\b[^>]*type="application\/ld\+json"[^>]*>([\s\S]*?)<\/script>/gi)) {
    try {
      ld.push(JSON.parse(body));
    } catch (e) {
      fail(`JSON-LD does not parse: ${e.message}`);
    }
  }
  // Audit W-13: every docs article carries dateModified, and the FAQ carries FAQPage.
  const nodes = ld.flatMap((d) => d["@graph"] ?? [d]);
  for (const a of nodes.filter((n) => n["@type"] === "TechArticle")) {
    if (!a.dateModified || Number.isNaN(Date.parse(a.dateModified))) fail("TechArticle without a valid dateModified");
  }
  if (rel === "guides/faq/index.html" && !nodes.some((n) => n["@type"] === "FAQPage" && n.mainEntity?.length)) fail("FAQ without FAQPage structured data");

  // Internal links and images. External URLs and in-page fragments are skipped.
  for (const [tag] of html.matchAll(/<(a|link|img|script)\b[^>]*>/gi)) {
    const target = attr(tag, "href") ?? attr(tag, "src");
    if (!target || /^(https?:|mailto:|tel:|data:|#|javascript:)/i.test(target)) continue;
    if (target.startsWith("//")) continue;
    if (!target.startsWith("/")) continue; // relative paths are not used in this site's output
    if (!fileFor(target)) fail(`broken internal link ${target}`);
  }
}

for (const [title, where] of titles) {
  if (where.length > 1) problems.push(`duplicate <title> "${title}" on ${where.join(", ")}`);
}

if (verbose) for (const f of files) console.log(relative(dist, f).split(sep).join("/"));

if (problems.length) {
  console.error(`check-seo: ${problems.length} problem(s) across ${files.length} pages`);
  for (const p of problems.slice(0, 40)) console.error(`  ${p}`);
  process.exit(1);
}
console.log(`check-seo: ${files.length} pages, no duplicate titles, descriptions and canonicals present, JSON-LD parses (dateModified on articles, FAQPage on the FAQ), internal links resolve`);
