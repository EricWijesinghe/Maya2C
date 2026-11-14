//! What the Hub's webview may say, checked on the shipped files in `ui/`.
//!
//! The Hub shows coins received on chain, never an income claim or a fiat
//! value (reports/audits/2026-11-12-brand-ux-devex-seo.md, W-32 and §5), and it
//! never calls the paused testnet live. These tests read the files the bundle
//! ships, so a copy edit that breaks either rule fails CI, and they run the
//! behavioural checks in `ui-tests/logic.test.mjs` under node.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn ui_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../ui")
}

/// Every file under `ui/` with one of `exts`, sorted, tokens.css excluded
/// (generated from design/tokens.json, so it is the one place colours live).
fn ui_files(exts: &[&str]) -> Vec<PathBuf> {
    fn walk(dir: &Path, exts: &[&str], out: &mut Vec<PathBuf>) {
        let mut entries: Vec<PathBuf> = fs::read_dir(dir)
            .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
            .map(|e| e.expect("dir entry").path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                walk(&path, exts, out);
            } else if path.file_name().is_some_and(|n| n != "tokens.css")
                && path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| exts.contains(&e))
            {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(&ui_dir(), exts, &mut out);
    out
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

// ------------------------------------------------------------ JS strings ----

/// Every string a script can put on screen: quoted literals and the text parts
/// of template literals (the `${...}` parts are code, scanned as code).
/// Comments and identifiers are not user text and are skipped.
fn js_strings(src: &str) -> Vec<String> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    scan_code(&chars, 0, false, &mut out);
    out
}

/// Scans code from `i`; with `in_braces`, stops after the `}` closing a
/// template interpolation. Returns the index after where it stopped.
fn scan_code(c: &[char], mut i: usize, in_braces: bool, out: &mut Vec<String>) -> usize {
    let mut depth = 0usize;
    while i < c.len() {
        match c[i] {
            '\\' => i += 2,
            '/' if c.get(i + 1) == Some(&'/') => {
                while i < c.len() && c[i] != '\n' {
                    i += 1;
                }
            }
            '/' if c.get(i + 1) == Some(&'*') => {
                i += 2;
                while i + 1 < c.len() && !(c[i] == '*' && c[i + 1] == '/') {
                    i += 1;
                }
                i += 2;
            }
            q @ ('"' | '\'' | '`') => i = literal(c, i + 1, q, out),
            '{' => {
                depth += 1;
                i += 1;
            }
            '}' if in_braces && depth == 0 => return i + 1,
            '}' => {
                depth = depth.saturating_sub(1);
                i += 1;
            }
            _ => i += 1,
        }
    }
    i
}

fn literal(c: &[char], mut i: usize, quote: char, out: &mut Vec<String>) -> usize {
    let mut text = String::new();
    while i < c.len() {
        match c[i] {
            '\\' => {
                if let Some(&next) = c.get(i + 1) {
                    text.push(next);
                }
                i += 2;
            }
            '$' if quote == '`' && c.get(i + 1) == Some(&'{') => {
                text.push(' ');
                i = scan_code(c, i + 2, true, out);
            }
            ch if ch == quote => {
                out.push(text);
                return i + 1;
            }
            ch => {
                text.push(ch);
                i += 1;
            }
        }
    }
    out.push(text);
    i
}

/// HTML a user can read or hear: everything outside `<script>` and `<style>`.
fn html_text(src: &str) -> String {
    let mut text = src.to_owned();
    for tag in ["script", "style"] {
        while let Some(start) = text.find(&format!("<{tag}")) {
            let close = format!("</{tag}>");
            let end = text[start..]
                .find(&close)
                .map_or(text.len(), |e| start + e + close.len());
            text.replace_range(start..end, " ");
        }
    }
    text
}

/// Every piece of user-facing text in the shipped UI, with its file.
fn user_text() -> Vec<(String, String)> {
    let mut all = Vec::new();
    for path in ui_files(&["js"]) {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        all.extend(
            js_strings(&read(&path))
                .into_iter()
                .map(|s| (name.clone(), s)),
        );
    }
    for path in ui_files(&["html"]) {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        // Line by line, so a failure names the line, not the whole page.
        let text = html_text(&read(&path));
        all.extend(
            text.lines()
                .map(|line| (name.clone(), line.trim().to_owned())),
        );
    }
    all
}

fn has_word_starting(text: &str, stem: &str) -> bool {
    let lower = text.to_lowercase();
    lower.match_indices(stem).any(|(at, _)| {
        !lower[..at]
            .chars()
            .next_back()
            .is_some_and(char::is_alphanumeric)
    })
}

const CURRENCY: &[char] = &['$', '€', '£', '¥', '₩', '₹', '₽', '₿', '¢'];
const FIAT: &[&str] = &["USD", "EUR", "KRW", "GBP", "JPY"];

#[test]
fn the_tokenizer_finds_template_text_and_skips_code() {
    let found = js_strings(
        "const a = `Paid ${maya(x)} MAYA ${b ? `in ${c}` : \"none\"}`; // earned\nif (/^\\d+$/.test(s)) f('ok');",
    );
    assert_eq!(found, ["in  ", "none", "Paid   MAYA  ", "ok"]);
}

#[test]
fn no_earn_wording_and_no_currency_symbol_in_user_text() {
    let mut bad = Vec::new();
    for (file, text) in user_text() {
        if has_word_starting(&text, "earn") {
            bad.push(format!("{file}: earn wording in {text:?}"));
        }
        if let Some(sym) = text.chars().find(|ch| CURRENCY.contains(ch)) {
            bad.push(format!("{file}: currency symbol {sym:?} in {text:?}"));
        }
        for code in FIAT {
            if text
                .split(|ch: char| !ch.is_ascii_alphanumeric())
                .any(|w| w == *code)
            {
                bad.push(format!("{file}: fiat code {code} in {text:?}"));
            }
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn the_word_check_catches_earn_and_spares_learn() {
    assert!(has_word_starting("Earned this session", "earn"));
    assert!(has_word_starting("you earn", "earn"));
    assert!(!has_word_starting("Learn more", "earn"));
}

/// An hourly rate or an "up to" amount reads as income even in a coin with no
/// value (audit §5); the Hub shows only what the chain already credited.
#[test]
fn no_income_rate_and_no_promised_amount_in_user_text() {
    const RATE: &[&str] = &["MAYA/h", "/h received", "per hour", "an hour", "up to"];
    let bad: Vec<String> = user_text()
        .into_iter()
        .filter(|(_, text)| {
            let lower = text.to_lowercase();
            RATE.iter().any(|r| lower.contains(&r.to_lowercase()))
        })
        .map(|(file, text)| format!("{file}: {text:?}"))
        .collect();
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

/// The synthetic preview (fabricated height, epoch and rewards) stays out of
/// `ui/`, which is the whole app bundle (`frontendDist = "../ui"`).
#[test]
fn the_preview_generator_is_not_shipped() {
    assert!(
        !ui_dir().join("preview.js").exists(),
        "ui/preview.js would ship in every build; it belongs in ui-tests/"
    );
    let html = read(&ui_dir().join("index.html"));
    assert!(
        !html.contains("src=\"preview.js\""),
        "index.html loads the preview in the app"
    );
    let conf = read(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tauri.conf.json"));
    assert!(
        conf.contains("\"frontendDist\": \"../ui\""),
        "the bundle root moved; re-check what ships"
    );
}

#[test]
fn nothing_is_written_as_live_and_no_october_trial_is_promised() {
    let html = read(&ui_dir().join("index.html"));
    assert!(
        !html.contains("Live on testnet"),
        "index.html hard-codes a live legend chip; NetStatus.legend draws it from the snapshot"
    );
    let universe = read(&ui_dir().join("universe.js"));
    assert!(
        !universe.contains("status: \"live\""),
        "universe.js writes an entry as live; built entries turn live only on a reachable snapshot"
    );
    for (file, text) in user_text() {
        assert!(!text.contains("mid-October"), "{file}: {text:?}");
        assert!(
            !text.contains("Node unreachable"),
            "{file}: the paused card replaces {text:?}"
        );
    }
}

#[test]
fn the_stop_report_link_is_the_same_page_in_the_webview_and_the_opener() {
    let page = "https://github.com/EricWijesinghe/Maya2C/blob/master/reports/sessions/2026-10-28-testnet-stopped.md";
    let lib = read(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs"));
    let status = read(&ui_dir().join("status.js"));
    assert!(lib.contains(page), "src/lib.rs STOP_REPORT");
    assert!(status.contains(page), "ui/status.js STOP_REPORT");
    let report = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../reports/sessions/2026-10-28-testnet-stopped.md");
    assert!(
        report.is_file(),
        "the report the link names exists: {}",
        report.display()
    );
}

// --------------------------------------------------------- colour literals ----

/// The rule of `cargo xtask design-tokens --literals` (design_tokens/literals.rs),
/// restated so the Hub's CI holds it on branches that predate that command:
/// a hex colour or a colour function with a literal first argument, outside
/// tokens.css and the `theme-color` meta line, is a literal.
fn literals_in(line: &str) -> Vec<String> {
    let is_word = |ch: char| ch.is_alphanumeric() || ch == '_';
    let chars: Vec<char> = line.chars().collect();
    let mut found = Vec::new();
    for (i, &ch) in chars.iter().enumerate() {
        let prev = i.checked_sub(1).map(|p| chars[p]);
        if ch == '#' && !prev.is_some_and(|p| p == '&' || p == '#' || is_word(p)) {
            let run = chars[i + 1..]
                .iter()
                .take_while(|c| c.is_ascii_hexdigit())
                .count();
            let after = chars.get(i + 1 + run).copied();
            if matches!(run, 3 | 4 | 6 | 8) && !after.is_some_and(is_word) {
                found.push(chars[i..=i + run].iter().collect());
            }
        }
    }
    let lower = line.to_lowercase();
    for name in [
        "rgba", "rgb", "hsla", "hsl", "hwb", "oklch", "oklab", "lab", "lch",
    ] {
        for (at, _) in lower.match_indices(&format!("{name}(")) {
            let prev = lower[..at].chars().next_back();
            if prev.is_some_and(|p| is_word(p) || p == '-') {
                continue;
            }
            let rest = lower[at + name.len() + 1..].trim_start();
            if rest.starts_with(|c: char| c.is_ascii_digit() || c == '.') {
                found.push(format!("{name}("));
            }
        }
    }
    found
}

#[test]
fn the_literal_rule_matches_the_xtask_cases() {
    assert_eq!(literals_in("color: #1fb5c2;"), ["#1fb5c2"]);
    assert_eq!(literals_in("fill: rgb(1 2 3)"), ["rgb("]);
    assert!(literals_in("x: rgb(var(--a)); &#8212; .feeds #fx").is_empty());
    assert!(literals_in("`rgb(${r} ${g} ${b} / ${a})`").is_empty());
}

#[test]
fn no_colour_literal_outside_tokens_css() {
    let mut bad = Vec::new();
    for path in ui_files(&["css", "html", "js", "mjs"]) {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        for (n, line) in read(&path).lines().enumerate() {
            if line.contains("name=\"theme-color\"") {
                continue;
            }
            for lit in literals_in(line) {
                bad.push(format!("{name}:{}: {lit}", n + 1));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "name a role from design/tokens.json:\n{}",
        bad.join("\n")
    );
}

// ----------------------------------------------------------- behaviour ----

/// `ui-tests/logic.test.mjs` under node. CI runners ship node; a laptop
/// without it skips with a line saying so, and CI (`CI` set) fails instead.
#[test]
fn ui_logic_tests_pass_under_node() {
    let hub = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let run = Command::new("node")
        .args(["--test", "ui-tests/logic.test.mjs"])
        .current_dir(&hub)
        .output();
    match run {
        Ok(out) => assert!(
            out.status.success(),
            "node --test ui-tests/logic.test.mjs failed:\n{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
        Err(e) if std::env::var_os("CI").is_none() => {
            eprintln!("NOT RUN: node is not on PATH ({e}); ui-tests/logic.test.mjs skipped");
        }
        Err(e) => panic!("node is required in CI for ui-tests/logic.test.mjs: {e}"),
    }
}
