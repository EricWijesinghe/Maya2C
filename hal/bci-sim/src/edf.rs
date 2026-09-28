//! European Data Format (EDF, Kemp et al. 1992), the common container for
//! EEG recordings: a 256-byte ASCII header, 256 bytes per signal, then data
//! records of little-endian 16-bit samples.
//!
//! This parser is REAL — it reads the format as specified — and bounded: the
//! header's counts are checked against the bytes present before anything is
//! allocated. Only what feature extraction needs is kept.

use crate::BciError;

const FIXED_HEADER: usize = 256;
const PER_SIGNAL: usize = 256;
/// Most signals accepted; clinical EEG caps have at most a few hundred.
pub const MAX_SIGNALS: usize = 512;

/// One signal's header and samples, scaled to physical units.
#[derive(Clone, Debug, PartialEq)]
pub struct Signal {
    /// Label, e.g. `Fp1`, trimmed.
    pub label: String,
    /// Samples per second.
    pub rate_hz: f64,
    /// Samples, in physical units (µV for EEG).
    pub samples: Vec<f64>,
}

/// A parsed recording.
#[derive(Clone, Debug, PartialEq)]
pub struct Recording {
    /// The signals, in file order.
    pub signals: Vec<Signal>,
}

fn field(bytes: &[u8], at: usize, len: usize) -> Result<&str, BciError> {
    let raw = bytes
        .get(at..at + len)
        .ok_or(BciError::Malformed("header truncated"))?;
    std::str::from_utf8(raw)
        .map(str::trim)
        .map_err(|_| BciError::Malformed("header is not ASCII"))
}

fn number<T: std::str::FromStr>(bytes: &[u8], at: usize, len: usize) -> Result<T, BciError> {
    field(bytes, at, len)?
        .parse()
        .map_err(|_| BciError::Malformed("header number"))
}

/// Parses an EDF file.
///
/// # Errors
///
/// [`BciError::Malformed`] for a truncated or inconsistent file.
pub fn parse(bytes: &[u8]) -> Result<Recording, BciError> {
    let header_bytes: usize = number(bytes, 184, 8)?;
    let records: usize = number(bytes, 236, 8)?;
    let duration: f64 = number(bytes, 244, 8)?;
    let ns: usize = number(bytes, 252, 4)?;
    if ns == 0 || ns > MAX_SIGNALS || header_bytes != FIXED_HEADER + ns * PER_SIGNAL {
        return Err(BciError::Malformed("signal count and header size disagree"));
    }
    if duration.is_nan() || duration <= 0.0 {
        return Err(BciError::Malformed("record duration"));
    }
    // Per-signal fields are stored field by field, each for all signals.
    let offset =
        |field_start: usize, width: usize, i: usize| FIXED_HEADER + ns * field_start + i * width;
    let mut meta = Vec::with_capacity(ns);
    for i in 0..ns {
        let label = field(bytes, offset(0, 16, i), 16)?.to_owned();
        let (pmin, pmax): (f64, f64) = (
            number(bytes, offset(104, 8, i), 8)?,
            number(bytes, offset(112, 8, i), 8)?,
        );
        let (dmin, dmax): (f64, f64) = (
            number(bytes, offset(120, 8, i), 8)?,
            number(bytes, offset(128, 8, i), 8)?,
        );
        let per_record: usize = number(bytes, offset(216, 8, i), 8)?;
        if dmax <= dmin || per_record == 0 {
            return Err(BciError::Malformed("signal range"));
        }
        meta.push((label, (pmax - pmin) / (dmax - dmin), pmin, dmin, per_record));
    }
    let record_samples: usize = meta.iter().map(|m| m.4).sum();
    let expected = records
        .checked_mul(record_samples * 2)
        .and_then(|d| d.checked_add(header_bytes))
        .ok_or(BciError::Malformed("size overflow"))?;
    if bytes.len() != expected {
        return Err(BciError::Malformed("data length disagrees with the header"));
    }
    let mut signals: Vec<Signal> = meta
        .iter()
        .map(|(label, _, _, _, per)| Signal {
            label: label.clone(),
            rate_hz: *per as f64 / duration,
            samples: Vec::with_capacity(per * records),
        })
        .collect();
    let mut at = header_bytes;
    for _ in 0..records {
        for (signal, (_, gain, pmin, dmin, per)) in signals.iter_mut().zip(&meta) {
            for _ in 0..*per {
                let digital = f64::from(i16::from_le_bytes([bytes[at], bytes[at + 1]]));
                signal.samples.push(pmin + (digital - dmin) * gain);
                at += 2;
            }
        }
    }
    Ok(Recording { signals })
}

fn pad(out: &mut Vec<u8>, text: &str, width: usize) {
    let mut field = text.as_bytes().to_vec();
    field.resize(width, b' ');
    out.extend_from_slice(&field[..width]);
}

/// Writes `signals` (all at `rate_hz`, one-second records) as EDF, with a
/// ±`range_uv` physical range. For tests and the simulator.
#[must_use]
pub fn write(labels: &[&str], rate_hz: usize, samples: &[Vec<f64>], range_uv: f64) -> Vec<u8> {
    let ns = labels.len();
    let records = samples.first().map_or(0, |s| s.len() / rate_hz);
    let mut out = Vec::new();
    pad(&mut out, "0", 8);
    pad(&mut out, "SIM subject", 80);
    pad(&mut out, "SIM recording (maya-bci-sim)", 80);
    pad(&mut out, "01.01.26", 8);
    pad(&mut out, "00.00.00", 8);
    pad(&mut out, &(FIXED_HEADER + ns * PER_SIGNAL).to_string(), 8);
    pad(&mut out, "", 44);
    pad(&mut out, &records.to_string(), 8);
    pad(&mut out, "1", 8);
    pad(&mut out, &ns.to_string(), 4);
    let each = |out: &mut Vec<u8>, value: &dyn Fn(usize) -> String, width: usize| {
        for i in 0..ns {
            pad(out, &value(i), width);
        }
    };
    each(&mut out, &|i| labels[i].to_owned(), 16);
    each(&mut out, &|_| "AgAgCl electrode".into(), 80);
    each(&mut out, &|_| "uV".into(), 8);
    each(&mut out, &|_| format!("{}", -range_uv), 8);
    each(&mut out, &|_| format!("{range_uv}"), 8);
    each(&mut out, &|_| "-32768".into(), 8);
    each(&mut out, &|_| "32767".into(), 8);
    each(&mut out, &|_| String::new(), 80);
    each(&mut out, &|_| rate_hz.to_string(), 8);
    each(&mut out, &|_| String::new(), 32);
    let gain = 65_535.0 / (2.0 * range_uv);
    for r in 0..records {
        for signal in samples {
            for v in &signal[r * rate_hz..(r + 1) * rate_hz] {
                let digital = ((v + range_uv) * gain - 32_768.0)
                    .round()
                    .clamp(-32_768.0, 32_767.0);
                // Clamped to i16's range just above, so the cast is exact.
                #[allow(clippy::cast_possible_truncation)]
                out.extend_from_slice(&(digital as i16).to_le_bytes());
            }
        }
    }
    out
}
