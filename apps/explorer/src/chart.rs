//! Inline SVG charts.
//!
//! Each chart is a pure function from data to markup. The same input gives the
//! same bytes, so the output can be pinned by a fixture test and screenshot-
//! diffed in CI. No script runs and no client library loads: a browser draws
//! the `<svg>` as it is, which keeps the explorer inside its CSP.
//!
//! Colours are not in the markup. The stylesheet assigns them by class, so the
//! same chart follows the theme the viewer picked.

use std::fmt::Write;

use crate::model::IndexedBlock;

/// Drawing width in SVG units. The viewBox scales it to the column.
const WIDTH: f64 = 640.0;
/// Drawing height in SVG units.
const HEIGHT: f64 = 200.0;
/// Margin inside the viewBox, so a peak bar does not touch the edge.
const PAD: f64 = 8.0;

/// One labelled value in a series.
#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    /// Shown as the hover title of its bar or point.
    pub label: String,
    /// The plotted value. Non-finite and negative values plot as zero.
    pub value: f64,
}

/// Vertical bars, one per sample, scaled to the largest value.
#[must_use]
pub fn bar_chart(samples: &[Sample], title: &str) -> String {
    if samples.is_empty() {
        return empty(title);
    }
    let values = clean(samples);
    let peak = peak(&values);
    let slot = (WIDTH - 2.0 * PAD) / samples.len() as f64;
    let bar_width = (slot * 0.8).max(1.0);

    let mut out = open(title);
    for (i, (sample, value)) in samples.iter().zip(&values).enumerate() {
        let height = scaled(*value, peak);
        let x = PAD + slot * i as f64 + (slot - bar_width) / 2.0;
        let y = HEIGHT - PAD - height;
        let _ = write!(
            out,
            "<rect class=\"bar\" x=\"{x:.1}\" y=\"{y:.1}\" width=\"{bar_width:.1}\" height=\"{height:.1}\"><title>{}</title></rect>",
            escape(&sample.label)
        );
    }
    out.push_str("</svg>");
    out
}

/// A polyline through the samples, oldest first, scaled from zero to the peak
/// so the rise is not exaggerated by a truncated axis.
#[must_use]
pub fn line_chart(samples: &[Sample], title: &str) -> String {
    if samples.is_empty() {
        return empty(title);
    }
    let values = clean(samples);
    let peak = peak(&values);
    let count = samples.len();

    let points: Vec<(f64, f64)> = values
        .iter()
        .enumerate()
        .map(|(i, value)| {
            let x = if count == 1 {
                WIDTH / 2.0
            } else {
                PAD + (WIDTH - 2.0 * PAD) * i as f64 / (count - 1) as f64
            };
            (x, HEIGHT - PAD - scaled(*value, peak))
        })
        .collect();

    let mut out = open(title);
    let joined: Vec<String> = points
        .iter()
        .map(|(x, y)| format!("{x:.1},{y:.1}"))
        .collect();
    let _ = write!(
        out,
        "<polyline class=\"line\" points=\"{}\"/>",
        joined.join(" ")
    );
    for ((x, y), sample) in points.iter().zip(samples) {
        let _ = write!(
            out,
            "<circle class=\"dot\" cx=\"{x:.1}\" cy=\"{y:.1}\" r=\"3\"><title>{}</title></circle>",
            escape(&sample.label)
        );
    }
    out.push_str("</svg>");
    out
}

/// Seconds between each recent block and its parent, oldest first.
///
/// Input is newest first, as the store returns it. A backwards clock (miner
/// timestamps are only loosely ordered) reads as a zero gap, never a negative
/// one.
#[must_use]
pub fn block_time_series(blocks: &[IndexedBlock]) -> Vec<Sample> {
    let mut series: Vec<Sample> = blocks
        .windows(2)
        .map(|pair| {
            let gap = pair[0].timestamp.saturating_sub(pair[1].timestamp).max(0);
            #[allow(clippy::cast_precision_loss)]
            let value = gap as f64;
            Sample {
                label: format!("block {} · {gap}s after its parent", pair[0].height),
                value,
            }
        })
        .collect();
    series.reverse();
    series
}

/// Transactions in each recent block, oldest first.
#[must_use]
pub fn tx_count_series(blocks: &[IndexedBlock]) -> Vec<Sample> {
    let mut series: Vec<Sample> = blocks
        .iter()
        .map(|block| Sample {
            label: format!("block {} · {} transaction(s)", block.height, block.tx_count),
            value: f64::from(block.tx_count),
        })
        .collect();
    series.reverse();
    series
}

/// Values with non-finite and negative entries replaced by zero.
fn clean(samples: &[Sample]) -> Vec<f64> {
    samples
        .iter()
        .map(|s| {
            if s.value.is_finite() && s.value > 0.0 {
                s.value
            } else {
                0.0
            }
        })
        .collect()
}

/// The largest value, or zero for a flat series. Callers treat zero as "draw
/// nothing", which keeps an all-zero chart from dividing by zero.
fn peak(values: &[f64]) -> f64 {
    values.iter().copied().fold(0.0, f64::max)
}

/// A value's height in the plot area, against the peak.
fn scaled(value: f64, peak: f64) -> f64 {
    if peak <= 0.0 {
        return 0.0;
    }
    value / peak * (HEIGHT - 2.0 * PAD)
}

/// The opening tag and accessible title shared by every chart.
fn open(title: &str) -> String {
    format!(
        "<svg class=\"chart-svg\" viewBox=\"0 0 {WIDTH:.0} {HEIGHT:.0}\" role=\"img\" aria-label=\"{t}\"><title>{t}</title>",
        t = escape(title)
    )
}

/// What a chart shows when it has nothing to plot.
fn empty(title: &str) -> String {
    format!(
        "{}<text class=\"axis\" x=\"{:.0}\" y=\"{:.0}\" text-anchor=\"middle\">No data</text></svg>",
        open(title),
        WIDTH / 2.0,
        HEIGHT / 2.0
    )
}

/// Total stake per validator (own bond plus delegations), in the order given.
///
/// The node sends amounts as exact decimal strings. Parsing them to `f64` is
/// lossy above 2^53, which is acceptable for a bar's height and never used for
/// anything that must be exact; the table beside the chart shows the figures.
#[must_use]
pub fn stake_series(validators: &[custom_l1_node::rpc::validators::ValidatorInfo]) -> Vec<Sample> {
    validators
        .iter()
        .map(|v| {
            let amount = |text: &str| text.parse::<f64>().unwrap_or(0.0);
            Sample {
                label: format!("validator {} · stake {}", short(&v.id), v.self_bond),
                value: amount(&v.self_bond) + amount(&v.delegated),
            }
        })
        .collect()
}

/// A short form of a hex id for a hover label. Character-based, so a malformed
/// id from the node cannot panic on a byte boundary.
fn short(id: &str) -> String {
    id.chars().take(12).collect()
}

/// Escapes text for element content and double-quoted attributes.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}
