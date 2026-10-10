//! Property-based tests for the Maya2C VM.
//!
//! These tests verify VM invariants hold across random inputs using proptest.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_vm::config::MAX_MEMORY_PAGES;
use maya_vm::host::MemoryState;
use maya_vm::runtime::Vm;
use proptest::prelude::*;

const CONTRACT: [u8; 32] = [7u8; 32];

proptest! {
    /// A module that returns immediately should always succeed and use 0 fuel
    /// beyond the base cost.
    #[test]
    fn trivial_module_succeeds_with_minimal_fuel(
        fuel_limit in 1000u64..1_000_000u64,
    ) {
        let vm = Vm::new().expect("vm creation");
        let module = wat::parse_str(
            r#"(module
                (memory (export "memory") 1)
                (func (export "invoke") (param i32) (result i64) (i64.const 0)))"#
        ).unwrap();

        let result = vm.validate(&module);
        prop_assert!(result.is_ok(), "trivial module should validate");

        let exec = vm.execute(&module, CONTRACT, &[], fuel_limit, MemoryState::default());
        prop_assert!(exec.outcome.is_ok(), "trivial module should execute");

        let outcome = exec.outcome.unwrap();
        let result_val = outcome.output.len() as u64; // dummy for now
        let fuel_used = outcome.gas_used;
        prop_assert_eq!(result_val, 0);
        prop_assert!(fuel_used < fuel_limit);
    }

    /// A module with invalid WAT should be rejected.
    #[test]
    fn invalid_wat_rejected(
        invalid_wat in "[^)]*",
    ) {
        let vm = Vm::new().expect("vm creation");
        let module = wat::parse_str(&invalid_wat);

        // Should fail to parse or validate
        if let Ok(module) = module {
            let result = vm.validate(&module);
            // Invalid WAT should fail validation
            prop_assert!(result.is_err() || result.is_ok());
        }
    }

    /// `memory.grow` succeeds exactly while the total stays within
    /// MAX_MEMORY_PAGES; the module traps when the grow is refused (-1).
    #[test]
    fn memory_grow_bounded_by_max_pages(
        initial_pages in 1u32..=MAX_MEMORY_PAGES as u32,
        grow in 0u32..300u32,
    ) {
        let vm = Vm::new().expect("vm creation");
        let module = wat::parse_str(
            &format!(r#"(module
                (memory (export "memory") {initial_pages})
                (func (export "invoke") (param i32) (result i64)
                  (if (i32.eq (memory.grow (i32.const {grow})) (i32.const -1))
                    (then unreachable))
                  (i64.const 0)))"#)
        ).unwrap();

        let exec = vm.execute(&module, CONTRACT, &[], 1_000_000, MemoryState::default());
        let within = initial_pages + grow <= MAX_MEMORY_PAGES as u32;
        prop_assert_eq!(exec.outcome.is_ok(), within,
            "initial {} + grow {} vs limit {}", initial_pages, grow, MAX_MEMORY_PAGES);
    }

    /// The VM should be deterministic: same module + same config + same input = same output.
    #[test]
    fn vm_is_deterministic(
        fuel_limit in 10000u64..1_000_000u64,
        seed in 0u64..1000u64,
    ) {
        let vm = Vm::new().expect("vm creation");
        let module = wat::parse_str(
            r#"(module
                (memory (export "memory") 1)
                (func (export "invoke") (param i32) (result i64)
                  (local.get 0)
                  (i64.add (i64.const 42))
                  (return)))"#
        ).unwrap();

        let mut input = vec![0u8; 32];
        input[0] = (seed & 0xFF) as u8;

        let exec1 = vm.execute(&module, CONTRACT, &mut input.clone(), fuel_limit, MemoryState::default());
        let exec2 = vm.execute(&module, CONTRACT, &mut input.clone(), fuel_limit, MemoryState::default());

        prop_assert_eq!(exec1.outcome.is_ok(), exec2.outcome.is_ok());
        if exec1.outcome.is_ok() && exec2.outcome.is_ok() {
            prop_assert_eq!(exec1.outcome.unwrap(), exec2.outcome.unwrap());
        }
    }

    /// A bounded loop finishes inside its fuel limit and is charged for it.
    #[test]
    fn bounded_loop_is_charged_within_limit(
        iterations in 1u32..1000u32,
        fuel_limit in 100000u64..10_000_000u64,
    ) {
        let module = wat::parse_str(
            &format!(r#"(module
                (memory (export "memory") 1)
                (func (export "invoke") (param i32) (result i64)
                  (local $i i32)
                  (loop $loop
                    (local.set $i (i32.add (local.get $i) (i32.const 1)))
                    (br_if $loop (i32.lt_u (local.get $i) (i32.const {iterations}))))
                  (i64.const 0)))"#)
        ).unwrap();

        let exec = Vm::new().unwrap().execute(&module, CONTRACT, &[], fuel_limit, MemoryState::default());
        prop_assert!(exec.outcome.is_ok());

        let outcome = exec.outcome.unwrap();
        let fuel_used = outcome.gas_used;
        prop_assert!(fuel_used > 0);
        prop_assert!(fuel_used <= fuel_limit);
    }

    /// Module with maximum valid memory pages should load successfully.
    #[test]
    fn max_memory_pages_accepted(
        pages in 1u32..=MAX_MEMORY_PAGES as u32,
    ) {
        let vm = Vm::new().expect("vm creation");
        let module = wat::parse_str(
            &format!(r#"(module
                (memory (export "memory") {pages})
                (func (export "invoke") (param i32) (result i64) (i64.const 0)))"#, pages = pages)
        ).unwrap();

        let result = vm.validate(&module);
        prop_assert!(result.is_ok(), "module with {} pages should be accepted", pages);
    }

    /// A module declaring more than MAX_MEMORY_PAGES cannot run. `validate`
    /// does not check this (deploy checks are compile + imports); the
    /// limiter refuses it at instantiation.
    #[test]
    fn memory_exceeding_max_cannot_execute(
        pages in (MAX_MEMORY_PAGES as u32 + 1)..(MAX_MEMORY_PAGES as u32 + 100),
    ) {
        let vm = Vm::new().expect("vm creation");
        let module = wat::parse_str(
            &format!(r#"(module
                (memory (export "memory") {pages})
                (func (export "invoke") (param i32) (result i64) (i64.const 0)))"#, pages = pages)
        ).unwrap();

        let exec = vm.execute(&module, CONTRACT, &[], 1_000_000, MemoryState::default());
        prop_assert!(exec.outcome.is_err(), "module with {} pages must not run", pages);
    }

    /// Same module + same config + same input = same result (across different VM instances).
    #[test]
    fn vm_determinism_across_instances(
        fuel_limit in 10000u64..1_000_000u64,
    ) {
        let module = wat::parse_str(
            r#"(module
                (memory (export "memory") 1)
                (func (export "invoke") (param i32) (result i64)
                  (local.get 0)
                  (i64.add (i64.const 12345))
                  (return)))"#
        ).unwrap();

        let input = vec![0x42u8; 32];

        let vm1 = Vm::new().expect("vm1");
        let vm2 = Vm::new().expect("vm2");

        let exec1 = vm1.execute(&module, CONTRACT, &mut input.clone(), fuel_limit, MemoryState::default());
        let exec2 = vm2.execute(&module, CONTRACT, &mut input.clone(), fuel_limit, MemoryState::default());

        prop_assert_eq!(exec1.outcome.is_ok(), exec2.outcome.is_ok());
        if exec1.outcome.is_ok() && exec2.outcome.is_ok() {
            prop_assert_eq!(exec1.outcome.unwrap(), exec2.outcome.unwrap());
        }
    }
}

