//! The dual KEM: ML-KEM and HQC together — ADR-009.
//!
//! ```text
//! ss = SHA3-256( "maya2c.dualkem.v1" ‖ id
//!              ‖ ss_mlkem ‖ ss_hqc
//!              ‖ ct_mlkem ‖ ct_hqc
//!              ‖ ek_mlkem ‖ ek_hqc )
//! ```
//!
//! Lattices and codes fail independently, so an adversary who breaks one
//! still faces the other: the output is a PRF of both secrets. Both
//! ciphertexts are bound (the brief), and so are both encapsulation keys — the
//! X-Wing argument, which stops a ciphertext for one key being replayed
//! against another when a component KEM is not ciphertext-binding.
//!
//! Wire format: `ek = ek_mlkem ‖ ek_hqc`, `ct = ct_mlkem ‖ ct_hqc`.

use core::marker::PhantomData;

use sha3::{Digest as _, Sha3_256};
use zeroize::ZeroizeOnDrop;

use super::{
    Hqc128, Hqc256, KemId, KemSuite, KemSuiteError, MlKem768, MlKem1024, SharedSecret, check_len,
};

const DOMAIN: &[u8] = b"maya2c.dualkem.v1";

/// Both decapsulation keys, and the combined encapsulation key the combiner
/// hashes on the receiving side.
pub struct DualKey<A: KemSuite, B: KemSuite> {
    first: A::DecapsulationKey,
    second: B::DecapsulationKey,
    encapsulation_key: Vec<u8>,
}

// Both key fields are `ZeroizeOnDrop` by the trait bound; the encapsulation
// key is public.
impl<A: KemSuite, B: KemSuite> ZeroizeOnDrop for DualKey<A, B> {}

impl<A: KemSuite, B: KemSuite> core::fmt::Debug for DualKey<A, B> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("DualKey(<redacted>)")
    }
}

/// The combiner over two suites, tagged with its own id.
pub struct Dual<A, B, const ID: u8>(PhantomData<(A, B)>);

/// Dual KEM: ML-KEM-768 + HQC-128.
pub type DualKem768Hqc128 = Dual<MlKem768, Hqc128, 0x31>;
/// Dual KEM: ML-KEM-1024 + HQC-256.
pub type DualKem1024Hqc256 = Dual<MlKem1024, Hqc256, 0x32>;

const fn id_of(byte: u8) -> KemId {
    match byte {
        0x31 => KemId::DualKem768Hqc128,
        0x32 => KemId::DualKem1024Hqc256,
        _ => panic!("not a DualKem id"),
    }
}

fn combine(
    id: KemId,
    secrets: [&SharedSecret; 2],
    cts: [&[u8]; 2],
    eks: [&[u8]; 2],
) -> SharedSecret {
    let mut hasher = Sha3_256::new();
    hasher.update(DOMAIN);
    hasher.update([id.to_byte()]);
    for part in secrets {
        hasher.update(part.as_bytes());
    }
    for part in cts.into_iter().chain(eks) {
        hasher.update(part);
    }
    SharedSecret::from_slice(&hasher.finalize())
}

impl<A: KemSuite, B: KemSuite, const ID: u8> KemSuite for Dual<A, B, ID> {
    const ID: KemId = id_of(ID);
    const ENCAPSULATION_KEY_LEN: usize = A::ENCAPSULATION_KEY_LEN + B::ENCAPSULATION_KEY_LEN;
    const CIPHERTEXT_LEN: usize = A::CIPHERTEXT_LEN + B::CIPHERTEXT_LEN;
    type DecapsulationKey = DualKey<A, B>;

    fn generate() -> Result<(DualKey<A, B>, Vec<u8>), KemSuiteError> {
        let (first, mut ek) = A::generate()?;
        let (second, ek_b) = B::generate()?;
        ek.extend_from_slice(&ek_b);
        let key = DualKey {
            first,
            second,
            encapsulation_key: ek.clone(),
        };
        Ok((key, ek))
    }

    fn encapsulate(ek: &[u8]) -> Result<(Vec<u8>, SharedSecret), KemSuiteError> {
        check_len(
            ek.len(),
            Self::ENCAPSULATION_KEY_LEN,
            KemSuiteError::EncapsulationKey(Self::ID),
        )?;
        let (ek_a, ek_b) = ek.split_at(A::ENCAPSULATION_KEY_LEN);
        let (ct_a, ss_a) = A::encapsulate(ek_a)?;
        let (ct_b, ss_b) = B::encapsulate(ek_b)?;
        let ss = combine(Self::ID, [&ss_a, &ss_b], [&ct_a, &ct_b], [ek_a, ek_b]);
        let mut ct = ct_a;
        ct.extend_from_slice(&ct_b);
        Ok((ct, ss))
    }

    fn decapsulate(key: &DualKey<A, B>, ct: &[u8]) -> Result<SharedSecret, KemSuiteError> {
        check_len(
            ct.len(),
            Self::CIPHERTEXT_LEN,
            KemSuiteError::Ciphertext(Self::ID),
        )?;
        let (ct_a, ct_b) = ct.split_at(A::CIPHERTEXT_LEN);
        let ss_a = A::decapsulate(&key.first, ct_a)?;
        let ss_b = B::decapsulate(&key.second, ct_b)?;
        let (ek_a, ek_b) = key.encapsulation_key.split_at(A::ENCAPSULATION_KEY_LEN);
        Ok(combine(
            Self::ID,
            [&ss_a, &ss_b],
            [ct_a, ct_b],
            [ek_a, ek_b],
        ))
    }
}

/// The combiner alone, for tests that need to show one component's secret
/// changes the output.
#[cfg(test)]
pub(crate) fn combine_for_test(
    id: KemId,
    secrets: [&SharedSecret; 2],
    cts: [&[u8]; 2],
    eks: [&[u8]; 2],
) -> SharedSecret {
    combine(id, secrets, cts, eks)
}
