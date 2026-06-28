//! Cron scheduling types.
//!
//! Defines the data model for scheduled jobs: [`CronJob`], its
//! [`CronSchedule`], [`CronPayload`], and runtime [`CronJobState`].
//! The [`CronStore`] is the top-level container persisted to disk.
//!
//! All timestamps use `DateTime<Utc>` for type safety. For backward
//! compatibility, the serde layer accepts both RFC 3339 strings and
//! millisecond-since-epoch integers via custom deserializers.

use chrono::{DateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};

/// How a cron job is scheduled.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleKind {
    /// Fire once at a specific timestamp.
    At,
    /// Fire repeatedly at a fixed interval.
    Every,
    /// Fire according to a cron expression.
    Cron,
}

/// Schedule definition for a cron job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronSchedule {
    /// The type of schedule.
    pub kind: ScheduleKind,

    /// For [`ScheduleKind::At`]: timestamp in milliseconds since epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at_ms: Option<i64>,

    /// For [`ScheduleKind::Every`]: interval in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub every_ms: Option<i64>,

    /// For [`ScheduleKind::Cron`]: cron expression (e.g. `"0 9 * * *"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expr: Option<String>,

    /// Timezone for cron expressions (e.g. `"UTC"`, `"Asia/Shanghai"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tz: Option<String>,
}

impl Default for CronSchedule {
    fn default() -> Self {
        Self {
            kind: ScheduleKind::Every,
            at_ms: None,
            every_ms: None,
            expr: None,
            tz: None,
        }
    }
}

/// What action to perform when a cron job fires.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PayloadKind {
    /// Emit a system-level event.
    SystemEvent,
    /// Trigger an agent turn with a message.
    AgentTurn,
}

/// Payload executed when a cron job fires.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronPayload {
    /// The type of payload.
    #[serde(default = "default_payload_kind")]
    pub kind: PayloadKind,

    /// Message to deliver or use as agent prompt.
    #[serde(default)]
    pub message: String,

    /// Whether to deliver the response to a channel.
    #[serde(default)]
    pub deliver: bool,

    /// Target channel name (e.g. `"whatsapp"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,

    /// Target recipient (e.g. phone number, user ID).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
}

fn default_payload_kind() -> PayloadKind {
    PayloadKind::AgentTurn
}

impl Default for CronPayload {
    fn default() -> Self {
        Self {
            kind: PayloadKind::AgentTurn,
            message: String::new(),
            deliver: false,
            channel: None,
            to: None,
        }
    }
}

/// Outcome of the last job execution.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    /// The job completed successfully.
    Ok,
    /// The job encountered an error.
    Error,
    /// The job was skipped (e.g. already running).
    Skipped,
}

/// Runtime state of a cron job.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CronJobState {
    /// Next scheduled run time (UTC).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_datetime_or_ms"
    )]
    pub next_run_at: Option<DateTime<Utc>>,

    /// Last actual run time (UTC).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_datetime_or_ms"
    )]
    pub last_run_at: Option<DateTime<Utc>>,

    /// Outcome of the last run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_status: Option<JobStatus>,

    /// Error message from the last failed run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

/// A scheduled job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJob {
    /// Unique job identifier.
    pub id: String,

    /// Human-readable job name.
    pub name: String,

    /// Whether the job is active.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// When and how often to run.
    #[serde(default)]
    pub schedule: CronSchedule,

    /// What to do when the job fires.
    #[serde(default)]
    pub payload: CronPayload,

    /// Runtime state (next run, last run, etc.).
    #[serde(default)]
    pub state: CronJobState,

    /// Creation timestamp (UTC).
    #[serde(
        default = "default_epoch",
        deserialize_with = "deserialize_datetime_or_ms"
    )]
    pub created_at: DateTime<Utc>,

    /// Last update timestamp (UTC).
    #[serde(
        default = "default_epoch",
        deserialize_with = "deserialize_datetime_or_ms"
    )]
    pub updated_at: DateTime<Utc>,

    /// If true, delete the job after its next successful run.
    #[serde(default)]
    pub delete_after_run: bool,
}

