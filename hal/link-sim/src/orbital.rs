//! Orbital routing: light-time delay, line of sight, and Doppler. SIM.
//!
//! Distances are the textbook ranges, not an ephemeris: Earth-Moon
//! 356,500-406,700 km; Earth-Mars 54.6M km at closest approach to 401M km
//! at conjunction, which is 3.0 to 22.3 light-minutes — the brief's
//! "3-22 minute delays".

/// Speed of light, m/s.
pub const C_M_S: u64 = 299_792_458;

/// One-way light time over `km`, microseconds.
pub fn light_time_us(km: u64) -> u64 {
    // km · 1000 / c seconds = km · 10^9 / c microseconds.
    u64::try_from(u128::from(km) * 1_000_000_000 / u128::from(C_M_S)).unwrap_or(u64::MAX)
}

/// Earth-Mars distance at closest approach, km.
pub const EARTH_MARS_MIN_KM: u64 = 54_600_000;
/// Earth-Mars distance at conjunction, km.
pub const EARTH_MARS_MAX_KM: u64 = 401_000_000;
/// Earth-Moon mean distance, km.
pub const EARTH_MOON_KM: u64 = 384_400;

/// Whether two points above a sphere of radius `r_km` can see each other:
/// the segment between them must stay above the surface. Integer geometry on
/// a 2D slice (positions in km from the centre).
pub fn line_of_sight(a: (i64, i64), b: (i64, i64), r_km: i64) -> bool {
    // Distance from the origin to segment AB, squared, compared with r².
    let (dx, dy) = (i128::from(b.0 - a.0), i128::from(b.1 - a.1));
    let (ax, ay) = (i128::from(a.0), i128::from(a.1));
    let len2 = dx * dx + dy * dy;
    if len2 == 0 {
        return ax * ax + ay * ay >= i128::from(r_km).pow(2);
    }
    // Parameter of the closest point, clamped to the segment, scaled by len2.
    let t = (-(ax * dx + ay * dy)).clamp(0, len2);
    let cx = ax * len2 + t * dx;
    let cy = ay * len2 + t * dy;
    // |C|² ≥ r²·len2² ⇔ closest point is outside the sphere.
    cx * cx + cy * cy >= i128::from(r_km).pow(2) * len2 * len2
}

/// Doppler shift of a carrier at `carrier_hz` for radial velocity `v_m_s`
/// (positive = receding), in Hz, first order.
pub fn doppler_hz(carrier_hz: u64, v_m_s: i64) -> i64 {
    let shift = -i128::from(v_m_s) * i128::from(carrier_hz) / i128::from(C_M_S);
    i64::try_from(shift).unwrap_or(0)
}
