//! Fuzzes the `maya-rwa` record decoders: tokens, legal attestations, revenue
//! distributions, eligibility records and cap-table pages.
//!
//! These records are the values the chain stores and reads back, so a decoder
//! that accepted a second encoding of one record would let two stored forms
//! hash differently. Each decoder is total over arbitrary bytes: a refusal, or
//! an accepted value that re-encodes to the same bytes.

#![no_main]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use libfuzzer_sys::fuzz_target;
use maya_rwa::cap_table::CapTablePage;
use maya_rwa::token::{Eligibility, LegalAttestation, RevenueDistribution, RwaToken};

#[path = "common/section.rs"]
mod section;

fuzz_target!(|data: &[u8]| {
    let Some((&selector, record)) = data.split_first() else {
        return;
    };
    match selector % 5 {
        0 => section::check_record(record, "rwa token", RwaToken::decode, RwaToken::encode),
        1 => section::check_record(
            record,
            "legal attestation",
            LegalAttestation::decode,
            LegalAttestation::encode,
        ),
        2 => section::check_record(
            record,
            "revenue distribution",
            RevenueDistribution::decode,
            RevenueDistribution::encode,
        ),
        3 => section::check_record(
            record,
            "eligibility",
            Eligibility::decode,
            Eligibility::encode,
        ),
        _ => section::check_record(
            record,
            "cap table page",
            CapTablePage::decode,
            CapTablePage::encode,
        ),
    }
});
