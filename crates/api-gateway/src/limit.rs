//! Per-client rate limiting, now implemented in `maya-web-security`.
//!
//! Re-exported unchanged so `maya_api_gateway::limit::*` keeps its paths. The
//! explorer's public API uses the same limiter from `maya-web-security`
//! directly; `crates/web-security/src/rate_limit.rs` documents how clients are
//! identified (`ClientIp`) and why a header is only trusted behind a proxy.

pub use maya_web_security::rate_limit::*;
