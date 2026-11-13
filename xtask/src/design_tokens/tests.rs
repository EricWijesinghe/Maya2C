use super::*;

fn real() -> Json {
    load(&crate::workspace_root()).unwrap()
}

fn real_doc() -> String {
    fs::read_to_string(crate::workspace_root().join(DOC)).unwrap()
}

/// The real token file with one literal replaced.
fn edited(from: &str, to: &str) -> Json {
    let path = crate::workspace_root().join(SOURCE);
    let text = fs::read_to_string(path).unwrap();
    assert!(
        text.contains(from),
        "fixture literal {from} not in {SOURCE}"
    );
    Json::parse(&text.replacen(from, to, 1)).unwrap()
}

fn below_minimum(tokens: &Json) -> Vec<Measured> {
    pair_ratios(tokens)
        .unwrap()
        .into_iter()
        .filter(|m| m.ratio < m.usage.minimum())
        .collect()
}

#[test]
fn every_listed_pair_meets_its_minimum_in_both_themes() {
    let tokens = real();
    let listed = pairs(&tokens).unwrap();
    let one_theme = listed.iter().filter(|p| p.3.is_some()).count();
    assert!(
        listed.len() >= 70,
        "70 pairs listed, found {}",
        listed.len()
    );
    let measured = pair_ratios(&tokens).unwrap();
    assert_eq!(
        measured.len(),
        (listed.len() - one_theme) * THEMES.len() + one_theme
    );
    for m in &measured {
        println!("{m}");
    }
    let failures = below_minimum(&tokens);
    assert!(failures.is_empty(), "below minimum: {failures:?}");
}

#[test]
fn the_required_matrix_is_listed() {
    let missing = missing_pairs(&real()).unwrap();
    assert!(missing.is_empty(), "{missing:?}");
}

#[test]
fn a_missing_required_pair_is_reported() {
    for (line, want) in [
        (
            "[\"text-3\", \"surface-3\", \"text\"],",
            "[\"text-3\", \"surface-3\", \"text\"]",
        ),
        (
            "[\"border-control\", \"surface-1\", \"non-text\"],",
            "[\"border-control\", \"surface-1\", \"non-text\"]",
        ),
        (
            "[\"info\", \"surface-2\", \"text\"],",
            "[\"info\", \"surface-2\", \"text\"]",
        ),
        (
            "[\"chart-6\", \"surface-1\", \"non-text\"],",
            "[\"chart-6\", \"surface-1\", \"non-text\"]",
        ),
        (
            "[\"chart-3\", \"surface-2\", \"non-text\"],",
            "[\"chart-3\", \"surface-2\", \"non-text\"]",
        ),
        (
            "[\"border-control\", \"surface-3\", \"non-text\"],",
            "[\"border-control\", \"surface-3\", \"non-text\"]",
        ),
        (
            "[\"on-fill\", \"danger-fill\", \"text\"],",
            "[\"on-fill\", \"danger-fill\", \"text\"]",
        ),
        (
            "[\"surface-3\", \"bg\", \"separation\", \"dark\"],",
            "[\"surface-3\", \"bg\", \"separation\", \"dark\"]",
        ),
    ] {
        let missing = missing_pairs(&edited(line, "")).unwrap();
        assert_eq!(missing, vec![want.to_owned()], "{line}");
    }
}

#[test]
fn a_large_text_entry_does_not_meet_a_text_requirement() {
    let tokens = edited(
        "[\"text-3\", \"surface-3\", \"text\"]",
        "[\"text-3\", \"surface-3\", \"large-text\"]",
    );
    assert_eq!(
        missing_pairs(&tokens).unwrap(),
        vec!["[\"text-3\", \"surface-3\", \"text\"]".to_owned()]
    );
}

#[test]
fn text_3_is_body_text_on_every_surface() {
    let measured = pair_ratios(&real()).unwrap();
    for theme_name in THEMES {
        for bg in SURFACES {
            let m = measured
                .iter()
                .find(|m| m.theme == theme_name && m.fg == "text-3" && m.bg == bg)
                .unwrap();
            assert_eq!(m.usage, Use::Text);
            assert!(m.ratio >= 4.5, "{m}");
        }
    }
}

#[test]
fn selected_panels_separate_without_a_hairline_in_both_themes() {
    let measured = pair_ratios(&real()).unwrap();
    let sep: Vec<_> = measured
        .iter()
        .filter(|m| m.usage == Use::Separation)
        .map(|m| (m.theme, m.fg.as_str(), m.bg.as_str(), m.ratio))
        .collect();
    assert_eq!(sep.len(), 2, "{sep:?}");
    for want in [
        ("dark", "surface-3", "bg"),
        ("light", "surface-3", "surface-1"),
    ] {
        let found = sep.iter().find(|s| (s.0, s.1, s.2) == want);
        assert!(found.is_some_and(|s| s.3 >= 1.25), "{want:?} in {sep:?}");
    }
}

