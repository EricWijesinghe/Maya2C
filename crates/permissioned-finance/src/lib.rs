//! Permissioned finance (Master Prompt 6 §4–5), as tested libraries.
//!
//! - [`cbdc`] — a permissioned vault: an issuer mints and redeems, accounts
//!   are admitted only with a STARK proof that an issuer-anchored credential
//!   meets the KYC tier, transfers respect per-tier limits and freezes.
//! - [`darkpool`] — sealed-bid batch matching: orders are hiding commitments
//!   until the batch closes, then clear at one uniform price; an auditor
//!   opening reveals exactly the orders an auditor key was given.
//! - [`tax`] — per-jurisdiction capital-gains calculators as pure integer
//!   functions over a transaction history.
//!
//! # Status and honest limits
//!
//! RESEARCH: nothing in consensus calls these. The dark pool hides orders
//! *until the batch closes* (commit-reveal), not after — the brief's MPC
//! matching that never reveals price and size is not built. The ZK-KYC proof
//! shows a credential in the issuer's tree meets the tier without revealing
//! which credential; binding that proof to the account being admitted needs
//! the subject as a public input, which the credential circuit does not yet
//! expose (listed in `reports/06-finance.md`).

pub mod cbdc;
pub mod darkpool;
pub mod tax;

/// An account.
pub type Account = [u8; 32];
