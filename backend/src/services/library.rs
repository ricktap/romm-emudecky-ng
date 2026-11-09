//! Library metadata, preferences, and offline registry reads.
//! Sync lifecycle callables belong to the orchestrator (not yet ported).

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{Map, Value, json};

use crate::domain::errors::{failure, unknown_failure};
use crate::domain::platform_prefs::{materialize_enabled_platforms, resolve_sync_enabled};
use crate::domain::settings::truthy;
use crate::services::ports::{Clock, KvConfigRepo, LibraryRepo, RommLibraryApi, SettingsPersister, SettingsStore};

const PLATFORM_NAMES_KEY: &str = "platform_names";

pub struct LibraryServiceConfig {
    pub romm: Arc<dyn RommLibraryApi>,
    pub settings: SettingsStore,
    pub persister: Arc<dyn SettingsPersister>,
    pub library: Arc<dyn LibraryRepo>,
    pub kv_config: Arc<dyn KvConfigRepo>,
    pub clock: Arc<dyn Clock>,
}

pub struct LibraryService {
    romm: Arc<dyn RommLibraryApi>,
    settings: SettingsStore,
    persister: Arc<dyn SettingsPersister>,
    library: Arc<dyn LibraryRepo>,
    kv_config: Arc<dyn KvConfigRepo>,
    clock: Arc<dyn Clock>,
}

impl LibraryService {
    pub fn new(config: LibraryServiceConfig) -> Self {
        Self {
            romm: config.romm,
            settings: config.settings,
            persister: config.persister,
            library: config.library,
            kv_config: config.kv_config,
            clock: config.clock,
        }
    }

    /// Before accepting any sync calls, finish records left running by a crash.
    /// Completed history remains the baseline for stats and incremental sync.
    pub async fn heal_interrupted_runs(&self) -> anyhow::Result<()> {
        self.library.interrupt_running_syncs(&self.clock.now_iso()).await?;
        Ok(())
    }

    pub async fn get_platforms(&self) -> Value {
        let platforms = match self.romm.list_platforms().await {
            Ok(Value::Array(platforms)) => platforms,
            Ok(other) => {
                tracing::error!("Unexpected platforms response type: {}", kind(&other));
                return failure("server_unreachable", "Invalid server response");
            }
            Err(err) => return err.to_failure(),
        };

        let shown_ids: Vec<String> = platforms
            .iter()
            .filter(|p| int_field(p, "rom_count").unwrap_or(0) > 0)
            .filter_map(|p| p.get("id").map(json_id))
            .collect();

        let mut enabled = self.enabled_platforms();
        if let Some(materialized) = materialize_enabled_platforms(&enabled, &shown_ids) {
            self.settings
                .set("enabled_platforms", Value::Object(materialized.clone()));
            if let Err(err) = self.persister.save_settings().await {
                tracing::error!("Failed to persist materialized platform prefs: {err:#}");
                return unknown_failure(&err);
            }
            enabled = materialized;
        }

        let result: Vec<Value> = platforms
            .iter()
            .filter(|p| int_field(p, "rom_count").unwrap_or(0) > 0)
            .filter_map(|p| {
                let pid = p.get("id")?;
                let pid_s = json_id(pid);
                Some(json!({
                    "id": pid.clone(),
                    "name": p.get("name").and_then(Value::as_str).unwrap_or(""),
                    "slug": p.get("slug").and_then(Value::as_str).unwrap_or(""),
                    "rom_count": int_field(p, "rom_count").unwrap_or(0),
                    "sync_enabled": resolve_sync_enabled(&enabled, &pid_s),
                }))
            })
            .collect();
        json!({"success": true, "platforms": result})
    }

