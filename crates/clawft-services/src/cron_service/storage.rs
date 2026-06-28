//! JSONL append-only persistence for cron jobs.
//!
//! Events are appended as newline-delimited JSON. On load, the event
//! log is replayed to reconstruct the current set of active jobs.
//!
//! Uses the canonical [`CronJob`] type from [`clawft_types::cron`].

use std::path::PathBuf;

use chrono::TimeZone;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tracing::warn;

use super::scheduler::CronJob;
use crate::error::Result;

/// Event types stored in the JSONL log.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum StorageEvent {
    /// A new job was created.
    Create { job: Box<CronJob> },
    /// A field on an existing job was updated.
    Update {
        job_id: String,
        field: String,
        value: serde_json::Value,
    },
    /// A job was deleted.
    Delete { job_id: String },
}

/// JSONL append-only storage for cron job events.
pub struct CronStorage {
    path: PathBuf,
}

impl CronStorage {
    /// Create a new storage instance backed by the given file path.
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Return the path to the storage file.
    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    /// Append a job creation event.
    pub async fn append_create(&self, job: &CronJob) -> Result<()> {
        let event = StorageEvent::Create {
            job: Box::new(job.clone()),
        };
        self.append_event(&event).await
    }

    /// Append a field update event.
    pub async fn append_update(
        &self,
        job_id: &str,
        field: &str,
        value: &serde_json::Value,
    ) -> Result<()> {
        let event = StorageEvent::Update {
            job_id: job_id.to_string(),
            field: field.to_string(),
            value: value.clone(),
        };
        self.append_event(&event).await
    }

    /// Append a deletion event.
    pub async fn append_delete(&self, job_id: &str) -> Result<()> {
        let event = StorageEvent::Delete {
            job_id: job_id.to_string(),
        };
        self.append_event(&event).await
    }

    /// Replay the event log and reconstruct all active jobs.
    ///
    /// Invalid lines are skipped with a warning.
    pub async fn load_jobs(&self) -> Result<Vec<CronJob>> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }

        let content = tokio::fs::read_to_string(&self.path).await?;
        Ok(replay_events(&content))
    }

    /// Append a serialized event followed by a newline.
    async fn append_event(&self, event: &StorageEvent) -> Result<()> {
        // Ensure parent directory exists.
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let mut line = serde_json::to_string(event)?;
        line.push('\n');

        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .await?;
        file.write_all(line.as_bytes()).await?;
        file.flush().await?;

        Ok(())
    }
}

/// Replay JSONL event content and reconstruct active jobs.
///
/// This is the shared logic used by both async ([`CronStorage::load_jobs`])
/// and synchronous ([`load_jobs_sync`]) loading paths.
pub fn replay_events(content: &str) -> Vec<CronJob> {
    let mut jobs = std::collections::HashMap::<String, CronJob>::new();

    for (line_no, line) in content.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        match serde_json::from_str::<StorageEvent>(line) {
            Ok(StorageEvent::Create { job }) => {
                jobs.insert(job.id.clone(), *job);
            }
            Ok(StorageEvent::Update {
                job_id,
                field,
                value,
            }) => {
                if let Some(job) = jobs.get_mut(&job_id) {
                    apply_field_update(job, &field, &value);
                }
            }
            Ok(StorageEvent::Delete { job_id }) => {
                jobs.remove(&job_id);
            }
            Err(e) => {
                warn!(line = line_no + 1, error = %e, "skipping invalid JSONL line");
            }
        }
    }

    jobs.into_values().collect()
}

/// Synchronously load jobs from a JSONL file.
///
/// Used by the CLI which runs without an async runtime.
pub fn load_jobs_sync(path: &std::path::Path) -> std::io::Result<Vec<CronJob>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = std::fs::read_to_string(path)?;
    Ok(replay_events(&content))
}

/// Synchronously append a create event to a JSONL file.
///
/// Used by the CLI which runs without an async runtime.
pub fn append_create_sync(path: &std::path::Path, job: &CronJob) -> std::io::Result<()> {
    let event = StorageEvent::Create {
        job: Box::new(job.clone()),
    };
    append_event_sync(path, &event)
}

