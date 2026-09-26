//! `cargo xtask spec-coverage` — which spec rules have vectors (Master Prompt 15 §2).
//!
//! Reads every `- **ID** …` rule line in `spec/*.md` and every case in
//! `spec/tests/*.json`. A case counts as a positive vector for each rule it
//! names when it expects success (or states a computed value), and as a
//! negative vector when it expects an error. A rule is covered when it has a
//! positive vector and, unless it is marked *(positive only: …)*, a negative
//! one. Networking rules (`NET-*`) are not consensus and never count as gaps.
//!
//! Default: print the table and the gap list, exit 0 — the brief allows gaps
//! to be listed. `--strict` exits non-zero on any consensus gap.

use std::collections::BTreeMap;
use std::path::Path;

#[derive(Default)]
struct Rule {
    section: String,
    positive_only: bool,
    /// `(external vectors: <path>)`: evidence outside spec/tests/, counted
    /// only if the path exists.
    external: Option<String>,
    pos: usize,
    neg: usize,
}

fn rules(spec: &Path) -> Result<BTreeMap<String, Rule>, String> {
    let mut out = BTreeMap::new();
    let mut files: Vec<_> = std::fs::read_dir(spec)
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .collect();
    files.sort();
    for path in files
        .iter()
        .filter(|p| p.extension().is_some_and(|x| x == "md"))
    {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let section = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        for line in text.lines() {
            let Some(rest) = line.strip_prefix("- **") else {
                continue;
            };
            let Some((id, body)) = rest.split_once("**") else {
                continue;
            };
            let external = body
                .split_once("(external vectors: ")
                .and_then(|(_, r)| r.split_once(')'))
                .map(|(p, _)| p.trim().to_string());
            let rule = Rule {
                section: section.clone(),
                positive_only: body.contains("(positive only"),
                external,
                ..Rule::default()
            };
            if out.insert(id.to_string(), rule).is_some() {
                return Err(format!("rule {id} is defined twice (IDs are never reused)"));
            }
        }
    }
    Ok(out)
}

fn count_vectors(tests: &Path, rules: &mut BTreeMap<String, Rule>) -> Result<usize, String> {
    let mut cases = 0;
    for entry in std::fs::read_dir(tests)
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
    {
        let path = entry.path();
        if path.extension().is_none_or(|x| x != "json") {
            continue;
        }
        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).map_err(|e| e.to_string())?)
                .map_err(|e| format!("{}: {e}", path.display()))?;
        let Some(list) = doc["cases"].as_array() else {
            continue;
        };
        for case in list {
            cases += 1;
            let negative = case["expect"]["result"] == "error";
            for id in case["rules"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str())
            {
                let rule = rules.get_mut(id).ok_or_else(|| {
                    format!(
                        "{}: case {} names unknown rule {id}",
                        path.display(),
                        case["id"]
                    )
                })?;
                if negative {
                    rule.neg += 1;
                } else {
                    rule.pos += 1;
                }
            }
        }
    }
    Ok(cases)
}

pub fn run(args: &[String]) -> Result<(), String> {
    let strict = args.iter().any(|a| a == "--strict");
    let root = crate::workspace_root().join("spec");
    let mut rules = rules(&root)?;
    let cases = count_vectors(&root.join("tests"), &mut rules)?;
    println!(
        "{:<8} {:<20} {:>4} {:>4}  status",
        "rule", "section", "+", "-"
    );
    let mut gaps = Vec::new();
    let ws = crate::workspace_root();
    for (id, r) in &rules {
        let consensus = !id.starts_with("NET-");
        let external_ok = r.external.as_ref().is_some_and(|p| ws.join(p).exists());
        let status = match (r.pos > 0, r.neg > 0 || r.positive_only) {
            _ if !consensus => "not consensus",
            _ if external_ok => "covered (external vectors)",
            _ if r.external.is_some() => "GAP: external vectors path missing",
            (true, true) if r.positive_only => "covered (positive only)",
            (true, true) => "covered",
            (true, false) => "GAP: no negative vector",
            (false, _) => "GAP: no vector",
        };
        if status.starts_with("GAP") {
            gaps.push(id.as_str());
        }
        println!(
            "{id:<8} {:<20} {:>4} {:>4}  {status}",
            r.section, r.pos, r.neg
        );
    }
    let covered = rules.len() - gaps.len() - rules.keys().filter(|k| k.starts_with("NET-")).count();
    println!(
        "\n{} rules, {cases} vectors; {covered} consensus rules covered, {} gaps: {}",
        rules.len(),
        gaps.len(),
        gaps.join(" ")
    );
    if strict && !gaps.is_empty() {
        return Err(format!(
            "{} consensus rules without full vector coverage",
            gaps.len()
        ));
    }
    Ok(())
}
