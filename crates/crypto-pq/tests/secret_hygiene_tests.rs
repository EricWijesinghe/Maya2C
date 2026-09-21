//! Secret-memory hygiene, checked by the compiler where it can be.
//!
//! Every secret type this crate hands out must be wiped on drop, must not be
//! `Copy` (an implicit copy is a copy nobody wipes), and must not be
//! `Display`. `Debug` is allowed only redacted, and the runtime tests below
//! check the redaction. Timing is `benches/dudect.rs`.

use maya_crypto_pq::kem_suite::{self, KemSuite};
use maya_crypto_pq::suite::{self, MasterSeed, SignatureSuite};
use static_assertions::{assert_impl_all, assert_not_impl_any};
use zeroize::ZeroizeOnDrop;

type MlDsa87Key = <suite::MlDsa87 as SignatureSuite>::SigningKey;
type SlhKey = <suite::SlhDsaShake256f as SignatureSuite>::SigningKey;
type Ed25519Key = <suite::Ed25519 as SignatureSuite>::SigningKey;
type DualKey = <kem_suite::DualKem1024Hqc256 as KemSuite>::DecapsulationKey;
type XWingKey = <kem_suite::XWing as KemSuite>::DecapsulationKey;

assert_not_impl_any!(MasterSeed: core::fmt::Display, Copy, Clone);
assert_not_impl_any!(suite::HybridSigningKey: core::fmt::Display, Copy);
assert_not_impl_any!(MlDsa87Key: core::fmt::Display, Copy);
assert_not_impl_any!(SlhKey: core::fmt::Display, Copy);
assert_not_impl_any!(Ed25519Key: core::fmt::Display, Copy);
assert_not_impl_any!(DualKey: core::fmt::Display, Copy, Clone);
assert_not_impl_any!(XWingKey: core::fmt::Display, Copy, Clone);
assert_not_impl_any!(kem_suite::SharedSecret: core::fmt::Display, Copy, Clone, PartialEq);
assert_not_impl_any!(maya_crypto_pq::kem::SharedSecret: core::fmt::Display, Copy, Clone, PartialEq);
assert_not_impl_any!(maya_crypto_pq::hqc::SharedSecret: core::fmt::Display, Copy, Clone, PartialEq);
assert_not_impl_any!(maya_crypto_pq::kem::DecapsulationKey: core::fmt::Display, Copy, Clone);

assert_impl_all!(suite::HybridSigningKey: ZeroizeOnDrop);
assert_impl_all!(MlDsa87Key: ZeroizeOnDrop);
assert_impl_all!(SlhKey: ZeroizeOnDrop);
assert_impl_all!(Ed25519Key: ZeroizeOnDrop);
assert_impl_all!(DualKey: ZeroizeOnDrop);
assert_impl_all!(XWingKey: ZeroizeOnDrop);

#[test]
fn debug_output_is_redacted() {
    let seed = MasterSeed::from_bytes([0x5a; 32]);
    let hybrid = <suite::HybridMlDsa65SlhDsa128s as SignatureSuite>::signing_key_from_seed(&seed);
    let (dual, _) = kem_suite::DualKem768Hqc128::generate().expect("generate");
    let (xwing, ek) = kem_suite::XWing::generate().expect("generate");
    let (_, secret) = kem_suite::XWing::encapsulate(&ek).expect("encapsulate");

    for text in [
        format!("{seed:?}"),
        format!("{hybrid:?}"),
        format!("{dual:?}"),
        format!("{xwing:?}"),
        format!("{secret:?}"),
    ] {
        assert!(text.contains("redacted"), "{text}");
        assert!(!text.contains("5a") && !text.contains("90"), "{text}");
    }
}
