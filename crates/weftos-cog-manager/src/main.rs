//! Native launcher for weft-cog-manager. Env: WEFTOS_HOST (default http://127.0.0.1:9480),
//! WEFTOS_WL_REGISTRY (our signed registry.json URL), WEFTOS_COGNITUM_REGISTRY (Cognitum mirror).

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result<()> {
    weftos_cog_manager::run_native()
}

#[cfg(target_arch = "wasm32")]
fn main() {}
