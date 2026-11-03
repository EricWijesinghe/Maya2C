//! OS toasts for the alerts that need the user to act. Off until the user
//! turns them on in Settings. A toast carries the device name and a fixed
//! sentence for the alert's kind, never an address, a key or miner output:
//! the raw alert text can hold error strings, so it is not shown outside the Hub.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};

use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;

use crate::miner::{Alert, AlertKind};

/// Shortest gap between two toasts of one kind. A crash loop or a hot GPU
/// raises its alert every few seconds; ten minutes keeps a toast a signal
/// rather than the noise that gets the feature switched off.
const MIN_INTERVAL_MS: u64 = 10 * 60 * 1000;

/// Decides which alerts become toasts. Holds no OS handle, so it is testable.
#[derive(Debug, Default)]
pub struct Notifier {
    enabled: bool,
    last_shown: HashMap<AlertKind, u64>,
}

impl Notifier {
    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
    }

    /// True when this alert should become a toast now, and records that it did.
    pub fn admit(&mut self, alert: &Alert) -> bool {
        if !self.enabled || !alert.kind.notifies() {
            return false;
        }
        // `abs_diff` rather than `saturating_sub`: a clock that steps backwards
        // must not silence the kind until the old time is passed again.
        let due = self
            .last_shown
            .get(&alert.kind)
            .is_none_or(|&last| alert.t_ms.abs_diff(last) >= MIN_INTERVAL_MS);
        if due {
            self.last_shown.insert(alert.kind, alert.t_ms);
        }
        due
    }
}

/// The notifier's lock. A poisoned lock still holds valid flags, so it is reused.
pub fn lock(state: &Mutex<Notifier>) -> MutexGuard<'_, Notifier> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The toast text, built from the kind and the device only.
#[must_use]
pub fn body(alert: &Alert) -> String {
    match alert.kind {
        AlertKind::GaveUp => format!(
            "{} stopped after repeated exits. Open the Hub to see why.",
            alert.device
        ),
        AlertKind::Thermal => format!(
            "{} stopped at its temperature ceiling. It restarts once it cools.",
            alert.device
        ),
        _ => format!("{} needs attention. Open the Hub.", alert.device),
    }
}

/// Shows one toast. A refused toast loses nothing: the alert stays in the Hub's log.
pub fn show(app: &AppHandle, alert: &Alert) {
    let _ = app
        .notification()
        .builder()
        .title("Maya2C Hub")
        .body(body(alert))
        .show();
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR_MS: u64 = 60 * 60 * 1000;

    fn alert(kind: AlertKind, t_ms: u64) -> Alert {
        Alert {
            t_ms,
            kind,
            device: "gpu0".into(),
            level: "critical",
            message: "raw text the toast must not show".into(),
        }
    }

    #[test]
    fn off_by_default_admits_nothing() {
        let mut n = Notifier::default();
        assert!(!n.admit(&alert(AlertKind::GaveUp, HOUR_MS)));
    }

    #[test]
    fn enabled_admits_the_first_alert_then_limits_its_kind() {
        let mut n = Notifier::default();
        n.set_enabled(true);
        assert!(n.admit(&alert(AlertKind::Thermal, HOUR_MS)));
        assert!(!n.admit(&alert(AlertKind::Thermal, HOUR_MS + MIN_INTERVAL_MS - 1)));
        assert!(n.admit(&alert(AlertKind::Thermal, HOUR_MS + MIN_INTERVAL_MS)));
    }

    #[test]
    fn kinds_are_limited_independently() {
        let mut n = Notifier::default();
        n.set_enabled(true);
        assert!(n.admit(&alert(AlertKind::Thermal, HOUR_MS)));
        assert!(n.admit(&alert(AlertKind::GaveUp, HOUR_MS)));
    }

    #[test]
    fn kinds_that_need_no_action_never_toast() {
        let mut n = Notifier::default();
        n.set_enabled(true);
        assert!(!n.admit(&alert(AlertKind::Exited, HOUR_MS)));
        assert!(!n.admit(&alert(AlertKind::Restarted, HOUR_MS)));
        assert!(!n.admit(&alert(AlertKind::Resumed, HOUR_MS)));
    }

    #[test]
    fn a_clock_step_backwards_does_not_silence_the_kind() {
        let mut n = Notifier::default();
        n.set_enabled(true);
        assert!(n.admit(&alert(AlertKind::GaveUp, HOUR_MS)));
        assert!(n.admit(&alert(AlertKind::GaveUp, HOUR_MS - MIN_INTERVAL_MS)));
    }

    #[test]
    fn toast_text_names_the_device_and_no_raw_alert_text() {
        for kind in [AlertKind::GaveUp, AlertKind::Thermal] {
            let text = body(&alert(kind, HOUR_MS));
            assert!(text.contains("gpu0"));
            assert!(!text.contains("raw text"));
        }
    }
}
