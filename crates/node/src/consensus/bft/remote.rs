//! A validator key held by `maya2c-signer` in another process (ADR-032).
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

use maya_dag_bft::{Digest, SignContext, SignKind, VOTE_DOMAIN};
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
                    eprintln!("{e}; round {} not signed", ctx.round);
                    return None;
                }
            }
        }
        let channel = conn.as_mut()?;
        match Self::exchange(channel, &request) {
            Ok(Response::Signed(sig)) => self.checked(sig, digest, ctx),
            Ok(Response::Refused(why)) => {
                eprintln!(
                    "remote signer refused round {} author {}: {why}",
                    ctx.round, ctx.author
                );
                None
            }
            Err(e) => {
                // The channel's counters are now out of step; start again.
                *conn = None;
                eprintln!(
                    "remote signer {}: {e}; round {} not signed",
                    self.addr, ctx.round
                );
                None
            }
        }
    }

    /// A signature is used only if it verifies under the validator key: a
    /// signer holding the wrong key must cost votes, not publish garbage.
    fn checked(&self, sig: Vec<u8>, digest: &Digest, ctx: SignContext) -> Option<Vec<u8>> {
        let message = [VOTE_DOMAIN, digest.as_slice()].concat();
        let valid = <&[u8; SIGNATURE_LENGTH]>::try_from(sig.as_slice())
            .is_ok_and(|s| self.public.verify(&message, s).is_ok());
        if valid {
            Some(sig)
        } else {
            eprintln!(
                "remote signer returned a signature that does not verify under the validator key; round {} not signed",
                ctx.round
            );
            None
        }
    }
}
