// The Hub UI in a real browser, on its preview data: the first-start sheet,
// canvas names, WCAG contrast (axe-core) in both themes, horizontal overflow
// from 360 px up, and screenshots into design/visual/hub/.
//
//   HUB_TOOLS=<dir with node_modules/playwright and axe-core> \
//   CHROMIUM_PATH=<chrome.exe> node apps/hub/ui-tests/browser-check.mjs
//
// Not in CI yet: it needs playwright and axe-core, which the Hub does not
// depend on. Exits 1 on any failure and prints each one.
import { createServer } from "node:http";
import { createRequire } from "node:module";
import { mkdirSync, readFileSync, existsSync } from "node:fs";
import { dirname, extname, join, normalize, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const ui = resolve(here, "..", "ui");
const shots = resolve(here, "..", "..", "..", "design", "visual", "hub");
const require = createRequire(process.env.HUB_TOOLS ? join(resolve(process.env.HUB_TOOLS), "index.js") : import.meta.url);
const { chromium } = require("playwright");
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
const base = `http://127.0.0.1:${server.address().port}/index.html`;

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
      const disabled = await page.evaluate(() => document.getElementById("power").disabled && /paused since 2026-10-28/.test(document.getElementById("power-why").textContent));
      disabled ? pass(`${theme} paused: Start disabled with the reason`) : fail(`${theme} paused: Start not disabled with a reason`);
    }
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
  for (const theme of ["dark", "light"]) {
    for (const [label, viewport] of [["360", { width: 360, height: 780 }], ["1280", DEFAULT_WINDOW], ["1440", { width: 1440, height: 900 }]]) {
      const { page, context } = await open({ theme, viewport });
      await startPreviewMiner(page);
      await page.screenshot({ path: join(shots, `hub-mining-${theme}-${label}.jpg`), type: "jpeg", quality: 82 });
      if (label === "1280") {
        for (const name of ["network", "universe"]) {
          await goto(page, name);
          await page.waitForTimeout(600);
          await page.screenshot({ path: join(shots, `hub-${name}-${theme}-${label}.jpg`), type: "jpeg", quality: 82 });
        }
      }
      await context.close();
    }
    for (const [label, viewport] of [["360", { width: 360, height: 780 }], ["1280", DEFAULT_WINDOW]]) {
      const paused = await open({ theme, viewport, hash: "#paused" });
      await paused.page.waitForSelector(".paused-slot .paused-card", { timeout: 8000 });
      await paused.page.screenshot({ path: join(shots, `hub-paused-${theme}-${label}.jpg`), type: "jpeg", quality: 82 });
      await paused.context.close();
      const sheet = await open({ theme, viewport, firstRun: false });
      await sheet.page.waitForTimeout(300);
      await sheet.page.screenshot({ path: join(shots, `hub-first-start-${theme}-${label}.jpg`), type: "jpeg", quality: 82 });
      await sheet.context.close();
    }
  }
  pass(`screenshots in ${shots}`);
}

await browser.close();
server.close();
console.log(`\n${failures.length ? `${failures.length} FAILED` : "all checks passed"}`);
process.exit(failures.length ? 1 : 0);