#[test]
fn a_pair_below_its_minimum_is_reported() {
    // Dark text-2 darkened to #3a4448: about 2:1 on the dark bg.
    let failures = below_minimum(&edited("#c2d5dc", "#3a4448"));
    assert!(
        failures
            .iter()
            .any(|m| m.theme == "dark" && m.fg == "text-2" && m.bg == "bg"),
        "{failures:?}"
    );
    assert!(
        failures
            .iter()
            .all(|m| m.theme == "dark" && m.fg == "text-2"),
        "{failures:?}"
    );
}

#[test]
fn a_pair_naming_a_non_hex_role_is_an_error() {
    let tokens = edited(
        "[\"text\", \"bg\", \"text\"]",
        "[\"border\", \"bg\", \"text\"]",
    );
    assert!(pair_ratios(&tokens).is_err());
}

#[test]
fn a_decorative_role_is_never_measured_as_text() {
    let tokens = edited(
        "[\"text\", \"bg\", \"text\"]",
        "[\"deco-teal\", \"bg\", \"text\"]",
    );
    assert!(pair_ratios(&tokens).unwrap_err().contains("decorative"));
}

#[test]
fn chart_series_stay_apart_under_colour_blind_vision() {
    let tokens = real();
    let min = min_distance(&tokens, CHART).unwrap();
    let worst = chart_worst(&tokens).unwrap();
    assert_eq!(worst.len(), THEMES.len() * Vision::ALL.len());
    for w in &worst {
        println!("{w}");
        assert!(w.distance >= min, "{w} below {min}");
    }
}

#[test]
fn series_that_collapse_only_for_deuteranopes_fail() {
    // #9fe870 (a green) is far from every series in normal vision and lands
    // on bronze #ffbb6b for a deuteranope.
    let tokens = edited("\"chart-6\": \"#d97f4f\"", "\"chart-6\": \"#9fe870\"");
    let min = min_distance(&tokens, CHART).unwrap();
    let close: Vec<_> = chart_worst(&tokens)
        .unwrap()
        .into_iter()
        .filter(|w| w.distance < min)
        .collect();
    assert!(
        close
            .iter()
            .any(|w| w.theme == "dark" && w.vision == Vision::Deuteranopia),
        "{close:?}"
    );
    assert!(
        close.iter().all(|w| w.vision != Vision::Normal),
        "{close:?}"
    );
}

#[test]
fn meaning_roles_stay_apart() {
    let tokens = real();
    let min = min_distance(&tokens, ROLES).unwrap();
    let worst = role_worst(&tokens).unwrap();
    // Two sets under normal vision, two cvd pairs under every model.
    assert_eq!(worst.len(), THEMES.len() * (2 + 2 * Vision::ALL.len()));
    for w in &worst {
        println!("{w}");
        assert!(w.distance >= min, "{w} below {min}");
    }
}

#[test]
fn a_warning_the_colour_of_a_coin_amount_fails() {
    // Light warn set to the light bronze: one colour, two meanings.
    let tokens = edited("\"warn\": \"#796300\"", "\"warn\": \"#782d00\"");
    let min = min_distance(&tokens, ROLES).unwrap();
    let close: Vec<_> = role_worst(&tokens)
        .unwrap()
        .into_iter()
        .filter(|w| w.distance < min)
        .collect();
    assert!(
        close
            .iter()
            .any(|w| w.theme == "light" && w.vision == Vision::Normal),
        "{close:?}"
    );
}

#[test]
fn bronze_and_warn_must_stay_apart_for_deuteranopes_too() {
    // #a33f00 clears every role under normal vision and lands on the light
    // warn #796300 for a deuteranope; only the cvd pair catches it.
    let tokens = edited("\"bronze\": \"#782d00\"", "\"bronze\": \"#a33f00\"");
    let min = min_distance(&tokens, ROLES).unwrap();
    let close: Vec<_> = role_worst(&tokens)
        .unwrap()
        .into_iter()
        .filter(|w| w.distance < min)
        .collect();
    assert!(
        close.iter().any(|w| w.theme == "light"
            && w.vision == Vision::Deuteranopia
            && (w.a.as_str(), w.b.as_str()) == ("bronze", "warn")),
        "{close:?}"
    );
    assert!(
        close.iter().all(|w| w.vision != Vision::Normal),
        "{close:?}"
    );
}

