//! Voice channel adapter -- substrate STT/TTS thin client.
//!
//! `start()` opens an audio source (default: cpal default input under
//! `voice-real-audio`; trait-injected fake otherwise), feeds frames
//! through an [`EnergyVad`], and on each speech-end POSTs the captured
//! 16 kHz mono `s16le` PCM (wrapped in a WAV header) to
//! `whisper_endpoint + transcribe_path` as `multipart/form-data` with a
//! `file` part. The returned JSON `{"text": "..."}` is published to the
//! agent pipeline via `ChannelAdapterHost::deliver_inbound`.
//!
//! `send()` POSTs `{"text": ...}` JSON to `tts_endpoint +
//! synthesize_path`, expects an `audio/wav` body in response, and plays
//! it through the configured [`PlaybackSink`] (default: cpal output
//! under `voice-real-audio`; trait-injected fake otherwise). Returns a
//! synthetic message id `voice-{ts_ms}`.
//!
//! `stop()` is implemented via the `CancellationToken` plumbed through
//! `start()` -- callers own the token and cancel it, the channel drops
//! its streams in response.
//!
//! # Why `AudioSource` / `PlaybackSink` traits
//!
//! cpal cannot run on CI Linux runners without alsa dev headers, and the
//! workspace already bans hard-deps on cpal in default builds. The two
//! traits let us:
//! - default-test under `--features voice` against in-memory PCM
//!   sources and a `Vec<i16>` sink (no native audio dep at all);
//! - opt-in to real cpal under `--features voice-real-audio`;
//! - keep cpal-touching tests behind `--features real-audio-test`
//!   (gated to `cfg(target_os = "linux")` because the CI macOS runner
//!   doesn't have a default input device either).

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use reqwest::multipart::{Form, Part};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use clawft_plugin::error::PluginError;
use clawft_plugin::message::MessagePayload;
use clawft_plugin::traits::{ChannelAdapter, ChannelAdapterHost};

use super::types::{VoiceAdapterConfig, VoiceError};
use super::vad::{EnergyVad, VadEvent};
use super::wav::{pcm_s16le_to_wav, wav_to_pcm_s16le};

/// One captured utterance ready to be transcribed.
#[derive(Debug, Clone)]
pub struct AudioSegment {
    /// Interleaved 16 kHz mono `s16le` PCM samples.
    pub samples: Vec<i16>,
    /// Sample rate (echoed from the source so the encoder doesn't have
    /// to assume).
    pub sample_rate: u32,
}

/// Audio capture trait. Default impl under `voice-real-audio` uses cpal;
/// tests inject a [`MemoryAudioSource`] (or any other implementor).
#[async_trait]
pub trait AudioSource: Send + Sync {
    /// Spawn a capture task. Must push `i16` PCM frames (any frame size,
    /// at the configured sample rate) into the supplied channel until
    /// `cancel` fires or the underlying device closes.
    ///
    /// Implementations should aim for ~20-100 ms frames.
    async fn run(
        &self,
        sample_rate: u32,
        device_name: Option<&str>,
        tx: mpsc::Sender<Vec<i16>>,
        cancel: CancellationToken,
    ) -> Result<(), VoiceError>;
}

/// Audio playback trait. Default impl under `voice-real-audio` uses
/// cpal; tests inject a [`MemoryPlaybackSink`].
#[async_trait]
pub trait PlaybackSink: Send + Sync {
    /// Render a single PCM utterance to the output device. Blocks (in
    /// the async sense) until playback completes.
    async fn play(&self, samples: &[i16], sample_rate: u32) -> Result<(), VoiceError>;
}

/// In-memory audio source used by the default test path. Replays a
/// fixed PCM script as fixed-size frames. Suitable for wiremock tests
/// without any native audio dependency.
pub struct MemoryAudioSource {
    samples: Vec<i16>,
    frame_size: usize,
}

impl MemoryAudioSource {
    /// Build a source that will replay `samples` in 100 ms frames at
    /// the source's sample rate.
    pub fn new(samples: Vec<i16>, frame_size: usize) -> Self {
        Self {
            samples,
            frame_size: frame_size.max(1),
        }
    }
}

#[async_trait]
impl AudioSource for MemoryAudioSource {
    async fn run(
        &self,
        _sample_rate: u32,
        _device_name: Option<&str>,
        tx: mpsc::Sender<Vec<i16>>,
        cancel: CancellationToken,
    ) -> Result<(), VoiceError> {
        for chunk in self.samples.chunks(self.frame_size) {
            if cancel.is_cancelled() {
                break;
            }
            if tx.send(chunk.to_vec()).await.is_err() {
                break;
            }
        }
        // Hold open until cancelled so end-of-script doesn't slam the
        // VAD with EOF before silence-tail can fire.
        cancel.cancelled().await;
        Ok(())
    }
}

/// In-memory playback sink. Records every utterance for assertions.
#[derive(Default)]
pub struct MemoryPlaybackSink {
    played: tokio::sync::Mutex<Vec<Vec<i16>>>,
}

impl MemoryPlaybackSink {
    /// Empty sink.
    pub fn new() -> Self {
        Self::default()
    }
    /// Snapshot of the utterances played so far (cloned).
    pub async fn played(&self) -> Vec<Vec<i16>> {
        self.played.lock().await.clone()
    }
}

