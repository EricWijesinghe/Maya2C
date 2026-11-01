//! Checks the wallet makes before a key signs: which chain it signs for, which
//! collector and fee it pays, and a person's yes.
//!
//! Every rule is a pure function so each refusal has a test. `main.rs` calls
//! them in `submit`, in this order: genesis, collector, fee, confirmation.
//! The threat model's P4 and P3 (`docs/security/THREAT_MODEL_SIGNER_CHAIN.md`)
//! are the reason: a node or RPC names the genesis, the collector and the fee,
//! and the screen must show what the key is about to sign.

use std::io::{BufRead, Write};

use anyhow::{Result, anyhow, bail};
use custom_l1_node::rpc::FeeInfo;
use custom_l1_node::state::fees::FEE_COLLECTOR;
use primitive_types::U256;

/// Genesis ids (lower-case hex) of the mainnet. Empty until the genesis
/// ceremony adds the id. Until then a node that claims a mainnet name is refused.
pub const MAINNET_GENESIS: &[&str] = &[];

/// Genesis ids of public testnets. Empty: the repo records no genesis id for
/// the current testnet (the last one on record predates the 2026-10-17
/// re-genesis), so a testnet is signed for only with `--genesis <id>`.
pub const TESTNET_GENESIS: &[&str] = &[];

/// Names a node may use only with the pinned mainnet genesis.
const MAINNET_NAMES: &[&str] = &["maya-mainnet", "mainnet"];

/// Default fee cap, in base units, for every transaction kind. One whole MAYA
/// (ADR-068: 24 decimals, `maya_amount::ONE_COIN`). A fixed figure, not a quote multiple, so a hostile
/// node cannot raise it together with the fee.
/// shortcut: 1 MAYA is a judgment, not a measured fee; it bounds a single
/// transaction's fee, not a fee market. Upgrade from testnet fee data, or
/// override per call with `--max-fee`.
pub const DEFAULT_MAX_FEE: U256 = U256([0x1bce_cced_a100_0000, 0xd3c2, 0, 0]); // 10^24

/// Transfers whose fee exceeds this share of the amount get a warning, not a refusal.
const FEE_WARN_DIVISOR: u64 = 10;

/// A warning for a transfer whose fee is more than a tenth of the amount sent.
/// Dust transfers are allowed to pay that; the warning makes the cost visible.
#[must_use]
pub fn fee_warning(fee: U256, amount: U256) -> Option<String> {
    (fee > amount / U256::from(FEE_WARN_DIVISOR)).then(|| {
        format!("warning: the fee {fee} base units is more than a tenth of the amount {amount}")
    })
}

/// How the wallet came to trust the chain it is about to sign for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GenesisTrust {
    /// The genesis id is in the mainnet pin table.
    Mainnet,
    /// The genesis id is in the testnet pin table.
    Testnet,
    /// Not pinned, but the person passed `--genesis` with this exact id.
    Expected,
}

impl GenesisTrust {
    /// The label a person reads on the confirmation screen.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Mainnet => "pinned mainnet",
            Self::Testnet => "pinned testnet",
            Self::Expected => "matches --genesis (not pinned in this build)",
        }
    }
}

/// Genesis ids are compared trimmed and lower-case.
fn normal(id: &str) -> String {
    id.trim().to_ascii_lowercase()
}

fn is_mainnet_name(name: Option<&str>) -> bool {
    name.map(normal)
        .is_some_and(|n| MAINNET_NAMES.contains(&n.as_str()))
}

