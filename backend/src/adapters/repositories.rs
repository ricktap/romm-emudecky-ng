//! SQLite repositories over the [`Db`] actor.
//!
//! Each repository is the persistence side of one service port: the SQL lives
//! here, the services see only the trait.

use async_trait::async_trait;
use rusqlite::{OptionalExtension, Row, params};

use crate::adapters::db::Db;
use crate::domain::download_job::{DownloadJob, JobStatus};
use crate::domain::rom_install::RomInstall;
use crate::services::ports::{DownloadJobRepo, KvConfigRepo, LibraryRepo, RomRegionsReader, RomRow};

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


const JOB_COLUMNS: &str = "rom_id, rom_name, platform_name, file_name, target_path, tmp_path, is_multi, total_bytes, \
     status, interrupt_reason, range_supported, position, created_at, updated_at";

fn row_to_job(r: &Row<'_>) -> rusqlite::Result<DownloadJob> {
    let status: String = r.get(8)?;
    let total: Option<i64> = r.get(7)?;
    Ok(DownloadJob {
        rom_id: r.get(0)?,
        rom_name: r.get(1)?,
        platform_name: r.get(2)?,
        file_name: r.get(3)?,
        target_path: r.get(4)?,
        tmp_path: r.get(5)?,
        is_multi: r.get::<_, i64>(6)? != 0,
        total_bytes: total.and_then(|t| u64::try_from(t).ok()),
        status: JobStatus::parse(&status).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(8, rusqlite::types::Type::Text, format!("bad status {status}").into())
        })?,
        interrupt_reason: r.get(9)?,
        range_supported: r.get::<_, Option<i64>>(10)?.map(|v| v != 0),
        position: r.get(11)?,
        created_at: r.get(12)?,
        updated_at: r.get(13)?,
    })
}

/// The `download_jobs` table (the DownloadJob aggregate).
#[derive(Clone)]
pub struct SqliteDownloadJobs {
    db: Db,
}

impl SqliteDownloadJobs {
    pub fn new(db: Db) -> Self {
        Self { db }
    }
}

#[async_trait]
impl DownloadJobRepo for SqliteDownloadJobs {
    async fn get(&self, rom_id: i64) -> anyhow::Result<Option<DownloadJob>> {
        self.db
            .call(move |c| {
                c.query_row(
                    &format!("SELECT {JOB_COLUMNS} FROM download_jobs WHERE rom_id = ?1"),
                    [rom_id],
                    row_to_job,
                )
                .optional()
            })
            .await
    }

    async fn save(&self, job: &DownloadJob) -> anyhow::Result<()> {
        let j = job.clone();
        self.db
            .call(move |c| {
                c.execute(
                    &format!(
                        "INSERT OR REPLACE INTO download_jobs ({JOB_COLUMNS}) \
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)"
                    ),
                    params![
                        j.rom_id,
                        j.rom_name,
                        j.platform_name,
                        j.file_name,
                        j.target_path,
                        j.tmp_path,
                        i64::from(j.is_multi),
                        j.total_bytes.map(|t| t as i64),
                        j.status.as_str(),
                        j.interrupt_reason,
                        j.range_supported.map(i64::from),
                        j.position,
                        j.created_at,
                        j.updated_at,
                    ],
                )
                .map(|_| ())
            })
            .await
    }

    async fn delete(&self, rom_id: i64) -> anyhow::Result<()> {
        self.db
            .call(move |c| c.execute("DELETE FROM download_jobs WHERE rom_id = ?1", [rom_id]).map(|_| ()))
            .await
    }

    async fn all(&self) -> anyhow::Result<Vec<DownloadJob>> {
        self.db
            .call(|c| {
                let mut stmt = c.prepare(&format!(
                    "SELECT {JOB_COLUMNS} FROM download_jobs ORDER BY position, rom_id"
                ))?;
                let rows = stmt.query_map([], row_to_job)?;
                rows.collect()
            })
            .await
    }

    async fn next_position(&self) -> anyhow::Result<i64> {
        let max: Option<i64> = self
            .db
            .call(|c| c.query_row("SELECT MAX(position) FROM download_jobs", [], |r| r.get(0)))
            .await?;
        Ok(max.map_or(1, |m| m + 1))
    }

    async fn min_position(&self) -> anyhow::Result<Option<i64>> {
        self.db
            .call(|c| c.query_row("SELECT MIN(position) FROM download_jobs", [], |r| r.get(0)))
            .await
    }
}

const ROM_COLUMNS: &str =
    "rom_id, platform_slug, name, fs_name, shortcut_app_id, emulator_override, selected_disc, sibling_group_key";

fn row_to_rom(r: &Row<'_>) -> rusqlite::Result<RomRow> {
    Ok(RomRow {
        rom_id: r.get(0)?,
        platform_slug: r.get(1)?,
        name: r.get(2)?,
        fs_name: r.get(3)?,
        shortcut_app_id: r.get(4)?,
        emulator_override: r.get(5)?,
        selected_disc: r.get(6)?,
        sibling_group_key: r.get(7)?,
    })
}