/// Synchronously append a delete event to a JSONL file.
pub fn append_delete_sync(path: &std::path::Path, job_id: &str) -> std::io::Result<()> {
    let event = StorageEvent::Delete {
        job_id: job_id.to_string(),
    };
    append_event_sync(path, &event)
}

/// Synchronously append an update event to a JSONL file.
pub fn append_update_sync(
    path: &std::path::Path,
    job_id: &str,
    field: &str,
    value: &serde_json::Value,
) -> std::io::Result<()> {
    let event = StorageEvent::Update {
        job_id: job_id.to_string(),
        field: field.to_string(),
        value: value.clone(),
    };
    append_event_sync(path, &event)
}

/// Synchronously append an event to the JSONL file.
fn append_event_sync(path: &std::path::Path, event: &StorageEvent) -> std::io::Result<()> {
    use std::io::Write;

    // Ensure parent directory exists.
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut line = serde_json::to_string(event)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    line.push('\n');

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(line.as_bytes())?;
    file.flush()?;

    Ok(())
}

/// Apply a field update to a job in memory.
fn apply_field_update(job: &mut CronJob, field: &str, value: &serde_json::Value) {
    match field {
        "enabled" => {
            if let Some(v) = value.as_bool() {
                job.enabled = v;
            }
        }
        "message" | "prompt" => {
            if let Some(v) = value.as_str() {
                job.payload.message = v.to_string();
            }
        }
        "name" => {
            if let Some(v) = value.as_str() {
                job.name = v.to_string();
            }
        }
        "last_run_at_ms" | "last_run_at" => {
            // Accept both i64 ms (legacy) and RFC 3339 string (new format).
            if let Some(ms) = value.as_i64() {
                job.state.last_run_at = chrono::Utc.timestamp_millis_opt(ms).single();
            } else if let Some(s) = value.as_str()
                && let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s)
            {
                job.state.last_run_at = Some(dt.with_timezone(&chrono::Utc));
            }
        }
        "next_run_at_ms" | "next_run_at" => {
            // Accept both i64 ms (legacy) and RFC 3339 string (new format).
            if let Some(ms) = value.as_i64() {
                job.state.next_run_at = chrono::Utc.timestamp_millis_opt(ms).single();
            } else if let Some(s) = value.as_str()
                && let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s)
            {
                job.state.next_run_at = Some(dt.with_timezone(&chrono::Utc));
            }
        }
        "last_status" => {
            if let Ok(v) =
                serde_json::from_value::<Option<clawft_types::cron::JobStatus>>(value.clone())
            {
                job.state.last_status = v;
            }
        }
        "last_error" => {
            job.state.last_error = value.as_str().map(|s| s.to_string());
        }
        _ => {
            warn!(field, "unknown field in storage update event");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use clawft_types::cron::{CronJobState, CronPayload, CronSchedule, ScheduleKind};

    fn make_job(id: &str, name: &str) -> CronJob {
        let now = Utc::now();
        CronJob {
            id: id.into(),
            name: name.into(),
            enabled: true,
            schedule: CronSchedule {
                kind: ScheduleKind::Cron,
                at_ms: None,
                every_ms: None,
                expr: Some("0 0 * * * * *".into()),
                tz: Some("UTC".into()),
            },
            payload: CronPayload {
                message: "test".into(),
                ..Default::default()
            },
            state: CronJobState::default(),
            created_at: now,
            updated_at: now,
            delete_after_run: false,
        }
    }

    #[tokio::test]
    async fn append_create_and_load() {
        let dir = std::env::temp_dir().join(format!("clawft-test-{}", uuid::Uuid::new_v4()));
        let path = dir.join("cron.jsonl");
        let storage = CronStorage::new(path);

        let job = make_job("j1", "test-job");
        storage.append_create(&job).await.unwrap();

        let jobs = storage.load_jobs().await.unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].id, "j1");
        assert_eq!(jobs[0].name, "test-job");

        // Cleanup.
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn append_update_and_reload() {
        let dir = std::env::temp_dir().join(format!("clawft-test-{}", uuid::Uuid::new_v4()));
        let path = dir.join("cron.jsonl");
        let storage = CronStorage::new(path);

        let job = make_job("j1", "test-job");
        storage.append_create(&job).await.unwrap();
        storage
            .append_update("j1", "enabled", &serde_json::json!(false))
            .await
            .unwrap();

        let jobs = storage.load_jobs().await.unwrap();
        assert_eq!(jobs.len(), 1);
        assert!(!jobs[0].enabled);

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn append_delete_and_reload() {
        let dir = std::env::temp_dir().join(format!("clawft-test-{}", uuid::Uuid::new_v4()));
        let path = dir.join("cron.jsonl");
        let storage = CronStorage::new(path);

        storage.append_create(&make_job("j1", "a")).await.unwrap();
        storage.append_create(&make_job("j2", "b")).await.unwrap();
        storage.append_delete("j1").await.unwrap();

        let jobs = storage.load_jobs().await.unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].id, "j2");

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn skip_invalid_jsonl_lines() {
        let dir = std::env::temp_dir().join(format!("clawft-test-{}", uuid::Uuid::new_v4()));
        let path = dir.join("cron.jsonl");
        tokio::fs::create_dir_all(&dir).await.unwrap();

        let job = make_job("j1", "valid");
        let valid_line =
            serde_json::to_string(&StorageEvent::Create { job: Box::new(job) }).unwrap();
        let content = format!("{valid_line}\nthis is garbage\n{{\n");
        tokio::fs::write(&path, content).await.unwrap();

        let storage = CronStorage::new(path);
        let jobs = storage.load_jobs().await.unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].name, "valid");

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn empty_file_returns_empty_list() {
        let dir = std::env::temp_dir().join(format!("clawft-test-{}", uuid::Uuid::new_v4()));
        let path = dir.join("cron.jsonl");
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(&path, "").await.unwrap();

        let storage = CronStorage::new(path);
        let jobs = storage.load_jobs().await.unwrap();
        assert!(jobs.is_empty());

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn nonexistent_file_returns_empty_list() {
        let path = std::env::temp_dir().join(format!(
            "clawft-test-nonexistent-{}.jsonl",
            uuid::Uuid::new_v4()
        ));
        let storage = CronStorage::new(path);
        let jobs = storage.load_jobs().await.unwrap();
        assert!(jobs.is_empty());
    }

    #[tokio::test]
    async fn update_message_field() {
        let dir = std::env::temp_dir().join(format!("clawft-test-{}", uuid::Uuid::new_v4()));
        let path = dir.join("cron.jsonl");
        let storage = CronStorage::new(path);

        storage
            .append_create(&make_job("j1", "test"))
            .await
            .unwrap();
        storage
            .append_update("j1", "message", &serde_json::json!("new prompt"))
            .await
            .unwrap();

        let jobs = storage.load_jobs().await.unwrap();
        assert_eq!(jobs[0].payload.message, "new prompt");

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    // -- Synchronous API tests --

    #[test]
    fn sync_append_and_load() {
        let dir = std::env::temp_dir().join(format!("clawft-sync-test-{}", uuid::Uuid::new_v4()));
        let path = dir.join("cron.jsonl");

        let job = make_job("s1", "sync-test");
        append_create_sync(&path, &job).unwrap();

        let jobs = load_jobs_sync(&path).unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].id, "s1");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sync_delete() {
        let dir = std::env::temp_dir().join(format!("clawft-sync-test-{}", uuid::Uuid::new_v4()));
        let path = dir.join("cron.jsonl");

        append_create_sync(&path, &make_job("s1", "a")).unwrap();
        append_create_sync(&path, &make_job("s2", "b")).unwrap();
        append_delete_sync(&path, "s1").unwrap();

        let jobs = load_jobs_sync(&path).unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].id, "s2");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sync_update() {
        let dir = std::env::temp_dir().join(format!("clawft-sync-test-{}", uuid::Uuid::new_v4()));
        let path = dir.join("cron.jsonl");

        append_create_sync(&path, &make_job("s1", "test")).unwrap();
        append_update_sync(&path, "s1", "enabled", &serde_json::json!(false)).unwrap();

        let jobs = load_jobs_sync(&path).unwrap();
        assert!(!jobs[0].enabled);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sync_nonexistent_returns_empty() {
        let path = std::env::temp_dir().join(format!(
            "clawft-sync-nonexistent-{}.jsonl",
            uuid::Uuid::new_v4()
        ));
        let jobs = load_jobs_sync(&path).unwrap();
        assert!(jobs.is_empty());
    }
}
