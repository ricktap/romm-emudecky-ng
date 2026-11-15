-- =============================================================================
-- 016_download_rom_files.sql — single files of an installed multi-file game
-- =============================================================================
--
-- The queue also carries single files of an installed multi-file ROM (an update
-- or DLC added to a Switch game folder on RomM), kind = 'rom_file', keyed by the
-- RomM file id. Such a job needs its game to resume after a restart, so the
-- table gains parent_rom_id (NULL for 'rom' and 'firmware' jobs).
--
-- SQLite cannot change a CHECK constraint in place, so the table is rebuilt as
-- in 014: create the new shape, copy every row unchanged, drop, rename, index.
-- -----------------------------------------------------------------------------
CREATE TABLE download_jobs_v3 (
    kind                  TEXT    NOT NULL DEFAULT 'rom' CHECK (kind IN ('rom', 'firmware', 'rom_file')),
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
    parent_rom_id         INTEGER,
    PRIMARY KEY (kind, item_id),
    CHECK (kind != 'rom_file' OR parent_rom_id IS NOT NULL)
) STRICT;

INSERT INTO download_jobs_v3 (
    kind, item_id, rom_name, platform_name, file_name, target_path, tmp_path, is_multi, total_bytes,
    status, interrupt_reason, range_supported, position, created_at, updated_at,
    etag, last_modified, representation_length, cached_zip, needs_recovery
)
SELECT
    kind, item_id, rom_name, platform_name, file_name, target_path, tmp_path, is_multi, total_bytes,
    status, interrupt_reason, range_supported, position, created_at, updated_at,
    etag, last_modified, representation_length, cached_zip, needs_recovery
FROM download_jobs;

DROP TABLE download_jobs;
ALTER TABLE download_jobs_v3 RENAME TO download_jobs;
CREATE INDEX idx_download_jobs_position ON download_jobs (position);
