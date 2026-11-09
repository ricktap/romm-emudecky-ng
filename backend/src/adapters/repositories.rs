//! SQLite repositories over the [`Db`] actor.
//!
//! Each repository is the persistence side of one service port: the SQL lives
//! here, the services see only the trait.

use async_trait::async_trait;
use rusqlite::OptionalExtension;

use crate::adapters::db::Db;
use crate::services::ports::{KvConfigRepo, RomRegionsReader};

/// The `kv_config` singleton-scalar table.
#[derive(Clone)]
pub struct SqliteKvConfig {
    db: Db,
}

impl SqliteKvConfig {
    pub fn new(db: Db) -> Self {
        Self { db }
    }
}

#[async_trait]
impl KvConfigRepo for SqliteKvConfig {
    async fn get(&self, key: &str) -> anyhow::Result<Option<String>> {
        let key = key.to_string();
        self.db
            .call(move |c| {
                c.query_row("SELECT value FROM kv_config WHERE key = ?1", [key], |r| r.get(0))
                    .optional()
            })
            .await
    }

    async fn set(&self, key: &str, value: &str) -> anyhow::Result<()> {
        let (key, value) = (key.to_string(), value.to_string());
        self.db
            .call(move |c| {
                c.execute(
                    "INSERT OR REPLACE INTO kv_config (key, value) VALUES (?1, ?2)",
                    [key, value],
                )
                .map(|_| ())
            })
            .await
    }

    async fn delete(&self, key: &str) -> anyhow::Result<()> {
        let key = key.to_string();
        self.db
            .call(move |c| c.execute("DELETE FROM kv_config WHERE key = ?1", [key]).map(|_| ()))
            .await
    }
}

/// Read side of the `roms.regions` JSON column.
#[derive(Clone)]
pub struct SqliteRomRegions {
    db: Db,
}

impl SqliteRomRegions {
    pub fn new(db: Db) -> Self {
        Self { db }
    }
}

#[async_trait]
impl RomRegionsReader for SqliteRomRegions {
    async fn all_rom_regions(&self) -> anyhow::Result<Vec<Vec<String>>> {
        let raw: Vec<String> = self
            .db
            .call(|c| {
                let mut stmt = c.prepare("SELECT regions FROM roms")?;
                let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
                rows.collect()
            })
            .await?;
        Ok(raw
            .iter()
            .map(|text| serde_json::from_str::<Vec<serde_json::Value>>(text).unwrap_or_default())
            .map(|values| {
                values
                    .into_iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn db() -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().unwrap();
        let (db, _) = Db::open(&dir.path().join("t.db")).await.unwrap();
        (dir, db)
    }

    #[tokio::test]
    async fn kv_round_trip() {
        let (_d, db) = db().await;
        let kv = SqliteKvConfig::new(db);
        assert_eq!(kv.get("device_id").await.unwrap(), None);
        kv.set("device_id", "abc").await.unwrap();
        kv.set("device_id", "def").await.unwrap();
        assert_eq!(kv.get("device_id").await.unwrap().as_deref(), Some("def"));
        kv.delete("device_id").await.unwrap();
        kv.delete("device_id").await.unwrap();
        assert_eq!(kv.get("device_id").await.unwrap(), None);
    }

    #[tokio::test]
    async fn rom_regions_parsed() {
        let (_d, db) = db().await;
        db.call(|c| {
            c.execute_batch(
                "INSERT INTO roms (rom_id, platform_slug, name, fs_name, last_synced_at, regions) VALUES
                 (1, 'switch', 'A', 'a.nsp', '2026-01-01T00:00:00', '[\"USA\",\"Europe\"]'),
                 (2, 'switch', 'B', 'b.nsp', '2026-01-01T00:00:00', '[]'),
                 (3, 'switch', 'C', 'c.nsp', '2026-01-01T00:00:00', '[\"Japan\", 5]');",
            )
        })
        .await
        .unwrap();
        let mut regions = SqliteRomRegions::new(db).all_rom_regions().await.unwrap();
        regions.sort();
        assert_eq!(
            regions,
            vec![vec![], vec!["Japan".to_string()], vec!["USA".into(), "Europe".into()]]
        );
    }
}
