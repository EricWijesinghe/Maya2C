//! `cargo xtask design-tokens --literals <dir>`: every hex or functional
//! colour written by hand outside `tokens.css`.
//!
//! The token file only helps if surfaces name its roles. Each later UI lane
//! (U2–U7) runs this on its own folder and drives the count to zero. The one
//! exception is a `<meta name="theme-color">` line: the browser reads it
//! before any stylesheet, so it cannot name a custom property. There is no
//! general opt-out marker, so nothing else can hide a literal.

use std::fs;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

/// The only line whose literal the platform requires.
const THEME_COLOR: &str = "name=\"theme-color\"";

/// Files that carry styles or markup. Markdown, SVG assets and images are not
/// scanned: a logo keeps its own colours.
const EXTENSIONS: &[&str] = &[
    "css", "scss", "html", "htm", "astro", "js", "mjs", "cjs", "ts", "tsx", "jsx", "svelte", "vue",
    "rs",
];

/// Build output and dependencies, never a surface's own source.
const SKIP_DIRS: &[&str] = &[
    "node_modules",
    "target",
    "dist",
    ".astro",
    ".git",
    ".svelte-kit",
];

/// `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`, not preceded by `&` (an HTML
/// character reference such as `&#8212;`) or a word character.
static HEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^|[^&\w#])(#(?:[0-9a-fA-F]{8}|[0-9a-fA-F]{6}|[0-9a-fA-F]{3,4}))\b")
        .expect("static regex")
});

/// A colour function with a literal first argument (`rgb(var(--x))` is not one).
static FUNC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|[^\w-])((?:rgba?|hsla?|hwb|oklch|oklab|lab|lch)\(\s*[\d.])")
        .expect("static regex")
});

pub(super) fn run(dir: &Path) -> Result<(), String> {
    let found = scan(dir)?;
    for f in &found {
        println!("{f}");
    }
    if found.is_empty() {
        println!("no colour literals outside tokens.css in {}", dir.display());
        return Ok(());
    }
    Err(format!(
        "{} colour literal(s) outside tokens.css in {}: name a role from design/tokens.json",
        found.len(),
        dir.display()
    ))
}

/// `path:line: literal` for every literal under `dir`, paths relative to it,
/// in sorted order.
pub(super) fn scan(dir: &Path) -> Result<Vec<String>, String> {
    if !dir.is_dir() {
        return Err(format!("{}: not a directory", dir.display()));
    }
    let mut out = Vec::new();
    visit(dir, dir, &mut out)?;
    Ok(out)
}

fn visit(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<(), String> {
    let mut entries = fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .map(|e| e.map(|e| e.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("{}: {e}", dir.display()))?;
    entries.sort();
    for path in entries {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if path.is_dir() {
            if !SKIP_DIRS.contains(&name) {
                visit(root, &path, out)?;
            }
            continue;
        }
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default();
        if name == "tokens.css" || !EXTENSIONS.contains(&ext) {
            continue;
        }
        let bytes = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        for (n, line) in String::from_utf8_lossy(&bytes).lines().enumerate() {
            if line.contains(THEME_COLOR) {
                continue;
            }
            for re in [&*HEX, &*FUNC] {
                for c in re.captures_iter(line) {
                    out.push(format!("{rel}:{}: {}", n + 1, &c[1]));
                }
            }
        }
    }
    Ok(())
}
