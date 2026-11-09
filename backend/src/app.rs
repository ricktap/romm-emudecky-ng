//! Composition root: the only place concrete adapters meet services, plus the
//! startup work (database migrations, settings load/upgrade, legacy credential
//! migration) and the callable table the protocol server serves.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use serde_json::{Value, json};

use crate::adapters::db::{DB_FILENAME, Db};
use crate::adapters::download_files::LocalFiles;
use crate::adapters::emudeck::{EmuDeckSettingsPaths, HomeEmulatorLocator, PlatformMap, SystemClock};
use crate::adapters::persistence::{EpochClock, FileSettingsPersister, SettingsFile};
use crate::adapters::plugin_metadata::read_plugin_version;
use crate::adapters::repositories::{SqliteDownloadJobs, SqliteKvConfig, SqliteLibrary, SqliteRomRegions};
use crate::adapters::romm::api::RommApi;
use crate::adapters::romm::http::{RetryListener, RetryPolicy, RommHttp};
use crate::domain::errors::unknown_failure;
use crate::domain::settings::{RESET_NOTICE_KEY, fold_legacy_save_sync_settings, migrate_settings, stored_version};
use crate::domain::version::MIN_REQUIRED_VERSION;
use crate::rpc::server::{EventHub, Handler, Registry};
use crate::services::adoption::{InstallAdoptionService, InstallAdoptionServiceConfig};
use crate::services::connection::{ConnectionService, ConnectionServiceConfig};
use crate::services::downloads::{DownloadService, DownloadServiceConfig};
use crate::services::launch::{LaunchService, LaunchServiceConfig};
use crate::services::library::{LibraryService, LibraryServiceConfig};
use crate::services::ports::{EventSink, SettingsPersister, SettingsStore};
use crate::services::rom_removal::{RomRemovalService, RomRemovalServiceConfig};
use crate::services::settings::{SettingsService, SettingsServiceConfig};

/// Every callable name the frontend may call — the same surface as the Python
/// `Backend` (kept in sync by `tests/test_rust_backend_callables.py`).
const CALLABLE_NAMES_JSON: &str = include_str!("../callables.json");

/// Callables this backend implements; every other listed name answers
/// "not implemented in the Rust backend yet".
pub const PORTED_CALLABLES: &[&str] = &[
    "get_version_list", "switch_version", "evaluate_launch", "check_local_drift", "check_core_change",
    "remove_rom", "uninstall_all_roms", "apply_steam_input_setting", "check_for_update",
    "record_session_start", "finalize_game_session", "get_all_playtime", "get_playtime_scope_notice", "reconcile_playtime",

    "get_cached_game_detail", "get_rom_metadata", "get_all_metadata_cache", "fetch_cover_base64", "refresh_cover_artwork",
    "get_sgdb_artwork_base64", "verify_sgdb_api_key", "save_sgdb_api_key", "get_sgdb_resolution", "search_sgdb_games", "apply_sgdb_game_id",
    "get_platform_core_info", "set_game_core", "clear_game_core", "get_disc_selection", "select_disc",
    "get_rom_relaunch_options", "get_installed_relaunch_options",
    "get_artwork_base64",
    "remove_all_shortcuts", "remove_platform_shortcuts", "report_removal_results", "reconcile_shortcuts", "save_shortcut_icon",
    "sync_preview", "sync_apply_delta", "sync_cancel_preview", "start_sync", "cancel_sync", "get_sync_status", "sync_heartbeat", "report_unit_results",
    "cancel_download",
    "connect_with_credentials",
    "connect_with_pairing_code",
    "connect_with_token",
    "debug_log",
    "dismiss_settings_reset_notice",
    "frontend_log",
    "get_download_queue",
    "get_app_id_rom_id_map",
    "get_collections",
    "get_installed_rom",
    "get_known_regions",
    "get_platforms",
    "get_registry_platforms",
    "get_settings",
    "get_settings_reset_notice",
    "get_sync_stats",
    "get_whitelist_settings",
    "pause_download",
    "probe_reachability",
    "clear_sync_cache",
    "resume_download",
    "save_collection_platform_groups",
    "save_collection_sync",
    "save_log_level",
    "save_platform_sync",
    "save_preferred_region",
    "save_server_url",
    "save_steam_input_setting",
    "set_all_collections_sync",
    "set_all_platforms_sync",
    "sign_out",
    "start_download",
    "test_connection",
    "update_whitelist_settings",
];

/// The full callable surface, sorted.
pub fn callable_names() -> Vec<String> {
    let mut names: Vec<String> =
        serde_json::from_str(CALLABLE_NAMES_JSON).expect("callables.json is a JSON string list");
    names.sort();
    names
}

