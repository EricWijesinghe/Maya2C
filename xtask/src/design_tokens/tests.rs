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
        listed.len() >= 130,
        "130 pairs listed, found {}",
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
    assert_eq!(sep.len(), 3, "{sep:?}");
    for want in [
        ("dark", "surface-3", "bg"),
        // A card on the dark ground, without leaning on its hairline.
        ("dark", "surface-1", "bg"),
        ("light", "surface-3", "surface-1"),
    ] {
        let found = sep.iter().find(|s| (s.0, s.1, s.2) == want);
        assert!(found.is_some_and(|s| s.3 >= 1.25), "{want:?} in {sep:?}");
    }
}

#[test]
fn a_pair_below_its_minimum_is_reported() {
    // Dark text-2 darkened to #3a4448: about 2:1 on the dark bg.
    let failures = below_minimum(&edited("#bcd0d7", "#3a4448"));
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
        assert!((w.min - min).abs() < f64::EPSILON, "{w}");
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
    // Four sets under normal vision, one cvd pair under every model.
    assert_eq!(worst.len(), THEMES.len() * (4 + Vision::ALL.len()));
    for w in &worst {
        println!("{w}");
        assert!(w.distance >= w.min, "{w}");
    }
    // The neutral set carries its own, smaller minimum; the rest the group's.
    assert!(worst.iter().any(|w| w.min < min && w.a == "steel"));
    assert!(
        worst
            .iter()
            .filter(|w| (w.min - min).abs() < f64::EPSILON)
            .count()
            >= THEMES.len() * 3
    );
}

#[test]
fn a_warning_the_colour_of_a_coin_amount_fails() {
    // Light warn-fill set to the light bronze-fill: one colour, two meanings.
    let tokens = edited("\"warn-fill\": \"#a06b00\"", "\"warn-fill\": \"#6f4612\"");
    let close: Vec<_> = role_worst(&tokens)
        .unwrap()
        .into_iter()
        .filter(|w| w.distance < w.min)
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
    // An olive #aaad01 clears every fill under normal vision and lands on
    // the dark bronze-fill #e98a13 for a deuteranope; only the cvd pair
    // catches it.
    let tokens = edited("\"warn-fill\": \"#f8ca06\"", "\"warn-fill\": \"#aaad01\"");
    let close: Vec<_> = role_worst(&tokens)
        .unwrap()
        .into_iter()
        .filter(|w| w.distance < w.min)
        .collect();
    assert!(
        close.iter().any(|w| w.theme == "dark"
            && w.vision == Vision::Deuteranopia
            && (w.a.as_str(), w.b.as_str()) == ("bronze-fill", "warn-fill")),
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
        &edited("\"bg\": \"#081c28\"", "\"bg\": \"#081c29\""),
    )
    .unwrap();
    assert_eq!(
        changed,
        vec!["`--bg` dark is `#081c28` in the doc, `#081c29` in the tokens".to_owned()]
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
    let tokens = edited("\"info\": \"#6cb6ff\"", "\"violet\": \"#6cb6ff\"");
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
        ":root, [data-theme=\"dark\"] {\n  color-scheme: dark;",
        "@media (prefers-color-scheme: light) {\n  :root:where(:not([data-theme=\"dark\"])) {\n    color-scheme: light;",
        "[data-theme=\"light\"] {\n  color-scheme: light;",
        "--bg: #081c28;",
        "--bg: #faf8f4;",
        "--chart-1: #1fd3cc;",
        "--chart-1: #089192;",
        "--type-display-1-size: 56px;",
        "--space-24: 96px;",
        "--r-pill: 999px;",
        "@media (prefers-reduced-motion: reduce) {\n  :root {\n    --dur-1: 0ms;",
    ] {
        assert!(css.contains(needle), "missing {needle:?}");
    }
    // Explicit choice beats the OS: the light attribute block comes after
    // the media query at equal specificity, and the media block skips a root
    // that chose dark. Each theme is written once per selector group.
    let media = css.find("prefers-color-scheme").unwrap();
    assert!(css.find("[data-theme=\"light\"] {").unwrap() > media);
    assert_eq!(css.matches("--bg: #081c28;").count(), 1);
    assert_eq!(css.matches("--bg: #faf8f4;").count(), 2);
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
        assert_eq!(css.matches(name).count(), 3, "{name}");
    }
}

