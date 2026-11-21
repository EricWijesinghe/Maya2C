//! Distance gates: chart series that stay apart under colour-blind vision,
//! and meaning roles that never share a colour (`OKLab`, ADR-136).

use maya_design_system::contrast::{Vision, distance};

use super::{THEMES, colour, group, theme};
use crate::ordered_json::Json;

/// The two distinctness groups in `tokens.json`.
pub(super) const CHART: &str = "chart-distinctness";
pub(super) const ROLES: &str = "role-distinctness";

/// A list of at least two role names.
fn names(v: &Json, what: &str) -> Result<Vec<String>, String> {
    let list = v
        .as_array()
        .ok_or_else(|| format!("{what}: expected an array of role names"))?;
    if list.len() < 2 {
        return Err(format!("{what}: at least two roles"));
    }
    list.iter()
        .map(|s| {
            s.as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("{what}: role names"))
        })
        .collect()
}

/// Role sets, each with the minimum it carries of its own, if any.
type Sets = Vec<(Vec<String>, Option<f64>)>;

/// An array of role sets under `group_name.key`: each a name list, or an
/// object `{ "roles": [...], "min-distance": "..." }` with its own minimum.
fn name_sets(tokens: &Json, group_name: &str, key: &str) -> Result<Sets, String> {
    let what = format!("{group_name}.{key}");
    group(tokens, group_name)?
        .get(key)
        .and_then(Json::as_array)
        .ok_or_else(|| format!("{what}: expected an array"))?
        .iter()
        .map(|set| match set {
            Json::Object(_) => {
                let roles = set
                    .get("roles")
                    .ok_or_else(|| format!("{what}: an object set needs `roles`"))?;
                Ok((names(roles, &what)?, Some(positive(set, &what)?)))
            }
            _ => Ok((names(set, &what)?, None)),
        })
        .collect()
}

pub(super) fn chart_series(tokens: &Json) -> Result<Vec<String>, String> {
    let what = format!("{CHART}.series");
    names(
        group(tokens, CHART)?
            .get("series")
            .ok_or_else(|| format!("{what}: missing"))?,
        &what,
    )
}

/// `obj.min-distance`, a positive number written as a string.
fn positive(obj: &Json, what: &str) -> Result<f64, String> {
    obj.get("min-distance")
        .and_then(Json::as_str)
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|m| m.is_finite() && *m > 0.0)
        .ok_or_else(|| format!("{what}.min-distance: a positive number as a string"))
}

pub(super) fn min_distance(tokens: &Json, group_name: &str) -> Result<f64, String> {
    positive(group(tokens, group_name)?, group_name)
}

/// The closest two roles of one set in one theme under one vision model, and
/// the minimum that set must keep.
#[derive(Debug)]
pub(super) struct Closest {
    pub(super) group: &'static str,
    pub(super) theme: &'static str,
    pub(super) vision: Vision,
    pub(super) a: String,
    pub(super) b: String,
    pub(super) distance: f64,
    pub(super) min: f64,
}

impl std::fmt::Display for Closest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:>5} {} {:<12} closest {} / {} OKLab {:.3} (min {})",
            self.theme,
            self.group,
            self.vision.name(),
            self.a,
            self.b,
            self.distance,
            self.min
        )
    }
}

/// The closest pair of `set`, per theme and per vision model in `visions`.
fn closest(
    group_name: &'static str,
    tokens: &Json,
    (set, min): (&[String], f64),
    visions: &[Vision],
) -> Result<Vec<Closest>, String> {
    let mut out = Vec::new();
    for theme_name in THEMES {
        let roles = theme(tokens, theme_name)?;
        let colours = set
            .iter()
            .map(|s| colour(&roles, theme_name, s))
            .collect::<Result<Vec<_>, _>>()?;
        for &vision in visions {
            let mut worst: Option<Closest> = None;
            for i in 0..set.len() {
                for j in i + 1..set.len() {
                    let d = distance(colours[i], colours[j], vision);
                    if worst.as_ref().is_none_or(|w| d < w.distance) {
                        worst = Some(Closest {
                            group: group_name,
                            theme: theme_name,
                            vision,
                            a: set[i].clone(),
                            b: set[j].clone(),
                            distance: d,
                            min,
                        });
                    }
                }
            }
            out.extend(worst);
        }
    }
    Ok(out)
}

pub(super) fn chart_worst(tokens: &Json) -> Result<Vec<Closest>, String> {
    let min = min_distance(tokens, CHART)?;
    closest(CHART, tokens, (&chart_series(tokens)?, min), &Vision::ALL)
}

/// Meaning roles: each set under normal vision, each cvd pair under every
/// model.
pub(super) fn role_worst(tokens: &Json) -> Result<Vec<Closest>, String> {
    let min = min_distance(tokens, ROLES)?;
    let mut out = Vec::new();
    for (set, own) in name_sets(tokens, ROLES, "sets")? {
        out.extend(closest(
            ROLES,
            tokens,
            (&set, own.unwrap_or(min)),
            &[Vision::Normal],
        )?);
    }
    for (pair, own) in name_sets(tokens, ROLES, "cvd-pairs")? {
        out.extend(closest(
            ROLES,
            tokens,
            (&pair, own.unwrap_or(min)),
            &Vision::ALL,
        )?);
    }
    Ok(out)
}
