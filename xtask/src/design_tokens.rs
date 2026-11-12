//! `cargo xtask design-tokens [--check]` — generates `tokens.css` from
//! `design/tokens.json` (Forged Light 2, ADR-136) into every surface.
//!
//! Five surfaces each wrote their own stylesheet from one palette and drifted
//! into five products (docs/design/audit-2026-11.md). One source, one
//! generator, byte-identical copies, and `--check` in CI is what keeps them in
//! step. Both modes refuse a token file whose listed text pairs miss WCAG AA,
//! so a colour change cannot land without its measured ratio.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use maya_design_system::contrast::{Rgb, Use, ratio};
use sha2::{Digest, Sha256};

use crate::ordered_json::Json;

/// The single source.
const SOURCE: &str = "design/tokens.json";

/// Every generated copy. `design/` holds the specimen's copy
/// (`crates/design-system/src/specimen.rs` inlines it).
const TARGETS: &[&str] = &[
    "design/tokens.css",
    "docs/site/src/styles/tokens.css",
    "apps/explorer/assets/tokens.css",
    "apps/hub/ui/tokens.css",
    "apps/chat-gui/ui/tokens.css",
    "apps/wallet-gui/ui/tokens.css",
];

/// Theme names in `tokens.json`; the first is the default.
const THEMES: [&str; 2] = ["dark", "light"];

pub fn run(args: &[String]) -> Result<(), String> {
    let check = match args {
        [] => false,
        [flag] if flag == "--check" => true,
        _ => return Err("usage: cargo xtask design-tokens [--check]".into()),
    };
    let root = crate::workspace_root();
    let tokens = load(&root)?;
    let failures = contrast_failures(&tokens)?;
    if !failures.is_empty() {
        for f in &failures {
            eprintln!("below WCAG AA: {f}");
        }
        return Err(format!("{SOURCE}: {} contrast pair(s) fail", failures.len()));
    }
    let css = render(&tokens)?;
    if check {
        let stale = stale(&root, &css);
        if !stale.is_empty() {
            for path in &stale {
                eprintln!("stale or missing: {path}");
            }
            return Err("run: cargo xtask design-tokens".into());
        }
        println!("tokens.css current in {} surfaces", TARGETS.len());
        return Ok(());
    }
    for target in TARGETS {
        let path = root.join(target);
        fs::write(&path, &css).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    println!("wrote tokens.css to {} surfaces", TARGETS.len());
    Ok(())
}

fn load(root: &Path) -> Result<Json, String> {
    let path = root.join(SOURCE);
    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    Json::parse(&text).map_err(|e| format!("{SOURCE}: {e}"))
}

/// One measured pair: theme, foreground role, background role, use, ratio.
#[derive(Debug)]
struct Measured {
    theme: &'static str,
    fg: String,
    bg: String,
    usage: Use,
    ratio: f64,
}

type Decls = Vec<(String, String)>;

/// Entries of an object, without `$comment` keys.
fn entries<'a>(value: &'a Json, what: &str) -> Result<Vec<(&'a str, &'a Json)>, String> {
    match value {
        Json::Object(e) => Ok(e
            .iter()
            .filter(|(k, _)| !k.starts_with('$'))
            .map(|(k, v)| (k.as_str(), v))
            .collect()),
        _ => Err(format!("{what}: expected an object")),
    }
}

fn group<'a>(value: &'a Json, key: &str) -> Result<&'a Json, String> {
    value.get(key).ok_or_else(|| format!("{SOURCE}: no `{key}`"))
}

/// Leaves of `value` as (property name, CSS value) in file order; nested keys
/// join with `-`.
fn flatten(prefix: &str, value: &Json, out: &mut Decls) -> Result<(), String> {
    match value {
        Json::String(v) => {
            check_name(prefix)?;
            check_value(prefix, v)?;
            out.push((prefix.to_owned(), v.clone()));
            Ok(())
        }
        Json::Object(_) => {
            for (k, v) in entries(value, prefix)? {
                let name = if prefix.is_empty() { k.to_owned() } else { format!("{prefix}-{k}") };
                flatten(&name, v, out)?;
            }
            Ok(())
        }
        _ => Err(format!("--{prefix}: token values are CSS strings")),
    }
}

