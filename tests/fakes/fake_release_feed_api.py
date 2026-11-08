"""In-memory ``ReleaseFeedApi`` for service and contract tests."""

from __future__ import annotations

from typing import Any

from lib.errors import ReleaseFeedError

VALID_SHA256 = "ab" * 32


def make_release(
    tag: str = "v9.9.9",
    *,
    body: str = "Release notes",
    asset_name: str = "romm-emudecky-ng.zip",
    url: str | None = None,
    digest: str | None = None,
) -> dict[str, Any]:
    """Build a GitHub ``releases/latest`` payload shaped like the real API."""
    return {
        "tag_name": tag,
        "body": body,
        "html_url": f"https://github.com/ricktap/romm-emudecky-ng/releases/tag/{tag}",
        "draft": False,
        "prerelease": False,
        "assets": [
            {
                "name": asset_name,
                "browser_download_url": url
                or f"https://github.com/ricktap/romm-emudecky-ng/releases/download/{tag}/{asset_name}",
                "digest": digest if digest is not None else f"sha256:{VALID_SHA256}",
            }
        ],
    }


class FakeReleaseFeedApi:
    """Returns a configured release payload or raises a configured error.

    ``fetch_count`` records how many times the feed was hit so tests can
    assert throttling.
    """

    def __init__(self, release: dict[str, Any] | None = None) -> None:
        self.release: dict[str, Any] = release if release is not None else make_release()
        self.error: ReleaseFeedError | Exception | None = None
        self.fetch_count = 0

    def fetch_latest_release(self) -> dict[str, Any]:
        self.fetch_count += 1
        if self.error is not None:
            raise self.error
        return self.release
