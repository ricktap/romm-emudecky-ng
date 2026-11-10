//! DownloadService over real SQLite + real files, with a fake RomM — the Rust
//! twin of `tests/services/test_downloads.py` (durable queue, scheduler,
//! classification, reconciliation, supersede).

use std::sync::Arc;

use serde_json::json;

use super::*;
use crate::domain::errors::RommError;
use crate::services::testing::{DownloadEnv, EDEN, payload, switch_rom, zip_bytes};

const ROM: i64 = 7;
const LEN: usize = 4096;

async fn env_with_rom() -> DownloadEnv {
    let env = DownloadEnv::new().await;
    env.seed_rom(ROM, "switch", "Zelda.nsp", Some(4242), None).await;
    env.romm.set_rom(switch_rom(ROM, "Zelda.nsp", LEN));
    env.romm.set_content(ROM, payload(LEN));
    env
}

fn target(env: &DownloadEnv, name: &str) -> String {
    format!("{}/switch/{name}", env.roms_s())
}

async fn job_row(env: &DownloadEnv, rom_id: i64) -> Option<DownloadJob> {
    env.jobs.get(JobKey::rom(rom_id)).await.unwrap()
}

#[tokio::test]
async fn single_file_happy_path() {
    let env = env_with_rom().await;
    let r = env.svc.start_download(ROM).await;
    assert_eq!(r, json!({"success": true, "message": "Download started"}));
    let entry = env.wait_status(ROM, "completed").await;
    assert_eq!(entry["progress"], 1.0);
    assert_eq!(entry["resumable"], true);
    let path = target(&env, "Zelda.nsp");
    assert_eq!(std::fs::read(&path).unwrap(), payload(LEN));
    assert!(!std::path::Path::new(&format!("{path}.tmp")).exists());
    assert!(job_row(&env, ROM).await.is_none(), "completed jobs delete their row");

    let [complete] = env.events.named("download_complete").try_into().unwrap();
    assert_eq!(
        complete,
        json!({
            "rom_id": ROM, "rom_name": "Game 7", "platform_name": "Nintendo Switch",
            "file_path": path, "app_id": 4242,
            "launch_options": format!("vblank_mode=0 %command% \"{EDEN}\" -f -g \"{path}\""),
            "resumable": true,
        })
    );
    let statuses = env.events.statuses(ROM);
    assert!(statuses.contains(&"downloading".to_string()), "{statuses:?}");
    let progress = env.events.named("download_progress");
    let last = progress.iter().rev().find(|f| f["status"] == "downloading").unwrap();
    assert_eq!(last["bytes_downloaded"], LEN);
    assert_eq!(last["total_bytes"], LEN);
    assert!(last.get("reason").is_none());

    let installed = env.svc.get_installed_rom(ROM).await.unwrap();
    assert_eq!(installed["file_name"], "Zelda.nsp");
    assert_eq!(installed["file_path"], path);
    assert_eq!(installed["system"], "switch");
    assert_eq!(installed["platform_slug"], "switch");
    assert_eq!(installed["installed_at"], "2026-01-01T00:00:00+00:00");
    assert_eq!(env.svc.get_installed_rom(99).await.unwrap(), Value::Null);
    assert_eq!(env.romm.calls()[0].file_name, "Zelda.nsp");
}

#[tokio::test]
async fn unbound_rom_completes_with_null_app_id_and_empty_launch_without_emulator() {
    let env = env_with_rom().await;
    env.seed_rom(ROM, "switch", "Zelda.nsp", None, None).await;
    env.locator.installed.lock().unwrap().clear();
    env.svc.start_download(ROM).await;
    env.wait_status(ROM, "completed").await;
    let complete = &env.events.named("download_complete")[0];
    assert_eq!(complete["app_id"], Value::Null);
    assert_eq!(complete["launch_options"], "");
}

#[tokio::test]
async fn per_game_override_and_missing_executable_are_baked() {
    let env = env_with_rom().await;
    env.locator
        .installed
        .lock()
        .unwrap()
        .insert("ryujinx".into(), "/h/Applications/publish/Ryujinx.sh".into());
    env.sql("UPDATE roms SET emulator_override = 'Ryujinx' WHERE rom_id = 7").await;
    env.svc.start_download(ROM).await;
    env.wait_status(ROM, "completed").await;
    let lo = env.events.named("download_complete")[0]["launch_options"].clone();
    assert_eq!(
        lo,
        format!(
            "vblank_mode=0 %command% \"/h/Applications/publish/Ryujinx.sh\" \"{}\"",
            target(&env, "Zelda.nsp")
        )
    );
    // An unresolved emulator must keep the empty placeholder, never a file-only command.
    env.locator.missing.lock().unwrap().insert(EDEN.into());
    let path = target(&env, "Zelda.nsp");
    let emulator = crate::domain::emulator_table::EmulatorInvocation::table("eden", EDEN, "Eden");
    assert_eq!(env.launch.launch_options(Some(&emulator), &path), "");
}

#[tokio::test]
async fn guards_already_downloading_and_rom_not_found() {
    let env = env_with_rom().await;
    *env.romm.stall_at.lock().unwrap() = Some(100);
    assert_eq!(env.svc.start_download(ROM).await["success"], true);
    let again = env.svc.start_download(ROM).await;
    assert_eq!(again["reason"], "already_downloading");
    env.svc.cancel_download(ROM);
    env.wait_status(ROM, "cancelled").await;

    let missing = env.svc.start_download(404).await;
    assert_eq!(missing["success"], false);
    assert_eq!(missing["reason"], "not_found");
    assert!(env.svc.active_download_rom_ids().is_empty(), "the claim is released");
}

#[tokio::test]
async fn disk_space_preflight_and_reservations() {
    let env = env_with_rom().await;
    *env.files.free.lock().unwrap() = Some(10);
    let r = env.svc.start_download(ROM).await;
    assert_eq!(r["reason"], "insufficient_space");
    assert!(r["message"].as_str().unwrap().starts_with("Not enough disk space (0MB free, need 100MB)"));
    assert!(env.svc.active_download_rom_ids().is_empty());
    assert!(job_row(&env, ROM).await.is_none());

    // A sibling that fits alone is blocked by the first one's reservation.
    let mb = 1024 * 1024;
    *env.files.free.lock().unwrap() = Some(SPACE_BUFFER + 150 * mb);
    env.seed_rom(8, "switch", "Big.nsp", None, None).await;
    env.romm.set_rom(switch_rom(ROM, "Zelda.nsp", 100 * mb as usize));
    env.romm.set_rom(switch_rom(8, "Big.nsp", 100 * mb as usize));
    *env.romm.stall_at.lock().unwrap() = Some(10);
    assert_eq!(env.svc.start_download(ROM).await["success"], true);
    assert_eq!(env.svc.start_download(8).await["reason"], "insufficient_space");
    env.svc.cancel_download(ROM);
    env.wait_status(ROM, "cancelled").await;
    assert!(env.svc.st().reserved.is_empty(), "the reservation is released");
}

#[tokio::test]
async fn multi_file_requires_double_space() {
    let env = DownloadEnv::new().await;
    env.seed_rom(9, "switch", "Game", None, None).await;
    let mut rom = switch_rom(9, "Game", 1000);
    rom["files"] = json!([{"file_name": "a"}, {"file_name": "b"}]);
    env.romm.set_rom(rom);
    *env.files.free.lock().unwrap() = Some(SPACE_BUFFER + 1500);
    assert_eq!(env.svc.start_download(9).await["reason"], "insufficient_space");
}

#[tokio::test]
async fn unsafe_platform_slug_is_rejected_before_any_write() {
    let env = env_with_rom().await;
    let mut rom = switch_rom(ROM, "Zelda.nsp", LEN);
    rom["platform_slug"] = json!("../../etc");
    env.romm.set_rom(rom);
    let r = env.svc.start_download(ROM).await;
    assert_eq!(r["reason"], "path_traversal");
    let [failed] = env.events.named("download_failed").try_into().unwrap();
    assert_eq!(failed["rom_id"], ROM);
    assert!(failed["error_message"].as_str().unwrap().contains("unsafe platform path"));
    assert!(!env.dir.path().join("etc").exists());
    assert!(env.svc.active_download_rom_ids().is_empty());
}

#[tokio::test]
async fn traversal_in_fs_name_is_sanitized_to_one_component() {
    let env = env_with_rom().await;
    env.romm.set_rom(switch_rom(ROM, "../../evil.nsp", LEN));
    env.svc.start_download(ROM).await;
    env.wait_status(ROM, "completed").await;
    assert!(std::path::Path::new(&target(&env, "evil.nsp")).exists());
    let mut rom = switch_rom(ROM, "..", LEN);
    rom["fs_name"] = json!("..");
    env.romm.set_rom(rom);
    env.svc.start_download(ROM).await;
    env.wait_status(ROM, "completed").await;
    assert!(std::path::Path::new(&target(&env, "rom_7")).exists());
}

