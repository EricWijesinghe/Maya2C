//! Typed transaction payloads.
//!
//! The base chain has no scripting, so state-channel operations reach the state
//! transition as typed payloads carried by an ordinary [`Transaction`].
//!
//! ## Wire compatibility
//!
//! A [`TxKind::Transfer`] carries **no payload section at all** — not an empty
//! one. That keeps a plain transfer's signing bytes byte-identical to the
//! pre-payload format, so existing signatures, transaction ids, and stored
//! blocks all remain valid. Adding even a zero-length marker would have
//! invalidated every signature ever produced.
//!
//! Non-transfer kinds append `tag || fields` after the nonce. Because the
//! preceding fields are fixed-width and a payload'd transaction is strictly
//! longer, no payload encoding can collide with a transfer's signing bytes.
//!
//! ## Batch authorization
//!
//! A [`ChannelClosure`] carries **both** parties' signatures over the channel
//! state. The enclosing transaction's own signature pays for inclusion and
//! nothing more: whoever submits a batch cannot move a single unit of value
//! that both channel participants did not already authorize off-chain. That
//! separation is what makes cross-channel batching safe — a batch settling a
//! hundred channels has two hundred authorizers, and one outer signature could
//! never speak for them.
//!
//! [`Transaction`]: crate::core::Transaction

use crate::core::codec::ByteReader;
use crate::core::dex_payload::{
    AssetRegistration, AssetTransfer, LiquidityDeposit, LiquidityWithdrawal, OrderPlacement,
    PoolCreation, SwapRequest, SwapRoute,
};
use crate::core::governance_payload::{
    Ballot, ProposalSubmission, StakeLock, StakeUnlock, WorkClaim,
};
use crate::core::htlc_payload::{HtlcClaim, HtlcLock, HtlcRefund};
use crate::core::identity_payload::{
    AnchorAttestation, RegisterDid, RevokeDid, RotateDidKey, SetRevocationBit,
};
use crate::core::oracle_payload::{
    BeaconSubmission, FeedCreation, FeedSubmission, RegistryRotation,
};
use crate::core::rwa_payload::{
    AttestLegal, DistributeRevenue, IssueRwa, RecordEligibility, SettleDvp,
};
use crate::core::sealed_payload::{RevealShare, SealedEnvelope};
use crate::crypto::hybrid::{
    HYBRID_PUBLIC_KEY_LEN, HYBRID_SIGNATURE_LENGTH, HybridPublicKey, HybridSignature,
};
use crate::error::{NodeError, Result};

/// Identifier of a payment channel.
pub type ChannelId = [u8; 32];

/// Domain separator for the bytes both parties sign to authorize a channel
/// state. Distinct from the transaction domain so a channel state can never be
/// replayed as a transaction signature, or vice versa.
const CHANNEL_STATE_DOMAIN: &[u8] = b"maya-flash.channel-state.v1";

/// Domain separator for deriving a channel identifier.
const CHANNEL_ID_DOMAIN: &str = "maya-flash channel id v1";

/// Encoded size of a [`ChannelClosure`].
///
/// 32-byte channel id, three `u64`s, a 32-byte commitment, then two hybrid
/// public keys and two hybrid signatures. Under ed25519 this was 216 bytes;
/// under ML-DSA alone it was 10,610; the key and signature pairs alone now
/// account for over twenty-five kilobytes.
///
/// A closure is the most signature-dense structure on the chain — four proofs
/// in one record — so it is where the hybrid scheme's bulk shows up worst.
pub const CLOSURE_SIZE: usize =
    32 + 8 + 8 + 8 + 32 + 2 * HYBRID_PUBLIC_KEY_LEN + 2 * HYBRID_SIGNATURE_LENGTH;

/// Maximum channels settled by one [`TxKind::SettleBatch`].
///
/// Bounded well below the codec's collection ceiling: each closure costs two
/// signature verifications, so an unbounded batch would be a cheap way to make
/// every node on the network do unbounded work validating one transaction.
///
/// Cut from 4096 to 128 with the move to ML-DSA-65. Both halves of the original
/// reasoning got worse at once: a closure grew from 216 bytes to ~10.5 KB, so
/// 4096 of them is a 43 MB transaction that no gossip layer will carry, and a
/// verification went from ~56 µs to ~200 µs, so 8192 of them is over a second
/// and a half of CPU for one transaction.
///
/// Held at 128 through the move to hybrid signing, and it is now the binding
/// constraint rather than a comfortable one. A closure is ~25.8 KB, so a full
/// batch is ~3.2 MiB of the 8 MiB gossip ceiling — two such transactions to a
/// block, no more. Verification is the half that got off lightly: 256 hybrid
/// checks at ~0.18 ms is ~46 ms, against ~26 ms under ML-DSA alone. The bytes
/// are what hurt, not the CPU.
///
/// The floor on the choice is the hundred-channel batch the L2 scale tests
/// settle in a single transaction; a limit that broke that would be picking a
/// round number over a property the system actually claims. 128 clears that
/// floor with little room, which is the honest description of where this sits.
pub const MAX_BATCH_CLOSURES: usize = 128;