/// Returns the Unix epoch as a default `DateTime<Utc>` (for `#[serde(default)]`).
fn default_epoch() -> DateTime<Utc> {
    DateTime::UNIX_EPOCH
}

/// Deserialize a `DateTime<Utc>` from either:
/// - An RFC 3339 string (new format)
/// - An integer (milliseconds since epoch, legacy format)
fn deserialize_datetime_or_ms<'de, D>(deserializer: D) -> Result<DateTime<Utc>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de;

    let value: serde_json::Value = Deserialize::deserialize(deserializer)?;
    match &value {
        serde_json::Value::String(s) => DateTime::parse_from_rfc3339(s)
            .map(|dt| dt.with_timezone(&Utc))
            .or_else(|_| s.parse::<DateTime<Utc>>())
            .map_err(de::Error::custom),
        serde_json::Value::Number(n) => {
            let ms = n
                .as_i64()
                .ok_or_else(|| de::Error::custom("expected i64"))?;
            Utc.timestamp_millis_opt(ms)
                .single()
                .ok_or_else(|| de::Error::custom(format!("invalid ms timestamp: {ms}")))
        }
        serde_json::Value::Null => Ok(DateTime::UNIX_EPOCH),
        _ => Err(de::Error::custom("expected string, integer, or null")),
    }
}

/// Deserialize an `Option<DateTime<Utc>>` from either:
/// - An RFC 3339 string (new format)
/// - An integer (milliseconds since epoch, legacy format)
/// - `null` / missing -> `None`
fn deserialize_optional_datetime_or_ms<'de, D>(
    deserializer: D,
) -> Result<Option<DateTime<Utc>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de;

    let value: Option<serde_json::Value> = Option::deserialize(deserializer)?;
    match value {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(s)) => DateTime::parse_from_rfc3339(&s)
            .map(|dt| Some(dt.with_timezone(&Utc)))
            .or_else(|_| s.parse::<DateTime<Utc>>().map(Some))
            .map_err(de::Error::custom),
        Some(serde_json::Value::Number(n)) => {
            let ms = n
                .as_i64()
                .ok_or_else(|| de::Error::custom("expected i64"))?;
            Ok(Utc.timestamp_millis_opt(ms).single())
        }
        _ => Err(de::Error::custom("expected string, integer, or null")),
    }
}

fn default_true() -> bool {
    true
}

/// Persistent store for cron jobs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronStore {
    /// Schema version for forward compatibility.
    #[serde(default = "default_version")]
    pub version: u32,

    /// All registered cron jobs.
    #[serde(default)]
    pub jobs: Vec<CronJob>,
}

fn default_version() -> u32 {
    1
}

impl Default for CronStore {
    fn default() -> Self {
        Self {
            version: 1,
            jobs: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_default() {
        let s = CronSchedule::default();
        assert_eq!(s.kind, ScheduleKind::Every);
        assert!(s.at_ms.is_none());
        assert!(s.every_ms.is_none());
    }

    #[test]
    fn payload_default() {
        let p = CronPayload::default();
        assert_eq!(p.kind, PayloadKind::AgentTurn);
        assert!(p.message.is_empty());
        assert!(!p.deliver);
    }

    #[test]
    fn cron_store_default() {
        let store = CronStore::default();
        assert_eq!(store.version, 1);
        assert!(store.jobs.is_empty());
    }

    #[test]
    fn cron_job_serde_roundtrip() {
        let now = Utc::now();
        let job = CronJob {
            id: "job-1".into(),
            name: "daily check".into(),
            enabled: true,
            schedule: CronSchedule {
                kind: ScheduleKind::Cron,
                at_ms: None,
                every_ms: None,
                expr: Some("0 9 * * *".into()),
                tz: Some("UTC".into()),
            },
            payload: CronPayload {
                kind: PayloadKind::AgentTurn,
                message: "run daily report".into(),
                deliver: true,
                channel: Some("slack".into()),
                to: Some("C123".into()),
            },
            state: CronJobState::default(),
            created_at: now,
            updated_at: now,
            delete_after_run: false,
        };
        let json = serde_json::to_string(&job).unwrap();
        let restored: CronJob = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.id, "job-1");
        assert_eq!(restored.schedule.kind, ScheduleKind::Cron);
        assert_eq!(restored.schedule.expr.as_deref(), Some("0 9 * * *"));
        assert_eq!(restored.payload.channel.as_deref(), Some("slack"));
    }

    #[test]
    fn cron_store_serde_roundtrip() {
        let store = CronStore {
            version: 1,
            jobs: vec![CronJob {
                id: "j1".into(),
                name: "test".into(),
                enabled: true,
                schedule: CronSchedule::default(),
                payload: CronPayload::default(),
                state: CronJobState::default(),
                created_at: DateTime::UNIX_EPOCH,
                updated_at: DateTime::UNIX_EPOCH,
                delete_after_run: true,
            }],
        };
        let json = serde_json::to_string(&store).unwrap();
        let restored: CronStore = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.version, 1);
        assert_eq!(restored.jobs.len(), 1);
        assert!(restored.jobs[0].delete_after_run);
    }

