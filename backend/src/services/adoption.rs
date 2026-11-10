//! InstallAdoptionService — treat ROM files already on disk as installed.
//!
//! A ROM counts as installed only when a `rom_installs` row exists. Files that
//! reached the ROM folders some other way (copied by hand, or downloaded by an
//! older plugin) are adopted here: when every file sits exactly where this
//! plugin's download would put it, with exactly RomM's size, the same
//! `RomInstall` a finished download writes is recorded. For systems whose
//! folder installs launch a file picked by extension (Switch), a ROM the exact
//! rule misses can still pair one-to-one with a system-folder entry by
//! normalized title. Adoption never touches the files.
//!
//! Entry points: [`InstallAdoptionService::plan_adoptions`] +
//! [`InstallAdoptionService::commit_adoptions`] for library sync, and
//! [`InstallAdoptionService::adopt_rom`] / `schedule_adoption_check` for one
//! ROM (the game page).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};

use indexmap::IndexMap;
use serde_json::{Value, json};

use crate::domain::emulator_table::launch_file_extensions;
use crate::domain::install_layout::{
    AdoptionCandidate, InstallLayout, expected_install_layout, multi_file_matches, pick_one_per_group,
    single_file_matches,
};
use crate::domain::rom_files::detect_launch_file;
use crate::domain::rom_install::RomInstall;
use crate::domain::title_match::{DiskEntry, TitleMatch, normalize_title, pair_by_title};
use crate::services::launch::LaunchService;
use crate::services::ports::{
    Clock, DownloadFiles, DownloadJobRepo, EmuDeckPaths, EventSink, LibraryRepo, RommRomApi, SystemResolver,
};
use crate::util::path_safety::{basename, join, safe_join, safe_join_lexical};

/// One RomM round-trip per ROM per window is plenty for a game page.
const CHECK_COOLDOWN_S: f64 = 30.0;

/// The rom ids with a live or queued download (the downloader's snapshot).
pub type ActiveDownloads = Arc<dyn Fn() -> HashSet<i64> + Send + Sync>;

/// Everything [`InstallAdoptionService`] depends on.
pub struct InstallAdoptionServiceConfig {
    pub romm: Arc<dyn RommRomApi>,
    pub files: Arc<dyn DownloadFiles>,
    pub systems: Arc<dyn SystemResolver>,
    pub paths: Arc<dyn EmuDeckPaths>,
    pub launch: Arc<LaunchService>,
    pub library: Arc<dyn LibraryRepo>,
    pub jobs: Arc<dyn DownloadJobRepo>,
    pub active_downloads: ActiveDownloads,
    pub events: Arc<dyn EventSink>,
    pub clock: Arc<dyn Clock>,
}

/// Per-ROM DB facts read before any filesystem probe.
#[derive(Debug, Clone)]
struct Eligibility {
    blocked: bool,
    group_key: Option<String>,
    selected_disc: Option<String>,
}

pub struct InstallAdoptionService {
    romm: Arc<dyn RommRomApi>,
    files: Arc<dyn DownloadFiles>,
    systems: Arc<dyn SystemResolver>,
    paths: Arc<dyn EmuDeckPaths>,
    launch: Arc<LaunchService>,
    library: Arc<dyn LibraryRepo>,
    jobs: Arc<dyn DownloadJobRepo>,
    active_downloads: ActiveDownloads,
    events: Arc<dyn EventSink>,
    clock: Arc<dyn Clock>,
    in_flight: Mutex<HashSet<i64>>,
    last_check: Mutex<HashMap<i64, f64>>,
}

fn rom_id_of(rom: &Value) -> Option<i64> {
    rom.get("id").and_then(Value::as_i64)
}

fn str_of<'a>(rom: &'a Value, key: &str) -> Option<&'a str> {
    rom.get(key).and_then(Value::as_str)
}

impl InstallAdoptionService {
    pub fn new(config: InstallAdoptionServiceConfig) -> Arc<Self> {
        Arc::new(Self {
            romm: config.romm,
            files: config.files,
            systems: config.systems,
            paths: config.paths,
            launch: config.launch,
            library: config.library,
            jobs: config.jobs,
            active_downloads: config.active_downloads,
            events: config.events,
            clock: config.clock,
            in_flight: Mutex::new(HashSet::new()),
            last_check: Mutex::new(HashMap::new()),
        })
    }

    // ── Eligibility (DB) ────────────────────────────────────────────────

    async fn sibling_installed(&self, rom_id: i64, group_key: Option<&str>) -> anyhow::Result<bool> {
        let Some(key) = group_key else {
            return Ok(false);
        };
        for member in self.library.roms_in_group(key).await? {
            if member.rom_id != rom_id && self.library.install(member.rom_id).await?.is_some() {
                return Ok(true);
            }
        }
        Ok(false)
    }