fn folder_rom(id: i64) -> Value {
    json!({
        "id": id, "name": "Mario Kart 8 Deluxe", "fs_name": "Mario Kart 8 Deluxe",
        "fs_name_no_ext": "Mario Kart 8 Deluxe", "fs_path": "switch", "fs_size_bytes": 3000,
        "platform_slug": "switch", "platform_name": "Nintendo Switch",
        "has_multiple_files": false, "has_nested_single_file": true,
        "files": [
            {"file_path": "switch/Mario Kart 8 Deluxe", "file_name": "base.nsp", "file_size_bytes": 2000},
            {"file_path": "switch/Mario Kart 8 Deluxe/update", "file_name": "u%20pd.nsp", "file_size_bytes": 10},
        ],
    })
}

#[tokio::test]
async fn multi_file_switch_folder_extracts_and_bakes_largest() {
    let env = DownloadEnv::new().await;
    env.seed_rom(9, "switch", "Mario Kart 8 Deluxe", Some(55), None).await;
    env.romm.set_rom(folder_rom(9));
    env.romm.set_content(
        9,
        zip_bytes(&[("base.nsp", &[1u8; 2000]), ("update/", b""), ("update/u%20pd.nsp", &[2u8; 10])]),
    );
    env.svc.start_download(9).await;
    env.wait_status(9, "completed").await;
    let dir = format!("{}/switch/Mario Kart 8 Deluxe", env.roms_s());
    assert_eq!(std::fs::read(format!("{dir}/base.nsp")).unwrap().len(), 2000);
    assert!(std::path::Path::new(&format!("{dir}/update/u pd.nsp")).exists(), "names are URL-decoded");
    assert!(!std::path::Path::new(&format!("{dir}.zip.tmp")).exists());
    let install = env.library.install(9).await.unwrap().unwrap();
    assert_eq!(install.file_path, format!("{dir}/base.nsp"));
    assert_eq!(install.rom_dir.as_deref(), Some(dir.as_str()));
    assert!(env.events.statuses(9).contains(&"extracting".to_string()));
    let extracting = env
        .events
        .named("download_progress")
        .into_iter()
        .find(|f| f["status"] == "extracting")
        .unwrap();
    assert_eq!(extracting["resumable"], false);
    let complete = &env.events.named("download_complete")[0];
    assert_eq!(complete["app_id"], 55);
    assert!(complete["launch_options"].as_str().unwrap().ends_with(&format!("-f -g \"{dir}/base.nsp\"")));

    // A persisted launch-file pick survives a reinstall.
    env.sql("UPDATE roms SET selected_disc = 'u pd.nsp' WHERE rom_id = 9").await;
    let bake = env.launch.resolve_bake_path(&install, Some("u pd.nsp")).await;
    assert_eq!(bake, format!("{dir}/update/u pd.nsp"));
    let item = env.launch.relaunch_item_for_rom(9).await.unwrap();
    assert!(item["launch_options"].as_str().unwrap().ends_with("u pd.nsp\""));
}

#[tokio::test]
async fn zip_slip_fails_and_cleans_up() {
    let env = DownloadEnv::new().await;
    env.seed_rom(9, "switch", "Mario Kart 8 Deluxe", None, None).await;
    env.romm.set_rom(folder_rom(9));
    env.romm.set_content(9, zip_bytes(&[("../../../escape.nsp", b"x"), ("b", b"y")]));
    env.svc.start_download(9).await;
    let entry = env.wait_status(9, "failed").await;
    assert!(entry["error"].as_str().unwrap().contains("outside target directory"));
    assert!(!std::path::Path::new(&format!("{}/switch/Mario Kart 8 Deluxe.zip.tmp", env.roms_s())).exists());
    assert!(!std::path::Path::new(&format!("{}/switch/Mario Kart 8 Deluxe", env.roms_s())).exists());
    assert!(job_row(&env, 9).await.is_none());
    assert_eq!(env.events.named("download_failed").len(), 1);
}

#[tokio::test]
async fn cancel_live_download_deletes_partial_and_row() {
    let env = env_with_rom().await;
    *env.romm.stall_at.lock().unwrap() = Some(1000);
    env.svc.start_download(ROM).await;
    env.romm.stalled.notified().await;
    let tmp = format!("{}.tmp", target(&env, "Zelda.nsp"));
    assert_eq!(std::fs::metadata(&tmp).unwrap().len(), 1000);
    assert_eq!(job_row(&env, ROM).await.unwrap().status, JobStatus::Downloading);
    assert_eq!(env.svc.cancel_download(ROM)["message"], "Download cancelled");
    let entry = env.wait_status(ROM, "cancelled").await;
    assert!(entry.get("reason").is_none());
    assert!(!std::path::Path::new(&tmp).exists());
    assert!(job_row(&env, ROM).await.is_none());
    assert_eq!(env.events.statuses(ROM).last().unwrap(), "cancelled");
    assert_eq!(env.svc.cancel_download(ROM)["reason"], "no_active_download");
    assert_eq!(env.svc.pause_download(ROM)["reason"], "no_active_download");
}

#[tokio::test]
async fn pause_keeps_partial_then_resume_appends_with_range_offset() {
    let env = env_with_rom().await;
    *env.romm.stall_at.lock().unwrap() = Some(1500);
    env.svc.start_download(ROM).await;
    env.romm.stalled.notified().await;
    assert_eq!(env.svc.pause_download(ROM)["message"], "Download paused");
    let entry = env.wait_status(ROM, "paused").await;
    assert_eq!(entry["resumable"], true);
    let tmp = format!("{}.tmp", target(&env, "Zelda.nsp"));
    assert_eq!(std::fs::metadata(&tmp).unwrap().len(), 1500);
    let job = job_row(&env, ROM).await.unwrap();
    assert_eq!(job.status, JobStatus::Paused);
    assert_eq!(job.total_bytes, Some(LEN as u64));
    assert_eq!(job.range_supported, Some(true));
    assert!(!env.svc.active_download_rom_ids().contains(&ROM));

    *env.romm.stall_at.lock().unwrap() = None;
    assert_eq!(env.svc.resume_download(ROM).await["success"], true);
    env.wait_status(ROM, "completed").await;
    let calls = env.romm.calls();
    assert_eq!((calls[1].resume, calls[1].offset), (true, 1500));
    assert_eq!(std::fs::read(target(&env, "Zelda.nsp")).unwrap(), payload(LEN));
    assert!(job_row(&env, ROM).await.is_none());
    assert_eq!(env.svc.resume_download(ROM).await["reason"], "not_paused");
}

#[tokio::test]
async fn cancel_or_evict_of_paused_job_discards_row_and_partial() {
    let env = env_with_rom().await;
    *env.romm.stall_at.lock().unwrap() = Some(1500);
    env.svc.start_download(ROM).await;
    env.romm.stalled.notified().await;
    env.svc.pause_download(ROM);
    env.wait_status(ROM, "paused").await;
    let tmp = format!("{}.tmp", target(&env, "Zelda.nsp"));
    assert_eq!(env.svc.cancel_download(ROM)["success"], true);
    env.wait_status(ROM, "cancelled").await;
    env.wait_until("partial deleted", || !std::path::Path::new(&tmp).exists()).await;
    assert!(job_row(&env, ROM).await.is_none());

    env.svc.start_download(ROM).await;
    env.romm.stalled.notified().await;
    env.svc.pause_download(ROM);
    env.wait_status(ROM, "paused").await;
    env.svc.evict(ROM).await;
    assert!(env.entry(ROM).is_none());
    assert!(!std::path::Path::new(&tmp).exists());
    assert!(job_row(&env, ROM).await.is_none());
}

#[tokio::test]
async fn transient_failures_interrupt_and_keep_the_partial() {
    for (err, why) in [
        (
            Arc::new(|| anyhow::Error::new(RommError::Connection("refused".into())))
                as Arc<dyn Fn() -> anyhow::Error + Send + Sync>,
            "server_unreachable",
        ),
        (
            Arc::new(|| {
                anyhow::Error::new(std::io::Error::from_raw_os_error(libc::ENOSPC))
                    .context(RommError::Connection("write".into()))
            }),
            "disk_full",
        ),
        (Arc::new(|| anyhow::Error::new(RommError::Auth("401".into()))), "auth_failed"),
    ] {
        let env = env_with_rom().await;
        let tmp = format!("{}.tmp", target(&env, "Zelda.nsp"));
        std::fs::create_dir_all(format!("{}/switch", env.roms_s())).unwrap();
        std::fs::write(&tmp, b"partial").unwrap();
        let f = err.clone();
        env.romm.fail_transfers(move || f());
        env.svc.start_download(ROM).await;
        let entry = env.wait_status(ROM, "interrupted").await;
        assert_eq!(entry["reason"], why);
        assert!(std::path::Path::new(&tmp).exists(), "{why}: partial kept");
        let job = job_row(&env, ROM).await.unwrap();
        assert_eq!((job.status, job.interrupt_reason.as_deref()), (JobStatus::Interrupted, Some(why)));
        let frame = env.events.named("download_progress").pop().unwrap();
        assert_eq!((frame["status"].clone(), frame["reason"].clone()), (json!("interrupted"), json!(why)));
        assert!(env.events.named("download_failed").is_empty());
    }
}

