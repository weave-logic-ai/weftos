//! Stub infrastructure for shaal PR #352 synergy features.
//!
//! ## KG-011: LogQuantized for DiskANN
//!
//! Logarithmic quantization replaces scalar PQ codebooks with
//! log-scaled bins, yielding 20-52% lower reconstruction error on
//! skewed embedding distributions.
//!
//! ## KG-012: Unified SIMD Distance Kernel
//!
//! Branch-free SIMD distance computation that unifies cosine, L2, and
//! inner-product kernels into a single codepath, giving +14% QPS on
//! SIFT1M-class workloads.
//!
//! **Both features require `ruvector-core` with PR #352 merged.**
//! Until that dependency is available these remain configuration-only
//! stubs that can be wired into [`VectorConfig`] today.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// LogQuantized configuration (KG-011)
// ---------------------------------------------------------------------------

/// Configuration for logarithmic quantization (from shaal's PR #352).
///
/// Activated when `ruvector-core >= version` with `LogQuantized` support.
/// Replaces scalar PQ codebooks with log-scaled bins, reducing
/// reconstruction error by 20-52% on skewed distributions.
///
/// Requires `ruvector-core` with PR #352 merged.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogQuantizedConfig {
    /// Whether logarithmic quantization is enabled.
    ///
    /// Default: `false` (stub -- awaiting ruvector-core upgrade).
    #[serde(default)]
    pub enabled: bool,

    /// Compression ratio for quantized vectors.
    ///
    /// A ratio of 4 means each `f32` is compressed to ~8 bits.
    /// Higher ratios save memory but increase reconstruction error.
    ///
    /// Default: `4`.
    #[serde(default = "default_compression_ratio")]
    pub compression_ratio: usize,

    /// Minimum magnitude threshold below which values are clamped to
    /// zero before log-scaling. Prevents log(0) instability.
    ///
    /// Default: `1e-7`.
    #[serde(default = "default_min_magnitude")]
    pub min_magnitude: f64,
}

fn default_compression_ratio() -> usize {
    4
}

fn default_min_magnitude() -> f64 {
    1e-7
}

impl Default for LogQuantizedConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            compression_ratio: default_compression_ratio(),
            min_magnitude: default_min_magnitude(),
        }
    }
}

impl LogQuantizedConfig {
    /// Returns `true` when the configuration requests activation AND
    /// the underlying `ruvector-core` dependency supports it.
    ///
    /// Currently always returns `false` because PR #352 has not yet
    /// merged. Once the dependency is upgraded, this will check the
    /// actual feature flag / version.
    pub fn is_available(&self) -> bool {
        // TODO(KG-011): Check ruvector-core version once PR #352 merges.
        false
    }
}

// ---------------------------------------------------------------------------
// SIMD distance configuration (KG-012)
// ---------------------------------------------------------------------------

/// Distance metric to use with the unified SIMD kernel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SimdDistanceMetric {
    /// Cosine similarity (default for text embeddings).
    #[default]
    Cosine,
    /// Euclidean (L2) distance.
    L2,
    /// Inner product (dot product).
    InnerProduct,
}

/// Configuration for the unified SIMD distance kernel (from shaal's
/// PR #352 `UnifiedDistanceParams`).
///
/// Branch-free SIMD distance achieves +14% QPS on SIFT1M. This
/// configuration controls alignment and metric selection.
///
/// Requires `ruvector-core` with PR #352 merged.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[derive(Default)]
pub struct SimdDistanceConfig {
    /// Whether the unified SIMD distance kernel is enabled.
    ///
    /// Default: `false` (stub -- awaiting ruvector-core upgrade).
    #[serde(default)]
    pub enabled: bool,

    /// Whether to pad vectors to the next power-of-two length for
    /// SIMD alignment.
    ///
    /// Default: `false`.
    ///
    /// **Caveat (shaal v4):** Padding may increase memory usage by
    /// up to 2x for odd-dimensioned embeddings. Only enable when
    /// benchmarks confirm a net benefit for your embedding dimensions.
    #[serde(default)]
    pub pad_to_power_of_two: bool,

