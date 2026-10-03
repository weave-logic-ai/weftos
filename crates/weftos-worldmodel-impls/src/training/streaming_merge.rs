//! Online streaming-merge trainer with AND-gate promotion (WEFT-531 surface 2).
//!
//! Importance-weighted replay → residual batch update → four-condition AND
//! gate. Checkpoints are only marked `promoted` when the gate allows.

extern crate alloc;

use alloc::vec::Vec;

use weftos_worldmodel_core::{
    zero_latent, GateVerdict, Latent, ModelCheckpoint, RollbackGate, SensorClass, SmallModelKind,
    StreamingMergeTrainer, StreamingTrainResult, TrainingSample, TrainingSurfaceKind,
    WorldModelError, WorldModelResult,
};

use crate::rollback_gate::FourConditionRollbackGate;

use super::offline_edge::OfflineEdgePipeline;
use super::replay::{ImportanceReplayBuffer, DEFAULT_REPLAY_CAPACITY};

/// Promote only when the AND gate allowed the checkpoint.
pub fn require_promoted(result: StreamingTrainResult) -> WorldModelResult<ModelCheckpoint> {
    if result.promoted {
        Ok(result.checkpoint)
    } else {
        Err(WorldModelError::CheckpointRejected)
    }
}

/// Default batch size for one streaming train step.
pub const DEFAULT_BATCH_SIZE: usize = 8;

/// Minimum replay samples before a train step is allowed.
pub const DEFAULT_MIN_REPLAY: usize = 2;

/// Online streaming-merge world-model trainer.
#[derive(Debug, Clone)]
pub struct StreamingMergePipeline {
    /// Importance-weighted replay buffer.
    pub replay: ImportanceReplayBuffer,
    /// Four-condition AND gate (shared metrics with host service).
    pub gate: FourConditionRollbackGate,
    /// Running residual weights (packed as latent).
    residual: Latent,
    /// Next version tag.
    pub next_version: u64,
    /// Batch size per train step.
    pub batch_size: usize,
    /// Minimum samples in replay for a step.
    pub min_replay: usize,
    /// Learning rate for residual EMA update.
    pub learning_rate: f32,
    /// Last gate verdict.
    last_gate: Option<GateVerdict>,
    /// Steps that produced a promoted checkpoint.
    pub promote_count: u64,
    /// Steps rejected by the AND gate.
    pub reject_count: u64,
    /// Total train steps attempted.
    pub step_count: u64,
}

impl Default for StreamingMergePipeline {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamingMergePipeline {
    /// Construct with defaults.
    pub fn new() -> Self {
        Self {
            replay: ImportanceReplayBuffer::new(DEFAULT_REPLAY_CAPACITY),
            gate: FourConditionRollbackGate::new(),
            residual: zero_latent(),
            next_version: 1,
            batch_size: DEFAULT_BATCH_SIZE,
            min_replay: DEFAULT_MIN_REPLAY,
            learning_rate: 0.25,
            last_gate: None,
            promote_count: 0,
            reject_count: 0,
            step_count: 0,
        }
    }

    /// Replace / share the gate (host may inject a live gate).
    pub fn with_gate(mut self, gate: FourConditionRollbackGate) -> Self {
        self.gate = gate;
        self
    }

    /// Current residual weights (latent view).
    pub fn residual(&self) -> &Latent {
        &self.residual
    }

    /// Feed a transition into the AND gate (host live path) so streaming
    /// promotion tracks SIGReg / VoE / probe / straighten.
    pub fn observe_transition_for_gate(
        &mut self,
        z_t: &Latent,
        z_hat: &Latent,
        z_tp1: &Latent,
        sigreg_health: Option<f32>,
    ) -> GateVerdict {
        let v = self
            .gate
            .observe_transition(z_t, z_hat, z_tp1, sigreg_health);
        self.last_gate = Some(v);
        v
    }

    fn update_residual_from_batch(&mut self, batch: &[TrainingSample]) {
        if batch.is_empty() {
            return;
        }
        let mut mean = zero_latent();
        let mut w_sum = 0.0f32;
        for s in batch {
            let w = ImportanceReplayBuffer::weight(s);
            if w <= 0.0 {
                continue;
            }
            let r = s.residual();
            for (m, ri) in mean.iter_mut().zip(r.iter()) {
                *m += w * ri;
            }
            w_sum += w;
        }
        if w_sum <= 0.0 {
            return;
        }
        let lr = self.learning_rate;
        for (res, m) in self.residual.iter_mut().zip(mean.iter()) {
            let target = m / w_sum;
            *res = (1.0 - lr) * *res + lr * target;
        }
    }

    /// Predict `ẑ_{t+1} = z_t + residual` (stub dynamics for gate VoE).
    pub fn predict_residual(&self, z_t: &Latent) -> Latent {
        let mut out = *z_t;
        for (o, res) in out.iter_mut().zip(self.residual.iter()) {
            *o += res;
        }
        out
    }
}

impl StreamingMergeTrainer for StreamingMergePipeline {
    fn push_sample(&mut self, sample: &TrainingSample) -> WorldModelResult<()> {
        // Also feed the gate so online metrics track live data.
        let z_hat = self.predict_residual(&sample.z_t);
        let _ = self.gate.observe_transition(
            &sample.z_t,
            &z_hat,
            &sample.z_tp1,
            None,
        );
        self.replay.push(*sample);
        Ok(())
    }

