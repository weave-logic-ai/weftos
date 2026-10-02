//! The ECG sensor's hook-up steps, after the shared connection steps (weftos-cog-companion).
//! Each is judged from the cog's report and the app's own calibration, in wiring order, with
//! the fix to try; the shell blocks later steps until earlier ones pass. Step ids match
//! `[links]` in the cog's guide.toml (ADR-104).

use crate::analysis::Calibration;
use serde_json::Value;
use weftos_cog_companion::{Mark, Step, step};

/// `configured_mains` is the cog's current `mains_hz`, when the agent reported it.
pub fn sensor_steps(
    report: Option<&Value>,
    source: &str,
    cal: &Calibration,
    configured_mains: Option<u32>,
) -> Vec<Step> {
    let status = report.and_then(|r| r["status"].as_str()).unwrap_or("");
    let f = |k: &str| report.and_then(|r| r[k].as_f64());
    let quality = f("quality").unwrap_or(0.0);
    vec![
        if status == "no_source" {
            let err = report.and_then(|r| r["error"].as_str()).unwrap_or("");
            step(
                "adc_found",
                "ADS1115 found on I2C",
                Mark::Fail,
                format!(
                    "{err}. Check 3V3 to pin 1, GND to pin 6, SDA to pin 3, SCL to pin 5, ADDR to GND (0x48)"
                ),
            )
        } else if report.is_some() {
            step(
                "adc_found",
                "ADS1115 found on I2C",
                Mark::Pass,
                source.to_string(),
            )
        } else {
            step(
                "adc_found",
                "ADS1115 found on I2C",
                Mark::Waiting,
                "no report yet",
            )
        },
        match (status, cal.baseline_ok()) {
            ("flat", _) => step(
                "sensor_powered",
                "Sensor powered",
                Mark::Fail,
                "signal is flat: SEN0213 + to 3.3 V, - to GND, A to ADS1115 A0",
            ),
            (_, Some(true)) => step(
                "sensor_powered",
                "Sensor powered",
                Mark::Pass,
                format!(
                    "baseline {:.2} V (AD8232 idles near 1.65 V)",
                    cal.baseline_v.unwrap_or(0.0)
                ),
            ),
            (_, Some(false)) => step(
                "sensor_powered",
                "Sensor powered",
                Mark::Warn,
                format!(
                    "baseline {:.2} V is off mid-supply: check the sensor's 3.3 V supply",
                    cal.baseline_v.unwrap_or(0.0)
                ),
            ),
            _ => step(
                "sensor_powered",
                "Sensor powered",
                Mark::Waiting,
                "collecting signal",
            ),
        },
        match status {
            "leads_off" => step(
                "electrodes",
                "Electrodes attached",
                Mark::Fail,
                format!(
                    "{:.0}% of samples at the rails: pads off or loose; press firmly on clean skin",
                    cal.rail_fraction * 100.0
                ),
            ),
            "ok" | "no_beats" => step(
                "electrodes",
                "Electrodes attached",
                Mark::Pass,
                "signal stays off the rails",
            ),
            _ => step(
                "electrodes",
                "Electrodes attached",
                Mark::Waiting,
                "needs a live signal",
            ),
        },
        match (status, f("heart_rate_bpm")) {
            ("ok", Some(hr)) if quality >= 0.8 => step(
                "beats",
                "Heartbeats detected",
                Mark::Pass,
                format!("{hr:.0} bpm, quality {quality:.2}"),
            ),
            ("ok", Some(hr)) => step(
                "beats",
                "Heartbeats detected",
                Mark::Warn,
                format!("{hr:.0} bpm but quality {quality:.2}: keep still, check pad contact"),
            ),
            ("no_beats", _) => step(
                "beats",
                "Heartbeats detected",
                Mark::Fail,
                "no R-peaks yet: wait 2 s for learning, keep still, move pads closer to the heart",
            ),
            _ => step(
                "beats",
                "Heartbeats detected",
                Mark::Waiting,
                "needs electrodes on",
            ),
        },
        match cal.inverted() {
            Some(true) => step(
                "polarity",
                "Lead polarity",
                Mark::Warn,
                "R-waves point down: swap the RA and LA pads",
            ),
            Some(false) => step(
                "polarity",
                "Lead polarity",
                Mark::Pass,
                format!(
                    "R-waves upright, {:.2} mV",
                    cal.r_amplitude_mv.unwrap_or(0.0)
                ),
            ),
            None => step(
                "polarity",
                "Lead polarity",
                Mark::Waiting,
                "needs detected beats",
            ),
        },
        match cal.recommended_mains_hz() {
            Some(hz) if configured_mains == Some(hz) => step(
                "mains",
                "Mains notch",
                Mark::Pass,
                format!(
                    "notch at {hz} Hz matches the hum ({:.3} mV at 50, {:.3} mV at 60, before the notch)",
                    cal.hum_50_mv.unwrap_or(0.0),
                    cal.hum_60_mv.unwrap_or(0.0)
                ),
            ),
            Some(hz) => step(
                "mains",
                "Mains notch",
                Mark::Warn,
                format!(
                    "hum strongest at {hz} Hz ({:.3} mV at 50, {:.3} mV at 60): set the notch to {hz} Hz",
                    cal.hum_50_mv.unwrap_or(0.0),
                    cal.hum_60_mv.unwrap_or(0.0)
                ),
            ),
            None if cal.hum_60_mv.is_some() => step(
                "mains",
                "Mains notch",
                Mark::Pass,
                "hum below 0.02 mV at 50 and 60 Hz",
            ),
            None => step("mains", "Mains notch", Mark::Waiting, "collecting signal"),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use weftos_cog_companion::widgets::chain;

    #[test]
    fn missing_adc_points_at_the_wiring_and_blocks_the_rest() {
        let r = serde_json::json!({"status":"no_source","error":"i2c write reg 0x01 at 0x48: Input/output error"});
        let steps = chain(sensor_steps(Some(&r), "", &Calibration::default(), None));
        assert_eq!(steps[0].mark, Mark::Fail);
        assert!(steps[0].detail.contains("SDA to pin 3"));
        assert!(steps[1..].iter().all(|s| s.mark == Mark::Waiting));
    }

    #[test]
    fn healthy_signal_passes_and_flags_polarity_and_hum() {
        let r = serde_json::json!({"status":"ok","heart_rate_bpm":71.0,"quality":0.95});
        let cal = Calibration {
            baseline_v: Some(1.6),
            r_amplitude_mv: Some(-0.8),
            hum_50_mv: Some(0.01),
            hum_60_mv: Some(0.3),
            ..Default::default()
        };
        let steps = sensor_steps(Some(&r), "ads1115", &cal, Some(50));
        assert!(steps[..4].iter().all(|s| s.mark == Mark::Pass), "{steps:?}");
        assert_eq!(steps[4].mark, Mark::Warn);
        assert!(steps[5].detail.contains("set the notch to 60 Hz"));
        assert_eq!(
            sensor_steps(Some(&r), "", &cal, Some(60))[5].mark,
            Mark::Pass
        );
    }

    #[test]
    fn every_step_has_a_guide_link_id() {
        let ids: Vec<&str> = sensor_steps(None, "", &Calibration::default(), None)
            .iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(
            ids,
            vec![
                "adc_found",
                "sensor_powered",
                "electrodes",
                "beats",
                "polarity",
                "mains"
            ]
        );
    }
}
