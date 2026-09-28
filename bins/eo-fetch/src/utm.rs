//! WGS84 longitude/latitude to UTM easting/northing (north hemisphere), by
//! the standard Transverse Mercator series (Snyder, "Map Projections — A
//! Working Manual", USGS 1987, equations 8-9 to 8-11). Accurate to well under
//! a metre inside a zone, which is a tenth of a Sentinel-2 pixel.

const A: f64 = 6_378_137.0;
const F: f64 = 1.0 / 298.257_223_563;
const K0: f64 = 0.9996;
const FALSE_EASTING: f64 = 500_000.0;

/// The UTM zone of a longitude.
#[must_use]
pub fn zone(lon: f64) -> u32 {
    // Longitudes are -180..180, so the zone is 1..=60.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let z = ((lon + 180.0) / 6.0).floor() as u32 + 1;
    z.min(60)
}

/// `(easting, northing)` in metres, in the longitude's own zone.
#[must_use]
pub fn forward(lon: f64, lat: f64) -> (f64, f64) {
    let e2 = F * (2.0 - F);
    let ep2 = e2 / (1.0 - e2);
    let lon0 = f64::from(zone(lon) - 1) * 6.0 - 180.0 + 3.0;
    let (phi, lam) = (lat.to_radians(), (lon - lon0).to_radians());
    let n = A / (1.0 - e2 * phi.sin().powi(2)).sqrt();
    let t = phi.tan().powi(2);
    let c = ep2 * phi.cos().powi(2);
    let a = phi.cos() * lam;
    let m = A
        * ((1.0 - e2 / 4.0 - 3.0 * e2 * e2 / 64.0 - 5.0 * e2.powi(3) / 256.0) * phi
            - (3.0 * e2 / 8.0 + 3.0 * e2 * e2 / 32.0 + 45.0 * e2.powi(3) / 1024.0)
                * (2.0 * phi).sin()
            + (15.0 * e2 * e2 / 256.0 + 45.0 * e2.powi(3) / 1024.0) * (4.0 * phi).sin()
            - (35.0 * e2.powi(3) / 3072.0) * (6.0 * phi).sin());
    let easting = FALSE_EASTING
        + K0 * n
            * (a + (1.0 - t + c) * a.powi(3) / 6.0
                + (5.0 - 18.0 * t + t * t + 72.0 * c - 58.0 * ep2) * a.powi(5) / 120.0);
    let northing = K0
        * (m + n
            * phi.tan()
            * (a * a / 2.0
                + (5.0 - t + 9.0 * c + 4.0 * c * c) * a.powi(4) / 24.0
                + (61.0 - 58.0 * t + t * t + 600.0 * c - 330.0 * ep2) * a.powi(6) / 720.0));
    (easting, northing)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_proj_for_a_central_valley_point() {
        // pyproj (PROJ) EPSG:4326 -> EPSG:32610 for (-120.30, 36.95):
        // 740417.490 E, 4092732.334 N.
        let (e, n) = forward(-120.30, 36.95);
        assert_eq!(zone(-120.30), 10);
        assert!((e - 740_417.490).abs() < 0.001, "{e}");
        assert!((n - 4_092_732.334).abs() < 0.001, "{n}");
    }
}
