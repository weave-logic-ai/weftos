//! Energy-based voice activity detector.
//!
//! Frame-by-frame RMS over an `s16le` PCM stream, gated by a dBFS
//! threshold and a silence tail. Emits [`VadEvent`]s on speech-start /
//! speech-end so the channel can slice utterances. Mirrors the model
//! `clawft-service-classify::EnergyClassifier` uses (-45 dBFS default)
//! so behaviour is consistent across the substrate.
//!
//! This is intentionally not Silero / WebRTC-VAD: those pull heavy ML
//! deps and the M5 ADR (WEFT-205) hasn't picked one. Energy-VAD is the
//! lowest-common-denominator that works without a model file and without
//! a runtime dependency on `voice_activity_detector` or sherpa-rs. If
//! WEFT-205 picks a real VAD later, this trait + module is the seam.

/// Events produced by the VAD as PCM frames are fed in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VadEvent {
    /// Speech onset detected at sample offset `at_sample`.
    SpeechStart {
        /// Cumulative sample index at which the utterance began.
        at_sample: u64,
    },
    /// Speech end detected at sample offset `at_sample`. The captured
    /// utterance covers `[start_sample, at_sample]`.
    SpeechEnd {
        /// Cumulative sample index at which speech started.
        start_sample: u64,
        /// Cumulative sample index at which silence reached the tail.
        at_sample: u64,
    },
}

/// Streaming energy-RMS VAD.
///
/// Hold one of these per capture stream. Push frames via [`Self::feed`];
/// it returns zero or more `VadEvent`s. State is fully internal.
///
/// # Adaptive silence (WEFT-230)
///
/// Attach a [`clawft_types::config::AdaptiveSilenceTimeout`] with
/// [`Self::with_adaptive`]; the VAD then tracks the longest *intra-utterance*
/// pause, feeds it to the estimator on each emitted `SpeechEnd`, and updates
/// the silence tail for the next utterance. Disabled by default so existing
/// fixed-timeout call sites keep their behaviour.
#[derive(Debug)]
pub struct EnergyVad {
    sample_rate: u32,
    threshold_dbfs: f32,
    /// Baseline silence tail from construction (ms); adaptive rebaselines here.
    baseline_silence_ms: u32,
    silence_tail_samples: u64,
    min_utterance_samples: u64,
    max_utterance_samples: u64,
    cumulative: u64,
    in_speech: bool,
    speech_start: u64,
    silence_run: u64,
    /// Samples of *active speech* observed since the most recent
    /// `SpeechStart`. Excludes silence frames; this is what
    /// `min_utterance_samples` is checked against, so a brief blip
    /// followed by a long silence tail does not count as an utterance.
    speech_samples: u64,
    /// Longest silence run (samples) *inside* the current utterance that
    /// was broken by resumed speech — natural mid-phrase pause signal.
    max_intra_pause_samples: u64,
    /// Optional per-session adaptive silence-timeout estimator (WEFT-230).
    adaptive: Option<clawft_types::config::AdaptiveSilenceTimeout>,
}

impl EnergyVad {
    /// Build a VAD.
    ///
    /// `silence_ms` is the trailing-silence window that ends an
    /// utterance. `min_utterance_ms` and `max_utterance_ms` clamp the
    /// emitted segment lengths.
    pub fn new(
        sample_rate: u32,
        threshold_dbfs: f32,
        silence_ms: u32,
        min_utterance_ms: u32,
        max_utterance_ms: u32,
    ) -> Self {
        let s = u64::from(sample_rate);
        Self {
            sample_rate,
            threshold_dbfs,
            baseline_silence_ms: silence_ms,
            silence_tail_samples: s * u64::from(silence_ms) / 1_000,
            min_utterance_samples: s * u64::from(min_utterance_ms) / 1_000,
            max_utterance_samples: s * u64::from(max_utterance_ms) / 1_000,
            cumulative: 0,
            in_speech: false,
            speech_start: 0,
            silence_run: 0,
            speech_samples: 0,
            max_intra_pause_samples: 0,
            adaptive: None,
        }
    }

    /// Attach a per-session adaptive silence-timeout estimator (WEFT-230).
    ///
    /// Immediately applies the estimator's current timeout to the silence
    /// tail. Call before feeding frames.
    pub fn with_adaptive(
        mut self,
        adaptive: clawft_types::config::AdaptiveSilenceTimeout,
    ) -> Self {
        let ms = adaptive.current_ms();
        self.adaptive = Some(adaptive);
        self.set_silence_ms(ms);
        self
    }

    /// Sample rate the VAD was constructed with.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Current silence-tail length in milliseconds (learned or baseline).
    pub fn silence_ms(&self) -> u32 {
        if self.sample_rate == 0 {
            return self.baseline_silence_ms;
        }
        ((self.silence_tail_samples * 1_000) / u64::from(self.sample_rate)) as u32
    }

    /// Baseline silence timeout from construction (ms).
    pub fn baseline_silence_ms(&self) -> u32 {
        self.baseline_silence_ms
    }

    /// Update the silence-tail length used to end an utterance.
    pub fn set_silence_ms(&mut self, silence_ms: u32) {
        let s = u64::from(self.sample_rate);
        self.silence_tail_samples = s * u64::from(silence_ms) / 1_000;
    }

    /// Borrow the adaptive estimator, if attached.
    pub fn adaptive(&self) -> Option<&clawft_types::config::AdaptiveSilenceTimeout> {
        self.adaptive.as_ref()
    }

    /// Mutable access to the adaptive estimator, if attached.
    pub fn adaptive_mut(
        &mut self,
    ) -> Option<&mut clawft_types::config::AdaptiveSilenceTimeout> {
        self.adaptive.as_mut()
    }

    /// Compute the RMS energy of `frame` in dBFS, clamped to [-100, 0].
    ///
    /// Empty frames return -100 (treated as silence).
    pub fn rms_dbfs(frame: &[i16]) -> f32 {
        if frame.is_empty() {
            return -100.0;
        }
        let sum_sq: f64 = frame.iter().map(|&s| (s as f64).powi(2)).sum();
        let rms = (sum_sq / frame.len() as f64).sqrt();
        if rms <= 0.0 {
            return -100.0;
        }
        // i16 full-scale = 32_768.
        let dbfs = 20.0 * (rms / 32_768.0).log10();
        dbfs.clamp(-100.0, 0.0) as f32
    }

    /// Whether the VAD currently believes speech is active.
    pub fn in_speech(&self) -> bool {
        self.in_speech
    }

