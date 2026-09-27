//! The DAG-BFT wire format: one frame per gossip message.
//!
//! Hand-written rather than derived for the reason every other frame in this
//! crate is: the bytes are consensus-adjacent (a certificate's encoding is
//! what a block's justification stores), the reader is bounds-checked against
//! hostile input, and one value has exactly one encoding.
//!
//! ```text
//! frame     = version:u8 epoch:u64 from:u16 to:u16 body
//! body      = 0 vertex sig | 1 digest round:u64 voter:u16 sig
//!           | 2 certificate | 3 digest
//! vertex    = epoch:u64 round:u64 author:u16 timestamp_ms:u64
//!             n:u64 digest*n  m:u64 (len:u64 bytes)*m
//! cert      = vertex k:u64 (voter:u16 sig)*k
//! sig       = len:u64 bytes
//! ```
//!
//! `to` is [`BROADCAST`] for a message every validator should read; a vote is
//! addressed to one author and ignored by the rest. Gossip carries both — the
//! mesh has no unicast — so `to` only saves the other validators the work.

use maya_dag_bft::{Certificate, Digest, Message, Payload, ValidatorId, Vertex};

use crate::core::codec::ByteReader;
use crate::error::{NodeError, Result};

/// Frame format version. A frame of any other version is refused.
pub const WIRE_VERSION: u8 = 1;

/// `to` for a broadcast.
pub const BROADCAST: u16 = u16::MAX;

/// Most bytes one signature may claim. ML-DSA-65's is 3,309; the bound only
/// has to stop a length prefix from asking for gigabytes.
const MAX_SIGNATURE_BYTES: usize = 8 * 1024;

/// Most bytes one transaction may claim inside a vertex.
const MAX_PAYLOAD_BYTES: usize = 1024 * 1024;

const TAG_PROPOSE: u8 = 0;
const TAG_VOTE: u8 = 1;
const TAG_CERT: u8 = 2;
const TAG_FETCH: u8 = 3;

/// One DAG-BFT gossip frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Envelope {
    /// Committee epoch the sender is running.
    pub epoch: u64,
    /// The sender's validator id. Claimed, never trusted: every proposal,
    /// vote and certificate carries its own signature. It only names who to
    /// answer a `Fetch`.
    pub from: ValidatorId,
    /// Addressee, or [`BROADCAST`].
    pub to: u16,
    /// The engine message.
    pub message: Message,
}

impl Envelope {
    /// Encodes the frame.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(256);
        out.push(WIRE_VERSION);
        out.extend_from_slice(&self.epoch.to_le_bytes());
        out.extend_from_slice(&self.from.to_le_bytes());
        out.extend_from_slice(&self.to.to_le_bytes());
        match &self.message {
            Message::Propose { vertex, signature } => {
                out.push(TAG_PROPOSE);
                put_vertex(&mut out, vertex);
                put_bytes(&mut out, signature);
            }
            Message::Vote {
                digest,
                round,
                voter,
                signature,
            } => {
                out.push(TAG_VOTE);
                out.extend_from_slice(digest);
                out.extend_from_slice(&round.to_le_bytes());
                out.extend_from_slice(&voter.to_le_bytes());
                put_bytes(&mut out, signature);
            }
            Message::Cert(c) => {
                out.push(TAG_CERT);
                put_certificate(&mut out, c);
            }
            Message::Fetch(d) => {
                out.push(TAG_FETCH);
                out.extend_from_slice(d);
            }
        }
        out
    }

    /// Decodes a frame, refusing trailing bytes.
    ///
    /// # Errors
    ///
    /// [`NodeError::Decode`] for a wrong version, an unknown tag, a length
    /// over its bound, truncation, or trailing bytes.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut r = ByteReader::new(bytes);
        let version = r.read_u8()?;
        if version != WIRE_VERSION {
            return Err(NodeError::Decode(format!(
                "bft frame version {version}, expected {WIRE_VERSION}"
            )));
        }
        let epoch = r.read_u64()?;
        let from = read_u16(&mut r)?;
        let to = read_u16(&mut r)?;
        let message = match r.read_u8()? {
            TAG_PROPOSE => Message::Propose {
                vertex: read_vertex(&mut r)?,
                signature: read_bytes(&mut r, MAX_SIGNATURE_BYTES)?,
            },
            TAG_VOTE => Message::Vote {
                digest: r.read_array()?,
                round: r.read_u64()?,
                voter: read_u16(&mut r)?,
                signature: read_bytes(&mut r, MAX_SIGNATURE_BYTES)?,
            },
            TAG_CERT => Message::Cert(read_certificate(&mut r)?),
            TAG_FETCH => Message::Fetch(r.read_array()?),
            tag => return Err(NodeError::Decode(format!("bft frame tag {tag}"))),
        };
        r.finish()?;
        Ok(Self {
            epoch,
            from,
            to,
            message,
        })
    }
}

/// Encodes one certificate on its own: what a block's justification stores.
#[must_use]
pub fn encode_certificate(c: &Certificate) -> Vec<u8> {
    let mut out = Vec::new();
    put_certificate(&mut out, c);
    out
}

/// Decodes one certificate, refusing trailing bytes.
///
/// # Errors
///
/// As [`Envelope::decode`].
pub fn decode_certificate(bytes: &[u8]) -> Result<Certificate> {
    let mut r = ByteReader::new(bytes);
    let c = read_certificate(&mut r)?;
    r.finish()?;
    Ok(c)
}

fn put_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
    out.extend_from_slice(bytes);
}

