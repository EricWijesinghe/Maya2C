//! HW-3 G0 differential suite (ADR-137; threat model hw-3, replay tests 1-8).
//!
//! Every test runs the same items through `maya-crypto-pq` (the verdict) and
//! through the split path on each backend: always the CPU reference, and every
//! GPU adapter wgpu can reach when built with `--features gpu`. A backend must
//! reproduce every intermediate word for word and finish on the CPU's verdict.
//!
//! Without the feature, or without an adapter, the GPU half prints `MISSING`
//! and is not run; the CPU half still is, because it is what a GPU answer is
//! compared against and a wrong reference makes every comparison worthless.
//!
//! Not built: replay test 2 (SLH-DSA-SHA2-128s). G0 covers the ML-DSA-65
//! half only; see the crate documentation.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_crypto_pq::suite::{MasterSeed, MlDsa65, SignatureSuite};
use maya_wgpu_verify::arith::{inv_ntt, mul_mod, negacyclic_mul, ntt};
use maya_wgpu_verify::lattice::{
    LatticeInput, LatticeOutput, MAX_DISPATCH_ITEMS, Stage, decode_outputs, encode_outputs,
    lattice_on_cpu,
};
use maya_wgpu_verify::params::{
    BETA, C_TILDE_LEN, GAMMA1, K, L, N, OMEGA, PUBLIC_KEY_LEN, Q, SIGNATURE_LEN, ZETA, ZETAS_MONT,
};
use maya_wgpu_verify::{
    Assisted, CpuLattice, Item, LatticeBackend, VerifyError, batch_verify, mldsa, shader,
};

/// The CPU reference, then every GPU adapter (with `--features gpu`).
fn backends() -> Vec<Box<dyn LatticeBackend>> {
    let mut list: Vec<Box<dyn LatticeBackend>> = vec![Box::new(CpuLattice)];
    #[cfg(feature = "gpu")]
    {
        use maya_wgpu_verify::gpu::GpuLattice;
        let adapters = GpuLattice::adapters();
        if adapters.is_empty() {
            eprintln!("MISSING: no GPU adapter; the GPU half of this test did not run");
        }
        for index in 0..adapters.len() {
            // An adapter that exists but cannot open is a failure, not a skip.
            list.push(Box::new(GpuLattice::new(index).expect("open adapter")));
        }
    }
    #[cfg(not(feature = "gpu"))]
    eprintln!("MISSING: built without `--features gpu`; the GPU half of this test did not run");
    list
}

/// `n` valid signatures over distinct messages, from four keys.
fn signed_batch(n: usize) -> Vec<Item> {
    let keys: Vec<_> = (0u8..4)
        .map(|seed| MlDsa65::signing_key_from_seed(&MasterSeed::from_bytes([seed; 32])))
        .collect();
    (0..n)
        .map(|i| {
            let key = &keys[i % keys.len()];
            let message = format!("hw-3 g0 item {i}").into_bytes();
            Item {
                public_key: MlDsa65::public_key(key),
                signature: MlDsa65::sign(key, &message).expect("sign"),
                message,
                context: Vec::new(),
            }
        })
        .collect()
}

fn flip(bytes: &mut [u8], position: usize) {
    bytes[position] ^= 0x01;
}

/// Runs `items` on every backend and asserts agreement; returns the verdicts.
fn agree_everywhere(items: &[Item]) -> Vec<bool> {
    let mut verdicts = None;
    for mut backend in backends() {
        let results = batch_verify(backend.as_mut(), items);
        assert_eq!(results.len(), items.len());
        for (index, result) in results.iter().enumerate() {
            assert!(
                matches!(result.assisted, Assisted::Agrees | Assisted::NotSent),
                "{}: item {index}: {result:?}",
                backend.name()
            );
        }
        verdicts = Some(results.iter().map(|r| r.verdict).collect());
    }
    verdicts.expect("the CPU backend always runs")
}

// ------------------------------------------------------------- replay tests

#[test]
fn acvp_sigver_vectors_agree_with_the_cpu_for_ml_dsa_65() {
    let file: serde_json::Value = serde_json::from_str(include_str!(
        "../../../crates/crypto-pq/tests/vectors/acvp/ML-DSA-sigVer-FIPS204.json"
    ))
    .expect("vector JSON");
    let field = |t: &serde_json::Value, f: &str| hex::decode(t[f].as_str().expect(f)).expect("hex");
    let mut items = Vec::new();
    let mut expected = Vec::new();
    for group in file["testGroups"].as_array().expect("groups") {
        if group["parameterSet"] != "ML-DSA-65" || group["signatureInterface"] != "external" {
            continue;
        }
        for t in group["tests"].as_array().expect("tests") {
            items.push(Item {
                public_key: field(t, "pk"),
                message: field(t, "message"),
                context: field(t, "context"),
                signature: field(t, "signature"),
            });
            expected.push(t["testPassed"].as_bool().expect("testPassed"));
        }
    }
    assert!(items.len() >= 15, "only {} vectors", items.len());
    assert!(expected.contains(&true) && expected.contains(&false));
    assert_eq!(agree_everywhere(&items), expected);
}

