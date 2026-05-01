//! Microphone reference adapter — first physical-sensor adapter for
//! WeftOS substrate.
//!
//! Reads signed-16-bit little-endian mono PCM samples from a
//! configurable byte source (default: a file backing that a test
//! harness or a future host-audio bridge can feed) and emits RMS +
//! peak levels into `substrate/sensor/mic` every 500 ms.
//!
//! ## Characterization level (spectrometer principle)
//!
//! This adapter declares [`Characterization::Rate`]: it produces a
//! scalar level per window with no spectral structure. It cannot
//! distinguish a 440 Hz tone from pink noise at the same RMS. That
//! distinction is the upgrade path to `Characterization::Spectral`
//! — add an FFT pass and emit per-bin magnitudes alongside the
//! scalar. The adapter surface stays the same; consumers that care
//! about spectral content check `characterization` in the emitted
//! object.
//!
//! ## File-backed source (today) → host-audio source (next)
//!
//! The backing source is a plain raw-PCM file. Tests write a known
//! buffer to a tempfile and assert against the computed RMS. The
//! path is parameterised at construction so the same adapter can be
//! hooked up to:
//!
//! - a synthetic stream for dev panels (e.g. mix a test tone)
//! - a host-audio bridge writing live capture bytes into a named
//!   pipe or `/dev/shm/weftos/mic/stream.raw`
//! - a CPAL / ALSA / CoreAudio / WASAPI shim (future — this is what
//!   lights up the webcam mic on the user's own machine)
//!
//! No host-audio dep in this slice. Keeps the preview reviewable.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};

use crate::adapter::{
    AdapterError, BufferPolicy, OntologyAdapter, PermissionReq, RefreshHint, Sensitivity, SubId,
    Subscription, TopicDecl,
};
use crate::delta::StateDelta;
use crate::healthcheck::{
    build_report_delta as build_health_delta, SensorHealthReport, SensorStatus,
};
use crate::physical::{
    Characterization, PhysicalSensorAdapter, SensorCalibration, SensorInterface,
};
// Used only by `tests::host_audio_direction_variant_compiles` via
// `use super::*` — keep the import so `cargo test` builds.
#[cfg(test)]
#[allow(unused_imports)]
use crate::physical::AudioDirection;

/// Window size in samples read per tick. At 16 kHz / 500 ms that's
/// 8000 samples = 16 000 bytes.
const WINDOW_SAMPLES: usize = 8000;
/// Default sample rate assumed when none is configured.
const DEFAULT_SAMPLE_RATE: u32 = 16_000;
/// Channel depth for the singleton topic. Sized to comfortably hold
/// the per-tick payload-plus-health pair (2 deltas) plus one tick of
/// slack for slow consumers.
const CHAN: usize = 4;
/// Emission cadence — matches WINDOW_SAMPLES at DEFAULT_SAMPLE_RATE.
const TICK_MS: u64 = 500;

/// Declared topics.
///
/// The mic adapter declares two topics:
/// - `substrate/sensor/mic` — the level-meter payload (RMS / peak dBFS).
/// - `substrate/meta/adapter/mic/healthcheck` — the per-sensor health
///   report (HEALTHCHECK-CONTRACT.md §3). Both are emitted from the
///   same poller; opening either subscribes to the shared producer
///   loop. The healthcheck path uses [`Sensitivity::Public`] because
///   it carries no audio content — only liveness counters.
pub const TOPICS: &[TopicDecl] = &[
    TopicDecl {
        path: "substrate/sensor/mic",
        shape: "ontology://audio-level",
        refresh_hint: RefreshHint::Periodic { ms: TICK_MS },
        // Audio-capture data CAN leak user content even at RMS level
        // (speech envelope is recoverable). `Capture` sensitivity per
        // ADR-012 forces a per-goal `CapabilityGrant` rather than a
        // one-off install-time prompt.
        sensitivity: Sensitivity::Capture,
        buffer_policy: BufferPolicy::Refuse,
        max_len: None,
    },
    TopicDecl {
        path: "substrate/meta/adapter/mic/healthcheck",
        shape: "ontology://sensor-health-report",
        refresh_hint: RefreshHint::Periodic { ms: TICK_MS },
        sensitivity: Sensitivity::Public,
        buffer_policy: BufferPolicy::Refuse,
        max_len: None,
    },
];

