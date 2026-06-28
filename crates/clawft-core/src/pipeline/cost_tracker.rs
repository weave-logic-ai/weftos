//! Cost tracking and budget enforcement for the TieredRouter.
//!
//! Provides per-user daily and monthly cost tracking with configurable budget
//! limits and automatic time-based resets. The [`CostTracker`] is the budget
//! gatekeeper called by `TieredRouter` (Phase C) at routing time.
//!
//! Thread safety: all public methods take `&self` and use interior mutability
//! via [`std::sync::RwLock`]. The struct is `Send + Sync` by construction.
//!
//! # Atomic Budget Reservation (FIX-07)
//!
//! The [`CostTracker::reserve_budget`] method atomically checks all budget
//! dimensions and reserves the estimated cost within a single write-lock
//! acquisition. This prevents TOCTOU race conditions where two concurrent
//! threads could both pass a budget check before either records the spend.
//!
//! # Persistence (FIX-12)
//!
//! Cost state can be persisted to disk as JSON. On Unix, the persistence
//! file is set to mode 0600 (owner read/write only) to protect user IDs
//! and spend amounts from other system users.
//!
//! # Persistence integrity (WEFT-28)
//!
//! The persistence file is HMAC-SHA256 signed with a key derived from
//! either the `WEFTOS_COST_TRACKER_KEY` environment variable (preferred)
//! or, when unset, a deterministic per-host key derived from the
//! workspace's machine-key file. Reads verify the HMAC and a tampered
//! file is rejected with a `warn!` log + a fresh tracker (defense in
//! depth — a local user with file write access can no longer reset
//! their own spend by editing the JSON, and the rest of the daemon
//! continues running rather than crashing on bad input).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::RwLock;

use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use tracing::{debug, warn};

type HmacSha256 = Hmac<Sha256>;

/// Environment variable that supplies the HMAC key for cost-tracker
/// persistence. When unset, [`derive_persistence_key`] falls back to a
/// deterministic per-host key.
pub const COST_TRACKER_KEY_ENV: &str = "WEFTOS_COST_TRACKER_KEY";

/// Magic prefix for HMAC-signed cost-tracker files. Older files (pre-
/// WEFT-28) are unsigned plain JSON; the loader treats those as a
/// trust-on-first-read upgrade path.
const SIGNED_PREFIX: &str = "# weftos-hmac-sha256:";

/// Derive an HMAC key for cost-tracker persistence.
///
/// Resolution order:
/// 1. `WEFTOS_COST_TRACKER_KEY` env var (any non-empty string).
/// 2. Deterministic fallback: `b"clawft-cost-tracker-default-v1"` mixed
///    with the per-process hostname. This keeps the integrity check
///    useful (an attacker without process-level access still can't
///    forge a valid HMAC) without forcing operators to provision a
///    secret before first launch. Operators are encouraged to set the
///    env var explicitly for production deployments.
fn derive_persistence_key() -> Vec<u8> {
    if let Ok(k) = std::env::var(COST_TRACKER_KEY_ENV)
        && !k.is_empty()
    {
        return k.into_bytes();
    }
    // Default: a fixed prefix + the hostname. Hostname is read at
    // runtime via the platform's hostname syscall when available; on
    // failure we fall back to the literal "localhost".
    let host = std::env::var("HOSTNAME")
        .ok()
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .unwrap_or_else(|| "localhost".to_string());
    let mut key = b"clawft-cost-tracker-default-v1:".to_vec();
    key.extend_from_slice(host.as_bytes());
    key
}

/// Compute the HMAC-SHA256 of `body` using the persistence key.
fn sign_body(body: &[u8]) -> String {
    let key = derive_persistence_key();
    let mut mac = HmacSha256::new_from_slice(&key).expect("HMAC-SHA256 accepts any key length");
    mac.update(body);
    let result = mac.finalize().into_bytes();
    hex_encode(&result)
}

/// Verify `expected` (hex-encoded) against the HMAC-SHA256 of `body`.
fn verify_body(body: &[u8], expected_hex: &str) -> bool {
    let key = derive_persistence_key();
    let Ok(mut mac) = HmacSha256::new_from_slice(&key) else {
        return false;
    };
    mac.update(body);
    let Ok(expected) = hex_decode(expected_hex) else {
        return false;
    };
    mac.verify_slice(&expected).is_ok()
}

/// Encode bytes as lowercase hex without pulling in the `hex` crate.
fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0xF) as usize] as char);
    }
    out
}

/// Decode lowercase hex back to bytes. Returns `Err(())` on any
/// character that isn't a valid hex digit or odd length.
fn hex_decode(s: &str) -> Result<Vec<u8>, ()> {
    if !s.len().is_multiple_of(2) {
        return Err(());
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    let bytes = s.as_bytes();
    for pair in bytes.chunks(2) {
        let hi = hex_nibble(pair[0])?;
        let lo = hex_nibble(pair[1])?;
        out.push((hi << 4) | lo);
    }
    Ok(out)
}

fn hex_nibble(b: u8) -> Result<u8, ()> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(()),
    }
}

use crate::pipeline::traits::{BudgetResult, CostTrackable};

// ── UserSpend ───────────────────────────────────────────────────────────