#[async_trait]
impl PlaybackSink for MemoryPlaybackSink {
    async fn play(&self, samples: &[i16], _sample_rate: u32) -> Result<(), VoiceError> {
        self.played.lock().await.push(samples.to_vec());
        Ok(())
    }
}

// -------------------------------------------------------------------
// Real cpal-backed source / sink (only compiled with voice-real-audio).
// -------------------------------------------------------------------

#[cfg(feature = "voice-real-audio")]
mod cpal_io {
    use super::{AudioSource, PlaybackSink, VoiceError};
    use async_trait::async_trait;
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use std::sync::{Arc, Mutex};
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    /// cpal-backed input. Resamples via the sample-rate-config selection
    /// (cpal will pick the closest supported config to `sample_rate`);
    /// we stamp the output with the cpal-reported rate so the VAD can
    /// rely on truthful timing.
    pub struct CpalAudioSource;

    #[async_trait]
    impl AudioSource for CpalAudioSource {
        async fn run(
            &self,
            sample_rate: u32,
            device_name: Option<&str>,
            tx: mpsc::Sender<Vec<i16>>,
            cancel: CancellationToken,
        ) -> Result<(), VoiceError> {
            // cpal's stream callbacks aren't Send / aren't async; build
            // the stream on a blocking thread and keep it alive there.
            let device_name = device_name.map(|s| s.to_string());
            tokio::task::spawn_blocking(move || -> Result<(), VoiceError> {
                let host = cpal::default_host();
                let device = match device_name {
                    Some(name) => host
                        .input_devices()
                        .map_err(|e| VoiceError::Audio(e.to_string()))?
                        .find(|d| d.name().ok().as_deref() == Some(name.as_str()))
                        .ok_or_else(|| {
                            VoiceError::Audio(format!("no cpal input device named {name:?}"))
                        })?,
                    None => host
                        .default_input_device()
                        .ok_or_else(|| {
                            VoiceError::Audio("no default cpal input device".into())
                        })?,
                };
                let config = device
                    .default_input_config()
                    .map_err(|e| VoiceError::Audio(e.to_string()))?;
                let stream_config: cpal::StreamConfig = config.clone().into();
                let actual_rate = stream_config.sample_rate.0;
                if actual_rate != sample_rate {
                    tracing::warn!(
                        wanted = sample_rate,
                        got = actual_rate,
                        "cpal device delivered a different sample rate; \
                         downstream VAD will use device rate"
                    );
                }
                let tx_inner = tx.clone();
                let cancel_inner = cancel.clone();
                let err_slot: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
                let err_clone = err_slot.clone();

                let stream = match config.sample_format() {
                    cpal::SampleFormat::I16 => device.build_input_stream(
                        &stream_config,
                        move |data: &[i16], _| {
                            if cancel_inner.is_cancelled() {
                                return;
                            }
                            let _ = tx_inner.try_send(data.to_vec());
                        },
                        move |e| {
                            *err_clone.lock().unwrap() = Some(e.to_string());
                        },
                        None,
                    ),
                    cpal::SampleFormat::F32 => device.build_input_stream(
                        &stream_config,
                        move |data: &[f32], _| {
                            if cancel_inner.is_cancelled() {
                                return;
                            }
                            let pcm: Vec<i16> = data
                                .iter()
                                .map(|&s| (s.clamp(-1.0, 1.0) * 32_767.0) as i16)
                                .collect();
                            let _ = tx_inner.try_send(pcm);
                        },
                        move |e| {
                            *err_clone.lock().unwrap() = Some(e.to_string());
                        },
                        None,
                    ),
                    other => {
                        return Err(VoiceError::Audio(format!(
                            "unsupported cpal sample format {other:?}"
                        )));
                    }
                }
                .map_err(|e| VoiceError::Audio(e.to_string()))?;
                stream.play().map_err(|e| VoiceError::Audio(e.to_string()))?;

                while !cancel.is_cancelled() {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    if let Some(e) = err_slot.lock().unwrap().take() {
                        return Err(VoiceError::Audio(e));
                    }
                }
                drop(stream);
                Ok(())
            })
            .await
            .map_err(|e| VoiceError::Audio(format!("cpal capture join: {e}")))?
        }
    }

    /// cpal-backed output sink.
    pub struct CpalPlaybackSink;

