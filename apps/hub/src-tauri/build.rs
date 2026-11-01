use std::path::PathBuf;

fn main() {
    tauri_build::build();
    // The release SHA256SUMS that pins the miner. Unset means no miner runs
    // (src/miner_guard.rs), so a build without it fails closed.
    println!("cargo:rerun-if-env-changed=MAYA2C_MINER_SHA256SUMS");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    let sums = match std::env::var_os("MAYA2C_MINER_SHA256SUMS") {
        Some(path) => {
            println!("cargo:rerun-if-changed={}", path.to_string_lossy());
            std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("MAYA2C_MINER_SHA256SUMS unreadable: {e}"))
        }
        None => String::new(),
    };
    std::fs::write(out.join("miner-sha256sums.txt"), sums).expect("write miner checksums");
}