#[tokio::test]
async fn final_failures_delete_the_partial() {
    for err in [
        Arc::new(|| {
            anyhow::Error::new(RommError::NotFound {
                message: "HTTP 404".into(),
                detail: None,
            })
        }) as Arc<dyn Fn() -> anyhow::Error + Send + Sync>,
        Arc::new(|| anyhow::Error::new(RommError::Connection("Download incomplete: got 9 bytes, expected 4".into()))),
    ] {
        let env = env_with_rom().await;
        let f = err.clone();
        env.romm.fail_transfers(move || f());
        std::fs::create_dir_all(format!("{}/switch", env.roms_s())).unwrap();
        let tmp = format!("{}.tmp", target(&env, "Zelda.nsp"));
        std::fs::write(&tmp, b"partial").unwrap();
        env.svc.start_download(ROM).await;
        let entry = env.wait_status(ROM, "failed").await;
        assert!(entry["error"].is_string());
        assert!(!std::path::Path::new(&tmp).exists());
        assert!(job_row(&env, ROM).await.is_none());
        let failed = &env.events.named("download_failed")[0];
        assert_eq!(failed["rom_id"], ROM);
        assert_eq!(failed["platform_name"], "Nintendo Switch");
    }
}

#[tokio::test]
async fn failed_redownload_keeps_the_preexisting_install() {
    let env = env_with_rom().await;
    let path = target(&env, "Zelda.nsp");
    std::fs::create_dir_all(format!("{}/switch", env.roms_s())).unwrap();
    std::fs::write(&path, b"installed").unwrap();
    env.romm.fail_transfers(|| anyhow::anyhow!("boom"));
    env.svc.start_download(ROM).await;
    env.wait_status(ROM, "failed").await;
    assert_eq!(std::fs::read(&path).unwrap(), b"installed");
}

#[tokio::test]
async fn install_without_rom_row_fails_on_the_foreign_key() {
    let env = DownloadEnv::new().await;
    env.romm.set_rom(switch_rom(3, "Orphan.nsp", 10));
    env.romm.set_content(3, payload(10));
    env.svc.start_download(3).await;
    let entry = env.wait_status(3, "failed").await;
    assert!(entry["error"].as_str().unwrap().contains("FOREIGN KEY"), "{entry}");
}

async fn pause_partial(env: &DownloadEnv, at: u64) {
    *env.romm.stall_at.lock().unwrap() = Some(at);
    env.svc.start_download(ROM).await;
    env.romm.stalled.notified().await;
    env.svc.pause_download(ROM);
    env.wait_status(ROM, "paused").await;
    *env.romm.stall_at.lock().unwrap() = None;
}

#[tokio::test]
async fn resume_with_insufficient_space_interrupts_disk_full() {
    let env = env_with_rom().await;
    pause_partial(&env, 100).await;
    *env.files.free.lock().unwrap() = Some(1);
    let r = env.svc.resume_download(ROM).await;
    assert_eq!(r["reason"], "insufficient_space");
    let entry = env.entry(ROM).unwrap();
    assert_eq!((entry["status"].clone(), entry["reason"].clone()), (json!("interrupted"), json!("disk_full")));
    assert!(std::path::Path::new(&format!("{}.tmp", target(&env, "Zelda.nsp"))).exists());
}

#[tokio::test]
async fn resume_when_rom_is_gone_drops_the_job_and_partial() {
    let env = env_with_rom().await;
    pause_partial(&env, 100).await;
    env.romm.roms.lock().unwrap().clear();
    let r = env.svc.resume_download(ROM).await;
    assert_eq!(r["reason"], "not_found");
    assert_eq!(env.entry(ROM).unwrap()["status"], "failed");
    assert!(job_row(&env, ROM).await.is_none());
    assert!(!std::path::Path::new(&format!("{}.tmp", target(&env, "Zelda.nsp"))).exists());
    assert_eq!(env.events.named("download_failed").len(), 1);

    let env = env_with_rom().await;
    pause_partial(&env, 100).await;
    env.romm.fail_rom(ROM, RommError::Connection("down".into()));
    let r = env.svc.resume_download(ROM).await;
    assert_eq!(r["reason"], "server_unreachable");
    assert_eq!(job_row(&env, ROM).await.unwrap().interrupt_reason.as_deref(), Some("server_unreachable"));
}

#[tokio::test]
async fn shutdown_keeps_row_and_partial() {
    let env = env_with_rom().await;
    *env.romm.stall_at.lock().unwrap() = Some(1500);
    env.svc.start_download(ROM).await;
    env.romm.stalled.notified().await;
    env.svc.shutdown().await;
    let tmp = format!("{}.tmp", target(&env, "Zelda.nsp"));
    assert_eq!(std::fs::metadata(&tmp).unwrap().len(), 1500);
    assert_eq!(job_row(&env, ROM).await.unwrap().status, JobStatus::Downloading);
    assert_eq!(env.entry(ROM).unwrap()["status"], "downloading");
}

// ── Scheduler ────────────────────────────────────────────────────────────

async fn three_roms(env: &DownloadEnv) {
    for (id, name) in [(1, "a.nsp"), (2, "b.nsp"), (3, "c.nsp")] {
        env.seed_rom(id, "switch", name, None, None).await;
        env.romm.set_rom(switch_rom(id, name, 100));
        env.romm.set_content(id, payload(100));
    }
}

#[tokio::test]
async fn nothing_is_started_or_written_while_emudeck_storage_is_unavailable() {
    use crate::domain::emudeck_health::EmuDeckHealth;
    let env = DownloadEnv::new().await;
    three_roms(&env).await;
    for health in [EmuDeckHealth::RootMissing, EmuDeckHealth::Absent, EmuDeckHealth::Unreadable, EmuDeckHealth::Invalid] {
        *env.health.lock().unwrap() = health;
        let r = env.svc.start_download(1).await;
        assert_eq!(r["success"], false, "{health:?}");
        assert_eq!(r["reason"], "emudeck_unavailable", "{health:?}");
        assert_eq!(r["message"], health.message());
        let fw = env.svc.start_firmware_download(5).await;
        assert_eq!(fw["reason"], "emudeck_unavailable");
    }
    assert!(env.romm.calls().is_empty(), "no transfer was attempted");
    assert!(env.romm.get_rom_calls.lock().unwrap().is_empty(), "RomM was not even asked");
    assert!(env.jobs.all().await.unwrap().is_empty(), "no job was queued");
    assert_eq!(env.svc.emudeck_health()["health"], "invalid");

    // The card is back: the same download goes through.
    *env.health.lock().unwrap() = EmuDeckHealth::Ok;
    assert_eq!(env.svc.start_download(1).await["success"], true);
    env.wait_status(1, "completed").await;
    assert_eq!(env.svc.emudeck_health()["ok"], true);
}

#[tokio::test]
async fn one_transfer_at_a_time_in_position_order_with_move_to_front() {
    let env = DownloadEnv::new().await;
    three_roms(&env).await;
    *env.romm.stall_at.lock().unwrap() = Some(10);
    for id in [1, 2, 3] {
        assert_eq!(env.svc.start_download(id).await["success"], true);
    }
    env.romm.stalled.notified().await;
    env.wait_until("2 and 3 queued", || {
        env.events.statuses(2).contains(&"queued".into()) && env.events.statuses(3).contains(&"queued".into())
    })
    .await;
    assert_eq!(env.romm.calls().len(), 1, "only one transfer runs");
    let positions: Vec<i64> = env.jobs.all().await.unwrap().iter().map(|j| j.position).collect();
    assert_eq!(positions, [1, 2, 3]);

    // 3 jumps the line; the running transfer is not pre-empted.
    assert_eq!(env.svc.move_to_front(JobKey::rom(3)).await["message"], "Moved to front");
    assert_eq!(job_row(&env, 3).await.unwrap().position, 0);
    assert_eq!(env.svc.move_to_front(JobKey::rom(3)).await["success"], true, "already first is a no-op");
    assert_eq!(job_row(&env, 3).await.unwrap().position, 0);
    assert_eq!(env.svc.move_to_front(JobKey::rom(77)).await["reason"], "not_queued");
    let queue = env.svc.get_download_queue();
    let order: Vec<i64> = queue["downloads"].as_array().unwrap().iter().map(|r| r["rom_id"].as_i64().unwrap()).collect();
    assert_eq!(order, [3, 1, 2], "the queue is listed in position order");

    *env.romm.stall_at.lock().unwrap() = None;
    env.romm.release.notify_one();
    for id in [1, 3, 2] {
        env.wait_status(id, "completed").await;
    }
    let order: Vec<i64> = env.romm.calls().iter().map(|c| c.rom_id).collect();
    assert_eq!(order, [1, 3, 2]);
}

#[tokio::test]
async fn pausing_active_starts_next_and_pausing_waiting_skips_it() {
    let env = DownloadEnv::new().await;
    three_roms(&env).await;
    *env.romm.stall_at.lock().unwrap() = Some(10);
    for id in [1, 2, 3] {
        env.svc.start_download(id).await;
    }
    env.romm.stalled.notified().await;
    env.svc.pause_download(2);
    env.wait_status(2, "paused").await;
    assert_eq!(job_row(&env, 2).await.unwrap().status, JobStatus::Paused);
    env.svc.pause_download(1);
    env.wait_status(1, "paused").await;
    env.romm.stalled.notified().await;
    assert_eq!(env.romm.calls().last().unwrap().rom_id, 3);
    env.svc.cancel_download(3);
    env.wait_status(3, "cancelled").await;
    assert_eq!(env.svc.st().active, None);
}

