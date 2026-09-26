#![allow(clippy::unwrap_used)]

use maya_design_system::contrast::{Rgb, Use, ratio};
use maya_design_system::flows::{FLOWS, Flow, check};
use maya_design_system::guard::{
    DomainVerdict, Token, TokenVerdict, domain, poisoning, skeleton, token,
};
use maya_design_system::i18n::{CATALOG, Dir, Key, dir, t};
use maya_design_system::tokens::{REQUIRED_PAIRS, Role, Theme, color, css};

#[test]
fn the_contrast_formula_matches_the_wcag_reference_points() {
    let (w, b) = (Rgb(255, 255, 255), Rgb(0, 0, 0));
    assert!((ratio(w, b) - 21.0).abs() < 1e-9);
    assert!((ratio(w, w) - 1.0).abs() < 1e-9);
    // #767676 on white is the well-known lightest grey that passes 4.5:1.
    let r = ratio(Rgb::hex("#767676").unwrap(), w);
    assert!((4.5..4.6).contains(&r), "{r}");
    assert!(ratio(Rgb::hex("#777777").unwrap(), w) < 4.5);
}

#[test]
fn every_required_pair_meets_wcag_aa_in_both_themes() {
    let mut failures = Vec::new();
    for theme in Theme::ALL {
        for &(fg, bg, usage) in REQUIRED_PAIRS {
            let r = ratio(color(theme, fg), color(theme, bg));
            println!(
                "{:>7} {fg:?} on {bg:?}: {r:.2} (min {})",
                theme.name(),
                usage.minimum()
            );
            if r < usage.minimum() {
                failures.push(format!("{} {fg:?}/{bg:?} {r:.2}", theme.name()));
            }
        }
    }
    assert!(failures.is_empty(), "below AA: {failures:?}");
}

/// Finding, not a gate: the explorer's pre-design-system palette, measured.
#[test]
fn the_old_explorer_palette_is_measured() {
    let c = |h| Rgb::hex(h).unwrap();
    let pairs = [
        ("muted on panel", c("#8a91a0"), c("#1a1d26")),
        ("error on bg", c("#ff8a8a"), c("#12141a")),
        ("accent on panel", c("#5ad1a0"), c("#1a1d26")),
        ("button text on accent", c("#0b0d12"), c("#5ad1a0")),
    ];
    for (name, fg, bg) in pairs {
        let r = ratio(fg, bg);
        println!(
            "old explorer {name}: {r:.2} ({})",
            if r >= Use::Text.minimum() {
                "AA"
            } else {
                "below AA"
            }
        );
    }
}

#[test]
fn the_stylesheet_emits_every_role_for_both_themes_and_honours_reduced_motion() {
    let s = css();
    for theme in Theme::ALL {
        for role in Role::ALL {
            let decl = format!("--{}: {};", role.var(), color(theme, role).to_hex());
            assert!(s.contains(&decl), "{} missing {decl}", theme.name());
        }
    }
    assert!(s.contains("prefers-reduced-motion: reduce"));
    assert!(s.contains("--motion-2: 0ms;"));
}

/// The committed `design/tokens.css` is what the apps link; it must be the
/// generator's output. `UPDATE_GOLDEN=1` rewrites it.
#[test]
fn the_committed_stylesheet_is_current() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../design/tokens.css");
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(path, css()).unwrap();
    }
    let committed = std::fs::read_to_string(path).unwrap();
    assert_eq!(
        committed,
        css(),
        "run with UPDATE_GOLDEN=1 and commit design/tokens.css"
    );
}

/// Same rule for the specimen page the visual regression job screenshots.
#[test]
fn the_committed_specimen_is_current() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../design/specimen.html");
    let page = maya_design_system::specimen::page();
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(path, &page).unwrap();
    }
    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        page,
        "run with UPDATE_GOLDEN=1"
    );
    assert!(
        page.contains(r#"dir="rtl""#),
        "the RTL panel is part of the fixture"
    );
}

