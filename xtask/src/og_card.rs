//! `cargo xtask og-card [--check]` — composes the 1200x630 Open Graph social card
//! from the logo pack and writes it to the explorer and dashboard.
//!
//! The pack has no 1.91:1 image, which is the ratio scrapers crop to, so the card
//! is generated from a committed recipe rather than shipped as an unexplained
//! binary. Every input comes from the pack: the wordmark verbatim, and the
//! background read from the manifest's `background_color`. Nothing is redrawn.
//!
//! The PNG bytes come from the `image` crate's encoder, not Pillow's, so the
//! committed files changed bytes when this replaced `make_og_card.py`. The pixels
//! were checked against the Pillow output (see the commit message).

use std::fs;
use std::io::Cursor;
use std::path::Path;

use image::imageops::{self, FilterType};
use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};

const PACK: &str = "logo-assets";
const CARD_WIDTH: u32 = 1200;
const CARD_HEIGHT: u32 = 630;
/// The wordmark's share of the card width. Two thirds leaves a margin wide
/// enough that a platform cropping tighter still contains the whole mark.
const LOGO_WIDTH_FRACTION: f64 = 2.0 / 3.0;
const SOURCE_LOGO: &str = "website/Header-Logo-2x_500x200.png";
const DESTINATIONS: &[&str] = &[
    "explorer/assets/og-image.png",
    "dashboard/assets/og-image.png",
];

pub fn run(args: &[String]) -> Result<(), String> {
    let check = match args {
        [] => false,
        [flag] if flag == "--check" => true,
        _ => return Err("usage: cargo xtask og-card [--check]".into()),
    };
    let root = crate::workspace_root();
    let bytes = encode(&compose(&root)?)?;

    if check {
        let mut stale = Vec::new();
        for dest in DESTINATIONS {
            let path = root.join("apps").join(dest);
            if fs::read(&path).ok().as_deref() != Some(bytes.as_slice()) {
                stale.push(format!("apps/{dest}"));
            }
        }
        if !stale.is_empty() {
            for path in &stale {
                eprintln!("stale or missing: {path}");
            }
            return Err("run: cargo xtask og-card".into());
        }
        println!("{} cards current", DESTINATIONS.len());
        return Ok(());
    }

    for dest in DESTINATIONS {
        let path = root.join("apps").join(dest);
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        fs::write(&path, &bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        println!("wrote apps/{dest} ({CARD_WIDTH}x{CARD_HEIGHT})");
    }
    Ok(())
}

/// The card: the wordmark, centred, on the pack's background.
// The casts are Python's `int()` truncation of positive float sizes.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn compose(root: &Path) -> Result<RgbaImage, String> {
    let pack = root.join(PACK);
    let background = background_colour(&pack)?;
    let mut card = RgbaImage::from_pixel(CARD_WIDTH, CARD_HEIGHT, background);

    let logo_path = pack.join(SOURCE_LOGO);
    let logo = image::open(&logo_path)
        .map_err(|e| format!("{}: {e}", logo_path.display()))?
        .to_rgba8();
    // Truncation, as Python's `int()` does on the same float arithmetic.
    let target_width = (f64::from(CARD_WIDTH) * LOGO_WIDTH_FRACTION) as u32;
    let scale = f64::from(target_width) / f64::from(logo.width());
    let target_height = (f64::from(logo.height()) * scale) as u32;
    // Lanczos3 for a downscale: nearest-neighbour stair-steps the letterforms.
    let resized = imageops::resize(&logo, target_width, target_height, FilterType::Lanczos3);

    let x = i64::from((CARD_WIDTH - resized.width()) / 2);
    let y = i64::from((CARD_HEIGHT - resized.height()) / 2);
    // `overlay` alpha-blends, so the wordmark's transparent pixels show the background.
    imageops::overlay(&mut card, &resized, x, y);
    Ok(card)
}

/// The manifest's own `background_color`, so a second hardcoded value cannot
/// disagree with the one the browser uses for the splash screen.
fn background_colour(pack: &Path) -> Result<Rgba<u8>, String> {
    let path = pack.join("favicons").join("site.webmanifest");
    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let manifest: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let colour = manifest
        .get("background_color")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("{} declares no background_color", path.display()))?;
    parse_hex_rgb(colour).ok_or_else(|| format!("background_color `{colour}` is not #rrggbb"))
}

/// `#rrggbb` to an opaque pixel.
fn parse_hex_rgb(text: &str) -> Option<Rgba<u8>> {
    let hex = text.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let channel = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
    Some(Rgba([channel(0)?, channel(2)?, channel(4)?, 255]))
}

fn encode(card: &RgbaImage) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    DynamicImage::ImageRgba8(card.clone())
        .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn the_target_width_matches_python_truncation() {
        // Python: int(1200 * (2 / 3)) == 800.
        assert_eq!((f64::from(CARD_WIDTH) * LOGO_WIDTH_FRACTION) as u32, 800);
    }

    #[test]
    fn the_background_is_read_as_an_opaque_pixel() {
        assert_eq!(parse_hex_rgb("#ffffff"), Some(Rgba([255, 255, 255, 255])));
        assert_eq!(parse_hex_rgb("#0a161b"), Some(Rgba([10, 22, 27, 255])));
        assert_eq!(parse_hex_rgb("ffffff"), None);
        assert_eq!(parse_hex_rgb("#fff"), None);
    }

    #[test]
    fn the_card_is_the_size_the_platforms_document() {
        let card = compose(&crate::workspace_root()).expect("the pack is in the repo");
        assert_eq!((card.width(), card.height()), (CARD_WIDTH, CARD_HEIGHT));
    }
}