#[test]
fn every_single_field_mutation_gets_the_cpu_answer() {
    let base = signed_batch(1).remove(0);
    let mut items = vec![base.clone()];
    let mut sig_positions: Vec<usize> = vec![0, C_TILDE_LEN - 1];
    for l in 0..L {
        sig_positions.push(C_TILDE_LEN + l * 640);
        sig_positions.push(C_TILDE_LEN + l * 640 + 639);
    }
    sig_positions.extend(C_TILDE_LEN + L * 640..SIGNATURE_LEN);
    for p in sig_positions {
        let mut m = base.clone();
        flip(&mut m.signature, p);
        items.push(m);
    }
    for p in [0, 31].into_iter().chain((0..K).map(|k| 32 + k * 320)) {
        let mut m = base.clone();
        flip(&mut m.public_key, p);
        items.push(m);
    }
    let mut m = base.clone();
    m.message.push(0);
    items.push(m);
    let mut m = base.clone();
    m.context.push(0);
    items.push(m);

    let verdicts = agree_everywhere(&items);
    assert!(verdicts[0], "the unmutated signature must verify");
    assert!(verdicts[1..].iter().all(|v| !v), "a mutation verified");
}

/// Writes the 20-bit encoding of `z` into coefficient `n` of response poly `l`.
fn set_z(sig: &mut [u8], l: usize, n: usize, z: i32) {
    let raw = u32::try_from(GAMMA1 - z).expect("in range");
    let start = (C_TILDE_LEN + l * 640) * 8 + n * 20;
    for bit in 0..20 {
        let (byte, shift) = ((start + bit) / 8, (start + bit) % 8);
        sig[byte] = (sig[byte] & !(1 << shift)) | (u8::from((raw >> bit) & 1 == 1) << shift);
    }
}

#[test]
fn a_response_at_the_norm_bound_and_one_past_it_agree_with_the_cpu() {
    let base = signed_batch(1).remove(0);
    let bound = GAMMA1 - BETA;
    let mut items = Vec::new();
    for (z, inside) in [
        (bound - 1, true),
        (bound, false),
        (bound + 1, false),
        (-(bound - 1), true),
        (-bound, false),
        (GAMMA1, false),
        (-GAMMA1 + 1, false),
    ] {
        let mut m = base.clone();
        set_z(&mut m.signature, L - 1, N - 1, z);
        let prepared = mldsa::prepare(&m).expect("decodes");
        assert_eq!(mldsa::z_norm_ok(&prepared), inside, "z = {z}");
        items.push(m);
    }
    assert!(agree_everywhere(&items).iter().all(|v| !v));
}

#[test]
fn malformed_hints_and_wrong_lengths_agree_with_the_cpu() {
    let base = signed_batch(1).remove(0);
    let hint = C_TILDE_LEN + L * 640;
    let count = |k: usize| hint + OMEGA + k;
    let total = usize::from(base.signature[count(K - 1)]);
    let mut items = Vec::new();
    let mut edit = |f: &dyn Fn(&mut Item)| {
        let mut m = base.clone();
        f(&mut m);
        items.push(m);
    };
    edit(&|m| m.signature[count(K - 1)] = u8::try_from(OMEGA + 1).unwrap());
    edit(&|m| m.signature[count(K - 1)] = u8::try_from(OMEGA).unwrap());
    edit(&|m| m.signature[count(0)] = m.signature[count(1)].saturating_add(1));
    if total < OMEGA {
        edit(&|m| m.signature[hint + total] = 1);
    }
    if total >= 2 {
        edit(&|m| m.signature[hint + 1] = m.signature[hint]);
        edit(&|m| m.signature.swap(hint, hint + 1));
    }
    edit(&|m| m.signature.clear());
    edit(&|m| m.signature.truncate(SIGNATURE_LEN - 1));
    edit(&|m| m.signature.push(0));
    edit(&|m| m.public_key.truncate(PUBLIC_KEY_LEN - 1));
    edit(&|m| m.public_key.push(0));
    edit(&|m| m.context = vec![0; 256]);
    assert!(agree_everywhere(&items).iter().all(|v| !v));
}

