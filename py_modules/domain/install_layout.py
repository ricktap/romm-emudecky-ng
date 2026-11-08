"""Where a ROM's files land on disk, and whether files already there match RomM.

The single source of truth for the on-disk names the download path writes
(the target file name of a single-file ROM, the extract-directory name of a
multi-file ROM) and for the "is this ROM already on disk?" decision that
adoption of pre-existing files makes. Both the downloader and adoption derive
their paths from :func:`resolve_download_names` so the two can never diverge.

Pure compute: no filesystem access. The caller joins the returned single safe
components onto the (realpath-validated) platform directory and reports the
observed sizes back for the match decision. The name coercion it reuses
(``coerce_safe_component`` / ``safe_path_component``) is lexical only.
"""

from __future__ import annotations

import posixpath
import urllib.parse
from collections.abc import Mapping
from dataclasses import dataclass
from typing import Any

from lib.path_safety import PathTraversalError, coerce_safe_component, safe_path_component

from domain.rom_files import (
    is_multi_file_download,
    resolve_extract_dir_name,
    resolve_local_file_name,
    synthetic_rom_name,
)
from domain.rom_install import RomInstall


@dataclass(frozen=True)
class DownloadNames:
    """The sanitized on-disk names the download path uses for one ROM.

    ``file_name`` is the single safe component the transfer is written to
    (``<platform dir>/<file_name>``; for a multi-file ROM it is only the
    temporary archive's stem). ``extract_dir_name`` is the single safe
    component of a multi-file ROM's extract directory (computed for every ROM;
    only a multi-file ROM uses it). The ``raw_*`` fields carry the server-supplied value
    before coercion so a caller can log the sanitization once.
    """

    file_name: str
    raw_file_name: str
    files_missing: bool
    is_multi: bool
    extract_dir_name: str
    raw_extract_dir_name: str

    @property
    def file_name_changed(self) -> bool:
        return self.file_name != self.raw_file_name

    @property
    def extract_dir_name_changed(self) -> bool:
        return self.extract_dir_name != self.raw_extract_dir_name


def resolve_download_names(rom_detail: dict[str, Any]) -> DownloadNames:
    """Resolve the sanitized target names for *rom_detail* (download + adoption).

    The file name comes from :func:`resolve_local_file_name` (``fs_name``, or
    ``files[0].file_name`` for a nested-single-file ROM) and the extract dir
    from :func:`resolve_extract_dir_name`; each is coerced to a single safe
    path component, with the synthetic ``rom_<id>`` identity substituted for a
    degenerate value (``..``/``.``/empty).
    """
    fallback = synthetic_rom_name(rom_detail)
    raw_file_name, files_missing = resolve_local_file_name(rom_detail)
    file_name, _ = coerce_safe_component(raw_file_name, fallback)
    is_multi = is_multi_file_download(rom_detail)
    raw_extract = resolve_extract_dir_name(rom_detail)
    extract_dir_name, _ = coerce_safe_component(raw_extract, fallback)
    return DownloadNames(
        file_name=file_name,
        raw_file_name=raw_file_name,
        files_missing=files_missing,
        is_multi=is_multi,
        extract_dir_name=extract_dir_name,
        raw_extract_dir_name=raw_extract,
    )


@dataclass(frozen=True)
class SingleFileLayout:
    """A single-file ROM: one file named ``file_name`` of ``size`` bytes in the platform dir."""

    file_name: str
    size: int


@dataclass(frozen=True)
class ExpectedFile:
    """One file of a multi-file ROM: ``rel_path`` (``/``-separated) inside the install dir."""

    rel_path: str
    size: int

    @property
    def parts(self) -> tuple[str, ...]:
        return tuple(self.rel_path.split("/"))


@dataclass(frozen=True)
class MultiFileLayout:
    """A multi-file ROM: every ``files`` entry inside one of ``dir_candidates``.

    ``dir_candidates`` are single safe components under the platform dir, the
    extract (staging) dir name first. The download path renames that dir after
    its top-level launch file for ES-DE's directory collapse (an auto-generated
    ``<fs_name_no_ext>.m3u`` or any top-level RomM file can be the launch
    file), so each such name is a candidate too. A candidate only matches when
    *every* expected file is inside it with the exact size, so the extra names
    cannot produce a false positive.
    """

    dir_candidates: tuple[str, ...]
    files: tuple[ExpectedFile, ...]


def _rom_root(rom_detail: dict[str, Any]) -> str | None:
    fs_path = rom_detail.get("fs_path")
    fs_name = rom_detail.get("fs_name")
    if not isinstance(fs_path, str) or not isinstance(fs_name, str) or not fs_path or not fs_name:
        return None
    return posixpath.join(fs_path, fs_name)