fn check_name(name: &str) -> Result<(), String> {
    let ok = name.starts_with(|c: char| c.is_ascii_lowercase())
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if ok { Ok(()) } else { Err(format!("`{name}`: token names are [a-z][a-z0-9-]*")) }
}

/// The value lands verbatim inside a declaration, so nothing that could end it,
/// open a block or close the header comment is accepted.
fn check_value(name: &str, v: &str) -> Result<(), String> {
    let bad = v.trim().is_empty()
        || v.contains(['{', '}', ';', '<', '\\', '\n', '\r'])
        || v.contains("*/");
    if bad { Err(format!("--{name}: value `{v}` is not a plain CSS value")) } else { Ok(()) }
}

fn theme(tokens: &Json, name: &str) -> Result<Decls, String> {
    let mut out = Vec::new();
    flatten("", group(group(tokens, "theme")?, name)?, &mut out)?;
    Ok(out)
}

fn pair_ratios(tokens: &Json) -> Result<Vec<Measured>, String> {
    let pairs = group(tokens, "contrast-pairs")?
        .get("pairs")
        .and_then(Json::as_array)
        .ok_or("contrast-pairs.pairs: expected an array")?;
    let mut out = Vec::new();
    for theme_name in THEMES {
        let roles = theme(tokens, theme_name)?;
        let colour = |role: &str| -> Result<Rgb, String> {
            let (_, v) = roles
                .iter()
                .find(|(n, _)| n == role)
                .ok_or_else(|| format!("contrast pair names unknown role `{role}`"))?;
            Rgb::hex(v).ok_or_else(|| format!("{theme_name} {role} = {v}: a contrast pair needs #rrggbb"))
        };
        for pair in pairs {
            let [fg, bg, usage] = triple(pair)?;
            let usage = match usage {
                "text" => Use::Text,
                "large-text" => Use::LargeText,
                "non-text" => Use::NonText,
                other => return Err(format!("contrast pair use `{other}`: text, large-text or non-text")),
            };
            out.push(Measured {
                theme: theme_name,
                fg: fg.to_owned(),
                bg: bg.to_owned(),
                usage,
                ratio: ratio(colour(fg)?, colour(bg)?),
            });
        }
    }
    Ok(out)
}

fn triple(pair: &Json) -> Result<[&str; 3], String> {
    let err = || "each contrast pair is [foreground, background, use]".to_owned();
    match pair.as_array().map(Vec::as_slice) {
        Some([a, b, c]) => match (a.as_str(), b.as_str(), c.as_str()) {
            (Some(a), Some(b), Some(c)) => Ok([a, b, c]),
            _ => Err(err()),
        },
        _ => Err(err()),
    }
}

/// `"<theme> <fg> on <bg> <ratio> (min <n>)"` for every pair below its minimum.
fn contrast_failures(tokens: &Json) -> Result<Vec<String>, String> {
    Ok(pair_ratios(tokens)?
        .into_iter()
        .filter(|m| m.ratio < m.usage.minimum())
        .map(|m| format!("{} {} on {} {:.2} (min {})", m.theme, m.fg, m.bg, m.ratio, m.usage.minimum()))
        .collect())
}

