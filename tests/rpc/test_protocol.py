"""Wire-format tests for :mod:`rpc.protocol`."""

from __future__ import annotations

import json
import math
import os

import pytest

from rpc import protocol


class TestEncodeDecode:
    def test_encode_is_one_newline_terminated_json_line(self):
        frame = protocol.encode(protocol.request(1, "get_settings", []))
        assert frame.endswith(b"\n")
        assert frame.count(b"\n") == 1
        assert json.loads(frame) == {"id": 1, "method": "get_settings", "args": []}

    def test_embedded_newlines_stay_escaped(self):
        frame = protocol.encode(protocol.result_response(3, "line1\nline2"))
        assert frame.count(b"\n") == 1
        assert protocol.decode(frame)["result"] == "line1\nline2"

    def test_unicode_round_trips(self):
        frame = protocol.encode(protocol.event("x", ["Pokémon [0100]"]))
        assert protocol.decode(frame) == {"event": "x", "args": ["Pokémon [0100]"]}

    def test_nan_is_rejected(self):
        with pytest.raises(ValueError):
            protocol.encode(protocol.result_response(1, math.nan))

    def test_bytes_are_rejected(self):
        with pytest.raises(TypeError):
            protocol.encode(protocol.result_response(1, b"raw"))

    def test_decode_malformed_raises(self):
        with pytest.raises(protocol.ProtocolError):
            protocol.decode(b"{not json\n")

    def test_decode_non_object_raises(self):
        with pytest.raises(protocol.ProtocolError):
            protocol.decode(b"[1, 2]\n")

    def test_error_response_shape(self):
        assert protocol.error_response(7, "boom") == {"id": 7, "error": {"message": "boom"}}


class TestSocketPath:
    def test_runtime_dir_used_when_short(self, tmp_path):
        short = "/tmp/rt"
        assert protocol.socket_path_for(short) == "/tmp/rt/backend.sock"

    def test_long_runtime_dir_falls_back_to_short_temp(self):
        long_dir = "/tmp/" + "x" * 200
        path = protocol.socket_path_for(long_dir)
        assert path.endswith("/backend.sock")
        assert len(path) <= 100
        assert os.path.isdir(os.path.dirname(path))

    def test_no_runtime_dir_falls_back(self):
        path = protocol.socket_path_for(None)
        assert path.endswith("backend.sock")
        assert os.path.isdir(os.path.dirname(path))