#[test]
fn a_batch_result_carries_its_own_nonce_and_a_stale_buffer_is_refused() {
    let outputs: Vec<LatticeOutput> = (0..3).map(|i| lattice_on_cpu(&uniform_input(i))).collect();
    let words = encode_outputs(&outputs, 7);
    assert_eq!(decode_outputs(&words, 7, 3).expect("fresh"), outputs);
    assert!(matches!(
        decode_outputs(&words, 8, 3),
        Err(VerifyError::StaleBuffer(_))
    ));
    assert!(matches!(
        decode_outputs(&words, 7, 4),
        Err(VerifyError::StaleBuffer(_))
    ));
    let mut swapped = encode_outputs(&outputs, 7);
    swapped[1] = 2;
    assert!(matches!(
        decode_outputs(&swapped, 7, 3),
        Err(VerifyError::StaleBuffer(_))
    ));
}

#[test]
fn the_shader_has_no_parameter_set_argument() {
    let source = shader::source();
    assert!(source.contains(&format!("const K: u32 = {K}u;")));
    assert!(source.contains(&format!("const L: u32 = {L}u;")));
    assert!(source.contains(&format!("const Q: u32 = {Q}u;")));
    // Every number comes from Rust: the body declares no constant and no
    // pipeline-overridable value, and the only uniform is the batch header.
    assert!(!shader::BODY.contains("const "));
    assert!(!source.contains("override"));
    let job = shader::BODY
        .split("struct Job {")
        .nth(1)
        .and_then(|rest| rest.split('}').next())
        .expect("Job struct");
    let fields: Vec<&str> = job
        .split(',')
        .map(|f| f.split(':').next().unwrap_or("").trim())
        .filter(|f| !f.is_empty())
        .collect();
    assert_eq!(fields, ["count", "nonce", "pad0", "pad1"]);
    assert_eq!(source.matches("var<uniform>").count(), 1);
    for float in ["f32", "f16", "f64"] {
        assert!(!source.contains(float), "shader mentions {float}");
    }
}

/// A backend whose device is gone.
struct LostDevice;

impl LatticeBackend for LostDevice {
    fn name(&self) -> String {
        "lost device".to_owned()
    }
    fn run(&mut self, _: &[LatticeInput]) -> Result<Vec<LatticeOutput>, VerifyError> {
        Err(VerifyError::Device("lost".to_owned()))
    }
}

#[test]
fn a_lost_device_returns_unavailable_not_a_verdict() {
    let mut items = signed_batch(3);
    flip(&mut items[1].signature, 0);
    items[2].signature.clear();
    let results = batch_verify(&mut LostDevice, &items);
    assert_eq!(
        results.iter().map(|r| r.verdict).collect::<Vec<_>>(),
        [true, false, false]
    );
    assert_eq!(results[0].assisted, Assisted::Unavailable);
    assert_eq!(results[1].assisted, Assisted::Unavailable);
    assert_eq!(results[2].assisted, Assisted::NotSent);
}

// ---------------------------------------------------------- further G0 tests

/// A backend that corrupts one word of `w_approx` (threat model G-1, G-2).
struct Liar;

impl LatticeBackend for Liar {
    fn name(&self) -> String {
        "liar".to_owned()
    }
    fn run(&mut self, inputs: &[LatticeInput]) -> Result<Vec<LatticeOutput>, VerifyError> {
        let mut out = CpuLattice.run(inputs)?;
        for o in &mut out {
            o.w_approx[0][0] = (o.w_approx[0][0] + 1) % Q;
        }
        Ok(out)
    }
}

#[test]
fn a_lying_backend_is_caught_and_changes_no_verdict() {
    let mut items = signed_batch(2);
    flip(&mut items[1].signature, 0);
    let results = batch_verify(&mut Liar, &items);
    assert!(results[0].verdict);
    assert!(!results[1].verdict);
    for r in &results {
        assert_eq!(r.assisted, Assisted::IntermediateMismatch(Stage::WApprox));
    }
}

/// Records the largest batch it was handed.
#[derive(Default)]
struct Recorder(usize, usize);

impl LatticeBackend for Recorder {
    fn name(&self) -> String {
        "recorder".to_owned()
    }
    fn run(&mut self, inputs: &[LatticeInput]) -> Result<Vec<LatticeOutput>, VerifyError> {
        self.0 = self.0.max(inputs.len());
        self.1 += 1;
        CpuLattice.run(inputs)
    }
}