/// Permissions — capture requires a dedicated grant; M1.5.2+ will
/// wire that through governance. For the preview, `open` still
/// proceeds but the sensitivity label on the topic alerts downstream.
pub const PERMISSIONS: &[PermissionReq] = &[];

type CancelTx = oneshot::Sender<()>;

struct Registry {
    next_id: u64,
    live: HashMap<SubId, CancelTx>,
}

impl Registry {
    fn new() -> Self {
        Self {
            next_id: 1,
            live: HashMap::new(),
        }
    }

    fn allocate(&mut self) -> SubId {
        let id = SubId(self.next_id);
        self.next_id = self.next_id.wrapping_add(1);
        id
    }
}

/// Microphone adapter.
pub struct MicrophoneAdapter {
    reg: Mutex<Registry>,
    source_path: PathBuf,
    sample_rate: u32,
}

impl Default for MicrophoneAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl MicrophoneAdapter {
    /// Build an adapter reading from the default file-backed source
    /// at `/tmp/weftos/mic/stream.raw`, 16 kHz mono s16le.
    pub fn new() -> Self {
        Self::with_source(
            PathBuf::from("/tmp/weftos/mic/stream.raw"),
            DEFAULT_SAMPLE_RATE,
        )
    }

    /// Build with an explicit source path + sample rate — used by
    /// tests (fake PCM fixture) and by a future synthetic-stream
    /// generator.
    pub fn with_source(source_path: PathBuf, sample_rate: u32) -> Self {
        Self {
            reg: Mutex::new(Registry::new()),
            source_path,
            sample_rate,
        }
    }
}

#[async_trait]
impl OntologyAdapter for MicrophoneAdapter {
    fn id(&self) -> &'static str {
        "mic"
    }

    fn topics(&self) -> &'static [TopicDecl] {
        TOPICS
    }

    fn permissions(&self) -> &'static [PermissionReq] {
        PERMISSIONS
    }

    async fn open(
        &self,
        topic: &str,
        _args: Value,
    ) -> Result<Subscription, AdapterError> {
        // Both declared topics share the same producer loop — the
        // poller emits the level payload AND the per-sensor health
        // report on every tick. Subscribing to either gives the caller
        // the full delta stream; the substrate routes by path.
        if topic != "substrate/sensor/mic"
            && topic != "substrate/meta/adapter/mic/healthcheck"
        {
            return Err(AdapterError::UnknownTopic(topic.into()));
        }
        let id = {
            let mut reg = self.reg.lock();
            reg.allocate()
        };
        let (cancel_tx, cancel_rx) = oneshot::channel();
        let (tx, rx) = mpsc::channel::<StateDelta>(CHAN);
        self.reg.lock().live.insert(id, cancel_tx);

        let source_path = self.source_path.clone();
        let sample_rate = self.sample_rate;
        tokio::spawn(async move {
            poll_level(source_path, sample_rate, tx, cancel_rx).await;
        });
        Ok(Subscription { id, rx })
    }

    async fn close(&self, sub_id: SubId) -> Result<(), AdapterError> {
        let _ = self.reg.lock().live.remove(&sub_id);
        Ok(())
    }
}

