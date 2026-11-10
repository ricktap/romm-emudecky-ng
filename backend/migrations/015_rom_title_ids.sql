-- 015_rom_title_ids.sql: the Switch title ID a ROM's saves belong to (ADR-0023).
--
-- Saves of Switch emulators are keyed by title ID, not by file name. The ID comes from a `[0100XXXXXXXXXXXX]` tag in the
-- ROM's file name, from RomM, or from the user's confirmation after a play session (`source`); a row exists only once a
-- title is known. Never guessed: a ROM without a row (and without a tag) is unsupported for save sync.
CREATE TABLE rom_title_ids (
    rom_id   INTEGER PRIMARY KEY REFERENCES roms(rom_id) ON DELETE CASCADE,
    title_id TEXT NOT NULL CHECK (length(title_id) = 16),   -- 16 upper-case hex digits, base title
    source   TEXT NOT NULL CHECK (source IN ('file_name', 'romm', 'learned', 'manual'))
) STRICT;