/// Decides whether the node's genesis may be signed for.
///
/// An explicit `expected` must equal the node's genesis, pinned or not. A
/// pinned id is trusted. A node that claims a mainnet name is refused unless
/// its genesis is the pinned mainnet one, because the name is only a claim.
/// Anything else needs `expected`, so an unknown chain is never signed for by accident.
///
/// # Errors
///
/// Returns an error when the genesis does not satisfy any rule above.
pub fn check_genesis(
    genesis: &str,
    claimed_name: Option<&str>,
    expected: Option<&str>,
    mainnet: &[&str],
    testnet: &[&str],
) -> Result<GenesisTrust> {
    let node = normal(genesis);
    if let Some(expected) = expected
        && normal(expected) != node
    {
        bail!(
            "the node reports genesis {node} but --genesis expects {}; refusing to sign",
            normal(expected)
        );
    }
    if mainnet.iter().any(|p| normal(p) == node) {
        return Ok(GenesisTrust::Mainnet);
    }
    if testnet.iter().any(|p| normal(p) == node) {
        return Ok(GenesisTrust::Testnet);
    }
    if is_mainnet_name(claimed_name) {
        bail!(
            "the node claims a mainnet name but its genesis {node} is not the pinned mainnet genesis; refusing to sign"
        );
    }
    if expected.is_some() {
        return Ok(GenesisTrust::Expected);
    }
    bail!(
        "the node's genesis {node} is not pinned in this wallet build; check it against the \
         network's published genesis id, then re-run with --genesis {node}"
    )
}

/// Refuses a fee collector that is not the protocol's (`state::fees`).
///
/// # Errors
///
/// Returns an error when the collector is not hex, or not [`FEE_COLLECTOR`].
pub fn check_collector(collector: &str) -> Result<()> {
    let bytes = hex::decode(collector.trim())
        .map_err(|e| anyhow!("the node's fee collector is not hex: {e}"))?;
    if bytes.as_slice() != FEE_COLLECTOR.as_slice() {
        bail!(
            "the node names fee collector {}, not the protocol collector {}; refusing to pay it",
            collector.trim(),
            hex::encode(FEE_COLLECTOR)
        );
    }
    Ok(())
}

/// Chooses the fee to pay and refuses one above the cap.
///
/// `requested` is the `--fee` value; `None` quotes `2 x base_fee x size` from
/// the node. The cap is `max_fee` when given, else [`DEFAULT_MAX_FEE`]. It does
/// not depend on the amount, so zero-amount kinds and dust transfers both get a cap.
///
/// # Errors
///
/// Returns an error for a bad or overflowing quote, or a fee above the cap.
pub fn quote_fee(
    info: &FeeInfo,
    size: u64,
    requested: Option<U256>,
    max_fee: Option<U256>,
) -> Result<U256> {
    let fee = match requested {
        Some(fee) => fee,
        None => {
            // Checked: a base fee near 2^256 must not wrap into a small, signable fee.
            let base_fee = U256::from_dec_str(&info.base_fee)
                .map_err(|_| anyhow!("the node sent a bad base_fee {:?}", info.base_fee))?;
            base_fee
                .checked_mul(U256::from(size))
                .and_then(|f| f.checked_mul(U256::from(2u8)))
                .ok_or_else(|| anyhow!("the node quoted a fee past 2^256 - 1"))?
        }
    };
    let cap = max_fee.unwrap_or(DEFAULT_MAX_FEE);
    if fee > cap {
        bail!(
            "the fee of {fee} base units is above the cap of {cap}; \
             raise it with --max-fee if you mean to pay it"
        );
    }
    Ok(fee)
}

