//! Post-quantum confidentiality for the P2P transport.
//!
//! ```text
//! TCP / Memory
//!   └── libp2p-noise   Noise_XX_25519_ChaChaPoly_SHA256   ← unchanged
//!         └── /maya/mlkem1024/1.0.0  (preferred, NIST level 5)
//!             /maya/mlkem/1.0.0      (accepted, NIST level 3)  ← this module
//!               ML-KEM-1024 or ML-KEM-768 (FIPS 203) + ChaCha20-Poly1305
//!                 └── yamux → gossipsub
//! ```
//!
//! ## Why a layer and not a replacement
//!
//! The obvious implementation would be a post-quantum Noise pattern —
//! `Noise_XXhfs_25519+MLKEM768_ChaChaPoly_SHA256`. It is not reachable from
//! here. `libp2p-noise` hard-codes its parameters in a private constant behind
//! a private resolver, and `Config` exposes no way to change the pattern or the
//! key exchange. One layer down, `snow` does expose a `Kem` trait under its
//! `hfs` feature — but `snow::params::KemChoice` is a closed enum with exactly
//! one variant, `Kyber1024`, so a pattern string naming ML-KEM-768 does not
//! parse. An ML-KEM implementation could be registered under the name
//! `Kyber1024`, and that was rejected: a handshake whose advertised protocol
//! name misstates its primitive is a trap for whoever audits it next.
//!
//! The remaining options were to fork two security-critical crates, or to write
//! a full authenticated key exchange from scratch. This layer is the third: it
//! leaves the audited Noise handshake exactly as it is and adds a small,
//! self-contained post-quantum stage above it.
//!
//! ## What this protects against
//!
//! **Harvest now, decrypt later.** An adversary recording gossip traffic today
//! and waiting for a quantum computer must break ML-KEM *and* X25519 to read
//! any of it. This is the threat that actually applies to a chain: gossip
//! reveals which peer originated which transaction, and that metadata does not
//! become less interesting with age.
//!
//! ## What it does not
//!
//! **Authentication stays classical.** The peer is authenticated by the Noise
//! layer below, against its ed25519 `PeerId`, and nothing here changes that. An
//! adversary holding a cryptographically relevant quantum computer *at the
//! moment a connection is made* could still impersonate a peer. That is an
//! active attack requiring the machine to exist and be on the wire — a
//! different and much harder proposition than recording packets — but it is a
//! real limit and it is not closed here. Closing it means post-quantum peer
//! identity, which changes what a `PeerId` is and is separate work.
//!
//! ## Which parameter set, and the downgrade
//!
//! Level 5 (ML-KEM-1024) is offered first, level 3 (ML-KEM-768) second, so a
//! peer that speaks both picks 1024. The 768 entry stays because a network
//! cannot move to a new protocol in one step: a node that offered only 1024
//! would partition from every node not yet upgraded. [`MlKem1024Policy::Required`]
//! removes the fallback once the network has upgraded.
//!
//! The downgrade is real, and it is stated here rather than left to be
//! discovered. Negotiation runs inside the Noise session, so the offered list
//! is encrypted and a passive observer cannot see or edit it. Forcing a
//! session to 768 therefore needs an *active* attacker who can break or
//! impersonate the Noise session while it is live. A recording adversary
//! cannot. The same reasoning covers the dual-KEM protocol below it.

pub mod dual;
pub mod handshake;
pub mod measure;
pub mod rotation;
pub mod stream;

use futures::future::BoxFuture;
use futures::{AsyncRead, AsyncWrite, FutureExt};
use libp2p::core::UpgradeInfo;
use libp2p::core::upgrade::{InboundConnectionUpgrade, OutboundConnectionUpgrade};
use libp2p::swarm::StreamProtocol;
use maya_crypto_pq::kem_suite::MlKem1024;

use crate::network::pq::dual::DualKemPolicy;
use crate::network::pq::handshake::SessionKeys;

pub use rotation::{EpochClock, ROTATION_INTERVAL_BLOCKS};
pub use stream::PqStream;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::NodeError;

