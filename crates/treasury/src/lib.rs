//! Treasury and vesting rules (Master Prompt 18 §6).
//!
//! Pure state machines over integers and block heights. No clock, no floats,
//! no maps iterated for order. Not wired into the node: the treasury exists
//! at genesis (`TreasuryGenesis`) but its spending path is governance's, and
//! these rules are what that path should enforce once it is built
//! (`reports/18-economics.md`).

pub mod spending;
pub mod vesting;
