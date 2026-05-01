// Prevents an additional console window from popping up on Windows in release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    clawft_ui_tauri_lib::run()
}
