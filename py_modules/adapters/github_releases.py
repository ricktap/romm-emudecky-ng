"""GitHub release-feed adapter — reads the plugin's latest published release.

Owns the unauthenticated HTTP GET against the GitHub REST API. The repository
is pinned in :mod:`domain.self_update`; this adapter only does the I/O and
maps transport failures onto :class:`lib.errors.ReleaseFeedError`.
"""

from __future__ import annotations

import json
import ssl
import urllib.error
import urllib.request
from typing import TYPE_CHECKING, Any

from domain.self_update import LATEST_RELEASE_API_URL
from lib.certifi_bundle import ca_bundle as _ca_bundle
from lib.errors import ReleaseFeedError

if TYPE_CHECKING:
    import logging

_DEFAULT_TIMEOUT_S = 10.0


class GitHubReleasesAdapter:
    """Concrete ``ReleaseFeedApi`` backed by ``api.github.com``.

    Parameters
    ----------
    user_agent:
        Outgoing ``User-Agent`` (``"romm-emudecky-ng/<version>"``). GitHub
        rejects requests without one.
    logger:
        Logger instance.
    timeout:
        Socket timeout in seconds for the single request.
    """

    def __init__(self, *, user_agent: str, logger: logging.Logger, timeout: float = _DEFAULT_TIMEOUT_S) -> None:
        self._user_agent = user_agent
        self._logger = logger
        self._timeout = timeout

    def fetch_latest_release(self) -> dict[str, Any]:
        """GET the latest (non-draft, non-prerelease) release JSON.

        Raises :class:`ReleaseFeedError` with ``kind`` ``rate_limited`` on
        HTTP 403/429, ``unreachable`` on any other HTTP error or transport
        failure, and ``bad_response`` when the body is not a JSON object.
        """
        req = urllib.request.Request(LATEST_RELEASE_API_URL, method="GET")
        req.add_header("User-Agent", self._user_agent)
        req.add_header("Accept", "application/vnd.github+json")
        req.add_header("X-GitHub-Api-Version", "2022-11-28")
        ctx = ssl.create_default_context(cafile=_ca_bundle())
        try:
            with urllib.request.urlopen(req, context=ctx, timeout=self._timeout) as resp:
                raw = resp.read()
        except urllib.error.HTTPError as e:
            if e.code in (403, 429):
                raise ReleaseFeedError("rate_limited", f"GitHub rate limit reached (HTTP {e.code})", e.code) from e
            raise ReleaseFeedError("unreachable", f"GitHub returned HTTP {e.code}", e.code) from e
        except (urllib.error.URLError, OSError) as e:
            raise ReleaseFeedError("unreachable", f"Could not reach GitHub: {e}") from e
        try:
            data = json.loads(raw.decode("utf-8"))
        except (UnicodeDecodeError, json.JSONDecodeError) as e:
            raise ReleaseFeedError("bad_response", "GitHub returned an unreadable response") from e
        if not isinstance(data, dict):
            raise ReleaseFeedError("bad_response", "GitHub returned an unexpected response")
        return data