#[test]
fn the_design_doc_table_equals_the_tokens() {
    let mismatches = doc::mismatches(&real_doc(), &real()).unwrap();
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

#[test]
fn a_token_the_doc_does_not_match_is_reported() {
    let changed = doc::mismatches(
        &real_doc(),
        &edited("\"bg\": \"#06202c\"", "\"bg\": \"#06202d\""),
    )
    .unwrap();
    assert_eq!(
        changed,
        vec!["`--bg` dark is `#06202c` in the doc, `#06202d` in the tokens".to_owned()]
    );
    let doc = real_doc().replacen("| `--info` |", "| `--infox` |", 1);
    let rows = doc::mismatches(&doc, &real()).unwrap();
    assert!(
        rows.iter().any(|m| m.contains("no row for `--info`")),
        "{rows:?}"
    );
    assert!(rows.iter().any(|m| m.contains("`--infox`")), "{rows:?}");
}

#[test]
fn violet_and_quantum_roles_are_refused() {
    let tokens = edited("\"info\": \"#71b0ff\"", "\"violet\": \"#71b0ff\"");
    assert!(render(&tokens).unwrap_err().contains("deferred to Eric"));
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
    let b = render(&edited("\"r-lg\": \"12px\"", "\"r-lg\": \"14px\"")).unwrap();
    assert_ne!(a.split_once("*/").unwrap().0, b.split_once("*/").unwrap().0);
    assert!(b.contains("--r-lg: 14px;"));
}

#[test]
fn the_stylesheet_has_both_themes_the_os_fallback_and_reduced_motion() {
    let css = render(&real()).unwrap();
    for needle in [
        ":root {\n  color-scheme: dark;",
        "@media (prefers-color-scheme: light) {\n  :root {\n    color-scheme: light;",
        "[data-theme=\"dark\"] {\n  color-scheme: dark;",
        "[data-theme=\"light\"] {\n  color-scheme: light;",
        "--bg: #06202c;",
        "--bg: #f7f3ec;",
        "--chart-1: #1fd3cc;",
        "--chart-1: #089192;",
        "--type-display-1-size: 56px;",
        "--space-24: 96px;",
        "--r-pill: 999px;",
        "@media (prefers-reduced-motion: reduce) {\n  :root {\n    --dur-1: 0ms;",
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
fn composed_tokens_are_declared_in_every_theme_block() {
    // A var() resolves where it is declared: declared once on :root, a
    // nested data-theme="light" would inherit the dark gradient.
    let css = render(&real()).unwrap();
    for name in [
        "--grad-cta:",
        "--grad-forge:",
        "--grad-hero:",
        "--chart-grid:",
        "--chart-axis:",
    ] {
        assert_eq!(css.matches(name).count(), 4, "{name}");
    }
}

#[test]
fn themes_must_name_the_same_roles() {
    let tokens = edited("\"steel\": \"#42546b\",", "");
    assert!(render(&tokens).unwrap_err().contains("steel"));
}

#[test]
fn reduced_motion_may_only_override_existing_tokens() {
    let tokens = edited("\"dur-3\": \"0ms\"", "\"dur-4\": \"0ms\"");
    assert!(render(&tokens).unwrap_err().contains("dur-4"));
}

#[test]
fn a_value_that_would_break_out_of_the_declaration_is_refused() {
    let tokens = edited("\"r-md\": \"8px\"", "\"r-md\": \"8px; } body { x: y\"");
    assert!(render(&tokens).is_err());
}

fn temp(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("design-tokens-{tag}-{}", std::process::id()))
}

#[test]
fn check_reports_each_drifted_or_missing_copy() {
    let css = render(&real()).unwrap();
    let root = temp("stale");
    for target in TARGETS {
        let path = root.join(target);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, &css).unwrap();
    }
    assert!(stale(&root, &css).is_empty());
    fs::write(root.join(TARGETS[3]), css.replace("#06202c", "#06202d")).unwrap();
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

#[test]
fn literals_are_found_outside_tokens_css_only() {
    let root = temp("literals");
    let write = |rel: &str, text: &str| {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    };
    write(
        "ui/app.css",
        ".a { color: #FF0000; }\n.b { background: rgb(1 2 3 / .5); border-color: var(--border); }\n.c { box-shadow: 0 0 0 rgb(var(--x)); }\n",
    );
    write(
        "ui/index.html",
        // Only the theme-color meta is exempt; the old opt-out marker hides nothing.
        "<p>&#8212; &#x2014;</p>\n<a href=\"#top\">top</a> <a href=\"#cafe\">x</a>\n<meta name=\"theme-color\" content=\"#081c28\">\n<p style=\"color:#123456\"> <!-- tokens:allow-literal -->\n",
    );
    write(
        "src/main.rs",
        "#[derive(Debug)]\n#![allow(x)]\nlet s = format!(\"{:#010x}\", 1);\nlet t = r#\"raw\"#;\nconst C: &str = \"#abcdef80\";\n",
    );
    write("ui/tokens.css", ":root { --bg: #081c28; }\n");
    write("node_modules/pkg/x.css", ".x { color: #fff; }\n");
    write("README.md", "#fff\n");
    let found = literals::scan(&root).unwrap();
    assert_eq!(
        found,
        vec![
            "src/main.rs:5: #abcdef80".to_owned(),
            "ui/app.css:1: #FF0000".to_owned(),
            "ui/app.css:2: rgb(1".to_owned(),
            "ui/index.html:2: #cafe".to_owned(),
            "ui/index.html:4: #123456".to_owned(),
        ]
    );
    assert!(literals::run(&root).is_err());
    fs::remove_dir_all(&root).unwrap();
    let clean = temp("literals-clean");
    fs::create_dir_all(&clean).unwrap();
    fs::write(clean.join("a.css"), ".a { color: var(--text); }\n").unwrap();
    assert!(literals::run(&clean).is_ok());
    fs::remove_dir_all(clean).unwrap();
}
