// Browser checks for the maya2c.dev shell (lane L02), against a built site.
//
//   cd docs/site && npm run build
//   npm install --no-save --prefix design/visual playwright@1.56.1 axe-core@4.10.2
//   node design/visual/site-shell.mjs          checks + screenshots
//   (or `npm run test:shell` in docs/site; CI runs it in the pinned
//   mcr.microsoft.com/playwright:v1.56.1-noble container, job `site shell`)
//
// CHROMIUM_PATH picks a Chromium binary (as regress.mjs does). Exits 1 on any
// failure and prints each one. What it proves, at real viewports in Chromium:
//   1. First paint is in the OS theme (light and dark emulated), a manual
//      choice survives a reload, and choosing System removes the stored key
//      (also from a manual choice equal to the OS, where the theme does not change).
//   2. At 390 px each page shows exactly one menu control, and the menu
//      exposes the "Theme" switch and the GitHub link by accessible name, on a
//      docs page (Starlight's button) and a splash page (the header's).
//   2b. At 768 and 880 px the header menu opens under its own button, reports
//      aria-expanded, and closes on Escape and on an outside tap; at 1024 px
//      the inline nav replaces it.
//   3. No page of the site scrolls sideways at 360, 390 or 768 px, and no
//      element pokes past the right edge outside a scroll container.
//   2c. At 390 px every visible link, button, summary and label in the open
//      menu and in the footer is at least 44 px high (the owner's tap rule).
//   4. axe-core (WCAG 2.0/2.1/2.2 A and AA rule tags, not contrast alone) finds
//      no violation on /, /guides/faq/, /learn/signatures-live/, /404 and the
//      open phone menu, in either theme.
//   5. Every series, axis and marker colour of the attack-cost plot is at
//      least 3:1 on the plot background, in both themes.
// Screenshots (JPEG) land in design/visual/site/: pages at 360 and 1440, the
// menus at 390/768/880, the header at 1024, and the charts and footer as
// element shots at 360 and 1440, in both themes.
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
const MIN_TAP = 44;
const AXE_TAGS = ["wcag2a", "wcag2aa", "wcag21a", "wcag21aa", "wcag22aa"];
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

// --- 1 and 2: theme on first paint, persistence, System clears, phone menu ----
// The theme switch is three radios (System, Light, Dark) in a "Theme" group.
// On a docs page below 50rem the one menu button is Starlight's (it opens the
// sidebar, whose top block is the site menu); on a splash page it is the
// header's own. Either way exactly one menu control is visible.
const MENU_CONTROLS = ["button.sl-menu-button", "details.mnav > summary"];
const openMenu = async (page) => {
  for (const sel of MENU_CONTROLS) {
    const l = page.locator(sel).filter({ visible: true });
    if (await l.count()) { await l.first().click(); return sel; }
  }
  return null;
};
const radio = (page, name) => page.getByRole("radio", { name, exact: true }).filter({ visible: true }).first();
const shot = (page, name) => page.screenshot({ path: join(shots, `${name}.jpg`), type: "jpeg", quality: 72 });

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
      if (width === 1440) await shot(page, `${shotName(path)}-${scheme}-1440`);
      if (width < 800) {
        const visible = [];
        for (const sel of MENU_CONTROLS) if (await page.locator(sel).filter({ visible: true }).count()) visible.push(sel);
        visible.length === 1 ? pass(`${tag} ${path}: one menu control (${visible[0]})`) : fail(`${tag} ${path}: ${visible.length} menu controls visible (${visible.join(", ")})`);
      }
    }
    // The switch: in the bar on desktop, inside the one menu on phones.
    for (const path of width < 800 ? ["/guides/faq/", "/"] : ["/guides/faq/"]) {
      await page.goto(BASE + path, { waitUntil: "load" });
      if (width < 800) await openMenu(page);
      const group = page.getByRole("group", { name: "Theme" }).filter({ visible: true });
      const gh = page.getByRole("link", { name: /GitHub/ }).filter({ visible: true });
      (await group.count()) > 0 ? pass(`${tag} ${path}: theme switch reachable by name`) : fail(`${tag} ${path}: no visible "Theme" group`);
      (await gh.count()) > 0 ? pass(`${tag} ${path}: GitHub link reachable by name`) : fail(`${tag} ${path}: no visible GitHub link`);
      if (width < 800) await shot(page, `menu-${path === "/" ? "home" : "faq"}-${scheme}-${width}`);
    }
    await page.goto(BASE + "/guides/faq/", { waitUntil: "load" });
    if (width < 800) await openMenu(page);
    const other = scheme === "light" ? "dark" : "light";
    await radio(page, other === "light" ? "Light" : "Dark").check();
    await page.reload({ waitUntil: "load" });
    await page.waitForFunction(() => window.__firstPaint);
    const after = await page.evaluate((k) => ({ theme: document.documentElement.dataset.theme, first: window.__firstPaint?.theme, key: localStorage.getItem(k), meta: [...document.querySelectorAll('meta[name="theme-color"]')].map((m) => m.content) }), THEME_KEY);
    after.first === other && after.key === other ? pass(`${tag}: manual ${other} survives reload, first paint ${after.first}`) : fail(`${tag}: after choosing ${other} and reloading got ${JSON.stringify(after)}`);
    after.meta[0] === after.meta[1] ? pass(`${tag}: both theme-color metas follow the choice (${after.meta[0]})`) : fail(`${tag}: theme-color metas ${after.meta} do not follow the choice`);
    // System from a manual choice equal to the OS: the theme does not change, the key must still go.
    if (width < 800) await openMenu(page);
    await radio(page, scheme === "light" ? "Light" : "Dark").check();
    await radio(page, "System").check();
    const now = await page.evaluate((k) => localStorage.getItem(k), THEME_KEY);
    now === null ? pass(`${tag}: System from manual ${scheme} removes the key at once`) : fail(`${tag}: after System the key is ${JSON.stringify(now)}`);
    await page.reload({ waitUntil: "load" });
    const sys = await page.evaluate((k) => ({ theme: document.documentElement.dataset.theme, pick: document.documentElement.dataset.themePick, has: Object.prototype.hasOwnProperty.call(localStorage, k) }), THEME_KEY);
    sys.theme === scheme && sys.pick === "system" && !sys.has ? pass(`${tag}: System clears the key and follows the OS`) : fail(`${tag}: after System got ${JSON.stringify(sys)}`);
    await ctx.close();
  }
}

