//! Bitcoin transactions, parsed natively: legacy and segwit (BIP 144)
//! serialization, and the txid (double SHA-256 of the serialization without
//! witness data, BIP 141).
//!
//! Only what a bridge reads is kept: each output's value and script. Inputs
//! and witnesses are walked for their lengths and for the txid, not
//! interpreted — the bridge never judges whether a Bitcoin script is valid,
//! which is the SPV trust model (`maya-btc-spv`).

use crate::BridgeError;

/// A standard transaction is at most 100 kB of weight / 4; anything larger
/// is refused before it is walked.
pub const MAX_TX_BYTES: usize = 400_000;
/// Bounds that no standard transaction reaches, so a hostile count cannot
/// make the parser reserve memory it will never fill.
const MAX_ITEMS: u64 = 100_000;
/// `OP_RETURN`.
pub const OP_RETURN: u8 = 0x6a;

/// One transaction output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TxOut {
    /// Satoshis.
    pub value: u64,
    /// `scriptPubKey`.
    pub script: Vec<u8>,
}

/// A parsed transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BtcTx {
    /// The txid, in internal byte order.
    pub txid: [u8; 32],
    /// Outputs, in order.
    pub outputs: Vec<TxOut>,
    /// Previous outpoints spent: `(txid, vout)`.
    pub inputs: Vec<([u8; 32], u32)>,
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], BridgeError> {
        let end = self.at.checked_add(n).filter(|e| *e <= self.bytes.len());
        let end = end.ok_or(BridgeError::Malformed("transaction truncated"))?;
        let out = &self.bytes[self.at..end];
        self.at = end;
        Ok(out)
    }
    fn u8(&mut self) -> Result<u8, BridgeError> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32, BridgeError> {
        Ok(u32::from_le_bytes(
            self.take(4)?
                .try_into()
                .map_err(|_| BridgeError::Malformed("u32"))?,
        ))
    }
    fn u64(&mut self) -> Result<u64, BridgeError> {
        Ok(u64::from_le_bytes(
            self.take(8)?
                .try_into()
                .map_err(|_| BridgeError::Malformed("u64"))?,
        ))
    }
    /// Bitcoin's `CompactSize`, refusing non-minimal encodings.
    fn varint(&mut self) -> Result<u64, BridgeError> {
        let (value, minimum) = match self.u8()? {
            0xfd => (
                u64::from(u16::from_le_bytes(
                    self.take(2)?
                        .try_into()
                        .map_err(|_| BridgeError::Malformed("varint"))?,
                )),
                0xfd,
            ),
            0xfe => (u64::from(self.u32()?), 0x1_0000),
            0xff => (self.u64()?, 0x1_0000_0000),
            small => return Ok(u64::from(small)),
        };
        if value < minimum {
            return Err(BridgeError::Malformed("non-minimal CompactSize"));
        }
        Ok(value)
    }
    fn count(&mut self) -> Result<usize, BridgeError> {
        let n = self.varint()?;
        if n > MAX_ITEMS {
            return Err(BridgeError::Malformed("count out of range"));
        }
        usize::try_from(n).map_err(|_| BridgeError::Malformed("count"))
    }
    fn bytes(&mut self) -> Result<&'a [u8], BridgeError> {
        let n = self.count()?;
        self.take(n)
    }
}

/// Parses a serialized transaction and computes its txid.
///
/// # Errors
///
/// [`BridgeError::Malformed`] for anything that is not exactly one
/// transaction: truncation, trailing bytes, a segwit flag other than 1, a
/// segwit transaction with no witness data, non-minimal lengths.
pub fn parse(raw: &[u8]) -> Result<BtcTx, BridgeError> {
    if raw.len() > MAX_TX_BYTES {
        return Err(BridgeError::Malformed("transaction too large"));
    }
    let mut r = Reader { bytes: raw, at: 0 };
    let version = r.take(4)?;
    let segwit = raw.get(4) == Some(&0) && raw.len() > 5;
    if segwit {
        let (_marker, flag) = (r.u8()?, r.u8()?);
        if flag != 1 {
            return Err(BridgeError::Malformed("unknown segwit flag"));
        }
    }
    let io_start = r.at;
    let n_in = r.count()?;
    if n_in == 0 {
        return Err(BridgeError::Malformed("no inputs"));
    }
    let mut inputs = Vec::with_capacity(n_in.min(1_024));
    for _ in 0..n_in {
        let txid: [u8; 32] = r
            .take(32)?
            .try_into()
            .map_err(|_| BridgeError::Malformed("outpoint"))?;
        let vout = r.u32()?;
        let _script_sig = r.bytes()?;
        let _sequence = r.u32()?;
        inputs.push((txid, vout));
    }
    let n_out = r.count()?;
    let mut outputs = Vec::with_capacity(n_out.min(1_024));
    for _ in 0..n_out {
        let value = r.u64()?;
        let script = r.bytes()?.to_vec();
        outputs.push(TxOut { value, script });
    }
    let io_end = r.at;
    if segwit {
        let mut any_witness = false;
        for _ in 0..n_in {
            let items = r.count()?;
            any_witness |= items > 0;
            for _ in 0..items {
                r.bytes()?;
            }
        }
        if !any_witness {
            return Err(BridgeError::Malformed(
                "segwit serialization without witness",
            ));
        }
    }
    let lock_time = r.take(4)?;
    if r.at != raw.len() {
        return Err(BridgeError::Malformed(
            "trailing bytes after the transaction",
        ));
    }
    // The txid covers version ‖ inputs ‖ outputs ‖ lock_time, never witness.
    let stripped = [version, &raw[io_start..io_end], lock_time].concat();
    // A 64-byte transaction can pass for an inner Merkle node (the known
    // SPV forgery); no real transaction is that size.
    if stripped.len() == 64 {
        return Err(BridgeError::Malformed("64-byte transaction"));
    }
    Ok(BtcTx {
        txid: maya_btc_spv::sha256d(&stripped),
        outputs,
        inputs,
    })
}

/// The 32-byte payload of an `OP_RETURN <32 bytes>` output, if `script` is one.
#[must_use]
pub fn op_return_32(script: &[u8]) -> Option<[u8; 32]> {
    match script {
        [OP_RETURN, 32, payload @ ..] if payload.len() == 32 => payload.try_into().ok(),
        _ => None,
    }
}
