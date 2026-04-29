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
#[derive(Debug)]
pub struct EnergyVad {
    sample_rate: u32,
    threshold_dbfs: f32,
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
            silence_tail_samples: s * u64::from(silence_ms) / 1_000,
            min_utterance_samples: s * u64::from(min_utterance_ms) / 1_000,
            max_utterance_samples: s * u64::from(max_utterance_ms) / 1_000,
            cumulative: 0,
            in_speech: false,
            speech_start: 0,
            silence_run: 0,
            speech_samples: 0,
        }
    }

    /// Sample rate the VAD was constructed with.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
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
                events.push(VadEvent::SpeechStart {
                    at_sample: self.speech_start,
                });
            }
        } else {
            if is_speech {
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
                }
                self.in_speech = false;
                self.silence_run = 0;
                self.speech_samples = 0;
            }
        }

        self.cumulative = self.cumulative.saturating_add(frame_len);
        events
    }
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
}
