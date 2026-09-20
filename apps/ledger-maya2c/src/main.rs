//! The device binary.
//!
//! A thin shell. Everything worth testing lives in the library beside it, so
//! this file is the part that cannot be exercised without hardware — and is
//! therefore kept as small as it can be.
//!
//! # Status
//!
//! **This has never run on a device or an emulator.** No Ledger hardware, no
//! Speculos, and no ARM toolchain beyond a bare `thumbv8m.main-none-eabi`
//! target were available where it was written. The `ledger_device_sdk` calls
//! below are written against the SDK's documented surface and must be checked
//! against the version actually pinned before anyone believes them.
//!
//! # Why the signing path refuses (`0x6A81`) rather than being wrong
//!
//! A Maya2C signature is a hybrid pair and `HybridVerifyingKey::verify` checks
//! both halves. The device can plausibly produce the ML-DSA-65 half; whether it
//! can produce the SLH-DSA half is the open question this crate exists to
//! measure, and the answer is very likely no.
//!
//! Wiring up a handler that returns 3,309 bytes and calls it a signature would
//! produce a device that appears to work and emits transactions the chain
//! rejects. Until the measurement says otherwise, this refuses instead.

// `no_std` only for the device. The host build is an ordinary binary that
// explains itself and exits; making the whole crate `no_std` unconditionally
// left the host build with no `eprintln` and no panic handler.
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
#![forbid(unsafe_code)]

// The host build has no BOLOS to link against and nothing to run. `cargo test`
// exercises the library; this binary only means anything on the device.
#[cfg(not(any(
    target_os = "nanosplus",
    target_os = "stax",
    target_os = "flex",
    target_os = "nanox"
)))]
fn main() {
    eprintln!(
        "app-maya2c is a Ledger application and does nothing on a host.\n\
         Build it for the device:\n\
         \n    cargo build --release --target thumbv8m.main-none-eabi\n\
         \n\
         The protocol layer is a library and is tested on the host:\n\
         \n    cargo test\n"
    );
}

#[cfg(any(
    target_os = "nanosplus",
    target_os = "stax",
    target_os = "flex",
    target_os = "nanox"
))]
mod device {
    use app_maya2c::apdu::{self, ApduError, Assembler, Instruction};
    use app_maya2c::derive::DerivationPath;
    use ledger_device_sdk::io;

    /// Status word for a command that completed.
    const SW_OK: u16 = 0x9000;

    /// Status word for a command the user declined.
    const SW_DENIED: u16 = 0x6985;

    /// Status word for a capability this build does not have.
    ///
    /// Distinct from every parse error: "the device will not do this" is not
    /// the same answer as "your frame was malformed", and a host that conflated
    /// them would retry forever.
    const SW_UNSUPPORTED: u16 = 0x6A81;

    pub fn run() -> ! {
        let mut comm = io::Comm::new();
        let mut assembler = Assembler::new();

        loop {
            let event = comm.next_event::<io::ApduHeader>();
            if let io::Event::Command(_) = event {
                let status = handle(&mut comm, &mut assembler);
                comm.reply(status);
            }
        }
    }

    fn handle(comm: &mut io::Comm, assembler: &mut Assembler) -> u16 {
        let frame = comm.get_data().unwrap_or(&[]);

        let command = match apdu::parse(frame) {
            Ok(command) => command,
            Err(error) => return error.status_word(),
        };

        match command.instruction {
            // Both need the derivation path validated before anything else
            // touches key material.
            Instruction::GetPublicKey | Instruction::DisplayAddress => {
                match DerivationPath::parse(command.payload) {
                    Ok(_path) => {
                        // Derivation and display land here once the SDK surface
                        // is pinned. Refusing is the honest placeholder: a stub
                        // that returned zeroes would be a device confidently
                        // reporting an address nobody can spend from.
                        SW_UNSUPPORTED
                    }
                    Err(error) => error.status_word(),
                }
            }

            Instruction::SignTransaction => match assembler.push(&command) {
                // More chunks expected; nothing to say yet.
                Ok(None) => SW_OK,
                Ok(Some(_transaction)) => {
                    // The whole transaction is here. What is missing is a
                    // signer that fits: see this crate's documentation and the
                    // measurement in `docs/ledger-feasibility.md`.
                    assembler.reset();
                    SW_UNSUPPORTED
                }
                Err(error) => error.status_word(),
            },
        }
    }

    /// Silences the unused-constant warning until the approval flow exists.
    const _: u16 = SW_DENIED;
}

#[cfg(any(
    target_os = "nanosplus",
    target_os = "stax",
    target_os = "flex",
    target_os = "nanox"
))]
#[unsafe(no_mangle)]
extern "C" fn _start() -> ! {
    device::run()
}

#[cfg(any(
    target_os = "nanosplus",
    target_os = "stax",
    target_os = "flex",
    target_os = "nanox"
))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    // A panicking signing device must stop, not continue with unknown state.
    loop {}
}
