"""Tests for domain.self_update — version compare and release validation."""

from __future__ import annotations

import pytest

from domain.self_update import (
    GITHUB_REPO,
    LATEST_RELEASE_API_URL,
    RELEASE_ASSET_NAME,
    RELEASE_DOWNLOAD_PREFIX,
    RELEASES_PAGE_URL,
    is_newer,
    normalize_version,
    parse_release,
    parse_version,
    release_version,
)

SHA = "0123456789abcdef" * 4


def _release(**overrides):
    data = {
        "tag_name": "v1.2.3",
        "body": "## Changes\n- fix",
        "html_url": "https://github.com/ricktap/romm-emudecky-ng/releases/tag/v1.2.3",
        "draft": False,
        "prerelease": False,
        "assets": [
            {"name": "other.txt", "browser_download_url": "https://example.com/x", "digest": None},
            {
                "name": "romm-emudecky-ng.zip",
                "browser_download_url": "https://github.com/ricktap/romm-emudecky-ng/releases/download/v1.2.3/romm-emudecky-ng.zip",
                "digest": f"sha256:{SHA}",
            },
        ],
    }
    data.update(overrides)
    return data


def _with_asset(**asset_overrides):
    data = _release()
    data["assets"][1].update(asset_overrides)
    return data


class TestConstants:
    def test_everything_derives_from_one_repo_slug(self):
        assert GITHUB_REPO == "ricktap/romm-emudecky-ng"
        assert LATEST_RELEASE_API_URL == "https://api.github.com/repos/ricktap/romm-emudecky-ng/releases/latest"
        assert RELEASE_DOWNLOAD_PREFIX == "https://github.com/ricktap/romm-emudecky-ng/releases/download/"
        assert RELEASES_PAGE_URL == "https://github.com/ricktap/romm-emudecky-ng/releases"
        assert RELEASE_ASSET_NAME == "romm-emudecky-ng.zip"


class TestParseVersion:
    @pytest.mark.parametrize(
        ("value", "expected"),
        [("1.2.3", (1, 2, 3)), ("v0.32.0", (0, 32, 0)), ("V10.0.1", None), (" v1.0.0 ", (1, 0, 0))],
    )
    def test_plain_versions(self, value, expected):
        assert parse_version(value) == expected

    @pytest.mark.parametrize("value", ["v1.2.3-rc1", "1.2.3+build", "1.2", "1.2.3.4", "", "latest", None, 123, ["1"]])
    def test_rejects_prerelease_and_garbage(self, value):
        assert parse_version(value) is None


class TestIsNewer:
    @pytest.mark.parametrize(
        ("latest", "current", "expected"),
        [
            ("v0.33.0", "0.32.0", True),
            ("0.32.1", "0.32.0", True),
            ("1.0.0", "0.99.99", True),
            ("0.10.0", "0.9.0", True),  # numeric, not lexical
            ("0.32.0", "0.32.0", False),
            ("v0.31.9", "0.32.0", False),
            ("v0.33.0-rc1", "0.32.0", False),  # pre-release never offered
            ("garbage", "0.32.0", False),
            ("0.0.1", "not-a-version", True),  # broken local version → treated as 0.0.0
        ],
    )
    def test_compare(self, latest, current, expected):
        assert is_newer(latest, current) is expected


def test_normalize_version_strips_leading_v():
    assert normalize_version("v1.2.3") == "1.2.3"
    assert normalize_version("1.2.3") == "1.2.3"


class TestReleaseVersion:
    def test_reads_tag(self):
        assert release_version(_release()) == "1.2.3"

    @pytest.mark.parametrize("data", [None, [], {}, {"tag_name": "v1.2.3-beta"}, {"tag_name": 5}])
    def test_none_for_unusable(self, data):
        assert release_version(data) is None


class TestParseRelease:
    def test_valid_release(self):
        info = parse_release(_release())
        assert info is not None
        assert info.version == "1.2.3"
        assert info.notes == "## Changes\n- fix"
        assert info.asset_url.endswith("/releases/download/v1.2.3/romm-emudecky-ng.zip")
        assert info.sha256 == SHA
        assert info.html_url == "https://github.com/ricktap/romm-emudecky-ng/releases/tag/v1.2.3"

    def test_uppercase_digest_is_lowercased(self):
        info = parse_release(_with_asset(digest=f"sha256:{SHA.upper()}"))
        assert info is not None
        assert info.sha256 == SHA

    def test_missing_body_becomes_empty_notes(self):
        info = parse_release(_release(body=None))
        assert info is not None
        assert info.notes == ""

    def test_untrusted_html_url_falls_back_to_releases_page(self):
        info = parse_release(_release(html_url="https://evil.example/release"))
        assert info is not None
        assert info.html_url == RELEASES_PAGE_URL

    @pytest.mark.parametrize(
        "data",
        [
            None,
            "not a dict",
            [],
            _release(tag_name="v1.2.3-rc1"),
            _release(tag_name=None),
            _release(draft=True),
            _release(prerelease=True),
            _release(assets=None),
            _release(assets="nope"),
            _release(assets=[]),
            _release(assets=[{"name": "romm-emudecky-ng.tar.gz"}]),
        ],
    )
    def test_malformed_release_is_not_usable(self, data):
        assert parse_release(data) is None

    @pytest.mark.parametrize(
        "url",
        [
            None,
            "",
            "http://github.com/ricktap/romm-emudecky-ng/releases/download/v1.2.3/romm-emudecky-ng.zip",
            "https://github.com/someone-else/romm-emudecky-ng/releases/download/v1.2.3/romm-emudecky-ng.zip",
            "https://github.com/ricktap/romm-emudecky-ng-evil/releases/download/v1.2.3/romm-emudecky-ng.zip",
            "https://evil.example/ricktap/romm-emudecky-ng/releases/download/v1.2.3/romm-emudecky-ng.zip",
            "https://github.com/ricktap/romm-emudecky-ng/releases/download/../../../evil/x.zip",
        ],
    )
    def test_untrusted_asset_url_is_rejected(self, url):
        assert parse_release(_with_asset(browser_download_url=url)) is None

    @pytest.mark.parametrize(
        "digest",
        [None, "", SHA, f"sha1:{SHA[:40]}", f"sha256:{SHA[:63]}", f"sha256:{SHA}0", f"sha256:{'z' * 64}", 42],
    )
    def test_missing_or_malformed_digest_is_rejected(self, digest):
        assert parse_release(_with_asset(digest=digest)) is None