    async fn read_eligibility(&self, rom_id: i64, rom: &Value) -> anyhow::Result<Eligibility> {
        let row = self.library.rom(rom_id).await?;
        let group_key = str_of(rom, "sibling_group_key")
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .or_else(|| row.as_ref().and_then(|r| r.sibling_group_key.clone()));
        let blocked = self.library.install(rom_id).await?.is_some()
            || self.jobs.get(crate::domain::download_job::JobKey::rom(rom_id)).await?.is_some()
            || self.sibling_installed(rom_id, group_key.as_deref()).await?;
        Ok(Eligibility {
            blocked,
            group_key,
            selected_disc: row.and_then(|r| r.selected_disc),
        })
    }

    // ── Filesystem probe ────────────────────────────────────────────────

    fn system_of(&self, rom: &Value) -> (String, String) {
        let slug = str_of(rom, "platform_slug").unwrap_or_default().to_string();
        let system = self.systems.resolve_system(&slug, str_of(rom, "platform_fs_slug"));
        (slug, system)
    }

    /// The system folder under the configured roms root (its own spelling), or
    /// `None` when the system name would escape it.
    fn system_dir(&self, system: &str) -> Option<String> {
        safe_join_lexical(&self.paths.roms_path(), &[system]).ok()
    }

    fn build_install(&self, rom: &Value, file_path: String, rom_dir: Option<String>, slug: &str, system: &str) -> Option<RomInstall> {
        RomInstall::mark_installed(
            rom_id_of(rom)?,
            file_path,
            rom_dir,
            slug.to_string(),
            system.to_string(),
            self.clock.now_iso(),
        )
        .ok()
    }

    /// The install a download would have written, when the files match exactly.
    async fn probe_install(&self, rom: &Value) -> Option<RomInstall> {
        let layout = expected_install_layout(rom)?;
        let (slug, system) = self.system_of(rom);
        let roms_dir = self.system_dir(&system)?;
        match layout {
            InstallLayout::Single { file_name, size } => {
                safe_join(&roms_dir, &[&file_name]).ok()?;
                let path = join(&roms_dir, &[&file_name]);
                if !single_file_matches(size, self.files.regular_file_size(&path).await) {
                    return None;
                }
                self.build_install(rom, path, None, &slug, &system)
            }
            InstallLayout::Multi { dir_candidates, files } => {
                for dir_name in dir_candidates {
                    let rom_dir = join(&roms_dir, &[&dir_name]);
                    if !self.files.is_dir(&rom_dir).await {
                        continue;
                    }
                    let mut observed = HashMap::new();
                    for expected in &files {
                        let size = match safe_join(&rom_dir, &expected.parts()) {
                            Ok(path) => self.files.regular_file_size(&path.to_string_lossy()).await,
                            Err(_) => None,
                        };
                        observed.insert(expected.rel_path.clone(), size);
                    }
                    if !multi_file_matches(&files, &observed) {
                        continue;
                    }
                    let scanned = self.files.scan_files_with_sizes(&rom_dir).await;
                    let launch = detect_launch_file(&scanned, false).unwrap_or_else(|| rom_dir.clone());
                    return self.build_install(rom, launch, Some(rom_dir), &slug, &system);
                }
                None
            }
        }
    }

    async fn candidate(&self, install: RomInstall, facts: &Eligibility) -> AdoptionCandidate {
        let bake_path = self
            .launch
            .resolve_bake_path(&install, facts.selected_disc.as_deref())
            .await;
        AdoptionCandidate {
            rom_id: install.rom_id,
            install,
            bake_path,
            group_key: facts.group_key.clone(),
        }
    }

    // ── Library-sync entry points ───────────────────────────────────────

    /// The ROMs among `roms` (RomM payloads) whose files are already on disk
    /// and not yet adopted. Skips installed ROMs, ROMs with a download job or
    /// an active download, and ROMs whose sibling group has an installed
    /// version; at most one version per group. Writes nothing.
    pub async fn plan_adoptions(&self, roms: &[Value]) -> Vec<AdoptionCandidate> {
        if roms.is_empty() {
            return Vec::new();
        }
        let mut eligibility = HashMap::new();
        for rom in roms {
            let Some(id) = rom_id_of(rom) else {
                continue;
            };
            match self.read_eligibility(id, rom).await {
                Ok(facts) => {
                    eligibility.insert(id, facts);
                }
                Err(e) => tracing::warn!("Install adoption: reading rom {id} failed: {e:#}"),
            }
        }
        let active = (self.active_downloads)();
        let mut candidates = Vec::new();
        let mut unmatched = Vec::new();
        for rom in roms {
            let Some(id) = rom_id_of(rom) else {
                continue;
            };
            let Some(facts) = eligibility.get(&id) else {
                continue;
            };
            if facts.blocked || active.contains(&id) {
                continue;
            }
            match self.probe_install(rom).await {
                Some(install) => candidates.push(self.candidate(install, facts).await),
                None => unmatched.push(rom),
            }
        }
        let fallback = self.plan_title_fallback(&unmatched, roms, &candidates, &eligibility).await;
        candidates.extend(fallback);
        pick_one_per_group(candidates)
    }

