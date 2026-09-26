//! WCAG 2.2 contrast ratio (success criteria 1.4.3 and 1.4.11).
//!
//! The formula is the one in the WCAG definition of relative luminance, with
//! the sRGB threshold 0.04045 (the spec text says 0.03928, a leftover from an
//! older sRGB draft; the two differ on no 8-bit value).

/// An sRGB color.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    /// Parses `#rrggbb`.
    #[must_use]
    pub fn hex(s: &str) -> Option<Self> {
        let s = s.strip_prefix('#')?;
        if s.len() != 6 {
            return None;
        }
        let c = |i: usize| u8::from_str_radix(s.get(i..i + 2)?, 16).ok();
        Some(Self(c(0)?, c(2)?, c(4)?))
    }

    /// `#rrggbb`.
    #[must_use]
    pub fn to_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.0, self.1, self.2)
    }

    /// Relative luminance, 0 (black) to 1 (white).
    #[must_use]
    pub fn luminance(self) -> f64 {
        let lin = |c: u8| {
            let v = f64::from(c) / 255.0;
            if v <= 0.040_45 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.0722f64.mul_add(
            lin(self.2),
            0.2126f64.mul_add(lin(self.0), 0.7152 * lin(self.1)),
        )
    }
}

/// Contrast ratio between two colors, 1 to 21.
#[must_use]
pub fn ratio(a: Rgb, b: Rgb) -> f64 {
    let (la, lb) = (a.luminance(), b.luminance());
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// What a pair of colors is used for, which sets its AA minimum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Use {
    /// Body text: 4.5:1 (SC 1.4.3).
    Text,
    /// Text at 18.66 px bold / 24 px regular or larger: 3:1 (SC 1.4.3).
    LargeText,
    /// Focus rings, input borders, icons that carry meaning: 3:1 (SC 1.4.11).
    NonText,
}

impl Use {
    /// The AA minimum ratio.
    #[must_use]
    pub const fn minimum(self) -> f64 {
        match self {
            Self::Text => 4.5,
            Self::LargeText | Self::NonText => 3.0,
        }
    }
}
