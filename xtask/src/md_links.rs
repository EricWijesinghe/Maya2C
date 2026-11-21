//! `cargo xtask md-links` — every relative link in a tracked Markdown file must
//! name a file or directory that exists (issue #530).
//!
//! `site-links.yml` checks the built website and nothing checks `docs/`, so two
//! ADRs (081 and 082, issue #526) shipped links to files that were renamed.
//! This walks `git ls-files '*.md'` and checks every `](target)` whose target
//! is a path inside the repository. Not checked, on purpose: URLs and `mailto:`
//! (a network check belongs to `site-links`), `#anchor`-only links, and
//! absolute `/paths`, which the docs site resolves against its own root.
//! Fenced code blocks are skipped, because a Markdown example is not a link.

use std::path::Path;
use std::process::Command;
use std::sync::LazyLock;

use regex::Regex;

static LINK: LazyLock<Regex> = LazyLock::new(|| {
    // `[text](target)` and `![alt](target)`: `<target with spaces>` or a bare
    // target; a trailing "title" is never reached because the bare form stops at whitespace.
    Regex::new(r"\]\(\s*(?:<([^>]+)>|([^)\s]+))").expect("static pattern compiles")
});

/// Every link target in `text`, outside fenced code blocks, in document order.
pub fn links(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut fenced = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        out.extend(
            LINK.captures_iter(line)
                .filter_map(|c| c.get(1).or(c.get(2)))
                .map(|m| m.as_str().to_string()),
        );
    }
    out
}

/// The repository-relative file a link points at, or `None` when the link is
/// not a path inside the repository (see the module doc for what is skipped).
pub fn local_path(target: &str) -> Option<&str> {
    let path = target.split('#').next().unwrap_or("");
    let external = path.contains("://") || path.starts_with("mailto:");
    if path.is_empty() || external || path.starts_with('/') {
        None
    } else {
        Some(path)
    }
}

/// `file -> link` for each relative link in `text` (a file at `rel`, relative
/// to `root`) whose target does not exist.
pub fn broken_in(root: &Path, rel: &str, text: &str) -> Vec<String> {
    let dir = Path::new(rel).parent().unwrap_or(Path::new(""));
    links(text)
        .into_iter()
        .filter(|t| local_path(t).is_some_and(|p| !root.join(dir).join(p).exists()))
        .map(|t| format!("{rel} -> {t}"))
        .collect()
}

fn tracked_markdown(root: &Path) -> Result<Vec<String>, String> {
    let out = Command::new("git")
        .args(["ls-files", "-z", "*.md"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("git ls-files: {e}"))?;
    if !out.status.success() {
        return Err("git ls-files *.md failed".into());
    }
    Ok(out
        .stdout
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect())
}

/// Checks every tracked Markdown file; fails listing each broken link.
pub fn run(_args: &[String]) -> Result<(), String> {
    let root = crate::workspace_root();
    let files = tracked_markdown(&root)?;
    let mut broken = Vec::new();
    for rel in &files {
        let text =
            std::fs::read_to_string(root.join(rel)).map_err(|e| format!("read {rel}: {e}"))?;
        broken.extend(broken_in(&root, rel, &text));
    }
    if broken.is_empty() {
        println!(
            "md-links: {} tracked Markdown files, no broken relative links",
            files.len()
        );
        return Ok(());
    }
    for b in &broken {
        println!("md-links: broken {b}");
    }
    Err(format!(
        "md-links: {} broken relative link(s)",
        broken.len()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("md-links-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("docs")).expect("scratch dir");
        dir
    }

    #[test]
    fn extracts_plain_image_titled_and_angle_links() {
        let text = "See [a](docs/a.md), ![img](x.png \"t\") and [b](<sp ace.md>).";
        assert_eq!(links(text), vec!["docs/a.md", "x.png", "sp ace.md"]);
    }

    #[test]
    fn skips_fenced_code_blocks() {
        let text = "```\n[no](missing.md)\n```\n[yes](here.md)";
        assert_eq!(links(text), vec!["here.md"]);
    }

    #[test]
    fn local_path_keeps_only_repository_paths() {
        assert_eq!(local_path("ADR-036.md#anchor"), Some("ADR-036.md"));
        assert_eq!(local_path("#only-anchor"), None);
        assert_eq!(local_path("https://example.com/x"), None);
        assert_eq!(local_path("mailto:a@b.c"), None);
        assert_eq!(local_path("/absolute/site/path"), None);
    }

    #[test]
    fn reports_missing_target_and_passes_existing_one() {
        let root = scratch("broken");
        std::fs::write(root.join("docs/exists.md"), "x").expect("write");
        let text = "[ok](exists.md) and [gone](missing.md)";
        let found = broken_in(&root, "docs/index.md", text);
        assert_eq!(found, vec!["docs/index.md -> missing.md"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn resolves_parent_directory_links() {
        let root = scratch("parent");
        std::fs::write(root.join("README.md"), "x").expect("write");
        assert!(broken_in(&root, "docs/a.md", "[r](../README.md)").is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
}
