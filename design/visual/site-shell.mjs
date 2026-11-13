// Browser checks for the maya2c.dev shell (lane L02), against a built site.
//
//   cd docs/site && npm run build
//   npm install --no-save --prefix design/visual playwright@1.56.1 axe-core@4
//   node design/visual/site-shell.mjs          checks + screenshots
//
// CHROMIUM_PATH picks a Chromium binary (as regress.mjs does). Exits 1 on any
// failure and prints each one. What it proves, at real viewports in Chromium:
//   1. First paint is in the OS theme (light and dark emulated), a manual
//      choice survives a reload, and choosing System removes the stored key.
//   2. At 390 px the header exposes the theme select and the GitHub link by
//      accessible name (inside the menu).
//   3. No page of the site scrolls sideways at 360, 390 or 768 px, and no
//      element pokes past the right edge outside a scroll container.
//   4. axe-core finds no colour-contrast violation on /, /guides/faq/ and
//      /learn/signatures-live/ in either theme.
//   5. Every series, axis and marker colour of the attack-cost plot is at
//      least 3:1 on the plot background, in both themes.
// Screenshots (JPEG, viewport only) land in design/visual/site/.
import { chromium } from "playwright";
import { createServer } from "node:http";
import { existsSync, mkdirSync, readFileSync, statSync } from "node:fs";
import { dirname, extname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const dist = resolve(here, "..", "..", "docs", "site", "dist");
const shots = join(here, "site");
const THEME_KEY = "starlight-theme";
const OVERFLOW_WIDTHS = [360, 390, 768];
const MIN_GRAPHIC_CONTRAST = 3;
const require = (await import("node:module")).createRequire(import.meta.url);
const axeSource = readFileSync(require.resolve("axe-core/axe.min.js"), "utf8");

if (!existsSync(join(dist, "index.html"))) {
  console.error(`site-shell: no build at ${dist}; run npm run build in docs/site first`);
  process.exit(1);
}
mkdirSync(shots, { recursive: true });

// --- a static server for dist/, with the host's 404 behaviour -----------------
const TYPES = { ".html": "text/html", ".css": "text/css", ".js": "text/javascript", ".svg": "image/svg+xml", ".png": "image/png", ".webp": "image/webp", ".avif": "image/avif", ".json": "application/json", ".woff2": "font/woff2", ".xml": "application/xml", ".wasm": "application/wasm", ".webmanifest": "application/manifest+json", ".ico": "image/x-icon", ".txt": "text/plain" };
const server = createServer((req, res) => {
  const path = decodeURIComponent(new URL(req.url, "http://x").pathname);
  let file = join(dist, path);
  if (existsSync(file) && statSync(file).isDirectory()) file = join(file, "index.html");
  const found = existsSync(file) && statSync(file).isFile();
  res.writeHead(found ? 200 : 404, { "content-type": found ? TYPES[extname(file)] ?? "application/octet-stream" : "text/html" });
  res.end(readFileSync(found ? file : join(dist, "404.html")));
});
await new Promise((ok) => server.listen(0, "127.0.0.1", ok));
const BASE = `http://127.0.0.1:${server.address().port}`;

const shotName = (path) => (path === "/" ? "home" : path === "/404" ? "404" : path.split("/").filter(Boolean).pop());
const failures = [];
const fail = (m) => { failures.push(m); console.error(`  FAIL ${m}`); };
const pass = (m) => console.log(`  ok   ${m}`);
const browser = await chromium.launch(process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {});
// The live status poll and fonts from other hosts are not under test; keep the run offline.
const context = async (opts) => {
  const c = await browser.newContext({ deviceScaleFactor: 1, bypassCSP: true, ...opts });
  await c.route(/^https?:\/\/(?!127\.0\.0\.1)/, (r) => r.abort());
  // Recorded in the frame before the first paint: the theme a reader first sees.
  await c.addInitScript(() => {
    requestAnimationFrame(() => {
      const el = document.documentElement;
      window.__firstPaint = { theme: el.dataset.theme, bg: getComputedStyle(el).backgroundColor };
    });
  });
  return c;
};

// --- 1 and 2: theme on first paint, persistence, System clears, phone header ---
console.log("theme");
for (const width of [1440, 390]) {
  for (const scheme of ["light", "dark"]) {
    const ctx = await context({ viewport: { width, height: 900 }, colorScheme: scheme });
    const page = await ctx.newPage();
    const tag = `${width}px OS ${scheme}`;
    for (const path of ["/", "/guides/faq/", "/404"]) {
      await page.goto(BASE + path, { waitUntil: "load" });
      const fp = await page.waitForFunction(() => window.__firstPaint).then((h) => h.jsonValue());
      fp?.theme === scheme ? pass(`${tag} ${path}: first paint ${fp.theme}`) : fail(`${tag} ${path}: first paint ${JSON.stringify(fp)}, expected ${scheme}`);
      const metas = await page.$$eval('meta[name="theme-color"]', (m) => m.length);
      metas === 2 ? pass(`${tag} ${path}: two theme-color metas`) : fail(`${tag} ${path}: ${metas} theme-color metas`);
      if (width === 1440) await page.screenshot({ path: join(shots, `${shotName(path)}-${scheme}-1440.jpg`), type: "jpeg", quality: 78 });
    }
    // The visible picker: in the bar on desktop, the last row of the menu on phones.
    await page.goto(BASE + "/guides/faq/");
    if (width < 800) await page.locator("details.mnav > summary").click();
    const select = page.getByRole("combobox", { name: "Select theme" }).filter({ visible: true }).first();
    if (width < 800) {
      const gh = page.getByRole("link", { name: /GitHub/ }).filter({ visible: true });
      (await select.isVisible()) ? pass(`${tag}: theme select reachable by name`) : fail(`${tag}: no visible "Select theme" combobox`);
      (await gh.count()) > 0 ? pass(`${tag}: GitHub link reachable by name`) : fail(`${tag}: no visible GitHub link`);
      await page.screenshot({ path: join(shots, `menu-${scheme}-${width}.jpg`), type: "jpeg", quality: 78 });
    }
    const other = scheme === "light" ? "dark" : "light";
    await select.selectOption(other);
    await page.reload({ waitUntil: "load" });
    await page.waitForFunction(() => window.__firstPaint);
    const after = await page.evaluate((k) => ({ theme: document.documentElement.dataset.theme, first: window.__firstPaint?.theme, key: localStorage.getItem(k), meta: [...document.querySelectorAll('meta[name="theme-color"]')].map((m) => m.content) }), THEME_KEY);
    after.first === other && after.key === other ? pass(`${tag}: manual ${other} survives reload, first paint ${after.first}`) : fail(`${tag}: after choosing ${other} and reloading got ${JSON.stringify(after)}`);
    after.meta[0] === after.meta[1] ? pass(`${tag}: both theme-color metas follow the choice (${after.meta[0]})`) : fail(`${tag}: theme-color metas ${after.meta} do not follow the choice`);
    if (width < 800) await page.locator("details.mnav > summary").click();
    await page.getByRole("combobox", { name: "Select theme" }).filter({ visible: true }).first().selectOption("auto");
    await page.reload({ waitUntil: "load" });
    const sys = await page.evaluate((k) => ({ theme: document.documentElement.dataset.theme, has: Object.prototype.hasOwnProperty.call(localStorage, k) }), THEME_KEY);
    sys.theme === scheme && !sys.has ? pass(`${tag}: System clears the key and follows the OS`) : fail(`${tag}: after System got ${JSON.stringify(sys)}`);
    await ctx.close();
  }
}

// Phone screenshots at the narrowest width the owner tests, both themes.
for (const scheme of ["light", "dark"]) {
  const ctx = await context({ viewport: { width: 360, height: 780 }, colorScheme: scheme });
  const page = await ctx.newPage();
  for (const path of ["/", "/guides/faq/", "/stats/", "/404"]) {
    await page.goto(BASE + path, { waitUntil: "load" });
    await page.screenshot({ path: join(shots, `${shotName(path)}-${scheme}-360.jpg`), type: "jpeg", quality: 78 });
  }
  await ctx.close();
}

// --- 3: no horizontal overflow on any page --------------------------------------
console.log("overflow");
const sitemap = readFileSync(join(dist, "sitemap-0.xml"), "utf8");
const paths = [...sitemap.matchAll(/<loc>https:\/\/maya2c\.dev([^<]*)<\/loc>/g)].map((m) => m[1]).concat("/404");
for (const width of OVERFLOW_WIDTHS) {
  const ctx = await context({ viewport: { width, height: 800 }, colorScheme: "dark" });
  const page = await ctx.newPage();
  let clean = 0;
  for (const path of paths) {
    await page.goto(BASE + path, { waitUntil: "load" });
    const r = await page.evaluate(() => {
      const vw = innerWidth;
      // An element inside a scroll container (code, wide table) may run long: that is the container's job.
      const scrolls = (el) => { for (let p = el.parentElement; p && p !== document.body; p = p.parentElement) { const o = getComputedStyle(p).overflowX; if (o === "auto" || o === "scroll" || o === "hidden" || o === "clip") return true; } return false; };
      const out = [];
      for (const el of document.body.querySelectorAll("*")) {
        const b = el.getBoundingClientRect();
        if (b.width === 0 || b.height === 0 || b.right <= vw + 1 || scrolls(el)) continue;
        const s = getComputedStyle(el);
        if (s.position === "fixed" || s.visibility === "hidden") continue;
        out.push(`${el.tagName.toLowerCase()}${el.id ? "#" + el.id : ""}${el.classList.length ? "." + [...el.classList].slice(0, 2).join(".") : ""} right=${Math.round(b.right)}`);
      }
      return { sw: document.documentElement.scrollWidth, vw, out: out.slice(0, 3) };
    });
    if (r.sw > r.vw || r.out.length) fail(`${width}px ${path}: scrollWidth ${r.sw} > ${r.vw} or past the edge: ${r.out.join(", ")}`);
    else clean++;
  }
  pass(`${width}px: ${clean}/${paths.length} pages without horizontal overflow`);
  await ctx.close();
}

// --- 4: axe colour contrast ------------------------------------------------------
console.log("contrast");
for (const scheme of ["light", "dark"]) {
  const ctx = await context({ viewport: { width: 1440, height: 900 }, colorScheme: scheme });
  const page = await ctx.newPage();
  for (const path of ["/", "/guides/faq/", "/learn/signatures-live/"]) {
    await page.goto(BASE + path, { waitUntil: "load" });
    await page.addScriptTag({ content: axeSource });
    const v = await page.evaluate(async () => (await window.axe.run(document, { runOnly: ["color-contrast"] })).violations);
    const nodes = v.flatMap((x) => x.nodes.map((n) => `${n.target.join(" ")}: ${n.any[0]?.message ?? ""}`.slice(0, 160)));
    nodes.length ? fail(`${scheme} ${path}: ${nodes.length} contrast violation(s): ${nodes.slice(0, 4).join(" | ")}`) : pass(`${scheme} ${path}: 0 contrast violations`);
  }
  // --- 5: the attack-cost plot ----------------------------------------------------
  await page.goto(BASE + "/guides/signatures/", { waitUntil: "load" });
  const plot = await page.evaluate(() => {
    const svg = document.querySelector("[data-attack] svg");
    if (!svg) return null;
    const rgb = (s) => (s.match(/\d+(\.\d+)?/g) ?? []).slice(0, 3).map(Number);
    return { bg: rgb(getComputedStyle(svg).backgroundColor), marks: [...svg.querySelectorAll(".series, .axis, .marker, .ref")].map((el) => ({ cls: el.getAttribute("class"), c: rgb(getComputedStyle(el).stroke) })) };
  });
  if (!plot || !plot.marks.length) fail(`${scheme}: attack-cost plot not found on /guides/signatures/`);
  else {
    const lum = ([r, g, b]) => [r, g, b].map((v) => { v /= 255; return v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; }).reduce((a, v, i) => a + v * [0.2126, 0.7152, 0.0722][i], 0);
    const ratio = (a, b) => { const [x, y] = [lum(a), lum(b)].sort((p, q) => q - p); return (x + 0.05) / (y + 0.05); };
    const low = plot.marks.map((m) => ({ ...m, r: ratio(m.c, plot.bg) })).filter((m) => m.r < MIN_GRAPHIC_CONTRAST);
    low.length ? fail(`${scheme} attack-cost: ${low.map((m) => `${m.cls} ${m.r.toFixed(2)}:1`).join(", ")}`) : pass(`${scheme} attack-cost: ${plot.marks.length} strokes all >= ${MIN_GRAPHIC_CONTRAST}:1 (lowest ${Math.min(...plot.marks.map((m) => ratio(m.c, plot.bg))).toFixed(2)}:1)`);
  }
  await ctx.close();
}

await browser.close();
server.close();
if (failures.length) {
  console.error(`site-shell: ${failures.length} failure(s)`);
  process.exit(1);
}
console.log("site-shell: all checks passed");
