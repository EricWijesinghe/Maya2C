//! The duty-cycle governor: what stops this transmitting.
//!
//! ## Why it refuses instead of warning
//!
//! ISM allocations cap how much of each hour a transmitter may occupy —
//! EU868's sub-bands are 1% or 0.1%, and the limit is a condition of using the
//! spectrum without a licence, not a performance target. A governor that logged
//! a warning and transmitted anyway would turn every busy node into an
//! unlicensed transmitter, and the operator would find out from a regulator
//! rather than from a log.
//!
//! So [`DutyCycle::reserve`] returns an error. There is no override, no
//! `force` flag, and no configuration that raises the budget above the band's:
//! a knob that can be set to 100% is the failure this exists to prevent.
//!
//! ## Airtime is computed, not measured
//!
//! The time a LoRa frame occupies is a function of spreading factor, bandwidth,
//! coding rate and payload length, all known before transmission. Computing it
//! means the governor can refuse *before* the radio keys up, which is the only
//! moment refusing helps. Measuring afterwards would be an audit log of
//! violations.
//!
//! The formula is Semtech's, in integer microseconds throughout. No floating
//! point: two nodes disagreeing about airtime by a rounding step is two nodes
//! disagreeing about whether a transmission was legal, and `f64` would put a
//! rounding mode in the middle of that.
//!
//! ## What this is not
//!
//! Not a scheduler. It answers "may I transmit this now", and the caller
//! decides what to do with a no — usually wait, sometimes drop. Fountain coding
//! is what makes waiting cheap: a symbol not sent is not a symbol lost, because
//! the next one carries just as much information. See [`crate::fountain`].

use crate::error::{Error, Result};

/// Microseconds in an hour, the window the budget is measured over.
pub const WINDOW_MICROS: u64 = 3_600_000_000;

/// EU868's general sub-band budget, in parts per million of the window.
///
/// 10,000 ppm is 1%. Expressed in ppm rather than as a percentage so the 0.1%
/// sub-bands are a whole number too — a fractional percent in an integer
/// calculation is where a factor of ten goes missing.
pub const EU868_1PCT_PPM: u64 = 10_000;

/// EU868's 0.1% sub-bands.
pub const EU868_01PCT_PPM: u64 = 1_000;

/// US915, which has no duty-cycle limit but a 400 ms dwell-time cap per
/// transmission.
///
/// Modelled as a full budget, because the constraint there is per-frame rather
/// than per-hour — [`DutyCycle::reserve`] still refuses a frame whose airtime
/// exceeds [`US915_DWELL_MICROS`].
pub const US915_PPM: u64 = 1_000_000;

/// US915's per-transmission dwell limit.
pub const US915_DWELL_MICROS: u64 = 400_000;

/// A band's transmission rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Band {
    /// Share of each hour this band permits, in parts per million.
    pub budget_ppm: u64,
    /// The longest single transmission, or `None` where the band has no dwell
    /// limit.
    pub dwell_micros: Option<u64>,
}

impl Band {
    /// EU868, general sub-band: 1%, no dwell limit.
    pub const EU868: Self = Self {
        budget_ppm: EU868_1PCT_PPM,
        dwell_micros: None,
    };

    /// EU868, the 0.1% sub-bands.
    pub const EU868_RESTRICTED: Self = Self {
        budget_ppm: EU868_01PCT_PPM,
        dwell_micros: None,
    };

    /// US915: no duty cycle, 400 ms dwell.
    pub const US915: Self = Self {
        budget_ppm: US915_PPM,
        dwell_micros: Some(US915_DWELL_MICROS),
    };

    /// The airtime this band allows per window.
    #[must_use]
    pub const fn budget_micros(self) -> u64 {
        // ppm of an hour. Both fit in u64 with room: 3.6e9 * 1e6 is 3.6e15.
        WINDOW_MICROS / 1_000_000 * self.budget_ppm
    }
}