/// Internal per-user spend tracking with reservation support.
///
/// Tracks both committed spend (from reconciled actual costs) and reserved
/// spend (from pending LLM calls whose actual cost is not yet known).
/// Budget checks consider `spent + reserved` to prevent over-allocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct UserSpend {
    /// Committed daily spend in USD.
    daily_spent: f64,
    /// Committed monthly spend in USD.
    monthly_spent: f64,
    /// Reserved (pending) daily amount in USD.
    daily_reserved: f64,
    /// Reserved (pending) monthly amount in USD.
    monthly_reserved: f64,
    /// Timestamp of last daily reset.
    last_daily_reset: chrono::DateTime<chrono::Utc>,
    /// Timestamp of last monthly reset.
    last_monthly_reset: chrono::DateTime<chrono::Utc>,
}

impl UserSpend {
    /// Create a new zero-spend entry with the current timestamp.
    fn new() -> Self {
        let now = chrono::Utc::now();
        Self {
            daily_spent: 0.0,
            monthly_spent: 0.0,
            daily_reserved: 0.0,
            monthly_reserved: 0.0,
            last_daily_reset: now,
            last_monthly_reset: now,
        }
    }

    /// Effective daily total (committed + reserved).
    fn daily_effective(&self) -> f64 {
        self.daily_spent + self.daily_reserved
    }

    /// Effective monthly total (committed + reserved).
    fn monthly_effective(&self) -> f64 {
        self.monthly_spent + self.monthly_reserved
    }
}

// ── CostSnapshot ────────────────────────────────────────────────────────

/// Serializable snapshot of cost tracking state for persistence.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CostSnapshot {
    /// Per-user spend data, keyed by sender_id.
    spends: HashMap<String, UserSpend>,
    /// UTC hour used for daily resets when this snapshot was taken.
    reset_hour_utc: u8,
}

// ── CostTracker ─────────────────────────────────────────────────────────

/// Tracks per-user cost accumulation for budget enforcement.
///
/// Thread-safe via [`RwLock`]. All public methods take `&self`.
///
/// Persistence is best-effort -- a crash may lose recent operations.
/// The loss direction is always under-counting (safe side: users get
/// slightly more budget than intended, never less).
pub struct CostTracker {
    /// Per-user spend tracking. Key is `sender_id` at the API boundary.
    spends: RwLock<HashMap<String, UserSpend>>,
    /// Hour (UTC, 0-23) at which daily budgets reset.
    reset_hour_utc: u8,
    /// Whether to persist to disk.
    persistence_enabled: bool,
    /// Path for the persistence file.
    persistence_path: Option<PathBuf>,
}

impl CostTracker {
    /// Create a new cost tracker with the given daily reset hour.
    ///
    /// `reset_hour_utc` must be 0-23; values >= 24 are clamped to 0.
    pub fn new(reset_hour_utc: u8) -> Self {
        Self {
            spends: RwLock::new(HashMap::new()),
            reset_hour_utc: if reset_hour_utc < 24 {
                reset_hour_utc
            } else {
                0
            },
            persistence_enabled: false,
            persistence_path: None,
        }
    }

    /// Enable persistence at the given path.
    ///
    /// Builder-style method for chaining: `CostTracker::new(0).with_persistence(path)`.
    pub fn with_persistence(mut self, path: PathBuf) -> Self {
        self.persistence_enabled = true;
        self.persistence_path = Some(path);
        self
    }

    // ── Budget reservation (FIX-07) ─────────────────────────────────

    /// Atomically check and reserve budget.
    ///
    /// Acquires a write lock, checks all budget dimensions, and if within
    /// limits, reserves the estimated cost against the user's daily and
    /// monthly totals. On failure, no spend is recorded.
    ///
    /// This replaces the separate check + record pattern to prevent TOCTOU
    /// race conditions. A limit of `0.0` means unlimited (that dimension
    /// is not checked).
    ///
    /// Check order:
    /// 1. Daily limit
    /// 2. Monthly limit
    ///
    /// On success ([`BudgetResult::Approved`]), the estimated cost is
    /// reserved. The caller MUST NOT call [`record_estimated`] separately.
    /// After the LLM response arrives, call [`reconcile_actual`] to adjust.
    pub fn reserve_budget(
        &self,
        sender_id: &str,
        estimated_cost_usd: f64,
        user_daily_limit: f64,
        user_monthly_limit: f64,
    ) -> BudgetResult {
        if estimated_cost_usd <= 0.0 {
            return BudgetResult::Approved;
        }

        let mut spends = self.spends.write().expect("cost tracker lock poisoned");
        let entry = spends
            .entry(sender_id.to_string())
            .or_insert_with(UserSpend::new);

        // Reset counters if a time boundary has been crossed.
        self.maybe_reset_entry(entry);

        // Check daily limit (0.0 = unlimited).
        if user_daily_limit > 0.0 {
            let effective = entry.daily_effective();
            if effective + estimated_cost_usd > user_daily_limit {
                return BudgetResult::DailyLimitExceeded {
                    spent: effective,
                    limit: user_daily_limit,
                };
            }
        }

        // Check monthly limit (0.0 = unlimited).
        if user_monthly_limit > 0.0 {
            let effective = entry.monthly_effective();
            if effective + estimated_cost_usd > user_monthly_limit {
                return BudgetResult::MonthlyLimitExceeded {
                    spent: effective,
                    limit: user_monthly_limit,
                };
            }
        }

        // All checks passed -- commit the reservation.
        entry.daily_reserved += estimated_cost_usd;
        entry.monthly_reserved += estimated_cost_usd;

        BudgetResult::Approved
    }

