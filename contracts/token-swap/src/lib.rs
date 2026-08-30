//! Constant-product token swap, written for the Maya VM.
//!
//! Demonstrates the full contract surface: persistent storage, account queries,
//! block height, and events — with no standard library and no allocator.
//!
//! ## ABI
//!
//! ```text
//! input_ptr()     -> i32   address of the static input buffer
//! input_cap()     -> i32   its capacity
//! invoke(len:i32) -> i64   (out_ptr << 32) | out_len ; negative on failure
//! ```
//!
//! ## Calls
//!
//! | Byte 0 | Method | Arguments | Returns |
//! |---|---|---|---|
//! | 0 | `init` | `reserve_a: u64`, `reserve_b: u64` | nothing |
//! | 1 | `swap_a_for_b` | `amount_in: u64` | `amount_out: u64` |
//! | 2 | `swap_b_for_a` | `amount_in: u64` | `amount_out: u64` |
//! | 3 | `reserves` | none | `reserve_a: u64`, `reserve_b: u64` |
//!
//! All integers are little-endian.
//!
//! ## No allocator, by design
//!
//! A `#![no_std]` contract has no heap. Rather than pull in an allocator for
//! two buffers, the module reserves them statically and the host writes call
//! input directly into them. That keeps the module small — which matters when
//! every byte is stored on chain — and removes allocation failure as a class.

#![no_std]

use core::panic::PanicInfo;

/// Traps the guest. Reached only on an arithmetic overflow or explicit panic,
/// both of which must abort the call rather than continue with wrong numbers.
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}

// --- host imports ----------------------------------------------------------

// Without `wasm_import_module` the linker treats these as undefined symbols
// rather than WebAssembly imports, and the module fails to link. The name must
// match the module the host registers them under.
#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn storage_read(key_ptr: i32, key_len: i32, out_ptr: i32, out_cap: i32) -> i32;
    fn storage_write(key_ptr: i32, key_len: i32, val_ptr: i32, val_len: i32);
    fn emit_event(topic_ptr: i32, topic_len: i32, data_ptr: i32, data_len: i32);
    fn block_height() -> i64;
}

// --- static buffers --------------------------------------------------------

const INPUT_CAPACITY: usize = 256;
const OUTPUT_CAPACITY: usize = 256;

static mut INPUT: [u8; INPUT_CAPACITY] = [0; INPUT_CAPACITY];
static mut OUTPUT: [u8; OUTPUT_CAPACITY] = [0; OUTPUT_CAPACITY];
static mut SCRATCH: [u8; 32] = [0; 32];

const KEY_RESERVE_A: &[u8] = b"ra";
const KEY_RESERVE_B: &[u8] = b"rb";
const TOPIC_SWAP: &[u8] = b"swap";

/// Fee numerator: 997/1000 means a 0.3% fee retained by the pool.
const FEE_NUMERATOR: u128 = 997;
const FEE_DENOMINATOR: u128 = 1000;

// --- exports ---------------------------------------------------------------

/// Address of the input buffer the host writes into.
#[unsafe(no_mangle)]
pub extern "C" fn input_ptr() -> i32 {
    (&raw const INPUT) as usize as i32
}

/// Capacity of that buffer.
#[unsafe(no_mangle)]
pub extern "C" fn input_cap() -> i32 {
    INPUT_CAPACITY as i32
}

/// Entry point. See the module documentation for the call encoding.
#[unsafe(no_mangle)]
pub extern "C" fn invoke(len: i32) -> i64 {
    let length = if len < 0 { 0 } else { len as usize };
    if length == 0 || length > INPUT_CAPACITY {
        return -1;
    }

    // Explicit slice construction: indexing through a raw-pointer deref would
    // create an implicit autoref, which edition 2024 rejects as dangerous.
    let input = unsafe { core::slice::from_raw_parts((&raw const INPUT).cast::<u8>(), length) };
    let method = input[0];
    let args = &input[1..];

    match method {
        0 => match (read_u64(args, 0), read_u64(args, 8)) {
            (Some(a), Some(b)) => {
                store_u64(KEY_RESERVE_A, a);
                store_u64(KEY_RESERVE_B, b);
                pack(0, 0)
            }
            _ => -1,
        },
        1 => swap(args, true),
        2 => swap(args, false),
        3 => {
            let a = load_u64(KEY_RESERVE_A);
            let b = load_u64(KEY_RESERVE_B);
            write_output(0, &a.to_le_bytes());
            write_output(8, &b.to_le_bytes());
            pack(output_address(), 16)
        }
        _ => -1,
    }
}