    /// Distance metric used by the SIMD kernel.
    #[serde(default)]
    pub metric: SimdDistanceMetric,

    /// Preferred SIMD lane width (128, 256, or 512 bits).
    /// `None` means auto-detect from CPU features.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane_width: Option<u16>,
}


impl SimdDistanceConfig {
    /// Returns `true` when the configuration requests activation AND
    /// the underlying `ruvector-core` dependency supports it.
    ///
    /// Currently always returns `false` because PR #352 has not yet
    /// merged.
    pub fn is_available(&self) -> bool {
        // TODO(KG-012): Check ruvector-core version once PR #352 merges.
        false
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- LogQuantizedConfig -------------------------------------------------

    #[test]
    fn log_quantized_default() {
        let cfg = LogQuantizedConfig::default();
        assert!(!cfg.enabled);
        assert_eq!(cfg.compression_ratio, 4);
        assert!((cfg.min_magnitude - 1e-7).abs() < 1e-15);
    }

    #[test]
    fn log_quantized_not_available_yet() {
        let cfg = LogQuantizedConfig {
            enabled: true,
            ..Default::default()
        };
        // Stub: always false until ruvector-core upgrade.
        assert!(!cfg.is_available());
    }

    #[test]
    fn log_quantized_serde_roundtrip() {
        let cfg = LogQuantizedConfig {
            enabled: true,
            compression_ratio: 8,
            min_magnitude: 1e-5,
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let restored: LogQuantizedConfig = serde_json::from_str(&json).unwrap();
        assert!(restored.enabled);
        assert_eq!(restored.compression_ratio, 8);
        assert!((restored.min_magnitude - 1e-5).abs() < 1e-12);
    }

    #[test]
    fn log_quantized_deserialize_defaults() {
        let json = "{}";
        let cfg: LogQuantizedConfig = serde_json::from_str(json).unwrap();
        assert!(!cfg.enabled);
        assert_eq!(cfg.compression_ratio, 4);
    }

    // -- SimdDistanceConfig -------------------------------------------------

    #[test]
    fn simd_distance_default() {
        let cfg = SimdDistanceConfig::default();
        assert!(!cfg.enabled);
        assert!(!cfg.pad_to_power_of_two);
        assert_eq!(cfg.metric, SimdDistanceMetric::Cosine);
        assert!(cfg.lane_width.is_none());
    }

    #[test]
    fn simd_distance_not_available_yet() {
        let cfg = SimdDistanceConfig {
            enabled: true,
            ..Default::default()
        };
        assert!(!cfg.is_available());
    }

    #[test]
    fn simd_distance_serde_roundtrip() {
        let cfg = SimdDistanceConfig {
            enabled: true,
            pad_to_power_of_two: true,
            metric: SimdDistanceMetric::L2,
            lane_width: Some(256),
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let restored: SimdDistanceConfig = serde_json::from_str(&json).unwrap();
        assert!(restored.enabled);
        assert!(restored.pad_to_power_of_two);
        assert_eq!(restored.metric, SimdDistanceMetric::L2);
        assert_eq!(restored.lane_width, Some(256));
    }

    #[test]
    fn simd_distance_deserialize_inner_product() {
        let json = r#"{"enabled": true, "metric": "innerproduct"}"#;
        let cfg: SimdDistanceConfig = serde_json::from_str(json).unwrap();
        assert!(cfg.enabled);
        assert_eq!(cfg.metric, SimdDistanceMetric::InnerProduct);
    }

    #[test]
    fn simd_distance_deserialize_defaults() {
        let json = "{}";
        let cfg: SimdDistanceConfig = serde_json::from_str(json).unwrap();
        assert!(!cfg.enabled);
        assert!(!cfg.pad_to_power_of_two);
        assert_eq!(cfg.metric, SimdDistanceMetric::Cosine);
    }
}
