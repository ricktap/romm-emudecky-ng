//! One sync operation. IDs and timestamps are supplied by the caller.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SyncRunStatus {
    Running,
    Completed,
    Cancelled,
    Errored,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncRun {
    pub id: String,
    pub started_at: String,
    pub status: SyncRunStatus,
    pub platforms_planned: i64,
    pub roms_planned: i64,
    pub finished_at: Option<String>,
    pub platforms_completed: Option<Vec<String>>,
    pub collections_completed: Option<Vec<String>>,
    pub error: Option<String>,
}

impl SyncRun {
    pub fn start(id: String, at: String, platforms_planned: i64, roms_planned: i64) -> Result<Self, String> {
        if id.is_empty() { return Err("id is required".into()); }
        if platforms_planned < 0 { return Err("platforms_planned must be non-negative".into()); }
        if roms_planned < 0 { return Err("roms_planned must be non-negative".into()); }
        Ok(Self {
            id, started_at: at, status: SyncRunStatus::Running, platforms_planned, roms_planned,
            finished_at: None, platforms_completed: None, collections_completed: None, error: None,
        })
    }

    pub fn complete(&mut self, at: String, platforms: Vec<String>, collections: Vec<String>) -> Result<(), String> {
        self.require_running()?;
        self.status = SyncRunStatus::Completed;
        self.finished_at = Some(at);
        self.platforms_completed = Some(platforms);
        self.collections_completed = Some(collections);
        Ok(())
    }

    pub fn mark_cancelled(&mut self, at: String, reason: String) -> Result<(), String> {
        self.require_running()?;
        self.status = SyncRunStatus::Cancelled;
        self.finished_at = Some(at);
        self.error = Some(reason);
        Ok(())
    }

    pub fn mark_errored(&mut self, at: String, error: String) -> Result<(), String> {
        self.require_running()?;
        self.status = SyncRunStatus::Errored;
        self.finished_at = Some(at);
        self.error = Some(error);
        Ok(())
    }

    fn require_running(&self) -> Result<(), String> {
        if self.status != SyncRunStatus::Running { return Err("run is not running".into()); }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running() -> SyncRun { SyncRun::start("run-1".into(), "start".into(), 3, 120).unwrap() }

    #[test]
    fn start_records_plan_and_leaves_outcome_unset() {
        assert_eq!(serde_json::to_value(running()).unwrap(), serde_json::json!({
            "id": "run-1", "started_at": "start", "status": "running",
            "platforms_planned": 3, "roms_planned": 120, "finished_at": null,
            "platforms_completed": null, "collections_completed": null, "error": null,
        }));
        assert!(SyncRun::start("zero".into(), "t".into(), 0, 0).is_ok());
        for (id, platforms, roms, message) in [
            ("", 0, 0, "id is required"),
            ("a", -1, 0, "platforms_planned must be non-negative"),
            ("a", 0, -1, "roms_planned must be non-negative"),
        ] {
            assert_eq!(SyncRun::start(id.into(), "t".into(), platforms, roms).unwrap_err(), message);
        }
    }

    #[test]
    fn completion_records_even_empty_lists() {
        for platforms in [vec![], vec!["switch".into()]] {
            let mut run = running();
            run.complete("end".into(), platforms.clone(), vec!["Favorites".into()]).unwrap();
            assert_eq!(run.status, SyncRunStatus::Completed);
            assert_eq!(run.finished_at.as_deref(), Some("end"));
            assert_eq!(run.platforms_completed, Some(platforms));
            assert_eq!(run.collections_completed, Some(vec!["Favorites".into()]));
            assert_eq!(run.error, None);
        }
    }

    #[test]
    fn terminal_transitions_are_irreversible_and_failed_mutations_are_noops() {
        for status in [SyncRunStatus::Completed, SyncRunStatus::Cancelled, SyncRunStatus::Errored] {
            let mut run = running();
            match status {
                SyncRunStatus::Completed => run.complete("end".into(), vec![], vec![]),
                SyncRunStatus::Cancelled => run.mark_cancelled("end".into(), "user aborted".into()),
                _ => run.mark_errored("end".into(), "offline".into()),
            }.unwrap();
            assert_eq!(run.status, status);
            assert_eq!(run.finished_at.as_deref(), Some("end"));
            if status != SyncRunStatus::Completed {
                assert_eq!(run.platforms_completed, None);
                assert_eq!(run.collections_completed, None);
                assert_eq!(run.error.as_deref(), Some(if status == SyncRunStatus::Cancelled { "user aborted" } else { "offline" }));
            }
            let before = run.clone();
            assert_eq!(run.complete("later".into(), vec![], vec![]).unwrap_err(), "run is not running");
            assert_eq!(run.mark_cancelled("later".into(), "x".into()).unwrap_err(), "run is not running");
            assert_eq!(run.mark_errored("later".into(), "x".into()).unwrap_err(), "run is not running");
            assert_eq!(run, before);
        }
    }
}
