//! `cargo xtask float-check` — invariant 36, mechanised.
//!
//! No float and no model output in a consensus rule. A float is a rounding
//! mode, and two validators on different hardware, compilers or flags can
//! round the same expression differently; one ULP of disagreement in a state
//! root is a fork. The rule used to live under invariant 20, which retired with
//! `crates/zkml` (ADR-008) and left it as prose nobody checked.
//!
//! This lexes every non-test file under `src/` of each crate the node binary
//! links (scope: [`crate::float_scope`]) and fails on a float token outside an
//! [`ALLOW`] entry, macro bodies included (a `syn` visitor would skip those):
//! a float primitive or SIMD lane type (`f16`..`f128`, `bf16`, `f32x4`), raw
//! (`r#f64`) or as a `_` segment (`as_f64`), a float trait or wrapper
//! ([`FLOAT_NAMES`]), or a float literal. Comments and strings never reach the
//! token stream. Items under `#[cfg(test)]` or `#[test]` are skipped, and so
//! is a file whose every `mod x;` declaration (macro bodies included) is
//! under `#[cfg(test)]`. A `use` that binds the name `test` or globs in an
//! outside crate (either could shadow `#[test]`) is itself a violation, and
//! so is a `#[path]` attribute outside a test item: it loads a file other
//! than the one this check maps the module to, possibly outside `src/`. It
//! also fails if an ML runtime ([`ML_CRATES`], [`ML_FAMILIES`], a denylist
//! that is not exhaustive) is anywhere in the node's dependency closure, or
//! if a scanned or excluded non-test file outside fee-market names that
//! crate's neural rule ([`MODEL_ENTRY`]).
//!
//! Not caught: a float with no float token in the scanned file, reached by
//! type inference (`"0.5".parse()` into an inferred type) or through a
//! registry crate (a `pub type Scalar = f64` alias, a function returning
//! `f64`); code outside `src/` that no `#[path]` names (a `build.rs`, a
//! `[lib] path` elsewhere, an `include!` of an unscanned file); and a
//! `#[macro_use] extern crate` exporting a `test` macro. None of these occurs
//! in linked code today. `clippy::float_arithmetic` uses type information and
//! closes the first two; it is not configured yet (BACKLOG.md).

use std::path::{Path, PathBuf};

use proc_macro2::{Delimiter, Spacing, TokenStream, TokenTree};

use crate::float_scope::{self, ALLOW, EXCLUDE, MUST_SCAN, under};

/// Crates that run a model. `maya-neural-fee` and `maya-zkml-prover` do not
/// exist yet: they are reserved for the neural fee gain moved out of the node
/// (PR #542) and a future zkML prover, so they fail the day they are linked.
/// A denylist is never exhaustive: a runtime missing here passes, which is
/// why the float scan, not this list, is the main line.
const ML_CRATES: &[&str] = &[
    "ort",
    "onnxruntime",
    "burn",
    "tch",
    "torch-sys",
    "tensorflow",
    "rten",
    "wonnx",
    "linfa",
    "smartcore",
    "dfdx",
    "mistralrs",
    "kalosm",
    "luminal",
    "maya-neural-fee",
    "maya-zkml-prover",
];

/// Families matched by prefix: a sub-crate (`burn-ndarray`, `ort-sys`) runs a
/// model without its facade, which it does not depend on.
const ML_FAMILIES: &[&str] = &[
    "tract-",
    "candle-",
    "burn-",
    "ort-",
    "onnxruntime-",
    "tensorflow-",
    "rten-",
    "wonnx-",
    "linfa-",
    "dfdx-",
    "llama-cpp-",
    "llama_cpp",
    "mistralrs-",
    "kalosm-",
    "luminal_",
];

fn is_ml_runtime(name: &str) -> bool {
    ML_CRATES.contains(&name) || ML_FAMILIES.iter().any(|p| name.starts_with(p))
}

/// fee-market's ways into its neural rule (`is_neural_active` gates it inside
/// `resolve_base_fee`, which `apply_block_fees*` call). The node computes its
/// base fee with the linear `next_base_fee` only, so its neural model is
/// dormant because nothing outside fee-market names any of these.
const MODEL_ENTRY: &[&str] = &[
    "apply_block_fees",
    "apply_block_fees_with_features",
    "resolve_base_fee",
    "is_neural_active",
    "next_base_fee_by_rule",
    "FeeRule",
    "MODEL_V1",
];