/// Where the backend runs and how it reaches its host.
pub struct BackendEnv {
    pub settings_dir: PathBuf,
    pub runtime_dir: PathBuf,
    pub plugin_dir: PathBuf,
    pub user_home: PathBuf,
    pub retry: RetryPolicy,
    pub clock: EpochClock,
}

/// Backend events reach the shim through the protocol's event hub.
struct HubEvents(Arc<EventHub>);

impl EventSink for HubEvents {
    fn emit(&self, event: &str, payload: Value) {
        self.0.emit(event, vec![payload]);
    }
}

/// The wired services.
pub struct Backend {
    pub settings: SettingsStore,
    pub settings_service: Arc<SettingsService>,
    pub connection_service: Arc<ConnectionService>,
    pub launch: Arc<LaunchService>,
    pub launch_choices: Arc<crate::services::launch_choices::LaunchChoicesService>,
    pub game_detail: Arc<crate::services::game_detail::GameDetailService>,
    pub game_covers: Arc<crate::services::game_covers::GameCoverService>,
    pub steamgrid: Arc<crate::services::steamgrid::SteamGridService>,
    pub versions: Arc<crate::services::versions::VersionService>,
    pub launch_gate: Arc<crate::services::launch_gate::LaunchGate>,
    pub game_actions: Arc<crate::services::game_actions::GameActions>,
    pub playtime: Arc<crate::services::playtime::PlaytimeService>,
    pub update: Arc<crate::services::update::UpdateService>,
    pub downloads: Arc<DownloadService>,
    pub library: Arc<LibraryService>,
    pub sync: Arc<crate::services::sync_orchestrator::SyncOrchestrator>,
    pub shortcuts: Arc<crate::services::shortcut_removal::ShortcutRemovalService>,
    /// Adoption of ROM files already on disk. Library sync and the game page
    /// (rewrite step 3) call it; no callable reaches it yet.
    pub adoption: Arc<InstallAdoptionService>,
}

impl Backend {
    /// Wire adapters and services and run the startup work. Fails (and the
    /// plugin stays inert) when the database cannot be opened or migrated or
    /// the settings file cannot be read or written.
    pub async fn start(env: BackendEnv, hub: Arc<EventHub>) -> anyhow::Result<Self> {
        let runtime=Arc::new(crate::adapters::emudeck::SyncRuntime::default());
        Self::start_with_sync_runtime(env,hub,Arc::new(SystemClock::default()),runtime.clone(),runtime).await
    }