// ── Startup reconciliation ───────────────────────────────────────────────

fn job(rom_id: i64, tmp: &str, status: JobStatus, position: i64, total: Option<u64>) -> DownloadJob {
    let mut j = DownloadJob::enqueue(
        NewJob {
            kind: JobKind::Rom,
            item_id: rom_id,
            rom_name: format!("Game {rom_id}"),
            platform_name: "Nintendo Switch".into(),
            file_name: "f".into(),
            target_path: tmp.trim_end_matches(".tmp").trim_end_matches(".zip").into(),
            tmp_path: tmp.into(),
            is_multi: tmp.ends_with(".zip.tmp"),
            position,
            total_bytes: total,
        },
        "t0",
    )
    .unwrap();
    match status {
        JobStatus::Paused => j.pause("t0").unwrap(),
        JobStatus::Downloading => j.start("t0").unwrap(),
        JobStatus::Interrupted => j.interrupt("auth_failed", "t0").unwrap(),
        _ => {}
    }
    j.range_supported = Some(true);
    j
}

#[tokio::test]
async fn reconciliation_restores_queue_cleans_orphans_and_resumes_in_order() {
    let env = DownloadEnv::new().await;
    three_roms(&env).await;
    let sw = format!("{}/switch", env.roms_s());
    std::fs::create_dir_all(&sw).unwrap();
    let (t1, t2, t3) = (format!("{sw}/a.nsp.tmp"), format!("{sw}/b.nsp.tmp"), format!("{sw}/c.nsp.tmp"));
    std::fs::write(&t1, payload(40)).unwrap();
    std::fs::write(&t2, payload(30)).unwrap();
    std::fs::write(&t3, payload(20)).unwrap();
    let orphan = format!("{sw}/orphan.nsp.tmp");
    std::fs::write(&orphan, b"x").unwrap();
    std::fs::write(format!("{sw}/old.zip.tmp"), b"x").unwrap();
    let bios = env.dir.path().join("Emulation/bios/prod.keys.tmp");
    std::fs::write(&bios, b"x").unwrap();
    env.jobs.save(&job(1, &t1, JobStatus::Downloading, 2, Some(100))).await.unwrap();
    env.jobs.save(&job(2, &t2, JobStatus::Paused, 3, Some(100))).await.unwrap();
    env.jobs.save(&job(3, &t3, JobStatus::Interrupted, 1, Some(100))).await.unwrap();

    *env.romm.stall_at.lock().unwrap() = Some(90);
    env.svc.reconcile_persisted_downloads().await;
    assert!(!std::path::Path::new(&orphan).exists());
    assert!(!std::path::Path::new(&format!("{sw}/old.zip.tmp")).exists());
    assert!(!bios.exists());
    assert!(std::path::Path::new(&t2).exists(), "a paused partial is kept");
    let paused = env.entry(2).unwrap();
    assert_eq!(paused["status"], "paused");
    assert_eq!(paused["bytes_downloaded"], 30);
    assert_eq!(job_row(&env, 1).await.unwrap().interrupt_reason.as_deref(), Some("restart"));

    *env.romm.stall_at.lock().unwrap() = None;
    env.romm.release.notify_one();
    env.wait_status(3, "completed").await;
    env.wait_status(1, "completed").await;
    let calls = env.romm.calls();
    assert_eq!(calls.iter().map(|c| c.rom_id).collect::<Vec<_>>(), [3, 1], "position order");
    assert_eq!((calls[0].offset, calls[1].offset), (20, 40));
    assert_eq!(env.entry(2).unwrap()["status"], "paused", "a user pause never auto-resumes");
    assert_eq!(std::fs::read(format!("{sw}/a.nsp")).unwrap(), payload(100));
}

#[tokio::test]
async fn reconciled_entry_reports_restart_and_partial_size() {
    let env = DownloadEnv::new().await;
    three_roms(&env).await;
    let sw = format!("{}/switch", env.roms_s());
    std::fs::create_dir_all(&sw).unwrap();
    let t1 = format!("{sw}/a.nsp.tmp");
    std::fs::write(&t1, payload(40)).unwrap();
    env.jobs.save(&job(1, &t1, JobStatus::Downloading, 1, Some(100))).await.unwrap();
    env.romm.fail_rom(1, RommError::Connection("down".into()));
    env.svc.reconcile_persisted_downloads().await;
    let e = env.entry(1).unwrap();
    assert_eq!(e["bytes_downloaded"], 40);
    assert_eq!(e["total_bytes"], 100);
    assert_eq!(e["progress"], 0.4);
    assert_eq!(e["resumable"], true);
}

#[tokio::test]
async fn unreachable_server_stops_auto_resume_without_retry() {
    let env = DownloadEnv::new().await;
    three_roms(&env).await;
    let sw = format!("{}/switch", env.roms_s());
    std::fs::create_dir_all(&sw).unwrap();
    for (id, pos) in [(1, 1), (2, 2), (3, 3)] {
        let t = format!("{sw}/{id}.tmp");
        std::fs::write(&t, b"x").unwrap();
        env.jobs.save(&job(id, &t, JobStatus::Downloading, pos, Some(100))).await.unwrap();
    }
    for id in [1, 2, 3] {
        env.romm.fail_rom(id, RommError::Connection("down".into()));
    }
    env.svc.reconcile_persisted_downloads().await;
    env.wait_until("all marked", || {
        [1, 2, 3]
            .iter()
            .all(|id| env.entry(*id).unwrap()["reason"] == "server_unreachable")
    })
    .await;
    assert_eq!(*env.romm.get_rom_calls.lock().unwrap(), vec![1], "no further attempts");
    for id in [1, 2, 3] {
        assert!(std::path::Path::new(&format!("{sw}/{id}.tmp")).exists());
        assert_eq!(
            job_row(&env, id).await.unwrap().interrupt_reason.as_deref(),
            Some("server_unreachable")
        );
    }
}

#[tokio::test]
async fn interrupted_downloads_resume_once_the_server_is_reachable_again() {
    let env = DownloadEnv::new().await;
    three_roms(&env).await;
    let sw = format!("{}/switch", env.roms_s());
    std::fs::create_dir_all(&sw).unwrap();
    for (id, pos) in [(1, 1), (2, 2)] {
        let t = format!("{sw}/{id}.tmp");
        std::fs::write(&t, b"x").unwrap();
        env.jobs.save(&job(id, &t, JobStatus::Downloading, pos, Some(100))).await.unwrap();
        env.romm.fail_rom(id, RommError::Connection("down".into()));
    }
    env.svc.reconcile_persisted_downloads().await;
    env.wait_until("both marked", || {
        [1, 2].iter().all(|id| env.entry(*id).unwrap()["reason"] == "server_unreachable")
    })
    .await;
    // Still down: another round changes nothing but the attempt.
    env.svc.resume_unreachable_interrupted().await;
    env.wait_until("retry ended", || env.svc.st().auto_resume.as_ref().is_none_or(|h| h.is_finished())).await;
    assert_eq!(env.entry(1).unwrap()["status"], "interrupted");

    // The server is back.
    for id in [1, 2] {
        env.romm.set_rom(switch_rom(id, "x.nsp", 100));
        env.romm.set_content(id, payload(100));
    }
    env.svc.resume_unreachable_interrupted().await;
    for id in [1, 2] {
        env.wait_status(id, "completed").await;
    }
}

