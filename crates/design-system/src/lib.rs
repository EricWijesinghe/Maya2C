//! One design system for the wallet, explorer, portal and dev hub
//! (Master Prompt 29).
//!
//! "World-class" is defined by measurements here, so every part of this crate
//! is something a test can hold to account:
//!
//! - [`tokens`]: color, type, spacing and motion for two themes, emitted as CSS
//!   custom properties. Every text/background pair the components use is
//!   listed in [`tokens::REQUIRED_PAIRS`] and checked against WCAG 2.2 AA by
//!   [`contrast`].
//! - [`flows`]: the wallet flows as state graphs. A test proves no flow has a
//!   dead end and every flow has an error state with a way out. This is a model
//!   of the flows, not the rendered UI: the browser-driven suite in
//!   `apps/wallet-gui/e2e` has still never run (see its README).
//! - [`guard`]: address-poisoning, look-alike-token and phishing-domain
//!   detection for the send and connect screens.
//! - [`i18n`]: the message catalog, ten locales, with text direction.
//! - [`specimen`]: the component page the screenshot-diff job captures.
//!
//! UI code, not consensus code: floats are allowed (contrast math), and nothing
//! here is linked by the node.

pub mod contrast;
pub mod flows;
pub mod guard;
pub mod i18n;
pub mod specimen;
pub mod tokens;
