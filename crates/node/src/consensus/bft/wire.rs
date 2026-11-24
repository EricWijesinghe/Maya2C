//! The DAG-BFT wire format: one frame per gossip message.
//!
//! Hand-written rather than derived for the reason every other frame in this
//! crate is: the bytes are consensus-adjacent (a certificate's encoding is
//! what a block's justification stores), the reader is bounds-checked against
//! hostile input, and one value has exactly one encoding.
//!
//! ```text
//! frame     = version:u8 epoch:u64 from:id to:id body
//! body      = 0 vertex sig | 1 digest round:u64 voter:id sig
//!           | 2 certificate | 3 digest
//! vertex    = epoch:u64 round:u64 author:id timestamp_ms:u64
//!             n:u64 digest*n  m:u64 (len:u64 bytes)*m
//! cert      = cert_version:u8 vertex k:u64 (voter:id sig)*k
//! header    = header_version:u8 epoch:u64 round:u64 author:id
//!             timestamp_ms:u64 parents_root:32 batch_root:32 sig
//! id        = u32 little-endian; 0xFFFFFFFF = broadcast / no sender (from, to only)
//! sig       = len:u64 bytes
//! ```
//!
//! `to` is [`BROADCAST`] for a message every validator should read; a vote is
//! addressed to one author and ignored by the rest. Gossip carries both — the
//! mesh has no unicast — so `to` only saves the other validators the work.
//!
//! Format room (ADR-155). Every validator id is four bytes on the wire, so a
//! committee wider than today's `u16` index never needs a new encoding; this
//! build refuses an id above `u16::MAX` cleanly at decode (NEW-7). Broadcast
//! and no-sender travel as `0xFFFFFFFF`, which is never an id, so the
//! widening moves no sentinel either. A
//! certificate record names its own version in its first byte, because it is
//! stored and served on its own, outside any frame (C2). A signed vertex
//! header is the equivocation-evidence form (NEW-1): a fixed size, however
//! large the vertex's batch.

use maya_dag_bft::{Certificate, Digest, Message, Payload, ValidatorId, Vertex, VertexHeader};

use crate::core::codec::ByteReader;
use crate::error::{NodeError, Result};

/// Frame format version. A frame of any other version is refused. Version 2
/// (ADR-155) widened every validator id to four bytes and versioned the
/// certificate.
pub const WIRE_VERSION: u8 = 2;

/// Certificate record format version, the first byte of every encoded
/// certificate. Any other value is refused, never guessed at.
pub const CERT_VERSION: u8 = 1;

/// Signed vertex header format version, the first byte of each half of an
/// equivocation report.
pub const HEADER_VERSION: u8 = 1;

/// Encoded size of a signed vertex header without its signature bytes:
/// version, epoch, round, author, time, two roots, signature length.
pub const SIGNED_HEADER_FIXED_BYTES: usize = 1 + 8 + 8 + 4 + 8 + 32 + 32 + 8;

/// Most bytes one signed vertex header may take: what an equivocation report
/// carries per proposal, at any block size.
pub const MAX_SIGNED_HEADER_BYTES: usize = SIGNED_HEADER_FIXED_BYTES + MAX_SIGNATURE_BYTES;

/// `to` for a broadcast, and `from` for an observer, in memory. On the wire
/// both are [`WIRE_BROADCAST`], so the in-memory value can follow the index
/// width while the bytes never change.
pub const BROADCAST: ValidatorId = ValidatorId::MAX;

/// Broadcast / no-sender on the wire: all ones, never a validator id at any
/// committee width. `0x0000FFFF` is the ordinary id 65,535 (NEW-7).
pub const WIRE_BROADCAST: u32 = u32::MAX;

/// Most bytes one signature may claim. An ML-DSA-87 validator signature is 4,627; the bound only
/// has to stop a length prefix from asking for gigabytes.
const MAX_SIGNATURE_BYTES: usize = 8 * 1024;