// --- the header menu between phone and desktop: anchored, and it closes ------
console.log("menu");
for (const scheme of ["light", "dark"]) {
  for (const width of [768, 880, 1024]) {
    const ctx = await context({ viewport: { width, height: 800 }, colorScheme: scheme });
    const page = await ctx.newPage();
    for (const path of ["/", "/guides/faq/"]) {
      await page.goto(BASE + path, { waitUntil: "load" });
      const tag = `${width}px ${scheme} ${path}`;
      const which = await openMenu(page);
      if (width >= 960) {
        // 60rem and up: the inline nav, no menu button.
        which === null && (await page.getByRole("navigation", { name: "Site", exact: true }).isVisible()) ? pass(`${tag}: inline nav, no menu button`) : fail(`${tag}: expected the inline nav, found menu ${which}`);
        await shot(page, `header-${shotName(path)}-${scheme}-${width}`);
        continue;
      }
      if (which === "details.mnav > summary") {
        // aria-expanded follows the <details> toggle event, which fires after the click.
        await page.waitForFunction(() => document.querySelector("details.mnav > summary")?.getAttribute("aria-expanded") === "true", null, { timeout: 2000 }).catch(() => {});
        const r = await page.evaluate(() => {
          const s = document.querySelector("details.mnav > summary").getBoundingClientRect();
          const p = document.querySelector(".mnav__panel").getBoundingClientRect();
          return { sRight: s.right, pRight: p.right, pLeft: p.left, vw: innerWidth, expanded: document.querySelector("details.mnav > summary").getAttribute("aria-expanded") };
        });
        r.expanded === "true" ? pass(`${tag}: aria-expanded true while open`) : fail(`${tag}: aria-expanded ${r.expanded} while open`);
        Math.abs(r.sRight - r.pRight) <= 2 && r.pLeft >= 0 ? pass(`${tag}: panel lines up under its button (right ${Math.round(r.pRight)} / ${Math.round(r.sRight)})`) : fail(`${tag}: panel right ${r.pRight} vs button right ${r.sRight}, left ${r.pLeft}`);
        await shot(page, `header-${shotName(path)}-${scheme}-${width}-open`);
        await page.keyboard.press("Escape");
        (await page.locator("details.mnav[open]").count()) === 0 ? pass(`${tag}: Escape closes the menu`) : fail(`${tag}: Escape left the menu open`);
        await openMenu(page);
        await page.mouse.click(width / 2, 700);
        (await page.locator("details.mnav[open]").count()) === 0 ? pass(`${tag}: an outside tap closes the menu`) : fail(`${tag}: an outside tap left the menu open`);
      } else if (which === "button.sl-menu-button") {
        (await page.locator("#starlight__sidebar .site-menu").isVisible()) ? pass(`${tag}: docs menu opens with the site menu on top`) : fail(`${tag}: docs menu has no site menu`);
        await shot(page, `header-${shotName(path)}-${scheme}-${width}-open`);
      } else fail(`${tag}: no menu control`);
    }
    await ctx.close();
  }
}

