//! Quantum exposure: is this key already public? (Master Prompt 28 §1)
//!
//! Shor's algorithm recovers an elliptic-curve private key from its public
//! key. What matters is whether the public key is **on-chain now**:
//!
//! - **Bitcoin.** P2PK and P2TR outputs put a public key (a Taproot output key
//!   is one) in the output itself: exposed from the moment they are created.
//!   P2PKH and P2WPKH commit to a hash: the key appears only when the output is
//!   spent, or already if the same address was spent from before (reuse).
//!   Script-hash outputs (P2SH, P2WSH) depend on the script behind them.
//! - **Ethereum.** An externally owned account's public key is recoverable
//!   from any signature it has made: exposed once it has sent a transaction
//!   (nonce > 0). A contract account has no key.
//!
//! The classification is exact for the script templates; "unknown" is said,
//! not guessed.

use sha2::{Digest, Sha256};

/// How exposed an output or account is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exposure {
    /// The public key is on-chain now.
    Exposed,
    /// Only a hash is on-chain; exposed when spent or if the address was reused.
    HashedUntilSpent,
    /// Depends on the script behind a script hash.
    DependsOnScript,
    /// No key: a contract, or an unspendable output.
    NoKey,
    /// Not a recognised template.
    Unknown,
}

/// A Bitcoin output script template.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Template {
    /// Pay to public key.
    P2pk,
    /// Pay to public-key hash.
    P2pkh,
    /// Pay to script hash.
    P2sh,
    /// `SegWit` v0 key hash.
    P2wpkh,
    /// `SegWit` v0 script hash.
    P2wsh,
    /// Taproot.
    P2tr,
    /// `OP_RETURN` data.
    NullData,
    /// Anything else.
    Other,
}

/// Recognises the standard templates by exact shape.
#[must_use]
pub fn template(script: &[u8]) -> Template {
    match script {
        [0x41, key @ .., 0xac] if key.len() == 65 => Template::P2pk,
        [0x21, key @ .., 0xac] if key.len() == 33 => Template::P2pk,
        [0x76, 0xa9, 0x14, h @ .., 0x88, 0xac] if h.len() == 20 => Template::P2pkh,
        [0xa9, 0x14, h @ .., 0x87] if h.len() == 20 => Template::P2sh,
        [0x00, 0x14, h @ ..] if h.len() == 20 => Template::P2wpkh,
        [0x00, 0x20, h @ ..] if h.len() == 32 => Template::P2wsh,
        [0x51, 0x20, k @ ..] if k.len() == 32 => Template::P2tr,
        [0x6a, ..] => Template::NullData,
        _ => Template::Other,
    }
}

/// Exposure of a Bitcoin output script.
#[must_use]
pub fn bitcoin_exposure(script: &[u8]) -> Exposure {
    match template(script) {
        Template::P2pk | Template::P2tr => Exposure::Exposed,
        Template::P2pkh | Template::P2wpkh => Exposure::HashedUntilSpent,
        Template::P2sh | Template::P2wsh => Exposure::DependsOnScript,
        Template::NullData => Exposure::NoKey,
        Template::Other => Exposure::Unknown,
    }
}

/// Exposure of an Ethereum account.
#[must_use]
pub fn ethereum_exposure(is_contract: bool, nonce: u64) -> Exposure {
    if is_contract {
        Exposure::NoKey
    } else if nonce > 0 {
        Exposure::Exposed
    } else {
        Exposure::HashedUntilSpent
    }
}

/// The plain-language explanation shown to a user.
#[must_use]
pub fn explain(e: Exposure) -> &'static str {
    match e {
        Exposure::Exposed => {
            "Your public key is already on the blockchain. A large enough quantum computer could compute your private key from it. Move these funds to a fresh address you have never spent from, or to a post-quantum vault."
        }
        Exposure::HashedUntilSpent => {
            "Only a hash of your key is public. It stays hidden until you spend, and spending reveals it. Do not reuse this address after spending from it."
        }
        Exposure::DependsOnScript => {
            "This address hides a script. Whether keys are exposed depends on that script and on whether it has been spent before."
        }
        Exposure::NoKey => "No private key controls this: a contract, or an unspendable output.",
        Exposure::Unknown => {
            "This output uses a script this tool does not recognise; no assessment is made."
        }
    }
}

/// A Bitcoin transaction's id and output scripts, parsed from its legacy
/// (non-witness) serialization.
#[derive(Debug)]
pub struct Tx {
    /// Double-SHA256 of the serialization, in the usual reversed display order.
    pub txid_display: [u8; 32],
    /// `(value in satoshi, output script)` per output.
    pub outputs: Vec<(u64, Vec<u8>)>,
}

fn varint(b: &[u8], at: &mut usize) -> Option<u64> {
    let first = *b.get(*at)?;
    *at += 1;
    let (n, len) = match first {
        0xfd => (2, 2),
        0xfe => (4, 4),
        0xff => (8, 8),
        v => return Some(u64::from(v)),
    };
    let bytes = b.get(*at..*at + len)?;
    *at += n;
    let mut v = [0u8; 8];
    v[..len].copy_from_slice(bytes);
    Some(u64::from_le_bytes(v))
}

fn take<'a>(b: &'a [u8], at: &mut usize, n: usize) -> Option<&'a [u8]> {
    let s = b.get(*at..*at + n)?;
    *at += n;
    Some(s)
}

/// Parses a legacy-serialized transaction; `None` if malformed.
#[must_use]
pub fn parse(raw: &[u8]) -> Option<Tx> {
    let mut at = 4; // version
    let inputs = varint(raw, &mut at)?;
    for _ in 0..inputs {
        take(raw, &mut at, 36)?; // outpoint
        let len = usize::try_from(varint(raw, &mut at)?).ok()?;
        take(raw, &mut at, len + 4)?; // script sig + sequence
    }
    let n = varint(raw, &mut at)?;
    let mut outputs = Vec::new();
    for _ in 0..n {
        let value = u64::from_le_bytes(take(raw, &mut at, 8)?.try_into().ok()?);
        let len = usize::try_from(varint(raw, &mut at)?).ok()?;
        outputs.push((value, take(raw, &mut at, len)?.to_vec()));
    }
    take(raw, &mut at, 4)?; // lock time
    if at != raw.len() {
        return None;
    }
    let mut id: [u8; 32] = Sha256::digest(Sha256::digest(raw)).into();
    id.reverse();
    Some(Tx {
        txid_display: id,
        outputs,
    })
}
