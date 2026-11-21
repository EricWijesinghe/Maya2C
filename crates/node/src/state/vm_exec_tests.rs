//! Lazy contract storage (ADR-142) against the whole-storage load it replaced.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_vm::host::HostState;

use super::*;

/// xorshift64*, so every case is reproducible and needs no dependency.
fn next(seed: &mut u64) -> u64 {
    *seed ^= *seed >> 12;
    *seed ^= *seed << 25;
    *seed ^= *seed >> 27;
    seed.wrapping_mul(0x2545_F491_4F6C_DD1D)
}

/// Up to `max` bytes from a four-letter alphabet, so keys collide, nest as
/// prefixes of each other, and include the empty key.
fn bytes(seed: &mut u64, max: u64) -> Vec<u8> {
    let len = next(seed) % (max + 1);
    (0..len)
        .map(|_| [0x00, 0x01, 0x7f, 0xff][(next(seed) % 4) as usize])
        .collect()
}

#[test]
fn lazy_storage_reads_exactly_what_the_whole_storage_load_did() {
    let contract = [7u8; 32];
    // Neighbours either side, so a lazy read that slipped past the contract's
    // prefix would find their keys.
    let (mut below, mut above) = (contract, contract);
    below[31] = 6;
    above[31] = 8;
    let owners = [below, contract, above];

    for round in 0..40u64 {
        let dir = tempfile::TempDir::new().unwrap();
        let db = StateDB::open(dir.path()).unwrap();
        let mut seed = 0x9E37_79B9_7F4A_7C15 ^ (round + 1);
        let mut probes = Vec::new();

        for _ in 0..60 {
            let owner = owners[(next(&mut seed) % 3) as usize];
            let key = bytes(&mut seed, 5);
            let value = bytes(&mut seed, 9);
            db.raw_put(&contract_storage_key(&owner, &key), &value)
                .unwrap();
            probes.push(key);
        }
        // This block's earlier writes: some over committed keys, some new.
        let mut overlay = Overlay::new();
        for _ in 0..20 {
            let owner = owners[(next(&mut seed) % 3) as usize];
            let key = if next(&mut seed).is_multiple_of(2) {
                probes[(next(&mut seed) % probes.len() as u64) as usize].clone()
            } else {
                bytes(&mut seed, 5)
            };
            let value = bytes(&mut seed, 9);
            std::sync::Arc::make_mut(&mut overlay.contract_storage)
                .insert((owner, key.clone()), value);
            probes.push(key);
        }
        for _ in 0..20 {
            probes.push(bytes(&mut seed, 6));
        }

        // The load `call_contract` did before ADR-142, verbatim.
        let mut whole = db.contract_storage(&contract).unwrap();
        for ((owner, key), value) in overlay.contract_storage.iter() {
            if owner == &contract {
                whole.insert(key.clone(), value.clone());
            }
        }
        let mut eager = ChainHost::new(1).with_storage(whole);
        let mut lazy = db
            .contract_host(&overlay, &[1u8; 32], &contract, BlockContext::at_height(1))
            .unwrap();

        // Nothing was copied in, not even this block's writes to the
        // contract: a copy per call is O(entries) work no gas price saw
        // (PR #549 review). They are read through the shared overlay.
        assert_eq!(lazy.preloaded(), 0, "round {round}");

        for key in &probes {
            assert_eq!(
                lazy.storage_get(&contract, key),
                eager.storage_get(&contract, key),
                "round {round}: key {key:02x?}"
            );
        }
        // And after the call writes some of them itself.
        for key in probes.iter().step_by(3) {
            let value = bytes(&mut seed, 9);
            lazy.storage_set(&contract, key.clone(), value.clone());
            eager.storage_set(&contract, key.clone(), value);
        }
        for key in &probes {
            assert_eq!(
                lazy.storage_get(&contract, key),
                eager.storage_get(&contract, key),
                "round {round}, after writes: key {key:02x?}"
            );
        }
        assert_eq!(lazy.writes(), eager.writes());
        assert_eq!(lazy.read_error(), None);
    }
}

#[test]
fn the_cap_checks_refuse_in_the_order_the_spec_states() {
    // GAS-1 before GAS-2: an over-cap limit is `GasAboveCap` even in a full
    // block, so the error says what the sender can fix.
    assert_eq!(
        admit_call_gas(11, 10, 20, 20),
        Err(NodeError::GasAboveCap { limit: 11, cap: 10 })
    );
    assert_eq!(
        admit_call_gas(10, 10, 20, 15),
        Err(NodeError::BlockGasExhausted {
            needed: 10,
            remaining: 5
        })
    );
    assert_eq!(admit_call_gas(5, 10, 20, 15), Ok(()));
    // A block that has somehow spent past its cap has nothing left, not a
    // wrapped enormous remainder.
    assert_eq!(
        admit_call_gas(1, 10, 20, 25),
        Err(NodeError::BlockGasExhausted {
            needed: 1,
            remaining: 0
        })
    );
}

