//! Inline SVG chart generators: pure functions from data to markup.
//!
//! The output is pinned byte for byte by the fixture test in `fixture_tests.rs`
//! and screenshot-diffed in CI, so these tests check determinism and escaping
//! rather than pixel positions.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_explorer::chart::{Sample, bar_chart, block_time_series, line_chart, tx_count_series};
use maya_explorer::model::IndexedBlock;

fn sample(label: &str, value: f64) -> Sample {
    Sample {
        label: label.to_string(),
        value,
    }
}

fn block(height: i64, timestamp: i64, tx_count: i32) -> IndexedBlock {
    IndexedBlock {
        height,
        id: format!("{height:064x}"),
        prev_hash: format!("{:064x}", height - 1),
        state_root: "ab".repeat(32),
        timestamp,
        nonce: 0,
        difficulty_target: "ff".repeat(32),
        tx_count,
        work: "1".to_string(),
    }
}

/// Every `<rect` height attribute, in order.
fn bar_heights(svg: &str) -> Vec<f64> {
    svg.split("<rect ")
        .skip(1)
        .map(|rest| {
            let start = rest.find("height=\"").expect("height") + "height=\"".len();
            let end = start + rest[start..].find('"').expect("quote");
            rest[start..end].parse::<f64>().expect("number")
        })
        .collect()
}

#[test]
fn the_same_input_gives_the_same_bytes() {
    let data = vec![sample("a", 1.5), sample("b", 3.25), sample("c", 0.0)];
    assert_eq!(
        bar_chart(&data, "Block time"),
        bar_chart(&data, "Block time")
    );
    assert_eq!(line_chart(&data, "Hashrate"), line_chart(&data, "Hashrate"));
}

#[test]
fn an_empty_series_says_so_rather_than_drawing_nothing() {
    let svg = bar_chart(&[], "Block time");
    assert!(svg.starts_with("<svg"), "{svg}");
    assert!(svg.contains("No data"), "{svg}");
    assert!(!svg.contains("<rect"), "{svg}");
}

#[test]
fn the_peak_bar_fills_the_plot_and_others_scale_against_it() {
    let heights = bar_heights(&bar_chart(
        &[sample("a", 2.0), sample("b", 4.0), sample("c", 1.0)],
        "t",
    ));
    assert_eq!(heights.len(), 3);
    assert!((heights[1] - 184.0).abs() < 0.11, "{heights:?}");
    assert!((heights[0] - 92.0).abs() < 0.11, "{heights:?}");
    assert!((heights[2] - 46.0).abs() < 0.11, "{heights:?}");
}

#[test]
fn a_non_finite_or_negative_value_draws_as_zero_height() {
    let heights = bar_heights(&bar_chart(
        &[
            sample("a", f64::NAN),
            sample("b", -3.0),
            sample("c", f64::INFINITY),
            sample("d", 2.0),
        ],
        "t",
    ));
    assert!(heights[0].abs() < 0.11, "{heights:?}");
    assert!(heights[1].abs() < 0.11, "{heights:?}");
    assert!(heights[2].abs() < 0.11, "{heights:?}");
    assert!(heights[3] > 180.0, "{heights:?}");
}

#[test]
fn all_zero_values_draw_flat_bars_without_dividing_by_zero() {
    let heights = bar_heights(&bar_chart(&[sample("a", 0.0), sample("b", 0.0)], "t"));
    assert!(heights.iter().all(|h| h.abs() < 0.11), "{heights:?}");
}

#[test]
fn labels_and_title_are_escaped_into_markup() {
    let svg = bar_chart(&[sample("<script>x</script> & \"q\"", 1.0)], "<b>t</b>");
    assert!(!svg.contains("<script>"), "{svg}");
    assert!(!svg.contains("<b>"), "{svg}");
    assert!(
        svg.contains("&lt;script&gt;x&lt;/script&gt; &amp; &quot;q&quot;"),
        "{svg}"
    );
}

#[test]
fn colours_come_from_css_not_the_markup() {
    // Themes switch by stylesheet; a hard-coded fill would stay dark on the
    // light theme.
    let svg = bar_chart(&[sample("a", 1.0)], "t");
    assert!(!svg.contains("fill=\"#"), "{svg}");
    assert!(!svg.contains("stroke=\"#"), "{svg}");
    let line = line_chart(&[sample("a", 1.0), sample("b", 2.0)], "t");
    assert!(!line.contains("stroke=\"#"), "{line}");
}

#[test]
fn a_line_chart_has_one_point_per_sample() {
    let svg = line_chart(
        &[sample("1", 1.0), sample("2", 2.0), sample("3", 3.0)],
        "Hashrate",
    );
    let points = svg
        .split("points=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("points attribute");
    assert_eq!(points.split_whitespace().count(), 3, "{points}");
    assert_eq!(svg.matches("<circle").count(), 3, "{svg}");
}

#[test]
fn a_single_point_line_is_drawn_not_dropped() {
    let svg = line_chart(&[sample("only", 5.0)], "t");
    assert_eq!(svg.matches("<circle").count(), 1, "{svg}");
}

#[test]
fn block_time_is_the_gap_to_each_parent_oldest_first() {
    // Newest first, as the store returns them: heights 3, 2, 1 at 30s, 15s, 0s.
    let blocks = vec![block(3, 30, 0), block(2, 15, 0), block(1, 0, 0)];
    let series = block_time_series(&blocks);
    assert_eq!(series.len(), 2);
    assert_eq!(series[0].value, 15.0);
    assert_eq!(series[1].value, 15.0);
    assert!(
        series[1].label.starts_with("block 3"),
        "{}",
        series[1].label
    );
}

#[test]
fn a_clock_running_backwards_is_clamped_to_zero_gap() {
    // Timestamps are miner-supplied and only loosely ordered.
    let series = block_time_series(&[block(2, 5, 0), block(1, 10, 0)]);
    assert_eq!(series[0].value, 0.0);
}

#[test]
fn tx_count_is_one_sample_per_block_oldest_first() {
    let series = tx_count_series(&[block(2, 0, 7), block(1, 0, 0)]);
    let values: Vec<f64> = series.iter().map(|s| s.value).collect();
    assert_eq!(values, vec![0.0, 7.0]);
}
