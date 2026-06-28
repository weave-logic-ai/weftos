//! Temperature-based quantization for vector storage (H2.7).
//!
//! Provides storage-layer quantization that reduces disk/memory usage
//! for less frequently accessed vectors without rebuilding the HNSW index.
//!
//! # Temperature tiers
//!
//! | Tier | Storage             | Access pattern     |
//! |------|---------------------|--------------------|
//! | Hot  | Full `Vec<f32>`     | Frequently accessed |
//! | Warm | fp16 on disk        | Moderate access     |
//! | Cold | Product-quantized   | Rarely accessed     |
//!
//! The HNSW index always uses full-precision (`f32`) vectors. Quantization
//! applies only to the storage/serialization layer. When a warm or cold
//! vector is accessed, it is decompressed to `f32` transparently.
//!
//! Tier transitions are driven by access frequency and recency.
//!
//! This module is gated behind the `rvf` feature flag.

use serde::{Deserialize, Serialize};

// ── Temperature tier ────────────────────────────────────────────────

/// Temperature tier for a stored vector.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Temperature {
    /// Full-precision `f32` in memory and on disk.
    Hot,
    /// fp16 on disk, decompressed to `f32` on access.
    Warm,
    /// Product-quantized on disk, decompressed to `f32` on access.
    Cold,
}

impl std::fmt::Display for Temperature {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Temperature::Hot => write!(f, "hot"),
            Temperature::Warm => write!(f, "warm"),
            Temperature::Cold => write!(f, "cold"),
        }
    }
}

// ── fp16 conversion ─────────────────────────────────────────────────

/// Convert an `f32` to IEEE 754 half-precision (fp16) stored as `u16`.
///
/// Uses a simplified conversion that handles normal numbers, zero,
/// and clamps denormals/overflows. Not suitable for NaN preservation.
pub fn f32_to_fp16(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = (bits >> 16) & 0x8000;
    let exponent = ((bits >> 23) & 0xFF) as i32;
    let mantissa = bits & 0x007F_FFFF;

    if exponent == 0 {
        // Zero or denormal -> fp16 zero.
        return sign as u16;
    }

    if exponent == 0xFF {
        // Inf or NaN -> fp16 inf.
        return (sign | 0x7C00) as u16;
    }

    let new_exp = exponent - 127 + 15;

    if new_exp >= 31 {
        // Overflow -> fp16 inf.
        return (sign | 0x7C00) as u16;
    }

    if new_exp <= 0 {
        // Underflow -> fp16 zero.
        return sign as u16;
    }

    let new_mantissa = mantissa >> 13;
    (sign | ((new_exp as u32) << 10) | new_mantissa) as u16
}

/// Convert an fp16 (`u16`) back to `f32`.
pub fn fp16_to_f32(half: u16) -> f32 {
    let sign = ((half as u32) & 0x8000) << 16;
    let exponent = ((half >> 10) & 0x1F) as u32;
    let mantissa = (half & 0x03FF) as u32;

    if exponent == 0 {
        if mantissa == 0 {
            // Zero.
            return f32::from_bits(sign);
        }
        // Denormal fp16 -> normalize to f32.
        let mut e = 1i32;
        let mut m = mantissa;
        while (m & 0x0400) == 0 {
            m <<= 1;
            e -= 1;
        }
        m &= 0x03FF;
        let f32_exp = ((127 - 15 + e) as u32) << 23;
        let f32_mantissa = m << 13;
        return f32::from_bits(sign | f32_exp | f32_mantissa);
    }

    if exponent == 31 {
        // Inf or NaN.
        return f32::from_bits(sign | 0x7F80_0000 | (mantissa << 13));
    }

    let f32_exp = (exponent + 127 - 15) << 23;
    let f32_mantissa = mantissa << 13;
    f32::from_bits(sign | f32_exp | f32_mantissa)
}

// ── Product quantization (simplified) ───────────────────────────────

/// Number of subvectors for product quantization.
const PQ_SUBVECTORS: usize = 8;

/// Codebook size per subvector (256 = 8-bit codes).
const PQ_CODEBOOK_SIZE: usize = 256;

/// A product-quantized vector: stores compact codes and a codebook
/// reference for decompression.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PqVector {
    /// Quantization codes (one per subvector).
    pub codes: Vec<u8>,
    /// Dimension of the original vector.
    pub original_dim: usize,
}

