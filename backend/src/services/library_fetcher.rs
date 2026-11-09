//! Read-only work planning and full pagination. No fetched rows are committed here.
use std::{collections::HashSet, sync::Arc};
use serde_json::{Value, json};
use crate::domain::{errors::RommError, platform_prefs::resolve_sync_enabled, settings::truthy, work_unit::{CollectionKind, WorkUnit}};
use crate::services::{library_state::LibraryRunState, ports::{RommLibraryApi, RommLibraryPages, SettingsStore}};

#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    #[error("Sync cancelled")]
    Cancelled,
    #[error(transparent)]
    Server(#[from] RommError),
    #[error("{0}")]
    Persistence(String),
    #[error("Invalid server response")]
    InvalidResponse,
}

pub struct LibraryFetcher {
    pub metadata: Arc<dyn RommLibraryApi>,
    pub pages: Arc<dyn RommLibraryPages>,
    pub settings: SettingsStore,
    pub state: Arc<LibraryRunState>,
}

impl LibraryFetcher {
    pub async fn build_work_queue(&self) -> Result<Vec<WorkUnit>, FetchError> {
        let platforms = self.metadata.list_platforms().await?;
        let prefs = self.settings.get("enabled_platforms").and_then(|v| v.as_object().cloned()).unwrap_or_default();
        let mut units = vec![];
        if let Some(platforms) = platforms.as_array() {
            for p in platforms {
                let id = p["id"].as_i64().ok_or(FetchError::InvalidResponse)?;
                if resolve_sync_enabled(&prefs, &id.to_string()) {
                    units.push(WorkUnit::Platform { id, name: text(p, "name", text(p, "display_name", "Unknown")).into(),
                        slug: text(p, "slug", "").into(), rom_count: p["rom_count"].as_i64().unwrap_or(0) });
                }
            }
        }
        let buckets = self.settings.get("enabled_collections").unwrap_or(json!({}));
        for (key, kind) in [("user", CollectionKind::User), ("smart", CollectionKind::Smart), ("franchise", CollectionKind::Franchise)] {
            let enabled: HashSet<String> = buckets[key].as_object().into_iter().flat_map(|b| b.iter())
                .filter(|(_, v)| truthy(v)).map(|(id, _)| id.clone()).collect();
            if enabled.is_empty() { continue; }
            let response = match kind {
                CollectionKind::User => self.metadata.list_collections().await,
                CollectionKind::Smart => self.metadata.list_smart_collections().await,
                CollectionKind::Franchise => self.metadata.list_virtual_collections("franchise").await,
            };
            // Optional collection listing failure must not suppress platform work.
            let Ok(Value::Array(collections)) = response else { continue; };
            for c in collections {
                let id = c["id"].as_str().map(str::to_string).unwrap_or_else(|| c["id"].to_string());
                if enabled.contains(&id) {
                    units.push(WorkUnit::Collection { name: text(&c, "name", &id).into(), id,
                        slug: text(&c, "slug", "").into(),
                        rom_count: c["rom_count"].as_i64().unwrap_or_else(|| c["rom_ids"].as_array().map_or(0, |v| v.len() as i64)),
                        collection_kind: kind });
                }
            }
        }
        Ok(units)
    }

    /// Skip a platform only when every persisted sibling is backfilled, the
    /// server count matches ALL rows, and the delta endpoint confirms no changes.
    pub async fn fetch_platform_unit(
        &self, unit: &WorkUnit,
        baseline_repo: &dyn crate::services::ports::LibraryBaselineRepo,
        delta: &dyn crate::services::ports::RommLibraryDelta,
    ) -> Result<(Vec<Value>, bool), FetchError> {
        let WorkUnit::Platform { id, name, slug, rom_count } = unit else { return Err(FetchError::InvalidResponse); };
        if self.state.is_cancelling() { return Err(FetchError::Cancelled); }
        let baseline = baseline_repo.baseline(slug).await.map_err(|e| FetchError::Persistence(e.to_string()))?;
        if let Some(timestamp) = baseline.last_sync.as_deref().filter(|t| !t.is_empty()) {
            if baseline.persisted_count > 0 && !baseline.needs_backfill && !baseline.missing_shortcuts && *rom_count == baseline.persisted_count as i64 {
                if let Ok(response) = delta.updated_after(*id, timestamp).await {
                    // Malformed responses are never proof that a platform is unchanged.
                    if response.get("total").and_then(Value::as_u64) == Some(0) {
                        let mut roms = baseline.bound_roms;
                        for rom in &mut roms {
                            rom["platform_name"] = json!(name);
                            rom["platform_display_name"] = json!(name);
                            rom["platform_slug"] = json!(slug);
                            rom["platform_id"] = json!(id);
                        }
                        return Ok((roms, true));
                    }
                }
            }
        }
        let (roms, _) = self.fetch_unit(unit, &mut HashSet::new()).await?;
        Ok((roms, false))
    }

