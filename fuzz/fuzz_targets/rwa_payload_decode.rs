//! Fuzzes the RWA payload decoders: issuance, DvP settlement, eligibility
//! records, revenue distribution and legal attestations.
//!
//! The first byte selects the decoder; the rest is the section. Shares the
//! canonicality and round-trip property with `payload_decode`, which reaches
//! these decoders only through their one-byte tags. `rwa_record_decode` covers
//! the stored record forms in the `maya-rwa` crate.

#![no_main]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::core::rwa_payload::{
    AttestLegal, DistributeRevenue, IssueRwa, RecordEligibility, SettleDvp,
};
use libfuzzer_sys::fuzz_target;

#[path = "common/section.rs"]
mod section;

fuzz_target!(|data: &[u8]| {
    let Some((&selector, payload)) = data.split_first() else {
        return;
    };
    match selector % 5 {
        0 => section::check_section(
            payload,
            "issue rwa",
            IssueRwa::decode,
            IssueRwa::encode_into,
        ),
        1 => section::check_section(
            payload,
            "settle dvp",
            SettleDvp::decode,
            SettleDvp::encode_into,
        ),
        2 => section::check_section(
            payload,
            "record eligibility",
            RecordEligibility::decode,
            RecordEligibility::encode_into,
        ),
        3 => section::check_section(
            payload,
            "distribute revenue",
            DistributeRevenue::decode,
            DistributeRevenue::encode_into,
        ),
        _ => section::check_section(
            payload,
            "attest legal",
            AttestLegal::decode,
            AttestLegal::encode_into,
        ),
    }
});
