//! Relativistic clock-rate offsets for orbital nodes. Floats, and off the
//! consensus path: the result corrects a node's *local* clock before it ever
//! reaches the integer drift check.
//!
//! For a clock in a circular orbit of radius `r` compared with a clock on the
//! geoid, first order in `1/c²`:
//!
//! - special relativity (velocity): `−v²/(2c²)`, with `v² = GM/r`
//! - general relativity (potential): `(GM/R_E − GM/r)/c² ` relative to the
//!   surface (ignoring Earth's rotation and oblateness, which are second-order
//!   here and stated as such)

/// Earth's gravitational parameter GM, m³/s².
pub const GM_EARTH: f64 = 3.986_004_418e14;
/// Earth's mean radius, m.
pub const R_EARTH_M: f64 = 6.371e6;
/// Speed of light, m/s.
pub const C: f64 = 299_792_458.0;
/// Seconds per day.
pub const DAY_S: f64 = 86_400.0;

/// Rate offsets for an orbit, microseconds gained per day (positive = the
/// orbiting clock runs fast).
#[derive(Clone, Copy, Debug)]
pub struct RateOffset {
    /// Velocity term.
    pub special_us_per_day: f64,
    /// Potential term.
    pub general_us_per_day: f64,
}

impl RateOffset {
    /// Net offset.
    pub fn net_us_per_day(&self) -> f64 {
        self.special_us_per_day + self.general_us_per_day
    }
}

/// Offsets for a circular orbit of radius `r_m` (from Earth's centre).
pub fn circular_orbit(r_m: f64) -> RateOffset {
    let v2 = GM_EARTH / r_m;
    let special = -v2 / (2.0 * C * C);
    let general = (GM_EARTH / R_EARTH_M - GM_EARTH / r_m) / (C * C);
    RateOffset {
        special_us_per_day: special * DAY_S * 1e6,
        general_us_per_day: general * DAY_S * 1e6,
    }
}

/// GPS orbit radius (semi-major axis), m.
pub const GPS_ORBIT_M: f64 = 26_560_000.0;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gps_clocks_gain_about_38_microseconds_a_day() {
        // The textbook figures: SR −7.2 µs/day, GR +45.7 µs/day, net +38.5.
        let o = circular_orbit(GPS_ORBIT_M);
        assert!((o.special_us_per_day + 7.2).abs() < 0.2, "{}", o.special_us_per_day);
        assert!((o.general_us_per_day - 45.7).abs() < 0.3, "{}", o.general_us_per_day);
        assert!((o.net_us_per_day() - 38.5).abs() < 0.5, "{}", o.net_us_per_day());
    }

    #[test]
    fn low_orbit_clocks_run_slow() {
        // Below ~1.5 Earth radii the velocity term wins: the ISS loses time.
        assert!(circular_orbit(R_EARTH_M + 400_000.0).net_us_per_day() < 0.0);
    }
}