    /// Full fetch. Any page failure aborts the unit so partial lists cannot reach stale removal.
    pub async fn fetch_unit(&self, unit: &WorkUnit, synced_ids: &mut HashSet<i64>) -> Result<(Vec<Value>, Vec<i64>), FetchError> {
        let mut roms = vec![];
        let mut memberships = vec![];
        let mut seen = synced_ids.clone();
        let mut offset = 0;
        loop {
            if self.state.is_cancelling() { return Err(FetchError::Cancelled); }
            let page = self.pages.page(unit, offset).await?;
            let items = page.as_array().or_else(|| page.get("items").and_then(Value::as_array)).ok_or(FetchError::InvalidResponse)?;
            for item in items {
                let mut rom = item.clone();
                let id = rom["id"].as_i64().ok_or(FetchError::InvalidResponse)?;
                match unit {
                    WorkUnit::Platform { name, slug, .. } => {
                        rom["platform_name"] = json!(name); rom["platform_slug"] = json!(slug);
                    }
                    WorkUnit::Collection { .. } => {
                        memberships.push(id);
                        if !seen.insert(id) { continue; }
                        if rom.get("platform_name").is_none() { rom["platform_name"] = json!(text(&rom, "platform_display_name", "Unknown")); }
                        if rom.get("platform_slug").is_none() { rom["platform_slug"] = json!(text(&rom, "platform_fs_slug", "")); }
                    }
                }
                roms.push(rom);
            }
            if items.len() < 50 { break; }
            offset += 50;
        }
        if matches!(unit, WorkUnit::Collection { .. }) { *synced_ids = seen; }
        Ok((roms, memberships))
    }
}

fn text<'a>(v: &'a Value, key: &str, fallback: &'a str) -> &'a str { v.get(key).and_then(Value::as_str).unwrap_or(fallback) }

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::Mutex;
    use crate::services::testing::FakeRomm;
    struct Pages(Mutex<std::collections::VecDeque<Result<Value, RommError>>>);
    #[async_trait]
    impl RommLibraryPages for Pages {
        async fn page(&self, _: &WorkUnit, offset: usize) -> Result<Value, RommError> {
            assert!(offset == 0 || offset == 50);
            self.0.lock().unwrap().pop_front().unwrap()
        }
    }
    fn fixture(pages: Vec<Result<Value, RommError>>) -> (LibraryFetcher, Arc<FakeRomm>) {
        let settings = SettingsStore::default();
        let api = Arc::new(FakeRomm::new(settings.clone()));
        (LibraryFetcher {metadata: api.clone(), pages: Arc::new(Pages(Mutex::new(pages.into()))), settings, state: Arc::new(LibraryRunState::default())}, api)
    }
    fn unit(kind: Option<CollectionKind>) -> WorkUnit {
        match kind { None => WorkUnit::Platform {id:1,name:"Switch".into(),slug:"switch".into(),rom_count:51},
            Some(collection_kind) => WorkUnit::Collection {id:"7".into(),name:"Favorites".into(),slug:"".into(),rom_count:51,collection_kind} }
    }
    fn full() -> Value { json!({"items": (1..=50).map(|id| json!({"id":id})).collect::<Vec<_>>()}) }
    #[tokio::test]
    async fn queue_order_preferences_and_payload() {
        let (f, api) = fixture(vec![]);
        FakeRomm::set(&api.list_platforms, Ok(json!([{"id":1,"display_name":"Switch","slug":"switch","rom_count":51},{"id":2}])));
        f.settings.set("enabled_platforms", json!({"1":true}));
        f.settings.set("enabled_collections", json!({"user":{"7":true},"smart":{"7":true},"franchise":{"7":true}}));
        for slot in [&api.list_collections,&api.list_smart_collections,&api.list_virtual_collections] { FakeRomm::set(slot, Ok(json!([{"id":7,"rom_ids":[1,2]},{"id":8}]))); }
        let units = f.build_work_queue().await.unwrap();
        assert_eq!(units.len(),4);
        assert_eq!(units[0],unit(None));
        let payload = serde_json::to_value(&units).unwrap();
        assert!(payload[0].get("collection_kind").is_none());
        assert_eq!(payload[1]["id"],"7");
        assert_eq!(payload[1]["rom_count"],2);
        for (i,kind) in ["user","smart","franchise"].iter().enumerate() { assert_eq!(payload[i+1]["collection_kind"],*kind); }
        for slot in [&api.list_collections,&api.list_smart_collections,&api.list_virtual_collections] { FakeRomm::set(slot, Err(RommError::Connection("offline".into()))); }
        assert_eq!(f.build_work_queue().await.unwrap().len(),1);
        FakeRomm::set(&api.list_platforms, Ok(json!({})));
        assert!(f.build_work_queue().await.unwrap().is_empty());
        FakeRomm::set(&api.list_platforms, Err(RommError::Connection("offline".into())));
        assert!(f.build_work_queue().await.is_err());
    }
    #[tokio::test]
    async fn full_pages_and_collection_deduplication() {
        for kind in [None,Some(CollectionKind::User),Some(CollectionKind::Smart),Some(CollectionKind::Franchise)] {
            let (f, _) = fixture(vec![Ok(full()),Ok(json!([{"id":51,"platform_display_name":"Switch","platform_fs_slug":"switch"}]))]);
            let mut seen = HashSet::from([1]);
            let (roms,members) = f.fetch_unit(&unit(kind),&mut seen).await.unwrap();
            assert_eq!(roms.len(),if kind.is_some() {50} else {51});
            assert_eq!(members.len(),if kind.is_some() {51} else {0});
            assert_eq!(roms.last().unwrap()["platform_slug"],"switch");
            assert_eq!(roms.last().unwrap()["platform_name"],"Switch");
            let (key,_) = unit(kind).query_filter();
            assert_eq!(key,match kind {None=>"platform_ids",Some(CollectionKind::User)=>"collection_id",Some(CollectionKind::Smart)=>"smart_collection_id",_=>"virtual_collection_id"});
        }
    }
    #[tokio::test]
    async fn failures_preserve_caller_accounting_and_cancellation_prevents_fetch() {
        for response in [Err(RommError::Connection("offline".into())),Ok(json!({})),Ok(json!([{}]))] {
            let (f,_) = fixture(vec![Ok(full()),response]);
            let mut seen = HashSet::from([100]);
            assert!(f.fetch_unit(&unit(Some(CollectionKind::User)),&mut seen).await.is_err());
            assert_eq!(seen,HashSet::from([100]));
        }
        let (f,_) = fixture(vec![]);
        f.state.try_begin_run("run".into()); f.state.request_cancel(None);
        assert!(matches!(f.fetch_unit(&unit(None),&mut HashSet::new()).await,Err(FetchError::Cancelled)));
    }
}

