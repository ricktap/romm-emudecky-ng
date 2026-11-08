"""Matching hand-copied games to RomM ROMs by normalized title.

The fallback adoption rule for systems whose games arrive under free-form
names (Switch dumps keep scene names, title-id tags and version tags, while
RomM sanitizes its folder names): a disk entry and a RomM ROM belong together
only when their titles are exactly equal after normalization, and only when
that pairing is one-to-one. Anything that decides what a title normalizes to,
whether a disk entry can be launched, or which entry pairs with which ROM
belongs here; reading the folder is the caller's job.

Pure compute, stdlib only. Sizes are used only to pick a folder's launch file
(largest wins, the same rule as a downloaded folder game) — never to match.
"""

from __future__ import annotations

import os
import re
import unicodedata
from collections.abc import Iterable
from dataclasses import dataclass

from domain.disc_selection import enumerate_size_ranked

# Container extensions dropped from the end of a name before comparing.
_TITLE_EXTENSIONS = (".xci", ".nsp", ".nsz", ".xcz")
_BRACKET_GROUPS = re.compile(r"\[[^\]]*\]|\([^)]*\)")
_SCENE_SUFFIX = re.compile(r"\bnsw-[a-z0-9]+$", re.IGNORECASE)
_NON_WORD = re.compile(r"[\W_]+")


def normalize_title(name: str) -> str:
    """Reduce a ROM or file/folder name to a comparable title key.

    In order: Unicode NFKD with combining marks removed (``Pokémon`` →
    ``Pokemon``), lowercase, drop one trailing ``.xci``/``.nsp``/``.nsz``/
    ``.xcz``, drop every ``[...]`` and ``(...)`` group, drop a trailing
    ``NSW-<group>`` scene suffix, turn every run of non-alphanumeric
    characters (``_`` ``:`` ``-`` ``+`` ``'`` ``.`` ``,`` ``&`` …) into one
    space, and strip. An empty result means "no title" and never matches.
    """
    decomposed = unicodedata.normalize("NFKD", name)
    text = "".join(ch for ch in decomposed if not unicodedata.combining(ch)).lower().strip()
    for ext in _TITLE_EXTENSIONS:
        if text.endswith(ext):
            text = text[: -len(ext)]
            break
    text = _BRACKET_GROUPS.sub(" ", text).strip()
    text = _SCENE_SUFFIX.sub(" ", text)
    return " ".join(_NON_WORD.sub(" ", text).split())


@dataclass(frozen=True)
class DiskEntry:
    """One top-level entry of a system folder.

    ``files`` are the ``(name, size)`` pairs of the regular files directly
    inside a directory entry (top level only); for a file entry it is the
    entry itself. ``claimed`` marks an entry an existing install already
    points at: it still counts towards ambiguity but is never adopted.
    """

    name: str
    is_dir: bool
    files: tuple[tuple[str, int], ...] = ()
    claimed: bool = False


@dataclass(frozen=True)
class TitleMatch:
    """A RomM ROM paired with a disk entry by normalized title.

    ``launch_file`` is the file to launch: the entry itself for a file entry,
    or the name of the largest launchable file inside a directory entry.
    """

    rom_id: int
    entry_name: str
    is_dir: bool
    launch_file: str


def entry_launch_file(entry: DiskEntry, extensions: frozenset[str]) -> str | None:
    """The launch file of *entry*, or ``None`` when it does not qualify.

    A file entry qualifies when its extension (case-insensitive) is in
    *extensions*; a directory entry when at least one of its top-level files
    does, the largest of those being the launch file (ties by lowercase name).
    """
    if not entry.is_dir:
        return entry.name if os.path.splitext(entry.name)[1].lower() in extensions else None
    ranked = enumerate_size_ranked(list(entry.files), extensions)
    return ranked[0].filename if ranked else None


def pair_by_title(
    roms: Iterable[tuple[int, str]],
    entries: Iterable[DiskEntry],
    extensions: frozenset[str],
) -> list[TitleMatch]:
    """Pair RomM ROMs with qualifying disk entries whose normalized titles are equal.

    *roms* are ``(rom_id, fs_name)`` for every RomM ROM of the system that
    could claim a title (not only the ones being adopted), so a title two
    ROMs share is recognised as ambiguous. A title pairs only when exactly one
    ROM and exactly one qualifying entry carry it; any ambiguity on either
    side adopts nothing for that title. Empty titles never pair; claimed
    entries count for ambiguity but are never returned. Sorted by ``rom_id``.
    """
    rom_ids_by_title: dict[str, set[int]] = {}
    for rom_id, fs_name in roms:
        title = normalize_title(fs_name)
        if title:
            rom_ids_by_title.setdefault(title, set()).add(rom_id)
    entries_by_title: dict[str, list[tuple[DiskEntry, str]]] = {}
    for entry in entries:
        title = normalize_title(entry.name)
        if not title or title not in rom_ids_by_title:
            continue
        launch = entry_launch_file(entry, extensions)
        if launch is not None:
            entries_by_title.setdefault(title, []).append((entry, launch))
    matches: list[TitleMatch] = []
    for title, found in entries_by_title.items():
        rom_ids = rom_ids_by_title[title]
        if len(rom_ids) != 1 or len(found) != 1:
            continue
        entry, launch = found[0]
        if entry.claimed:
            continue
        matches.append(
            TitleMatch(rom_id=next(iter(rom_ids)), entry_name=entry.name, is_dir=entry.is_dir, launch_file=launch)
        )
    return sorted(matches, key=lambda m: m.rom_id)