#[test]
fn a_committed_key_is_cold_once_per_block_and_only_where_it_reaches_disk() {
    // GAS-4: the block's first read of a committed key pays the cold price;
    // a key the block wrote, or an applied call of the block already read,
    // or this call already read, is warm. Decided by the block, not a cache.
    let contract = [7u8; 32];
    let dir = tempfile::TempDir::new().unwrap();
    let db = StateDB::open(dir.path()).unwrap();
    for key in [b"a", b"b", b"c"] {
        db.raw_put(&contract_storage_key(&contract, key), b"v")
            .unwrap();
    }
    let mut overlay = Overlay::new();
    std::sync::Arc::make_mut(&mut overlay.contract_storage)
        .insert((contract, b"b".to_vec()), b"staged".to_vec());
    std::sync::Arc::make_mut(&mut overlay.vm_reads).insert((contract, b"c".to_vec()));
    let mut host = db
        .contract_host(&overlay, &[1u8; 32], &contract, BlockContext::at_height(1))
        .unwrap();
    assert!(
        host.touch_storage(&contract, b"a"),
        "first read of a committed key"
    );
    assert!(
        !host.touch_storage(&contract, b"a"),
        "this call read it already"
    );
    assert!(!host.touch_storage(&contract, b"b"), "the block wrote it");
    assert!(
        !host.touch_storage(&contract, b"c"),
        "an applied call read it"
    );
    host.storage_set(&contract, b"d".to_vec(), b"w".to_vec());
    assert!(!host.touch_storage(&contract, b"d"), "this call wrote it");
    // An absent key costs a probe too, so it is cold the first time.
    assert!(host.touch_storage(&contract, b"absent"));
    let (_, reads) = host.into_writes_and_reads();
    assert_eq!(
        reads.into_iter().collect::<Vec<_>>(),
        vec![b"a".to_vec(), b"absent".to_vec()]
    );
}

/// A module whose one function declares 1,025 locals: past
/// `MAX_LOCALS_PER_FUNCTION`, so `shape::scan` refuses it. What stored code
/// would look like after a release tightened a shape bound.
fn past_a_shape_bound() -> Vec<u8> {
    let mut code = b"\0asm\x01\0\0\0".to_vec();
    code.extend([1, 4, 1, 0x60, 0, 0]);
    code.extend([3, 2, 1, 0]);
    code.extend([10, 7, 1, 5, 1, 0x81, 0x08, 0x7e, 0x0b]);
    code
}

#[test]
fn a_cold_call_below_the_length_price_is_refused_without_the_shape_scan() {
    // 09 R-5: the cold price is `deploy_fuel(len)` plus the shape's, so a
    // `gas_limit` below `deploy_fuel(len)` is out of gas whatever the shape.
    // The scan used to run first: on this code it refused with the scan's
    // error instead, after reading every operator.
    let dir = tempfile::TempDir::new().unwrap();
    let db = StateDB::open(dir.path()).unwrap();
    let code = past_a_shape_bound();
    let contract = [9u8; 32];
    db.raw_put(&code_key(&contract), &code).unwrap();
    let length_price = gas::deploy_fuel(code.len());
    let call_with = |gas_limit| ContractCall {
        contract,
        input: Vec::new(),
        gas_limit,
    };

    let mut overlay = Overlay::new();
    let refused = db.call_contract(
        &mut overlay,
        &[1u8; 32],
        &call_with(length_price - 1),
        BlockContext::at_height(1),
    );
    let out_of_gas = maya_vm::VmError::OutOfGas {
        limit: length_price - 1,
    }
    .to_string();
    assert_eq!(refused, Err(NodeError::Vm(out_of_gas)));
    assert_eq!(
        overlay.vm_gas,
        gas::priced(gas::CALL_BASE_FUEL, 1, code.len() as u64),
        "the code read is counted, as before"
    );

    // At the length price the shape decides, so the scan runs.
    let mut overlay = Overlay::new();
    let scanned = db.call_contract(
        &mut overlay,
        &[1u8; 32],
        &call_with(length_price),
        BlockContext::at_height(1),
    );
    assert!(
        matches!(&scanned, Err(NodeError::Vm(message)) if message.contains("locals")),
        "{scanned:?}"
    );
    assert_eq!(overlay.vm_gas, length_price, "the scan it ran counts");
}

#[test]
fn a_cold_call_refused_after_the_shape_scan_counts_the_length_price() {
    // 09 R-5: a `gas_limit` from `deploy_fuel(len)` up to the shape's price
    // pays for the read and the scan, then is out of gas. It counted
    // `50,000 + len`, which the scan alone outran (1.12 ns per unit on a
    // 512 KiB body of `nop`, release, ADR-142 amendment 4). It now counts
    // the length price it reached.
    let dir = tempfile::TempDir::new().unwrap();
    let db = StateDB::open(dir.path()).unwrap();
    let code = wat::parse_str(
        r#"(module (memory (export "memory") 1)
            (func (export "invoke") (param i32) (result i64) (i64.const 0)))"#,
    )
    .unwrap();
    let contract = [9u8; 32];
    db.raw_put(&code_key(&contract), &code).unwrap();
    let length_price = gas::deploy_fuel(code.len());
    assert!(length_price < gas::cold_call_fuel(&code).unwrap());

    let mut overlay = Overlay::new();
    let refused = db.call_contract(
        &mut overlay,
        &[1u8; 32],
        &ContractCall {
            contract,
            input: Vec::new(),
            gas_limit: length_price,
        },
        BlockContext::at_height(1),
    );
    let out_of_gas = maya_vm::VmError::OutOfGas {
        limit: length_price,
    }
    .to_string();
    assert_eq!(refused, Err(NodeError::Vm(out_of_gas)));
    assert_eq!(overlay.vm_gas, length_price);
}