#[async_trait]
impl PhysicalSensorAdapter for MicrophoneAdapter {
    fn model(&self) -> &'static str {
        // Preview stub — real adapters will report the actual device
        // model from CPAL / ALSA / CoreAudio.
        "file-backed s16le PCM (preview stub)"
    }

    fn interface(&self) -> SensorInterface {
        // FileBacked for the preview; real hardware flips to
        // HostAudio { Capture } when the CPAL bridge lands.
        SensorInterface::FileBacked {
            path: self.source_path.clone(),
        }
    }

    fn unit(&self) -> &'static str {
        "dBFS"
    }

    fn range(&self) -> (f64, f64) {
        // dBFS: -∞ (silence) to 0 (full-scale). We clamp the silence
        // floor to -120 dB in the emitted value for numerical
        // stability; the trait-level range is still the idealised
        // one.
        (f64::NEG_INFINITY, 0.0)
    }

    fn calibration(&self) -> SensorCalibration {
        SensorCalibration {
            scale: 1.0,
            offset: 0.0,
            reference: Some(
                "s16le full-scale = ±32767; dBFS = 20 * log10(rms / 32768)".into(),
            ),
        }
    }

    fn characterization(&self) -> Characterization {
        // Honest: we emit a scalar level, not spectral bins.
        // Upgrading to Spectral means adding an FFT pass + emitting
        // per-bin magnitudes on a sibling topic.
        Characterization::Rate
    }
}

async fn poll_level(
    source_path: PathBuf,
    sample_rate: u32,
    tx: mpsc::Sender<StateDelta>,
    mut cancel_rx: oneshot::Receiver<()>,
) {
    let mut ticker = tokio::time::interval(Duration::from_millis(TICK_MS));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    // File-read cursor that advances each tick. The file is expected
    // to be a rolling buffer (test, synthetic stream, or mmapped
    // audio bridge) — we read the tail.
    let mut cursor: u64 = 0;
    // Healthcheck producer state — counts ticks + errors so we can
    // emit the per-sensor health shape per HEALTHCHECK-CONTRACT.md §3.
    // Configured rate is `1000 / TICK_MS` Hz.
    let mut tick_count: u64 = 0;
    let mut error_count: u64 = 0;
    let mut last_emit_ts: u64 = 0;
    let mut last_error: Option<String> = None;
    // Emit an initial `unknown`-status report so subscribers see the
    // healthcheck path populated immediately (contract §9 open question
    // 1: ship `unknown` for the pre-first-emit window).
    let configured_rate_hz = 1000.0 / TICK_MS as f64;
    let initial_report = SensorHealthReport::unknown(configured_rate_hz);
    if tx
        .send(build_health_delta("mic", &initial_report))
        .await
        .is_err()
    {
        return;
    }

    loop {
        tokio::select! {
            _ = &mut cancel_rx => return,
            _ = ticker.tick() => {
                tick_count += 1;
                let value = read_and_measure(&source_path, sample_rate, &mut cursor);
                let reason = value
                    .get("reason")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                // If the backing file is missing we emit nothing on the
                // primary topic. Otherwise we'd overwrite
                // externally-published values (e.g. from the ESP32 bridge
                // calling `substrate.publish`) on every tick with
                // `{available: false, reason: "source-missing"}`, which
                // has no `rms_db` key and breaks the gauge binding. The
                // healthcheck path *does* still update so subscribers
                // can see the sensor is unhealthy.
                let payload_published = if reason.as_deref() == Some("source-missing") {
                    error_count += 1;
                    last_error = Some("source-missing".into());
                    false
                } else {
                    let delta = StateDelta::Replace {
                        path: "substrate/sensor/mic".to_string(),
                        value,
                    };
                    if tx.send(delta).await.is_err() {
                        return;
                    }
                    last_emit_ts = now_ms();
                    true
                };

                // Build + send the per-sensor health report. Cadence
                // policy: every tick is fine at 2 Hz (contract §3.3 —
                // "every emit, or at least every N=4 emits"); this keeps
                // the producer state machine simple.
                let observed_rate_hz = if payload_published {
                    // Tracking observed-rate from inside the producer is
                    // necessarily approximate (a true rolling-window
                    // measurement belongs to the §7 aggregator). For the
                    // preview we report `configured` when the last tick
                    // succeeded and zero otherwise — this is what
                    // `derive_status` needs to produce a useful status.
                    configured_rate_hz
                } else {
                    0.0
                };
                let mut report = SensorHealthReport {
                    status: SensorStatus::Unknown,
                    last_emit_ts,
                    configured_rate_hz,
                    observed_rate_hz,
                    error_count,
                    tick: tick_count,
                    since_ms: None,
                    last_error: last_error.clone(),
                    notes: reason.map(|r| format!("read-result: {r}")),
                };
                report.status = SensorHealthReport::derive_status(
                    observed_rate_hz,
                    configured_rate_hz,
                    if payload_published { 0 } else { 1 },
                );
                if tx.send(build_health_delta("mic", &report)).await.is_err() {
                    return;
                }
            }
        }
    }
}

