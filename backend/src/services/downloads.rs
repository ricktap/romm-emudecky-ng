//! DownloadService — ROM download orchestration.
//!
//! Owns every step between a frontend download request and a ROM landing on
//! disk: the durable download queue (`download_jobs` rows that survive a
//! restart; rows change on status transitions only, the partial file's size
//! is the progress), the one-at-a-time scheduler ordered by queue position,
//! the disk-space pre-flight, single-file and multi-file (ZIP → folder)
//! downloads, failure classification (`interrupted` keeps the partial,
//! `failed` deletes it), startup reconciliation with auto-resume, and the
//! sibling-version supersede. The launch command baked on completion comes
//! from the shared [`LaunchService`].

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use indexmap::IndexMap;
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::{Mutex as AsyncMutex, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::domain::archive_transfer::{ArchiveIdentity, ResumePlan, plan_resume};
use crate::domain::download_failure::{ArchiveRecoveryNeeded, Outcome, TransferCancelled, classify_download_failure};
use crate::domain::download_job::{
    DownloadJob, INTERRUPT_AUTH_FAILED, INTERRUPT_DISK_FULL, INTERRUPT_NEEDS_RECOVERY, INTERRUPT_SERVER_UNREACHABLE,
    INTERRUPT_TIMEOUT, JobError, JobKey, JobKind, JobStatus, NewJob,
};
use crate::domain::errors::{failure, reason};
use crate::domain::install_layout::resolve_download_names;
use crate::domain::rom_files::{detect_launch_file, is_multi_file_download};
use crate::domain::rom_install::RomInstall;
use crate::services::launch::LaunchService;
use crate::services::ports::{
    Clock, DownloadFiles, DownloadJobRepo, EmuDeckPaths, EventSink, IdentitySink, LibraryRepo, RommRomApi,
    SystemResolver, TransferHooks, TransferMode, TransferOutcome, TransferPhase, FirmwareFileKind, FirmwareJobs,
    FirmwareTarget, RommFirmwareApi,
};
use crate::services::rom_removal::RomRemovalService;
use crate::util::path_safety::{basename, join, safe_join_lexical};

const MAX_TERMINAL: usize = 50;
/// Shown as the platform of a firmware download in the queue.
const FIRMWARE_LABEL: &str = "Switch firmware";
const ZIP_TMP_EXT: &str = ".zip.tmp";
const TMP_EXT: &str = ".tmp";
/// Free space kept on top of what a download needs.
const SPACE_BUFFER: u64 = 100 * 1024 * 1024;
const EMIT_INTERVAL_S: f64 = 0.5;
const LOG_INTERVAL_S: f64 = 30.0;
/// How long shutdown waits for running transfers to flush and stop.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

fn dirname(path: &str) -> &str {
    path.rfind('/').map_or("", |i| if i == 0 { "/" } else { &path[..i] })
}

/// One row of the in-memory queue the frontend reads (`get_download_queue`)
/// and the `download_progress` frames mirror.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct QueueEntry {
    /// What this download is; with `item_id` it is the entry's identity. Both are written into
    /// the frames by [`QueueEntry::identity`] (`rom_id` for a ROM, `firmware_id` for firmware).
    #[serde(skip)]
    pub kind: JobKind,
    #[serde(skip)]
    pub item_id: i64,
    pub rom_name: String,
    pub platform_name: String,
    pub file_name: String,
    pub status: String,
    pub progress: f64,
    pub bytes_downloaded: u64,
    pub total_bytes: u64,
    pub resumable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Live transfer phase while the status is `downloading`: `waiting` (request
    /// out, no response headers yet) or `retrying` (backing off). `None` while
    /// bytes flow. Never persisted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    /// Epoch seconds when the current phase began (the UI counts elapsed time locally).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase_since: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempt: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_attempts: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_delay_s: Option<f64>,
}

impl QueueEntry {
    pub fn key(&self) -> JobKey {
        JobKey { kind: self.kind, id: self.item_id }
    }

    /// The identity fields every frame and queue row carries: `kind`, plus the id under the name its
    /// consumers already use (`rom_id` for a ROM; `firmware_id` for firmware).
    fn identity(&self, v: &mut Value) {
        set_identity(v, self.key());
    }

    /// The row `get_download_queue` returns.
    fn row(&self) -> Value {
        let mut v = serde_json::to_value(self).unwrap_or(Value::Null);
        self.identity(&mut v);
        v
    }

    fn frame(&self) -> Value {
        let mut v = json!({
            "rom_name": self.rom_name,
            "platform_name": self.platform_name,
            "file_name": self.file_name,
            "status": self.status,
            "progress": self.progress,
            "bytes_downloaded": self.bytes_downloaded,
            "total_bytes": self.total_bytes,
            "resumable": self.resumable,
        });
        self.identity(&mut v);
        if let Some(reason) = self.reason.as_deref().filter(|r| !r.is_empty()) {
            v["reason"] = json!(reason);
        }
        if self.status == "downloading"
            && let Some(phase) = &self.phase
        {
            v["phase"] = json!(phase);
            v["phase_since"] = json!(self.phase_since);
            v["attempt"] = json!(self.attempt);
            v["max_attempts"] = json!(self.max_attempts);
            if let Some(delay) = self.retry_delay_s {
                v["retry_delay_s"] = json!(delay);
            }
        }
        v
    }

    fn clear_phase(&mut self) {
        self.phase = None;
        self.phase_since = None;
        self.attempt = None;
        self.max_attempts = None;
        self.retry_delay_s = None;
    }
}

/// Write a job's identity into a frame: `kind`, plus the id under the name its consumers use
/// (`rom_id` for a ROM, `firmware_id` for firmware).
fn set_identity(v: &mut Value, key: JobKey) {
    v["kind"] = json!(key.kind.as_str());
    match key.kind {
        JobKind::Rom => v["rom_id"] = json!(key.id),
        JobKind::Firmware => v["firmware_id"] = json!(key.id),
    }
}

/// Live transfer frames never carry the interrupt reason.
fn strip_reason(frame: &mut Value) {
    if let Some(obj) = frame.as_object_mut() {
        obj.shift_remove("reason");
    }
}

fn ratio(done: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        (done as f64 / total as f64).min(1.0)
    }
}

/// Cooperative stop flags of one download attempt. A cancel deletes the
/// partial, a pause keeps it, a shutdown keeps the partial and the row.
struct Control {
    id: u64,
    token: CancellationToken,
    cancelled: AtomicBool,
    paused: AtomicBool,
    shutdown: AtomicBool,
}

impl Control {
    fn stop(&self, flag: &AtomicBool) {
        flag.store(true, Ordering::SeqCst);
        self.token.cancel();
    }
}

struct Attempt {
    control: Arc<Control>,
    handle: Option<JoinHandle<()>>,
}

struct Waiter {
    key: JobKey,
    control_id: u64,
    tx: oneshot::Sender<()>,
}

#[derive(Default)]
struct State {
    in_progress: HashSet<JobKey>,
    queue: IndexMap<JobKey, QueueEntry>,
    attempts: HashMap<JobKey, Attempt>,
    /// Control id holding the single transfer slot.
    active: Option<u64>,
    waiters: Vec<Waiter>,
    /// Mirror of each persisted job's queue position (the scheduler never reads SQLite).
    positions: HashMap<JobKey, i64>,
    persisted_totals: HashSet<JobKey>,
    reserved: HashMap<JobKey, u64>,
    auto_resume: Option<JoinHandle<()>>,
    next_control: u64,
}

impl State {
    fn prune(&mut self) {
        let terminal: Vec<JobKey> = self
            .queue
            .iter()
            .filter(|(_, e)| matches!(e.status.as_str(), "completed" | "failed" | "cancelled"))
            .map(|(id, _)| *id)
            .collect();
        let excess = terminal.len().saturating_sub(MAX_TERMINAL);
        for id in &terminal[..excess] {
            self.queue.shift_remove(id);
        }
    }
}

/// Everything [`DownloadService`] depends on.
pub struct DownloadServiceConfig {
    pub romm: Arc<dyn RommRomApi>,
    pub files: Arc<dyn DownloadFiles>,
    pub jobs: Arc<dyn DownloadJobRepo>,
    pub library: Arc<dyn LibraryRepo>,
    pub paths: Arc<dyn EmuDeckPaths>,
    pub systems: Arc<dyn SystemResolver>,
    pub launch: Arc<LaunchService>,
    pub removal: Arc<RomRemovalService>,
    pub events: Arc<dyn EventSink>,
    pub clock: Arc<dyn Clock>,
    pub firmware_api: Arc<dyn RommFirmwareApi>,
    pub firmware_jobs: Arc<dyn FirmwareJobs>,
}

pub struct DownloadService {
    /// Serializes download admission with version switching and uninstall.
    pub mutation_lock: Arc<tokio::sync::Mutex<()>>,

