// Lane L04 browser checks on the built site (docs/site/dist), in Chromium:
//
//  1. No horizontal overflow: document.scrollWidth <= innerWidth on every lane
//     page at 360, 390, 430, 768, 1024 and 1440 px, in both themes (owner,
//     2026-11-14: "some objects go over the display").
//  2. /experience/ built with the testnet paused opens no WebSocket and no
//     request to any maya2c.dev host.
//  3. axe-core, WCAG 2.2 A/AA rules, on /experience/ and /guides/testnet/ in
//     both themes: zero violations.
//  4. Screenshots of four pages at 360 and 1440 px in both themes, written
//     next to this file, so a reviewer sees the phone layout without a phone.
//
// Run after `npm run build` in docs/site:
//   node design/visual/site-paused/check.mjs
// CHROMIUM_PATH picks a browser binary; PW_MODULE and AXE_PATH point at a
// playwright and an axe.min.js outside this repo when they are not installed
// here (the site keeps no browser dependency in its package.json).

import { createServer } from 'node:http';
import { existsSync, readFileSync, statSync } from 'node:fs';
import { createRequire } from 'node:module';
import { dirname, extname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const { chromium } = await import(process.env.PW_MODULE || 'playwright');
const here = dirname(fileURLToPath(import.meta.url));
const dist = resolve(here, '../../../docs/site/dist');
const axeSource = readFileSync(process.env.AXE_PATH || createRequire(import.meta.url).resolve('axe-core/axe.min.js'), 'utf8');

const PAGES = [
  'experience', 'health', 'guides/testnet', 'guides/help-run', 'guides/start-mining', 'guides/validators',
  'guides/wallet', 'guides/contributors', 'guides/contracts', 'guides/api', 'guides/mining', 'guides/faq',
  'guides/pq-comparison', 'guides/break-it',
];
const WIDTHS = [360, 390, 430, 768, 1024, 1440];
const THEMES = ['dark', 'light'];
const AXE_PAGES = ['experience', 'guides/testnet'];
const SHOTS = ['experience', 'guides/testnet', 'guides/wallet', 'guides/contributors'];
const TYPES = { '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.svg': 'image/svg+xml', '.png': 'image/png', '.woff2': 'font/woff2', '.json': 'application/json', '.webp': 'image/webp', '.txt': 'text/plain' };

if (!existsSync(dist)) throw new Error(`no ${dist}: run npm run build in docs/site first`);

// A static server with the site's clean URLs (/x/ -> /x/index.html).
const server = createServer((req, res) => {
  let p = join(dist, decodeURIComponent(new URL(req.url, 'http://x').pathname));
  if (existsSync(p) && statSync(p).isDirectory()) p = join(p, 'index.html');
  if (!existsSync(p)) { res.writeHead(404); res.end(); return; }
  res.writeHead(200, { 'content-type': TYPES[extname(p)] || 'application/octet-stream' });
  res.end(readFileSync(p));
});
await new Promise((ok) => server.listen(0, '127.0.0.1', ok));
const origin = `http://127.0.0.1:${server.address().port}`;

const browser = await chromium.launch(process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {});
const failures = [];
const fail = (m) => { failures.push(m); console.log(`FAIL ${m}`); };

async function open(path, width, theme, height = 900, bypassCSP = false) {
  // bypassCSP only for axe, whose injected script the site's CSP rightly blocks.
  const ctx = await browser.newContext({ viewport: { width, height }, deviceScaleFactor: 1, colorScheme: theme, reducedMotion: 'reduce', bypassCSP });
  // Starlight reads its theme from this key before first paint.
  await ctx.addInitScript((t) => { try { localStorage.setItem('starlight-theme', t); } catch {} }, theme);
  const page = await ctx.newPage();
  await page.goto(`${origin}/${path}/`, { waitUntil: 'load' });
  return { ctx, page };
}

// 1. Overflow.
let measured = 0;
for (const theme of THEMES) {
  for (const width of WIDTHS) {
    for (const path of PAGES) {
      const { ctx, page } = await open(path, width, theme);
      const { sw, iw, culprit } = await page.evaluate(() => {
        const iw = window.innerWidth;
        let culprit = '';
        for (const el of document.querySelectorAll('body *')) {
          const r = el.getBoundingClientRect();
          if (r.right > iw + 1 && getComputedStyle(el).position !== 'fixed') { culprit = `${el.tagName.toLowerCase()}.${[...el.classList].join('.')} right=${Math.round(r.right)}`; break; }
        }
        return { sw: document.documentElement.scrollWidth, iw, culprit };
      });
      measured++;
      if (sw > iw) fail(`overflow /${path}/ ${width}px ${theme}: scrollWidth ${sw} > ${iw} (${culprit})`);
      await ctx.close();
    }
  }
}
console.log(`overflow: ${measured} page/width/theme combinations measured`);

// 2. /experience/ while paused: no socket, no call to a maya2c.dev host.
{
  const { ctx, page } = await open('experience', 1440, 'dark');
  const hits = [];
  page.on('websocket', (ws) => hits.push(`websocket ${ws.url()}`));
  page.on('request', (r) => { if (/(^|\.)maya2c\.dev$/.test(new URL(r.url()).hostname)) hits.push(`${r.method()} ${r.url()}`); });
  await page.reload({ waitUntil: 'networkidle' });
  await page.waitForTimeout(3000);
  const requests = await page.evaluate(() => performance.getEntriesByType('resource').map((e) => e.name));
  const external = requests.filter((u) => /maya2c\.dev/.test(new URL(u).hostname));
  console.log(`experience network: ${requests.length} resources, ${external.length} to maya2c.dev hosts, ${hits.length} sockets/requests to maya2c.dev`);
  for (const h of [...hits, ...external]) fail(`experience opened ${h}`);
  await ctx.close();
}

// 3. axe-core.
for (const theme of THEMES) {
  for (const path of AXE_PAGES) {
    const { ctx, page } = await open(path, 1440, theme, 900, true);
    await page.addScriptTag({ content: axeSource });
    const result = await page.evaluate(() => window.axe.run(document, { runOnly: { type: 'tag', values: ['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa'] } }));
    console.log(`axe /${path}/ ${theme}: ${result.violations.length} violations, ${result.passes.length} rules passed`);
    for (const v of result.violations) fail(`axe /${path}/ ${theme}: ${v.id} (${v.nodes.length}) ${v.nodes.slice(0, 2).map((n) => n.target.join(' ')).join(' | ')}`);
    await ctx.close();
  }
}

// 4. Screenshots.
for (const theme of THEMES) {
  for (const width of [360, 1440]) {
    for (const path of SHOTS) {
      const { ctx, page } = await open(path, width, theme, width === 360 ? 780 : 900);
      await page.screenshot({ path: join(here, `${path.replace(/\//g, '-')}-${width}-${theme}.png`) });
      await ctx.close();
    }
  }
}
console.log(`screenshots: ${SHOTS.length * 4} written to ${here}`);

await browser.close();
server.close();
if (failures.length) { console.log(`${failures.length} failure(s)`); process.exit(1); }
console.log('all checks passed');
