//! `cargo xtask claims-check` — the prior-art standing order, mechanised.
//!
//! `CLAUDE.md`: no document, UI or announcement may call a feature of this chain
//! "first", "only" or "unprecedented" unless `docs/prior-art/<feature>.md`
//! records a dated search. This scans the public surfaces for superlative
//! claims and fails on any line that does not cite a prior-art record which:
//!
//! 1. exists;
//! 2. says `**Search status:** searched <date>` ("partially searched" and
//!    "not searched" authorise nothing);
//! 3. does not list the claim among its forbidden phrases (a quoted phrase on
//!    a line containing `**forbidden**`).
//!
//! What counts as a claim is deliberately narrow, because "the first deposit"
//! and "the only crate" are ordinary technical English: a superlative word
//! directly before a product noun ("first chain", "only wallet"), or
//! "unprecedented", "world's first", "first-ever", "industry-first".

use std::path::{Path, PathBuf};

const NOUNS: &[&str] = &[
    "chain",
    "blockchain",
    "l1",
    "layer-1",
    "layer",
    "network",
    "protocol",
    "wallet",
    "ledger",
    "platform",
    "post-quantum",
    "quantum-safe",
    "quantum-resistant",
    "pq",
];
const PHRASES: &[&str] = &[
    "unprecedented",
    "world's first",
    "first-ever",
    "first ever",
    "industry-first",
    "industry first",
];

/// Whether `line` makes a superlative claim.
fn is_claim(line: &str) -> bool {
    let l = line.to_ascii_lowercase();
    if PHRASES.iter().any(|p| l.contains(p)) {
        return true;
    }
    let words: Vec<&str> = l
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '\''))
        .filter(|w| !w.is_empty())
        .collect();
    words
        .windows(2)
        .any(|w| matches!(w[0], "first" | "only") && NOUNS.contains(&w[1]))
}

/// Why a claim line is not authorised, or `None` if it is.
fn unauthorised(line: &str, prior_art: &Path) -> Option<String> {
    let Some(at) = line.find("prior-art/") else {
        return Some("cites no docs/prior-art/ record".into());
    };
    let name: String = line[at + "prior-art/".len()..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .collect();
    let Ok(record) = std::fs::read_to_string(prior_art.join(&name)) else {
        return Some(format!("cites prior-art/{name}, which does not exist"));
    };
    let searched = record
        .lines()
        .find_map(|l| l.strip_prefix("**Search status:** "))
        .is_some_and(|s| s.starts_with("searched 2"));
    if !searched {
        return Some(format!(
            "prior-art/{name} records no completed, dated search"
        ));
    }
    let lower = line.to_ascii_lowercase();
    for rule in record.lines().filter(|l| l.contains("**forbidden**")) {
        for (i, quoted) in rule.split('"').enumerate() {
            if i % 2 == 1 && !quoted.is_empty() && lower.contains(&quoted.to_ascii_lowercase()) {
                return Some(format!("prior-art/{name} forbids \"{quoted}\""));
            }
        }
    }
    None
}

fn surfaces(root: &Path) -> Vec<PathBuf> {
    let mut out = vec![
        root.join("README.md"),
        root.join("LAUNCH.md"),
        root.join("llms.txt"),
    ];
    let mut dirs = vec![root.join("docs/public"), root.join("docs/site/src/content")];
    while let Some(d) = dirs.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                dirs.push(p);
            } else if p.extension().is_some_and(|x| x == "md" || x == "mdx") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// Scans the public surfaces; fails on any unauthorised claim.
pub fn run(_args: &[String]) -> Result<(), String> {
    let root = crate::workspace_root();
    let prior_art = root.join("docs/prior-art");
    let mut failures = Vec::new();
    let files = surfaces(&root);
    for path in &files {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        for (n, line) in text.lines().enumerate() {
            if is_claim(line)
                && let Some(why) = unauthorised(line, &prior_art)
            {
                let rel = path.strip_prefix(&root).unwrap_or(path).display();
                failures.push(format!("{rel}:{}: {why}\n    {}", n + 1, line.trim()));
            }
        }
    }
    println!(
        "claims-check: {} public files scanned, {} unauthorised claims",
        files.len(),
        failures.len()
    );
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("\n"))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("claims-check-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join("pq.md"),
            "**Search status:** searched 2026-09-26.\n\"first post-quantum L1\" is **forbidden**.\n",
        )
        .unwrap();
        std::fs::write(d.join("lanes.md"), "**Search status:** not searched.\n").unwrap();
        d
    }

    #[test]
    fn ordinary_english_is_not_a_claim() {
        assert!(!is_claim("the first depositor can mint one share"));
        assert!(!is_claim("the only crate that instantiates ml-kem"));
        assert!(is_claim("Maya2C is the first post-quantum L1"));
        assert!(is_claim("the only wallet you need"));
        assert!(is_claim("An unprecedented design"));
    }

    #[test]
    fn a_claim_needs_a_completed_search_that_does_not_forbid_it() {
        let d = dir();
        assert!(
            unauthorised("the first chain", &d)
                .unwrap()
                .contains("cites no")
        );
        assert!(
            unauthorised("the only network (prior-art/nope.md)", &d)
                .unwrap()
                .contains("does not exist")
        );
        assert!(
            unauthorised("the first network with lanes (prior-art/lanes.md)", &d)
                .unwrap()
                .contains("no completed")
        );
        let forbidden =
            unauthorised("Maya2C, the first post-quantum L1 (prior-art/pq.md)", &d).unwrap();
        assert!(forbidden.contains("forbids"), "{forbidden}");
        assert_eq!(
            unauthorised(
                "the first network to ship hybrid signing by default in its RPC (prior-art/pq.md)",
                &d
            ),
            None
        );
        std::fs::remove_dir_all(d).unwrap();
    }
}
