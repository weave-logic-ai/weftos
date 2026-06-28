//! In-memory cron scheduler.
//!
//! Maintains a map of [`CronJob`] entries and determines which are due
//! to fire based on their `next_run_at` timestamp.
//!
//! Uses the canonical [`CronJob`] type from [`clawft_types::cron`].

use std::collections::HashMap;
use std::str::FromStr;

use chrono::{DateTime, TimeZone, Utc};
use cron::Schedule;

use crate::error::{Result, ServiceError};

// Re-export the canonical CronJob from clawft-types.
pub use clawft_types::cron::{
    CronJob, CronJobState, CronPayload, CronSchedule, CronStore, JobStatus, PayloadKind,
    ScheduleKind,
};

/// In-memory scheduler holding all jobs.
pub struct CronScheduler {
    jobs: HashMap<String, CronJob>,
}

impl CronScheduler {
    /// Create an empty scheduler.
    pub fn new() -> Self {
        Self {
            jobs: HashMap::new(),
        }
    }

    /// Add a job to the scheduler.
    ///
    /// Validates the cron expression (if the schedule kind is `Cron`)
    /// and rejects duplicate names.
    pub fn add_job(&mut self, job: CronJob) -> Result<()> {
        // Validate the cron expression if present.
        if job.schedule.kind == ScheduleKind::Cron
            && let Some(ref expr) = job.schedule.expr
        {
            Schedule::from_str(expr)
                .map_err(|e| ServiceError::InvalidCronExpression(e.to_string()))?;
        }

        // Check for duplicate names.
        if self
            .jobs
            .values()
            .any(|j| j.name == job.name && j.id != job.id)
        {
            return Err(ServiceError::DuplicateJobName(job.name.clone()));
        }

        self.jobs.insert(job.id.clone(), job);
        Ok(())
    }

    /// Remove a job by ID.
    pub fn remove_job(&mut self, job_id: &str) -> Result<()> {
        self.jobs
            .remove(job_id)
            .ok_or_else(|| ServiceError::JobNotFound(job_id.to_string()))?;
        Ok(())
    }

    /// Return all enabled jobs whose `next_run_at` is at or before now.
    pub fn get_due_jobs(&self) -> Vec<CronJob> {
        let now = Utc::now();
        self.jobs
            .values()
            .filter(|j| j.enabled && j.state.next_run_at.is_some_and(|nr| nr <= now))
            .cloned()
            .collect()
    }

    /// List all jobs.
    pub fn list_jobs(&self) -> Vec<CronJob> {
        self.jobs.values().cloned().collect()
    }

    /// Get a reference to a job by ID.
    pub fn get_job(&self, job_id: &str) -> Option<&CronJob> {
        self.jobs.get(job_id)
    }

    /// Get a mutable reference to a job by ID.
    pub fn get_job_mut(&mut self, job_id: &str) -> Option<&mut CronJob> {
        self.jobs.get_mut(job_id)
    }

    /// Record that a job has run and compute the next run time.
    pub fn update_job_run(&mut self, job_id: &str, run_time: DateTime<Utc>) -> Result<()> {
        let job = self
            .jobs
            .get_mut(job_id)
            .ok_or_else(|| ServiceError::JobNotFound(job_id.to_string()))?;

        job.state.last_run_at = Some(run_time);
        job.state.last_status = Some(JobStatus::Ok);
        job.updated_at = run_time;

        // Compute next run from the cron schedule expression.
        if job.schedule.kind == ScheduleKind::Cron
            && let Some(ref expr) = job.schedule.expr
            && let Ok(schedule) = Schedule::from_str(expr)
        {
            job.state.next_run_at = schedule
                .after(&run_time)
                .next()
                .map(|dt| dt.with_timezone(&Utc));
        }

        Ok(())
    }
}

impl Default for CronScheduler {
    fn default() -> Self {
        Self::new()
    }
}