    #[async_trait]
    impl PlaybackSink for CpalPlaybackSink {
        async fn play(
            &self,
            samples: &[i16],
            sample_rate: u32,
        ) -> Result<(), VoiceError> {
            let pcm = samples.to_vec();
            tokio::task::spawn_blocking(move || -> Result<(), VoiceError> {
                let host = cpal::default_host();
                let device = host
                    .default_output_device()
                    .ok_or_else(|| VoiceError::Audio("no default cpal output device".into()))?;
                let stream_config = cpal::StreamConfig {
                    channels: 1,
                    sample_rate: cpal::SampleRate(sample_rate),
                    buffer_size: cpal::BufferSize::Default,
                };
                let pcm = Arc::new(pcm);
                let pcm_inner = pcm.clone();
                let cursor = Arc::new(Mutex::new(0usize));
                let cursor_inner = cursor.clone();
                let stream = device
                    .build_output_stream(
                        &stream_config,
                        move |out: &mut [i16], _| {
                            let mut c = cursor_inner.lock().unwrap();
                            for slot in out.iter_mut() {
                                if *c < pcm_inner.len() {
                                    *slot = pcm_inner[*c];
                                    *c += 1;
                                } else {
                                    *slot = 0;
                                }
                            }
                        },
                        |e| {
                            tracing::warn!(error = %e, "cpal output stream error");
                        },
                        None,
                    )
                    .map_err(|e| VoiceError::Audio(e.to_string()))?;
                stream.play().map_err(|e| VoiceError::Audio(e.to_string()))?;
                let total_samples = pcm.len();
                let dur_ms =
                    (total_samples as u64 * 1_000) / u64::from(sample_rate.max(1));
                std::thread::sleep(std::time::Duration::from_millis(dur_ms + 50));
                drop(stream);
                Ok(())
            })
            .await
            .map_err(|e| VoiceError::Audio(format!("cpal playback join: {e}")))?
        }
    }
}

#[cfg(feature = "voice-real-audio")]
pub use cpal_io::{CpalAudioSource, CpalPlaybackSink};

// ---------------------------------------------------------------------
// Voice channel adapter
// ---------------------------------------------------------------------

/// Voice channel adapter (substrate STT/TTS thin client).
pub struct VoiceChannelAdapter {
    config: VoiceAdapterConfig,
    http: reqwest::Client,
    source: Arc<dyn AudioSource>,
    sink: Arc<dyn PlaybackSink>,
}

impl VoiceChannelAdapter {
    /// Build an adapter with the supplied audio source + sink.
    ///
    /// Used by the trait-injected test path; production callers should
    /// prefer [`Self::new_real`] (under `voice-real-audio`) or the
    /// factory.
    pub fn new(
        config: VoiceAdapterConfig,
        source: Arc<dyn AudioSource>,
        sink: Arc<dyn PlaybackSink>,
    ) -> Result<Self, VoiceError> {
        config.validate().map_err(VoiceError::Config)?;
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(config.request_timeout_s))
            .build()
            .map_err(|e| VoiceError::Transport(e.to_string()))?;
        Ok(Self {
            config,
            http,
            source,
            sink,
        })
    }

    /// Build an adapter with cpal-backed source + sink.
    ///
    /// Available under `voice-real-audio` only.
    #[cfg(feature = "voice-real-audio")]
    pub fn new_real(config: VoiceAdapterConfig) -> Result<Self, VoiceError> {
        Self::new(
            config,
            Arc::new(CpalAudioSource),
            Arc::new(CpalPlaybackSink),
        )
    }

    /// Borrow the resolved config.
    pub fn config(&self) -> &VoiceAdapterConfig {
        &self.config
    }

    /// Allow-list check used by the inbound delivery path.
    pub fn is_sender_allowed(&self, sender: &str) -> bool {
        if self.config.allowed_senders.is_empty() {
            return true;
        }
        self.config.allowed_senders.iter().any(|s| s == sender)
    }

    /// POST a captured segment to the substrate Whisper endpoint.
    /// Returns the transcribed text. Visible for tests.
    pub async fn transcribe_segment(
        &self,
        seg: &AudioSegment,
    ) -> Result<String, VoiceError> {
        let wav = pcm_s16le_to_wav(&seg.samples, seg.sample_rate);
        let part = Part::bytes(wav)
            .file_name("audio.wav")
            .mime_str("audio/wav")
            .map_err(|e| VoiceError::Transport(e.to_string()))?;
        let mut form = Form::new().part("file", part);
        if !self.config.language.is_empty() {
            form = form.text("language", self.config.language.clone());
        }
        let resp = self
            .http
            .post(self.config.transcribe_url())
            .multipart(form)
            .send()
            .await
            .map_err(|e| VoiceError::Transport(e.to_string()))?;
        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| VoiceError::Transport(e.to_string()))?;
        if !status.is_success() {
            return Err(VoiceError::Server {
                status: status.as_u16(),
                body: truncate(&body, 4096),
            });
        }
        let v: serde_json::Value = serde_json::from_str(&body)
            .map_err(|e| VoiceError::Malformed(format!("{e}: {}", truncate(&body, 256))))?;
        let text = v
            .get("text")
            .and_then(|t| t.as_str())
            .ok_or_else(|| VoiceError::Malformed("response missing `text`".into()))?
            .trim()
            .to_string();
        Ok(text)
    }

    /// POST text to the substrate TTS endpoint, return the decoded
    /// `(samples, sample_rate)` pair. Accepts either an `audio/wav`
    /// body (decoded locally) or a JSON body of shape
    /// `{"audio_b64": "...", "sample_rate": 16000}`.
    pub async fn synthesize_text(
        &self,
        text: &str,
    ) -> Result<(Vec<i16>, u32), VoiceError> {
        let body = serde_json::json!({ "text": text });
        let resp = self
            .http
            .post(self.config.synthesize_url())
            .json(&body)
            .send()
            .await
            .map_err(|e| VoiceError::Transport(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp
                .text()
                .await
                .unwrap_or_else(|_| "<unreadable>".into());
            return Err(VoiceError::Server {
                status: status.as_u16(),
                body: truncate(&body, 4096),
            });
        }
        let ct = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_lowercase();
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| VoiceError::Transport(e.to_string()))?;
        if ct.contains("audio/") || (bytes.len() >= 12 && &bytes[0..4] == b"RIFF") {
            let (pcm, sr) = wav_to_pcm_s16le(&bytes)
                .map_err(|e| VoiceError::Malformed(format!("tts wav: {e}")))?;
            return Ok((pcm, sr));
        }
        // Fallback: JSON `{audio_b64, sample_rate}` shape (no extra crate;
        // we use a tiny inline base64 decoder to avoid pulling in another
        // workspace dep just for the synth path).
        let v: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| {
            VoiceError::Malformed(format!("tts response: {e}"))
        })?;
        let b64 = v
            .get("audio_b64")
            .and_then(|s| s.as_str())
            .ok_or_else(|| VoiceError::Malformed("tts json missing audio_b64".into()))?;
        let sr = v
            .get("sample_rate")
            .and_then(|s| s.as_u64())
            .unwrap_or(u64::from(self.config.sample_rate)) as u32;
        let raw = decode_base64(b64)
            .map_err(|e| VoiceError::Malformed(format!("tts audio_b64: {e}")))?;
        let mut pcm = Vec::with_capacity(raw.len() / 2);
        for chunk in raw.chunks_exact(2) {
            pcm.push(i16::from_le_bytes([chunk[0], chunk[1]]));
        }
        Ok((pcm, sr))
    }
}