#[test]
fn no_wallet_flow_has_a_dead_end() {
    assert_eq!(FLOWS.len(), 10);
    let problems: Vec<String> = FLOWS.iter().flat_map(check).collect();
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn the_flow_checker_catches_each_kind_of_dead_end() {
    let trapped = Flow {
        name: "t",
        edges: &[
            ("start", "error"),
            ("start", "done"),
            ("start", "cancelled"),
        ],
    };
    assert!(
        check(&trapped)
            .iter()
            .any(|p| p.contains("`error` cannot reach done"))
    );
    let no_error = Flow {
        name: "n",
        edges: &[("start", "done"), ("start", "cancelled")],
    };
    assert!(
        check(&no_error)
            .iter()
            .any(|p| p.contains("no error state"))
    );
    let orphan = Flow {
        name: "o",
        edges: &[
            ("start", "done"),
            ("start", "cancelled"),
            ("error", "start"),
        ],
    };
    assert!(
        check(&orphan)
            .iter()
            .any(|p| p.contains("`error` is unreachable"))
    );
    let no_exit = Flow {
        name: "x",
        edges: &[
            ("start", "a"),
            ("a", "b"),
            ("b", "done"),
            ("a", "error"),
            ("error", "a"),
        ],
    };
    assert!(
        check(&no_exit)
            .iter()
            .any(|p| p.contains("`a` cannot leave"))
    );
}

#[test]
fn address_poisoning_is_caught_on_matching_ends() {
    let book = ["0x1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8fabcd"];
    let planted = "0x1a2b99999999999999999999999999999999abcd";
    assert_eq!(poisoning(planted, &book), Some(book[0]));
    assert_eq!(poisoning(book[0], &book), None, "the real contact is fine");
    assert_eq!(
        poisoning(&book[0].to_uppercase().replace("0X", "0x"), &book),
        None
    );
    assert_eq!(
        poisoning("0x9999999999999999999999999999999999999999", &book),
        None
    );
}

#[test]
fn look_alike_tokens_are_caught_by_contract_not_symbol() {
    let verified = [Token {
        symbol: "USDC",
        contract: "c-usdc",
    }];
    let t = |symbol, contract| token(Token { symbol, contract }, &verified);
    assert_eq!(t("USDC", "c-usdc"), TokenVerdict::Verified);
    assert_eq!(
        t("USDC", "c-fake"),
        TokenVerdict::Impersonates("USDC"),
        "same symbol, wrong contract"
    );
    assert_eq!(
        t("USDС", "c-fake"),
        TokenVerdict::Impersonates("USDC"),
        "Cyrillic С"
    );
    assert_eq!(
        t("U\u{200b}SDC", "c-fake"),
        TokenVerdict::Impersonates("USDC"),
        "zero-width space"
    );
    assert_eq!(
        t("usdc", "c-fake"),
        TokenVerdict::Impersonates("USDC"),
        "case"
    );
    assert_eq!(
        t("USDT", "c-usdt"),
        TokenVerdict::Unknown,
        "a different token is not flagged"
    );
    assert_eq!(skeleton("MАYА"), "MAYA");
}

#[test]
fn phishing_domains_are_classified() {
    let allow = ["maya2c.io"];
    assert_eq!(domain("maya2c.io", &allow), DomainVerdict::Trusted);
    assert_eq!(domain("wallet.maya2c.io", &allow), DomainVerdict::Trusted);
    assert_eq!(
        domain("maya2c.io.evil.com", &allow),
        DomainVerdict::LookAlike("maya2c.io")
    );
    assert_eq!(
        domain("rnaya2c.io", &allow),
        DomainVerdict::LookAlike("maya2c.io"),
        "rn for m"
    );
    assert_eq!(
        domain("maya2c.co", &allow),
        DomainVerdict::LookAlike("maya2c.io")
    );
    assert_eq!(
        domain("mауа2с.io", &allow),
        DomainVerdict::LookAlike("maya2c.io"),
        "Cyrillic"
    );
    assert_eq!(domain("xn--mya2c-3ve.io", &allow), DomainVerdict::Punycode);
    assert_eq!(domain("example.org", &allow), DomainVerdict::Unknown);
    assert_eq!(
        domain("evilmaya2c.io", &allow),
        DomainVerdict::Unknown,
        "not a subdomain, not close"
    );
}

#[test]
fn every_locale_is_complete_keeps_placeholders_and_one_is_rtl() {
    assert_eq!(CATALOG.len(), 10);
    for (tag, _, strings) in CATALOG {
        for key in Key::ALL {
            let s = strings[key as usize];
            assert!(!s.trim().is_empty(), "{tag} {key:?} empty");
            for p in key.placeholders() {
                assert!(s.contains(p), "{tag} {key:?} lost {p}");
            }
        }
    }
    assert_eq!(dir("ar"), Dir::Rtl);
    assert_eq!(dir("en"), Dir::Ltr);
    assert_eq!(
        t("xx", Key::Send),
        "Send",
        "unknown locale falls back to English"
    );
    assert_ne!(t("ja", Key::Send), t("en", Key::Send));
}