    /// Pair exact-rule misses with system-folder entries by normalized title,
    /// only for systems with launch-file extensions (Switch).
    async fn plan_title_fallback(
        &self,
        unmatched: &[&Value],
        batch: &[Value],
        exact: &[AdoptionCandidate],
        eligibility: &HashMap<i64, Eligibility>,
    ) -> Vec<AdoptionCandidate> {
        let mut by_system: IndexMap<(String, String), Vec<&Value>> = IndexMap::new();
        for rom in unmatched {
            let key = self.system_of(rom);
            if launch_file_extensions(Some(&key.1)).is_some() {
                by_system.entry(key).or_default().push(rom);
            }
        }
        let mut out = Vec::new();
        for ((slug, system), misses) in by_system {
            match self
                .title_fallback_for_system(&slug, &system, &misses, batch, exact, eligibility)
                .await
            {
                Ok(found) => out.extend(found),
                Err(e) => tracing::warn!("Install adoption title match failed for {system}: {e:#}"),
            }
        }
        out
    }

    async fn title_fallback_for_system(
        &self,
        slug: &str,
        system: &str,
        misses: &[&Value],
        batch: &[Value],
        exact: &[AdoptionCandidate],
        eligibility: &HashMap<i64, Eligibility>,
    ) -> anyhow::Result<Vec<AdoptionCandidate>> {
        let (Some(extensions), Some(roms_dir)) = (launch_file_extensions(Some(system)), self.system_dir(system)) else {
            return Ok(Vec::new());
        };
        // RomM side: every synced ROM of the platform, overlaid with this
        // batch (a first sync has no rows yet), so a shared title is ambiguous.
        let mut romm_side: BTreeMap<i64, String> = self
            .library
            .roms_on_platform(slug)
            .await?
            .into_iter()
            .map(|r| (r.rom_id, r.fs_name))
            .collect();
        for rom in batch {
            if str_of(rom, "platform_slug").unwrap_or_default() == slug
                && let (Some(id), Some(fs_name)) = (rom_id_of(rom), str_of(rom, "fs_name"))
            {
                romm_side.insert(id, fs_name.to_string());
            }
        }
        let mut claimed: HashSet<String> = self
            .library
            .installs()
            .await?
            .iter()
            .filter(|i| i.system == system)
            .map(|i| basename(i.rom_dir.as_deref().unwrap_or(&i.file_path)).to_string())
            .collect();
        claimed.extend(
            exact
                .iter()
                .map(|c| basename(c.install.rom_dir.as_deref().unwrap_or(&c.install.file_path)).to_string()),
        );
        let wanted: HashSet<String> = misses
            .iter()
            .map(|r| normalize_title(str_of(r, "fs_name").unwrap_or_default()))
            .filter(|t| !t.is_empty())
            .collect();
        if wanted.is_empty() {
            return Ok(Vec::new());
        }
        let mut entries = Vec::new();
        for (name, is_dir, size) in self.files.list_entries(&roms_dir).await {
            let is_claimed = claimed.contains(&name);
            if !is_dir {
                entries.push(DiskEntry {
                    files: vec![(name.clone(), size)],
                    name,
                    is_dir: false,
                    claimed: is_claimed,
                });
            } else if wanted.contains(&normalize_title(&name)) {
                let children = self.files.list_entries(&join(&roms_dir, &[&name])).await;
                entries.push(DiskEntry {
                    files: children.into_iter().filter(|c| !c.1).map(|(n, _, s)| (n, s)).collect(),
                    name,
                    is_dir: true,
                    claimed: is_claimed,
                });
            }
        }
        let miss_by_id: HashMap<i64, &Value> = misses.iter().filter_map(|r| Some((rom_id_of(r)?, *r))).collect();
        let romm_side: Vec<(i64, String)> = romm_side.into_iter().collect();
        let mut out = Vec::new();
        for m in pair_by_title(&romm_side, &entries, extensions) {
            let (Some(rom), Some(facts)) = (miss_by_id.get(&m.rom_id), eligibility.get(&m.rom_id)) else {
                continue;
            };
            if let Some(install) = self.title_install(rom, &m, &roms_dir, slug, system) {
                tracing::info!("Install adoption: rom {} matched '{}' by title", m.rom_id, m.entry_name);
                out.push(self.candidate(install, facts).await);
            }
        }
        Ok(out)
    }