    pub async fn save_platform_sync(&self, platform_id: &Value, enabled: &Value) -> anyhow::Result<Value> {
        let pid = json_id(platform_id);
        self.settings.with(|s| {
            let entry = s
                .entry("enabled_platforms")
                .or_insert_with(|| Value::Object(Map::new()));
            if !entry.is_object() {
                *entry = Value::Object(Map::new());
            }
            entry.as_object_mut().unwrap().insert(pid, json!(truthy(enabled)));
        });
        self.persist_ok().await
    }

    pub async fn set_all_platforms_sync(&self, enabled: &Value) -> Value {
        let platforms = match self.romm.list_platforms().await {
            Ok(Value::Array(platforms)) => platforms,
            Ok(_) => return failure("server_unreachable", "Invalid server response"),
            Err(err) => return err.to_failure(),
        };
        let enabled = truthy(enabled);
        let map: Map<String, Value> = platforms
            .iter()
            .filter_map(|p| p.get("id").map(|id| (json_id(id), json!(enabled))))
            .collect();
        self.settings.set("enabled_platforms", Value::Object(map));
        match self.persister.save_settings().await {
            Ok(()) => json!({"success": true}),
            Err(err) => unknown_failure(&err),
        }
    }

    pub async fn get_collections(&self) -> Value {
        let user = match self.romm.list_collections().await {
            Ok(Value::Array(v)) => v,
            Ok(_) => return failure("server_unreachable", "Invalid server response"),
            Err(err) => return err.to_failure(),
        };
        let smart = match self.romm.list_smart_collections().await {
            Ok(Value::Array(v)) => v,
            Ok(_) | Err(_) => Vec::new(),
        };
        let franchise = match self.romm.list_virtual_collections("franchise").await {
            Ok(Value::Array(v)) => v,
            Ok(_) | Err(_) => Vec::new(),
        };

        let enabled = self.enabled_collection_buckets();
        let mut result = Vec::new();
        push_collections(&mut result, &user, "user", &enabled);
        push_collections(&mut result, &smart, "smart", &enabled);
        push_collections(&mut result, &franchise, "franchise", &enabled);
        result.sort_by(|a, b| {
            let ak = collection_sort_key(a);
            let bk = collection_sort_key(b);
            ak.cmp(&bk)
        });
        json!({"success": true, "collections": result})
    }

    pub async fn save_collection_sync(
        &self,
        collection_id: &Value,
        kind: &Value,
        enabled: &Value,
    ) -> anyhow::Result<Value> {
        let Some(kind) = kind.as_str().filter(|k| matches!(*k, "user" | "smart" | "franchise")) else {
            return Ok(failure(
                "invalid_kind",
                &format!("Invalid collection kind: {}", kind.as_str().unwrap_or("")),
            ));
        };
        let cid = json_id(collection_id);
        let enabled = truthy(enabled);
        self.settings.with(|s| {
            let mut buckets = enabled_collection_buckets_from(s.get("enabled_collections"));
            buckets
                .get_mut(kind)
                .expect("known collection bucket")
                .insert(cid, json!(enabled));
            s.insert("enabled_collections".into(), buckets_to_value(buckets));
        });
        self.persist_ok().await
    }

