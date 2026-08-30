//! Post-quantum confidentiality for the P2P transport.
//!
//! ```text
//! TCP / Memory
//!   └── libp2p-noise   Noise_XX_25519_ChaChaPoly_SHA256   ← unchanged
//!         └── /maya/mlkem/1.0.0                           ← this module
//!               ML-KEM-768 (FIPS 203) + ChaCha20-Poly1305
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
//! and waiting for a quantum computer must break ML-KEM-768 *and* X25519 to
//! read any of it. This is the threat that actually applies to a chain: gossip
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
//! ## No downgrade
//!
//! The upgrade is not optional. A peer that does not speak `/maya/mlkem/1.0.0`
//! fails multistream-select negotiation and the connection is dropped. There is
//! deliberately no fallback path: an optional post-quantum layer is one an
//! attacker strips.

pub mod handshake;
pub mod rotation;
pub mod stream;

use std::iter;

use futures::future::BoxFuture;
use futures::{AsyncRead, AsyncWrite, FutureExt};
use libp2p::core::UpgradeInfo;
use libp2p::core::upgrade::{InboundConnectionUpgrade, OutboundConnectionUpgrade};
use libp2p::swarm::StreamProtocol;

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
}

/// Protocol name announced during connection upgrade.
///
/// Versioned, because this is the negotiation point for any future change to
/// the exchange — a second parameter set, or an authenticated variant, becomes
/// `/maya/mlkem/2.0.0` and peers sort it out themselves.
pub const PROTOCOL: &str = "/maya/mlkem/1.0.0";

/// The connection upgrade applying ML-KEM-768 above the Noise session.
///
/// Stateless and trivially cloneable, which libp2p requires: the same upgrade
/// value is used for every connection, and each one generates its own ephemeral
/// keypair inside the handshake.
#[derive(Clone, Copy, Debug, Default)]
pub struct PqUpgrade;

impl PqUpgrade {
    /// Creates the upgrade.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl UpgradeInfo for PqUpgrade {
    type Info = StreamProtocol;
    type InfoIter = iter::Once<Self::Info>;

    fn protocol_info(&self) -> Self::InfoIter {
        iter::once(StreamProtocol::new(PROTOCOL))
    }
}

impl<C> InboundConnectionUpgrade<C> for PqUpgrade
where
    C: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    type Output = PqStream<C>;
    type Error = NodeError;
    type Future = BoxFuture<'static, Result<Self::Output, Self::Error>>;

    /// The listening side, which generates the keypair and decapsulates.
    fn upgrade_inbound(self, mut socket: C, _: Self::Info) -> Self::Future {
        async move {
            let keys = handshake::respond(&mut socket).await?;
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

    /// The dialing side, which encapsulates to the key it receives.
    fn upgrade_outbound(self, mut socket: C, _: Self::Info) -> Self::Future {
        async move {
            let keys = handshake::initiate(&mut socket).await?;
            Ok(PqStream::new(socket, keys, true))
        }
        .boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_protocol_name_is_versioned() {
        // A name without a version is a name that cannot be changed later
        // without a flag day.
        assert!(PROTOCOL.ends_with("/1.0.0"));
        assert!(StreamProtocol::try_from_owned(PROTOCOL.to_string()).is_ok());
    }

    #[test]
    fn exactly_one_protocol_is_offered() {
        // Offering a second would be offering a downgrade.
        let offered: Vec<_> = PqUpgrade::new().protocol_info().collect();
        assert_eq!(offered.len(), 1);
        assert_eq!(offered[0].as_ref(), PROTOCOL);
    }
}
