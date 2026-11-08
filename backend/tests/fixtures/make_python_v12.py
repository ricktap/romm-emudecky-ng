"""Regenerate python_v12.db — a database created by the PYTHON plugin's migration runner.

The Rust integration tests start the native backend against this file to prove it
opens and continues a database the Python plugin wrote. Run from the repo root:

    .venv/bin/python backend/tests/fixtures/make_python_v12.py
"""

import os
import sqlite3
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
sys.path.insert(0, os.path.join(ROOT, "py_modules"))

from adapters.sqlite_migrations import MIGRATIONS_DIR, apply_migrations  # noqa: E402

out = os.path.join(os.path.dirname(os.path.abspath(__file__)), "python_v12.db")
for suffix in ("", "-wal", "-shm"):
    if os.path.exists(out + suffix):
        os.remove(out + suffix)
assert apply_migrations(out, MIGRATIONS_DIR) == 12
conn = sqlite3.connect(out)
conn.executescript(
    """
    INSERT INTO roms (rom_id, platform_slug, name, fs_name, last_synced_at, regions)
    VALUES (101, 'switch', 'Game A', 'a.nsp', '2026-11-01T00:00:00+00:00', '["USA", "Europe"]'),
           (102, 'switch', 'Game B', 'b.nsp', '2026-11-01T00:00:00+00:00', '["Japan"]'),
           (103, 'switch', 'Game C', 'c.xci', '2026-11-01T00:00:00+00:00', '[]');
    INSERT INTO kv_config (key, value) VALUES ('device_id', 'dev-from-python'),
                                              ('playtime_scope_notice', '1');
    """
)
conn.commit()
conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
conn.execute("PRAGMA journal_mode=DELETE")
conn.close()

for suffix in ("-wal", "-shm"):
    if os.path.exists(out + suffix):
        os.remove(out + suffix)

# settings.json exactly as the Python PersistenceAdapter writes it (indent=2,
# ensure_ascii, version stamped), for a signed-in install with a minted token.
import logging  # noqa: E402

from adapters.persistence import DEFAULT_SETTINGS, PersistenceAdapter  # noqa: E402

settings_dir = os.path.join(os.path.dirname(out), "python_settings")
os.makedirs(settings_dir, exist_ok=True)
settings = dict(DEFAULT_SETTINGS)
settings.update(
    {
        "romm_url": "http://romm.example:8080",
        "romm_api_token": "rmm_python_token",
        "romm_api_token_id": 7,
        "romm_api_token_origin": "http://romm.example:8080",
        "romm_api_token_source": "minted",
        "device_name": "Deck été",
        "enabled_platforms": {"4": True},
        "steamgriddb_api_key": "sgdb-key",
        "version": 11,
        "some_future_key": {"kept": True},
    }
)
PersistenceAdapter(settings_dir, settings_dir, logging.getLogger("fixture")).save_settings(settings)
os.remove(os.path.join(settings_dir, "settings.json.lock"))
print(os.path.join(settings_dir, "settings.json"))
