"""Tests for services.update — throttled, never-raising self-update check."""

from __future__ import annotations

import asyncio
import logging

import pytest
from fakes.fake_plugin_metadata_reader import FakePluginMetadataReader
from fakes.fake_release_feed_api import VALID_SHA256, FakeReleaseFeedApi, make_release
from fakes.system_time import FakeClock

from lib.errors import ReleaseFeedError
from services.update import CHECK_INTERVAL_S, FAILURE_RETRY_S, UpdateService, UpdateServiceConfig

_SUCCESS_KEYS = {
    "success",
    "current_version",
    "latest_version",
    "update_available",
    "notes",
    "asset_url",
    "sha256",
    "html_url",
    "plugin_name",
    "checked_at",
}


def _make(release=None, *, version="0.32.0", plugin_name="EmuDecky RomM NG"):
    feed = FakeReleaseFeedApi(release)
    metadata = FakePluginMetadataReader(version=version, plugin_name=plugin_name)
    clock = FakeClock()
    service = UpdateService(
        config=UpdateServiceConfig(
            release_api=feed,
            plugin_metadata=metadata,
            plugin_dir="/plugin",
            clock=clock,
            loop=asyncio.get_running_loop(),
            logger=logging.getLogger("test"),
        )
    )
    return service, feed, metadata, clock


class TestCheckForUpdateHappyPath:
    async def test_newer_release_is_offered(self):
        service, _, metadata, _ = _make(make_release("v0.33.0", body="notes!"))
        result = await service.check_for_update(False)
        assert set(result) == _SUCCESS_KEYS
        assert result == {
            "success": True,
            "current_version": "0.32.0",
            "latest_version": "0.33.0",
            "update_available": True,
            "notes": "notes!",
            "asset_url": "https://github.com/ricktap/romm-emudecky-ng/releases/download/v0.33.0/romm-emudecky-ng.zip",
            "sha256": VALID_SHA256,
            "html_url": "https://github.com/ricktap/romm-emudecky-ng/releases/tag/v0.33.0",
            "plugin_name": "EmuDecky RomM NG",
            "checked_at": "2026-01-01T00:00:00+00:00",
        }
        assert metadata.last_plugin_dir == "/plugin"

    async def test_plugin_name_comes_from_plugin_metadata(self):
        service, *_ = _make(make_release("v0.33.0"), plugin_name="Whatever plugin.json Says")
        result = await service.check_for_update(False)
        assert result["plugin_name"] == "Whatever plugin.json Says"

    async def test_same_version_is_not_an_update(self):
        service, *_ = _make(make_release("v0.32.0"))
        result = await service.check_for_update(False)
        assert result["success"] is True
        assert result["update_available"] is False
        assert result["latest_version"] == "0.32.0"

    async def test_older_release_is_not_an_update(self):
        service, *_ = _make(make_release("v0.31.0"))
        assert (await service.check_for_update(False))["update_available"] is False

    async def test_prerelease_tag_is_skipped(self):
        service, *_ = _make(make_release("v0.33.0-rc1"))
        result = await service.check_for_update(False)
        assert result["success"] is True
        assert result["update_available"] is False
        assert result["latest_version"] is None
        assert result["asset_url"] == ""

    async def test_release_without_valid_digest_offers_nothing(self):
        service, *_ = _make(make_release("v0.33.0", digest="sha256:short"))
        result = await service.check_for_update(False)
        assert set(result) == _SUCCESS_KEYS
        assert result["update_available"] is False
        assert result["latest_version"] == "0.33.0"
        assert result["sha256"] == ""
        assert result["asset_url"] == ""
        assert result["html_url"] == "https://github.com/ricktap/romm-emudecky-ng/releases"

    async def test_release_with_foreign_asset_url_offers_nothing(self):
        service, *_ = _make(make_release("v0.33.0", url="https://evil.example/romm-emudecky-ng.zip"))
        result = await service.check_for_update(False)
        assert result["update_available"] is False
        assert result["asset_url"] == ""