/// Most bytes one transaction may claim inside a vertex: the most the
/// transaction codec accepts, or a legal transaction could be mined by no one.
const MAX_PAYLOAD_BYTES: usize = crate::core::MAX_TX_BYTES;

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
    pub to: ValidatorId,
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
        put_addr(&mut out, self.from);
        put_addr(&mut out, self.to);
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
                put_id(&mut out, *voter);
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
        let from = read_addr(&mut r)?;
        let to = read_addr(&mut r)?;
        let message = match r.read_u8()? {
            TAG_PROPOSE => Message::Propose {
                vertex: read_vertex(&mut r)?,
                signature: read_bytes(&mut r, MAX_SIGNATURE_BYTES)?,
            },
            TAG_VOTE => Message::Vote {
                digest: r.read_array()?,
                round: r.read_u64()?,
                voter: read_id(&mut r)?,
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
    put_id(out, v.author);
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
    out.push(CERT_VERSION);
    put_vertex(out, &c.vertex);
    out.extend_from_slice(&(c.votes.len() as u64).to_le_bytes());
    for (voter, sig) in c.votes.iter().zip(&c.signatures) {
        put_id(out, *voter);
        put_bytes(out, sig);
    }
}

/// A validator id as four bytes (NEW-7).
fn put_id(out: &mut Vec<u8>, id: ValidatorId) {
    out.extend_from_slice(&u32::from(id).to_le_bytes());
}

/// A frame's sender or addressee: [`BROADCAST`] travels as [`WIRE_BROADCAST`].
fn put_addr(out: &mut Vec<u8>, id: ValidatorId) {
    if id == BROADCAST {
        out.extend_from_slice(&WIRE_BROADCAST.to_le_bytes());
    } else {
        put_id(out, id);
    }
}

/// A frame's sender or addressee: [`WIRE_BROADCAST`] is [`BROADCAST`], any
/// other value an ordinary id.
fn read_addr(r: &mut ByteReader<'_>) -> Result<ValidatorId> {
    let id = u32::from_le_bytes(r.read_array()?);
    if id == WIRE_BROADCAST {
        return Ok(BROADCAST);
    }
    narrow_id(id)
}

/// A four-byte validator id, refused when it is all ones (never an id) or
/// wider than this build's committee index can hold.
fn read_id(r: &mut ByteReader<'_>) -> Result<ValidatorId> {
    let id = u32::from_le_bytes(r.read_array()?);
    if id == WIRE_BROADCAST {
        return Err(NodeError::Decode(
            "validator id 0xFFFFFFFF is reserved".into(),
        ));
    }
    narrow_id(id)
}

fn narrow_id(id: u32) -> Result<ValidatorId> {
    ValidatorId::try_from(id)
        .map_err(|_| NodeError::Decode(format!("validator id {id} wider than this build's index")))
}

/// Encodes a vertex header and its author's signature: one half of an
/// equivocation report (NEW-1).
#[must_use]
pub fn encode_signed_header(header: &VertexHeader, signature: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(SIGNED_HEADER_FIXED_BYTES + signature.len());
    out.push(HEADER_VERSION);
    out.extend_from_slice(&header.epoch.to_le_bytes());
    out.extend_from_slice(&header.round.to_le_bytes());
    put_id(&mut out, header.author);
    out.extend_from_slice(&header.timestamp_ms.to_le_bytes());
    out.extend_from_slice(&header.parents_root);
    out.extend_from_slice(&header.batch_root);
    put_bytes(&mut out, signature);
    out
}

/// Decodes what [`encode_signed_header`] wrote, refusing trailing bytes.
///
/// # Errors
///
/// [`NodeError::Decode`] for another version, an id too wide, a signature
/// over its bound, truncation or trailing bytes.
pub fn decode_signed_header(bytes: &[u8]) -> Result<(VertexHeader, Vec<u8>)> {
    let mut r = ByteReader::new(bytes);
    let version = r.read_u8()?;
    if version != HEADER_VERSION {
        return Err(NodeError::Decode(format!(
            "vertex header version {version}, expected {HEADER_VERSION}"
        )));
    }
    let header = VertexHeader {
        epoch: r.read_u64()?,
        round: r.read_u64()?,
        author: read_id(&mut r)?,
        timestamp_ms: r.read_u64()?,
        parents_root: r.read_array()?,
        batch_root: r.read_array()?,
    };
    let signature = read_bytes(&mut r, MAX_SIGNATURE_BYTES)?;
    r.finish()?;
    Ok((header, signature))
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
    let author = read_id(r)?;
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
    let version = r.read_u8()?;
    if version != CERT_VERSION {
        return Err(NodeError::Decode(format!(
            "certificate version {version}, expected {CERT_VERSION}"
        )));
    }
    let vertex = read_vertex(r)?;
    // Smallest vote entry: a four-byte id and an empty signature's length.
    let k = r.read_collection_len(4 + 8)?;
    let mut votes = Vec::with_capacity(k);
    let mut signatures = Vec::with_capacity(k);
    for _ in 0..k {
        votes.push(read_id(r)?);
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
    fn a_vertex_carries_a_transaction_of_the_largest_size_the_codec_accepts() {
        // The mempool and the transaction codec accept up to MAX_TX_BYTES, and
        // a batch may hold that much. A smaller per-item cap here made every
        // peer refuse a vertex that carried such a transaction.
        let mut big = vertex();
        big.batch = vec![vec![0xcc; crate::core::MAX_TX_BYTES]];
        let env = Envelope {
            epoch: 2,
            from: 3,
            to: BROADCAST,
            message: Message::Propose {
                vertex: big,
                signature: vec![5; 3309],
            },
        };
        assert_eq!(Envelope::decode(&env.encode()).unwrap(), env);
    }

    #[test]
    fn a_batch_item_one_byte_over_the_transaction_cap_is_refused() {
        // Regression pin: the wire bound on a batch item is MAX_TX_BYTES, so a
        // frame cannot carry a transaction the codec would refuse.
        let mut over = vertex();
        over.batch = vec![vec![0xcc; crate::core::MAX_TX_BYTES + 1]];
        let env = Envelope {
            epoch: 2,
            from: 3,
            to: BROADCAST,
            message: Message::Propose {
                vertex: over,
                signature: vec![5; 3309],
            },
        };
        assert!(Envelope::decode(&env.encode()).is_err());
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
    fn a_signed_header_round_trips_and_refuses_another_version_or_trailing_bytes() {
        let header = vertex().header();
        let bytes = encode_signed_header(&header, &[5; 3309]);
        assert_eq!(bytes.len(), SIGNED_HEADER_FIXED_BYTES + 3309);
        assert_eq!(
            decode_signed_header(&bytes).unwrap(),
            (header.clone(), vec![5; 3309])
        );
        assert_eq!(
            header.digest(),
            vertex().digest(),
            "the header proves the digest"
        );
        let mut other = bytes.clone();
        other[0] = HEADER_VERSION + 1;
        assert!(decode_signed_header(&other).is_err());
        let mut long = bytes;
        long.push(0);
        assert!(decode_signed_header(&long).is_err());
    }

    #[test]
    fn a_certificate_round_trips_on_its_own() {
        let Message::Cert(c) = every_message().swap_remove(2) else {
            unreachable!()
        };
        assert_eq!(decode_certificate(&encode_certificate(&c)).unwrap(), c);
    }
}