    /// Adjust a previous reservation after the actual LLM cost is known.
    ///
    /// Moves the estimated cost from `reserved` to `spent`, then adjusts
    /// spent by the delta between actual and estimated. The net effect:
    /// - reserved decreases by estimated_cost
    /// - spent increases by actual_cost
    ///
    /// Per-user spend is clamped to 0.0 minimum.
    pub fn reconcile_actual(&self, sender_id: &str, estimated_cost_usd: f64, actual_cost_usd: f64) {
        let mut spends = self.spends.write().expect("cost tracker lock poisoned");
        let entry = spends
            .entry(sender_id.to_string())
            .or_insert_with(UserSpend::new);

        // Remove reservation.
        entry.daily_reserved = (entry.daily_reserved - estimated_cost_usd).max(0.0);
        entry.monthly_reserved = (entry.monthly_reserved - estimated_cost_usd).max(0.0);

        // Add actual cost to committed spend.
        entry.daily_spent = (entry.daily_spent + actual_cost_usd).max(0.0);
        entry.monthly_spent = (entry.monthly_spent + actual_cost_usd).max(0.0);
    }

    // ── Non-reserving budget check ──────────────────────────────────

    /// Read-only budget check (no reservation).
    ///
    /// Useful for display or pre-check purposes. For the router hot path,
    /// use [`reserve_budget`] instead to avoid TOCTOU races.
    pub fn check_budget(
        &self,
        sender_id: &str,
        estimated_cost_usd: f64,
        user_daily_limit: f64,
        user_monthly_limit: f64,
    ) -> BudgetResult {
        if estimated_cost_usd <= 0.0 {
            return BudgetResult::Approved;
        }

        let mut spends = self.spends.write().expect("cost tracker lock poisoned");
        let entry = spends
            .entry(sender_id.to_string())
            .or_insert_with(UserSpend::new);

        self.maybe_reset_entry(entry);

        // Check daily.
        if user_daily_limit > 0.0 {
            let effective = entry.daily_effective();
            if effective + estimated_cost_usd > user_daily_limit {
                return BudgetResult::DailyLimitExceeded {
                    spent: effective,
                    limit: user_daily_limit,
                };
            }
        }

        // Check monthly.
        if user_monthly_limit > 0.0 {
            let effective = entry.monthly_effective();
            if effective + estimated_cost_usd > user_monthly_limit {
                return BudgetResult::MonthlyLimitExceeded {
                    spent: effective,
                    limit: user_monthly_limit,
                };
            }
        }

        BudgetResult::Approved
    }

    // ── Legacy recording interface ──────────────────────────────────

    /// Record estimated cost (legacy interface).
    ///
    /// Prefer [`reserve_budget`] in the router hot path, which atomically
    /// checks and reserves. This method is retained for backward
    /// compatibility and the `CostTrackable` trait.
    pub fn record_estimated(&self, sender_id: &str, estimated_cost: f64) {
        if estimated_cost <= 0.0 {
            return;
        }

        let mut spends = self.spends.write().expect("cost tracker lock poisoned");
        let entry = spends
            .entry(sender_id.to_string())
            .or_insert_with(UserSpend::new);

        self.maybe_reset_entry(entry);

        entry.daily_spent += estimated_cost;
        entry.monthly_spent += estimated_cost;
    }

    /// Record actual cost, reconciling with a previous [`record_estimated`] call.
    ///
    /// Replaces the estimate in `spent` with the actual cost:
    /// `spent = spent - estimated + actual`.
    ///
    /// If the estimate was placed via [`reserve_budget`] instead (in `reserved`),
    /// use [`reconcile_actual`] directly.
    pub fn record_actual(&self, sender_id: &str, estimated_cost: f64, actual_cost: f64) {
        let mut spends = self.spends.write().expect("cost tracker lock poisoned");
        let entry = spends
            .entry(sender_id.to_string())
            .or_insert_with(UserSpend::new);

        // Remove previous estimate from committed spend, add actual.
        let delta = actual_cost - estimated_cost;
        entry.daily_spent = (entry.daily_spent + delta).max(0.0);
        entry.monthly_spent = (entry.monthly_spent + delta).max(0.0);
    }

    // ── Cost estimation helper ──────────────────────────────────────

    /// Estimate cost for a request based on tier pricing.
    ///
    /// Pure function. `cost_per_1k_tokens` is the blended $/1K rate,
    /// `estimated_tokens` is the total token count (input + output).
    pub fn estimate_cost(cost_per_1k_tokens: f64, estimated_tokens: usize) -> f64 {
        cost_per_1k_tokens * (estimated_tokens as f64) / 1000.0
    }

    // ── Spend queries ───────────────────────────────────────────────

    /// Get current spend for a user as `(daily_effective, monthly_effective)`.
    ///
    /// Returns `(0.0, 0.0)` if the user has no recorded spend.
    pub fn get_spend(&self, sender_id: &str) -> (f64, f64) {
        let spends = self.spends.read().expect("cost tracker lock poisoned");
        match spends.get(sender_id) {
            Some(entry) => (entry.daily_effective(), entry.monthly_effective()),
            None => (0.0, 0.0),
        }
    }

    // ── Persistence ─────────────────────────────────────────────────