    pub async fn set_all_collections_sync(&self, enabled: &Value, scope: Option<&Value>) -> Value {
        if scope.is_some_and(|v| !v.is_null() && !v.is_string()) {
            return failure("invalid_scope", &format!("Invalid scope: {}", scope.unwrap()));
        }
        let scope = scope.and_then(Value::as_str);
        if !matches!(scope, None | Some("my" | "smart" | "franchise")) {
            return failure("invalid_scope", &format!("Invalid scope: {}", scope.unwrap_or("")));
        }
        let enabled_bool = truthy(enabled);
        // Fetch first, then merge into the live preferences. A single-collection
        // toggle can arrive while these requests are in flight; replacing a
        // pre-fetch snapshot would erase changes outside this bulk operation.
        let mut buckets = enabled_collection_buckets_from(None);

        if matches!(scope, None | Some("my")) {
            let user = match self.romm.list_collections().await {
                Ok(Value::Array(v)) => v,
                Ok(_) => return failure("server_unreachable", "Invalid server response"),
                Err(err) => return err.to_failure(),
            };
            for c in user {
                if scope == Some("my") && c.get("is_favorite").is_some_and(truthy) {
                    continue;
                }
                if let Some(id) = c.get("id") {
                    buckets
                        .get_mut("user")
                        .unwrap()
                        .insert(json_id(id), json!(enabled_bool));
                }
            }
        }
        if matches!(scope, None | Some("smart")) {
            match self.romm.list_smart_collections().await {
                Ok(Value::Array(v)) => {
                    for c in v {
                        if let Some(id) = c.get("id") {
                            buckets
                                .get_mut("smart")
                                .unwrap()
                                .insert(json_id(id), json!(enabled_bool));
                        }
                    }
                }
                Err(err) if scope == Some("smart") => return err.to_failure(),
                Ok(_) if scope == Some("smart") => return failure("server_unreachable", "Invalid server response"),
                _ => {}
            }
        }
        if matches!(scope, None | Some("franchise")) {
            match self.romm.list_virtual_collections("franchise").await {
                Ok(Value::Array(v)) => {
                    for c in v {
                        if let Some(id) = c.get("id") {
                            buckets
                                .get_mut("franchise")
                                .unwrap()
                                .insert(json_id(id), json!(enabled_bool));
                        }
                    }
                }
                Err(err) if scope == Some("franchise") => return err.to_failure(),
                Ok(_) if scope == Some("franchise") => return failure("server_unreachable", "Invalid server response"),
                _ => {}
            }
        }
        self.settings.with(|s| {
            let mut current = enabled_collection_buckets_from(s.get("enabled_collections"));
            for (kind, updates) in buckets {
                current.get_mut(&kind).unwrap().extend(updates);
            }
            s.insert("enabled_collections".into(), buckets_to_value(current));
        });
        match self.persister.save_settings().await {
            Ok(()) => json!({"success": true}),
            Err(err) => unknown_failure(&err),
        }
    }

    pub async fn get_registry_platforms(&self) -> anyhow::Result<Value> {
        let names = self.platform_names().await?;
        let mut platforms = Vec::new();
        for row in self.library.registry_platforms().await? {
            let name = names
                .get(&row.platform_slug)
                .cloned()
                .unwrap_or_else(|| row.platform_slug.clone());
            platforms.push(json!({"name": name, "slug": row.platform_slug, "count": row.count}));
        }
        platforms.sort_by(|a, b| a["name"].as_str().unwrap_or("").cmp(b["name"].as_str().unwrap_or("")));
        Ok(json!({"platforms": platforms}))
    }

    pub async fn get_sync_stats(&self) -> anyhow::Result<Value> {
        let enabled_platforms = self.enabled_platforms();
        let platform_count = enabled_platforms.values().filter(|v| truthy(v)).count();
        let enabled_collections = self.enabled_collection_buckets();
        let collection_count = enabled_collections
            .values()
            .flat_map(|bucket| bucket.values())
            .filter(|v| truthy(v))
            .count();
        let last_sync = self.library.latest_completed_sync_finished_at().await?;
        let rom_count = self.library.bound_rom_count().await?;
        Ok(json!({
            "last_sync": last_sync,
            "platforms": platform_count,
            "collections": collection_count,
            "roms": rom_count,
            "total_shortcuts": rom_count,
        }))
    }

    pub async fn get_app_id_rom_id_map(&self) -> anyhow::Result<Value> {
        let mut map = Map::new();
        for (app_id, rom_id) in self.library.app_id_rom_id_map().await? {
            map.insert(app_id.to_string(), json!(rom_id));
        }
        Ok(Value::Object(map))
    }

