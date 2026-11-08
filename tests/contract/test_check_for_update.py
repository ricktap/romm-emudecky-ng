"""Contract test for the self-update check callable.

Driven frontend-shaped per ``src/api/backend.ts``:
``checkForUpdate = callable<[boolean], UpdateCheckResult>("check_for_update")``.

Pins the success shape the QAM banner + Decky installer handoff read
(``plugin_name`` from the real ``plugin.json`` via the real metadata adapter),
the canonical failure shape, and the 6 h throttle vs. ``force``.
"""

from __future__ import annotations

import json

from fakes.fake_release_feed_api import VALID_SHA256, make_release

from lib.errors import ReleaseFeedError


def _write_plugin_metadata(harness, *, version="0.32.0", name="EmuDecky RomM+"):
    plugin_dir = harness.tmp_path / "plugin"
    plugin_dir.mkdir(parents=True, exist_ok=True)
    (plugin_dir / "package.json").write_text(json.dumps({"name": "romm-emudecky-ng", "version": version}))
    (plugin_dir / "plugin.json").write_text(json.dumps({"name": name}))


async def test_check_for_update_happy_path_shape(harness):
    _write_plugin_metadata(harness, name="Name From plugin.json")
    harness.release_feed.release = make_release("v0.33.0", body="What's new")

    result = await harness.plugin.check_for_update(False)

    assert result == {
        "success": True,
        "current_version": "0.32.0",
        "latest_version": "0.33.0",
        "update_available": True,
        "notes": "What's new",
        "asset_url": "https://github.com/ricktap/romm-emudecky-ng/releases/download/v0.33.0/romm-emudecky-ng.zip",
        "sha256": VALID_SHA256,
        "html_url": "https://github.com/ricktap/romm-emudecky-ng/releases/tag/v0.33.0",
        "plugin_name": "Name From plugin.json",
        "checked_at": harness.clock.now().isoformat(),
    }


async def test_check_for_update_up_to_date(harness):
    _write_plugin_metadata(harness, version="0.33.0")
    harness.release_feed.release = make_release("v0.33.0")

    result = await harness.plugin.check_for_update(False)

    assert result["success"] is True
    assert result["update_available"] is False


async def test_check_for_update_failure_shape(harness):
    _write_plugin_metadata(harness)
    harness.release_feed.error = ReleaseFeedError("rate_limited", "HTTP 403", 403)

    result = await harness.plugin.check_for_update(True)

    assert set(result) == {"success", "reason", "message", "current_version", "checked_at"}
    assert result["success"] is False
    assert result["reason"] == "rate_limited"
    assert isinstance(result["message"], str)
    assert result["message"]
    assert result["current_version"] == "0.32.0"


async def test_check_for_update_offline_is_server_unreachable(harness):
    _write_plugin_metadata(harness)
    harness.release_feed.error = ReleaseFeedError("unreachable", "no route")

    result = await harness.plugin.check_for_update(True)

    assert result["success"] is False
    assert result["reason"] == "server_unreachable"


async def test_check_for_update_throttles_unless_forced(harness):
    _write_plugin_metadata(harness)
    harness.release_feed.release = make_release("v0.33.0")

    await harness.plugin.check_for_update(False)
    await harness.plugin.check_for_update(False)
    assert harness.release_feed.fetch_count == 1

    await harness.plugin.check_for_update(True)
    assert harness.release_feed.fetch_count == 2
