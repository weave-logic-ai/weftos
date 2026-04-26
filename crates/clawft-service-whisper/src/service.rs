//! Substrate-connected whisper pipeline.
//!
//! Binds a [`WhisperClient`](crate::WhisperClient) to an in-process
//! [`SubstrateService`]: subscribes to
//! [`crate::SUBSTRATE_PCM_INPUT_PATH`], windows incoming PCM, posts to
//! `/inference`, and publishes transcripts to
//! [`crate::SUBSTRATE_TRANSCRIPT_OUTPUT_PATH`].
//!
//! # Backpressure
//!
//! Per the journal §A5 + service-API §1, whisper serializes one
//! in-flight inference per instance (no 429). This service chooses
//! **drop-oldest** on input: if a new window is ready while
//! [`WhisperClient::transcribe`] is busy, the new window replaces any
//! still-queued window. That biases freshness over completeness — a
//! deliberate choice for live streaming ("what are you saying now" is
//! more valuable than "reconstruct every syllable"). See journal for
//! the alternatives (unbounded queue, block upstream).
//!
//! # Retry
//!
//! 5xx + 503-loading are retriable per API §7 (idempotent at T=0).
//! The service does a single retry with 500ms delay, then drops the
//! window. 4xx is a programmer bug (malformed WAV etc.) so we log +
//! drop immediately without retry.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use clawft_kernel::{SubscriberId, SubstrateService};
use serde_json::{json, Value};
use tokio::sync::{mpsc, watch};
use tracing::{debug, error, info, warn};

use crate::client::{TranscribeError, WhisperClient};
use crate::wav::write_wav;
use crate::windower::{PcmChunk, PcmWindow, Windower};
use crate::SUBSTRATE_PCM_INPUT_PATH;

/// Configuration for [`WhisperService`].
#[derive(Debug, Clone)]
pub struct WhisperServiceConfig {
    /// Target window length in ms. 2000 is the API-doc sweet spot.
    pub window_ms: u64,
    /// Delay between a 5xx/503 response and the retry attempt.
    pub retry_backoff: Duration,
    /// Node id this service publishes under. Required — every
    /// substrate write is node-attributed under the node-identity
    /// gate. Typically set to the daemon's own node-id (the daemon
    /// is "the node running whisper" in the ontology); the output
    /// path must sit under `substrate/<node_id>/...` until the
    /// mesh-canonical gate lands.
    pub node_id: String,
    /// Substrate path to read PCM from.
    pub input_path: String,
    /// Substrate path to write transcripts to. Must start with
    /// `substrate/<node_id>/` for the publish gate to accept it.
    pub output_path: String,
    /// Service-level enable flag. When `false`, the inference loop
    /// drops incoming chunks before windowing — no work is done.
    /// Defaults to a fresh `Arc<AtomicBool>(true)` if you don't
    /// supply one. Caller can keep its own clone of the Arc to
    /// flip the flag at runtime (e.g. from a control-plane RPC).
    pub service_enabled: Arc<AtomicBool>,
    /// Source-sensor enable flag (consumer-side soft-disable). When
    /// `false`, the service keeps its substrate subscription alive
    /// but drops every chunk that arrives — the bridge for "the
    /// firmware is still emitting because it hasn't picked up the
    /// control-path subscribe yet, but the user wants this off."
    /// Defaults to enabled if you don't supply one.
    pub source_enabled: Arc<AtomicBool>,
    /// Optional substrate path of an upstream audio-classifier
    /// publisher (e.g. `clawft-service-classify`). When set, the
    /// service spawns a background task that subscribes to that
    /// path and updates an internal `is_speech` flag from each
    /// classification. The chunk-receive arm then drops chunks
    /// while the flag is `false` (silence) so whisper inference
    /// only runs on speech windows.
    ///
    /// When `None` (the default), the service runs every chunk
    /// through inference unconditionally — preserving the
    /// pre-classifier behaviour for tests and for daemons that
    /// haven't wired the classifier in.
    pub classifier_input: Option<String>,
    /// Stickiness window for a "speech" classification, in ms.
    /// Once the classifier reports speech, the gate stays open for
    /// at least this long after the last speech tick — so we don't
    /// clip the leading silence of a speech window when the
    /// classifier briefly drops back to silence between syllables.
    /// Default 1500 ms is roughly two pcm_chunk periods at the
    /// firmware's 2 Hz cadence; long enough to bridge a normal
    /// inter-syllabic pause, short enough that a sustained quiet
    /// period correctly closes the gate.
    pub gate_window_ms: u64,
}

