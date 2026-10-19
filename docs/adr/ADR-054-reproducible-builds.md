# ADR-054: Reproducible Linux builds of the node

**Status:** Accepted (queue item 1 of the upgrades Eric approved on 2026-10-19). Linux only for now.

## Context

A published binary is only as trustworthy as the machine that built it. If two builds of the same commit give the same bytes, anyone can rebuild the release and compare its SHA-256 with `SHA256SUMS`, and a tampered CI runner or a swapped download becomes detectable without trusting us.

The release profile (`Cargo.toml`, `[profile.release]`) was already most of the way: fat LTO, `codegen-units = 1`, `strip = true`. No build script in the tree embeds a time or a git hash (checked with `rg` over every `build.rs` on 2026-10-19). What still differs between two machines is **paths**: panic locations and RocksDB's C++ asserts carry the absolute path of the checkout and of `~/.cargo/registry`.

## Decision

1. `scripts/repro_build.sh <target>` is the one way to build the release node. It maps the checkout to `/maya2c` and `CARGO_HOME` to `/cargo`, for Rust with `--remap-path-prefix` and for C/C++ with `-ffile-prefix-map`. It passes the Rust flags through `--config target.<triple>.rustflags`, which cargo appends to `.cargo/config.toml`'s list; `RUSTFLAGS` would replace that list and drop the mold link argument without a word.
2. `release-binaries.yml` uses the script for both Linux targets, so the binary in a release is the reproducible one.
3. `reproducible-build.yml` builds twice on one runner, from checkouts at different depths with different cargo homes, and fails unless the hashes match. It runs on changes to the lockfile, manifests, cargo config, toolchain file or the script; weekly; and by hand. It is not a required check because two fat-LTO builds take most of an hour.

## Not covered (yet)

- **Windows**: PE headers and PDB references carry a timestamp and a GUID unless the linker is run in `/Brepro` mode, and the MSYS and Windows forms of a path differ, so the remap has to be done twice. A later change.
- **macOS**: `ld64` writes a UUID derived from the content, which is usually stable, but nobody has checked it here.
- **Different runner images**: the check shows two builds on one image agree. A rebuild on a different distro with a different system `clang` or `libstdc++` can differ in the RocksDB objects. The toolchain is pinned (`rust-toolchain.toml`) but the C toolchain is the image's. Pinning it, for example by building inside a digest-pinned container, is the next step.

## Consequences

The Linux release can be checked by anyone with the pinned toolchain and Ubuntu 24.04. A failed weekly run means something in the build became nondeterministic; the job uploads both binaries so the cause can be found with `diffoscope`.