// --- 2c: tap targets in the phone menu and the footer --------------------------
console.log("tap targets");
for (const scheme of ["light", "dark"]) {
  const ctx = await context({ viewport: { width: 390, height: 844 }, colorScheme: scheme });
  const page = await ctx.newPage();
  for (const path of ["/", "/guides/faq/"]) {
    await page.goto(BASE + path, { waitUntil: "load" });
    const which = await openMenu(page);
    const small = await page.evaluate((min) => {
      const panel = document.querySelector("details.mnav[open] .mnav__panel") ?? document.querySelector("#starlight__sidebar");
      const out = [];
      let n = 0;
      for (const [where, box] of [["menu", panel], ["footer", document.querySelector("footer.maya-footer")]]) {
        if (!box) { out.push(`${where}: not found`); continue; }
        for (const el of box.querySelectorAll("a, button, summary, label")) {
          const b = el.getBoundingClientRect();
          const st = getComputedStyle(el);
          if (b.width === 0 || b.height === 0 || st.visibility === "hidden" || el.closest(".sr-only, [hidden]")) continue;
          n++;
          if (b.height < min - 0.5) out.push(`${where} ${el.tagName.toLowerCase()} "${(el.textContent || el.getAttribute("aria-label") || "").trim().slice(0, 30)}" ${Math.round(b.height)} px`);
        }
      }
      return { n, out };
    }, MIN_TAP);
    const tag = `390px ${scheme} ${path} (${which ?? "no menu"})`;
    small.out.length ? fail(`${tag}: ${small.out.length} target(s) under ${MIN_TAP} px: ${small.out.slice(0, 6).join(" | ")}`) : pass(`${tag}: ${small.n} menu and footer targets all >= ${MIN_TAP} px`);
  }
  await ctx.close();
}

// --- screenshots: phones at 360, the charts and the footer at 360 and 1440 -----
const PIECES = [
  { name: "attack", path: "/guides/signatures/", sel: "[data-attack]", tab: "For scientists" },
  { name: "quorum", path: "/guides/consensus/", sel: "figure.quorum" },
  { name: "delivery", path: "/experience/", sel: "figure.delivery" },
  { name: "support", path: "/guides/support/", sel: "section.support" },
  { name: "footer", path: "/guides/faq/", sel: "footer.maya-footer" },
];
for (const scheme of ["light", "dark"]) {
  for (const width of [360, 1440]) {
    const ctx = await context({ viewport: { width, height: width === 360 ? 780 : 900 }, colorScheme: scheme });
    const page = await ctx.newPage();
    const full = width === 360 ? ["/", "/guides/faq/", "/stats/", "/404", "/learn/signatures-live/", "/guides/testnet/"] : ["/learn/signatures-live/", "/guides/testnet/"];
    for (const path of full) {
      await page.goto(BASE + path, { waitUntil: "load" });
      await shot(page, `${shotName(path)}-${scheme}-${width}`);
    }
    for (const p of PIECES) {
      await page.goto(BASE + p.path, { waitUntil: "load" });
      if (p.tab) await page.getByRole("tab", { name: p.tab }).first().click();
      const el = page.locator(p.sel).filter({ visible: true }).first();
      if (!(await el.count())) { fail(`${p.path}: no visible ${p.sel} for the ${p.name} screenshot`); continue; }
      await el.scrollIntoViewIfNeeded();
      await el.screenshot({ path: join(shots, `${p.name}-${scheme}-${width}.jpg`), type: "jpeg", quality: 72 });
    }
    await ctx.close();
  }
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

// --- 4: axe, WCAG 2.x A and AA rules ---------------------------------------------
console.log("axe (" + AXE_TAGS.join(", ") + ")");
const axe = async (page, label) => {
  // expressive-code gives a scrolling code block tabindex="0" from an idle
  // callback after a debounced ResizeObserver; let it run before auditing.
  await page.waitForFunction(() => [...document.querySelectorAll(".expressive-code pre")].every((p) => p.scrollWidth <= p.clientWidth || p.hasAttribute("tabindex")), null, { timeout: 3000 }).catch(() => {});
  await page.addScriptTag({ content: axeSource });
  const v = await page.evaluate(async (tags) => (await window.axe.run(document, { runOnly: { type: "tag", values: tags } })).violations, AXE_TAGS);
  const nodes = v.flatMap((x) => x.nodes.map((n) => `${x.id} ${n.target.join(" ")}: ${(n.any[0] ?? n.all[0] ?? n.none[0])?.message ?? ""}`.slice(0, 180)));
  const contrast = v.filter((x) => x.id === "color-contrast").reduce((a, x) => a + x.nodes.length, 0);
  nodes.length ? fail(`${label}: ${nodes.length} violation(s): ${nodes.slice(0, 5).join(" | ")}`) : pass(`${label}: 0 violations (0 contrast)`);
  return contrast;
};
for (const scheme of ["light", "dark"]) {
  const phone = await context({ viewport: { width: 390, height: 844 }, colorScheme: scheme });
  const pp = await phone.newPage();
  for (const path of ["/", "/guides/faq/"]) {
    await pp.goto(BASE + path, { waitUntil: "load" });
    const which = await openMenu(pp);
    await axe(pp, `${scheme} 390px ${path} menu open (${which})`);
  }
  await phone.close();
  const ctx = await context({ viewport: { width: 1440, height: 900 }, colorScheme: scheme });
  const page = await ctx.newPage();
  for (const path of ["/", "/guides/faq/", "/learn/signatures-live/", "/404"]) {
    await page.goto(BASE + path, { waitUntil: "load" });
    await axe(page, `${scheme} ${path}`);
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
