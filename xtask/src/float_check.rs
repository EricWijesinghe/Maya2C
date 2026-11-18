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
//! is a file declared by `#[cfg(test)] mod x;`; a `use` that binds the name
//! `test` (and so could shadow `#[test]`) is itself a violation. It also fails
//! if an ML runtime ([`ML_CRATES`]) is anywhere in the node's dependency
//! closure.
//!
//! Not caught: a float reached by type inference with no float token in the
//! scanned file (`"0.5".parse()` into an inferred type, a function from an
//! unscanned crate that returns `f64`), and code outside `src/` (a `build.rs`,
//! a `[lib] path` elsewhere, an `include!` of an unscanned file; none of these
//! occurs in linked code today). `clippy::float_arithmetic` uses type
//! information and closes the first gap; it is not configured yet.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use proc_macro2::{Delimiter, Spacing, TokenStream, TokenTree};

use crate::float_scope::{self, EXCLUDE, MUST_SCAN, under};

/// Crates that run a model. `maya-neural-fee` and `maya-zkml-prover` do not
/// exist yet: they are reserved for the neural fee gain moved out of the node
/// (PR #542) and a future zkML prover, so they fail the day they are linked.
const ML_CRATES: &[&str] = &[
    "tract-onnx",
    "tract-core",
    "ort",
    "onnxruntime",
    "candle-core",
    "candle-nn",
    "burn",
    "tch",
    "maya-neural-fee",
    "maya-zkml-prover",
];

/// `(repo-relative file, the exact float tokens it may hold, why none of them
/// reaches a consensus rule)`. Any other float in the file still fails. An
/// entry needs a reviewer.
const ALLOW: &[(&str, &[&str], &str)] = &[(
    "bins/maya2c-node/src/bft.rs",
    &["f64", "1_000.0"],
    "`observe_finality` takes the finality latency in seconds for a Prometheus histogram; it is computed after the block is final and feeds nothing back",
)];

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
}

/// Every float token in `src` outside test items, and its test modules.
pub fn scan_file(src: &str) -> Result<FileScan, String> {
    let stream: TokenStream = src.parse().map_err(|e| format!("does not lex: {e:?}"))?;
    let tokens: Vec<TokenTree> = stream.into_iter().collect();
    let mut out = FileScan::default();
    for i in 0..tokens.len() {
        if is_test_attr(&tokens, i) {
            out.test_mods.extend(out_of_line_mod(&tokens, i + 2));
        }
    }
    scan(&tokens, &mut out.hits);
    Ok(out)
}

