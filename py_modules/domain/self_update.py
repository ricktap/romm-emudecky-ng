"""Self-update decisions: which GitHub release is a usable, newer plugin build.

Pure compute over the GitHub "latest release" JSON and version strings. The
release source (repo slug, asset name, trusted download prefix) is pinned here
so the adapter that fetches the release and the validation that accepts it can
never disagree about where an update may come from.
"""

from __future__ import annotations

import re
from dataclasses import dataclass
from typing import Any

# The one place the release repository is named. Everything else derives from it.
GITHUB_REPO = "ricktap/romm-emudecky-ng"
LATEST_RELEASE_API_URL = f"https://api.github.com/repos/{GITHUB_REPO}/releases/latest"
RELEASES_PAGE_URL = f"https://github.com/{GITHUB_REPO}/releases"
RELEASE_DOWNLOAD_PREFIX = f"{RELEASES_PAGE_URL}/download/"
RELEASE_ASSET_NAME = "romm-emudecky-ng.zip"

_VERSION_RE = re.compile(r"^v?(\d+)\.(\d+)\.(\d+)$")
_DIGEST_RE = re.compile(r"^sha256:([0-9a-fA-F]{64})$")


@dataclass(frozen=True)
class ReleaseInfo:
    """A validated, installable release."""

    version: str
    notes: str
    asset_url: str
    sha256: str
    html_url: str


def parse_version(value: object) -> tuple[int, int, int] | None:
    """Parse a plain ``X.Y.Z`` (optional leading ``v``) into a tuple.

    Pre-release / build suffixes (``v1.2.3-rc1``, ``1.2.3+abc``) and any
    non-string input return ``None`` — such versions are never offered as
    updates.
    """
    if not isinstance(value, str):
        return None
    match = _VERSION_RE.match(value.strip())
    if match is None:
        return None
    return int(match.group(1)), int(match.group(2)), int(match.group(3))


def is_newer(latest: object, current: object) -> bool:
    """True when *latest* is a plain release strictly newer than *current*.

    An unparseable *latest* (including pre-releases) is never newer. An
    unparseable *current* is treated as ``0.0.0`` so a broken local version
    string still lets a valid release through.
    """
    latest_v = parse_version(latest)
    if latest_v is None:
        return False
    current_v = parse_version(current) or (0, 0, 0)
    return latest_v > current_v


def normalize_version(value: str) -> str:
    """Strip a leading ``v`` from a version tag (``"v1.2.3"`` → ``"1.2.3"``)."""
    stripped = value.strip()
    return stripped[1:] if stripped[:1] in ("v", "V") else stripped


def release_version(data: object) -> str | None:
    """The release's plain version (``"1.2.3"``) from ``tag_name``, or ``None``."""
    if not isinstance(data, dict):
        return None
    tag = data.get("tag_name")
    if not isinstance(tag, str) or parse_version(tag) is None:
        return None
    return normalize_version(tag)


def parse_release(data: object) -> ReleaseInfo | None:
    """Validate a GitHub release JSON object into a :class:`ReleaseInfo`.

    Returns ``None`` (no usable update) unless all of these hold:

    * ``tag_name`` is a plain ``X.Y.Z`` / ``vX.Y.Z`` version (no pre-release);
    * an asset named :data:`RELEASE_ASSET_NAME` exists;
    * its ``browser_download_url`` starts with :data:`RELEASE_DOWNLOAD_PREFIX`
      (and has no ``..`` segment);
    * its ``digest`` is ``sha256:<64 hex>``.

    Drafts and releases flagged ``prerelease`` are rejected as well.
    """
    if not isinstance(data, dict):
        return None
    if data.get("draft") is True or data.get("prerelease") is True:
        return None
    tag = release_version(data)
    if tag is None:
        return None

    assets = data.get("assets")
    if not isinstance(assets, list):
        return None
    asset = next(
        (a for a in assets if isinstance(a, dict) and a.get("name") == RELEASE_ASSET_NAME),
        None,
    )
    if asset is None:
        return None

    url = asset.get("browser_download_url")
    if not isinstance(url, str) or not url.startswith(RELEASE_DOWNLOAD_PREFIX) or ".." in url:
        return None
    digest = asset.get("digest")
    if not isinstance(digest, str):
        return None
    digest_match = _DIGEST_RE.match(digest)
    if digest_match is None:
        return None

    notes = data.get("body")
    html_url = data.get("html_url")
    return ReleaseInfo(
        version=tag,
        notes=notes if isinstance(notes, str) else "",
        asset_url=url,
        sha256=digest_match.group(1).lower(),
        html_url=_safe_html_url(html_url),
    )


def _safe_html_url(value: Any) -> str:
    """Accept only a release page on our repo; else fall back to the releases list."""
    fallback = RELEASES_PAGE_URL
    if isinstance(value, str) and value.startswith(f"https://github.com/{GITHUB_REPO}/"):
        return value
    return fallback
