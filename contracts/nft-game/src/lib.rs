//! NFT game: an ERC-721 subset with a level per token, for the Maya VM.
//!
//! Every state change is authorised by the account that **signed the
//! transaction**, read from the host's `caller` function (ADR-026) — the
//! Maya equivalent of Solidity's `msg.sender`. The first version of this
//! contract took the acting account as an argument, because the VM had no
//! caller identity, and anyone could move anyone's token by claiming to be
//! them; `crates/reference-apps/tests/nft_game.rs` kept that proof until the
//! host function existed, and now proves the opposite.
//!
//! ## Calls (all integers little-endian)
//!
//! | Byte 0 | Method | Arguments | Returns |
//! |---|---|---|---|
//! | 0 | `init` | `admin: [u8; 32]` | nothing |
//! | 1 | `mint` (admin) | `id: u64`, `to: [u8; 32]` | nothing |
//! | 2 | `transfer` (owner) | `id: u64`, `to: [u8; 32]` | nothing |
//! | 3 | `owner_of` | `id: u64` | `owner: [u8; 32]` |
//! | 4 | `level_up` (owner) | `id: u64` | `level: u64` |
//! | 5 | `level_of` | `id: u64` | `level: u64` |

#![no_std]

use core::panic::PanicInfo;

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}

#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn storage_read(key_ptr: i32, key_len: i32, out_ptr: i32, out_cap: i32) -> i32;
    fn storage_write(key_ptr: i32, key_len: i32, val_ptr: i32, val_len: i32);
    fn emit_event(topic_ptr: i32, topic_len: i32, data_ptr: i32, data_len: i32);
    fn block_height() -> i64;
    fn caller(out_ptr: i32) -> i32;
}

/// The transaction's signer, or `None` outside a transaction.
fn signer() -> Option<Account> {
    let mut out = [0u8; 32];
    // SAFETY: `out` is a live 32-byte buffer for the duration of the call.
    let status = unsafe { caller(out.as_mut_ptr() as i32) };
    (status == 0).then_some(out)
}

const INPUT_CAPACITY: usize = 128;
const OUTPUT_CAPACITY: usize = 64;

static mut INPUT: [u8; INPUT_CAPACITY] = [0; INPUT_CAPACITY];
static mut OUTPUT: [u8; OUTPUT_CAPACITY] = [0; OUTPUT_CAPACITY];

type Account = [u8; 32];

const TOPIC_TRANSFER: &[u8] = b"transfer";
const KEY_ADMIN: &[u8] = b"admin";

#[unsafe(no_mangle)]
pub extern "C" fn input_ptr() -> i32 {
    (&raw const INPUT) as usize as i32
}

#[unsafe(no_mangle)]
pub extern "C" fn input_cap() -> i32 {
    INPUT_CAPACITY as i32
}

#[unsafe(no_mangle)]
pub extern "C" fn invoke(len: i32) -> i64 {
    let length = if len < 0 { 0 } else { len as usize };
    if length == 0 || length > INPUT_CAPACITY {
        return -1;
    }
    // SAFETY: the host wrote `length` bytes into INPUT, bounded above, and
    // nothing else aliases the buffer during a call.
    let input = unsafe { core::slice::from_raw_parts((&raw const INPUT).cast::<u8>(), length) };
    let args = &input[1..];
    let done = match input[0] {
        0 => account(args, 0).and_then(|admin| write(KEY_ADMIN, &admin)),
        1 => mint(args),
        2 => transfer(args),
        3 => return id(args, 0).map_or(-1, |i| output(&load_owner(i).unwrap_or([0; 32]))),
        4 => return level_up(args),
        5 => return id(args, 0).map_or(-1, |i| output(&load_level(i).to_le_bytes())),
        _ => None,
    };
    if done.is_some() { pack(0, 0) } else { -1 }
}

