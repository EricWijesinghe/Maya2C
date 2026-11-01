//! Fuzzes the DEX payload decoders in `core::dex_payload`: asset registration
//! and transfer, pool creation, liquidity deposit and withdrawal, swaps, swap
//! routes and order placement.
//!
//! Each is reachable from a transaction, so any sender may submit its bytes.
//! The first byte selects the decoder; the rest is the section. Shares the
//! canonicality and round-trip property with `payload_decode`, which reaches
//! these same decoders only through their one-byte tags.

#![no_main]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::core::dex_payload::{
    AssetRegistration, AssetTransfer, LiquidityDeposit, LiquidityWithdrawal, OrderPlacement,
    PoolCreation, SwapRequest, SwapRoute,
};
use libfuzzer_sys::fuzz_target;

#[path = "common/section.rs"]
mod section;

fuzz_target!(|data: &[u8]| {
    let Some((&selector, payload)) = data.split_first() else {
        return;
    };
    match selector % 8 {
        0 => section::check_section(
            payload,
            "asset registration",
            AssetRegistration::decode,
            AssetRegistration::encode_into,
        ),
        1 => section::check_section(
            payload,
            "asset transfer",
            AssetTransfer::decode,
            AssetTransfer::encode_into,
        ),
        2 => section::check_section(
            payload,
            "pool creation",
            PoolCreation::decode,
            PoolCreation::encode_into,
        ),
        3 => section::check_section(
            payload,
            "liquidity deposit",
            LiquidityDeposit::decode,
            LiquidityDeposit::encode_into,
        ),
        4 => section::check_section(
            payload,
            "liquidity withdrawal",
            LiquidityWithdrawal::decode,
            LiquidityWithdrawal::encode_into,
        ),
        5 => section::check_section(
            payload,
            "swap request",
            SwapRequest::decode,
            SwapRequest::encode_into,
        ),
        6 => section::check_section(
            payload,
            "swap route",
            SwapRoute::decode,
            SwapRoute::encode_into,
        ),
        _ => section::check_section(
            payload,
            "order placement",
            OrderPlacement::decode,
            OrderPlacement::encode_into,
        ),
    }
});