#[test]
fn themes_must_name_the_same_roles() {
    let tokens = edited("\"steel\": \"#445469\",", "");
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
    let copies = copies(&real()).unwrap();
    let root = temp("stale");
    for (target, css) in &copies {
        let path = root.join(target);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, css).unwrap();
    }
    assert!(stale(&root, &copies).is_empty());
    let (drifted, missing) = (TARGETS[3].0, TARGETS[1].0);
    fs::write(
        root.join(drifted),
        copies[3].1.replace("#081c28", "#081c29"),
    )
    .unwrap();
    fs::remove_file(root.join(missing)).unwrap();
    let mut found = stale(&root, &copies);
    found.sort();
    let mut want = vec![missing.to_owned(), drifted.to_owned()];
    want.sort();
    assert_eq!(found, want);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn the_committed_copies_are_current() {
    let stale = stale(&crate::workspace_root(), &copies(&real()).unwrap());
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

/// The real token file with several literals replaced.
fn edited_all(edits: &[(&str, &str)]) -> Json {
    let path = crate::workspace_root().join(SOURCE);
    let mut text = fs::read_to_string(path).unwrap();
    for (from, to) in edits {
        assert!(
            text.contains(from),
            "fixture literal {from} not in {SOURCE}"
        );
        text = text.replacen(from, to, 1);
    }
    Json::parse(&text).unwrap()
}

#[test]
fn composed_grounds_are_painted_as_a_browser_paints_them() {
    let roles = theme(&real(), "dark").unwrap();
    // deco-teal rgb(31 211 204 / 0.26) over surface-1 #16303d.
    assert_eq!(ground(&roles, "dark", HERO).unwrap().to_hex(), "#185a62");
    assert_eq!(ground(&roles, "dark", "bg").unwrap().to_hex(), "#081c28");
    // An opaque top or an unknown ground is refused, not guessed.
    assert!(ground(&roles, "dark", "bronze over surface-1").is_err());
    assert!(ground(&roles, "dark", "deco-teal over nothing").is_err());
}

#[test]
fn hero_chip_and_selected_text_is_gated_on_its_composed_ground() {
    let measured = pair_ratios(&real()).unwrap();
    for theme_name in THEMES {
        for (fg, bg) in [
            ("text-2", HERO),
            ("accent-text", HERO),
            ("warn", "warn-subtle over surface-2"),
            ("danger", "danger-subtle over bg"),
            ("text-3", SELECTED),
        ] {
            let m = measured
                .iter()
                .find(|m| m.theme == theme_name && m.fg == fg && m.bg == bg)
                .unwrap_or_else(|| panic!("{theme_name} {fg} on {bg} not measured"));
            assert!(m.ratio >= 4.5, "{m}");
        }
    }
    // text-3 is never set on the hero, so it is not required there.
    assert!(
        !required(&[])
            .iter()
            .any(|(fg, bg, ..)| fg == "text-3" && bg == HERO)
    );
}

#[test]
fn a_brighter_hero_glow_that_drowns_its_text_fails() {
    let failures = below_minimum(&edited(
        "\"deco-teal\": \"rgb(31 211 204 / 0.26)\"",
        "\"deco-teal\": \"rgb(31 211 204 / 0.6)\"",
    ));
    assert!(
        failures.iter().any(|m| m.theme == "dark" && m.bg == HERO),
        "{failures:?}"
    );
}

#[test]
fn a_ramp_step_named_by_reference_is_measured_as_the_step() {
    let roles = theme(&real(), "light").unwrap();
    assert_eq!(
        colour(&roles, "light", "bronze-fill").unwrap(),
        colour(&roles, "light", "bronze-text").unwrap()
    );
    let looped = edited(
        "\"bronze-display\": \"var(--bronze)\"",
        "\"bronze-display\": \"var(--bronze-display)\"",
    );
    let dark = theme(&looped, "dark").unwrap();
    assert!(
        colour(&dark, "dark", "bronze-display")
            .unwrap_err()
            .contains("deeper")
    );
}

#[test]
fn a_family_that_drifts_off_its_hue_fails() {
    // The light bronze moved to copper #782d00, as the first revision did.
    let tokens = edited("\"bronze\": \"#8a5a1f\"", "\"bronze\": \"#782d00\"");
    let lines = families::report(&tokens).unwrap();
    assert!(
        lines
            .iter()
            .any(|l| l.failure.as_deref().is_some_and(|f| f.contains("bronze"))),
        "{:?}",
        lines.iter().map(|l| &l.text).collect::<Vec<_>>()
    );
    assert!(
        families::report(&real())
            .unwrap()
            .iter()
            .all(|l| l.failure.is_none())
    );
    assert!((families::spread(vec![350.0, 10.0]) - 20.0).abs() < 1e-9);
    assert!((families::spread(vec![10.0, 40.0, 25.0]) - 30.0).abs() < 1e-9);
}

#[test]
fn three_dark_series_on_paper_fail_the_ink_rule() {
    // The first revision's light chart: ink #06202a, navy and brown.
    let tokens = edited_all(&[
        ("\"chart-3\": \"#056190\"", "\"chart-3\": \"#123f99\""),
        ("\"chart-6\": \"#8b3f03\"", "\"chart-6\": \"#733f1c\""),
    ]);
    let lines = families::report(&tokens).unwrap();
    assert!(
        lines
            .iter()
            .any(|l| l.failure.is_some() && l.text.contains("light chart ink")),
        "{:?}",
        lines.iter().map(|l| &l.text).collect::<Vec<_>>()
    );
}

#[test]
fn a_value_that_fetches_is_refused() {
    for bad in [
        "url(https://x.test/a.png)",
        "URL(a)",
        "@import 'x'",
        "image-set(\"a\" 1x)",
    ] {
        assert!(check_value("x", bad).is_err(), "{bad}");
    }
    assert!(check_value("x", "linear-gradient(180deg, #000 0%, #fff 100%)").is_ok());
}

#[test]
fn the_explorer_copy_omits_what_it_does_not_draw() {
    let copies = copies(&real()).unwrap();
    let full = &copies[0].1;
    let explorer = &copies
        .iter()
        .find(|(t, _)| t.starts_with("apps/explorer"))
        .unwrap()
        .1;
    assert!(explorer.len() < full.len());
    for gone in [
        "--deco-teal:",
        "--grad-hero:",
        "--grad-forge:",
        "--chart-1:",
    ] {
        assert!(!explorer.contains(gone), "{gone}");
        assert!(full.contains(gone), "{gone}");
    }
    assert!(explorer.contains("--grad-cta:") && explorer.contains("--danger:"));
    assert!(explorer.contains("Omits: deco-"));
    // A copy may not omit a token one of its own values still uses.
    assert!(
        render_omitting(&real(), &["accent-grad-"])
            .unwrap_err()
            .contains("--grad-cta uses --accent-grad-from")
    );
}