    /// The install a download of `m` would record (file, or folder + launch file).
    fn title_install(&self, rom: &Value, m: &TitleMatch, roms_dir: &str, slug: &str, system: &str) -> Option<RomInstall> {
        let entry_path = safe_join_lexical(roms_dir, &[&m.entry_name]).ok()?;
        if !m.is_dir {
            return self.build_install(rom, entry_path, None, slug, system);
        }
        let launch = safe_join_lexical(&entry_path, &[&m.launch_file]).ok()?;
        self.build_install(rom, launch, Some(entry_path), slug, system)
    }

    /// Persist the candidates' install rows, re-checking every guard inside
    /// the write (row exists, no install, no job, no installed sibling) so a
    /// download or sibling adoption that landed since planning wins.
    pub async fn commit_adoptions(&self, candidates: &[AdoptionCandidate]) -> Vec<i64> {
        let active = (self.active_downloads)();
        let mut adopted = Vec::new();
        for cand in candidates {
            if active.contains(&cand.rom_id) {
                continue;
            }
            match self.library.adopt_install(&cand.install, cand.group_key.as_deref()).await {
                Ok(true) => {
                    tracing::info!("Adopted existing files as installed: rom {}", cand.rom_id);
                    adopted.push(cand.rom_id);
                }
                Ok(false) => {}
                Err(e) => tracing::warn!("Install adoption of rom {} failed: {e:#}", cand.rom_id),
            }
        }
        adopted
    }

    // ── Single ROM (game page) ──────────────────────────────────────────

    /// Start a background adoption check for `rom_id`, deduped and
    /// rate-limited to one per 30 s. Never fails.
    pub fn schedule_adoption_check(self: &Arc<Self>, rom_id: i64) {
        let now = self.clock.monotonic();
        {
            let mut last = self.last_check.lock().unwrap_or_else(|p| p.into_inner());
            if self.in_flight.lock().unwrap_or_else(|p| p.into_inner()).contains(&rom_id) {
                return;
            }
            if last.get(&rom_id).is_some_and(|t| now - t < CHECK_COOLDOWN_S) {
                return;
            }
            last.insert(rom_id, now);
            self.in_flight.lock().unwrap_or_else(|p| p.into_inner()).insert(rom_id);
        }
        let me = self.clone();
        tokio::spawn(async move {
            me.adopt_rom(rom_id).await;
            me.in_flight.lock().unwrap_or_else(|p| p.into_inner()).remove(&rom_id);
        });
    }

    async fn quick_blocked(&self, rom_id: i64) -> anyhow::Result<bool> {
        if (self.active_downloads)().contains(&rom_id) {
            return Ok(true);
        }
        let Some(row) = self.library.rom(rom_id).await? else {
            return Ok(true);
        };
        Ok(self.library.install(rom_id).await?.is_some()
            || self.jobs.get(crate::domain::download_job::JobKey::rom(rom_id)).await?.is_some()
            || self.sibling_installed(rom_id, row.sibling_group_key.as_deref()).await?)
    }

    /// Adopt one synced ROM's on-disk files, then announce it like a finished
    /// download (`download_complete` with `adopted: true` and the resolved
    /// launch command). Returns whether it adopted; never fails.
    pub async fn adopt_rom(&self, rom_id: i64) -> bool {
        match self.quick_blocked(rom_id).await {
            Ok(false) => {}
            Ok(true) => return false,
            Err(e) => {
                tracing::warn!("Install adoption check failed for rom {rom_id}: {e:#}");
                return false;
            }
        }
        let rom = match self.romm.get_rom(rom_id).await {
            Ok(rom) => rom,
            Err(e) => {
                tracing::warn!("Install adoption check failed for rom {rom_id}: {e}");
                return false;
            }
        };
        if rom_id_of(&rom) != Some(rom_id) {
            return false;
        }
        let candidates = self.plan_adoptions(std::slice::from_ref(&rom)).await;
        let Some(first) = candidates.first() else {
            return false;
        };
        if !self.commit_adoptions(&candidates).await.contains(&rom_id) {
            return false;
        }
        let item = self.launch.relaunch_item_for_rom(rom_id).await;
        self.events.emit(
            "download_complete",
            json!({
                "rom_id": rom_id,
                "rom_name": str_of(&rom, "name").unwrap_or_default(),
                "platform_name": str_of(&rom, "platform_name")
                    .or_else(|| str_of(&rom, "platform_slug"))
                    .unwrap_or_default(),
                "file_path": first.install.file_path,
                "app_id": item.as_ref().map_or(Value::Null, |i| i["app_id"].clone()),
                "launch_options": item.as_ref().map_or(json!(""), |i| i["launch_options"].clone()),
                "resumable": false,
                "adopted": true,
            }),
        );
        true
    }
}

#[cfg(test)]
#[path = "adoption_tests.rs"]
mod tests;
