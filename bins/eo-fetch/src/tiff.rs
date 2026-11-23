//! Just enough TIFF/GeoTIFF to read a window of a tiled, DEFLATE-compressed,
//! 16-bit single-band cloud-optimised `GeoTIFF` over HTTP ranges: the first
//! IFD's tile layout and georeferencing, then only the tiles a window needs.

// Offsets, counts and pixel indices in a Sentinel-2 COG are far below 2^52,
// so reading them as f64 loses nothing.
#![allow(clippy::cast_precision_loss)]

use std::io::Read as _;

use anyhow::{Context as _, anyhow, bail};

const TILE_WIDTH: u16 = 322;
const TILE_LENGTH: u16 = 323;
const TILE_OFFSETS: u16 = 324;
const TILE_BYTE_COUNTS: u16 = 325;
const COMPRESSION: u16 = 259;
const PREDICTOR: u16 = 317;
const BITS_PER_SAMPLE: u16 = 258;
const IMAGE_WIDTH: u16 = 256;
const IMAGE_LENGTH: u16 = 257;
const MODEL_PIXEL_SCALE: u16 = 33_550;
const MODEL_TIEPOINT: u16 = 33_922;
const DEFLATE: [u64; 2] = [8, 32_946];
/// Sentinel-2 COG tiles are 1024 px; past this a header's tile size is
/// refused rather than allocated as `tile² × 2` bytes.
const MAX_TILE: usize = 4096;

/// The first image of a `GeoTIFF`.
pub struct Image {
    width: usize,
    height: usize,
    tile: usize,
    predictor: u64,
    offsets: Vec<u64>,
    counts: Vec<u64>,
    origin: (f64, f64),
    scale: (f64, f64),
}

fn u16_at(b: &[u8], at: usize) -> anyhow::Result<u16> {
    Ok(u16::from_le_bytes(
        b.get(at..at + 2).context("TIFF truncated")?.try_into()?,
    ))
}

fn u32_at(b: &[u8], at: usize) -> anyhow::Result<u32> {
    Ok(u32::from_le_bytes(
        b.get(at..at + 4).context("TIFF truncated")?.try_into()?,
    ))
}

/// One IFD entry's values, fetching them if they live outside `head`.
fn values(
    head: &[u8],
    entry: usize,
    fetch: &mut impl FnMut(u64, u64) -> anyhow::Result<Vec<u8>>,
) -> anyhow::Result<Vec<f64>> {
    let kind = u16_at(head, entry + 2)?;
    let count = u64::from(u32_at(head, entry + 4)?);
    let size: u64 = match kind {
        3 => 2,
        4 => 4,
        12 | 16 => 8,
        // ASCII, bytes, rationals: metadata this reader never needs.
        _ => return Ok(Vec::new()),
    };
    let bytes = count * size;
    let data = if bytes <= 4 {
        head.get(entry + 8..entry + 8 + usize::try_from(bytes)?)
            .context("TIFF truncated")?
            .to_vec()
    } else {
        let at = u64::from(u32_at(head, entry + 8)?);
        match head.get(usize::try_from(at)?..usize::try_from(at + bytes)?) {
            Some(inline) => inline.to_vec(),
            None => fetch(at, bytes)?,
        }
    };
    data.chunks_exact(usize::try_from(size)?)
        .map(|c| {
            Ok(match kind {
                3 => f64::from(u16::from_le_bytes(c.try_into()?)),
                4 => f64::from(u32::from_le_bytes(c.try_into()?)),
                12 => f64::from_le_bytes(c.try_into()?),
                _ => u64::from_le_bytes(c.try_into()?) as f64,
            })
        })
        .collect()
}

fn to_u64(v: f64) -> u64 {
    // TIFF offsets and counts are non-negative integers read as f64 above.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let out = v as u64;
    out
}

