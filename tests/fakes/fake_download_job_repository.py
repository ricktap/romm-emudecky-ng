"""In-memory ``DownloadJobRepository`` implementation for service tests."""

from __future__ import annotations

import copy
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from collections.abc import Iterator

    from domain.download_job import DownloadJob


class FakeDownloadJobRepository:
    """Dict-backed ``DownloadJobRepository`` keyed by ``rom_id``."""

    def __init__(self) -> None:
        self._jobs: dict[int, DownloadJob] = {}
        self.save_count = 0

    def get(self, rom_id: int) -> DownloadJob | None:
        return copy.deepcopy(self._jobs.get(rom_id))

    def save(self, job: DownloadJob) -> None:
        self.save_count += 1
        self._jobs[job.rom_id] = copy.deepcopy(job)

    def delete(self, rom_id: int) -> None:
        self._jobs.pop(rom_id, None)

    def iter_all(self) -> Iterator[DownloadJob]:
        ordered = sorted(self._jobs.values(), key=lambda j: (j.position, j.rom_id))
        return iter([copy.deepcopy(j) for j in ordered])

    def next_position(self) -> int:
        return max((j.position for j in self._jobs.values()), default=0) + 1

    def min_position(self) -> int | None:
        return min((j.position for j in self._jobs.values()), default=None)

    def _snapshot(self) -> dict[int, DownloadJob]:
        return copy.deepcopy(self._jobs)

    def _restore(self, state: dict[int, DownloadJob]) -> None:
        self._jobs = state