const INSTALL_COLUMNS: &str = "rom_id, file_path, rom_dir, platform_slug, system, installed_at";

fn row_to_install(r: &Row<'_>) -> rusqlite::Result<RomInstall> {
    Ok(RomInstall {
        rom_id: r.get(0)?,
        file_path: r.get(1)?,
        rom_dir: r.get(2)?,
        platform_slug: r.get(3)?,
        system: r.get(4)?,
        installed_at: r.get(5)?,
    })
}

fn insert_install(c: &rusqlite::Connection, i: &RomInstall) -> rusqlite::Result<()> {
    c.execute(
        &format!("INSERT OR REPLACE INTO rom_installs ({INSTALL_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6)"),
        params![i.rom_id, i.file_path, i.rom_dir, i.platform_slug, i.system, i.installed_at],
    )
    .map(|_| ())
}

/// The `roms` registry and the `rom_installs` records.
#[derive(Clone)]
pub struct SqliteLibrary {
    db: Db,
}

impl SqliteLibrary {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    async fn roms_where(&self, clause: &'static str, arg: String) -> anyhow::Result<Vec<RomRow>> {
        self.db
            .call(move |c| {
                let mut stmt = c.prepare(&format!("SELECT {ROM_COLUMNS} FROM roms WHERE {clause} ORDER BY rom_id"))?;
                let rows = stmt.query_map([arg], row_to_rom)?;
                rows.collect()
            })
            .await
    }
}

#[async_trait]
impl LibraryRepo for SqliteLibrary {
    async fn rom(&self, rom_id: i64) -> anyhow::Result<Option<RomRow>> {
        self.db
            .call(move |c| {
                c.query_row(
                    &format!("SELECT {ROM_COLUMNS} FROM roms WHERE rom_id = ?1"),
                    [rom_id],
                    row_to_rom,
                )
                .optional()
            })
            .await
    }

    async fn roms_in_group(&self, group_key: &str) -> anyhow::Result<Vec<RomRow>> {
        self.roms_where("sibling_group_key = ?1", group_key.to_string()).await
    }

    async fn roms_on_platform(&self, platform_slug: &str) -> anyhow::Result<Vec<RomRow>> {
        self.roms_where("platform_slug = ?1", platform_slug.to_string()).await
    }

    async fn install(&self, rom_id: i64) -> anyhow::Result<Option<RomInstall>> {
        self.db
            .call(move |c| {
                c.query_row(
                    &format!("SELECT {INSTALL_COLUMNS} FROM rom_installs WHERE rom_id = ?1"),
                    [rom_id],
                    row_to_install,
                )
                .optional()
            })
            .await
    }

    async fn installs(&self) -> anyhow::Result<Vec<RomInstall>> {
        self.db
            .call(|c| {
                let mut stmt = c.prepare(&format!("SELECT {INSTALL_COLUMNS} FROM rom_installs ORDER BY rom_id"))?;
                let rows = stmt.query_map([], row_to_install)?;
                rows.collect()
            })
            .await
    }

    async fn save_install(&self, install: &RomInstall) -> anyhow::Result<()> {
        let i = install.clone();
        self.db.call(move |c| insert_install(c, &i)).await
    }

    async fn delete_install(&self, rom_id: i64) -> anyhow::Result<()> {
        self.db
            .call(move |c| c.execute("DELETE FROM rom_installs WHERE rom_id = ?1", [rom_id]).map(|_| ()))
            .await
    }

    async fn adopt_install(&self, install: &RomInstall, group_key: Option<&str>) -> anyhow::Result<bool> {
        let i = install.clone();
        let key = group_key.map(str::to_string);
        self.db
            .call(move |c| {
                let tx = c.transaction()?;
                let exists = |sql: &str, arg: &dyn rusqlite::ToSql| -> rusqlite::Result<bool> {
                    tx.query_row(sql, [arg], |r| r.get::<_, i64>(0)).map(|n| n > 0)
                };
                let id = i.rom_id;
                if !exists("SELECT count(*) FROM roms WHERE rom_id = ?1", &id)?
                    || exists("SELECT count(*) FROM rom_installs WHERE rom_id = ?1", &id)?
                    || exists("SELECT count(*) FROM download_jobs WHERE rom_id = ?1", &id)?
                {
                    return Ok(false);
                }
                if let Some(key) = &key {
                    let sibling: i64 = tx.query_row(
                        "SELECT count(*) FROM roms r JOIN rom_installs i ON i.rom_id = r.rom_id \
                         WHERE r.sibling_group_key = ?1 AND r.rom_id != ?2",
                        params![key, id],
                        |r| r.get(0),
                    )?;
                    if sibling > 0 {
                        return Ok(false);
                    }
                }
                insert_install(&tx, &i)?;
                tx.commit()?;
                Ok(true)
            })
            .await
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