fn scan(tokens: &[TokenTree], out: &mut Vec<String>) {
    let mut i = 0;
    while i < tokens.len() {
        if is_test_attr(tokens, i) {
            i = skip_item(tokens, i + 2);
            continue;
        }
        if use_binds_test(tokens, i) {
            out.push("use binds `test`".to_owned());
        }
        match &tokens[i] {
            TokenTree::Group(g) => scan(&g.stream().into_iter().collect::<Vec<_>>(), out),
            TokenTree::Ident(id) if names_a_float(&id.to_string()) => out.push(id.to_string()),
            TokenTree::Literal(lit) if !is_tuple_index(tokens, i) => {
                let text = lit.to_string();
                if is_float_literal(&text) {
                    out.push(text);
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

/// A `use` item at `i` that binds the name `test` (`use m::test;`,
/// `use m::{a, test};`, `use m::x as test;`). It would shadow the built-in
/// `#[test]` this scanner trusts to mark test-only items.
fn use_binds_test(tokens: &[TokenTree], i: usize) -> bool {
    if ident_at(tokens, i).as_deref() != Some("use") {
        return false;
    }
    let end = tokens[i..]
        .iter()
        .position(|t| is_punct(Some(t), ';'))
        .map_or(tokens.len(), |p| i + p);
    let mut flat = Vec::new();
    flatten(&tokens[i + 1..end], &mut flat);
    (0..flat.len()).any(|k| {
        ident_at(&flat, k).as_deref() == Some("test")
            && !is_punct(flat.get(k + 1), ':')
            && ident_at(&flat, k + 1).as_deref() != Some("as")
    })
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

/// `x` when the item at `j` is `mod x;`.
fn out_of_line_mod(tokens: &[TokenTree], j: usize) -> Option<String> {
    let k = after_attrs(tokens, j);
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
            ';' => return j,
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
            Some((dir, "mod.rs" | "lib.rs" | "main.rs")) => dir,
            _ => file.trim_end_matches(".rs"),
        };
        mods.iter().any(|m| {
            let p = format!("{base}/{m}");
            path == format!("{p}.rs") || under(path, &p)
        })
    })
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

pub fn run(_args: &[String]) -> Result<(), String> {
    let root = crate::workspace_root();
    let linked = float_scope::linked(&root)?;
    let mut paths = Vec::new();
    for d in &linked.dirs {
        rust_files(&root.join(d).join("src"), &mut paths)?;
    }
    let mut all: Vec<String> = paths.iter().map(|p| rel(&root, p)).collect();
    all.sort();
    let (excluded, lexed): (Vec<String>, Vec<String>) = all
        .into_iter()
        .partition(|f| EXCLUDE.iter().any(|(e, _)| under(f, e)));
    let mut scans = Vec::new();
    for f in &lexed {
        let src = std::fs::read_to_string(root.join(f)).map_err(|e| format!("{f}: {e}"))?;
        scans.push((f.clone(), scan_file(&src).map_err(|e| format!("{f}: {e}"))?));
    }
    let decls: Vec<(String, Vec<String>)> = scans
        .iter()
        .filter(|(_, s)| !s.test_mods.is_empty())
        .map(|(f, s)| (f.clone(), s.test_mods.clone()))
        .collect();
    let scanned: Vec<&(String, FileScan)> = scans
        .iter()
        .filter(|(f, _)| !is_test_file(f, &decls))
        .collect();
    let mut failures = Vec::new();
    for (f, s) in &scanned {
        let allowed = ALLOW
            .iter()
            .find(|(p, _, _)| p == f)
            .map_or(&[][..], |(_, toks, _)| *toks);
        if s.hits != allowed {
            failures.push(format!(
                "{f}: {} (allowlist expects [{}])",
                s.hits.join(", "),
                allowed.join(", ")
            ));
        }
    }
    let scanned_names: Vec<String> = scanned.iter().map(|(f, _)| f.clone()).collect();
    for (p, _, _) in ALLOW
        .iter()
        .filter(|(p, _, _)| !scanned_names.contains(&(*p).to_owned()))
    {
        failures.push(format!("{p}: allowlisted but not scanned (stale entry)"));
    }
    for (e, _) in EXCLUDE
        .iter()
        .filter(|(e, _)| !excluded.iter().any(|f| under(f, e)))
    {
        failures.push(format!(
            "{e}: excluded but not linked by the node (stale entry)"
        ));
    }
    // A one-file exclusion is only justified while the file holds a float.
    for f in excluded
        .iter()
        .filter(|f| EXCLUDE.iter().any(|(e, _)| e == f))
    {
        let src = std::fs::read_to_string(root.join(f)).map_err(|e| format!("{f}: {e}"))?;
        if scan_file(&src)
            .map_err(|e| format!("{f}: {e}"))?
            .hits
            .is_empty()
        {
            failures.push(format!("{f}: excluded but holds no float (stale entry)"));
        }
    }
    for m in unscanned(MUST_SCAN, &scanned_names) {
        failures.push(format!("{m}: consensus code no longer scanned"));
    }
    let ml: BTreeSet<&str> = ML_CRATES
        .iter()
        .copied()
        .filter(|c| linked.names.contains(*c))
        .collect();
    for dep in ml {
        failures.push(format!(
            "{dep}: ML runtime in the node's dependency closure"
        ));
    }
    println!(
        "float-check: {} files in {} linked crates scanned ({} test modules, {} excluded), {} packages checked for ML runtimes, {} allowlisted, {} violations",
        scanned.len(),
        linked.dirs.len(),
        lexed.len() - scanned.len(),
        excluded.len(),
        linked.names.len(),
        ALLOW.len(),
        failures.len()
    );
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "invariant 36: float or model in consensus code\n{}",
            failures.join("\n")
        ))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn hits(src: &str) -> Vec<String> {
        scan_file(src).unwrap().hits
    }

    #[test]
    fn a_float_type_or_literal_is_found_anywhere_in_code() {
        assert_eq!(hits("fn f(x: u64) -> f64 { x as f64 }"), ["f64", "f64"]);
        assert_eq!(
            hits("const R: u32 = 0; static S: [f32; 1] = [0.0];"),
            ["f32", "0.0"]
        );
        assert_eq!(
            hits("let a = 1e9; let b = 2f64; let c = 3.5_f32;"),
            ["1e9", "2f64", "3.5_f32"]
        );
        assert_eq!(hits("let v = vec![1.5, 2.0];"), ["1.5", "2.0"]);
        assert_eq!(hits("let r = ..1.5;"), ["1.5"]);
        assert_eq!(hits("let x = f64::from(3u32);"), ["f64"]);
        assert_eq!(
            hits("let a: f16 = b; let c: f128 = d; let e = 2f16 + 1f128;"),
            ["f16", "f128", "2f16", "1f128"]
        );
        assert_eq!(hits("let x: r#f64 = y();"), ["r#f64"]);
        assert_eq!(hits("let a = 1e-9 + 2.5E+3;"), ["1e-9", "2.5E+3"]);
    }

    #[test]
    fn other_float_types_and_wrappers_are_found() {
        assert_eq!(
            hits("let x = half::f16::from_bits(0x3C00) * half::bf16::ONE;"),
            ["f16", "bf16"]
        );
        assert_eq!(
            hits("let v: f32x4 = w; let u = core::simd::f64x2::splat(z);"),
            ["f32x4", "f64x2"]
        );
        assert_eq!(
            hits("fn g<T: FloatCore + Real>(a: OrderedFloat<T>, b: NotNan<T>) {}"),
            ["FloatCore", "Real", "OrderedFloat", "NotNan"]
        );
        assert_eq!(
            hits("let a = f32x; let b = f64xa; let c = buf16; let d = Realm;"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn an_import_that_shadows_the_test_attribute_is_found() {
        assert_eq!(hits("use m::pass as test;"), ["use binds `test`"]);
        assert_eq!(hits("pub use m::{a, test};"), ["use binds `test`"]);
        assert_eq!(hits("use m::test;"), ["use binds `test`"]);
        assert_eq!(
            hits("use crate::test::helpers; use m::test as t; use test_util::x;"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_float_named_inside_an_identifier_or_a_float_bound_is_found() {
        assert_eq!(hits("let x = v.as_f64();"), ["as_f64"]);
        assert_eq!(
            hits("let y = n.to_f32().map(from_f64); let b = f64_bits;"),
            ["to_f32", "from_f64", "f64_bits"]
        );
        assert_eq!(hits("fn g<T: num_traits::Float>() {}"), ["Float"]);
        assert_eq!(
            hits("let a = buf64; let b = f640; let c = af64_x;"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn integers_comments_strings_and_tuple_indices_are_not_floats() {
        let src = r#"
            // 0.5 in a comment, and f64 too
            /// doc: 1.5 f32
            const A: u64 = 0xE5 + 1_000u64 + 7usize as u64 + 0b10 + 0o7;
            const S: &str = "1.5 f64";
            fn g(t: ((u8, u8), u8)) -> u8 { t.0.1 + (0..3).len() as u8 }
            let m = 1.max(2);
        "#;
        assert_eq!(hits(src), Vec::<String>::new());
    }

    #[test]
    fn test_items_are_skipped_but_code_after_them_is_not() {
        let src = r"
            #[cfg(test)]
            mod tests { fn t() -> f64 { 0.5 } }
            #[test]
            fn u() { let _ = 1.5; }
            #[cfg(test)]
            mod more;
            #[cfg(test)]
            fn gen<A, B>() where A: Into<f64>, B: Copy { 0.5 }
            fn live() -> f32 { 0.0 }
        ";
        assert_eq!(hits(src), ["f32", "0.0"]);
    }

    #[test]
    fn a_test_field_parameter_variant_or_arm_hides_only_itself() {
        assert_eq!(hits("struct S { #[cfg(test)] a: u8, b: f64 }"), ["f64"]);
        assert_eq!(hits("fn f(#[cfg(test)] x: u8, y: f32) {}"), ["f32"]);
        // Fails closed: after a `,` inside `<..>` the rest is scanned, so a
        // float in a test-only generic is reported (an ALLOW entry absorbs it).
        assert_eq!(
            hits("struct S { #[cfg(test)] pub a: HashMap<K, f32>, b: f64 }"),
            ["f32", "f64"]
        );
        assert_eq!(hits("enum E { #[cfg(test)] A(f32), B(f64) }"), ["f64"]);
        assert_eq!(
            hits("match k { #[cfg(test)] A => 1, B => g(0.5) }"),
            ["0.5"]
        );
        assert_eq!(
            hits("match k { #[cfg(test)] A => a < b, B => g(0.5) }"),
            ["0.5"]
        );
        assert_eq!(
            hits("match k { #[cfg(test)] A => a <= 1.5, B => g(0.5) }"),
            ["0.5"]
        );
        assert_eq!(hits("let s = S { #[cfg(test)] a: 1.5, b: 2.5 };"), ["2.5"]);
        // A `<` comparison in a test field must not hide the next field.
        assert_eq!(
            hits("let s = S { #[cfg(test)] a: x < y, b: 1.5 > w, c: 0 };"),
            ["1.5"]
        );
        assert_eq!(
            hits("fn f(#[cfg(test)] x: impl Fn(u8) -> u8, y: f32) {}"),
            ["f32"]
        );
    }

    #[test]
    fn only_a_module_declared_under_cfg_test_marks_its_file_as_a_test() {
        let s = scan_file("#[cfg(test)]\nmod tests;\npub mod real;\n#[cfg(test)] #[path = \"x.rs\"] pub(crate) mod more;").unwrap();
        assert_eq!(s.test_mods, ["tests", "more"]);
        let decls = [
            ("crates/a/src/suite/mod.rs".to_owned(), s.test_mods.clone()),
            ("crates/a/src/lib.rs".to_owned(), vec!["tests".to_owned()]),
        ];
        assert!(is_test_file("crates/a/src/suite/tests.rs", &decls));
        assert!(is_test_file("crates/a/src/suite/tests/vectors.rs", &decls));
        assert!(is_test_file("crates/a/src/tests.rs", &decls));
        assert!(!is_test_file("crates/a/src/suite/real.rs", &decls));
        assert!(!is_test_file("crates/a/src/other/tests.rs", &decls));
        let flat = [(
            "crates/a/src/agility.rs".to_owned(),
            vec!["tests".to_owned()],
        )];
        assert!(is_test_file("crates/a/src/agility/tests.rs", &flat));
    }

    #[test]
    fn a_consensus_path_with_no_scanned_file_is_reported() {
        let scanned = ["crates/dex/src/lib.rs".to_owned()];
        assert_eq!(
            unscanned(&["crates/dex", "crates/vm"], &scanned),
            ["crates/vm"]
        );
    }

    #[test]
    fn the_consensus_crates_hold_no_float_and_no_model() {
        run(&[]).unwrap();
    }
}