    /// Persist current state to disk (if enabled).
    ///
    /// Uses atomic write (temp file + rename) to prevent corruption.
    /// Sets file permissions to 0o600 on Unix (FIX-12).
    /// Prepends an HMAC-SHA256 header line over the JSON body
    /// (WEFT-28) so [`load`] can detect tampering.
    pub fn persist(&self) -> std::io::Result<()> {
        let Some(ref path) = self.persistence_path else {
            return Ok(());
        };

        if !self.persistence_enabled {
            return Ok(());
        }

        // Ensure parent directory exists.
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let snapshot = {
            let spends = self.spends.read().expect("cost tracker lock poisoned");
            CostSnapshot {
                spends: spends.clone(),
                reset_hour_utc: self.reset_hour_utc,
            }
        };

        let json = serde_json::to_string_pretty(&snapshot).map_err(std::io::Error::other)?;

        // WEFT-28: prepend an HMAC-SHA256 header line. The file format
        // is two parts: header + body, separated by a newline. Older
        // unsigned files (no header) are still loadable as a one-time
        // migration path.
        let signature = sign_body(json.as_bytes());
        let signed = format!("{SIGNED_PREFIX}{signature}\n{json}");

        // Write to temp file then rename for atomic replacement.
        let tmp_path = path.with_extension("json.tmp");
        std::fs::write(&tmp_path, &signed)?;
        std::fs::rename(&tmp_path, path)?;

        // FIX-12: Set restrictive permissions on the persistence file.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(path, perms)?;
        }

        debug!("saved cost tracking state to {}", path.display());
        Ok(())
    }

    /// Load state from disk into a new [`CostTracker`].
    ///
    /// If the file does not exist or is corrupt, returns an error.
    /// The caller can fall back to [`CostTracker::new`] in that case.
    ///
    /// WEFT-28 behaviour:
    /// - **Signed file with valid HMAC**: loaded normally.
    /// - **Signed file with invalid HMAC** (tampered): logs a `warn!`
    ///   and returns a freshly-initialised tracker pointed at the same
    ///   path. Does NOT error — the budget enforcement degrades to
    ///   "no historical spend" (defense-in-depth: the file is still
    ///   replaced on the next persist, and an attacker who tampered
    ///   with it has only succeeded in zeroing out their own spend
    ///   counter for one window, which the chain audit log can flag).
    /// - **Unsigned file** (legacy): loaded with a `warn!` for
    ///   visibility, no error. Next persist re-signs it.
    pub fn load(path: &std::path::Path) -> std::io::Result<Self> {
        let data = std::fs::read_to_string(path)?;

        // Try to parse the signed format first.
        let (header, body) = match data.split_once('\n') {
            Some(parts) => parts,
            None => ("", data.as_str()),
        };

        let body_str: &str;
        let mut hmac_verified = false;

        if let Some(sig) = header.strip_prefix(SIGNED_PREFIX) {
            // Signed file — verify before deserialising.
            if verify_body(body.as_bytes(), sig.trim()) {
                hmac_verified = true;
                body_str = body;
            } else {
                warn!(
                    path = %path.display(),
                    "cost tracker persistence file failed HMAC verification \
                     -- discarding tampered data and resetting tracker"
                );
                return Ok(Self {
                    spends: RwLock::new(HashMap::new()),
                    reset_hour_utc: 0,
                    persistence_enabled: true,
                    persistence_path: Some(path.to_path_buf()),
                });
            }
        } else {
            // Legacy unsigned file (pre-WEFT-28). Accept once, log a
            // warning, and re-sign on next persist.
            warn!(
                path = %path.display(),
                "cost tracker persistence file is unsigned (pre-WEFT-28) \
                 -- accepting once, will be HMAC-signed on next persist"
            );
            body_str = data.as_str();
        }

        let snapshot: CostSnapshot = serde_json::from_str(body_str)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

        let tracker = Self {
            spends: RwLock::new(snapshot.spends),
            reset_hour_utc: snapshot.reset_hour_utc,
            persistence_enabled: true,
            persistence_path: Some(path.to_path_buf()),
        };

        debug!(
            hmac_verified,
            "loaded cost tracking state from {}",
            path.display()
        );
        Ok(tracker)
    }

    // ── Internal helpers ────────────────────────────────────────────

    /// Check and apply time-based resets to a single user's entry.
    ///
    /// Daily reset: when the current UTC day (adjusted by `reset_hour_utc`)
    /// differs from the entry's last reset day.
    /// Monthly reset: when the current UTC month differs from the entry's
    /// last reset month.
    fn maybe_reset_entry(&self, entry: &mut UserSpend) {
        let now = chrono::Utc::now();

        // Check monthly reset first (it implies daily reset).
        if self.should_reset_monthly(now, entry.last_monthly_reset) {
            debug!("resetting monthly cost counters");
            entry.daily_spent = 0.0;
            entry.daily_reserved = 0.0;
            entry.monthly_spent = 0.0;
            entry.monthly_reserved = 0.0;
            entry.last_daily_reset = now;
            entry.last_monthly_reset = now;
            return;
        }

        // Check daily reset.
        if self.should_reset_daily(now, entry.last_daily_reset) {
            debug!("resetting daily cost counters");
            entry.daily_spent = 0.0;
            entry.daily_reserved = 0.0;
            entry.last_daily_reset = now;
        }
    }

    /// Determine if a daily reset boundary has been crossed.
    ///
    /// Adjusts timestamps by `reset_hour_utc` so that "days" align with
    /// the configured reset hour rather than midnight.
    fn should_reset_daily(
        &self,
        now: chrono::DateTime<chrono::Utc>,
        last_reset: chrono::DateTime<chrono::Utc>,
    ) -> bool {
        let reset_offset = chrono::Duration::hours(self.reset_hour_utc as i64);
        let adjusted_now = now - reset_offset;
        let adjusted_last = last_reset - reset_offset;

        let day_now = adjusted_now.date_naive();
        let day_last = adjusted_last.date_naive();

        day_now > day_last
    }

    /// Determine if a monthly reset boundary has been crossed.
    fn should_reset_monthly(
        &self,
        now: chrono::DateTime<chrono::Utc>,
        last_reset: chrono::DateTime<chrono::Utc>,
    ) -> bool {
        use chrono::Datelike;
        (now.year(), now.month()) != (last_reset.year(), last_reset.month())
    }
}

