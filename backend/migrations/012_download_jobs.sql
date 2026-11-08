-- =============================================================================
-- 012_download_jobs.sql — durable download queue (DownloadJob aggregate)
-- =============================================================================
--
-- One row per ROM whose download has not finished: queued, downloading,
-- extracting, user-paused, or interrupted by a transient failure (server
-- unreachable, auth, disk full, plugin restart). Completed / cancelled / failed
-- downloads delete their row — the recent-downloads history stays in memory.
--
-- The partial transfer file at tmp_path IS the progress record: the row changes
-- only on status transitions, never per progress tick (no SSD wear). Startup
-- reconciliation keeps a job's tmp file, deletes orphan tmp files, and
-- auto-resumes interrupted jobs in position order (user-paused jobs stay paused).
--
-- No foreign key onto roms: a job is a transient queue record keyed by the RomM
-- rom_id; the install it produces carries the FK (rom_installs).
--
-- Transaction-safe DDL only — the runner (adapters/sqlite_migrations.py) wraps
-- BEGIN/COMMIT and stamps PRAGMA user_version = 12.
-- -----------------------------------------------------------------------------
CREATE TABLE download_jobs (
    rom_id           INTEGER PRIMARY KEY,
    rom_name         TEXT    NOT NULL,
    platform_name    TEXT    NOT NULL,
    file_name        TEXT    NOT NULL,
    target_path      TEXT    NOT NULL,
    tmp_path         TEXT    NOT NULL,
    is_multi         INTEGER NOT NULL CHECK (is_multi IN (0, 1)),
    total_bytes      INTEGER,                                  -- NULL until the server reports it
    status           TEXT    NOT NULL CHECK (status IN ('queued', 'downloading', 'extracting', 'paused', 'interrupted')),
    interrupt_reason TEXT,                                     -- short slug; set only while interrupted
    range_supported  INTEGER CHECK (range_supported IS NULL OR range_supported IN (0, 1)),
    position         INTEGER NOT NULL,                         -- queue order, ascending
    created_at       TEXT    NOT NULL,                         -- ISO-8601
    updated_at       TEXT    NOT NULL                          -- ISO-8601
) STRICT;

CREATE INDEX idx_download_jobs_position ON download_jobs (position);
