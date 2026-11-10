//! InstallAdoptionService over real SQLite + real files — the Rust twin of
//! `tests/services/test_install_adoption.py`.

use std::os::unix::fs::symlink;
use std::path::Path;

use serde_json::json;

use super::*;
use crate::domain::download_job::{DownloadJob, NewJob};
use crate::domain::errors::RommError;
use crate::services::testing::{DownloadEnv, EDEN, switch_rom};

const PAD: usize = 64;

fn gba(id: i64, fs_name: &str, size: usize) -> Value {
    json!({
        "id": id, "name": format!("Game {id}"), "fs_name": fs_name, "fs_size_bytes": size,
        "platform_slug": "gba", "platform_name": "Game Boy Advance",
        "files": [{"file_name": fs_name, "file_size_bytes": size}],
    })
}

fn place(env: &DownloadEnv, rel: &str, size: usize) -> String {
    let path = env.roms().join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, vec![0u8; size]).unwrap();
    path.to_string_lossy().into_owned()
}

async fn job_for(env: &DownloadEnv, rom_id: i64) {
    let job = DownloadJob::enqueue(
        NewJob {
            kind: crate::domain::download_job::JobKind::Rom,
            item_id: rom_id,
            rom_name: "x".into(),
            platform_name: "x".into(),
            file_name: "x".into(),
            target_path: "/t/x".into(),
            tmp_path: "/t/x.tmp".into(),
            is_multi: false,
            position: 1,
            total_bytes: None,
        },
        "t",
    )
    .unwrap();
    env.jobs.save(&job).await.unwrap();
}

async fn install(env: &DownloadEnv, rom_id: i64, path: &str, rom_dir: Option<&str>, system: &str) {
    let i = RomInstall::mark_installed(
        rom_id,
        path.into(),
        rom_dir.map(str::to_string),
        system.into(),
        system.into(),
        "t".into(),
    )
    .unwrap();
    env.library.save_install(&i).await.unwrap();
}

fn ids(c: &[AdoptionCandidate]) -> Vec<i64> {
    c.iter().map(|c| c.rom_id).collect()
}

#[tokio::test]
async fn exact_single_file_is_planned_like_a_download() {
    let env = DownloadEnv::new().await;
    let path = place(&env, "gba/Pokemon Emerald (USA).gba", PAD);
    let plan = env.adoption.plan_adoptions(&[gba(7, "Pokemon Emerald (USA).gba", PAD)]).await;
    assert_eq!(plan.len(), 1);
    let c = &plan[0];
    assert_eq!(c.install.file_path, path);
    assert_eq!(c.install.rom_dir, None);
    assert_eq!((c.install.system.as_str(), c.install.platform_slug.as_str()), ("gba", "gba"));
    assert_eq!(c.bake_path, path);

    for rom in [
        gba(7, "Pokemon Emerald (USA).gba", PAD + 1),
        gba(7, "Missing.gba", PAD),
        gba(7, "Pokemon Emerald.gba", PAD),
        json!({"id": 7, "fs_name": "Pokemon Emerald (USA).gba", "platform_slug": "gba"}),
    ] {
        assert!(env.adoption.plan_adoptions(&[rom.clone()]).await.is_empty(), "{rom}");
    }
    std::fs::create_dir_all(env.roms().join("gba/Dir.gba")).unwrap();
    assert!(env.adoption.plan_adoptions(&[gba(7, "Dir.gba", 0)]).await.is_empty());
    let mut unsafe_slug = gba(7, "x.gba", PAD);
    unsafe_slug["platform_slug"] = json!("../..");
    assert!(env.adoption.plan_adoptions(&[unsafe_slug]).await.is_empty());
    assert!(env.adoption.plan_adoptions(&[]).await.is_empty());
}

#[tokio::test]
async fn traversal_fs_name_only_checks_the_download_target_and_symlinks_cannot_escape() {
    let env = DownloadEnv::new().await;
    let outside = env.dir.path().join("outside.gba");
    std::fs::write(&outside, vec![0u8; PAD]).unwrap();
    assert!(env.adoption.plan_adoptions(&[gba(7, "../../../outside.gba", PAD)]).await.is_empty());
    std::fs::create_dir_all(env.roms().join("gba")).unwrap();
    symlink(&outside, env.roms().join("gba/link.gba")).unwrap();
    assert!(env.adoption.plan_adoptions(&[gba(7, "link.gba", PAD)]).await.is_empty());
}

