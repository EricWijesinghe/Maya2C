//! Plain-language descriptions of every transaction kind.
//!
//! A wallet that signs what it cannot read is blind-signing, and blind signing
//! is how large losses happen. This module is the single place that says, in a
//! sentence, what a transaction will do if signed, so a wallet, a hardware
//! device, the explorer and the CLI all show the same words.
//!
//! The `match` over [`TxKind`] has **no wildcard arm**. A new transaction kind
//! does not compile until someone has decided what it says and whether it needs
//! a careful look, which is the property that keeps this list from falling
//! behind the chain.
//!
//! The sentence says only what the transaction itself proves. Amounts are base
//! units: the display symbol and decimals belong to the wallet. Addresses and
//! identifiers appear in full in [`Description::details`], never abbreviated
//! there, because a shortened address is what an address-poisoning attack
//! counts on the reader accepting.

use maya_governance::tally::Choice;
use primitive_types::U256;

use crate::core::council_payload::CouncilKind;
use crate::core::payload::TxKind;
use crate::core::staking_payload::StakingAction;
use crate::core::transaction::Transaction;
use crate::core::transaction::TxOutput;
use crate::core::vault_payload::VaultAction;
use crate::state::fees::FEE_COLLECTOR;
use crate::state::shielded::FEE_SINK;

/// How closely a signer should look.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Review {
    /// Nothing leaves the account's control and no one gains authority.
    Routine,
    /// It moves value out of the spendable balance, locks funds, runs or
    /// publishes code, or changes who controls an account or a system. A
    /// wallet should make the signer confirm it explicitly.
    Careful,
}

/// What a transaction does, in words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Description {
    /// One sentence.
    pub summary: String,
    /// Whether the signer should look twice.
    pub review: Review,
    /// Labelled values to show in full, in order: addresses, identifiers and
    /// exact amounts. Empty when the sentence already says everything.
    pub details: Vec<(&'static str, String)>,
}

impl Description {
    fn new(summary: impl Into<String>, review: Review) -> Self {
        Self {
            summary: summary.into(),
            review,
            details: Vec::new(),
        }
    }

    fn with(mut self, label: &'static str, value: impl Into<String>) -> Self {
        self.details.push((label, value.into()));
        self
    }
}

/// Groups digits in threes: `1234567` reads `1,234,567`.
#[must_use]
pub fn group_digits(value: U256) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn amount(value: U256) -> String {
    format!("{} base units", group_digits(value))
}

fn full(bytes: &[u8]) -> String {
    hex::encode(bytes)
}

