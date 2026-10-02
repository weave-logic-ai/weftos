//! Depth frames merged from the cog's `/frames?seconds=N` polls, plus the analysis the hook-up
//! and calibration tools need: a flat-wall levelling check, per-zone noise, and a colour map.

use std::collections::VecDeque;

pub const KEEP_MS: u64 = 30_000;
pub const MAX_MM: u16 = 3500;

#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub t_ms: u64,
    pub side: usize,
    pub mm: Vec<u16>,
}

#[derive(Default)]
pub struct Frames {
    pub frames: VecDeque<Frame>,
    pub source: String,
}

impl Frames {
    /// Appends frames newer than the last one held; a large step back in time (cog restart)
    /// starts over. Returns how many were added.
    pub fn merge(&mut self, incoming: Vec<Frame>) -> usize {
        let last = self.frames.back().map(|f| f.t_ms);
        let prev_side = self.frames.back().map(|f| f.side);
        let restart = incoming
            .first()
            .zip(last)
            .is_some_and(|(f, l)| f.t_ms + 5_000 < l || prev_side.is_some_and(|s| s != f.side));
        if restart {
            self.frames.clear();
        }
        let last = self.frames.back().map(|f| f.t_ms);
        let mut added = 0;
        for f in incoming
            .into_iter()
            .filter(|f| last.is_none_or(|l| f.t_ms > l))
        {
            self.frames.push_back(f);
            added += 1;
        }
        if let Some(newest) = self.frames.back().map(|f| f.t_ms) {
            while self
                .frames
                .front()
                .is_some_and(|f| f.t_ms + KEEP_MS < newest)
            {
                self.frames.pop_front();
            }
        }
        added
    }

    pub fn latest(&self) -> Option<&Frame> {
        self.frames.back()
    }

    pub fn last_n(&self, n: usize) -> Vec<&Frame> {
        let skip = self.frames.len().saturating_sub(n);
        self.frames.iter().skip(skip).collect()
    }

    pub fn to_csv(&self) -> String {
        let zones = self.frames.front().map_or(0, |f| f.mm.len());
        let mut out = String::from("t_ms,side");
        for z in 0..zones {
            out.push_str(&format!(",z{z}"));
        }
        out.push('\n');
        for f in &self.frames {
            out.push_str(&format!("{},{}", f.t_ms, f.side));
            for d in &f.mm {
                out.push_str(&format!(",{d}"));
            }
            out.push('\n');
        }
        out
    }
}