    /// Alternate runtime ports let lifecycle tests advance time without sleeping
    /// through production heartbeat/preview TTLs. All persistence and HTTP are real.
    pub async fn start_with_sync_runtime(env:BackendEnv,hub:Arc<EventHub>,clock:Arc<dyn crate::services::ports::Clock>,ids:Arc<dyn crate::services::ports::SyncIds>,sleeper:Arc<dyn crate::services::ports::SyncSleeper>)->anyhow::Result<Self> {
        let (db, version) = Db::open(&env.runtime_dir.join(DB_FILENAME))
            .await
            .context("SQLite schema migration failed; plugin cannot start")?;
        tracing::info!("SQLite schema at version {version}");

        let file = Arc::new(SettingsFile::new(
            &env.settings_dir,
            &env.runtime_dir,
            env.clock.clone(),
        ));
        let settings = load_and_upgrade_settings(file.clone()).await?;
        let persister: Arc<dyn SettingsPersister> = Arc::new(FileSettingsPersister::new(file, settings.clone()));
        persister.save_settings().await.context("writing settings.json")?;

        let user_agent = format!("romm-emudecky-ng/{}", read_plugin_version(&env.plugin_dir).await);
        let retry_hub = hub.clone();
        let on_retry: RetryListener = Arc::new(move |attempt, max_attempts, delay_s| {
            retry_hub.emit(
                "server_retry_progress",
                vec![json!({"attempt": attempt, "max_attempts": max_attempts, "delay_s": delay_s})],
            );
        });
        let http = Arc::new(RommHttp::new(settings.clone(), user_agent.clone(), env.retry, Some(on_retry))?);
        let romm_api = Arc::new(RommApi::new(http));
        let events: Arc<dyn EventSink> = Arc::new(HubEvents(hub.clone()));

        let kv_config = Arc::new(SqliteKvConfig::new(db.clone()));
        let settings_service = Arc::new(SettingsService::new(SettingsServiceConfig {
            settings: settings.clone(),
            persister: persister.clone(),
            regions: Arc::new(SqliteRomRegions::new(db.clone())),
        }));
        let connection_service = Arc::new(ConnectionService::new(ConnectionServiceConfig {
            settings: settings.clone(),
            romm_api: romm_api.clone(),
            persister: persister.clone(),
            kv_config: kv_config.clone(),
            min_required_version: MIN_REQUIRED_VERSION,
        }));

        let files = Arc::new(LocalFiles);
        let paths = Arc::new(EmuDeckSettingsPaths::new(&env.user_home));
        let systems = Arc::new(PlatformMap::load(&env.plugin_dir));
        let library = Arc::new(SqliteLibrary::new(db.clone()));
        let library_service = Arc::new(LibraryService::new(LibraryServiceConfig {
            romm: romm_api.clone(),
            settings: settings.clone(),
            persister: persister.clone(),
            library: library.clone(),
            kv_config: kv_config.clone(),
            clock: clock.clone(),
        }));
        let jobs = Arc::new(SqliteDownloadJobs::new(db.clone()));
        let launch = Arc::new(LaunchService::new(LaunchServiceConfig {
            library: library.clone(),
            locator: Arc::new(HomeEmulatorLocator::new(&env.user_home)),
            systems: systems.clone(),
            files: files.clone(),
            settings: settings.clone(),
        }));
        let removal = Arc::new(RomRemovalService::new(RomRemovalServiceConfig {
            library: library.clone(),
            files: files.clone(),
            paths: paths.clone(),
        }));
        let launch_choices = Arc::new(crate::services::launch_choices::LaunchChoicesService::new(
            crate::services::launch_choices::LaunchChoicesServiceConfig { preferences: library.clone(), launch: launch.clone() }
        ));
        let downloads = DownloadService::new(DownloadServiceConfig {
            romm: romm_api.clone(),
            files: files.clone(),
            jobs: jobs.clone(),
            library: library.clone(),
            paths: paths.clone(),
            systems: systems.clone(),
            launch: launch.clone(),
            removal: removal.clone(),
            events: events.clone(),
            clock: clock.clone(),
        });
        let active = downloads.clone();
        let adoption = InstallAdoptionService::new(InstallAdoptionServiceConfig {
            romm: romm_api.clone(),
            files,
            systems,
            paths,
            launch: launch.clone(),
            library: library.clone(),
            jobs,
            active_downloads: Arc::new(move || active.active_download_rom_ids()),
            events: events.clone(),
            clock: clock.clone(),
        });

        let sync_state = Arc::new(crate::services::library_state::LibraryRunState::default());
        let sync_repo = Arc::new(crate::adapters::sync_library::SqliteSyncLibrary::new(db.clone()));
        let assets = Arc::new(crate::adapters::steam_assets::LocalSteamAssets::new(env.user_home.clone(),env.runtime_dir.join("covers")));
        let artwork = Arc::new(crate::services::artwork::ArtworkService {api:romm_api.clone(),assets:assets.clone(),registry:sync_repo.clone(),lock:Default::default()});
        if let Err(error)=artwork.prune().await {tracing::warn!("Artwork startup cleanup failed: {error}");}
        let shortcuts = Arc::new(crate::services::shortcut_removal::ShortcutRemovalService {registry:sync_repo.clone(),bindings:sync_repo.clone(),assets:assets.clone(),kv:kv_config.clone()});
        let reporter = Arc::new(crate::services::sync_reporter::SyncReporter::with_assets(sync_repo.clone(),assets.clone(),settings.clone()));
        let game_reads=Arc::new(crate::adapters::game_reads::SqliteGameReads::new(db.clone()));
        let adopter=adoption.clone();
        let game_detail=Arc::new(crate::services::game_detail::GameDetailService::new(crate::services::game_detail::GameDetailServiceConfig {
            repo:game_reads.clone(),settings:settings.clone(),clock:clock.clone(),schedule_adoption:Arc::new(move |id|adopter.schedule_adoption_check(id)),
        }));
        let game_covers=Arc::new(crate::services::game_covers::GameCoverService::new(crate::services::game_covers::GameCoverServiceConfig {
            artwork:artwork.clone(),romm:romm_api.clone(),records:game_reads.clone(),registry:sync_repo.clone(),pending:reporter.clone(),
        }));
        let steamgrid=Arc::new(crate::services::steamgrid::SteamGridService::new(crate::services::steamgrid::SteamGridServiceConfig {
            api:crate::adapters::steamgrid::SteamGridHttp::new(&user_agent,crate::adapters::steamgrid::API_BASE)?,
            romm:romm_api.clone(),assets:Arc::new(crate::adapters::steam_assets::LocalSteamAssets::new(env.user_home.clone(),env.runtime_dir.join("artwork"))),
            registry:sync_repo.clone(),records:game_reads,pending:reporter.clone(),settings:settings.clone(),persister:persister.clone(),
        }));
        if let Err(error)=steamgrid.prune().await{tracing::warn!("SteamGridDB cache cleanup failed: {error}");}

        let save_safety=Arc::new(crate::adapters::switch_save_safety::SwitchSaveSafety::new(db.clone(),settings.clone()));
        let version_repo=Arc::new(crate::adapters::versions::SqliteVersions::new(db.clone()));
        let active=downloads.clone();
        let versions=Arc::new(crate::services::versions::VersionService::new(crate::services::versions::VersionServiceConfig {
            repo:version_repo.clone(),romm:romm_api.clone(),settings:settings.clone(),launch:launch.clone(),saves:save_safety.clone(),
            clock:clock.clone(),reachable:romm_api.clone(),active:Arc::new(move||active.active_download_rom_ids()),mutation_lock:downloads.mutation_lock.clone(),
        }));
        let launch_gate=Arc::new(crate::services::launch_gate::LaunchGate::new(crate::services::launch_gate::LaunchGateConfig {
            repo:version_repo,saves:save_safety.clone(),settings:settings.clone(),
        }));
        let game_actions=Arc::new(crate::services::game_actions::GameActions::new(crate::services::game_actions::GameActionsConfig {
            library:library.clone(),registry:sync_repo.clone(),assets:assets.clone(),removal,downloads:downloads.clone(),settings:settings.clone(),
        }));
        let playtime=crate::services::playtime::PlaytimeService::new(crate::services::playtime::PlaytimeServiceConfig {
            repo:Arc::new(crate::adapters::playtime::SqlitePlaytime::new(db.clone())),api:romm_api.clone(),kv:kv_config,
            clock:clock.clone(),saves:save_safety,
        });
        let update=crate::services::update::UpdateService::new(crate::services::update::UpdateConfig {
            feed:Arc::new(crate::adapters::releases::GitHubReleases::new(&user_agent,crate::domain::self_update::LATEST)?),
            metadata:Arc::new(crate::adapters::releases::FilePluginMetadata::new(env.plugin_dir.clone())),clock:clock.clone(),
        });
        let sync = crate::services::sync_orchestrator::SyncOrchestrator::new(crate::services::sync_orchestrator::SyncOrchestratorConfig {
            fetcher: Arc::new(crate::services::library_fetcher::LibraryFetcher {metadata:romm_api.clone(),pages:romm_api.clone(),settings:settings.clone(),state:sync_state.clone()}),
            state:sync_state,reporter,library:library.clone(),registry:sync_repo.clone(),baseline:library,
            delta:romm_api,finalizer:sync_repo,runs:Arc::new(crate::adapters::sync_runs::SqliteSyncRuns::new(db)),
            launch:launch.clone(),adoption:adoption.clone(),events,clock,ids,sleeper,
            settings:settings.clone(),plugin_dir:env.plugin_dir.to_string_lossy().into_owned(),artwork,
        });

        connection_service.migrate_legacy_credentials().await;
        library_service.heal_interrupted_runs().await?;
        // Restore the durable download queue: keep job partials, drop orphan
        // tmp files, and auto-resume interrupted jobs in the background.
        downloads.reconcile_persisted_downloads().await;
        Ok(Self {
            settings,
            settings_service,
            connection_service,
            launch,
            launch_choices,
            versions,
            launch_gate,
            game_actions,
            playtime,
            update,
            game_detail,
            game_covers,
            steamgrid,
            downloads,
            library: library_service,
            sync,
            shortcuts,
            adoption,
        })
    }

