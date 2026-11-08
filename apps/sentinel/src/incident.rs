//! Turns level-triggered findings into one alert per incident.
//!
//! A finding that appears is raised once. It stays one incident while it keeps
//! appearing. It resolves only after it has been absent for `clear_ms`, so a
//! condition that flaps around a threshold does not page twice.

use std::collections::BTreeMap;

use crate::detect::Finding;

/// What the notifier is asked to say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// A new incident.
    Raised(Finding),
    /// An incident that has cleared.
    Resolved {
        /// The incident's key.
        key: String,
        /// Its headline, so the resolve message names what cleared.
        title: String,
    },
}

#[derive(Debug)]
struct Open {
    finding: Finding,
    last_seen: u64,
}

/// The set of open incidents.
#[derive(Debug)]
pub struct Incidents {
    clear_ms: u64,
    open: BTreeMap<String, Open>,
}

impl Incidents {
    /// Incidents that clear after `clear_ms` without their finding.
    #[must_use]
    pub const fn new(clear_ms: u64) -> Self {
        Self {
            clear_ms,
            open: BTreeMap::new(),
        }
    }

    /// Folds this poll's findings in and returns what changed.
    pub fn update(&mut self, now: u64, findings: Vec<Finding>) -> Vec<Event> {
        let mut events = Vec::new();
        for f in findings {
            if let Some(o) = self.open.get_mut(&f.key) {
                o.last_seen = now;
                continue;
            }
            events.push(Event::Raised(f.clone()));
            self.open.insert(
                f.key.clone(),
                Open {
                    finding: f,
                    last_seen: now,
                },
            );
        }
        let cleared: Vec<String> = self
            .open
            .iter()
            .filter(|(_, o)| now.saturating_sub(o.last_seen) >= self.clear_ms && o.last_seen != now)
            .map(|(k, _)| k.clone())
            .collect();
        for key in cleared {
            if let Some(o) = self.open.remove(&key) {
                events.push(Event::Resolved {
                    key,
                    title: o.finding.title,
                });
            }
        }
        events
    }
}