    romm: Arc<dyn RommRomApi>,
    files: Arc<dyn DownloadFiles>,
    jobs: Arc<dyn DownloadJobRepo>,
    library: Arc<dyn LibraryRepo>,
    paths: Arc<dyn EmuDeckPaths>,
    systems: Arc<dyn SystemResolver>,
    launch: Arc<LaunchService>,
    removal: Arc<RomRemovalService>,
    events: Arc<dyn EventSink>,
    clock: Arc<dyn Clock>,
    firmware_api: Arc<dyn RommFirmwareApi>,
    firmware_jobs: Arc<dyn FirmwareJobs>,
    state: Mutex<State>,
    /// Serializes every read-modify-write of a `download_jobs` row.
    job_lock: AsyncMutex<()>,
}

/// How a transfer attempt ended before its terminal handling.
enum Ended {
    Committed(String),
    Stopped,
    Error(anyhow::Error),
}

/// The fixed facts of one download attempt.
#[derive(Clone)]
struct Plan {
    /// What is downloading; its id is `rom_id` for a ROM and the firmware id for firmware.
    key: JobKey,
    /// The ROM id (a firmware plan carries its firmware id here and never reads it as a ROM).
    rom_id: i64,
    /// The firmware file to fetch and where it goes; `None` for a ROM.
    firmware: Option<FirmwareTarget>,
    rom: Value,
    target_path: String,
    system: String,
    file_name: String,
    rom_name: String,
    platform_name: String,
    has_multiple: bool,
    extract_dir_name: String,
    /// The user chose to restart a partial of unknown origin.
    restart: bool,
}

impl DownloadService {
    pub fn new(config: DownloadServiceConfig) -> Arc<Self> {
        Arc::new(Self {
            mutation_lock: Arc::new(tokio::sync::Mutex::new(())),
            romm: config.romm,
            files: config.files,
            jobs: config.jobs,
            library: config.library,
            paths: config.paths,
            systems: config.systems,
            launch: config.launch,
            removal: config.removal,
            events: config.events,
            clock: config.clock,
            firmware_api: config.firmware_api,
            firmware_jobs: config.firmware_jobs,
            state: Mutex::new(State::default()),
            job_lock: AsyncMutex::new(()),
        })
    }