/// Truncate a string to at most `n` bytes for safe logging.
fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_string()
    } else {
        format!("{}…", &s[..n])
    }
}

/// Hand-rolled standard-base64 decoder. Tiny and dependency-free; the
/// TTS-JSON branch is intentionally a fallback so we don't pull
/// `base64` into the crate just for it.
fn decode_base64(s: &str) -> Result<Vec<u8>, String> {
    let cleaned: String = s.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    let s = cleaned.trim_end_matches('=');
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut buf: u32 = 0;
    let mut bits: u32 = 0;
    for ch in s.chars() {
        let v: u32 = match ch {
            'A'..='Z' => (ch as u32) - ('A' as u32),
            'a'..='z' => (ch as u32) - ('a' as u32) + 26,
            '0'..='9' => (ch as u32) - ('0' as u32) + 52,
            '+' => 62,
            '/' => 63,
            _ => return Err(format!("invalid base64 char {ch:?}")),
        };
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xFF) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Ok(out)
}

#[async_trait]
impl ChannelAdapter for VoiceChannelAdapter {
    fn name(&self) -> &str {
        "voice"
    }

    fn display_name(&self) -> &str {
        "Voice (substrate STT/TTS)"
    }

    fn supports_threads(&self) -> bool {
        false
    }

    fn supports_media(&self) -> bool {
        true
    }