    /// Push a frame of PCM samples and collect events.
    ///
    /// Returns the events produced by this frame (zero, one, or — in
    /// degenerate cases like a max-length flush followed by a continued
    /// frame — two).
    pub fn feed(&mut self, frame: &[i16]) -> Vec<VadEvent> {
        let mut events = Vec::new();
        if frame.is_empty() {
            return events;
        }
        let dbfs = Self::rms_dbfs(frame);
        let frame_len = frame.len() as u64;
        let is_speech = dbfs >= self.threshold_dbfs;

        if !self.in_speech {
            if is_speech {
                self.in_speech = true;
                self.speech_start = self.cumulative;
                self.silence_run = 0;
                self.speech_samples = frame_len;
                self.max_intra_pause_samples = 0;
                events.push(VadEvent::SpeechStart {
                    at_sample: self.speech_start,
                });
            }
        } else {
            if is_speech {
                // Resumed speech after a mid-utterance pause: record the
                // pause length for adaptive learning (WEFT-230).
                if self.silence_run > 0 {
                    self.max_intra_pause_samples =
                        self.max_intra_pause_samples.max(self.silence_run);
                }
                self.silence_run = 0;
                self.speech_samples = self.speech_samples.saturating_add(frame_len);
            } else {
                self.silence_run = self.silence_run.saturating_add(frame_len);
            }
            // End-of-speech if silence tail exceeded.
            let end_at = self.cumulative + frame_len;
            let utterance_len = end_at.saturating_sub(self.speech_start);
            let end_by_silence = self.silence_run >= self.silence_tail_samples;
            let end_by_max = utterance_len >= self.max_utterance_samples;
            if end_by_silence || end_by_max {
                // `min_utterance_samples` is checked against speech-only
                // duration so brief blips followed by long silence don't
                // count as utterances.
                if self.speech_samples >= self.min_utterance_samples {
                    events.push(VadEvent::SpeechEnd {
                        start_sample: self.speech_start,
                        at_sample: end_at,
                    });
                    self.learn_from_completed_utterance();
                }
                self.in_speech = false;
                self.silence_run = 0;
                self.speech_samples = 0;
                self.max_intra_pause_samples = 0;
            }
        }

        self.cumulative = self.cumulative.saturating_add(frame_len);
        events
    }

    /// Feed speech/pause stats into the adaptive estimator and refresh
    /// the silence tail for the next utterance (no-op when adaptive is off).
    fn learn_from_completed_utterance(&mut self) {
        let Some(adaptive) = self.adaptive.as_mut() else {
            return;
        };
        let sr = u64::from(self.sample_rate).max(1);
        let speech_ms = ((self.speech_samples * 1_000) / sr) as u32;
        let max_pause_ms = ((self.max_intra_pause_samples * 1_000) / sr) as u32;
        adaptive.record_utterance(speech_ms, max_pause_ms);
        let next = adaptive.current_ms();
        let s = u64::from(self.sample_rate);
        self.silence_tail_samples = s * u64::from(next) / 1_000;
    }
}

/// Adaptive noise-floor tracker (dBFS).
///
/// Room tone varies machine-to-machine (fans, HVAC, street noise) *and* with
/// the OS input-gain slider, which the user may change between sessions or
/// mid-session. A fixed dBFS gate mis-classifies a loud room as permanent
/// speech and the silence-based endpointer never finalizes a turn (observed
/// live: a -37 dBFS room vs the -45 dBFS default gate — Talk-Mode "never heard"
/// anyone). Two failure directions have to be defended at once:
///
/// * **Gate stuck CLOSED** — quiet speech drags a chasing floor up under
///   itself until only word attacks clear the gate (round-6). Defence: gate at
///   `floor + margin` (Schmitt-trigger) and **freeze the floor while voiced**,
///   learning it only from genuine silence.
/// * **Gate stuck OPEN** — room tone above the frozen floor reads as perpetual
///   speech, so no genuine-silence frame ever arrives, the floor can never
///   learn up, and the endpointer never fires (round-7, triggered the instant
///   the user raised input gain). Defence: **startup calibration** (seed the
///   floor from the first `CALIB_MS` of a session, assumed non-speech, so any
///   gain setting is handled at boot) plus a **stuck-open watchdog** (if
///   "speech" persists past `WATCHDOG_MS` with no endpoint, force-recalibrate
///   the floor from the quietest recent frames — a real monologue has
///   micro-pauses, so continuous voicing that long means the floor is wrong).
#[derive(Debug)]
pub struct NoiseFloor {
    /// Quiet-p10 noise floor — tracks the QUIETEST recent ambient (asymmetric,
    /// fast-down). Kept for SNR reporting ([`floor_dbfs`](Self::floor_dbfs)).
    floor_dbfs: f32,
    /// TYPICAL-ambient gate reference — a symmetric EMA that settles at the
    /// room's median level, not its quietest. The onset/sustain gate is computed
    /// from THIS, not `floor_dbfs`: the fast-down floor drifts below the room's
    /// typical, dropping the gate under the room's own PEAKS so a finished
    /// utterance's trailing room re-crosses onset / holds sustain until the
    /// watchdog (the Wall-5 sustain caveat). Referencing the typical level keeps
    /// room peaks below both thresholds by construction.
    gate_floor_dbfs: f32,
    margin_db: f32,
    /// Capture sample rate — frames the gate-reference learning by frame
    /// duration so its time constant is frame-size independent.
    sample_rate: u32,
    /// Whether startup calibration has completed (floor seeded from the first
    /// `CALIB_MS`). Until then `classify` buffers frames and reports non-voiced.
    calibrated: bool,
    /// dBFS of frames seen during the calibration window (median → floor seed).
    calib_frames: Vec<f32>,
    /// Samples accumulated toward the `calib_target_samples` window.
    calib_samples: u64,
    /// Calibration window length in samples (from the sample rate).
    calib_target_samples: u64,
    /// Hard cap on the (possibly extended) calibration window before it arms
    /// anyway (from the sample rate).
    calib_max_samples: u64,
    /// Whether the gate currently reads speech (drives the sustain hysteresis
    /// threshold and the floor freeze).
    in_speech: bool,
    /// Remaining hangover (samples): while > 0 a below-threshold frame is still
    /// treated as voiced so a brief intra-word dip doesn't close the gate.
    hangover_remaining: u64,
    /// Hangover length in samples (from the sample rate).
    hangover_samples: u64,
    /// Count of frames judged genuine silence. The floor only learns from these
    /// (see [`converged`](Self::converged)).
    quiet_frames: u32,
    /// Continuous voiced run (samples, including hangover). Reset by any
    /// genuine-silence frame; when it reaches `watchdog_samples` the stuck-open
    /// watchdog fires.
    voiced_run_samples: u64,
    /// Watchdog trip point in samples (from the sample rate).
    watchdog_samples: u64,
    /// Recent per-frame `(dbfs, len)` over ~`WATCHDOG_WINDOW_MS`; the watchdog
    /// recalibrates the floor from the p10 of this ring.
    recent: std::collections::VecDeque<(f32, u64)>,
    /// Sum of `len` in `recent`, for O(1) window eviction.
    recent_samples: u64,
    /// Rolling window length in samples (from the sample rate).
    recent_target_samples: u64,
    /// How many times the stuck-open watchdog has force-recalibrated the floor.
    /// Surfaced for diagnostics / tests.
    recalibrations: u32,
}

