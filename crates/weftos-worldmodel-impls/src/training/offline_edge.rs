//! Offline per-sensor-class edge-intelligence pipeline (WEFT-531 surface 1).
//!
//! Accumulates ExoChain / synthetic samples, runs a residual-mean train cycle
//! (stub for real optimiser), and emits an RVF-deliverable checkpoint for
//! tick-aligned hot-swap via [`crate::training::SensorModelRegistry`].

extern crate alloc;

use alloc::vec::Vec;

use weftos_worldmodel_core::{
    zero_latent, Latent, ModelCheckpoint, OfflineEdgeTrainer, SensorClass, SmallModelKind,
    TrainingSample, TrainingSurfaceKind, WorldModelError, WorldModelResult, LATENT_DIM,
};

use super::replay::{ImportanceReplayBuffer, DEFAULT_REPLAY_CAPACITY};

/// Minimum samples required for an offline train cycle.
pub const DEFAULT_MIN_OFFLINE_SAMPLES: usize = 2;

/// Offline edge-intelligence trainer (weights-free residual stub).
#[derive(Debug, Clone)]
pub struct OfflineEdgePipeline {
    /// Sample buffer (all classes; filtered at train time).
    pub buffer: ImportanceReplayBuffer,
    /// Next version tag to assign (monotonic).
    pub next_version: u64,
    /// Minimum samples per class for a cycle.
    pub min_samples: usize,
    /// Train cycles completed.
    pub cycles_completed: u64,
}

impl Default for OfflineEdgePipeline {
    fn default() -> Self {
        Self::new()
    }
}

impl OfflineEdgePipeline {
    /// Construct with default capacity / min samples.
    pub fn new() -> Self {
        Self {
            buffer: ImportanceReplayBuffer::new(DEFAULT_REPLAY_CAPACITY),
            next_version: 1,
            min_samples: DEFAULT_MIN_OFFLINE_SAMPLES,
            cycles_completed: 0,
        }
    }

    /// Pack a residual latent as little-endian f32 weight bytes.
    pub fn pack_residual(residual: &Latent) -> Vec<u8> {
        let mut out = Vec::with_capacity(LATENT_DIM * 4);
        for &v in residual.iter() {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out
    }

    /// Unpack weight bytes back into a residual latent (zeros on short input).
    pub fn unpack_residual(weights: &[u8]) -> Latent {
        let mut z = zero_latent();
        for (i, zi) in z.iter_mut().enumerate() {
            let start = i * 4;
            if start + 4 > weights.len() {
                break;
            }
            let mut buf = [0u8; 4];
            buf.copy_from_slice(&weights[start..start + 4]);
            *zi = f32::from_le_bytes(buf);
        }
        z
    }

    /// Importance-weighted mean residual over samples of `class`.
    fn weighted_mean_residual(&self, class: SensorClass) -> Option<Latent> {
        let mut acc = zero_latent();
        let mut w_sum = 0.0f32;
        for s in self.buffer.iter_class(class) {
            let w = ImportanceReplayBuffer::weight(s);
            if w <= 0.0 {
                continue;
            }
            let r = s.residual();
            for (a, ri) in acc.iter_mut().zip(r.iter()) {
                *a += w * ri;
            }
            w_sum += w;
        }
        if w_sum <= 0.0 {
            return None;
        }
        for a in acc.iter_mut() {
            *a /= w_sum;
        }
        Some(acc)
    }
}

impl OfflineEdgeTrainer for OfflineEdgePipeline {
    fn push_sample(&mut self, sample: &TrainingSample) -> WorldModelResult<()> {
        self.buffer.push(*sample);
        Ok(())
    }

    fn train_cycle(
        &mut self,
        class: SensorClass,
        kind: SmallModelKind,
        target_tick: u64,
    ) -> WorldModelResult<ModelCheckpoint> {
        if self.buffer.count_class(class) < self.min_samples {
            return Err(WorldModelError::TrainingFailed);
        }
        let residual = self
            .weighted_mean_residual(class)
            .ok_or(WorldModelError::TrainingFailed)?;
        let weights = Self::pack_residual(&residual);
        let version = self.next_version;
        self.next_version = self.next_version.saturating_add(1);
        self.cycles_completed = self.cycles_completed.saturating_add(1);
        Ok(ModelCheckpoint::new(
            class,
            kind,
            version,
            TrainingSurfaceKind::OfflineEdge,
            weights,
            Some(target_tick),
        ))
    }

    fn sample_count(&self) -> usize {
        self.buffer.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use weftos_worldmodel_core::OfflineEdgeTrainer;

    fn step_sample(class: SensorClass, t: f32) -> TrainingSample {
        let mut z0 = zero_latent();
        let mut z1 = zero_latent();
        z0[0] = t;
        z1[0] = t + 2.0; // residual 2.0 on dim 0
        TrainingSample::basic(class, z0, z1, (t as u64) * 10)
    }

    #[test]
    fn one_offline_cycle_emits_checkpoint() {
        let mut pipe = OfflineEdgePipeline::new();
        pipe.push_sample(&step_sample(SensorClass::Imu, 0.0))
            .unwrap();
        pipe.push_sample(&step_sample(SensorClass::Imu, 1.0))
            .unwrap();
        let cp = pipe
            .train_cycle(SensorClass::Imu, SmallModelKind::Encode, 10)
            .expect("cycle");
        assert_eq!(cp.surface, TrainingSurfaceKind::OfflineEdge);
        assert_eq!(cp.sensor_class, SensorClass::Imu);
        assert_eq!(cp.kind, SmallModelKind::Encode);
        assert_eq!(cp.target_tick, Some(10));
        assert_eq!(cp.version_tag, 1);
        let r = OfflineEdgePipeline::unpack_residual(&cp.weights);
        assert!((r[0] - 2.0).abs() < 1e-5, "residual={}", r[0]);
        assert_eq!(pipe.cycles_completed, 1);
    }

    #[test]
    fn insufficient_samples_fail() {
        let mut pipe = OfflineEdgePipeline::new();
        pipe.push_sample(&step_sample(SensorClass::Audio, 0.0))
            .unwrap();
        assert!(matches!(
            pipe.train_cycle(SensorClass::Audio, SmallModelKind::Aggregate, 1),
            Err(WorldModelError::TrainingFailed)
        ));
    }
}
