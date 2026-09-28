//! The debugger maps each host call back to the contract's source line, from
//! the DWARF `rustc -g` puts in the wasm (Master Prompt 24's "source-line
//! mapping"). The contract is compiled here, so the line table is real.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_vm::host::MemoryState;
use maya2c_cli::record;

const SOURCE: &str = r#"#![no_std]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }
#[link(wasm_import_module = "env")]
extern "C" {
    fn storage_write(key: *const u8, key_len: i32, value: *const u8, value_len: i32);
    fn emit_event(topic: *const u8, topic_len: i32, data: *const u8, data_len: i32);
}
#[no_mangle]
pub extern "C" fn invoke(_len: i32) -> i64 {
    unsafe {
        storage_write(b"k1".as_ptr(), 2, b"vv".as_ptr(), 2);
        emit_event(b"t".as_ptr(), 1, b"d".as_ptr(), 1);
    }
    0
}
"#;

/// Compiles `SOURCE` to wasm with debug info.
fn contract() -> Vec<u8> {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("probe.rs");
    let out = dir.path().join("probe.wasm");
    std::fs::write(&src, SOURCE).unwrap();
    let status =
        std::process::Command::new(std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into()))
            .args([
                "--edition",
                "2021",
                "--crate-type",
                "cdylib",
                "--target",
                "wasm32-unknown-unknown",
                "-g",
                "-C",
                "opt-level=0",
                "-C",
                "panic=abort",
                "-o",
            ])
            .arg(&out)
            .arg(&src)
            .status()
            .expect("running rustc");
    assert!(
        status.success(),
        "rustc could not build the probe contract (is the wasm32 target installed?)"
    );
    std::fs::read(out).unwrap()
}

fn line_of(needle: &str) -> u64 {
    let index = SOURCE.lines().position(|l| l.contains(needle)).unwrap();
    u64::try_from(index + 1).unwrap()
}

#[test]
fn each_host_call_maps_to_the_line_that_made_it() {
    let wasm = contract();
    let (mut session, _) = record(
        &wasm,
        [0x51; 32],
        &[],
        10_000_000,
        MemoryState::at_height(1),
    )
    .unwrap();
    assert_eq!(session.total(), 2, "{}", session.listing());

    session.goto(1);
    let write = session
        .source()
        .expect("a line for the storage write")
        .clone();
    session.goto(2);
    let emit = session.source().expect("a line for the event").clone();
    println!(
        "step 1 at {}:{}, step 2 at {}:{}",
        write.file_name(),
        write.line,
        emit.file_name(),
        emit.line
    );
    assert_eq!(write.file_name(), "probe.rs");
    assert_eq!(write.line, line_of("storage_write(b\"k1\""));
    assert_eq!(emit.line, line_of("emit_event(b\"t\""));
    assert!(
        session.command("l").unwrap().contains("probe.rs:"),
        "the listing names the source line"
    );

    // Gas per line: both lines are charged, and the profile names them.
    let by_line = session.gas_by_line();
    println!("gas by line: {by_line:?}");
    assert_eq!(by_line.len(), 2);
    assert!(by_line.values().all(|gas| *gas > 0));
    let profile = session.command("p").unwrap();
    assert!(
        profile.contains("probe.rs:12") && profile.contains("gas"),
        "{profile}"
    );
}