    pub async fn clear_sync_cache(&self) -> anyhow::Result<Value> {
        self.library.delete_completed_sync_runs().await?;
        Ok(json!({"success": true, "message": "Next sync will do a full fetch"}))
    }

    fn enabled_platforms(&self) -> Map<String, Value> {
        self.settings
            .get("enabled_platforms")
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default()
    }

    fn enabled_collection_buckets(&self) -> BTreeMap<String, Map<String, Value>> {
        let snapshot = self.settings.snapshot();
        enabled_collection_buckets_from(snapshot.get("enabled_collections"))
    }

    async fn platform_names(&self) -> anyhow::Result<BTreeMap<String, String>> {
        let Some(raw) = self.kv_config.get(PLATFORM_NAMES_KEY).await? else {
            return Ok(BTreeMap::new());
        };
        let parsed: serde_json::Result<Map<String, Value>> = serde_json::from_str(&raw);
        Ok(parsed
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(k, v)| v.as_str().map(|s| (k, s.to_string())))
            .collect())
    }

    async fn persist_ok(&self) -> anyhow::Result<Value> {
        self.persister.save_settings().await?;
        Ok(json!({"success": true}))
    }
}

fn enabled_collection_buckets_from(raw: Option<&Value>) -> BTreeMap<String, Map<String, Value>> {
    let mut buckets = BTreeMap::from([
        ("franchise".to_string(), Map::new()),
        ("smart".to_string(), Map::new()),
        ("user".to_string(), Map::new()),
    ]);
    let Some(raw) = raw.and_then(Value::as_object) else {
        return buckets;
    };
    for kind in ["user", "smart", "franchise"] {
        if let Some(bucket) = raw.get(kind).and_then(Value::as_object) {
            buckets.insert(kind.to_string(), bucket.clone());
        }
    }
    buckets
}

fn buckets_to_value(buckets: BTreeMap<String, Map<String, Value>>) -> Value {
    let mut out = Map::new();
    for kind in ["user", "smart", "franchise"] {
        out.insert(
            kind.to_string(),
            Value::Object(buckets.get(kind).cloned().unwrap_or_default()),
        );
    }
    Value::Object(out)
}

fn push_collections(
    result: &mut Vec<Value>,
    collections: &[Value],
    kind: &str,
    enabled: &BTreeMap<String, Map<String, Value>>,
) {
    for c in collections {
        let Some(id) = c.get("id") else {
            continue;
        };
        let cid = json_id(id);
        let is_enabled = enabled
            .get(kind)
            .and_then(|bucket| bucket.get(&cid))
            .is_some_and(truthy);
        result.push(json!({
            "id": cid,
            "name": c.get("name").and_then(Value::as_str).unwrap_or(""),
            "rom_count": int_field(c, "rom_count")
                .unwrap_or_else(|| c.get("rom_ids").and_then(Value::as_array).map_or(0, Vec::len) as i64),
            "sync_enabled": is_enabled,
            "kind": kind,
            "is_favorite": if kind == "user" { c.get("is_favorite").is_some_and(truthy) } else { false },
        }));
    }
}

fn collection_sort_key(v: &Value) -> (u8, String) {
    let order = match v.get("kind").and_then(Value::as_str).unwrap_or("") {
        "user" => 0,
        "smart" => 1,
        "franchise" => 2,
        _ => 99,
    };
    let name = v.get("name").and_then(Value::as_str).unwrap_or("").to_lowercase();
    (order, name)
}

fn int_field(v: &Value, field: &str) -> Option<i64> {
    v.get(field).and_then(|n| {
        n.as_i64()
            .or_else(|| n.as_u64().and_then(|u| i64::try_from(u).ok()))
            .or_else(|| n.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64))
    })
}

fn json_id(v: &Value) -> String {
    v.as_str()
        .map(str::to_string)
        .or_else(|| v.as_i64().map(|n| n.to_string()))
        .or_else(|| v.as_u64().map(|n| n.to_string()))
        .unwrap_or_else(|| v.to_string())
}