    async fn start(
        &self,
        host: Arc<dyn ChannelAdapterHost>,
        cancel: clawft_plugin::traits::CancellationToken,
    ) -> Result<(), PluginError> {
        info!(
            whisper = %self.config.transcribe_url(),
            tts = %self.config.synthesize_url(),
            sample_rate = self.config.sample_rate,
            "voice channel starting"
        );

        // Adapt the plugin-shim CancellationToken to a tokio_util one
        // for the cpal blocking thread / mpsc loop.
        let internal = CancellationToken::new();
        let bridge = internal.clone();
        let plug_cancel = cancel.clone();
        // Spawn a watchdog that ticks the bridge when the plugin token
        // fires. The plugin shim only exposes `is_cancelled()` -- no
        // `cancelled().await` -- so we poll. 50 ms is fast enough for
        // shutdown and quiet enough not to register on a flame graph.
        let watchdog = {
            let bridge = bridge.clone();
            tokio::spawn(async move {
                while !plug_cancel.is_cancelled() {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
                bridge.cancel();
            })
        };

        let (tx, mut rx) = mpsc::channel::<Vec<i16>>(64);
        let source = self.source.clone();
        let device_name = self.config.device_name.clone();
        let sample_rate = self.config.sample_rate;
        let cap_cancel = internal.clone();
        let cap_handle = tokio::spawn(async move {
            if let Err(e) = source
                .run(sample_rate, device_name.as_deref(), tx, cap_cancel)
                .await
            {
                warn!(error = %e, "voice capture ended with error");
            }
        });

        let mut vad = EnergyVad::new(
            self.config.sample_rate,
            self.config.vad_threshold_dbfs,
            self.config.silence_ms,
            self.config.min_utterance_ms,
            self.config.max_utterance_ms,
        );
        // We accumulate raw PCM and slice on SpeechEnd. Bound the
        // buffer by max_utterance_ms × sample_rate so a buggy VAD
        // can't OOM us.
        let mut buf: Vec<i16> = Vec::new();
        let max_buf = (u64::from(self.config.sample_rate)
            * u64::from(self.config.max_utterance_ms)
            / 1_000) as usize
            + self.config.sample_rate as usize; // +1 s slack
        let mut buffering = false;
        let mut consumed: u64 = 0;

        loop {
            tokio::select! {
                _ = internal.cancelled() => {
                    info!("voice channel shutting down");
                    break;
                }
                frame = rx.recv() => {
                    let Some(frame) = frame else {
                        debug!("voice capture channel closed");
                        break;
                    };
                    let events = vad.feed(&frame);
                    let frame_len = frame.len() as u64;
                    if buffering {
                        buf.extend_from_slice(&frame);
                        if buf.len() > max_buf {
                            // VAD failed to flush; drop oldest to cap memory.
                            let drop_n = buf.len() - max_buf;
                            buf.drain(..drop_n);
                        }
                    }
                    consumed += frame_len;
                    for ev in events {
                        match ev {
                            VadEvent::SpeechStart { at_sample } => {
                                buffering = true;
                                buf.clear();
                                // Include the current frame from speech-start.
                                let into_frame = at_sample.saturating_sub(consumed - frame_len);
                                let into_frame = (into_frame as usize).min(frame.len());
                                buf.extend_from_slice(&frame[into_frame..]);
                            }
                            VadEvent::SpeechEnd { start_sample, at_sample } => {
                                let segment = AudioSegment {
                                    samples: std::mem::take(&mut buf),
                                    sample_rate: self.config.sample_rate,
                                };
                                buffering = false;
                                let span_ms = (at_sample.saturating_sub(start_sample) * 1_000)
                                    / u64::from(self.config.sample_rate.max(1));
                                debug!(
                                    samples = segment.samples.len(),
                                    span_ms,
                                    "voice utterance captured; calling whisper"
                                );
                                match self.transcribe_segment(&segment).await {
                                    Ok(text) if !text.is_empty() => {
                                        if !self.is_sender_allowed(&self.config.sender_id) {
                                            warn!(
                                                sender = %self.config.sender_id,
                                                "voice transcript dropped: sender not in allow-list"
                                            );
                                            continue;
                                        }
                                        let mut metadata: HashMap<String, serde_json::Value> = HashMap::new();
                                        metadata.insert(
                                            "voice_span_ms".into(),
                                            serde_json::json!(span_ms),
                                        );
                                        metadata.insert(
                                            "voice_sample_rate".into(),
                                            serde_json::json!(self.config.sample_rate),
                                        );
                                        let payload = MessagePayload::text(text.clone());
                                        if let Err(e) = host
                                            .deliver_inbound(
                                                "voice",
                                                &self.config.sender_id,
                                                &self.config.chat_id,
                                                payload,
                                                metadata,
                                            )
                                            .await
                                        {
                                            warn!(error = %e, "voice deliver_inbound failed");
                                        } else {
                                            debug!(text = %text, "voice transcript delivered");
                                        }
                                    }
                                    Ok(_) => debug!("voice transcript empty; skipping"),
                                    Err(e) => warn!(error = %e, "voice transcription failed"),
                                }
                            }
                        }
                    }
                }
            }
        }

        // Best-effort: cancel the cpal task and wait for it to drain.
        internal.cancel();
        let _ = cap_handle.await;
        watchdog.abort();
        Ok(())
    }

    async fn send(
        &self,
        target: &str,
        payload: &MessagePayload,
    ) -> Result<String, PluginError> {
        let text = match payload.as_text() {
            Some(t) => t,
            None => {
                warn!("voice channel: non-text payload ignored");
                return Ok("voice-skipped".into());
            }
        };
        if text.trim().is_empty() {
            return Ok("voice-empty".into());
        }
        let (pcm, sr) = self
            .synthesize_text(text)
            .await
            .map_err(voice_to_plugin_err)?;
        if let Err(e) = self.sink.play(&pcm, sr).await {
            warn!(error = %e, "voice playback failed");
        }
        let id = format!(
            "voice-{}-{}",
            target.replace('/', "-"),
            chrono::Utc::now().timestamp_millis()
        );
        Ok(id)
    }
}

fn voice_to_plugin_err(e: VoiceError) -> PluginError {
    match e {
        VoiceError::Config(s) => PluginError::LoadFailed(s),
        VoiceError::Audio(s) => PluginError::ExecutionFailed(format!("voice audio: {s}")),
        VoiceError::Transport(s) => {
            PluginError::ExecutionFailed(format!("voice transport: {s}"))
        }
        VoiceError::Server { status, body } => {
            PluginError::ExecutionFailed(format!("voice server {status}: {body}"))
        }
        VoiceError::Malformed(s) => PluginError::ExecutionFailed(format!("voice malformed: {s}")),
    }
}

/// Factory wiring [`VoiceChannelAdapter`] under JSON config.
pub struct VoiceChannelAdapterFactory;

impl VoiceChannelAdapterFactory {
    /// Build with trait-injected source + sink. Used by tests + by
    /// callers that don't want cpal pulled in.
    pub fn build_with(
        config: &serde_json::Value,
        source: Arc<dyn AudioSource>,
        sink: Arc<dyn PlaybackSink>,
    ) -> Result<Arc<dyn ChannelAdapter>, PluginError> {
        let cfg: VoiceAdapterConfig = serde_json::from_value(config.clone())
            .map_err(|e| PluginError::LoadFailed(format!("invalid voice config: {e}")))?;
        let adapter = VoiceChannelAdapter::new(cfg, source, sink).map_err(voice_to_plugin_err)?;
        Ok(Arc::new(adapter))
    }

