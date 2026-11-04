//! `cargo xtask brand-assets [--check]` — copies the logo pack's web assets into
//! each app that serves a page, and writes each app's `site.webmanifest`.
//!
//! Three apps need the same favicon set, and keeping three copies in step by hand
//! after the pack changes is how they drift. `--check` reports drift and changes
//! nothing; CI and reviewers run it.
//!
//! Two defects in the pack are corrected here rather than propagated:
//! `favicons/site.webmanifest` ships with an empty `name` and `short_name`, which
//! an installed PWA shows under its icon, so each app gets its own; and its icon
//! paths are root-absolute, which the apps (served under `/assets/`) cannot
//! resolve, so they are made relative. `logo-assets/` itself is never touched.

use std::fs;
use std::path::{Path, PathBuf};

use crate::ordered_json::Json;

use crate::pyfmt;

const PACK: &str = "logo-assets";

/// Copied byte-for-byte into every app: the whole favicon set, since a browser
/// asks for whichever size it wants and a missing one 404s in somebody's console.
const FAVICON_FILES: &[&str] = &[
    "favicon.ico",
    "favicon-16x16.png",
    "favicon-32x32.png",
    "favicon-48x48.png",
    "apple-touch-icon.png",
    "android-chrome-192x192.png",
    "android-chrome-512x512.png",
    "mstile-150x150.png",
    "browserconfig.xml",
];

/// The nav wordmark at both densities: `srcset` picks between them.
const FULL_LOGO: &[(&str, &str)] = &[
    ("website/Header-Logo_250x100.png", "logo.png"),
    ("website/Header-Logo-2x_500x200.png", "logo@2x.png"),
];

/// The compact pair for a titlebar too short for the full wordmark.
const COMPACT_LOGO: &[(&str, &str)] = &[
    ("website/Compact-Header_200x50.png", "logo.png"),
    ("website/Compact-Header-2x_400x100.png", "logo@2x.png"),
];

struct App {
    name: &'static str,
    assets: &'static str,
    short_name: &'static str,
    logo: &'static [(&'static str, &'static str)],
}

const APPS: &[App] = &[
    App {
        name: "Maya2C Explorer",
        assets: "apps/explorer/assets",
        short_name: "Explorer",
        logo: FULL_LOGO,
    },
    App {
        name: "Maya2C Network",
        assets: "apps/dashboard/assets",
        short_name: "Maya2C",
        logo: FULL_LOGO,
    },
    App {
        name: "Maya Wallet",
        assets: "apps/wallet-gui/ui/assets",
        short_name: "Wallet",
        logo: COMPACT_LOGO,
    },
];

pub fn run(args: &[String]) -> Result<(), String> {
    let check = match args {
        [] => false,
        [flag] if flag == "--check" => true,
        _ => return Err("usage: cargo xtask brand-assets [--check]".into()),
    };
    let root = crate::workspace_root();
    let files = planned(&root)?;

    if check {
        let mut stale = Vec::new();
        for (path, content) in &files {
            if fs::read(path).ok().as_deref() != Some(content.as_slice()) {
                stale.push(rel(&root, path));
            }
        }
        if !stale.is_empty() {
            for path in &stale {
                eprintln!("stale or missing: {path}");
            }
            return Err("run: cargo xtask brand-assets".into());
        }
        println!("{} assets current across {} apps", files.len(), APPS.len());
        return Ok(());
    }

    for (path, content) in &files {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        fs::write(path, content).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    println!("deployed {} assets across {} apps", files.len(), APPS.len());
    Ok(())
}

/// Every file this command writes, with its exact content.
fn planned(root: &Path) -> Result<Vec<(PathBuf, Vec<u8>)>, String> {
    let pack = root.join(PACK);
    let mut out = Vec::new();
    for app in APPS {
        let assets = root.join(app.assets);
        for name in FAVICON_FILES {
            out.push((assets.join(name), read(&pack.join("favicons").join(name))?));
        }
        for (source, name) in app.logo {
            out.push((assets.join(name), read(&pack.join(source))?));
        }
        out.push((assets.join("site.webmanifest"), manifest(&pack, app)?));
    }
    Ok(out)
}

/// Python's `json.dumps(indent=2)` plus a newline, key order preserved.
fn manifest(pack: &Path, app: &App) -> Result<Vec<u8>, String> {
    let path = pack.join("favicons").join("site.webmanifest");
    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut manifest = Json::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    manifest.set("name", Json::String(app.name.into()))?;
    manifest.set("short_name", Json::String(app.short_name.into()))?;
    if let Some(Json::Array(icons)) = manifest.get_mut("icons") {
        for icon in icons {
            let src = icon.get("src").and_then(Json::as_str).map(str::to_owned);
            if let Some(src) = src {
                icon.set(
                    "src",
                    Json::String(format!("./{}", src.trim_start_matches('/'))),
                )?;
            }
        }
    }
    let text = pyfmt::json(&manifest, 2)? + "\n";
    Ok(text.into_bytes())
}

fn read(path: &Path) -> Result<Vec<u8>, String> {
    fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn rel(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}
