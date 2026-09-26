//! The request handler: protection first, signature second.

use crate::backend::SignerBackend;
use crate::protection::{DbError, Kind, SlashingDb};
use serde::{Deserialize, Serialize};

/// Domain separator for every consensus message this signer signs, so a
/// signature over a vote can never be presented as one over anything else.
const DOMAIN: &[u8] = b"maya2c consensus signing v1";

/// What the node asks for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    /// Vertex or vote.
    pub kind: Kind,
    /// Consensus round.
    pub round: u64,
    /// The message body (a vertex digest, a vote's target), opaque here.
    pub payload: Vec<u8>,
}

/// What the signer answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Response {
    /// The ML-DSA-65 signature over [`signed_message`].
    Signed(Vec<u8>),
    /// Refused, with the reason. Never an error the node may retry around.
    Refused(String),
}

/// The exact bytes signed for a request: domain, kind, round, payload.
#[must_use]
pub fn signed_message(req: &Request) -> Vec<u8> {
    let mut m = DOMAIN.to_vec();
    m.push(match req.kind {
        Kind::Vertex => 0,
        Kind::Vote => 1,
    });
    m.extend_from_slice(&req.round.to_le_bytes());
    m.extend_from_slice(&req.payload);
    m
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
        let root = *blake3::hash(&message).as_bytes();
        match self.db.approve(req.kind, req.round, root) {
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