/// Running totals for the post-quantum transport.
///
/// Counters rather than a `Metrics` handle threaded into the swarm task. The
/// node's metrics are *sampled on a timer* by the binary that owns the
/// exporter — see [`crate::metrics`] — deliberately, to keep instrumentation
/// out of hot paths and off the single task that owns the swarm. These follow
/// that pattern: the driver increments, the exporter reads.
///
/// Cheap to clone; all clones observe the same totals.
#[derive(Clone, Debug, Default)]
pub struct SessionStats {
    established: Arc<AtomicU64>,
    rotated: Arc<AtomicU64>,
    by_protocol: Arc<[AtomicU64; PqProtocol::COUNT]>,
}

impl SessionStats {
    /// Zeroed counters.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Notes a completed handshake.
    pub fn record_established(&self) {
        self.established.fetch_add(1, Ordering::Relaxed);
    }

    /// Notes a session closed because its keys aged out.
    pub fn record_rotated(&self) {
        self.rotated.fetch_add(1, Ordering::Relaxed);
    }

    /// Notes which exchange a completed handshake ran.
    ///
    /// Recorded by the upgrade itself, because libp2p does not report the
    /// upgrade protocol on the connection events the swarm sees.
    pub fn record_protocol(&self, protocol: PqProtocol) {
        self.by_protocol[protocol as usize].fetch_add(1, Ordering::Relaxed);
    }

    /// Sessions established since the node started.
    #[must_use]
    pub fn established(&self) -> u64 {
        self.established.load(Ordering::Relaxed)
    }

    /// Sessions closed for rotation since the node started.
    #[must_use]
    pub fn rotated(&self) -> u64 {
        self.rotated.load(Ordering::Relaxed)
    }

    /// Handshakes that ran `protocol` since the node started.
    #[must_use]
    pub fn negotiated(&self, protocol: PqProtocol) -> u64 {
        self.by_protocol[protocol as usize].load(Ordering::Relaxed)
    }
}

/// Which key exchange a session ran.
///
/// Exists so an operator can see the share of sessions on level 5 rather than
/// infer it from configuration. Discriminants index [`SessionStats`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum PqProtocol {
    /// ML-KEM-768 over `/maya/mlkem/1.0.0`, NIST level 3.
    MlKem768 = 0,
    /// ML-KEM-1024 over `/maya/mlkem1024/1.0.0`, NIST level 5.
    MlKem1024 = 1,
    /// The dual-KEM combiner over `/maya/dualkem/1.0.0`.
    DualKem = 2,
}

impl PqProtocol {
    /// Number of variants, and so of [`SessionStats`] counters.
    pub const COUNT: usize = 3;

    /// Every variant, in index order.
    pub const ALL: [Self; Self::COUNT] = [Self::MlKem768, Self::MlKem1024, Self::DualKem];

    /// The metric label for this exchange.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::MlKem768 => "mlkem768",
            Self::MlKem1024 => "mlkem1024",
            Self::DualKem => "dualkem",
        }
    }

    /// The exchange a negotiated protocol name selects.
    ///
    /// # Errors
    ///
    /// [`NodeError::PqHandshake`] for a name this build does not know. Mapping
    /// it to ML-KEM-768 would turn an unknown name into a silent downgrade, so
    /// the upgrade fails instead.
    pub fn for_protocol_name(name: &str) -> Result<Self, NodeError> {
        match name {
            PROTOCOL_1024 => Ok(Self::MlKem1024),
            PROTOCOL => Ok(Self::MlKem768),
            dual::PROTOCOL => Ok(Self::DualKem),
            other => Err(NodeError::PqHandshake(format!(
                "peer negotiated an unknown protocol: {other}"
            ))),
        }
    }
}

/// Protocol name for ML-KEM-768, the single-KEM exchange.
///
/// Versioned, because this is the negotiation point for any future change to
/// the exchange. A change of primitive gets a new name rather than a new
/// version of this one, so peers that speak only the old name still connect.
pub const PROTOCOL: &str = "/maya/mlkem/1.0.0";

/// Protocol name for ML-KEM-1024, NIST level 5.
///
/// A different name, not a version bump of [`PROTOCOL`]. The name is what makes
/// the two negotiable. A version bump would make 768 unspeakable and force a
/// flag day on a network that cannot coordinate one.
pub const PROTOCOL_1024: &str = "/maya/mlkem1024/1.0.0";