fn render(tokens: &Json) -> Result<String, String> {
    let dark = theme(tokens, THEMES[0])?;
    let light = theme(tokens, THEMES[1])?;
    let names = |d: &Decls| d.iter().map(|(n, _)| n.clone()).collect::<BTreeSet<_>>();
    let (dn, ln) = (names(&dark), names(&light));
    if dn != ln {
        let diff: Vec<_> = dn.symmetric_difference(&ln).cloned().collect();
        return Err(format!("dark and light name different roles: {}", diff.join(", ")));
    }
    let mut global = Vec::new();
    for (key, g) in entries(group(tokens, "global")?, "global")? {
        if !matches!(g, Json::Object(_)) {
            return Err(format!("global.{key}: expected a group object"));
        }
        flatten("", g, &mut global)?;
    }
    let mut seen = dn;
    if let Some((n, _)) = global.iter().find(|(n, _)| !seen.insert(n.clone())) {
        return Err(format!("--{n} is defined twice"));
    }
    let mut reduced = Vec::new();
    flatten("", group(tokens, "reduced-motion")?, &mut reduced)?;
    if let Some((n, _)) = reduced.iter().find(|(n, _)| !global.iter().any(|(g, _)| g == n)) {
        return Err(format!("reduced-motion overrides unknown token --{n}"));
    }

    // Every selector has one class's specificity, so a surface's own sheet,
    // loaded after this one, still wins on a shared name; the attribute blocks
    // come after the media query, so an explicit choice beats the OS.
    let mut body = String::new();
    block(&mut body, "", ":root", Some("dark"), &dark);
    body.push_str("@media (prefers-color-scheme: light) {\n");
    block(&mut body, "  ", ":root", Some("light"), &light);
    body.push_str("}\n");
    block(&mut body, "", "[data-theme=\"dark\"]", Some("dark"), &dark);
    block(&mut body, "", "[data-theme=\"light\"]", Some("light"), &light);
    block(&mut body, "", ":root", None, &global);
    body.push_str("@media (prefers-reduced-motion: reduce) {\n");
    block(&mut body, "  ", ":root", None, &reduced);
    body.push_str("}\n");
    let digest = hex::encode(Sha256::digest(body.as_bytes()));
    Ok(format!(
        "/* Generated by `cargo xtask design-tokens` from {SOURCE}; do not edit.\n   sha256:{digest} */\n{body}"
    ))
}

fn block(out: &mut String, indent: &str, selector: &str, scheme: Option<&str>, decls: &Decls) {
    let _ = writeln!(out, "{indent}{selector} {{");
    if let Some(s) = scheme {
        let _ = writeln!(out, "{indent}  color-scheme: {s};");
    }
    for (n, v) in decls {
        let _ = writeln!(out, "{indent}  --{n}: {v};");
    }
    let _ = writeln!(out, "{indent}}}");
}

