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

/// The exact bytes signed for a request: the same `VOTE_DOMAIN ‖ digest`
/// a validator with a local key signs and every validator verifies, so a
/// network can mix local keys and remote signers (ADR-033). Kind, round and
/// author decide *whether* to sign; they are not signed.
#[must_use]
pub fn signed_message(req: &Request) -> Vec<u8> {
    [maya_dag_bft::VOTE_DOMAIN, req.digest.as_slice()].concat()
}

/// A signer: one key, one protection database.
pub struct Service<B> {
    backend: B,
    db: SlashingDb,
}

impl<B: SignerBackend> Service<B> {
    /// A service over `backend`, protected by `db`.
    pub fn new(backend: B, db: SlashingDb) -> Self {
        Self { backend, db }
    }

    /// Answers one request. The protection record is durable before the
    /// signature exists.
    pub fn handle(&mut self, req: &Request) -> Response {
        let message = signed_message(req);
        match self.db.approve(req.kind, req.round, req.author, req.digest) {
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
