//! `cargo xtask check-brand-refs` — every branding reference in the frontends
//! resolves to a real file.
//!
//! Trunk fails a build on a missing `data-trunk` asset, so the frontends'
//! *directories* are verified by building them. It does not check the
//! individual `href`/`src` paths inside the HTML: a favicon link to a file the
//! pack does not ship builds green and 404s in the browser. So this walks each
//! built `dist/` and resolves every reference against the files shipped there.
//!
//! It also scans each frontend's Rust source. The header logo is rendered by
//! wasm at runtime; its path is a string literal in a `view!` macro that never
//! appears in the built HTML. The explorer is not checked here: its HTML is a
//! Rust string with no build step, and `apps/explorer/tests/server_tests.rs`
//! asks the running server for every path the page mentions.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use regex::Regex;

/// Each frontend: its built output, and the source whose `view!` macros name
/// assets that never reach `index.html`.
const BUNDLES: &[(&str, &str)] = &[
    ("dashboard/dist", "dashboard/src"),
    ("wallet-gui/ui/dist", "wallet-gui/ui/src"),
];

pub fn run(_args: &[String]) -> ExitCode {
    let root = crate::workspace_root();
    let mut problems = Vec::new();
    for (bundle, source) in BUNDLES {
        match check(&root, &root.join(bundle), &root.join(source)) {
            Ok(found) if found.is_empty() => println!("ok  {bundle} + {source}"),
            Ok(found) => problems.extend(found),
            Err(e) => {
                eprintln!("check-brand-refs: {e}");
                return ExitCode::FAILURE;
            }
        }
    }
    if !problems.is_empty() {
        for problem in &problems {
            eprintln!("{problem}");
        }
        return ExitCode::FAILURE;
    }
    println!(
        "every branding reference in {} bundles resolves",
        BUNDLES.len()
    );
    ExitCode::SUCCESS
}

/// Unresolvable references in one built bundle and its source.
fn check(root: &Path, bundle: &Path, source: &Path) -> Result<Vec<String>, String> {
    let index = bundle.join("index.html");
    let shown = rel(root, bundle);
    if !index.exists() {
        return Ok(vec![format!("{shown} has no index.html — build it first")]);
    }
    let html = fs::read_to_string(&index).map_err(|e| format!("{}: {e}", index.display()))?;
    let attr = Regex::new(r#"(?:href|src|srcset|content)="([^"]+)""#).map_err(|e| e.to_string())?;
    // Asset paths written in a `view!` macro, from every Rust file in the
    // crate rather than a named one: a logo moved to a new component should
    // still be checked.
    let asset = Regex::new(r#""((?:\./)?assets/[^"]+)""#).map_err(|e| e.to_string())?;
    let mut values: Vec<String> = attr
        .captures_iter(&html)
        .map(|c| c[1].to_string())
        .collect();
    for file in rust_files(source)? {
        let text = fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
        values.extend(asset.captures_iter(&text).map(|c| c[1].to_string()));
    }

    let mut problems = Vec::new();
    for value in &values {
        for path in paths_in(value) {
            // External URLs, data URIs, and in-page anchors name no file.
            if ["http://", "https://", "data:", "#", "//"]
                .iter()
                .any(|p| path.starts_with(p))
            {
                continue;
            }
            // Non-path `content=` values: a theme colour, a description.
            if !path.contains('/') && !path.contains('.') {
                continue;
            }
            if !bundle.join(path.trim_start_matches('/')).exists() {
                problems.push(format!("{shown}: {path} does not exist"));
            }
        }
    }
    Ok(problems)
}

/// Every file path inside one attribute value. A `srcset` is a comma-separated
/// list of `url descriptor` pairs, so each value is split on commas and the
/// descriptor dropped; treating a srcset as one path reports a good page broken.
fn paths_in(value: &str) -> Vec<&str> {
    value
        .split(',')
        .filter_map(|candidate| candidate.split_whitespace().next())
        .collect()
}

/// Every `*.rs` under `dir`, sorted; empty when `dir` does not exist.
fn rust_files(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(current) = pending.pop() {
        let Ok(entries) = fs::read_dir(&current) else {
            continue;
        };
        for entry in entries {
            let path = entry.map_err(|e| e.to_string())?.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                found.push(path);
            }
        }
    }
    found.sort();
    Ok(found)
}

fn rel(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::paths_in;

    #[test]
    fn srcset_is_split_and_descriptors_dropped() {
        assert_eq!(paths_in("a.png 1x, b.png 2x"), vec!["a.png", "b.png"]);
        assert_eq!(paths_in("  , x.svg"), vec!["x.svg"]);
    }
}
