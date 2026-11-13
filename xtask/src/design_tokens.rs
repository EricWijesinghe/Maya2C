//! `cargo xtask design-tokens [--check | --literals <dir>]` — generates
//! `tokens.css` from `design/tokens.json` (Forged Light 2, ADR-136) into every
//! surface.
//!
//! Five surfaces each wrote their own stylesheet from one palette and drifted
//! into five products (docs/design/audit-2026-11.md). One source, one
//! generator, byte-identical copies, and `--check` in CI is what keeps them in
//! step. Both modes refuse a token file whose listed pairs miss their minimum,
//! that leaves out a pair of the required matrix, whose chart series collapse
//! under colour-blind vision, where two meanings (bronze value and amber
//! warning, say) share one colour, or that the design doc's colour table no longer
//! matches, so a colour change cannot land without its measured evidence.
//! `--literals <dir>` is the other half: it finds the colours a surface still
//! writes by hand instead of naming a role.

mod distinct;
mod doc;
mod families;
mod literals;

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

#[cfg(test)]
use maya_design_system::contrast::Vision;
use maya_design_system::contrast::{Rgb, Use, ratio};
use sha2::{Digest, Sha256};

use crate::ordered_json::Json;
use distinct::{CHART, chart_series, chart_worst, role_worst};
#[cfg(test)]
use distinct::{ROLES, min_distance};

/// The single source.
const SOURCE: &str = "design/tokens.json";

/// The design doc whose §3.1 table must equal the theme tokens.
const DOC: &str = "docs/design/forged-light-2.md";

/// Every generated copy, with the token-name prefixes it leaves out. `design/`
/// holds the specimen's full copy (`crates/design-system/src/specimen.rs`
/// inlines it).
const TARGETS: &[(&str, &[&str])] = &[
    ("design/tokens.css", &[]),
    ("docs/site/src/styles/tokens.css", &[]),
    // The explorer inlines its tokens into every page under a byte budget
    // (apps/explorer budget_tests) and draws no hero, forge button or token
    // chart yet; the lane that adds one removes its prefix here.
    ("apps/explorer/assets/tokens.css", EXPLORER_OMITS),
    ("apps/hub/ui/tokens.css", &[]),
    ("apps/chat-gui/ui/tokens.css", &[]),
    ("apps/wallet-gui/ui/tokens.css", &[]),
    ("apps/status/src/tokens.css", &[]),
];

const EXPLORER_OMITS: &[&str] = &[
    "deco-",
    "grad-hero",
    "grad-forge",
    "forge-",
    "on-forge",
    "chart-",
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
    ("on-fill", "bronze-fill"),
    ("on-fill", "success-fill"),
    ("on-fill", "warn-fill"),
    ("on-fill", "danger-fill"),
    ("on-fill", "info-fill"),
];

/// Grounds a chart may be drawn on: never the pressed or selected surface-3.
const CHART_SURFACES: [&str; 3] = ["bg", "surface-1", "surface-2"];

/// A selected row: the teal tint over a card, not a darker neutral.
const SELECTED: &str = "accent-subtle over surface-1";

/// The hero's brightest point, where its glow is strongest under the text.
const HERO: &str = "deco-teal over surface-1";

/// What the hero may set as body text; text-3 is never used on it.
const HERO_TEXT: [&str; 4] = ["text", "text-2", "accent-text", "bronze-text"];

/// Meanings shown as a chip: role ink and edge on `<role>-subtle`.
const CHIPS: [&str; 5] = ["bronze", "success", "warn", "danger", "info"];

/// Solid colours drawn as a dot, an alert edge or a button face: 3:1 on the
/// ground and on a card.
const SOLIDS: [&str; 7] = [
    "bronze-fill",
    "success-fill",
    "warn-fill",
    "danger-fill",
    "info-fill",
    "forge-grad-from",
    "forge-grad-to",
];

type Required = (String, String, &'static str, Option<&'static str>);

