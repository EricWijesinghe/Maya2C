//! Two palette rules the pair and distance gates cannot express. Each meaning
//! keeps one hue across its tiers and both themes, so a failing value is
//! fixed by lightness rather than by drifting to a neighbouring hue (bronze
//! to copper, red to crimson), and a chart holds at most `max-ink-series`
//! series near the text colour, which read as smudges on paper even when
//! every pair is far apart in `OKLab`.

use maya_design_system::contrast::Vision;

use super::{CHART, THEMES, chart_series, colour, entries, group, theme};
use crate::ordered_json::Json;

const FAMILIES: &str = "hue-families";

/// Below this `OKLCh` chroma a colour is grey and its hue is noise.
const MIN_CHROMA: f64 = 0.03;

/// One printed line of the report, and the failure it carries, if any.
pub(super) struct Line {
    pub(super) text: String,
    pub(super) failure: Option<String>,
}

pub(super) fn report(tokens: &Json) -> Result<Vec<Line>, String> {
    let mut out = hues(tokens)?;
    out.extend(inks(tokens)?);
    Ok(out)
}

fn text<'a>(v: Option<&'a Json>, what: &str) -> Result<&'a str, String> {
    v.and_then(Json::as_str)
        .ok_or_else(|| format!("{what}: a number as a string"))
}

fn number(v: Option<&Json>, what: &str) -> Result<f64, String> {
    text(v, what)?
        .parse::<f64>()
        .ok()
        .filter(|x| x.is_finite() && *x >= 0.0)
        .ok_or_else(|| format!("{what}: a non-negative number as a string"))
}

/// The smallest arc, in degrees, that holds every hue.
pub(super) fn spread(mut hues: Vec<f64>) -> f64 {
    hues.sort_by(f64::total_cmp);
    let (Some(first), Some(last)) = (hues.first(), hues.last()) else {
        return 0.0;
    };
    let gap = hues
        .windows(2)
        .map(|w| w[1] - w[0])
        .fold(360.0 - last + first, f64::max);
    360.0 - gap
}

fn hues(tokens: &Json) -> Result<Vec<Line>, String> {
    let g = group(tokens, FAMILIES)?;
    let max = number(g.get("max-spread"), &format!("{FAMILIES}.max-spread"))?;
    let families = g
        .get("families")
        .ok_or_else(|| format!("{FAMILIES}.families: missing"))?;
    let themes = THEMES
        .iter()
        .map(|&t| theme(tokens, t).map(|d| (t, d)))
        .collect::<Result<Vec<_>, _>>()?;
    let mut out = Vec::new();
    for (name, roles) in entries(families, FAMILIES)? {
        let roles = roles
            .as_array()
            .filter(|r| !r.is_empty())
            .ok_or_else(|| format!("{FAMILIES}.{name}: a list of roles"))?;
        let mut found = Vec::new();
        for (t, decls) in &themes {
            for r in roles {
                let r = r
                    .as_str()
                    .ok_or_else(|| format!("{FAMILIES}.{name}: role names"))?;
                let [_, a, b] = colour(decls, t, r)?.oklab(Vision::Normal);
                let chroma = a.hypot(b);
                if chroma < MIN_CHROMA {
                    return Err(format!(
                        "{FAMILIES}.{name}: {t} {r} has chroma {chroma:.3}, too grey to hold a hue"
                    ));
                }
                found.push(b.atan2(a).to_degrees().rem_euclid(360.0));
            }
        }
        let s = spread(found);
        let text = format!("hue family {name:<8} spread {s:5.1} degrees (max {max})");
        let failure = (s > max).then(|| format!("{FAMILIES}: {text}"));
        out.push(Line { text, failure });
    }
    Ok(out)
}

fn inks(tokens: &Json) -> Result<Vec<Line>, String> {
    let g = group(tokens, CHART)?;
    let what = format!("{CHART}.ink-lightness");
    let ink = g
        .get("ink-lightness")
        .ok_or_else(|| format!("{what}: missing"))?;
    let max_what = format!("{CHART}.max-ink-series");
    let max: usize = text(g.get("max-ink-series"), &max_what)?
        .parse()
        .map_err(|_| format!("{max_what}: a whole number as a string"))?;
    let series = chart_series(tokens)?;
    let mut out = Vec::new();
    for t in THEMES {
        let limit = number(ink.get(t), &format!("{what}.{t}"))?;
        let decls = theme(tokens, t)?;
        let mut found = Vec::new();
        for s in &series {
            let l = colour(&decls, t, s)?.oklab(Vision::Normal)[0];
            // On the dark ground the ink is near white; on paper, near black.
            let near_ink = if t == "dark" { l > limit } else { l < limit };
            if near_ink {
                found.push(s.as_str());
            }
        }
        let text = format!(
            "{t:>5} chart ink series (OKLab L {} {limit}): {} of max {max} [{}]",
            if t == "dark" { ">" } else { "<" },
            found.len(),
            found.join(", ")
        );
        let failure = (found.len() > max).then(|| format!("{CHART}: {text}"));
        out.push(Line { text, failure });
    }
    Ok(out)
}