#[test]
fn bad_signatures_at_first_middle_last_and_chunk_boundaries_are_found_per_item() {
    let n = 2 * MAX_DISPATCH_ITEMS + 3;
    let bad = [
        0,
        MAX_DISPATCH_ITEMS - 1,
        MAX_DISPATCH_ITEMS,
        n / 2,
        2 * MAX_DISPATCH_ITEMS - 1,
        2 * MAX_DISPATCH_ITEMS,
        n - 1,
    ];
    let mut items = signed_batch(n);
    for &i in &bad {
        flip(&mut items[i].signature, C_TILDE_LEN + 7);
    }
    let expected: Vec<bool> = (0..n).map(|i| !bad.contains(&i)).collect();
    assert_eq!(agree_everywhere(&items), expected);

    let mut recorder = Recorder::default();
    let _ = batch_verify(&mut recorder, &items);
    assert_eq!(
        recorder.0, MAX_DISPATCH_ITEMS,
        "a dispatch exceeded the bound"
    );
    assert_eq!(recorder.1, n.div_ceil(MAX_DISPATCH_ITEMS));
}

/// Every coefficient of every input set to `v`.
fn uniform_input(v: u32) -> LatticeInput {
    LatticeInput {
        z: [[v; N]; L],
        c: [v; N],
        t1: [[v; N]; K],
        a_hat: [[[v; N]; L]; K],
    }
}

#[test]
fn edge_coefficients_0_and_q_minus_1_agree_between_cpu_and_gpu() {
    let mut alternating = uniform_input(0);
    for (i, w) in alternating.z.iter_mut().flatten().enumerate() {
        *w = if i % 2 == 0 { 0 } else { Q - 1 };
    }
    alternating.c[0] = Q - 1;
    alternating.c[N - 1] = 1;
    let inputs = [
        uniform_input(0),
        uniform_input(Q - 1),
        uniform_input(1),
        alternating,
    ];
    let reference: Vec<LatticeOutput> = inputs.iter().map(lattice_on_cpu).collect();
    for out in &reference {
        let all = out
            .z_hat
            .iter()
            .chain(&out.t1_hat)
            .chain(&out.w_approx)
            .flatten();
        assert!(all.chain(&out.c_hat).all(|&w| w < Q), "non-canonical word");
    }
    for mut backend in backends() {
        assert_eq!(
            backend.run(&inputs).expect("runs"),
            reference,
            "{}",
            backend.name()
        );
    }
}

// ------------------------------------------------------ the CPU reference

/// A deterministic polynomial; the generator is not the point.
fn poly(seed: u32) -> [u32; N] {
    let mut x = seed;
    core::array::from_fn(|_| {
        x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        x % Q
    })
}

#[test]
fn the_ntt_multiplies_like_schoolbook_negacyclic_multiplication() {
    for seed in [1, 2, 3] {
        let (a, b) = (poly(seed), poly(seed + 100));
        let (a_hat, b_hat) = (ntt(&a), ntt(&b));
        let product: [u32; N] = core::array::from_fn(|i| mul_mod(a_hat[i], b_hat[i]));
        assert_eq!(inv_ntt(&product), negacyclic_mul(&a, &b));
        assert_eq!(inv_ntt(&a_hat), a);
    }
    let top = [Q - 1; N];
    assert_eq!(inv_ntt(&ntt(&top)), top);
}

/// FIPS 204's zeta table derived independently of `params::zetas_mont`:
/// walk the powers of `zeta` once and scatter each to its bit-reversed slot,
/// the way `fips204::helpers::gen_zeta_table_mont` builds `ZETA_TABLE_MONT`
/// (that table is `pub(crate)`, and `crypto-pq` has no NTT of its own, so
/// the test cannot read it). Montgomery factor `2^32`, as in the CPU code.
const fn reference_zetas_mont() -> [u32; N] {
    let mut table = [0u32; N];
    let mut power: u64 = 1;
    let mut i = 0;
    while i < N {
        table[(i as u8).reverse_bits() as usize] = ((power << 32) % Q as u64) as u32;
        power = power * ZETA as u64 % Q as u64;
        i += 1;
    }
    table
}

#[test]
fn the_zeta_table_matches_the_fips_204_reference_values() {
    let reference = reference_zetas_mont();
    for (i, (&shader, &expected)) in ZETAS_MONT.iter().zip(reference.iter()).enumerate() {
        assert_eq!(shader, expected, "zeta {i}");
    }
    // Anchors from fips204 0.4.6's own `helpers::tests::check_zeta`, so a
    // mistake shared by both derivations above still fails.
    assert_eq!(&reference[..3], &[4_193_792, 25_847, 5_771_523]);
    let mut power = 1u64;
    for _ in 0..256 {
        power = power * u64::from(ZETA) % u64::from(Q);
    }
    assert_eq!(power, u64::from(Q - 1), "zeta must have order 512");
}
