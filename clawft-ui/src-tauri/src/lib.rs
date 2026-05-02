//! ClawFT agent dashboard — Tauri 2.0 desktop shell (WEFT-313, scaffold).
//!
//! Today this crate wraps the React `clawft-ui/dist/` bundle in a single
//! native window. The pieces in the 0.7.0 release-gate plan that are
//! **not yet** wired here are tracked as follow-up items:
//!
//! - System tray with agent-status colour states.
//! - Global hotkey (`Cmd+Shift+W` / `Ctrl+Shift+W`).
//! - `weft gateway` side-car that launches on app start and terminates
//!   on quit.
//! - macOS Spotlight registration.
//! - Native notification bridge (Linux / Windows / macOS).
//!
//! Each will land as its own commit with its own Plane item; this
//! scaffold gets the build target into the tree so the harder
//! follow-ups can be reviewed in isolation.

use serde::Serialize;

/// Uniform success/error envelope for `#[tauri::command]` responses.
#[derive(Serialize)]
struct CmdResponse<T: Serialize> {
    ok: bool,
    data: Option<T>,
    error: Option<String>,
}

impl<T: Serialize> CmdResponse<T> {
    fn success(data: T) -> Self {
        Self { ok: true, data: Some(data), error: None }
    }
}

/// Returns the shell version. Useful for the dashboard "About" pane to
/// show whether it is running inside Tauri vs a plain browser.
#[tauri::command]
fn shell_info() -> CmdResponse<ShellInfo> {
    CmdResponse::success(ShellInfo {
        product: "clawft-ui-tauri".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        target_os: std::env::consts::OS.to_string(),
        target_arch: std::env::consts::ARCH.to_string(),
    })
}

#[derive(Serialize)]
struct ShellInfo {
    product: String,
    version: String,
    target_os: String,
    target_arch: String,
}

/// Tauri entry point. Called from `main.rs`.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .invoke_handler(tauri::generate_handler![shell_info])
        .run(tauri::generate_context!())
        .expect("error while running ClawFT desktop shell");
}