// Payload tags. Never renumber these — they are consensus.
const TAG_OPEN: u8 = 1;
const TAG_COOPERATIVE_CLOSE: u8 = 2;
const TAG_DISPUTE_CLOSE: u8 = 3;
const TAG_PENALTY: u8 = 4;
const TAG_SETTLE_BATCH: u8 = 5;
const TAG_FINALIZE: u8 = 6;
const TAG_DEPLOY: u8 = 7;
const TAG_CALL: u8 = 8;
const TAG_SHIELDED: u8 = 9;
// Trading. The structures these carry live in `crate::core::dex_payload`; the
// numbering stays here, because a numbering split across two files is a
// numbering that gets reused.
const TAG_REGISTER_ASSET: u8 = 10;
const TAG_TRANSFER_ASSET: u8 = 11;
const TAG_CREATE_POOL: u8 = 12;
const TAG_ADD_LIQUIDITY: u8 = 13;
const TAG_REMOVE_LIQUIDITY: u8 = 14;
const TAG_SWAP: u8 = 15;
const TAG_SWAP_ROUTE: u8 = 16;
const TAG_PLACE_ORDER: u8 = 17;
const TAG_CANCEL_ORDER: u8 = 18;
// Oracle. Randomness and external prices — the first constructions on this
// chain that ask it to believe a named party rather than a proof.
const TAG_CREATE_FEED: u8 = 19;
const TAG_SUBMIT_FEED: u8 = 20;
const TAG_ROTATE_AUTHORITIES: u8 = 21;
const TAG_SUBMIT_BEACON: u8 = 22;
// Governance. The chain amending its own rules — see `governance_payload`.
const TAG_CLAIM_WORK: u8 = 23;
const TAG_LOCK_STAKE: u8 = 24;
const TAG_UNLOCK_STAKE: u8 = 25;
const TAG_PROPOSE: u8 = 26;
const TAG_CAST_VOTE: u8 = 27;
const TAG_CANCEL_PROPOSAL: u8 = 28;

// The sealed mempool.
const TAG_SEAL: u8 = 29;
const TAG_REVEAL_SHARE: u8 = 30;

// Self-sovereign identity.
const TAG_REGISTER_DID: u8 = 31;
const TAG_ROTATE_DID_KEY: u8 = 32;
const TAG_REVOKE_DID: u8 = 33;
const TAG_ANCHOR_ATTESTATION: u8 = 34;
const TAG_SET_REVOCATION_BIT: u8 = 35;

// Real-world assets.
const TAG_ISSUE_RWA: u8 = 36;
const TAG_SETTLE_DVP: u8 = 37;
const TAG_RECORD_ELIGIBILITY: u8 = 38;
const TAG_DISTRIBUTE_REVENUE: u8 = 39;
const TAG_ATTEST_LEGAL: u8 = 40;

// Lattice hash-time-locked contracts.
const TAG_HTLC_LOCK: u8 = 41;
const TAG_HTLC_CLAIM: u8 = 42;
const TAG_HTLC_REFUND: u8 = 43;

/// Encoded size of a [`ShieldedJoinSplit`].
///
/// Fixed width by construction: anchor, two nullifiers, two commitments, three
/// amounts, a recipient, and the proof. Nothing here is variable-length, which
/// is what keeps every shielded transaction the same size on the wire — a
/// variable size would leak how many real inputs a spend had.
pub const JOINSPLIT_SIZE: usize = 32 + 32 * 2 + 32 * 2 + 8 * 3 + 32 + 192;

/// Largest contract module a deployment may carry.
///
/// Mirrors the VM''s own module ceiling. Code is stored on chain verbatim and
/// every node keeps it forever, so the limit is a storage decision as much as a
/// validation one.
pub const MAX_CONTRACT_CODE: usize = 512 * 1024;

/// Largest input a contract call may carry.
pub const MAX_CALL_INPUT: usize = 64 * 1024;

/// Deploying a contract module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContractDeploy {
    /// The WebAssembly module.
    pub code: Vec<u8>,
}

/// Calling a deployed contract.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContractCall {
    /// Contract to invoke.
    pub contract: ChannelId,
    /// Opaque input handed to the contract.
    pub input: Vec<u8>,
    /// Gas ceiling.
    ///
    /// A halting bound, not a price: exceeding it traps the call and discards
    /// its writes, but nothing is billed to the sender.
    pub gas_limit: u64,
}

/// A shielded joinsplit: the on-chain form of a private transaction.
///
/// Carries no sender, no recipient, and no amount for the shielded side. What
/// it does carry is a proof that *some* set of notes under `anchor` was spent
/// by whoever holds their keys, that the two published nullifiers are the ones
/// those notes yield, and that value balanced.
///
/// ## The transparent edges
///
/// `public_in` is debited from the transaction's signer and `public_out` is
/// credited to `recipient`. Those two are necessarily public — value entering
/// or leaving the pool has to touch an account that everyone can see. A
/// shielded-to-shielded transfer leaves both at zero and reveals nothing.
///
/// `recipient` is bound into the proof, so a miner who rewrites it invalidates
/// the transaction rather than redirecting the withdrawal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShieldedJoinSplit {
    /// Commitment tree root the spent notes are proved against.
    pub anchor: [u8; 32],
    /// Nullifiers retired by this joinsplit.
    pub nullifiers: [[u8; 32]; 2],
    /// Note commitments created by this joinsplit.
    pub commitments: [[u8; 32]; 2],
    /// Transparent value entering the pool, debited from the signer.
    pub public_in: u64,
    /// Transparent value leaving the pool, credited to `recipient`.
    pub public_out: u64,
    /// Fee, paid in the clear.
    pub fee: u64,
    /// Transparent account receiving `public_out`.
    pub recipient: [u8; 32],
    /// The Groth16 proof.
    pub proof: [u8; 192],
}