    /// The protocol registry: ported callables bound to their services, every
    /// other name bound to the "not implemented yet" error.
    pub fn registry(&self) -> Registry {
        let mut registry = Registry::new();
        for name in callable_names() {
            let handler = self.ported_handler(&name).unwrap_or_else(|| not_implemented(&name));
            registry.insert(name, handler);
        }
        registry
    }

    fn ported_handler(&self, name: &str) -> Option<Handler> {
        let s = self.settings_service.clone();
        let c = self.connection_service.clone();
        let d = self.downloads.clone();
        let l = self.library.clone();
        let sync = self.sync.clone();
        let shortcuts = self.shortcuts.clone();
        let choices = self.launch_choices.clone();
        let game = self.game_detail.clone();
        let covers = self.game_covers.clone();
        let sgdb = self.steamgrid.clone();
        let versions=self.versions.clone();
        let gate=self.launch_gate.clone();
        let actions=self.game_actions.clone();
        let playtime=self.playtime.clone();
        let update=self.update.clone();
        let h: Handler = match name {
            "get_version_list" => bind(name,1,1,versions,|s,a|async move{s.list(rom_id_arg(&a[0])?).await.map_err(err)}),
            "switch_version" => bind(name,3,3,versions,|s,a|async move{Ok(s.switch(rom_id_arg(&a[0])?,rom_id_arg(&a[1])?,crate::domain::settings::truthy(&a[2])).await.unwrap_or_else(|e|unknown_failure(&e)))}),
            "evaluate_launch" => bind(name,1,1,gate,|s,a|async move{s.evaluate(rom_id_arg(&a[0])?).await.map_err(err)}),
            "check_local_drift" => bind(name,1,1,gate,|s,a|async move{Ok(s.drift(rom_id_arg(&a[0])?).await)}),
            "check_core_change" => bind(name,1,1,gate,|s,a|async move{Ok(s.core_change(rom_id_arg(&a[0])?).await)}),
            "remove_rom" => bind(name,1,1,actions,|s,a|async move{Ok(s.remove(rom_id_arg(&a[0])?).await)}),
            "uninstall_all_roms" => bind(name,0,0,actions,|s,_|async move{Ok(s.uninstall_all().await.unwrap_or_else(|e|unknown_failure(&e)))}),
            "apply_steam_input_setting" => bind(name,0,0,actions,|s,_|async move{Ok(s.apply_input().await.unwrap_or_else(|e|unknown_failure(&e)))}),
            "check_for_update" => bind(name,1,1,update,|s,a|async move{Ok(s.check(crate::domain::settings::truthy(&a[0])).await)}),
            "record_session_start" => bind(name,1,1,playtime,|s,a|async move{let result=s.start(rom_id_arg(&a[0])?).await.map_err(err)?;s.schedule_flush().await;Ok(result)}),
            "finalize_game_session" => bind(name,1,1,playtime,|s,a|async move{Ok(s.finalize(rom_id_arg(&a[0])?).await)}),
            "get_all_playtime" => bind(name,0,0,playtime,|s,_|async move{s.all().await.map_err(err)}),
            "get_playtime_scope_notice" => bind(name,0,0,playtime,|s,_|async move{s.scope_notice().await.map_err(err)}),
            "reconcile_playtime" => bind(name,1,1,playtime,|s,a|async move{s.reconcile(rom_id_arg(&a[0])?).await.map_err(err)}),

            "get_cached_game_detail" => bind(name,1,1,game,|s,a|async move{s.detail(rom_id_arg(&a[0])?).await.map_err(err)}),
            "get_rom_metadata" => bind(name,1,1,game,|s,a|async move{s.metadata(rom_id_arg(&a[0])?).await.map_err(err)}),
            "get_all_metadata_cache" => bind(name,0,0,game,|s,_|async move{s.all_metadata().await.map_err(err)}),
            "fetch_cover_base64" => bind(name,1,1,covers,|s,a|async move{Ok(s.fetch(rom_id_arg(&a[0])?).await)}),
            "refresh_cover_artwork" => bind(name,1,1,covers,|s,a|async move{Ok(s.refresh(rom_id_arg(&a[0])?).await.unwrap_or_else(|e|unknown_failure(&e)))}),
            "get_sgdb_artwork_base64" => bind(name,2,2,sgdb,|s,a|async move{Ok(s.artwork(rom_id_arg(&a[0])?,rom_id_arg(&a[1])?).await)}),
            "verify_sgdb_api_key" => bind(name,0,1,sgdb,|s,a|async move{let key=opt_str(&a,0,"api_key")?;Ok(s.verify(Some(&key)).await)}),
            "save_sgdb_api_key" => bind(name,1,1,sgdb,|s,a|async move{Ok(s.save_key(&opt_str(&a,0,"api_key")?).await.unwrap_or_else(|e|unknown_failure(&e)))}),
            "get_sgdb_resolution" => bind(name,1,1,sgdb,|s,a|async move{Ok(s.resolution(rom_id_arg(&a[0])?).await.unwrap_or_else(|e|unknown_failure(&e)))}),
            "search_sgdb_games" => bind(name,1,1,sgdb,|s,a|async move{Ok(s.search(&req_str(&a,0,"term")?).await)}),
            "apply_sgdb_game_id" => bind(name,2,2,sgdb,|s,a|async move{Ok(s.apply(rom_id_arg(&a[0])?,rom_id_arg(&a[1])?).await.unwrap_or_else(|e|unknown_failure(&e)))}),
            "get_platform_core_info" => bind(name,1,1,choices,|s,a|async move {s.get_platform_core_info(rom_id_arg(&a[0])?).await.map_err(err)}),
            "set_game_core" => bind(name,2,2,choices,|s,a|async move {Ok(s.set_game_core(rom_id_arg(&a[0])?, &req_str(&a,1,"label")?).await.unwrap_or_else(|e|unknown_failure(&e)))}),
            "clear_game_core" => bind(name,1,1,choices,|s,a|async move {Ok(s.clear_game_core(rom_id_arg(&a[0])?).await.unwrap_or_else(|e|unknown_failure(&e)))}),
            "get_disc_selection" => bind(name,1,1,choices,|s,a|async move {s.get_disc_selection(rom_id_arg(&a[0])?).await.map_err(err)}),
            "select_disc" => bind(name,2,2,choices,|s,a|async move {
                let filename = if a[1].is_null() { None } else { Some(req_str(&a,1,"filename")?) };
                Ok(s.select_disc(rom_id_arg(&a[0])?, filename.as_deref()).await.unwrap_or_else(|e|unknown_failure(&e)))
            }),
            "get_rom_relaunch_options" => bind(name,1,1,choices,|s,a|async move {s.get_rom_relaunch_options(rom_id_arg(&a[0])?).await.map_err(err)}),
            "get_installed_relaunch_options" => bind(name,0,0,choices,|s,_|async move {s.get_installed_relaunch_options().await.map_err(err)}),
            "remove_all_shortcuts" => bind(name,0,0,shortcuts,|s,_|async move {Ok(s.removal_set(None).await.unwrap_or_else(|e|unknown_failure(&e)))}),
            "remove_platform_shortcuts" => bind(name,1,1,shortcuts,|s,a|async move {Ok(s.removal_set(Some(a[0].as_str().ok_or_else(||"platform_slug must be a string".to_string())?)).await.unwrap_or_else(|e|unknown_failure(&e)))}),
            "report_removal_results" => bind(name,1,1,shortcuts,|s,a|async move {Ok(s.report(&a[0]).await.unwrap_or_else(|e|unknown_failure(&e)))}),
            "reconcile_shortcuts" => bind(name,1,1,shortcuts,|s,a|async move {Ok(s.reconcile(&a[0]).await.unwrap_or_else(|e|unknown_failure(&e)))}),
            "save_shortcut_icon" => bind(name,2,2,shortcuts,|s,a|async move {Ok(s.save_icon(rom_id_arg(&a[0])?,a[1].as_str().ok_or_else(||"icon_base64 must be a string".to_string())?).await)}),
            "get_artwork_base64" => bind(name,1,1,covers,|s,a|async move{Ok(s.get_base64(rom_id_arg(&a[0])?).await)}),
            "sync_preview" => bind(name,0,0,sync,|s,_|async move{Ok(s.preview().await)}),
            "sync_apply_delta" => bind(name,1,1,sync,|s,a|async move{Ok(s.apply_preview(a[0].as_str().unwrap_or("")).await)}),
            "sync_cancel_preview" => bind(name,0,0,sync,|s,_|async move{Ok(s.cancel_preview().await)}),
            "start_sync" => bind(name,0,0,sync,|s,_|async move {Ok(s.start().await)}),
            "cancel_sync" => bind(name,0,1,sync,|s,a|async move {
                let id=a.first().filter(|v|crate::domain::settings::truthy(v)).map(|v|v.as_str().map(str::to_string).unwrap_or_else(||v.to_string()));
                Ok(s.cancel(id.as_deref()))
            }),
            "get_sync_status" => bind(name,0,0,sync,|s,_|async move {Ok(s.get_sync_status())}),
            "sync_heartbeat" => bind(name,0,0,sync,|s,_|async move {Ok(s.heartbeat())}),
            "report_unit_results" => bind(name,3,3,sync,|s,a|async move {
                let run=a[1].as_str().unwrap_or("");let id=a[2].as_str().map(str::to_string).unwrap_or_else(||a[2].to_string());
                Ok(s.report(&a[0],run,&id).await.unwrap_or_else(|e|unknown_failure(&e)))
            }),
            "start_download" => bind(name, 1, 1, d, |d, a| async move {
                let id = rom_id_arg(&a[0])?;
                Ok(d.start_download(id).await)
            }),
            "resume_download" => bind(name, 1, 1, d, |d, a| async move {
                let id = rom_id_arg(&a[0])?;
                Ok(d.resume_download(id).await)
            }),
            "cancel_download" => bind(name, 1, 1, d, |d, a| async move {
                Ok(d.cancel_download(rom_id_arg(&a[0])?))
            }),
            "pause_download" => bind(
                name,
                1,
                1,
                d,
                |d, a| async move { Ok(d.pause_download(rom_id_arg(&a[0])?)) },
            ),
            "get_download_queue" => bind(name, 0, 0, d, |d, _| async move { Ok(d.get_download_queue()) }),
            "get_installed_rom" => bind(name, 1, 1, d, |d, a| async move {
                let id = rom_id_arg(&a[0])?;
                d.get_installed_rom(id).await.map_err(err)
            }),
            "get_platforms" => bind(name, 0, 0, l, |l, _| async move { Ok(l.get_platforms().await) }),
            "save_platform_sync" => bind(name, 2, 2, l, |l, a| async move {
                Ok(l.save_platform_sync(&a[0], &a[1]).await.unwrap_or_else(|e| unknown_failure(&e)))
            }),
            "set_all_platforms_sync" => bind(name, 1, 1, l, |l, a| async move {
                Ok(l.set_all_platforms_sync(&a[0]).await)
            }),
            "get_collections" => bind(name, 0, 0, l, |l, _| async move { Ok(l.get_collections().await) }),
            "save_collection_sync" => bind(name, 3, 3, l, |l, a| async move {
                Ok(l.save_collection_sync(&a[0], &a[1], &a[2]).await.unwrap_or_else(|e| unknown_failure(&e)))
            }),
            "set_all_collections_sync" => bind(name, 1, 2, l, |l, a| async move {
                Ok(l.set_all_collections_sync(&a[0], a.get(1)).await)
            }),
            "get_registry_platforms" => bind(name, 0, 0, l, |l, _| async move {
                Ok(l.get_registry_platforms().await.unwrap_or_else(|e| unknown_failure(&e)))
            }),
            "get_sync_stats" => bind(
                name,
                0,
                0,
                l,
                |l, _| async move { Ok(l.get_sync_stats().await.unwrap_or_else(|e| unknown_failure(&e))) },
            ),
            "get_app_id_rom_id_map" => bind(name, 0, 0, l, |l, _| async move {
                Ok(l.get_app_id_rom_id_map().await.unwrap_or_else(|e| unknown_failure(&e)))
            }),
            "clear_sync_cache" => bind(
                name,
                0,
                0,
                l,
                |l, _| async move { Ok(l.clear_sync_cache().await.unwrap_or_else(|e| unknown_failure(&e))) },
            ),
            "test_connection" => bind(name, 0, 0, c, |c, _| async move { Ok(c.test_connection().await) }),
            "probe_reachability" => bind(name, 0, 0, c, |c, _| async move { Ok(c.probe_reachability().await) }),
            "connect_with_credentials" => bind(name, 3, 4, c, |c, a| async move {
                let url = opt_str(&a, 0, "romm_url")?;
                let user = req_str(&a, 1, "username")?;
                let pass = req_str(&a, 2, "password")?;
                Ok(c.establish_token(&url, &user, &pass, a.get(3)).await)
            }),
            "connect_with_token" => bind(name, 2, 3, c, |c, a| async move {
                let url = opt_str(&a, 0, "romm_url")?;
                let token = req_str(&a, 1, "token")?;
                Ok(c.establish_user_token(&url, &token, a.get(2)).await)
            }),
            "connect_with_pairing_code" => bind(name, 2, 3, c, |c, a| async move {
                let url = opt_str(&a, 0, "romm_url")?;
                let code = req_str(&a, 1, "code")?;
                Ok(c.establish_paired_token(&url, &code, a.get(2)).await)
            }),
            "sign_out" => bind(name, 0, 0, c, |c, _| async move { Ok(c.sign_out().await) }),
            "save_server_url" => bind(name, 1, 2, s, |s, a| async move {
                let url = req_str(&a, 0, "romm_url")?;
                Ok(s.save_server_url(&url, a.get(1)).await)
            }),
            "frontend_log" => bind(name, 2, 2, s, |s, a| async move {
                s.frontend_log(&loose_str(&a[0]), &loose_str(&a[1]));
                Ok(Value::Null)
            }),
            "debug_log" => bind(name, 1, 1, s, |s, a| async move {
                s.frontend_log("debug", &loose_str(&a[0]));
                Ok(Value::Null)
            }),
            "save_log_level" => bind(name, 1, 1, s, |s, a| async move {
                s.save_log_level(&a[0]).await.map_err(err)
            }),
            "save_steam_input_setting" => bind(name, 1, 1, s, |s, a| async move {
                s.save_steam_input_setting(&a[0]).await.map_err(err)
            }),
            "save_preferred_region" => bind(name, 1, 1, s, |s, a| async move {
                s.save_preferred_region(&a[0]).await.map_err(err)
            }),
            "get_known_regions" => bind(
                name,
                0,
                0,
                s,
                |s, _| async move { s.get_known_regions().await.map_err(err) },
            ),
            "get_settings" => bind(name, 0, 0, s, |s, _| async move { Ok(s.get_settings().await) }),
            "get_whitelist_settings" => bind(name, 0, 0, s, |s, _| async move { Ok(s.get_whitelist_settings()) }),
            "update_whitelist_settings" => bind(name, 2, 2, s, |s, a| async move {
                s.update_whitelist_settings(&a[0], &a[1]).await.map_err(err)
            }),
            "save_collection_platform_groups" => bind(name, 1, 1, s, |s, a| async move {
                s.save_collection_platform_groups(&a[0]).await.map_err(err)
            }),
            "get_settings_reset_notice" => bind(name, 0, 0, s, |s, _| async move { Ok(s.get_settings_reset_notice()) }),
            "dismiss_settings_reset_notice" => bind(name, 0, 0, s, |s, _| async move {
                s.dismiss_settings_reset_notice().await.map_err(err)
            }),
            _ => return None,
        };
        Some(h)
    }
}

