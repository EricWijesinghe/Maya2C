//! `cargo xtask design-tokens [--check | --literals <dir>]` — generates
//! `tokens.css` from `design/tokens.json` (Forged Light 2, ADR-136) into every
//! surface.
//!
//! Five surfaces each wrote their own stylesheet from one palette and drifted
//! into five products (docs/design/audit-2026-11.md). One source, one
//! generator, byte-identical copies, and `--check` in CI is what keeps them in
//! step. Both modes refuse a token file whose listed pairs miss their minimum,
//! that leaves out a pair of the required matrix, whose chart series collapse
//! under colour-blind vision, or that the design doc's colour table no longer
//! matches, so a colour change cannot land without its measured evidence.
//! `--literals <dir>` is the other half: it finds the colours a surface still
//! writes by hand instead of naming a role.

mod doc;
mod literals;

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use maya_design_system::contrast::{Rgb, Use, Vision, distance, ratio};
use sha2::{Digest, Sha256};

use crate::ordered_json::Json;

/// The single source.
const SOURCE: &str = "design/tokens.json";

/// The design doc whose §3.1 table must equal the theme tokens.
const DOC: &str = "docs/design/forged-light-2.md";

/// Every generated copy. `design/` holds the specimen's copy
/// (`crates/design-system/src/specimen.rs` inlines it).
const TARGETS: &[&str] = &[
    "design/tokens.css",
    "docs/site/src/styles/tokens.css",
    "apps/explorer/assets/tokens.css",
    "apps/hub/ui/tokens.css",
    "apps/chat-gui/ui/tokens.css",
    "apps/wallet-gui/ui/tokens.css",
    "apps/status/src/tokens.css",
];

/// Theme names in `tokens.json`; the first is the default.
const THEMES: [&str; 2] = ["dark", "light"];

/// The four grounds a foreground can sit on.
const SURFACES: [&str; 4] = ["bg", "surface-1", "surface-2", "surface-3"];

/// Roles a component renders as body-size text on any ground. Each must be
/// listed as `text` (4.5:1) on all four, so none of them is a large-text-only
/// colour a component could misuse at 12 px.
const TEXT_ON_EVERY_SURFACE: &[&str] = &[
    "text",
    "text-2",
    "text-3",
    "accent-text",
    "bronze",
    "bronze-text",
    "steel",
    "success",
    "warn",
    "danger",
    "info",
];

/// Ink on a filled control: (ink, fill), each as `text`.
const ON_FILLS: &[(&str, &str)] = &[
    ("on-accent", "accent"),
    ("on-accent", "accent-hover"),
    ("on-accent", "accent-grad-from"),
    ("on-accent", "accent-grad-to"),
    ("on-forge", "forge-grad-from"),
    ("on-forge", "forge-grad-to"),
];

/// The minimum set of pairs the token file must list, as (fg, bg, use,
/// theme or `None` for both). Chart series on `bg` and `surface-1` are added
/// from `chart-distinctness.series`.
fn required(series: &[String]) -> Vec<(String, String, &'static str, Option<&'static str>)> {
    let mut out = Vec::new();
    let mut add = |fg: &str, bg: &str, usage: &'static str, theme: Option<&'static str>| {
        out.push((fg.to_owned(), bg.to_owned(), usage, theme));
    };
    for &fg in TEXT_ON_EVERY_SURFACE {
        for bg in SURFACES {
            add(fg, bg, "text", None);
        }
    }
    for bg in SURFACES {
        add("accent", bg, "non-text", None);
    }
    for &(ink, fill) in ON_FILLS {
        add(ink, fill, "text", None);
    }
    for bg in ["bg", "surface-1"] {
        add("border-control", bg, "non-text", None);
        for s in series {
            add(s.as_str(), bg, "non-text", None);
        }
    }
    add("surface-3", "bg", "separation", Some("dark"));
    out
}