/// The crate that defines [`MODEL_ENTRY`].
const MODEL_HOME: &str = "crates/fee-market";

/// Leading words of an item whose `,` does not end it (`fn f<A, B>()`, a
/// `where` list). Anything else under a test attribute is a field, parameter,
/// variant or match arm, and ends at its top-level `,`.
const ITEM_KEYWORDS: &[&str] = &[
    "fn",
    "impl",
    "mod",
    "struct",
    "enum",
    "union",
    "trait",
    "use",
    "const",
    "static",
    "type",
    "unsafe",
    "async",
    "extern",
    "macro_rules",
    "let",
];

/// What one source file holds outside test items.
#[derive(Default)]
pub struct FileScan {
    pub hits: Vec<String>,
    /// `x` for each top-level `#[cfg(test)] mod x;`: files that are tests.
    pub test_mods: Vec<String>,
    /// [`MODEL_ENTRY`] names the file mentions.
    pub model_refs: Vec<String>,
}

/// Every float token in `src` outside test items, and its test modules.
pub fn scan_file(src: &str) -> Result<FileScan, String> {
    let stream: TokenStream = src.parse().map_err(|e| format!("does not lex: {e:?}"))?;
    let tokens: Vec<TokenTree> = stream.into_iter().collect();
    let mut out = FileScan {
        test_mods: test_only_mods(&tokens),
        ..FileScan::default()
    };
    scan(&tokens, &mut out);
    Ok(out)
}

/// `x` for each top-level `mod x;` whose every declaration in the file is
/// under `#[cfg(test)]`: a `#[cfg(not(test))] mod x;` beside it ships `x`.
fn test_only_mods(tokens: &[TokenTree]) -> Vec<String> {
    let test: Vec<String> = (0..tokens.len())
        .filter(|&i| is_test_attr(tokens, i))
        .filter_map(|i| out_of_line_mod(tokens, i + 2))
        .collect();
    let mut all = Vec::new();
    all_mod_decls(tokens, &mut all);
    let count = |v: &[String], m: &str| v.iter().filter(|x| *x == m).count();
    test.iter()
        .filter(|m| count(&test, m) == count(&all, m))
        .cloned()
        .collect()
}

/// Every `mod x;` in `tokens`, nested groups included: a macro body
/// (`m! { mod x; }`) can compile one in beside a `#[cfg(test)] mod x;`.
fn all_mod_decls(tokens: &[TokenTree], out: &mut Vec<String>) {
    for (i, t) in tokens.iter().enumerate() {
        if let TokenTree::Group(g) = t {
            all_mod_decls(&g.stream().into_iter().collect::<Vec<_>>(), out);
        }
        out.extend(mod_decl(tokens, i));
    }
}

/// `#[path = ..]`, `#![path = ..]`, or a `cfg_attr` that sets `path`, at `i`.
/// The file it loads is not the one [`is_test_file`] maps the module to and
/// may lie outside `src/`, so outside a test item it fails closed.
fn is_path_attr(tokens: &[TokenTree], i: usize) -> bool {
    if !is_punct(tokens.get(i), '#') {
        return false;
    }
    let g = match (tokens.get(i + 1), tokens.get(i + 2)) {
        (Some(TokenTree::Group(g)), _) => g,
        (Some(t), Some(TokenTree::Group(g))) if is_punct(Some(t), '!') => g,
        _ => return false,
    };
    if g.delimiter() != Delimiter::Bracket {
        return false;
    }
    let body: Vec<TokenTree> = g.stream().into_iter().collect();
    match ident_at(&body, 0).as_deref() {
        Some("path") => is_punct(body.get(1), '='),
        Some("cfg_attr") => {
            let mut flat = Vec::new();
            flatten(&body[1..], &mut flat);
            (0..flat.len()).any(|k| {
                ident_at(&flat, k).as_deref() == Some("path") && is_punct(flat.get(k + 1), '=')
            })
        }
        _ => false,
    }
}