impl Image {
    /// Parses the first IFD from `head` (the file's first bytes), fetching
    /// arrays that lie beyond it.
    ///
    /// # Errors
    ///
    /// Anything but a little-endian classic TIFF with a tiled, DEFLATE,
    /// 16-bit, georeferenced first image.
    pub fn parse(
        head: &[u8],
        mut fetch: impl FnMut(u64, u64) -> anyhow::Result<Vec<u8>>,
    ) -> anyhow::Result<Self> {
        if head.get(..4) != Some(b"II*\0") {
            bail!("not a little-endian classic TIFF");
        }
        let ifd = usize::try_from(u32_at(head, 4)?)?;
        let n = usize::from(u16_at(head, ifd)?);
        let mut tags = std::collections::BTreeMap::new();
        for i in 0..n {
            let entry = ifd + 2 + 12 * i;
            tags.insert(u16_at(head, entry)?, values(head, entry, &mut fetch)?);
        }
        let one = |tag: u16| -> anyhow::Result<f64> {
            tags.get(&tag)
                .and_then(|v| v.first().copied())
                .ok_or_else(|| anyhow!("TIFF tag {tag} missing"))
        };
        if !DEFLATE.contains(&to_u64(one(COMPRESSION)?)) || to_u64(one(BITS_PER_SAMPLE)?) != 16 {
            bail!("expected DEFLATE-compressed 16-bit samples");
        }
        if to_u64(one(TILE_WIDTH)?) != to_u64(one(TILE_LENGTH)?) {
            bail!("non-square tiles");
        }
        // The header is the remote store's: every length below is checked
        // rather than indexed, so a malformed file is an error, not a panic.
        let (Some(&[_, _, _, x0, y0, ..]), Some(&[sx, sy, ..])) = (
            tags.get(&MODEL_TIEPOINT).map(Vec::as_slice),
            tags.get(&MODEL_PIXEL_SCALE).map(Vec::as_slice),
        ) else {
            bail!("tiepoint needs at least 5 values and pixel scale 2");
        };
        if !(sx.is_finite() && sy.is_finite() && sx > 0.0 && sy > 0.0) {
            bail!("pixel scale {sx}×{sy} is not positive");
        }
        let tile = usize::try_from(to_u64(one(TILE_WIDTH)?))?;
        if !(1..=MAX_TILE).contains(&tile) {
            bail!("tile size {tile} outside 1..={MAX_TILE}");
        }
        let width = usize::try_from(to_u64(one(IMAGE_WIDTH)?))?;
        let height = usize::try_from(to_u64(one(IMAGE_LENGTH)?))?;
        let list = |tag: u16, what: &str| -> anyhow::Result<Vec<u64>> {
            let v = tags.get(&tag).ok_or_else(|| anyhow!("no tile {what}"))?;
            Ok(v.iter().map(|v| to_u64(*v)).collect())
        };
        let (offsets, counts) = (
            list(TILE_OFFSETS, "offsets")?,
            list(TILE_BYTE_COUNTS, "byte counts")?,
        );
        let tiles = width
            .div_ceil(tile)
            .checked_mul(height.div_ceil(tile))
            .context("tile grid overflows")?;
        if offsets.len() != counts.len() || offsets.len() < tiles {
            bail!(
                "{} tile offsets and {} byte counts for {tiles} tiles",
                offsets.len(),
                counts.len()
            );
        }
        Ok(Self {
            width,
            height,
            tile,
            predictor: tags
                .get(&PREDICTOR)
                .and_then(|v| v.first().copied())
                .map_or(1, to_u64),
            offsets,
            counts,
            origin: (x0, y0),
            scale: (sx, sy),
        })
    }

