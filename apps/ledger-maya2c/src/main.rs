//! The device binary: APDU dispatch, the review screens, and the one FFI call
//! that derives the key. Everything testable lives in the library.
//!
//! # Commands
//!
//! | INS | Command | Payload | Answer |
//! |---|---|---|---|
//! | `02` | `GET_PUBLIC_KEY` | derivation path | page 0 of the 1,952-byte suite-`0x10` key |
//! | `06` | `DISPLAY_ADDRESS` | derivation path | the 32-byte address, after the user confirms it on screen |
//! | `04` | `SIGN_TRANSACTION` | chunk 0: the path; chunks 1…: the v7 signing bytes | page 0 of the 3,309-byte signature, after review |
//! | `08` | `GET_PAGE` | none; P2 = page | page P2 of the last key or signature |
//!
//! A reply carries at most 255 bytes. So a command that produces a key or a
//! signature keeps it in one response buffer, answers with page 0, and the
//! host collects the rest with `GET_PAGE`. The buffer is overwritten by the
//! next command that produces something, and zeroed when a sequence restarts.
//!
//! # Signing is ML-DSA-65 alone, suite `0x10`
//!
//! Not the hybrid: its SLH-DSA half does not fit (`docs/ledger-feasibility.md`).
//! The key and the signature come from `lowmem`, the low-memory ML-DSA-65 that
//! equals `fips204` and the NIST ACVP vectors byte for byte and needs about
//! 7 KiB of stack on this CPU where `fips204` needs over 140 KiB.

#![cfg_attr(
    any(
        target_os = "nanosplus",
        target_os = "stax",
        target_os = "flex",
        target_os = "nanox"
    ),
    no_std
)]
#![cfg_attr(
    any(
        target_os = "nanosplus",
        target_os = "stax",
        target_os = "flex",
        target_os = "nanox"
    ),
    no_main
)]
// Two `unsafe` items exist, both device FFI with nothing to exercise on a
// host: the SDK entry point and the SLIP-0010 derivation syscall. Each is
// allowed where it stands, with its SAFETY comment; everything else is denied.
#![deny(unsafe_code)]

#[cfg(not(any(
    target_os = "nanosplus",
    target_os = "stax",
    target_os = "flex",
    target_os = "nanox"
)))]
fn main() {
    eprintln!(
        "app-maya2c is a Ledger application and does nothing on a host.\n\
         Build it for a device with `cargo ledger build nanosplus`; the protocol\n\
         layer is a library and is tested on the host with `cargo test`."
    );
}

#[cfg(any(
    target_os = "nanosplus",
    target_os = "stax",
    target_os = "flex",
    target_os = "nanox"
))]
mod device {
    use core::fmt::Write as _;

    use app_maya2c::apdu::{self, Assembler, Chunk, Command, Instruction, MAX_APDU_PAYLOAD};
    use app_maya2c::derive::{DerivationPath, PATH_LEN};
    use app_maya2c::review::{self, MAX_OUTPUTS};
    use app_maya2c::suite::{self, PUBLIC_KEY_LEN, SECRET_KEY_LEN, SIGNATURE_LEN};
    use ledger_device_sdk::io::{ApduHeader, Comm, Event, Reply};
    use ledger_device_sdk::nbgl::{
        Field, NbglAddressReview, NbglHomeAndSettings, NbglReview, NbglReviewStatus, StatusType,
    };
    use zeroize::Zeroizing;

    const SW_OK: u16 = 0x9000;
    const SW_DENIED: u16 = 0x6985;
    const SW_NO_SUCH_PAGE: u16 = 0x6A86;
    const SW_INTERNAL: u16 = 0x6F00;
    /// The OS derivation produced nothing usable.
    const SW_DERIVATION_FAILED: u16 = 0x6F01;

    /// The derivation path's wire length: a count byte and five `u32`s.
    const PATH_BYTES: usize = 1 + 4 * PATH_LEN;

    /// The last key or signature, served a page at a time.
    struct Response {
        bytes: [u8; SIGNATURE_LEN],
        len: usize,
    }

    impl Response {
        const fn new() -> Self {
            Self {
                bytes: [0; SIGNATURE_LEN],
                len: 0,
            }
        }

