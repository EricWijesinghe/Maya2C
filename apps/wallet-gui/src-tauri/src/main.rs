//! Desktop entry point.
//!
//! The application logic lives in the library so Tauri''s mobile targets, which
//! build a library and call into it from a platform entry point, share exactly
//! the same code.

// Hides the console window on Windows release builds. A wallet spawning a
// terminal alongside itself looks like malware.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    maya_wallet_gui_lib::run();
}