impl ShieldedJoinSplit {
    /// Appends the fixed-width encoding.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.anchor);
        for nullifier in &self.nullifiers {
            buf.extend_from_slice(nullifier);
        }
        for commitment in &self.commitments {
            buf.extend_from_slice(commitment);
        }
        buf.extend_from_slice(&self.public_in.to_le_bytes());
        buf.extend_from_slice(&self.public_out.to_le_bytes());
        buf.extend_from_slice(&self.fee.to_le_bytes());
        buf.extend_from_slice(&self.recipient);
        buf.extend_from_slice(&self.proof);
    }

    /// Decodes a joinsplit.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the section is truncated.
    pub fn decode(reader: &mut ByteReader<'_>) -> Result<Self> {
        Ok(Self {
            anchor: reader.read_array::<32>()?,
            nullifiers: [reader.read_array::<32>()?, reader.read_array::<32>()?],
            commitments: [reader.read_array::<32>()?, reader.read_array::<32>()?],
            public_in: reader.read_u64()?,
            public_out: reader.read_u64()?,
            fee: reader.read_u64()?,
            recipient: reader.read_array::<32>()?,
            proof: reader.read_array::<192>()?,
        })
    }
}

/// Opening a channel: escrow `funding` against `counterparty`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelOpen {
    /// The other participant's address.
    pub counterparty: [u8; 32],
    /// Amount the opener locks into the channel.
    pub funding: u64,
    /// Blocks a unilateral close must wait before funds can be swept.
    ///
    /// This is the window in which a defrauded counterparty can submit a
    /// penalty proof; too short and the punishment is unenforceable.
    pub dispute_window: u64,
}

/// A final channel state, signed by both participants.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelClosure {
    /// Channel being settled.
    pub channel_id: ChannelId,
    /// State sequence number. Higher supersedes lower.
    pub seq: u64,
    /// Final balance of party A.
    pub balance_a: u64,
    /// Final balance of party B.
    pub balance_b: u64,
    /// Commitment to this state's revocation secret.
    ///
    /// A single secret per state, known to both parties once they advance past
    /// it. Whoever publishes a revoked state can be punished by the other with
    /// the preimage; the publisher knowing it too is harmless, since the chain
    /// refuses a penalty claim from the closer.
    pub revocation_commitment: [u8; 32],
    /// Party A's ML-DSA-65 public key.
    ///
    /// Carried here rather than stored in the channel record, for the same
    /// reason a transaction carries its own key: an address is now a hash and
    /// no longer reveals the key that controls it. The chain binds this key to
    /// the recorded participant by checking `address_of(pubkey_a) == party_a`
    /// before it verifies anything, so supplying someone else's key fails at
    /// that check rather than at the signature.
    ///
    /// Keeping the keys in the closure rather than the record also keeps the
    /// on-disk channel record small: a record is written once per channel and
    /// read on every settlement, while a closure appears once.
    pub pubkey_a: Box<HybridPublicKey>,
    /// Party B's hybrid public key pair.
    pub pubkey_b: Box<HybridPublicKey>,
    /// Party A's signature pair over [`channel_state_signing_bytes`].
    pub sig_a: Box<HybridSignature>,
    /// Party B's signature pair over the same bytes.
    pub sig_b: Box<HybridSignature>,
}

impl ChannelClosure {
    /// The bytes both parties sign for this state.
    #[must_use]
    pub fn signing_bytes(&self) -> Vec<u8> {
        channel_state_signing_bytes(
            &self.channel_id,
            self.seq,
            self.balance_a,
            self.balance_b,
            &self.revocation_commitment,
        )
    }

    /// Total value the closure distributes.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::BalanceOverflow`] if the balances overflow `u64`.
    pub fn total(&self) -> Result<u64> {
        maya_ledger_math::combined_balance(self.balance_a, self.balance_b)
            .ok_or(NodeError::BalanceOverflow)
    }