/// Silence frames the floor must observe before it is a trustworthy noise
/// estimate — until then SNR is flagged unreliable in the record.
const FLOOR_CONVERGE_FRAMES: u32 = 10;

/// Startup calibration window: the first this-many ms of a session are assumed
/// non-speech and seed the floor from their median. Handles any input-gain
/// setting at boot (the round-7 stuck-open cause). A user who starts talking
/// inside this window loses only the opening ~half second.
const CALIB_MS: u32 = 500;

/// Frames quieter than this are treated as PRE-SPIN-UP DEAD AIR, not room tone:
/// cpal emits near-digital-silence (~-97 dBFS) for the first tens-to-hundreds of
/// ms before the input device starts delivering real samples. They must NOT seed
/// the floor — the p20 quiet-end seed would otherwise latch onto them and arm the
/// gate at ~-97 dBFS, so room tone reads as permanent speech and the loop goes
/// deaf ("Wall 3"). Real room tone is far louder than this even in a quiet room.
const DEAD_FRAME_DBFS: f32 = -80.0;

/// Physical floor clamp: no real microphone room tone sits below this. Clamping
/// the seed here is a backstop against any remaining dead-air contamination so
/// the onset can never arm absurdly low.
const FLOOR_MIN_DBFS: f32 = -65.0;

/// Hard cap on how long calibration may EXTEND while the window stays unstable
/// before it arms anyway (from the quiet end, clamped). Bounds the worst case so
/// the gate always arms.
const CALIB_MAX_MS: u32 = 3_000;

/// Calibration-window dBFS spread above which the window is deemed unstable
/// (speech/movement contaminated the assumed-silent seed): calibration EXTENDS
/// (keeps collecting real frames) rather than arming on the noisy seed, up to
/// [`CALIB_MAX_MS`]. Room tone alone sits within a few dB; speech spikes 15+ dB.
const CALIB_UNSTABLE_STDDEV_DB: f32 = 6.0;

/// Hangover tail: once voiced, stay in capture until this much CONTINUOUS
/// below-threshold audio, so brief intra-word dips (stop closures, unvoiced
/// consonants) don't split an utterance.
const HANGOVER_MS: u32 = 400;

/// Stuck-open watchdog trip: continuous voicing longer than this with no
/// intervening genuine-silence frame means the floor is miscalibrated (a real
/// monologue breathes), so recalibrate from the quietest recent frames.
const WATCHDOG_MS: u32 = 15_000;

/// Rolling window the watchdog samples for its p10 recalibration floor.
const WATCHDOG_WINDOW_MS: u32 = 5_000;

/// Percentile of the recent window used as the recalibrated floor (quietest
/// tenth ≈ the true room tone even while the mean is dominated by speech).
const WATCHDOG_PCT: usize = 10;

/// Time constant (seconds) for the typical-ambient gate reference to settle
/// (~63%). Learned from ANY below-onset frame (see `classify`), FRAME-RATE
/// INDEPENDENT (scaled by frame duration), so ~1 s of ambient lifts it whether
/// capture delivers 10 ms or 100 ms frames — the fix#2-regression was the
/// reference frozen while the room held the gate, and a per-frame rate that only
/// settled at one frame size. ~1 s: fast enough that a room-held gate closes in a
/// couple seconds, slow enough that a brief mid-utterance ambient dip barely
/// moves it.
const GATE_TIME_CONSTANT_S: f32 = 1.0;

impl NoiseFloor {
    /// `margin_db` is the strict onset margin over the tracked floor a frame
    /// must clear to START speech (8–10 dB at conversational distance); the
    /// loose sustain threshold is half that. `sample_rate` sizes the calibration
    /// window, hangover, and watchdog timers.
    pub fn new(margin_db: f32, sample_rate: u32) -> Self {
        let sr = u64::from(sample_rate);
        Self {
            floor_dbfs: -100.0,
            gate_floor_dbfs: -100.0,
            margin_db,
            sample_rate: sample_rate.max(1),
            calibrated: false,
            calib_frames: Vec::new(),
            calib_samples: 0,
            calib_target_samples: sr * u64::from(CALIB_MS) / 1_000,
            calib_max_samples: sr * u64::from(CALIB_MAX_MS) / 1_000,
            in_speech: false,
            hangover_remaining: 0,
            hangover_samples: sr * u64::from(HANGOVER_MS) / 1_000,
            quiet_frames: 0,
            voiced_run_samples: 0,
            watchdog_samples: sr * u64::from(WATCHDOG_MS) / 1_000,
            recent: std::collections::VecDeque::new(),
            recent_samples: 0,
            recent_target_samples: sr * u64::from(WATCHDOG_WINDOW_MS) / 1_000,
            recalibrations: 0,
        }
    }

