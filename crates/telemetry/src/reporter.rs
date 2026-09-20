//! The miner's outbound half: post a rate, forget about it.
//!
//! # Telemetry must never be able to stop a miner
//!
//! That is the entire design constraint. A collector that is down, slow,
//! misconfigured, or returning 500s has to cost the miner nothing but a log
//! line — a miner that stalls its dispatch loop because a dashboard is
//! unreachable has been made worse by being observed.
//!
//! Three consequences:
//!
//! - [`Reporter::spawn`] returns immediately and reports from its own task.
//!   Nothing in the mining loop awaits it.
//! - Every failure is logged at debug and dropped. There is no retry queue,
//!   because a queue of stale rates is worse than a gap: the dashboard's TTL
//!   already treats a missing report as "unknown", which is the truth.
//! - The interval is fixed and the request has a timeout, so a hung collector
//!   parks one task rather than accumulating them.
//!
//! # Opt-in, and it says where it is sending
//!
//! Reporting is off unless a collector URL is given, and the miner logs the
//! destination once at startup. A miner that phoned home by default would be
//! a miner that phoned home by default.

use std::sync::Arc;
use std::time::Duration;

use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;

use crate::meter::HashMeter;
use crate::report::{MinerReport, Report, ReporterId};

/// How often a miner reports.
///
/// Well inside `REPORT_TTL_SECONDS`, so a single dropped report does not make
/// a live miner vanish from the dashboard.
pub const REPORT_INTERVAL: Duration = Duration::from_secs(60);

/// How long a single post may take.
///
/// Short: this is a status update, and there is another one in a minute.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// A miner's periodic reporter.
pub struct Reporter {
    url: String,
    reporter: ReporterId,
    backend: String,
    device: String,
    meter: Arc<HashMeter>,
}

impl Reporter {
    /// Builds a reporter.
    ///
    /// # Errors
    ///
    /// The [`crate::report::ReportError`] if the id or labels are outside what
    /// a collector accepts — checked here, at startup, rather than discovered
    /// as a silent 400 every minute for the life of the process.
    pub fn new(
        url: impl Into<String>,
        reporter: &str,
        backend: impl Into<String>,
        device: impl Into<String>,
        meter: Arc<HashMeter>,
    ) -> Result<Self, crate::report::ReportError> {
        let reporter = ReporterId::parse(reporter)?;
        let backend = backend.into();
        let device = device.into();

        // Validate the labels now, with a throwaway report, for the same
        // reason.
        Report::Miner(MinerReport {
            reporter: reporter.clone(),
            hash_rate: 0,
            backend: backend.clone(),
            device: device.clone(),
        })
        .validate()?;

        Ok(Self {
            url: url.into(),
            reporter,
            backend,
            device,
            meter,
        })
    }

    /// The report this reporter would send right now.
    ///
    /// Public so a miner can log it, and so a test can check the rate reaches
    /// the wire without standing up a collector.
    #[must_use]
    pub fn current(&self) -> Report {
        Report::Miner(MinerReport {
            reporter: self.reporter.clone(),
            hash_rate: self.meter.rate(),
            backend: self.backend.clone(),
            device: self.device.clone(),
        })
    }

    /// Posts one report and waits for the answer.
    ///
    /// For a process that ends — a benchmark, a one-shot script. A miner that
    /// keeps running uses [`Reporter::spawn`] instead, because a report is
    /// only meaningful while it is fresh.
    ///
    /// # Errors
    ///
    /// A description of what went wrong, for logging. The caller should not
    /// treat it as fatal: a collector being down is not a problem with the
    /// miner.
    pub async fn post_once(&self) -> Result<(), String> {
        let client: Client<_, String> = Client::builder(TokioExecutor::new()).build_http();
        self.post(&client).await
    }

    /// Starts reporting on a background task.
    ///
    /// Returns immediately. The returned handle can be dropped — the task
    /// ends when the runtime does.
    pub fn spawn(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let client: Client<_, String> = Client::builder(TokioExecutor::new()).build_http();
            let mut ticker = tokio::time::interval(REPORT_INTERVAL);
            // A missed tick means the previous post was slow. Skipping it is
            // right: bursting to catch up would post two identical rates back
            // to back.
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            loop {
                ticker.tick().await;
                if let Err(error) = self.post(&client).await {
                    // Debug, not warn. A collector being down is not a
                    // problem with the miner, and a miner that logged a
                    // warning every minute about somebody else's dashboard
                    // would train its operator to ignore warnings.
                    tracing::debug!(%error, url = %self.url, "telemetry report not delivered");
                }
            }
        })
    }

    /// Posts one report.
    async fn post<C>(&self, client: &Client<C, String>) -> Result<(), String>
    where
        C: hyper_util::client::legacy::connect::Connect + Clone + Send + Sync + 'static,
    {
        let body = serde_json::to_string(&self.current()).map_err(|e| e.to_string())?;

        let request = hyper::Request::builder()
            .method(hyper::Method::POST)
            .uri(&self.url)
            .header(hyper::header::CONTENT_TYPE, "application/json")
            .body(body)
            .map_err(|e| e.to_string())?;

        let response = tokio::time::timeout(REQUEST_TIMEOUT, client.request(request))
            .await
            .map_err(|_| format!("no response within {REQUEST_TIMEOUT:?}"))?
            .map_err(|e| e.to_string())?;

        if response.status().is_success() {
            Ok(())
        } else {
            Err(format!("collector answered {}", response.status()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meter() -> Arc<HashMeter> {
        Arc::new(HashMeter::new())
    }

    #[test]
    fn a_bad_reporter_id_fails_at_startup_not_every_minute() {
        // A silent 400 once a minute for the life of the process is a miner
        // that believes it is reporting and is not.
        assert!(
            Reporter::new(
                "http://localhost:9000/report",
                "rig 1",
                "wgpu",
                "test",
                meter()
            )
            .is_err()
        );
    }

    #[test]
    fn a_device_string_that_a_collector_would_reject_fails_at_startup() {
        let long = "x".repeat(crate::report::MAX_NAME_LEN + 1);
        assert!(
            Reporter::new(
                "http://localhost:9000/report",
                "rig-1",
                "wgpu",
                long,
                meter()
            )
            .is_err()
        );
    }

    #[test]
    fn the_report_carries_the_meters_current_rate() {
        let start = std::time::Instant::now();
        let meter = Arc::new(HashMeter::starting_at(start));
        meter.record(1_000);

        let reporter = Reporter::new(
            "http://localhost:9000/report",
            "rig-1",
            "wgpu",
            "test device",
            Arc::clone(&meter),
        )
        .expect("valid");

        match reporter.current() {
            Report::Miner(report) => {
                assert_eq!(report.backend, "wgpu");
                assert_eq!(report.device, "test device");
                assert_eq!(report.reporter.as_str(), "rig-1");
                // The rate itself is the meter's business and is tested there;
                // what matters here is that a recorded batch reaches the wire
                // rather than a zero.
                assert!(
                    report.hash_rate > 0,
                    "the meter's rate must reach the report"
                );
            }
            Report::Node(_) => panic!("a miner reports as a miner"),
        }
    }

    #[test]
    fn a_report_this_reporter_builds_is_one_a_collector_accepts() {
        // The two halves are in one crate precisely so this can be checked.
        let reporter = Reporter::new(
            "http://localhost:9000/report",
            "rig-1",
            "wgpu",
            "NVIDIA GeForce RTX 4090",
            meter(),
        )
        .expect("valid");

        reporter
            .current()
            .validate()
            .expect("what the miner sends must be what the collector takes");
    }
}