/// Compute the next run time for a cron expression after a given time.
///
/// Returns `None` if the schedule has no further occurrences.
pub fn compute_next_run(
    schedule_expr: &str,
    after: &DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>> {
    let schedule = Schedule::from_str(schedule_expr)
        .map_err(|e| ServiceError::InvalidCronExpression(e.to_string()))?;
    Ok(schedule
        .after(after)
        .next()
        .map(|dt| dt.with_timezone(&Utc)))
}

/// Convert a millisecond timestamp to a `DateTime<Utc>`, if valid.
pub fn ms_to_datetime(ms: i64) -> Option<DateTime<Utc>> {
    Utc.timestamp_millis_opt(ms).single()
}

#[cfg(test)]
mod tests {
    use chrono::Datelike;

    use super::*;

    fn make_job(id: &str, name: &str, schedule_expr: &str) -> CronJob {
        let now = Utc::now();
        CronJob {
            id: id.into(),
            name: name.into(),
            enabled: true,
            schedule: CronSchedule {
                kind: ScheduleKind::Cron,
                at_ms: None,
                every_ms: None,
                expr: Some(schedule_expr.into()),
                tz: Some("UTC".into()),
            },
            payload: CronPayload {
                message: "test prompt".into(),
                ..Default::default()
            },
            state: CronJobState::default(),
            created_at: now,
            updated_at: now,
            delete_after_run: false,
        }
    }

    #[test]
    fn parse_valid_cron_expression() {
        // 7-field: sec min hour dom month dow year
        let result = compute_next_run("0 0 * * * * *", &Utc::now());
        assert!(result.is_ok());
        assert!(result.unwrap().is_some());
    }

    #[test]
    fn reject_invalid_cron_expression() {
        let result = compute_next_run("not a cron", &Utc::now());
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ServiceError::InvalidCronExpression(_)
        ));
    }

    #[test]
    fn add_job_and_list() {
        let mut sched = CronScheduler::new();
        let job = make_job("j1", "hourly", "0 0 * * * * *");
        sched.add_job(job).unwrap();

        let jobs = sched.list_jobs();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].name, "hourly");
    }

    #[test]
    fn add_job_with_invalid_schedule_fails() {
        let mut sched = CronScheduler::new();
        let job = make_job("j1", "bad", "not valid");
        let result = sched.add_job(job);
        assert!(result.is_err());
    }

    #[test]
    fn add_duplicate_name_fails() {
        let mut sched = CronScheduler::new();
        sched
            .add_job(make_job("j1", "hourly", "0 0 * * * * *"))
            .unwrap();
        let result = sched.add_job(make_job("j2", "hourly", "0 0 * * * * *"));
        assert!(matches!(
            result.unwrap_err(),
            ServiceError::DuplicateJobName(_)
        ));
    }

    #[test]
    fn remove_job() {
        let mut sched = CronScheduler::new();
        sched
            .add_job(make_job("j1", "hourly", "0 0 * * * * *"))
            .unwrap();
        assert!(sched.remove_job("j1").is_ok());
        assert!(sched.list_jobs().is_empty());
    }

    #[test]
    fn remove_nonexistent_job_fails() {
        let mut sched = CronScheduler::new();
        let result = sched.remove_job("nope");
        assert!(matches!(result.unwrap_err(), ServiceError::JobNotFound(_)));
    }

    #[test]
    fn get_due_jobs_returns_past_jobs() {
        let mut sched = CronScheduler::new();
        let mut job = make_job("j1", "past", "0 0 * * * * *");
        // Set next_run in the past (2020-01-01 00:00:00 UTC).
        job.state.next_run_at = Some(Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap());
        sched.add_job(job).unwrap();

        let due = sched.get_due_jobs();
        assert_eq!(due.len(), 1);
    }

    #[test]
    fn no_due_jobs_when_all_in_future() {
        let mut sched = CronScheduler::new();
        let mut job = make_job("j1", "future", "0 0 * * * * *");
        job.state.next_run_at = Some(Utc.with_ymd_and_hms(2099, 12, 31, 23, 59, 59).unwrap());
        sched.add_job(job).unwrap();

        let due = sched.get_due_jobs();
        assert!(due.is_empty());
    }

    #[test]
    fn disabled_jobs_not_due() {
        let mut sched = CronScheduler::new();
        let mut job = make_job("j1", "disabled", "0 0 * * * * *");
        job.state.next_run_at = Some(Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap());
        job.enabled = false;
        sched.add_job(job).unwrap();

        let due = sched.get_due_jobs();
        assert!(due.is_empty());
    }

    #[test]
    fn jobs_without_next_run_not_due() {
        let mut sched = CronScheduler::new();
        let job = make_job("j1", "no-next", "0 0 * * * * *");
        // next_run_at is None by default.
        sched.add_job(job).unwrap();

        let due = sched.get_due_jobs();
        assert!(due.is_empty());
    }

    #[test]
    fn update_job_run_sets_last_and_next() {
        let mut sched = CronScheduler::new();
        sched
            .add_job(make_job("j1", "hourly", "0 0 * * * * *"))
            .unwrap();

        let run_time = Utc::now();
        sched.update_job_run("j1", run_time).unwrap();

        let job = sched.get_job("j1").unwrap();
        assert_eq!(job.state.last_run_at, Some(run_time));
        assert!(job.state.next_run_at.is_some());
        // next_run must be after run_time.
        assert!(job.state.next_run_at.unwrap() > run_time);
    }

    #[test]
    fn update_nonexistent_job_fails() {
        let mut sched = CronScheduler::new();
        let result = sched.update_job_run("nope", Utc::now());
        assert!(matches!(result.unwrap_err(), ServiceError::JobNotFound(_)));
    }

    #[test]
    fn get_job_by_id() {
        let mut sched = CronScheduler::new();
        sched
            .add_job(make_job("j1", "test", "0 0 * * * * *"))
            .unwrap();
        assert!(sched.get_job("j1").is_some());
        assert!(sched.get_job("nope").is_none());
    }

    #[test]
    fn default_creates_empty_scheduler() {
        let sched = CronScheduler::default();
        assert!(sched.list_jobs().is_empty());
    }

    #[test]
    fn ms_to_datetime_valid() {
        let dt = ms_to_datetime(1_700_000_000_000);
        assert!(dt.is_some());
        assert_eq!(dt.unwrap().year(), 2023);
    }

    #[test]
    fn update_job_sets_status() {
        let mut sched = CronScheduler::new();
        sched
            .add_job(make_job("j1", "status-test", "0 0 * * * * *"))
            .unwrap();

        let run_time = Utc::now();
        sched.update_job_run("j1", run_time).unwrap();

        let job = sched.get_job("j1").unwrap();
        assert_eq!(job.state.last_status, Some(JobStatus::Ok));
    }
}