/// The minimum set of pairs the token file must list, as (fg, bg, use,
/// theme or `None` for both). Chart series on [`CHART_SURFACES`] are added
/// from `chart-distinctness.series`.
fn required(series: &[String]) -> Vec<Required> {
    let mut out = Vec::new();
    let mut add = |fg: &str, bg: &str, usage: &'static str, theme: Option<&'static str>| {
        out.push((fg.to_owned(), bg.to_owned(), usage, theme));
    };
    for bg in SURFACES.into_iter().chain([SELECTED]) {
        for &fg in TEXT_ON_EVERY_SURFACE {
            add(fg, bg, "text", None);
        }
        add("bronze-display", bg, "large-text", None);
        add("accent", bg, "non-text", None);
        // A checkbox inside a selected row (SC 1.4.11).
        add("border-control", bg, "non-text", None);
    }
    for fg in HERO_TEXT {
        add(fg, HERO, "text", None);
    }
    add("bronze-display", HERO, "large-text", None);
    for role in CHIPS {
        for bg in CHART_SURFACES {
            add(role, &format!("{role}-subtle over {bg}"), "text", None);
        }
    }
    for &(ink, fill) in ON_FILLS {
        add(ink, fill, "text", None);
    }
    for solid in SOLIDS {
        for bg in ["bg", "surface-1"] {
            add(solid, bg, "non-text", None);
        }
    }
    for bg in CHART_SURFACES {
        for s in series {
            add(s.as_str(), bg, "non-text", None);
        }
    }
    add("surface-3", "bg", "separation", Some("dark"));
    add("surface-1", "bg", "separation", Some("dark"));
    add("surface-3", "surface-1", "separation", Some("light"));
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
    let copies = copies(&tokens)?;
    if check {
        let stale = stale(&root, &copies);
        if !stale.is_empty() {
            for path in &stale {
                eprintln!("stale or missing: {path}");
            }
            return Err("run: cargo xtask design-tokens".into());
        }
        println!("tokens.css current in {} surfaces", TARGETS.len());
        return Ok(());
    }
    for (target, css) in &copies {
        let path = root.join(target);
        fs::write(&path, css).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    println!("wrote tokens.css to {} surfaces", TARGETS.len());
    Ok(())
}

/// Prints every measurement and fails on any pair below its minimum, a
/// missing required pair, chart series or meaning roles too close, or a doc
/// table mismatch.
fn gate(root: &Path, tokens: &Json) -> Result<(), String> {
    let measured = pair_ratios(tokens)?;
    for m in &measured {
        println!("{m}");
    }
    let chart = chart_worst(tokens)?;
    let roles = role_worst(tokens)?;
    for c in chart.iter().chain(&roles) {
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
    failures.extend(
        chart
            .iter()
            .chain(&roles)
            .filter(|c| c.distance < c.min)
            .map(|c| format!("{} too close (min {}): {c}", c.group, c.min)),
    );
    for line in families::report(tokens)? {
        println!("{}", line.text);
        if let Some(f) = line.failure {
            failures.push(f);
        }
    }
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
/// open a block or close the header comment is accepted, and nothing that
/// makes a page fetch: every surface is self-hosted, and a token is not where
/// a request belongs.
fn check_value(name: &str, v: &str) -> Result<(), String> {
    let lower = v.to_ascii_lowercase();
    let bad = v.trim().is_empty()
        || v.contains(['{', '}', ';', '<', '\\', '\n', '\r'])
        || v.contains("*/")
        || lower.contains("url(")
        || lower.contains("@import")
        || lower.contains("image-set(");
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

/// A theme role's raw value.
fn value<'a>(roles: &'a Decls, role: &str) -> Result<&'a str, String> {
    roles
        .iter()
        .find(|(n, _)| n == role)
        .map(|(_, v)| v.as_str())
        .ok_or_else(|| format!("unknown role `{role}`"))
}

/// A theme role as `#rrggbb`, following `var(--x)` to x (a ramp step named
/// by reference is measured as the step).
fn colour(roles: &Decls, theme_name: &str, role: &str) -> Result<Rgb, String> {
    if role.starts_with("deco-") {
        return Err(format!(
            "`{role}` is decorative: it is never text or a measured edge"
        ));
    }
    let mut name = role;
    for _ in 0..4 {
        let v = value(roles, name)?;
        match v.strip_prefix("var(--").and_then(|r| r.strip_suffix(')')) {
            Some(next) => name = next,
            None => {
                return Rgb::hex(v).ok_or_else(|| {
                    format!("{theme_name} {role} = {v}: a measured role needs #rrggbb")
                });
            }
        }
    }
    Err(format!("{theme_name} {role}: var() chain deeper than four"))
}

/// `rgb(r g b / a)` as the colour and its alpha.
fn rgba(v: &str) -> Option<(Rgb, f64)> {
    let inner = v.strip_prefix("rgb(")?.strip_suffix(')')?;
    let (rgb, alpha) = inner.split_once('/')?;
    let channels: Vec<u8> = rgb
        .split_whitespace()
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    let alpha: f64 = alpha.trim().parse().ok()?;
    match channels.as_slice() {
        &[red, green, blue] if (0.0..=1.0).contains(&alpha) => Some((Rgb(red, green, blue), alpha)),
        _ => None,
    }
}

/// A measured ground: a role, or `a over b`, the translucent role a
/// composited over the opaque ground b in 8-bit sRGB, as a browser paints it.
fn ground(roles: &Decls, theme_name: &str, name: &str) -> Result<Rgb, String> {
    let Some((top, under)) = name.split_once(" over ") else {
        return colour(roles, theme_name, name);
    };
    let raw = value(roles, top)?;
    let (over, alpha) = rgba(raw)
        .ok_or_else(|| format!("{theme_name} {top} = {raw}: an overlay needs rgb(r g b / a)"))?;
    let base = colour(roles, theme_name, under)?;
    // In range: a weighted mean of two bytes.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let mix = |top: u8, low: u8| {
        alpha
            .mul_add(f64::from(top), (1.0 - alpha) * f64::from(low))
            .round() as u8
    };
    Ok(Rgb(
        mix(over.0, base.0),
        mix(over.1, base.1),
        mix(over.2, base.2),
    ))
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
    match *strs.ok_or_else(err)?.as_slice() {
        [fg, bg, usage] => Ok((fg, bg, usage, None)),
        [fg, bg, usage, only] if THEMES.contains(&only) => Ok((fg, bg, usage, Some(only))),
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
                    ground(&roles, theme_name, bg)?,
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

/// The full stylesheet, as the specimen and most surfaces take it.
#[cfg(test)]
fn render(tokens: &Json) -> Result<String, String> {
    render_omitting(tokens, &[])
}

/// Each target's stylesheet: the full one less the target's omitted prefixes.
fn copies(tokens: &Json) -> Result<Vec<(&'static str, String)>, String> {
    TARGETS
        .iter()
        .map(|&(t, omit)| Ok((t, render_omitting(tokens, omit)?)))
        .collect()
}

fn render_omitting(tokens: &Json, omit: &[&str]) -> Result<String, String> {
    let keep = |d: Decls| -> Decls {
        d.into_iter()
            .filter(|(n, _)| !omit.iter().any(|p| n.starts_with(p)))
            .collect()
    };
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

    let (dark, light, global) = (keep(dark), keep(light), keep(global));
    for (n, v) in dark.iter().chain(&light).chain(&global) {
        for (_, rest) in v.match_indices("var(--").map(|(i, _)| v.split_at(i + 6)) {
            let used = rest.split(')').next().unwrap_or_default();
            if omit.iter().any(|p| used.starts_with(p)) {
                return Err(format!("--{n} uses --{used}, which this copy omits"));
            }
        }
    }

    // The dark block serves :root and an explicit dark theme at once; the OS
    // light block skips a root that chose dark, and the explicit light block
    // comes last. Every selector has one class's specificity (:where() adds
    // none), so a surface's own sheet, loaded after this one, still wins on a
    // shared name, and an explicit choice beats the OS.
    let mut body = String::new();
    block(
        &mut body,
        "",
        ":root, [data-theme=\"dark\"]",
        Some("dark"),
        &dark,
    );
    body.push_str("@media (prefers-color-scheme: light) {\n");
    block(
        &mut body,
        "  ",
        ":root:where(:not([data-theme=\"dark\"]))",
        Some("light"),
        &light,
    );
    body.push_str("}\n");
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
    let subset = if omit.is_empty() {
        String::new()
    } else {
        format!("\n   Omits: {}", omit.join(" "))
    };
    Ok(format!(
        "/* Generated by `cargo xtask design-tokens` from {SOURCE}; do not edit.{subset}\n   sha256:{digest} */\n{body}"
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

/// Targets under `root` whose bytes differ from their copy (or that are
/// missing).
fn stale(root: &Path, copies: &[(&str, String)]) -> Vec<String> {
    copies
        .iter()
        .filter(|(t, css)| fs::read(root.join(t)).ok().as_deref() != Some(css.as_bytes()))
        .map(|(t, _)| (*t).to_owned())
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests;
