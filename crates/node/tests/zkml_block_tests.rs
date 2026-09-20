//! zkML inside a block: an ONNX model proven off-chain, the proof checked by a
//! contract during L1 block execution, and the classification committed to
//! contract storage in the same atomic batch as everything else in the block.
//!
//! `crates/zkml-prover/tests/` covers the circuit and the verifier; `crates/vm/tests/` covers the
//! host function against a fake verifier. This is where the real verifier, the
//! real VM, and a real `StateDB` meet.
//!
//! Every block here is applied with `with_zkml_activation(0)`, because nothing
//! in the node ever sets it: [`ZKML_ACTIVATION_HEIGHT`] is `u64::MAX`, and
//! `the_node_leaves_verification_dark` pins that.

use std::path::PathBuf;

use custom_l1_node::core::payload::{ContractCall, ContractDeploy, derive_contract_id};
use custom_l1_node::core::{Block, BlockHeader, Transaction, TxKind};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, generate_signing_key};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::state::context::ZKML_ACTIVATION_HEIGHT;
use custom_l1_node::state::{Account, BlockContext, StateDB};
use maya_vm::zkml::{
    MAX_ZKML_PROOF_BYTES, MAX_ZKML_PUBLIC_INPUTS, MAX_ZKML_VK_BYTES, ZKML_MODEL_ID_LEN,
    verification_fuel,
};
use maya_zkml::model::QuantizedMlp;
use maya_zkml::{ZkmlError, verify};
use maya_zkml_prover::onnx;
use maya_zkml_prover::prove::{ProvingSetup, keygen, prove};
use tempfile::TempDir;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/zkml/classifier.onnx")
}

