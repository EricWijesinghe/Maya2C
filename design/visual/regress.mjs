// Visual regression for the design-system specimen (Master Prompt 29 §1).
//
// Screenshots every <section> of design/specimen.html in Chromium and compares
// it pixel by pixel with design/visual/baseline/<id>.png. A pixel "differs"
// when any channel moves by more than CHANNEL_TOLERANCE; the run fails when
// more than MAX_DIFF_RATIO of a section's pixels differ, or its size changes.
//
// Font rendering differs between machines, so baselines are only comparable
// when produced by the same Chromium build and fonts. CI therefore runs this in
// the playwright container pinned in .github/workflows/ci.yml, and a baseline
// is regenerated there (UPDATE_BASELINE=1), never on a laptop.
//
//   node design/visual/regress.mjs          compare
//   UPDATE_BASELINE=1 node design/visual/regress.mjs   rewrite baselines

import { chromium } from 'playwright';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const CHANNEL_TOLERANCE = 32;
const MAX_DIFF_RATIO = 0.001;

const here = dirname(fileURLToPath(import.meta.url));
const specimen = resolve(here, '..', 'specimen.html');
const baselineDir = join(here, 'baseline');
const outDir = join(here, 'out');
const update = Boolean(process.env.UPDATE_BASELINE);

const browser = await chromium.launch(
  process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {},
);
const page = await browser.newPage({ viewport: { width: 800, height: 600 }, deviceScaleFactor: 1 });
await page.goto('file://' + specimen);
const ids = await page.$$eval('section', (s) => s.map((e) => e.id));

// Compares two PNGs inside the page, so the job needs no image library.
async function diff(a, b) {
  return page.evaluate(async ([a, b, tol]) => {
    const load = (b64) => new Promise((ok, err) => {
      const img = new Image();
      img.onload = () => ok(img);
      img.onerror = err;
      img.src = 'data:image/png;base64,' + b64;
    });
    const [ia, ib] = await Promise.all([load(a), load(b)]);
    if (ia.width !== ib.width || ia.height !== ib.height) {
      return { sizeChanged: `${ib.width}x${ib.height} -> ${ia.width}x${ia.height}` };
    }
    const px = (img) => {
      const c = document.createElement('canvas');
      c.width = img.width; c.height = img.height;
      const ctx = c.getContext('2d');
      ctx.drawImage(img, 0, 0);
      return ctx.getImageData(0, 0, img.width, img.height).data;
    };
    const [da, db] = [px(ia), px(ib)];
    let changed = 0;
    for (let i = 0; i < da.length; i += 4) {
      if (Math.abs(da[i] - db[i]) > tol || Math.abs(da[i + 1] - db[i + 1]) > tol
        || Math.abs(da[i + 2] - db[i + 2]) > tol) changed++;
    }
    return { ratio: changed / (da.length / 4) };
  }, [a.toString('base64'), b.toString('base64'), CHANNEL_TOLERANCE]);
}

let failed = 0;
mkdirSync(baselineDir, { recursive: true });
for (const id of ids) {
  const shot = await page.locator('#' + id).screenshot();
  const base = join(baselineDir, id + '.png');
  if (update || !existsSync(base)) {
    writeFileSync(base, shot);
    console.log(`${id}: baseline written`);
    continue;
  }
  const r = await diff(shot, readFileSync(base));
  if (r.sizeChanged || r.ratio > MAX_DIFF_RATIO) {
    failed++;
    mkdirSync(outDir, { recursive: true });
    writeFileSync(join(outDir, id + '.png'), shot);
    console.log(`${id}: FAIL ${r.sizeChanged ?? (r.ratio * 100).toFixed(3) + '% of pixels changed'}`);
  } else {
    console.log(`${id}: ok (${(r.ratio * 100).toFixed(3)}% of pixels changed)`);
  }
}
await browser.close();
console.log(`${ids.length} sections, ${failed} failed`);
process.exit(failed ? 1 : 0);