        fn set(&mut self, data: &[u8]) {
            self.bytes.fill(0);
            self.bytes[..data.len()].copy_from_slice(data);
            self.len = data.len();
        }

        /// The first `PUBLIC_KEY_LEN` bytes, for a key written in place.
        fn key_slot(&mut self) -> &mut [u8; PUBLIC_KEY_LEN] {
            self.bytes.fill(0);
            self.len = PUBLIC_KEY_LEN;
            // `SIGNATURE_LEN` > `PUBLIC_KEY_LEN`, so the slice always converts.
            (&mut self.bytes[..PUBLIC_KEY_LEN])
                .try_into()
                .unwrap_or_else(|_| unreachable!())
        }

        fn page(&self, index: usize) -> Option<&[u8]> {
            let start = index.checked_mul(MAX_APDU_PAYLOAD)?;
            if start >= self.len {
                return None;
            }
            Some(&self.bytes[start..self.len.min(start + MAX_APDU_PAYLOAD)])
        }
    }

    /// The SLIP-0010 ed25519 child key for `path` — the chain key the desktop
    /// wallet derives from the same recovery phrase (`wallet-gui/core/src/hd.rs`).
    ///
    /// `None` if the syscall left the buffer untouched. It returns `void` and
    /// signals failure by throwing, so a zero key is the only evidence a Rust
    /// caller can check — and signing with one would produce a signature for a
    /// key the user's recovery phrase does not control.
    fn chain_key(path: &DerivationPath) -> Option<Zeroizing<[u8; 32]>> {
        let components = path.components();
        let mut node = Zeroizing::new([0u8; 64]);
        // SAFETY: `components` is a live array of `PATH_LEN` u32s and `node` a
        // live 64-byte buffer, the lengths the syscall writes and reads (the
        // SDK's own `Ed25519::derive_from_path_slip10` makes the same call with
        // the same buffers). The chain-code and seed-key pointers may be null.
        // The syscall exists only on the device; there is no host path to
        // test, and `tests/ledger_tests.rs` checks its output under Speculos
        // against a host SLIP-0010 derivation of the same phrase.
        #[allow(unsafe_code)]
        unsafe {
            ledger_device_sdk::sys::os_perso_derive_node_with_seed_key(
                ledger_device_sdk::sys::HDW_ED25519_SLIP10,
                ledger_device_sdk::ecc::CurvesId::Ed25519 as u8,
                components.as_ptr(),
                PATH_LEN as u32,
                node.as_mut_ptr(),
                core::ptr::null_mut(),
                core::ptr::null_mut(),
                0,
            );
        }
        let mut key = Zeroizing::new([0u8; 32]);
        key.copy_from_slice(&node[..32]);
        if key.iter().all(|b| *b == 0) {
            return None;
        }
        Some(key)
    }

    /// Fixed-capacity text for a screen field.
    struct Text<const N: usize> {
        bytes: [u8; N],
        len: usize,
    }

    impl<const N: usize> Text<N> {
        const fn new() -> Self {
            Self {
                bytes: [0; N],
                len: 0,
            }
        }

        fn as_str(&self) -> &str {
            core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
        }
    }