/// A simple product quantization codebook.
///
/// Each subvector has 256 centroids. During encoding, each subvector
/// is mapped to its nearest centroid. During decoding, the centroid
/// values are concatenated to reconstruct the approximation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PqCodebook {
    /// Centroids indexed by [subvector_index][centroid_index].
    /// Each centroid is a flat vector of subvector_dim floats.
    pub centroids: Vec<Vec<Vec<f32>>>,
    /// Number of subvectors.
    pub num_subvectors: usize,
    /// Dimension per subvector.
    pub subvector_dim: usize,
}

impl PqCodebook {
    /// Build a trivial codebook from a set of vectors.
    ///
    /// This is a simplified implementation that uses uniform quantization
    /// (divides the range into 256 equal bins per dimension). A production
    /// implementation would use k-means clustering.
    pub fn build(vectors: &[Vec<f32>], dim: usize) -> Self {
        let num_sub = PQ_SUBVECTORS.min(dim);
        let sub_dim = dim / num_sub;
        let remainder = dim % num_sub;

        let mut centroids = Vec::with_capacity(num_sub);

        for s in 0..num_sub {
            let start = s * sub_dim + s.min(remainder);
            let actual_sub_dim = sub_dim + if s < remainder { 1 } else { 0 };

            // Find min/max per dimension in this subvector.
            let mut mins = vec![f32::MAX; actual_sub_dim];
            let mut maxs = vec![f32::MIN; actual_sub_dim];

            for vec in vectors {
                for d in 0..actual_sub_dim {
                    if start + d < vec.len() {
                        let v = vec[start + d];
                        if v < mins[d] {
                            mins[d] = v;
                        }
                        if v > maxs[d] {
                            maxs[d] = v;
                        }
                    }
                }
            }

            // Build 256 uniformly spaced centroids.
            let mut sub_centroids = Vec::with_capacity(PQ_CODEBOOK_SIZE);
            for c in 0..PQ_CODEBOOK_SIZE {
                let t = c as f32 / (PQ_CODEBOOK_SIZE - 1).max(1) as f32;
                let centroid: Vec<f32> = (0..actual_sub_dim)
                    .map(|d| mins[d] + t * (maxs[d] - mins[d]))
                    .collect();
                sub_centroids.push(centroid);
            }
            centroids.push(sub_centroids);
        }

        PqCodebook {
            centroids,
            num_subvectors: num_sub,
            subvector_dim: sub_dim,
        }
    }

    /// Encode a vector into PQ codes.
    pub fn encode(&self, vector: &[f32]) -> PqVector {
        let dim = vector.len();
        let remainder = dim % self.num_subvectors;
        let mut codes = Vec::with_capacity(self.num_subvectors);

        for s in 0..self.num_subvectors {
            let start = s * self.subvector_dim + s.min(remainder);
            let actual_sub_dim = self.subvector_dim + if s < remainder { 1 } else { 0 };
            let end = (start + actual_sub_dim).min(dim);

            let sub = &vector[start..end];

            // Find nearest centroid.
            let mut best_code: u8 = 0;
            let mut best_dist = f32::MAX;

            for (c, centroid) in self.centroids[s].iter().enumerate() {
                let dist: f32 = sub
                    .iter()
                    .zip(centroid.iter())
                    .map(|(a, b)| (a - b) * (a - b))
                    .sum();
                if dist < best_dist {
                    best_dist = dist;
                    best_code = c as u8;
                }
            }
            codes.push(best_code);
        }

        PqVector {
            codes,
            original_dim: dim,
        }
    }

    /// Decode PQ codes back to an approximate vector.
    pub fn decode(&self, pq: &PqVector) -> Vec<f32> {
        let dim = pq.original_dim;
        let mut result = Vec::with_capacity(dim);

        for s in 0..self.num_subvectors {
            let code = pq.codes[s] as usize;
            let centroid = &self.centroids[s][code];
            result.extend_from_slice(centroid);
        }

        result.truncate(dim);
        result
    }
}

// ── Quantized storage entry ─────────────────────────────────────────

/// A vector stored with temperature-based quantization.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QuantizedVector {
    /// Full-precision f32 vector (hot tier).
    Hot(Vec<f32>),
    /// fp16-compressed vector (warm tier).
    Warm(Vec<u16>),
    /// Product-quantized vector (cold tier).
    Cold(PqVector),
}

impl QuantizedVector {
    /// Decompress to full-precision f32, regardless of tier.
    pub fn decompress(&self, codebook: Option<&PqCodebook>) -> Vec<f32> {
        match self {
            QuantizedVector::Hot(v) => v.clone(),
            QuantizedVector::Warm(fp16s) => fp16s.iter().map(|&h| fp16_to_f32(h)).collect(),
            QuantizedVector::Cold(pq) => {
                if let Some(cb) = codebook {
                    cb.decode(pq)
                } else {
                    // Cannot decode without codebook; return zeros.
                    vec![0.0; pq.original_dim]
                }
            }
        }
    }