/// A contract that trusts exactly one model.
///
/// The model id is a data segment — fixed when the contract is deployed, so the
/// caller cannot choose which model it is judged by. Input is
/// `[vk_len u32][vk][proof_len u32][proof][5 × i32 public inputs]`. A proof that
/// verifies writes the class to `class`; one that does not writes `rejected`.
/// Both paths succeed: a bad proof is an answer, and the contract acts on it.
fn gate_contract(model_id: &[u8; ZKML_MODEL_ID_LEN]) -> Vec<u8> {
    let id: String = model_id.iter().map(|b| format!("\\{b:02x}")).collect();
    let wat = format!(
        r#"(module
            (import "env" "host_verify_zkml_proof"
                (func $verify (param i32 i32 i32 i32 i32 i32 i32) (result i32)))
            (import "env" "storage_write" (func $write (param i32 i32 i32 i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "{id}")
            (data (i32.const 64) "class")
            (data (i32.const 80) "rejected")
            (data (i32.const 96) "\01\00\00\00")
            (func (export "input_ptr") (result i32) (i32.const 4096))
            (func (export "input_cap") (result i32) (i32.const 40960))
            (func (export "invoke") (param i32) (result i64)
                (local $vk i32) (local $vk_len i32)
                (local $proof i32) (local $proof_len i32) (local $public i32)
                (local.set $vk_len (i32.load (i32.const 4096)))
                (local.set $vk (i32.const 4100))
                (local.set $proof_len (i32.load (i32.add (local.get $vk) (local.get $vk_len))))
                (local.set $proof (i32.add (i32.add (local.get $vk) (local.get $vk_len)) (i32.const 4)))
                (local.set $public (i32.add (local.get $proof) (local.get $proof_len)))
                (if (i32.eq (i32.const 1)
                        (call $verify (i32.const 0)
                            (local.get $vk) (local.get $vk_len)
                            (local.get $public) (i32.const 5)
                            (local.get $proof) (local.get $proof_len)))
                    (then (call $write (i32.const 64) (i32.const 5)
                        (i32.add (local.get $public) (i32.const 16)) (i32.const 4)))
                    (else (call $write (i32.const 80) (i32.const 8) (i32.const 96) (i32.const 4))))
                (i64.const 0)))"#
    );
    wat::parse_str(&wat).expect("wat")
}

fn input(vk: &[u8], proof: &[u8], public: &[i32]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(vk.len() as u32).to_le_bytes());
    out.extend_from_slice(vk);
    out.extend_from_slice(&(proof.len() as u32).to_le_bytes());
    out.extend_from_slice(proof);
    for value in public {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

fn block_of(transactions: Vec<Transaction>) -> Block {
    Block::new(
        BlockHeader {
            prev_hash: [0u8; 32],
            state_root: [0u8; 32],
            timestamp: 1_756_252_800,
            nonce: 0,
            difficulty_target: target_from_leading_zero_bits(0),
            tx_root: [0; 32],
        },
        transactions,
    )
}

fn signed(kind: TxKind, nonce: u64, key: &HybridSigningKey) -> Transaction {
    let mut tx = Transaction::with_kind(kind, nonce);
    tx.sign(key).expect("sign");
    tx
}

/// A deployed gate contract and a prover for the model it trusts.
struct Deployed {
    db: StateDB,
    _dir: TempDir,
    caller: HybridSigningKey,
    contract: [u8; 32],
    setup: ProvingSetup,
}

fn deployed() -> Deployed {
    let setup = keygen(&onnx::load(fixture()).expect("fixture")).expect("keygen");
    let caller = generate_signing_key().expect("keygen");
    let dir = TempDir::new().expect("temp dir");
    let db = StateDB::open(dir.path()).expect("open");
    db.put_account(
        &caller.address(),
        &Account {
            balance: 1_000,
            nonce: 0,
        },
    )
    .expect("fund");

    let code = gate_contract(&setup.model_id());
    let deploy = signed(
        TxKind::DeployContract(ContractDeploy { code: code.clone() }),
        0,
        &caller,
    );
    db.apply_block(&block_of(vec![deploy]), BlockContext::at_height(1))
        .expect("deploy");
    let contract = derive_contract_id(&caller.address(), 0, &code);

    Deployed {
        db,
        _dir: dir,
        caller,
        contract,
        setup,
    }
}

/// Enough gas for one verification plus the contract's own few hundred
/// instructions. Computed from the published price, not guessed.
fn gas_for(vk: &[u8], proof: &[u8]) -> u64 {
    verification_fuel(vk.len(), proof.len(), 5) + 1_000_000
}

fn call(
    d: &Deployed,
    input: Vec<u8>,
    gas_limit: u64,
    nonce: u64,
    context: BlockContext,
) -> Result<(), String> {
    let tx = signed(
        TxKind::CallContract(ContractCall {
            contract: d.contract,
            input,
            gas_limit,
        }),
        nonce,
        &d.caller,
    );
    d.db.apply_block(&block_of(vec![tx]), context)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn live(height: u64) -> BlockContext {
    BlockContext::at_height(height).with_zkml_activation(0)
}

fn stored(d: &Deployed, key: &[u8]) -> Option<Vec<u8>> {
    d.db.get_contract_storage(&d.contract, key).expect("read")
}

#[test]
fn a_proven_classification_lands_in_contract_storage_inside_a_block() {
    let d = deployed();
    let x = [127i8, -128, 5, 0];
    let (proof, class) = prove(&d.setup, &x).expect("prove");
    let public: Vec<i32> = x
        .iter()
        .map(|&v| i32::from(v))
        .chain([class as i32])
        .collect();
    let vk = d.setup.verifying_key();

    call(
        &d,
        input(vk, &proof, &public),
        gas_for(vk, &proof),
        1,
        live(2),
    )
    .expect("block applies");

    assert_eq!(
        stored(&d, b"class"),
        Some((class as u32).to_le_bytes().to_vec())
    );
    assert_eq!(stored(&d, b"rejected"), None);
}

#[test]
fn a_forged_class_is_rejected_by_the_contract_and_the_block_still_applies() {
    // The proof is genuine; the claim attached to it is not. The verifier says
    // 0, the contract records the rejection, and the block — which contains a
    // transaction someone crafted to lie — applies normally. Had the host
    // trapped instead, crafting a bad proof would be a way to void blocks.
    let d = deployed();
    let x = [127i8, -128, 5, 0];
    let (proof, class) = prove(&d.setup, &x).expect("prove");
    let forged = ((class + 1) % 3) as i32;
    let public: Vec<i32> = x.iter().map(|&v| i32::from(v)).chain([forged]).collect();
    let vk = d.setup.verifying_key();

    call(
        &d,
        input(vk, &proof, &public),
        gas_for(vk, &proof),
        1,
        live(2),
    )
    .expect("block applies");

    assert_eq!(stored(&d, b"class"), None);
    assert_eq!(stored(&d, b"rejected"), Some(1u32.to_le_bytes().to_vec()));
}

#[test]
fn a_valid_proof_for_a_different_model_is_rejected() {
    // A caller brings their own model and a perfectly valid proof for it. The
    // contract pinned a model id at deployment, the key does not hash to it, and
    // the proof — true of *some* model — says nothing about this one.
    let d = deployed();
    let original = d.setup.model().clone();
    let other = QuantizedMlp::new(
        original.inputs(),
        original.hidden(),
        original.classes(),
        vec![1; original.inputs() * original.hidden()],
        vec![0; original.hidden()],
        vec![1; original.hidden() * original.classes()],
        vec![0; original.classes()],
        original.shift(),
    )
    .expect("model");
    let impostor = keygen(&other).expect("keygen");
    let x = [1i8, 2, 3, 4];
    let (proof, class) = prove(&impostor, &x).expect("prove");
    assert_eq!(
        verify::verify(
            impostor.verifying_key(),
            &x.iter()
                .map(|&v| i64::from(v))
                .chain([class as i64])
                .collect::<Vec<_>>(),
            &proof
        ),
        Ok(true),
        "the impostor's proof is valid for the impostor's model"
    );

    let public: Vec<i32> = x
        .iter()
        .map(|&v| i32::from(v))
        .chain([class as i32])
        .collect();
    let vk = impostor.verifying_key();
    call(
        &d,
        input(vk, &proof, &public),
        gas_for(vk, &proof),
        1,
        live(2),
    )
    .expect("block applies");

    assert_eq!(stored(&d, b"class"), None);
    assert_eq!(stored(&d, b"rejected"), Some(1u32.to_le_bytes().to_vec()));
}

#[test]
fn the_node_leaves_verification_dark() {
    // The context every block in the node is built with. The call traps, the
    // block fails, and nothing is written — the same outcome on every node,
    // which is what "dark" has to mean for a consensus feature.
    assert_eq!(ZKML_ACTIVATION_HEIGHT, u64::MAX);
    assert!(!BlockContext::at_height(1_000_000).zkml_active());

    let d = deployed();
    let x = [127i8, -128, 5, 0];
    let (proof, class) = prove(&d.setup, &x).expect("prove");
    let public: Vec<i32> = x
        .iter()
        .map(|&v| i32::from(v))
        .chain([class as i32])
        .collect();
    let vk = d.setup.verifying_key();

    let error = call(
        &d,
        input(vk, &proof, &public),
        gas_for(vk, &proof),
        1,
        BlockContext::at_height(2),
    )
    .expect_err("verification is not available");
    assert!(
        error.contains("zkML verification is not available"),
        "{error}"
    );
    assert_eq!(stored(&d, b"class"), None);
    assert_eq!(stored(&d, b"rejected"), None);
}

#[test]
fn activation_takes_effect_at_its_height_and_not_before() {
    let d = deployed();
    let x = [0i8, 0, 0, 0];
    let (proof, class) = prove(&d.setup, &x).expect("prove");
    let public: Vec<i32> = x
        .iter()
        .map(|&v| i32::from(v))
        .chain([class as i32])
        .collect();
    let vk = d.setup.verifying_key();
    let at = |height| BlockContext::at_height(height).with_zkml_activation(10);

    assert!(
        call(
            &d,
            input(vk, &proof, &public),
            gas_for(vk, &proof),
            1,
            at(9)
        )
        .is_err()
    );
    call(
        &d,
        input(vk, &proof, &public),
        gas_for(vk, &proof),
        1,
        at(10),
    )
    .expect("live at 10");
    assert_eq!(
        stored(&d, b"class"),
        Some((class as u32).to_le_bytes().to_vec())
    );
}

#[test]
fn a_call_without_gas_for_verification_fails_and_writes_nothing() {
    let d = deployed();
    let x = [127i8, -128, 5, 0];
    let (proof, class) = prove(&d.setup, &x).expect("prove");
    let public: Vec<i32> = x
        .iter()
        .map(|&v| i32::from(v))
        .chain([class as i32])
        .collect();
    let vk = d.setup.verifying_key();

    // The VM's default budget: plenty for ordinary contracts, a fifteenth of
    // one verification.
    let error =
        call(&d, input(vk, &proof, &public), 10_000_000, 1, live(2)).expect_err("out of gas");
    assert!(error.to_lowercase().contains("gas"), "{error}");
    assert_eq!(stored(&d, b"class"), None);
    assert_eq!(stored(&d, b"rejected"), None);
}

#[test]
fn the_vm_and_the_verifier_agree_on_every_bound() {
    // The VM cannot import `maya-zkml` — it links no pairing library — so the
    // bounds are duplicated. A VM bound larger than the verifier's would copy
    // buffers the verifier then refuses; smaller, and valid proofs trap.
    assert_eq!(MAX_ZKML_VK_BYTES, verify::MAX_VK_BYTES);
    assert_eq!(MAX_ZKML_PROOF_BYTES, verify::MAX_PROOF_BYTES);
    assert_eq!(MAX_ZKML_PUBLIC_INPUTS, verify::MAX_PUBLIC_INPUTS);
    assert_eq!(ZKML_MODEL_ID_LEN, verify::MODEL_ID_LEN);
}

#[test]
fn choosing_an_activation_height_cannot_also_choose_mainnet() {
    use custom_l1_node::state::zkml::check_setup;
    assert_eq!(check_setup("mainnet", u64::MAX), Ok(()), "inert while dark");
    for chain in ["mainnet", "maya-mainnet"] {
        assert_eq!(check_setup(chain, 100), Err(ZkmlError::UntrustedSetup));
    }
    assert_eq!(check_setup("maya-testnet", 100), Ok(()));
}
