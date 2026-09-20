//! Error types for the node.
//!
//! Every fallible operation returns [`Result`]; no production path panics.

use thiserror::Error;

/// Errors produced by node crypto and consensus primitives.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum NodeError {
    /// Argon2 rejected the requested parameter set.
    #[error("invalid Argon2 parameters: {0}")]
    InvalidArgonParams(String),

    /// The Argon2id pass itself failed.
    #[error("Argon2id hashing failed: {0}")]
    ArgonHash(String),

    /// A proof-of-work cache or dataset could not be allocated.
    ///
    /// Reported rather than left to the allocator's abort path. The cache is
    /// tens of megabytes and the dataset is gigabytes, so this is a plausible
    /// outcome on a small machine — and a validator that cannot spare the
    /// memory should say so and exit cleanly, not die inside a `Vec` growth.
    #[error("could not allocate {bytes} bytes for the proof-of-work dataset")]
    DagAllocation {
        /// Size of the failed reservation.
        bytes: usize,
    },

    /// An epoch's verification cache could not be made available.
    ///
    /// Distinct from [`NodeError::DagAllocation`]: that one means the memory
    /// was refused, this one means the registry holding the caches is unusable
    /// — a thread panicked while generating one. Validation cannot proceed
    /// either way, but the operator's next move is different.
    #[error("proof-of-work cache for epoch {epoch} is unavailable: {reason}")]
    DagCacheUnavailable {
        /// Epoch whose cache was requested.
        epoch: u64,
        /// What went wrong.
        reason: String,
    },

    /// The public key bytes are not a valid ML-DSA-65 encoding.
    #[error("malformed ML-DSA-65 public key")]
    MalformedPublicKey,

    /// Verification was attempted on a transaction that has not been signed.
    #[error("transaction is not signed")]
    MissingSignature,

    /// The lattice (FIPS 204) signature did not verify against the payload and
    /// public key.
    #[error("ML-DSA-65 signature verification failed")]
    SignatureVerification,

    /// The hash-based (FIPS 205) signature did not verify against the payload
    /// and public key.
    ///
    /// Distinguished from [`NodeError::SignatureVerification`] on purpose.
    /// Every transaction carries both proofs and both must pass, so which half
    /// failed carries real operational meaning: a chain that starts rejecting
    /// one scheme's proofs and not the other's is a chain whose operators need
    /// to know which scheme, immediately.
    #[error("SLH-DSA-SHA2-128s signature verification failed")]
    HashSignatureVerification,

    /// The post-quantum transport handshake failed.
    ///
    /// Distinct from [`NodeError::Network`] because it names a specific,
    /// actionable condition: the peer either does not speak the ML-KEM upgrade
    /// or sent something unusable. An operator seeing these needs to know it is
    /// a protocol mismatch, not a socket problem.
    #[error("ML-KEM transport handshake failed: {0}")]
    PqHandshake(String),

    /// A wire frame received from a peer could not be decoded.
    #[error("malformed wire encoding: {0}")]
    Decode(String),

    /// A transaction was rejected before entering the mempool.
    #[error("transaction rejected: {0}")]
    MempoolRejected(String),

    /// The networking stack reported a failure.
    #[error("network error: {0}")]
    Network(String),

    /// The underlying key-value store reported a failure.
    #[error("storage error: {0}")]
    Storage(String),

    /// The transaction's nonce did not match the sender's expected next nonce.
    ///
    /// This is the condition that rejects a replayed or double-spent
    /// transaction: the first spend advances the nonce, so a second one
    /// carrying the same value no longer matches.
    #[error("invalid nonce for {address}: expected {expected}, got {actual}")]
    InvalidNonce {
        /// Hex-encoded sender address.
        address: String,
        /// Nonce the sender's account currently expects.
        expected: u64,
        /// Nonce the transaction supplied.
        actual: u64,
    },

    /// The sender cannot cover the transaction's outputs.
    #[error("insufficient balance for {address}: need {required}, have {available}")]
    InsufficientBalance {
        /// Hex-encoded sender address.
        address: String,
        /// Total value the transaction attempts to move.
        required: u64,
        /// Balance actually available.
        available: u64,
    },

    /// An arithmetic operation on balances would overflow `u64`.
    #[error("balance arithmetic overflowed")]
    BalanceOverflow,

    /// The computed post-state root did not match the value committed in the
    /// block header.
    #[error("state root mismatch: header commits {expected}, execution produced {actual}")]
    StateRootMismatch {
        /// Hex-encoded root from the block header.
        expected: String,
        /// Hex-encoded root produced by executing the block.
        actual: String,
    },

    /// The database was started with a different genesis block than the one
    /// configured. Opening it would graft one chain's history onto another's.
    #[error("database holds genesis {stored}, configuration names {configured}")]
    GenesisMismatch {
        /// Hex id of the genesis stored in the database.
        stored: String,
        /// Hex id of the genesis the configuration produced.
        configured: String,
    },

    /// A block or reorg reaches at or below the height this node has pruned.
    ///
    /// A pruned node holds no undo journal there, so it cannot revert to it,
    /// and it treats blocks that deep as final by local policy.
    #[error("height {height} is at or below the prune horizon {horizon}")]
    BelowPruneHorizon {
        /// The height the block or reorg would reach.
        height: u64,
        /// This node's prune horizon.
        horizon: u64,
    },

    /// A block's state transition breaks an invariant the guard checks.
    ///
    /// Refused exactly as a wrong state root is refused: the block creates or
    /// destroys value, nothing commits, and no breaker is written because
    /// there is no committed state to protect. See
    /// [`crate::state::invariant_guard::conservation`].
    #[error("invariant violated: {0}")]
    InvariantViolation(String),

    /// A module is in emergency read-only mode.
    ///
    /// Set by the circuit breaker after an anomaly, and cleared by height
    /// rather than by a write. Transfers are never gated by it — see
    /// [`crate::state::invariant_guard::Module`].
    #[error("{module} is halted until block {until}: {invariant} tripped at block {tripped_at}")]
    ModuleHalted {
        /// The halted module.
        module: &'static str,
        /// Which check fired.
        invariant: &'static str,
        /// Height of the block that tripped it.
        tripped_at: u64,
        /// First height at which the module is usable again.
        until: u64,
    },

    /// Archiving pruned blocks, or fetching one back, failed.
    #[error("archive: {0}")]
    Archive(String),

    /// A block's transactions are not the ones its header commits to.
    ///
    /// The header — and so the block id and the proof of work — binds the
    /// transaction list through `tx_root`. A body that disagrees is somebody
    /// else's transactions under an honest miner's work.
    #[error("tx root mismatch: header commits {expected}, the body hashes to {actual}")]
    TxRootMismatch {
        /// Hex-encoded root from the block header.
        expected: String,
        /// Hex-encoded root of the transactions actually carried.
        actual: String,
    },

    /// A channel already exists under the derived identifier.
    #[error("channel {0} already exists")]
    ChannelExists(String),

    /// A channel operation referenced an unknown channel.
    #[error("unknown channel {0}")]
    UnknownChannel(String),

    /// An operation was attempted on a channel in the wrong lifecycle state.
    #[error("channel {channel} is {actual}, expected {expected}")]
    ChannelState {
        /// Hex-encoded channel id.
        channel: String,
        /// Status the channel is actually in.
        actual: &'static str,
        /// Status the operation required.
        expected: &'static str,
    },

    /// A closure's signature did not verify against a registered participant.
    #[error("channel {channel}: signature for party {party} is invalid")]
    ClosureSignature {
        /// Hex-encoded channel id.
        channel: String,
        /// Which side failed: `"a"` or `"b"`.
        party: &'static str,
    },

    /// A closure distributed a different total than the channel holds.
    ///
    /// The check that stops a channel from being used to mint or burn value.
    #[error("channel {channel}: closure totals {actual}, capacity is {expected}")]
    ChannelCapacityMismatch {
        /// Hex-encoded channel id.
        channel: String,
        /// Capacity escrowed on chain.
        expected: u64,
        /// What the closure distributes.
        actual: u64,
    },

    /// A submitted state did not supersede the one already on chain.
    #[error("channel {channel}: state {proposed} does not supersede {current}")]
    StaleChannelState {
        /// Hex-encoded channel id.
        channel: String,
        /// Sequence already recorded.
        current: u64,
        /// Sequence submitted.
        proposed: u64,
    },

    /// A non-participant attempted a channel operation.
    #[error("channel {channel}: {address} is not a participant")]
    NotAParticipant {
        /// Hex-encoded channel id.
        channel: String,
        /// Hex-encoded address that tried.
        address: String,
    },

    /// A penalty was claimed by the same party that submitted the state.
    ///
    /// Self-punishment is not a thing: it would let a closer drain the channel
    /// by "catching" its own fraud.
    #[error("channel {channel}: the closing party cannot claim a penalty")]
    PenaltyByCloser {
        /// Hex-encoded channel id.
        channel: String,
    },

    /// A revocation proof did not match the recorded commitment.
    #[error("channel {channel}: revocation proof does not match the submitted state")]
    InvalidRevocationProof {
        /// Hex-encoded channel id.
        channel: String,
    },

    /// A dispute was finalized before its window elapsed.
    #[error("channel {channel}: dispute window runs until height {deadline}, now {height}")]
    DisputeWindowOpen {
        /// Hex-encoded channel id.
        channel: String,
        /// Height the window closes.
        deadline: u64,
        /// Current height.
        height: u64,
    },

    /// A payload transaction also carried transfer outputs.
    ///
    /// Mixing them would make the value flow ambiguous, so they are disjoint.
    #[error("a {0} transaction must not carry transfer outputs")]
    MixedTransactionKind(&'static str),

    /// A contract already exists at the derived address.
    #[error("contract {0} already exists")]
    ContractExists(String),

    /// A call referenced an address with no deployed code.
    #[error("unknown contract {0}")]
    UnknownContract(String),

    /// The virtual machine refused or failed the execution.
    #[error("vm: {0}")]
    Vm(String),

    /// A shielded proof did not hold against its public inputs.
    #[error("shielded proof rejected: {0}")]
    ProofVerification(String),

    /// A joinsplit named a commitment tree root the pool does not recognise.
    ///
    /// Either the anchor is fabricated, or it aged out of the window while the
    /// transaction sat in a mempool.
    #[error("unknown or expired shielded anchor {anchor}")]
    UnknownAnchor {
        /// Hex of the offending root.
        anchor: String,
    },

    /// A nullifier was already spent, so this is a double-spend.
    #[error("shielded note already spent: nullifier {nullifier}")]
    NullifierSpent {
        /// Hex of the nullifier.
        nullifier: String,
    },

    /// One joinsplit published the same nullifier twice.
    #[error("joinsplit spends the same note twice: nullifier {nullifier}")]
    DuplicateNullifier {
        /// Hex of the repeated nullifier.
        nullifier: String,
    },

    /// The commitment tree reached capacity.
    #[error("the shielded pool is full")]
    ShieldedPoolFull,

    /// More value left the shielded pool than it held.
    ///
    /// Unreachable if the proof system is sound — the circuit enforces value
    /// conservation — so this firing means a soundness break, not a bookkeeping
    /// slip. It is checked precisely because a shielded pool cannot be audited
    /// from outside.
    #[error("shielded pool underflow: holds {held}, {withdrawn} withdrawn")]
    ShieldedBalanceUnderflow {
        /// Value the pool held.
        held: u64,
        /// Value the withdrawal attempted to remove.
        withdrawn: u64,
    },

    /// A block carried more joinsplits than verification budget allows.
    #[error("block has {actual} joinsplits, the limit is {limit}")]
    TooManyShielded {
        /// Joinsplits in the block.
        actual: usize,
        /// Maximum allowed.
        limit: usize,
    },

    /// The shielded setup is untrusted and the network claims to hold value.
    #[error(
        "the shielded pool uses a reproducible test setup, which cannot secure \
         real value on network '{network}'"
    )]
    UntrustedShieldedSetup {
        /// Network the node was asked to serve.
        network: String,
    },

    /// The persisted libp2p identity could not be read, written, or decoded.
    ///
    /// Separate from [`NodeError::Storage`] because the consequence is
    /// different: a node that loses its key does not lose data, it loses the
    /// `PeerId` every bootnode address in the fleet points at.
    #[error("node identity at {path}: {reason}")]
    Identity {
        /// Key file involved.
        path: String,
        /// What went wrong.
        reason: String,
    },

    /// A lattice HTLC lock the sender could not make: nothing escrowed, an
    /// expiry already reached, or HTLC-L not active at this height.
    ///
    /// Only locks raise this. A claim or refund that loses is a no-op, never
    /// an error — invariant 7.
    #[error("htlc: {0}")]
    Htlc(String),

    /// An attack attestation whose evidence does not verify, or one
    /// submitted before threat intel is active.
    ///
    /// Evidence already on chain raises nothing — it is a no-op.
    #[error("threat intel: {0}")]
    ThreatIntel(String),

    /// An IoT anchor transaction refused: a proof or signature that does not
    /// verify, a second enrollment, a revocation by a non-owner, or any of them
    /// before activation. A relayed batch that merely loses raises nothing.
    #[error("iot anchor: {0}")]
    Iot(String),

    /// An asset ticker was empty, contained something other than uppercase
    /// ASCII and digits, or carried interior padding.
    ///
    /// Restricted at the boundary rather than left to the wallet that renders
    /// it: a control character or a look-alike glyph in a ticker is a phishing
    /// tool, and there is no later point at which rejecting it is cheaper.
    #[error("invalid asset symbol: {reason}")]
    InvalidAssetSymbol {
        /// What was wrong with it.
        reason: String,
    },

    /// A transaction named an asset that has never been registered.
    #[error("unknown asset {0}")]
    UnknownAsset(String),

    /// A registration landed on an identifier that already exists.
    ///
    /// Requires the same sender, nonce, and symbol, so it is unreachable while
    /// nonces are enforced. It is checked anyway, because the consequence of
    /// missing it is one asset's supply silently replacing another's.
    #[error("asset {0} already exists")]
    AssetExists(String),

    /// An account held less of an asset than a transaction tried to move.
    ///
    /// Separate from [`NodeError::InsufficientBalance`], which is about the
    /// native coin: the two live in different keyspaces, and one error for both
    /// would report an address as short of a balance it does not have.
    #[error("account {address} holds {available} of asset {asset}, needs {required}")]
    InsufficientAssetBalance {
        /// Asset involved.
        asset: String,
        /// Account involved.
        address: String,
        /// Units needed.
        required: u64,
        /// Units held.
        available: u64,
    },

    /// A pool was asked for that trades an asset against itself.
    #[error("cannot pair asset {asset} with itself")]
    DegeneratePair {
        /// The asset named twice.
        asset: String,
    },

    /// A pool creation named a pair that already has a pool at that fee rate.
    #[error("pool {0} already exists")]
    PoolExists(String),

    /// A transaction named a pool that does not exist.
    #[error("unknown pool {0}")]
    UnknownPool(String),

    /// The trading engine refused an operation.
    ///
    /// Carries the engine's own message rather than a re-classification of it.
    /// [`maya_dex`] returns a closed set of arithmetic and rule failures and
    /// deliberately does not know about `NodeError`; mapping each of its
    /// variants onto a node variant would be a second taxonomy to keep in step
    /// with the first.
    ///
    /// [`maya_dex`]: https://docs.rs/maya-dex
    #[error("{operation} refused: {reason}")]
    Trade {
        /// Operation that was attempted.
        operation: &'static str,
        /// The engine's account of why it could not be done.
        reason: String,
    },

    /// An operation returned less than the sender said they would accept.
    ///
    /// Only raised for operations that execute in place — deposits,
    /// withdrawals, and routes. A plain swap that misses its bound is skipped
    /// rather than raised, because a swap settles in a batch alongside other
    /// people's, and an error there would let one trader's bound invalidate
    /// every block that carried their transaction.
    #[error("{what}: expected at least {expected}, got {actual}")]
    SlippageExceeded {
        /// Which quantity fell short.
        what: &'static str,
        /// The sender's floor.
        expected: u64,
        /// What the operation would actually have produced.
        actual: u64,
    },

    /// A transaction named an order that is not resting.
    #[error("unknown order {0}")]
    UnknownOrder(String),

    /// Someone other than an order's owner tried to cancel it.
    #[error("order {order} belongs to {owner}, not {claimant}")]
    NotOrderOwner {
        /// Order involved.
        order: String,
        /// Who placed it.
        owner: String,
        /// Who tried to cancel it.
        claimant: String,
    },

    /// A transaction's deadline had already passed.
    ///
    /// The defence against a held transaction. A miner who sits on a swap
    /// cannot make it execute against a market that has moved on since the
    /// sender priced it.
    #[error("deadline {deadline} passed at height {height}")]
    DeadlineExpired {
        /// Last height the sender was willing to execute at.
        deadline: u64,
        /// Height the block is being applied at.
        height: u64,
    },

    /// More swaps were staged against one pool in one block than its batch may
    /// clear.
    #[error("pool {pair} already holds {limit} swaps for this block")]
    TooManySwaps {
        /// Pool involved.
        pair: String,
        /// The ceiling.
        limit: usize,
    },

    /// A route's legs do not join up.
    ///
    /// Leg `n`'s output asset must be leg `n+1`'s input. A route that does not
    /// join up is not a path, and executing it would leave the sender holding
    /// an asset they never asked for.
    #[error("route leg {leg} does not continue from the previous leg")]
    RouteDiscontinuity {
        /// Index of the leg that does not join.
        leg: usize,
    },

    /// The native coin was moved through the asset transfer path.
    ///
    /// It has its own: a transaction's outputs. Two spending paths over one
    /// balance is one more than can be reasoned about.
    #[error("the native coin moves through transaction outputs, not asset transfers")]
    NativeAssetTransfer,

    /// An authority set was too small, too large, held a duplicate, or carried
    /// a threshold that is not a strict majority.
    ///
    /// A quorum below a majority admits two disjoint quorums, and therefore two
    /// contradictory values for one round, each of them perfectly valid.
    #[error("invalid oracle registry: {reason}")]
    InvalidOracleRegistry {
        /// What was wrong with it.
        reason: String,
    },

    /// A feed name was empty, contained something other than uppercase ASCII,
    /// digits and `/`, or carried interior padding.
    #[error("invalid feed name: {reason}")]
    InvalidFeedName {
        /// What was wrong with it.
        reason: String,
    },

    /// A transaction named a feed that has never been created.
    #[error("unknown price feed {0}")]
    UnknownFeed(String),

    /// A feed submission carried fewer valid signatures than the registry
    /// requires.
    #[error("feed {feed}: {supplied} valid signatures, quorum is {required}")]
    QuorumNotMet {
        /// Feed involved.
        feed: String,
        /// Distinct authorities that signed correctly.
        supplied: usize,
        /// How many were needed.
        required: usize,
    },

    /// A signature in a feed submission was not from a registered authority.
    ///
    /// Refused rather than ignored. Ignoring it would let a submitter pad a
    /// quorum with signatures that look valid to a casual reader of the
    /// transaction, which is the shape most oracle post-mortems take.
    #[error("{address} is not an oracle authority")]
    NotAnAuthority {
        /// The address that signed.
        address: String,
    },

    /// One authority signed the same submission twice.
    ///
    /// A quorum is a count of *distinct* authorities. Without this check, one
    /// key repeated `quorum` times is a quorum.
    #[error("authority {address} signed feed {feed} more than once")]
    DuplicateObservation {
        /// The repeated authority.
        address: String,
        /// Feed involved.
        feed: String,
    },

    /// A feed submission named a round at or below the one already stored.
    ///
    /// The replay guard. Without it, a quorum's signatures over an old price
    /// stay valid forever and can be resubmitted whenever that price suits
    /// somebody.
    #[error("feed {feed}: round {supplied} is not newer than {current}")]
    StaleFeedRound {
        /// Feed involved.
        feed: String,
        /// Round that was submitted.
        supplied: u64,
        /// Round already recorded.
        current: u64,
    },

    /// A beacon proof was submitted by an authority that was not this height's
    /// proposer, or for the wrong height.
    #[error("beacon proof at height {height}: {reason}")]
    InvalidBeaconProof {
        /// Height involved.
        height: u64,
        /// Why the proof was refused.
        reason: String,
    },

    /// More than one beacon proof was staged in a single block.
    ///
    /// Exactly one authority is entitled to propose at each height, so a second
    /// proof is either a duplicate or an attempt to give the miner a choice of
    /// accumulator values. See [`crate::oracle::beacon`].
    #[error("a second beacon proof was submitted in one block")]
    DuplicateBeaconProof,

    /// The governance rules refused an operation.
    ///
    /// Carries [`maya_governance`]'s own message rather than a
    /// re-classification of it. That crate returns a closed set of lifecycle
    /// and bounds failures and deliberately does not know about `NodeError`;
    /// mapping each variant onto a node variant would be a second taxonomy to
    /// keep in step with the first.
    ///
    /// [`maya_governance`]: https://docs.rs/maya-governance
    #[error("governance refused: {reason}")]
    Governance {
        /// What the rules said.
        reason: String,
    },

    /// A transaction named a proposal that does not exist.
    #[error("unknown proposal {0}")]
    UnknownProposal(String),

    /// Somebody other than a proposal's author tried to withdraw it.
    #[error("proposal {proposal} belongs to {owner}, not {claimant}")]
    NotProposer {
        /// Proposal involved.
        proposal: String,
        /// Who opened it.
        owner: String,
        /// Who tried to withdraw it.
        claimant: String,
    },

    /// A withdrawal was attempted before the stake's lock elapsed.
    ///
    /// The commitment a vote rests on. Releasing early would let a voter
    /// shorten their own exposure after the fact.
    #[error("stake unlocks at height {unlock_height}; this is {height}")]
    StakeLocked {
        /// Earliest height it may be withdrawn.
        unlock_height: u64,
        /// Height the withdrawal was attempted at.
        height: u64,
    },

    /// More than one work claim was staged in a single block.
    ///
    /// A block represents one unit of work, so it credits one beneficiary.
    /// Admitting a second would let a miner mint voting weight out of a single
    /// proof of work.
    #[error("a second work claim was submitted in one block")]
    DuplicateWorkClaim,

    /// A state proof was structurally invalid.
    ///
    /// Distinct from a proof that simply does not verify: this one could not
    /// be evaluated at all, so "it did not match" would be the wrong thing to
    /// report about it.
    #[error("malformed state proof: {reason}")]
    MalformedProof {
        /// What was wrong with it.
        reason: String,
    },

    /// The encryption committee is missing, malformed, or unusable.
    ///
    /// Separate from the per-envelope failures below because it is a chain
    /// configuration problem rather than a transaction problem: no sender can
    /// fix it, and every sealed transaction fails identically until it is.
    #[error("invalid sealed-mempool committee: {reason}")]
    SealedCommittee {
        /// What was wrong with it.
        reason: String,
    },

    /// A sealed transaction arrived on a chain with no committee installed.
    ///
    /// The committee is set at genesis. A chain without one has no sealed
    /// mempool at all, and the honest answer to a sealed transaction there is
    /// that the feature is absent rather than that the sender did something
    /// wrong.
    #[error("this chain has no sealed-mempool committee")]
    SealedCommitteeUnset,

    /// An envelope named a reveal height outside the permitted window.
    ///
    /// The lower bound is what the whole scheme rests on: an envelope opened in
    /// the block that included it would have been readable by the miner who
    /// ordered it. The upper bound is what keeps the pending queue finite.
    #[error("reveal height {reveal_height} is not within {min}..={max} blocks of height {height}")]
    SealedRevealWindow {
        /// Height the envelope asked to be opened at.
        reveal_height: u64,
        /// Height the block is being applied at.
        height: u64,
        /// Earliest permitted delay.
        min: u64,
        /// Latest permitted delay.
        max: u64,
    },

    /// Two envelopes from one sender at one nonce.
    ///
    /// Unreachable through the transaction path, where the nonce is consumed —
    /// but the identifier is a hash of sender and nonce, and a collision here
    /// would silently replace a pending envelope rather than fail.
    #[error("envelope {0} already exists")]
    SealedEnvelopeExists(String),

    /// A decryption share named an envelope that is not pending.
    ///
    /// Either it never existed, or it has already been opened or expired. All
    /// three are the same fact from the submitter's side: there is nothing here
    /// to open.
    #[error("no envelope {0} is awaiting reveal")]
    UnknownSealedEnvelope(String),

    /// A decryption share was refused.
    ///
    /// Covers a share from a non-member, a share whose Chaum–Pedersen proof
    /// does not verify, and a share submitted outside its envelope's reveal
    /// block. The first two are evidence against a committee member; the third
    /// is the rule that stops a plaintext appearing while blocks are still
    /// being built against the ciphertext.
    #[error("decryption share from member {member} refused: {reason}")]
    InvalidDecryptionShare {
        /// Committee member that submitted it.
        member: u16,
        /// Why it was refused.
        reason: String,
    },

    /// One member submitted two shares for one envelope.
    ///
    /// Interpolation divides by the difference of two member indices, so a
    /// duplicate is a division by zero before it is a member voting twice.
    #[error("member {member} has already submitted a share for envelope {envelope}")]
    DuplicateDecryptionShare {
        /// Committee member that submitted it.
        member: u16,
        /// Hex-encoded envelope.
        envelope: String,
    },

    /// A block carried more sealed-mempool work than its ceiling allows.
    #[error("sealed mempool: {reason}")]
    SealedBlockLimit {
        /// Which ceiling was hit.
        reason: String,
    },

    /// A byte slice had the wrong length to decode into a fixed-size array.
    #[error("invalid {what} length: expected {expected} bytes, got {actual}")]
    InvalidLength {
        /// Name of the field being decoded.
        what: &'static str,
        /// Required length.
        expected: usize,
        /// Length actually supplied.
        actual: usize,
    },
}

/// Convenience alias used throughout the crate.
///
/// Leading `::` disambiguates the `core` crate from this crate's `core` module.
pub type Result<T> = ::core::result::Result<T, NodeError>;