fn kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::Map;
    use std::sync::Mutex;

    use crate::domain::errors::RommError;
    use crate::domain::rom_install::RomInstall;
    use crate::services::ports::{RegistryPlatformRow, RomRow};
    use crate::services::testing::{FakeKv, FakePersister, FakeRomm};

    #[derive(Default)]
    struct FakeLibrary {
        bound_count: Mutex<i64>,
        registry: Mutex<Vec<RegistryPlatformRow>>,
        app_map: Mutex<Vec<(i64, i64)>>,
        last_sync: Mutex<Option<String>>,
    }

    #[async_trait]
    impl LibraryRepo for FakeLibrary {
        async fn rom(&self, _rom_id: i64) -> anyhow::Result<Option<RomRow>> {
            Ok(None)
        }
        async fn roms_in_group(&self, _group_key: &str) -> anyhow::Result<Vec<RomRow>> {
            Ok(vec![])
        }
        async fn roms_on_platform(&self, _platform_slug: &str) -> anyhow::Result<Vec<RomRow>> {
            Ok(vec![])
        }
        async fn install(&self, _rom_id: i64) -> anyhow::Result<Option<RomInstall>> {
            Ok(None)
        }
        async fn installs(&self) -> anyhow::Result<Vec<RomInstall>> {
            Ok(vec![])
        }
        async fn save_install(&self, _install: &RomInstall) -> anyhow::Result<()> {
            Ok(())
        }
        async fn delete_install(&self, _rom_id: i64) -> anyhow::Result<()> {
            Ok(())
        }
        async fn adopt_install(&self, _install: &RomInstall, _group_key: Option<&str>) -> anyhow::Result<bool> {
            Ok(false)
        }
        async fn bound_rom_count(&self) -> anyhow::Result<i64> {
            Ok(*self.bound_count.lock().unwrap())
        }
        async fn registry_platforms(&self) -> anyhow::Result<Vec<RegistryPlatformRow>> {
            Ok(self.registry.lock().unwrap().clone())
        }
        async fn app_id_rom_id_map(&self) -> anyhow::Result<Vec<(i64, i64)>> {
            Ok(self.app_map.lock().unwrap().clone())
        }
        async fn latest_completed_sync_finished_at(&self) -> anyhow::Result<Option<String>> {
            Ok(self.last_sync.lock().unwrap().clone())
        }
        async fn delete_completed_sync_runs(&self) -> anyhow::Result<()> {
            *self.last_sync.lock().unwrap() = None;
            Ok(())
        }
        async fn interrupt_running_syncs(&self, _finished_at: &str) -> anyhow::Result<usize> {
            Ok(0)
        }
    }

    struct Fixture {
        settings: SettingsStore,
        persister: Arc<FakePersister>,
        romm: Arc<FakeRomm>,
        library: Arc<FakeLibrary>,
        kv: Arc<FakeKv>,
        service: LibraryService,
    }

    fn fixture() -> Fixture {
        let settings = SettingsStore::new(Map::new());
        let persister = Arc::new(FakePersister::default());
        let romm = Arc::new(FakeRomm::new(settings.clone()));
        let library = Arc::new(FakeLibrary::default());
        let kv = Arc::new(FakeKv::default());
        let service = LibraryService::new(LibraryServiceConfig {
            romm: romm.clone(),
            settings: settings.clone(),
            persister: persister.clone(),
            library: library.clone(),
            kv_config: kv.clone(),
            clock: Arc::new(crate::services::testing::FakeClock::default()),
        });
        Fixture {
            settings,
            persister,
            romm,
            library,
            kv,
            service,
        }
    }

    #[tokio::test]
    async fn platforms_materialize_empty_enabled_map() {
        let f = fixture();
        FakeRomm::set(
            &f.romm.list_platforms,
            Ok(json!([
                {"id": 1, "name": "Switch", "slug": "switch", "rom_count": 4},
                {"id": 2, "name": "Empty", "slug": "empty", "rom_count": 0},
            ])),
        );

        let out = f.service.get_platforms().await;

        assert_eq!(out["success"], true);
        assert_eq!(out["platforms"].as_array().unwrap().len(), 1);
        assert_eq!(out["platforms"][0]["sync_enabled"], true);
        assert_eq!(f.settings.get("enabled_platforms"), Some(json!({"1": true})));
        assert_eq!(f.persister.saves(), 1);
    }

    #[tokio::test]
    async fn one_platform_toggle_updates_existing_map() {
        let f = fixture();
        f.settings.set("enabled_platforms", json!({"1": true, "2": true}));

        let out = f.service.save_platform_sync(&json!(2), &json!(false)).await.unwrap();

        assert_eq!(out, json!({"success": true}));
        assert_eq!(
            f.settings.get("enabled_platforms"),
            Some(json!({"1": true, "2": false}))
        );
    }

    #[tokio::test]
    async fn collection_read_merges_three_kinds() {
        let f = fixture();
        f.settings.set(
            "enabled_collections",
            json!({"user": {"7": true}, "smart": {"8": true}, "franchise": {"zelda": true}}),
        );
        FakeRomm::set(
            &f.romm.list_collections,
            Ok(json!([{"id": 7, "name": "Favorites", "rom_count": 2, "is_favorite": true}])),
        );
        FakeRomm::set(
            &f.romm.list_smart_collections,
            Ok(json!([{"id": 8, "name": "Recent", "rom_ids": [1, 2, 3]}])),
        );
        FakeRomm::set(
            &f.romm.list_virtual_collections,
            Ok(json!([{"id": "zelda", "name": "Zelda", "rom_count": 9}])),
        );

        let out = f.service.get_collections().await;

        let rows = out["collections"].as_array().unwrap();
        assert_eq!(
            rows.iter().map(|r| r["kind"].as_str().unwrap()).collect::<Vec<_>>(),
            vec!["user", "smart", "franchise"]
        );
        assert!(rows.iter().all(|r| r["sync_enabled"] == true));
        assert_eq!(rows[1]["rom_count"], 3);
    }

    #[tokio::test]
    async fn stats_and_registry_are_offline_reads() {
        let f = fixture();
        f.settings.set(
            "enabled_collections",
            json!({"user": {"7": true}, "smart": {}, "franchise": {"x": false}}),
        );
        f.settings
            .set("enabled_platforms", json!({"1": true, "2": false, "3": true}));
        *f.library.bound_count.lock().unwrap() = 2;
        *f.library.last_sync.lock().unwrap() = Some("2026-01-02T03:04:05Z".into());
        *f.library.registry.lock().unwrap() = vec![RegistryPlatformRow {
            platform_slug: "switch".into(),
            count: 2,
        }];
        f.kv.insert("platform_names", r#"{"switch":"Nintendo Switch"}"#);

        let stats = f.service.get_sync_stats().await.unwrap();
        assert_eq!(stats["platforms"], 2);
        assert_eq!(stats["collections"], 1);
        assert_eq!(stats["roms"], 2);
        assert_eq!(stats["last_sync"], "2026-01-02T03:04:05Z");

        let registry = f.service.get_registry_platforms().await.unwrap();
        assert_eq!(
            registry,
            json!({"platforms": [{"name": "Nintendo Switch", "slug": "switch", "count": 2}]})
        );
    }

    #[tokio::test]
    async fn romm_failures_use_canonical_shape() {
        let f = fixture();
        FakeRomm::set(&f.romm.list_platforms, Err(RommError::Connection("offline".into())));

        let out = f.service.get_platforms().await;

        assert_eq!(out["success"], false);
        assert_eq!(out["reason"], "server_unreachable");
        assert!(out.get("error").is_none());
    }

    #[tokio::test]
    async fn empty_platforms_do_not_materialize_or_save() {
        let f = fixture();
        FakeRomm::set(&f.romm.list_platforms, Ok(json!([
            {"id": 1, "rom_count": 0}, {"id": 2},
        ])));
        assert_eq!(f.service.get_platforms().await, json!({"success": true, "platforms": []}));
        assert_eq!(f.persister.saves(), 0);
        assert_eq!(f.settings.get("enabled_platforms"), None);
    }

    #[tokio::test]
    async fn platform_materialization_is_idempotent_and_absent_ids_default_off() {
        let f = fixture();
        FakeRomm::set(&f.romm.list_platforms, Ok(json!([{ "id": 1, "rom_count": 2 }])));
        f.service.get_platforms().await;
        f.service.get_platforms().await;
        assert_eq!(f.persister.saves(), 1);
        FakeRomm::set(&f.romm.list_platforms, Ok(json!([{ "id": 2, "rom_count": 3 }])));
        assert_eq!(f.service.get_platforms().await["platforms"][0]["sync_enabled"], false);
        assert_eq!(f.settings.get("enabled_platforms"), Some(json!({"1": true})));
    }

    #[tokio::test]
    async fn collection_preferences_repair_bad_buckets_and_isolate_kinds() {
        for raw in [json!(null), json!([]), json!({"user": [], "smart": false})] {
            let f = fixture();
            f.settings.set("enabled_collections", raw);
            f.service.save_collection_sync(&json!(7), &json!("user"), &json!(1)).await.unwrap();
            f.service.save_collection_sync(&json!(7), &json!("smart"), &json!(false)).await.unwrap();
            assert_eq!(f.settings.get("enabled_collections"), Some(json!({
                "user": {"7": true}, "smart": {"7": false}, "franchise": {},
            })));
            assert_eq!(f.persister.saves(), 2);
        }
    }

    #[tokio::test]
    async fn persistence_errors_never_report_success() {
        let f = fixture();
        f.persister.fail_with("disk full");
        FakeRomm::set(&f.romm.list_platforms, Ok(json!([{ "id": 1, "rom_count": 2 }])));
        for out in [
            f.service.get_platforms().await,
            f.service.set_all_platforms_sync(&json!(true)).await,
            f.service.set_all_collections_sync(&json!(true), None).await,
        ] {
            assert_eq!(out, json!({"success": false, "reason": "unknown", "message": "disk full"}));
        }
        assert!(f.service.save_platform_sync(&json!(1), &json!(true)).await.is_err());
        assert!(f.service.save_collection_sync(&json!(1), &json!("user"), &json!(true)).await.is_err());
        assert_eq!(f.persister.saves(), 0);
    }

    #[tokio::test]
    async fn required_collection_failures_preserve_preferences() {
        for response in [Err(RommError::Connection("offline".into())), Ok(json!({}))] {
            let f = fixture();
            f.settings.set("enabled_collections", json!({"user": {"1": true}}));
            let before = f.settings.snapshot();
            FakeRomm::set(&f.romm.list_collections, response);
            assert_eq!(f.service.get_collections().await["success"], false);
            assert_eq!(f.service.set_all_collections_sync(&json!(false), None).await["success"], false);
            assert_eq!(f.settings.snapshot(), before);
            assert_eq!(f.persister.saves(), 0);
        }
    }

    #[tokio::test]
    async fn optional_collection_failures_keep_saved_choices_during_bulk_update() {
        let f = fixture();
        f.settings.set("enabled_collections", json!({
            "user": {"old": true}, "smart": {"3": true}, "franchise": {"series": true},
        }));
        FakeRomm::set(&f.romm.list_collections, Ok(json!([{ "id": 1 }])));
        FakeRomm::set(&f.romm.list_smart_collections, Err(RommError::Connection("offline".into())));
        FakeRomm::set(&f.romm.list_virtual_collections, Err(RommError::Connection("offline".into())));
        assert_eq!(f.service.set_all_collections_sync(&json!(false), None).await, json!({"success": true}));
        assert_eq!(f.settings.get("enabled_collections"), Some(json!({
            "user": {"old": true, "1": false}, "smart": {"3": true}, "franchise": {"series": true},
        })));
    }

    #[tokio::test]
    async fn malformed_optional_collection_lists_fail_only_for_explicit_scope() {
        let f = fixture();
        FakeRomm::set(&f.romm.list_smart_collections, Ok(json!({})));
        FakeRomm::set(&f.romm.list_virtual_collections, Ok(json!(null)));
        assert_eq!(f.service.get_collections().await, json!({"success": true, "collections": []}));
        for scope in ["smart", "franchise"] {
            assert_eq!(f.service.set_all_collections_sync(&json!(true), Some(&json!(scope))).await["success"], false);
        }
        assert_eq!(f.persister.saves(), 0);
        assert_eq!(f.service.set_all_collections_sync(&json!(true), None).await, json!({"success": true}));
    }

    #[tokio::test]
    async fn registry_names_fall_back_sort_and_database_errors_propagate() {
        let f = fixture();
        *f.library.registry.lock().unwrap() = vec![
            RegistryPlatformRow { platform_slug: "a".into(), count: 1 },
            RegistryPlatformRow { platform_slug: "b".into(), count: 2 },
        ];
        assert_eq!(f.service.get_registry_platforms().await.unwrap()["platforms"][0]["name"], "a");
        f.kv.insert("platform_names", r#"{"a": "Z", "b": "A"}"#);
        assert_eq!(f.service.get_registry_platforms().await.unwrap()["platforms"][0]["slug"], "b");
        f.kv.fail();
        assert!(f.service.get_registry_platforms().await.is_err());
    }

    struct PausedRomm {
        inner: Arc<FakeRomm>,
        started: tokio::sync::Notify,
        resume: tokio::sync::Notify,
    }

    #[async_trait]
    impl RommLibraryApi for PausedRomm {
        async fn list_platforms(&self) -> Result<Value, RommError> {
            RommLibraryApi::list_platforms(self.inner.as_ref()).await
        }
        async fn list_collections(&self) -> Result<Value, RommError> {
            self.started.notify_one();
            self.resume.notified().await;
            self.inner.list_collections().await
        }
        async fn list_smart_collections(&self) -> Result<Value, RommError> {
            self.inner.list_smart_collections().await
        }
        async fn list_virtual_collections(&self, kind: &str) -> Result<Value, RommError> {
            self.inner.list_virtual_collections(kind).await
        }
    }

    #[tokio::test]
    async fn bulk_collection_fetch_preserves_concurrent_toggles_outside_its_scope() {
        let mut f = fixture();
        FakeRomm::set(&f.romm.list_collections, Ok(json!([
            {"id": 1}, {"id": 2, "is_favorite": true},
        ])));
        let paused = Arc::new(PausedRomm {
            inner: f.romm.clone(), started: Default::default(), resume: Default::default(),
        });
        f.service.romm = paused.clone();
        let bulk = async { f.service.set_all_collections_sync(&json!(false), Some(&json!("my"))).await };
        let toggle = async {
            paused.started.notified().await;
            f.service.save_collection_sync(&json!(2), &json!("user"), &json!(true)).await.unwrap();
            f.service.save_collection_sync(&json!(3), &json!("smart"), &json!(true)).await.unwrap();
            paused.resume.notify_one();
        };
        let (out, ()) = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            tokio::join!(bulk, toggle)
        }).await.unwrap();
        assert_eq!(out, json!({"success": true}));
        assert_eq!(f.settings.get("enabled_collections"), Some(json!({
            "user": {"1": false, "2": true}, "smart": {"3": true}, "franchise": {},
        })));
    }
}
