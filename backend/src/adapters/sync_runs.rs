//! Sync history over the existing Python-compatible schema and shared DB actor.
use async_trait::async_trait;
use rusqlite::{OptionalExtension, Row, params, types::Type};
use crate::adapters::db::Db;
use crate::domain::sync_run::{SyncRun, SyncRunStatus};
use crate::services::ports::SyncRunRepo;

const COLUMNS: &str = "id, started_at, status, platforms_planned, roms_planned, finished_at, platforms_completed, collections_completed, error";

#[derive(Clone)]
pub struct SqliteSyncRuns { db: Db }

impl SqliteSyncRuns {
    pub fn new(db: Db) -> Self { Self { db } }
}

fn json_column(row: &Row<'_>, index: usize) -> rusqlite::Result<Option<Vec<String>>> {
    let raw: Option<String> = row.get(index)?;
    raw.map(|text| serde_json::from_str(&text).map_err(|err|
        rusqlite::Error::FromSqlConversionFailure(index, Type::Text, Box::new(err))
    )).transpose()
}

fn read_run(row: &Row<'_>) -> rusqlite::Result<SyncRun> {
    let status: String = row.get(2)?;
    let status = serde_json::from_value(serde_json::Value::String(status)).map_err(|err|
        rusqlite::Error::FromSqlConversionFailure(2, Type::Text, Box::new(err))
    )?;
    Ok(SyncRun {
        id: row.get(0)?, started_at: row.get(1)?, status,
        platforms_planned: row.get(3)?, roms_planned: row.get(4)?, finished_at: row.get(5)?,
        platforms_completed: json_column(row, 6)?, collections_completed: json_column(row, 7)?,
        error: row.get(8)?,
    })
}

#[async_trait]
impl SyncRunRepo for SqliteSyncRuns {
    async fn get(&self, id: &str) -> anyhow::Result<Option<SyncRun>> {
        let id = id.to_string();
        self.db.call(move |c| c.query_row(
            &format!("SELECT {COLUMNS} FROM sync_runs WHERE id = ?1"), [id], read_run,
        ).optional()).await
    }

    async fn save(&self, run: &SyncRun) -> anyhow::Result<()> {
        let run = run.clone();
        let platforms = run.platforms_completed.as_ref().map(serde_json::to_string).transpose()?;
        let collections = run.collections_completed.as_ref().map(serde_json::to_string).transpose()?;
        let status = match run.status {
            SyncRunStatus::Running => "running", SyncRunStatus::Completed => "completed",
            SyncRunStatus::Cancelled => "cancelled", SyncRunStatus::Errored => "errored",
        };
        self.db.call(move |c| c.execute(
            &format!("INSERT OR REPLACE INTO sync_runs ({COLUMNS}) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)"),
            params![run.id, run.started_at, status, run.platforms_planned, run.roms_planned,
                run.finished_at, platforms, collections, run.error],
        ).map(|_| ())).await
    }

    async fn latest_completed(&self) -> anyhow::Result<Option<SyncRun>> {
        self.db.call(|c| c.query_row(
            &format!("SELECT {COLUMNS} FROM sync_runs WHERE status = 'completed' ORDER BY started_at DESC LIMIT 1"),
            [], read_run,
        ).optional()).await
    }