    fn st(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn emit(&self, event: &str, payload: Value) {
        self.events.emit(event, payload);
    }

    fn emit_entry(&self, key: JobKey) {
        let frame = self.st().queue.get(&key).map(QueueEntry::frame);
        if let Some(frame) = frame {
            self.emit("download_progress", frame);
        }
    }

    fn update_entry(&self, key: JobKey, f: impl FnOnce(&mut QueueEntry)) -> bool {
        match self.st().queue.get_mut(&key) {
            Some(entry) => {
                f(entry);
                true
            }
            None => false,
        }
    }

    // ── Durable queue (download_jobs rows) ──────────────────────────────

    async fn load_job(&self, key: JobKey) -> Option<DownloadJob> {
        match self.jobs.get(key).await {
            Ok(job) => job,
            Err(e) => {
                tracing::warn!("Failed to read download job for {key}: {e:#}");
                None
            }
        }
    }

    /// Apply one verb to the persisted job. A missing row is a no-op; an
    /// illegal transition or a DB failure is logged and swallowed —
    /// persistence never breaks the transfer itself.
    async fn mutate_job(
        &self,
        key: JobKey,
        verb: impl FnOnce(&mut DownloadJob) -> Result<(), JobError>,
    ) -> Option<DownloadJob> {
        let _guard = self.job_lock.lock().await;
        let mut job = match self.jobs.get(key).await {
            Ok(Some(job)) => job,
            Ok(None) => return None,
            Err(e) => {
                tracing::warn!("Failed to persist download job for {key}: {e:#}");
                return None;
            }
        };
        if let Err(e) = verb(&mut job) {
            tracing::warn!("Download job {key}: {e}");
            return None;
        }
        if let Err(e) = self.jobs.save(&job).await {
            tracing::warn!("Failed to persist download job for {key}: {e:#}");
            return None;
        }
        self.st().positions.insert(key, job.position);
        Some(job)
    }

    /// Persist the job as `queued`: a paused/interrupted row is requeued in
    /// place (resume never jumps the line), else it is appended.
    async fn enqueue_job(&self, new: NewJob) -> Option<DownloadJob> {
        let key = JobKey { kind: new.kind, id: new.item_id };
        let now = self.clock.now_iso();
        let _guard = self.job_lock.lock().await;
        let result: anyhow::Result<DownloadJob> = async {
            let existing = self.jobs.get(key).await?;
            let job = match existing {
                Some(mut job) if job.status.is_resumable() => {
                    job.requeue(&now)?;
                    job
                }
                other => {
                    let position = match other {
                        Some(job) => job.position,
                        None => self.jobs.next_position().await?,
                    };
                    DownloadJob::enqueue(NewJob { position, ..new }, &now)?
                }
            };
            self.jobs.save(&job).await?;
            Ok(job)
        }
        .await;
        match result {
            Ok(job) => {
                self.st().positions.insert(key, job.position);
                Some(job)
            }
            Err(e) => {
                tracing::warn!("Failed to persist queued download job for {key}: {e:#}");
                None
            }
        }
    }

    /// Delete the job row (completed / cancelled / failed / evicted).
    async fn drop_job(&self, key: JobKey) {
        {
            let mut st = self.st();
            st.positions.remove(&key);
            st.persisted_totals.remove(&key);
        }
        let _guard = self.job_lock.lock().await;
        if let Err(e) = self.jobs.delete(key).await {
            tracing::warn!("Failed to delete download job for {key}: {e:#}");
        }
    }

    /// Record the transfer's full size + Range verdict once per attempt.
    async fn persist_transfer_observation(&self, key: JobKey, total: u64) {
        let resumable = {
            let mut st = self.st();
            if total == 0 || !st.persisted_totals.insert(key) {
                return;
            }
            st.queue.get(&key).map(|e| e.resumable)
        };
        let now = self.clock.now_iso();
        self.mutate_job(key, |job| {
            job.observe_transfer(Some(total), resumable, &now);
            Ok(())
        })
        .await;
    }

    /// Move an unfinished download to the head of the queue: it gets the
    /// transfer slot next (a running transfer is not pre-empted).
    pub async fn move_to_front(&self, key: JobKey) -> Value {
        let now = self.clock.now_iso();
        let _guard = self.job_lock.lock().await;
        let result: anyhow::Result<Option<i64>> = async {
            let Some(mut job) = self.jobs.get(key).await? else {
                return Ok(None);
            };
            if let Some(lowest) = self.jobs.min_position().await?
                && lowest < job.position
            {
                job.move_to_front(lowest - 1, &now);
                self.jobs.save(&job).await?;
            }
            Ok(Some(job.position))
        }
        .await;
        match result {
            Ok(Some(position)) => {
                self.st().positions.insert(key, position);
                json!({"success": true, "message": "Moved to front"})
            }
            Ok(None) => failure("not_queued", "No queued download for this ROM"),
            Err(e) => {
                tracing::warn!("Failed to move download {key:?} to the front: {e:#}");
                failure(reason::UNKNOWN, "Failed to reorder the queue")
            }
        }
    }

    // ── Transfer scheduler ──────────────────────────────────────────────

    /// Wait for the single transfer slot. Returns `false` when the attempt
    /// was stopped while waiting (the slot is released again if granted).
    async fn acquire_slot(&self, key: JobKey, control: &Control) -> bool {
        let rx = {
            let mut st = self.st();
            if st.active.is_none() {
                st.active = Some(control.id);
                return true;
            }
            let (tx, rx) = oneshot::channel();
            st.waiters.push(Waiter {
                key,
                control_id: control.id,
                tx,
            });
            rx
        };
        tokio::select! {
            biased;
            granted = rx => if granted.is_ok() { return true; },
            () = control.token.cancelled() => {}
        }
        self.st().waiters.retain(|w| w.control_id != control.id);
        self.release_slot(control.id);
        false
    }

    /// Release the slot if `control_id` holds it and hand it to the live
    /// waiter with the lowest job position (FIFO on ties).
    fn release_slot(&self, control_id: u64) {
        let mut st = self.st();
        if st.active != Some(control_id) {
            return;
        }
        st.active = None;
        loop {
            st.waiters.retain(|w| !w.tx.is_closed());
            let chosen = st
                .waiters
                .iter()
                .enumerate()
                .min_by_key(|(i, w)| (st.positions.get(&w.key).copied().unwrap_or(i64::MAX), *i))
                .map(|(i, _)| i);
            let Some(index) = chosen else {
                return;
            };
            let waiter = st.waiters.remove(index);
            st.active = Some(waiter.control_id);
            if waiter.tx.send(()).is_ok() {
                return;
            }
            st.active = None;
        }
    }

    // ── Startup reconciliation ──────────────────────────────────────────

    /// Restore the durable queue after a (re)start: every job keeps its
    /// partial (its real size is the resume point); anything not user-paused
    /// becomes `interrupted` (reason `restart` unless already set); the
    /// in-memory queue is rebuilt; `.tmp` / `.zip.tmp` files of no job (and
    /// every BIOS `.tmp`) are deleted — but no ROM partial is deleted when the
    /// job table cannot be read. Interrupted jobs then auto-resume in queue
    /// order in the background.
    pub async fn reconcile_persisted_downloads(self: &Arc<Self>) {
        let now = self.clock.now_iso();
        let loaded: anyhow::Result<Vec<DownloadJob>> = async {
            let _guard = self.job_lock.lock().await;
            let mut jobs = self.jobs.all().await?;
            for job in &mut jobs {
                job.reconcile_after_restart(&now);
                self.jobs.save(job).await?;
            }
            Ok(jobs)
        }
        .await;
        let (jobs, jobs_loaded) = match loaded {
            Ok(jobs) => (jobs, true),
            Err(e) => {
                tracing::error!("Failed to load persisted downloads: {e:#}");
                (Vec::new(), false)
            }
        };
        let mut keep = HashSet::new();
        for job in &jobs {
            keep.insert(job.tmp_path.clone());
            let on_disk = self.files.file_size(&job.tmp_path).await;
            let total = job.total_bytes.unwrap_or(0);
            let entry = QueueEntry {
                kind: job.kind,
                item_id: job.item_id,
                rom_name: job.rom_name.clone(),
                platform_name: job.platform_name.clone(),
                file_name: job.file_name.clone(),
                status: job.status.as_str().into(),
                progress: ratio(on_disk, total),
                bytes_downloaded: on_disk,
                total_bytes: total,
                resumable: job.range_supported.unwrap_or(false),
                reason: job.interrupt_reason.clone().filter(|r| !r.is_empty()),
                error: None,
                ..QueueEntry::default()
            };
            let mut st = self.st();
            st.positions.insert(job.key(), job.position);
            st.queue.insert(job.key(), entry);
        }
        let rom_cleaned = if jobs_loaded {
            self.clean_orphan_rom_tmp_files(&keep).await
        } else {
            0
        };
        let cleaned = rom_cleaned + self.clean_bios_tmp_files(&keep).await;
        if cleaned > 0 {
            tracing::info!("Cleaned {cleaned} orphan tmp file(s)");
        }
        if !jobs.is_empty() {
            tracing::info!("Restored {} unfinished download(s)", jobs.len());
        }
        let to_resume: Vec<JobKey> = jobs
            .iter()
            .filter(|j| j.status == JobStatus::Interrupted)
            .map(DownloadJob::key)
            .collect();
        if !to_resume.is_empty() {
            let me = self.clone();
            let handle = tokio::spawn(async move { me.auto_resume(to_resume).await });
            self.st().auto_resume = Some(handle);
        }
    }

    async fn remove_tmp_files(&self, paths: &[String]) -> usize {
        let mut removed = 0;
        for path in paths {
            match self.files.remove_file(path).await {
                Ok(()) => removed += 1,
                Err(e) => tracing::warn!("Failed to remove tmp file {path}: {e}"),
            }
        }
        removed
    }

    async fn clean_orphan_rom_tmp_files(&self, keep: &HashSet<String>) -> usize {
        let base = self.paths.roms_path();
        if base.is_empty() {
            return 0;
        }
        let found = self.files.walk_files_matching_suffixes(&base, &[TMP_EXT, ZIP_TMP_EXT]).await;
        let orphans: Vec<String> = found.into_iter().filter(|p| !keep.contains(p)).collect();
        self.remove_tmp_files(&orphans).await
    }

    /// A `.tmp` under the BIOS folder that no download job owns is stale (a firmware archive in progress is a job).
    async fn clean_bios_tmp_files(&self, keep: &HashSet<String>) -> usize {
        let base = self.paths.bios_path();
        if base.is_empty() {
            return 0;
        }
        let found = self.files.walk_files_matching_suffixes(&base, &[TMP_EXT]).await;
        let orphans: Vec<String> = found.into_iter().filter(|p| !keep.contains(p)).collect();
        self.remove_tmp_files(&orphans).await
    }

    /// Resume interrupted jobs in order; stop at the first unreachable server
    /// or rejected login, marking the rest with that reason (no retry loop).
    async fn auto_resume(self: Arc<Self>, keys: Vec<JobKey>) {
        for (index, key) in keys.iter().enumerate() {
            let result = match key.kind {
                JobKind::Rom => self.resume_inner(key.id, false).await,
                JobKind::Firmware => self.resume_firmware_download(key.id).await,
            };
            if result["success"] == true {
                continue;
            }
            let why = result["reason"].as_str().unwrap_or_default();
            if why == INTERRUPT_SERVER_UNREACHABLE || why == INTERRUPT_TIMEOUT || why == INTERRUPT_AUTH_FAILED {
                for remaining in &keys[index + 1..] {
                    self.mark_interrupted(*remaining, why).await;
                }
                return;
            }
            tracing::info!("Auto-resume of {key} skipped: {}", result["message"]);
        }
    }

    /// Resume downloads that stopped because the server could not be reached
    /// (reason `server_unreachable` / `timeout`), in queue order. A no-op while
    /// an earlier auto-resume is still running. Called periodically, so a
    /// download picks up again once the server is back.
    pub async fn resume_unreachable_interrupted(self: &Arc<Self>) {
        let keys: Vec<JobKey> = {
            let mut st = self.st();
            if st.auto_resume.as_ref().is_some_and(|h| !h.is_finished()) {
                return;
            }
            let positions = st.positions.clone();
            let mut found: Vec<(i64, JobKey)> = st
                .queue
                .values()
                .filter(|e| {
                    e.status == "interrupted"
                        && matches!(e.reason.as_deref(), Some(INTERRUPT_SERVER_UNREACHABLE | INTERRUPT_TIMEOUT))
                })
                .map(|e| (positions.get(&e.key()).copied().unwrap_or(i64::MAX), e.key()))
                .collect();
            found.sort();
            let keys: Vec<JobKey> = found.into_iter().map(|(_, k)| k).collect();
            if keys.is_empty() {
                return;
            }
            let me = self.clone();
            let task_keys = keys.clone();
            st.auto_resume = Some(tokio::spawn(async move { me.auto_resume(task_keys).await }));
            keys
        };
        tracing::info!("Retrying {} download(s) interrupted by an unreachable server", keys.len());
    }

    // ── Start / resume ──────────────────────────────────────────────────

    /// Start downloading `rom_id`. At most one downloaded version per
    /// shortcut binding: an installed sibling bound to this shortcut (or
    /// unbound) is removed first; a removal failure aborts the download.
    pub async fn start_download(self: &Arc<Self>, rom_id: i64) -> Value {
        let _mutation = self.mutation_lock.lock().await;
        if !self.st().in_progress.insert(JobKey::rom(rom_id)) {
            return failure("already_downloading", "Already downloading");
        }
        if let Some(fail) = self.remove_conflicting_sibling_installs(rom_id).await {
            self.st().in_progress.remove(&JobKey::rom(rom_id));
            return fail;
        }
        self.begin_download(rom_id, false, false).await
    }

    /// Resume a paused or interrupted download from its partial. Refused
    /// (`superseded`, entry dropped) when a version switch moved the group's
    /// shortcut binding to a sibling while this one was stopped.
    ///
    /// A multipart partial of unknown origin (`needs_recovery`: made before
    /// archive validators were stored) is never resumed on its own. The user's
    /// resume of such a job is the explicit restart: the partial is replaced
    /// once the server's response for the whole archive is accepted.
    pub async fn resume_download(self: &Arc<Self>, rom_id: i64) -> Value {
        self.resume_inner(rom_id, true).await
    }

    async fn resume_inner(self: &Arc<Self>, rom_id: i64, user_initiated: bool) -> Value {
        let _mutation = self.mutation_lock.lock().await;
        let resumable = self
            .st()
            .queue
            .get(&JobKey::rom(rom_id))
            .is_some_and(|e| e.status == "paused" || e.status == "interrupted");
        if !resumable {
            return failure("not_paused", "No paused download for this ROM");
        }
        if self.st().in_progress.contains(&JobKey::rom(rom_id)) {
            return failure("already_downloading", "Already downloading");
        }
        if self.resume_target_superseded(rom_id).await {
            self.evict(rom_id).await;
            return failure("superseded", "Another version is now active");
        }
        let job = self.load_job(JobKey::rom(rom_id)).await;
        // A partial of unknown origin waits for the user, unless it is already as long as the
        // whole archive: the transfer then validates it (structure + CRCs) and finishes it.
        let unverified = match &job {
            Some(job) if job.is_multi && job.needs_recovery => {
                let partial = self.files.file_size(&job.tmp_path).await;
                partial > 0 && job.total_bytes != Some(partial)
            }
            _ => false,
        };
        if unverified && !user_initiated {
            self.mark_interrupted(JobKey::rom(rom_id), INTERRUPT_NEEDS_RECOVERY).await;
            return failure(
                INTERRUPT_NEEDS_RECOVERY,
                "The partial download cannot be verified; restart it to continue",
            );
        }
        self.st().in_progress.insert(JobKey::rom(rom_id));
        if let Some(fail) = self.remove_conflicting_sibling_installs(rom_id).await {
            self.st().in_progress.remove(&JobKey::rom(rom_id));
            return fail;
        }
        self.begin_download(rom_id, true, unverified).await
    }

    async fn resume_target_superseded(&self, rom_id: i64) -> bool {
        let Ok(Some(rom)) = self.library.rom(rom_id).await else {
            return false;
        };
        let Some(key) = rom.sibling_group_key.as_deref() else {
            return false;
        };
        if rom.shortcut_app_id.is_some() {
            return false;
        }
        self.library
            .roms_in_group(key)
            .await
            .unwrap_or_default()
            .iter()
            .any(|m| m.rom_id != rom_id && m.shortcut_app_id.is_some())
    }

    /// Installed group members this download supersedes: unbound ones and
    /// ones bound to the same shortcut. A member bound to a different
    /// shortcut (a grandfathered duplicate) is never removed.
    async fn conflicting_sibling_install_ids(&self, rom_id: i64) -> anyhow::Result<Vec<i64>> {
        let Some(rom) = self.library.rom(rom_id).await? else {
            return Ok(Vec::new());
        };
        let Some(key) = rom.sibling_group_key.as_deref() else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for member in self.library.roms_in_group(key).await? {
            if member.rom_id == rom_id || self.library.install(member.rom_id).await?.is_none() {
                continue;
            }
            if member.shortcut_app_id.is_some() && member.shortcut_app_id != rom.shortcut_app_id {
                continue;
            }
            out.push(member.rom_id);
        }
        Ok(out)
    }

    /// Remove superseded sibling installs; `Some(failure)` aborts the caller.
    async fn remove_conflicting_sibling_installs(&self, rom_id: i64) -> Option<Value> {
        let ids = match self.conflicting_sibling_install_ids(rom_id).await {
            Ok(ids) => ids,
            Err(e) => {
                tracing::error!("Reading the sibling group of rom {rom_id} failed: {e:#}");
                return Some(failure(reason::UNKNOWN, "Failed to start download"));
            }
        };
        for sibling in ids {
            let result = self.removal.remove_rom(sibling).await;
            if result["success"] == true {
                tracing::info!("Superseding install of rom {sibling} (group of rom {rom_id})");
                self.evict(sibling).await;
                continue;
            }
            if result["reason"] == "not_installed" {
                continue;
            }
            tracing::error!(
                "Superseding install of rom {sibling} (group of rom {rom_id}) failed: {}",
                result["message"]
            );
            return Some(result);
        }
        None
    }

    /// Shared core of start and resume: fetch the ROM, resolve the target,
    /// pre-flight the disk, persist the `queued` job, and spawn the transfer.
    /// A resume that cannot proceed keeps the partial whenever it could
    /// succeed later; only a can-never-succeed failure drops the job.
    async fn begin_download(self: &Arc<Self>, rom_id: i64, resume: bool, restart: bool) -> Value {
        self.st().in_progress.insert(JobKey::rom(rom_id));
        let release = |me: &Self| {
            me.st().in_progress.remove(&JobKey::rom(rom_id));
        };
        let rom = match self.romm.get_rom(rom_id).await {
            Ok(rom) => rom,
            Err(e) => {
                release(self);
                tracing::error!("Failed to fetch ROM {rom_id}: {e}");
                let mut response = e.to_failure();
                if resume {
                    match classify_download_failure(&anyhow::Error::new(e)) {
                        Outcome::Interrupted(why) => {
                            self.mark_interrupted(JobKey::rom(rom_id), why).await;
                            response["reason"] = json!(why);
                        }
                        Outcome::Failed => {
                            let message = response["message"].as_str().map(str::to_string);
                            self.discard_resumable(JobKey::rom(rom_id), "failed", message).await;
                        }
                    }
                }
                return response;
            }
        };
        let str_of = |key: &str| rom.get(key).and_then(Value::as_str).map(str::to_string);
        let platform_slug = str_of("platform_slug").unwrap_or_default();
        let system = self
            .systems
            .resolve_system(&platform_slug, str_of("platform_fs_slug").as_deref());
        let roms_path = self.paths.roms_path();
        let roms_dir = match safe_join_lexical(&roms_path, &[&system]) {
            Ok(dir) => dir,
            Err(e) => {
                release(self);
                tracing::error!("Rejected download for ROM {rom_id}: unsafe platform slug {system:?}: {e}");
                let message = "Server sent an unsafe platform path \u{2014} download aborted";
                if resume {
                    self.discard_resumable(JobKey::rom(rom_id), "failed", Some(message.into())).await;
                }
                self.emit(
                    "download_failed",
                    json!({
                        "rom_id": rom_id,
                        "rom_name": str_of("name").unwrap_or_default(),
                        "platform_name": str_of("platform_name").unwrap_or_else(|| platform_slug.clone()),
                        "error_message": message,
                    }),
                );
                return failure("path_traversal", message);
            }
        };
        let names = resolve_download_names(&rom);
        if names.files_missing {
            tracing::warn!(
                "has_nested_single_file=true but files list is empty; falling back to fs_name='{}'",
                names.raw_file_name
            );
        }
        if names.file_name_changed() {
            tracing::warn!("Sanitized fs_name from '{}' to '{}'", names.raw_file_name, names.file_name);
        }
        let file_name = names.file_name.clone();
        let file_size = rom.get("fs_size_bytes").and_then(Value::as_u64).unwrap_or(0);
        let target_path = join(&roms_dir, &[&file_name]);
        let is_multi = is_multi_file_download(&rom);
        let tmp_path = format!("{target_path}{}", if is_multi { ZIP_TMP_EXT } else { TMP_EXT });
        let partial_bytes = if resume { self.files.file_size(&tmp_path).await } else { 0 };

        let required = {
            let base = if is_multi { file_size * 2 } else { file_size } + SPACE_BUFFER;
            if resume { base.saturating_sub(partial_bytes) } else { base }
        };
        let preflight: anyhow::Result<Option<Value>> = async {
            self.files.make_dirs(&roms_dir).await?;
            let free = self.files.disk_free(&roms_dir).await?;
            let reserved: u64 = self.st().reserved.values().sum();
            let available = i128::from(free) - i128::from(reserved);
            if file_size > 0 && available < i128::from(required) {
                let free_mb = available.max(0) / (1024 * 1024);
                let need_mb = required / (1024 * 1024);
                return Ok(Some(failure(
                    "insufficient_space",
                    &format!("Not enough disk space ({free_mb}MB free, need {need_mb}MB)"),
                )));
            }
            Ok(None)
        }
        .await;
        match preflight {
            Ok(None) => {}
            Ok(Some(no_space)) => {
                release(self);
                if resume {
                    // The partial is kept: free some space and resume.
                    self.mark_interrupted(JobKey::rom(rom_id), INTERRUPT_DISK_FULL).await;
                }
                return no_space;
            }
            Err(e) => {
                release(self);
                tracing::error!("Failed to prepare download for ROM {rom_id}: {e:#}");
                return failure(reason::UNKNOWN, "Failed to start download");
            }
        }

        let rom_name = str_of("name").unwrap_or_else(|| file_name.clone());
        let platform_name = str_of("platform_name").unwrap_or_else(|| platform_slug.clone());
        let job = self
            .enqueue_job(NewJob {
                kind: JobKind::Rom,
                item_id: rom_id,
                rom_name: rom_name.clone(),
                platform_name: platform_name.clone(),
                file_name: file_name.clone(),
                target_path: target_path.clone(),
                tmp_path,
                is_multi,
                position: 0,
                total_bytes: (!is_multi).then_some(file_size),
            })
            .await;
        let total_bytes = job.and_then(|j| j.total_bytes).unwrap_or(file_size);

        let plan = Plan {
            rom_id,
            rom: rom.clone(),
            target_path,
            system,
            file_name: file_name.clone(),
            rom_name: rom_name.clone(),
            platform_name: platform_name.clone(),
            has_multiple: is_multi,
            extract_dir_name: names.extract_dir_name.clone(),
            restart,
            key: JobKey::rom(rom_id),
            firmware: None,
        };
        self.launch_transfer(plan, partial_bytes, total_bytes, required, resume);
        json!({"success": true, "message": "Download started"})
    }

    /// Register the queue entry, the disk reservation and the cancellation handles of `plan`, then spawn its
    /// transfer. The status stays `queued` until the scheduler hands over the transfer slot.
    fn launch_transfer(self: &Arc<Self>, plan: Plan, partial_bytes: u64, total_bytes: u64, required: u64, resume: bool) {
        let key = plan.key;
        let control = {
            let mut st = self.st();
            st.persisted_totals.remove(&key);
            st.next_control += 1;
            let control = Arc::new(Control {
                id: st.next_control,
                token: CancellationToken::new(),
                cancelled: AtomicBool::new(false),
                paused: AtomicBool::new(false),
                shutdown: AtomicBool::new(false),
            });
            let resumable = st.queue.get(&key).is_some_and(|e| e.resumable);
            st.queue.insert(
                key,
                QueueEntry {
                    kind: key.kind,
                    item_id: key.id,
                    rom_name: plan.rom_name.clone(),
                    platform_name: plan.platform_name.clone(),
                    file_name: plan.file_name.clone(),
                    status: "queued".into(),
                    progress: ratio(partial_bytes, total_bytes),
                    bytes_downloaded: partial_bytes,
                    total_bytes,
                    resumable,
                    reason: None,
                    error: None,
                    ..QueueEntry::default()
                },
            );
            st.reserved.insert(key, required);
            st.attempts.insert(
                key,
                Attempt {
                    control: control.clone(),
                    handle: None,
                },
            );
            control
        };
        let me = self.clone();
        let task_control = control.clone();
        let handle = tokio::spawn(async move { me.do_download(plan, task_control, resume).await });
        if let Some(attempt) = self.st().attempts.get_mut(&key)
            && Arc::ptr_eq(&attempt.control, &control)
        {
            attempt.handle = Some(handle);
        }
    }

    // ── Firmware ────────────────────────────────────────────────────────

    /// Queue the download of RomM firmware `firmware_id` (a key file or a firmware archive). What it fetches and
    /// where it goes is decided by the firmware service at this moment; nothing is downloaded when this machine
    /// already has what the file would provide.
    pub async fn start_firmware_download(self: &Arc<Self>, firmware_id: i64) -> Value {
        let _mutation = self.mutation_lock.lock().await;
        if !self.st().in_progress.insert(JobKey::firmware(firmware_id)) {
            return failure("already_downloading", "Already downloading");
        }
        self.begin_firmware(firmware_id, false).await
    }

    /// Resume a paused or interrupted firmware download from its partial.
    pub async fn resume_firmware_download(self: &Arc<Self>, firmware_id: i64) -> Value {
        let _mutation = self.mutation_lock.lock().await;
        let key = JobKey::firmware(firmware_id);
        let resumable = self.st().queue.get(&key).is_some_and(|e| e.status == "paused" || e.status == "interrupted");
        if !resumable {
            return failure("not_paused", "No paused download for this firmware");
        }
        if !self.st().in_progress.insert(key) {
            return failure("already_downloading", "Already downloading");
        }
        self.begin_firmware(firmware_id, true).await
    }

    async fn begin_firmware(self: &Arc<Self>, firmware_id: i64, resume: bool) -> Value {
        let key = JobKey::firmware(firmware_id);
        let release = |me: &Self| {
            me.st().in_progress.remove(&key);
        };
        let Some(target) = self.firmware_jobs.target(firmware_id).await else {
            release(self);
            if resume {
                // Nothing left to fetch (the key appeared, the archive was saved): the stopped job is obsolete.
                self.evict_job(key).await;
            }
            return failure("nothing_to_download", "Nothing to download for this firmware file");
        };
        let dest_dir = dirname(&target.dest_path).to_string();
        let tmp_path = format!("{}{TMP_EXT}", target.dest_path);
        let partial_bytes = if resume { self.files.file_size(&tmp_path).await } else { 0 };
        if !resume && let Err(e) = self.files.remove_file(&tmp_path).await {
            tracing::warn!("Could not remove a stale {tmp_path}: {e}");
        }
        // Keys are a few KB, the archive hundreds of MB: only the archive needs the generous buffer.
        let buffer = if target.kind == FirmwareFileKind::Archive { SPACE_BUFFER } else { 1024 * 1024 };
        let required = target.size.saturating_sub(partial_bytes) + buffer;
        let preflight: anyhow::Result<Option<Value>> = async {
            self.files.make_dirs(&dest_dir).await?;
            let free = self.files.disk_free(&dest_dir).await?;
            let reserved: u64 = self.st().reserved.values().sum();
            let available = i128::from(free) - i128::from(reserved);
            if target.size > 0 && available < i128::from(required) {
                let free_mb = available.max(0) / (1024 * 1024);
                return Ok(Some(failure(
                    "insufficient_space",
                    &format!("Not enough disk space ({free_mb}MB free, need {}MB)", required / (1024 * 1024)),
                )));
            }
            Ok(None)
        }
        .await;
        match preflight {
            Ok(None) => {}
            Ok(Some(no_space)) => {
                release(self);
                if resume {
                    self.mark_interrupted(key, INTERRUPT_DISK_FULL).await;
                }
                return no_space;
            }
            Err(e) => {
                release(self);
                tracing::error!("Failed to prepare firmware download {firmware_id}: {e:#}");
                return failure(reason::UNKNOWN, "Failed to start download");
            }
        }
        let job = self
            .enqueue_job(NewJob {
                kind: JobKind::Firmware,
                item_id: firmware_id,
                rom_name: target.file_name.clone(),
                platform_name: FIRMWARE_LABEL.into(),
                file_name: target.file_name.clone(),
                target_path: target.dest_path.clone(),
                tmp_path,
                is_multi: false,
                position: 0,
                total_bytes: (target.size > 0).then_some(target.size),
            })
            .await;
        let total_bytes = job.and_then(|j| j.total_bytes).unwrap_or(target.size);
        let plan = Plan {
            key,
            rom_id: firmware_id,
            firmware: Some(target.clone()),
            rom: Value::Null,
            target_path: target.dest_path.clone(),
            system: String::new(),
            file_name: target.file_name.clone(),
            rom_name: target.file_name.clone(),
            platform_name: FIRMWARE_LABEL.into(),
            has_multiple: false,
            extract_dir_name: String::new(),
            restart: false,
        };
        self.launch_transfer(plan, partial_bytes, total_bytes, required, resume);
        json!({"success": true, "message": "Download started"})
    }

    /// Resume a firmware file: verify it, store it, copy keys to the other emulators.
    async fn post_download_firmware(&self, plan: &Plan, tmp: &str) -> anyhow::Result<String> {
        let target = plan.firmware.as_ref().ok_or_else(|| anyhow::anyhow!("a firmware plan without a target"))?;
        if let Some(want) = target.sha1.as_deref() {
            let got = self.files.sha1_hex(tmp).await;
            if got.as_deref() != Some(want) {
                anyhow::bail!("{} does not match the checksum RomM reports; it was not installed", target.file_name);
            }
        }
        // Other emulators that still lack the key get a copy of the verified file; existing files are never replaced.
        for dst in &target.copy_to {
            if !self.files.exists(dst).await {
                self.files.copy_file(tmp, dst).await.map_err(|e| anyhow::anyhow!("copying to {dst}: {e}"))?;
            }
        }
        if self.files.exists(&target.dest_path).await {
            tracing::info!("{} appeared while downloading; keeping the existing file", target.dest_path);
            let _ = self.files.remove_file(tmp).await;
        } else {
            self.files.rename(tmp, &target.dest_path).await?;
        }
        Ok(target.dest_path.clone())
    }

    async fn finalize_firmware_complete(&self, plan: &Plan, final_path: &str) {
        let key = plan.key;
        self.drop_job(key).await;
        self.update_entry(key, |e| {
            e.status = "completed".into();
            e.progress = 1.0;
            e.reason = None;
        });
        let mut payload = json!({
            "rom_name": plan.rom_name,
            "platform_name": plan.platform_name,
            "file_path": final_path,
        });
        set_identity(&mut payload, key);
        self.emit("download_complete", payload);
        tracing::info!("Firmware download complete: {} -> {final_path}", plan.rom_name);
    }

    // ── Terminal state helpers ──────────────────────────────────────────

    /// Flip a job to `interrupted` (row + entry) and emit the frame; partial kept.
    async fn mark_interrupted(&self, key: JobKey, why: &str) {
        let now = self.clock.now_iso();
        self.mutate_job(key, |job| job.interrupt(why, &now)).await;
        if self.update_entry(key, |e| {
            e.status = "interrupted".into();
            e.reason = Some(why.into());
        }) {
            self.emit_entry(key);
        }
    }

    /// Drop a paused/interrupted job for good: delete its row and partial.
    async fn discard_resumable(&self, key: JobKey, status: &str, error: Option<String>) {
        if let Some(job) = self.load_job(key).await {
            self.remove_tmp_files(&[job.tmp_path]).await;
        }
        self.drop_job(key).await;
        let names = {
            let mut st = self.st();
            let Some(entry) = st.queue.get_mut(&key) else {
                return;
            };
            entry.status = status.into();
            entry.reason = None;
            if error.is_some() {
                entry.error.clone_from(&error);
            }
            let names = (entry.rom_name.clone(), entry.platform_name.clone());
            st.prune();
            names
        };
        if status == "failed" {
            self.emit(
                "download_failed",
                {
                    let mut payload = json!({
                        "rom_name": names.0,
                        "platform_name": names.1,
                        "error_message": error.unwrap_or_else(|| "Download failed".into()),
                    });
                    set_identity(&mut payload, key);
                    payload
                },
            );
        } else {
            self.emit_entry(key);
        }
    }

    // ── The transfer task ───────────────────────────────────────────────

    fn progress_hook(self: &Arc<Self>, plan: &Plan) -> Box<dyn FnMut(u64, u64) + Send> {
        let me = self.clone();
        let key = plan.key;
        let rom_name = plan.rom_name.clone();
        let (mut last_emit, mut last_log) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
        Box::new(move |downloaded, total| {
            let now = me.clock.monotonic();
            if now - last_log >= LOG_INTERVAL_S {
                last_log = now;
                tracing::info!(
                    "Download progress: {rom_name} \u{2014} {:.1}/{:.1} MB ({:.0}%)",
                    downloaded as f64 / 1_048_576.0,
                    total as f64 / 1_048_576.0,
                    ratio(downloaded, total) * 100.0
                );
            }
            if now - last_emit < EMIT_INTERVAL_S && downloaded < total {
                return;
            }
            last_emit = now;
            let frame = {
                let mut st = me.st();
                let Some(entry) = st.queue.get_mut(&key) else {
                    return; // evicted mid-download: never resurrect
                };
                entry.progress = ratio(downloaded, total);
                entry.bytes_downloaded = downloaded;
                entry.total_bytes = total;
                let mut frame = entry.frame();
                frame["status"] = json!("downloading");
                strip_reason(&mut frame);
                frame
            };
            let observer = me.clone();
            tokio::spawn(async move { observer.persist_transfer_observation(key, total).await });
            me.emit("download_progress", frame);
        })
    }

    fn meta_hook(self: &Arc<Self>, key: JobKey) -> Box<dyn FnOnce(bool) + Send> {
        let me = self.clone();
        Box::new(move |range_supported| {
            let frame = {
                let mut st = me.st();
                let Some(entry) = st.queue.get_mut(&key) else {
                    return;
                };
                entry.resumable = range_supported;
                let mut frame = entry.frame();
                strip_reason(&mut frame);
                frame
            };
            me.emit("download_progress", frame);
        })
    }

    fn extract_hook(self: &Arc<Self>, plan: &Plan) -> Box<dyn FnMut(u64, u64) + Send> {
        let me = self.clone();
        let key = plan.key;
        let rom_name = plan.rom_name.clone();
        let (mut last_emit, mut last_log) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
        Box::new(move |extracted, total| {
            let now = me.clock.monotonic();
            if now - last_log >= LOG_INTERVAL_S {
                last_log = now;
                tracing::info!(
                    "Extract progress: {rom_name} \u{2014} {:.1}/{:.1} MB ({:.0}%)",
                    extracted as f64 / 1_048_576.0,
                    total as f64 / 1_048_576.0,
                    ratio(extracted, total) * 100.0
                );
            }
            if now - last_emit < EMIT_INTERVAL_S && extracted < total {
                return;
            }
            last_emit = now;
            let frame = {
                let mut st = me.st();
                let Some(entry) = st.queue.get_mut(&key) else {
                    return;
                };
                entry.status = "extracting".into();
                entry.progress = ratio(extracted, total);
                entry.bytes_downloaded = extracted;
                entry.total_bytes = total;
                let mut frame = entry.frame();
                frame["resumable"] = json!(false);
                strip_reason(&mut frame);
                frame
            };
            me.emit("download_progress", frame);
        })
    }

    /// The full size of the transfer file: the persisted server total, else
    /// RomM's `fs_size_bytes` for a single file (unknown for a ZIP).
    async fn expected_transfer_total(&self, plan: &Plan) -> u64 {
        if let Some(total) = self.load_job(plan.key).await.and_then(|j| j.total_bytes) {
            return total;
        }
        if plan.has_multiple {
            return 0;
        }
        plan.rom.get("fs_size_bytes").and_then(Value::as_u64).unwrap_or(0)
    }

    /// Live phase changes (waiting for headers, retry back-off, bytes flowing)
    /// as queue frames, so the UI can show an honest waiting state.
    fn phase_hook(self: &Arc<Self>, key: JobKey) -> Box<dyn FnMut(TransferPhase) + Send> {
        let me = self.clone();
        Box::new(move |phase| {
            let now = me.clock.epoch_seconds();
            let frame = {
                let mut st = me.st();
                let Some(entry) = st.queue.get_mut(&key) else {
                    return;
                };
                match phase {
                    TransferPhase::Waiting { attempt, max_attempts } => {
                        entry.phase = Some("waiting".into());
                        entry.phase_since = Some(now);
                        entry.attempt = Some(attempt);
                        entry.max_attempts = Some(max_attempts);
                        entry.retry_delay_s = None;
                    }
                    TransferPhase::Retrying { attempt, max_attempts, delay_secs } => {
                        entry.phase = Some("retrying".into());
                        entry.phase_since = Some(now);
                        entry.attempt = Some(attempt);
                        entry.max_attempts = Some(max_attempts);
                        entry.retry_delay_s = Some(delay_secs);
                    }
                    TransferPhase::Transferring => entry.clear_phase(),
                }
                let mut frame = entry.frame();
                strip_reason(&mut frame);
                frame
            };
            me.emit("download_progress", frame);
        })
    }

    /// Saves a representation's validators (or clears them) in the job row.
    /// The transfer awaits this before it writes the first byte of that
    /// representation, so a crash can never leave bytes the row vouches for
    /// wrongly. A failed write is an error, not a log line.
    fn identity_sink(self: &Arc<Self>, key: JobKey) -> IdentitySink {
        let me = self.clone();
        Box::new(move |identity: Option<ArchiveIdentity>| {
            let me = me.clone();
            Box::pin(async move {
                let now = me.clock.now_iso();
                let _guard = me.job_lock.lock().await;
                let Some(mut job) = me.jobs.get(key).await? else {
                    return Ok(());
                };
                job.record_archive_identity(identity.as_ref(), &now);
                if let Some(i) = &identity {
                    job.observe_transfer(Some(i.length), Some(true), &now);
                }
                me.jobs.save(&job).await
            })
        })
    }

    fn hooks(self: &Arc<Self>, plan: &Plan, control: &Control) -> TransferHooks {
        TransferHooks {
            progress: self.progress_hook(plan),
            on_meta: Some(self.meta_hook(plan.key)),
            on_phase: self.phase_hook(plan.key),
            on_identity: plan.has_multiple.then(|| self.identity_sink(plan.key)),
            cancel: control.token.clone(),
        }
    }

    /// Run the HTTP transfer into `tmp_path` unless the partial is already
    /// complete (returns `true`: skipped). A single-file partial larger than
    /// expected is stale: it is deleted and the transfer restarts.
    async fn transfer(self: &Arc<Self>, plan: &Plan, tmp_path: &str, control: &Control, resume: bool) -> anyhow::Result<bool> {
        if plan.has_multiple {
            return self.transfer_archive(plan, tmp_path, control).await;
        }
        if plan.key.kind == JobKind::Firmware {
            return self.transfer_firmware(plan, tmp_path, control, resume).await;
        }
        let mut resume = resume;
        if resume {
            let expected = self.expected_transfer_total(plan).await;
            let on_disk = self.files.file_size(tmp_path).await;
            if expected > 0 && on_disk == expected {
                tracing::info!(
                    "Partial for ROM {} is already complete ({on_disk} bytes); skipping transfer",
                    plan.rom_id
                );
                self.update_entry(plan.key, |e| {
                    e.progress = ratio(on_disk, expected);
                    e.bytes_downloaded = on_disk;
                    e.total_bytes = expected;
                });
                return Ok(true);
            }
            if expected > 0 && on_disk > expected {
                tracing::warn!(
                    "Partial for ROM {} exceeds the expected size ({on_disk} > {expected}); restarting",
                    plan.rom_id
                );
                self.remove_tmp_files(&[tmp_path.to_string()]).await;
                resume = false;
            }
        }
        self.romm
            .download_rom_content(
                plan.rom_id,
                &plan.file_name,
                tmp_path,
                TransferMode::Single { resume },
                self.hooks(plan, control),
            )
            .await?;
        Ok(false)
    }

    /// One firmware file: continue the partial when the server honours `Range`, skip the transfer when the partial
    /// is already complete, restart when it is longer than the file.
    async fn transfer_firmware(self: &Arc<Self>, plan: &Plan, tmp_path: &str, control: &Control, resume: bool) -> anyhow::Result<bool> {
        let target = plan.firmware.as_ref().ok_or_else(|| anyhow::anyhow!("a firmware plan without a target"))?;
        let mut resume = resume;
        if resume {
            let on_disk = self.files.file_size(tmp_path).await;
            if target.size > 0 && on_disk == target.size {
                self.update_entry(plan.key, |e| {
                    e.progress = 1.0;
                    e.bytes_downloaded = on_disk;
                    e.total_bytes = on_disk;
                });
                return Ok(true);
            }
            if target.size > 0 && on_disk > target.size {
                self.remove_tmp_files(&[tmp_path.to_string()]).await;
                resume = false;
            }
        }
        self.firmware_api
            .download_firmware_content(
                target.firmware_id,
                &target.file_name,
                tmp_path,
                TransferMode::Single { resume },
                self.hooks(plan, control),
            )
            .await?;
        Ok(false)
    }

    /// A multipart ROM as RomM's cached ZIP. The first request already asks for
    /// the cached archive (`Range: bytes=0-`); a partial is continued only
    /// against the strong validator saved with it, and a partial that predates
    /// validators is left alone until the user restarts it.
    ///
    /// A partial whose size equals the known complete length is checked first
    /// (structure and every CRC): a complete, valid ZIP is done whichever
    /// representation produced it, so no transfer is needed. One that fails the
    /// check is discarded once and fetched again.
    async fn transfer_archive(self: &Arc<Self>, plan: &Plan, tmp_path: &str, control: &Control) -> anyhow::Result<bool> {
        let mut restart = plan.restart;
        for pass in 0..2 {
            let job = self.load_job(plan.key).await;
            let partial = self.files.file_size(tmp_path).await;
            let identity = job.as_ref().and_then(DownloadJob::archive_identity);
            let needs_recovery = job.as_ref().is_some_and(|j| j.needs_recovery);
            let known_total = identity.as_ref().map(|i| i.length).or(job.as_ref().and_then(|j| j.total_bytes));
            let outcome = if partial > 0 && known_total == Some(partial) {
                TransferOutcome::AlreadyComplete
            } else {
                let mode = plan_resume(partial, identity.as_ref(), needs_recovery, restart);
                if mode == ResumePlan::NeedsRecovery {
                    return Err(ArchiveRecoveryNeeded("a partial of unknown origin".into()).into());
                }
                self.romm
                    .download_rom_content(
                        plan.rom_id,
                        &plan.file_name,
                        tmp_path,
                        TransferMode::Archive(mode),
                        self.hooks(plan, control),
                    )
                    .await?
            };
            if outcome == TransferOutcome::Downloaded {
                return Ok(false);
            }
            match self.files.verify_zip(tmp_path).await {
                Ok(()) => {
                    let on_disk = self.files.file_size(tmp_path).await;
                    self.update_entry(plan.key, |e| {
                        e.progress = 1.0;
                        e.bytes_downloaded = on_disk;
                        e.total_bytes = on_disk;
                    });
                    return Ok(true);
                }
                Err(e) if pass == 0 => {
                    tracing::warn!(
                        "Archive for ROM {} is complete on disk but invalid ({e:#}); fetching it again",
                        plan.rom_id
                    );
                    self.remove_tmp_files(&[tmp_path.to_string()]).await;
                    let now = self.clock.now_iso();
                    self.mutate_job(plan.key, |job| {
                        job.record_archive_identity(None, &now);
                        Ok(())
                    })
                    .await;
                    restart = true;
                }
                Err(e) => return Err(e.context("the downloaded archive failed validation")),
            }
        }
        unreachable!("the second pass always returns")
    }

    /// Persist the install record a finished download (or adoption) writes.
    /// An invariant failure removes the just-installed artifact.
    async fn record_install(&self, plan: &Plan, file_path: String, rom_dir: Option<String>) -> anyhow::Result<String> {
        let platform_slug = plan
            .rom
            .get("platform_slug")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let install = match RomInstall::mark_installed(
            plan.rom_id,
            file_path.clone(),
            rom_dir.clone(),
            platform_slug,
            plan.system.clone(),
            self.clock.now_iso(),
        ) {
            Ok(install) => install,
            Err(e) => {
                let cleanup = match &rom_dir {
                    Some(dir) => self.files.remove_tree(dir).await,
                    None => self.files.remove_file(&file_path).await,
                };
                if let Err(c) = cleanup {
                    tracing::warn!("Cleanup after invalid install metadata failed: {c}");
                }
                anyhow::bail!("Invalid install metadata: {e}");
            }
        };
        self.library.save_install(&install).await?;
        Ok(file_path)
    }

    async fn post_download_single(&self, plan: &Plan) -> anyhow::Result<String> {
        let tmp = format!("{}{TMP_EXT}", plan.target_path);
        self.files.rename(&tmp, &plan.target_path).await?;
        self.record_install(plan, plan.target_path.clone(), None).await
    }

    /// Extract the archive into the ROM-identity-named folder and record the
    /// largest launch file. `fresh` (a complete archive left by an attempt
    /// that died mid-extraction) clears the half-extracted folder first.
    async fn post_download_multi(self: &Arc<Self>, plan: &Plan, fresh: bool) -> anyhow::Result<String> {
        let extract_dir = join(dirname(&plan.target_path), &[&plan.extract_dir_name]);
        if fresh {
            self.files.remove_tree(&extract_dir).await?;
        }
        self.files.make_dirs(&extract_dir).await?;
        let tmp_zip = format!("{}{ZIP_TMP_EXT}", plan.target_path);
        let roms_base = self.paths.roms_path();
        self.files
            .extract_zip(&tmp_zip, &extract_dir, &roms_base, self.extract_hook(plan))
            .await?;
        self.files.remove_file(&tmp_zip).await?;
        self.files.decode_url_encoded_names(&extract_dir).await?;
        let files = self.files.scan_files_with_sizes(&extract_dir).await;
        // No ES-DE config on EmuDeck: a bundled `.m3u` is never the launch file.
        let launch_file = detect_launch_file(&files, false).unwrap_or_else(|| extract_dir.clone());
        self.record_install(plan, launch_file, Some(extract_dir)).await
    }

    async fn run_attempt(self: &Arc<Self>, plan: &Plan, control: &Control, resume: bool) -> Ended {
        tracing::info!(
            "Download starting: {} (rom_id={}, multi={}) -> {}",
            plan.rom_name,
            plan.rom_id,
            plan.has_multiple,
            plan.target_path
        );
        let slot_taken = self.st().active.is_some_and(|id| id != control.id);
        if slot_taken && self.update_entry(plan.key, |e| e.status = "queued".into()) {
            self.emit_entry(plan.key);
        }
        if !self.acquire_slot(plan.key, control).await {
            return Ended::Stopped;
        }
        self.update_entry(plan.key, |e| {
            e.status = "downloading".into();
            e.reason = None;
            e.clear_phase();
        });
        let now = self.clock.now_iso();
        self.mutate_job(plan.key, |job| job.start(&now)).await;

        let suffix = if plan.has_multiple { ZIP_TMP_EXT } else { TMP_EXT };
        let tmp = format!("{}{suffix}", plan.target_path);
        let skipped = match self.transfer(plan, &tmp, control, resume).await {
            Ok(skipped) => skipped,
            Err(e) if e.downcast_ref::<TransferCancelled>().is_some() => return Ended::Stopped,
            Err(e) => return Ended::Error(e),
        };
        // Post-transfer work is not cancellable: it runs to completion, and a
        // stop that raced a committed install is honoured as completed.
        let committed = if plan.has_multiple {
            let now = self.clock.now_iso();
            self.mutate_job(plan.key, |job| job.begin_extract(&now)).await;
            self.post_download_multi(plan, skipped).await
        } else if plan.key.kind == JobKind::Firmware {
            self.post_download_firmware(plan, &tmp).await
        } else {
            self.post_download_single(plan).await
        };
        match committed {
            Ok(path) => Ended::Committed(path),
            Err(_) if control.token.is_cancelled() => Ended::Stopped,
            Err(e) => Ended::Error(e),
        }
    }

    async fn do_download(self: Arc<Self>, plan: Plan, control: Arc<Control>, resume: bool) {
        let ended = self.run_attempt(&plan, &control, resume).await;
        let key = plan.key;
        match ended {
            Ended::Committed(path) => {
                if control.token.is_cancelled() {
                    tracing::info!(
                        "Download cancelled after install committed; surfaced as complete: {}",
                        plan.rom_name
                    );
                }
                self.finalize_download_complete(&plan, &path).await;
            }
            Ended::Stopped if control.shutdown.load(Ordering::SeqCst) => {
                // Plugin shutdown: keep the partial AND the row as they are;
                // the next start reconciles and resumes it. Nothing is emitted.
                tracing::info!("Download stopped by plugin shutdown; will resume on restart: {}", plan.rom_name);
            }
            Ended::Stopped if control.paused.load(Ordering::SeqCst) => {
                let now = self.clock.now_iso();
                self.mutate_job(key, |job| job.pause(&now)).await;
                self.update_entry(key, |e| {
                    e.status = "paused".into();
                    e.reason = None;
                });
                self.emit_entry(key);
                tracing::info!("Download paused: {}", plan.rom_name);
            }
            Ended::Stopped => {
                self.update_entry(key, |e| {
                    e.status = "cancelled".into();
                    e.reason = None;
                });
                self.drop_job(key).await;
                self.cleanup_partial_download(&plan).await;
                self.emit_entry(key);
                tracing::info!("Download cancelled: {}", plan.rom_name);
            }
            Ended::Error(e) => match classify_download_failure(&e) {
                Outcome::Interrupted(why) => {
                    tracing::warn!("Download interrupted for {} ({why}): {e:#}", plan.rom_name);
                    if why == INTERRUPT_NEEDS_RECOVERY {
                        // The partial stays untouched; it is no longer auto-resumed.
                        let now = self.clock.now_iso();
                        self.mutate_job(key, |job| {
                            job.flag_needs_recovery(&now);
                            Ok(())
                        })
                        .await;
                    }
                    self.mark_interrupted(key, why).await;
                }
                Outcome::Failed => {
                    let message = format!("{e:#}");
                    self.update_entry(key, |entry| {
                        entry.status = "failed".into();
                        entry.error = Some(message.clone());
                        entry.reason = None;
                    });
                    self.drop_job(key).await;
                    self.cleanup_partial_download(&plan).await;
                    tracing::error!("Download failed for {}: {e:#}", plan.rom_name);
                    let mut payload = json!({
                        "rom_name": plan.rom_name,
                        "platform_name": plan.platform_name,
                        "error_message": message,
                    });
                    set_identity(&mut payload, key);
                    self.emit("download_failed", payload);
                }
            },
        }
        self.release_slot(control.id);
        let mut st = self.st();
        // A newer attempt may already own these registrations.
        if st.attempts.get(&key).is_some_and(|a| Arc::ptr_eq(&a.control, &control)) {
            st.attempts.remove(&key);
            st.in_progress.remove(&key);
            st.reserved.remove(&key);
        }
        st.prune();
    }

    /// Remove only the transient transfer artifacts and, for a multi-file
    /// ROM, the extract folder this download created. Never the bare target:
    /// that would destroy a pre-existing install.
    async fn cleanup_partial_download(&self, plan: &Plan) {
        if plan.key.kind == JobKind::Firmware {
            // A firmware download only ever owns its `<dest>.tmp`; the destination may be the user's own file.
            let tmp = format!("{}{TMP_EXT}", plan.target_path);
            if let Err(e) = self.files.remove_file(&tmp).await {
                tracing::warn!("Cleanup failed for {tmp}: {e}");
            }
            return;
        }
        for suffix in [ZIP_TMP_EXT, TMP_EXT] {
            let path = format!("{}{suffix}", plan.target_path);
            if let Err(e) = self.files.remove_file(&path).await {
                tracing::warn!("Cleanup failed for {path}: {e}");
            }
        }
        if plan.has_multiple {
            let staging = join(dirname(&plan.target_path), &[&plan.extract_dir_name]);
            if let Err(e) = self.files.remove_tree(&staging).await {
                tracing::warn!("Cleanup failed for directory {staging}: {e}");
            }
        }
    }

    /// Mark the entry completed and emit `download_complete` with the bound
    /// shortcut's app id and the re-baked launch command.
    async fn finalize_download_complete(&self, plan: &Plan, final_path: &str) {
        let key = plan.key;
        if key.kind == JobKind::Firmware {
            self.finalize_firmware_complete(plan, final_path).await;
            return;
        }
        let rom_id = plan.rom_id;
        self.drop_job(key).await;
        let mut resumable = false;
        self.update_entry(key, |e| {
            e.status = "completed".into();
            e.progress = 1.0;
            e.reason = None;
            resumable = e.resumable;
        });
        let (app_id, launch_options) = self.bound_launch(rom_id, final_path).await;
        self.emit(
            "download_complete",
            json!({
                "rom_id": rom_id,
                "rom_name": plan.rom_name,
                "platform_name": plan.platform_name,
                "file_path": final_path,
                "app_id": app_id,
                "launch_options": launch_options,
                "resumable": resumable,
            }),
        );
        tracing::info!("Download complete: {} -> {final_path}", plan.rom_name);
    }

    /// `(shortcut_app_id, launch_options)` for a just-installed ROM: the full
    /// active emulator and the (pinned or largest) launch file.
    async fn bound_launch(&self, rom_id: i64, file_path: &str) -> (Option<i64>, String) {
        let rom = self.library.rom(rom_id).await.ok().flatten();
        let install = self.library.install(rom_id).await.ok().flatten();
        let Some(rom) = rom else {
            return (None, self.launch.launch_options(None, file_path));
        };
        let emulator = self.launch.active_emulator_for_rom(rom_id, None).await;
        let bake = match &install {
            Some(install) => self.launch.resolve_bake_path(install, rom.selected_disc.as_deref()).await,
            None => file_path.to_string(),
        };
        (rom.shortcut_app_id, self.launch.launch_options(emulator.as_ref(), &bake))
    }

    // ── Callables ───────────────────────────────────────────────────────

    /// Cancel a download: abort a live one, or discard a paused/interrupted one.
    pub fn cancel_download(self: &Arc<Self>, rom_id: i64) -> Value {
        self.cancel_job(JobKey::rom(rom_id))
    }

    /// Cancel a download of any kind.
    pub fn cancel_job(self: &Arc<Self>, key: JobKey) -> Value {
        let st = self.st();
        if let Some(attempt) = st.attempts.get(&key) {
            attempt.control.stop(&attempt.control.cancelled);
            return json!({"success": true, "message": "Download cancelled"});
        }
        let stopped = st
            .queue
            .get(&key)
            .is_some_and(|e| e.status == "paused" || e.status == "interrupted");
        drop(st);
        if stopped {
            let me = self.clone();
            tokio::spawn(async move { me.discard_resumable(key, "cancelled", None).await });
            return json!({"success": true, "message": "Download cancelled"});
        }
        failure("no_active_download", "No active download for this ROM")
    }

    /// Pause a live download, keeping the partial for resume.
    pub fn pause_download(&self, rom_id: i64) -> Value {
        self.pause_job(JobKey::rom(rom_id))
    }

    /// Pause a live download of any kind.
    pub fn pause_job(&self, key: JobKey) -> Value {
        match self.st().attempts.get(&key) {
            Some(attempt) => {
                attempt.control.stop(&attempt.control.paused);
                json!({"success": true, "message": "Download paused"})
            }
            None => failure("no_active_download", "No active download for this ROM"),
        }
    }

    pub fn get_download_queue(&self) -> Value {
        let (mut entries, positions): (Vec<QueueEntry>, HashMap<JobKey, i64>) = {
            let st = self.st();
            (st.queue.values().cloned().collect(), st.positions.clone())
        };
        for entry in &mut entries {
            if entry.status != "downloading" {
                entry.clear_phase(); // a stopped download is not waiting for anything
            }
        }
        entries.sort_by_key(|e| (positions.get(&e.key()).copied().unwrap_or(i64::MAX), e.item_id));
        let rows: Vec<Value> = entries.iter().map(QueueEntry::row).collect();
        json!({"downloads": rows})
    }

    /// The install record of `rom_id` in the frontend `InstalledRom` shape, or `null`.
    pub async fn get_installed_rom(&self, rom_id: i64) -> anyhow::Result<Value> {
        Ok(match self.library.install(rom_id).await? {
            None => Value::Null,
            Some(install) => json!({
                "rom_id": install.rom_id,
                "file_name": basename(&install.file_path),
                "file_path": install.file_path,
                "system": install.system,
                "platform_slug": install.platform_slug,
                "installed_at": install.installed_at,
            }),
        })
    }

    /// ROM ids with a live or queued download (paused/interrupted excluded).
    pub fn active_download_rom_ids(&self) -> HashSet<i64> {
        self.st()
            .in_progress
            .iter()
            .filter(|k| k.kind == JobKind::Rom)
            .map(|k| k.id)
            .collect()
    }

    /// Remove the queue entry and job row of `rom_id`. A paused or
    /// interrupted download's partial goes with it; a live transfer's partial
    /// is left to that transfer.
    pub async fn evict(&self, rom_id: i64) {
        self.evict_job(JobKey::rom(rom_id)).await;
    }

    pub async fn evict_job(&self, key: JobKey) {
        let stopped_partial = {
            let mut st = self.st();
            let entry = st.queue.shift_remove(&key);
            entry.is_some_and(|e| e.status == "paused" || e.status == "interrupted")
                && !st.attempts.contains_key(&key)
        };
        if stopped_partial && let Some(job) = self.load_job(key).await {
            self.remove_tmp_files(&[job.tmp_path]).await;
        }
        self.drop_job(key).await;
    }

    /// Plugin unload: stop every transfer keeping its partial and its row.
    pub async fn shutdown(&self) {
        let handles: Vec<JoinHandle<()>> = {
            let mut st = self.st();
            if let Some(task) = st.auto_resume.take() {
                task.abort();
            }
            st.attempts
                .values_mut()
                .filter_map(|a| {
                    a.control.stop(&a.control.shutdown);
                    a.handle.take()
                })
                .collect()
        };
        for handle in handles {
            let _ = tokio::time::timeout(SHUTDOWN_GRACE, handle).await;
        }
    }
}

#[cfg(test)]
#[path = "downloads_tests.rs"]
mod tests;
