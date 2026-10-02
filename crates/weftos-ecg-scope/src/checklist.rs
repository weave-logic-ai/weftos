//! The hook-up checklist: each step is judged from live state, in wiring order, with the fix
//! to try when it fails. Later steps stay "waiting" until the earlier ones pass.

use crate::analysis::Calibration;
use crate::model::{Probe, State};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    Pass,
    Warn,
    Fail,
    Waiting,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub title: &'static str,
    pub mark: Mark,
    pub detail: String,
}

impl Step {
    /// Stable id for linking a step to a cog guide page (`[links]` in guide.toml, ADR-104).
    pub fn id(&self) -> &'static str {
        match self.title {
            "Seed agent reachable" => "seed_api",
            "Cog installed" => "cog_installed",
            "Cog running" => "cog_running",
            "Signal export reachable" => "export",
            "ADS1115 found on I2C" => "adc_found",
            "Sensor powered" => "sensor_powered",
            "Electrodes attached" => "electrodes",
            "Heartbeats detected" => "beats",
            "Lead polarity" => "polarity",
            "Mains notch" => "mains",
            _ => "",
        }
    }
}

fn step(title: &'static str, mark: Mark, detail: impl Into<String>) -> Step {
    Step {
        title,
        mark,
        detail: detail.into(),
    }
}

