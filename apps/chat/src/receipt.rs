//! Delivery receipts (ADR-074, decision 2). A receipt is an ordinary session
//! message whose plaintext starts with [`MAGIC`]; the signed statement binds it
//! to one session, one recipient and one sender, so it cannot be moved.

use maya_crypto_pq::suite::{self, SuiteId};

use crate::identity::address_of;
use crate::{Address, ChatError};

/// Marks a session plaintext as a receipt. [`crate::client::Client::queue`]
/// refuses user text that starts with it, so text cannot pose as one.
pub const MAGIC: &[u8] = b"\0maya-chat receipt v1\0";
const DOMAIN: &str = "maya-chat 2026-11 receipt v1";
/// Most envelope ids one receipt carries (one collection pass's worth).
pub const MAX_IDS: usize = 1_000;
const ID_LEN: usize = 32;

/// Whether a decrypted plaintext is a receipt.
#[must_use]
pub fn is_receipt(plain: &[u8]) -> bool {
    plain.starts_with(MAGIC)
}

/// The statement a receipt signs: the session, the signer (`recipient`), the
/// other party (`sender`), and each envelope id.
#[must_use]
pub fn digest(
    session: &[u8; 32],
    recipient: &Address,
    sender: &Address,
    ids: &[[u8; 32]],
) -> [u8; 32] {
    let mut h = blake3::Hasher::new_derive_key(DOMAIN);
    h.update(session);
    h.update(recipient);
    h.update(sender);
    for id in ids {
        h.update(id);
    }
    *h.finalize().as_bytes()
}

/// A receipt's plaintext: [`MAGIC`], the id count, the ids, the signature.
#[must_use]
pub fn encode(ids: &[[u8; 32]], signature: &[u8]) -> Vec<u8> {
    let count = u32::try_from(ids.len()).unwrap_or(u32::MAX);
    let mut out = [MAGIC, &count.to_le_bytes()].concat();
    for id in ids {
        out.extend_from_slice(id);
    }
    out.extend_from_slice(signature);
    out
}

/// Splits a receipt into its ids and signature. Checks only the shape; the
/// signature is [`verify`]'s job.
///
/// # Errors
///
/// [`ChatError::Refused`] for a malformed or oversized receipt.
pub fn parse(plain: &[u8]) -> Result<(Vec<[u8; 32]>, &[u8]), ChatError> {
    let body = plain
        .strip_prefix(MAGIC)
        .ok_or(ChatError::Refused("not a receipt"))?;
    let (count, rest) = body
        .split_first_chunk::<4>()
        .ok_or(ChatError::Refused("malformed receipt"))?;
    let count = usize::try_from(u32::from_le_bytes(*count)).unwrap_or(usize::MAX);
    if count == 0 || count > MAX_IDS {
        return Err(ChatError::Refused("receipt id count out of range"));
    }
    let ids_len = count * ID_LEN;
    if rest.len() <= ids_len {
        return Err(ChatError::Refused("malformed receipt"));
    }
    let (chunks, _) = rest[..ids_len].as_chunks::<ID_LEN>();
    let ids = chunks.to_vec();
    Ok((ids, &rest[ids_len..]))
}

/// Checks a receipt signed by `signer` (the other party, `from`). The key
/// must be `from`'s: its address is `from`.
///
/// # Errors
///
/// [`ChatError::BadSignature`] for any mismatch, including a missing key.
pub fn verify(
    signer_key: &[u8],
    from: &Address,
    me: &Address,
    session: &[u8; 32],
    ids: &[[u8; 32]],
    signature: &[u8],
) -> Result<(), ChatError> {
    if signer_key.is_empty() || address_of(signer_key) != *from {
        return Err(ChatError::BadSignature);
    }
    suite::verify(
        SuiteId::MlDsa65,
        signer_key,
        &digest(session, from, me, ids),
        signature,
    )
    .map_err(|_| ChatError::BadSignature)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // a test that fails should panic, and say where
mod tests {
    use super::*;
    use crate::identity::Identity;

    const SESSION: [u8; 32] = [4; 32];

    fn ids() -> Vec<[u8; 32]> {
        vec![[1; 32], [2; 32]]
    }

    #[test]
    fn a_receipt_round_trips_through_its_plaintext() {
        let plain = encode(&ids(), &[9; 64]);
        assert!(is_receipt(&plain));
        let (parsed, sig) = parse(&plain).unwrap();
        assert_eq!(parsed, ids());
        assert_eq!(sig, &[9; 64][..]);
    }

    #[test]
    fn malformed_receipts_are_refused() {
        assert!(parse(b"hello").is_err(), "not a receipt");
        let none = encode(&[], &[9; 8]);
        assert!(parse(&none).is_err(), "zero ids");
        let no_signature = encode(&ids(), &[]);
        assert!(parse(&no_signature).is_err(), "no signature");
        let too_many = encode(&vec![[0; 32]; MAX_IDS + 1], &[9; 8]);
        assert!(parse(&too_many).is_err(), "over the id bound");
    }

    #[test]
    fn a_signature_verifies_only_for_its_session_peer_and_ids() {
        let me = Identity::from_seed([1; 32]);
        let peer = Identity::from_seed([2; 32]);
        let (me_addr, peer_addr) = (me.address(), peer.address());
        // The peer signs what it received from `me`: it is the signer.
        let sig = peer
            .sign(&digest(&SESSION, &peer_addr, &me_addr, &ids()))
            .unwrap();
        let key = peer.public_key();
        assert!(verify(key, &peer_addr, &me_addr, &SESSION, &ids(), &sig).is_ok());
        assert_eq!(
            verify(key, &peer_addr, &me_addr, &[5; 32], &ids(), &sig),
            Err(ChatError::BadSignature),
            "another session"
        );
        assert_eq!(
            verify(key, &peer_addr, &me_addr, &SESSION, &[[3; 32]], &sig),
            Err(ChatError::BadSignature),
            "other ids"
        );
        assert_eq!(
            verify(key, &me_addr, &me_addr, &SESSION, &ids(), &sig),
            Err(ChatError::BadSignature),
            "a key that is not the claimed sender's"
        );
        assert_eq!(
            verify(&[], &peer_addr, &me_addr, &SESSION, &ids(), &sig),
            Err(ChatError::BadSignature),
            "no key (a session saved before receipts)"
        );
    }
}