/// Load `settings.json`, run the one-time legacy fold and the schema upgrade,
/// and record a corrupt-file reset as the persistent notice marker.
async fn load_and_upgrade_settings(file: Arc<SettingsFile>) -> anyhow::Result<SettingsStore> {
    tokio::task::spawn_blocking(move || {
        let loaded = file.load()?;
        let mut settings = loaded.settings;
        if stored_version(&settings) < 4 {
            settings = fold_legacy_save_sync_settings(&settings, file.load_legacy_save_sync_state().as_ref());
        }
        let mut settings = migrate_settings(&settings);
        if let Some(backup) = loaded.corrupt_backup {
            settings.insert(RESET_NOTICE_KEY.into(), json!({"backed_up_to": backup}));
        }
        anyhow::Ok(SettingsStore::new(settings))
    })
    .await?
}

fn err(e: anyhow::Error) -> String {
    format!("{e:#}")
}

fn not_implemented(name: &str) -> Handler {
    let message = format!("not implemented in the Rust backend yet: {name}");
    Arc::new(move |_| {
        let message = message.clone();
        Box::pin(async move { Err(message) })
    })
}

/// Bind a service method as a handler with an arity check of `min..=max`
/// positional arguments.
fn bind<S, F, Fut>(name: &str, min: usize, max: usize, service: Arc<S>, f: F) -> Handler
where
    S: Send + Sync + 'static,
    F: Fn(Arc<S>, Vec<Value>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<Value, String>> + Send + 'static,
{
    let name = name.to_string();
    let f = Arc::new(f);
    Arc::new(move |args: Vec<Value>| {
        if args.len() < min || args.len() > max {
            let expected = if min == max {
                format!("{min}")
            } else {
                format!("{min} to {max}")
            };
            let message = format!(
                "{name}() takes {expected} positional arguments but {} were given",
                args.len()
            );
            return Box::pin(async move { Err(message) });
        }
        Box::pin(f(service.clone(), args))
    })
}

/// A ROM id argument: an integer, or a string holding one (Python `int()`).
fn rom_id_arg(v: &Value) -> Result<i64, String> {
    let parsed = match v {
        Value::Number(n) => n
            .as_i64()
            .or_else(|| n.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64)),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    };
    parsed.ok_or_else(|| format!("argument 'rom_id' must be an integer, got {v}"))
}