#[tokio::test]
async fn failed_job_load_never_deletes_rom_partials() {
    let env = DownloadEnv::new().await;
    let sw = format!("{}/switch", env.roms_s());
    std::fs::create_dir_all(&sw).unwrap();
    let partial = format!("{sw}/big.nsp.tmp");
    std::fs::write(&partial, b"precious").unwrap();
    let bios = env.dir.path().join("Emulation/bios/x.tmp");
    std::fs::write(&bios, b"x").unwrap();
    env.sql("DROP TABLE download_jobs").await;
    env.svc.reconcile_persisted_downloads().await;
    assert!(std::path::Path::new(&partial).exists());
    assert!(!bios.exists(), "BIOS tmp files are still wiped");
    assert!(env.svc.get_download_queue()["downloads"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn complete_partial_skips_transfer_and_oversized_restarts() {
    let env = DownloadEnv::new().await;
    three_roms(&env).await;
    let sw = format!("{}/switch", env.roms_s());
    std::fs::create_dir_all(&sw).unwrap();
    let t1 = format!("{sw}/a.nsp.tmp");
    std::fs::write(&t1, payload(100)).unwrap();
    env.jobs.save(&job(1, &t1, JobStatus::Downloading, 1, Some(100))).await.unwrap();
    let t2 = format!("{sw}/b.nsp.tmp");
    std::fs::write(&t2, payload(150)).unwrap();
    env.jobs.save(&job(2, &t2, JobStatus::Downloading, 2, Some(100))).await.unwrap();
    env.svc.reconcile_persisted_downloads().await;
    env.wait_status(1, "completed").await;
    env.wait_status(2, "completed").await;
    let calls = env.romm.calls();
    assert_eq!(calls.len(), 1, "the complete partial skipped the request");
    assert_eq!((calls[0].rom_id, calls[0].resume, calls[0].offset), (2, false, 0));
    assert_eq!(std::fs::read(format!("{sw}/a.nsp")).unwrap(), payload(100));
    assert_eq!(std::fs::read(format!("{sw}/b.nsp")).unwrap(), payload(100));
}

#[tokio::test]
async fn extraction_crash_re_extracts_from_the_complete_archive() {
    let env = DownloadEnv::new().await;
    env.seed_rom(9, "switch", "Mario Kart 8 Deluxe", None, None).await;
    env.romm.set_rom(folder_rom(9));
    let sw = format!("{}/switch", env.roms_s());
    let dir = format!("{sw}/Mario Kart 8 Deluxe");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(format!("{dir}/half-written.nsp"), b"junk").unwrap();
    let archive = zip_bytes(&[("base.nsp", &[1u8; 2000]), ("update/u%20pd.nsp", &[2u8; 10])]);
    let tmp = format!("{sw}/base.nsp.zip.tmp");
    std::fs::write(&tmp, &archive).unwrap();
    let mut j = job(9, &tmp, JobStatus::Downloading, 1, Some(archive.len() as u64));
    j.begin_extract("t").unwrap();
    env.jobs.save(&j).await.unwrap();
    env.svc.reconcile_persisted_downloads().await;
    env.wait_status(9, "completed").await;
    assert!(env.romm.calls().is_empty(), "no new transfer");
    assert!(!std::path::Path::new(&format!("{dir}/half-written.nsp")).exists());
    assert_eq!(std::fs::read(format!("{dir}/base.nsp")).unwrap().len(), 2000);
}

#[tokio::test]
async fn no_jobs_schedules_nothing() {
    let env = DownloadEnv::new().await;
    env.svc.reconcile_persisted_downloads().await;
    assert!(env.svc.st().auto_resume.is_none());
}

// ── Supersede ────────────────────────────────────────────────────────────

#[tokio::test]
async fn start_supersedes_unbound_sibling_but_keeps_grandfathered() {
    let env = env_with_rom().await;
    env.seed_rom(ROM, "switch", "Zelda.nsp", Some(4242), Some("igdb:1:5")).await;
    env.seed_rom(8, "switch", "Zelda (EU).nsp", None, Some("igdb:1:5")).await;
    env.seed_rom(9, "switch", "Zelda (JP).nsp", Some(999), Some("igdb:1:5")).await;
    let sw = format!("{}/switch", env.roms_s());
    std::fs::create_dir_all(&sw).unwrap();
    for (id, name) in [(8, "Zelda (EU).nsp"), (9, "Zelda (JP).nsp")] {
        let p = format!("{sw}/{name}");
        std::fs::write(&p, b"x").unwrap();
        let install =
            RomInstall::mark_installed(id, p, None, "switch".into(), "switch".into(), "t".into()).unwrap();
        env.library.save_install(&install).await.unwrap();
    }
    assert_eq!(env.svc.conflicting_sibling_install_ids(ROM).await.unwrap(), vec![8]);
    env.svc.start_download(ROM).await;
    env.wait_status(ROM, "completed").await;
    assert!(env.library.install(8).await.unwrap().is_none());
    assert!(!std::path::Path::new(&format!("{sw}/Zelda (EU).nsp")).exists());
    assert!(env.library.install(9).await.unwrap().is_some(), "grandfathered shortcut kept");
    assert!(env.svc.conflicting_sibling_install_ids(99).await.unwrap().is_empty());
}

#[tokio::test]
async fn resume_refused_when_binding_moved_to_a_sibling() {
    let env = env_with_rom().await;
    env.seed_rom(ROM, "switch", "Zelda.nsp", None, Some("g")).await;
    pause_partial(&env, 100).await;
    env.seed_rom(8, "switch", "Zelda (EU).nsp", Some(4242), Some("g")).await;
    let r = env.svc.resume_download(ROM).await;
    assert_eq!(r["reason"], "superseded");
    assert!(env.entry(ROM).is_none());
    assert!(job_row(&env, ROM).await.is_none());
}

#[tokio::test]
async fn queue_prunes_oldest_terminal_entries() {
    let env = DownloadEnv::new().await;
    {
        let mut st = env.svc.st();
        for id in 0..55 {
            st.queue.insert(
                JobKey::rom(id),
                QueueEntry {
                    kind: JobKind::Rom,
                    item_id: id,
                    rom_name: String::new(),
                    platform_name: String::new(),
                    file_name: String::new(),
                    status: if id == 0 { "downloading".into() } else { "completed".into() },
                    progress: 1.0,
                    bytes_downloaded: 0,
                    total_bytes: 0,
                    resumable: false,
                    reason: None,
                    error: None,
                    ..QueueEntry::default()
                },
            );
        }
        st.prune();
        assert_eq!(st.queue.len(), 51);
        assert!(st.queue.contains_key(&JobKey::rom(0)) && !st.queue.contains_key(&JobKey::rom(1)) && st.queue.contains_key(&JobKey::rom(54)));
    }
}

#[tokio::test]
async fn progress_is_throttled_but_the_final_tick_always_emits() {
    let env = env_with_rom().await;
    *env.clock.mono.lock().unwrap() = 0.0;
    let clock = Arc::new(crate::services::testing::FakeClock {
        mono: std::sync::Mutex::new(0.0),
        step: 0.01,
    });
    let plan = Plan {
        rom_id: ROM,
        rom: Value::Null,
        target_path: String::new(),
        system: String::new(),
        file_name: "f".into(),
        rom_name: "r".into(),
        platform_name: "p".into(),
        has_multiple: false,
        extract_dir_name: String::new(),
        restart: false,
        key: JobKey::rom(ROM),
        firmware: None,
    };
    let svc = DownloadService::new(DownloadServiceConfig {
        romm: env.romm.clone(),
        files: env.files.clone(),
        jobs: env.jobs.clone(),
        library: env.library.clone(),
        paths: Arc::new(crate::services::testing::FixedPaths {
            roms: String::new(),
            bios: String::new(),
        }),
        systems: Arc::new(crate::services::testing::TestSystems),
        launch: env.launch.clone(),
        removal: Arc::new(RomRemovalService::new(crate::services::rom_removal::RomRemovalServiceConfig {
            library: env.library.clone(),
            files: env.files.clone(),
            paths: Arc::new(crate::services::testing::FixedPaths {
                roms: String::new(),
                bios: String::new(),
            }),
        })),
        events: env.events.clone(),
        clock,
        firmware_api: env.romm.clone(),
        firmware_jobs: env.firmware_jobs.clone(),
    });
    let mut hook = svc.progress_hook(&plan);
    hook(1, 10); // no entry yet: nothing
    assert!(env.events.named("download_progress").is_empty());
    svc.st().queue.insert(
        JobKey::rom(ROM),
        QueueEntry {
            kind: JobKind::Rom,
            item_id: ROM,
            rom_name: "r".into(),
            platform_name: "p".into(),
            file_name: "f".into(),
            status: "downloading".into(),
            progress: 0.0,
            bytes_downloaded: 0,
            total_bytes: 0,
            resumable: true,
            reason: Some("restart".into()),
            error: None,
            ..QueueEntry::default()
        },
    );
    let mut hook = svc.progress_hook(&plan);
    hook(1, 10);
    hook(2, 10);
    hook(3, 10);
    hook(10, 10);
    let frames = env.events.named("download_progress");
    assert_eq!(frames.len(), 2, "first tick + final tick");
    assert_eq!(frames[1]["bytes_downloaded"], 10);
    assert!(frames[1].get("reason").is_none());
    let mut zero = svc.progress_hook(&plan);
    zero(0, 0);
    assert_eq!(env.events.named("download_progress").last().unwrap()["progress"], 0.0);
    // The observation (full size) is persisted once per attempt.
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    assert!(svc.st().persisted_totals.contains(&JobKey::rom(ROM)));
}

// ── Multipart archives (RomM's cached ZIP) ───────────────────────────────

mod archives {
    use super::*;
    use crate::domain::archive_transfer::{ArchiveIdentity, ResumePlan};

    const ETAG: &str = "\"cached-v1\"";

    fn members() -> Vec<(&'static str, Vec<u8>)> {
        vec![
            ("base.nsp", (0..2000u32).map(|i| (i % 251) as u8).collect()),
            ("update/u%20pd.nsp", vec![2u8; 10]),
        ]
    }

    fn archive() -> Vec<u8> {
        let m = members();
        let refs: Vec<(&str, &[u8])> = m.iter().map(|(n, d)| (*n, d.as_slice())).collect();
        zip_bytes(&refs)
    }

    fn identity(zip: &[u8]) -> ArchiveIdentity {
        ArchiveIdentity {
            etag: ETAG.into(),
            last_modified: Some("Mon, 01 Jan 2026 00:00:00 GMT".into()),
            length: zip.len() as u64,
        }
    }

    /// ROM 9 (a Switch folder game) whose cached archive RomM would serve.
    async fn env() -> (DownloadEnv, Vec<u8>, String) {
        let env = DownloadEnv::new().await;
        env.seed_rom(9, "switch", "Mario Kart 8 Deluxe", Some(55), None).await;
        env.romm.set_rom(folder_rom(9));
        let zip = archive();
        env.romm.set_content(9, zip.clone());
        *env.romm.range_supported.lock().unwrap() = true;
        *env.romm.archive_identity.lock().unwrap() = Some(identity(&zip));
        let tmp = format!("{}/switch/base.nsp.zip.tmp", env.roms_s());
        (env, zip, tmp)
    }

    fn seed_partial(tmp: &str, bytes: &[u8]) {
        std::fs::create_dir_all(std::path::Path::new(tmp).parent().unwrap()).unwrap();
        std::fs::write(tmp, bytes).unwrap();
    }

    async fn seed_job(env: &DownloadEnv, tmp: &str, total: Option<u64>, f: impl FnOnce(&mut DownloadJob)) {
        let mut j = job(9, tmp, JobStatus::Interrupted, 1, total);
        f(&mut j);
        env.jobs.save(&j).await.unwrap();
    }

    /// Wait until the queue entry is interrupted for `reason` (the seeded job starts out
    /// interrupted for another reason, so the status alone is not enough).
    async fn wait_interrupted(env: &DownloadEnv, reason: &str) -> Value {
        for _ in 0..300 {
            if let Some(e) = env.entry(9).filter(|e| e["status"] == "interrupted" && e["reason"] == reason) {
                if !env.svc.active_download_rom_ids().contains(&9) {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    return e;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("never interrupted for {reason}: {:?}", env.entry(9));
    }

    fn mode(env: &DownloadEnv, i: usize) -> TransferMode {
        env.romm.calls()[i].mode.clone()
    }

    #[tokio::test]
    async fn a_new_multipart_download_asks_for_the_cached_zip_and_saves_its_identity_before_any_byte() {
        let (env, zip, _) = env().await;
        *env.romm.stall_at.lock().unwrap() = Some(100);
        env.svc.start_download(9).await;
        env.romm.stalled.notified().await;
        assert_eq!(mode(&env, 0), TransferMode::Archive(ResumePlan::Fresh));
        // The identity was handed over when the file was still empty ...
        assert_eq!(*env.romm.identity_events.lock().unwrap(), vec![(Some(identity(&zip)), 0)]);
        // ... and is in the row while bytes are flowing.
        let row = job_row(&env, 9).await.unwrap();
        assert_eq!(row.etag.as_deref(), Some(ETAG));
        assert_eq!(row.representation_length, Some(zip.len() as u64));
        assert_eq!(row.last_modified.as_deref(), Some("Mon, 01 Jan 2026 00:00:00 GMT"));
        assert!(row.cached_zip && !row.needs_recovery);
        assert_eq!(row.archive_identity(), Some(identity(&zip)));
        env.svc.cancel_download(9);
        env.wait_status(9, "cancelled").await;
    }

    #[tokio::test]
    async fn a_completed_multipart_download_extracts_a_byte_identical_archive() {
        let (env, zip, tmp) = env().await;
        env.svc.start_download(9).await;
        env.wait_status(9, "completed").await;
        assert!(!std::path::Path::new(&tmp).exists());
        let dir = format!("{}/switch/Mario Kart 8 Deluxe", env.roms_s());
        assert_eq!(std::fs::read(format!("{dir}/base.nsp")).unwrap(), members()[0].1);
        assert!(job_row(&env, 9).await.is_none());
        let _ = zip;
    }

    #[tokio::test]
    async fn a_saved_identity_resumes_with_exactly_that_identity() {
        let (env, zip, tmp) = env().await;
        seed_partial(&tmp, &zip[..900]);
        seed_job(&env, &tmp, Some(zip.len() as u64), |j| j.record_archive_identity(Some(&identity(&zip)), "t0")).await;
        env.svc.reconcile_persisted_downloads().await;
        env.wait_status(9, "completed").await;
        assert_eq!(mode(&env, 0), TransferMode::Archive(ResumePlan::Append(identity(&zip))));
        assert_eq!(env.romm.calls()[0].offset, 900, "the actual file size is the offset");
        assert!(env.romm.identity_events.lock().unwrap().is_empty(), "nothing new to save when appending");
        let dir = format!("{}/switch/Mario Kart 8 Deluxe", env.roms_s());
        assert_eq!(std::fs::read(format!("{dir}/base.nsp")).unwrap(), members()[0].1);
    }

    #[tokio::test]
    async fn a_partial_without_a_validator_is_restarted_not_appended() {
        let (env, zip, tmp) = env().await;
        seed_partial(&tmp, &zip[..900]);
        seed_job(&env, &tmp, None, |_| {}).await;
        env.svc.reconcile_persisted_downloads().await;
        env.wait_status(9, "completed").await;
        assert_eq!(mode(&env, 0), TransferMode::Archive(ResumePlan::Fresh));
        assert_eq!(env.romm.calls()[0].offset, 0);
    }

    #[tokio::test]
    async fn a_legacy_partial_is_kept_and_never_auto_resumed_but_restarts_when_the_user_asks() {
        let (env, zip, tmp) = env().await;
        seed_partial(&tmp, &zip[..900]);
        seed_job(&env, &tmp, None, |j| j.needs_recovery = true).await;
        env.svc.reconcile_persisted_downloads().await;
        wait_interrupted(&env, "needs_recovery").await;
        assert!(env.romm.calls().is_empty(), "no request was made for an unverifiable partial");
        assert_eq!(std::fs::read(&tmp).unwrap(), zip[..900], "user progress is preserved");
        assert!(job_row(&env, 9).await.unwrap().needs_recovery);

        // The user's Resume is the explicit restart.
        assert_eq!(env.svc.resume_download(9).await["success"], true);
        env.wait_status(9, "completed").await;
        assert_eq!(mode(&env, 0), TransferMode::Archive(ResumePlan::Fresh));
        assert_eq!(env.romm.calls()[0].offset, 0);
        assert_eq!(env.romm.identity_events.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_legacy_partial_that_is_already_a_complete_valid_archive_finishes_without_a_transfer() {
        let (env, zip, tmp) = env().await;
        seed_partial(&tmp, &zip);
        seed_job(&env, &tmp, Some(zip.len() as u64), |j| j.needs_recovery = true).await;
        env.svc.reconcile_persisted_downloads().await;
        env.wait_status(9, "completed").await;
        assert!(env.romm.calls().is_empty());
        let dir = format!("{}/switch/Mario Kart 8 Deluxe", env.roms_s());
        assert_eq!(std::fs::read(format!("{dir}/base.nsp")).unwrap(), members()[0].1);
    }

    #[tokio::test]
    async fn a_complete_but_corrupt_archive_is_discarded_and_fetched_again() {
        let (env, zip, tmp) = env().await;
        let mut bad = zip.clone();
        bad[100] ^= 0xff; // inside the first member's stored data
        seed_partial(&tmp, &bad);
        seed_job(&env, &tmp, Some(zip.len() as u64), |j| j.record_archive_identity(Some(&identity(&zip)), "t0")).await;
        env.svc.reconcile_persisted_downloads().await;
        env.wait_status(9, "completed").await;
        assert_eq!(env.romm.calls().len(), 1, "one fresh transfer, no append onto the corrupt bytes");
        assert_eq!(mode(&env, 0), TransferMode::Archive(ResumePlan::Fresh));
        let dir = format!("{}/switch/Mario Kart 8 Deluxe", env.roms_s());
        assert_eq!(std::fs::read(format!("{dir}/base.nsp")).unwrap(), members()[0].1);
    }

    #[tokio::test]
    async fn an_untrustworthy_resume_response_flags_the_job_and_keeps_the_partial() {
        let (env, zip, tmp) = env().await;
        seed_partial(&tmp, &zip[..900]);
        seed_job(&env, &tmp, Some(zip.len() as u64), |j| j.record_archive_identity(Some(&identity(&zip)), "t0")).await;
        env.romm.fail_transfers(|| anyhow::Error::new(ArchiveRecoveryNeeded("length changed".into())));
        env.svc.reconcile_persisted_downloads().await;
        wait_interrupted(&env, "needs_recovery").await;
        assert_eq!(std::fs::read(&tmp).unwrap(), zip[..900]);
        assert!(job_row(&env, 9).await.unwrap().needs_recovery);
        // After a plugin restart it is not tried again by itself.
        env.romm.transfer_error.lock().unwrap().take();
        let calls_before = env.romm.calls().len();
        env.svc.reconcile_persisted_downloads().await;
        wait_interrupted(&env, "needs_recovery").await;
        assert_eq!(env.romm.calls().len(), calls_before);
    }

    #[tokio::test]
    async fn a_response_timeout_is_a_timeout_not_an_unreachable_server() {
        let (env, _, _) = env().await;
        env.romm.fail_transfers(|| anyhow::Error::new(RommError::Timeout("no headers".into())));
        env.svc.start_download(9).await;
        let entry = env.wait_status(9, "interrupted").await;
        assert_eq!(entry["reason"], "timeout");
    }

    #[tokio::test]
    async fn the_waiting_phase_is_published_and_clears_when_bytes_flow() {
        let (env, _, _) = env().await;
        *env.romm.hold_headers.lock().unwrap() = true;
        env.svc.start_download(9).await;
        let mut waiting = None;
        for _ in 0..200 {
            if let Some(e) = env.entry(9).filter(|e| e["phase"] == "waiting") {
                waiting = Some(e);
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let waiting = waiting.expect("a waiting phase");
        assert_eq!(waiting["status"], "downloading");
        assert_eq!((waiting["attempt"].clone(), waiting["max_attempts"].clone()), (json!(1), json!(3)));
        assert!(waiting["phase_since"].as_f64().is_some());
        assert_eq!(waiting["bytes_downloaded"], 0);
        let frame = env.events.named("download_progress").into_iter().rev().find(|f| f["phase"] == "waiting").unwrap();
        assert_eq!(frame["status"], "downloading");

        env.romm.release.notify_one();
        env.wait_status(9, "completed").await;
        let last = env.events.named("download_progress").into_iter().rev().find(|f| f["status"] == "downloading").unwrap();
        assert!(last.get("phase").is_none(), "{last}");
    }

    #[tokio::test]
    async fn cancel_and_pause_work_while_waiting_for_headers() {
        let (env, _, tmp) = env().await;
        *env.romm.hold_headers.lock().unwrap() = true;
        env.svc.start_download(9).await;
        for _ in 0..200 {
            if env.entry(9).is_some_and(|e| e["phase"] == "waiting") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        env.svc.pause_download(9);
        let paused = env.wait_status(9, "paused").await;
        assert!(paused.get("phase").is_none(), "a paused download is not waiting: {paused}");
        assert!(job_row(&env, 9).await.is_some());

        assert_eq!(env.svc.resume_download(9).await["success"], true);
        for _ in 0..200 {
            if env.entry(9).is_some_and(|e| e["phase"] == "waiting") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        env.svc.cancel_download(9);
        env.wait_status(9, "cancelled").await;
        assert!(job_row(&env, 9).await.is_none());
        assert!(!std::path::Path::new(&tmp).exists());
        assert!(env.svc.active_download_rom_ids().is_empty(), "no worker keeps the destination");
    }
}

// ── Job kinds ────────────────────────────────────────────────────────────

mod kinds {
    use super::*;

    fn entry(kind: JobKind, id: i64, status: &str) -> QueueEntry {
        QueueEntry {
            kind,
            item_id: id,
            rom_name: "n".into(),
            status: status.into(),
            ..QueueEntry::default()
        }
    }

    #[test]
    fn a_rom_row_and_frame_carry_kind_and_rom_id_a_firmware_one_firmware_id() {
        let rom = entry(JobKind::Rom, 7, "queued");
        let fw = entry(JobKind::Firmware, 7, "queued");
        for v in [rom.frame(), rom.row()] {
            assert_eq!((v["kind"].clone(), v["rom_id"].clone()), (json!("rom"), json!(7)));
            assert!(v.get("firmware_id").is_none());
        }
        for v in [fw.frame(), fw.row()] {
            assert_eq!((v["kind"].clone(), v["firmware_id"].clone()), (json!("firmware"), json!(7)));
            assert!(v.get("rom_id").is_none(), "a firmware id must never be read as a ROM id");
        }
        assert_eq!(rom.key(), JobKey::rom(7));
        assert_eq!(fw.key(), JobKey::firmware(7));
    }

    #[tokio::test]
    async fn the_same_id_in_two_kinds_is_two_independent_queue_entries() {
        let env = DownloadEnv::new().await;
        {
            let mut st = env.svc.st();
            st.queue.insert(JobKey::rom(7), entry(JobKind::Rom, 7, "downloading"));
            st.queue.insert(JobKey::firmware(7), entry(JobKind::Firmware, 7, "paused"));
            st.in_progress.insert(JobKey::firmware(7));
        }
        let rows = env.svc.get_download_queue();
        let rows = rows["downloads"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert!(env.svc.update_entry(JobKey::firmware(7), |e| e.status = "interrupted".into()));
        assert_eq!(env.svc.st().queue[&JobKey::rom(7)].status, "downloading", "the ROM entry is untouched");
        // Only ROM downloads block version switches and uninstalls.
        assert!(env.svc.active_download_rom_ids().is_empty());
        env.svc.st().in_progress.insert(JobKey::rom(7));
        assert_eq!(env.svc.active_download_rom_ids(), HashSet::from([7]));
    }

    #[tokio::test]
    async fn cancelling_a_rom_never_touches_a_firmware_job_with_the_same_id() {
        let env = env_with_rom().await;
        // A stopped firmware download with the same number as the ROM.
        env.svc.st().queue.insert(JobKey::firmware(ROM), entry(JobKind::Firmware, ROM, "paused"));
        *env.romm.stall_at.lock().unwrap() = Some(100);
        env.svc.start_download(ROM).await;
        env.romm.stalled.notified().await;
        env.svc.cancel_download(ROM);
        env.wait_status(ROM, "cancelled").await;
        let st = env.svc.st();
        assert_eq!(st.queue[&JobKey::firmware(ROM)].status, "paused");
    }
}

// ── Firmware downloads ───────────────────────────────────────────────────

mod firmware {
    use super::*;
    use crate::services::ports::{FirmwareFileKind, FirmwareTarget};

    const FW: i64 = 3;

    fn sha1(bytes: &[u8]) -> String {
        let mut ctx = ring::digest::Context::new(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY);
        ctx.update(bytes);
        ctx.finish().as_ref().iter().map(|b| format!("{b:02x}")).collect()
    }

    fn target(env: &DownloadEnv, name: &str, bytes: &[u8], kind: FirmwareFileKind, copy_to: &[&str]) -> FirmwareTarget {
        let dir = match kind {
            FirmwareFileKind::Key => format!("{}/home/.local/share/eden/keys", env.dir.path().display()),
            FirmwareFileKind::Archive => format!("{}/Emulation/bios/switch", env.dir.path().display()),
        };
        FirmwareTarget {
            firmware_id: FW,
            file_name: name.into(),
            size: bytes.len() as u64,
            sha1: Some(sha1(bytes)),
            kind,
            dest_path: format!("{dir}/{name}"),
            copy_to: copy_to.iter().map(|p| format!("{}/home/{p}/{name}", env.dir.path().display())).collect(),
        }
    }

    fn register(env: &DownloadEnv, t: &FirmwareTarget, bytes: &[u8]) {
        env.firmware_jobs.targets.lock().unwrap().insert(t.firmware_id, t.clone());
        env.romm.set_firmware_content(t.firmware_id, bytes.to_vec());
    }

    async fn wait_fw(env: &DownloadEnv, status: &str) -> Value {
        for _ in 0..500 {
            let entry = env.svc.get_download_queue()["downloads"]
                .as_array()
                .unwrap()
                .iter()
                .find(|e| e["firmware_id"] == FW)
                .cloned();
            if let Some(e) = entry.filter(|e| e["status"] == status && !env.svc.st().in_progress.contains(&JobKey::firmware(FW))) {
                tokio::time::sleep(Duration::from_millis(5)).await;
                return e;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("firmware download never reached {status}");
    }

    #[tokio::test]
    async fn a_key_file_is_verified_stored_and_copied_to_the_other_emulators_that_lack_it() {
        let env = DownloadEnv::new().await;
        let bytes = b"prod keys from RomM".to_vec();
        let t = target(&env, "prod.keys", &bytes, FirmwareFileKind::Key, &[".config/Ryujinx/system", ".local/share/yuzu/keys"]);
        register(&env, &t, &bytes);
        assert_eq!(env.svc.start_firmware_download(FW).await["success"], true);
        wait_fw(&env, "completed").await;
        assert_eq!(std::fs::read(&t.dest_path).unwrap(), bytes);
        for dst in &t.copy_to {
            assert_eq!(std::fs::read(dst).unwrap(), bytes, "{dst}");
        }
        assert!(!std::path::Path::new(&format!("{}.tmp", t.dest_path)).exists());
        assert!(env.jobs.get(JobKey::firmware(FW)).await.unwrap().is_none(), "the finished job's row is gone");
        let [done] = env.events.named("download_complete").try_into().unwrap();
        assert_eq!(done["kind"], "firmware");
        assert_eq!(done["firmware_id"], FW);
        assert!(done.get("rom_id").is_none(), "a firmware id is never announced as a ROM id");
        assert_eq!(done["file_path"], t.dest_path);
        // its progress frames are firmware frames too
        let frames = env.events.named("download_progress");
        assert!(frames.iter().all(|f| f["kind"] == "firmware" && f.get("rom_id").is_none()), "{frames:?}");
    }

    #[tokio::test]
    async fn a_download_that_does_not_match_romms_checksum_is_not_installed() {
        let env = DownloadEnv::new().await;
        let bytes = b"expected".to_vec();
        let t = target(&env, "prod.keys", &bytes, FirmwareFileKind::Key, &[".config/Ryujinx/system"]);
        env.firmware_jobs.targets.lock().unwrap().insert(FW, t.clone());
        env.romm.set_firmware_content(FW, b"corrupted!".to_vec());
        env.svc.start_firmware_download(FW).await;
        let entry = wait_fw(&env, "failed").await;
        assert!(entry["error"].as_str().unwrap().contains("does not match the checksum"), "{entry}");
        assert!(!std::path::Path::new(&t.dest_path).exists(), "nothing was installed");
        assert!(!std::path::Path::new(&t.copy_to[0]).exists(), "and nothing was copied");
        assert!(!std::path::Path::new(&format!("{}.tmp", t.dest_path)).exists(), "the bad download was removed");
        assert_eq!(env.events.named("download_failed")[0]["firmware_id"], FW);
    }

    #[tokio::test]
    async fn an_existing_key_file_that_appears_during_the_download_is_never_replaced() {
        let env = DownloadEnv::new().await;
        let bytes = b"romm keys".to_vec();
        let t = target(&env, "title.keys", &bytes, FirmwareFileKind::Key, &[".config/Ryujinx/system"]);
        register(&env, &t, &bytes);
        // the user's own file shows up at the destination before the transfer finishes
        std::fs::create_dir_all(std::path::Path::new(&t.dest_path).parent().unwrap()).unwrap();
        std::fs::write(&t.dest_path, b"my own keys").unwrap();
        env.svc.start_firmware_download(FW).await;
        wait_fw(&env, "completed").await;
        assert_eq!(std::fs::read(&t.dest_path).unwrap(), b"my own keys");
        assert_eq!(std::fs::read(&t.copy_to[0]).unwrap(), bytes, "the other emulator still gets RomM's verified file");
        assert!(!std::path::Path::new(&format!("{}.tmp", t.dest_path)).exists());
    }

    #[tokio::test]
    async fn an_existing_copy_at_another_emulator_is_left_alone() {
        let env = DownloadEnv::new().await;
        let bytes = b"romm keys".to_vec();
        let t = target(&env, "prod.keys", &bytes, FirmwareFileKind::Key, &[".config/Ryujinx/system"]);
        register(&env, &t, &bytes);
        std::fs::create_dir_all(std::path::Path::new(&t.copy_to[0]).parent().unwrap()).unwrap();
        std::fs::write(&t.copy_to[0], b"ryujinx own").unwrap();
        env.svc.start_firmware_download(FW).await;
        wait_fw(&env, "completed").await;
        assert_eq!(std::fs::read(&t.copy_to[0]).unwrap(), b"ryujinx own");
    }

    #[tokio::test]
    async fn the_archive_goes_to_the_bios_folder_and_a_firmware_download_can_be_paused_and_resumed() {
        let env = DownloadEnv::new().await;
        let bytes: Vec<u8> = (0..4000u32).map(|i| (i % 251) as u8).collect();
        let t = target(&env, "Firmware.23.0.1.zip", &bytes, FirmwareFileKind::Archive, &[]);
        register(&env, &t, &bytes);
        *env.romm.stall_at.lock().unwrap() = Some(1000);
        *env.romm.range_supported.lock().unwrap() = true;
        env.svc.start_firmware_download(FW).await;
        env.romm.stalled.notified().await;
        assert_eq!(env.svc.pause_job(JobKey::firmware(FW))["success"], true);
        let paused = wait_fw(&env, "paused").await;
        assert_eq!((paused["kind"].clone(), paused["bytes_downloaded"].clone()), (json!("firmware"), json!(1000)));
        assert_eq!(std::fs::read(format!("{}.tmp", t.dest_path)).unwrap(), bytes[..1000], "the partial is kept");
        assert!(env.jobs.get(JobKey::firmware(FW)).await.unwrap().is_some());

        *env.romm.stall_at.lock().unwrap() = None;
        assert_eq!(env.svc.resume_firmware_download(FW).await["success"], true);
        wait_fw(&env, "completed").await;
        let names: Vec<String> = env.romm.calls().iter().map(|c| c.file_name.clone()).collect();
        assert_eq!(names, ["Firmware.23.0.1.zip", "Firmware.23.0.1.zip"], "the transfer and its resume");
        assert!(env.romm.calls()[1].resume && env.romm.calls()[1].offset == 1000, "continued from the partial");
        assert_eq!(std::fs::read(&t.dest_path).unwrap(), bytes, "byte-identical after the pause");
    }

    #[tokio::test]
    async fn cancelling_removes_the_partial_but_never_the_destination() {
        let env = DownloadEnv::new().await;
        let bytes: Vec<u8> = vec![9u8; 4000];
        let t = target(&env, "Firmware.23.0.1.zip", &bytes, FirmwareFileKind::Archive, &[]);
        register(&env, &t, &bytes);
        *env.romm.stall_at.lock().unwrap() = Some(500);
        env.svc.start_firmware_download(FW).await;
        env.romm.stalled.notified().await;
        env.svc.cancel_job(JobKey::firmware(FW));
        wait_fw(&env, "cancelled").await;
        assert!(!std::path::Path::new(&format!("{}.tmp", t.dest_path)).exists());
        assert!(env.jobs.get(JobKey::firmware(FW)).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn nothing_to_download_is_reported_and_leaves_no_job() {
        let env = DownloadEnv::new().await;
        let r = env.svc.start_firmware_download(99).await;
        assert_eq!(r["success"], false);
        assert_eq!(r["reason"], "nothing_to_download");
        assert!(env.jobs.all().await.unwrap().is_empty());
        assert!(env.svc.st().in_progress.is_empty(), "the claim is released");
    }

    #[tokio::test]
    async fn a_firmware_download_shares_the_single_transfer_slot_with_rom_downloads() {
        let env = env_with_rom().await;
        let bytes = vec![1u8; 2000];
        let t = target(&env, "prod.keys", &bytes, FirmwareFileKind::Key, &[]);
        register(&env, &t, &bytes);
        *env.romm.stall_at.lock().unwrap() = Some(100);
        env.svc.start_download(ROM).await; // the ROM holds the slot (stalled)
        env.romm.stalled.notified().await;
        env.svc.start_firmware_download(FW).await;
        // the firmware job queues behind it
        tokio::time::sleep(Duration::from_millis(50)).await;
        let queue = env.svc.get_download_queue();
        let fw = queue["downloads"].as_array().unwrap().iter().find(|e| e["firmware_id"] == FW).unwrap();
        assert_eq!(fw["status"], "queued");
        assert!(!std::path::Path::new(&t.dest_path).exists());
        // finishing the ROM hands over the slot
        *env.romm.stall_at.lock().unwrap() = None;
        env.romm.release.notify_waiters();
        wait_fw(&env, "completed").await;
        assert_eq!(std::fs::read(&t.dest_path).unwrap(), bytes);
        // a firmware job does not block ROM-only guards
        assert!(env.svc.active_download_rom_ids().is_empty());
    }

    #[tokio::test]
    async fn an_interrupted_firmware_job_resumes_after_a_restart_and_a_no_longer_needed_one_is_dropped() {
        let env = DownloadEnv::new().await;
        let bytes: Vec<u8> = (0..3000u32).map(|i| (i % 200) as u8).collect();
        let t = target(&env, "Firmware.23.0.1.zip", &bytes, FirmwareFileKind::Archive, &[]);
        register(&env, &t, &bytes);
        std::fs::create_dir_all(std::path::Path::new(&t.dest_path).parent().unwrap()).unwrap();
        std::fs::write(format!("{}.tmp", t.dest_path), &bytes[..1200]).unwrap();
        let mut j = job(FW, &format!("{}.tmp", t.dest_path), JobStatus::Interrupted, 1, Some(bytes.len() as u64));
        j.kind = JobKind::Firmware;
        j.is_multi = false;
        j.target_path = t.dest_path.clone();
        env.jobs.save(&j).await.unwrap();
        *env.romm.range_supported.lock().unwrap() = true;
        env.svc.reconcile_persisted_downloads().await;
        wait_fw(&env, "completed").await;
        assert_eq!(std::fs::read(&t.dest_path).unwrap(), bytes);

        // A second stopped job whose file is no longer needed (the target vanished) is evicted, not retried forever.
        env.firmware_jobs.targets.lock().unwrap().clear();
        let r = env.svc.resume_firmware_download(FW).await;
        assert_eq!(r["reason"], "not_paused");
    }

    #[tokio::test]
    async fn the_partial_of_a_running_firmware_job_survives_startup_cleanup_but_orphans_do_not() {
        let env = DownloadEnv::new().await;
        let bios = format!("{}/Emulation/bios/switch", env.dir.path().display());
        std::fs::create_dir_all(&bios).unwrap();
        let owned = format!("{bios}/Firmware.23.0.1.zip.tmp");
        let orphan = format!("{bios}/Firmware.9.2.0.zip.tmp");
        std::fs::write(&owned, b"partial").unwrap();
        std::fs::write(&orphan, b"abandoned").unwrap();
        let mut j = job(FW, &owned, JobStatus::Paused, 1, Some(100));
        j.kind = JobKind::Firmware;
        j.is_multi = false;
        env.jobs.save(&j).await.unwrap();
        env.svc.reconcile_persisted_downloads().await;
        assert!(std::path::Path::new(&owned).exists(), "a job's partial under bios/ is kept");
        assert!(!std::path::Path::new(&orphan).exists(), "an orphan is cleaned");
    }
}
