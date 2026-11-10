//! The plugin's SQLite database, owned by one dedicated connection thread.
//!
//! A single `rusqlite::Connection` lives on its own OS thread (the "actor");
//! callers hand it closures through [`Db::call`] and await the result, so no
//! blocking SQLite call ever runs on the tokio runtime and every access is
//! serialized on one connection. Opening the database also applies the schema
//! migrations embedded from `backend/migrations/` with the same semantics as
//! the Python runner (`PRAGMA user_version`, one transaction per file).

use std::path::Path;
use std::sync::mpsc;
use std::thread;

use anyhow::{Context, anyhow};
use rusqlite::Connection;
use tokio::sync::oneshot;

/// Database file name inside `DECKY_PLUGIN_RUNTIME_DIR`.
pub const DB_FILENAME: &str = "romm_sync.db";

/// Every schema migration, in order: `(version, file name, DDL)`. Files contain
/// transaction-safe DDL only — no `BEGIN` / `COMMIT`; the runner supplies those.
pub const MIGRATIONS: &[(u32, &str, &str)] = &[
    (1, "001_initial.sql", include_str!("../../migrations/001_initial.sql")),
    (
        2,
        "002_add_emulator_override.sql",
        include_str!("../../migrations/002_add_emulator_override.sql"),
    ),
    (
        3,
        "003_unique_shortcut_app_id.sql",
        include_str!("../../migrations/003_unique_shortcut_app_id.sql"),
    ),
    (
        4,
        "004_add_selected_disc.sql",
        include_str!("../../migrations/004_add_selected_disc.sql"),
    ),
    (
        5,
        "005_unconfirm_legacy_slot_confirmations.sql",
        include_str!("../../migrations/005_unconfirm_legacy_slot_confirmations.sql"),
    ),
    (
        6,
        "006_native_play_sessions.sql",
        include_str!("../../migrations/006_native_play_sessions.sql"),
    ),
    (
        7,
        "007_add_last_played.sql",
        include_str!("../../migrations/007_add_last_played.sql"),
    ),
    (
        8,
        "008_add_version_metadata.sql",
        include_str!("../../migrations/008_add_version_metadata.sql"),
    ),
    (
        9,
        "009_add_last_session_start_monotonic.sql",
        include_str!("../../migrations/009_add_last_session_start_monotonic.sql"),
    ),
    (
        10,
        "010_add_sibling_group_key_index.sql",
        include_str!("../../migrations/010_add_sibling_group_key_index.sql"),
    ),
    (
        11,
        "011_rekey_sibling_group_key.sql",
        include_str!("../../migrations/011_rekey_sibling_group_key.sql"),
    ),
    (
        12,
        "012_download_jobs.sql",
        include_str!("../../migrations/012_download_jobs.sql"),
    ),
    (
        13,
        "013_download_archive_identity.sql",
        include_str!("../../migrations/013_download_archive_identity.sql"),
    ),
];

/// Apply every migration newer than the database's `user_version`.
///
/// Each migration runs as `BEGIN; <ddl>; PRAGMA user_version = N; COMMIT;`; on
/// failure the transaction is rolled back (the database stays at the last good
/// version) and the error is returned. Returns the resulting `user_version`.
pub fn apply_migrations(conn: &Connection, migrations: &[(u32, &str, &str)]) -> anyhow::Result<u32> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    let mut current: u32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    for &(version, name, ddl) in migrations {
        if version <= current {
            continue;
        }
        tracing::info!("Applying SQLite migration {name}");
        let script = format!("BEGIN;\n{ddl}\nPRAGMA user_version = {version};\nCOMMIT;");
        if let Err(err) = conn.execute_batch(&script) {
            if !conn.is_autocommit() {
                let _ = conn.execute_batch("ROLLBACK");
            }
            return Err(anyhow!(err).context(format!("SQLite migration {name} failed")));
        }
        current = version;
    }
    Ok(current)
}

type Job = Box<dyn FnOnce(&mut Connection) + Send>;

/// Async handle to the database actor. Cheap to clone; the actor thread exits
/// once every handle is dropped.
#[derive(Clone)]
pub struct Db {
    tx: mpsc::Sender<Job>,
}

impl Db {
    /// Open (creating if needed) the database at `path`, apply the runtime
    /// PRAGMAs and the embedded migrations, and start the actor thread.
    /// Returns the handle and the schema version after migrating.
    pub async fn open(path: &Path) -> anyhow::Result<(Db, u32)> {
        Self::open_with(path, MIGRATIONS).await
    }

