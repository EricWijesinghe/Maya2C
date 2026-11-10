// Renders one 1200x630 link-preview card per main page from src/data/og-pages.json.
//
// Local and free: sharp rasterises an SVG that this script writes, with the
// emblem embedded. No browser, no paid image service. A card that already
// exists is kept, so the build does not churn committed PNGs; `--force`
// re-renders them all. The card text is the page's own title and summary, so a
// preview never shows a claim the page does not make.
import { readFile, mkdir } from "node:fs/promises";
import { existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { join } from "node:path";
import sharp from "sharp";

const here = fileURLToPath(new URL(".", import.meta.url));
const site = join(here, "..");
const force = process.argv.includes("--force");
const W = 1200;
const H = 630;
// Brand tokens from src/styles/maya.css (dark theme).
const BG = "#050b0f";
const INK = "#edf3f2";
const MUTED = "#93a4aa";
const TEAL = "#1fb5c2";
const BRONZE = "#d9b97a";
const FONT = "'Segoe UI', 'Helvetica Neue', Arial, sans-serif";

const esc = (s) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");

// Greedy word wrap. Character width is estimated from the font size (about
// 0.55 em for a sans face), so lines stay inside the card without measuring glyphs.
function wrap(text, size, maxWidth, maxLines) {
  const perLine = Math.floor(maxWidth / (size * 0.55));
  const lines = [];
  let cur = "";
  for (const word of text.split(/\s+/)) {
    const next = cur ? `${cur} ${word}` : word;
    if (next.length <= perLine) cur = next;
    else {
      lines.push(cur);
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

const tspans = (lines, x, y, lineHeight, attrs) =>
  lines.map((l, i) => `<tspan x="${x}" y="${y + i * lineHeight}" ${attrs}>${esc(l)}</tspan>`).join("");

async function card(entry, emblemHref) {
  const title = wrap(entry.title, 66, 1040, 2);
  const line = wrap(entry.line, 30, 1040, 3);
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="${W}" height="${H}" viewBox="0 0 ${W} ${H}">
  <defs>
    <linearGradient id="g" x1="0" y1="0" x2="1" y2="1">
      <stop offset="0" stop-color="${BG}"/><stop offset="1" stop-color="#0b2228"/>
    </linearGradient>
    <linearGradient id="rule" x1="0" y1="0" x2="1" y2="0">
      <stop offset="0" stop-color="${TEAL}"/><stop offset="1" stop-color="${BRONZE}"/>
    </linearGradient>
  </defs>
  <rect width="${W}" height="${H}" fill="url(#g)"/>
  <image href="${emblemHref}" x="80" y="70" width="120" height="120"/>
  <text x="222" y="130" font-family="${FONT}" font-size="40" font-weight="800" fill="${INK}">Maya2C</text>
  <text x="222" y="170" font-family="${FONT}" font-size="22" fill="${MUTED}">Post-quantum layer-1 blockchain</text>
  <rect x="80" y="230" width="140" height="6" rx="3" fill="url(#rule)"/>
  <text font-family="${FONT}" font-size="66" font-weight="800" fill="${INK}">${tspans(title, 80, 340, 78, "")}</text>
  <text font-family="${FONT}" font-size="30" fill="${MUTED}">${tspans(line, 80, 340 + (title.length - 1) * 78 + 80, 42, "")}</text>
  <text x="80" y="606" font-family="${FONT}" font-size="24" fill="${TEAL}">maya2c.dev</text>
</svg>`;
  return sharp(Buffer.from(svg)).png().toBuffer();
}

const pages = JSON.parse(await readFile(join(site, "src/data/og-pages.json"), "utf8"));
const emblem = await readFile(join(site, "src/assets/emblem.png"));
const emblemHref = `data:image/png;base64,${emblem.toString("base64")}`;
const outDir = join(site, "public/og/pages");
await mkdir(outDir, { recursive: true });

let made = 0;
for (const entry of pages) {
  const out = join(outDir, `${entry.slug.replace(/\//g, "-")}.png`);
  if (existsSync(out) && !force) continue;
  const png = await card(entry, emblemHref);
  const meta = await sharp(png).metadata();
  if (meta.width !== W || meta.height !== H) throw new Error(`gen-og: ${out} is ${meta.width}x${meta.height}`);
  await sharp(png).toFile(out);
  made += 1;
}
console.log(`gen-og: ${pages.length} cards, ${made} rendered${force ? " (forced)" : ""}`);
