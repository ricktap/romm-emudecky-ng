"""In-memory ``InstallAdopter`` for library-sync tests.

Adopts nothing by default; a test stages ``candidates`` to have
``plan_adoptions`` return them, and inspects ``planned_with`` /
``committed`` to assert the orchestrator's calls.
"""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    from domain.install_layout import AdoptionCandidate


class FakeInstallAdopter:
    def __init__(self, candidates: list[AdoptionCandidate] | None = None) -> None:
        self.candidates: list[AdoptionCandidate] = list(candidates or [])
        self.planned_with: list[list[dict[str, Any]]] = []
        self.committed: list[list[AdoptionCandidate]] = []

    def plan_adoptions(self, roms: list[dict[str, Any]]) -> list[AdoptionCandidate]:
        self.planned_with.append([dict(r) for r in roms])
        ids = {int(r["id"]) for r in roms}
        return [c for c in self.candidates if c.rom_id in ids]

    def commit_adoptions(self, candidates: list[AdoptionCandidate]) -> list[int]:
        self.committed.append(list(candidates))
        return [c.rom_id for c in candidates]