#[cfg(test)]
mod incremental_tests {
    use super::*;
    use async_trait::async_trait;
    use crate::services::ports::{IncrementalBaseline, LibraryBaselineRepo, RommLibraryDelta};
    struct Baseline(IncrementalBaseline);
    #[async_trait]
    impl LibraryBaselineRepo for Baseline {
        async fn baseline(&self, _: &str) -> anyhow::Result<IncrementalBaseline> { Ok(self.0.clone()) }
    }
    struct Delta(Result<Value,RommError>);
    #[async_trait]
    impl RommLibraryDelta for Delta {
        async fn updated_after(&self, id:i64, at:&str) -> Result<Value,RommError> {
            assert_eq!(id, 1); assert_eq!(at,"watermark"); self.0.clone()
        }
    }
    struct Pages;
    #[async_trait]
    impl RommLibraryPages for Pages {
        async fn page(&self,_:&WorkUnit,offset:usize)->Result<Value,RommError> { assert_eq!(offset,0); Ok(json!({"items":[{"id":99}]})) }
    }
    fn baseline() -> IncrementalBaseline { IncrementalBaseline {last_sync:Some("watermark".into()),bound_roms:vec![json!({"id":1,"sibling_group_key":"igdb:7:1"})],persisted_count:2,needs_backfill:false,missing_shortcuts:false} }
    #[tokio::test]
    async fn skip_counts_unbound_siblings_and_restores_live_platform_identity() {
        let settings = SettingsStore::default();
        let f = LibraryFetcher { metadata:Arc::new(crate::services::testing::FakeRomm::new(settings.clone())),pages:Arc::new(Pages),settings,state:Arc::new(LibraryRunState::default()) };
        let unit = WorkUnit::Platform {id:1,name:"Nintendo Switch".into(),slug:"switch".into(),rom_count:2};
        let (roms,skipped) = f.fetch_platform_unit(&unit,&Baseline(baseline()),&Delta(Ok(json!({"total":0})))).await.unwrap();
        assert!(skipped); assert_eq!(roms.len(),1);
        assert_eq!(roms[0]["platform_id"],1); assert_eq!(roms[0]["platform_name"],"Nintendo Switch");
        assert!(roms[0].get("metadatum").is_none());
        assert_eq!(roms[0]["sibling_group_key"],"igdb:7:1");
        for bad in [Ok(json!({"total":1})),Ok(json!({})),Ok(json!([])),Err(RommError::Connection("offline".into()))] {
            let (roms,skipped) = f.fetch_platform_unit(&unit,&Baseline(baseline()),&Delta(bad)).await.unwrap();
            assert!(!skipped);assert_eq!(roms[0]["id"],99);
        }
        for change in 0..4 {
            let mut b=baseline();
            match change {0=>b.last_sync=None,1=>b.persisted_count=0,2=>b.persisted_count=1,_=>b.needs_backfill=true}
            assert!(!f.fetch_platform_unit(&unit,&Baseline(b),&Delta(Ok(json!({"total":0})))).await.unwrap().1);
        }
        let collection=WorkUnit::Collection {id:"7".into(),name:"X".into(),slug:"".into(),rom_count:0,collection_kind:CollectionKind::User};
        assert!(f.fetch_platform_unit(&collection,&Baseline(baseline()),&Delta(Ok(json!({"total":0})))).await.is_err());
    }
}
