//! DownloadJob — one ROM download that has not finished yet.
//!
//! The durable half of the download queue: a row exists exactly while a ROM's
//! download is queued, transferring, extracting, paused by the user, or
//! interrupted by a transient failure. Finished downloads (completed,
//! cancelled, failed) have no job. The partial transfer file at `tmp_path` is
//! the progress record; the job only changes on status transitions.

/// Why a job is `interrupted`.
pub const INTERRUPT_SERVER_UNREACHABLE: &str = "server_unreachable";
pub const INTERRUPT_AUTH_FAILED: &str = "auth_failed";
pub const INTERRUPT_DISK_FULL: &str = "disk_full";
pub const INTERRUPT_RESTART: &str = "restart";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    Queued,
    Downloading,
    Extracting,
    Paused,
    Interrupted,
}

impl JobStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Downloading => "downloading",
            Self::Extracting => "extracting",
            Self::Paused => "paused",
            Self::Interrupted => "interrupted",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "queued" => Self::Queued,
            "downloading" => Self::Downloading,
            "extracting" => Self::Extracting,
            "paused" => Self::Paused,
            "interrupted" => Self::Interrupted,
            _ => return None,
        })
    }

    /// Stopped but unfinished: no transfer runs and the partial is kept.
    pub fn is_resumable(self) -> bool {
        matches!(self, Self::Paused | Self::Interrupted)
    }
}

/// A verb applied to a job in a status that does not allow it.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum JobError {
    #[error("cannot {verb} a download job in status {status:?}")]
    InvalidTransition { verb: &'static str, status: &'static str },
    #[error("{0}")]
    Invalid(String),
}

/// What [`DownloadJob::enqueue`] needs.
#[derive(Debug, Clone)]
pub struct NewJob {
    pub rom_id: i64,
    pub rom_name: String,
    pub platform_name: String,
    pub file_name: String,
    pub target_path: String,
    pub tmp_path: String,
    pub is_multi: bool,
    pub position: i64,
    pub total_bytes: Option<u64>,
}

/// One unfinished ROM download, keyed by `rom_id`, ordered by `position`.
#[derive(Debug, Clone, PartialEq)]
pub struct DownloadJob {
    pub rom_id: i64,
    pub rom_name: String,
    pub platform_name: String,
    pub file_name: String,
    pub target_path: String,
    pub tmp_path: String,
    pub is_multi: bool,
    pub status: JobStatus,
    pub position: i64,
    pub created_at: String,
    pub updated_at: String,
    pub total_bytes: Option<u64>,
    pub interrupt_reason: Option<String>,
    pub range_supported: Option<bool>,
}

impl DownloadJob {
    /// Register a new download waiting for its turn at `position`.
    pub fn enqueue(new: NewJob, now: &str) -> Result<Self, JobError> {
        if new.rom_id <= 0 {
            return Err(JobError::Invalid(format!("rom_id must be positive, got {}", new.rom_id)));
        }
        if new.target_path.is_empty() || new.tmp_path.is_empty() {
            return Err(JobError::Invalid("target_path and tmp_path are required".into()));
        }
        if new.tmp_path == new.target_path {
            return Err(JobError::Invalid("tmp_path must differ from target_path".into()));
        }
        Ok(Self {
            rom_id: new.rom_id,
            rom_name: new.rom_name,
            platform_name: new.platform_name,
            file_name: new.file_name,
            target_path: new.target_path,
            tmp_path: new.tmp_path,
            is_multi: new.is_multi,
            status: JobStatus::Queued,
            position: new.position,
            created_at: now.to_string(),
            updated_at: now.to_string(),
            total_bytes: new.total_bytes.filter(|t| *t > 0),
            interrupt_reason: None,
            range_supported: None,
        })
    }

    fn require(&self, verb: &'static str, allowed: &[JobStatus]) -> Result<(), JobError> {
        if allowed.contains(&self.status) {
            Ok(())
        } else {
            Err(JobError::InvalidTransition {
                verb,
                status: self.status.as_str(),
            })
        }
    }

    /// The scheduler handed this job the transfer slot.
    pub fn start(&mut self, now: &str) -> Result<(), JobError> {
        self.require("start", &[JobStatus::Queued])?;
        self.status = JobStatus::Downloading;
        self.interrupt_reason = None;
        self.updated_at = now.into();
        Ok(())
    }

    /// The archive transfer finished; extraction began.
    pub fn begin_extract(&mut self, now: &str) -> Result<(), JobError> {
        self.require("begin extraction of", &[JobStatus::Downloading])?;
        self.status = JobStatus::Extracting;
        self.updated_at = now.into();
        Ok(())
    }