    /// Build with cpal-backed source + sink. Compiled only with
    /// `voice-real-audio`.
    #[cfg(feature = "voice-real-audio")]
    pub fn build(
        config: &serde_json::Value,
    ) -> Result<Arc<dyn ChannelAdapter>, PluginError> {
        Self::build_with(
            config,
            Arc::new(CpalAudioSource),
            Arc::new(CpalPlaybackSink),
        )
    }
}

// ---------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Single recorded `deliver_inbound` invocation.
    type Delivery = (
        String,
        String,
        String,
        MessagePayload,
        HashMap<String, serde_json::Value>,
    );

    /// Test host that records `deliver_inbound` calls.
    #[derive(Default)]
    struct RecordingHost {
        delivered: tokio::sync::Mutex<Vec<Delivery>>,
    }

    #[async_trait]
    impl ChannelAdapterHost for RecordingHost {
        async fn deliver_inbound(
            &self,
            channel: &str,
            sender_id: &str,
            chat_id: &str,
            payload: MessagePayload,
            metadata: HashMap<String, serde_json::Value>,
        ) -> Result<(), PluginError> {
            self.delivered.lock().await.push((
                channel.to_string(),
                sender_id.to_string(),
                chat_id.to_string(),
                payload,
                metadata,
            ));
            Ok(())
        }
    }

    fn loud_frame(n: usize) -> Vec<i16> {
        (0..n).map(|i| if i % 2 == 0 { 8_000 } else { -8_000 }).collect()
    }

    fn silent_frame(n: usize) -> Vec<i16> {
        vec![0i16; n]
    }

    fn build_script(sample_rate: u32) -> Vec<i16> {
        // 500 ms speech + 1 s silence (well past the 700 ms tail).
        let speech = loud_frame((sample_rate as usize) / 2);
        let silence = silent_frame(sample_rate as usize);
        let mut s = Vec::with_capacity(speech.len() + silence.len());
        s.extend_from_slice(&speech);
        s.extend_from_slice(&silence);
        s
    }

    #[test]
    fn adapter_metadata() {
        let cfg = VoiceAdapterConfig::default();
        let adapter = VoiceChannelAdapter::new(
            cfg,
            Arc::new(MemoryAudioSource::new(vec![], 1_600)),
            Arc::new(MemoryPlaybackSink::new()),
        )
        .unwrap();
        assert_eq!(adapter.name(), "voice");
        assert_eq!(adapter.display_name(), "Voice (substrate STT/TTS)");
        assert!(!adapter.supports_threads());
        assert!(adapter.supports_media());
    }

    #[test]
    fn rejects_invalid_config() {
        let cfg = VoiceAdapterConfig {
            whisper_endpoint: String::new(),
            ..Default::default()
        };
        let r = VoiceChannelAdapter::new(
            cfg,
            Arc::new(MemoryAudioSource::new(vec![], 1)),
            Arc::new(MemoryPlaybackSink::new()),
        );
        assert!(matches!(r, Err(VoiceError::Config(_))));
    }

    #[test]
    fn allow_list_empty_allows_all() {
        let adapter = VoiceChannelAdapter::new(
            VoiceAdapterConfig::default(),
            Arc::new(MemoryAudioSource::new(vec![], 1)),
            Arc::new(MemoryPlaybackSink::new()),
        )
        .unwrap();
        assert!(adapter.is_sender_allowed("alice"));
    }

    #[test]
    fn allow_list_filters() {
        let cfg = VoiceAdapterConfig {
            allowed_senders: vec!["alice".into()],
            ..Default::default()
        };
        let adapter = VoiceChannelAdapter::new(
            cfg,
            Arc::new(MemoryAudioSource::new(vec![], 1)),
            Arc::new(MemoryPlaybackSink::new()),
        )
        .unwrap();
        assert!(adapter.is_sender_allowed("alice"));
        assert!(!adapter.is_sender_allowed("eve"));
    }

    #[tokio::test]
    async fn factory_build_with_trait_injection() {
        let json = serde_json::json!({
            "whisperEndpoint": "http://localhost:8112",
            "ttsEndpoint": "http://localhost:8113",
        });
        let adapter = VoiceChannelAdapterFactory::build_with(
            &json,
            Arc::new(MemoryAudioSource::new(vec![], 1_600)),
            Arc::new(MemoryPlaybackSink::new()),
        )
        .unwrap();
        assert_eq!(adapter.name(), "voice");
    }

    #[tokio::test]
    async fn factory_rejects_bad_config() {
        let json = serde_json::json!({
            "whisperEndpoint": "ftp://nope",
        });
        let r = VoiceChannelAdapterFactory::build_with(
            &json,
            Arc::new(MemoryAudioSource::new(vec![], 1)),
            Arc::new(MemoryPlaybackSink::new()),
        );
        assert!(r.is_err());
    }