    fn train_step(
        &mut self,
        class: SensorClass,
        kind: SmallModelKind,
    ) -> WorldModelResult<StreamingTrainResult> {
        if self.replay.count_class(class) < self.min_replay {
            return Err(WorldModelError::TrainingFailed);
        }
        let batch = self.replay.sample_batch(self.batch_size, Some(class));
        if batch.is_empty() {
            return Err(WorldModelError::TrainingFailed);
        }
        self.update_residual_from_batch(&batch);

        // Refresh VoE / probe metrics under the updated residual using
        // **time-ordered** class samples (not the random batch). Random
        // re-feeds would zigzag the temporal-straightening trajectory and
        // false-veto healthy promotions.
        let ordered: Vec<TrainingSample> = self.replay.iter_class(class).copied().collect();
        // Soft reset trajectory-sensitive stats while keeping SIGReg health.
        let sigreg = self.gate.metrics().sigreg_health;
        self.gate.reset_stats();
        self.gate.set_sigreg_health(sigreg);
        for s in &ordered {
            let z_hat = self.predict_residual(&s.z_t);
            let v = self
                .gate
                .observe_transition(&s.z_t, &z_hat, &s.z_tp1, None);
            self.last_gate = Some(v);
        }

        let gate = self.gate.evaluate_now();
        self.last_gate = Some(gate);
        self.step_count = self.step_count.saturating_add(1);

        let weights = OfflineEdgePipeline::pack_residual(&self.residual);
        let version = self.next_version;
        self.next_version = self.next_version.saturating_add(1);
        let checkpoint = ModelCheckpoint::new(
            class,
            kind,
            version,
            TrainingSurfaceKind::OnlineStreamingMerge,
            weights,
            None, // online: no deferred tick; host may still stage if desired
        );

        let promoted = gate.promote;
        if promoted {
            self.promote_count = self.promote_count.saturating_add(1);
        } else {
            self.reject_count = self.reject_count.saturating_add(1);
        }

        Ok(StreamingTrainResult {
            checkpoint,
            gate,
            promoted,
        })
    }

    fn last_gate(&self) -> Option<GateVerdict> {
        self.last_gate
    }

    fn replay_len(&self) -> usize {
        self.replay.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use weftos_worldmodel_core::{GateCondition, LATENT_DIM, StreamingMergeTrainer};

    fn good_sample(t: u64) -> TrainingSample {
        // Straight residual + perfect match for residual predictor after train.
        let mut z0 = zero_latent();
        let mut z1 = zero_latent();
        z0[0] = t as f32;
        z1[0] = (t as f32) + 1.0;
        let mut s = TrainingSample::basic(SensorClass::RgbD, z0, z1, t * 1000);
        s.importance = 1.0;
        s.surprise = 0.1;
        s
    }

    #[test]
    fn one_streaming_cycle_promotes_when_gate_ok() {
        let mut pipe = StreamingMergePipeline::new();
        pipe.gate.min_samples = 2;
        for t in 0..8 {
            pipe.push_sample(&good_sample(t)).unwrap();
        }
        // Healthy SIGReg so only dynamics matter.
        pipe.gate.set_sigreg_health(0.95);
        let result = pipe
            .train_step(SensorClass::RgbD, SmallModelKind::Encode)
            .expect("step");
        assert_eq!(
            result.checkpoint.surface,
            TrainingSurfaceKind::OnlineStreamingMerge
        );
        // With consistent residual 1.0 and perfect pred after update, gate should promote.
        assert!(
            result.promoted,
            "expected promote, gate={:?}",
            result.gate
        );
        assert!(pipe.promote_count >= 1);
        assert!(result.checkpoint.weights.len() == LATENT_DIM * 4);
    }

    #[test]
    fn and_gate_veto_blocks_promotion() {
        let mut pipe = StreamingMergePipeline::new();
        pipe.gate.min_samples = 2;
        for t in 0..6 {
            pipe.push_sample(&good_sample(t)).unwrap();
        }
        // Force SIGReg-only veto.
        pipe.gate.set_sigreg_health(0.10);
        let result = pipe
            .train_step(SensorClass::RgbD, SmallModelKind::Aggregate)
            .expect("step");
        assert!(!result.promoted);
        assert_eq!(
            result.gate.first_veto(),
            Some(GateCondition::ClusterSigRegHealth)
        );
        assert!(matches!(
            require_promoted(result),
            Err(WorldModelError::CheckpointRejected)
        ));
        assert!(pipe.reject_count >= 1);
    }

    #[test]
    fn empty_replay_fails() {
        let mut pipe = StreamingMergePipeline::new();
        assert!(matches!(
            pipe.train_step(SensorClass::Imu, SmallModelKind::TransmitGate),
            Err(WorldModelError::TrainingFailed)
        ));
    }
}
