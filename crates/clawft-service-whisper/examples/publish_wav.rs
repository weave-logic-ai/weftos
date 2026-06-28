//! Test harness: publish a WAV file's PCM into substrate as pcm_chunk
//! events, driving the whisper service without needing the ESP32.
//!
//! # Status (WEFT-237)
//!
//! Kept as a long-lived dev/operator harness for the canonical
//! substrate-side STT path (see ADR-053). It exercises the full
//! sensor → PCM → window → whisper → transcript loop in-process,
//! making it the lowest-friction reproducer when the live deployment
//! is misbehaving and the easiest entry point for new contributors
//! who need to see the substrate STT contract end-to-end without
//! standing up sensor hardware.
//!
//! Do not delete: it underpins manual triage of the canonical STT
//! path and serves as live documentation of the
//! `substrate/_derived/transcript/<source-node-id>/mic` shape.
//!
//! # Usage
//!
//! ```bash
//! # Defaults: chunk_ms=500, window_ms=2000, whisper URL from env or localhost
//! cargo run -p clawft-service-whisper --example publish_wav -- \
//!     crates/clawft-service-whisper/tests/fixtures/jfk.wav
//! ```
//!
//! The example spins up an in-process [`SubstrateService`] + the
//! [`WhisperService`], then publishes the WAV's PCM to
//! [`clawft_service_whisper::SUBSTRATE_PCM_INPUT_PATH`] in chunks. It
//! prints every transcript the service emits on
//! the configured `output_path_derived` until the WAV is exhausted +
//! a short grace window elapses.
//!
//! This is deliberately a stand-alone example rather than wired into
//! the daemon. Wiring to the live daemon is a single additional step
//! (see README / journal §"service registration") once the operator
//! decides to run it in-process vs. as a sidecar.

use std::env;
use std::fs;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use clawft_kernel::SubstrateService;
use clawft_service_whisper::{
    SUBSTRATE_PCM_INPUT_PATH, WhisperClient, WhisperConfig, WhisperService, WhisperServiceConfig,
    wav::parse_wav,
};
use serde_json::{Value, json};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,clawft_service_whisper=debug".into()),
        )
        .init();

    let wav_path = env::args()
        .nth(1)
        .ok_or("usage: publish_wav <path-to-wav-file>")?;
    let wav_bytes = fs::read(&wav_path)?;
    let (pcm, sample_rate, channels) = parse_wav(&wav_bytes).map_err(|e| e.to_string())?;
    if channels != 1 {
        eprintln!("warning: WAV has {channels} channels; whisper expects mono. Submitting anyway.");
    }

    println!(
        "publish_wav: {} ({} bytes of PCM, {} Hz, {} ch)",
        wav_path,
        pcm.len(),
        sample_rate,
        channels
    );

    let substrate = SubstrateService::new();
    let client = WhisperClient::new(WhisperConfig::from_env())?;
    let whisper_url = client.config().base_url.clone();
    // The default config issues a `transcript` grant for `n-test00`
    // against its own embedded `NodeRegistry`, so the example's
    // canonical publish lands without further wiring.
    let cfg = WhisperServiceConfig::default();
    let transcript_path = cfg.output_path_derived.clone();
    let service = WhisperService::spawn(substrate.clone(), client, cfg)?;
    println!("publish_wav: whisper service spawned against {whisper_url}");

    // Tap the transcript output to print what the service publishes.
    let (_id, mut transcript_rx) = substrate
        .subscribe(Some("publish_wav"), &transcript_path)
        .map_err(|e| e.to_string())?;

    // Background printer.
    let printer = tokio::spawn(async move {
        while let Some(line) = transcript_rx.recv().await {
            let end = if line.last() == Some(&b'\n') {
                line.len() - 1
            } else {
                line.len()
            };
            if let Ok(v) = serde_json::from_slice::<Value>(&line[..end])
                && v["kind"] == "publish"
            {
                let body = &v["value"];
                println!(
                    "[{:>6}..{:<6} ms] {}",
                    body["start_ms"], body["end_ms"], body["text"],
                );
            }
        }
    });

    // Chunk the PCM at 500ms intervals (matches the ESP32 bridge).
    let chunk_ms: u64 = 500;
    let bytes_per_ms: usize = (sample_rate as usize * 2 * channels as usize) / 1_000;
    let chunk_bytes: usize = bytes_per_ms * chunk_ms as usize;

    let mut seq: u64 = 0;
    let mut cursor = 0;
    let start = tokio::time::Instant::now();
    while cursor < pcm.len() {
        let end = (cursor + chunk_bytes).min(pcm.len());
        let slice = &pcm[cursor..end];
        cursor = end;
        seq += 1;

        let payload = json!({
            "pcm_b64": B64.encode(slice),
            "sample_rate": sample_rate,
            "channels": channels,
            "seq": seq,
            "chunk_ms": chunk_ms,
        });
        substrate.publish(Some("publish_wav"), SUBSTRATE_PCM_INPUT_PATH, payload);

        // Pace ourselves to wall-clock so we don't flood: sleep until
        // the (seq * chunk_ms) mark.
        let target = start + Duration::from_millis(seq * chunk_ms);
        if target > tokio::time::Instant::now() {
            tokio::time::sleep_until(target).await;
        }
    }

    println!("publish_wav: PCM exhausted ({seq} chunks). Waiting 5s for last transcripts…");
    tokio::time::sleep(Duration::from_secs(5)).await;

    service.shutdown().await;
    drop(printer);
    Ok(())
}
