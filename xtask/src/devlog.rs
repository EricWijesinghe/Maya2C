//! `cargo xtask devlog` — the repository's public record, built only from what
//! already exists: merged pull requests and the ecosystem catalog.
//!
//! - `changelog` writes `CHANGELOG.md` from every pull request merged into
//!   `master` (read through `gh`), one section per UTC merge day.
//! - `week [--since YYYY-MM-DD]` prints a dev-log post body for the pull
//!   requests merged since that date (default: seven days ago). It posts
//!   nothing: a person reads it and posts it to Discussions.
//! - `roadmap [--check]` writes `docs/ecosystem/ROADMAP.md` from the wave
//!   column of `docs/ecosystem/CATALOG.md`; `--check` fails when they differ.
//!
//! Every line of output is a merged pull request or a catalog row. Nothing is
//! typed in, so the record cannot claim activity that did not happen.

use std::fmt::Write as _;
use std::fs;
use std::process::Command;

use serde::Deserialize;

const USAGE: &str = "usage: cargo xtask devlog changelog | week [--since YYYY-MM-DD] | roadmap [--check]";
/// Above the number of merged PRs today; reaching it is an error, not a silent cut.
const PR_LIMIT: usize = 5000;
const CATALOG: &str = "docs/ecosystem/CATALOG.md";
const ROADMAP: &str = "docs/ecosystem/ROADMAP.md";
const DAYS_PER_WEEK: i64 = 7;

/// Short wave rules, restated from CATALOG.md's Waves table for a public reader.
const WAVES: [(&str, &str); 5] = [
    ("W0", "Now, before genesis. Non-consensus work only; the frozen node binary is not touched."),
    ("W1", "Mainnet to +3 months. User apps and builder tools; no consensus changes."),
    ("W2", "+3 to +6 months. Commerce, identity and operator tools; consensus items only at a written activation height."),
    ("W3", "+6 to +12 months. The AI network, bridges and the fields."),
    ("W4", "Later. Needs research, legal work or hardware first."),
];

