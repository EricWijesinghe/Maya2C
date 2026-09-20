//! An opening: the short `(s, e)` a claim reveals.
//!
//! ## The bound is the security
//!
//! `A·s + e = t` alone is satisfied by *every* `s`: pick one, set
//! `e = t - A·s`. What makes an opening hard to find without the secret is that
//! every coefficient of both vectors is in `[-η, η]`. So the bound is checked
//! where an [`Opening`] is built, and there is no other way to build one —
//! `tests/lattice_tests.rs` constructs exactly that forgery and shows the
//! equation holding while the constructor refuses it.
//!
//! ## One encoding per opening
//!
//! A nibble per coefficient, storing `η - c` in `0..=2η`. The seven nibble
//! values above `2η` are refused rather than reduced, so a claim's bytes are a
//! function of its opening and nothing else — two encodings of one opening
//! would be two transaction ids for one claim.

use std::fmt;

use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::error::{Error, Result};
use crate::params::{ETA, K, L, N, OPENING_BYTES, OPENING_COEFFICIENTS};

/// The largest nibble an opening encoding may carry: `2η`.
const MAX_NIBBLE: u8 = (2 * ETA) as u8;

/// A short `(s, e)` with every coefficient in `[-η, η]`.
///
/// Secret until a claim publishes it, and zeroized on drop either way — the
/// same value cannot be both, and treating it as secret costs nothing.
#[derive(Clone, PartialEq, Eq, Zeroize, ZeroizeOnDrop)]
pub struct Opening {
    s: Vec<i8>,
    e: Vec<i8>,
}

impl fmt::Debug for Opening {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Opening")
            .field("s", &format_args!("[{} coefficients]", self.s.len()))
            .field("e", &format_args!("[{} coefficients]", self.e.len()))
            .finish()
    }
}

impl Opening {
    /// Builds an opening, checking both lengths and every coefficient's bound.
    ///
    /// Takes `i32` so a caller can hand it a value that does not fit the
    /// bound and be told so, rather than having it truncated into range.
    ///
    /// # Errors
    ///
    /// [`Error::Length`] for `s` not `L × N` or `e` not `K × N`, and
    /// [`Error::OutOfBound`] naming the first coefficient outside `[-η, η]`.
    pub fn new(s: &[i32], e: &[i32]) -> Result<Self> {
        if s.len() != L * N {
            return Err(Error::Length {
                what: "s",
                expected: L * N,
                found: s.len(),
            });
        }
        if e.len() != K * N {
            return Err(Error::Length {
                what: "e",
                expected: K * N,
                found: e.len(),
            });
        }
        Ok(Self {
            s: bounded(s, 0)?,
            e: bounded(e, L * N)?,
        })
    }

    /// Builds an opening from coefficients the caller has already bounded.
    pub(crate) fn from_bounded(s: Vec<i8>, e: Vec<i8>) -> Self {
        debug_assert!(s.len() == L * N && e.len() == K * N);
        debug_assert!(s.iter().chain(&e).all(|c| i32::from(*c).abs() <= ETA));
        Self { s, e }
    }

    /// `s`: `L × N` coefficients.
    #[must_use]
    pub fn s(&self) -> &[i8] {
        &self.s
    }

    /// `e`: `K × N` coefficients.
    #[must_use]
    pub fn e(&self) -> &[i8] {
        &self.e
    }