impl Default for WhisperServiceConfig {
    fn default() -> Self {
        // Defaults are test-friendly. Daemon-side wiring overrides
        // `node_id` (with the daemon's own id) and the paths (with
        // the actual ESP32 node-id for input + the daemon's prefix
        // for output).
        Self {
            window_ms: 2_000,
            retry_backoff: Duration::from_millis(500),
            node_id: "n-test00".to_string(),
            input_path: SUBSTRATE_PCM_INPUT_PATH.to_string(),
            output_path: "substrate/n-test00/derived/transcript/mic".to_string(),
            service_enabled: Arc::new(AtomicBool::new(true)),
            source_enabled: Arc::new(AtomicBool::new(true)),
            classifier_input: None,
            gate_window_ms: 1_500,
        }
    }
}

/// Runtime handle for a spawned whisper service task.
#[derive(Debug)]
pub struct WhisperService {
    shutdown: watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
}

impl WhisperService {
    /// Spawn the pipeline on the tokio runtime of the caller.
    ///
    /// Wiring:
    /// 1. `substrate.subscribe(input_path)` — gets an mpsc of update
    ///    lines (JSON bytes).
    /// 2. Parses each line, pulls `value.pcm_b64` + metadata, feeds a
    ///    [`Windower`].
    /// 3. When a window emits, wraps PCM in WAV, POSTs to whisper.
    /// 4. On success, publishes transcript to `output_path`.
    ///
    /// # Lifecycle
    ///
    /// The returned [`WhisperService`] owns a watch-channel shutdown
    /// signal. Call [`Self::shutdown`] to stop cleanly; the internal
    /// task drains the in-flight HTTP request before exiting.
    ///
    /// # Errors
    ///
    /// Returns `Err` only if the initial `substrate.subscribe` call
    /// fails egress gating. Runtime errors (HTTP 5xx, malformed chunks,
    /// whisper-service-down) are logged + absorbed.
    pub fn spawn(
        substrate: SubstrateService,
        client: WhisperClient,
        config: WhisperServiceConfig,
    ) -> Result<Self, String> {
        // Subscribe under the configured node id so capture-tier
        // egress accepts the read. The egress layer requires *any*
        // non-None caller for capture paths, not a specific role.
        let (id, rx) = substrate
            .subscribe(Some(&config.node_id), &config.input_path)
            .map_err(|e| format!("substrate subscribe failed: {e}"))?;
        info!(
            sub_id = id.0,
            path = %config.input_path,
            window_ms = config.window_ms,
            whisper_url = %client.config().base_url,
            classifier_input = ?config.classifier_input,
            gate_window_ms = config.gate_window_ms,
            "whisper service: subscribed to PCM input"
        );

        // Optional classifier-gate state. Two atomics:
        //   - is_speech: latest "speech" verdict from the classifier
        //   - last_speech_ms: monotonic ms of the last speech verdict
        // The pipeline reads both to apply the sticky-window rule.
        // When `classifier_input` is None, both stay at their default
        // values (false / 0) and the pipeline ignores them entirely.
        let is_speech = Arc::new(AtomicBool::new(false));
        let last_speech_ms = Arc::new(AtomicU64::new(0));
        let classifier_unsub: Option<(String, SubscriberId)> =
            if let Some(path) = config.classifier_input.clone() {
                match substrate.subscribe(Some(&config.node_id), &path) {
                    Ok((cid, crx)) => {
                        info!(
                            sub_id = cid.0,
                            path = %path,
                            "whisper service: subscribed to classifier output"
                        );
                        let is_speech_clone = Arc::clone(&is_speech);
                        let last_speech_ms_clone = Arc::clone(&last_speech_ms);
                        tokio::spawn(classifier_subscriber_loop(
                            crx,
                            is_speech_clone,
                            last_speech_ms_clone,
                        ));
                        Some((path, cid))
                    }
                    Err(e) => {
                        warn!(
                            err = %e,
                            path = %path,
                            "whisper service: classifier subscribe failed; \
                             gating disabled (every chunk will be transcribed)"
                        );
                        None
                    }
                }
            } else {
                None
            };

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let substrate_for_task = substrate.clone();
        let input_path_cleanup = config.input_path.clone();
        let is_speech_for_task = Arc::clone(&is_speech);
        let last_speech_for_task = Arc::clone(&last_speech_ms);
        let task = tokio::spawn(async move {
            run_pipeline(
                rx,
                substrate_for_task.clone(),
                client,
                config,
                shutdown_rx,
                is_speech_for_task,
                last_speech_for_task,
            )
            .await;
            // Clean up the subscription on exit (idempotent).
            substrate_for_task.unsubscribe(&input_path_cleanup, id);
            if let Some((cpath, cid)) = classifier_unsub {
                substrate_for_task.unsubscribe(&cpath, cid);
            }
        });
        Ok(Self {
            shutdown: shutdown_tx,
            task,
        })
    }

