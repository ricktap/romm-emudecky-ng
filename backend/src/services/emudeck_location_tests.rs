use std::sync::Arc;

use serde_json::json;

use crate::domain::emudeck_health::EmuDeckHealth;
use crate::domain::rom_install::RomInstall;
use crate::services::emudeck_location::{EmuDeckLocationConfig, EmuDeckLocationService};
use crate::services::ports::LibraryRepo;
use crate::services::testing::{DownloadEnv, FakeKv};

fn service(env: &DownloadEnv, kv: &Arc<FakeKv>) -> Arc<EmuDeckLocationService> {
    // The same fakes the download tests use: GatedPaths (health + storage path), the real library, FakeFiles.
    EmuDeckLocationService::new(EmuDeckLocationConfig {
        paths: Arc::new(crate::services::testing::GatedPaths {
            inner: crate::services::testing::FixedPaths { roms: String::new(), bios: String::new() },
            health: env.health.clone(),
            storage: env.storage.clone(),
        }),
        kv: kv.clone(),
        library: env.library.clone(),
        files: env.files.clone(),
    })
}

async fn install(env: &DownloadEnv, id: i64, path: &str) {
    env.seed_rom(id, "switch", &format!("g{id}.nsp"), None, None).await;
    let rec = RomInstall::mark_installed(id, path.into(), None, "switch".into(), "switch".into(), "2026-01-01T00:00:00Z".into()).unwrap();
    env.library.save_install(&rec).await.unwrap();
}

#[tokio::test]
async fn the_first_healthy_reading_is_remembered_without_a_warning() {
    let env = DownloadEnv::new().await;
    let kv = Arc::new(FakeKv::default());
    let svc = service(&env, &kv);
    let s = svc.status().await;
    assert_eq!(s["health"], "ok");
    assert_eq!(s["moved"], false);
    assert_eq!(kv.rows.lock().unwrap().get("emudeck_storage_path").map(String::as_str), Some("/sd"));
}

#[tokio::test]
async fn a_moved_storage_path_is_reported_with_how_many_installs_are_gone_and_files_are_left_alone() {
    let env = DownloadEnv::new().await;
    let kv = Arc::new(FakeKv::default());
    kv.insert("emudeck_storage_path", "/old-card");
    let here = env.roms().join("switch/b.nsp");
    std::fs::create_dir_all(here.parent().unwrap()).unwrap();
    std::fs::write(&here, b"x").unwrap();
    install(&env, 1, "/old-card/Emulation/roms/switch/a.nsp").await;
    install(&env, 2, &here.to_string_lossy()).await;
    let svc = service(&env, &kv);
    let s = svc.status().await;
    assert_eq!(s["moved"], true);
    assert_eq!(s["accepted_path"], "/old-card");
    assert_eq!(s["storage_path"], "/sd");
    assert_eq!((s["installed"].clone(), s["missing"].clone()), (json!(2), json!(1)));
    assert_eq!(std::fs::read(&here).unwrap(), b"x", "no file was touched");
    // Reporting it again changes nothing until the user accepts.
    assert_eq!(svc.status().await["moved"], true);
}

#[tokio::test]
async fn accepting_records_the_new_location_and_ends_the_warning() {
    let env = DownloadEnv::new().await;
    let kv = Arc::new(FakeKv::default());
    kv.insert("emudeck_storage_path", "/old-card");
    let svc = service(&env, &kv);
    assert_eq!(svc.status().await["moved"], true);
    let r = svc.accept().await;
    assert_eq!(r["success"], true);
    assert_eq!(r["storage_path"], "/sd");
    assert_eq!(svc.status().await["moved"], false);
}

#[tokio::test]
async fn nothing_is_reported_or_accepted_while_emudeck_is_unhealthy() {
    let env = DownloadEnv::new().await;
    let kv = Arc::new(FakeKv::default());
    kv.insert("emudeck_storage_path", "/old-card");
    let svc = service(&env, &kv);
    *env.health.lock().unwrap() = EmuDeckHealth::RootMissing;
    let s = svc.status().await;
    assert_eq!(s["health"], "root_missing");
    assert_eq!(s["moved"], false, "an ejected card is not a moved location");
    let r = svc.accept().await;
    assert_eq!(r["success"], false);
    assert_eq!(r["reason"], "emudeck_unavailable");
    assert_eq!(kv.rows.lock().unwrap().get("emudeck_storage_path").map(String::as_str), Some("/old-card"));
    // No storage path known at all: also refused.
    *env.health.lock().unwrap() = EmuDeckHealth::Ok;
    *env.storage.lock().unwrap() = None;
    assert_eq!(svc.accept().await["reason"], "emudeck_unavailable");
}

#[tokio::test]
async fn a_failing_store_never_breaks_the_health_answer() {
    let env = DownloadEnv::new().await;
    let kv = Arc::new(FakeKv::default());
    kv.fail();
    let svc = service(&env, &kv);
    let s = svc.status().await;
    assert_eq!(s["health"], "ok");
    assert_eq!(s["moved"], false);
    assert_eq!(svc.accept().await["success"], false);
}