    fn encode_into(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.channel_id);
        buf.extend_from_slice(&self.seq.to_le_bytes());
        buf.extend_from_slice(&self.balance_a.to_le_bytes());
        buf.extend_from_slice(&self.balance_b.to_le_bytes());
        buf.extend_from_slice(&self.revocation_commitment);
        self.pubkey_a.encode_into(buf);
        self.pubkey_b.encode_into(buf);
        self.sig_a.encode_into(buf);
        self.sig_b.encode_into(buf);
    }

    fn decode(reader: &mut ByteReader<'_>) -> Result<Self> {
        Ok(Self {
            channel_id: reader.read_array::<32>()?,
            seq: reader.read_u64()?,
            balance_a: reader.read_u64()?,
            balance_b: reader.read_u64()?,
            revocation_commitment: reader.read_array::<32>()?,
            pubkey_a: Box::new(HybridPublicKey::decode(reader)?),
            pubkey_b: Box::new(HybridPublicKey::decode(reader)?),
            sig_a: Box::new(HybridSignature::decode(reader)?),
            sig_b: Box::new(HybridSignature::decode(reader)?),
        })
    }
}

/// Proof that a submitted channel state had already been revoked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RevocationProof {
    /// Channel the fraud occurred on.
    pub channel_id: ChannelId,
    /// Sequence number of the revoked state that was submitted.
    pub revoked_seq: u64,
    /// Preimage of the revoked state's commitment.
    ///
    /// Knowing it proves the state was superseded: a party only learns the
    /// secret when its counterparty advances past that state.
    pub secret: [u8; 32],
}

/// What a transaction does.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum TxKind {
    /// A plain value transfer. Carries no payload section.
    #[default]
    Transfer,
    /// Lock funds into a new channel.
    OpenChannel(ChannelOpen),
    /// Close a channel immediately on a mutually signed final state.
    CooperativeClose(ChannelClosure),
    /// Unilaterally close, opening the dispute window.
    DisputeClose(ChannelClosure),
    /// Punish a counterparty that submitted a revoked state.
    PenaltyClaim(RevocationProof),
    /// Settle many channels in one transaction.
    SettleBatch(Vec<ChannelClosure>),
    /// Pay out a dispute whose window has elapsed with no penalty claim.
    FinalizeDispute(ChannelId),
    /// Store a new contract module on chain.
    DeployContract(ContractDeploy),
    /// Invoke a deployed contract.
    CallContract(ContractCall),
    /// A private transaction against the shielded pool.
    Shielded(Box<ShieldedJoinSplit>),
    /// Create a new asset, crediting its whole supply to the sender.
    RegisterAsset(AssetRegistration),
    /// Move units of a non-native asset.
    TransferAsset(AssetTransfer),
    /// Create a constant-product pool and seed it.
    CreatePool(PoolCreation),
    /// Deposit into a pool, minting shares.
    AddLiquidity(LiquidityDeposit),
    /// Burn shares, withdrawing a slice of both reserves.
    RemoveLiquidity(LiquidityWithdrawal),
    /// Swap against a pool. Settles in that pool's batch for the block rather
    /// than where it sits in the block — see [`crate::core::dex_payload`].
    Swap(SwapRequest),
    /// An atomic multi-hop swap, executed in place.
    SwapRoute(SwapRoute),
    /// Rest a limit order on a book.
    PlaceOrder(OrderPlacement),
    /// Withdraw a resting order and refund its escrow.
    CancelOrder(ChannelId),
    /// Create a price feed, so that submissions have somewhere to land.
    CreateFeed(FeedCreation),
    /// Update a price feed with a quorum of signed observations.
    ///
    /// Boxed: a quorum of post-quantum signatures is tens of kilobytes, and an
    /// unboxed variant would make every `TxKind` in the process that large.
    SubmitFeed(Box<FeedSubmission>),
    /// Replace the oracle authority set, approved by the outgoing one.
    RotateAuthorities(Box<RegistryRotation>),
    /// Prove this block's randomness.
    SubmitBeacon(BeaconSubmission),
    /// Credit this block's work to an address the miner names.
    ClaimWork(WorkClaim),
    /// Lock native coin for voting weight.
    LockStake(StakeLock),
    /// Withdraw locked coin once its height has passed.
    UnlockStake(StakeUnlock),
    /// Open a proposal to change the chain's own rules.
    Propose(Box<ProposalSubmission>),
    /// Vote on an open proposal.
    CastVote(Ballot),
    /// Withdraw a proposal before voting closes.
    CancelProposal(ChannelId),
    /// Submit an action nobody can read until its reveal height.
    ///
    /// Boxed: the ciphertext is a heap allocation either way, and an unboxed
    /// variant would widen every `TxKind` in the process to hold a `Vec`
    /// inline.
    Seal(Box<SealedEnvelope>),
    /// Contribute one committee member's share toward opening an envelope.
    RevealShare(RevealShare),
    /// Register `did:maya2c:<sender>`. The subject is the sender, never a
    /// field: a registration that named its subject would let anyone register a
    /// document for anybody.
    RegisterDid(Box<RegisterDid>),
    /// Install a new key, authorised by the one it replaces — the transaction
    /// is signed by the current key by construction.
    RotateDidKey(Box<RotateDidKey>),
    /// Revoke the sender's own DID, permanently.
    RevokeDid(RevokeDid),
    /// Publish an issuer's credential-tree root. Carries a root, never a claim.
    AnchorAttestation(Box<AnchorAttestation>),
    /// Flip one bit of the sender's own revocation bitmap.
    SetRevocationBit(SetRevocationBit),
    /// Issue a real-world asset token, seating its supply with the sender.
    IssueRwa(Box<IssueRwa>),
    /// Swap native coin for RWA units. A swap that cannot settle is a
    /// **no-op**, never an error — invariant 7.
    SettleDvp(Box<SettleDvp>),
    /// Record that the sender cleared an asset's transfer rule.
    RecordEligibility(RecordEligibility),
    /// Pay revenue across every holder on the named cap table pages.
    DistributeRevenue(Box<DistributeRevenue>),
    /// Record a legal attestation about an asset. A hash, never a document.
    AttestLegal(Box<AttestLegal>),
    /// Escrow native coin under a lattice commitment until an expiry height.
    HtlcLock(Box<HtlcLock>),
    /// Claim a lattice HTLC by publishing its opening. A claim that loses is a
    /// **no-op**, never an error — invariant 7.
    HtlcClaim(Box<HtlcClaim>),
    /// Refund an expired lattice HTLC. A no-op if it is not yet expired or
    /// already settled.
    HtlcRefund(HtlcRefund),
}

