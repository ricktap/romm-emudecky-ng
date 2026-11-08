"""SQLite adapter for the ``DownloadJob`` aggregate over the ``download_jobs`` table."""

from __future__ import annotations

from typing import TYPE_CHECKING

from adapters.repositories._base import BaseRepository
from domain.download_job import DownloadJob

if TYPE_CHECKING:
    import sqlite3
    from collections.abc import Iterator

_COLUMNS = (
    "rom_id, rom_name, platform_name, file_name, target_path, tmp_path, is_multi, total_bytes, "
    "status, interrupt_reason, range_supported, position, created_at, updated_at"
)


def _row_to_job(row: sqlite3.Row) -> DownloadJob:
    range_supported = row["range_supported"]
    return DownloadJob(
        rom_id=row["rom_id"],
        rom_name=row["rom_name"],
        platform_name=row["platform_name"],
        file_name=row["file_name"],
        target_path=row["target_path"],
        tmp_path=row["tmp_path"],
        is_multi=bool(row["is_multi"]),
        total_bytes=row["total_bytes"],
        status=row["status"],
        interrupt_reason=row["interrupt_reason"],
        range_supported=None if range_supported is None else bool(range_supported),
        position=row["position"],
        created_at=row["created_at"],
        updated_at=row["updated_at"],
    )


class SqliteDownloadJobRepository(BaseRepository):
    """Unfinished ROM downloads keyed by ``rom_id``, ordered by ``position``."""

    def get(self, rom_id: int) -> DownloadJob | None:
        row = self._conn.execute(f"SELECT {_COLUMNS} FROM download_jobs WHERE rom_id = ?", (rom_id,)).fetchone()
        return _row_to_job(row) if row is not None else None

    def save(self, job: DownloadJob) -> None:
        self._conn.execute(
            f"INSERT OR REPLACE INTO download_jobs ({_COLUMNS}) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            (
                job.rom_id,
                job.rom_name,
                job.platform_name,
                job.file_name,
                job.target_path,
                job.tmp_path,
                1 if job.is_multi else 0,
                job.total_bytes,
                job.status,
                job.interrupt_reason,
                None if job.range_supported is None else (1 if job.range_supported else 0),
                job.position,
                job.created_at,
                job.updated_at,
            ),
        )

    def delete(self, rom_id: int) -> None:
        self._conn.execute("DELETE FROM download_jobs WHERE rom_id = ?", (rom_id,))

    def iter_all(self) -> Iterator[DownloadJob]:
        for row in self._conn.execute(f"SELECT {_COLUMNS} FROM download_jobs ORDER BY position, rom_id"):
            yield _row_to_job(row)

    def next_position(self) -> int:
        row = self._conn.execute("SELECT MAX(position) FROM download_jobs").fetchone()
        return 1 if row[0] is None else int(row[0]) + 1

    def min_position(self) -> int | None:
        row = self._conn.execute("SELECT MIN(position) FROM download_jobs").fetchone()
        return None if row[0] is None else int(row[0])