// --- swap ------------------------------------------------------------------

/// Executes a constant-product swap in the requested direction.
fn swap(args: &[u8], a_to_b: bool) -> i64 {
    let Some(amount_in) = read_u64(args, 0) else {
        return -1;
    };
    if amount_in == 0 {
        return -1;
    }

    let reserve_a = load_u64(KEY_RESERVE_A);
    let reserve_b = load_u64(KEY_RESERVE_B);
    if reserve_a == 0 || reserve_b == 0 {
        // An uninitialised pool has no price.
        return -1;
    }

    let (reserve_in, reserve_out) = if a_to_b {
        (reserve_a, reserve_b)
    } else {
        (reserve_b, reserve_a)
    };

    // out = (in * fee * reserve_out) / (reserve_in * denom + in * fee)
    //
    // Computed in u128 throughout: the numerator alone can exceed u64 for
    // realistic reserves, and a wrap here would misprice the entire pool.
    let amount_with_fee = u128::from(amount_in) * FEE_NUMERATOR;
    let numerator = amount_with_fee * u128::from(reserve_out);
    let denominator = u128::from(reserve_in) * FEE_DENOMINATOR + amount_with_fee;
    if denominator == 0 {
        return -1;
    }
    let amount_out_128 = numerator / denominator;

    // The invariant that makes the pool solvent: never pay out more than is
    // held. Without this a large input could drain the reserve entirely.
    if amount_out_128 >= u128::from(reserve_out) {
        return -1;
    }
    let amount_out = amount_out_128 as u64;
    if amount_out == 0 {
        // Rounding swallowed the whole trade; refuse rather than take the input
        // for nothing.
        return -1;
    }

    let (new_a, new_b) = if a_to_b {
        (reserve_a + amount_in, reserve_b - amount_out)
    } else {
        (reserve_a - amount_out, reserve_b + amount_in)
    };

    store_u64(KEY_RESERVE_A, new_a);
    store_u64(KEY_RESERVE_B, new_b);

    // Event payload: height, amount in, amount out.
    let height = unsafe { block_height() } as u64;
    write_output(0, &height.to_le_bytes());
    write_output(8, &amount_in.to_le_bytes());
    write_output(16, &amount_out.to_le_bytes());
    unsafe {
        emit_event(
            TOPIC_SWAP.as_ptr() as i32,
            TOPIC_SWAP.len() as i32,
            output_address(),
            24,
        );
    }

    write_output(0, &amount_out.to_le_bytes());
    pack(output_address(), 8)
}

// --- helpers ---------------------------------------------------------------

fn output_address() -> i32 {
    (&raw const OUTPUT) as usize as i32
}

/// Packs a pointer and length into the ABI's single `i64` return.
fn pack(ptr: i32, len: i32) -> i64 {
    ((ptr as i64) << 32) | (len as i64 & 0xFFFF_FFFF)
}

fn write_output(offset: usize, bytes: &[u8]) {
    if offset + bytes.len() > OUTPUT_CAPACITY {
        return;
    }
    unsafe {
        let out = core::slice::from_raw_parts_mut(
            (&raw mut OUTPUT).cast::<u8>().add(offset),
            bytes.len(),
        );
        out.copy_from_slice(bytes);
    }
}

fn read_u64(bytes: &[u8], offset: usize) -> Option<u64> {
    if offset + 8 > bytes.len() {
        return None;
    }
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&bytes[offset..offset + 8]);
    Some(u64::from_le_bytes(buf))
}

fn store_u64(key: &[u8], value: u64) {
    let bytes = value.to_le_bytes();
    unsafe {
        let scratch = core::slice::from_raw_parts_mut((&raw mut SCRATCH).cast::<u8>(), 8);
        scratch.copy_from_slice(&bytes);
        storage_write(
            key.as_ptr() as i32,
            key.len() as i32,
            (&raw const SCRATCH) as usize as i32,
            8,
        );
    }
}

/// Reads a `u64`, treating an absent key as zero.
fn load_u64(key: &[u8]) -> u64 {
    unsafe {
        let read = storage_read(
            key.as_ptr() as i32,
            key.len() as i32,
            (&raw const SCRATCH) as usize as i32,
            8,
        );
        if read != 8 {
            return 0;
        }
        let mut buf = [0u8; 8];
        let scratch = core::slice::from_raw_parts((&raw const SCRATCH).cast::<u8>(), 8);
        buf.copy_from_slice(scratch);
        u64::from_le_bytes(buf)
    }
}