/// A required string argument.
fn req_str(args: &[Value], i: usize, name: &str) -> Result<String, String> {
    args.get(i)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("argument '{name}' must be a string"))
}

/// A string argument where `null` reads as empty (the "not configured" case).
fn opt_str(args: &[Value], i: usize, name: &str) -> Result<String, String> {
    match args.get(i) {
        None | Some(Value::Null) => Ok(String::new()),
        Some(_) => req_str(args, i, name),
    }
}

/// Any JSON value as log text: strings verbatim, everything else as JSON.
fn loose_str(v: &Value) -> String {
    v.as_str().map_or_else(|| v.to_string(), str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surface_matches_python_backend_size_and_includes_ported() {
        let names = callable_names();
        assert_eq!(names.len(), 106);
        let mut dedup = names.clone();
        dedup.dedup();
        assert_eq!(dedup.len(), names.len());
        for ported in PORTED_CALLABLES {
            assert!(names.iter().any(|n| n == ported), "{ported} not in callables.json");
        }
        assert!(names.iter().all(|n| !n.starts_with('_')));
    }

    #[test]
    fn argument_helpers() {
        let args = vec![json!("x"), Value::Null, json!(5)];
        assert_eq!(req_str(&args, 0, "a").unwrap(), "x");
        assert!(req_str(&args, 1, "a").is_err());
        assert!(req_str(&args, 9, "a").is_err());
        assert_eq!(opt_str(&args, 1, "a").unwrap(), "");
        assert_eq!(opt_str(&args, 9, "a").unwrap(), "");
        assert!(opt_str(&args, 2, "a").is_err());
        assert_eq!(loose_str(&json!("hi")), "hi");
        assert_eq!(loose_str(&json!({"a": 1})), r#"{"a":1}"#);
        assert_eq!(rom_id_arg(&json!(7)), Ok(7));
        assert_eq!(rom_id_arg(&json!(7.0)), Ok(7));
        assert_eq!(rom_id_arg(&json!(" 12 ")), Ok(12));
        assert!(rom_id_arg(&json!(7.5)).is_err());
        assert!(rom_id_arg(&json!("x")).is_err());
        assert!(rom_id_arg(&Value::Null).is_err());
    }
}
