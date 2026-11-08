"""Tests for adapters.github_releases — the GitHub latest-release reader."""

from __future__ import annotations

import io
import logging
import urllib.error
from unittest.mock import MagicMock, patch

import pytest

from adapters.github_releases import GitHubReleasesAdapter
from domain.self_update import LATEST_RELEASE_API_URL
from lib.errors import ReleaseFeedError


def _adapter(**kwargs):
    return GitHubReleasesAdapter(user_agent="romm-emudecky-ng/9.9.9", logger=logging.getLogger("test"), **kwargs)


def _resp(body: bytes):
    mock_resp = MagicMock()
    mock_resp.read.return_value = body
    mock_resp.__enter__ = lambda s: s
    mock_resp.__exit__ = MagicMock(return_value=False)
    return mock_resp


def _http_error(code: int):
    return urllib.error.HTTPError(LATEST_RELEASE_API_URL, code, "err", {}, io.BytesIO(b""))  # type: ignore[arg-type]


class TestFetchLatestRelease:
    def test_returns_parsed_json(self):
        with patch("urllib.request.urlopen", return_value=_resp(b'{"tag_name": "v1.0.0"}')):
            assert _adapter().fetch_latest_release() == {"tag_name": "v1.0.0"}

    def test_requests_latest_release_url_with_user_agent(self):
        with patch("urllib.request.urlopen", return_value=_resp(b"{}")) as mock_open:
            _adapter().fetch_latest_release()
        req = mock_open.call_args[0][0]
        assert req.full_url == LATEST_RELEASE_API_URL
        assert req.get_method() == "GET"
        assert req.get_header("User-agent") == "romm-emudecky-ng/9.9.9"
        assert req.get_header("Accept") == "application/vnd.github+json"
        assert req.get_header("Authorization") is None

    def test_uses_short_default_timeout(self):
        with patch("urllib.request.urlopen", return_value=_resp(b"{}")) as mock_open:
            _adapter().fetch_latest_release()
        assert mock_open.call_args.kwargs["timeout"] == 10.0
        assert mock_open.call_args.kwargs["context"] is not None

    def test_timeout_is_configurable(self):
        with patch("urllib.request.urlopen", return_value=_resp(b"{}")) as mock_open:
            _adapter(timeout=3.0).fetch_latest_release()
        assert mock_open.call_args.kwargs["timeout"] == 3.0

    @pytest.mark.parametrize("code", [403, 429])
    def test_rate_limit_statuses(self, code):
        with patch("urllib.request.urlopen", side_effect=_http_error(code)), pytest.raises(ReleaseFeedError) as exc:
            _adapter().fetch_latest_release()
        assert exc.value.kind == "rate_limited"
        assert exc.value.status_code == code

    @pytest.mark.parametrize("code", [404, 500, 502])
    def test_other_http_errors_are_unreachable(self, code):
        with patch("urllib.request.urlopen", side_effect=_http_error(code)), pytest.raises(ReleaseFeedError) as exc:
            _adapter().fetch_latest_release()
        assert exc.value.kind == "unreachable"
        assert exc.value.status_code == code

    @pytest.mark.parametrize("error", [urllib.error.URLError("no route"), TimeoutError("timed out"), OSError("reset")])
    def test_transport_errors_are_unreachable(self, error):
        with patch("urllib.request.urlopen", side_effect=error), pytest.raises(ReleaseFeedError) as exc:
            _adapter().fetch_latest_release()
        assert exc.value.kind == "unreachable"
        assert exc.value.status_code is None

    @pytest.mark.parametrize("body", [b"<html>", b"\xff\xfe", b"[1, 2]", b'"str"'])
    def test_bad_body_is_bad_response(self, body):
        with patch("urllib.request.urlopen", return_value=_resp(body)), pytest.raises(ReleaseFeedError) as exc:
            _adapter().fetch_latest_release()
        assert exc.value.kind == "bad_response"
