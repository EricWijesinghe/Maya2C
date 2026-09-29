// Prevents an extra console window on Windows in release; do not remove.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    maya_chat_gui_lib::run();
}
