// Bundles the extension for Chrome and Firefox into dist/<browser>/.
// Requires wasm/ from build-wasm.sh. Everything shipped is local: no remote
// script, no CDN, nothing fetched at run time but the gateway's JSON.
import { build } from "esbuild";
import { cpSync, mkdirSync, readFileSync, rmSync, writeFileSync, existsSync } from "node:fs";

const here = new URL(".", import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, "$1");
const icons = `${here}../wallet-gui/src-tauri/icons`;
if (!existsSync(`${here}wasm/maya_sdk_wasm_bg.wasm`)) {
  throw new Error("wasm/ is missing: run `npm run build:wasm` first");
}

const base = JSON.parse(readFileSync(`${here}manifest.base.json`, "utf8"));
const targets = {
  chrome: { ...base, background: { service_worker: "background.js" }, minimum_chrome_version: "111" },
  firefox: {
    ...base,
    background: { scripts: ["background.js"] },
    browser_specific_settings: { gecko: { id: "wallet-extension@maya2c.dev", strict_min_version: "128.0" } },
  },
};

for (const [name, manifest] of Object.entries(targets)) {
  const out = `${here}dist/${name}`;
  rmSync(out, { recursive: true, force: true });
  mkdirSync(`${out}/icons`, { recursive: true });
  mkdirSync(`${out}/wasm`, { recursive: true });
  await build({
    entryPoints: ["background", "content", "inpage", "popup"].map((e) => `${here}src/${e}.ts`),
    outdir: out,
    bundle: true,
    format: "iife",
    target: "es2022",
    minify: false, // reviewable by a store reviewer and by the person installing it
    legalComments: "none",
    // wasm-bindgen's default path uses import.meta.url; background.ts always
    // passes an explicit runtime URL, so the default is never evaluated.
    define: { "import.meta.url": '"about:invalid"' },
    logLevel: "warning",
  });
  for (const f of ["popup.html", "popup.css"]) cpSync(`${here}src/${f}`, `${out}/${f}`);
  cpSync(`${icons}/32x32.png`, `${out}/icons/32.png`);
  cpSync(`${icons}/128x128.png`, `${out}/icons/128.png`);
  cpSync(`${here}wasm/maya_sdk_wasm_bg.wasm`, `${out}/wasm/maya_sdk_wasm_bg.wasm`);
  writeFileSync(`${out}/manifest.json`, JSON.stringify(manifest, null, 2));
  console.log(`built dist/${name}`);
}
