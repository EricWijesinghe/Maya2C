// The homepage in a real browser (L03): what dist/ checks cannot see.
//   - nothing overflows sideways at 360, 390, 430, 768, 1024 or 1440 px, in
//     either theme (document scrollWidth <= innerWidth; offenders are named);
//   - no two footer blocks draw over each other, and none overflows its column;
//   - with prefers-reduced-motion, no animation is running anywhere on /, light and dark;
//   - axe-core finds 0 WCAG 2.2 A/AA violations, light and dark;
//   - on a 360x640 and a 390x700 phone screen the hero's primary button is
//     fully visible without scrolling, and so is a slice of the orbit (that is
//     also the first-screen shot);
//   - pixel-sampled text contrast at 360 and 1440 px on the gradient-backed
//     text axe cannot resolve (CONTRAST_TARGETS), light and dark;
//   - the transaction row's SVG text renders at 14 px or more at 1024 px;
//   - screenshots at 360, 390 and 1440 px in both themes, when --shots <dir>:
//     the first screen at scroll 0, and the full page with the sticky header
//     put back in the flow (a full-page capture otherwise paints it mid-page).
//
// Not part of `npm run build`: it needs a browser. Serve the build first
// (`npx astro preview --port 4321`), then:
//   PLAYWRIGHT_MODULE=<dir of playwright> AXE_PATH=<axe.min.js> [CHROMIUM_PATH=<chrome>] \
//     node scripts/check-home-browser.mjs [--url http://localhost:4321/] [--shots <dir>]
import { readFileSync, mkdirSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

const arg = (name, dflt) => { const i = process.argv.indexOf(name); return i > 0 ? process.argv[i + 1] : dflt; };
const url = arg("--url", "http://localhost:4321/");
const shots = arg("--shots", null);
const WIDTHS = [360, 390, 430, 768, 1024, 1440];
const SHOT_WIDTHS = [360, 390, 1440];
const THEMES = ["dark", "light"];
// Real phone viewports (after browser chrome): the primary button must be on the first screen.
const PHONE_SCREENS = [[360, 640], [390, 700]];

const MIN_SVG_TEXT_PX = 14;
const MIN_ART_SLICE_PX = 96;
// Text whose background is a gradient, which axe cannot resolve (it reports those as incomplete, not passed).
const CONTRAST_TARGETS = [".hero h1", ".hero .sub", ".hero .chip", ".replay", ".tile .val", ".tile .lbl", ".tile .note", ".home-lede", ".trust a", ".home-close .home-h2", ".home-close .close-line", ".home-close .close-link"];

const lin = (c) => { const v = c / 255; return v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; };
const lum = ([r, g, b]) => 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
const ratio = (a, b) => { const [x, y] = [lum(a), lum(b)].sort((p, q) => q - p); return (x + 0.05) / (y + 0.05); };

// Pixel-sampled contrast: the element is shot with its text made transparent, and the text colour is
// compared with every background pixel behind it. The 5th percentile is reported, so one stray dot of a
// pattern does not decide it, while a gradient's weak end does.
async function contrast(page) {
  const sharp = (await import("sharp")).default;
  const out = [];
  for (const sel of CONTRAST_TARGETS) {
    const loc = page.locator(sel);
    const n = await loc.count();
    for (let i = 0; i < n; i++) {
      const el = loc.nth(i);
      if (!(await el.isVisible())) continue;
      const info = await el.evaluate((node) => {
        const cs = getComputedStyle(node);
        const c = document.createElement("canvas").getContext("2d");
        c.fillStyle = cs.color; c.fillRect(0, 0, 1, 1);
        const [r, g, b] = c.getImageData(0, 0, 1, 1).data;
        node.dataset.ctHide = "";
        return { rgb: [r, g, b], size: parseFloat(cs.fontSize), weight: parseInt(cs.fontWeight, 10) };
      });
      const tag = await page.addStyleTag({ content: "[data-ct-hide], [data-ct-hide] * { color: transparent !important; -webkit-text-fill-color: transparent !important; text-shadow: none !important; }" });
      const png = await el.screenshot({ animations: "disabled" });
      await tag.evaluate((t) => t.remove());
      await el.evaluate((node) => delete node.dataset.ctHide);
      const { data, info: meta } = await sharp(png).removeAlpha().raw().toBuffer({ resolveWithObject: true });
      const rs = [];
      for (let p = 0; p < meta.width * meta.height; p += 3) rs.push(ratio(info.rgb, [data[p * 3], data[p * 3 + 1], data[p * 3 + 2]]));
      rs.sort((a, b) => a - b);
      const large = info.size >= 24 || (info.size >= 18.66 && info.weight >= 700);
      const need = large ? 3 : 4.5;
      const r = rs[Math.floor(rs.length * 0.05)] ?? 0;
      out.push({ sel, i, ratio: r, need, ok: r >= need });
    }
  }
  return out;
}

const mod = process.env.PLAYWRIGHT_MODULE ? pathToFileURL(join(process.env.PLAYWRIGHT_MODULE, "index.mjs")).href : "playwright";
const { chromium } = await import(mod);
const axeSource = readFileSync(process.env.AXE_PATH ?? "node_modules/axe-core/axe.min.js", "utf8");
const browser = await chromium.launch(process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {});
const fails = [];

async function open(width, theme, reducedMotion = "no-preference", height = 900) {
  const ctx = await browser.newContext({ viewport: { width, height }, colorScheme: theme, reducedMotion, deviceScaleFactor: 1, bypassCSP: true });
  const page = await ctx.newPage();
  await page.goto(url, { waitUntil: "networkidle" });
  return { ctx, page };
}

// Walk the page so one-shot scroll effects have fired before anything is measured.
const scrollThrough = (page) => page.evaluate(async () => {
  for (let y = 0; y < document.documentElement.scrollHeight; y += 400) { window.scrollTo({ top: y, behavior: "instant" }); await new Promise((r) => setTimeout(r, 120)); }
  window.scrollTo({ top: 0, behavior: "instant" });
});

for (const theme of THEMES) {
  for (const width of WIDTHS) {
    const { ctx, page } = await open(width, theme);
    await scrollThrough(page);
    const r = await page.evaluate(() => {
      const w = window.innerWidth;
      const wide = [...document.querySelectorAll("body *")]
        .filter((el) => { const b = el.getBoundingClientRect(); return b.width > 0 && b.right > w + 1 && getComputedStyle(el).position !== "fixed" && el.checkVisibility({ contentVisibilityAuto: true, visibilityProperty: true }); })
        // Inside a box that clips or scrolls sideways (a code block, the app nav row) is contained, not overflow.
        .filter((el) => { for (let p = el.parentElement; p && p !== document.body; p = p.parentElement) if (getComputedStyle(p).overflowX !== "visible") return false; return true; })
        .slice(0, 5)
        .map((el) => `${el.tagName.toLowerCase()}.${[...el.classList].join(".")} right=${Math.round(el.getBoundingClientRect().right)}`);
      return { scroll: document.documentElement.scrollWidth, inner: w, wide };
    });
    // The site clips overflow-x on <html>, so scrollWidth alone can hide a box hanging past the edge.
    const ok = r.scroll <= r.inner && r.wide.length === 0;
    console.log(`overflow ${theme} ${width}px: scrollWidth ${r.scroll} / innerWidth ${r.inner} ${ok ? "ok" : "FAIL"}${r.wide.length ? ` (past the edge: ${r.wide.join(", ")})` : ""}`);
    if (!ok) fails.push(`overflow at ${width}px ${theme}`);
    // Footer: no two of its blocks may draw over each other (the network readings once covered the About text).
    const overlaps = await page.evaluate(() => {
      const kids = [...document.querySelectorAll(".maya-footer > *")].map((el) => ({ el, b: el.getBoundingClientRect() }));
      const hit = [];
      for (let i = 0; i < kids.length; i++) for (let j = i + 1; j < kids.length; j++) {
        const a = kids[i].b, c = kids[j].b;
        if (a.left < c.right - 1 && c.left < a.right - 1 && a.top < c.bottom - 1 && c.top < a.bottom - 1) hit.push(`${kids[i].el.className} / ${kids[j].el.className}`);
      }
      // A child that is wider than its own box (nowrap text) overlaps without moving the box: compare scrollWidth too.
      for (const { el } of kids) if (el.scrollWidth > el.clientWidth + 1) hit.push(`${el.className} overflows its column`);
      return hit;
    });
    console.log(`footer ${theme} ${width}px: ${overlaps.length ? `FAIL ${overlaps.join("; ")}` : "no overlap"}`);
    if (overlaps.length) fails.push(`footer overlap at ${width}px ${theme}`);
    if (shots && SHOT_WIDTHS.includes(width)) {
      mkdirSync(shots, { recursive: true });
      // The desktop first screen at scroll 0 (phones get theirs at a real phone height, below)...
      if (!PHONE_SCREENS.some(([w]) => w === width)) {
        const first = join(shots, `home-${theme}-${width}-first-screen.png`);
        await page.screenshot({ path: first, animations: "disabled" });
        console.log(`shot ${first}`);
      }
      // ...and the whole page, with the sticky header put back in the flow so it is not painted mid-page.
      await page.addStyleTag({ content: "header.header, .header { position: static !important; } .page { padding-top: 0 !important; } .main-frame { padding-top: 0 !important; }" });
      const file = join(shots, `home-${theme}-${width}.png`);
      await page.screenshot({ path: file, fullPage: true, animations: "disabled" });
      console.log(`shot ${file}`);
    }
    if (width === 390 || width === 1440) {
      await page.addScriptTag({ content: axeSource });
      const axe = await page.evaluate(() => window.axe.run(document, { runOnly: { type: "tag", values: ["wcag2a", "wcag2aa", "wcag21a", "wcag21aa", "wcag22aa"] } }));
      const v = axe.violations;
      console.log(`axe ${theme} ${width}px: ${v.length} violations, ${axe.passes.length} rules passed`);
      for (const x of v) console.log(`  ${x.id} (${x.impact}): ${x.nodes.length} nodes, e.g. ${x.nodes[0]?.target.join(" ")}`);
      if (v.length) fails.push(`axe ${theme} ${width}px`);
    }
    if (width === 360 || width === 1440) {
      for (const r of await contrast(page)) {
        console.log(`contrast ${theme} ${width}px ${r.sel}#${r.i}: ${r.ratio.toFixed(2)}:1 (need ${r.need}) ${r.ok ? "ok" : "FAIL"}`);
        if (!r.ok) fails.push(`contrast ${r.sel} ${theme} ${width}px`);
      }
    }
    // SVG text scales with its viewBox: the row graphic's step lines must stay readable where the row first appears.
    if (width === 1024) {
      const px = await page.evaluate(() => {
        const svg = document.querySelector(".txp .row");
        const l = svg?.querySelector(".l");
        return svg && l ? parseFloat(getComputedStyle(l).fontSize) * svg.getBoundingClientRect().width / svg.viewBox.baseVal.width : 0;
      });
      const ok = px >= MIN_SVG_TEXT_PX;
      console.log(`svg text ${theme} 1024px: transaction row lines render at ${px.toFixed(1)} px ${ok ? "ok" : "FAIL"}`);
      if (!ok) fails.push(`transaction row text ${px.toFixed(1)} px at 1024 ${theme}`);
    }
    await ctx.close();
  }
}

// The first screen on a phone: chip, headline and the primary button, without scrolling.
for (const theme of THEMES) {
  for (const [width, height] of PHONE_SCREENS) {
    const { ctx, page } = await open(width, theme, "no-preference", height);
    const bottom = await page.evaluate(() => document.querySelector(".hero .ui-btn--primary")?.getBoundingClientRect().bottom ?? Infinity);
    const ok = bottom <= height;
    console.log(`first screen ${theme} ${width}x${height}: primary button bottom ${Math.round(bottom)} ${ok ? "ok" : "FAIL"}`);
    if (!ok) fails.push(`primary button below the first screen at ${width}x${height} ${theme}`);
    // ...and a real slice of the orbit, so the first screen is not a text wall.
    const artTop = await page.evaluate(() => document.querySelector(".hero .art")?.getBoundingClientRect().top ?? Infinity);
    const slice = Math.round(height - artTop);
    console.log(`first screen ${theme} ${width}x${height}: ${slice} px of the orbit visible ${slice >= MIN_ART_SLICE_PX ? "ok" : "FAIL"}`);
    if (slice < MIN_ART_SLICE_PX) fails.push(`orbit not on the first screen at ${width}x${height} ${theme}`);
    if (shots) {
      const first = join(shots, `home-${theme}-${width}-first-screen.png`);
      await page.screenshot({ path: first, animations: "disabled" });
      console.log(`shot ${first}`);
    }
    await ctx.close();
  }
}

// Reduced motion: nothing may still be running once the page has been walked.
for (const [width, theme] of THEMES.flatMap((t) => [390, 1440].map((w) => [w, t]))) {
  const { ctx, page } = await open(width, theme, "reduce");
  await scrollThrough(page);
  const running = await page.evaluate(() => document.getAnimations().filter((a) => a.playState === "running").map((a) => {
    const t = a.effect?.target;
    return `${a.animationName ?? a.constructor.name} on ${t ? `${t.tagName?.toLowerCase()}.${[...(t.classList ?? [])].join(".")}` : "?"}`;
  }));
  console.log(`reduced motion ${theme} ${width}px: ${running.length} running animations${running.length ? `: ${running.join(", ")}` : ""}`);
  if (running.length) fails.push(`reduced motion ${theme} ${width}px`);
  await ctx.close();
}

await browser.close();
if (fails.length) { console.error(`check-home-browser: FAIL ${fails.join("; ")}`); process.exit(1); }
console.log("check-home-browser: ok");