/// Which ML-KEM parameter sets a node offers.
///
/// # Why the default prefers 1024 and still offers 768
///
/// Level 5 is the security target (ADR-051). 768 stays in the offer so that a
/// network can move one node at a time: until every peer runs a build that
/// knows 1024, dropping 768 would split the network. The cost is the downgrade
/// described in the module docs.
///
/// # What has to be true before the default flips to `Required`
///
/// The same discipline as [`DualKemPolicy`]: the operator decides this once
/// every peer they depend on offers 1024, not when a release is cut.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MlKem1024Policy {
    /// Offer ML-KEM-1024 first, ML-KEM-768 second.
    #[default]
    Preferred,
    /// Offer ML-KEM-768 only. An operator rollback. Peers that require 1024
    /// will refuse this node.
    Off,
    /// Offer ML-KEM-1024 only. No downgrade is possible, and a peer that
    /// speaks only 768 cannot connect.
    Required,
}

impl core::str::FromStr for MlKem1024Policy {
    type Err = String;

    /// Parses the operator-facing names `preferred`, `off` and `required`.
    /// Case is ignored, as for `--dual-kem`.
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "preferred" => Ok(Self::Preferred),
            "off" => Ok(Self::Off),
            "required" => Ok(Self::Required),
            other => Err(format!(
                "unknown ML-KEM-1024 policy '{other}': expected preferred, off, or required"
            )),
        }
    }
}

impl core::fmt::Display for MlKem1024Policy {
    /// The operator-facing name, so a log line round-trips through `from_str`.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::Preferred => "preferred",
            Self::Off => "off",
            Self::Required => "required",
        })
    }
}

impl MlKem1024Policy {
    /// Whether `/maya/mlkem1024/1.0.0` is advertised.
    #[must_use]
    pub const fn offers_1024(self) -> bool {
        !matches!(self, Self::Off)
    }

    /// Whether `/maya/mlkem/1.0.0` is advertised.
    #[must_use]
    pub const fn offers_768(self) -> bool {
        !matches!(self, Self::Required)
    }
}

/// The connection upgrade applying post-quantum key encapsulation above the
/// Noise session.
///
/// Trivially cloneable, which libp2p requires: the same upgrade value is used
/// for every connection, and each one generates its own ephemeral keypair
/// inside the handshake.
///
/// # Which protocols it offers
///
/// Order is the preference: libp2p's multistream-select takes the dialer's
/// first mutually-supported entry, so listing the stronger protocol first is
/// what makes it preferred rather than merely available.
///
/// | [`DualKemPolicy`] | [`MlKem1024Policy`] | Offered, in preference order |
/// |---|---|---|
/// | `Disabled` | `Preferred` (default) | `/maya/mlkem1024/1.0.0`, `/maya/mlkem/1.0.0` |
/// | `Disabled` | `Off` | `/maya/mlkem/1.0.0` |
/// | `Disabled` | `Required` | `/maya/mlkem1024/1.0.0` |
/// | `Offered` | `Preferred` | `/maya/dualkem/1.0.0`, `/maya/mlkem1024/1.0.0`, `/maya/mlkem/1.0.0` |
/// | `Required` | any | `/maya/dualkem/1.0.0` |
#[derive(Clone, Debug, Default)]
pub struct PqUpgrade {
    policy: DualKemPolicy,
    mlkem1024: MlKem1024Policy,
    stats: SessionStats,
}

impl PqUpgrade {
    /// Creates the upgrade with dual-KEM disabled and ML-KEM-1024 preferred.
    ///
    /// The default, and what every existing caller gets.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates the upgrade under an explicit dual-KEM policy.
    #[must_use]
    pub fn with_policy(policy: DualKemPolicy) -> Self {
        Self {
            policy,
            ..Self::default()
        }
    }

    /// Sets the ML-KEM-1024 policy.
    #[must_use]
    pub fn with_mlkem1024(self, mlkem1024: MlKem1024Policy) -> Self {
        Self { mlkem1024, ..self }
    }

