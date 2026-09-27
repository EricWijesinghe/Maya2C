//! solana-sbpf as a hosted engine: SBF programs, interpreted, metered.
//!
//! A program receives one input region holding the calling Maya account's
//! balance (u64, little-endian) — the unified state mapping's SBF face — and
//! returns r0. Compute units are one per instruction, converted to fuel by
//! [`crate::gas::FUEL_PER_SBF_CU`]. Interpreter only: the JIT is not a
//! consensus-grade tier on every platform, and Windows has none.

use std::ptr;
use std::sync::Arc;

use solana_sbpf::aligned_memory::AlignedMemory;
use solana_sbpf::assembler::assemble;
use solana_sbpf::ebpf;
use solana_sbpf::error::ProgramResult;
use solana_sbpf::memory_region::{MemoryMapping, MemoryRegion};
use solana_sbpf::program::BuiltinProgram;
use solana_sbpf::verifier::RequisiteVerifier;
use solana_sbpf::vm::{CallFrame, ContextObject, EbpfVm, ExecutionMode};

use crate::MultiVmError;
use crate::gas::sbf_cu_to_fuel;

/// What one SBF run did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SbfOutcome {
    /// r0 at exit.
    pub result: u64,
    /// Compute units consumed.
    pub compute_units: u64,
    /// The same, in Maya VM fuel.
    pub fuel: u64,
}

/// Instruction meter and the active memory mapping, as the VM requires.
struct Meter {
    remaining: u64,
    mapping: MemoryMapping,
}

impl ContextObject for Meter {
    fn consume(&mut self, amount: u64) {
        self.remaining = self.remaining.saturating_sub(amount);
    }

    fn get_remaining(&self) -> u64 {
        self.remaining
    }

    fn active_mapping_ptr(&mut self) -> ptr::NonNull<MemoryMapping> {
        ptr::NonNull::from(&mut self.mapping)
    }
}

fn err(e: impl std::fmt::Debug) -> MultiVmError {
    MultiVmError::Engine {
        engine: "solana-sbpf",
        message: format!("{e:?}"),
    }
}

/// Assembles, verifies and runs `program` (SBF assembly) with `balance` as
/// the caller's account, within `budget` compute units.
///
/// # Errors
///
/// Assembly or verification failed, the budget ran out, or the program
/// faulted.
pub fn run(program: &str, balance: u64, budget: u64) -> Result<SbfOutcome, MultiVmError> {
    let loader = Arc::new(BuiltinProgram::<Meter>::new_mock());
    let executable = assemble::<Meter>(program, loader).map_err(err)?;
    executable.verify::<RequisiteVerifier>().map_err(err)?;
    let config = executable.get_config();
    let mut stack = AlignedMemory::<{ ebpf::HOST_ALIGN }>::zero_filled(config.stack_size());
    let stack_len = stack.len();
    let mut heap = AlignedMemory::<{ ebpf::HOST_ALIGN }>::with_capacity(0);
    let mut input = balance.to_le_bytes().to_vec();
    let regions = vec![
        executable.get_ro_region(),
        MemoryRegion::new(&raw mut *stack.as_slice_mut(), ebpf::MM_STACK_START),
        MemoryRegion::new(&raw mut *heap.as_slice_mut(), ebpf::MM_HEAP_START),
        MemoryRegion::new(&raw mut input[..], ebpf::MM_INPUT_START),
    ];
    // SAFETY: `MemoryMapping::new` requires every region's host memory to
    // outlive the mapping and not be aliased while the VM runs. `stack`,
    // `heap` and `input` are locals of this function, borrowed only through
    // these raw regions, and the mapping (inside `meter`) and the VM are
    // dropped before them at the end of this scope. The read-only region
    // points into `executable`, which also outlives both. Exercised by
    // `tests/multivm_tests.rs` (normal exit, out-of-budget, bad memory access).
    let mapping = unsafe { MemoryMapping::new(regions, config, executable.get_sbpf_version()) }
        .map_err(err)?;
    let mut meter = Meter {
        remaining: budget,
        mapping,
    };
    let mut vm = EbpfVm::new(
        executable.get_loader().clone(),
        executable.get_sbpf_version(),
        &mut meter,
        stack_len,
    );
    vm.registers[1] = ebpf::MM_INPUT_START;
    let mut frames = vec![CallFrame::default(); config.max_call_depth];
    let (instructions, result) =
        vm.execute_program(&executable, &mut ExecutionMode::Interpreted, &mut frames);
    match result {
        ProgramResult::Ok(value) => Ok(SbfOutcome {
            result: value,
            compute_units: instructions,
            fuel: sbf_cu_to_fuel(instructions),
        }),
        ProgramResult::Err(e) => Err(MultiVmError::Failed {
            engine: "solana-sbpf",
            message: format!("{e:?}"),
        }),
    }
}