    /// [`Db::open`] with an explicit migration list (tests).
    pub async fn open_with(
        path: &Path,
        migrations: &'static [(u32, &'static str, &'static str)],
    ) -> anyhow::Result<(Db, u32)> {
        let path = path.to_path_buf();
        let (ready_tx, ready_rx) = oneshot::channel::<anyhow::Result<u32>>();
        let (tx, rx) = mpsc::channel::<Job>();
        thread::Builder::new()
            .name("sqlite-actor".into())
            .spawn(move || {
                let opened = (|| -> anyhow::Result<(Connection, u32)> {
                    if let Some(parent) = path.parent() {
                        std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
                    }
                    let conn = Connection::open(&path).with_context(|| format!("opening {}", path.display()))?;
                    let version = apply_migrations(&conn, migrations)?;
                    conn.execute_batch(
                        "PRAGMA foreign_keys=ON; PRAGMA synchronous=NORMAL; \
                         PRAGMA busy_timeout=5000; PRAGMA temp_store=MEMORY;",
                    )?;
                    Ok((conn, version))
                })();
                let mut conn = match opened {
                    Ok((conn, version)) => {
                        let _ = ready_tx.send(Ok(version));
                        conn
                    }
                    Err(err) => {
                        let _ = ready_tx.send(Err(err));
                        return;
                    }
                };
                while let Ok(job) = rx.recv() {
                    job(&mut conn);
                }
            })
            .context("spawning the SQLite actor thread")?;
        let version = ready_rx
            .await
            .map_err(|_| anyhow!("SQLite actor exited during open"))??;
        Ok((Db { tx }, version))
    }

    /// Run `f` on the actor's connection and return its result.
    pub async fn call<R, F>(&self, f: F) -> anyhow::Result<R>
    where
        R: Send + 'static,
        F: FnOnce(&mut Connection) -> rusqlite::Result<R> + Send + 'static,
    {
        let (reply_tx, reply_rx) = oneshot::channel();
        let job: Job = Box::new(move |conn| {
            let _ = reply_tx.send(f(conn));
        });
        self.tx.send(job).map_err(|_| anyhow!("SQLite actor has stopped"))?;
        Ok(reply_rx
            .await
            .map_err(|_| anyhow!("SQLite actor dropped the request"))??)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_list_matches_directory() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
        let mut on_disk: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".sql"))
            .collect();
        on_disk.sort();
        let listed: Vec<String> = MIGRATIONS.iter().map(|(_, n, _)| n.to_string()).collect();
        assert_eq!(listed, on_disk);
        for (i, (version, name, ddl)) in MIGRATIONS.iter().enumerate() {
            assert_eq!(*version as usize, i + 1);
            assert!(name.starts_with(&format!("{version:03}_")));
            let upper = ddl.to_ascii_uppercase();
            assert!(
                !upper.lines().any(|l| l.trim() == "BEGIN;" || l.trim() == "COMMIT;"),
                "{name}"
            );
        }
    }

    #[tokio::test]
    async fn fresh_database_reaches_latest_version() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join(DB_FILENAME);
        let (db, version) = Db::open(&path).await.unwrap();
        assert_eq!(version, 13);
        let mode: String = db
            .call(|c| c.pragma_query_value(None, "journal_mode", |r| r.get(0)))
            .await
            .unwrap();
        assert_eq!(mode, "wal");
        let fk: i64 = db
            .call(|c| c.pragma_query_value(None, "foreign_keys", |r| r.get(0)))
            .await
            .unwrap();
        assert_eq!(fk, 1);
        let tables: i64 = db
            .call(|c| {
                c.query_row("SELECT count(*) FROM sqlite_master WHERE name='kv_config'", [], |r| {
                    r.get(0)
                })
            })
            .await
            .unwrap();
        assert_eq!(tables, 1);
        drop(db);
        // Re-open: nothing pending, version unchanged.
        let (_db, version) = Db::open(&path).await.unwrap();
        assert_eq!(version, 13);
    }

    static BROKEN: &[(u32, &str, &str)] = &[
        (1, "001_ok.sql", "CREATE TABLE a (x INTEGER);"),
        (2, "002_bad.sql", "CREATE TABLE b (x INTEGER); THIS IS NOT SQL;"),
        (3, "003_never.sql", "CREATE TABLE c (x INTEGER);"),
    ];

    #[tokio::test]
    async fn failing_migration_rolls_back_and_stops() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.sqlite");
        let err = Db::open_with(&path, BROKEN).await.err().expect("must fail");
        assert!(format!("{err:#}").contains("002_bad.sql"));
        let conn = Connection::open(&path).unwrap();
        let version: u32 = conn.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
        assert_eq!(version, 1);
        let b: i64 = conn
            .query_row("SELECT count(*) FROM sqlite_master WHERE name='b'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(b, 0, "the failed migration's DDL was rolled back");
    }

    #[tokio::test]
    async fn call_surfaces_sql_errors() {
        let dir = tempfile::tempdir().unwrap();
        let (db, _) = Db::open(&dir.path().join("x.db")).await.unwrap();
        let err = db
            .call(|c| c.execute("INSERT INTO nope VALUES (1)", []))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("nope"));
    }

    #[tokio::test]
    async fn concurrent_calls_are_serialized() {
        let dir = tempfile::tempdir().unwrap();
        let (db, _) = Db::open(&dir.path().join("x.db")).await.unwrap();
        let mut handles = Vec::new();
        for i in 0..20 {
            let db = db.clone();
            handles.push(tokio::spawn(async move {
                db.call(move |c| c.execute("INSERT INTO kv_config (key, value) VALUES (?1, 'v')", [format!("k{i}")]))
                    .await
                    .unwrap()
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
        let n: i64 = db
            .call(|c| c.query_row("SELECT count(*) FROM kv_config", [], |r| r.get(0)))
            .await
            .unwrap();
        assert_eq!(n, 20);
    }
}