#[tokio::test]
async fn guards_block_adoption() {
    let env = DownloadEnv::new().await;
    place(&env, "gba/a.gba", PAD);
    place(&env, "gba/b.gba", PAD);
    env.seed_rom(1, "gba", "a.gba", None, None).await;
    install(&env, 1, "/elsewhere/a.gba", None, "gba").await;
    assert!(env.adoption.plan_adoptions(&[gba(1, "a.gba", PAD)]).await.is_empty());
    job_for(&env, 2).await;
    assert!(env.adoption.plan_adoptions(&[gba(2, "b.gba", PAD)]).await.is_empty());

    // Installed sibling (via the row's key, or a key stamped on the payload).
    env.seed_rom(3, "gba", "c.gba", None, Some("g")).await;
    env.seed_rom(4, "gba", "d.gba", None, Some("g")).await;
    install(&env, 4, "/x/d.gba", None, "gba").await;
    place(&env, "gba/c.gba", PAD);
    assert!(env.adoption.plan_adoptions(&[gba(3, "c.gba", PAD)]).await.is_empty());
    let mut stamped = gba(5, "e.gba", PAD);
    stamped["sibling_group_key"] = json!("g");
    place(&env, "gba/e.gba", PAD);
    assert!(env.adoption.plan_adoptions(&[stamped]).await.is_empty());

    // Two versions of one game on disk: only the lowest id is adopted.
    let mut a = gba(11, "v1.gba", PAD);
    a["sibling_group_key"] = json!("h");
    let mut b = gba(10, "v2.gba", PAD);
    b["sibling_group_key"] = json!("h");
    place(&env, "gba/v1.gba", PAD);
    place(&env, "gba/v2.gba", PAD);
    assert_eq!(ids(&env.adoption.plan_adoptions(&[a, b]).await), [10]);
}

#[tokio::test]
async fn active_download_blocks() {
    let env = DownloadEnv::new().await;
    env.seed_rom(7, "switch", "Zelda.nsp", None, None).await;
    env.romm.set_rom(switch_rom(7, "Zelda.nsp", 100));
    *env.romm.stall_at.lock().unwrap() = Some(10);
    env.romm.set_content(7, vec![1u8; 100]);
    env.svc.start_download(7).await;
    env.romm.stalled.notified().await;
    place(&env, "switch/Zelda.nsp", 100);
    assert!(env.adoption.plan_adoptions(&[switch_rom(7, "Zelda.nsp", 100)]).await.is_empty());
    assert!(!env.adoption.adopt_rom(7).await);
    env.svc.cancel_download(7);
    env.wait_status(7, "cancelled").await;
}

fn psx(files: Vec<Value>) -> Value {
    json!({
        "id": 9, "name": "Final Fantasy VII", "fs_path": "psx", "fs_name": "FF7 (USA)",
        "fs_name_no_ext": "FF7 (USA)", "platform_slug": "psx", "has_multiple_files": true, "files": files,
    })
}

fn f(rel: &str, size: u64) -> Value {
    let full = format!("psx/FF7 (USA)/{rel}");
    let (path, name) = full.rsplit_once('/').unwrap();
    json!({"file_path": path, "file_name": name, "file_size_bytes": size})
}

#[tokio::test]
async fn multi_file_exact_rule() {
    let env = DownloadEnv::new().await;
    place(&env, "psx/FF7 (USA)/FF7.cue", 10);
    place(&env, "psx/FF7 (USA)/sub/FF7.bin", 20);
    let plan = env.adoption.plan_adoptions(&[psx(vec![f("FF7.cue", 10), f("sub/FF7.bin", 20)])]).await;
    assert_eq!(plan.len(), 1);
    let dir = env.roms().join("psx/FF7 (USA)").to_string_lossy().into_owned();
    assert_eq!(plan[0].install.rom_dir.as_deref(), Some(dir.as_str()));
    assert_eq!(plan[0].install.file_path, format!("{dir}/FF7.cue"));
    assert!(
        env.adoption
            .plan_adoptions(&[psx(vec![f("FF7.cue", 10), f("sub/FF7.bin", 21)])])
            .await
            .is_empty()
    );
    assert!(
        env.adoption
            .plan_adoptions(&[psx(vec![f("FF7.cue", 10), f("gone.bin", 1)])])
            .await
            .is_empty()
    );
    // An ES-DE-collapsed folder (named after the launch file) is found too.
    std::fs::rename(env.roms().join("psx/FF7 (USA)"), env.roms().join("psx/FF7.cue")).unwrap();
    assert_eq!(
        env.adoption
            .plan_adoptions(&[psx(vec![f("FF7.cue", 10), f("sub/FF7.bin", 20)])])
            .await
            .len(),
        1
    );
}