pub fn run(args: &[String]) -> Result<(), String> {
    let root = crate::workspace_root();
    let check = match args {
        [] => false,
        [flag] if flag == "--check" => true,
        [flag, dir] if flag == "--literals" => {
            let dir = Path::new(dir);
            return literals::run(&if dir.is_absolute() {
                dir.to_path_buf()
            } else {
                root.join(dir)
            });
        }
        _ => return Err("usage: cargo xtask design-tokens [--check | --literals <dir>]".into()),
    };
    let tokens = load(&root)?;
    gate(&root, &tokens)?;
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

/// Prints every measurement and fails on any pair below its minimum, a
/// missing required pair, chart series too close, or a doc table mismatch.
fn gate(root: &Path, tokens: &Json) -> Result<(), String> {
    let measured = pair_ratios(tokens)?;
    for m in &measured {
        println!("{m}");
    }
    let chart = chart_worst(tokens)?;
    for c in &chart {
        println!("{c}");
    }
    let mut failures: Vec<String> = measured
        .iter()
        .filter(|m| m.ratio < m.usage.minimum())
        .map(|m| format!("below minimum: {m}"))
        .collect();
    failures.extend(
        missing_pairs(tokens)?
            .into_iter()
            .map(|p| format!("required pair not listed: {p}")),
    );
    let min = chart_min(tokens)?;
    failures.extend(
        chart
            .iter()
            .filter(|c| c.distance < min)
            .map(|c| format!("chart series too close (min {min}): {c}")),
    );
    let doc_path = root.join(DOC);
    let doc_text =
        fs::read_to_string(&doc_path).map_err(|e| format!("{}: {e}", doc_path.display()))?;
    failures.extend(
        doc::mismatches(&doc_text, tokens)?
            .into_iter()
            .map(|d| format!("{DOC} §3.1: {d}")),
    );
    if failures.is_empty() {
        return Ok(());
    }
    for f in &failures {
        eprintln!("{f}");
    }
    Err(format!("{SOURCE}: {} check(s) fail", failures.len()))
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

impl std::fmt::Display for Measured {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:>5} {:>16} on {:<16} {:>6.2} (min {}, {})",
            self.theme,
            self.fg,
            self.bg,
            self.ratio,
            self.usage.minimum(),
            use_name(self.usage)
        )
    }
}

const fn use_name(u: Use) -> &'static str {
    match u {
        Use::Text => "text",
        Use::LargeText => "large-text",
        Use::NonText => "non-text",
        Use::Separation => "separation",
    }
}

fn parse_use(s: &str) -> Result<Use, String> {
    match s {
        "text" => Ok(Use::Text),
        "large-text" => Ok(Use::LargeText),
        "non-text" => Ok(Use::NonText),
        "separation" => Ok(Use::Separation),
        other => Err(format!(
            "contrast pair use `{other}`: text, large-text, non-text or separation"
        )),
    }
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
    value
        .get(key)
        .ok_or_else(|| format!("{SOURCE}: no `{key}`"))
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
                let name = if prefix.is_empty() {
                    k.to_owned()
                } else {
                    format!("{prefix}-{k}")
                };
                flatten(&name, v, out)?;
            }
            Ok(())
        }
        _ => Err(format!("--{prefix}: token values are CSS strings")),
    }
}

fn check_name(name: &str) -> Result<(), String> {
    let ok = name.starts_with(|c: char| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    // Eric has not chosen a violet/quantum role (ADR-136, second revision).
    let reserved = name.contains("violet") || name.contains("quantum");
    if ok && !reserved {
        Ok(())
    } else if reserved {
        Err(format!(
            "`{name}`: violet/quantum roles are deferred to Eric"
        ))
    } else {
        Err(format!("`{name}`: token names are [a-z][a-z0-9-]*"))
    }
}

/// The value lands verbatim inside a declaration, so nothing that could end it,
/// open a block or close the header comment is accepted.
fn check_value(name: &str, v: &str) -> Result<(), String> {
    let bad = v.trim().is_empty()
        || v.contains(['{', '}', ';', '<', '\\', '\n', '\r'])
        || v.contains("*/");
    if bad {
        Err(format!("--{name}: value `{v}` is not a plain CSS value"))
    } else {
        Ok(())
    }
}

fn theme(tokens: &Json, name: &str) -> Result<Decls, String> {
    let mut out = Vec::new();
    flatten("", group(group(tokens, "theme")?, name)?, &mut out)?;
    Ok(out)
}

/// A theme role as `#rrggbb`.
fn colour(roles: &Decls, theme_name: &str, role: &str) -> Result<Rgb, String> {
    if role.starts_with("deco-") {
        return Err(format!(
            "`{role}` is decorative: it is never text or a measured edge"
        ));
    }
    let (_, v) = roles
        .iter()
        .find(|(n, _)| n == role)
        .ok_or_else(|| format!("unknown role `{role}`"))?;
    Rgb::hex(v).ok_or_else(|| format!("{theme_name} {role} = {v}: a measured role needs #rrggbb"))
}

/// A listed pair: foreground, background, use, and the one theme it applies
/// to (`None` for both).
type Pair<'a> = (&'a str, &'a str, &'a str, Option<&'a str>);

fn pairs(tokens: &Json) -> Result<Vec<Pair<'_>>, String> {
    group(tokens, "contrast-pairs")?
        .get("pairs")
        .and_then(Json::as_array)
        .ok_or("contrast-pairs.pairs: expected an array")?
        .iter()
        .map(pair)
        .collect()
}

fn pair(p: &Json) -> Result<Pair<'_>, String> {
    let err = || "each contrast pair is [foreground, background, use(, theme)]".to_owned();
    let strs: Option<Vec<&str>> = p
        .as_array()
        .ok_or_else(err)?
        .iter()
        .map(Json::as_str)
        .collect();
    match strs.ok_or_else(err)?.as_slice() {
        &[a, b, c] => Ok((a, b, c, None)),
        &[a, b, c, t] if THEMES.contains(&t) => Ok((a, b, c, Some(t))),
        _ => Err(err()),
    }
}