    /// Records negotiated exchanges into `stats`.
    #[must_use]
    pub fn with_stats(self, stats: SessionStats) -> Self {
        Self { stats, ..self }
    }

    /// The dual-KEM policy this upgrade was built with.
    #[must_use]
    pub const fn policy(&self) -> DualKemPolicy {
        self.policy
    }

    /// The ML-KEM-1024 policy this upgrade was built with.
    #[must_use]
    pub const fn mlkem1024_policy(&self) -> MlKem1024Policy {
        self.mlkem1024
    }
}

impl UpgradeInfo for PqUpgrade {
    type Info = StreamProtocol;
    type InfoIter = std::vec::IntoIter<Self::Info>;

    /// # Why this may offer several protocols
    ///
    /// It used to offer exactly one, and the comment here said that offering a
    /// second would be offering a downgrade. That is still true of the
    /// fallback, and it is still why the 768 entry is the one that
    /// [`MlKem1024Policy::Required`] removes.
    ///
    /// The dual-KEM protocol is *stronger*, not weaker, and it depends on a
    /// release candidate tracking a draft standard. Requiring it outright would
    /// make every connection depend on that crate being correct — see `dual`'s
    /// module documentation on confidentiality being an OR while availability
    /// is an AND.
    fn protocol_info(&self) -> Self::InfoIter {
        let mut offered = Vec::with_capacity(3);
        if self.policy.offers() {
            offered.push(StreamProtocol::new(dual::PROTOCOL));
        }
        // Dual-KEM Required is the one setting that suppresses both ML-KEM names.
        if !self.policy.requires() {
            if self.mlkem1024.offers_1024() {
                offered.push(StreamProtocol::new(PROTOCOL_1024));
            }
            if self.mlkem1024.offers_768() {
                offered.push(StreamProtocol::new(PROTOCOL));
            }
        }
        offered.into_iter()
    }
}

impl<C> InboundConnectionUpgrade<C> for PqUpgrade
where
    C: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    type Output = PqStream<C>;
    type Error = NodeError;
    type Future = BoxFuture<'static, Result<Self::Output, Self::Error>>;

    /// The listening side, which generates the keypair(s) and decapsulates.
    ///
    /// Dispatches on the *negotiated* protocol rather than on this node's
    /// policy. The two can differ — a node offering both ML-KEM sets reaches
    /// the 768 branch whenever its peer does not speak 1024 — and trusting the
    /// policy instead of the negotiation result is how a handshake ends up
    /// running the wrong exchange on a stream.
    fn upgrade_inbound(self, mut socket: C, protocol: Self::Info) -> Self::Future {
        async move {
            let kind = PqProtocol::for_protocol_name(protocol.as_ref())?;
            let keys = respond_for(kind, &mut socket).await?;
            self.stats.record_protocol(kind);
            Ok(PqStream::new(socket, keys, false))
        }
        .boxed()
    }
}

impl<C> OutboundConnectionUpgrade<C> for PqUpgrade
where
    C: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    type Output = PqStream<C>;
    type Error = NodeError;
    type Future = BoxFuture<'static, Result<Self::Output, Self::Error>>;

    /// The dialing side, which encapsulates to the key(s) it receives.
    ///
    /// Dispatches on the negotiated protocol, for the reason given on
    /// [`PqUpgrade::upgrade_inbound`].
    fn upgrade_outbound(self, mut socket: C, protocol: Self::Info) -> Self::Future {
        async move {
            let kind = PqProtocol::for_protocol_name(protocol.as_ref())?;
            let keys = initiate_for(kind, &mut socket).await?;
            self.stats.record_protocol(kind);
            Ok(PqStream::new(socket, keys, true))
        }
        .boxed()
    }
}

/// Runs the responder half of the exchange `kind` names.
async fn respond_for<S>(kind: PqProtocol, socket: &mut S) -> Result<SessionKeys, NodeError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    match kind {
        // Boxed: the arms' futures are large (ML-KEM-1024 keys are inline), and
        // an unboxed match would make every connection future that large.
        PqProtocol::MlKem1024 => Box::pin(handshake::respond_with::<MlKem1024, _>(socket)).await,
        PqProtocol::MlKem768 => Box::pin(handshake::respond(socket)).await,
        PqProtocol::DualKem => Box::pin(dual_respond(socket)).await,
    }
}