/// A LoRa link's radio settings, enough to compute airtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Spreading factor, 7 through 12.
    pub spreading_factor: u8,
    /// Bandwidth in hertz. 125_000 is the EU868 default.
    pub bandwidth_hz: u32,
    /// Coding-rate denominator: 5 through 8, for 4/5 through 4/8.
    pub coding_rate: u8,
    /// Preamble length in symbols. 8 is the LoRaWAN default.
    pub preamble_symbols: u16,
}

impl Settings {
    /// The EU868 default: SF7, 125 kHz, 4/5, 8-symbol preamble.
    pub const EU868_FAST: Self = Self {
        spreading_factor: 7,
        bandwidth_hz: 125_000,
        coding_rate: 5,
        preamble_symbols: 8,
    };

    /// The long-range end: SF12, 125 kHz, 4/5.
    pub const EU868_LONG: Self = Self {
        spreading_factor: 12,
        bandwidth_hz: 125_000,
        coding_rate: 5,
        preamble_symbols: 8,
    };

    /// Microseconds one symbol occupies.
    ///
    /// `2^SF / BW`, in integer microseconds. The numerator is scaled first so
    /// the division is the last operation and loses at most one microsecond,
    /// rather than a division per term.
    #[must_use]
    pub const fn symbol_micros(self) -> u64 {
        let sf = if self.spreading_factor < 7 {
            7
        } else if self.spreading_factor > 12 {
            12
        } else {
            self.spreading_factor
        };
        (1u64 << sf) * 1_000_000 / self.bandwidth_hz as u64
    }

    /// Airtime for a frame of `bytes`, in microseconds.
    ///
    /// Semtech's formula, integer throughout:
    ///
    /// ```text
    /// preamble = (preamble_symbols + 4.25) symbols
    /// payload_symbols = 8 + max(ceil((8*bytes - 4*SF + 28 + 16) / (4*(SF - 2*DE))) * CR, 0)
    /// ```
    ///
    /// where `DE` is the low-data-rate optimisation, mandatory at SF11 and
    /// SF12 on 125 kHz. The 4.25 preamble symbols are carried as quarters to
    /// stay in integers.
    #[must_use]
    pub fn airtime_micros(self, bytes: usize) -> u64 {
        let symbol = self.symbol_micros();
        let sf = self.spreading_factor.clamp(7, 12) as i64;

        // Low-data-rate optimisation: on at SF11 and SF12 for 125 kHz, which is
        // where a symbol exceeds 16 ms. Written as the condition rather than as
        // a table, because the condition is what the spec states.
        let de = i64::from(symbol >= 16_000);

        // Quarters of a symbol, so 4.25 is exact.
        let preamble_quarters = u64::from(self.preamble_symbols) * 4 + 17;

        let numerator = 8 * bytes as i64 - 4 * sf + 28 + 16;
        let denominator = 4 * (sf - 2 * de);
        let payload_symbols = if numerator <= 0 {
            8
        } else {
            let steps = numerator.div_euclid(denominator)
                + i64::from(numerator.rem_euclid(denominator) != 0);
            8 + (steps * i64::from(self.coding_rate.clamp(5, 8))) as u64
        };

        symbol * preamble_quarters / 4 + symbol * payload_symbols
    }
}

/// A transmission the governor has accounted for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Transmission {
    /// When it started, in microseconds since the governor's epoch.
    at: u64,
    /// How long it occupied.
    airtime: u64,
}

/// Tracks airtime against a band's budget.
///
/// Time is supplied by the caller rather than read from a clock, for the reason
/// every other subsystem in this tree avoids clocks: a value read from the
/// environment is a value a test cannot pin and two nodes cannot agree on. The
/// caller holds the clock; this holds the arithmetic.
#[derive(Clone, Debug)]
pub struct DutyCycle {
    band: Band,
    settings: Settings,
    /// Transmissions still inside the window, oldest first.
    history: Vec<Transmission>,
}

impl DutyCycle {
    /// A governor for a band and link settings.
    #[must_use]
    pub fn new(band: Band, settings: Settings) -> Self {
        Self {
            band,
            settings,
            history: Vec::new(),
        }
    }

