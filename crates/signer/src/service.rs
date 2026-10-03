//! The request handler: protection first, signature second.

use crate::backend::SignerBackend;
use crate::protection::{DbError, Kind, SlashingDb};
use serde::{Deserialize, Serialize};

/// What the node asks for: the slot a signature fills, and the vertex
/// digest to sign for it (ADR-033).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    /// Proposal or vote.
    pub kind: Kind,
    /// The vertex's round.
    pub round: u64,
    /// The vertex's author.
    pub author: u16,
    /// The vertex digest.
    pub digest: [u8; 32],
}

/// What the signer answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Response {
    /// The ML-DSA-65 signature over [`signed_message`].
    Signed(Vec<u8>),
    /// Refused, with the reason. Never an error the node may retry around.
    Refused(String),
}

/// Domain of a block attestation's signed bytes (ADR-038), distinct from
/// [`maya_dag_bft::VOTE_DOMAIN`] so no vote can be replayed as an
/// attestation or the reverse.
pub const ATTEST_DOMAIN: &[u8] = b"maya2c block attestation v1";

/// The exact bytes signed for a request: the same `domain ‖ digest` a
/// validator with a local key signs and every validator verifies, so a
/// network can mix local keys and remote signers (ADR-033). The domain is
/// [`maya_dag_bft::VOTE_DOMAIN`] for vertices and votes and [`ATTEST_DOMAIN`]
/// for attestations. Kind, round and author decide *whether* to sign; they
/// are not signed.
#[must_use]
pub fn signed_message(req: &Request) -> Vec<u8> {
    let domain = match req.kind {
        Kind::Vertex | Kind::Vote => maya_dag_bft::VOTE_DOMAIN,
        Kind::Attestation => ATTEST_DOMAIN,
    };
    [domain, req.digest.as_slice()].concat()
}

/// A signer: one key, one protection database for the DAG, and optionally
/// one for block attestations.
pub struct Service<B> {
    backend: B,
    db: SlashingDb,
    attestations: Option<SlashingDb>,
}

impl<B: SignerBackend> Service<B> {
    /// A service over `backend`, protected by `db`. It refuses attestations
    /// until [`Service::with_attestations`] gives them a database.
    pub fn new(backend: B, db: SlashingDb) -> Self {
        Self {
            backend,
            db,
            attestations: None,
        }
    }

    /// The same service, signing block attestations under `db` (ADR-038).
    #[must_use]
    pub fn with_attestations(self, db: SlashingDb) -> Self {
        Self {
            attestations: Some(db),
            ..self
        }
    }

    /// Answers one request. The protection record is durable before the
    /// signature exists.
    pub fn handle(&mut self, req: &Request) -> Response {
        let message = signed_message(req);
        let db = match req.kind {
            Kind::Vertex | Kind::Vote => &mut self.db,
            Kind::Attestation => match self.attestations.as_mut() {
                Some(db) => db,
                // Fail closed: without its own record an attestation could be
                // signed twice for one height.
                None => {
                    return Response::Refused(
                        "attestations are not enabled on this signer".to_string(),
                    );
                }
            },
        };
        match db.approve(req.kind, req.round, req.author, req.digest) {
            Ok(()) => match self.backend.sign(&message) {
                Ok(sig) => Response::Signed(sig),
                Err(e) => Response::Refused(e.to_string()),
            },
            Err(DbError::Refused(r)) => Response::Refused(r.to_string()),
            Err(e) => Response::Refused(format!("not signed: {e}")),
        }
    }

    /// The protection database, for export.
    pub fn db(&self) -> &SlashingDb {
        &self.db
    }

    /// The protection database, for import.
    pub fn db_mut(&mut self) -> &mut SlashingDb {
        &mut self.db
    }

    /// The backend's public key.
    pub fn public_key(&self) -> &[u8] {
        self.backend.public_key()
    }
}