/// Wall-clock millis since UNIX epoch — small helper kept private so
/// the system-time read isn't sprinkled through the poller body.
fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Read up to `WINDOW_SAMPLES` from the file at `cursor`, compute
/// RMS + peak in dBFS, return the emission shape. Advances `cursor`
/// to the new end-of-read position.
fn read_and_measure(
    source_path: &Path,
    sample_rate: u32,
    cursor: &mut u64,
) -> Value {
    let Ok(mut file) = std::fs::File::open(source_path) else {
        return json!({
            "available": false,
            "reason": "source-missing",
            "characterization": Characterization::Rate.as_str(),
        });
    };
    if file.seek(SeekFrom::Start(*cursor)).is_err() {
        // File truncated / replaced — reset and return a 'stream
        // reset' marker.
        *cursor = 0;
        return json!({
            "available": false,
            "reason": "stream-reset",
            "characterization": Characterization::Rate.as_str(),
        });
    }
    let mut buf = vec![0u8; WINDOW_SAMPLES * 2];
    let n = file.read(&mut buf).unwrap_or(0);
    let samples_read = n / 2;
    *cursor += n as u64;
    if samples_read == 0 {
        return json!({
            "available": true,
            "rms_db": -120.0,
            "peak_db": -120.0,
            "sample_rate": sample_rate,
            "samples_in_window": 0,
            "characterization": Characterization::Rate.as_str(),
        });
    }

    let (rms_db, peak_db) = rms_and_peak_dbfs(&buf[..n]);
    json!({
        "available": true,
        "rms_db": rms_db,
        "peak_db": peak_db,
        "sample_rate": sample_rate,
        "samples_in_window": samples_read,
        "characterization": Characterization::Rate.as_str(),
    })
}

/// Compute RMS + peak level of a signed-16-bit little-endian PCM
/// byte buffer, in dBFS. Silence floor clamped to -120 dB.
fn rms_and_peak_dbfs(bytes: &[u8]) -> (f64, f64) {
    let sample_count = bytes.len() / 2;
    if sample_count == 0 {
        return (-120.0, -120.0);
    }
    let mut sum_sq: f64 = 0.0;
    let mut peak: i32 = 0;
    for chunk in bytes.chunks_exact(2) {
        let s = i16::from_le_bytes([chunk[0], chunk[1]]) as i32;
        sum_sq += (s as f64) * (s as f64);
        let abs = s.abs();
        if abs > peak {
            peak = abs;
        }
    }
    let rms = (sum_sq / sample_count as f64).sqrt();
    let rms_db = sample_to_dbfs(rms);
    let peak_db = sample_to_dbfs(peak as f64);
    (rms_db, peak_db)
}