fn pair_ratios(tokens: &Json) -> Result<Vec<Measured>, String> {
    let listed = pairs(tokens)?;
    let mut out = Vec::new();
    for theme_name in THEMES {
        let roles = theme(tokens, theme_name)?;
        for &(fg, bg, usage, only) in &listed {
            let usage = parse_use(usage)?;
            if only.is_some_and(|t| t != theme_name) {
                continue;
            }
            out.push(Measured {
                theme: theme_name,
                fg: fg.to_owned(),
                bg: bg.to_owned(),
                usage,
                ratio: ratio(
                    colour(&roles, theme_name, fg)?,
                    colour(&roles, theme_name, bg)?,
                ),
            });
        }
    }
    Ok(out)
}

/// Required pairs the file does not list, exactly (a `text` requirement is not
/// met by a `large-text` entry).
fn missing_pairs(tokens: &Json) -> Result<Vec<String>, String> {
    let listed: BTreeSet<_> = pairs(tokens)?.into_iter().collect();
    let series = chart_series(tokens)?;
    Ok(required(&series)
        .into_iter()
        .filter(|(fg, bg, u, t)| !listed.contains(&(fg.as_str(), bg.as_str(), *u, *t)))
        .map(|(fg, bg, u, t)| match t {
            Some(t) => format!("[\"{fg}\", \"{bg}\", \"{u}\", \"{t}\"]"),
            None => format!("[\"{fg}\", \"{bg}\", \"{u}\"]"),
        })
        .collect())
}

fn chart_series(tokens: &Json) -> Result<Vec<String>, String> {
    group(tokens, "chart-distinctness")?
        .get("series")
        .and_then(Json::as_array)
        .ok_or("chart-distinctness.series: expected an array")?
        .iter()
        .map(|s| {
            s.as_str()
                .map(str::to_owned)
                .ok_or_else(|| "chart-distinctness.series: role names".to_owned())
        })
        .collect()
}

fn chart_min(tokens: &Json) -> Result<f64, String> {
    group(tokens, "chart-distinctness")?
        .get("min-distance")
        .and_then(Json::as_str)
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|m| m.is_finite() && *m > 0.0)
        .ok_or_else(|| "chart-distinctness.min-distance: a positive number as a string".into())
}

/// The closest two series in one theme under one vision model.
#[derive(Debug)]
struct ChartWorst {
    theme: &'static str,
    vision: Vision,
    a: String,
    b: String,
    distance: f64,
}

impl std::fmt::Display for ChartWorst {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:>5} chart {:<12} closest {} / {} OKLab {:.3}",
            self.theme,
            self.vision.name(),
            self.a,
            self.b,
            self.distance
        )
    }
}

fn chart_worst(tokens: &Json) -> Result<Vec<ChartWorst>, String> {
    let series = chart_series(tokens)?;
    if series.len() < 2 {
        return Err("chart-distinctness.series: at least two series".into());
    }
    let mut out = Vec::new();
    for theme_name in THEMES {
        let roles = theme(tokens, theme_name)?;
        let colours = series
            .iter()
            .map(|s| colour(&roles, theme_name, s))
            .collect::<Result<Vec<_>, _>>()?;
        for vision in Vision::ALL {
            let mut worst: Option<ChartWorst> = None;
            for i in 0..series.len() {
                for j in i + 1..series.len() {
                    let d = distance(colours[i], colours[j], vision);
                    if worst.as_ref().is_none_or(|w| d < w.distance) {
                        worst = Some(ChartWorst {
                            theme: theme_name,
                            vision,
                            a: series[i].clone(),
                            b: series[j].clone(),
                            distance: d,
                        });
                    }
                }
            }
            out.extend(worst);
        }
    }
    Ok(out)
}

fn render(tokens: &Json) -> Result<String, String> {
    let dark = theme(tokens, THEMES[0])?;
    let light = theme(tokens, THEMES[1])?;
    let names = |d: &Decls| d.iter().map(|(n, _)| n.clone()).collect::<BTreeSet<_>>();
    let (dn, ln) = (names(&dark), names(&light));
    if dn != ln {
        let diff: Vec<_> = dn.symmetric_difference(&ln).cloned().collect();
        return Err(format!(
            "dark and light name different roles: {}",
            diff.join(", ")
        ));
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
    if let Some((n, _)) = reduced
        .iter()
        .find(|(n, _)| !global.iter().any(|(g, _)| g == n))
    {
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
    block(
        &mut body,
        "",
        "[data-theme=\"light\"]",
        Some("light"),
        &light,
    );
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
mod tests;
