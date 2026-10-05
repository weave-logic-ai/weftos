//! Weave Manager. Tries this machine's cog-host first (`WEFTOS_HOST`, else
//! `http://127.0.0.1:9480`), then the user-local address book.

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result<()> {
    weftos_cog_manager::run_native()
}

#[cfg(target_arch = "wasm32")]
fn main() {}