    impl<const N: usize> core::fmt::Write for Text<N> {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            let end = self.len + s.len();
            if end > N {
                return Err(core::fmt::Error);
            }
            self.bytes[self.len..end].copy_from_slice(s.as_bytes());
            self.len = end;
            Ok(())
        }
    }

    fn hex<const N: usize>(bytes: &[u8]) -> Text<N> {
        let mut text = Text::new();
        for b in bytes {
            let _ = write!(text, "{b:02x}");
        }
        text
    }

    // Every handler is `#[inline(never)]`: inlined into `run`, their locals
    // shared one 28 KB frame and the 32 KiB stack overflowed on the first
    // key. Separate, only the one running is on the stack.

    #[inline(never)]
    fn get_public_key(payload: &[u8], response: &mut Response) -> u16 {
        match DerivationPath::parse(payload) {
            Ok(path) => {
                let Some(chain_key) = chain_key(&path) else {
                    return SW_DERIVATION_FAILED;
                };
                let mut secret = Zeroizing::new([0u8; SECRET_KEY_LEN]);
                suite::keypair_into(&chain_key, response.key_slot(), &mut secret);
                SW_OK
            }
            Err(error) => error.status_word(),
        }
    }

    #[inline(never)]
    fn display_address(payload: &[u8], response: &mut Response) -> u16 {
        let Ok(path) = DerivationPath::parse(payload) else {
            return apdu::ApduError::BadDerivationPath.status_word();
        };
        let Some(chain_key) = chain_key(&path) else {
            return SW_DERIVATION_FAILED;
        };
        let mut public = [0u8; PUBLIC_KEY_LEN];
        let mut secret = Zeroizing::new([0u8; SECRET_KEY_LEN]);
        suite::keypair_into(&chain_key, &mut public, &mut secret);
        drop(secret);
        let address = suite::address(&public);
        let shown = hex::<64>(&address);
        let approved = NbglAddressReview::new()
            .review_title("Verify Maya2C address")
            .review_subtitle("ML-DSA-65 (suite 0x10)")
            .show(shown.as_str());
        NbglReviewStatus::new()
            .status_type(StatusType::Address)
            .show(approved);
        if approved {
            response.set(&address);
            SW_OK
        } else {
            SW_DENIED
        }
    }

    /// Reviews and signs an assembled `path ‖ signing bytes`.
    #[inline(never)]
    fn sign_transaction(assembled: &[u8], response: &mut Response) -> u16 {
        if assembled.len() < PATH_BYTES {
            return apdu::ApduError::BadDerivationPath.status_word();
        }
        let (path, signing_bytes) = assembled.split_at(PATH_BYTES);
        let Ok(path) = DerivationPath::parse(path) else {
            return apdu::ApduError::BadDerivationPath.status_word();
        };
        let Some(chain_key) = chain_key(&path) else {
            return SW_DERIVATION_FAILED;
        };
        let mut public = [0u8; PUBLIC_KEY_LEN];
        let mut secret = Zeroizing::new([0u8; SECRET_KEY_LEN]);
        suite::keypair_into(&chain_key, &mut public, &mut secret);
        let shown = match review::review(signing_bytes, &public) {
            Ok(shown) => shown,
            Err(error) => return error.status_word(),
        };
        if !approve(&shown, &public) {
            return SW_DENIED;
        }
        response.bytes.fill(0);
        match suite::sign_into(&secret, signing_bytes, &mut response.bytes) {
            Ok(()) => {
                response.len = SIGNATURE_LEN;
                SW_OK
            }
            Err(_) => {
                response.len = 0;
                SW_INTERNAL
            }
        }
    }

    /// The review screens: every output, the nonce, and the paying account.
    ///
    /// That is the whole debit. This chain is account-based: a transfer debits
    /// the sender exactly the sum of its outputs and there is no fee field
    /// (`crates/node/src/state/db.rs`, `total_outputs` then `debit`). A
    /// transaction's `inputs` are encoded and never read by any state
    /// transition, so unlike a UTXO chain there is no input value the device
    /// would have to prove or display.
    #[inline(never)]
    fn approve(shown: &review::Review, public: &[u8; PUBLIC_KEY_LEN]) -> bool {
        // 20 digits of u64 plus the unit suffix; `Text` refuses to write past
        // its capacity, and a truncated amount on a review screen is exactly
        // what must not happen silently.
        const AMOUNT_CAPACITY: usize = 40;
        const _: () = assert!(AMOUNT_CAPACITY >= 20 + " base units".len());
        let mut amounts: [Text<AMOUNT_CAPACITY>; MAX_OUTPUTS] =
            [const { Text::new() }; MAX_OUTPUTS];
        let mut recipients: [Text<64>; MAX_OUTPUTS] = [const { Text::new() }; MAX_OUTPUTS];
        for (i, output) in shown.outputs.iter().take(shown.output_count).enumerate() {
            let _ = write!(amounts[i], "{} base units", output.amount);
            recipients[i] = hex(&output.recipient);
        }
        let mut nonce = Text::<20>::new();
        let _ = write!(nonce, "{}", shown.nonce);
        let from = hex::<64>(&suite::address(public));

        const NAMES: [(&str, &str); MAX_OUTPUTS] = [
            ("Amount 1", "To 1"),
            ("Amount 2", "To 2"),
            ("Amount 3", "To 3"),
            ("Amount 4", "To 4"),
        ];
        let mut fields = [const {
            Field {
                name: "",
                value: "",
            }
        }; 2 * MAX_OUTPUTS + 2];
        let mut n = 0;
        for i in 0..shown.output_count {
            fields[n] = Field {
                name: NAMES[i].0,
                value: amounts[i].as_str(),
            };
            fields[n + 1] = Field {
                name: NAMES[i].1,
                value: recipients[i].as_str(),
            };
            n += 2;
        }
        fields[n] = Field {
            name: "From",
            value: from.as_str(),
        };
        fields[n + 1] = Field {
            name: "Nonce",
            value: nonce.as_str(),
        };
        n += 2;

        let approved = NbglReview::new()
            .titles(
                "Review Maya2C transfer",
                "ML-DSA-65 (suite 0x10)",
                "Sign transfer",
            )
            .show(&fields[..n]);
        NbglReviewStatus::new().show(approved);
        approved
    }

    fn command_of<'a>(header: &ApduHeader, payload: &'a [u8]) -> Result<Command<'a>, u16> {
        let instruction = Instruction::from_byte(header.ins).map_err(|e| e.status_word())?;
        let chunk = Chunk::from_p1(header.p1).map_err(|e| e.status_word())?;
        Ok(Command {
            instruction,
            chunk,
            p2: header.p2,
            payload,
        })
    }

    #[inline(never)]
    fn handle(
        comm: &mut Comm,
        header: &ApduHeader,
        assembler: &mut Assembler,
        response: &mut Response,
    ) -> u16 {
        let payload = match comm.get_data() {
            Ok(payload) => payload,
            Err(status) => return Reply::from(status).0,
        };
        if header.cla != apdu::CLA {
            return apdu::ApduError::BadClass(header.cla).status_word();
        }
        if header.ins == apdu::INS_GET_PAGE {
            return match response.page(usize::from(header.p2)) {
                Some(page) => {
                    comm.append(page);
                    SW_OK
                }
                None => SW_NO_SUCH_PAGE,
            };
        }
        let command = match command_of(header, payload) {
            Ok(command) => command,
            Err(status) => return status,
        };
        let status = match command.instruction {
            Instruction::GetPublicKey => get_public_key(command.payload, response),
            Instruction::DisplayAddress => display_address(command.payload, response),
            Instruction::SignTransaction => match assembler.push(&command) {
                Ok(None) => return SW_OK,
                Ok(Some(assembled)) => {
                    let status = sign_transaction(assembled, response);
                    assembler.reset();
                    status
                }
                Err(error) => error.status_word(),
            },
        };
        if status == SW_OK {
            if let Some(page) = response.page(0) {
                comm.append(page);
            }
        }
        status
    }

    pub fn run() -> ! {
        let mut comm = Comm::new();
        ledger_device_sdk::nbgl::init_comm(&mut comm);
        let mut home = NbglHomeAndSettings::new().infos(
            "Maya2C",
            env!("CARGO_PKG_VERSION"),
            "Maya2C contributors",
        );
        home.show_and_return();

        let mut assembler = Assembler::new();
        let mut response = Response::new();
        loop {
            if let Event::Command(header) = comm.next_event::<ApduHeader>() {
                let status = handle(&mut comm, &header, &mut assembler, &mut response);
                comm.reply(Reply(status));
                home.show_and_return();
            }
        }
    }
}

#[cfg(any(
    target_os = "nanosplus",
    target_os = "stax",
    target_os = "flex",
    target_os = "nanox"
))]
ledger_device_sdk::set_panic!(ledger_device_sdk::exiting_panic);

// SAFETY: `sample_main` is the symbol the Ledger SDK's C runtime calls once
// the OS has started the app; it takes no arguments and never returns.
#[cfg(any(
    target_os = "nanosplus",
    target_os = "stax",
    target_os = "flex",
    target_os = "nanox"
))]
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
extern "C" fn sample_main() {
    device::run()
}