impl TxKind {
    /// Whether this kind carries a payload section on the wire.
    #[must_use]
    pub fn has_payload(&self) -> bool {
        !matches!(self, Self::Transfer)
    }

    /// Short label for errors and logs.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::Transfer => "transfer",
            Self::OpenChannel(_) => "open_channel",
            Self::CooperativeClose(_) => "cooperative_close",
            Self::DisputeClose(_) => "dispute_close",
            Self::PenaltyClaim(_) => "penalty_claim",
            Self::SettleBatch(_) => "settle_batch",
            Self::FinalizeDispute(_) => "finalize_dispute",
            Self::DeployContract(_) => "deploy_contract",
            Self::CallContract(_) => "call_contract",
            Self::Shielded(_) => "shielded",
            Self::RegisterAsset(_) => "register_asset",
            Self::TransferAsset(_) => "transfer_asset",
            Self::CreatePool(_) => "create_pool",
            Self::AddLiquidity(_) => "add_liquidity",
            Self::RemoveLiquidity(_) => "remove_liquidity",
            Self::Swap(_) => "swap",
            Self::SwapRoute(_) => "swap_route",
            Self::PlaceOrder(_) => "place_order",
            Self::CancelOrder(_) => "cancel_order",
            Self::CreateFeed(_) => "create_feed",
            Self::SubmitFeed(_) => "submit_feed",
            Self::RotateAuthorities(_) => "rotate_authorities",
            Self::SubmitBeacon(_) => "submit_beacon",
            Self::ClaimWork(_) => "claim_work",
            Self::LockStake(_) => "lock_stake",
            Self::UnlockStake(_) => "unlock_stake",
            Self::Propose(_) => "propose",
            Self::CastVote(_) => "cast_vote",
            Self::CancelProposal(_) => "cancel_proposal",
            Self::Seal(_) => "seal",
            Self::RevealShare(_) => "reveal_share",
            Self::RegisterDid(_) => "register_did",
            Self::RotateDidKey(_) => "rotate_did_key",
            Self::RevokeDid(_) => "revoke_did",
            Self::AnchorAttestation(_) => "anchor_attestation",
            Self::SetRevocationBit(_) => "set_revocation_bit",
            Self::IssueRwa(_) => "issue_rwa",
            Self::SettleDvp(_) => "settle_dvp",
            Self::RecordEligibility(_) => "record_eligibility",
            Self::DistributeRevenue(_) => "distribute_revenue",
            Self::AttestLegal(_) => "attest_legal",
            Self::HtlcLock(_) => "htlc_lock",
            Self::HtlcClaim(_) => "htlc_claim",
            Self::HtlcRefund(_) => "htlc_refund",
        }
    }

    /// Appends the payload encoding.
    ///
    /// Writes nothing for [`TxKind::Transfer`], which is what preserves the
    /// legacy signing bytes.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        match self {
            Self::Transfer => {}
            Self::OpenChannel(open) => {
                buf.push(TAG_OPEN);
                buf.extend_from_slice(&open.counterparty);
                buf.extend_from_slice(&open.funding.to_le_bytes());
                buf.extend_from_slice(&open.dispute_window.to_le_bytes());
            }
            Self::CooperativeClose(closure) => {
                buf.push(TAG_COOPERATIVE_CLOSE);
                closure.encode_into(buf);
            }
            Self::DisputeClose(closure) => {
                buf.push(TAG_DISPUTE_CLOSE);
                closure.encode_into(buf);
            }
            Self::PenaltyClaim(proof) => {
                buf.push(TAG_PENALTY);
                buf.extend_from_slice(&proof.channel_id);
                buf.extend_from_slice(&proof.revoked_seq.to_le_bytes());
                buf.extend_from_slice(&proof.secret);
            }
            Self::SettleBatch(closures) => {
                buf.push(TAG_SETTLE_BATCH);
                buf.extend_from_slice(&(closures.len() as u64).to_le_bytes());
                for closure in closures {
                    closure.encode_into(buf);
                }
            }
            Self::FinalizeDispute(channel_id) => {
                buf.push(TAG_FINALIZE);
                buf.extend_from_slice(channel_id);
            }
            Self::DeployContract(deploy) => {
                buf.push(TAG_DEPLOY);
                buf.extend_from_slice(&(deploy.code.len() as u64).to_le_bytes());
                buf.extend_from_slice(&deploy.code);
            }
            Self::CallContract(call) => {
                buf.push(TAG_CALL);
                buf.extend_from_slice(&call.contract);
                buf.extend_from_slice(&call.gas_limit.to_le_bytes());
                buf.extend_from_slice(&(call.input.len() as u64).to_le_bytes());
                buf.extend_from_slice(&call.input);
            }
            Self::Shielded(joinsplit) => {
                buf.push(TAG_SHIELDED);
                joinsplit.encode_into(buf);
            }
            Self::RegisterAsset(registration) => {
                buf.push(TAG_REGISTER_ASSET);
                registration.encode_into(buf);
            }
            Self::TransferAsset(transfer) => {
                buf.push(TAG_TRANSFER_ASSET);
                transfer.encode_into(buf);
            }
            Self::CreatePool(creation) => {
                buf.push(TAG_CREATE_POOL);
                creation.encode_into(buf);
            }
            Self::AddLiquidity(deposit) => {
                buf.push(TAG_ADD_LIQUIDITY);
                deposit.encode_into(buf);
            }
            Self::RemoveLiquidity(withdrawal) => {
                buf.push(TAG_REMOVE_LIQUIDITY);
                withdrawal.encode_into(buf);
            }
            Self::Swap(request) => {
                buf.push(TAG_SWAP);
                request.encode_into(buf);
            }
            Self::SwapRoute(route) => {
                buf.push(TAG_SWAP_ROUTE);
                route.encode_into(buf);
            }
            Self::PlaceOrder(placement) => {
                buf.push(TAG_PLACE_ORDER);
                placement.encode_into(buf);
            }
            Self::CancelOrder(order_id) => {
                buf.push(TAG_CANCEL_ORDER);
                buf.extend_from_slice(order_id);
            }
            Self::CreateFeed(creation) => {
                buf.push(TAG_CREATE_FEED);
                creation.encode_into(buf);
            }
            Self::SubmitFeed(submission) => {
                buf.push(TAG_SUBMIT_FEED);
                submission.encode_into(buf);
            }
            Self::RotateAuthorities(rotation) => {
                buf.push(TAG_ROTATE_AUTHORITIES);
                rotation.encode_into(buf);
            }
            Self::SubmitBeacon(beacon) => {
                buf.push(TAG_SUBMIT_BEACON);
                beacon.encode_into(buf);
            }
            Self::ClaimWork(claim) => {
                buf.push(TAG_CLAIM_WORK);
                claim.encode_into(buf);
            }
            Self::LockStake(lock) => {
                buf.push(TAG_LOCK_STAKE);
                lock.encode_into(buf);
            }
            Self::UnlockStake(unlock) => {
                buf.push(TAG_UNLOCK_STAKE);
                unlock.encode_into(buf);
            }
            Self::Propose(submission) => {
                buf.push(TAG_PROPOSE);
                submission.encode_into(buf);
            }
            Self::CastVote(ballot) => {
                buf.push(TAG_CAST_VOTE);
                ballot.encode_into(buf);
            }
            Self::CancelProposal(id) => {
                buf.push(TAG_CANCEL_PROPOSAL);
                buf.extend_from_slice(id);
            }
            Self::Seal(envelope) => {
                buf.push(TAG_SEAL);
                envelope.encode_into(buf);
            }
            Self::RegisterDid(payload) => {
                buf.push(TAG_REGISTER_DID);
                payload.encode_into(buf);
            }
            Self::RotateDidKey(payload) => {
                buf.push(TAG_ROTATE_DID_KEY);
                payload.encode_into(buf);
            }
            Self::RevokeDid(payload) => {
                buf.push(TAG_REVOKE_DID);
                payload.encode_into(buf);
            }
            Self::AnchorAttestation(payload) => {
                buf.push(TAG_ANCHOR_ATTESTATION);
                payload.encode_into(buf);
            }
            Self::SetRevocationBit(payload) => {
                buf.push(TAG_SET_REVOCATION_BIT);
                payload.encode_into(buf);
            }
            Self::IssueRwa(payload) => {
                buf.push(TAG_ISSUE_RWA);
                payload.encode_into(buf);
            }
            Self::SettleDvp(payload) => {
                buf.push(TAG_SETTLE_DVP);
                payload.encode_into(buf);
            }
            Self::RecordEligibility(payload) => {
                buf.push(TAG_RECORD_ELIGIBILITY);
                payload.encode_into(buf);
            }
            Self::DistributeRevenue(payload) => {
                buf.push(TAG_DISTRIBUTE_REVENUE);
                payload.encode_into(buf);
            }
            Self::AttestLegal(payload) => {
                buf.push(TAG_ATTEST_LEGAL);
                payload.encode_into(buf);
            }
            Self::HtlcLock(payload) => {
                buf.push(TAG_HTLC_LOCK);
                payload.encode_into(buf);
            }
            Self::HtlcClaim(payload) => {
                buf.push(TAG_HTLC_CLAIM);
                payload.encode_into(buf);
            }
            Self::HtlcRefund(payload) => {
                buf.push(TAG_HTLC_REFUND);
                payload.encode_into(buf);
            }
            Self::RevealShare(share) => {
                buf.push(TAG_REVEAL_SHARE);
                share.encode_into(buf);
            }
        }
    }

    /// Decodes a payload section.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] for an unknown tag, a truncated section,
    /// or a batch larger than [`MAX_BATCH_CLOSURES`].
    pub fn decode(reader: &mut ByteReader<'_>) -> Result<Self> {
        let tag = reader.read_u8()?;
        match tag {
            TAG_OPEN => Ok(Self::OpenChannel(ChannelOpen {
                counterparty: reader.read_array::<32>()?,
                funding: reader.read_u64()?,
                dispute_window: reader.read_u64()?,
            })),
            TAG_COOPERATIVE_CLOSE => Ok(Self::CooperativeClose(ChannelClosure::decode(reader)?)),
            TAG_DISPUTE_CLOSE => Ok(Self::DisputeClose(ChannelClosure::decode(reader)?)),
            TAG_PENALTY => Ok(Self::PenaltyClaim(RevocationProof {
                channel_id: reader.read_array::<32>()?,
                revoked_seq: reader.read_u64()?,
                secret: reader.read_array::<32>()?,
            })),
            TAG_SETTLE_BATCH => {
                let count = reader.read_collection_len(CLOSURE_SIZE)?;
                if count > MAX_BATCH_CLOSURES {
                    return Err(NodeError::Decode(format!(
                        "batch of {count} closures exceeds the maximum {MAX_BATCH_CLOSURES}"
                    )));
                }
                let mut closures = Vec::with_capacity(count);
                for _ in 0..count {
                    closures.push(ChannelClosure::decode(reader)?);
                }
                Ok(Self::SettleBatch(closures))
            }
            TAG_FINALIZE => Ok(Self::FinalizeDispute(reader.read_array::<32>()?)),
            TAG_DEPLOY => {
                let length = reader.read_collection_len(1)?;
                if length > MAX_CONTRACT_CODE {
                    return Err(NodeError::Decode(format!(
                        "contract code of {length} bytes exceeds the maximum {MAX_CONTRACT_CODE}"
                    )));
                }
                Ok(Self::DeployContract(ContractDeploy {
                    code: reader.read_slice(length)?.to_vec(),
                }))
            }
            TAG_CALL => {
                let contract = reader.read_array::<32>()?;
                let gas_limit = reader.read_u64()?;
                let length = reader.read_collection_len(1)?;
                if length > MAX_CALL_INPUT {
                    return Err(NodeError::Decode(format!(
                        "call input of {length} bytes exceeds the maximum {MAX_CALL_INPUT}"
                    )));
                }
                Ok(Self::CallContract(ContractCall {
                    contract,
                    gas_limit,
                    input: reader.read_slice(length)?.to_vec(),
                }))
            }
            TAG_SHIELDED => Ok(Self::Shielded(Box::new(ShieldedJoinSplit::decode(reader)?))),
            TAG_REGISTER_ASSET => Ok(Self::RegisterAsset(AssetRegistration::decode(reader)?)),
            TAG_TRANSFER_ASSET => Ok(Self::TransferAsset(AssetTransfer::decode(reader)?)),
            TAG_CREATE_POOL => Ok(Self::CreatePool(PoolCreation::decode(reader)?)),
            TAG_ADD_LIQUIDITY => Ok(Self::AddLiquidity(LiquidityDeposit::decode(reader)?)),
            TAG_REMOVE_LIQUIDITY => Ok(Self::RemoveLiquidity(LiquidityWithdrawal::decode(reader)?)),
            TAG_SWAP => Ok(Self::Swap(SwapRequest::decode(reader)?)),
            TAG_SWAP_ROUTE => Ok(Self::SwapRoute(SwapRoute::decode(reader)?)),
            TAG_PLACE_ORDER => Ok(Self::PlaceOrder(OrderPlacement::decode(reader)?)),
            TAG_CANCEL_ORDER => Ok(Self::CancelOrder(reader.read_array::<32>()?)),
            TAG_CREATE_FEED => Ok(Self::CreateFeed(FeedCreation::decode(reader)?)),
            TAG_SUBMIT_FEED => Ok(Self::SubmitFeed(Box::new(FeedSubmission::decode(reader)?))),
            TAG_ROTATE_AUTHORITIES => Ok(Self::RotateAuthorities(Box::new(
                RegistryRotation::decode(reader)?,
            ))),
            TAG_SUBMIT_BEACON => Ok(Self::SubmitBeacon(BeaconSubmission::decode(reader)?)),
            TAG_CLAIM_WORK => Ok(Self::ClaimWork(WorkClaim::decode(reader)?)),
            TAG_LOCK_STAKE => Ok(Self::LockStake(StakeLock::decode(reader)?)),
            TAG_UNLOCK_STAKE => Ok(Self::UnlockStake(StakeUnlock::decode(reader)?)),
            TAG_PROPOSE => Ok(Self::Propose(Box::new(ProposalSubmission::decode(reader)?))),
            TAG_CAST_VOTE => Ok(Self::CastVote(Ballot::decode(reader)?)),
            TAG_CANCEL_PROPOSAL => Ok(Self::CancelProposal(reader.read_array::<32>()?)),
            TAG_SEAL => Ok(Self::Seal(Box::new(SealedEnvelope::decode(reader)?))),
            TAG_REVEAL_SHARE => Ok(Self::RevealShare(RevealShare::decode(reader)?)),
            TAG_REGISTER_DID => Ok(Self::RegisterDid(Box::new(RegisterDid::decode(reader)?))),
            TAG_ROTATE_DID_KEY => Ok(Self::RotateDidKey(Box::new(RotateDidKey::decode(reader)?))),
            TAG_REVOKE_DID => Ok(Self::RevokeDid(RevokeDid::decode(reader)?)),
            TAG_ANCHOR_ATTESTATION => Ok(Self::AnchorAttestation(Box::new(
                AnchorAttestation::decode(reader)?,
            ))),
            TAG_SET_REVOCATION_BIT => Ok(Self::SetRevocationBit(SetRevocationBit::decode(reader)?)),
            TAG_ISSUE_RWA => Ok(Self::IssueRwa(Box::new(IssueRwa::decode(reader)?))),
            TAG_SETTLE_DVP => Ok(Self::SettleDvp(Box::new(SettleDvp::decode(reader)?))),
            TAG_RECORD_ELIGIBILITY => Ok(Self::RecordEligibility(RecordEligibility::decode(
                reader,
            )?)),
            TAG_DISTRIBUTE_REVENUE => Ok(Self::DistributeRevenue(Box::new(
                DistributeRevenue::decode(reader)?,
            ))),
            TAG_ATTEST_LEGAL => Ok(Self::AttestLegal(Box::new(AttestLegal::decode(reader)?))),
            TAG_HTLC_LOCK => Ok(Self::HtlcLock(Box::new(HtlcLock::decode(reader)?))),
            TAG_HTLC_CLAIM => Ok(Self::HtlcClaim(Box::new(HtlcClaim::decode(reader)?))),
            TAG_HTLC_REFUND => Ok(Self::HtlcRefund(HtlcRefund::decode(reader)?)),
            other => Err(NodeError::Decode(format!(
                "unknown transaction payload tag {other}"
            ))),
        }
    }
}