    /// The pixel `(column, row)` holding a projected point.
    ///
    /// # Errors
    ///
    /// A point outside the image.
    pub fn pixel_of(&self, easting: f64, northing: f64) -> anyhow::Result<(usize, usize)> {
        let col = ((easting - self.origin.0) / self.scale.0).floor();
        let row = ((self.origin.1 - northing) / self.scale.1).floor();
        if col < 0.0 || row < 0.0 || col >= self.width as f64 || row >= self.height as f64 {
            bail!("the point is outside this image");
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        Ok((col as usize, row as usize))
    }

    fn tile(
        &self,
        index: usize,
        fetch: &mut impl FnMut(u64, u64) -> anyhow::Result<Vec<u8>>,
    ) -> anyhow::Result<Vec<u16>> {
        let (Some(&at), Some(&len)) = (self.offsets.get(index), self.counts.get(index)) else {
            bail!("no tile {index}");
        };
        let raw = fetch(at, len)?;
        let mut bytes = Vec::with_capacity(self.tile * self.tile * 2);
        flate2::read::ZlibDecoder::new(&raw[..]).read_to_end(&mut bytes)?;
        let mut samples: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        if samples.len() != self.tile * self.tile {
            bail!("tile inflated to {} samples", samples.len());
        }
        if self.predictor == 2 {
            for row in samples.chunks_exact_mut(self.tile) {
                for i in 1..row.len() {
                    row[i] = row[i].wrapping_add(row[i - 1]);
                }
            }
        }
        Ok(samples)
    }

    /// A `size`×`size` window with its top-left at `(col, row)`, row-major.
    ///
    /// # Errors
    ///
    /// A window off the image, or a tile that cannot be fetched or inflated.
    pub fn read_window(
        &self,
        col: usize,
        row: usize,
        size: usize,
        mut fetch: impl FnMut(u64, u64) -> anyhow::Result<Vec<u8>>,
    ) -> anyhow::Result<Vec<u16>> {
        // `size` is the caller's --size: checked, since a huge one overflows.
        let fits = |at: usize, end: usize| at.checked_add(size).is_some_and(|e| e <= end);
        if !fits(col, self.width) || !fits(row, self.height) {
            bail!("window off the image");
        }
        let across = self.width.div_ceil(self.tile);
        let mut cache = std::collections::BTreeMap::new();
        let mut out = Vec::with_capacity(size * size);
        for r in row..row + size {
            for c in col..col + size {
                let index = (r / self.tile) * across + c / self.tile;
                if let std::collections::btree_map::Entry::Vacant(e) = cache.entry(index) {
                    e.insert(self.tile(index, &mut fetch)?);
                }
                out.push(cache[&index][(r % self.tile) * self.tile + c % self.tile]);
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
mod tests {
    use super::*;

    const SHORT: u16 = 3;
    const DOUBLE: u16 = 12;

    /// A classic little-endian TIFF with one IFD of `entries`, values that do
    /// not fit in four bytes laid out after it.
    fn tiff(entries: &[(u16, u16, Vec<f64>)]) -> Vec<u8> {
        let n = u16::try_from(entries.len()).unwrap();
        let mut data_at = 8 + 2 + 12 * entries.len() + 4;
        let (mut ifd, mut data) = (n.to_le_bytes().to_vec(), Vec::new());
        for (tag, kind, values) in entries {
            let bytes: Vec<u8> = values
                .iter()
                .flat_map(|v| match *kind {
                    SHORT => (*v as u16).to_le_bytes().to_vec(),
                    _ => v.to_le_bytes().to_vec(),
                })
                .collect();
            ifd.extend(tag.to_le_bytes());
            ifd.extend(kind.to_le_bytes());
            ifd.extend(u32::try_from(values.len()).unwrap().to_le_bytes());
            if bytes.len() <= 4 {
                ifd.extend(&bytes);
                ifd.extend(vec![0; 4 - bytes.len()]);
            } else {
                ifd.extend(u32::try_from(data_at).unwrap().to_le_bytes());
                data_at += bytes.len();
                data.extend(bytes);
            }
        }
        ifd.extend(0u32.to_le_bytes());
        [&b"II*\0"[..], &8u32.to_le_bytes(), &ifd, &data].concat()
    }

    /// A 4×4 image of 2×2 tiles, every tag well formed.
    fn tags() -> Vec<(u16, u16, Vec<f64>)> {
        vec![
            (IMAGE_WIDTH, SHORT, vec![4.0]),
            (IMAGE_LENGTH, SHORT, vec![4.0]),
            (BITS_PER_SAMPLE, SHORT, vec![16.0]),
            (COMPRESSION, SHORT, vec![8.0]),
            (TILE_WIDTH, SHORT, vec![2.0]),
            (TILE_LENGTH, SHORT, vec![2.0]),
            (TILE_OFFSETS, DOUBLE, vec![0.0, 1.0, 2.0, 3.0]),
            (TILE_BYTE_COUNTS, DOUBLE, vec![1.0; 4]),
            (MODEL_PIXEL_SCALE, DOUBLE, vec![10.0, 10.0, 0.0]),
            (
                MODEL_TIEPOINT,
                DOUBLE,
                vec![0.0, 0.0, 0.0, 500_000.0, 4_000_000.0, 0.0],
            ),
        ]
    }

    fn with(tag: u16, kind: u16, values: Vec<f64>) -> Vec<u8> {
        let mut t = tags();
        t.iter_mut().find(|e| e.0 == tag).unwrap().2 = values;
        t.iter_mut().find(|e| e.0 == tag).unwrap().1 = kind;
        tiff(&t)
    }

    fn refused(head: &[u8]) -> String {
        let no_fetch = |_: u64, _: u64| -> anyhow::Result<Vec<u8>> { bail!("fetched") };
        match Image::parse(head, no_fetch) {
            Ok(_) => panic!("parsed"),
            Err(e) => e.to_string(),
        }
    }

    #[test]
    fn a_well_formed_header_parses() {
        let image = Image::parse(&tiff(&tags()), |_, _| bail!("fetched")).unwrap();
        assert_eq!((image.width, image.tile, image.offsets.len()), (4, 2, 4));
        assert_eq!(image.pixel_of(500_015.0, 3_999_985.0).unwrap(), (1, 1));
    }

    #[test]
    fn a_short_tiepoint_or_pixel_scale_is_refused() {
        assert!(refused(&with(MODEL_TIEPOINT, DOUBLE, vec![0.0; 4])).contains("tiepoint"));
        assert!(refused(&with(MODEL_PIXEL_SCALE, DOUBLE, vec![10.0])).contains("pixel scale"));
        assert!(refused(&with(MODEL_PIXEL_SCALE, DOUBLE, vec![0.0, 10.0])).contains("positive"));
    }

    #[test]
    fn a_zero_tile_size_is_refused() {
        let mut t = tags();
        for e in t
            .iter_mut()
            .filter(|e| e.0 == TILE_WIDTH || e.0 == TILE_LENGTH)
        {
            e.2 = vec![0.0];
        }
        assert!(refused(&tiff(&t)).contains("tile size 0"));
    }

    #[test]
    fn too_few_or_mismatched_tile_offsets_are_refused() {
        assert!(refused(&with(TILE_OFFSETS, DOUBLE, vec![0.0, 1.0, 2.0])).contains("for 4 tiles"));
        assert!(refused(&with(TILE_BYTE_COUNTS, DOUBLE, vec![1.0; 5])).contains("for 4 tiles"));
    }

    #[test]
    fn an_inline_value_past_the_end_of_the_header_is_refused() {
        // One entry of two shorts, its four inline bytes cut to two.
        let head = tiff(&[(IMAGE_WIDTH, SHORT, vec![4.0, 4.0])]);
        let cut = &head[..8 + 2 + 8 + 2];
        assert!(refused(cut).contains("truncated"));
    }

    #[test]
    fn a_window_whose_size_overflows_is_refused() {
        let image = Image::parse(&tiff(&tags()), |_, _| bail!("fetched")).unwrap();
        let err = image.read_window(1, 1, usize::MAX, |_, _| bail!("fetched"));
        assert!(err.unwrap_err().to_string().contains("off the image"));
    }
}
