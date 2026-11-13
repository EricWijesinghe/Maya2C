// The homepage in a real browser (L03): what dist/ checks cannot see.
//   - nothing overflows sideways at 360, 390, 430, 768, 1024 or 1440 px, in
//     either theme (document scrollWidth <= innerWidth; offenders are named);
//   - with prefers-reduced-motion, no animation is running anywhere on /;
//   - axe-core finds 0 WCAG 2.2 A/AA violations, light and dark;
//   - screenshots at 360, 390 and 1440 px in both themes, when --shots <dir>.
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

const mod = process.env.PLAYWRIGHT_MODULE ? pathToFileURL(join(process.env.PLAYWRIGHT_MODULE, "index.mjs")).href : "playwright";
const { chromium } = await import(mod);
const axeSource = readFileSync(process.env.AXE_PATH ?? "node_modules/axe-core/axe.min.js", "utf8");
const browser = await chromium.launch(process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {});
const fails = [];

async function open(width, theme, reducedMotion = "no-preference") {
  const ctx = await browser.newContext({ viewport: { width, height: 900 }, colorScheme: theme, reducedMotion, deviceScaleFactor: 1, bypassCSP: true });
  const page = await ctx.newPage();
  await page.goto(url, { waitUntil: "networkidle" });
  return { ctx, page };
}

// Walk the page so one-shot scroll effects have fired before anything is measured.
const scrollThrough = (page) => page.evaluate(async () => {
  for (let y = 0; y < document.documentElement.scrollHeight; y += 400) { window.scrollTo(0, y); await new Promise((r) => setTimeout(r, 120)); }
  window.scrollTo(0, 0);
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
    if (shots && SHOT_WIDTHS.includes(width)) {
      mkdirSync(shots, { recursive: true });
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
    await ctx.close();
  }
}

// Reduced motion: nothing may still be running once the page has been walked.
for (const width of [390, 1440]) {
  const { ctx, page } = await open(width, "dark", "reduce");
  await scrollThrough(page);
  const running = await page.evaluate(() => document.getAnimations().filter((a) => a.playState === "running").map((a) => {
    const t = a.effect?.target;
    return `${a.animationName ?? a.constructor.name} on ${t ? `${t.tagName?.toLowerCase()}.${[...(t.classList ?? [])].join(".")}` : "?"}`;
  }));
  console.log(`reduced motion ${width}px: ${running.length} running animations${running.length ? `: ${running.join(", ")}` : ""}`);
  if (running.length) fails.push(`reduced motion ${width}px`);
  await ctx.close();
}

await browser.close();
if (fails.length) { console.error(`check-home-browser: FAIL ${fails.join("; ")}`); process.exit(1); }
console.log("check-home-browser: ok");
