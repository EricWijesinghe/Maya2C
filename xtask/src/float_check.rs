//! `cargo xtask float-check` — invariant 36, mechanised.
//!
//! No float and no model output in a consensus rule. A float is a rounding
//! mode, and two validators on different hardware, compilers or flags can
//! round the same expression differently; one ULP of disagreement in a state
//! root is a fork. The rule used to live under invariant 20, which retired with
//! `crates/zkml` (ADR-008) and left it as prose nobody checked.
//!
//! This lexes every non-test source file of the consensus crates and fails on
//! an `f32`/`f64` token or a float literal anywhere outside an [`ALLOW`] entry,
//! macro bodies included (a `syn` visitor would skip those). Comments and
//! strings never reach the token stream. Items under `#[cfg(test)]` or
//! `#[test]` are skipped. It also fails if a scanned crate depends directly on
//! an ML runtime ([`ML_CRATES`]), the other half of the rule: a model's output
//! must not decide a block, a payout or a parameter.

use std::path::{Path, PathBuf};

use proc_macro2::{Delimiter, TokenStream, TokenTree};

/// Directories and files that decide consensus, relative to the repo root.
const ROOTS: &[&str] = &[
    "crates/node/src/state",
    "crates/node/src/consensus",
    "crates/node/src/core",
    "crates/node/src/upgrade.rs",
    "crates/node/src/genesis.rs",
    "crates/ledger-math/src",
    "crates/dex/src",
    "crates/governance/src",
    "crates/fee-market/src",
    "crates/vrf/src",
    "crates/crypto-pq/src",
];

/// Manifests whose `[dependencies]` must name no ML runtime.
const MANIFESTS: &[&str] = &[
    "crates/node/Cargo.toml",
    "crates/ledger-math/Cargo.toml",
    "crates/dex/Cargo.toml",
    "crates/governance/Cargo.toml",
    "crates/fee-market/Cargo.toml",
    "crates/vrf/Cargo.toml",
    "crates/crypto-pq/Cargo.toml",
];

/// Crates that run a model, or hold the dormant neural fee model (ADR-029b).
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

/// `(repo-relative path, why a float there cannot reach a consensus rule)`.
/// Empty: every scanned file is float-free. An entry needs a reviewer.
const ALLOW: &[(&str, &str)] = &[];

/// Every `f32`/`f64` token and float literal in `src`, outside test items.
pub fn floats_in(src: &str) -> Result<Vec<String>, String> {
    let stream: TokenStream = src.parse().map_err(|e| format!("does not lex: {e:?}"))?;
    let mut out = Vec::new();
    scan(stream, &mut out);
    Ok(out)
}

fn scan(stream: TokenStream, out: &mut Vec<String>) {
    let tokens: Vec<TokenTree> = stream.into_iter().collect();
    let mut i = 0;
    while i < tokens.len() {
        if is_test_attr(&tokens, i) {
            i = skip_item(&tokens, i + 2);
            continue;
        }
        match &tokens[i] {
            TokenTree::Group(g) => scan(g.stream(), out),
            TokenTree::Ident(id) if id == "f32" || id == "f64" => out.push(id.to_string()),
            TokenTree::Literal(lit) if !is_tuple_index(&tokens, i) => {
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

fn is_punct(t: Option<&TokenTree>, c: char) -> bool {
    matches!(t, Some(TokenTree::Punct(p)) if p.as_char() == c)
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

/// Index after the item starting at `j`: its brace body or its `;`.
fn skip_item(tokens: &[TokenTree], mut j: usize) -> usize {
    while let Some(t) = tokens.get(j) {
        j += 1;
        match t {
            TokenTree::Group(g) if g.delimiter() == Delimiter::Brace => return j,
            TokenTree::Punct(p) if p.as_char() == ';' => return j,
            _ => {}
        }
    }
    j
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
    if text.ends_with("f32") || text.ends_with("f64") {
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
        // shortcut: a file named tests.rs is taken as a `#[cfg(test)] mod tests;`
        // (true for all three in scope today); track the declaring attribute if one is not.
        let is_test_file = p.file_name().is_some_and(|n| n == "tests.rs");
        if p.is_dir() {
            rust_files(&p, out)?;
        } else if p.extension().is_some_and(|x| x == "rs") && !is_test_file {
            out.push(p);
        }
    }
    Ok(())
}

/// ML runtimes named in a manifest's `[dependencies]` (by key or `package =`).
pub fn ml_deps_in(manifest: &str) -> Result<Vec<String>, String> {
    let doc: toml::Table = manifest.parse().map_err(|e| format!("not TOML: {e}"))?;
    let Some(deps) = doc.get("dependencies").and_then(toml::Value::as_table) else {
        return Ok(Vec::new());
    };
    Ok(deps
        .iter()
        .map(|(key, v)| {
            v.get("package")
                .and_then(toml::Value::as_str)
                .unwrap_or(key)
                .to_owned()
        })
        .filter(|name| ML_CRATES.contains(&name.as_str()))
        .collect())
}

fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/")
}

pub fn run(_args: &[String]) -> Result<(), String> {
    let root = crate::workspace_root();
    let mut files = Vec::new();
    for r in ROOTS {
        rust_files(&root.join(r), &mut files)?;
    }
    let mut failures = Vec::new();
    for f in &files {
        let path = rel(&root, f);
        if ALLOW.iter().any(|(p, _)| *p == path) {
            continue;
        }
        let src = std::fs::read_to_string(f).map_err(|e| format!("{path}: {e}"))?;
        let hits = floats_in(&src).map_err(|e| format!("{path}: {e}"))?;
        if !hits.is_empty() {
            failures.push(format!("{path}: {}", hits.join(", ")));
        }
    }
    for m in MANIFESTS {
        let text = std::fs::read_to_string(root.join(m)).map_err(|e| format!("{m}: {e}"))?;
        for dep in ml_deps_in(&text).map_err(|e| format!("{m}: {e}"))? {
            failures.push(format!("{m}: depends on ML runtime `{dep}`"));
        }
    }
    println!(
        "float-check: {} consensus files and {} manifests scanned, {} allowlisted, {} violations",
        files.len(),
        MANIFESTS.len(),
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
        floats_in(src).unwrap()
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
            fn live() -> f32 { 0.0 }
        ";
        assert_eq!(hits(src), ["f32", "0.0"]);
    }

    #[test]
    fn an_ml_runtime_dependency_is_found_by_key_or_rename() {
        let m = "[dependencies]\nserde = \"1\"\nort = \"2\"\nnn = { package = \"candle-core\", version = \"0.8\" }\n[dev-dependencies]\ntract-onnx = \"0.21\"\n";
        // Keys come back sorted: `nn` before `ort`.
        assert_eq!(ml_deps_in(m).unwrap(), ["candle-core", "ort"]);
        assert_eq!(
            ml_deps_in("[package]\nname = \"x\"\n").unwrap(),
            Vec::<String>::new()
        );
    }

    #[test]
    fn the_consensus_crates_hold_no_float_and_no_model() {
        run(&[]).unwrap();
    }
}
