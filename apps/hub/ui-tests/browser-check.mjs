// The Hub UI in a real browser, on its preview data: the first-start sheet,
// canvas names, WCAG contrast (axe-core) in both themes, horizontal overflow
// from 360 px up, and screenshots into design/visual/hub/.
//
//   HUB_TOOLS=<dir with node_modules/playwright (or playwright-core) and axe-core> \
//   CHROMIUM_PATH=<chrome.exe> node apps/hub/ui-tests/browser-check.mjs
//
// CI runs it (job `hub-ui` in .github/workflows/ci.yml) with NO_SHOTS=1 in the
// pinned Playwright image; the Hub itself does not depend on playwright or
// axe-core. Screenshots show the paused network, which is the real state since
// 2026-10-28, except the `hub-preview-*` set: the live look on synthetic data,
// each image stamped "Preview data" across the top. Exits 1 on any failure
// and prints each one.
import { createServer } from "node:http";
import { createRequire } from "node:module";
import { mkdirSync, readFileSync, existsSync } from "node:fs";
import { dirname, extname, join, normalize, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
// Served from apps/hub, so ui/index.html can load ../ui-tests/preview.js,
// which the app bundle (ui/ only) never ships.
const ui = resolve(here, "..");
const shots = resolve(here, "..", "..", "..", "design", "visual", "hub");
const require = createRequire(process.env.HUB_TOOLS ? join(resolve(process.env.HUB_TOOLS), "index.js") : import.meta.url);
// playwright or playwright-core: the same API, the latter without bundled browsers.
const { chromium } = (() => { try { return require("playwright"); } catch { return require("playwright-core"); } })();
const axeSource = readFileSync(require.resolve("axe-core/axe.min.js"), "utf8");

const PAGES = ["mining", "network", "wallet", "universe", "settings"];
const WIDTHS = [360, 390, 430, 768, 1024, 1280, 1440];
const DEFAULT_WINDOW = { width: 1280, height: 820 }; // src-tauri/tauri.conf.json
const failures = [];
const fail = (what) => {
  failures.push(what);
  console.log(`FAIL ${what}`);
};
const pass = (what) => console.log(`ok   ${what}`);

// ------------------------------------------------------------ server ----
const TYPES = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png", ".woff2": "font/woff2" };
const server = createServer((req, res) => {
  const path = normalize(join(ui, decodeURIComponent(new URL(req.url, "http://x").pathname)));
  const file = path.endsWith("\\") || path.endsWith("/") ? join(path, "index.html") : path;
  if (!file.startsWith(ui) || !existsSync(file)) {
    res.writeHead(404).end();
    return;
  }
  res.writeHead(200, { "content-type": TYPES[extname(file)] || "application/octet-stream" }).end(readFileSync(file));
});
await new Promise((ok) => server.listen(0, "127.0.0.1", ok));
const base = `http://127.0.0.1:${server.address().port}/ui/index.html`;

const browser = await chromium.launch(process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {});

async function open({ theme = "dark", viewport = DEFAULT_WINDOW, firstRun = true, hash = "" } = {}) {
  const context = await browser.newContext({ viewport, deviceScaleFactor: 1, colorScheme: theme, reducedMotion: "reduce" });
  await context.addInitScript(({ theme, firstRun }) => {
    localStorage.setItem("hub.theme", theme);
    if (firstRun) localStorage.setItem("hub.firstRunDone", "1");
  }, { theme, firstRun });
  const page = await context.newPage();
  const errors = [];
  page.on("pageerror", (e) => errors.push(String(e)));
  await page.goto(base + hash);
  await page.waitForFunction(() => document.querySelectorAll("#universe .planet").length > 0);
  return { page, context, errors };
}

const goto = (page, name) => page.click(`.nav[data-page="${name}"]`);

async function startPreviewMiner(page) {
  await page.click("#power");
  await page.mouse.move(0, 0); // off the button, so its look is the resting one
  await page.waitForFunction(() => document.getElementById("power-text").textContent === "Stop mining", null, { timeout: 8000 });
  await page.waitForTimeout(2500);
}

// ------------------------------------------------------- first start ----
{
  const { page, context, errors } = await open({ firstRun: false });
  const sheetOpen = () => page.evaluate(() => document.getElementById("first-run").open);
  (await sheetOpen()) ? pass("first launch without the flag opens the sheet") : fail("first launch did not open the sheet");
  await page.click("#fr-next");
  await page.click("#fr-next");
  (await page.isDisabled("#fr-start")) ? pass("Start is disabled on the last step before the backup box") : fail("Start enabled without the backup box");
  await page.click("#fr-close");
  await page.click("#power");
  const running = await page.evaluate(() => document.getElementById("power-text").textContent);
  (await sheetOpen()) && running === "Start mining" ? pass("the hero Start reopens the sheet and starts nothing") : fail(`hero Start without the flag: sheet ${await sheetOpen()}, power "${running}"`);
  await page.click("#fr-next");
  await page.check("#fr-backup");
  await page.click("#fr-next");
  (await page.isEnabled("#fr-start")) ? pass("ticking the backup box enables Start") : fail("Start stayed disabled after the backup box");
  await page.click("#fr-start");
  await page.waitForFunction(() => document.getElementById("power-text").textContent === "Stop mining", null, { timeout: 8000 }).catch(() => undefined);
  const flag = await page.evaluate(() => localStorage.getItem("hub.firstRunDone"));
  const after = await page.evaluate(() => document.getElementById("power-text").textContent);
  flag === "1" && after === "Stop mining" && !(await sheetOpen()) ? pass("finishing stores the flag and starts the preview miner") : fail(`after finish: flag ${flag}, power "${after}"`);
  errors.length ? fail(`page errors: ${errors.join(" | ")}`) : pass("no page errors in the first-start flow");
  await context.close();
}

// ------------------------------------------------------- power states ----
// Idle, mining and blocked are three looks, so the one control a new user
// looks for never reads as disabled while it works.
const powerLook = (page) => page.evaluate(() => {
  const p = document.getElementById("power");
  const cs = getComputedStyle(p);
  return { state: p.dataset.state, disabled: p.disabled, look: `${cs.backgroundImage}|${cs.backgroundColor}|${cs.opacity}` };
});
{
  const { page, context } = await open();
  await page.waitForFunction(() => document.getElementById("power").dataset.state === "idle" && !document.getElementById("power").disabled, null, { timeout: 8000 });
  const idle = await powerLook(page);
  await startPreviewMiner(page);
  const mining = await powerLook(page);
  await context.close();
  const paused = await open({ hash: "#paused" });
  await paused.page.waitForSelector(".paused-slot .paused-card", { timeout: 8000 });
  const blocked = await powerLook(paused.page);
  await paused.context.close();
  const states = [idle.state, mining.state, blocked.state].join(",");
  const looks = new Set([idle.look, mining.look, blocked.look]).size;
  states === "idle,mining,blocked" && looks === 3 && blocked.disabled && !idle.disabled
    ? pass("power control: idle, mining and blocked each have their own look")
    : fail(`power control: states ${states}, ${looks} distinct looks, blocked disabled ${blocked.disabled}`);
}

// ------------------------------------------------------------- theme ----
{
  const { page, context } = await open({ theme: "dark" });
  await goto(page, "settings");
  await page.click('#s-theme label:has(input[value="light"])');
  const light = await page.evaluate(() => [document.documentElement.dataset.theme, localStorage.getItem("hub.theme")].join(","));
  await page.click('#s-theme label:has(input[value="system"])');
  const system = await page.evaluate(() => [document.documentElement.dataset.theme ?? "", localStorage.getItem("hub.theme")].join(","));
  light === "light,light" && system === ",system" ? pass("Settings theme control sets and stores light, then system") : fail(`theme control: light "${light}", system "${system}"`);
  await context.close();
}

// --------------------------------------------------- canvases, contrast ----
async function canvasNames(page, where) {
  const bad = await page.evaluate(() => [...document.querySelectorAll(".page.active canvas")]
    .filter((c) => c.getAttribute("aria-hidden") !== "true" && !c.closest("[hidden]")) // hidden: not in the accessibility tree
    .filter((c) => c.getAttribute("role") !== "img" || !(c.getAttribute("aria-label") || "").trim())
    .map((c) => c.id || c.className));
  bad.length ? fail(`${where}: canvases without role=img and a label: ${bad.join(", ")}`) : pass(`${where}: every chart canvas has role=img and a label`);
}

// The hover state axe never sees: a primary button under the pointer, its text
// against its background, measured from the computed colours.
async function hoverContrast(page, where) {
  await goto(page, "settings");
  await page.hover("#s-save");
  const ratio = await page.evaluate(() => {
    const parse = (c) => {
      const srgb = c.match(/color\(srgb ([\d.]+) ([\d.]+) ([\d.]+)/);
      if (srgb) return srgb.slice(1, 4).map(Number);
      return c.match(/[\d.]+/g).slice(0, 3).map((v) => Number(v) / 255);
    };
    const lum = (rgb) => {
      const [r, g, b] = rgb.map((v) => (v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4));
      return 0.2126 * r + 0.7152 * g + 0.0722 * b;
    };
    const cs = getComputedStyle(document.getElementById("s-save"));
    const [a, b] = [lum(parse(cs.backgroundColor)), lum(parse(cs.color))].sort((x, y) => y - x);
    return (a + 0.05) / (b + 0.05);
  });
  await page.mouse.move(0, 0);
  ratio >= 4.5 ? pass(`${where}: primary button hover ${ratio.toFixed(2)}:1`) : fail(`${where}: primary button hover ${ratio.toFixed(2)}:1, under 4.5:1`);
}

async function contrast(page, where) {
  await page.addScriptTag({ content: axeSource });
  const result = await page.evaluate(() => window.axe.run(document, { runOnly: { type: "rule", values: ["color-contrast"] } }));
  const nodes = result.violations.flatMap((v) => v.nodes.map((n) => `${n.target.join(" ")}: ${n.any.map((a) => a.message).join("; ")}`));
  nodes.length ? fail(`${where}: ${nodes.length} contrast violation(s)\n    ${nodes.join("\n    ")}`) : pass(`${where}: axe color-contrast 0 violations`);
}

for (const theme of ["dark", "light"]) {
  for (const hash of ["", "#paused"]) {
    const { page, context, errors } = await open({ theme, hash });
    if (!hash) await startPreviewMiner(page);
    else await page.waitForSelector(".paused-slot .paused-card", { timeout: 8000 });
    for (const name of PAGES) {
      await goto(page, name);
      await page.waitForTimeout(400);
      const where = `${theme}${hash || " live"} ${name}`;
      if (name === "mining" || name === "network") await canvasNames(page, where);
      await contrast(page, where);
    }
    if (hash) {
      const legend = await page.evaluate(() => document.getElementById("legend").textContent + [...document.querySelectorAll(".planet .chip")].map((c) => c.textContent).join("|"));
      /\blive\b/i.test(legend) ? fail(`${theme} paused: universe says live: ${legend}`) : pass(`${theme} paused: no universe chip says live`);
      const disabled = await page.evaluate(() => document.getElementById("power").disabled
        && document.getElementById("power-why").textContent === "Waits for the network."
        && document.querySelector("#page-mining .paused-title").textContent === "Testnet paused since 2026-10-28");
      disabled ? pass(`${theme} paused: Start disabled with the reason, the banner carries the date`) : fail(`${theme} paused: Start not disabled with a reason`);
      // Said once above the fold: the banner, not also the gauge or the header.
      const once = await page.evaluate(() => {
        const page = document.getElementById("page-mining");
        page.closest("main").scrollTop = 0;
        document.querySelector('.nav[data-page="mining"]').click();
        const seen = [...document.querySelectorAll("#page-mining *, .topbar *")].filter((el) => {
          if (!el.children.length && el.offsetParent !== null) {
            const r = el.getBoundingClientRect();
            return r.bottom <= innerHeight && /paused/i.test(el.textContent);
          }
          return false;
        });
        return seen.length;
      });
      once === 1 ? pass(`${theme} paused: "paused" appears once above the fold on Mining`) : fail(`${theme} paused: "paused" appears ${once} times above the fold on Mining`);
      // No network figure is shown while the network does not answer.
      const figures = await page.evaluate(() => ["t-epoch", "t-reward", "t-balance", "n-height", "n-chain"].map((id) => `${id}=${document.getElementById(id).textContent}`));
      figures.every((f) => f.endsWith("=—")) ? pass(`${theme} paused: epoch, reward, balance, height and chain show —`) : fail(`${theme} paused: figures shown: ${figures.join(" ")}`);
      // textContent: every page, shown or not (the scripts are all external).
      const said = await page.evaluate(() => document.body.textContent);
      /\blive\b/i.test(said) ? fail(`${theme} paused: the page says live somewhere`) : pass(`${theme} paused: no text on any page says live`);
    }
    if (!hash) await hoverContrast(page, `${theme}`);
    errors.length ? fail(`${theme}${hash}: page errors: ${errors.join(" | ")}`) : pass(`${theme}${hash || " live"}: no page errors`);
    await context.close();
  }
  // The sheet itself, in both themes.
  const { page, context } = await open({ theme, firstRun: false });
  await contrast(page, `${theme} first-start sheet`);
  await context.close();
}

// ---------------------------------------------------------- overflow ----
for (const width of WIDTHS) {
  const { page, context } = await open({ viewport: { width, height: 800 } });
  await startPreviewMiner(page);
  for (const name of PAGES) {
    await goto(page, name);
    await page.waitForTimeout(300);
    const over = await page.evaluate(() => {
      const vw = window.innerWidth;
      const main = document.querySelector("main");
      const out = [];
      if (document.documentElement.scrollWidth > vw) out.push(`document ${document.documentElement.scrollWidth}px`);
      if (main.scrollWidth > main.clientWidth + 1) out.push(`main ${main.scrollWidth}px > ${main.clientWidth}px`);
      for (const el of document.querySelectorAll(".page.active *, .topbar *, .rail *")) {
        if (el.closest(".net-layer") || el.closest("[hidden]") || el.closest("details:not([open]) code")) continue;
        // Inside a row that scrolls or clips sideways (the phone legend), the
        // row is what must fit, and it is checked as an element itself.
        let clipped = false;
        for (let up = el.parentElement; up && up !== document.body && !clipped; up = up.parentElement) {
          clipped = up.matches(".page, main") ? false : ["auto", "scroll", "hidden"].includes(getComputedStyle(up).overflowX);
        }
        if (clipped) continue;
        const r = el.getBoundingClientRect();
        if (r.width && r.right > vw + 1) out.push(`${el.tagName.toLowerCase()}${el.id ? "#" + el.id : ""}.${String(el.className.baseVal ?? el.className).split(" ")[0]} right ${Math.round(r.right)}`);
      }
      return out.slice(0, 6);
    });
    over.length ? fail(`${width}px ${name}: overflow: ${over.join("; ")}`) : pass(`${width}px ${name}: no horizontal overflow`);
  }
  await context.close();
}
{
  const { page, context } = await open({ viewport: { width: 360, height: 780 }, firstRun: false });
  const over = await page.evaluate(() => {
    const r = document.getElementById("first-run").getBoundingClientRect();
    return r.right > window.innerWidth + 1 || r.left < -1 ? `sheet ${Math.round(r.left)}..${Math.round(r.right)}` : "";
  });
  over ? fail(`360px first-start sheet overflows: ${over}`) : pass("360px first-start sheet fits");
  await context.close();
}

// -------------------------------------------------------- screenshots ----
if (!process.env.NO_SHOTS) {
  mkdirSync(shots, { recursive: true });
  const shot = (page, name, fullPage = false) => page.screenshot({ path: join(shots, `${name}.jpg`), type: "jpeg", quality: 82, fullPage });
  // fullPage of an app whose <main> scrolls: let the document grow instead.
  const unclip = (page) => page.addStyleTag({ content: "html, body { height: auto !important; overflow: visible !important; } .app { height: auto !important; } main { overflow: visible !important; }" });
  // Stamped on every live-look image: the figures in it are synthetic.
  const stamp = (page) => page.evaluate(() => {
    const s = document.createElement("div");
    s.textContent = "Preview data · synthetic figures, not the network";
    s.style.cssText = "position:fixed;left:0;right:0;top:0;z-index:99;padding:6px 12px;text-align:center;font:700 14px Inter,sans-serif;letter-spacing:.04em;background:var(--steel);color:var(--bg);opacity:.92";
    document.body.append(s);
  });
  const SIZES = { 360: { width: 360, height: 780 }, 768: { width: 768, height: 1024 }, 1024: { width: 1024, height: 768 }, 1280: DEFAULT_WINDOW, 1440: { width: 1440, height: 900 } };
  for (const theme of ["dark", "light"]) {
    // Every page on the paused network: what every real user sees today.
    for (const [label, viewport] of Object.entries(SIZES)) {
      const paused = await open({ theme, viewport, hash: "#paused" });
      await paused.page.waitForSelector(".paused-slot .paused-card", { timeout: 8000 });
      await shot(paused.page, `hub-mining-paused-${theme}-${label}`);
      if (label === "360" || label === "1280") {
        for (const name of ["network", "universe", "wallet", "settings"]) {
          await goto(paused.page, name);
          await paused.page.waitForTimeout(600);
          await shot(paused.page, `hub-${name}-paused-${theme}-${label}`);
        }
      }
      await paused.context.close();
    }
    // Whole pages, below the fold too: paused, then the live look on preview data.
    for (const label of ["360", "1440"]) {
      const paused = await open({ theme, viewport: SIZES[label], hash: "#paused" });
      await paused.page.waitForSelector(".paused-slot .paused-card", { timeout: 8000 });
      await unclip(paused.page);
      for (const name of ["mining", "network"]) {
        await goto(paused.page, name);
        await paused.page.waitForTimeout(600);
        await shot(paused.page, `hub-${name}-paused-${theme}-${label}-full`, true);
      }
      await paused.context.close();
      const live = await open({ theme, viewport: SIZES[label] });
      await startPreviewMiner(live.page);
      await unclip(live.page);
      await stamp(live.page);
      for (const name of ["mining", "network", "universe"]) {
        await goto(live.page, name);
        await live.page.waitForTimeout(900);
        await shot(live.page, `hub-preview-${name}-${theme}-${label}-full`, true);
      }
      await live.context.close();
    }
    for (const label of ["360", "1280"]) {
      const sheet = await open({ theme, viewport: SIZES[label], firstRun: false });
      await sheet.page.waitForTimeout(300);
      await shot(sheet.page, `hub-first-start-${theme}-${label}`);
      await sheet.context.close();
    }
    // The power control alone, in its three states. Cropped to the control
    // and its reason line: no chain figure is in frame.
    const live = await open({ theme });
    await live.page.waitForFunction(() => document.getElementById("power").dataset.state === "idle", null, { timeout: 8000 });
    await live.page.locator(".hero-side").screenshot({ path: join(shots, `hub-power-idle-${theme}.jpg`), type: "jpeg", quality: 82 });
    await startPreviewMiner(live.page);
    await live.page.locator(".hero-side").screenshot({ path: join(shots, `hub-power-mining-${theme}.jpg`), type: "jpeg", quality: 82 });
    await live.context.close();
    const blocked = await open({ theme, hash: "#paused" });
    await blocked.page.waitForSelector(".paused-slot .paused-card", { timeout: 8000 });
    await blocked.page.locator(".hero-side").screenshot({ path: join(shots, `hub-power-blocked-${theme}.jpg`), type: "jpeg", quality: 82 });
    await blocked.context.close();
  }
  pass(`screenshots in ${shots}`);
}

await browser.close();
server.close();
console.log(`\n${failures.length ? `${failures.length} FAILED` : "all checks passed"}`);
process.exit(failures.length ? 1 : 0);