    #[tokio::test]
    async fn transcribe_segment_round_trip() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/inference"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"text": " hello world "}),
            ))
            .mount(&server)
            .await;

        let cfg = VoiceAdapterConfig {
            whisper_endpoint: server.uri(),
            tts_endpoint: "http://localhost:1".into(),
            ..Default::default()
        };
        let adapter = VoiceChannelAdapter::new(
            cfg,
            Arc::new(MemoryAudioSource::new(vec![], 1)),
            Arc::new(MemoryPlaybackSink::new()),
        )
        .unwrap();
        let seg = AudioSegment {
            samples: loud_frame(1_600),
            sample_rate: 16_000,
        };
        let text = adapter.transcribe_segment(&seg).await.unwrap();
        assert_eq!(text, "hello world");
    }

    #[tokio::test]
    async fn transcribe_segment_5xx_is_retriable_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/inference"))
            .respond_with(ResponseTemplate::new(503).set_body_string("loading"))
            .mount(&server)
            .await;

        let cfg = VoiceAdapterConfig {
            whisper_endpoint: server.uri(),
            tts_endpoint: "http://localhost:1".into(),
            ..Default::default()
        };
        let adapter = VoiceChannelAdapter::new(
            cfg,
            Arc::new(MemoryAudioSource::new(vec![], 1)),
            Arc::new(MemoryPlaybackSink::new()),
        )
        .unwrap();
        let seg = AudioSegment {
            samples: loud_frame(1_600),
            sample_rate: 16_000,
        };
        let err = adapter.transcribe_segment(&seg).await.unwrap_err();
        assert!(matches!(err, VoiceError::Server { status: 503, .. }));
    }

    #[tokio::test]
    async fn transcribe_segment_malformed_json() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/inference"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
            .mount(&server)
            .await;

        let cfg = VoiceAdapterConfig {
            whisper_endpoint: server.uri(),
            tts_endpoint: "http://localhost:1".into(),
            ..Default::default()
        };
        let adapter = VoiceChannelAdapter::new(
            cfg,
            Arc::new(MemoryAudioSource::new(vec![], 1)),
            Arc::new(MemoryPlaybackSink::new()),
        )
        .unwrap();
        let seg = AudioSegment {
            samples: loud_frame(1_600),
            sample_rate: 16_000,
        };
        let err = adapter.transcribe_segment(&seg).await.unwrap_err();
        assert!(matches!(err, VoiceError::Malformed(_)));
    }

    #[tokio::test]
    async fn synthesize_text_wav_round_trip() {
        let server = MockServer::start().await;
        let pcm = loud_frame(800);
        let wav = pcm_s16le_to_wav(&pcm, 16_000);
        Mock::given(method("POST"))
            .and(path("/synthesize"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "audio/wav")
                    .set_body_bytes(wav),
            )
            .mount(&server)
            .await;
        let cfg = VoiceAdapterConfig {
            whisper_endpoint: "http://localhost:1".into(),
            tts_endpoint: server.uri(),
            ..Default::default()
        };
        let adapter = VoiceChannelAdapter::new(
            cfg,
            Arc::new(MemoryAudioSource::new(vec![], 1)),
            Arc::new(MemoryPlaybackSink::new()),
        )
        .unwrap();
        let (out_pcm, sr) = adapter.synthesize_text("hello").await.unwrap();
        assert_eq!(sr, 16_000);
        assert_eq!(out_pcm, pcm);
    }

    #[tokio::test]
    async fn synthesize_text_json_b64_round_trip() {
        let server = MockServer::start().await;
        // Encode 4 PCM samples as little-endian s16 then base64 by hand.
        let pcm: Vec<i16> = vec![10, -10, 1_000, -1_000];
        let raw: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
        // Simple base64 encoder for the test.
        fn enc(b: &[u8]) -> String {
            const A: &[u8] =
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
            let mut out = String::new();
            let mut i = 0;
            while i + 3 <= b.len() {
                let n = ((b[i] as u32) << 16) | ((b[i + 1] as u32) << 8) | b[i + 2] as u32;
                out.push(A[((n >> 18) & 0x3F) as usize] as char);
                out.push(A[((n >> 12) & 0x3F) as usize] as char);
                out.push(A[((n >> 6) & 0x3F) as usize] as char);
                out.push(A[(n & 0x3F) as usize] as char);
                i += 3;
            }
            // Pad remaining (test PCM is 8 bytes — multiple of 3? 8 % 3 = 2)
            if i < b.len() {
                let mut buf = [0u8; 3];
                let rem = b.len() - i;
                buf[..rem].copy_from_slice(&b[i..]);
                let n = ((buf[0] as u32) << 16) | ((buf[1] as u32) << 8) | buf[2] as u32;
                out.push(A[((n >> 18) & 0x3F) as usize] as char);
                out.push(A[((n >> 12) & 0x3F) as usize] as char);
                if rem == 2 {
                    out.push(A[((n >> 6) & 0x3F) as usize] as char);
                    out.push('=');
                } else {
                    out.push('=');
                    out.push('=');
                }
            }
            out
        }
        let body = serde_json::json!({
            "audio_b64": enc(&raw),
            "sample_rate": 16_000,
        });
        Mock::given(method("POST"))
            .and(path("/synthesize"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_json(body),
            )
            .mount(&server)
            .await;
        let cfg = VoiceAdapterConfig {
            whisper_endpoint: "http://localhost:1".into(),
            tts_endpoint: server.uri(),
            ..Default::default()
        };
        let adapter = VoiceChannelAdapter::new(
            cfg,
            Arc::new(MemoryAudioSource::new(vec![], 1)),
            Arc::new(MemoryPlaybackSink::new()),
        )
        .unwrap();
        let (out_pcm, sr) = adapter.synthesize_text("hi").await.unwrap();
        assert_eq!(sr, 16_000);
        assert_eq!(out_pcm, pcm);
    }

    #[tokio::test]
    async fn send_records_message_id() {
        let server = MockServer::start().await;
        let pcm = loud_frame(160);
        let wav = pcm_s16le_to_wav(&pcm, 16_000);
        Mock::given(method("POST"))
            .and(path("/synthesize"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "audio/wav")
                    .set_body_bytes(wav),
            )
            .mount(&server)
            .await;
        let cfg = VoiceAdapterConfig {
            whisper_endpoint: "http://localhost:1".into(),
            tts_endpoint: server.uri(),
            ..Default::default()
        };
        let sink = Arc::new(MemoryPlaybackSink::new());
        let adapter = VoiceChannelAdapter::new(
            cfg,
            Arc::new(MemoryAudioSource::new(vec![], 1)),
            sink.clone(),
        )
        .unwrap();
        let id = adapter
            .send("user", &MessagePayload::text("hello"))
            .await
            .unwrap();
        assert!(id.starts_with("voice-user-"));
        let played = sink.played().await;
        assert_eq!(played.len(), 1);
        assert_eq!(played[0], pcm);
    }

    #[tokio::test]
    async fn send_skips_non_text_payload() {
        let adapter = VoiceChannelAdapter::new(
            VoiceAdapterConfig::default(),
            Arc::new(MemoryAudioSource::new(vec![], 1)),
            Arc::new(MemoryPlaybackSink::new()),
        )
        .unwrap();
        let id = adapter
            .send(
                "user",
                &MessagePayload::structured(serde_json::json!({"k": "v"})),
            )
            .await
            .unwrap();
        assert_eq!(id, "voice-skipped");
    }

    #[tokio::test]
    async fn send_returns_empty_for_blank_text() {
        let adapter = VoiceChannelAdapter::new(
            VoiceAdapterConfig::default(),
            Arc::new(MemoryAudioSource::new(vec![], 1)),
            Arc::new(MemoryPlaybackSink::new()),
        )
        .unwrap();
        let id = adapter
            .send("user", &MessagePayload::text("   "))
            .await
            .unwrap();
        assert_eq!(id, "voice-empty");
    }

    /// End-to-end: capture script → VAD splits utterance → transcribe →
    /// `deliver_inbound` is called with the substrate transcript text.
    #[tokio::test]
    async fn end_to_end_capture_vad_stt_publish() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/inference"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"text": "hello from substrate"}),
            ))
            .mount(&server)
            .await;
        let script = build_script(16_000);
        let source = Arc::new(MemoryAudioSource::new(script, 1_600));
        let sink = Arc::new(MemoryPlaybackSink::new());
        let cfg = VoiceAdapterConfig {
            whisper_endpoint: server.uri(),
            tts_endpoint: "http://localhost:1".into(),
            silence_ms: 500,
            min_utterance_ms: 100,
            sender_id: "voice-test".into(),
            chat_id: "voice-test-chat".into(),
            ..Default::default()
        };
        let adapter = Arc::new(
            VoiceChannelAdapter::new(cfg, source, sink).unwrap(),
        );
        let host = Arc::new(RecordingHost::default());
        let host_dyn: Arc<dyn ChannelAdapterHost> = host.clone();
        let cancel = clawft_plugin::traits::CancellationToken::new();
        let cancel_clone = cancel.clone();
        let adapter_clone = adapter.clone();
        let handle = tokio::spawn(async move {
            adapter_clone.start(host_dyn, cancel_clone).await.unwrap();
        });
        // The script is 1.5 s of audio at 16 kHz delivered in 100 ms
        // chunks; allow generous wall time for the VAD + HTTP round-trip.
        for _ in 0..40 {
            if !host.delivered.lock().await.is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        cancel.cancel();
        // Watchdog polls every 50 ms; allow up to 500 ms for shutdown.
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            handle,
        )
        .await
        .unwrap();
        let delivered = host.delivered.lock().await;
        assert!(
            !delivered.is_empty(),
            "expected at least one inbound delivery"
        );
        let (channel, sender, chat, payload, meta) = &delivered[0];
        assert_eq!(channel, "voice");
        assert_eq!(sender, "voice-test");
        assert_eq!(chat, "voice-test-chat");
        assert_eq!(payload.as_text(), Some("hello from substrate"));
        assert!(meta.contains_key("voice_span_ms"));
        assert!(meta.contains_key("voice_sample_rate"));
    }

    #[test]
    fn base64_decoder_basic() {
        assert_eq!(decode_base64("AAAA").unwrap(), vec![0u8, 0, 0]);
        // "Hello" -> SGVsbG8=
        assert_eq!(
            decode_base64("SGVsbG8=").unwrap(),
            b"Hello".to_vec()
        );
        assert!(decode_base64("@@@@").is_err());
    }
}