fn scan(tokens: &[TokenTree], out: &mut FileScan) {
    let mut i = 0;
    while i < tokens.len() {
        if is_test_attr(tokens, i) {
            i = skip_item(tokens, i + 2);
            continue;
        }
        if let Some(why) = use_shadow(tokens, i) {
            out.hits.push(why.to_owned());
        }
        if is_path_attr(tokens, i) {
            out.hits.push("`#[path]` attribute".to_owned());
        }
        match &tokens[i] {
            TokenTree::Group(g) => scan(&g.stream().into_iter().collect::<Vec<_>>(), out),
            TokenTree::Ident(id) if names_a_float(&id.to_string()) => {
                out.hits.push(id.to_string());
            }
            TokenTree::Ident(id) if MODEL_ENTRY.contains(&id.to_string().as_str()) => {
                out.model_refs.push(id.to_string());
            }
            TokenTree::Literal(lit) if !is_tuple_index(tokens, i) => {
                let text = lit.to_string();
                if is_float_literal(&text) {
                    out.hits.push(text);
                }
            }
            _ => {}
        }
        i += 1;
    }
}

/// Float traits and wrappers (`num_traits`, `ordered-float`).
const FLOAT_NAMES: &[&str] = &["Float", "FloatCore", "Real", "OrderedFloat", "NotNan"];

/// Bit widths of the float primitives, `f16` and `f128` included.
const FLOAT_WIDTHS: &[&str] = &["16", "32", "64", "128"];

/// `f64`, `r#f64`, `as_f64`, `half::bf16`, `f32x4`, or a [`FLOAT_NAMES`] entry.
fn names_a_float(id: &str) -> bool {
    let id = id.strip_prefix("r#").unwrap_or(id);
    FLOAT_NAMES.contains(&id) || id.split('_').any(is_float_segment)
}

/// `f16`..`f128`, `bf16`, or a SIMD lane type such as `f32x4`.
fn is_float_segment(s: &str) -> bool {
    if s == "bf16" {
        return true;
    }
    let Some(rest) = s.strip_prefix('f') else {
        return false;
    };
    let width = match rest.split_once('x') {
        Some((w, lanes)) if !lanes.is_empty() && lanes.bytes().all(|b| b.is_ascii_digit()) => w,
        Some(_) => return false,
        None => rest,
    };
    FLOAT_WIDTHS.contains(&width)
}

/// Paths a glob may import from: none of them can bring in an attribute
/// macro named `test` without a `use` this check also reads.
const GLOB_ROOTS: &[&str] = &["std", "core", "alloc", "crate", "super", "self"];

/// Why the `use` item at `i` could shadow the built-in `#[test]` this scanner
/// trusts to mark test-only items: it binds the name `test` (`use m::test;`,
/// `use m::{a, test};`, `use m::x as test;`), or globs in an outside crate
/// (`use m::*;`), which may export an attribute macro of that name.
fn use_shadow(tokens: &[TokenTree], i: usize) -> Option<&'static str> {
    if ident_at(tokens, i).as_deref() != Some("use") {
        return None;
    }
    let end = tokens[i..]
        .iter()
        .position(|t| is_punct(Some(t), ';'))
        .map_or(tokens.len(), |p| i + p);
    let mut flat = Vec::new();
    flatten(&tokens[i + 1..end], &mut flat);
    let binds = (0..flat.len()).any(|k| {
        ident_at(&flat, k).as_deref() == Some("test")
            && !is_punct(flat.get(k + 1), ':')
            && ident_at(&flat, k + 1).as_deref() != Some("as")
    });
    if binds {
        return Some("use binds `test`");
    }
    // The root is the first token after a leading `::` or a macro's `$`
    // (`$crate`); a `{..}` group root is reported (fails closed).
    let root = tokens[i + 1..end]
        .iter()
        .find(|t| !is_punct(Some(t), ':') && !is_punct(Some(t), '$'));
    let root_ok =
        matches!(root, Some(TokenTree::Ident(id)) if GLOB_ROOTS.contains(&id.to_string().as_str()));
    let glob = flat.iter().any(|t| is_punct(Some(t), '*'));
    (glob && !root_ok).then_some("glob `use` of an outside crate")
}