    /// Classify one frame as voiced. Applies a **Schmitt-trigger** gate (strict
    /// onset at `floor+margin`, loose sustain at `floor+margin/2`), a
    /// **hangover** tail so intra-word dips don't close the gate, and — the key
    /// discipline — **freezes the floor while in speech/hangover**, learning it
    /// ONLY from genuine silence (fast down, very slow up). Startup calibration
    /// seeds the floor from the first [`CALIB_MS`] (assumed non-speech) so the
    /// gate is correct at any input gain from boot; a stuck-open watchdog
    /// force-recalibrates the floor from the quietest recent frames if voicing
    /// runs past [`WATCHDOG_MS`] with no genuine-silence frame — the escape
    /// hatch from a gate wedged OPEN by a mid-session gain jump.
    pub fn classify(&mut self, dbfs: f32, frame_len: u64) -> bool {
        // Startup calibration: buffer the first CALIB_MS as non-speech and seed
        // the floor from their median. Report non-voiced throughout the window.
        if !self.calibrated {
            // Ignore pre-spin-up DEAD frames (cpal emits ~digital silence before
            // the device starts). They must never seed the floor: p20 would latch
            // onto them and arm the gate at ~-97 dBFS, wedging it permanently open
            // ("Wall 3"). Don't even start the window until real audio arrives.
            if dbfs < DEAD_FRAME_DBFS {
                return false;
            }
            self.calib_frames.push(dbfs);
            self.calib_samples = self.calib_samples.saturating_add(frame_len);
            if self.calib_samples < self.calib_target_samples {
                return false;
            }
            // Have a full window of real audio. If it's UNSTABLE (speech/movement
            // mixed in), keep collecting rather than arming on a noisy seed —
            // bounded by calib_max_samples so the gate always arms eventually.
            let spread = stddev(&self.calib_frames);
            if spread > CALIB_UNSTABLE_STDDEV_DB && self.calib_samples < self.calib_max_samples {
                return false;
            }
            // Seed from the 20th percentile (≈ the quietest frames, i.e. room
            // tone even if speech contaminated the window), clamped to a physical
            // floor so a stray dead frame can't arm the onset absurdly low.
            self.floor_dbfs = percentile(&self.calib_frames, 20).clamp(FLOOR_MIN_DBFS, 0.0);
            // Seed the gate reference from the SAME quiet p20 (contamination-safe
            // — never from the p50, which speech in the window would poison); it
            // learns UP to the typical ambient over the session.
            self.gate_floor_dbfs = self.floor_dbfs;
            self.calibrated = true;
            if spread > CALIB_UNSTABLE_STDDEV_DB {
                tracing::warn!(
                    floor_dbfs = self.floor_dbfs,
                    stddev_db = spread,
                    "noise-floor calibration stayed unstable to the cap — armed from p20 (clamped)"
                );
            }
            return false;
        }

        self.push_recent(dbfs, frame_len);

        // Stuck-open watchdog: continuous voicing past the trip point without a
        // genuine-silence frame means the floor is wrong. Recalibrate it to the
        // p10 of the recent window before this frame's gate decision so the very
        // frame that tripped the watchdog is re-judged against the true floor.
        if self.voiced_run_samples >= self.watchdog_samples {
            let quiet = self.recent_percentile(WATCHDOG_PCT);
            let typical = self.recent_percentile(50);
            let old = self.gate_floor_dbfs;
            self.floor_dbfs = quiet;
            self.gate_floor_dbfs = typical.max(quiet);
            self.in_speech = false;
            self.hangover_remaining = 0;
            self.voiced_run_samples = 0;
            self.recalibrations = self.recalibrations.saturating_add(1);
            tracing::warn!(
                old_gate_floor_dbfs = old,
                new_gate_floor_dbfs = self.gate_floor_dbfs,
                new_floor_dbfs = quiet,
                "noise-floor watchdog: gate stuck open — recalibrated to recent typical"
            );
        }

        // Gate off the TYPICAL-ambient reference so room peaks stay below both
        // thresholds (Wall-5 fix); the quiet floor is for SNR only.
        let onset = (self.gate_floor_dbfs + self.margin_db).clamp(-100.0, 0.0);
        let sustain = (self.gate_floor_dbfs + self.margin_db * 0.5).clamp(-100.0, 0.0);
        let gate = if self.in_speech { sustain } else { onset };

        // Learn the gate reference from ANY below-onset (ambient) frame — even one
        // currently HOLDING the gate open via sustain/hangover. Freezing it during
        // a room-held gate is what let the room keep the gate open for the full
        // 15 s watchdog window (the fix#2 regression); keeping it learning lifts
        // sustain above the room within ~1 s so the gate closes. Real speech
        // (above onset) leaves it frozen. Scaled by frame duration → the ~1 s time
        // constant holds whether capture delivers 10 ms or 100 ms frames.
        if dbfs < onset {
            let alpha = (frame_len as f32 / self.sample_rate as f32 / GATE_TIME_CONSTANT_S)
                .clamp(0.0, 1.0);
            self.gate_floor_dbfs += (dbfs - self.gate_floor_dbfs) * alpha;
            self.gate_floor_dbfs = self.gate_floor_dbfs.max(self.floor_dbfs);
        }

        if dbfs >= gate {
            self.in_speech = true;
            self.hangover_remaining = self.hangover_samples;
            self.voiced_run_samples = self.voiced_run_samples.saturating_add(frame_len);
            true
        } else if self.hangover_remaining > 0 {
            self.hangover_remaining = self.hangover_remaining.saturating_sub(frame_len);
            self.voiced_run_samples = self.voiced_run_samples.saturating_add(frame_len);
            true
        } else {
            // Genuine silence: the quiet floor learns here (asymmetric — fast
            // toward quieter evidence, very slow up, settling at the p10 for SNR);
            // the gate reference already learned above from this below-onset frame.
            self.in_speech = false;
            self.voiced_run_samples = 0;
            let delta = dbfs - self.floor_dbfs;
            self.floor_dbfs += if delta < 0.0 { delta * 0.3 } else { delta * 0.02 };
            self.quiet_frames = self.quiet_frames.saturating_add(1);
            false
        }
    }

    /// Push a frame into the rolling watchdog window, evicting stale samples.
    fn push_recent(&mut self, dbfs: f32, frame_len: u64) {
        self.recent.push_back((dbfs, frame_len));
        self.recent_samples = self.recent_samples.saturating_add(frame_len);
        while self.recent_samples > self.recent_target_samples && self.recent.len() > 1 {
            if let Some((_, len)) = self.recent.pop_front() {
                self.recent_samples = self.recent_samples.saturating_sub(len);
            }
        }
    }

    /// The `pct`-th percentile dBFS of the recent window (quietest tenth ≈ the
    /// true room tone). Falls back to the current floor if the window is empty.
    fn recent_percentile(&self, pct: usize) -> f32 {
        if self.recent.is_empty() {
            return self.floor_dbfs;
        }
        let mut vals: Vec<f32> = self.recent.iter().map(|&(d, _)| d).collect();
        vals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let idx = (vals.len().saturating_sub(1)) * pct / 100;
        vals[idx]
    }

