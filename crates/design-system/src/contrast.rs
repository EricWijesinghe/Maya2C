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
        0.0722f64.mul_add(
            linear(self.2),
            0.2126f64.mul_add(linear(self.0), 0.7152 * linear(self.1)),
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
    /// A panel against the page ground with no hairline: 1.25:1. A house rule
    /// for Forged Light 2 (ADR-136), not a WCAG criterion.
    Separation,
}

impl Use {
    /// The minimum ratio: WCAG AA for the first three, the house rule for
    /// `Separation`.
    #[must_use]
    pub const fn minimum(self) -> f64 {
        match self {
            Self::Text => 4.5,
            Self::LargeText | Self::NonText => 3.0,
            Self::Separation => 1.25,
        }
    }
}

/// A colour-vision model for checking that chart series stay apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Vision {
    /// Typical trichromat vision.
    Normal,
    /// No working M cones (the commonest red-green deficiency).
    Deuteranopia,
    /// No working L cones.
    Protanopia,
}

impl Vision {
    /// Every model, in the order a report prints them.
    pub const ALL: [Self; 3] = [Self::Normal, Self::Deuteranopia, Self::Protanopia];

    /// Lower-case name for reports.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Deuteranopia => "deuteranopia",
            Self::Protanopia => "protanopia",
        }
    }

    /// Machado, Oliveira and Fernandes (2009), severity 1.0, on linear sRGB.
    const fn matrix(self) -> Option<[[f64; 3]; 3]> {
        match self {
            Self::Normal => None,
            Self::Deuteranopia => Some([
                [0.367_322, 0.860_646, -0.227_968],
                [0.280_085, 0.672_501, 0.047_413],
                [-0.011_820, 0.042_940, 0.968_881],
            ]),
            Self::Protanopia => Some([
                [0.152_286, 1.052_583, -0.204_868],
                [0.114_503, 0.786_281, 0.099_216],
                [-0.003_882, -0.048_116, 1.051_998],
            ]),
        }
    }
}

fn linear(c: u8) -> f64 {
    let v = f64::from(c) / 255.0;
    if v <= 0.040_45 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

impl Rgb {
    /// The colour as `vision` sees it, in `OKLab` (L, a, b), L from 0 to 1.
    #[must_use]
    pub fn oklab(self, vision: Vision) -> [f64; 3] {
        let mut rgb = [linear(self.0), linear(self.1), linear(self.2)];
        if let Some(m) = vision.matrix() {
            rgb = m.map(|row| {
                row[2]
                    .mul_add(rgb[2], row[0].mul_add(rgb[0], row[1] * rgb[1]))
                    .clamp(0.0, 1.0)
            });
        }
        let [r, g, b] = rgb;
        let lms = [
            [0.412_221_470_8_f64, 0.536_332_536_3, 0.051_445_992_9],
            [0.211_903_498_2, 0.680_699_545_1, 0.107_396_956_6],
            [0.088_302_461_9, 0.281_718_837_6, 0.629_978_700_5],
        ]
        .map(|k| k[2].mul_add(b, k[0].mul_add(r, k[1] * g)).cbrt());
        let [l, m, s] = lms;
        [
            0.210_454_255_3f64.mul_add(l, 0.793_617_785f64.mul_add(m, -0.004_072_046_8 * s)),
            1.977_998_495_1f64.mul_add(l, (-2.428_592_205f64).mul_add(m, 0.450_593_709_9 * s)),
            0.025_904_037_1f64.mul_add(l, 0.782_771_766_2f64.mul_add(m, -0.808_675_766 * s)),
        ]
    }
}

/// Euclidean `OKLab` distance between two colours as `vision` sees them.
#[must_use]
pub fn distance(a: Rgb, b: Rgb, vision: Vision) -> f64 {
    let (p, q) = (a.oklab(vision), b.oklab(vision));
    (p[0] - q[0]).hypot(p[1] - q[1]).hypot(p[2] - q[2])
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn oklab_of_white_and_black() {
        let w = Rgb(255, 255, 255).oklab(Vision::Normal);
        assert!(
            (w[0] - 1.0).abs() < 1e-3 && w[1].abs() < 1e-3 && w[2].abs() < 1e-3,
            "{w:?}"
        );
        let k = Rgb(0, 0, 0).oklab(Vision::Normal);
        assert!(k.iter().all(|v| v.abs() < 1e-9), "{k:?}");
    }

    #[test]
    fn red_and_green_collapse_under_red_green_deficiency() {
        // The pair a colour-blind reader cannot tell apart, while blue and
        // yellow stay apart under every model.
        let (red, green) = (Rgb::hex("#d03030").unwrap(), Rgb::hex("#3a8a2a").unwrap());
        let normal = distance(red, green, Vision::Normal);
        for v in [Vision::Deuteranopia, Vision::Protanopia] {
            assert!(distance(red, green, v) < normal / 2.0, "{v:?}");
        }
        let (blue, yellow) = (Rgb::hex("#2f6bff").unwrap(), Rgb::hex("#ffd43b").unwrap());
        for v in Vision::ALL {
            assert!(distance(blue, yellow, v) > 0.3, "{v:?}");
        }
    }

    #[test]
    fn grey_is_unchanged_by_the_simulation() {
        let g = Rgb::hex("#808080").unwrap();
        // Every Machado row sums to 1, so a neutral stays where it was.
        for v in Vision::ALL {
            let (n, s) = (g.oklab(Vision::Normal), g.oklab(v));
            assert!((n[0] - s[0]).abs() < 2e-3, "{v:?} {n:?} {s:?}");
        }
    }
}