/// Targets under `root` whose bytes differ from `css` (or that are missing).
fn stale(root: &Path, css: &str) -> Vec<String> {
    TARGETS
        .iter()
        .filter(|t| fs::read(root.join(t)).ok().as_deref() != Some(css.as_bytes()))
        .map(|t| (*t).to_owned())
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn real() -> Json {
        load(&crate::workspace_root()).unwrap()
    }

    /// The real token file with one literal replaced.
    fn edited(from: &str, to: &str) -> Json {
        let path = crate::workspace_root().join(SOURCE);
        let text = fs::read_to_string(path).unwrap();
        assert!(text.contains(from), "fixture literal {from} not in {SOURCE}");
        Json::parse(&text.replacen(from, to, 1)).unwrap()
    }

    #[test]
    fn every_listed_pair_meets_wcag_aa_in_both_themes() {
        let measured = pair_ratios(&real()).unwrap();
        let pairs = real()
            .get("contrast-pairs")
            .and_then(|p| p.get("pairs"))
            .and_then(Json::as_array)
            .unwrap()
            .len();
        assert!(pairs >= 27, "the spec lists 27 pairs per theme, found {pairs}");
        assert_eq!(measured.len(), pairs * THEMES.len());
        for m in &measured {
            println!(
                "{:>5} {:>11} on {:<7} {:>5.2} (min {})",
                m.theme,
                m.fg,
                m.bg,
                m.ratio,
                m.usage.minimum()
            );
        }
        let failures = contrast_failures(&real()).unwrap();
        assert!(failures.is_empty(), "below AA: {failures:?}");
    }

    #[test]
    fn a_pair_below_aa_is_reported() {
        // Dark muted darkened to #3a4448: about 2:1 on the abyss.
        let failures = contrast_failures(&edited("#8a9ca3", "#3a4448")).unwrap();
        assert!(
            failures.iter().any(|f| f.starts_with("dark muted on bg")),
            "{failures:?}"
        );
        assert!(failures.iter().all(|f| f.starts_with("dark muted")), "{failures:?}");
    }

    #[test]
    fn a_pair_naming_a_non_hex_role_is_an_error() {
        let tokens = edited("[\"text\", \"bg\", \"text\"]", "[\"line\", \"bg\", \"text\"]");
        assert!(pair_ratios(&tokens).is_err());
    }

    #[test]
    fn the_generator_is_deterministic() {
        let a = render(&real()).unwrap();
        let b = render(&real()).unwrap();
        assert_eq!(a, b);
        let (header, body) = a.split_once("*/\n").unwrap();
        let digest = hex::encode(Sha256::digest(body.as_bytes()));
        assert!(header.contains(&format!("sha256:{digest}")), "{header}");
        assert!(header.contains("do not edit"), "{header}");
    }

    #[test]
    fn a_token_change_changes_the_output_and_its_hash() {
        let a = render(&real()).unwrap();
        let b = render(&edited("\"r-2\": \"12px\"", "\"r-2\": \"14px\"")).unwrap();
        assert_ne!(a.split_once("*/").unwrap().0, b.split_once("*/").unwrap().0);
        assert!(b.contains("--r-2: 14px;"));
    }

    #[test]
    fn the_stylesheet_has_both_themes_the_os_fallback_and_reduced_motion() {
        let css = render(&real()).unwrap();
        for needle in [
            ":root {\n  color-scheme: dark;",
            "@media (prefers-color-scheme: light) {\n  :root {\n    color-scheme: light;",
            "[data-theme=\"dark\"] {\n  color-scheme: dark;",
            "[data-theme=\"light\"] {\n  color-scheme: light;",
            "--bg: #050b0f;",
            "--bg: #f5f8f8;",
            "--type-display-1-size: 56px;",
            "--space-9: 96px;",
            "--r-pill: 999px;",
            "--chart-1: var(--accent);",
            "@media (prefers-reduced-motion: reduce) {\n  :root {\n    --dur-1: 1ms;",
        ] {
            assert!(css.contains(needle), "missing {needle:?}");
        }
        // Explicit choice beats the OS: the attribute blocks come after the
        // media query at equal specificity.
        let media = css.find("prefers-color-scheme").unwrap();
        assert!(css.find("[data-theme=\"dark\"]").unwrap() > media);
        assert!(!css.contains("$comment"));
    }

    #[test]
    fn themes_must_name_the_same_roles() {
        let tokens = edited("\"info\": \"#2f55b8\",", "");
        assert!(render(&tokens).unwrap_err().contains("info"));
    }

    #[test]
    fn reduced_motion_may_only_override_existing_tokens() {
        let tokens = edited("\"loop\": \"1\"", "\"loops\": \"1\"");
        assert!(render(&tokens).unwrap_err().contains("loops"));
    }

    #[test]
    fn a_value_that_would_break_out_of_the_declaration_is_refused() {
        let tokens = edited("\"r-1\": \"8px\"", "\"r-1\": \"8px; } body { x: y\"");
        assert!(render(&tokens).is_err());
    }

    #[test]
    fn check_reports_each_drifted_or_missing_copy() {
        let css = render(&real()).unwrap();
        let root = std::env::temp_dir().join(format!("design-tokens-{}", std::process::id()));
        for target in TARGETS {
            let path = root.join(target);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, &css).unwrap();
        }
        assert!(stale(&root, &css).is_empty());
        fs::write(root.join(TARGETS[3]), css.replace("#050b0f", "#050b10")).unwrap();
        fs::remove_file(root.join(TARGETS[1])).unwrap();
        let mut found = stale(&root, &css);
        found.sort();
        let mut want = vec![TARGETS[1].to_owned(), TARGETS[3].to_owned()];
        want.sort();
        assert_eq!(found, want);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_committed_copies_are_current() {
        let css = render(&real()).unwrap();
        let stale = stale(&crate::workspace_root(), &css);
        assert!(stale.is_empty(), "run cargo xtask design-tokens: {stale:?}");
    }
}
