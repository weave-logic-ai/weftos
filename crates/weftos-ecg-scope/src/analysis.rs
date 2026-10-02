//! Calibration measurements computed from the merged signal: baseline offset, rail headroom,
//! mains hum at 50 and 60 Hz, R-wave amplitude and polarity, noise floor and SNR, plus a
//! tap-along pulse counter to cross-check the cog's heart rate.

use crate::model::Sample;
use std::f64::consts::PI;
use web_time::Instant;

/// AD8232 output idles near mid-supply (about 1.65 V at 3.3 V).
pub const BASELINE_OK: (f64, f64) = (1.0, 2.3);
pub const RAIL_MARGIN_V: f64 = 0.05;
pub const SUPPLY_V: f64 = 3.3;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Calibration {
    pub n: usize,
    pub baseline_v: Option<f64>,
    pub rail_fraction: f64,
    pub hum_50_mv: Option<f64>,
    pub hum_60_mv: Option<f64>,
    pub r_amplitude_mv: Option<f64>,
    pub noise_mv: Option<f64>,
    pub snr_db: Option<f64>,
}

impl Calibration {
    pub fn baseline_ok(&self) -> Option<bool> {
        self.baseline_v
            .map(|b| (BASELINE_OK.0..=BASELINE_OK.1).contains(&b))
    }

    /// True when the R-waves point down: RA and LA are swapped.
    pub fn inverted(&self) -> Option<bool> {
        self.r_amplitude_mv.map(|a| a < 0.0)
    }

    /// Which notch the hum says to use: Some(50|60), or None when hum is negligible.
    pub fn recommended_mains_hz(&self) -> Option<u32> {
        let (h50, h60) = (self.hum_50_mv?, self.hum_60_mv?);
        if h50.max(h60) < 0.02 {
            return None;
        }
        Some(if h60 >= h50 { 60 } else { 50 })
    }
}

