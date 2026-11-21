// Phone-width layout check for the wallet UI (P-10, the Android wallet).
//
// Opens the built frontend (ui/dist, from `trunk build`) in Chromium at 360
// and 390 CSS px, walks every screen that needs no camera or node, and fails
// when the page scrolls sideways or any element pokes past the left or right
// edge. Text clipped inside its own box (overflow: hidden) is not detected.
//
// The Tauri bridge is replaced by a fake `window.__TAURI__.core.invoke` that
// answers with realistic worst-case data (24-word phrase, full-length
// addresses), so the check needs no core, no keychain and no network.
//
//   trunk build --config ui/Trunk.toml          (from apps/wallet-gui)
//   npm install --no-save --prefix ui/tests playwright@1.56.1
//   node ui/tests/overflow.mjs

import { chromium } from 'playwright';
import { createServer } from 'node:http';
import { existsSync, readFileSync, statSync } from 'node:fs';
import { extname, join, normalize, resolve, dirname, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const WIDTHS = [360, 390];
const HEIGHT = 800;
// One CSS px of slack: sub-pixel rounding of a 100%-wide box is not overflow.
const SLACK_PX = 1;

const dist = resolve(dirname(fileURLToPath(import.meta.url)), '..', 'dist');
if (!existsSync(join(dist, 'index.html'))) {
  console.error(`no build at ${dist}: run trunk build --config ui/Trunk.toml first`);
  process.exit(2);
}

const TYPES = {
  '.html': 'text/html', '.js': 'text/javascript', '.wasm': 'application/wasm',
  '.css': 'text/css', '.png': 'image/png', '.woff2': 'font/woff2', '.ico': 'image/x-icon',
  '.json': 'application/json', '.webmanifest': 'application/manifest+json', '.xml': 'text/xml',
};
const server = createServer((req, res) => {
  let file = join(dist, normalize(decodeURIComponent(new URL(req.url, 'http://x').pathname)));
  if (existsSync(file) && statSync(file).isDirectory()) file = join(file, 'index.html');
  // Refuse anything that normalizes outside dist.
  if (!file.startsWith(dist + sep) || !existsSync(file)) { res.writeHead(404).end(); return; }
  res.writeHead(200, { 'content-type': TYPES[extname(file)] ?? 'application/octet-stream' });
  res.end(readFileSync(file));
});
await new Promise((ok) => server.listen(0, '127.0.0.1', ok));
const origin = `http://127.0.0.1:${server.address().port}/`;

// Runs in the page before the wasm loads. `existing` picks Setup or Unlock.
function fakeTauri(existing) {
  const hex = 'ab'.repeat(32);
  const bech = 'maya1' + 'q'.repeat(58);
  const grouped = bech.match(/.{1,4}/g).join(' ');
  const account = (index) => ({ index, path: `m/44'/1717'/${index}'/0'/0'`, address: hex });
  const qr = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><rect width="10" height="10"/></svg>';
  const replies = {
    wallet_exists: existing,
    network_status: { network: 'maya2c-testnet', genesis: hex, height: 123456789, class: 'testnet', fingerprint: 'ab12 cd34 ef56' },
    create_wallet: { mnemonic: Array(24).fill('abandon').join(' '), account: account(0) },
    unlock: [account(0), account(1), account(2)],
    list_accounts: [account(0), account(1), account(2)],
    fetch_account: { balance: '123456789012345678901234567890', nonce: 42 },
    history: [{ transfer: { txid: hex, sender: hex, raw_hex: hex, recipient: hex, amount: '1000000000000000000', fee: '1000', nonce: 41 }, status: 'pending', detail: null }],
    receive_request: { address: bech, grouped, uri: `maya:${bech}?amount=1`, qr_svg: qr },
    contacts: [{ address: bech, grouped, name: 'A rather long contact name for a phone', note: 'rent' }],
    fee_options: { collector: null, base_fee: '1000', options: [{ label: 'Normal', fee: '1000' }, { label: 'Fast', fee: '2000' }], genesis: hex },
    take_pending_pay: null,
    check_phrase: true,
  };
  window.__TAURI__ = {
    core: {
      invoke: async (cmd) => {
        if (cmd in replies) return replies[cmd];
        throw `${cmd}: not faked in overflow.mjs`;
      },
    },
  };
}

// Every element whose left or right edge passes the viewport, plus the page's own
// sideways scroll. Returns human-readable offenders.
async function overflow(page) {
  return page.evaluate((slack) => {
    const vw = document.documentElement.clientWidth;
    const out = [];
    if (document.documentElement.scrollWidth > vw + slack) {
      out.push(`page scrollWidth ${document.documentElement.scrollWidth} > ${vw}`);
    }
    for (const el of document.body.querySelectorAll('*')) {
      const r = el.getBoundingClientRect();
      if (r.width && (r.right > vw + slack || r.left < -slack)) {
        out.push(`${el.tagName.toLowerCase()}.${[...el.classList].join('.')} left=${Math.round(r.left)} right=${Math.round(r.right)}`);
      }
    }
    return out.slice(0, 8);
  }, SLACK_PX);
}

const browser = await chromium.launch(
  process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {},
);
let failed = 0;
let checked = 0;

async function check(page, width, screen) {
  await page.waitForTimeout(250);
  const bad = await overflow(page);
  checked++;
  if (bad.length) {
    failed++;
    console.log(`FAIL ${width}px ${screen}: ${bad.join('; ')}`);
  } else {
    console.log(`ok   ${width}px ${screen}`);
  }
}

// Clicks a button by its exact visible text.
async function go(page, text) {
  await page.getByRole('button', { name: text, exact: true }).first().click();
}

for (const width of WIDTHS) {
  const ctx = await browser.newContext({ viewport: { width, height: HEIGHT }, deviceScaleFactor: 2, isMobile: true, hasTouch: true });

  // First run: Setup, then the one-time Backup of a 24-word phrase.
  let page = await ctx.newPage();
  await page.addInitScript(fakeTauri, false);
  await page.goto(origin);
  await page.locator('h1').first().waitFor();
  await check(page, width, 'setup');
  const pw = page.locator('input[type=password]');
  await pw.nth(0).fill('pass');
  await pw.nth(1).fill('pass');
  await go(page, 'Create new wallet');
  await page.locator('ol.phrase').waitFor();
  await check(page, width, 'backup');
  await page.close();

  // A stored wallet: Unlock, Wallet, then each screen off the action row.
  page = await ctx.newPage();
  await page.addInitScript(fakeTauri, true);
  await page.goto(origin);
  await page.getByRole('heading', { name: 'Unlock' }).waitFor();
  await check(page, width, 'unlock');
  await page.locator('input[type=password]').fill('pass');
  await go(page, 'Unlock');
  await page.locator('.hero-card').waitFor();
  await check(page, width, 'wallet');
  for (const [action, back] of [['Send', 'Back'], ['Receive', 'Back'], ['Contacts', 'Back'], ['Scan', 'Cancel']]) {
    await page.locator('.action', { hasText: action }).click();
    await check(page, width, action.toLowerCase());
    await page.getByRole('button', { name: back, exact: true }).last().click();
    await page.locator('.hero-card').waitFor();
  }
  await ctx.close();
}

await browser.close();
server.close();
console.log(`${checked - failed}/${checked} screen widths without horizontal overflow`);
process.exit(failed ? 1 : 0);
