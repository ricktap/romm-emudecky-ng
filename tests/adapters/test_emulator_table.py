"""Disk probe for table emulators (``EmulatorLocatorAdapter``) and the table-aware core-info wrapper."""

from __future__ import annotations

import logging
from unittest.mock import MagicMock

import pytest

from adapters.emulator_table import EmulatorLocatorAdapter, TableAwareCoreInfoAdapter
from domain.emulator_table import find_emulator
from domain.shortcut_data import EmulatorInvocation

_LOG = logging.getLogger("emulator-table-test")


def _exe(path, executable: bool = True):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("#!/bin/sh\n")
    path.chmod(0o755 if executable else 0o644)
    return str(path)


@pytest.fixture
def home(tmp_path):
    return tmp_path / "home"


class TestLocate:
    def test_finds_eden_appimage_by_glob(self, home):
        path = _exe(home / "Applications" / "Eden.AppImage")
        assert EmulatorLocatorAdapter(user_home=str(home), logger=_LOG).locate(find_emulator("eden")) == path

    def test_glob_matches_versioned_names(self, home):
        path = _exe(home / "Applications" / "Eden-v0.0.3-x86_64.AppImage")
        assert EmulatorLocatorAdapter(user_home=str(home), logger=_LOG).locate(find_emulator("eden")) == path

    def test_match_is_case_insensitive(self, home):
        path = _exe(home / "applications" / "YUZU-mainline.appimage")
        assert EmulatorLocatorAdapter(user_home=str(home), logger=_LOG).locate(find_emulator("yuzu")) == path

    def test_ryujinx_nested_script(self, home):
        path = _exe(home / "Applications" / "publish" / "Ryujinx.sh")
        assert EmulatorLocatorAdapter(user_home=str(home), logger=_LOG).locate(find_emulator("ryujinx")) == path

    def test_non_executable_is_not_installed(self, home):
        _exe(home / "Applications" / "Eden.AppImage", executable=False)
        assert EmulatorLocatorAdapter(user_home=str(home), logger=_LOG).locate(find_emulator("eden")) is None

    def test_directory_named_like_appimage_is_not_installed(self, home):
        (home / "Applications" / "Eden.AppImage").mkdir(parents=True)
        assert EmulatorLocatorAdapter(user_home=str(home), logger=_LOG).locate(find_emulator("eden")) is None

    def test_missing_applications_dir(self, home):
        home.mkdir()
        assert EmulatorLocatorAdapter(user_home=str(home), logger=_LOG).locate(find_emulator("eden")) is None

    def test_multiple_matches_pick_is_stable(self, home):
        first = _exe(home / "Applications" / "Eden-a.AppImage")
        _exe(home / "Applications" / "Eden-b.AppImage")
        assert EmulatorLocatorAdapter(user_home=str(home), logger=_LOG).locate(find_emulator("eden")) == first

    def test_home_falls_back_to_env(self, home, monkeypatch):
        path = _exe(home / "Applications" / "Eden.AppImage")
        monkeypatch.setenv("HOME", str(home))
        assert EmulatorLocatorAdapter(user_home=None, logger=_LOG).locate(find_emulator("eden")) == path

    def test_no_home_at_all(self, monkeypatch):
        monkeypatch.delenv("HOME", raising=False)
        assert EmulatorLocatorAdapter(user_home="", logger=_LOG).locate(find_emulator("eden")) is None

    def test_installed_maps_ids_to_paths(self, home):
        eden = _exe(home / "Applications" / "Eden.AppImage")
        ryu = _exe(home / "Applications" / "publish" / "Ryujinx.sh")
        locator = EmulatorLocatorAdapter(user_home=str(home), logger=_LOG)
        assert locator.installed("switch") == {"eden": eden, "ryujinx": ryu}
        assert locator.installed("psx") == {}


class TestTableAwareCoreInfo:
    def _adapter(self, home):
        inner = MagicMock()
        locator = EmulatorLocatorAdapter(user_home=str(home), logger=_LOG)
        return TableAwareCoreInfoAdapter(inner=inner, locator=locator), inner

    def test_switch_default_is_first_installed(self, home):
        ryu = _exe(home / "Applications" / "publish" / "Ryujinx.sh")
        _exe(home / "Applications" / "yuzu.AppImage")
        adapter, inner = self._adapter(home)
        assert adapter.get_default_emulator("switch") == EmulatorInvocation.table("ryujinx", ryu, "Ryujinx")
        inner.get_default_emulator.assert_not_called()

    def test_switch_nothing_installed_has_no_default(self, home):
        adapter, _inner = self._adapter(home)
        assert adapter.get_default_emulator("switch") is None

    def test_switch_options_list_the_whole_table(self, home):
        _exe(home / "Applications" / "Eden.AppImage")
        adapter, _inner = self._adapter(home)
        result = adapter.get_emulator_options("switch")
        assert result["available"] is True
        assert [(o.label, o.status) for o in result["options"]] == [
            ("Eden", "bakeable"),
            ("Ryujinx", "needs_setup"),
            ("yuzu", "needs_setup"),
        ]

    def test_switch_has_no_libretro_active_core(self, home):
        adapter, _inner = self._adapter(home)
        assert adapter.get_active_core("switch") == (None, None)

    def test_uncovered_systems_delegate(self, home):
        adapter, inner = self._adapter(home)
        adapter.get_default_emulator("psx")
        adapter.get_emulator_options("psx")
        adapter.get_active_core("psx")
        adapter.resolve_sandbox_launcher("%EMULATOR_X% %ROM%")
        adapter.reset_cache()
        inner.get_default_emulator.assert_called_once_with("psx")
        inner.get_emulator_options.assert_called_once_with("psx")
        inner.get_active_core.assert_called_once_with("psx")
        inner.resolve_sandbox_launcher.assert_called_once()
        inner.reset_cache.assert_called_once()
