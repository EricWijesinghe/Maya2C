//! Phone push through ntfy: message shape, topic check, rate limit, the POST.
//!
//! A topic name works like a password on ntfy.sh, so it is read from the
//! environment by the binary and never has a default here.

use std::collections::VecDeque;

use crate::detect::Severity;
use crate::incident::Event;

/// Where pushes go. Only the topic is secret.
pub const NTFY_BASE: &str = "https://ntfy.sh";
/// Longest topic ntfy accepts.
pub const MAX_TOPIC_LEN: usize = 64;

/// Priority for a resolved incident: normal, no urgent buzz.
pub const PRIORITY_RESOLVED: u8 = 3;
/// ntfy "high".
pub const PRIORITY_WARNING: u8 = 4;
/// ntfy "urgent": the loudest notification the app raises.
pub const PRIORITY_CRITICAL: u8 = 5;

/// One push, before it is sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    /// Shown as the notification title. ASCII.
    pub title: String,
    /// The notification body.
    pub body: String,
    /// ntfy priority, 1 (min) to 5 (urgent).
    pub priority: u8,
    /// ntfy emoji tags.
    pub tags: &'static str,
}

/// Maps an event to the push it should produce.
#[must_use]
pub fn render(event: &Event) -> Message {
    match event {
        Event::Raised(f) => {
            let (priority, tags) = match f.severity {
                Severity::Critical => (PRIORITY_CRITICAL, "rotating_light"),
                Severity::Warning => (PRIORITY_WARNING, "warning"),
            };
            Message {
                title: f.title.clone(),
                body: f.body.clone(),
                priority,
                tags,
            }
        }
        Event::Resolved { title, .. } => Message {
            title: format!("RESOLVED: {title}"),
            body: "condition has cleared".into(),
            priority: PRIORITY_RESOLVED,
            tags: "white_check_mark",
        },
    }
}

/// Accepts only what ntfy accepts: letters, digits, `-` and `_`, 1 to 64.
///
/// # Errors
///
/// Returns a reason when the topic is empty, too long or has other characters.
pub fn valid_topic(topic: &str) -> Result<&str, String> {
    let ok = !topic.is_empty()
        && topic.len() <= MAX_TOPIC_LEN
        && topic
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if ok {
        Ok(topic)
    } else {
        Err(format!(
            "ntfy topic must be 1-{MAX_TOPIC_LEN} letters, digits, '-' or '_'"
        ))
    }
}

/// At most `max` pushes per `window_ms`. Excess pushes are dropped by the caller,
/// which logs them.
#[derive(Debug)]
pub struct RateLimit {
    window_ms: u64,
    max: usize,
    sent: VecDeque<u64>,
}

impl RateLimit {
    /// A limit of `max` pushes in any `window_ms` span.
    #[must_use]
    pub fn new(max: usize, window_ms: u64) -> Self {
        Self {
            window_ms,
            max,
            sent: VecDeque::new(),
        }
    }

    /// Whether a push may go out at `now`. Records it when it may.
    pub fn allow(&mut self, now: u64) -> bool {
        while let Some(&front) = self.sent.front() {
            if now.saturating_sub(front) < self.window_ms {
                break;
            }
            self.sent.pop_front();
        }
        if self.sent.len() < self.max {
            self.sent.push_back(now);
            true
        } else {
            false
        }
    }
}

/// POSTs one message to `base/topic`.
///
/// # Errors
///
/// Returns the transport or HTTP status error as text.
pub async fn send(
    http: &reqwest::Client,
    base: &str,
    topic: &str,
    m: &Message,
) -> Result<(), String> {
    http.post(format!("{base}/{topic}"))
        .header("Title", &m.title)
        .header("Priority", m.priority.to_string())
        .header("Tags", m.tags)
        .body(m.body.clone())
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::Finding;

    fn raised(sev: Severity) -> Event {
        Event::Raised(Finding {
            key: "halt".into(),
            severity: sev,
            title: "Maya2C chain halted".into(),
            body: "no new block".into(),
        })
    }

    #[test]
    fn severity_maps_to_ntfy_priority() {
        assert_eq!(render(&raised(Severity::Critical)).priority, 5);
        assert_eq!(render(&raised(Severity::Warning)).priority, 4);
        let resolved = Event::Resolved {
            key: "halt".into(),
            title: "Maya2C chain halted".into(),
        };
        let m = render(&resolved);
        assert_eq!(m.priority, PRIORITY_RESOLVED);
        assert_eq!(m.title, "RESOLVED: Maya2C chain halted");
    }

    #[test]
    fn topic_must_be_a_plain_name() {
        assert!(valid_topic("maya2c-ops_7Kq").is_ok());
        assert!(valid_topic("").is_err());
        assert!(valid_topic("a/b").is_err());
        assert!(valid_topic("a?x=1").is_err());
        assert!(valid_topic(&"x".repeat(MAX_TOPIC_LEN + 1)).is_err());
    }

    #[test]
    fn rate_limit_drops_past_the_cap_and_recovers_after_the_window() {
        let mut rl = RateLimit::new(2, 1_000);
        assert!(rl.allow(0));
        assert!(rl.allow(10));
        assert!(!rl.allow(20), "third push inside the window is dropped");
        assert!(rl.allow(1_000), "the first push has left the window");
    }
}
