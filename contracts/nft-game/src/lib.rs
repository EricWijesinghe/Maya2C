//! NFT game: an ERC-721 subset with a level per token, for the Maya VM.
//!
//! **Do not deploy.** Ported from the Solidity shape of ERC-721, where every
//! state change checks `msg.sender`. The Maya VM gives a contract no caller
//! identity (ADR-026), so this port has to take the acting account as an
//! argument, and an argument is whatever the caller writes. The reference-app
//! test `anyone_can_move_anyones_token` shows the result. The contract is kept
//! because it is the concrete case the ADR is decided against, and because
//! every line except the authorisation check is what the template will be.
//!
//! ## Calls (all integers little-endian)
//!
//! | Byte 0 | Method | Arguments | Returns |
//! |---|---|---|---|
//! | 0 | `init` | `admin: [u8; 32]` | nothing |
//! | 1 | `mint` | `acting: [u8; 32]`, `id: u64`, `to: [u8; 32]` | nothing |
//! | 2 | `transfer` | `acting: [u8; 32]`, `id: u64`, `to: [u8; 32]` | nothing |
//! | 3 | `owner_of` | `id: u64` | `owner: [u8; 32]` |
//! | 4 | `level_up` | `acting: [u8; 32]`, `id: u64` | `level: u64` |
//! | 5 | `level_of` | `id: u64` | `level: u64` |
//!
//! `acting` is the account the caller **claims** to be. That is the gap.

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
    let (acting, token, to) = (account(args, 0)?, id(args, 32)?, account(args, 40)?);
    // The Solidity original: require(msg.sender == admin). Here `acting` is
    // only a claim.
    if read32(KEY_ADMIN)? != acting || load_owner(token).is_some() {
        return None;
    }
    store_owner(token, &to);
    Some(())
}

fn transfer(args: &[u8]) -> Option<()> {
    let (acting, token, to) = (account(args, 0)?, id(args, 32)?, account(args, 40)?);
    // The Solidity original: require(ownerOf(id) == msg.sender).
    if load_owner(token)? != acting {
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
    let (Some(acting), Some(token)) = (account(args, 0), id(args, 32)) else {
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