/// Runs the initiator half of the exchange `kind` names.
async fn initiate_for<S>(kind: PqProtocol, socket: &mut S) -> Result<SessionKeys, NodeError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    match kind {
        PqProtocol::MlKem1024 => Box::pin(handshake::initiate_with::<MlKem1024, _>(socket)).await,
        PqProtocol::MlKem768 => Box::pin(handshake::initiate(socket)).await,
        PqProtocol::DualKem => Box::pin(dual_initiate(socket)).await,
    }
}

#[cfg(feature = "hqc")]
async fn dual_respond<S>(socket: &mut S) -> Result<SessionKeys, NodeError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    dual::respond(socket).await
}

#[cfg(feature = "hqc")]
async fn dual_initiate<S>(socket: &mut S) -> Result<SessionKeys, NodeError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    dual::initiate(socket).await
}

/// A build without HQC never offers the dual protocol, so a peer negotiating
/// it has not done so from this node's offer list. Refused rather than run on
/// some other exchange.
#[cfg(not(feature = "hqc"))]
async fn dual_respond<S>(_socket: &mut S) -> Result<SessionKeys, NodeError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    Err(not_built_in())
}

#[cfg(not(feature = "hqc"))]
async fn dual_initiate<S>(_socket: &mut S) -> Result<SessionKeys, NodeError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    Err(not_built_in())
}