def _relative_file_path(entry: dict[str, Any], rom_root: str) -> str | None:
    """Path of one RomM file entry relative to the ROM's own folder, as the download extracts it.

    RomM builds the multi-file archive with each member named relative to the
    ROM folder (``<file_path>/<file_name>`` minus ``<fs_path>/<fs_name>/``);
    the downloader then URL-decodes every extracted name. Each component is
    decoded the same way and must be a single safe component — a traversal
    (``..``, absolute, NUL) makes the whole layout undeterminable.
    """
    file_path = entry.get("file_path")
    file_name = entry.get("file_name")
    if not isinstance(file_path, str) or not isinstance(file_name, str) or not file_name:
        return None
    full = posixpath.join(file_path, file_name) if file_path else file_name
    prefix = rom_root.rstrip("/") + "/"
    if not full.startswith(prefix):
        return None
    parts = full[len(prefix) :].split("/")
    decoded: list[str] = []
    try:
        for part in parts:
            decoded.append(safe_path_component(urllib.parse.unquote(part)))
    except PathTraversalError:
        return None
    return "/".join(decoded)


def _entry_size(entry: dict[str, Any]) -> int | None:
    size = entry.get("file_size_bytes")
    if isinstance(size, bool) or not isinstance(size, int) or size < 0:
        return None
    return size


def _multi_file_layout(rom_detail: dict[str, Any], names: DownloadNames) -> MultiFileLayout | None:
    files = rom_detail.get("files") or []
    rom_root = _rom_root(rom_detail)
    if not files or rom_root is None:
        return None
    expected: list[ExpectedFile] = []
    for entry in files:
        if not isinstance(entry, dict):
            return None
        rel = _relative_file_path(entry, rom_root)
        size = _entry_size(entry)
        if rel is None or size is None:
            return None
        expected.append(ExpectedFile(rel_path=rel, size=size))
    candidates: list[str] = [names.extract_dir_name]
    # ES-DE collapse renames the dir after a top-level launch file: any
    # top-level RomM file, or the auto-generated game-named playlist.
    top_level = [f.rel_path for f in expected if "/" not in f.rel_path]
    m3u_stem = rom_detail.get("fs_name_no_ext", rom_detail.get("name", "playlist"))
    if isinstance(m3u_stem, str) and m3u_stem:
        top_level.append(f"{m3u_stem}.m3u")
    for name in top_level:
        safe, changed = coerce_safe_component(name, "")
        if not changed and safe and safe not in candidates:
            candidates.append(safe)
    return MultiFileLayout(dir_candidates=tuple(candidates), files=tuple(expected))


def expected_install_layout(rom_detail: dict[str, Any]) -> SingleFileLayout | MultiFileLayout | None:
    """Where the downloader would put *rom_detail*'s files, or ``None`` if not determinable.

    Single-file: ``<platform dir>/<file_name>`` with RomM's ``fs_size_bytes``;
    ``None`` when RomM reports no positive size. Multi-file: every ``files``
    entry at its archive-relative path inside the install dir; ``None`` when
    any entry lacks a size, the ROM's own folder (``fs_path``/``fs_name``) is
    unknown, an entry lies outside that folder, or a name is unsafe.
    """
    names = resolve_download_names(rom_detail)
    if names.is_multi:
        return _multi_file_layout(rom_detail, names)
    size = rom_detail.get("fs_size_bytes")
    if isinstance(size, bool) or not isinstance(size, int) or size <= 0:
        return None
    return SingleFileLayout(file_name=names.file_name, size=size)


def single_file_matches(layout: SingleFileLayout, observed_size: int | None) -> bool:
    """True when a regular file was observed at the target with exactly RomM's size."""
    return observed_size is not None and observed_size == layout.size


def multi_file_matches(layout: MultiFileLayout, observed: Mapping[str, int | None]) -> bool:
    """True when every expected file was observed (by ``rel_path``) with its exact size."""
    return all(observed.get(f.rel_path) == f.size for f in layout.files)


@dataclass(frozen=True)
class AdoptionCandidate:
    """A ROM whose files were found on disk exactly where a download would put them.

    ``install`` is the ``RomInstall`` a finished download would have written;
    ``bake_path`` is the disc-resolved launch path for the shortcut's launch
    command; ``group_key`` is the ROM's sibling-group key (``None`` when solo).
    """

    rom_id: int
    install: RomInstall
    bake_path: str
    group_key: str | None


def pick_one_per_group(candidates: list[AdoptionCandidate]) -> list[AdoptionCandidate]:
    """Keep at most one candidate per sibling group (lowest ``rom_id`` wins).

    A sibling group holds at most one installed version; when several versions
    of one game are on disk, the lowest RomM id is adopted and the rest stay
    untouched (never deleted). Candidates with no group key are all kept.
    """
    chosen: dict[str, AdoptionCandidate] = {}
    solo: list[AdoptionCandidate] = []
    for cand in sorted(candidates, key=lambda c: c.rom_id):
        if cand.group_key is None:
            solo.append(cand)
        elif cand.group_key not in chosen:
            chosen[cand.group_key] = cand
    return sorted([*solo, *chosen.values()], key=lambda c: c.rom_id)
