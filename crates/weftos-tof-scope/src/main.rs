//! Native launcher for weft-tof-scope. Env: SEED_HOST (default 169.254.42.1, the USB link),
//! COGNITUM_SEED_TOKEN (optional, for agent writes), COMPANION_GUIDE_DIR (local guide folder).

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result<()> {
    weftos_tof_scope::run_native()
}

#[cfg(target_arch = "wasm32")]
fn main() {}
