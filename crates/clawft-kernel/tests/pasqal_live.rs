//! Live Pasqal Cloud integration test — EXPERIMENTAL (0.6.x).
//!
//! Exercises the same interface against real Pasqal endpoints. Gated on
//! credentials via environment variables, and marked `#[ignore]` so it only
//! runs when explicitly requested.
//!
//! ## Usage
//!
//! ```bash
//! # T1 EMU-FREE (free tier; recommended first real run)
//! export PASQAL_CLIENT_ID=...
//! export PASQAL_CLIENT_SECRET=...
//! export PASQAL_PROJECT_ID=...
//! export PASQAL_DEVICE=EMU_FREE
//! cargo test -p clawft-kernel --features quantum-pasqal --test pasqal_live \
//!     -- --ignored --nocapture
//!
//! # T3 EMU-TN (paid emulator)
//! export PASQAL_DEVICE=EMU_TN
//! cargo test -p clawft-kernel --features quantum-pasqal --test pasqal_live \
//!     -- --ignored --nocapture
//!
//! # T4 Fresnel QPU (real hardware, ~$5-15/run)
//! export PASQAL_DEVICE=FRESNEL
//! cargo test -p clawft-kernel --features quantum-pasqal --test pasqal_live \
//!     -- --ignored --nocapture
//! ```

#![cfg(feature = "quantum-pasqal")]

use std::time::Duration;

use clawft_kernel::{
    EvolutionParams, JobStatus, PasqalBackend, PasqalConfig, PasqalDevice, QuantumBackend,
    QuantumCognitiveState, RegisterConstraints, build_register,
};

fn device_from_env() -> PasqalDevice {
    match std::env::var("PASQAL_DEVICE").as_deref() {
        Ok("EMU_TN") => PasqalDevice::EmuTn,
        Ok("FRESNEL") => PasqalDevice::Fresnel,
        _ => PasqalDevice::EmuFree,
    }
}

fn config_from_env() -> Option<PasqalConfig> {
    let client_id = std::env::var("PASQAL_CLIENT_ID").ok()?;
    let client_secret = std::env::var("PASQAL_CLIENT_SECRET").ok()?;
    let project_id = std::env::var("PASQAL_PROJECT_ID").ok()?;
    Some(PasqalConfig {
        client_id,
        client_secret: Some(client_secret),
        project_id,
        device: device_from_env(),
        http_timeout: Duration::from_secs(60),
        ..PasqalConfig::default()
    })
}

/// Full POC: submit a 3-atom triangle quantum walk, poll to completion,
/// retrieve bitstrings, verify shape of results.
#[tokio::test]
#[ignore = "requires PASQAL_* credentials; run explicitly with --ignored"]
async fn live_triangle_quantum_walk() {
    let Some(cfg) = config_from_env() else {
        panic!("missing PASQAL_CLIENT_ID / PASQAL_CLIENT_SECRET / PASQAL_PROJECT_ID env vars");
    };
    eprintln!("device = {:?}", cfg.device);
    let backend = PasqalBackend::new(cfg).expect("build backend");

    // 3-node triangle graph.
    let adjacency: Vec<Vec<(usize, f64)>> = vec![
        vec![(1, 1.0), (2, 1.0)],
        vec![(0, 1.0), (2, 1.0)],
        vec![(0, 1.0), (1, 1.0)],
    ];
    let register =
        build_register(&adjacency, RegisterConstraints::neutral_atom_default()).expect("layout");
    eprintln!("register = {:?}", register);

    let state = QuantumCognitiveState::uniform(3, &[0, 1, 2]);
    let params = EvolutionParams {
        duration_ns: 1000,
        omega_rad_per_us: 1.0,
        detuning_rad_per_us: 0.0,
        phase_rad: 0.0,
        shots: 100,
    };

    let handle = backend
        .submit_evolution(&register, &state, params)
        .await
        .expect("submit");
    eprintln!("submitted: {:?}", handle);

    // Poll up to 5 minutes.
    let deadline = std::time::Instant::now() + Duration::from_secs(300);
    let final_status = loop {
        let s = backend.poll(&handle).await.expect("poll");
        eprintln!("status = {:?}", s);
        match s {
            JobStatus::Done | JobStatus::Canceled | JobStatus::Error => break s,
            _ if std::time::Instant::now() > deadline => {
                panic!("job did not complete within 5 minutes");
            }
            _ => tokio::time::sleep(Duration::from_secs(5)).await,
        }
    };
    assert_eq!(final_status, JobStatus::Done, "job did not finish DONE");

    let results = backend
        .get_results(&handle)
        .await
        .expect("get_results")
        .expect("results present");
    eprintln!(
        "shots = {}, per-atom rydberg probs = {:?}",
        results.shots, results.rydberg_probs
    );
    assert_eq!(results.rydberg_probs.len(), 3);
    assert!(results.shots > 0);
}

/// T0 smoke: verify the public types compile and `build_sequence_json`
/// produces a well-formed register. Runs in the default (non-ignored) set so
/// `cargo test --features quantum-pasqal` covers at least a shape check.
#[test]
fn public_api_shape() {
    let adj: Vec<Vec<(usize, f64)>> =
        vec![vec![(1, 1.0)], vec![(0, 1.0), (2, 1.0)], vec![(1, 1.0)]];
    let reg = build_register(&adj, RegisterConstraints::neutral_atom_default()).unwrap();
    assert_eq!(reg.len(), 3);
}