    /// The user paused the job; it never resumes on its own.
    pub fn pause(&mut self, now: &str) -> Result<(), JobError> {
        self.require("pause", &[JobStatus::Queued, JobStatus::Downloading])?;
        self.status = JobStatus::Paused;
        self.interrupt_reason = None;
        self.updated_at = now.into();
        Ok(())
    }

    /// A transient failure stopped the job; the partial is kept.
    pub fn interrupt(&mut self, reason: &str, now: &str) -> Result<(), JobError> {
        if reason.is_empty() {
            return Err(JobError::Invalid("an interrupt needs a reason".into()));
        }
        self.status = JobStatus::Interrupted;
        self.interrupt_reason = Some(reason.into());
        self.updated_at = now.into();
        Ok(())
    }

    /// A paused or interrupted job goes back in line, keeping its position.
    pub fn requeue(&mut self, now: &str) -> Result<(), JobError> {
        self.require("requeue", &[JobStatus::Paused, JobStatus::Interrupted])?;
        self.status = JobStatus::Queued;
        self.interrupt_reason = None;
        self.updated_at = now.into();
        Ok(())
    }

    /// Reorder the job to `position` (a value below every other job's).
    pub fn move_to_front(&mut self, position: i64, now: &str) {
        self.position = position;
        self.updated_at = now.into();
    }

    /// Record what the server revealed about the transfer.
    pub fn observe_transfer(&mut self, total_bytes: Option<u64>, range_supported: Option<bool>, now: &str) {
        if let Some(total) = total_bytes.filter(|t| *t > 0) {
            self.total_bytes = Some(total);
        }
        if range_supported.is_some() {
            self.range_supported = range_supported;
        }
        self.updated_at = now.into();
    }