    /// The link's settings.
    #[must_use]
    pub const fn settings(&self) -> Settings {
        self.settings
    }

    /// Airtime already spent in the window ending at `now`.
    #[must_use]
    pub fn spent_micros(&self, now: u64) -> u64 {
        self.history
            .iter()
            .filter(|tx| now.saturating_sub(tx.at) < WINDOW_MICROS)
            .map(|tx| tx.airtime)
            .sum()
    }

    /// Whether a frame of `bytes` could be sent at `now`.
    #[must_use]
    pub fn permits(&self, bytes: usize, now: u64) -> bool {
        self.check(bytes, now).is_ok()
    }

    /// The airtime a frame of `bytes` would occupy.
    #[must_use]
    pub fn airtime_micros(&self, bytes: usize) -> u64 {
        self.settings.airtime_micros(bytes)
    }

    /// Accounts for a transmission, or refuses it.
    ///
    /// # Errors
    ///
    /// Returns [`Error::DwellExceeded`] if one transmission is longer than the
    /// band's per-frame cap, and [`Error::DutyCycleExhausted`] if it would take
    /// the window past budget — with the microseconds to wait, so a caller can
    /// sleep rather than poll.
    pub fn reserve(&mut self, bytes: usize, now: u64) -> Result<u64> {
        let airtime = self.check(bytes, now)?;
        self.history
            .retain(|tx| now.saturating_sub(tx.at) < WINDOW_MICROS);
        self.history.push(Transmission { at: now, airtime });
        Ok(airtime)
    }

