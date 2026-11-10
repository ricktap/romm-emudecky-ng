-- =============================================================================
-- 013_download_archive_identity.sql — archive validators for multipart downloads
-- =============================================================================
--
-- RomM serves a multipart ROM as a cached ZIP when the request carries a Range
-- header, and as a differently-laid-out streamed ZIP otherwise. A partial may
-- only be appended to when it provably belongs to one cached representation, so
-- the job now stores that representation's identity next to the partial:
--
--   etag                  exact ETag header value (quotes and any W/ preserved)
--   last_modified         Last-Modified header, when supplied
--   representation_length complete length of the representation, in bytes
--   cached_zip            1 when the partial came from the cached-ZIP request path
--   needs_recovery        1 when the partial's origin is unknown (made before this
--                         migration): it is kept untouched until the user restarts
--
-- Existing multipart jobs were streamed (or half-streamed) by older versions, so
-- they are flagged needs_recovery: no validator is invented for bytes we cannot
-- prove belong to it, and no user progress is discarded by the migration.
-- Single-file jobs are unaffected.
-- -----------------------------------------------------------------------------
ALTER TABLE download_jobs ADD COLUMN etag TEXT;
ALTER TABLE download_jobs ADD COLUMN last_modified TEXT;
ALTER TABLE download_jobs ADD COLUMN representation_length INTEGER;
ALTER TABLE download_jobs ADD COLUMN cached_zip INTEGER NOT NULL DEFAULT 0 CHECK (cached_zip IN (0, 1));
ALTER TABLE download_jobs ADD COLUMN needs_recovery INTEGER NOT NULL DEFAULT 0 CHECK (needs_recovery IN (0, 1));

UPDATE download_jobs SET needs_recovery = 1 WHERE is_multi = 1;