fn flatten(tokens: &[TokenTree], out: &mut Vec<TokenTree>) {
    for t in tokens {
        match t {
            TokenTree::Group(g) => flatten(&g.stream().into_iter().collect::<Vec<_>>(), out),
            other => out.push(other.clone()),
        }
    }
}

fn punct(t: Option<&TokenTree>) -> Option<&proc_macro2::Punct> {
    match t {
        Some(TokenTree::Punct(p)) => Some(p),
        _ => None,
    }
}

fn is_punct(t: Option<&TokenTree>, c: char) -> bool {
    punct(t).is_some_and(|p| p.as_char() == c)
}

/// `#[test]` or `#[cfg(test)]` starting at `i`.
fn is_test_attr(tokens: &[TokenTree], i: usize) -> bool {
    if !is_punct(tokens.get(i), '#') {
        return false;
    }
    let Some(TokenTree::Group(g)) = tokens.get(i + 1) else {
        return false;
    };
    let body: String = g.stream().to_string().split_whitespace().collect();
    g.delimiter() == Delimiter::Bracket && (body == "test" || body == "cfg(test)")
}

/// Index of the first token after further `#[..]` attributes and a `pub`/`pub(..)`.
fn after_attrs(tokens: &[TokenTree], mut j: usize) -> usize {
    loop {
        match tokens.get(j) {
            Some(TokenTree::Punct(p)) if p.as_char() == '#' => j += 2,
            Some(TokenTree::Ident(id)) if id == "pub" => {
                j += 1;
                if matches!(tokens.get(j), Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Parenthesis)
                {
                    j += 1;
                }
            }
            _ => return j,
        }
    }
}

fn ident_at(tokens: &[TokenTree], j: usize) -> Option<String> {
    match tokens.get(j) {
        Some(TokenTree::Ident(id)) => Some(id.to_string()),
        _ => None,
    }
}

/// `x` when the item at `j`, after its attributes, is `mod x;` with no
/// `#[path]`: such a module loads some other file, so `x.rs` is not a test.
fn out_of_line_mod(tokens: &[TokenTree], j: usize) -> Option<String> {
    let k = after_attrs(tokens, j);
    if (j..k).any(|a| is_path_attr(tokens, a)) {
        return None;
    }
    mod_decl(tokens, k)
}

/// `x` when the tokens at `k` are `mod x;`.
fn mod_decl(tokens: &[TokenTree], k: usize) -> Option<String> {
    if ident_at(tokens, k)? == "mod" && is_punct(tokens.get(k + 2), ';') {
        ident_at(tokens, k + 1)
    } else {
        None
    }
}

/// `p` is joined to a following `c` (`<=`, `>=`, `=>`).
fn joined(p: &proc_macro2::Punct, next: Option<&TokenTree>, c: char) -> bool {
    p.spacing() == Spacing::Joint && is_punct(next, c)
}

/// Index after the test-only item whose tokens start at `start`: its brace
/// body, its `;`, or, for a field, parameter, variant or match arm, its
/// top-level `,` (not one inside `<..>` generics).
fn skip_item(tokens: &[TokenTree], start: usize) -> usize {
    let commas_end = ident_at(tokens, after_attrs(tokens, start))
        .is_none_or(|w| !ITEM_KEYWORDS.contains(&w.as_str()));
    let mut angle = 0usize;
    // A `,` passed inside `<`: a generic list, or the `<` was a comparison.
    let mut first_comma = None;
    let mut j = start;
    while let Some(t) = tokens.get(j) {
        j += 1;
        let p = match t {
            TokenTree::Group(g) if g.delimiter() == Delimiter::Brace => return j,
            TokenTree::Punct(p) => p,
            _ => continue,
        };
        let next = tokens.get(j);
        let after_arrow = j >= 2
            && punct(tokens.get(j - 2))
                .is_some_and(|q| q.spacing() == Spacing::Joint && matches!(q.as_char(), '-' | '='));
        match p.as_char() {
            ';' => return first_comma.unwrap_or(j),
            // Fail closed: if a `,` was passed inside `<`, that `<` may have
            // been a comparison, so scanning resumes after the first `,`.
            ',' if commas_end && angle == 0 => return first_comma.unwrap_or(j),
            ',' if commas_end => {
                first_comma.get_or_insert(j);
            }
            '<' if !joined(p, next, '=') => angle += 1,
            '>' if !after_arrow && !joined(p, next, '=') => angle = angle.saturating_sub(1),
            // A match arm cannot span `=>`: the open `<` was a comparison.
            '=' if joined(p, next, '>') && angle > 0 => {
                if let Some(c) = first_comma {
                    return c;
                }
            }
            _ => {}
        }
    }
    // `<` never closed: it was a comparison, so the item ended at the comma.
    first_comma.unwrap_or(j)
}