    /// Return the temperature tier.
    pub fn temperature(&self) -> Temperature {
        match self {
            QuantizedVector::Hot(_) => Temperature::Hot,
            QuantizedVector::Warm(_) => Temperature::Warm,
            QuantizedVector::Cold(_) => Temperature::Cold,
        }
    }

    /// Compress a full-precision vector to the specified tier.
    pub fn compress(vector: &[f32], tier: Temperature, codebook: Option<&PqCodebook>) -> Self {
        match tier {
            Temperature::Hot => QuantizedVector::Hot(vector.to_vec()),
            Temperature::Warm => {
                let fp16s: Vec<u16> = vector.iter().map(|&v| f32_to_fp16(v)).collect();
                QuantizedVector::Warm(fp16s)
            }
            Temperature::Cold => {
                if let Some(cb) = codebook {
                    QuantizedVector::Cold(cb.encode(vector))
                } else {
                    // Fall back to warm if no codebook.
                    let fp16s: Vec<u16> = vector.iter().map(|&v| f32_to_fp16(v)).collect();
                    QuantizedVector::Warm(fp16s)
                }
            }
        }
    }
}

// ── Access tracker ──────────────────────────────────────────────────

/// Tracks access patterns for temperature-based tier transitions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessTracker {
    /// Total number of accesses.
    pub access_count: u64,
    /// Unix timestamp of the last access (seconds since epoch).
    pub last_access: u64,
    /// Current temperature tier.
    pub temperature: Temperature,
}

impl AccessTracker {
    /// Create a new tracker for a hot entry.
    pub fn new() -> Self {
        Self {
            access_count: 0,
            last_access: now_secs(),
            temperature: Temperature::Hot,
        }
    }

    /// Record an access.
    pub fn record_access(&mut self) {
        self.access_count += 1;
        self.last_access = now_secs();
    }

    /// Determine the recommended tier based on access patterns.
    ///
    /// - Hot: accessed within the last hour or > 10 accesses
    /// - Warm: accessed within the last day or > 3 accesses
    /// - Cold: everything else
    pub fn recommended_tier(&self) -> Temperature {
        let now = now_secs();
        let age_secs = now.saturating_sub(self.last_access);

        if age_secs < 3600 || self.access_count > 10 {
            Temperature::Hot
        } else if age_secs < 86400 || self.access_count > 3 {
            Temperature::Warm
        } else {
            Temperature::Cold
        }
    }
}