fn put_vertex(out: &mut Vec<u8>, v: &Vertex) {
    out.extend_from_slice(&v.epoch.to_le_bytes());
    out.extend_from_slice(&v.round.to_le_bytes());
    out.extend_from_slice(&v.author.to_le_bytes());
    out.extend_from_slice(&v.timestamp_ms.to_le_bytes());
    out.extend_from_slice(&(v.parents.len() as u64).to_le_bytes());
    for p in &v.parents {
        out.extend_from_slice(p);
    }
    out.extend_from_slice(&(v.batch.len() as u64).to_le_bytes());
    for tx in &v.batch {
        put_bytes(out, tx);
    }
}

fn put_certificate(out: &mut Vec<u8>, c: &Certificate) {
    put_vertex(out, &c.vertex);
    out.extend_from_slice(&(c.votes.len() as u64).to_le_bytes());
    for (voter, sig) in c.votes.iter().zip(&c.signatures) {
        out.extend_from_slice(&voter.to_le_bytes());
        put_bytes(out, sig);
    }
}

fn read_u16(r: &mut ByteReader<'_>) -> Result<u16> {
    Ok(u16::from_le_bytes(r.read_array()?))
}

fn read_bytes(r: &mut ByteReader<'_>, max: usize) -> Result<Vec<u8>> {
    let len = r.read_u64()?;
    let len = usize::try_from(len)
        .ok()
        .filter(|l| *l <= max)
        .ok_or_else(|| NodeError::Decode(format!("bft field of {len} bytes over {max}")))?;
    Ok(r.read_slice(len)?.to_vec())
}

fn read_vertex(r: &mut ByteReader<'_>) -> Result<Vertex> {
    let epoch = r.read_u64()?;
    let round = r.read_u64()?;
    let author = read_u16(r)?;
    let timestamp_ms = r.read_u64()?;
    let n = r.read_collection_len(32)?;
    let parents = (0..n)
        .map(|_| r.read_array::<32>())
        .collect::<Result<Vec<Digest>>>()?;
    let m = r.read_collection_len(8)?;
    let batch = (0..m)
        .map(|_| read_bytes(r, MAX_PAYLOAD_BYTES))
        .collect::<Result<Vec<Payload>>>()?;
    Ok(Vertex {
        epoch,
        round,
        author,
        timestamp_ms,
        parents,
        batch,
    })
}

fn read_certificate(r: &mut ByteReader<'_>) -> Result<Certificate> {
    let vertex = read_vertex(r)?;
    let k = r.read_collection_len(10)?;
    let mut votes = Vec::with_capacity(k);
    let mut signatures = Vec::with_capacity(k);
    for _ in 0..k {
        votes.push(read_u16(r)?);
        signatures.push(read_bytes(r, MAX_SIGNATURE_BYTES)?);
    }
    Ok(Certificate {
        vertex,
        votes,
        signatures,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn vertex() -> Vertex {
        Vertex {
            epoch: 2,
            round: 9,
            author: 3,
            timestamp_ms: 1_700_000_000_123,
            parents: vec![[1; 32], [2; 32], [3; 32]],
            batch: vec![vec![0xaa; 40], vec![], vec![0xbb; 3]],
        }
    }

    fn every_message() -> Vec<Message> {
        vec![
            Message::Propose {
                vertex: vertex(),
                signature: vec![5; 3309],
            },
            Message::Vote {
                digest: [4; 32],
                round: 9,
                voter: 1,
                signature: vec![6; 3309],
            },
            Message::Cert(Certificate {
                vertex: vertex(),
                votes: vec![0, 1, 3],
                signatures: vec![vec![7; 3309], vec![8; 3309], vec![9; 3309]],
            }),
            Message::Fetch([9; 32]),
        ]
    }

    #[test]
    fn every_message_round_trips() {
        for message in every_message() {
            let env = Envelope {
                epoch: 2,
                from: 3,
                to: BROADCAST,
                message,
            };
            assert_eq!(Envelope::decode(&env.encode()).unwrap(), env);
        }
    }

    #[test]
    fn truncation_trailing_bytes_and_bad_versions_are_refused() {
        for message in every_message() {
            let bytes = Envelope {
                epoch: 0,
                from: 0,
                to: 1,
                message,
            }
            .encode();
            for cut in [1, bytes.len() / 2, bytes.len() - 1] {
                assert!(Envelope::decode(&bytes[..cut]).is_err(), "cut at {cut}");
            }
            let mut long = bytes.clone();
            long.push(0);
            assert!(Envelope::decode(&long).is_err(), "trailing byte");
            let mut version = bytes;
            version[0] = WIRE_VERSION + 1;
            assert!(Envelope::decode(&version).is_err(), "version");
        }
    }

    #[test]
    fn a_length_prefix_cannot_ask_for_more_than_its_bound() {
        let mut bytes = Envelope {
            epoch: 0,
            from: 0,
            to: 0,
            message: Message::Vote {
                digest: [0; 32],
                round: 0,
                voter: 0,
                signature: vec![],
            },
        }
        .encode();
        // Replace the empty signature's length with u64::MAX.
        let at = bytes.len() - 8;
        bytes[at..].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(Envelope::decode(&bytes).is_err());
    }

    #[test]
    fn a_certificate_round_trips_on_its_own() {
        let Message::Cert(c) = every_message().swap_remove(2) else {
            unreachable!()
        };
        assert_eq!(decode_certificate(&encode_certificate(&c)).unwrap(), c);
    }
}