#[tokio::test]
async fn commit_rechecks_guards() {
    let env = DownloadEnv::new().await;
    place(&env, "gba/a.gba", PAD);
    place(&env, "gba/b.gba", PAD);
    place(&env, "gba/c.gba", PAD);
    env.seed_rom(1, "gba", "a.gba", None, None).await;
    env.seed_rom(2, "gba", "b.gba", None, None).await;
    let plan = env
        .adoption
        .plan_adoptions(&[gba(1, "a.gba", PAD), gba(2, "b.gba", PAD), gba(3, "c.gba", PAD)])
        .await;
    assert_eq!(ids(&plan), [1, 2, 3]);
    job_for(&env, 2).await; // a download started since planning
    assert_eq!(env.adoption.commit_adoptions(&plan).await, vec![1], "3 has no roms row");
    assert!(env.library.install(1).await.unwrap().is_some());
    assert!(env.adoption.commit_adoptions(&plan[..1]).await.is_empty(), "already installed");
    assert!(env.adoption.commit_adoptions(&[]).await.is_empty());
}

#[tokio::test]
async fn adopt_rom_writes_install_and_emits_download_complete() {
    let env = DownloadEnv::new().await;
    env.seed_rom(7, "gba", "Pokemon Emerald (USA).gba", Some(4242), None).await;
    env.romm.set_rom(gba(7, "Pokemon Emerald (USA).gba", PAD));
    let path = place(&env, "gba/Pokemon Emerald (USA).gba", PAD);
    assert!(env.adoption.adopt_rom(7).await);
    let [event] = env.events.named("download_complete").try_into().unwrap();
    assert_eq!(
        event,
        json!({
            "rom_id": 7, "rom_name": "Game 7", "platform_name": "Game Boy Advance", "file_path": path,
            "app_id": 4242, "launch_options": "", "resumable": false, "adopted": true,
        })
    );
    assert_eq!(env.library.install(7).await.unwrap().unwrap().file_path, path);
    // Installed now: no RomM round-trip on the next check.
    env.romm.get_rom_calls.lock().unwrap().clear();
    assert!(!env.adoption.adopt_rom(7).await);
    assert!(env.romm.get_rom_calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn adopt_rom_negative_paths() {
    let env = DownloadEnv::new().await;
    // Unbound: null app id, empty launch options.
    env.seed_rom(7, "gba", "a.gba", None, None).await;
    env.romm.set_rom(gba(7, "a.gba", PAD));
    place(&env, "gba/a.gba", PAD);
    assert!(env.adoption.adopt_rom(7).await);
    let event = env.events.named("download_complete").pop().unwrap();
    assert_eq!((event["app_id"].clone(), event["launch_options"].clone()), (Value::Null, json!("")));
    // Unsynced, job, sibling, no match, server failure.
    assert!(!env.adoption.adopt_rom(99).await);
    env.seed_rom(8, "gba", "b.gba", None, None).await;
    job_for(&env, 8).await;
    assert!(!env.adoption.adopt_rom(8).await);
    env.seed_rom(9, "gba", "c.gba", None, Some("g")).await;
    env.seed_rom(10, "gba", "d.gba", None, Some("g")).await;
    install(&env, 10, "/x", None, "gba").await;
    assert!(!env.adoption.adopt_rom(9).await);
    env.seed_rom(11, "gba", "e.gba", None, None).await;
    env.romm.set_rom(gba(11, "e.gba", PAD));
    assert!(!env.adoption.adopt_rom(11).await);
    env.seed_rom(12, "gba", "f.gba", None, None).await;
    env.romm.fail_rom(12, RommError::Connection("down".into()));
    assert!(!env.adoption.adopt_rom(12).await);
    env.romm.set_rom(json!({"id": 13}));
    env.seed_rom(14, "gba", "g.gba", None, None).await;
    env.romm.roms.lock().unwrap().insert(14, Ok(json!({"id": 13})));
    assert!(!env.adoption.adopt_rom(14).await, "a payload for another ROM is ignored");
}

#[tokio::test]
async fn schedule_runs_in_background_dedupes_and_cools_down() {
    let env = DownloadEnv::new().await;
    env.seed_rom(7, "gba", "a.gba", Some(1), None).await;
    env.romm.set_rom(gba(7, "a.gba", PAD));
    place(&env, "gba/a.gba", PAD);
    env.adoption.schedule_adoption_check(7);
    env.adoption.schedule_adoption_check(7);
    env.wait_until("adopted", || !env.events.named("download_complete").is_empty()).await;
    env.wait_until("idle", || env.adoption.in_flight.lock().unwrap().is_empty()).await;
    assert_eq!(*env.romm.get_rom_calls.lock().unwrap(), vec![7]);
    // Within the cooldown nothing runs; after it a new check runs.
    env.adoption.schedule_adoption_check(7);
    assert!(env.adoption.in_flight.lock().unwrap().is_empty());
    *env.clock.mono.lock().unwrap() += 60.0;
    env.adoption.schedule_adoption_check(7);
    env.wait_until("idle", || env.adoption.in_flight.lock().unwrap().is_empty()).await;
}

#[tokio::test]
async fn recorded_paths_keep_the_configured_root_spelling() {
    let real = tempfile::tempdir().unwrap();
    let env = DownloadEnv::new().await;
    // Replace the roms root with a symlink to elsewhere (SD-card mount style).
    std::fs::remove_dir_all(env.roms()).unwrap();
    std::fs::create_dir_all(real.path().join("switch")).unwrap();
    symlink(real.path(), env.roms()).unwrap();
    std::fs::write(real.path().join("switch/Zelda.nsp"), vec![0u8; 10]).unwrap();
    let plan = env.adoption.plan_adoptions(&[switch_rom(7, "Zelda.nsp", 10)]).await;
    assert_eq!(plan[0].install.file_path, format!("{}/switch/Zelda.nsp", env.roms_s()));
    std::fs::create_dir_all(real.path().join("switch/Catherine Full Body")).unwrap();
    std::fs::write(real.path().join("switch/Catherine Full Body/C [01].xci"), b"x").unwrap();
    let plan = env.adoption.plan_adoptions(&[switch_rom(8, "Catherine_ Full Body", 10)]).await;
    assert_eq!(
        plan[0].install.rom_dir.as_deref(),
        Some(format!("{}/switch/Catherine Full Body", env.roms_s()).as_str())
    );
}

// ── Normalized-title fallback (Switch) ───────────────────────────────────

const XCI: usize = 32; // stands in for the 7,985,954,816-byte cart padding

/// The real `roms/switch/` of the Deck (Phase 1.5).
fn real_deck_switch_folder(env: &DownloadEnv) {
    place(
        env,
        "switch/Catherine Full Body/Catherine Full Body [0100BF00112C0000]+[v1.0.1](US-EU).xci",
        XCI,
    );
    place(env, "switch/Fire Emblem Three Houses/Fire Emblem Three Houses NSW-HR.xci", XCI);
    place(env, "switch/The Legend Of Zelda Echoes Of Wisdom NSW-NiiNTENDO.xci", XCI);
    place(env, "switch/Mario Kart 8 Deluxe/Mario Kart 8 Deluxe [0100152000022000][v0].xci", XCI);
    place(env, "switch/Mario Kart 8 Deluxe/update.nsp", 4);
    place(env, "switch/Pokemon Scarlet/Pokemon Scarlet NSW-NiiNTENDO.xci", XCI);
    place(env, "switch/metadata.txt", 10);
    std::fs::create_dir_all(env.roms().join("switch/Xenoblade Chronicles 3")).unwrap();
}

/// The RomM library of the Deck (folder games carry sanitized names).
fn real_romm_switch() -> Vec<Value> {
    [
        (1, "Catherine_ Full Body"),
        (2, "Fire Emblem_ Three Houses"),
        (3, "The Legend of Zelda_ Echoes of Wisdom"),
        (4, "Mario Kart 8 Deluxe"),
        (5, "Super Mario Galaxy"),
        (8, "Pokémon Legends_ Z-A"),
    ]
    .iter()
    .map(|(id, name)| switch_rom(*id, name, 999))
    .collect()
}

#[tokio::test]
async fn title_fallback_on_the_real_device_layout() {
    let env = DownloadEnv::new().await;
    real_deck_switch_folder(&env);
    let plan = env.adoption.plan_adoptions(&real_romm_switch()).await;
    assert_eq!(ids(&plan), [1, 2, 3, 4]);
    let sw = format!("{}/switch", env.roms_s());
    let mk = &plan[3].install;
    assert_eq!(mk.rom_dir.as_deref(), Some(format!("{sw}/Mario Kart 8 Deluxe").as_str()));
    assert_eq!(
        mk.file_path,
        format!("{sw}/Mario Kart 8 Deluxe/Mario Kart 8 Deluxe [0100152000022000][v0].xci"),
        "the largest Switch file launches"
    );
    let zelda = &plan[2].install;
    assert_eq!(zelda.rom_dir, None);
    assert_eq!(zelda.file_path, format!("{sw}/The Legend Of Zelda Echoes Of Wisdom NSW-NiiNTENDO.xci"));
}

#[tokio::test]
async fn title_fallback_ambiguity_claims_and_scope() {
    let env = DownloadEnv::new().await;
    place(&env, "switch/Mario Kart 8 Deluxe/MK8.xci", XCI);
    place(&env, "switch/Mario Kart 8 Deluxe NSW-GRP.xci", XCI);
    assert!(env.adoption.plan_adoptions(&[switch_rom(4, "Mario Kart 8 Deluxe", 1)]).await.is_empty());

    let env = DownloadEnv::new().await;
    place(&env, "switch/Mario Kart 8 Deluxe/MK8.xci", XCI);
    let two = [switch_rom(4, "Mario Kart 8 Deluxe", 1), switch_rom(9, "Mario Kart 8_ Deluxe", 1)];
    assert!(env.adoption.plan_adoptions(&two).await.is_empty());
    // A synced (not in this batch) ROM sharing the title makes it ambiguous.
    env.seed_rom(9, "switch", "Mario Kart 8_ Deluxe", None, None).await;
    assert!(env.adoption.plan_adoptions(&[switch_rom(4, "Mario Kart 8 Deluxe", 1)]).await.is_empty());

    // An entry an existing install points at is never adopted again.
    let env = DownloadEnv::new().await;
    let dir = format!("{}/switch/Mario Kart 8 Deluxe", env.roms_s());
    place(&env, "switch/Mario Kart 8 Deluxe/MK8.xci", XCI);
    env.seed_rom(3, "switch", "Other", None, None).await;
    install(&env, 3, &format!("{dir}/MK8.xci"), Some(&dir), "switch").await;
    assert!(env.adoption.plan_adoptions(&[switch_rom(4, "Mario Kart 8 Deluxe", 1)]).await.is_empty());

    // A nested-only launch file does not qualify; other systems never use titles.
    let env = DownloadEnv::new().await;
    place(&env, "switch/Mario Kart 8 Deluxe/update/MK8.xci", XCI);
    assert!(env.adoption.plan_adoptions(&[switch_rom(4, "Mario Kart 8 Deluxe", 1)]).await.is_empty());
    place(&env, "gba/Pokemon Emerald.gba", 5);
    assert!(env.adoption.plan_adoptions(&[gba(5, "Pokemon Emerald (USA).gba", 99)]).await.is_empty());

    // Missing switch folder / unsafe system.
    let env = DownloadEnv::new().await;
    assert!(env.adoption.plan_adoptions(&real_romm_switch()).await.is_empty());
}

#[tokio::test]
async fn exact_rule_wins_and_claims_its_entry() {
    let env = DownloadEnv::new().await;
    place(&env, "switch/Zelda.nsp", 50);
    let exact = switch_rom(1, "Zelda.nsp", 50);
    let title = switch_rom(2, "Zelda", 999);
    let plan = env.adoption.plan_adoptions(&[exact, title]).await;
    assert_eq!(ids(&plan), [1]);
}

#[tokio::test]
async fn game_page_adopts_a_hand_copied_switch_folder_by_title() {
    let env = DownloadEnv::new().await;
    real_deck_switch_folder(&env);
    env.seed_rom(1, "switch", "Catherine_ Full Body", Some(777), None).await;
    env.romm.set_rom(switch_rom(1, "Catherine_ Full Body", 999));
    assert!(env.adoption.adopt_rom(1).await);
    let event = env.events.named("download_complete").pop().unwrap();
    let launch = format!(
        "{}/switch/Catherine Full Body/Catherine Full Body [0100BF00112C0000]+[v1.0.1](US-EU).xci",
        env.roms_s()
    );
    assert_eq!(event["adopted"], true);
    assert_eq!(event["app_id"], 777);
    assert_eq!(event["file_path"], launch);
    assert_eq!(
        event["launch_options"],
        format!("vblank_mode=0 %command% \"{EDEN}\" -f -g \"{launch}\"")
    );
    let installed = env.library.install(1).await.unwrap().unwrap();
    assert_eq!(
        installed.rom_dir.as_deref(),
        Some(format!("{}/switch/Catherine Full Body", env.roms_s()).as_str())
    );
    assert!(Path::new(&launch).exists(), "adoption never touches the files");
}