    /// Current voice onset threshold (`gate_floor + margin`).
    pub fn threshold_dbfs(&self) -> f32 {
        (self.gate_floor_dbfs + self.margin_db).clamp(-100.0, 0.0)
    }

    /// Current tracked QUIET floor (p10) — the SNR reference.
    pub fn floor_dbfs(&self) -> f32 {
        self.floor_dbfs
    }

    /// Current TYPICAL-ambient gate reference — the onset/sustain are computed
    /// from this (`gate_floor + margin` / `+ margin/2`).
    pub fn gate_floor_dbfs(&self) -> f32 {
        self.gate_floor_dbfs
    }

    /// How many times the stuck-open watchdog has force-recalibrated the floor
    /// this session. Non-zero means the gate had to be rescued from a wedged
    /// state (usually a mid-session input-gain jump).
    pub fn recalibrations(&self) -> u32 {
        self.recalibrations
    }

    /// Whether startup calibration has completed (the floor is seeded). Callers
    /// use the transition to log the armed floor + onset once at session start.
    pub fn calibrated(&self) -> bool {
        self.calibrated
    }

    /// Current continuous-voiced run in samples — how close the stuck-open
    /// watchdog is to tripping (fires at [`WATCHDOG_MS`]). Surfaced so a probe can
    /// show it climbing: if it never approaches the trip point while the gate is
    /// wedged, frames are being dropped before the gate, not a watchdog defect.
    pub fn voiced_run_samples(&self) -> u64 {
        self.voiced_run_samples
    }

    /// Whether the floor has observed enough real silence to be a trustworthy
    /// noise estimate. Until this is true (e.g. the very first turn of a
    /// speech-first session), any SNR derived from the floor is unreliable and
    /// should be flagged in the record.
    pub fn converged(&self) -> bool {
        self.quiet_frames >= FLOOR_CONVERGE_FRAMES
    }
}

/// `pct`-th percentile of a dBFS slice (seeds the floor from the calibration
/// window's quiet end). `-100` for an empty slice.
fn percentile(vals: &[f32], pct: usize) -> f32 {
    if vals.is_empty() {
        return -100.0;
    }
    let mut v = vals.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = v.len().saturating_sub(1) * pct / 100;
    v[idx]
}

