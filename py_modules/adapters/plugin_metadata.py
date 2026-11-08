"""Concrete ``PluginMetadataReader`` adapter — reads ``package.json`` and ``plugin.json``.

Owns the raw ``open()`` + ``json.load()`` round-trip behind the
``PluginMetadataReader`` Protocol. A missing or malformed
metadata file is not a hard failure — bootstrap must keep wiring
services even when the metadata read fails, so the adapter returns the
documented fallback.
"""

from __future__ import annotations

import json
import os


class PluginMetadataAdapter:
    """Real ``PluginMetadataReader`` backed by the on-disk ``package.json``."""

    def read_version(self, plugin_dir: str) -> str:
        try:
            with open(os.path.join(plugin_dir, "package.json")) as f:
                return json.load(f).get("version", "0.0.0")
        except (OSError, json.JSONDecodeError, AttributeError):
            return "0.0.0"

    def read_plugin_name(self, plugin_dir: str) -> str:
        try:
            with open(os.path.join(plugin_dir, "plugin.json")) as f:
                name = json.load(f).get("name", "")
        except (OSError, json.JSONDecodeError, AttributeError):
            return ""
        return name if isinstance(name, str) else ""
