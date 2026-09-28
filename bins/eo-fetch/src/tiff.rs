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
        head[entry + 8..entry + 8 + usize::try_from(bytes)?].to_vec()
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
        let tie = tags
            .get(&MODEL_TIEPOINT)
            .ok_or_else(|| anyhow!("no tiepoint"))?;
        let scale = tags
            .get(&MODEL_PIXEL_SCALE)
            .ok_or_else(|| anyhow!("no pixel scale"))?;
        Ok(Self {
            width: usize::try_from(to_u64(one(IMAGE_WIDTH)?))?,
            height: usize::try_from(to_u64(one(IMAGE_LENGTH)?))?,
            tile: usize::try_from(to_u64(one(TILE_WIDTH)?))?,
            predictor: tags
                .get(&PREDICTOR)
                .and_then(|v| v.first().copied())
                .map_or(1, to_u64),
            offsets: tags
                .get(&TILE_OFFSETS)
                .ok_or_else(|| anyhow!("no tile offsets"))?
                .iter()
                .map(|v| to_u64(*v))
                .collect(),
            counts: tags
                .get(&TILE_BYTE_COUNTS)
                .ok_or_else(|| anyhow!("no tile byte counts"))?
                .iter()
                .map(|v| to_u64(*v))
                .collect(),
            origin: (tie[3], tie[4]),
            scale: (scale[0], scale[1]),
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
        let raw = fetch(self.offsets[index], self.counts[index])?;
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
        if col + size > self.width || row + size > self.height {
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