/// `x.0.1` lexes `0.1` as a float literal; it is two tuple indices. `..1.5`
/// (a range) is not excluded: its `.` follows another `.`.
fn is_tuple_index(tokens: &[TokenTree], i: usize) -> bool {
    i > 0 && is_punct(tokens.get(i - 1), '.') && (i < 2 || !is_punct(tokens.get(i - 2), '.'))
}

fn is_float_literal(text: &str) -> bool {
    let radix_prefixed = ["0x", "0o", "0b"].iter().any(|p| text.starts_with(p));
    if !text.starts_with(|c: char| c.is_ascii_digit()) || radix_prefixed {
        return false;
    }
    if FLOAT_WIDTHS
        .iter()
        .any(|w| text.ends_with(&format!("f{w}")))
    {
        return true;
    }
    // An integer suffix starts with `i` or `u`; no float body contains either.
    let body = text.split(['i', 'u']).next().unwrap_or(text);
    body.contains(['.', 'e', 'E'])
}

fn rust_files(path: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    if path.is_file() {
        out.push(path.to_path_buf());
        return Ok(());
    }
    let entries = std::fs::read_dir(path).map_err(|e| format!("{}: {e}", path.display()))?;
    for entry in entries {
        let p = entry
            .map_err(|e| format!("{}: {e}", path.display()))?
            .path();
        if p.is_dir() {
            rust_files(&p, out)?;
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
    Ok(())
}

/// `path` is `dir/m.rs` or under `dir/m/` for a `#[cfg(test)] mod m;` in a
/// file of `decls`.
fn is_test_file(path: &str, decls: &[(String, Vec<String>)]) -> bool {
    decls.iter().any(|(file, mods)| {
        let base = match file.rsplit_once('/') {
            Some((dir, "mod.rs")) => dir,
            Some((dir, "lib.rs" | "main.rs")) if is_crate_root_dir(dir) => dir,
            _ => file.trim_end_matches(".rs"),
        };
        mods.iter().any(|m| {
            let p = format!("{base}/{m}");
            path == format!("{p}.rs") || under(path, &p)
        })
    })
}

/// `src` or `src/bin/<name>`: where a `lib.rs`/`main.rs` is a crate root.
fn is_crate_root_dir(dir: &str) -> bool {
    let parent = dir.rsplit_once('/').map_or("", |(p, _)| p);
    dir == "src" || dir.ends_with("/src") || parent == "src/bin" || parent.ends_with("/src/bin")
}

/// [`MUST_SCAN`] entries no scanned file falls under.
fn unscanned<'a>(must: &[&'a str], scanned: &[String]) -> Vec<&'a str> {
    must.iter()
        .copied()
        .filter(|m| !scanned.iter().any(|f| under(f, m)))
        .collect()
}

fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/")
}

/// `(file, its scan)`, one per lexed source file.
type Scans = Vec<(String, FileScan)>;

/// What `float-check` enforces, as data so a fixture tree can bring its own.
struct Rules<'a> {
    allow: &'a [(&'a str, &'a [&'a str], &'a str)],
    exclude: &'a [(&'a str, &'a str)],
    must_scan: &'a [&'a str],
}

pub fn run(_args: &[String]) -> Result<(), String> {
    let rules = Rules {
        allow: ALLOW,
        exclude: EXCLUDE,
        must_scan: MUST_SCAN,
    };
    let (summary, failures) = check(&crate::workspace_root(), &rules)?;
    println!("{summary}");
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "invariant 36: float or model in consensus code\n{}",
            failures.join("\n")
        ))
    }
}

