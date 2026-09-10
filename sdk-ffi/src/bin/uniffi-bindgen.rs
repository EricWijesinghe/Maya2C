//! The bindings generator, as uniffi requires it: a binary in the crate that
//! defines the interface, so the generator sees the same proc-macro metadata
//! the library exports.
fn main() {
    uniffi::uniffi_bindgen_main();
}
