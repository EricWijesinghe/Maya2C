//! `eo-fetch` — reads a small window of a Sentinel-2 L2A scene's red (B04)
//! and near-infrared (B08) bands straight from the public cloud-optimised
//! `GeoTIFFs` Element84's Earth Search indexes, and writes it as a fixture for
//! `maya-eo-oracle` (Master Prompt 6 §8).
//!
//! Only the bytes needed are fetched: the TIFF header and tile index, then
//! the one or two 1024×1024 tiles the window touches, by HTTP range request.
//! Tiles are DEFLATE-compressed with TIFF predictor 2 (horizontal
//! differencing), undone here.
//!
//! ```text
//! eo-fetch --lon -120.30 --lat 36.95 --from 2025-06-01 --to 2025-07-31 \
//!          --scenes 3 --size 32 --out crates/eo-oracle/tests/fixtures/field.json
//! ```

mod tiff;
mod utm;

use std::io::Read as _;

use anyhow::{Context as _, anyhow, bail};
use serde_json::{Value, json};

const SEARCH: &str = "https://earth-search.aws.element84.com/v1/search";

struct Args {
    lon: f64,
    lat: f64,
    from: String,
    to: String,
    scenes: usize,
    size: usize,
    out: String,
}

fn args() -> anyhow::Result<Args> {
    let mut a = Args {
        lon: 0.0,
        lat: 0.0,
        from: String::new(),
        to: String::new(),
        scenes: 3,
        size: 32,
        out: String::new(),
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let value = it.next().ok_or_else(|| anyhow!("{flag} needs a value"))?;
        match flag.as_str() {
            "--lon" => a.lon = value.parse()?,
            "--lat" => a.lat = value.parse()?,
            "--from" => a.from = value,
            "--to" => a.to = value,
            "--scenes" => a.scenes = value.parse()?,
            "--size" => a.size = value.parse()?,
            "--out" => a.out = value,
            other => bail!("unknown flag {other}"),
        }
    }
    if a.out.is_empty() || a.from.is_empty() || a.to.is_empty() {
        bail!(
            "usage: eo-fetch --lon X --lat Y --from DATE --to DATE [--scenes N] [--size PX] --out FILE"
        );
    }
    Ok(a)
}

/// A search item's acquisition day, `YYYY-MM-DD`; `None` when the service
/// sent no usable datetime. `get`, not `[..10]`: the string is the remote
/// service's, and a byte slice of a shorter one panics.
fn scene_day(f: &Value) -> Option<&str> {
    f["properties"]["datetime"]
        .as_str()
        .and_then(|d| d.get(..10))
}

