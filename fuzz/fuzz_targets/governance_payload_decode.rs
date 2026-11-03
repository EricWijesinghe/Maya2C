//! Fuzzes the governance payload decoders: work claims, stake locks and
//! unlocks, proposals, ballots and council actions.
//!
//! These are the bytes by which the chain amends its own rules, so a decoder
//! that accepted a non-canonical encoding would let two spellings of one vote
//! or one proposal reach consensus. The first byte selects the decoder; the
//! rest is the section. Shares the canonicality and round-trip property with
//! `payload_decode`.

#![no_main]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::core::council_payload::CouncilAction;
use custom_l1_node::core::governance_payload::{
    Ballot, ProposalSubmission, StakeLock, StakeUnlock, WorkClaim,
};
use libfuzzer_sys::fuzz_target;

#[path = "common/section.rs"]
mod section;

fuzz_target!(|data: &[u8]| {
    let Some((&selector, payload)) = data.split_first() else {
        return;
    };
    match selector % 6 {
        0 => section::check_section(
            payload,
            "work claim",
            WorkClaim::decode,
            WorkClaim::encode_into,
        ),
        1 => section::check_section(
            payload,
            "stake lock",
            StakeLock::decode,
            StakeLock::encode_into,
        ),
        2 => section::check_section(
            payload,
            "stake unlock",
            StakeUnlock::decode,
            StakeUnlock::encode_into,
        ),
        3 => section::check_section(
            payload,
            "proposal submission",
            ProposalSubmission::decode,
            ProposalSubmission::encode_into,
        ),
        4 => section::check_section(payload, "ballot", Ballot::decode, Ballot::encode_into),
        _ => section::check_section(
            payload,
            "council action",
            CouncilAction::decode,
            CouncilAction::encode_into,
        ),
    }
});