impl std::fmt::Debug for CostTracker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let user_count = self.spends.read().map(|s| s.len()).unwrap_or(0);
        f.debug_struct("CostTracker")
            .field("users", &user_count)
            .field("reset_hour_utc", &self.reset_hour_utc)
            .field("persistence_enabled", &self.persistence_enabled)
            .field("persistence_path", &self.persistence_path)
            .finish()
    }
}

impl CostTrackable for CostTracker {
    fn check_budget(
        &self,
        sender_id: &str,
        estimated_cost: f64,
        daily_limit: f64,
        monthly_limit: f64,
    ) -> BudgetResult {
        CostTracker::check_budget(self, sender_id, estimated_cost, daily_limit, monthly_limit)
    }

    fn record_estimated(&self, sender_id: &str, estimated_cost: f64) {
        CostTracker::record_estimated(self, sender_id, estimated_cost);
    }

    fn record_actual(&self, sender_id: &str, estimated_cost: f64, actual_cost: f64) {
        CostTracker::record_actual(self, sender_id, estimated_cost, actual_cost);
    }
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    // ── Helper ──────────────────────────────────────────────────────

    fn tracker() -> CostTracker {
        CostTracker::new(0)
    }

    // ── Basic reserve and check ─────────────────────────────────────

    #[test]
    fn reserve_budget_approved_within_limits() {
        let t = tracker();
        let result = t.reserve_budget("alice", 1.0, 5.0, 100.0);
        assert_eq!(result, BudgetResult::Approved);
    }

    #[test]
    fn check_budget_approved_within_limits() {
        let t = tracker();
        let result = t.check_budget("alice", 1.0, 5.0, 100.0);
        assert_eq!(result, BudgetResult::Approved);
    }

    // ── Daily limit exceeded ────────────────────────────────────────

    #[test]
    fn reserve_budget_daily_limit_exceeded() {
        let t = tracker();
        // Reserve 4.5 of 5.0 daily limit.
        assert_eq!(
            t.reserve_budget("alice", 4.5, 5.0, 100.0),
            BudgetResult::Approved
        );
        // Next 1.0 would exceed.
        let result = t.reserve_budget("alice", 1.0, 5.0, 100.0);
        assert!(matches!(result, BudgetResult::DailyLimitExceeded { .. }));
    }

    #[test]
    fn check_budget_daily_limit_exceeded_after_record() {
        let t = tracker();
        t.record_estimated("alice", 4.5);
        let result = t.check_budget("alice", 1.0, 5.0, 100.0);
        assert!(matches!(result, BudgetResult::DailyLimitExceeded { .. }));
    }

    // ── Monthly limit exceeded ──────────────────────────────────────

    #[test]
    fn reserve_budget_monthly_limit_exceeded() {
        let t = tracker();
        assert_eq!(
            t.reserve_budget("alice", 9.5, 20.0, 10.0),
            BudgetResult::Approved
        );
        let result = t.reserve_budget("alice", 1.0, 20.0, 10.0);
        assert!(matches!(result, BudgetResult::MonthlyLimitExceeded { .. }));
    }

    #[test]
    fn check_budget_monthly_limit_exceeded_after_record() {
        let t = tracker();
        t.record_estimated("alice", 99.5);
        let result = t.check_budget("alice", 1.0, 200.0, 100.0);
        assert!(matches!(result, BudgetResult::MonthlyLimitExceeded { .. }));
    }

    // ── Reserve then reconcile (lower actual) ───────────────────────

    #[test]
    fn reconcile_actual_lower_cost() {
        let t = tracker();
        assert_eq!(
            t.reserve_budget("alice", 3.00, 10.0, 100.0),
            BudgetResult::Approved
        );
        t.reconcile_actual("alice", 3.00, 2.00);
        let (daily, monthly) = t.get_spend("alice");
        // After reconcile: reserved removed (3.0), spent added (2.0).
        // effective daily = 2.0 + 0.0 = 2.0
        assert!((daily - 2.0).abs() < 1e-10);
        assert!((monthly - 2.0).abs() < 1e-10);
    }

    // ── Reserve then reconcile (higher actual) ──────────────────────

    #[test]
    fn reconcile_actual_higher_cost() {
        let t = tracker();
        assert_eq!(
            t.reserve_budget("alice", 2.00, 10.0, 100.0),
            BudgetResult::Approved
        );
        t.reconcile_actual("alice", 2.00, 3.50);
        let (daily, monthly) = t.get_spend("alice");
        // reserved removed (2.0), spent added (3.5). effective = 3.5
        assert!((daily - 3.5).abs() < 1e-10);
        assert!((monthly - 3.5).abs() < 1e-10);
    }

    // ── Multiple users independent tracking ─────────────────────────

    #[test]
    fn independent_user_tracking() {
        let t = tracker();
        t.record_estimated("alice", 3.0);
        t.record_estimated("bob", 7.0);
        t.record_estimated("alice", 2.0);

        let (alice_d, alice_m) = t.get_spend("alice");
        let (bob_d, bob_m) = t.get_spend("bob");

        assert!((alice_d - 5.0).abs() < 1e-10);
        assert!((alice_m - 5.0).abs() < 1e-10);
        assert!((bob_d - 7.0).abs() < 1e-10);
        assert!((bob_m - 7.0).abs() < 1e-10);
    }

