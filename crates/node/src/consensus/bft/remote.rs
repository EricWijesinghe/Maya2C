//! A validator key held by `maya2c-signer` in another process (ADR-033).
//!
//! The node never sees the key. For each proposal or vote it sends the
//! signer the slot (`kind`, `round`, `author`) and the vertex digest over the
//! mutually authenticated post-quantum channel (`maya_signer::channel`). The
//! signer applies its own slashing protection and returns a signature over
//! exactly the bytes every validator verifies (`VOTE_DOMAIN ‖ digest`).
//!
//! Every failure is a missing vote, never a wrong one. A refusal, a timeout, a
//! broken channel or a signature that does not verify under the validator's
//! key all return `None`, which the engine treats as "did not vote". That
//! costs liveness at worst; the safety rules are the signer's protection and
//! the node's own safety log, and either alone prevents a double sign.

use std::net::{SocketAddr, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use maya_dag_bft::{Digest, SignContext, SignKind};
use maya_signer::channel::{self, Channel, Identity};
use maya_signer::protection::Kind;
use maya_signer::service::{Request, Response};

use crate::crypto::SIGNATURE_LENGTH;
use crate::crypto::keys::{SigningKey, VerifyingKey};
use crate::error::{NodeError, Result};

/// How long one signing round-trip may take by default: well under the
/// anchor timeout, so a slow signer costs a vote, not a round.
pub const DEFAULT_DEADLINE: Duration = Duration::from_secs(2);

/// Where a validator's signatures come from.
#[derive(Clone)]
pub enum ValidatorKey {
    /// A key file on this host.
    Local(Arc<SigningKey>),
    /// A key held by `maya2c-signer`.
    Remote(Arc<RemoteSigner>),
}

impl ValidatorKey {
    /// The public key the committee knows this validator by.
    #[must_use]
    pub fn verifying_key(&self) -> VerifyingKey {
        match self {
            Self::Local(key) => key.verifying_key(),
            Self::Remote(remote) => remote.public.clone(),
        }
    }

    /// This validator's attestation signature on block `block` at `height`
    /// of `chain` in `epoch`, as committee member `validator` (ADR-038).
    /// `None` when signing failed or the remote signer refused, which the
    /// signer reports; a missing attestation costs a checkpoint, never safety.
    #[must_use]
    pub fn sign_attestation(
        &self,
        validator: u16,
        chain: &crate::core::ChainTag,
        epoch: u64,
        height: u64,
        block: &[u8; 32],
    ) -> Option<Vec<u8>> {
        use super::attest::{attestation_bytes, attestation_digest};
        match self {
            Self::Local(key) => key
                .sign(&attestation_bytes(chain, epoch, height, block))
                .map(|s| s.to_vec())
                .map_err(|e| eprintln!("attestation at height {height} not signed: {e}"))
                .ok(),
            Self::Remote(remote) => remote.sign_attestation(
                height,
                validator,
                &attestation_digest(chain, epoch, height, block),
            ),
        }
    }
}

impl From<Arc<SigningKey>> for ValidatorKey {
    fn from(key: Arc<SigningKey>) -> Self {
        Self::Local(key)
    }
}

/// A connection to `maya2c-signer`, reopened on demand.
pub struct RemoteSigner {
    addr: SocketAddr,
    identity: Identity,
    signer_pin: Vec<u8>,
    public: VerifyingKey,
    deadline: Duration,
    conn: Mutex<Option<Channel<TcpStream>>>,
}

impl core::fmt::Debug for RemoteSigner {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RemoteSigner")
            .field("addr", &self.addr)
            .field("deadline", &self.deadline)
            .finish_non_exhaustive()
    }
}

impl RemoteSigner {
    /// A signer at `addr` whose channel identity is `signer_pin`, holding the
    /// validator key `public`. `identity` is this node's channel identity,
    /// which the signer must have pinned. Connects once, so a wrong address or
    /// pin fails at startup rather than at the first vote.
    ///
    /// # Errors
    ///
    /// [`NodeError::Network`] when the first connection or handshake fails.
    pub fn connect(
        addr: SocketAddr,
        identity: Identity,
        signer_pin: Vec<u8>,
        public: VerifyingKey,
        deadline: Duration,
    ) -> Result<Self> {
        let signer = Self {
            addr,
            identity,
            signer_pin,
            public,
            deadline,
            conn: Mutex::new(None),
        };
        let channel = signer.open()?;
        if let Ok(mut conn) = signer.conn.lock() {
            *conn = Some(channel);
        }
        Ok(signer)
    }