/// Convert an absolute s16 linear value into dBFS, clamped at -120.
fn sample_to_dbfs(v: f64) -> f64 {
    if v <= 0.5 {
        return -120.0;
    }
    let db = 20.0 * (v / 32768.0).log10();
    if db < -120.0 {
        -120.0
    } else {
        db
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_pcm(dir: &TempDir, name: &str, samples: &[i16]) -> PathBuf {
        let path = dir.path().join(name);
        let mut bytes = Vec::with_capacity(samples.len() * 2);
        for s in samples {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn silence_reports_floor_level() {
        let dir = TempDir::new().unwrap();
        let path = write_pcm(&dir, "silence.raw", &[0i16; 1024]);
        let mut cursor = 0u64;
        let v = read_and_measure(&path, 16000, &mut cursor);
        assert_eq!(v["available"], true);
        assert_eq!(v["rms_db"], -120.0);
        assert_eq!(v["peak_db"], -120.0);
    }

    #[test]
    fn full_scale_sine_peaks_near_zero_dbfs() {
        let dir = TempDir::new().unwrap();
        // A full-scale square wave — peak = 32767, RMS = 32767.
        let samples: Vec<i16> = (0..1024).map(|i| if i % 2 == 0 { 32767 } else { -32767 }).collect();
        let path = write_pcm(&dir, "fullscale.raw", &samples);
        let mut cursor = 0u64;
        let v = read_and_measure(&path, 16000, &mut cursor);
        let peak = v["peak_db"].as_f64().unwrap();
        let rms = v["rms_db"].as_f64().unwrap();
        // Square at full scale: peak ~ 0 dBFS, RMS ~ 0 dBFS.
        assert!(peak > -0.1, "peak_db = {peak}");
        assert!(rms > -0.1, "rms_db = {rms}");
    }

    #[test]
    fn half_scale_sine_rms_is_minus_nine_dbfs_ish() {
        // A 50%-amplitude square wave. RMS = 16384; 20*log10(16384/32768) = -6.02 dBFS.
        let dir = TempDir::new().unwrap();
        let samples: Vec<i16> = (0..1024).map(|i| if i % 2 == 0 { 16384 } else { -16384 }).collect();
        let path = write_pcm(&dir, "halfscale.raw", &samples);
        let mut cursor = 0u64;
        let v = read_and_measure(&path, 16000, &mut cursor);
        let rms = v["rms_db"].as_f64().unwrap();
        assert!((rms + 6.02).abs() < 0.1, "expected ~-6.02 dBFS, got {rms}");
    }

    #[test]
    fn missing_source_emits_unavailable() {
        let v = read_and_measure(Path::new("/nonexistent/weftos/mic.raw"), 16000, &mut 0u64);
        assert_eq!(v["available"], false);
        assert_eq!(v["reason"], "source-missing");
        assert_eq!(v["characterization"], Characterization::Rate.as_str());
    }

    #[test]
    fn cursor_advances_between_reads() {
        let dir = TempDir::new().unwrap();
        // Two full windows back-to-back: first silent, second
        // full-scale. Using exactly WINDOW_SAMPLES per window so each
        // read consumes one window's worth and the next read picks up
        // from the right offset.
        let mut samples: Vec<i16> = vec![0i16; WINDOW_SAMPLES];
        samples.extend(
            (0..WINDOW_SAMPLES).map(|i| if i % 2 == 0 { 32767 } else { -32767 }),
        );
        let path = write_pcm(&dir, "twohalf.raw", &samples);

        let mut cursor = 0u64;
        let v1 = read_and_measure(&path, 16000, &mut cursor);
        let v2 = read_and_measure(&path, 16000, &mut cursor);
        // First window is silent (-120).
        assert_eq!(v1["rms_db"], -120.0);
        // Second window jumps to ~0 dBFS.
        let second_rms = v2["rms_db"].as_f64().unwrap();
        assert!(second_rms > -0.1, "second rms_db = {second_rms}");
    }

    #[tokio::test]
    async fn adapter_open_unknown_topic_errors() {
        let a = MicrophoneAdapter::new();
        let r = a.open("substrate/sensor/bogus", Value::Null).await;
        assert!(matches!(r, Err(AdapterError::UnknownTopic(_))));
    }

    #[test]
    fn physical_trait_declares_rate_characterization() {
        let a = MicrophoneAdapter::new();
        // The point of this test: the sensor honestly declares it
        // can't discriminate signal content — only magnitude.
        assert_eq!(a.characterization(), Characterization::Rate);
        assert_eq!(a.unit(), "dBFS");
    }

    #[test]
    fn file_backed_interface_roundtrips_path() {
        let a = MicrophoneAdapter::with_source(PathBuf::from("/tmp/weftos/demo.raw"), 48000);
        match a.interface() {
            SensorInterface::FileBacked { path } => {
                assert_eq!(path, PathBuf::from("/tmp/weftos/demo.raw"));
            }
            other => panic!("expected FileBacked, got {other:?}"),
        }
    }

    #[test]
    fn host_audio_direction_variant_compiles() {
        // Sanity check that `AudioDirection::Capture` is exposed for
        // the future CPAL bridge — no logic, just that the variant
        // round-trips.
        let d = AudioDirection::Capture;
        assert_eq!(d, AudioDirection::Capture);
    }

    #[test]
    fn declares_both_payload_and_healthcheck_topics() {
        // WEFT-432: the mic adapter declares the per-sensor
        // healthcheck topic alongside its level-meter payload.
        let paths: Vec<&str> = TOPICS.iter().map(|t| t.path).collect();
        assert!(paths.contains(&"substrate/sensor/mic"));
        assert!(paths.contains(&"substrate/meta/adapter/mic/healthcheck"));
    }

    #[tokio::test]
    async fn open_succeeds_for_healthcheck_topic() {
        // WEFT-432: subscribers can open the healthcheck topic directly.
        let a = MicrophoneAdapter::new();
        let r = a
            .open("substrate/meta/adapter/mic/healthcheck", Value::Null)
            .await;
        assert!(r.is_ok(), "open(healthcheck) should succeed: {r:?}");
    }

    #[tokio::test]
    async fn poller_emits_initial_unknown_health_report() {
        // WEFT-432: the contract calls for an `unknown` pre-first-emit
        // report so subscribers see the path populated immediately.
        // Verify it's the very first delta the poller produces.
        let dir = TempDir::new().unwrap();
        let path = write_pcm(&dir, "silence.raw", &[0i16; 1024]);
        let a = MicrophoneAdapter::with_source(path, 16000);
        let mut sub = a
            .open("substrate/sensor/mic", Value::Null)
            .await
            .expect("open");

        // The very first delta must be the unknown-status health
        // report — sent before the ticker has fired.
        let first = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            sub.rx.recv(),
        )
        .await
        .expect("recv timed out")
        .expect("channel closed");
        match first {
            StateDelta::Replace { path, value } => {
                assert_eq!(path, "substrate/meta/adapter/mic/healthcheck");
                assert_eq!(value["status"], "unknown");
                assert_eq!(value["configured_rate_hz"], 1000.0 / TICK_MS as f64);
                assert_eq!(value["error_count"], 0);
            }
            other => panic!("expected Replace, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn poller_emits_degraded_health_report_when_source_missing() {
        // WEFT-432: when the backing audio source is missing, the
        // poller must surface that on the healthcheck path (status:
        // degraded — error counter increments while observed rate
        // drops to zero). This is honest reporting per the contract:
        // the sensor *can* tell you it tried to read and failed, even
        // before the §4.2 "down for 10s" threshold trips.
        // The legacy `substrate/sensor/mic` payload is NOT overwritten
        // (covered by `missing_source_emits_unavailable`).
        let nonexistent = PathBuf::from("/nonexistent/weftos/mic-test.raw");
        let a = MicrophoneAdapter::with_source(nonexistent, 16000);
        let mut sub = a
            .open("substrate/meta/adapter/mic/healthcheck", Value::Null)
            .await
            .expect("open");

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut got_unhealthy = false;
        while std::time::Instant::now() < deadline {
            let delta = tokio::time::timeout(
                std::time::Duration::from_millis(700),
                sub.rx.recv(),
            )
            .await;
            let Ok(Some(delta)) = delta else { continue };
            if let StateDelta::Replace { path, value } = delta
                && path == "substrate/meta/adapter/mic/healthcheck"
                && (value["status"] == "degraded" || value["status"] == "down")
                && value["error_count"].as_u64().unwrap_or(0) > 0
            {
                got_unhealthy = true;
                break;
            }
        }
        assert!(
            got_unhealthy,
            "expected at least one degraded/down-status health report with error_count > 0"
        );
    }
}
