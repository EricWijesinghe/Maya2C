//! Hands the target triple this crate is compiled for to the engine
//! configuration, which names it so wasmtime infers no ISA flags from the CPU
//! a node happens to run on (`src/config.rs`, re-freeze review R-6). Cargo
//! knows the triple only here; it is the same string wasmtime's own
//! `Triple::host()` is parsed from, so the two always match.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // Cargo always sets TARGET for a build script; a build without it is not
    // a cargo build, and must not produce an engine that guesses.
    let target = std::env::var("TARGET").expect("cargo sets TARGET for build scripts");
    println!("cargo:rustc-env=MAYA_VM_TARGET={target}");
}
