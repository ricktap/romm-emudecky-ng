import logging
import os
import sys
import tempfile
from unittest.mock import AsyncMock, MagicMock

import pytest

# Mirror Decky's sys.path setup: py_modules/ holds the shim's protocol client.
_project_root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
_tests_root = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(_project_root, "py_modules"))
sys.path.insert(0, _tests_root)

# Create mock decky module before any imports of main
mock_decky = MagicMock()
mock_decky.DECKY_PLUGIN_DIR = _project_root
mock_decky.DECKY_PLUGIN_SETTINGS_DIR = tempfile.mkdtemp()
mock_decky.DECKY_PLUGIN_RUNTIME_DIR = tempfile.mkdtemp()
mock_decky.DECKY_PLUGIN_LOG_DIR = tempfile.mkdtemp()
mock_decky.DECKY_USER_HOME = os.path.expanduser("~")
mock_decky.logger = logging.getLogger("test_romm")
mock_decky.emit = AsyncMock()

sys.modules["decky"] = mock_decky


@pytest.fixture(autouse=True)
def _reset_decky_mock_paths():
    """Fresh settings/runtime dirs on the mock decky module for every test."""
    mock_decky.DECKY_USER_HOME = os.path.expanduser("~")
    mock_decky.DECKY_PLUGIN_DIR = _project_root
    mock_decky.DECKY_PLUGIN_SETTINGS_DIR = tempfile.mkdtemp()
    mock_decky.DECKY_PLUGIN_RUNTIME_DIR = tempfile.mkdtemp()
    yield
