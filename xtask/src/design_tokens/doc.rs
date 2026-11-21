//! The colour table in `docs/design/forged-light-2.md` §3.1 must equal the
//! theme tokens. A design doc that quotes a value the tokens no longer have is
//! how the first Forged Light 2 doc came to describe a palette nobody shipped;
//! this check replaces comparing them by eye.

use std::collections::BTreeMap;

use crate::ordered_json::Json;

/// The heading the table sits under; the section ends at the next heading.
const SECTION: &str = "### 3.1 ";

/// One row per theme role: `` | `--role` | `dark` | `light` | use | ``.
/// Every role must have a row except one composed only of `var()` references
/// in both themes (gradients, chart grid and axis), which the prose describes;
/// a row for one of those must still match.
pub(super) fn mismatches(doc: &str, tokens: &Json) -> Result<Vec<String>, String> {
    let rows = table(doc)?;
    let dark = super::theme(tokens, "dark")?;
    let light = super::theme(tokens, "light")?;
    let mut out = Vec::new();
    let mut by_role = BTreeMap::new();
    for (role, d, l) in &rows {
        if by_role.insert(role.as_str(), (d, l)).is_some() {
            out.push(format!("`--{role}` has two rows"));
        }
        if !dark.iter().any(|(n, _)| n == role) {
            out.push(format!("row for `--{role}`, which is not a theme token"));
        }
    }
    for (role, d) in &dark {
        let l = light
            .iter()
            .find(|(n, _)| n == role)
            .map_or("", |(_, v)| v.as_str());
        match by_role.get(role.as_str()) {
            None if d.contains("var(") && l.contains("var(") => {}
            None => out.push(format!("no row for `--{role}`")),
            Some((rd, rl)) => {
                if *rd != d {
                    out.push(format!(
                        "`--{role}` dark is `{rd}` in the doc, `{d}` in the tokens"
                    ));
                }
                if rl.as_str() != l {
                    out.push(format!(
                        "`--{role}` light is `{rl}` in the doc, `{l}` in the tokens"
                    ));
                }
            }
        }
    }
    Ok(out)
}

/// (role, dark, light) for every table row in the section.
fn table(doc: &str) -> Result<Vec<(String, String, String)>, String> {
    let mut lines = doc.lines().skip_while(|l| !l.starts_with(SECTION));
    if lines.next().is_none() {
        return Err(format!("no `{SECTION}` section"));
    }
    let rows: Vec<_> = lines
        .take_while(|l| !l.starts_with("## ") && !l.starts_with("### "))
        .filter(|l| l.starts_with("| `--"))
        .map(|l| {
            let cells: Vec<&str> = l.split('|').map(|c| c.trim().trim_matches('`')).collect();
            let cell = |i: usize| cells.get(i).copied().unwrap_or_default().to_owned();
            (
                cell(1).trim_start_matches("--").to_owned(),
                cell(2),
                cell(3),
            )
        })
        .collect();
    if rows.is_empty() {
        return Err(format!("the `{SECTION}` section has no `--role` table"));
    }
    Ok(rows)
}