impl Default for AccessTracker {
    fn default() -> Self {
        Self::new()
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── fp16 conversion tests ────────────────────────────────────

    #[test]
    fn fp16_roundtrip_zero() {
        let h = f32_to_fp16(0.0);
        let v = fp16_to_f32(h);
        assert!((v - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn fp16_roundtrip_one() {
        let h = f32_to_fp16(1.0);
        let v = fp16_to_f32(h);
        assert!((v - 1.0).abs() < 0.001);
    }

    #[test]
    fn fp16_roundtrip_negative() {
        let h = f32_to_fp16(-0.5);
        let v = fp16_to_f32(h);
        assert!((v - (-0.5)).abs() < 0.001);
    }

    #[test]
    fn fp16_roundtrip_small_value() {
        let h = f32_to_fp16(0.1);
        let v = fp16_to_f32(h);
        assert!((v - 0.1).abs() < 0.01);
    }

    #[test]
    fn fp16_overflow_becomes_inf() {
        let h = f32_to_fp16(100_000.0);
        let v = fp16_to_f32(h);
        assert!(v.is_infinite());
    }

    // ── Quantized vector tests ───────────────────────────────────

    #[test]
    fn hot_decompress_is_identity() {
        let v = vec![1.0, 2.0, 3.0];
        let qv = QuantizedVector::Hot(v.clone());
        assert_eq!(qv.decompress(None), v);
        assert_eq!(qv.temperature(), Temperature::Hot);
    }

    #[test]
    fn warm_decompress_approximate() {
        let v = vec![0.5, -0.3, 0.7];
        let qv = QuantizedVector::compress(&v, Temperature::Warm, None);
        let decompressed = qv.decompress(None);

        assert_eq!(qv.temperature(), Temperature::Warm);
        assert_eq!(decompressed.len(), v.len());

        for (orig, dec) in v.iter().zip(decompressed.iter()) {
            assert!(
                (orig - dec).abs() < 0.01,
                "fp16 roundtrip error too large: {orig} vs {dec}"
            );
        }
    }

    #[test]
    fn cold_with_codebook_roundtrip() {
        let vectors = vec![
            vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            vec![0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            vec![0.5, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        ];
        let codebook = PqCodebook::build(&vectors, 8);

        let qv = QuantizedVector::compress(&vectors[0], Temperature::Cold, Some(&codebook));
        assert_eq!(qv.temperature(), Temperature::Cold);

        let decompressed = qv.decompress(Some(&codebook));
        assert_eq!(decompressed.len(), 8);

        // PQ is lossy but should preserve rough structure.
        assert!(decompressed[0] > decompressed[1]);
    }

    #[test]
    fn cold_without_codebook_fallback() {
        let v = vec![1.0, 2.0, 3.0];
        let qv = QuantizedVector::compress(&v, Temperature::Cold, None);
        // Should fall back to Warm.
        assert_eq!(qv.temperature(), Temperature::Warm);
    }

    // ── Access tracker tests ─────────────────────────────────────

    #[test]
    fn new_tracker_is_hot() {
        let tracker = AccessTracker::new();
        assert_eq!(tracker.temperature, Temperature::Hot);
        assert_eq!(tracker.access_count, 0);
    }

    #[test]
    fn record_access_increments_count() {
        let mut tracker = AccessTracker::new();
        tracker.record_access();
        tracker.record_access();
        assert_eq!(tracker.access_count, 2);
    }

    #[test]
    fn recent_access_stays_hot() {
        let tracker = AccessTracker {
            access_count: 1,
            last_access: now_secs(),
            temperature: Temperature::Hot,
        };
        assert_eq!(tracker.recommended_tier(), Temperature::Hot);
    }

    #[test]
    fn many_accesses_stays_hot() {
        let tracker = AccessTracker {
            access_count: 15,
            last_access: now_secs() - 7200, // 2 hours ago.
            temperature: Temperature::Hot,
        };
        assert_eq!(tracker.recommended_tier(), Temperature::Hot);
    }

    #[test]
    fn moderate_access_becomes_warm() {
        let tracker = AccessTracker {
            access_count: 5,
            last_access: now_secs() - 7200, // 2 hours ago.
            temperature: Temperature::Hot,
        };
        assert_eq!(tracker.recommended_tier(), Temperature::Warm);
    }

    #[test]
    fn old_low_access_becomes_cold() {
        let tracker = AccessTracker {
            access_count: 1,
            last_access: now_secs() - 200_000, // > 2 days ago.
            temperature: Temperature::Hot,
        };
        assert_eq!(tracker.recommended_tier(), Temperature::Cold);
    }

    // ── Temperature display ──────────────────────────────────────

    #[test]
    fn temperature_display() {
        assert_eq!(format!("{}", Temperature::Hot), "hot");
        assert_eq!(format!("{}", Temperature::Warm), "warm");
        assert_eq!(format!("{}", Temperature::Cold), "cold");
    }

    // ── Codebook tests ───────────────────────────────────────────

    #[test]
    fn codebook_build_creates_centroids() {
        let vectors = vec![
            vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            vec![0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        ];
        let codebook = PqCodebook::build(&vectors, 8);
        assert_eq!(codebook.num_subvectors, PQ_SUBVECTORS);
        assert_eq!(codebook.centroids.len(), PQ_SUBVECTORS);
        for sub in &codebook.centroids {
            assert_eq!(sub.len(), PQ_CODEBOOK_SIZE);
        }
    }

    #[test]
    fn codebook_encode_produces_correct_code_count() {
        let vectors = vec![vec![1.0; 8], vec![0.0; 8]];
        let codebook = PqCodebook::build(&vectors, 8);
        let pq = codebook.encode(&[0.5; 8]);
        assert_eq!(pq.codes.len(), PQ_SUBVECTORS);
        assert_eq!(pq.original_dim, 8);
    }

    #[test]
    fn codebook_decode_correct_length() {
        let vectors = vec![vec![1.0; 8], vec![0.0; 8]];
        let codebook = PqCodebook::build(&vectors, 8);
        let pq = codebook.encode(&[0.5; 8]);
        let decoded = codebook.decode(&pq);
        assert_eq!(decoded.len(), 8);
    }

    #[test]
    fn default_access_tracker() {
        let tracker = AccessTracker::default();
        assert_eq!(tracker.access_count, 0);
    }
}