    /// Startup verdict: anything not user-paused becomes `interrupted`
    /// (reason `restart`); an interrupted job keeps its original reason.
    pub fn reconcile_after_restart(&mut self, now: &str) {
        if self.status == JobStatus::Paused {
            return;
        }
        if self.status == JobStatus::Interrupted && self.interrupt_reason.as_deref().is_some_and(|r| !r.is_empty()) {
            return;
        }
        self.status = JobStatus::Interrupted;
        self.interrupt_reason = Some(INTERRUPT_RESTART.into());
        self.updated_at = now.into();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: &str = "2026-01-01T00:00:00+00:00";
    const T1: &str = "2026-01-01T00:01:00+00:00";

    fn new_job() -> NewJob {
        NewJob {
            rom_id: 7,
            rom_name: "Zelda".into(),
            platform_name: "Nintendo 64".into(),
            file_name: "zelda.z64".into(),
            target_path: "/roms/n64/zelda.z64".into(),
            tmp_path: "/roms/n64/zelda.z64.tmp".into(),
            is_multi: false,
            position: 3,
            total_bytes: Some(1024),
        }
    }

    fn job() -> DownloadJob {
        DownloadJob::enqueue(new_job(), T0).unwrap()
    }

    fn in_status(status: JobStatus) -> DownloadJob {
        let mut j = job();
        match status {
            JobStatus::Queued => {}
            JobStatus::Paused => j.pause(T0).unwrap(),
            JobStatus::Interrupted => j.interrupt(INTERRUPT_SERVER_UNREACHABLE, T0).unwrap(),
            JobStatus::Downloading => j.start(T0).unwrap(),
            JobStatus::Extracting => {
                j.start(T0).unwrap();
                j.begin_extract(T0).unwrap();
            }
        }
        j
    }

    const ALL: [JobStatus; 5] = [
        JobStatus::Queued,
        JobStatus::Downloading,
        JobStatus::Extracting,
        JobStatus::Paused,
        JobStatus::Interrupted,
    ];

    #[test]
    fn enqueue_new_job() {
        let j = job();
        assert_eq!(j.status, JobStatus::Queued);
        assert_eq!(j.position, 3);
        assert_eq!(j.created_at, T0);
        assert_eq!(j.updated_at, T0);
        assert_eq!(j.interrupt_reason, None);
        assert_eq!(j.range_supported, None);
        assert_eq!(j.total_bytes, Some(1024));
    }

    #[test]
    fn enqueue_validation() {
        let mut n = new_job();
        n.total_bytes = Some(0);
        assert_eq!(DownloadJob::enqueue(n, T0).unwrap().total_bytes, None);
        for id in [0, -1] {
            let mut n = new_job();
            n.rom_id = id;
            assert!(DownloadJob::enqueue(n, T0).unwrap_err().to_string().contains("rom_id"));
        }
        let mut n = new_job();
        n.tmp_path = String::new();
        assert!(DownloadJob::enqueue(n, T0).is_err());
        let mut n = new_job();
        n.tmp_path = n.target_path.clone();
        assert!(DownloadJob::enqueue(n, T0).is_err());
    }

    #[test]
    fn start_and_extract() {
        let mut j = job();
        j.start(T1).unwrap();
        assert_eq!(j.status, JobStatus::Downloading);
        assert_eq!(j.updated_at, T1);
        for s in [JobStatus::Downloading, JobStatus::Extracting, JobStatus::Paused, JobStatus::Interrupted] {
            assert!(matches!(in_status(s).start(T1), Err(JobError::InvalidTransition { .. })));
        }
        let mut j = in_status(JobStatus::Downloading);
        j.begin_extract(T1).unwrap();
        assert_eq!(j.status, JobStatus::Extracting);
        for s in [JobStatus::Queued, JobStatus::Extracting, JobStatus::Paused, JobStatus::Interrupted] {
            assert!(in_status(s).begin_extract(T1).is_err());
        }
    }

    #[test]
    fn pause_rules() {
        for s in [JobStatus::Queued, JobStatus::Downloading] {
            let mut j = in_status(s);
            j.pause(T1).unwrap();
            assert_eq!(j.status, JobStatus::Paused);
            assert_eq!(j.interrupt_reason, None);
        }
        for s in [JobStatus::Extracting, JobStatus::Paused, JobStatus::Interrupted] {
            assert!(in_status(s).pause(T1).is_err());
        }
    }

    #[test]
    fn interrupt_from_any_status() {
        for s in ALL {
            let mut j = in_status(s);
            j.interrupt(INTERRUPT_DISK_FULL, T1).unwrap();
            assert_eq!(j.status, JobStatus::Interrupted);
            assert_eq!(j.interrupt_reason.as_deref(), Some(INTERRUPT_DISK_FULL));
            assert_eq!(j.updated_at, T1);
        }
        assert!(job().interrupt("", T1).is_err());
    }

    #[test]
    fn requeue_rules() {
        for s in [JobStatus::Paused, JobStatus::Interrupted] {
            let mut j = in_status(s);
            j.requeue(T1).unwrap();
            assert_eq!(j.status, JobStatus::Queued);
            assert_eq!(j.position, 3);
            assert_eq!(j.interrupt_reason, None);
        }
        for s in [JobStatus::Queued, JobStatus::Downloading, JobStatus::Extracting] {
            assert!(in_status(s).requeue(T1).is_err());
        }
        let mut j = in_status(JobStatus::Interrupted);
        j.requeue(T1).unwrap();
        j.start(T1).unwrap();
        assert_eq!(j.interrupt_reason, None);
    }

    #[test]
    fn move_to_front_and_observe() {
        let mut j = job();
        j.move_to_front(-5, T1);
        assert_eq!((j.position, j.updated_at.as_str()), (-5, T1));
        let mut n = new_job();
        n.total_bytes = None;
        let mut j = DownloadJob::enqueue(n, T0).unwrap();
        j.observe_transfer(Some(4096), Some(true), T1);
        assert_eq!((j.total_bytes, j.range_supported), (Some(4096), Some(true)));
        let mut j = job();
        j.observe_transfer(Some(2048), Some(false), T0);
        j.observe_transfer(Some(0), None, T1);
        assert_eq!((j.total_bytes, j.range_supported), (Some(2048), Some(false)));
    }

    #[test]
    fn reconcile_after_restart() {
        for s in [JobStatus::Queued, JobStatus::Downloading, JobStatus::Extracting] {
            let mut j = in_status(s);
            j.reconcile_after_restart(T1);
            assert_eq!(j.status, JobStatus::Interrupted);
            assert_eq!(j.interrupt_reason.as_deref(), Some(INTERRUPT_RESTART));
        }
        let mut j = in_status(JobStatus::Interrupted);
        j.reconcile_after_restart(T1);
        assert_eq!(j.interrupt_reason.as_deref(), Some(INTERRUPT_SERVER_UNREACHABLE));
        let mut j = in_status(JobStatus::Paused);
        j.reconcile_after_restart(T1);
        assert_eq!((j.status, j.updated_at.as_str()), (JobStatus::Paused, T0));
        assert_eq!(j.interrupt_reason, None);
    }

    #[test]
    fn status_round_trip() {
        for s in ALL {
            assert_eq!(JobStatus::parse(s.as_str()), Some(s));
        }
        assert_eq!(JobStatus::parse("done"), None);
        assert!(JobStatus::Paused.is_resumable() && JobStatus::Interrupted.is_resumable());
        assert!(!JobStatus::Queued.is_resumable());
    }
}
