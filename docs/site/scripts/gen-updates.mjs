// Writes public/updates/<app>.json, the manifests the desktop apps' updater
// reads (apps/app-update), from the newest node-v* release that carries
// signed update bundles (*.sig, made when release CI has the signing key).
//
// The site, not GitHub's /releases/latest/, serves them: every release here
// is a pre-release, and "latest" skips pre-releases.
//
// No signed release yet, or GitHub unreachable: nothing is written and the
// build goes on. An app that finds no manifest simply has no update.
import { mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

const REPO = "EricWijesinghe/Maya2C";
const OUT = new URL("../public/updates/", import.meta.url);

// Release asset name prefix -> manifest name. Asset names have spaces
// turned into dots by the release job ("Maya2C Wallet" -> "Maya2C.Wallet").
// The wallet was "Maya Wallet" before its rename (L06); both prefixes map to
// one manifest, so a release from either side of the rename still updates.
export const APPS = {
  "Maya2C.Wallet": "wallet",
  "Maya.Wallet": "wallet",
  "Maya.Chat": "chat",
  "Maya2C.Hub": "hub",
};

/**
 * The manifest an asset belongs to, or undefined for one no app ships. The
 * prefix must end at "_" ("Maya2C.Wallet_0.1.1_x64-setup.exe") or "."
 * ("Maya2C.Wallet.app.tar.gz", macOS bundles carry no version).
 */
export function appOf(assetName) {
  const prefix = Object.keys(APPS).find(
    (p) => assetName.startsWith(`${p}_`) || assetName.startsWith(`${p}.`),
  );
  return prefix && APPS[prefix];
}

// Which updater bundle serves which Tauri platform key.
const PLATFORMS = [
  [/-setup\.exe$/, "windows-x86_64"],
  [/\.app\.tar\.gz$/, "darwin-aarch64"],
  [/\.AppImage$/, "linux-x86_64"],
];

const headers = { "User-Agent": "maya2c-site", Accept: "application/vnd.github+json" };
if (process.env.GITHUB_TOKEN) headers.Authorization = `Bearer ${process.env.GITHUB_TOKEN}`;

async function json(url) {
  const res = await fetch(url, { headers });
  if (!res.ok) throw new Error(`${url}: HTTP ${res.status}`);
  return res.json();
}

async function text(url) {
  const res = await fetch(url, { headers: { "User-Agent": headers["User-Agent"] } });
  if (!res.ok) throw new Error(`${url}: HTTP ${res.status}`);
  return (await res.text()).trim();
}

/** { app: { platform: { url, signature } } } for one release. */
async function bundles(release) {
  const byName = new Map(release.assets.map((a) => [a.name, a]));
  const out = {};
  for (const sig of release.assets.filter((a) => a.name.endsWith(".sig"))) {
    const bundle = byName.get(sig.name.slice(0, -".sig".length));
    const app = appOf(sig.name);
    const platform = PLATFORMS.find(([re]) => re.test(bundle?.name ?? ""))?.[1];
    if (!bundle || !app || !platform) {
      // Said, not silent: a renamed app whose prefix is missing above would
      // otherwise lose its update manifest without a word.
      console.warn(`updates: ${sig.name} matches no app or platform; skipped`);
      continue;
    }
    (out[app] ??= {})[platform] = {
      url: bundle.browser_download_url,
      signature: await text(sig.browser_download_url),
    };
  }
  return out;
}

/** The app version CI stamped into the bundle names: "<App>_<version>_...". */
function versionOf(platforms) {
  const url = Object.values(platforms)[0].url;
  return decodeURIComponent(url.split("/").pop()).split("_")[1] ?? null;
}

async function main() {
  const releases = await json(`https://api.github.com/repos/${REPO}/releases?per_page=20`);
  const signed = releases.find(
    (r) => r.tag_name.startsWith("node-v") && r.assets.some((a) => a.name.endsWith(".sig")),
  );
  if (!signed) {
    console.log("updates: no signed release yet; no manifests written");
    return;
  }
  await mkdir(OUT, { recursive: true });
  for (const [app, platforms] of Object.entries(await bundles(signed))) {
    const version = versionOf(platforms);
    if (!version) continue;
    const manifest = {
      version,
      notes: `Maya2C ${signed.tag_name}: https://github.com/${REPO}/releases/tag/${signed.tag_name}`,
      pub_date: signed.published_at,
      platforms,
    };
    await writeFile(join(OUT.pathname.replace(/^\/([A-Za-z]:)/, "$1"), `${app}.json`),
      JSON.stringify(manifest, null, 2));
    console.log(`updates: ${app} ${version} (${Object.keys(platforms).join(", ")}) from ${signed.tag_name}`);
  }
}

// Run as a script, not when a test imports appOf.
if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  main().catch((error) => console.warn(`updates: skipped (${error.message})`));
}