class TestThrottle:
    async def test_second_check_within_6h_uses_cache(self):
        service, feed, _, clock = _make(make_release("v0.33.0"))
        first = await service.check_for_update(False)
        clock.advance(CHECK_INTERVAL_S - 1)
        second = await service.check_for_update(False)
        assert feed.fetch_count == 1
        assert second == first

    async def test_check_after_6h_refetches(self):
        service, feed, _, clock = _make(make_release("v0.33.0"))
        await service.check_for_update(False)
        clock.advance(CHECK_INTERVAL_S)
        await service.check_for_update(False)
        assert feed.fetch_count == 2

    async def test_wall_clock_drives_throttle_across_suspend(self):
        service, feed, _, clock = _make(make_release("v0.33.0"))
        await service.check_for_update(False)
        clock.advance_wall(CHECK_INTERVAL_S + 1)  # device slept; monotonic frozen
        await service.check_for_update(False)
        assert feed.fetch_count == 2

    async def test_force_bypasses_cache(self):
        service, feed, _, _ = _make(make_release("v0.33.0"))
        await service.check_for_update(False)
        feed.release = make_release("v0.34.0")
        result = await service.check_for_update(True)
        assert feed.fetch_count == 2
        assert result["latest_version"] == "0.34.0"

    async def test_failure_is_retried_after_short_interval(self):
        service, feed, _, clock = _make()
        feed.error = ReleaseFeedError("unreachable", "offline")
        await service.check_for_update(False)
        clock.advance(FAILURE_RETRY_S - 1)
        await service.check_for_update(False)
        assert feed.fetch_count == 1
        clock.advance(1)
        feed.error = None
        result = await service.check_for_update(False)
        assert feed.fetch_count == 2
        assert result["success"] is True

    async def test_returned_dict_is_a_copy(self):
        service, *_ = _make(make_release("v0.33.0"))
        first = await service.check_for_update(False)
        first["update_available"] = "mutated"
        assert (await service.check_for_update(False))["update_available"] is True


class TestFailureShapes:
    @pytest.mark.parametrize(
        ("error", "reason"),
        [
            (ReleaseFeedError("unreachable", "offline"), "server_unreachable"),
            (ReleaseFeedError("rate_limited", "403", 403), "rate_limited"),
            (ReleaseFeedError("bad_response", "html"), "unknown"),
            (RuntimeError("surprise"), "unknown"),
        ],
    )
    async def test_failures_use_canonical_shape_and_never_raise(self, error, reason):
        service, feed, _, _ = _make()
        feed.error = error
        result = await service.check_for_update(True)
        assert result["success"] is False
        assert result["reason"] == reason
        assert isinstance(result["message"], str)
        assert result["message"]
        assert result["current_version"] == "0.32.0"
        assert result["checked_at"] == "2026-01-01T00:00:00+00:00"
        assert "error" not in result
        assert "error_code" not in result

    async def test_rate_limit_message_is_specific(self):
        service, feed, _, _ = _make()
        feed.error = ReleaseFeedError("rate_limited", "429", 429)
        result = await service.check_for_update(True)
        assert "rate limit" in result["message"]

    async def test_metadata_read_failure_is_unknown(self):
        service, _, metadata, _ = _make()

        def boom(_plugin_dir):
            raise OSError("disk gone")

        metadata.read_version = boom  # type: ignore[method-assign]
        result = await service.check_for_update(True)
        assert result["success"] is False
        assert result["reason"] == "unknown"
        assert result["current_version"] == "0.0.0"


class TestBackgroundCheck:
    async def test_schedule_runs_one_non_forced_check(self):
        service, feed, _, _ = _make(make_release("v0.33.0"))
        service.schedule_background_check()
        await asyncio.sleep(0)
        await asyncio.sleep(0.01)
        for _ in range(50):
            if feed.fetch_count:
                break
            await asyncio.sleep(0.01)
        assert feed.fetch_count == 1
        # The background result primes the cache for the first QAM open.
        await service.check_for_update(False)
        assert feed.fetch_count == 1

    async def test_shutdown_cancels_in_flight_check(self):
        service, *_ = _make()
        service.schedule_background_check()
        await service.shutdown()
        assert not service._background_tasks or all(t.done() for t in service._background_tasks)

    async def test_shutdown_without_tasks_is_noop(self):
        service, *_ = _make()
        await service.shutdown()