    /// Signal shutdown and await the internal task.
    pub async fn shutdown(self) {
        let _ = self.shutdown.send(true);
        let _ = self.task.await;
    }
}

async fn run_pipeline(
    mut rx: mpsc::Receiver<Vec<u8>>,
    substrate: SubstrateService,
    client: WhisperClient,
    config: WhisperServiceConfig,
    mut shutdown_rx: watch::Receiver<bool>,
    is_speech: Arc<AtomicBool>,
    last_speech_ms: Arc<AtomicU64>,
) {
    // Health probe is fire-and-forget: if whisper isn't up the service
    // still stays subscribed and will start processing once POSTs
    // start succeeding. See journal §"degraded-but-alive".
    if !client.wait_for_healthy().await {
        warn!(
            base_url = %client.config().base_url,
            "whisper service: starting in degraded mode (service not reachable)"
        );
    } else {
        info!(base_url = %client.config().base_url, "whisper service: ready");
    }

    let mut windower = Windower::new(config.window_ms);

    // Drop-oldest policy: a single slot for the pending window. When
    // the inference task is free it takes the slot; new windows
    // overwrite the slot if busy.
    let mut pending: Option<PcmWindow> = None;
    let mut in_flight: Option<tokio::task::JoinHandle<(
        PcmWindow,
        Result<crate::client::InferenceResponse, TranscribeError>,
    )>> = None;

    loop {
        tokio::select! {
            changed = shutdown_rx.changed() => {
                if changed.is_ok() && *shutdown_rx.borrow() {
                    debug!("whisper service: shutdown requested");
                    break;
                }
            }
            line = rx.recv() => {
                let Some(bytes) = line else {
                    debug!("whisper service: substrate sender dropped, exiting");
                    break;
                };
                // Control-plane gate: drop the chunk before any work
                // is done if either the service or the source sensor
                // has been toggled off. Subscription stays live so a
                // re-enable picks up the next chunk seamlessly.
                if !config.service_enabled.load(Ordering::SeqCst) {
                    debug!("whisper service: chunk dropped (service disabled)");
                    continue;
                }
                if !config.source_enabled.load(Ordering::SeqCst) {
                    debug!("whisper service: chunk dropped (source sensor disabled)");
                    continue;
                }
                // Classifier gate. Only evaluated when an upstream
                // classifier was configured; otherwise every chunk
                // proceeds (preserves the pre-classifier behaviour).
                //
                // Sticky-window rule: a chunk is allowed through if
                // EITHER the latest classification is still "speech"
                // OR a "speech" verdict landed within the last
                // `gate_window_ms` ms — bridging the inter-syllabic
                // pauses where the classifier flips back to silence.
                if config.classifier_input.is_some()
                    && !is_speech_allowed(
                        &is_speech,
                        &last_speech_ms,
                        config.gate_window_ms,
                    )
                {
                    debug!("whisper service: chunk dropped (classifier says silence)");
                    continue;
                }
                if let Some(chunk) = decode_update_line(&bytes) {
                    match decode_pcm_chunk(&chunk) {
                        Ok((pcm, sr, ch, seq, chunk_ms)) => {
                            if let Some(win) = windower.push(&pcm, sr, ch, seq, chunk_ms) {
                                // Drop-oldest: replace any pending window
                                // that hasn't been picked up yet.
                                if pending.replace(win).is_some() {
                                    warn!(
                                        "whisper service: dropped oldest window (in-flight whisper request still running)"
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            debug!(err = %e, "whisper service: skipping malformed chunk");
                        }
                    }
                }
            }
            finished = async {
                match in_flight.as_mut() {
                    Some(h) => h.await.ok(),
                    // Park forever — select! only polls this arm when
                    // `in_flight` is Some.
                    None => std::future::pending::<Option<_>>().await,
                }
            }, if in_flight.is_some() => {
                in_flight = None;
                if let Some((window, result)) = finished {
                    handle_inference_result(&substrate, &config, window, result).await;
                }
            }
        }

        // If the HTTP worker is free and a window is pending, launch.
        if in_flight.is_none()
            && let Some(window) = pending.take() {
                let client_clone = client.clone();
                let window_clone = window.clone();
                let retry_backoff = config.retry_backoff;
                in_flight = Some(tokio::spawn(async move {
                    let result = run_one_inference(&client_clone, &window_clone, retry_backoff).await;
                    (window_clone, result)
                }));
            }
    }

    // On shutdown: flush any partial window synchronously for the
    // last-gasp transcript, then await an in-flight request.
    if let Some(partial) = windower.flush() {
        let result = run_one_inference(&client, &partial, config.retry_backoff).await;
        handle_inference_result(&substrate, &config, partial, result).await;
    }
    if let Some(h) = in_flight.take()
        && let Ok((window, result)) = h.await {
            handle_inference_result(&substrate, &config, window, result).await;
        }
}

/// Monotonic milliseconds since process start. Used by the
/// classifier-gate stickiness window so we don't depend on system
/// wall-clock for a "did speech land in the last N ms" check.
fn monotonic_ms() -> u64 {
    use std::sync::OnceLock;
    use std::time::Instant;
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    let epoch = *EPOCH.get_or_init(Instant::now);
    epoch.elapsed().as_millis() as u64
}

/// Read the gate state and decide whether to let a chunk through.
///
/// Allows iff (a) the latest classification is currently "speech" OR
/// (b) the most recent "speech" verdict landed within the last
/// `gate_window_ms` ms. Both conditions checked atomically; no lock.
fn is_speech_allowed(
    is_speech: &AtomicBool,
    last_speech_ms: &AtomicU64,
    gate_window_ms: u64,
) -> bool {
    if is_speech.load(Ordering::SeqCst) {
        return true;
    }
    let last = last_speech_ms.load(Ordering::SeqCst);
    if last == 0 {
        return false;
    }
    let now = monotonic_ms();
    now.saturating_sub(last) <= gate_window_ms
}

/// Background task: subscribe to the classifier output path and
/// update the gate flags as classifications arrive.
///
/// The classifier publishes a stable JSON shape (see
/// `clawft-service-classify::Classification`); we only read `class`
/// here. Any other class string than `"speech"` (e.g. `"silence"`,
/// future `"music"` / `"noise"`) closes the gate — a future
/// "should we transcribe music?" knob can read additional fields
/// at the call site without reshaping this loop.
async fn classifier_subscriber_loop(
    mut rx: mpsc::Receiver<Vec<u8>>,
    is_speech: Arc<AtomicBool>,
    last_speech_ms: Arc<AtomicU64>,
) {
    while let Some(line) = rx.recv().await {
        let Some(value) = decode_update_line(&line) else { continue };
        let speech = value
            .get("class")
            .and_then(|v| v.as_str())
            .map(|s| s == "speech")
            .unwrap_or(false);
        is_speech.store(speech, Ordering::SeqCst);
        if speech {
            last_speech_ms.store(monotonic_ms(), Ordering::SeqCst);
        }
    }
    debug!("whisper service: classifier subscriber loop exiting (sender dropped)");
}

/// Single inference call with one in-line retry for retriable errors.
///
/// API doc §7: at `temperature=0` `/inference` is idempotent, so the
/// retry is safe. We cap at one retry to avoid burning the whisper
/// single-in-flight mutex on a genuinely sick service.
async fn run_one_inference(
    client: &WhisperClient,
    window: &PcmWindow,
    retry_backoff: Duration,
) -> Result<crate::client::InferenceResponse, TranscribeError> {
    let wav = write_wav(&window.pcm_s16le, window.sample_rate, window.channels);
    match client.transcribe(wav.clone()).await {
        Ok(r) => Ok(r),
        Err(e) if e.is_retriable() => {
            debug!(err = %e, "whisper service: retriable error, one retry");
            tokio::time::sleep(retry_backoff).await;
            client.transcribe(wav).await
        }
        Err(e) => Err(e),
    }
}

async fn handle_inference_result(
    substrate: &SubstrateService,
    config: &WhisperServiceConfig,
    window: PcmWindow,
    result: Result<crate::client::InferenceResponse, TranscribeError>,
) {
    match result {
        Ok(r) => {
            let payload = json!({
                "text": r.text,
                "start_ms": window.start_ms,
                "end_ms": window.end_ms,
                // The `json` response format doesn't carry per-segment
                // confidence; verbose_json would. Keeping `null` so
                // downstream object-type shape is stable when we later
                // flip the format.
                "confidence": Value::Null,
                "lang": "en",
                "seq": window.last_seq,
            });
            // Run through the node-identity gate. Output path must
            // sit under `substrate/<node_id>/...` (config-checked by
            // the gate); WhisperServiceConfig builders are
            // responsible for that. Mesh-canonical placement
            // (`substrate/_derived/transcript/...`) requires the
            // capability path that ships with the next gate slice.
            match substrate.publish_gated(
                Some(&config.node_id),
                &config.output_path,
                payload,
            ) {
                Ok(tick) => info!(
                    tick,
                    start_ms = window.start_ms,
                    end_ms = window.end_ms,
                    seq = window.last_seq,
                    "whisper service: transcript published"
                ),
                Err(e) => error!(
                    err = %e,
                    output_path = %config.output_path,
                    node_id = %config.node_id,
                    "whisper service: gate denied transcript publish"
                ),
            }
        }
        Err(e) => {
            error!(
                err = %e,
                start_ms = window.start_ms,
                end_ms = window.end_ms,
                "whisper service: transcription failed (window dropped)"
            );
        }
    }
}

/// Parse a substrate-subscribe update line.
///
/// Shape (see `clawft_kernel::substrate_service::build_update_line`):
///
/// ```json
/// {"path":"…","tick":N,"kind":"publish|notify","value":{…},"actor_id":…}\n
/// ```
///
/// Returns the `value` field when `kind == "publish"`, else `None`.
fn decode_update_line(line: &[u8]) -> Option<Value> {
    // Strip trailing newline if present.
    let end = if line.last() == Some(&b'\n') {
        line.len() - 1
    } else {
        line.len()
    };
    let v: Value = serde_json::from_slice(&line[..end]).ok()?;
    if v.get("kind")?.as_str()? != "publish" {
        return None;
    }
    Some(v.get("value")?.clone())
}

/// Decode a single [`PcmChunk`] JSON value into raw bytes + metadata.
///
/// Only base64 + i16le are supported today. Chunks declaring a
/// different encoding/format are rejected with a descriptive error
/// so future protocol changes surface loudly instead of silently
/// dropping data.
fn decode_pcm_chunk(value: &Value) -> Result<(Vec<u8>, u32, u16, u64, u64), String> {
    let chunk: PcmChunk = serde_json::from_value(value.clone())
        .map_err(|e| format!("not a PcmChunk: {e}"))?;
    if chunk.encoding != "base64" {
        return Err(format!("unsupported encoding: {:?} (want \"base64\")", chunk.encoding));
    }
    if chunk.format != "i16le" {
        return Err(format!("unsupported format: {:?} (want \"i16le\")", chunk.format));
    }
    let pcm = B64
        .decode(chunk.data.as_bytes())
        .map_err(|e| format!("data b64 decode: {e}"))?;
    let chunk_ms = chunk.effective_chunk_ms();
    Ok((pcm, chunk.sample_rate, chunk.channels, chunk.start_ts_ms, chunk_ms))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::WhisperConfig;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn make_client(base_url: String) -> WhisperClient {
        WhisperClient::new(WhisperConfig {
            base_url,
            request_timeout: Duration::from_secs(5),
            health_deadline: Duration::from_millis(200),
            ..Default::default()
        })
        .unwrap()
    }

    fn publish_pcm_chunk(
        substrate: &SubstrateService,
        actor_id: &str,
        path: &str,
        pcm: &[u8],
        chunk_ms: u64,
        seq: u64,
    ) {
        // Mirror the firmware wire shape: data + encoding + format
        // + samples + start_ts_ms. `samples` is derived from the
        // requested chunk_ms so the windower's accumulation timing
        // stays test-deterministic.
        let samples = (chunk_ms * 16_000) / 1000;
        let payload = json!({
            "data": B64.encode(pcm),
            "encoding": "base64",
            "format": "i16le",
            "sample_rate": 16_000,
            "channels": 1,
            "samples": samples,
            "start_ts_ms": seq,
        });
        substrate.publish(Some(actor_id), path, payload);
    }

    #[tokio::test]
    async fn decode_update_line_extracts_publish_value() {
        let raw = br#"{"path":"p","tick":1,"kind":"publish","value":{"x":1},"actor_id":null}"#;
        let v = decode_update_line(raw).unwrap();
        assert_eq!(v["x"], 1);
    }

    #[tokio::test]
    async fn decode_update_line_ignores_notify() {
        let raw = br#"{"path":"p","tick":1,"kind":"notify","value":null,"actor_id":null}"#;
        assert!(decode_update_line(raw).is_none());
    }

    #[tokio::test]
    async fn decode_pcm_chunk_roundtrips() {
        let pcm = vec![1u8, 2, 3, 4];
        let v = json!({
            "pcm_b64": B64.encode(&pcm),
            "sample_rate": 16_000,
            "channels": 1,
            "seq": 7,
            "chunk_ms": 500,
        });
        let (out, sr, ch, seq, ms) = decode_pcm_chunk(&v).unwrap();
        assert_eq!(out, pcm);
        assert_eq!(sr, 16_000);
        assert_eq!(ch, 1);
        assert_eq!(seq, 7);
        assert_eq!(ms, 500);
    }

    /// Full pipeline: substrate → windower → mocked whisper → substrate.
    ///
    /// This is the hermetic end-to-end test; no live service needed.
    #[tokio::test]
    async fn end_to_end_with_mocked_whisper() {
        // Mocked whisper: both endpoints.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"status":"ok"}"#))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/inference"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(r#"{"text": " unit test speaks"}"#),
            )
            .mount(&server)
            .await;

        let substrate = SubstrateService::new();
        let client = make_client(server.uri());
        // Shorter window so the test fires quickly.
        let cfg = WhisperServiceConfig {
            window_ms: 500,
            ..Default::default()
        };
        let input_path = cfg.input_path.clone();
        let output_path = cfg.output_path.clone();
        let actor = cfg.node_id.clone();

        // Pre-subscribe to the OUTPUT to catch the transcript. Must be
        // done before pumping input so we don't race the first
        // transcript publish.
        let (_out_id, mut out_rx) = substrate.subscribe(Some(&actor), &output_path).unwrap();

        let svc = WhisperService::spawn(substrate.clone(), client, cfg).unwrap();

        // Push 500ms worth of silence at 16kHz mono s16le = 16000 bytes.
        // Use two 250ms chunks to exercise the windower's accumulation path.
        let half = vec![0u8; 8_000];
        publish_pcm_chunk(&substrate, &actor, &input_path, &half, 250, 1);
        publish_pcm_chunk(&substrate, &actor, &input_path, &half, 250, 2);

        // Wait up to 3s for a transcript to show up on the output path.
        let got = tokio::time::timeout(Duration::from_secs(3), out_rx.recv()).await;
        let line = got.expect("transcript not published within 3s").expect("substrate closed");
        let update: Value = serde_json::from_slice(&line[..line.len() - 1]).unwrap();
        assert_eq!(update["kind"], "publish");
        assert_eq!(update["path"], output_path);
        let body = &update["value"];
        assert_eq!(body["text"], "unit test speaks");
        assert_eq!(body["start_ms"], 0);
        assert_eq!(body["end_ms"], 500);
        assert_eq!(body["seq"], 2);
        assert_eq!(body["lang"], "en");
        assert!(body["confidence"].is_null());

        svc.shutdown().await;
    }

    #[tokio::test]
    async fn service_survives_whisper_down_at_start() {
        // No mock server at all — reqwest will fail the health probe
        // and every /inference. Service must still spawn and exit
        // cleanly on shutdown.
        let substrate = SubstrateService::new();
        let client = make_client("http://127.0.0.1:1".into()); // unreachable
        let cfg = WhisperServiceConfig {
            window_ms: 500,
            ..Default::default()
        };
        let svc = WhisperService::spawn(substrate, client, cfg).unwrap();
        // Tiny delay to let the pipeline enter its main select!.
        tokio::time::sleep(Duration::from_millis(100)).await;
        svc.shutdown().await;
    }

    #[tokio::test]
    async fn disabled_service_flag_drops_chunks_no_inference() {
        // POST /inference must NOT be called while service_enabled = false.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"status":"ok"}"#))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/inference"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(r#"{"text": " not heard"}"#),
            )
            .expect(0) // ← key assertion: zero inference calls
            .mount(&server)
            .await;

        let substrate = SubstrateService::new();
        let client = make_client(server.uri());
        let service_enabled = Arc::new(AtomicBool::new(false)); // start disabled
        let cfg = WhisperServiceConfig {
            window_ms: 500,
            service_enabled: Arc::clone(&service_enabled),
            ..Default::default()
        };
        let input_path = cfg.input_path.clone();
        let actor = cfg.node_id.clone();
        let svc = WhisperService::spawn(substrate.clone(), client, cfg).unwrap();

        // Pump in three windows worth of audio while disabled.
        let half = vec![0u8; 8_000];
        for i in 0..3 {
            publish_pcm_chunk(&substrate, &actor, &input_path, &half, 250, i + 1);
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        // Give the pipeline a moment to settle. wiremock's
        // .expect(0) verifies on drop.
        tokio::time::sleep(Duration::from_millis(200)).await;

        svc.shutdown().await;
        // Implicit assertion: server drop checks .expect(0) and
        // panics if any /inference call landed.
    }

    #[tokio::test]
    async fn disabled_source_flag_drops_chunks_no_inference() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"status":"ok"}"#))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/inference"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"text": "x"}"#))
            .expect(0)
            .mount(&server)
            .await;

        let substrate = SubstrateService::new();
        let client = make_client(server.uri());
        let source_enabled = Arc::new(AtomicBool::new(false));
        let cfg = WhisperServiceConfig {
            window_ms: 500,
            source_enabled: Arc::clone(&source_enabled),
            ..Default::default()
        };
        let input_path = cfg.input_path.clone();
        let actor = cfg.node_id.clone();
        let svc = WhisperService::spawn(substrate.clone(), client, cfg).unwrap();

        let half = vec![0u8; 8_000];
        for i in 0..3 {
            publish_pcm_chunk(&substrate, &actor, &input_path, &half, 250, i + 1);
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        svc.shutdown().await;
    }

    #[tokio::test]
    async fn drops_oldest_window_when_inference_slow() {
        // Mock /inference to hang for 2s. Feed three windows in
        // quick succession; the service should drop at least one
        // mid-window instead of queueing them all.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"status":"ok"}"#))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/inference"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_millis(1_500))
                    .set_body_string(r#"{"text": " slow"}"#),
            )
            .mount(&server)
            .await;

        let substrate = SubstrateService::new();
        let client = make_client(server.uri());
        let cfg = WhisperServiceConfig {
            window_ms: 200,
            ..Default::default()
        };
        let input_path = cfg.input_path.clone();
        let actor = cfg.node_id.clone();
        let svc = WhisperService::spawn(substrate.clone(), client, cfg).unwrap();

        // Feed five 200ms windows back-to-back. Most should be dropped
        // (only the first kicks off a 1.5s inference; windows 2–5
        // overwrite the pending slot).
        for i in 0..5 {
            let buf = vec![0u8; 6_400]; // 200ms at 16kHz mono s16le
            publish_pcm_chunk(&substrate, &actor, &input_path, &buf, 200, i + 1);
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        // Let the service settle.
        tokio::time::sleep(Duration::from_millis(300)).await;
        // We don't assert a specific drop count — timing varies by CI —
        // but we do assert the service didn't stall: shutdown must
        // complete without hanging forever.
        tokio::time::timeout(Duration::from_secs(5), svc.shutdown())
            .await
            .expect("shutdown hung — the pipeline did not drain");
    }

    /// Helper for the classifier-gate tests: publish a fake
    /// `Classification`-shaped value at the given path. Mirrors what
    /// `clawft-service-classify`'s service publishes, but without
    /// taking a build-time edge into the classify crate (which would
    /// flip the dependency direction we want — whisper does not
    /// depend on classify).
    fn publish_classification(
        substrate: &SubstrateService,
        actor: &str,
        path: &str,
        class: &str,
    ) {
        let payload = json!({
            "class": class,
            "confidence": 1.0,
            "rms_db": -10.0,
            "sample_rate": 16_000,
            "samples": 8_000,
            "ts_ms": 0,
            "source_node": "n-bfc4cd",
            "source_seq": 0,
        });
        substrate.publish(Some(actor), path, payload);
    }

    #[tokio::test]
    async fn whisper_skips_chunk_when_classifier_says_silence() {
        // Wire up a classifier-input path; publish a `silence`
        // classification first so the gate is closed; then pump pcm
        // and assert /inference is never hit.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"status":"ok"}"#))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/inference"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(r#"{"text": " heard"}"#),
            )
            .expect(0) // gate must keep us at zero
            .mount(&server)
            .await;

        let substrate = SubstrateService::new();
        let client = make_client(server.uri());
        let classifier_path = "substrate/n-test00/derived/classify/n-bfc4cd/mic".to_string();
        let cfg = WhisperServiceConfig {
            window_ms: 500,
            classifier_input: Some(classifier_path.clone()),
            gate_window_ms: 1_500,
            ..Default::default()
        };
        let input_path = cfg.input_path.clone();
        let actor = cfg.node_id.clone();
        let svc = WhisperService::spawn(substrate.clone(), client, cfg).unwrap();

        // Publish silence so the gate is unambiguously closed.
        publish_classification(&substrate, &actor, &classifier_path, "silence");
        // Yield so the classifier-subscriber loop ingests it before
        // we start pumping audio.
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Pump audio that WOULD trigger inference if the gate were
        // open (two 250 ms chunks → one 500 ms window).
        let half = vec![0u8; 8_000];
        publish_pcm_chunk(&substrate, &actor, &input_path, &half, 250, 1);
        publish_pcm_chunk(&substrate, &actor, &input_path, &half, 250, 2);
        tokio::time::sleep(Duration::from_millis(300)).await;

        svc.shutdown().await;
        // Implicit assertion: wiremock .expect(0) panics on drop if
        // any /inference call landed.
    }

    #[tokio::test]
    async fn whisper_processes_chunk_when_classifier_says_speech() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"status":"ok"}"#))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/inference"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(r#"{"text": " gated speech"}"#),
            )
            .mount(&server)
            .await;

        let substrate = SubstrateService::new();
        let client = make_client(server.uri());
        let classifier_path = "substrate/n-test00/derived/classify/n-bfc4cd/mic".to_string();
        let cfg = WhisperServiceConfig {
            window_ms: 500,
            classifier_input: Some(classifier_path.clone()),
            gate_window_ms: 1_500,
            ..Default::default()
        };
        let input_path = cfg.input_path.clone();
        let output_path = cfg.output_path.clone();
        let actor = cfg.node_id.clone();
        let (_oid, mut out_rx) = substrate.subscribe(Some(&actor), &output_path).unwrap();
        let svc = WhisperService::spawn(substrate.clone(), client, cfg).unwrap();

        // Open the gate.
        publish_classification(&substrate, &actor, &classifier_path, "speech");
        tokio::time::sleep(Duration::from_millis(50)).await;

        let half = vec![0u8; 8_000];
        publish_pcm_chunk(&substrate, &actor, &input_path, &half, 250, 1);
        publish_pcm_chunk(&substrate, &actor, &input_path, &half, 250, 2);

        let line = tokio::time::timeout(Duration::from_secs(3), out_rx.recv())
            .await
            .expect("transcript not published within 3s")
            .expect("substrate sender closed");
        let update: Value = serde_json::from_slice(&line[..line.len() - 1]).unwrap();
        assert_eq!(update["kind"], "publish");
        assert_eq!(update["value"]["text"], "gated speech");

        svc.shutdown().await;
    }

    #[tokio::test]
    async fn is_speech_stickiness_works_within_window() {
        // Pure-unit test of the gate function — no substrate / no HTTP.
        // Verifies: speech=true → allow; speech=false but recent →
        // allow; speech=false and stale → deny.
        //
        // We use a deliberately tiny `gate_window_ms` (50) and
        // sleep > that interval to make the staleness branch
        // observable without depending on absolute clock values.
        // (The function reads `monotonic_ms()` internally, so we
        // can't fully fake the clock; we test the *behaviour* with
        // real elapsed time.)
        let is_speech = AtomicBool::new(false);
        let last = AtomicU64::new(0);
        let window = 50u64;

        // No prior speech → denied.
        assert!(!is_speech_allowed(&is_speech, &last, window));

        // Speech currently true → allowed regardless of timestamp.
        is_speech.store(true, Ordering::SeqCst);
        assert!(is_speech_allowed(&is_speech, &last, window));

        // Speech now false but a recent verdict just landed → allowed.
        // Prime the monotonic clock first so subsequent reads return
        // a non-zero value (the function treats `last == 0` as a
        // sentinel for "never set").
        let _ = monotonic_ms();
        tokio::time::sleep(Duration::from_millis(10)).await;
        is_speech.store(false, Ordering::SeqCst);
        let just_now = monotonic_ms();
        assert!(just_now > 0, "monotonic_ms should have advanced past 0");
        last.store(just_now, Ordering::SeqCst);
        assert!(
            is_speech_allowed(&is_speech, &last, window),
            "fresh verdict should keep gate open within window (just_now={just_now})"
        );

        // Sleep past the window. The verdict is now stale → denied.
        tokio::time::sleep(Duration::from_millis(window + 50)).await;
        assert!(
            !is_speech_allowed(&is_speech, &last, window),
            "stale verdict should not keep gate open after window+slack ms",
        );
    }
}