/// Parses `{"frames":[{"t_ms","side","mm":[..]}...], "source"}`.
pub fn parse_frames(v: &serde_json::Value) -> (Vec<Frame>, String) {
    let frames = v["frames"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|f| {
                    let side = f["side"].as_u64()? as usize;
                    let mm: Vec<u16> = f["mm"]
                        .as_array()?
                        .iter()
                        .filter_map(|d| d.as_u64().map(|x| x.min(u64::from(u16::MAX)) as u16))
                        .collect();
                    (side > 0 && mm.len() == side * side).then(|| Frame {
                        t_ms: f["t_ms"].as_u64().unwrap_or(0),
                        side,
                        mm,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    (frames, v["source"].as_str().unwrap_or("").to_string())
}

pub fn valid(d: u16) -> bool {
    (20..=MAX_MM).contains(&d)
}

/// Flat-wall levelling: with the sensor square-on to a flat wall every row and column average
/// should match. Positive `vertical_pct` = the top reads farther than the bottom (tilted up);
/// positive `horizontal_pct` = the right reads farther than the left.
#[derive(Clone, Debug, PartialEq)]
pub struct Level {
    pub mean_mm: f64,
    pub vertical_pct: f64,
    pub horizontal_pct: f64,
}

impl Level {
    /// Within +/-5 % both ways counts as level (corner zones naturally read a little long).
    pub fn is_level(&self) -> bool {
        self.vertical_pct.abs() <= 5.0 && self.horizontal_pct.abs() <= 5.0
    }
}

pub fn level(frame: &Frame) -> Option<Level> {
    let s = frame.side;
    let avg = |pick: &dyn Fn(usize, usize) -> bool| {
        let v: Vec<f64> = (0..s * s)
            .filter(|i| pick(i % s, i / s) && valid(frame.mm[*i]))
            .map(|i| f64::from(frame.mm[i]))
            .collect();
        (v.len() >= s / 2).then(|| v.iter().sum::<f64>() / v.len() as f64)
    };
    let top = avg(&|_, y| y == 0)?;
    let bottom = avg(&|_, y| y == s - 1)?;
    let left = avg(&|x, _| x == 0)?;
    let right = avg(&|x, _| x == s - 1)?;
    let mean = avg(&|_, _| true)?;
    Some(Level {
        mean_mm: mean,
        vertical_pct: 100.0 * (top - bottom) / mean,
        horizontal_pct: 100.0 * (right - left) / mean,
    })
}

/// Per-zone sample std (mm) over the given frames; None where fewer than 2 valid reads.
pub fn zone_noise(frames: &[&Frame]) -> Vec<Option<f64>> {
    let zones = frames.first().map_or(0, |f| f.mm.len());
    (0..zones)
        .map(|z| {
            let v: Vec<f64> = frames
                .iter()
                .filter_map(|f| f.mm.get(z).copied())
                .filter(|d| valid(*d))
                .map(f64::from)
                .collect();
            if v.len() < 2 {
                return None;
            }
            let m = v.iter().sum::<f64>() / v.len() as f64;
            Some((v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (v.len() - 1) as f64).sqrt())
        })
        .collect()
}

/// Index of the nearest valid zone.
pub fn nearest(mm: &[u16]) -> Option<usize> {
    mm.iter()
        .enumerate()
        .filter(|(_, d)| valid(**d))
        .min_by_key(|(_, d)| **d)
        .map(|(i, _)| i)
}

/// Zones closer than `background` by more than `margin` mm (none without a background).
pub fn occupied(mm: &[u16], background: Option<&[u16]>, margin: u16) -> Vec<usize> {
    let Some(bg) = background.filter(|b| b.len() == mm.len()) else {
        return Vec::new();
    };
    mm.iter()
        .zip(bg)
        .enumerate()
        .filter(|(_, (d, b))| valid(**d) && (!valid(**b) || b.saturating_sub(**d) > margin))
        .map(|(i, _)| i)
        .collect()
}

/// Near = warm, far = cool, on a 0..max_mm scale. Invalid zones are drawn separately.
pub fn color_for(mm: u16, max_mm: u16) -> [u8; 3] {
    let t = (f32::from(mm) / f32::from(max_mm.max(1))).clamp(0.0, 1.0);
    // red (near) -> yellow -> green -> blue (far)
    let stops = [
        [230.0, 60.0, 50.0],
        [240.0, 200.0, 50.0],
        [70.0, 190.0, 110.0],
        [50.0, 110.0, 220.0],
    ];
    let pos = t * 3.0;
    let i = (pos.floor() as usize).min(2);
    let f = pos - i as f32;
    let c = |k: usize| (stops[i][k] + (stops[i + 1][k] - stops[i][k]) * f) as u8;
    [c(0), c(1), c(2)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(t: u64, side: usize, mm: u16) -> Frame {
        Frame {
            t_ms: t,
            side,
            mm: vec![mm; side * side],
        }
    }

    #[test]
    fn merge_dedups_ages_out_and_resets_on_restart_or_mode_change() {
        let mut f = Frames::default();
        assert_eq!(f.merge(vec![frame(0, 8, 1000), frame(100, 8, 1000)]), 2);
        assert_eq!(f.merge(vec![frame(100, 8, 1000), frame(200, 8, 1000)]), 1);
        f.merge(vec![frame(40_000, 8, 1000)]);
        assert_eq!(f.frames.len(), 1);
        f.merge(vec![frame(40_100, 4, 1000)]);
        assert_eq!(f.latest().unwrap().side, 4, "mode change starts over");
        assert_eq!(f.frames.len(), 1);
    }

    #[test]
    fn parses_frames_and_rejects_wrong_sizes() {
        let v = serde_json::json!({"source":"sim","frames":[{"t_ms":1,"side":2,"mm":[1,2,3,4]},{"t_ms":2,"side":2,"mm":[1]}]});
        let (fr, src) = parse_frames(&v);
        assert_eq!(fr.len(), 1);
        assert_eq!(src, "sim");
    }

    #[test]
    fn level_is_flat_for_an_even_wall_and_signed_for_a_tilt() {
        let flat = frame(0, 8, 2000);
        let l = level(&flat).unwrap();
        assert!(l.is_level() && l.vertical_pct.abs() < 1e-9);
        let mut tilt = frame(0, 8, 2000);
        for x in 0..8 {
            tilt.mm[x] = 2300; // top row farther
        }
        let l = level(&tilt).unwrap();
        assert!(l.vertical_pct > 10.0 && !l.is_level());
        assert!(level(&frame(0, 8, 0)).is_none(), "no valid zones");
    }

    #[test]
    fn noise_and_csv() {
        let a = frame(0, 2, 1000);
        let mut b = frame(1, 2, 1000);
        b.mm[0] = 1020;
        let n = zone_noise(&[&a, &b]);
        assert!((n[0].unwrap() - 14.142).abs() < 0.01);
        assert_eq!(n[1], Some(0.0));
        let mut f = Frames::default();
        f.merge(vec![a, b]);
        assert!(f.to_csv().starts_with("t_ms,side,z0,z1,z2,z3\n"));
        assert_eq!(f.to_csv().lines().count(), 3);
    }

    #[test]
    fn nearest_and_occupied_from_the_displayed_frame() {
        let mm = vec![2000, 900, 0, 1990];
        assert_eq!(nearest(&mm), Some(1));
        assert_eq!(occupied(&mm, Some(&[2000, 2000, 2000, 2000]), 150), vec![1]);
        assert!(occupied(&mm, None, 150).is_empty());
        assert!(
            occupied(&mm, Some(&[1, 2]), 150).is_empty(),
            "size mismatch (mode change)"
        );
    }

    #[test]
    fn colours_run_warm_to_cool() {
        assert_eq!(color_for(0, 3500), [230, 60, 50]);
        assert_eq!(color_for(3500, 3500), [50, 110, 220]);
    }
}