/// Asks for a typed `yes` on a terminal. `--yes` skips the question for scripts.
/// Without a terminal and without `--yes` nothing is signed, and nothing is read.
///
/// # Errors
///
/// Returns an error when there is no terminal and no `--yes`, when the answer is
/// not `yes`, or when the prompt cannot be read or written.
pub fn require_confirmation(
    yes: bool,
    is_terminal: bool,
    input: &mut impl BufRead,
    prompt: &mut impl Write,
) -> Result<()> {
    if yes {
        return Ok(());
    }
    if !is_terminal {
        bail!("not signing: there is no terminal to confirm on; pass --yes to sign unattended");
    }
    write!(prompt, "Type `yes` to sign this transaction: ")?;
    prompt.flush()?;
    let mut answer = String::new();
    input.read_line(&mut answer)?;
    if answer.trim() == "yes" {
        Ok(())
    } else {
        bail!("not confirmed: nothing was signed")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    const PINNED: &str = "aa11bb22cc33dd44ee55ff6600778899aa11bb22cc33dd44ee55ff6600778899";
    const OTHER: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn fee_info(base_fee: &str, collector: &str) -> FeeInfo {
        FeeInfo {
            active: true,
            base_fee: base_fee.to_string(),
            collector: collector.to_string(),
        }
    }

    fn protocol_info(base_fee: &str) -> FeeInfo {
        fee_info(base_fee, &hex::encode(FEE_COLLECTOR))
    }

    // --- fee ---------------------------------------------------------------

    #[test]
    fn the_default_cap_is_one_maya_in_base_units() {
        // ADR-068: 24 decimals, so one MAYA is 10^24 base units.
        assert_eq!(DEFAULT_MAX_FEE, U256::from(10u64).pow(U256::from(24u8)));
    }

    #[test]
    fn a_hostile_quote_above_the_default_cap_is_refused() {
        // Quote 2 * 10^18 * 10^6 = 2 * 10^24, above the 10^24 default cap.
        let info = protocol_info("1000000000000000000");
        let err = quote_fee(&info, 1_000_000, None, None).unwrap_err();
        assert!(err.to_string().contains("above the cap"), "{err}");
    }

    #[test]
    fn a_quote_within_the_cap_is_paid() {
        let info = protocol_info("1");
        let fee = quote_fee(&info, 13_000, None, None).unwrap();
        assert_eq!(fee, U256::from(26_000u32));
    }

    #[test]
    fn a_dust_transfer_pays_a_quote_within_the_absolute_cap() {
        // Amount 1 base unit: a relative cap would be 0 and refuse every fee.
        let info = protocol_info("1");
        let fee = quote_fee(&info, 13_000, None, None).unwrap();
        assert_eq!(fee, U256::from(26_000u32));
        let warning = fee_warning(fee, U256::from(1u32)).expect("a dust fee is warned about");
        assert!(warning.contains("more than a tenth"), "{warning}");
    }

    #[test]
    fn a_fee_within_a_tenth_of_the_amount_is_not_warned() {
        assert!(fee_warning(U256::from(100u32), U256::from(1000u32)).is_none());
        assert!(fee_warning(U256::from(101u32), U256::from(1000u32)).is_some());
    }

    #[test]
    fn a_zero_amount_kind_gets_the_default_cap() {
        // Stake, governance and vault ops move no output amount; they must not be capped at zero.
        let info = protocol_info("1");
        let fee = quote_fee(&info, 13_000, None, None).unwrap();
        assert_eq!(fee, U256::from(26_000u32));
    }

    #[test]
    fn a_base_fee_near_two_to_the_256_is_refused_not_wrapped() {
        let max = U256::MAX.to_string();
        let info = protocol_info(&max);
        let err = quote_fee(&info, 1000, None, None).unwrap_err();
        assert!(err.to_string().contains("past 2^256"), "{err}");
    }

    #[test]
    fn max_fee_raises_the_cap_only_when_given() {
        let info = protocol_info("1000000000000");
        let fee = quote_fee(&info, 1_000_000, None, Some(U256::MAX)).unwrap();
        assert_eq!(
            fee,
            U256::from(2u8) * U256::from(1_000_000_000_000u64) * U256::from(1_000_000u64)
        );
    }

    #[test]
    fn an_explicit_fee_above_the_default_cap_is_refused() {
        let info = protocol_info("1");
        let over = DEFAULT_MAX_FEE + U256::one();
        let err = quote_fee(&info, 10, Some(over), None).unwrap_err();
        assert!(err.to_string().contains("above the cap"), "{err}");
        assert!(quote_fee(&info, 10, Some(DEFAULT_MAX_FEE), None).is_ok());
    }

    // --- collector ---------------------------------------------------------

    #[test]
    fn the_protocol_collector_is_accepted() {
        assert!(check_collector(&hex::encode(FEE_COLLECTOR)).is_ok());
        assert!(check_collector(&format!(" {} ", hex::encode(FEE_COLLECTOR))).is_ok());
    }

    #[test]
    fn a_wrong_collector_is_refused() {
        let err = check_collector(OTHER).unwrap_err();
        assert!(
            err.to_string().contains("not the protocol collector"),
            "{err}"
        );
    }

    #[test]
    fn a_collector_that_is_not_hex_is_refused() {
        assert!(check_collector("not-hex").is_err());
    }

    // --- genesis -----------------------------------------------------------

    #[test]
    fn an_unpinned_genesis_without_the_flag_is_refused() {
        let err = check_genesis(OTHER, Some("maya-testnet-1"), None, &[PINNED], &[]).unwrap_err();
        assert!(err.to_string().contains("not pinned"), "{err}");
    }

    #[test]
    fn an_unpinned_genesis_with_a_matching_flag_is_signed_for() {
        let trust = check_genesis(OTHER, None, Some(OTHER), &[PINNED], &[]).unwrap();
        assert_eq!(trust, GenesisTrust::Expected);
    }

    #[test]
    fn a_flag_that_does_not_match_the_node_is_refused_even_for_a_pinned_genesis() {
        let err = check_genesis(PINNED, None, Some(OTHER), &[PINNED], &[]).unwrap_err();
        assert!(err.to_string().contains("refusing to sign"), "{err}");
    }

    #[test]
    fn a_mainnet_name_with_an_unpinned_genesis_is_refused_even_with_the_flag() {
        let err =
            check_genesis(OTHER, Some("maya-mainnet"), Some(OTHER), &[PINNED], &[]).unwrap_err();
        assert!(err.to_string().contains("claims a mainnet name"), "{err}");
    }

    #[test]
    fn a_pinned_testnet_genesis_needs_no_flag() {
        let trust = check_genesis(PINNED, Some("anything"), None, &[], &[PINNED]).unwrap();
        assert_eq!(trust, GenesisTrust::Testnet);
    }

    #[test]
    fn a_pinned_mainnet_genesis_is_mainnet_whatever_the_node_calls_it() {
        let trust = check_genesis(&PINNED.to_uppercase(), None, None, &[PINNED], &[]).unwrap();
        assert_eq!(trust, GenesisTrust::Mainnet);
    }

    #[test]
    fn the_empty_build_pins_refuse_every_mainnet_name() {
        // Before the ceremony MAINNET_GENESIS is empty; signing must refuse, not trust.
        let err = check_genesis(
            OTHER,
            Some("maya-mainnet"),
            None,
            MAINNET_GENESIS,
            TESTNET_GENESIS,
        )
        .unwrap_err();
        assert!(err.to_string().contains("claims a mainnet name"), "{err}");
    }

    // --- confirmation ------------------------------------------------------

    #[test]
    fn without_a_terminal_or_yes_nothing_is_read_and_nothing_is_signed() {
        let mut input = Cursor::new(b"yes\n".to_vec());
        let mut prompt = Vec::new();
        let err = require_confirmation(false, false, &mut input, &mut prompt).unwrap_err();
        assert!(err.to_string().contains("--yes"), "{err}");
        assert_eq!(input.position(), 0, "a piped answer must not confirm");
        assert!(prompt.is_empty());
    }

    #[test]
    fn yes_skips_the_question_for_scripts() {
        let mut input = Cursor::new(Vec::new());
        let mut prompt = Vec::new();
        assert!(require_confirmation(true, false, &mut input, &mut prompt).is_ok());
        assert!(prompt.is_empty());
    }

    #[test]
    fn a_typed_yes_on_a_terminal_confirms() {
        let mut input = Cursor::new(b"yes\n".to_vec());
        let mut prompt = Vec::new();
        assert!(require_confirmation(false, true, &mut input, &mut prompt).is_ok());
    }

    #[test]
    fn anything_but_yes_on_a_terminal_is_refused() {
        for answer in [&b"y\n"[..], b"\n", b"YES\n", b"no\n"] {
            let mut input = Cursor::new(answer.to_vec());
            let mut prompt = Vec::new();
            assert!(
                require_confirmation(false, true, &mut input, &mut prompt).is_err(),
                "{answer:?} must not confirm"
            );
        }
    }
}