#[cfg(not(feature = "hqc"))]
fn not_built_in() -> NodeError {
    NodeError::PqHandshake(format!("{} is not built into this node", dual::PROTOCOL))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn names(upgrade: &PqUpgrade) -> Vec<String> {
        upgrade
            .protocol_info()
            .map(|p| p.as_ref().to_string())
            .collect()
    }

    #[test]
    fn the_offered_protocols_follow_the_policy() {
        let with = |policy| PqUpgrade::with_policy(policy);

        // Level 5 first, level 3 kept as the fallback.
        assert_eq!(
            names(&with(DualKemPolicy::Disabled)),
            vec![PROTOCOL_1024.to_string(), PROTOCOL.to_string()]
        );

        // Dual first: multistream-select takes the first mutual entry, so
        // order is what makes it preferred rather than merely available.
        assert_eq!(
            names(&with(DualKemPolicy::Offered)),
            vec![
                dual::PROTOCOL.to_string(),
                PROTOCOL_1024.to_string(),
                PROTOCOL.to_string()
            ]
        );

        // Required offers no fallback, so there is nothing to strip.
        assert_eq!(
            names(&with(DualKemPolicy::Required)),
            vec![dual::PROTOCOL.to_string()]
        );
    }

    #[test]
    fn the_mlkem1024_policy_selects_the_ml_kem_entries() {
        let with = |policy| PqUpgrade::new().with_mlkem1024(policy);

        assert_eq!(
            names(&with(MlKem1024Policy::Off)),
            vec![PROTOCOL.to_string()]
        );
        assert_eq!(
            names(&with(MlKem1024Policy::Required)),
            vec![PROTOCOL_1024.to_string()]
        );
    }

    #[test]
    fn a_required_node_and_a_disabled_node_share_no_protocol() {
        // The refusal is structural rather than a check somewhere: the two
        // sets simply do not intersect, so negotiation fails and the
        // connection drops.
        let required = names(&PqUpgrade::with_policy(DualKemPolicy::Required));
        let disabled = names(&PqUpgrade::with_policy(DualKemPolicy::Disabled));

        assert!(
            !required.iter().any(|r| disabled.contains(r)),
            "a required node must not be able to fall back"
        );
    }

    #[test]
    fn a_1024_required_node_refuses_a_768_only_one() {
        let required = names(&PqUpgrade::new().with_mlkem1024(MlKem1024Policy::Required));
        let legacy = names(&PqUpgrade::new().with_mlkem1024(MlKem1024Policy::Off));

        assert!(
            !required.iter().any(|r| legacy.contains(r)),
            "a level-5 node must not fall back to level 3"
        );
    }

    #[test]
    fn an_offered_node_can_talk_to_either_kind() {
        let offered = names(&PqUpgrade::with_policy(DualKemPolicy::Offered));

        for other in [DualKemPolicy::Disabled, DualKemPolicy::Required] {
            let theirs = names(&PqUpgrade::with_policy(other));
            assert!(
                offered.iter().any(|o| theirs.contains(o)),
                "an offered node must find common ground with {other}"
            );
        }
    }

    #[test]
    fn the_protocol_names_are_versioned_and_distinct() {
        // A name without a version is a name that cannot be changed later
        // without a flag day.
        assert!(PROTOCOL.ends_with("/1.0.0"));
        assert!(PROTOCOL_1024.ends_with("/1.0.0"));
        assert_ne!(PROTOCOL, PROTOCOL_1024);
        assert_ne!(PROTOCOL_1024, dual::PROTOCOL);
        assert!(StreamProtocol::try_from_owned(PROTOCOL_1024.to_string()).is_ok());
    }

    #[test]
    fn the_default_upgrade_offers_mlkem1024_then_mlkem768() {
        // Level 5 first. The 768 entry stays so older peers connect.
        assert_eq!(
            names(&PqUpgrade::new()),
            vec![PROTOCOL_1024.to_string(), PROTOCOL.to_string()]
        );
        assert_eq!(
            PqUpgrade::new().mlkem1024_policy(),
            MlKem1024Policy::Preferred
        );
    }

    #[test]
    fn a_negotiated_name_selects_its_exchange() {
        assert_eq!(
            PqProtocol::for_protocol_name(PROTOCOL_1024).unwrap(),
            PqProtocol::MlKem1024
        );
        assert_eq!(
            PqProtocol::for_protocol_name(PROTOCOL).unwrap(),
            PqProtocol::MlKem768
        );
        assert_eq!(
            PqProtocol::for_protocol_name(dual::PROTOCOL).unwrap(),
            PqProtocol::DualKem
        );
    }

    #[test]
    fn an_unknown_name_is_refused_not_downgraded() {
        // Mapping this to ML-KEM-768 would be a silent downgrade.
        let error = PqProtocol::for_protocol_name("/maya/mlkem/9.9.9").expect_err("must refuse");
        assert!(error.to_string().contains("unknown protocol"), "{error}");
    }

    #[test]
    fn every_variant_indexes_its_own_counter() {
        // `record_protocol` and `negotiated` index by discriminant, so ALL must
        // list the variants in discriminant order or counts land in the wrong slot.
        for (index, protocol) in PqProtocol::ALL.iter().enumerate() {
            assert_eq!(*protocol as usize, index);
        }
        assert_eq!(PqProtocol::ALL.len(), PqProtocol::COUNT);
    }

    #[test]
    fn the_mlkem1024_policy_parses_the_operator_names() {
        assert_eq!("preferred".parse::<MlKem1024Policy>(), Ok(MlKem1024Policy::Preferred));
        assert_eq!("OFF".parse::<MlKem1024Policy>(), Ok(MlKem1024Policy::Off));
        assert_eq!("Required".parse::<MlKem1024Policy>(), Ok(MlKem1024Policy::Required));
        assert!("strict".parse::<MlKem1024Policy>().is_err());
        for policy in [MlKem1024Policy::Preferred, MlKem1024Policy::Off, MlKem1024Policy::Required] {
            assert_eq!(policy.to_string().parse::<MlKem1024Policy>(), Ok(policy));
        }
    }

    #[test]
    fn session_stats_count_each_exchange_separately() {
        let stats = SessionStats::new();
        stats.record_protocol(PqProtocol::MlKem1024);
        stats.record_protocol(PqProtocol::MlKem1024);
        stats.record_protocol(PqProtocol::MlKem768);

        assert_eq!(stats.negotiated(PqProtocol::MlKem1024), 2);
        assert_eq!(stats.negotiated(PqProtocol::MlKem768), 1);
        assert_eq!(stats.negotiated(PqProtocol::DualKem), 0);
    }
}
