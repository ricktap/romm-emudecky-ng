"""Tests for the PluginMetadataAdapter — reads plugin package.json."""

from __future__ import annotations

import json

import pytest

from adapters.plugin_metadata import PluginMetadataAdapter


class TestPluginMetadataAdapter:
    def test_read_version_returns_declared_version(self, tmp_path):
        plugin_dir = tmp_path / "plugin"
        plugin_dir.mkdir()
        (plugin_dir / "package.json").write_text(json.dumps({"version": "1.2.3"}))

        adapter = PluginMetadataAdapter()
        assert adapter.read_version(str(plugin_dir)) == "1.2.3"

    def test_read_version_missing_file_returns_fallback(self, tmp_path):
        """A missing package.json must not abort bootstrap."""
        plugin_dir = tmp_path / "plugin"
        plugin_dir.mkdir()

        adapter = PluginMetadataAdapter()
        assert adapter.read_version(str(plugin_dir)) == "0.0.0"

    def test_read_version_missing_directory_returns_fallback(self, tmp_path):
        adapter = PluginMetadataAdapter()
        assert adapter.read_version(str(tmp_path / "does-not-exist")) == "0.0.0"

    def test_read_version_malformed_json_returns_fallback(self, tmp_path):
        plugin_dir = tmp_path / "plugin"
        plugin_dir.mkdir()
        (plugin_dir / "package.json").write_text("{not valid json")

        adapter = PluginMetadataAdapter()
        assert adapter.read_version(str(plugin_dir)) == "0.0.0"

    def test_read_version_missing_version_field_returns_fallback(self, tmp_path):
        """A package.json without a ``version`` key falls back to ``0.0.0``."""
        plugin_dir = tmp_path / "plugin"
        plugin_dir.mkdir()
        (plugin_dir / "package.json").write_text(json.dumps({"name": "romm-emudecky-ng"}))

        adapter = PluginMetadataAdapter()
        assert adapter.read_version(str(plugin_dir)) == "0.0.0"

    @pytest.mark.parametrize("empty_value", ["", None])
    def test_read_version_empty_version_returned_as_is(self, tmp_path, empty_value):
        """The adapter does not validate the version string — empty/None pass through.

        The fallback is only applied when the field is missing entirely; an
        empty string or ``None`` reflects what the file actually contained
        and surfaces upstream so a malformed publish can be diagnosed.
        """
        plugin_dir = tmp_path / "plugin"
        plugin_dir.mkdir()
        (plugin_dir / "package.json").write_text(json.dumps({"version": empty_value}))

        adapter = PluginMetadataAdapter()
        assert adapter.read_version(str(plugin_dir)) == empty_value


class TestReadPluginName:
    def test_reads_plugin_json_name(self, tmp_path):
        (tmp_path / "plugin.json").write_text(json.dumps({"name": "EmuDecky RomM+"}))
        assert PluginMetadataAdapter().read_plugin_name(str(tmp_path)) == "EmuDecky RomM+"

    def test_missing_file_returns_empty(self, tmp_path):
        assert PluginMetadataAdapter().read_plugin_name(str(tmp_path)) == ""

    @pytest.mark.parametrize("content", ["{broken", "[1, 2]", json.dumps({"name": 7}), json.dumps({})])
    def test_malformed_or_missing_name_returns_empty(self, tmp_path, content):
        (tmp_path / "plugin.json").write_text(content)
        assert PluginMetadataAdapter().read_plugin_name(str(tmp_path)) == ""

    def test_real_plugin_json_name_is_the_pinned_install_name(self):
        """Decky's installer matches the installed plugin by this exact name.

        Changing it breaks self-update for every existing install — this pin
        makes a rename a deliberate, visible test change.
        """
        import os

        repo_root = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
        assert PluginMetadataAdapter().read_plugin_name(repo_root) == "EmuDecky RomM+"
