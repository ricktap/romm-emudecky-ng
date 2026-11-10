-- =============================================================================
-- 014_download_job_kinds.sql — one durable queue for several kinds of download
-- =============================================================================
--
-- download_jobs was keyed by rom_id alone. The queue now also carries firmware
-- files (0.45.0), whose ids are a different number space, so a job is identified
-- by (kind, item_id). Every existing row is a ROM download and becomes
-- kind = 'rom' with item_id = its old rom_id; nothing else changes.
--
-- rom_name keeps its name for now but holds the display name of any kind (a
-- firmware file's name for kind = 'firmware').
--
-- SQLite cannot change a primary key in place, so the table is rebuilt:
-- create the new shape, copy, drop the old table (its position index goes with
-- it), rename, and recreate the index. Nothing references download_jobs.
-- -----------------------------------------------------------------------------
CREATE TABLE download_jobs_v2 (
    kind                  TEXT    NOT NULL DEFAULT 'rom' CHECK (kind IN ('rom', 'firmware')),
    item_id               INTEGER NOT NULL,
    rom_name              TEXT    NOT NULL,
    platform_name         TEXT    NOT NULL,
    file_name             TEXT    NOT NULL,
    target_path           TEXT    NOT NULL,
    tmp_path              TEXT    NOT NULL,
    is_multi              INTEGER NOT NULL CHECK (is_multi IN (0, 1)),
    total_bytes           INTEGER,
    status                TEXT    NOT NULL CHECK (status IN ('queued', 'downloading', 'extracting', 'paused', 'interrupted')),
    interrupt_reason      TEXT,
    range_supported       INTEGER CHECK (range_supported IS NULL OR range_supported IN (0, 1)),
    position              INTEGER NOT NULL,
    created_at            TEXT    NOT NULL,
    updated_at            TEXT    NOT NULL,
    etag                  TEXT,
    last_modified         TEXT,
    representation_length INTEGER,
    cached_zip            INTEGER NOT NULL DEFAULT 0 CHECK (cached_zip IN (0, 1)),
    needs_recovery        INTEGER NOT NULL DEFAULT 0 CHECK (needs_recovery IN (0, 1)),
    PRIMARY KEY (kind, item_id)
) STRICT;

INSERT INTO download_jobs_v2 (
    kind, item_id, rom_name, platform_name, file_name, target_path, tmp_path, is_multi, total_bytes,
    status, interrupt_reason, range_supported, position, created_at, updated_at,
    etag, last_modified, representation_length, cached_zip, needs_recovery
)
SELECT
    'rom', rom_id, rom_name, platform_name, file_name, target_path, tmp_path, is_multi, total_bytes,
    status, interrupt_reason, range_supported, position, created_at, updated_at,
    etag, last_modified, representation_length, cached_zip, needs_recovery
FROM download_jobs;

DROP TABLE download_jobs;
ALTER TABLE download_jobs_v2 RENAME TO download_jobs;
CREATE INDEX idx_download_jobs_position ON download_jobs (position);