fn short(bytes: &[u8]) -> String {
    match bytes {
        [a, b, .., y, z] => format!("{a:02x}{b:02x}…{y:02x}{z:02x}"),
        other => hex::encode(other),
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

/// Describes `tx`: the native transfer's outputs, or its payload.
#[must_use]
pub fn describe(tx: &Transaction) -> Description {
    if matches!(tx.kind, TxKind::Transfer) {
        return describe_transfer(tx);
    }
    describe_kind(&tx.kind)
}

/// A fee output pays the chain's collector, or the burn sink on a chain that
/// burns fees. Any other output after the first is a payment, however small.
#[must_use]
pub fn is_fee_output(output: &TxOutput) -> bool {
    output.recipient == FEE_COLLECTOR || output.recipient == FEE_SINK
}

/// The first output is always the payment, so a transfer to the burn sink
/// still reads as a send. Later outputs are fees only when they pay a fee
/// address; a second output to anyone else is shown as a second recipient.
fn describe_transfer(tx: &Transaction) -> Description {
    let Some((first, rest)) = tx.outputs.split_first() else {
        return Description::new("Send 0 base units to 0 recipients.", Review::Careful);
    };
    let (fees, others): (Vec<&TxOutput>, Vec<&TxOutput>) =
        rest.iter().partition(|o| is_fee_output(o));
    let payments: Vec<&TxOutput> = std::iter::once(first).chain(others).collect();
    let total = sum(payments.iter().map(|o| o.amount));
    let fee = sum(fees.iter().map(|o| o.amount));
    let to = plural(payments.len(), "recipient", "recipients");
    let fee_words = if fees.is_empty() {
        String::new()
    } else {
        format!(", with a fee of {}", amount(fee))
    };
    let mut described = Description::new(
        format!("Send {} to {to}{fee_words}.", amount(total)),
        Review::Careful,
    );
    for output in payments {
        described = described.with("pay to", full(&output.recipient));
        described = described.with("amount", amount(output.amount));
    }
    for output in fees {
        described = described.with("fee to", full(&output.recipient));
        described = described.with("fee", amount(output.amount));
    }
    described
}

fn sum(values: impl Iterator<Item = U256>) -> U256 {
    values.fold(U256::zero(), U256::saturating_add)
}

/// Describes a payload kind. A `Transfer` here has no outputs to read; use
/// [`describe`] for a whole transaction.
#[must_use]
#[allow(clippy::too_many_lines)] // one arm per kind is the point of the exhaustive match
pub fn describe_kind(kind: &TxKind) -> Description {
    use Review::{Careful, Routine};
    match kind {
        TxKind::Transfer => Description::new("Send native coin.", Careful),
        TxKind::OpenChannel(c) => Description::new(
            format!(
                "Open a payment channel funding {} with a counterparty.",
                amount(c.funding)
            ),
            Careful,
        )
        .with("counterparty", full(&c.counterparty))
        .with("dispute window", format!("{} blocks", c.dispute_window)),
        TxKind::CooperativeClose(c) => Description::new(
            format!("Close a channel by agreement at update {}.", c.seq),
            Routine,
        )
        .with("channel", full(&c.channel_id)),
        TxKind::DisputeClose(c) => Description::new(
            format!("Close a channel unilaterally at update {}.", c.seq),
            Routine,
        )
        .with("channel", full(&c.channel_id)),
        TxKind::PenaltyClaim(p) => Description::new(
            format!("Claim a channel's funds by proving update {} was revoked.", p.revoked_seq),
            Routine,
        )
        .with("channel", full(&p.channel_id)),
        TxKind::SettleBatch(closures) => Description::new(
            format!(
                "Settle {} on this chain.",
                plural(closures.len(), "channel closure", "channel closures")
            ),
            Routine,
        ),
        TxKind::FinalizeDispute(id) => {
            Description::new("Finalize a channel dispute whose window has passed.", Routine)
                .with("channel", full(id))
        }
        TxKind::DeployContract(d) => Description::new(
            format!("Publish a contract of {} bytes of code.", d.code.len()),
            Careful,
        ),
        TxKind::CallContract(c) => Description::new(
            format!(
                "Call a contract with {} of input, spending up to {} gas.",
                plural(c.input.len(), "byte", "bytes"),
                c.gas_limit
            ),
            Careful,
        )
        .with("contract", full(&c.contract)),
        TxKind::Shielded(_) => Description::new(
            "Move value inside the shielded pool. Amounts and parties are hidden by design.",
            Careful,
        ),
        TxKind::RegisterAsset(a) => Description::new(
            format!(
                "Register a new asset with a supply of {} units, all credited to you.",
                group_digits(a.total_supply)
            ),
            Careful,
        )
        .with("symbol", String::from_utf8_lossy(&a.symbol).trim_end_matches('\0').to_owned()),
        TxKind::TransferAsset(t) => Description::new(
            format!("Send {} units of an asset to {}.", group_digits(t.amount), short(&t.recipient)),
            Careful,
        )
        .with("asset", full(&t.asset))
        .with("pay to", full(&t.recipient)),
        TxKind::CreatePool(p) => Description::new(
            format!(
                "Create a trading pool and deposit {} and {} units, with a {} basis point fee.",
                group_digits(p.amount_a),
                group_digits(p.amount_b),
                p.lp_fee_bps
            ),
            Careful,
        )
        .with("asset a", full(&p.asset_a))
        .with("asset b", full(&p.asset_b)),
        TxKind::AddLiquidity(d) => Description::new(
            format!(
                "Deposit up to {} and {} units into a trading pool.",
                group_digits(d.base_desired),
                group_digits(d.quote_desired)
            ),
            Careful,
        )
        .with("pool", full(&d.pair)),
        TxKind::RemoveLiquidity(w) => Description::new(
            format!("Withdraw {} pool shares.", group_digits(w.shares)),
            Routine,
        )
        .with("pool", full(&w.pair)),
        TxKind::Swap(s) => Description::new(
            format!(
                "Swap {} units, accepting no less than {} back.",
                group_digits(s.amount_in),
                group_digits(s.min_out)
            ),
            Careful,
        )
        .with("pool", full(&s.pair))
        .with("deadline", format!("block {}", s.deadline)),
        TxKind::SwapRoute(r) => Description::new(
            format!(
                "Swap {} units through {}, accepting no less than {} back.",
                group_digits(r.amount_in),
                plural(r.legs.len(), "pool", "pools"),
                group_digits(r.min_out)
            ),
            Careful,
        )
        .with("deadline", format!("block {}", r.deadline)),
        TxKind::PlaceOrder(o) => Description::new(
            format!(
                "Place a limit order for {} units at price {}.",
                group_digits(o.amount),
                group_digits(o.price)
            ),
            Careful,
        )
        .with("pool", full(&o.pair))
        .with("expires", if o.expiry == 0 { "never".to_owned() } else { format!("block {}", o.expiry) }),
        TxKind::CancelOrder(id) => {
            Description::new("Cancel a resting order and take back its escrow.", Routine)
                .with("order", full(id))
        }
        TxKind::CreateFeed(f) => Description::new(
            format!(
                "Create a price feed named {}.",
                String::from_utf8_lossy(&f.name).trim_end_matches('\0')
            ),
            Routine,
        ),
        TxKind::SubmitFeed(s) => Description::new(
            format!(
                "Submit {} for round {} of a price feed.",
                plural(s.observations.len(), "observation", "observations"),
                s.round
            ),
            Routine,
        )
        .with("feed", full(&s.feed_id)),
        TxKind::RotateAuthorities(r) => Description::new(
            format!(
                "Replace the oracle authority set with {} from epoch {}.",
                plural(r.authorities.len(), "authority", "authorities"),
                r.epoch
            ),
            Careful,
        ),
        TxKind::SubmitBeacon(b) => {
            Description::new(format!("Submit a randomness beacon for block {}.", b.height), Routine)
        }
        TxKind::ClaimWork(w) => Description::new(
            "Credit this block's proof of work as voting weight to an address.",
            Routine,
        )
        .with("credited to", full(&w.beneficiary)),
        TxKind::LockStake(l) => Description::new(
            format!(
                "Lock {} for governance until block {}. You cannot spend it until then.",
                amount(l.amount),
                l.unlock_height
            ),
            Careful,
        ),
        TxKind::UnlockStake(u) => Description::new(
            format!("Release {} of locked governance stake.", amount(u.amount)),
            Routine,
        ),
        TxKind::Propose(p) => Description::new(
            format!(
                "Propose changing {} on-chain, with {} blocks of voting and a {}-block delay.",
                plural(p.changes.len(), "parameter", "parameters"),
                p.voting_blocks,
                p.timelock_blocks
            ),
            Careful,
        ),
        TxKind::CastVote(b) => Description::new(
            // The governance crate's own tags, never numbers written here: a
            // wallet that told a signer "for" when the wire said "against"
            // would be worse than one that showed nothing.
            match Choice::from_tag(b.choice) {
                Some(Choice::For) => "Vote for a governance proposal.",
                Some(Choice::Against) => "Vote against a governance proposal.",
                Some(Choice::Abstain) => "Abstain on a governance proposal.",
                None => "Vote on a governance proposal with a choice this build does not know.",
            },
            Routine,
        )
        .with("proposal", full(&b.proposal)),
        TxKind::CancelProposal(id) => {
            Description::new("Withdraw your governance proposal.", Routine).with("proposal", full(id))
        }
        TxKind::Seal(s) => Description::new(
            format!(
                "Submit a sealed transaction, readable only from block {}.",
                s.reveal_height
            ),
            Careful,
        ),
        TxKind::RevealShare(r) => Description::new(
            format!("Reveal decryption share {} for a sealed transaction.", r.member),
            Routine,
        )
        .with("envelope", full(&r.envelope)),
        TxKind::RegisterDid(d) => Description::new(
            format!(
                "Register a decentralised identity with {}.",
                plural(d.endpoints.len(), "service endpoint", "service endpoints")
            ),
            Routine,
        ),
        TxKind::RotateDidKey(r) => Description::new(
            format!("Replace your identity's key (rotation {}).", r.epoch),
            Careful,
        ),
        TxKind::RevokeDid(r) => Description::new(
            format!("Permanently revoke your identity (reason code {}). This cannot be undone.", r.reason),
            Careful,
        ),
        TxKind::AnchorAttestation(a) => Description::new(
            format!("Anchor an attestation under schema \"{}\".", a.schema),
            Routine,
        )
        .with("root", full(&a.root)),
        TxKind::SetRevocationBit(s) => Description::new(
            format!("Revoke credential number {} that you issued.", s.index),
            Careful,
        ),
        TxKind::IssueRwa(i) => Description::new(
            format!("Issue a real-world asset \"{}\" of {} units.", i.label, i.total_units),
            Careful,
        ),
        TxKind::SettleDvp(s) => Description::new(
            format!(
                "Buy {} units of an asset for {} in one atomic settlement.",
                s.units,
                amount(s.price)
            ),
            Careful,
        )
        .with("seller", full(&s.seller))
        .with("asset", full(&s.asset)),
        TxKind::RecordEligibility(r) => {
            Description::new("Record an eligibility check for an asset.", Routine)
                .with("asset", full(&r.asset))
        }
        TxKind::DistributeRevenue(d) => Description::new(
            format!(
                "Distribute {} to an asset's holders in round {}.",
                amount(d.total),
                d.round
            ),
            Careful,
        )
        .with("asset", full(&d.asset)),
        TxKind::AttestLegal(a) => Description::new(
            format!("Attest a legal document for an asset (reference \"{}\").", a.reference),
            Routine,
        )
        .with("asset", full(&a.asset)),
        TxKind::HtlcLock(l) => Description::new(
            format!(
                "Lock {} for a recipient, claimable with a secret until block {}, then refundable to you.",
                amount(l.amount),
                l.expiry_height
            ),
            Careful,
        )
        .with("recipient", full(&l.recipient)),
        TxKind::HtlcClaim(c) => {
            Description::new("Claim a locked amount by revealing its secret.", Routine)
                .with("lock", full(&c.lock_id))
        }
        TxKind::HtlcRefund(r) => {
            Description::new("Take back an expired locked amount.", Routine)
                .with("lock", full(&r.lock_id))
        }
        TxKind::AttestAttack(_) => Description::new("Submit a signed attack attestation.", Routine),
        TxKind::EnrollDevice(_) => Description::new("Enroll a device.", Routine),
        TxKind::SubmitTelemetry(_) => Description::new("Submit a batch of device telemetry.", Routine),
        TxKind::ReportTamper(_) => Description::new("Report a device tamper event.", Routine),
        TxKind::ProveEquivocation(_) => {
            Description::new("Prove a device signed two conflicting records.", Routine)
        }
        TxKind::RevokeDevice(id) => Description::new("Revoke a device.", Careful).with("device", full(id.as_ref())),
        TxKind::Staking(action) => describe_staking(action),
        TxKind::Council(action) => match &action.kind {
            CouncilKind::Pause { module, blocks } => Description::new(
                format!("Council: pause module {module} for {blocks} blocks."),
                Careful,
            ),
            CouncilKind::Resume { module } => {
                Description::new(format!("Council: resume module {module}."), Careful)
            }
        },
        TxKind::Vault(action) => describe_vault(action),
        TxKind::MiningShare(m) => Description::new(
            format!("Submit a mining share for epoch {}.", m.epoch),
            Routine,
        ),
    }
}

fn describe_staking(action: &StakingAction) -> Description {
    use Review::{Careful, Routine};
    match action {
        StakingAction::Register {
            bond,
            commission_bps,
            ..
        } => Description::new(
            format!(
                "Register as a validator, bonding {} with a {} basis point commission. The bond can be cut for misbehaviour.",
                amount(*bond),
                commission_bps
            ),
            Careful,
        ),
        StakingAction::BondMore {
            validator,
            amount: a,
        } => Description::new(
            format!("Add {} to your validator bond.", amount(*a)),
            Careful,
        )
        .with("validator", full(validator)),
        StakingAction::Unbond {
            validator,
            amount: a,
        } => Description::new(
            format!("Start unbonding {} from your validator bond.", amount(*a)),
            Routine,
        )
        .with("validator", full(validator)),
        StakingAction::Delegate {
            validator,
            amount: a,
        } => Description::new(
            format!(
                "Delegate {} to validator {}. It can be cut if that validator misbehaves.",
                amount(*a),
                short(validator)
            ),
            Careful,
        )
        .with("validator", full(validator)),
        StakingAction::Undelegate {
            validator,
            amount: a,
        } => Description::new(
            format!("Start withdrawing {} of your delegation.", amount(*a)),
            Routine,
        )
        .with("validator", full(validator)),
        StakingAction::ReportEquivocation { .. } => Description::new(
            "Report a validator that signed two conflicting messages.",
            Routine,
        ),
    }
}

fn describe_vault(action: &VaultAction) -> Description {
    use Review::Careful;
    match action {
        VaultAction::Configure(c) => Description::new(
            format!(
                "Set this account's vault rules: at most {} per {} blocks, with {}.",
                amount(c.limit),
                c.delay_blocks,
                plural(
                    c.guardians.len(),
                    "guardian who can cancel withdrawals",
                    "guardians who can cancel withdrawals"
                )
            ),
            Careful,
        ),
        VaultAction::Request { to, amount: a } => Description::new(
            format!(
                "Request a delayed withdrawal of {} to {}.",
                amount(*a),
                short(to)
            ),
            Careful,
        )
        .with("pay to", full(to)),
        VaultAction::Execute { owner, id } => {
            Description::new(format!("Execute delayed withdrawal number {id}."), Careful)
                .with("vault owner", full(owner))
        }
        VaultAction::Cancel { owner, id } => Description::new(
            format!("Cancel delayed withdrawal number {id}."),
            Review::Routine,
        )
        .with("vault owner", full(owner)),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::core::dex_payload::{AssetTransfer, SwapRequest};
    use crate::core::governance_payload::{Ballot, StakeLock};
    use crate::core::transaction::TxOutput;
    use crate::core::vault_payload::VaultConfig;

    #[test]
    fn digits_are_grouped_in_threes() {
        assert_eq!(group_digits(U256::zero()), "0");
        assert_eq!(group_digits(U256::from(999)), "999");
        assert_eq!(group_digits(U256::from(1_000)), "1,000");
        assert_eq!(group_digits(U256::from(1_234_567)), "1,234,567");
        assert_eq!(
            group_digits(U256::MAX),
            "115,792,089,237,316,195,423,570,985,008,687,907,853,269,984,665,640,564,039,457,584,007,913,129,639,935"
        );
    }

    #[test]
    fn a_transfer_says_how_much_to_whom_in_full() {
        let tx = Transaction::new(
            vec![],
            vec![
                TxOutput {
                    amount: U256::from(1_500),
                    recipient: [0xAB; 32],
                },
                TxOutput {
                    amount: U256::from(500),
                    recipient: [0xCD; 32],
                },
            ],
            0,
        );
        let d = describe(&tx);
        assert_eq!(d.summary, "Send 2,000 base units to 2 recipients.");
        assert_eq!(d.review, Review::Careful);
        assert!(d.details.contains(&("pay to", "ab".repeat(32))));
        assert!(d.details.contains(&("pay to", "cd".repeat(32))));
    }

    fn transfer_with(outputs: Vec<([u8; 32], u64)>) -> Transaction {
        let outputs = outputs
            .into_iter()
            .map(|(recipient, amount)| TxOutput {
                amount: U256::from(amount),
                recipient,
            })
            .collect();
        Transaction::new(vec![], outputs, 0)
    }

    #[test]
    fn a_fee_to_the_collector_is_named_as_the_fee_not_a_recipient() {
        let tx = transfer_with(vec![([0xAB; 32], 1_000), (FEE_COLLECTOR, 7)]);
        let d = describe(&tx);
        assert_eq!(
            d.summary,
            "Send 1,000 base units to 1 recipient, with a fee of 7 base units."
        );
        assert!(d.details.contains(&("fee to", full(&FEE_COLLECTOR))));
        assert!(d.details.contains(&("fee", "7 base units".to_string())));
        assert!(!d.details.iter().any(|(l, v)| *l == "pay to" && *v == full(&FEE_COLLECTOR)));
    }

    #[test]
    fn a_fee_burned_to_the_sink_is_named_as_the_fee() {
        let d = describe(&transfer_with(vec![([0xAB; 32], 50), (FEE_SINK, 3)]));
        assert_eq!(
            d.summary,
            "Send 50 base units to 1 recipient, with a fee of 3 base units."
        );
    }

    #[test]
    fn a_second_output_to_anyone_else_stays_a_recipient() {
        let d = describe(&transfer_with(vec![([0xAB; 32], 10), ([0xCD; 32], 90)]));
        assert_eq!(d.summary, "Send 100 base units to 2 recipients.");
        assert!(!d.summary.contains("fee"));
    }

    #[test]
    fn a_transfer_to_the_burn_sink_is_a_send_not_a_fee() {
        let d = describe(&transfer_with(vec![(FEE_SINK, 9)]));
        assert_eq!(d.summary, "Send 9 base units to 1 recipient.");
        assert!(d.details.contains(&("pay to", full(&FEE_SINK))));
    }

    #[test]
    fn describing_a_transfer_twice_gives_the_same_words() {
        let tx = transfer_with(vec![([0xAB; 32], 1), (FEE_COLLECTOR, 2)]);
        assert_eq!(describe(&tx), describe(&tx));
    }

    #[test]
    fn the_full_address_is_never_abbreviated_in_details() {
        let d = describe_kind(&TxKind::TransferAsset(AssetTransfer {
            asset: [1; 32],
            recipient: [0xEE; 32],
            amount: U256::from(7),
        }));
        let shown = d.details.iter().find(|(l, _)| *l == "pay to").unwrap();
        assert_eq!(shown.1.len(), 64, "a shortened address invites poisoning");
        assert!(
            d.summary.contains("eeee…eeee"),
            "the sentence may abbreviate"
        );
    }

    #[test]
    fn spending_and_control_changes_need_a_careful_look_and_reads_do_not() {
        let swap = TxKind::Swap(SwapRequest {
            pair: [2; 32],
            direction: 0,
            amount_in: U256::from(10),
            min_out: U256::from(9),
            deadline: 50,
        });
        assert_eq!(describe_kind(&swap).review, Review::Careful);

        let lock = TxKind::LockStake(StakeLock {
            amount: U256::from(5),
            unlock_height: 9,
        });
        assert_eq!(describe_kind(&lock).review, Review::Careful);

        for (choice, words) in [
            (Choice::For, "Vote for a governance proposal."),
            (Choice::Against, "Vote against a governance proposal."),
            (Choice::Abstain, "Abstain on a governance proposal."),
        ] {
            let vote = TxKind::CastVote(Ballot {
                proposal: [3; 32],
                choice: choice.tag(),
            });
            let d = describe_kind(&vote);
            assert_eq!(d.review, Review::Routine);
            assert_eq!(d.summary, words);
        }
        let unknown = TxKind::CastVote(Ballot {
            proposal: [3; 32],
            choice: 0,
        });
        assert!(describe_kind(&unknown).summary.contains("does not know"));
    }

    #[test]
    fn a_vault_reconfiguration_states_the_limit_and_the_delay() {
        let d = describe_kind(&TxKind::Vault(Box::new(VaultAction::Configure(
            VaultConfig {
                delay_blocks: 100,
                limit: U256::from(2_500),
                guardians: vec![[1; 32], [2; 32]],
            },
        ))));
        assert_eq!(d.review, Review::Careful);
        assert!(
            d.summary.contains("2,500 base units per 100 blocks"),
            "{}",
            d.summary
        );
        assert!(d.summary.contains("2 guardians"), "{}", d.summary);
    }

    #[test]
    fn every_description_is_a_sentence() {
        let samples = vec![
            TxKind::Transfer,
            TxKind::CancelOrder([4; 32]),
            TxKind::FinalizeDispute([5; 32]),
            TxKind::CancelProposal([6; 32]),
            TxKind::LockStake(StakeLock {
                amount: U256::from(1),
                unlock_height: 2,
            }),
            TxKind::Vault(Box::new(VaultAction::Cancel {
                owner: [7; 32],
                id: 3,
            })),
        ];
        for kind in samples {
            let d = describe_kind(&kind);
            assert!(d.summary.ends_with('.'), "{}: {}", kind.label(), d.summary);
            assert!(d.summary.len() > 10, "{}", kind.label());
        }
    }
}