    #[test]
    fn schedule_kind_serde() {
        let kinds = [
            (ScheduleKind::At, "\"at\""),
            (ScheduleKind::Every, "\"every\""),
            (ScheduleKind::Cron, "\"cron\""),
        ];
        for (kind, expected) in &kinds {
            let json = serde_json::to_string(kind).unwrap();
            assert_eq!(&json, expected);
            let restored: ScheduleKind = serde_json::from_str(&json).unwrap();
            assert_eq!(restored, *kind);
        }
    }

    #[test]
    fn job_status_serde() {
        let statuses = [
            (JobStatus::Ok, "\"ok\""),
            (JobStatus::Error, "\"error\""),
            (JobStatus::Skipped, "\"skipped\""),
        ];
        for (status, expected) in &statuses {
            let json = serde_json::to_string(status).unwrap();
            assert_eq!(&json, expected);
        }
    }

    #[test]
    fn cron_job_defaults_on_missing_fields() {
        let json = r#"{"id": "j1", "name": "test"}"#;
        let job: CronJob = serde_json::from_str(json).unwrap();
        assert!(job.enabled); // default true
        assert_eq!(job.schedule.kind, ScheduleKind::Every);
        assert_eq!(job.payload.kind, PayloadKind::AgentTurn);
        assert!(!job.delete_after_run);
    }

    #[test]
    fn job_state_with_error() {
        let now = Utc::now();
        let state = CronJobState {
            next_run_at: Some(now),
            last_run_at: Some(now),
            last_status: Some(JobStatus::Error),
            last_error: Some("connection refused".into()),
        };
        let json = serde_json::to_string(&state).unwrap();
        let restored: CronJobState = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.last_status, Some(JobStatus::Error));
        assert_eq!(restored.last_error.as_deref(), Some("connection refused"));
    }

    #[test]
    fn backward_compat_ms_timestamps() {
        // Legacy format: millisecond integers.
        let json = r#"{
            "id": "legacy-1",
            "name": "old-job",
            "created_at": 1700000000000,
            "updated_at": 1700000000000,
            "state": {
                "next_run_at": 1700000100000,
                "last_run_at": 1700000000000
            }
        }"#;
        let job: CronJob = serde_json::from_str(json).unwrap();
        assert_eq!(job.id, "legacy-1");
        assert_eq!(job.created_at.timestamp_millis(), 1_700_000_000_000);
        assert!(job.state.next_run_at.is_some());
        assert!(job.state.last_run_at.is_some());
    }

    #[test]
    fn backward_compat_legacy_field_names() {
        // Legacy JSONL may have old field names with _ms suffix.
        // These will be ignored by the new struct (fields renamed).
        // This test verifies the new fields parse from their new names.
        let json = r#"{
            "id": "j1",
            "name": "test",
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z"
        }"#;
        let job: CronJob = serde_json::from_str(json).unwrap();
        assert_eq!(
            job.created_at,
            Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap()
        );
    }
}
