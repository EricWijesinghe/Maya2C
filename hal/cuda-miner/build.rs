//! Compiles `kernels/dag.cu` with nvcc, but only under the `cuda` feature.
//!
//! ## Why this is gated
//!
//! This crate is a workspace member, so `cargo build --workspace` compiles it —
//! including on the GPU-less CI runners the security-audit workflow uses. With
//! `cuda` off, this script does nothing at all: no nvcc, no CUDA runtime, no
//! link step. Turning it on is an explicit `--features cuda` on a machine that
//! has a toolkit.
//!
//! ## Why nvcc directly rather than the `cc` crate
//!
//! The `cc` crate drives host compilers. Device code needs nvcc's own
//! architecture flags, and the alternative — adding a build-dependency that
//! wraps nvcc — would be a third-party crate in the build graph of a consensus
//! component to save a `Command::new`.
//!
//! ## Architectures
//!
//! Compiled for the range of consumer and datacenter cards this proof of work
//! targets, with PTX embedded for the newest so a card released after this
//! build still runs it through JIT. Override with `MAYA_CUDA_ARCHS`, e.g.
//! `MAYA_CUDA_ARCHS="89"` for a single-architecture build that finishes in a
//! fraction of the time.

use std::path::PathBuf;
use std::process::Command;

/// Compute capabilities built by default.
///
/// 61 (Pascal) through 89 (Ada). GDDR6/6X cards, which is what this proof of
/// work is tuned for.
const DEFAULT_ARCHS: &[&str] = &["61", "70", "75", "86", "89"];

fn main() {
    println!("cargo:rerun-if-changed=kernels/dag.cu");
    println!("cargo:rerun-if-env-changed=MAYA_CUDA_ARCHS");
    println!("cargo:rerun-if-env-changed=CUDA_PATH");

    if std::env::var_os("CARGO_FEATURE_CUDA").is_none() {
        return;
    }

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets OUT_DIR"));
    let library = out_dir.join(if cfg!(windows) {
        "mayadag.lib"
    } else {
        "libmayadag.a"
    });

    let archs = std::env::var("MAYA_CUDA_ARCHS").map_or_else(
        |_| {
            DEFAULT_ARCHS
                .iter()
                .map(|arch| (*arch).to_string())
                .collect()
        },
        |value| {
            value
                .split(',')
                .map(|arch| arch.trim().to_string())
                .filter(|arch| !arch.is_empty())
                .collect::<Vec<_>>()
        },
    );

    let mut command = Command::new(nvcc());
    command
        .arg("--lib")
        .arg("-O3")
        .arg("-lineinfo")
        .arg("-o")
        .arg(&library);

    for (index, arch) in archs.iter().enumerate() {
        // The last architecture also embeds PTX, so a newer card than any built
        // for here still runs the kernel through the driver's JIT rather than
        // failing to launch.
        let last = index + 1 == archs.len();
        command.arg(format!(
            "-gencode=arch=compute_{arch},code={}",
            if last {
                format!("[sm_{arch},compute_{arch}]")
            } else {
                format!("sm_{arch}")
            }
        ));
    }

    command.arg("kernels/dag.cu");

    let status = command
        .status()
        .unwrap_or_else(|error| panic!("could not run nvcc: {error}. Is the CUDA toolkit installed and on PATH, or CUDA_PATH set?"));
    assert!(status.success(), "nvcc failed to compile kernels/dag.cu");

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=mayadag");

    for path in cuda_library_paths() {
        println!("cargo:rustc-link-search=native={}", path.display());
    }
    println!("cargo:rustc-link-lib=dylib=cudart");
}

/// The nvcc to invoke: `CUDA_PATH/bin/nvcc` when the toolkit sets that, and
/// otherwise whatever is on `PATH`.
fn nvcc() -> PathBuf {
    match std::env::var_os("CUDA_PATH") {
        Some(root) => PathBuf::from(root).join("bin").join("nvcc"),
        None => PathBuf::from("nvcc"),
    }
}

/// Where the CUDA runtime import library lives.
fn cuda_library_paths() -> Vec<PathBuf> {
    let Some(root) = std::env::var_os("CUDA_PATH") else {
        return Vec::new();
    };
    let root = PathBuf::from(root);

    if cfg!(windows) {
        vec![root.join("lib").join("x64")]
    } else {
        vec![root.join("lib64"), root.join("lib")]
    }
}