/// One scene per acquisition day in UTM `epsg`, oldest first, at most
/// `scenes`; and how many were skipped for having no usable datetime.
///
/// Skipped, not keyed on an empty day: `dedup_by_key` would merge every
/// undated scene into one and drop the rest unseen.
fn pick(features: &[Value], epsg: u32, scenes: usize) -> (Vec<Value>, usize) {
    let (mut picked, undated): (Vec<Value>, Vec<Value>) = features
        .iter()
        .filter(|f| {
            let p = &f["properties"];
            p["proj:epsg"].as_u64() == Some(u64::from(epsg))
                || p["proj:code"].as_str() == Some(&format!("EPSG:{epsg}"))
        })
        .cloned()
        .partition(|f| scene_day(f).is_some());
    picked.sort_by_key(|f| {
        f["properties"]["datetime"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    });
    picked.dedup_by_key(|f| scene_day(f).map(str::to_owned));
    picked.truncate(scenes);
    (picked, undated.len())
}

fn search(client: &reqwest::blocking::Client, a: &Args) -> anyhow::Result<Vec<Value>> {
    let body = json!({
        "collections": ["sentinel-2-l2a"],
        "intersects": {"type": "Point", "coordinates": [a.lon, a.lat]},
        "datetime": format!("{}T00:00:00Z/{}T23:59:59Z", a.from, a.to),
        "query": {"eo:cloud_cover": {"lt": 5}},
        "limit": 50,
    });
    let found: Value = client
        .post(SEARCH)
        .json(&body)
        .send()?
        .error_for_status()?
        .json()?;
    let features = found["features"]
        .as_array()
        .ok_or_else(|| anyhow!("no features in the search result"))?;
    // One scene per acquisition day, in the point's own UTM zone.
    let zone = utm::zone(a.lon);
    let (picked, undated) = pick(features, 32_600 + zone, a.scenes);
    if undated > 0 {
        eprintln!("warning: skipped {undated} scene(s) with no usable datetime");
    }
    if picked.is_empty() {
        bail!("no low-cloud scene in UTM zone {zone} for that point and range");
    }
    Ok(picked)
}

fn range(
    client: &reqwest::blocking::Client,
    url: &str,
    start: u64,
    len: u64,
) -> anyhow::Result<Vec<u8>> {
    // A zero-length range has no last byte; `start + len - 1` would underflow.
    let last = len
        .checked_sub(1)
        .and_then(|l| start.checked_add(l))
        .ok_or_else(|| anyhow!("bad byte range: {len} bytes at {start}"))?;
    let response = client
        .get(url)
        .header("Range", format!("bytes={start}-{last}"))
        .send()?
        .error_for_status()?;
    let mut out = Vec::new();
    response.take(len).read_to_end(&mut out)?;
    Ok(out)
}

fn window(
    client: &reqwest::blocking::Client,
    url: &str,
    easting: f64,
    northing: f64,
    size: usize,
) -> anyhow::Result<Vec<u16>> {
    let head = range(client, url, 0, 65_536)?;
    let image = tiff::Image::parse(&head, |start, len| range(client, url, start, len))?;
    let (col, row) = image.pixel_of(easting, northing)?;
    let half = size / 2;
    let (c0, r0) = (
        col.checked_sub(half).context("window off the image")?,
        row.checked_sub(half).context("window off the image")?,
    );
    image.read_window(c0, r0, size, |start, len| range(client, url, start, len))
}

fn main() -> anyhow::Result<()> {
    let a = args()?;
    let client = reqwest::blocking::Client::builder()
        .user_agent("maya2c-eo-fetch")
        .build()?;
    let (easting, northing) = utm::forward(a.lon, a.lat);
    let mut scenes = Vec::new();
    for item in search(&client, &a)? {
        let p = &item["properties"];
        let href = |band: &str| {
            item["assets"][band]["href"]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| anyhow!("no {band} asset"))
        };
        let red = window(&client, &href("red")?, easting, northing, a.size)?;
        let nir = window(&client, &href("nir")?, easting, northing, a.size)?;
        println!(
            "{} {} {}: {} red and {} nir pixels",
            item["id"],
            p["datetime"],
            p["platform"],
            red.len(),
            nir.len()
        );
        scenes.push(json!({
            "id": item["id"], "datetime": p["datetime"], "platform": p["platform"],
            "processing_baseline": p["s2:processing_baseline"], "cloud_cover": p["eo:cloud_cover"],
            "red_href": href("red")?, "nir_href": href("nir")?,
            "red": red, "nir": nir,
        }));
    }
    let doc = json!({
        "source": "Sentinel-2 L2A via Element84 Earth Search (earth-search.aws.element84.com), fetched by eo-fetch",
        "point": {"lon": a.lon, "lat": a.lat, "utm_zone": utm::zone(a.lon), "easting": easting, "northing": northing},
        "window_px": a.size, "pixel_m": 10,
        "note": "digital numbers; L2A reflectance = (DN + BOA_ADD_OFFSET) / 10000, BOA_ADD_OFFSET = -1000 from processing baseline 04.00",
        "scenes": scenes,
    });
    std::fs::write(&a.out, serde_json::to_string_pretty(&doc)? + "\n")
        .with_context(|| format!("writing {}", a.out))?;
    println!("wrote {}", a.out);
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_short_or_missing_datetime_from_the_search_is_no_day_not_a_panic() {
        let item = |d: Value| json!({"properties": {"datetime": d}});
        assert_eq!(
            scene_day(&item(json!("2025-06-14T18:39:21Z"))),
            Some("2025-06-14")
        );
        assert_eq!(scene_day(&item(json!("2025"))), None);
        assert_eq!(
            scene_day(&item(json!("2025-06-1é"))),
            None,
            "byte 10 is inside a character"
        );
        assert_eq!(scene_day(&item(Value::Null)), None);
        assert_eq!(scene_day(&json!({})), None);
    }

    #[test]
    fn an_empty_or_overflowing_byte_range_is_refused_before_any_request() {
        // Port 9 (discard) is never contacted: both are refused up front.
        let client = reqwest::blocking::Client::new();
        for (start, len) in [(5, 0), (u64::MAX, 2)] {
            let err = range(&client, "http://127.0.0.1:9/", start, len).unwrap_err();
            assert!(err.to_string().contains("bad byte range"), "{err}");
        }
    }

    #[test]
    fn undated_scenes_are_counted_as_skipped_not_merged_into_one() {
        let item = |id: &str, d: Value| json!({"id": id, "properties": {"datetime": d, "proj:epsg": 32_610}});
        let features = [
            item("a", json!("2025-06-14T18:39:21Z")),
            item("b", Value::Null),
            item("c", json!("2025")),
            item("d", json!("2025-06-14T18:40:00Z")),
            item("e", json!("2025-06-19T18:39:21Z")),
        ];
        let (picked, undated) = pick(&features, 32_610, 10);
        let ids: Vec<_> = picked.iter().map(|f| f["id"].as_str()).collect();
        assert_eq!(ids, [Some("a"), Some("e")], "one per day, none undated");
        assert_eq!(undated, 2);
    }
}