/// RMS amplitude (in the samples' unit) of the `freq` component, by Goertzel over `x`.
pub fn tone_rms(x: &[f64], fs: f64, freq: f64) -> Option<f64> {
    if x.len() < 16 || fs <= 2.0 * freq {
        return None;
    }
    let mean = x.iter().sum::<f64>() / x.len() as f64;
    let w = 2.0 * PI * freq / fs;
    let coeff = 2.0 * w.cos();
    let (mut s1, mut s2) = (0.0_f64, 0.0_f64);
    for v in x {
        let s0 = (v - mean) + coeff * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    let power = s1 * s1 + s2 * s2 - coeff * s1 * s2;
    let amplitude = 2.0 * power.max(0.0).sqrt() / x.len() as f64;
    Some(amplitude / std::f64::consts::SQRT_2)
}

fn median(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    let m = v.len() / 2;
    Some(if v.len().is_multiple_of(2) {
        (v[m - 1] + v[m]) / 2.0
    } else {
        v[m]
    })
}

/// Measures the window. `peaks_ms` are the cog's R-peak times inside or around it.
pub fn calibrate(window: &[Sample], peaks_ms: &[u64], fs: f64) -> Calibration {
    let n = window.len();
    if n == 0 {
        return Calibration::default();
    }
    let raw: Vec<f64> = window.iter().map(|s| s.raw_v).collect();
    let baseline = raw.iter().sum::<f64>() / n as f64;
    let rail = raw
        .iter()
        .filter(|&&v| !(RAIL_MARGIN_V..=SUPPLY_V - RAIL_MARGIN_V).contains(&v))
        .count() as f64
        / n as f64;
    // Hum is measured on the raw signal (before the cog's notch) in mV.
    let raw_mv: Vec<f64> = raw.iter().map(|v| v * 1000.0).collect();
    let hum_50 = tone_rms(&raw_mv, fs, 50.0);
    let hum_60 = tone_rms(&raw_mv, fs, 60.0);

    // R amplitude: the filtered extreme within +/-40 ms of each peak. The cog marks R on its
    // band-passed signal, which lags the display waveform by a few samples.
    let at_peaks: Vec<f64> = peaks_ms
        .iter()
        .filter_map(|&p| snap_peak(window, p).map(|s| s.filtered_mv))
        .collect();
    let r_amp = median(at_peaks);

    // Noise floor away from QRS complexes (+/-120 ms): RMS of the first difference / sqrt(2).
    // Differencing ignores the slow P and T waves and baseline, so this measures wide-band
    // noise (EMG, electrode, ADC), not the ECG itself.
    let mut diffs = Vec::new();
    for w in window.windows(2) {
        let quiet = |s: &Sample| peaks_ms.iter().all(|&p| s.t_ms.abs_diff(p) > 120);
        if quiet(&w[0]) && quiet(&w[1]) {
            diffs.push(w[1].filtered_mv - w[0].filtered_mv);
        }
    }
    let noise = if diffs.len() >= 16 {
        let m = diffs.iter().sum::<f64>() / diffs.len() as f64;
        let var = diffs.iter().map(|d| (d - m).powi(2)).sum::<f64>() / diffs.len() as f64;
        Some((var / 2.0).sqrt())
    } else {
        None
    };
    let snr = match (r_amp, noise) {
        (Some(a), Some(nz)) if nz > 1e-9 && a.abs() > 0.0 => Some(20.0 * (a.abs() / nz).log10()),
        _ => None,
    };
    Calibration {
        n,
        baseline_v: Some(baseline),
        rail_fraction: rail,
        hum_50_mv: hum_50,
        hum_60_mv: hum_60,
        r_amplitude_mv: r_amp,
        noise_mv: noise,
        snr_db: snr,
    }
}

/// The sample with the largest |filtered| within +/-40 ms of a peak time.
pub fn snap_peak(window: &[Sample], peak_ms: u64) -> Option<&Sample> {
    window
        .iter()
        .filter(|s| s.t_ms.abs_diff(peak_ms) <= 40)
        .max_by(|a, b| a.filtered_mv.abs().total_cmp(&b.filtered_mv.abs()))
}

/// Tap along with the subject's pulse; compares against the cog's heart rate.
#[derive(Default)]
pub struct TapTempo {
    taps: Vec<Instant>,
}

impl TapTempo {
    pub fn tap(&mut self, at: Instant) {
        // A pause over 3 s starts a new count.
        if self
            .taps
            .last()
            .is_some_and(|l| at.duration_since(*l).as_secs_f64() > 3.0)
        {
            self.taps.clear();
        }
        self.taps.push(at);
        if self.taps.len() > 16 {
            self.taps.remove(0);
        }
    }

    pub fn reset(&mut self) {
        self.taps.clear();
    }

    pub fn count(&self) -> usize {
        self.taps.len()
    }

    /// BPM from the median tap interval; needs 4+ taps.
    pub fn bpm(&self) -> Option<f64> {
        if self.taps.len() < 4 {
            return None;
        }
        let iv: Vec<f64> = self
            .taps
            .windows(2)
            .map(|w| w[1].duration_since(w[0]).as_secs_f64())
            .collect();
        median(iv).filter(|m| *m > 0.0).map(|m| 60.0 / m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use web_time::Duration;

    fn synth(
        fs: f64,
        secs: f64,
        hum_hz: f64,
        hum_mv: f64,
        invert: bool,
    ) -> (Vec<Sample>, Vec<u64>) {
        let n = (fs * secs) as usize;
        let mut samples = Vec::with_capacity(n);
        let mut peaks = Vec::new();
        for i in 0..n {
            let t = i as f64 / fs;
            let t_ms = (t * 1000.0).round() as u64;
            let ph = t % 1.0;
            let r = (-((ph - 0.3) * (ph - 0.3)) / (2.0 * 0.01f64.powi(2))).exp();
            let ecg_mv = if invert { -r } else { r } * 1000.0;
            let hum = hum_mv * (2.0 * PI * hum_hz * t).sin();
            // 5 mV of 13 Hz "muscle noise" gives the noise floor something to measure.
            let noise = 5.0 * (2.0 * PI * 13.0 * t).sin();
            samples.push(Sample {
                t_ms,
                raw_v: 1.65 + (ecg_mv * 0.5 + hum) / 1000.0,
                filtered_mv: ecg_mv * 0.5 + noise,
            });
            if (ph - 0.3).abs() < 0.5 / fs {
                peaks.push(t_ms);
            }
        }
        (samples, peaks)
    }

    #[test]
    fn goertzel_measures_a_known_tone() {
        let fs = 250.0;
        let x: Vec<f64> = (0..1000)
            .map(|i| 10.0 * (2.0 * PI * 60.0 * i as f64 / fs).sin())
            .collect();
        let rms = tone_rms(&x, fs, 60.0).unwrap();
        assert!((rms - 10.0 / 2f64.sqrt()).abs() < 0.1, "{rms}");
        assert!(tone_rms(&x, fs, 50.0).unwrap() < 0.5);
        assert!(tone_rms(&x, 100.0, 60.0).is_none(), "above Nyquist");
    }

    #[test]
    fn calibration_finds_baseline_hum_amplitude_and_polarity() {
        let (w, p) = synth(250.0, 8.0, 60.0, 20.0, false);
        let c = calibrate(&w, &p, 250.0);
        assert!((c.baseline_v.unwrap() - 1.66).abs() < 0.05);
        assert_eq!(c.baseline_ok(), Some(true));
        assert_eq!(c.recommended_mains_hz(), Some(60));
        assert!(c.r_amplitude_mv.unwrap() > 400.0);
        assert_eq!(c.inverted(), Some(false));
        assert!(c.snr_db.unwrap() > 20.0);
        assert_eq!(c.rail_fraction, 0.0);

        let (w, p) = synth(250.0, 8.0, 50.0, 20.0, true);
        let c = calibrate(&w, &p, 250.0);
        assert_eq!(c.recommended_mains_hz(), Some(50));
        assert_eq!(c.inverted(), Some(true));
    }

    #[test]
    fn rails_and_empty_windows_are_reported() {
        let w: Vec<Sample> = (0..100)
            .map(|i| Sample {
                t_ms: i * 4,
                raw_v: 3.3,
                filtered_mv: 0.0,
            })
            .collect();
        let c = calibrate(&w, &[], 250.0);
        assert_eq!(c.rail_fraction, 1.0);
        assert_eq!(c.baseline_ok(), Some(false));
        assert_eq!(calibrate(&[], &[], 250.0), Calibration::default());
    }

    #[test]
    fn tap_tempo_reports_bpm_and_restarts_after_a_pause() {
        let t0 = Instant::now();
        let mut t = TapTempo::default();
        for i in 0..6 {
            t.tap(t0 + Duration::from_millis(i * 800));
        }
        assert!((t.bpm().unwrap() - 75.0).abs() < 0.01);
        t.tap(t0 + Duration::from_secs(20));
        assert_eq!(t.count(), 1);
        assert!(t.bpm().is_none());
    }
}