    async fn running(&self) -> anyhow::Result<Option<SyncRun>> {
        self.db.call(|c| c.query_row(
            &format!("SELECT {COLUMNS} FROM sync_runs WHERE status = 'running' LIMIT 1"),
            [], read_run,
        ).optional()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::repositories::SqliteLibrary;
    use crate::services::ports::LibraryRepo;

    fn run(id: &str, at: &str) -> SyncRun {
        SyncRun::start(id.into(), at.into(), 1, 10).unwrap()
    }

    #[tokio::test]
    async fn roundtrip_upsert_and_restart_preserve_every_outcome() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sync.db");
        let (db, _) = Db::open(&path).await.unwrap();
        let repo = SqliteSyncRuns::new(db.clone());
        assert_eq!(repo.get("missing").await.unwrap(), None);
        assert_eq!(repo.running().await.unwrap(), None);
        assert_eq!(repo.latest_completed().await.unwrap(), None);
        let mut expected = vec![];
        for id in ["completed", "cancelled", "errored", "running"] {
            let mut run = run(id, "2026-01-01");
            repo.save(&run).await.unwrap();
            assert_eq!(repo.get(id).await.unwrap(), Some(run.clone()));
            match id {
                "completed" => run.complete("end".into(), vec!["switch".into()], vec!["日本語 \"Favorites\"".into()]).unwrap(),
                "cancelled" => run.mark_cancelled("end".into(), "user aborted".into()).unwrap(),
                "errored" => run.mark_errored("end".into(), "offline".into()).unwrap(),
                _ => (),
            }
            repo.save(&run).await.unwrap();
            assert_eq!(repo.get(id).await.unwrap(), Some(run.clone()));
            expected.push(run);
        }
        drop(repo);
        drop(db);
        let (db, _) = Db::open(&path).await.unwrap();
        let repo = SqliteSyncRuns::new(db);
        for run in expected { assert_eq!(repo.get(&run.id).await.unwrap(), Some(run)); }
        assert_eq!(repo.running().await.unwrap().unwrap().id, "running");
    }

    #[tokio::test]
    async fn history_orders_by_start_and_cache_reset_and_healing_share_the_same_rows() {
        let dir = tempfile::tempdir().unwrap();
        let (db, _) = Db::open(&dir.path().join("sync.db")).await.unwrap();
        let repo = SqliteSyncRuns::new(db.clone());
        let library = SqliteLibrary::new(db);
        for (id, start, finish) in [("old", "2026-01", "2026-04"), ("new", "2026-02", "2026-03")] {
            let mut run = run(id, start);
            run.complete(finish.into(), vec![], vec![]).unwrap();
            repo.save(&run).await.unwrap();
        }
        repo.save(&run("active", "2026-05")).await.unwrap();
        assert_eq!(repo.latest_completed().await.unwrap().unwrap().id, "new");
        assert_eq!(library.latest_completed_sync_finished_at().await.unwrap().as_deref(), Some("2026-03"));
        assert_eq!(library.interrupt_running_syncs("2026-06").await.unwrap(), 1);
        assert_eq!(library.interrupt_running_syncs("2026-07").await.unwrap(), 0);
        assert_eq!(repo.running().await.unwrap(), None);
        let healed = repo.get("active").await.unwrap().unwrap();
        assert_eq!(healed.status, SyncRunStatus::Errored);
        assert_eq!(healed.finished_at.as_deref(), Some("2026-06"));
        assert_eq!(healed.error.as_deref(), Some("interrupted by restart"));
        repo.save(&run("still-running", "2026-08")).await.unwrap();
        library.delete_completed_sync_runs().await.unwrap();
        library.delete_completed_sync_runs().await.unwrap();
        assert_eq!(repo.latest_completed().await.unwrap(), None);
        assert_eq!(repo.get("old").await.unwrap(), None);
        assert_eq!(repo.get("new").await.unwrap(), None);
        assert_eq!(repo.get("active").await.unwrap(), Some(healed));
        assert_eq!(repo.running().await.unwrap().unwrap().id, "still-running");
    }

    #[tokio::test]
    async fn malformed_history_is_an_error_not_an_empty_sync_baseline() {
        let dir = tempfile::tempdir().unwrap();
        let (db, _) = Db::open(&dir.path().join("sync.db")).await.unwrap();
        let repo = SqliteSyncRuns::new(db.clone());
        let mut run = run("broken", "start");
        run.complete("end".into(), vec![], vec![]).unwrap();
        repo.save(&run).await.unwrap();
        db.call(|c| c.execute("UPDATE sync_runs SET platforms_completed = '{}'", [])).await.unwrap();
        assert!(repo.get("broken").await.is_err());
        assert!(repo.latest_completed().await.is_err());
        db.call(|c| c.execute("DROP TABLE sync_runs", [])).await.unwrap();
        assert!(repo.save(&run).await.is_err());
        assert!(repo.running().await.is_err());
    }
}