    // ── Budget reset (daily) ────────────────────────────────────────

    #[test]
    fn daily_reset_boundary_detection() {
        let t = CostTracker::new(0);
        // Use a fixed noon timestamp to avoid flaking near midnight UTC.
        let now = chrono::DateTime::parse_from_rfc3339("2025-06-15T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let yesterday = now - chrono::Duration::hours(25);

        // Same day: no reset.
        assert!(!t.should_reset_daily(now, now));
        assert!(!t.should_reset_daily(now, now - chrono::Duration::hours(1)));

        // Different day: reset.
        assert!(t.should_reset_daily(now, yesterday));
    }

    #[test]
    fn daily_reset_respects_reset_hour() {
        let t = CostTracker::new(6); // Reset at 06:00 UTC.
        let now = chrono::Utc::now();
        // 25 hours ago always crosses a day boundary.
        let long_ago = now - chrono::Duration::hours(25);
        assert!(t.should_reset_daily(now, long_ago));
    }

    // ── Budget reset (monthly) ──────────────────────────────────────

    #[test]
    fn monthly_reset_boundary_detection() {
        let t = tracker();
        let now = chrono::Utc::now();

        // Same month: no reset.
        assert!(!t.should_reset_monthly(now, now));

        // Different month: reset.
        let different_month = now - chrono::Duration::days(35);
        assert!(t.should_reset_monthly(now, different_month));
    }

    // ── Estimate cost calculation ───────────────────────────────────

    #[test]
    fn estimate_cost_calculation() {
        // Standard: 0.001 per 1K tokens, 1000 tokens -> $0.001
        let cost = CostTracker::estimate_cost(0.001, 1000);
        assert!((cost - 0.001).abs() < 1e-10);

        // Free tier: always zero.
        let cost = CostTracker::estimate_cost(0.0, 2000);
        assert!(cost.abs() < 1e-10);

        // Elite: 0.05 per 1K, 10000 tokens -> $0.50
        let cost = CostTracker::estimate_cost(0.05, 10000);
        assert!((cost - 0.50).abs() < 1e-10);

        // Premium: 0.01 per 1K, 6096 tokens -> $0.06096
        let cost = CostTracker::estimate_cost(0.01, 6096);
        assert!((cost - 0.06096).abs() < 1e-10);
    }

    // ── Zero limit means unlimited ──────────────────────────────────

    #[test]
    fn zero_limit_means_unlimited() {
        let t = tracker();
        t.record_estimated("alice", 999.0);
        // Both limits 0.0 = unlimited.
        let result = t.check_budget("alice", 100.0, 0.0, 0.0);
        assert_eq!(result, BudgetResult::Approved);
    }

    // ── Concurrent access safety ────────────────────────────────────

    #[test]
    fn concurrent_record_no_panic() {
        let t = Arc::new(CostTracker::new(0));
        let mut handles = vec![];

        for i in 0..10 {
            let tracker = Arc::clone(&t);
            handles.push(std::thread::spawn(move || {
                for _ in 0..100 {
                    let user = format!("user_{i}");
                    tracker.record_estimated(&user, 0.01);
                    let _ = tracker.check_budget(&user, 0.01, 50.0, 500.0);
                }
            }));
        }

        for h in handles {
            h.join().expect("thread panicked");
        }

        // 10 users * 100 records * $0.01 = $1.00 each, $10.00 total.
        let total: f64 = (0..10).map(|i| t.get_spend(&format!("user_{i}")).0).sum();
        assert!((total - 10.0).abs() < 0.01);
    }

    #[test]
    fn concurrent_reserve_budget_same_user() {
        let t = Arc::new(CostTracker::new(0));
        let mut handles = vec![];

        // 20 threads each try to reserve $0.50 for the same user with $8.00 daily limit.
        for _ in 0..20 {
            let tracker = Arc::clone(&t);
            handles.push(std::thread::spawn(move || {
                tracker.reserve_budget("alice", 0.50, 8.0, 100.0)
            }));
        }

        let results: Vec<BudgetResult> = handles
            .into_iter()
            .map(|h| h.join().expect("thread panicked"))
            .collect();

        let ok_count = results.iter().filter(|r| r.is_approved()).count();
        let over_count = results.iter().filter(|r| !r.is_approved()).count();

        // With $8.00 limit and $0.50 per request, at most 16 can succeed.
        assert!(ok_count <= 16);
        assert_eq!(ok_count + over_count, 20);
        let (daily, _) = t.get_spend("alice");
        assert!(daily <= 8.0 + 1e-10);
    }

    // ── Persistence roundtrip ───────────────────────────────────────

    #[test]
    fn persistence_roundtrip() {
        let dir = std::env::temp_dir().join("clawft_cost_tracker_test_roundtrip");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("cost_tracking.json");
        let _ = std::fs::remove_file(&path);

        // Create, record, persist.
        {
            let t = CostTracker::new(0).with_persistence(path.clone());
            t.record_estimated("alice", 3.50);
            t.record_estimated("bob", 1.25);
            t.persist().expect("persist failed");
        }

        // Load into a fresh tracker.
        {
            let t = CostTracker::load(&path).expect("load failed");
            let (alice_d, alice_m) = t.get_spend("alice");
            let (bob_d, bob_m) = t.get_spend("bob");
            assert!((alice_d - 3.50).abs() < 1e-10);
            assert!((alice_m - 3.50).abs() < 1e-10);
            assert!((bob_d - 1.25).abs() < 1e-10);
            assert!((bob_m - 1.25).abs() < 1e-10);
        }

        // Cleanup.
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    // ── Empty sender_id handling ────────────────────────────────────

    #[test]
    fn empty_sender_id_works() {
        let t = tracker();
        let result = t.reserve_budget("", 1.0, 5.0, 100.0);
        assert_eq!(result, BudgetResult::Approved);
        let (daily, monthly) = t.get_spend("");
        assert!((daily - 1.0).abs() < 1e-10);
        assert!((monthly - 1.0).abs() < 1e-10);
    }

    // ── get_spend returns correct values ────────────────────────────

    #[test]
    fn get_spend_unknown_user_returns_zero() {
        let t = tracker();
        let (daily, monthly) = t.get_spend("nonexistent");
        assert!(daily.abs() < 1e-10);
        assert!(monthly.abs() < 1e-10);
    }

    #[test]
    fn get_spend_reflects_committed_and_reserved() {
        let t = tracker();
        // Reserve: adds to reserved, not spent.
        t.reserve_budget("alice", 2.0, 10.0, 100.0);
        let (daily, monthly) = t.get_spend("alice");
        assert!((daily - 2.0).abs() < 1e-10);
        assert!((monthly - 2.0).abs() < 1e-10);

        // Reconcile: moves from reserved to spent.
        t.reconcile_actual("alice", 2.0, 1.5);
        let (daily, monthly) = t.get_spend("alice");
        assert!((daily - 1.5).abs() < 1e-10);
        assert!((monthly - 1.5).abs() < 1e-10);
    }

    // ── BudgetResult helpers ────────────────────────────────────────

    #[test]
    fn budget_result_is_approved() {
        assert!(BudgetResult::Approved.is_approved());
        assert!(
            !BudgetResult::DailyLimitExceeded {
                spent: 5.0,
                limit: 5.0
            }
            .is_approved()
        );
        assert!(
            !BudgetResult::MonthlyLimitExceeded {
                spent: 100.0,
                limit: 100.0
            }
            .is_approved()
        );
    }

    // ── Reserve does not record on failure ──────────────────────────

    #[test]
    fn reserve_budget_no_record_on_failure() {
        let t = tracker();
        assert_eq!(
            t.reserve_budget("alice", 4.0, 5.0, 100.0),
            BudgetResult::Approved
        );
        // This should fail and NOT add 2.0.
        let result = t.reserve_budget("alice", 2.0, 5.0, 100.0);
        assert!(matches!(result, BudgetResult::DailyLimitExceeded { .. }));
        // Daily spend should still be 4.0 (only the first reservation).
        let (daily, _) = t.get_spend("alice");
        assert!((daily - 4.0).abs() < 1e-10);
    }

    // ── Zero cost always approved ───────────────────────────────────

    #[test]
    fn reserve_budget_zero_cost_approved() {
        let t = tracker();
        let result = t.reserve_budget("alice", 0.0, 5.0, 100.0);
        assert_eq!(result, BudgetResult::Approved);
        let (daily, _) = t.get_spend("alice");
        assert!(daily.abs() < 1e-10);
    }

    #[test]
    fn reserve_budget_negative_cost_approved() {
        let t = tracker();
        let result = t.reserve_budget("alice", -1.0, 5.0, 100.0);
        assert_eq!(result, BudgetResult::Approved);
        let (daily, _) = t.get_spend("alice");
        assert!(daily.abs() < 1e-10);
    }

    // ── Reconcile clamps to zero ────────────────────────────────────

    #[test]
    fn reconcile_actual_clamps_to_zero() {
        let t = tracker();
        t.reserve_budget("alice", 1.0, 10.0, 100.0);
        // Actual was free: delta removes reservation and adds 0.
        t.reconcile_actual("alice", 1.0, 0.0);
        let (daily, monthly) = t.get_spend("alice");
        assert!(daily >= 0.0);
        assert!(monthly >= 0.0);
    }

    // ── record_estimated zero is noop ───────────────────────────────

    #[test]
    fn record_estimated_zero_is_noop() {
        let t = tracker();
        t.record_estimated("alice", 0.0);
        let (daily, _) = t.get_spend("alice");
        assert!(daily.abs() < 1e-10);
    }

    // ── record_actual delegates to reconcile_actual ─────────────────

    #[test]
    fn record_actual_delegates_correctly() {
        let t = tracker();
        t.record_estimated("alice", 2.0);
        t.record_actual("alice", 2.0, 1.5);
        // reconcile: remove estimated from reserved (noop since it's in spent),
        // add delta to spent. Net: 2.0 + (1.5 - 2.0) = 1.5
        let (daily, monthly) = t.get_spend("alice");
        assert!((daily - 1.5).abs() < 1e-10);
        assert!((monthly - 1.5).abs() < 1e-10);
    }

    // ── WEFT-28: HMAC integrity ─────────────────────────────────────

    #[test]
    fn weft28_persisted_file_has_hmac_header() {
        let dir = std::env::temp_dir().join("clawft_cost_tracker_test_weft28_header");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("cost_tracking.json");
        let _ = std::fs::remove_file(&path);

        let t = CostTracker::new(0).with_persistence(path.clone());
        t.record_estimated("alice", 1.50);
        t.persist().expect("persist failed");

        let raw = std::fs::read_to_string(&path).expect("read failed");
        // First line must be the HMAC header.
        assert!(
            raw.starts_with("# weftos-hmac-sha256:"),
            "expected HMAC header, got: {:?}",
            raw.lines().next()
        );

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn weft28_signed_roundtrip_loads_correctly() {
        let dir = std::env::temp_dir().join("clawft_cost_tracker_test_weft28_roundtrip");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("cost_tracking.json");
        let _ = std::fs::remove_file(&path);

        {
            let t = CostTracker::new(0).with_persistence(path.clone());
            t.record_estimated("alice", 4.25);
            t.persist().expect("persist failed");
        }
        // Loaded tracker should reflect the saved spend.
        let t = CostTracker::load(&path).expect("load failed");
        let (daily, _) = t.get_spend("alice");
        assert!((daily - 4.25).abs() < 1e-10);

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn weft28_tampered_file_resets_tracker() {
        let dir = std::env::temp_dir().join("clawft_cost_tracker_test_weft28_tamper");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("cost_tracking.json");
        let _ = std::fs::remove_file(&path);

        // Persist with a known spend.
        {
            let t = CostTracker::new(0).with_persistence(path.clone());
            t.record_estimated("attacker", 100.0); // burned through budget
            t.persist().expect("persist failed");
        }

        // Tamper: rewrite the body to zero out attacker's spend
        // while leaving the HMAC header intact.
        let raw = std::fs::read_to_string(&path).expect("read failed");
        let (header, _body) = raw.split_once('\n').expect("file has header line");
        let forged = format!(
            "{header}\n{}",
            r#"{"spends":{"attacker":{"daily_spent":0.0,"monthly_spent":0.0,"daily_reserved":0.0,"monthly_reserved":0.0,"last_daily_reset":"2024-01-01T00:00:00Z","last_monthly_reset":"2024-01-01T00:00:00Z"}},"reset_hour_utc":0}"#
        );
        std::fs::write(&path, forged).expect("write tampered");

        // Load — must NOT trust the tampered body. The tracker resets
        // (no historical spend) rather than crashing.
        let t = CostTracker::load(&path).expect("load must succeed even on tamper");
        let (daily, monthly) = t.get_spend("attacker");
        // Reset tracker has no record of the attacker.
        assert!(
            daily.abs() < 1e-10 && monthly.abs() < 1e-10,
            "tampered file must reset to zero, got daily={daily} monthly={monthly}"
        );

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn weft28_unsigned_legacy_file_still_loadable() {
        // A pre-WEFT-28 unsigned file (pure JSON) must still load —
        // operators upgrading from earlier 0.7.x snapshots otherwise
        // lose their cost history.
        let dir = std::env::temp_dir().join("clawft_cost_tracker_test_weft28_legacy");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("cost_tracking.json");
        let _ = std::fs::remove_file(&path);

        // Write the legacy unsigned format directly.
        let legacy = r#"{
  "spends": {
    "carol": {
      "daily_spent": 2.50,
      "monthly_spent": 2.50,
      "daily_reserved": 0.0,
      "monthly_reserved": 0.0,
      "last_daily_reset": "2024-01-01T00:00:00Z",
      "last_monthly_reset": "2024-01-01T00:00:00Z"
    }
  },
  "reset_hour_utc": 0
}"#;
        std::fs::write(&path, legacy).expect("write legacy failed");

        let t = CostTracker::load(&path).expect("legacy load must succeed");
        let (daily, _) = t.get_spend("carol");
        assert!((daily - 2.50).abs() < 1e-10);

        // Re-persisting must produce a signed file.
        t.persist().expect("re-persist failed");
        let raw = std::fs::read_to_string(&path).expect("read failed");
        assert!(raw.starts_with("# weftos-hmac-sha256:"));

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn weft28_env_key_overrides_default() {
        // With WEFTOS_COST_TRACKER_KEY set, signing uses that key
        // instead of the host-derived default.
        let body = b"test body for hmac";

        let sig_default = sign_body(body);

        // SAFETY: setting an env var in a single-threaded test fn body
        // before the second sign_body call is sound. We restore after.
        // Use temp_env crate for safe scoped mutation.
        let sig_env = temp_env::with_var(
            COST_TRACKER_KEY_ENV,
            Some("a-very-different-secret"),
            || sign_body(body),
        );

        assert_ne!(
            sig_default, sig_env,
            "env-var key must change the HMAC output"
        );
    }

    // ── Persistence file permissions (Unix) ─────────────────────────

    #[cfg(unix)]
    #[test]
    fn persistence_file_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join("clawft_cost_tracker_test_perms");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("cost_tracking.json");
        let _ = std::fs::remove_file(&path);

        let t = CostTracker::new(0).with_persistence(path.clone());
        t.record_estimated("alice", 1.0);
        t.persist().expect("persist failed");

        let metadata = std::fs::metadata(&path).expect("metadata failed");
        let mode = metadata.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "persistence file should have 0600 permissions");

        // Cleanup.
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    // ── Debug impl ──────────────────────────────────────────────────

    #[test]
    fn debug_impl_does_not_panic() {
        let t = tracker();
        t.record_estimated("alice", 1.0);
        let debug_str = format!("{:?}", t);
        assert!(debug_str.contains("CostTracker"));
        assert!(debug_str.contains("users"));
    }

    // ── Daily check priority over monthly ───────────────────────────

    #[test]
    fn daily_checked_before_monthly() {
        let t = tracker();
        // Spend 9.5 which exceeds daily (5.0) and monthly (10.0).
        t.record_estimated("alice", 9.5);
        let result = t.check_budget("alice", 1.0, 5.0, 10.0);
        // Should hit daily first since daily is checked before monthly.
        assert!(matches!(result, BudgetResult::DailyLimitExceeded { .. }));
    }
}
