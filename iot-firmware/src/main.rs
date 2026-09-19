//! Device lifecycle on a Cortex-M33 under QEMU: PUF-derived seed, enrollment,
//! a telemetry batch, a tamper event — with the stack high-water mark measured.
//!
//! Prints results over semihosting and exits QEMU with the outcome. Stand-ins,
//! stated: the RNG is deterministic (QEMU has no TRNG) and the PUF response is
//! generated (QEMU SRAM powers up as zeros).

#![no_std]
#![no_main]

use cortex_m_rt::entry;
use cortex_m_semihosting::{debug, hprintln};
use maya_iot_anchor::keysource::{KeySource, PufSource};
use maya_iot_anchor::merkle::ReadingsAccumulator;
use maya_iot_anchor::puf::{HelperData, RESPONSE_BYTES};
use maya_iot_anchor::{Bounds, DeviceKey, SensorClass, TamperCause};
use panic_semihosting as _;
use rand_core::{CryptoRng, Error, RngCore};

/// Bytes below the stack pointer painted before the measured work.
const PAINTED: usize = 512 * 1024;
const PAINT: u8 = 0xA5;

/// xorshift64*. Deterministic, for QEMU only — never a device RNG.
struct QemuRng(u64);

impl RngCore for QemuRng {
    fn next_u32(&mut self) -> u32 {
        (self.next_u64() >> 32) as u32
    }
    fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        for chunk in dest.chunks_mut(8) {
            let bytes = self.next_u64().to_le_bytes();
            chunk.copy_from_slice(&bytes[..chunk.len()]);
        }
    }
    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), Error> {
        self.fill_bytes(dest);
        Ok(())
    }
}

impl CryptoRng for QemuRng {}

/// Paints `PAINTED` bytes below the current stack pointer.
fn paint(bottom: usize) {
    // SAFETY: QEMU's AN505 model maps 4 MiB of RAM at 0x3800_0000 and the stack
    // starts at its top; `bottom..sp` lies inside that RAM, below every live
    // frame, and nothing else uses it. Firmware stack measurement has no safe
    // equivalent; this binary is outside the node's no-`unsafe` paths
    // (execution directive 4) and runs only under an emulator.
    unsafe { core::ptr::write_bytes(bottom as *mut u8, PAINT, PAINTED - 256) };
}

/// Bytes of painted stack overwritten since `paint`.
fn used(bottom: usize) -> usize {
    let untouched = (0..PAINTED - 256)
        // SAFETY: the same painted region `paint` wrote, read byte by byte.
        .take_while(|offset| unsafe { core::ptr::read((bottom + offset) as *const u8) } == PAINT)
        .count();
    PAINTED - 256 - untouched
}

#[inline(never)]
fn lifecycle(rng: &mut QemuRng) -> Result<(), maya_iot_anchor::IotError> {
    // Manufacture: bind a TRNG secret to the chip's PUF response.
    let mut response = [0u8; RESPONSE_BYTES];
    rng.fill_bytes(&mut response);
    let mut secret = [0u8; 32];
    rng.fill_bytes(&mut secret);
    let helper = HelperData::enroll(&response, &secret);

    // Field: the PUF regenerates the seed, with one noisy bit.
    let mut source = PufSource::new(helper, |out: &mut [u8; RESPONSE_BYTES]| {
        *out = response;
        out[0] ^= 1;
    });
    let key = DeviceKey::from_seed(&source.seed()?);

    let owner = [0x42; 32];
    let bounds = Bounds::new(-40_000, 8_000, 5_000)?;
    let enrollment = key.enroll(&owner, SensorClass::ColdChainTemperature, bounds, rng)?;
    enrollment.verify(&owner)?;

    let mut readings = ReadingsAccumulator::new();
    for (counter, value) in (1u64..=60).zip((0i64..).map(|i| -18_000 + i * 10)) {
        readings.push(counter, value)?;
    }
    let summary = readings
        .summary()
        .ok_or(maya_iot_anchor::IotError::InvalidRange)?;
    // The first batch starts the chain; firmware persists each batch's hash
    // in flash and names it as the next batch's predecessor.
    let batch = key.sign_batch(&summary, &maya_iot_anchor::GENESIS_PREVIOUS, rng)?;
    batch.verify(key.public_key())?;

    let tamper = key.sign_tamper(61, TamperCause::Enclosure, rng)?;
    tamper.verify(key.public_key())
}

#[entry]
fn main() -> ! {
    let sp = cortex_m::register::msp::read() as usize;
    let bottom = sp - PAINTED;
    paint(bottom);

    let mut rng = QemuRng(0x9E37_79B9_7F4A_7C15);
    let outcome = lifecycle(&mut rng);
    let stack = used(bottom);

    match outcome {
        Ok(()) => {
            hprintln!("iot-firmware: PUF seed, enrollment, batch and tamper event verified");
            hprintln!("iot-firmware: stack high-water mark {} bytes", stack);
            debug::exit(debug::EXIT_SUCCESS);
        }
        Err(error) => {
            hprintln!("iot-firmware: failed: {}", error);
            debug::exit(debug::EXIT_FAILURE);
        }
    }
    loop {
        cortex_m::asm::wfi();
    }
}