fn mint(args: &[u8]) -> Option<()> {
    let (token, to) = (id(args, 0)?, account(args, 8)?);
    // require(msg.sender == admin)
    if read32(KEY_ADMIN)? != signer()? || load_owner(token).is_some() {
        return None;
    }
    store_owner(token, &to);
    Some(())
}

fn transfer(args: &[u8]) -> Option<()> {
    let (token, to) = (id(args, 0)?, account(args, 8)?);
    // require(ownerOf(id) == msg.sender)
    if load_owner(token)? != signer()? {
        return None;
    }
    store_owner(token, &to);
    let mut event = [0u8; 40];
    event[..8].copy_from_slice(&token.to_le_bytes());
    event[8..].copy_from_slice(&to);
    // SAFETY: both pointers name live, correctly sized buffers for the call.
    unsafe {
        emit_event(TOPIC_TRANSFER.as_ptr() as i32, TOPIC_TRANSFER.len() as i32, event.as_ptr() as i32, 40);
    }
    Some(())
}

fn level_up(args: &[u8]) -> i64 {
    let (Some(acting), Some(token)) = (signer(), id(args, 0)) else {
        return -1;
    };
    if load_owner(token) != Some(acting) {
        return -1;
    }
    // One level per token per block: store the height of the last level-up.
    // SAFETY: a host call with no arguments.
    let height = unsafe { block_height() } as u64;
    let last_key = key(b'h', token);
    if read_u64(&last_key) == Some(height) {
        return -1;
    }
    write(&last_key, &height.to_le_bytes());
    let level = load_level(token) + 1;
    write(&key(b'l', token), &level.to_le_bytes());
    output(&level.to_le_bytes())
}

fn key(tag: u8, token: u64) -> [u8; 9] {
    let mut k = [tag; 9];
    k[1..].copy_from_slice(&token.to_le_bytes());
    k
}

fn load_owner(token: u64) -> Option<Account> {
    read32(&key(b'o', token))
}

fn store_owner(token: u64, owner: &Account) {
    write(&key(b'o', token), owner);
}

fn load_level(token: u64) -> u64 {
    read_u64(&key(b'l', token)).unwrap_or(0)
}

fn account(b: &[u8], at: usize) -> Option<Account> {
    b.get(at..at + 32)?.try_into().ok()
}

fn id(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

fn write(k: &[u8], v: &[u8]) -> Option<()> {
    // SAFETY: both slices are live for the duration of the host call.
    unsafe { storage_write(k.as_ptr() as i32, k.len() as i32, v.as_ptr() as i32, v.len() as i32) };
    Some(())
}

fn read32(k: &[u8]) -> Option<[u8; 32]> {
    let mut out = [0u8; 32];
    // SAFETY: `out` is a live 32-byte buffer and the host writes at most 32.
    let n = unsafe { storage_read(k.as_ptr() as i32, k.len() as i32, out.as_mut_ptr() as i32, 32) };
    (n == 32).then_some(out)
}

fn read_u64(k: &[u8]) -> Option<u64> {
    let mut out = [0u8; 8];
    // SAFETY: `out` is a live 8-byte buffer and the host writes at most 8.
    let n = unsafe { storage_read(k.as_ptr() as i32, k.len() as i32, out.as_mut_ptr() as i32, 8) };
    (n == 8).then(|| u64::from_le_bytes(out))
}

fn output(bytes: &[u8]) -> i64 {
    if bytes.len() > OUTPUT_CAPACITY {
        return -1;
    }
    // SAFETY: bounded above; OUTPUT is only touched here, within one call.
    unsafe {
        core::slice::from_raw_parts_mut((&raw mut OUTPUT).cast::<u8>(), bytes.len()).copy_from_slice(bytes);
    }
    pack((&raw const OUTPUT) as usize as i32, bytes.len() as i32)
}

fn pack(ptr: i32, len: i32) -> i64 {
    (i64::from(ptr) << 32) | (i64::from(len) & 0xFFFF_FFFF)
}