    /// The wire form: [`OPENING_BYTES`] bytes, `s` then `e`, low nibble first.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(OPENING_BYTES);
        let mut coefficients = self.s.iter().chain(&self.e);
        while let (Some(low), Some(high)) = (coefficients.next(), coefficients.next()) {
            out.push(nibble(*low) | (nibble(*high) << 4));
        }
        out
    }

    /// Reads the wire form.
    ///
    /// # Errors
    ///
    /// [`Error::Length`] for anything but [`OPENING_BYTES`] bytes, and
    /// [`Error::NonCanonical`] for a nibble past `2η`.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != OPENING_BYTES {
            return Err(Error::Length {
                what: "opening encoding",
                expected: OPENING_BYTES,
                found: bytes.len(),
            });
        }
        let mut coefficients = Vec::with_capacity(OPENING_COEFFICIENTS);
        for (index, byte) in bytes.iter().enumerate() {
            for (offset, value) in [byte & 0x0f, byte >> 4].into_iter().enumerate() {
                if value > MAX_NIBBLE {
                    return Err(Error::NonCanonical {
                        what: "opening",
                        index: 2 * index + offset,
                    });
                }
                // value ≤ 2η = 8, so η - value is in [-4, 4].
                coefficients.push((ETA as i8) - value as i8);
            }
        }
        let e = coefficients.split_off(L * N);
        let opening = Self::from_bounded(coefficients, e);
        Ok(opening)
    }
}

/// Narrows coefficients into `i8`, refusing any outside `[-η, η]`.
fn bounded(values: &[i32], offset: usize) -> Result<Vec<i8>> {
    values
        .iter()
        .enumerate()
        .map(|(index, &value)| {
            // A range test, not `abs()`: `i32::MIN.abs()` wraps to `i32::MIN`
            // in release, which is not `> ETA`, and would pass as in bound.
            if !(-ETA..=ETA).contains(&value) {
                return Err(Error::OutOfBound {
                    index: offset + index,
                });
            }
            // Within ±4, so the narrowing is exact.
            Ok(value as i8)
        })
        .collect()
}

/// `η - c`, for a coefficient already known to be in `[-η, η]`.
fn nibble(coefficient: i8) -> u8 {
    (ETA as i8 - coefficient) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Opening {
        let s: Vec<i32> = (0..L * N).map(|i| (i % 9) as i32 - ETA).collect();
        let e: Vec<i32> = (0..K * N).map(|i| ((i * 4) % 9) as i32 - ETA).collect();
        Opening::new(&s, &e).expect("bounded")
    }

    #[test]
    fn encoding_round_trips() {
        let opening = sample();
        let bytes = opening.encode();
        assert_eq!(bytes.len(), OPENING_BYTES);
        assert_eq!(Opening::decode(&bytes).expect("decode"), opening);
    }

    #[test]
    fn every_nibble_past_twice_eta_is_refused() {
        for bad in MAX_NIBBLE + 1..=0x0f {
            let mut bytes = sample().encode();
            bytes[10] = (bytes[10] & 0xf0) | bad;
            assert_eq!(
                Opening::decode(&bytes),
                Err(Error::NonCanonical {
                    what: "opening",
                    index: 20
                })
            );
        }
    }

    #[test]
    fn a_coefficient_one_past_the_bound_is_refused() {
        let mut e = vec![0i32; K * N];
        e[3] = ETA + 1;
        assert_eq!(
            Opening::new(&vec![0; L * N], &e),
            Err(Error::OutOfBound { index: L * N + 3 })
        );
        e[3] = -(ETA + 1);
        assert!(Opening::new(&vec![0; L * N], &e).is_err());
    }

    #[test]
    fn the_extremes_of_i32_are_out_of_bound_too() {
        // `i32::MIN.abs()` wraps to itself in release, and `as i8` would then
        // truncate it to 0 — an in-bound coefficient nobody supplied.
        for extreme in [i32::MIN, i32::MAX] {
            let mut s = vec![0i32; L * N];
            s[0] = extreme;
            assert_eq!(
                Opening::new(&s, &vec![0; K * N]),
                Err(Error::OutOfBound { index: 0 })
            );
        }
    }

    #[test]
    fn debug_output_does_not_print_coefficients() {
        let rendered = format!("{:?}", sample());
        assert!(rendered.contains("1280 coefficients"));
        assert!(!rendered.contains("-4"));
    }
}
