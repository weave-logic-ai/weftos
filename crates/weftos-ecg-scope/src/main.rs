//! Native launcher for weft-ecg-scope. Env: SEED_HOST (default 169.254.42.1, the USB link),
//! COGNITUM_SEED_TOKEN (optional, for agent writes).

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result<()> {
    weftos_ecg_scope::run_native()
}

#[cfg(target_arch = "wasm32")]
fn main() {}