#[derive(Deserialize)]
struct Label {
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Pr {
    number: u64,
    title: String,
    url: String,
    labels: Vec<Label>,
    merged_at: String,
}

pub fn run(args: &[String]) -> Result<(), String> {
    let root = crate::workspace_root();
    match args {
        [cmd] if cmd == "changelog" => {
            let text = render_changelog(&merged_prs(None)?);
            fs::write(root.join("CHANGELOG.md"), text).map_err(|e| format!("CHANGELOG.md: {e}"))?;
            println!("wrote CHANGELOG.md");
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "week" => {
            let since = match rest {
                [] => (chrono::Local::now().date_naive() - chrono::Duration::days(DAYS_PER_WEEK)).to_string(),
                [flag, date] if flag == "--since" && is_date(date) => date.clone(),
                _ => return Err(USAGE.into()),
            };
            let prs = merged_prs(Some(&format!("merged:>={since}")))?;
            print!("{}", render_week(&since, &prs));
            Ok(())
        }
        [cmd, flags @ ..] if cmd == "roadmap" => {
            let check = match flags {
                [] => false,
                [flag] if flag == "--check" => true,
                _ => return Err(USAGE.into()),
            };
            let catalog = fs::read_to_string(root.join(CATALOG)).map_err(|e| format!("{CATALOG}: {e}"))?;
            let text = render_roadmap(&catalog_rows(&catalog));
            let path = root.join(ROADMAP);
            if check {
                let current = fs::read_to_string(&path).unwrap_or_default().replace("\r\n", "\n");
                if current != text {
                    return Err(format!("{ROADMAP} is stale: run cargo xtask devlog roadmap"));
                }
                println!("{ROADMAP} matches {CATALOG}");
                return Ok(());
            }
            fs::write(&path, text).map_err(|e| format!("{ROADMAP}: {e}"))?;
            println!("wrote {ROADMAP}");
            Ok(())
        }
        _ => Err(USAGE.into()),
    }
}

fn is_date(s: &str) -> bool {
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok()
}

fn merged_prs(search: Option<&str>) -> Result<Vec<Pr>, String> {
    let limit = PR_LIMIT.to_string();
    let mut cmd = Command::new("gh");
    cmd.args(["pr", "list", "--state", "merged", "--base", "master", "--limit", &limit]);
    cmd.args(["--json", "number,title,url,labels,mergedAt"]);
    if let Some(query) = search {
        cmd.args(["--search", query]);
    }
    let out = cmd.output().map_err(|e| format!("gh: {e} (install the GitHub CLI and run `gh auth login`)"))?;
    if !out.status.success() {
        return Err(format!("gh pr list failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let mut prs: Vec<Pr> = serde_json::from_slice(&out.stdout).map_err(|e| format!("gh output: {e}"))?;
    if prs.len() >= PR_LIMIT {
        return Err(format!("{PR_LIMIT} or more merged PRs: raise PR_LIMIT in xtask/src/devlog.rs"));
    }
    prs.sort_by(|a, b| b.merged_at.cmp(&a.merged_at).then(b.number.cmp(&a.number)));
    Ok(prs)
}

/// A title as Markdown: `@name` would notify someone when the text is posted,
/// so a token holding `@` goes in backticks; `<` is escaped so it stays text.
fn safe_title(title: &str) -> String {
    let words: Vec<String> = title
        .split(' ')
        .map(|w| if w.contains('@') { format!("`{}`", w.replace('`', "")) } else { w.replace('<', "&lt;") })
        .collect();
    words.join(" ")
}

fn entry(pr: &Pr) -> String {
    let mut line = format!("- {} ([#{}]({}))", safe_title(&pr.title), pr.number, pr.url);
    for label in &pr.labels {
        let _ = write!(line, " `{}`", label.name);
    }
    line.push('\n');
    line
}

fn render_changelog(prs: &[Pr]) -> String {
    let mut out = String::from(
        "# Changelog\n\n\
         Every pull request merged into `master`, newest first, grouped by the UTC day it\n\
         merged. Generated by `cargo xtask devlog changelog` from GitHub's record of\n\
         merged pull requests; no line is written by hand. Work committed before the\n\
         repository became pull-request-only is in `git log`, not here.\n",
    );
    let mut day = "";
    for pr in prs {
        let merged = pr.merged_at.get(..10).unwrap_or(&pr.merged_at);
        if merged != day {
            day = merged;
            let _ = write!(out, "\n## {day}\n\n");
        }
        out.push_str(&entry(pr));
    }
    out
}

/// The conventional-commit type of a title, ignoring a leading `[tag]`.
fn kind(title: &str) -> &'static str {
    let t = title.trim_start();
    let t = if t.starts_with('[') { t.split_once(']').map_or(t, |(_, r)| r.trim_start()) } else { t };
    let head = t.split(':').next().unwrap_or("");
    let ty = head.split('(').next().unwrap_or("").trim_end_matches('!');
    match ty {
        "feat" => "New",
        "fix" => "Fixed",
        "docs" => "Documentation",
        "test" => "Tests",
        "perf" => "Performance",
        _ => "Other changes",
    }
}

fn render_week(since: &str, prs: &[Pr]) -> String {
    let mut out = format!(
        "## Maya2C dev log: merged since {since}\n\n\
         {} pull requests merged into `master`. Every line links to the pull request \
         it describes. Where the project stands: \
         [README](https://github.com/EricWijesinghe/Maya2C#readme).\n",
        prs.len()
    );
    for group in ["New", "Fixed", "Performance", "Tests", "Documentation", "Other changes"] {
        let items: Vec<&Pr> = prs.iter().filter(|p| kind(&p.title) == group).collect();
        if items.is_empty() {
            continue;
        }
        let _ = write!(out, "\n### {group} ({})\n\n", items.len());
        for p in items {
            out.push_str(&entry(p));
        }
    }
    out
}

struct Row {
    section: String,
    id: String,
    what: String,
    size: String,
    wave: String,
}

/// Rows of the lettered sections (`## A. ...`) that carry a wave cell.
fn catalog_rows(catalog: &str) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut section: Option<String> = None;
    for line in catalog.lines() {
        if let Some(h) = line.strip_prefix("## ") {
            let lettered = h.chars().next().is_some_and(|c| c.is_ascii_uppercase()) && h.get(1..3) == Some(". ");
            section = lettered.then(|| h.to_string());
            continue;
        }
        let (Some(sec), Some(body)) = (&section, line.strip_prefix('|')) else { continue };
        let cells: Vec<&str> = body.trim_end_matches('|').split('|').map(str::trim).collect();
        let Some(w) = cells.iter().position(|c| c.len() == 2 && c.starts_with('W') && c.as_bytes()[1].is_ascii_digit())
        else {
            continue;
        };
        if w < 2 {
            continue;
        }
        let size = cells[w - 1];
        rows.push(Row {
            section: sec.clone(),
            id: cells[0].trim_matches('`').to_string(),
            what: cells[1].to_string(),
            size: if ["S", "M", "L"].contains(&size) { size.to_string() } else { String::new() },
            wave: cells[w].to_string(),
        });
    }
    rows
}

fn render_roadmap(rows: &[Row]) -> String {
    let mut out = String::from(
        "# Roadmap\n\n\
         <!-- Generated by `cargo xtask devlog roadmap` from CATALOG.md. Do not edit by hand. -->\n\n\
         What gets built, in which order. This is planned work, not a claim that\n\
         anything works: what is built and tested is in\n\
         [`features.toml`](../../features.toml). Each entry, its dependencies and its\n\
         gate are in [CATALOG.md](CATALOG.md); CI fails when this page and the catalog\n\
         differ. W1 onwards count from the mainnet launch date.\n\n\
         Size: S is up to a few days, M one to three weeks, L longer.\n",
    );
    for (wave, rule) in WAVES {
        let items: Vec<&Row> = rows.iter().filter(|r| r.wave == wave).collect();
        let _ = write!(out, "\n## {wave} ({} entries)\n\n{rule}\n", items.len());
        let mut section = "";
        for r in items {
            if r.section != section {
                section = &r.section;
                let _ = write!(out, "\n**{section}**\n\n");
            }
            let size = if r.size.is_empty() { String::new() } else { format!(" ({})", r.size) };
            let _ = writeln!(out, "- `{}`{size}: {}", r.id, r.what);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr(number: u64, title: &str, merged_at: &str) -> Pr {
        Pr {
            number,
            title: title.into(),
            url: format!("https://example.invalid/{number}"),
            labels: vec![Label { name: "docs".into() }],
            merged_at: merged_at.into(),
        }
    }

    #[test]
    fn a_mention_in_a_title_cannot_notify_anyone() {
        assert_eq!(safe_title("bump @types/node in <site>"), "bump `@types/node` in &lt;site>");
    }

    #[test]
    fn titles_group_by_conventional_type_past_a_leading_tag() {
        assert_eq!(kind("[re-freeze] fix: proof order"), "Fixed");
        assert_eq!(kind("feat(wallet)!: clear signing"), "New");
        assert_eq!(kind("Bump serde"), "Other changes");
    }

    #[test]
    fn changelog_has_one_section_per_merge_day() {
        let prs = [pr(3, "docs: c", "2026-11-13T09:00:00Z"), pr(2, "fix: b", "2026-11-13T01:00:00Z"), pr(1, "feat: a", "2026-11-12T23:00:00Z")];
        let text = render_changelog(&prs);
        assert_eq!(text.matches("\n## 2026-11-13\n").count(), 1);
        assert!(text.contains("\n## 2026-11-12\n\n- feat: a ([#1](https://example.invalid/1)) `docs`\n"));
    }

    #[test]
    fn week_counts_every_pr_once() {
        let prs = [pr(2, "fix: b", "2026-11-13T01:00:00Z"), pr(1, "chore: a", "2026-11-12T01:00:00Z")];
        let text = render_week("2026-11-07", &prs);
        assert!(text.contains("2 pull requests merged"));
        assert!(text.contains("### Fixed (1)") && text.contains("### Other changes (1)"));
    }

    #[test]
    fn roadmap_reads_lettered_sections_and_skips_the_waves_table() {
        let catalog = "# C\n\n## A. Apps\n\n| id | What | Builds on | Gate | Size | Wave |\n|---|---|---|---|---|---|\n\
                       | wallet | A wallet | x | none | S | W0 |\n| shield | Guard | x | none | M | W1 | R0 |\n\
                       ## H. Vision\n\n| id | Direction | Builds on | Gate |\n| os | An OS | x | y |\n\
                       ## Waves\n\n| Wave | When | Rule |\n| W0 | Now | rule |\n";
        let rows = catalog_rows(catalog);
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[1].id.as_str(), rows[1].size.as_str(), rows[1].wave.as_str()), ("shield", "M", "W1"));
        let text = render_roadmap(&rows);
        assert!(text.contains("## W0 (1 entries)") && text.contains("- `wallet` (S): A wallet"));
        assert!(text.contains("## W4 (0 entries)"));
    }
}