/// `configured_mains` is the cog's current `mains_hz`, when the agent reported it.
pub fn evaluate(st: &State, cal: &Calibration, configured_mains: Option<u32>) -> Vec<Step> {
    let mut out = Vec::new();
    let mut blocked = false;
    let mut push = |s: Step, out: &mut Vec<Step>| {
        let s = if blocked {
            Step {
                mark: Mark::Waiting,
                detail: "waiting for the step above".into(),
                ..s
            }
        } else {
            s
        };
        if s.mark == Mark::Fail {
            blocked = true;
        }
        out.push(s);
    };

    push(
        match &st.seed_api {
            Probe::Ok => step("Seed agent reachable", Mark::Pass, "agent app API answers"),
            Probe::Failed(e) => step(
                "Seed agent reachable",
                Mark::Fail,
                format!("{e}. USB: http://169.254.42.1; tailnet: https://<ip>:8443"),
            ),
            Probe::Unknown => step("Seed agent reachable", Mark::Waiting, "probing"),
        },
        &mut out,
    );
    push(
        match st.cog_installed {
            Some(true) => step(
                "Cog installed",
                Mark::Pass,
                "sen0213-ecg is in /api/v1/apps",
            ),
            Some(false) => step(
                "Cog installed",
                Mark::Fail,
                "sideload it: scripts/seed-sideload.sh sen0213-ecg <seed>",
            ),
            None => step("Cog installed", Mark::Waiting, "needs the agent"),
        },
        &mut out,
    );
    push(
        match st.cog_running {
            Some(true) => step(
                "Cog running",
                Mark::Pass,
                "continuous mode, exporting the last 60 s",
            ),
            _ => step(
                "Cog running",
                Mark::Fail,
                "press Start cog (continuous mode keeps the export up)",
            ),
        },
        &mut out,
    );
    push(
        match &st.export {
            Probe::Ok => step(
                "Signal export reachable",
                Mark::Pass,
                "polling /raw and /status",
            ),
            Probe::Failed(e) => step(
                "Signal export reachable",
                Mark::Fail,
                format!("{e}. Start the cog; it serves :8046 on all interfaces from 0.1.1"),
            ),
            Probe::Unknown => step("Signal export reachable", Mark::Waiting, "probing"),
        },
        &mut out,
    );
    let status = st.report_str("status").unwrap_or("");
    push(
        if status == "no_source" {
            let err = st
                .report
                .as_ref()
                .and_then(|r| r["error"].as_str())
                .unwrap_or("");
            step(
                "ADS1115 found on I2C",
                Mark::Fail,
                format!(
                    "{err}. Check 3V3 to pin 1, GND to pin 6, SDA to pin 3, SCL to pin 5, ADDR to GND (0x48)"
                ),
            )
        } else if st.report.is_some() {
            step("ADS1115 found on I2C", Mark::Pass, st.signal.source.clone())
        } else {
            step("ADS1115 found on I2C", Mark::Waiting, "no report yet")
        },
        &mut out,
    );
    push(
        match (status, cal.baseline_ok()) {
            ("flat", _) => step(
                "Sensor powered",
                Mark::Fail,
                "signal is flat: SEN0213 + to 3.3 V, - to GND, A to ADS1115 A0",
            ),
            (_, Some(true)) => step(
                "Sensor powered",
                Mark::Pass,
                format!(
                    "baseline {:.2} V (AD8232 idles near 1.65 V)",
                    cal.baseline_v.unwrap_or(0.0)
                ),
            ),
            (_, Some(false)) => step(
                "Sensor powered",
                Mark::Warn,
                format!(
                    "baseline {:.2} V is off mid-supply: check the sensor's 3.3 V supply",
                    cal.baseline_v.unwrap_or(0.0)
                ),
            ),
            _ => step("Sensor powered", Mark::Waiting, "collecting signal"),
        },
        &mut out,
    );
    push(
        match status {
            "leads_off" => step(
                "Electrodes attached",
                Mark::Fail,
                format!(
                    "{:.0}% of samples at the rails: pads off or loose; press firmly on clean skin",
                    cal.rail_fraction * 100.0
                ),
            ),
            "ok" | "no_beats" => step(
                "Electrodes attached",
                Mark::Pass,
                "signal stays off the rails",
            ),
            _ => step("Electrodes attached", Mark::Waiting, "needs a live signal"),
        },
        &mut out,
    );
    let quality = st.report_f64("quality").unwrap_or(0.0);
    push(
        match (status, st.report_f64("heart_rate_bpm")) {
            ("ok", Some(hr)) if quality >= 0.8 => step(
                "Heartbeats detected",
                Mark::Pass,
                format!("{hr:.0} bpm, quality {quality:.2}"),
            ),
            ("ok", Some(hr)) => step(
                "Heartbeats detected",
                Mark::Warn,
                format!("{hr:.0} bpm but quality {quality:.2}: keep still, check pad contact"),
            ),
            ("no_beats", _) => step(
                "Heartbeats detected",
                Mark::Fail,
                "no R-peaks yet: wait 2 s for learning, keep still, move pads closer to the heart",
            ),
            _ => step("Heartbeats detected", Mark::Waiting, "needs electrodes on"),
        },
        &mut out,
    );
    push(
        match cal.inverted() {
            Some(true) => step(
                "Lead polarity",
                Mark::Warn,
                "R-waves point down: swap the RA and LA pads",
            ),
            Some(false) => step(
                "Lead polarity",
                Mark::Pass,
                format!(
                    "R-waves upright, {:.2} mV",
                    cal.r_amplitude_mv.unwrap_or(0.0)
                ),
            ),
            None => step("Lead polarity", Mark::Waiting, "needs detected beats"),
        },
        &mut out,
    );
    push(
        match cal.recommended_mains_hz() {
            Some(hz) if configured_mains == Some(hz) => step(
                "Mains notch",
                Mark::Pass,
                format!(
                    "notch at {hz} Hz matches the hum ({:.3} mV at 50, {:.3} mV at 60, before the notch)",
                    cal.hum_50_mv.unwrap_or(0.0),
                    cal.hum_60_mv.unwrap_or(0.0)
                ),
            ),
            Some(hz) => step(
                "Mains notch",
                Mark::Warn,
                format!(
                    "hum strongest at {hz} Hz ({:.3} mV at 50, {:.3} mV at 60): set the notch to {hz} Hz (Cog settings)",
                    cal.hum_50_mv.unwrap_or(0.0),
                    cal.hum_60_mv.unwrap_or(0.0)
                ),
            ),
            None if cal.hum_60_mv.is_some() => step(
                "Mains notch",
                Mark::Pass,
                "hum below 0.02 mV at 50 and 60 Hz",
            ),
            None => step("Mains notch", Mark::Waiting, "collecting signal"),
        },
        &mut out,
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marks(v: &[Step]) -> Vec<Mark> {
        v.iter().map(|s| s.mark).collect()
    }

    #[test]
    fn every_step_has_a_guide_link_id() {
        let steps = evaluate(&State::default(), &Calibration::default(), None);
        assert_eq!(steps.len(), 10);
        assert!(steps.iter().all(|s| !s.id().is_empty()), "{steps:?}");
    }

    #[test]
    fn unreachable_seed_blocks_every_later_step() {
        let st = State {
            seed_api: Probe::Failed("refused".into()),
            ..Default::default()
        };
        let steps = evaluate(&st, &Calibration::default(), None);
        assert_eq!(steps[0].mark, Mark::Fail);
        assert!(steps[1..].iter().all(|s| s.mark == Mark::Waiting));
    }

    #[test]
    fn missing_adc_points_at_the_wiring() {
        let st = State {
            seed_api: Probe::Ok,
            cog_installed: Some(true),
            cog_running: Some(true),
            export: Probe::Ok,
            report: Some(
                serde_json::json!({"status":"no_source","error":"i2c write reg 0x01 at 0x48: Input/output error"}),
            ),
            ..Default::default()
        };
        let steps = evaluate(&st, &Calibration::default(), None);
        assert_eq!(
            &marks(&steps)[..5],
            &[Mark::Pass, Mark::Pass, Mark::Pass, Mark::Pass, Mark::Fail]
        );
        assert!(steps[4].detail.contains("SDA to pin 3"));
    }

    #[test]
    fn healthy_signal_passes_and_flags_polarity_and_hum() {
        let st = State {
            seed_api: Probe::Ok,
            cog_installed: Some(true),
            cog_running: Some(true),
            export: Probe::Ok,
            report: Some(serde_json::json!({"status":"ok","heart_rate_bpm":71.0,"quality":0.95})),
            ..Default::default()
        };
        let cal = Calibration {
            baseline_v: Some(1.6),
            r_amplitude_mv: Some(-0.8),
            hum_50_mv: Some(0.01),
            hum_60_mv: Some(0.3),
            ..Default::default()
        };
        let steps = evaluate(&st, &cal, Some(50));
        assert!(steps[..8].iter().all(|s| s.mark == Mark::Pass), "{steps:?}");
        assert_eq!(steps[8].mark, Mark::Warn);
        assert!(steps[9].detail.contains("set the notch to 60 Hz"));
        assert_eq!(evaluate(&st, &cal, Some(60))[9].mark, Mark::Pass);
    }
}
