//! `cargo xtask slo-check` — every SLO has its four companions (Master Prompt 19 §2).
//!
//! For each row of the table in `docs/SLO.md`:
//!
//! - **metric**: the name is registered by the node's metrics module
//!   (`crates/node/src/metrics/mod.rs`, prefix `maya_`, counters exported with
//!   `_total`). A metric an SLO names but the node never emits is a
//!   dashboard of nothing;
//! - **dashboard**: the JSON file exists and has a panel whose `id` or
//!   `title` slug matches the anchor;
//! - **alert**: the rules file exists and defines `alert: <anchor>`;
//! - **runbook**: the file exists.
//!
//! Fails if any row lacks any of the four, and names each gap.

use std::collections::BTreeSet;
use std::path::Path;

/// Metric names the node registers, with the prefix and counter suffix applied.
///
/// Each collector is declared (`let x = Counter::default();`) on the line
/// before its `registry.register("name", …)`; a `Counter` (or a `Family` of
/// counters) is exported with `_total`, as `OpenMetrics` requires.
fn node_metrics(root: &Path) -> Result<BTreeSet<String>, String> {
    let src = std::fs::read_to_string(root.join("crates/node/src/metrics/mod.rs"))
        .map_err(|e| e.to_string())?;
    let mut out = BTreeSet::new();
    let mut from = 0;
    while let Some(i) = src[from..].find("registry.register(").map(|i| i + from) {
        from = i + 1;
        let after = &src[i + "registry.register(".len()..];
        let Some(open) = after.find('"') else { break };
        let Some(close) = after[open + 1..].find('"') else {
            break;
        };
        let name = &after[open + 1..open + 1 + close];
        let declaration = src[..i].trim_end().rsplit(';').nth(1).unwrap_or("");
        let base = format!("maya_{name}");
        out.insert(if declaration.contains("Counter") {
            format!("{base}_total")
        } else {
            base
        });
    }
    Ok(out)
}

fn slug(s: &str) -> String {
    s.to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

fn check_row(root: &Path, metrics: &BTreeSet<String>, cells: &[&str]) -> Vec<String> {
    let mut gaps = Vec::new();
    let metric = cells[3].trim_matches('`');
    if !metrics.contains(metric) {
        gaps.push(format!("metric `{metric}` is not registered by the node"));
    }
    let (dash_file, panel) = cells[4].split_once('#').unwrap_or((cells[4], ""));
    match std::fs::read_to_string(root.join(dash_file)) {
        Ok(text) => {
            let hit = text.contains(&format!("\"id\": \"{panel}\""))
                || text
                    .lines()
                    .any(|l| l.contains("\"title\"") && slug(l).contains(&slug(panel)));
            if !hit {
                gaps.push(format!("dashboard {dash_file} has no panel `{panel}`"));
            }
        }
        Err(_) => gaps.push(format!("dashboard {dash_file} does not exist")),
    }
    let (alert_file, alert) = cells[5].split_once('#').unwrap_or((cells[5], ""));
    match std::fs::read_to_string(root.join(alert_file)) {
        Ok(text) if text.contains(&format!("alert: {alert}")) => {}
        Ok(_) => gaps.push(format!("alert rules {alert_file} define no `{alert}`")),
        Err(_) => gaps.push(format!("alert rules {alert_file} do not exist")),
    }
    if !root.join(cells[6]).exists() {
        gaps.push(format!("runbook {} does not exist", cells[6]));
    }
    gaps
}

pub fn run(args: &[String]) -> Result<(), String> {
    let quiet = args.iter().any(|a| a == "--quiet");
    let root = crate::workspace_root();
    let metrics = node_metrics(&root)?;
    let slo = std::fs::read_to_string(root.join("docs/SLO.md")).map_err(|e| e.to_string())?;
    let (mut rows, mut failing) = (0, 0);
    for line in slo
        .lines()
        .filter(|l| l.starts_with("| ") && !l.starts_with("| ID") && !l.starts_with("|--"))
    {
        let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
        if cells.len() < 7 {
            continue;
        }
        rows += 1;
        let gaps = check_row(&root, &metrics, &cells);
        if gaps.is_empty() {
            if !quiet {
                println!("ok    {}", cells[0]);
            }
        } else {
            failing += 1;
            if !quiet {
                println!("GAP   {}: {}", cells[0], gaps.join("; "));
            }
        }
    }
    if !quiet {
        println!(
            "\n{rows} SLOs, {} with all four companions, {failing} with gaps",
            rows - failing
        );
    }
    if failing > 0 {
        return Err(format!(
            "{failing} SLOs lack a metric, dashboard panel, alert or runbook"
        ));
    }
    Ok(())
}
