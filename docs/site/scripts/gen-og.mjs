// Renders every 1200x630 link-preview card from src/data/og-pages.json: one
// per main page (`slug` -> public/og/pages/<slug>.png) and the site and
// section cards (`card` -> public/<card>, e.g. og-card.png, og/guides.png).
//
// Local and free. One SVG template, two rasterisers:
//   - `--force` re-renders every card in Chromium through Playwright, which
//     loads Inter from the site's own font package, then compresses with sharp.
//     Playwright is not a site dependency: point PLAYWRIGHT_MODULE at an
//     installed copy (and CHROMIUM_PATH at a browser) when regenerating.
//   - The build only renders a card that does not exist yet, with sharp alone
//     (librsvg cannot load a WOFF2 font, so such a card falls back to a system
//     sans until someone runs --force). Committed cards are never churned.
// The card text is the page's own title and summary, so a preview never shows
// a claim the page does not make. The status line follows
// src/data/network-state.ts, so a forced run after re-genesis drops it.
import { readFile, mkdir, writeFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { fileURLToPath, pathToFileURL } from "node:url";
import { dirname, join } from "node:path";
import sharp from "sharp";

const here = fileURLToPath(new URL(".", import.meta.url));
const site = join(here, "..");
const force = process.argv.includes("--force");
const W = 1200;
const H = 630;
/** Link previewers fetch the card on every share; keep each one small. */
const MAX_BYTES = 150 * 1024;
// Forged Light 2, refreshed dark palette (PR #521, design/tokens.css): a card
// is an image, so it carries literal colours rather than custom properties.
const C = {
  bg: "#081c28", deep: "#05121a", ink: "#f2f9fa", muted: "#bcd0d7",
  teal: "#1fd3cc", bronze: "#ffbb6b", ring: "#2b4a57",
};
const FONT = "Inter, 'Segoe UI', 'Helvetica Neue', Arial, sans-serif";

const esc = (s) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");

/**
 * Greedy word wrap. Width is estimated from the font size (about 0.56 em per
 * character for Inter at these weights), so lines stay inside the text column
 * without measuring glyphs. Overflow ends in an ellipsis.
 */
export function wrap(text, size, maxWidth, maxLines) {
  const perLine = Math.floor(maxWidth / (size * 0.56));
  const lines = [];
  let cur = "";
  for (const word of text.split(/\s+/)) {
    const next = cur ? `${cur} ${word}` : word;
    if (next.length <= perLine) cur = next;
    else {
      if (cur) lines.push(cur);
      cur = word;
    }
  }
  if (cur) lines.push(cur);
  if (lines.length > maxLines) {
    lines.length = maxLines;
    lines[maxLines - 1] = `${lines[maxLines - 1].replace(/[\s.,;:]+$/, "")}…`;
  }
  return lines;
}

const tspans = (lines, x, y, lh) => lines.map((l, i) => `<tspan x="${x}" y="${y + i * lh}">${esc(l)}</tspan>`).join("");

/** Six committee seats on a ring around the emblem: the same motif as the homepage hero. */
function orbit(cx, cy, emblemHref) {
  const seats = [0, 1, 2, 3, 4, 5].map((i) => {
    const a = (i / 6) * Math.PI * 2 - Math.PI / 2;
    const x = (cx + 170 * Math.cos(a)).toFixed(1), y = (cy + 170 * Math.sin(a)).toFixed(1);
    const c = i % 2 ? C.bronze : C.teal;
    return `<line x1="${cx}" y1="${cy}" x2="${x}" y2="${y}" stroke="${C.teal}" stroke-opacity=".16"/>` +
      `<circle cx="${x}" cy="${y}" r="16" fill="${c}" fill-opacity=".2"/><circle cx="${x}" cy="${y}" r="7" fill="${c}"/>`;
  }).join("");
  return `<circle cx="${cx}" cy="${cy}" r="130" fill="url(#core)"/>
  <circle cx="${cx}" cy="${cy}" r="214" fill="none" stroke="${C.ring}" stroke-dasharray="2 9"/>
  <circle cx="${cx}" cy="${cy}" r="170" fill="none" stroke="url(#ring)" stroke-width="2.5"/>
  <circle cx="${cx}" cy="${cy}" r="118" fill="none" stroke="${C.teal}" stroke-opacity=".35"/>
  ${seats}
  <image href="${emblemHref}" x="${cx - 62}" y="${cy - 62}" width="124" height="124"/>`;
}

export function cardSvg(entry, { emblemHref, fontCss = "", status }) {
  // A long title steps down a size and takes a third line, and the summary gives one up.
  const long = wrap(entry.title, 60, 660, Infinity).length > 2;
  const t = long ? { size: 48, lh: 56, lines: 3 } : { size: 60, lh: 70, lines: 2 };
  const title = wrap(entry.title, t.size, 660, t.lines);
  const line = wrap(entry.line, 28, 660, title.length > 2 ? 2 : 3);
  const lineY = 290 + (title.length - 1) * t.lh + 74;
  const pillW = Math.round(status.length * 20 * 0.5 + 66);
  return `<svg xmlns="http://www.w3.org/2000/svg" width="${W}" height="${H}" viewBox="0 0 ${W} ${H}">
  <defs>
    <style>${fontCss} text { font-family: ${FONT}; }</style>
    <linearGradient id="bg" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="${C.bg}"/><stop offset="1" stop-color="${C.deep}"/></linearGradient>
    <radialGradient id="aurora-t" cx="0.82" cy="0.1" r="0.6"><stop offset="0" stop-color="${C.teal}" stop-opacity=".30"/><stop offset="1" stop-color="${C.teal}" stop-opacity="0"/></radialGradient>
    <radialGradient id="aurora-b" cx="0.7" cy="0.95" r="0.5"><stop offset="0" stop-color="${C.bronze}" stop-opacity=".20"/><stop offset="1" stop-color="${C.bronze}" stop-opacity="0"/></radialGradient>
    <radialGradient id="core"><stop offset="0" stop-color="${C.teal}" stop-opacity=".28"/><stop offset="1" stop-color="${C.teal}" stop-opacity="0"/></radialGradient>
    <linearGradient id="ring" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="${C.teal}"/><stop offset="1" stop-color="${C.bronze}"/></linearGradient>
    <linearGradient id="rule" x1="0" y1="0" x2="1" y2="0"><stop offset="0" stop-color="${C.teal}"/><stop offset="1" stop-color="${C.bronze}"/></linearGradient>
  </defs>
  <rect width="${W}" height="${H}" fill="url(#bg)"/>
  <rect width="${W}" height="${H}" fill="url(#aurora-t)"/>
  <rect width="${W}" height="${H}" fill="url(#aurora-b)"/>
  ${orbit(965, 300, emblemHref)}
  <text x="72" y="104" font-size="36" font-weight="800" fill="${C.ink}" letter-spacing="-0.5">Maya2C</text>
  <text x="72" y="142" font-size="22" fill="${C.muted}">Post-quantum layer-1 blockchain</text>
  <rect x="72" y="190" width="120" height="6" rx="3" fill="url(#rule)"/>
  <text font-size="${t.size}" font-weight="800" fill="${C.ink}" letter-spacing="-1.5">${tspans(title, 72, 290, t.lh)}</text>
  <text font-size="28" fill="${C.muted}">${tspans(line, 72, lineY, 40)}</text>
  <rect x="72" y="540" width="${pillW}" height="44" rx="22" fill="${C.bronze}" fill-opacity=".12" stroke="${C.bronze}" stroke-opacity=".55"/>
  <circle cx="98" cy="562" r="6" fill="${C.bronze}"/>
  <text x="114" y="569" font-size="20" font-weight="600" fill="${C.ink}">${esc(status)}</text>
  <text x="${W - 72}" y="572" font-size="24" font-weight="600" fill="${C.teal}" text-anchor="end">maya2c.dev</text>
</svg>`;
}

/** palette PNG keeps gradients smooth with dithering and lands well under MAX_BYTES. */
async function compress(png) {
  const out = await sharp(png).png({ palette: true, quality: 92, effort: 10, compressionLevel: 9, dither: 1 }).toBuffer();
  const meta = await sharp(out).metadata();
  if (meta.width !== W || meta.height !== H) throw new Error(`gen-og: card is ${meta.width}x${meta.height}, not ${W}x${H}`);
  if (out.length > MAX_BYTES) throw new Error(`gen-og: card is ${out.length} bytes, over ${MAX_BYTES}`);
  return out;
}

async function chromiumRenderer() {
  const mod = process.env.PLAYWRIGHT_MODULE ? pathToFileURL(join(process.env.PLAYWRIGHT_MODULE, "index.mjs")).href : "playwright";
  const { chromium } = await import(mod).catch(() => {
    throw new Error("gen-og --force needs Playwright: set PLAYWRIGHT_MODULE to an installed playwright package directory");
  });
  const browser = await chromium.launch(process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {});
  const page = await browser.newPage({ viewport: { width: W, height: H }, deviceScaleFactor: 1 });
  return {
    async render(svg) {
      await page.setContent(`<!doctype html><html><body style="margin:0;background:${C.bg}">${svg}</body></html>`);
      await page.evaluate(() => document.fonts.ready);
      return page.screenshot({ type: "png", clip: { x: 0, y: 0, width: W, height: H } });
    },
    close: () => browser.close(),
  };
}

const sharpRenderer = { render: (svg) => sharp(Buffer.from(svg)).png().toBuffer(), close: async () => {} };

const outPath = (e) => join(site, "public", e.card ?? `og/pages/${e.slug.replace(/\//g, "-")}.png`);

async function main() {
  const pages = JSON.parse(await readFile(join(site, "src/data/og-pages.json"), "utf8"));
  const state = await readFile(join(site, "src/data/network-state.ts"), "utf8");
  const paused = /\bpaused:\s*true\b/.test(state);
  const status = paused ? "Pre-mainnet, testnet run complete" : "Pre-mainnet, testnet running";
  const emblem = await readFile(join(site, "src/assets/emblem.png"));
  const emblemHref = `data:image/png;base64,${emblem.toString("base64")}`;
  const todo = pages.filter((e) => force || !existsSync(outPath(e)));
  let fontCss = "";
  let r = sharpRenderer;
  if (force && todo.length) {
    const inter = await readFile(join(site, "node_modules/@fontsource-variable/inter/files/inter-latin-wght-normal.woff2"));
    fontCss = `@font-face { font-family: Inter; font-weight: 100 900; src: url(data:font/woff2;base64,${inter.toString("base64")}) format("woff2"); }`;
    r = await chromiumRenderer();
  }
  try {
    for (const e of todo) {
      const png = await compress(await r.render(cardSvg(e, { emblemHref, fontCss, status })));
      await mkdir(dirname(outPath(e)), { recursive: true });
      await writeFile(outPath(e), png);
      console.log(`gen-og: ${outPath(e).slice(site.length + 1)} ${png.length} bytes`);
    }
  } finally {
    await r.close();
  }
  console.log(`gen-og: ${pages.length} cards, ${todo.length} rendered${force ? " (forced)" : ""}`);
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) await main();