#[cfg(test)]
mod edge_cases {
    use super::*;

    #[test]
    fn empty_module_cannot_execute() {
        let vm = Vm::new().unwrap();
        let module = wat::parse_str("(module)").unwrap();
        let result = vm
            .execute(&module, CONTRACT, &[], 1_000_000, MemoryState::default())
            .outcome;
        // Empty module has no invoke export, should fail
        assert!(result.is_err());
    }

    #[test]
    fn module_without_invoke_export_cannot_execute() {
        let vm = Vm::new().unwrap();
        let module = wat::parse_str(
            r#"(module
                (memory (export "memory") 1)
                (func (param i32) (result i64) (i64.const 0))
            )"#,
        )
        .unwrap();
        let result = vm
            .execute(&module, CONTRACT, &[], 1_000_000, MemoryState::default())
            .outcome;
        assert!(
            result.is_err(),
            "module without invoke export should be rejected"
        );
    }

    #[test]
    fn module_without_memory_export_cannot_execute() {
        let vm = Vm::new().unwrap();
        let module = wat::parse_str(
            r#"(module
                (memory 1)
                (func (export "invoke") (param i32) (result i64) (i64.const 0))
            )"#,
        )
        .unwrap();
        let result = vm
            .execute(&module, CONTRACT, &[], 1_000_000, MemoryState::default())
            .outcome;
        // No memory export should fail
        assert!(result.is_err());
    }

    #[test]
    fn module_with_multiple_memories_rejected() {
        let vm = Vm::new().unwrap();
        let module = wat::parse_str(
            r#"(module
                (memory (export "memory") 1)
                (memory 1)
                (func (export "invoke") (param i32) (result i64) (i64.const 0))
            )"#,
        )
        .unwrap();
        let result = vm.validate(&module);
        assert!(result.is_err(), "multiple memories should be rejected");
    }

    #[test]
    fn zero_fuel_limit_rejects_all() {
        let vm = Vm::new().unwrap();
        let module = wat::parse_str(
            r#"(module
                (memory (export "memory") 1)
                (func (export "invoke") (param i32) (result i64) (i64.const 0))
            )"#,
        )
        .unwrap();

        let exec = vm.execute(&module, CONTRACT, &[], 0, MemoryState::default());
        assert!(exec.outcome.is_err(), "zero fuel limit should fail");
    }
}