/// Standard deviation of a dBFS slice — the calibration-window stability check
/// (a high spread means speech/movement contaminated the assumed-silent window).
fn stddev(vals: &[f32]) -> f32 {
    if vals.len() < 2 {
        return 0.0;
    }
    let n = vals.len() as f32;
    let mean = vals.iter().sum::<f32>() / n;
    (vals.iter().map(|&x| (x - mean).powi(2)).sum::<f32>() / n).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a tone frame at the given amplitude.
    fn tone(len: usize, amp: i16) -> Vec<i16> {
        (0..len)
            .map(|i| if i % 2 == 0 { amp } else { -amp })
            .collect()
    }

    #[test]
    fn rms_silence_is_minus_100() {
        assert_eq!(EnergyVad::rms_dbfs(&[0i16; 256]), -100.0);
    }

    #[test]
    fn rms_full_scale_is_zero() {
        let v = vec![i16::MAX; 256];
        let dbfs = EnergyVad::rms_dbfs(&v);
        assert!(dbfs > -1.0, "expected near-zero dBFS, got {dbfs}");
    }

    #[test]
    fn rms_half_scale_is_around_minus_six() {
        let v = vec![16_000i16; 256];
        let dbfs = EnergyVad::rms_dbfs(&v);
        assert!(dbfs > -8.0 && dbfs < -4.0, "got {dbfs}");
    }

    #[test]
    fn vad_speech_then_silence_tail_emits_pair() {
        // 16 kHz, -45 dBFS threshold, 300 ms silence tail, 100 ms min,
        // 10 s max.
        let mut vad = EnergyVad::new(16_000, -45.0, 300, 100, 10_000);
        let frame = 1_600; // 100 ms
        // ~3000 amplitude → ~-20 dBFS, well above -45.
        let speech = tone(frame, 3_000);
        let silence = vec![0i16; frame];

        // 200 ms speech.
        let e1 = vad.feed(&speech);
        assert!(matches!(e1.first(), Some(VadEvent::SpeechStart { .. })));
        let e2 = vad.feed(&speech);
        assert!(e2.is_empty());
        assert!(vad.in_speech());

        // 400 ms silence — should cross the 300 ms tail and emit end.
        let e3 = vad.feed(&silence);
        let e4 = vad.feed(&silence);
        let e5 = vad.feed(&silence);
        let e6 = vad.feed(&silence);
        let ended = [e3, e4, e5, e6]
            .iter()
            .flatten()
            .any(|ev| matches!(ev, VadEvent::SpeechEnd { .. }));
        assert!(ended, "expected SpeechEnd within silence frames");
        assert!(!vad.in_speech());
    }

    #[test]
    fn vad_drops_below_min_utterance() {
        let mut vad = EnergyVad::new(16_000, -45.0, 200, 500, 10_000);
        let frame = 800; // 50 ms
        let speech = tone(frame, 3_000);
        let silence = vec![0i16; 16_000]; // 1 s
        // 50 ms speech (< 500 ms min) then silence.
        let e1 = vad.feed(&speech);
        assert!(matches!(e1.first(), Some(VadEvent::SpeechStart { .. })));
        let e2 = vad.feed(&silence);
        let no_end = !e2.iter().any(|ev| matches!(ev, VadEvent::SpeechEnd { .. }));
        assert!(no_end, "should drop sub-min utterance silently");
        assert!(!vad.in_speech());
    }

    #[test]
    fn vad_max_utterance_forces_flush() {
        // 200 ms max, 500 ms silence tail. Continuous speech should hit
        // the max-utterance ceiling and flush even though no silence
        // was observed. After the flush the next speech frame starts a
        // fresh utterance, so we just assert the flush happened.
        let mut vad = EnergyVad::new(16_000, -45.0, 500, 50, 200);
        let frame = 1_600; // 100 ms
        let speech = tone(frame, 3_000);
        let mut all_events = Vec::new();
        for _ in 0..3 {
            all_events.extend(vad.feed(&speech));
        }
        let ended_count = all_events
            .iter()
            .filter(|ev| matches!(ev, VadEvent::SpeechEnd { .. }))
            .count();
        assert!(
            ended_count >= 1,
            "max-len ceiling should force at least one flush"
        );
    }

    #[test]
    fn vad_does_not_emit_on_pure_silence() {
        let mut vad = EnergyVad::new(16_000, -45.0, 300, 100, 10_000);
        let silence = vec![0i16; 16_000];
        let events = vad.feed(&silence);
        assert!(events.is_empty());
        assert!(!vad.in_speech());
    }

    #[test]
    fn set_silence_ms_updates_tail() {
        let mut vad = EnergyVad::new(16_000, -45.0, 300, 100, 10_000);
        assert_eq!(vad.silence_ms(), 300);
        vad.set_silence_ms(1_500);
        assert_eq!(vad.silence_ms(), 1_500);
        assert_eq!(vad.baseline_silence_ms(), 300);
    }

    #[test]
    fn adaptive_learns_from_intra_pauses() {
        use clawft_types::config::{AdaptiveSilenceConfig, AdaptiveSilenceTimeout};

        // Baseline 1500 ms silence tail. Utterance with mid-pause ~400 ms
        // should pull the learned timeout down over repeated utterances.
        let adaptive = AdaptiveSilenceTimeout::new(
            1_500,
            AdaptiveSilenceConfig {
                enabled: true,
                learning_rate: 0.5,
                pause_margin_ms: 250,
                min_ms: 500,
                max_ms: 3_000,
                window_size: 8,
            },
        );
        let mut vad = EnergyVad::new(16_000, -45.0, 1_500, 50, 30_000).with_adaptive(adaptive);
        assert_eq!(vad.silence_ms(), 1_500);

        let frame = 1_600; // 100 ms
        let speech = tone(frame, 3_000);
        let silence = vec![0i16; frame];

        for _ in 0..6 {
            // 200 ms speech
            let _ = vad.feed(&speech);
            let _ = vad.feed(&speech);
            // 400 ms mid-phrase pause (under 1500 tail)
            let _ = vad.feed(&silence);
            let _ = vad.feed(&silence);
            let _ = vad.feed(&silence);
            let _ = vad.feed(&silence);
            // 200 ms speech resume
            let _ = vad.feed(&speech);
            let _ = vad.feed(&speech);
            // 1500+ ms trailing silence → SpeechEnd
            let mut ended = false;
            for _ in 0..20 {
                let ev = vad.feed(&silence);
                if ev.iter().any(|e| matches!(e, VadEvent::SpeechEnd { .. })) {
                    ended = true;
                    break;
                }
            }
            assert!(ended, "expected SpeechEnd after silence tail");
        }

        let learned = vad.silence_ms();
        assert!(
            (500..1_500).contains(&learned),
            "expected adaptive pull-down from 1500, got {learned}"
        );
        assert!(
            vad.adaptive().map(|a| a.observation_count()).unwrap_or(0) >= 1,
            "expected observations recorded"
        );
    }

    const F100: u64 = 1_600; // 100 ms @ 16 kHz

    #[test]
    fn floor_frozen_through_quiet_speech_no_truncation() {
        // Round-6 acceptance: at low SNR a full sentence stays ONE open
        // utterance and the floor stays flat across it. Learn a low floor from
        // leading silence, then feed 3 s of quiet speech (~10 dB over floor)
        // with periodic intra-word dips — every frame must read voiced and the
        // floor must not creep up under the speech.
        let mut nf = NoiseFloor::new(8.0, 16_000);
        for _ in 0..20 {
            nf.classify(-65.0, F100); // leading silence
        }
        let floor_before = nf.floor_dbfs();
        assert!(floor_before < -60.0, "floor learned from silence: {floor_before}");

        let mut voiced = 0;
        for i in 0..30 {
            // Speech at -55 with a brief dip to -63 (below the sustain gate) —
            // the hangover must bridge it.
            let dbfs = if i % 5 == 4 { -63.0 } else { -55.0 };
            if nf.classify(dbfs, F100) {
                voiced += 1;
            }
        }
        assert_eq!(voiced, 30, "every quiet-speech frame (incl. dips) stays voiced");
        let floor_after = nf.floor_dbfs();
        assert!(
            (floor_after - floor_before).abs() < 2.0,
            "floor must stay flat through speech: {floor_before} -> {floor_after}"
        );
    }

    #[test]
    fn floor_flat_across_speech_heavy_session() {
        // Acceptance: floor delta < 2 dB before vs after 10 utterances.
        let mut nf = NoiseFloor::new(8.0, 16_000);
        for _ in 0..15 {
            nf.classify(-65.0, F100); // establish the floor
        }
        let before = nf.floor_dbfs();
        for _ in 0..10 {
            for _ in 0..15 {
                nf.classify(-55.0, F100); // ~1.5 s of speech
            }
            for _ in 0..8 {
                nf.classify(-65.0, F100); // inter-utterance silence
            }
        }
        let after = nf.floor_dbfs();
        assert!(
            (after - before).abs() < 2.0,
            "floor drifted across a speech-heavy session: {before} -> {after}"
        );
    }

    #[test]
    fn hysteresis_strict_onset_loose_sustain() {
        let mut nf = NoiseFloor::new(8.0, 16_000);
        for _ in 0..10 {
            nf.classify(-65.0, F100); // floor ~-65 at rest
        }
        // At rest a -60 frame (+5 over floor) is BELOW the strict onset gate
        // (floor+8 ≈ -57) → not voiced.
        assert!(!nf.classify(-60.0, F100), "onset gate is strict");
        // A -55 frame (+10) clears onset and enters speech.
        assert!(nf.classify(-55.0, F100), "-55 clears onset");
        // Now a -60 frame stays voiced — the sustain gate (floor+4) is looser.
        assert!(nf.classify(-60.0, F100), "sustain gate is loose");
    }

    #[test]
    fn hangover_bridges_dips_then_closes() {
        let mut nf = NoiseFloor::new(8.0, 16_000);
        for _ in 0..10 {
            nf.classify(-65.0, F100);
        }
        nf.classify(-55.0, F100); // enter speech → 400 ms hangover
        // A deep dip well below the sustain gate stays voiced for <400 ms.
        for ms in 1..=4 {
            assert!(nf.classify(-90.0, F100), "dip at {}00ms within hangover", ms);
        }
        // 5th consecutive 100 ms silence exceeds the 400 ms tail → gate closes.
        assert!(!nf.classify(-90.0, F100), "gate closes after 400ms continuous silence");
    }

    #[test]
    fn noise_floor_converges_only_after_seeing_silence() {
        let mut nf = NoiseFloor::new(8.0, 16_000);
        assert!(!nf.converged(), "fresh floor is unconverged");
        // Stable calibration window (real room tone) arms the floor at ~-60.
        for _ in 0..6 {
            nf.classify(-60.0, F100);
        }
        assert!(!nf.converged(), "arming alone does not converge the floor");
        // Enough genuine-silence frames past calibration converge it (-60 sits
        // below the onset floor+8, so each is a genuine-silence observation).
        for _ in 0..(FLOOR_CONVERGE_FRAMES + 5) {
            nf.classify(-60.0, F100);
        }
        assert!(nf.converged(), "floor converges once it has seen silence");
    }

    #[test]
    fn startup_calibration_seeds_floor_from_first_500ms() {
        // Round-7: with raised input gain the room tone sits at -40 dBFS, above
        // the old -53 init cap. Calibration must seat the floor at the ACTUAL
        // level, not a fixed cap — five 100 ms frames fill the 500 ms window.
        let mut nf = NoiseFloor::new(8.0, 16_000);
        for _ in 0..5 {
            assert!(!nf.classify(-40.0, F100), "calibration window reads non-speech");
        }
        assert!(
            (nf.floor_dbfs() - (-40.0)).abs() < 1.0,
            "floor seeded to room tone, got {}",
            nf.floor_dbfs()
        );
    }

    #[test]
    fn calibration_seeds_from_quiet_end_when_window_contaminated() {
        // User talking/moving through calibration: the window stays unstable, so
        // it EXTENDS to the cap rather than arming on the noisy seed, then seeds
        // from the quiet-end p20 (room tone), never the speech-level median.
        let mut nf = NoiseFloor::new(4.0, 16_000);
        for i in 0..30 {
            let d = if i % 5 == 4 { -56.0 } else { -28.0 };
            assert!(!nf.classify(d, F100), "calibration window is non-voiced");
        }
        let floor = nf.floor_dbfs();
        assert!(
            (floor - (-56.0)).abs() < 4.0,
            "contaminated calibration must seed near the quiet floor, got {floor}"
        );
    }

    #[test]
    fn calibration_ignores_startup_dead_frames() {
        // Wall 3: cpal emits ~digital silence before the device spins up. Those
        // dead frames must NOT seed the floor — else p20 latches at ~-97 dBFS and
        // room tone reads as permanent speech (deaf). Seed from real room tone.
        let mut nf = NoiseFloor::new(4.0, 16_000);
        for _ in 0..4 {
            assert!(!nf.classify(-100.0, F100), "dead frames ignored, non-voiced");
        }
        for _ in 0..6 {
            nf.classify(-40.0, F100); // real room tone
        }
        let floor = nf.floor_dbfs();
        assert!(
            (floor - (-40.0)).abs() < 3.0,
            "floor must seed from real room tone, not startup zeros, got {floor}"
        );
        // Room tone now reads SILENCE (onset -36), not perpetual speech.
        assert!(!nf.classify(-40.0, F100), "room tone below onset after correct seed");
    }

    #[test]
    fn seeded_floor_is_clamped_to_physical_minimum() {
        // Even a window of genuinely-quiet-but-not-dead frames can't SEED the
        // onset absurdly low: the seed clamps at FLOOR_MIN_DBFS. (Post-arm the
        // floor may still learn down toward genuine quiet — the clamp guards the
        // seed, not the tracker.) Feed exactly the window so we read the seed.
        let mut nf = NoiseFloor::new(4.0, 16_000);
        for _ in 0..5 {
            nf.classify(-78.0, F100); // above the dead cutoff, below the clamp
        }
        assert!(
            (nf.floor_dbfs() - FLOOR_MIN_DBFS).abs() < 0.01,
            "seed clamped to physical minimum, got {}",
            nf.floor_dbfs()
        );
    }

    #[test]
    fn room_dynamics_stay_unvoiced_only_speech_voices() {
        // Wall 5: the floor tracks the quiet-p10 (~-50) but room tone swings ~7 dB
        // up to its peaks (~-43/-46). The onset margin must sit ABOVE those peaks
        // so the room never self-triggers — else the gate reads voiced forever
        // and the endpointer never sees the silence it needs to finalize a turn.
        let mut nf = NoiseFloor::new(10.0, 16_000); // production margin
        for _ in 0..6 {
            nf.classify(-50.0, F100); // calibrate at the quiet room level
        }
        // Room fluctuates around -50 up to -46 peaks — must stay UNVOICED.
        let mut any_voiced = false;
        for i in 0..40 {
            let d = if i % 2 == 0 { -46.0 } else { -54.0 };
            if nf.classify(d, F100) {
                any_voiced = true;
            }
        }
        assert!(!any_voiced, "room dynamics must not cross the onset gate");
        // Real speech clears easily.
        assert!(nf.classify(-25.0, F100), "speech clears the gate");
        // After speech, room tone returns → the gate must CLOSE (past the 400 ms
        // hangover) so the endpoint can fire and the turn finalizes.
        let mut closed = false;
        for _ in 0..10 {
            if !nf.classify(-50.0, F100) {
                closed = true;
            }
        }
        assert!(closed, "gate closes on room tone after speech so the endpoint fires");
    }

    #[test]
    fn gate_reference_tracks_typical_not_quiet_so_room_peaks_dont_sustain() {
        // Wall-5 caveat (fix #2): the quiet floor (fast-down) drifts BELOW the
        // room's typical level, which would drop the sustain gate under the room's
        // peaks and hold a finished utterance open until the watchdog. The gate
        // reference tracks the TYPICAL ambient instead, so room peaks stay below
        // sustain and the gate closes. RED under the old floor-based gate.
        let mut nf = NoiseFloor::new(10.0, 16_000);
        for _ in 0..6 {
            nf.classify(-47.0, F100); // calibrate at the typical room level
        }
        // Mostly-typical room with occasional quiet dips pulls the fast-down floor
        // below typical; the gate reference should hold near -47.
        for i in 0..60 {
            let d = if i % 6 == 0 { -55.0 } else { -47.0 };
            nf.classify(d, F100);
        }
        let quiet = nf.floor_dbfs();
        let gate = nf.gate_floor_dbfs();
        assert!(
            quiet < gate - 2.0,
            "quiet floor must drift below the gate ref: quiet {quiet} gate {gate}"
        );
        assert!((gate - (-47.0)).abs() < 3.0, "gate ref stays near typical, got {gate}");
        // Speech opens the gate; a trailing room PEAK (-44, above the quiet floor's
        // sustain but below the typical-referenced sustain) must CLOSE it.
        assert!(nf.classify(-20.0, F100), "speech opens the gate");
        let mut closed = false;
        for _ in 0..10 {
            if !nf.classify(-44.0, F100) {
                closed = true;
            }
        }
        assert!(closed, "room peak below the typical-referenced sustain closes the gate");
    }

    #[test]
    fn room_held_gate_closes_within_seconds_via_learning_not_watchdog() {
        // fix#2-regression (Wall-5 time constant): when the room HOLDS the gate
        // open via sustain, the gate reference must keep learning UP (below-onset
        // ambient, not speech) so sustain rises above the room and the gate CLOSES
        // within a couple seconds — NOT frozen until the 15 s watchdog (which
        // merged two say sentences across a 7 s gap into one turn). RED under the
        // learn-only-in-genuine-silence behaviour.
        let mut nf = NoiseFloor::new(10.0, 16_000);
        for _ in 0..6 {
            nf.classify(-50.0, F100); // calibrate LOW (quiet p10 -50)
        }
        assert!((nf.gate_floor_dbfs() - (-50.0)).abs() < 1.0, "gate ref seeds at the quiet floor");
        assert!(nf.classify(-20.0, F100), "speech opens the gate");
        // The room at its TYPICAL -42 now HOLDS the gate via sustain (gate_floor+5
        // = -45; -42 > -45). The reference must rise and the gate close well before
        // the 15 s watchdog.
        let mut closed_at = None;
        for i in 0..40 {
            if !nf.classify(-42.0, F100) {
                closed_at = Some(i);
                break;
            }
        }
        let closed = closed_at.expect("a room-held gate must close via learning within the window");
        assert!(closed < 30, "gate closed in {closed} frames (~{}s), not the 15 s watchdog", closed / 10);
        assert_eq!(nf.recalibrations(), 0, "closed via the gate ref rising, not the watchdog");
    }

    #[test]
    fn watchdog_fires_from_a_learned_down_low_floor() {
        // Wall-3 safety net: the seed now clamps at -65, but the tracker can still
        // LEARN the floor far down over genuine near-silence. If room tone then
        // rises above the (now too-low) onset, the gate wedges open — the watchdog
        // MUST recover it within 15 s. This is the forced-low-floor test.
        let mut nf = NoiseFloor::new(4.0, 16_000);
        for _ in 0..6 {
            nf.classify(-45.0, F100); // arm at a normal floor
        }
        for _ in 0..40 {
            nf.classify(-95.0, F100); // room goes near-silent → floor learns down
        }
        assert!(nf.floor_dbfs() < -80.0, "floor learned down, got {}", nf.floor_dbfs());
        // Gain/room jumps to -39, far above the now-stale onset → stuck open.
        for _ in 0..170 {
            nf.classify(-39.0, F100); // 17 s > the 15 s trip
        }
        assert!(
            nf.recalibrations() >= 1,
            "watchdog must fire from a learned-down low floor"
        );
        assert!(
            nf.floor_dbfs() > -50.0,
            "recalibrated toward the room tone, got {}",
            nf.floor_dbfs()
        );
    }

    #[test]
    fn calibration_window_reports_non_voiced_even_if_loud() {
        // The first 500 ms are assumed non-speech regardless of level, so the
        // floor seed is never poisoned by a session that starts mid-utterance.
        let mut nf = NoiseFloor::new(8.0, 16_000);
        for i in 0..5 {
            assert!(!nf.classify(-10.0, F100), "frame {i} in calib window is non-voiced");
        }
    }

    #[test]
    fn high_gain_room_reads_silence_not_perpetual_speech() {
        // The round-7 blocker: at raised gain, -40 dBFS room tone under the
        // pre-calibration gate (init cap -53, onset -45) read as PERPETUAL
        // speech and the endpointer never fired → zero renders. Calibration
        // seats the floor at -40; continued room tone must now read SILENCE and
        // real speech must still voice.
        let mut nf = NoiseFloor::new(8.0, 16_000);
        for _ in 0..5 {
            nf.classify(-40.0, F100); // calibration
        }
        let mut voiced = 0;
        for _ in 0..20 {
            if nf.classify(-40.0, F100) {
                voiced += 1;
            }
        }
        assert_eq!(voiced, 0, "high-gain room tone must not read as speech");
        assert!(
            nf.classify(-25.0, F100),
            "speech above the calibrated floor still voices"
        );
    }

    #[test]
    fn stuck_open_watchdog_recovers_within_15s() {
        // Inverse failure: a mid-session input-gain jump lifts room tone above
        // the FROZEN floor+margin, wedging the gate OPEN — no genuine-silence
        // frame ever arrives, so only the watchdog can recover it. Calibrate to
        // a quiet floor, then feed continuous -40 (jumped room tone) past the
        // 15 s trip point; the watchdog must recalibrate to the quiet p10 and
        // the gate must read silence again so the endpointer can finalize.
        let mut nf = NoiseFloor::new(8.0, 16_000);
        for _ in 0..5 {
            nf.classify(-65.0, F100); // calibrate to a quiet -65 floor
        }
        // 15 s = 150 frames of 100 ms; the watchdog trips on the 151st voiced
        // frame. Run a little past it and confirm recovery.
        let mut last = true;
        for _ in 0..160 {
            last = nf.classify(-40.0, F100);
        }
        assert!(
            nf.recalibrations() >= 1,
            "watchdog must fire on 15 s+ continuous voicing"
        );
        assert!(!last, "gate recovers to silence after watchdog recalibration");
        assert!(
            (nf.floor_dbfs() - (-40.0)).abs() < 2.0,
            "floor recalibrated to the room tone, got {}",
            nf.floor_dbfs()
        );
    }

    #[test]
    fn watchdog_does_not_defeat_the_freeze_on_normal_turns() {
        // Discriminator: sustained level WITH endpoint-firing turns (speech +
        // micro-pauses) must NOT trip the watchdog — only never-ending voicing
        // does. Ten 1.5 s utterances separated by silence: zero recalibrations.
        let mut nf = NoiseFloor::new(8.0, 16_000);
        for _ in 0..15 {
            nf.classify(-65.0, F100);
        }
        for _ in 0..10 {
            for _ in 0..15 {
                nf.classify(-55.0, F100); // 1.5 s speech
            }
            for _ in 0..8 {
                nf.classify(-65.0, F100); // 800 ms silence — resets voiced run
            }
        }
        assert_eq!(nf.recalibrations(), 0, "normal turns must not trip the watchdog");
    }
}