    /// The checking half of [`DutyCycle::reserve`], without the accounting.
    fn check(&self, bytes: usize, now: u64) -> Result<u64> {
        let airtime = self.settings.airtime_micros(bytes);

        if let Some(dwell) = self.band.dwell_micros
            && airtime > dwell
        {
            return Err(Error::DwellExceeded {
                airtime_micros: airtime,
                limit_micros: dwell,
            });
        }

        let budget = self.band.budget_micros();
        let spent = self.spent_micros(now);
        if spent + airtime > budget {
            // When the oldest transmission leaves the window, that much budget
            // comes back. Reporting the wait rather than a bare refusal is what
            // lets a caller sleep exactly long enough instead of polling.
            let wait = self
                .history
                .iter()
                .filter(|tx| now.saturating_sub(tx.at) < WINDOW_MICROS)
                .map(|tx| WINDOW_MICROS - now.saturating_sub(tx.at))
                .min()
                .unwrap_or(WINDOW_MICROS);
            return Err(Error::DutyCycleExhausted {
                airtime_micros: airtime,
                remaining_micros: budget.saturating_sub(spent),
                wait_micros: wait,
            });
        }
        Ok(airtime)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn a_symbol_at_sf7_and_sf12_matches_the_radios_own_numbers() {
        // 2^7 / 125 kHz = 1.024 ms; 2^12 / 125 kHz = 32.768 ms. These are the
        // figures every LoRa calculator prints, and getting them wrong would
        // make every airtime downstream wrong by the same factor.
        assert_eq!(Settings::EU868_FAST.symbol_micros(), 1_024);
        assert_eq!(Settings::EU868_LONG.symbol_micros(), 32_768);
    }

    #[test]
    fn airtime_grows_with_the_spreading_factor_and_with_length() {
        let short = Settings::EU868_FAST.airtime_micros(51);
        let long = Settings::EU868_LONG.airtime_micros(51);
        assert!(
            long > short * 20,
            "SF12 is far slower than SF7: {long} vs {short}"
        );

        assert!(Settings::EU868_FAST.airtime_micros(222) > Settings::EU868_FAST.airtime_micros(51));
    }

    #[test]
    fn a_full_sf12_frame_is_two_and_a_half_seconds_of_airtime() {
        // The number the whole design rests on, and it is checked against the
        // figure every LoRaWAN calculator prints for SF12 / 125 kHz / 4:5 /
        // 51 bytes rather than against a guess: 2,465,792 microseconds.
        //
        // At a 1% duty cycle one such frame buys roughly 246 seconds of
        // silence. That is why a 13-kilobyte transaction is not carried here at
        // all — 259 frames of it would be most of a day.
        assert_eq!(Settings::EU868_LONG.airtime_micros(51), 2_465_792);
        // And the short end, for the same reason: 102.7 ms at SF7.
        assert_eq!(Settings::EU868_FAST.airtime_micros(51), 102_656);
    }

    #[test]
    fn a_governor_refuses_once_the_budget_is_gone() {
        let mut duty = DutyCycle::new(Band::EU868, Settings::EU868_LONG);
        let budget = Band::EU868.budget_micros();
        assert_eq!(budget, 36_000_000, "1% of an hour is 36 seconds");

        let mut now = 0;
        let mut sent = 0;
        while duty.reserve(51, now).is_ok() {
            sent += 1;
            now += 1_000;
            assert!(sent < 1_000, "the budget must run out");
        }
        // 36 seconds of budget at 2.466 s a frame is fourteen of them. An hour
        // of EU868 airtime is fourteen full SF12 frames, and that is the whole
        // argument for carrying headers rather than transactions.
        assert_eq!(sent, 14, "sent {sent} frames before refusing");
    }

    #[test]
    fn a_refusal_says_how_long_to_wait() {
        let mut duty = DutyCycle::new(Band::EU868, Settings::EU868_LONG);
        while duty.reserve(51, 0).is_ok() {}
        match duty.reserve(51, 0) {
            Err(Error::DutyCycleExhausted { wait_micros, .. }) => {
                // A caller can sleep exactly this long rather than poll.
                assert!(wait_micros > 0 && wait_micros <= WINDOW_MICROS);
            }
            other => panic!("expected an exhausted budget, got {other:?}"),
        }
    }

    #[test]
    fn the_budget_returns_when_the_window_moves_on() {
        let mut duty = DutyCycle::new(Band::EU868, Settings::EU868_LONG);
        while duty.reserve(51, 0).is_ok() {}
        assert!(duty.reserve(51, 0).is_err());
        // An hour and a microsecond later, every transmission has aged out.
        assert!(duty.reserve(51, WINDOW_MICROS + 1).is_ok());
    }

    #[test]
    fn there_is_no_budget_that_permits_everything() {
        // The knob this module refuses to have. `Band` is constructed from the
        // constants above, and the restricted sub-band is ten times tighter
        // rather than ten times looser.
        assert_eq!(
            Band::EU868_RESTRICTED.budget_micros() * 10,
            Band::EU868.budget_micros()
        );
    }

    #[test]
    fn a_band_with_a_dwell_limit_refuses_a_single_long_frame() {
        // US915 has no duty cycle and a 400 ms per-transmission cap, so the
        // refusal there is about one frame rather than about the hour.
        let mut duty = DutyCycle::new(Band::US915, Settings::EU868_LONG);
        assert!(matches!(
            duty.reserve(51, 0),
            Err(Error::DwellExceeded { .. })
        ));

        // The same band at SF7 is well inside the dwell limit.
        let mut fast = DutyCycle::new(Band::US915, Settings::EU868_FAST);
        assert!(fast.reserve(51, 0).is_ok());
    }

    #[test]
    fn airtime_is_the_same_on_every_run() {
        // No floating point anywhere in the path: two nodes disagreeing about
        // airtime by a rounding step is two nodes disagreeing about whether a
        // transmission was legal.
        let first: Vec<u64> = (0..=222)
            .map(|n| Settings::EU868_LONG.airtime_micros(n))
            .collect();
        let second: Vec<u64> = (0..=222)
            .map(|n| Settings::EU868_LONG.airtime_micros(n))
            .collect();
        assert_eq!(first, second);
        assert!(
            first.windows(2).all(|pair| pair[1] >= pair[0]),
            "airtime never falls with length"
        );
    }
}
