#![allow(clippy::unwrap_used)]

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
    let s = scan_file("#[cfg(test)]\nmod tests;\npub mod real;\n#[cfg(test)] pub(crate) mod more;")
        .unwrap();
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
fn a_module_also_declared_outside_cfg_test_is_scanned() {
    let s = scan_file(
        "#[cfg(test)] mod helpers;\n#[cfg(not(test))] mod helpers;\n#[cfg(test)] mod only;",
    )
    .unwrap();
    assert_eq!(s.test_mods, ["only"]);
    // A `lib.rs` or `main.rs` below `src/` is the module `lib`/`main`, not a root.
    let nested = [("crates/a/src/x/lib.rs".to_owned(), vec!["m".to_owned()])];
    assert!(is_test_file("crates/a/src/x/lib/m.rs", &nested));
    assert!(!is_test_file("crates/a/src/x/m.rs", &nested));
    let bin = [(
        "crates/a/src/bin/tool/main.rs".to_owned(),
        vec!["t".to_owned()],
    )];
    assert!(is_test_file("crates/a/src/bin/tool/t.rs", &bin));
}

#[test]
fn a_path_attribute_fails_and_never_marks_a_test_file() {
    // `live` compiles helpers.rs in production; the test `mod helpers;` must
    // not hide it.
    let s = scan_file("#[cfg(test)] mod helpers;\n#[path = \"helpers.rs\"] mod live;").unwrap();
    assert_eq!(s.hits, ["`#[path]` attribute"]);
    // A file outside `src/`, which the scan never reads.
    assert_eq!(
        hits("#[path = \"../../elsewhere/x.rs\"] mod m;"),
        ["`#[path]` attribute"]
    );
    assert_eq!(
        hits("#![path = \"a\"] #[cfg_attr(unix, path = \"u.rs\")] mod n;"),
        ["`#[path]` attribute", "`#[path]` attribute"]
    );
    // A test module loading another file marks neither file as a test.
    let t = scan_file("#[cfg(test)] #[path = \"x.rs\"] mod more;").unwrap();
    assert_eq!((t.hits.len(), t.test_mods.len()), (0, 0));
    assert_eq!(
        hits("#[doc = \"path = x\"] #[serde(rename = \"path\")] fn f() {}"),
        Vec::<String>::new()
    );
}

#[test]
fn a_module_a_macro_also_declares_is_scanned() {
    let s = scan_file("#[cfg(test)] mod x;\nm! { mod x; }\n#[cfg(test)] mod t;").unwrap();
    assert_eq!(s.test_mods, ["t"]);
}

#[test]
fn a_glob_import_of_an_outside_crate_is_found() {
    assert_eq!(
        hits("use outside::prelude::*;"),
        ["glob `use` of an outside crate"]
    );
    assert_eq!(
        hits("pub use {std::io, other::*};"),
        ["glob `use` of an outside crate"]
    );
    assert_eq!(
        hits(
            "use super::*; use crate::a::*; use std::io::*; use ::core::fmt::{self, *}; use $crate::m::*;"
        ),
        Vec::<String>::new()
    );
}

#[test]
fn a_semicolon_after_a_comma_inside_a_comparison_does_not_hide_the_rest() {
    assert_eq!(hits("m! { #[cfg(test)] x < y, z = 1.5; }"), ["1.5"]);
}

#[test]
fn an_ml_runtime_is_matched_by_its_family_not_only_its_facade() {
    for name in [
        "burn-ndarray",
        "burn-core",
        "ort-sys",
        "onnxruntime-sys",
        "torch-sys",
        "tract-linalg",
        "candle-transformers",
        "rten",
        "wonnx",
        "tensorflow",
        "linfa-trees",
        "smartcore",
        "dfdx-core",
        "llama-cpp-2",
        "mistralrs-core",
    ] {
        assert!(is_ml_runtime(name), "{name} not caught");
    }
    for name in ["tracing", "ortho", "burnt", "candles", "tokio"] {
        assert!(!is_ml_runtime(name), "{name} caught");
    }
}

/// A throwaway tree, removed when the test ends.
struct Fixture(PathBuf);

impl Drop for Fixture {
    fn drop(&mut self) {
        // Cleanup of a temp dir; a leftover is harmless and must not mask the assertion.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn every_kind_of_violation_in_a_tree_is_reported() {
    let fx = Fixture(std::env::temp_dir().join(format!("float-check-{}", std::process::id())));
    let node = "bins/maya2c-node";
    let manifest = "[package]\nname = \"maya2c-node\"\n[dependencies]\nburn-ndarray = \"0.1\"\n";
    let main =
        "use outside::*;\nfn main() { let _x: f64 = 0.5; maya_fee_market::apply_block_fees(); }\n";
    for (path, text) in [
        ("Cargo.toml", "[workspace]\n"),
        ("Cargo.lock", "version = 4\n"),
        (&format!("{node}/Cargo.toml"), manifest),
        (&format!("{node}/src/main.rs"), main),
        (&format!("{node}/src/quiet.rs"), "pub fn q() -> u8 { 1 }\n"),
        // An exclusion waives floats, never the model check.
        (
            &format!("{node}/src/loud.rs"),
            "pub fn l() -> f64 { resolve_base_fee() }\n",
        ),
        (&format!("{node}/src/calm/a.rs"), "pub fn a() -> u8 { 1 }\n"),
    ] {
        let p = fx.0.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    let rules = Rules {
        allow: &[("bins/maya2c-node/src/gone.rs", &["f64"], "fixture")],
        exclude: &[
            ("bins/maya2c-node/src/quiet.rs", "fixture"),
            ("bins/maya2c-node/src/loud.rs", "fixture"),
            ("bins/maya2c-node/src/calm", "fixture"),
            ("crates/unlinked", "fixture"),
        ],
        must_scan: &["bins/maya2c-node/src/main.rs", "crates/vm"],
    };
    let (_, failures) = check(&fx.0, &rules).unwrap();
    assert_eq!(
        failures,
        [
            "bins/maya2c-node/src/main.rs: glob `use` of an outside crate, f64, 0.5 (allowlist expects [])",
            "bins/maya2c-node/src/main.rs: apply_block_fees (fee-market's neural rule, reached from outside it)",
            "bins/maya2c-node/src/loud.rs: resolve_base_fee (fee-market's neural rule, reached from outside it)",
            "bins/maya2c-node/src/gone.rs: allowlisted but not scanned (stale entry)",
            "bins/maya2c-node/src/quiet.rs: excluded but holds no float (stale entry)",
            "bins/maya2c-node/src/calm: excluded but holds no float (stale entry)",
            "crates/unlinked: excluded but not linked by the node (stale entry)",
            "crates/vm: consensus code no longer scanned",
            "burn-ndarray: ML runtime in the node's dependency closure",
        ]
    );
}

#[test]
fn the_consensus_crates_hold_no_float_and_no_model() {
    run(&[]).unwrap();
}