/// The canonical bytes both participants sign to authorize a channel state.
///
/// Shared by the L2 crate (which produces signatures) and the L1 settlement
/// path (which verifies them). Keeping one definition is what stops the two
/// sides from silently disagreeing about what was signed.
#[must_use]
pub fn channel_state_signing_bytes(
    channel_id: &ChannelId,
    seq: u64,
    balance_a: u64,
    balance_b: u64,
    revocation_commitment: &[u8; 32],
) -> Vec<u8> {
    let mut buf = Vec::with_capacity(CHANNEL_STATE_DOMAIN.len() + 88);
    buf.extend_from_slice(CHANNEL_STATE_DOMAIN);
    buf.extend_from_slice(channel_id);
    buf.extend_from_slice(&seq.to_le_bytes());
    buf.extend_from_slice(&balance_a.to_le_bytes());
    buf.extend_from_slice(&balance_b.to_le_bytes());
    // Signed, not merely carried: an unsigned commitment could be swapped by
    // the submitter to one whose preimage nobody holds, defeating the penalty.
    buf.extend_from_slice(revocation_commitment);
    buf
}

/// Derives a contract address from its deployer, nonce, and code.
///
/// Includes the code so redeploying different bytes cannot land on an existing
/// contract, and the nonce so one deployer can publish the same module twice.
#[must_use]
pub fn derive_contract_id(deployer: &[u8; 32], nonce: u64, code: &[u8]) -> ChannelId {
    let mut hasher = blake3::Hasher::new_derive_key("maya-vm contract id v1");
    hasher.update(deployer);
    hasher.update(&nonce.to_le_bytes());
    hasher.update(code);
    *hasher.finalize().as_bytes()
}

/// Derives a channel identifier from its opening parameters.
///
/// Includes the opener's nonce so the same pair can hold many channels without
/// the identifiers colliding.
#[must_use]
pub fn derive_channel_id(
    party_a: &[u8; 32],
    party_b: &[u8; 32],
    funding: u64,
    nonce: u64,
) -> ChannelId {
    let mut hasher = blake3::Hasher::new_derive_key(CHANNEL_ID_DOMAIN);
    hasher.update(party_a);
    hasher.update(party_b);
    hasher.update(&funding.to_le_bytes());
    hasher.update(&nonce.to_le_bytes());
    *hasher.finalize().as_bytes()
}

/// Commitment to a revocation secret.
///
/// The commitment goes on-chain with a submitted state; the preimage is what a
/// defrauded party later produces to prove the state was revoked.
#[must_use]
pub fn revocation_commitment(secret: &[u8; 32]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new_derive_key("maya-flash revocation commitment v1");
    hasher.update(secret);
    *hasher.finalize().as_bytes()
}
