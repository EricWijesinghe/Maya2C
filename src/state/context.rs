//! Execution context for a block.

/// Chain facts a transaction may depend on while executing.
///
/// A newtype rather than a bare `u64` parameter: `apply_block(&block, 7)` next
/// to a nonce or an amount is easy to get wrong and compiles silently either
/// way. Timelocks are the reason this exists at all — an HTLC expiry or a
/// dispute deadline is meaningless without knowing which block is executing.
///
/// Height rather than timestamp: block timestamps are miner-influenced within
/// the consensus tolerance, so a timestamp-based deadline is a deadline an
/// adversary can nudge. Height cannot be forged without doing the work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockContext {
    /// Height of the block being executed.
    pub height: u64,
    /// First height at which `host_verify_zkml_proof` answers.
    ///
    /// [`ZKML_ACTIVATION_HEIGHT`] everywhere the node builds a context, which is
    /// never. Tests set it explicitly with [`Self::with_zkml_activation`].
    pub zkml_activation: u64,
    /// First height at which lattice HTLC transactions execute.
    ///
    /// [`HTLC_L_ACTIVATION_HEIGHT`] everywhere the node builds a context. Tests
    /// set it with [`Self::with_htlc_activation`].
    pub htlc_activation: u64,
    /// First height whose block commits accounts as a sparse tree.
    ///
    /// [`STATELESS_ACTIVATION_HEIGHT`] everywhere the node builds a context.
    /// Tests set it with [`Self::with_stateless_activation`].
    pub stateless_activation: u64,
    /// First height at which attack attestations execute.
    ///
    /// [`THREAT_INTEL_ACTIVATION_HEIGHT`] everywhere the node builds a context.
    /// Tests set it with [`Self::with_threat_intel_activation`].
    pub threat_intel_activation: u64,
    /// First height at which IoT anchor transactions execute.
    ///
    /// [`IOT_ACTIVATION_HEIGHT`] everywhere the node builds a context. Tests set
    /// it with [`Self::with_iot_activation`].
    pub iot_activation: u64,
}

/// First height at which IoT anchor transactions execute: none.
///
/// The research-branch pattern. What has to be decided first is written in
/// `docs/iot-anchor.md`: device keys are ML-DSA-65 alone rather than the
/// hybrid, enrollment proves key possession rather than genuine hardware, and
/// signing time on real microcontrollers is unmeasured.
pub const IOT_ACTIVATION_HEIGHT: u64 = u64::MAX;

/// First height at which attack attestations execute: none.
///
/// The research-branch pattern. What has to be decided first is written in
/// `docs/threat-intel.md`: the evidence signature is classical ed25519, so a
/// quantum adversary can frame any peer id, and evidence markers are never
/// pruned, so their growth needs a bound before any network carries them.
pub const THREAT_INTEL_ACTIVATION_HEIGHT: u64 = u64::MAX;

/// First height at which the accounts root becomes a sparse Merkle tree, so a
/// stateless node can verify transfer blocks from witnesses: none.
///
/// The research-branch pattern. Activation is a state-root format change —
/// every root from that block on is computed differently — so what has to be
/// decided first is written in `docs/stateless.md`: how witnesses travel (no
/// gossip topic carries them yet), and whether the end-of-block passes can be
/// made witness-verifiable, without which only chains that never run the
/// oracle, governance or the sealed mempool can be followed statelessly.
pub const STATELESS_ACTIVATION_HEIGHT: u64 = u64::MAX;

/// First height at which lattice HTLC locks, claims and refunds execute: none.
///
/// The research-branch pattern, as for zkML. What has to be decided first is
/// written in `docs/htlc-lattice.md`: the claim's verification cost measured
/// against a full block of claims, and whether a counterparty chain running the
/// same verifier exists to swap with — without one, a lock has nothing to be
/// atomic with.
pub const HTLC_L_ACTIVATION_HEIGHT: u64 = u64::MAX;

/// First height at which contracts can verify zkML proofs: none.
///
/// The research-branch pattern (`crypto/dag/registry.rs`, `lattice-pow`): the
/// code ships and is tested, and no network runs it until somebody chooses a
/// height. Two things have to be true first, and neither is. The SRS must be a
/// real ceremony rather than the public-seed one in `maya_zkml::srs`, and a
/// call's `gas_limit` must be capped — today nothing bounds it, so a price in
/// fuel bounds a verification only relative to a limit the sender picks. See
/// `docs/zkml.md`.
pub const ZKML_ACTIVATION_HEIGHT: u64 = u64::MAX;

impl BlockContext {
    /// Context for a block at `height`.
    #[must_use]
    pub const fn at_height(height: u64) -> Self {
        Self {
            height,
            zkml_activation: ZKML_ACTIVATION_HEIGHT,
            htlc_activation: HTLC_L_ACTIVATION_HEIGHT,
            stateless_activation: STATELESS_ACTIVATION_HEIGHT,
            threat_intel_activation: THREAT_INTEL_ACTIVATION_HEIGHT,
            iot_activation: IOT_ACTIVATION_HEIGHT,
        }
    }

    /// The same context with attack attestations active from `height` on.
    #[must_use]
    pub const fn with_threat_intel_activation(self, height: u64) -> Self {
        Self {
            threat_intel_activation: height,
            ..self
        }
    }

    /// Whether attack attestations execute in this block.
    #[must_use]
    pub const fn threat_intel_active(self) -> bool {
        self.height >= self.threat_intel_activation
    }

    /// The same context with IoT anchor transactions active from `height` on.
    #[must_use]
    pub const fn with_iot_activation(self, height: u64) -> Self {
        Self {
            iot_activation: height,
            ..self
        }
    }

    /// Whether IoT anchor transactions execute in this block.
    #[must_use]
    pub const fn iot_active(self) -> bool {
        self.height >= self.iot_activation
    }

    /// The same context with sparse accounts active from `height` on.
    #[must_use]
    pub const fn with_stateless_activation(self, height: u64) -> Self {
        Self {
            stateless_activation: height,
            ..self
        }
    }

    /// Whether this block switches, or has switched, accounts to the sparse
    /// tree.
    #[must_use]
    pub const fn stateless_active(self) -> bool {
        self.height >= self.stateless_activation
    }

    /// The same context with lattice HTLCs active from `height` on.
    #[must_use]
    pub const fn with_htlc_activation(self, height: u64) -> Self {
        Self {
            htlc_activation: height,
            ..self
        }
    }

    /// Whether lattice HTLC transactions execute in this block.
    #[must_use]
    pub const fn htlc_active(self) -> bool {
        self.height >= self.htlc_activation
    }

    /// The same context with zkML verification active from `height` on.
    #[must_use]
    pub const fn with_zkml_activation(self, height: u64) -> Self {
        Self {
            zkml_activation: height,
            ..self
        }
    }

    /// Whether zkML verification answers in this block.
    #[must_use]
    pub const fn zkml_active(self) -> bool {
        self.height >= self.zkml_activation
    }

    /// Genesis-era context.
    ///
    /// Correct for plain transfers, which do not consult the height at all.
    /// Anything with a timelock must use the real height.
    pub const GENESIS: Self = Self::at_height(0);
}

impl Default for BlockContext {
    fn default() -> Self {
        Self::GENESIS
    }
}
