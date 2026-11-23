//! Per-client rate limiting, now implemented in `maya-web-security`.
//!
//! Re-exported so `maya_api_gateway::limit::*` keeps its paths. The move also
//! changed what the gateway counts as one client: an IPv6 client is keyed on
//! its /64, and an IPv4-mapped address on the IPv4 address it carries
//! (DECISIONS.md, 2026-11-23). The refill period is now set in nanoseconds,
//! so `--rate-per-second` is the rate enforced, and a rate above 1e9/s is
//! refused instead of silently capped. The explorer's public API uses the same
//! limiter; `crates/web-security/src/rate_limit.rs` documents how clients are
//! identified (`ClientIp`) and why a header is only trusted behind a proxy.

pub use maya_web_security::rate_limit::*;