    fn open(&self) -> Result<Channel<TcpStream>> {
        let network = |e: &dyn core::fmt::Display| {
            NodeError::Network(format!("remote signer {}: {e}", self.addr))
        };
        let stream =
            TcpStream::connect_timeout(&self.addr, self.deadline).map_err(|e| network(&e))?;
        stream
            .set_read_timeout(Some(self.deadline))
            .and_then(|()| stream.set_write_timeout(Some(self.deadline)))
            .map_err(|e| network(&e))?;
        channel::client(stream, &self.identity, &self.signer_pin).map_err(|e| network(&e))
    }

    fn exchange(
        channel: &mut Channel<TcpStream>,
        request: &Request,
    ) -> core::result::Result<Response, String> {
        let body = serde_json::to_vec(request).map_err(|e| e.to_string())?;
        channel.send(&body).map_err(|e| e.to_string())?;
        let reply = channel.recv().map_err(|e| e.to_string())?;
        serde_json::from_slice(&reply).map_err(|e| e.to_string())
    }

    /// The signer's signature for `ctx` over `digest`, checked against the
    /// validator key; `None` on any failure, which is reported on stderr.
    #[must_use]
    pub fn sign(&self, ctx: SignContext, digest: &Digest) -> Option<Vec<u8>> {
        let request = Request {
            kind: match ctx.kind {
                SignKind::Proposal => Kind::Vertex,
                SignKind::Vote => Kind::Vote,
            },
            round: ctx.round,
            author: ctx.author,
            digest: *digest,
        };
        self.sign_request(&request)
    }

    /// The signer's attestation of the block whose attestation digest is
    /// `digest`, at `height`, by committee member `author` (ADR-038). The
    /// signer refuses a second block at one height.
    #[must_use]
    pub fn sign_attestation(&self, height: u64, author: u16, digest: &Digest) -> Option<Vec<u8>> {
        self.sign_request(&Request {
            kind: Kind::Attestation,
            round: height,
            author,
            digest: *digest,
        })
    }

    fn sign_request(&self, request: &Request) -> Option<Vec<u8>> {
        // A panic while another call held the lock must not disable signing
        // for good: take the lock back and drop whatever channel it guarded,
        // whose counters may be mid-exchange.
        let mut conn = self.conn.lock().unwrap_or_else(|poisoned| {
            let mut guard = poisoned.into_inner();
            *guard = None;
            guard
        });
        if conn.is_none() {
            match self.open() {
                Ok(channel) => *conn = Some(channel),
                Err(e) => {
                    eprintln!("{e}; {:?} {} not signed", request.kind, request.round);
                    return None;
                }
            }
        }
        let channel = conn.as_mut()?;
        match Self::exchange(channel, request) {
            Ok(Response::Signed(sig)) => self.checked(sig, request),
            Ok(Response::Refused(why)) => {
                eprintln!(
                    "remote signer refused {:?} {} author {}: {why}",
                    request.kind, request.round, request.author
                );
                None
            }
            Err(e) => {
                // The channel's counters are now out of step; start again.
                *conn = None;
                eprintln!(
                    "remote signer {}: {e}; {:?} {} not signed",
                    self.addr, request.kind, request.round
                );
                None
            }
        }
    }

    /// A signature is used only if it verifies under the validator key over
    /// the bytes this kind signs: a signer holding the wrong key must cost
    /// votes, not publish garbage.
    fn checked(&self, sig: Vec<u8>, request: &Request) -> Option<Vec<u8>> {
        let message = maya_signer::service::signed_message(request);
        let valid = <&[u8; SIGNATURE_LENGTH]>::try_from(sig.as_slice())
            .is_ok_and(|s| self.public.verify(&message, s).is_ok());
        if valid {
            Some(sig)
        } else {
            eprintln!(
                "remote signer returned a signature that does not verify under the validator key; {:?} {} not signed",
                request.kind, request.round
            );
            None
        }
    }
}