/// The summary line and every violation in the tree at `root`.
fn check(root: &Path, rules: &Rules) -> Result<(String, Vec<String>), String> {
    let linked = float_scope::linked(root)?;
    let mut paths = Vec::new();
    for d in &linked.dirs {
        rust_files(&root.join(d).join("src"), &mut paths)?;
    }
    let mut scans = Scans::new();
    for p in &paths {
        let f = rel(root, p);
        let src = std::fs::read_to_string(p).map_err(|e| format!("{f}: {e}"))?;
        let scan = scan_file(&src).map_err(|e| format!("{f}: {e}"))?;
        scans.push((f, scan));
    }
    scans.sort_by(|a, b| a.0.cmp(&b.0));
    let decls: Vec<(String, Vec<String>)> = scans
        .iter()
        .filter(|(_, s)| !s.test_mods.is_empty())
        .map(|(f, s)| (f.clone(), s.test_mods.clone()))
        .collect();
    let lexed = scans.len();
    scans.retain(|(f, _)| !is_test_file(f, &decls));
    let tests = lexed - scans.len();
    let (excluded, scanned): (Scans, Scans) = scans
        .into_iter()
        .partition(|(f, _)| rules.exclude.iter().any(|(e, _)| under(f, e)));
    let mut failures = token_failures(&scanned, rules.allow);
    // An exclusion waives floats only: the model check reads excluded files too.
    failures.extend(model_failures(&scanned));
    failures.extend(model_failures(&excluded));
    failures.extend(stale_entries(&scanned, &excluded, rules));
    let names: Vec<String> = scanned.iter().map(|(f, _)| f.clone()).collect();
    for m in unscanned(rules.must_scan, &names) {
        failures.push(format!("{m}: consensus code no longer scanned"));
    }
    for dep in linked.names.iter().filter(|n| is_ml_runtime(n)) {
        failures.push(format!(
            "{dep}: ML runtime in the node's dependency closure"
        ));
    }
    let summary = format!(
        "float-check: {} files in {} linked crates scanned ({tests} test modules, {} excluded), {} packages checked for ML runtimes, {} allowlisted, {} violations",
        scanned.len(),
        linked.dirs.len(),
        excluded.len(),
        linked.names.len(),
        rules.allow.len(),
        failures.len()
    );
    Ok((summary, failures))
}

/// Floats outside the allowlist.
fn token_failures(scanned: &Scans, allow: &[(&str, &[&str], &str)]) -> Vec<String> {
    let mut out = Vec::new();
    for (f, s) in scanned {
        let allowed = allow
            .iter()
            .find(|(p, _, _)| p == f)
            .map_or(&[][..], |(_, toks, _)| *toks);
        if s.hits != allowed {
            out.push(format!(
                "{f}: {} (allowlist expects [{}])",
                s.hits.join(", "),
                allowed.join(", ")
            ));
        }
    }
    out
}

/// Files outside fee-market that name its neural rule.
fn model_failures(files: &Scans) -> Vec<String> {
    files
        .iter()
        .filter(|(f, s)| !s.model_refs.is_empty() && !under(f, MODEL_HOME))
        .map(|(f, s)| {
            format!(
                "{f}: {} (fee-market's neural rule, reached from outside it)",
                s.model_refs.join(", ")
            )
        })
        .collect()
}

/// Allowlist entries for no scanned file, and exclusions that cover no linked
/// file or no float: an exclusion is only justified while it hides a float.
fn stale_entries(scanned: &Scans, excluded: &Scans, rules: &Rules) -> Vec<String> {
    let allow = rules
        .allow
        .iter()
        .filter(|(p, _, _)| !scanned.iter().any(|(f, _)| f == p))
        .map(|(p, _, _)| format!("{p}: allowlisted but not scanned (stale entry)"));
    let exclude = rules.exclude.iter().filter_map(|(e, _)| {
        let mut files = excluded.iter().filter(|(f, _)| under(f, e)).peekable();
        if files.peek().is_none() {
            Some(format!(
                "{e}: excluded but not linked by the node (stale entry)"
            ))
        } else if files.all(|(_, s)| s.hits.is_empty()) {
            Some(format!("{e}: excluded but holds no float (stale entry)"))
        } else {
            None
        }
    });
    allow.chain(exclude).collect()
}

#[cfg(test)]
mod tests;
