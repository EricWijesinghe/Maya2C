//! IEEE 1547-2018 abnormal-frequency and abnormal-voltage trip checks for a
//! distributed energy resource.
//!
//! IEEE 1547 is a performance standard, not a wire protocol: its settings
//! reach an inverter over `SunSpec` Modbus, DNP3 or IEEE 2030.5. What is REAL
//! here is the rule — an excursion beyond a threshold that lasts its clearing
//! time must trip — applied to measured samples. The default settings below
//! are the standard's defaults for 60 Hz systems (frequency, all categories)
//! and Category II (voltage), transcribed, not read from a purchased copy of
//! the standard: an interconnection must use the utility's settings.
//!
//! Units are integers: millihertz, thousandths of nominal voltage, and
//! milliseconds. No float reaches a decision.

/// A trip element: a threshold, a direction and a clearing time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Element {
    /// Name as the standard uses it (OF1, UV2, ...).
    pub name: &'static str,
    /// Threshold, in the monitored unit.
    pub threshold: u32,
    /// Trips above the threshold if true, below it if false.
    pub over: bool,
    /// How long the excursion must last, in ms.
    pub clearing_ms: u64,
}

impl Element {
    fn exceeded(&self, value: u32) -> bool {
        if self.over {
            value > self.threshold
        } else {
            value < self.threshold
        }
    }
}

/// Default frequency trip settings, 60 Hz systems, in mHz.
pub const FREQUENCY_DEFAULTS: [Element; 4] = [
    Element {
        name: "OF2",
        threshold: 62_000,
        over: true,
        clearing_ms: 160,
    },
    Element {
        name: "OF1",
        threshold: 61_200,
        over: true,
        clearing_ms: 300_000,
    },
    Element {
        name: "UF1",
        threshold: 58_500,
        over: false,
        clearing_ms: 300_000,
    },
    Element {
        name: "UF2",
        threshold: 56_500,
        over: false,
        clearing_ms: 160,
    },
];

/// Default Category II voltage trip settings, in thousandths of nominal.
pub const VOLTAGE_DEFAULTS_CAT_II: [Element; 4] = [
    Element {
        name: "OV2",
        threshold: 1_200,
        over: true,
        clearing_ms: 160,
    },
    Element {
        name: "OV1",
        threshold: 1_100,
        over: true,
        clearing_ms: 2_000,
    },
    Element {
        name: "UV1",
        threshold: 700,
        over: false,
        clearing_ms: 10_000,
    },
    Element {
        name: "UV2",
        threshold: 450,
        over: false,
        clearing_ms: 160,
    },
];

/// Watches one quantity against a set of elements.
#[derive(Clone, Debug)]
pub struct Monitor<const N: usize> {
    elements: [Element; N],
    /// When each element's current excursion began.
    since: [Option<u64>; N],
    last_ms: Option<u64>,
}

/// Why a sample was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SampleError {
    /// Timestamps must not go backwards.
    #[error("sample at {at} ms after one at {last} ms")]
    OutOfOrder {
        /// This sample.
        at: u64,
        /// The previous one.
        last: u64,
    },
}

impl<const N: usize> Monitor<N> {
    /// A monitor over `elements`.
    #[must_use]
    pub fn new(elements: [Element; N]) -> Self {
        Self {
            elements,
            since: [None; N],
            last_ms: None,
        }
    }

    /// Feeds a sample; returns the elements that must trip now. An excursion
    /// is measured from the first sample beyond the threshold, and a sample
    /// back inside it resets that element.
    ///
    /// # Errors
    ///
    /// A sample older than the previous one.
    pub fn sample(&mut self, at_ms: u64, value: u32) -> Result<Vec<Element>, SampleError> {
        if let Some(last) = self.last_ms
            && at_ms < last
        {
            return Err(SampleError::OutOfOrder { at: at_ms, last });
        }
        self.last_ms = Some(at_ms);
        let mut trips = Vec::new();
        for (element, since) in self.elements.iter().zip(self.since.iter_mut()) {
            if !element.exceeded(value) {
                *since = None;
                continue;
            }
            let start = *since.get_or_insert(at_ms);
            if at_ms - start >= element.clearing_ms {
                trips.push(*element);
            }
        }
        Ok(trips)
    }
}
