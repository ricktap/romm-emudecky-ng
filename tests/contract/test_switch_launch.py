"""Contract tests for Switch launches resolved from the plugin's emulator table.

Driven through the shim over the backend socket, frontend-shaped. Emulators are
"installed" by placing executables under the harness home
(``<tmp>/home/Applications/...``) exactly where EmuDeck puts them; the real
``EmulatorLocatorAdapter`` finds them and the real ``EmuDeckLauncherAdapter``
renders the command (the harness routes table systems to the real resolver).

Each bake site — relaunch re-confirm, startup reconcile, per-game emulator
override, launch-file pick, sync, download-complete, page-time adoption — must
yield the same table command for the same ROM.
"""

from __future__ import annotations

import asyncio
import os

from domain.rom_install import RomInstall

from ._harness import background_tasks
from ._seed import seed_install, seed_rom

_ROM_ID = 501
_NSP = "Pokemon Shining Pearl [010018E011D92000][v0].nsp"


def _install_emulator(harness, *relative: str) -> str:
    path = harness.tmp_path.joinpath("home", *relative)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("#!/bin/sh\n")
    path.chmod(0o755)
    return str(path)


def _eden(harness) -> str:
    return _install_emulator(harness, "Applications", "Eden.AppImage")


def _ryujinx(harness) -> str:
    return _install_emulator(harness, "Applications", "publish", "Ryujinx.sh")


def _eden_cmd(exe: str, rom: str) -> str:
    return f'vblank_mode=0 %command% "{exe}" -f -g "{rom}"'


def _seed_switch_file(harness, rom_id: int = _ROM_ID, name: str = _NSP) -> str:
    return seed_install(harness, rom_id, system="switch", platform_slug="switch", file_name=name)


def _seed_switch_folder(harness, rom_id: int, files: dict[str, int]) -> tuple[str, dict[str, str]]:
    """A folder install (base + update/DLC) with real files of the given sizes."""
    seed_rom(harness, rom_id, platform_slug="switch")
    rom_dir = harness.tmp_path / "retrodeck" / "roms" / "switch" / f"Game {rom_id}"
    rom_dir.mkdir(parents=True)
    paths = {}
    for name, size in files.items():
        (rom_dir / name).write_bytes(b"\0" * size)
        paths[name] = str(rom_dir / name)
    # Record a non-game file as file_path to prove the bake re-picks the largest game file.
    with harness.uow_factory() as uow:
        uow.rom_installs.save(
            RomInstall.mark_installed(
                rom_id=rom_id,
                file_path=str(rom_dir),
                rom_dir=str(rom_dir),
                platform_slug="switch",
                system="switch",
                installed_at="2026-01-01T00:00:00",
            )
        )
    return str(rom_dir), paths


async def test_single_file_install_bakes_eden_command(harness):
    exe = _eden(harness)
    rom = _seed_switch_file(harness)

    item = await harness.plugin.get_rom_relaunch_options(_ROM_ID)

    assert item == {"app_id": _ROM_ID, "launch_options": _eden_cmd(exe, rom)}


async def test_startup_reconcile_bakes_eden_command(harness):
    exe = _eden(harness)
    rom = _seed_switch_file(harness)

    items = await harness.plugin.get_installed_relaunch_options()

    assert {"app_id": _ROM_ID, "launch_options": _eden_cmd(exe, rom)} in items


async def test_sync_invocations_use_the_table(harness):
    exe = _eden(harness)
    _seed_switch_file(harness)

    invocations = harness.backend._sync_service._orchestrator._build_invocations(
        [{"id": _ROM_ID, "platform_slug": "switch"}]
    )

    assert invocations == {_ROM_ID: f'vblank_mode=0 %command% "{exe}" -f -g'}


async def test_default_is_first_installed_when_eden_is_absent(harness):
    ryu = _ryujinx(harness)
    rom = _seed_switch_file(harness)

    item = await harness.plugin.get_rom_relaunch_options(_ROM_ID)

    assert item["launch_options"] == f'vblank_mode=0 %command% "{ryu}" "{rom}"'


async def test_override_to_ryujinx_rebakes(harness):
    _eden(harness)
    ryu = _ryujinx(harness)
    rom = _seed_switch_file(harness)

    result = await harness.plugin.set_game_core(_ROM_ID, "Ryujinx")

    assert result == {
        "success": True,
        "launch_options": f'vblank_mode=0 %command% "{ryu}" "{rom}"',
        "app_id": _ROM_ID,
    }
    item = await harness.plugin.get_rom_relaunch_options(_ROM_ID)
    assert item["launch_options"] == result["launch_options"]

    cleared = await harness.plugin.clear_game_core(_ROM_ID)
    assert "Eden.AppImage" in cleared["launch_options"]


async def test_override_to_missing_emulator_is_refused(harness):
    _eden(harness)
    _seed_switch_file(harness)

    result = await harness.plugin.set_game_core(_ROM_ID, "yuzu")

    assert result["success"] is False
    assert result["reason"] == "core_unavailable"


async def test_stale_override_falls_back_to_default_and_is_reported(harness):
    exe = _eden(harness)
    rom = _seed_switch_file(harness)
    with harness.uow_factory() as uow:
        uow.roms.set_emulator_override(_ROM_ID, "yuzu")  # pinned, then yuzu was uninstalled

    item = await harness.plugin.get_rom_relaunch_options(_ROM_ID)
    info = await harness.plugin.get_platform_core_info(_ROM_ID)

    assert item["launch_options"] == _eden_cmd(exe, rom)
    assert info["has_game_override"] is True
    assert info["game_override_label"] == "yuzu"
    assert info["active_core_label"] == "Eden"
    yuzu = next(e for e in info["emulators"] if e["label"] == "yuzu")
    assert (yuzu["bakeable"], yuzu["reason"]) == (False, "not_installed")


async def test_picker_lists_table_emulators(harness):
    _ryujinx(harness)
    _seed_switch_file(harness)

    info = await harness.plugin.get_platform_core_info(_ROM_ID)

    assert info["emulator_data_available"] is True
    assert [(e["label"], e["bakeable"], e["is_default"]) for e in info["emulators"]] == [
        ("Eden", False, False),
        ("Ryujinx", True, True),
        ("yuzu", False, False),
    ]
    assert info["active_core_label"] == "Ryujinx"


async def test_no_emulator_installed_is_needs_setup(harness):
    rom = _seed_switch_file(harness)

    item = await harness.plugin.get_rom_relaunch_options(_ROM_ID)
    info = await harness.plugin.get_platform_core_info(_ROM_ID)
    resolution = harness.backend._relaunch_options_resolver._launch_resolver.resolve_for(
        {"id": _ROM_ID}, None, "switch"
    )

    assert resolution.status == "needs_setup"
    assert "%command%" not in item["launch_options"]
    assert item["launch_options"].strip() == f'"{rom}"'
    assert not any(e["bakeable"] for e in info["emulators"])
    assert info["active_core_label"] is None


async def test_folder_install_launches_largest_nsp(harness):
    exe = _eden(harness)
    _rom_dir, paths = _seed_switch_folder(
        harness,
        77,
        {"base.nsp": 4000, "update [v196608].NSP": 900, "dlc.xci": 20, "notes.txt": 99_999},
    )

    item = await harness.plugin.get_rom_relaunch_options(77)

    assert item["launch_options"] == _eden_cmd(exe, paths["base.nsp"])


async def test_folder_install_offers_and_respects_launch_file_pick(harness):
    exe = _eden(harness)
    _rom_dir, paths = _seed_switch_folder(harness, 78, {"base.nsp": 4000, "update [v196608].NSP": 900})

    selection = await harness.plugin.get_disc_selection(78)
    assert selection["multi_disc"] is True
    assert [d["filename"] for d in selection["discs"]] == ["base.nsp", "update [v196608].NSP"]
    assert selection["default"]["filename"] == "base.nsp"

    result = await harness.plugin.select_disc(78, "update [v196608].NSP")
    assert result["success"] is True
    assert result["launch_options"] == _eden_cmd(exe, paths["update [v196608].NSP"])

    item = await harness.plugin.get_rom_relaunch_options(78)
    assert item["launch_options"] == result["launch_options"]


def _server_switch_rom(harness, rom_id: int, fs_name: str, size: int) -> None:
    harness.romm.roms[rom_id] = {
        "id": rom_id,
        "name": f"Switch {rom_id}",
        "fs_name": fs_name,
        "fs_size_bytes": size,
        "platform_id": 9,
        "platform_slug": "switch",
        "platform_fs_slug": "switch",
        "platform_name": "Nintendo Switch",
        "files": [{"file_name": fs_name, "file_size_bytes": size}],
    }


def _events(harness, name: str) -> list[dict]:
    return [c.args[1] for c in harness.emit.call_args_list if c.args and c.args[0] == name]


async def _drain(harness) -> None:
    for _ in range(6):
        pending = background_tasks()
        if not pending:
            break
        await asyncio.gather(*pending, return_exceptions=True)
    await harness.flush_events()


async def test_download_complete_bakes_eden_command(harness):
    exe = _eden(harness)
    seed_rom(harness, 90, platform_slug="switch")
    _server_switch_rom(harness, 90, _NSP, 8)
    harness.romm.download_payloads[f"rom:90:{_NSP}"] = b"SWITCHGM"

    result = await harness.plugin.start_download(90)
    assert result["success"] is True
    await _drain(harness)

    [event] = _events(harness, "download_complete")
    assert event["launch_options"] == _eden_cmd(exe, event["file_path"])
    assert event["file_path"].endswith(_NSP)


async def test_page_time_adoption_bakes_eden_command(harness):
    exe = _eden(harness)
    seed_rom(harness, 91, platform_slug="switch", shortcut_app_id=9191)
    _server_switch_rom(harness, 91, _NSP, 16)
    platform_dir = os.path.join(harness.retrodeck_paths.roms_path(), "switch")
    os.makedirs(platform_dir, exist_ok=True)
    with open(os.path.join(platform_dir, _NSP), "wb") as f:
        f.write(b"\0" * 16)

    await harness.plugin.get_cached_game_detail(9191)
    await _drain(harness)

    [event] = _events(harness, "download_complete")
    assert event["adopted"] is True
    assert event["launch_options"].startswith(f'vblank_mode=0 %command% "{exe}" -f -g "')
    assert event["launch_options"].endswith(f'{_NSP}"')


async def test_folder_pick_reports_file_kind(harness):
    _eden(harness)
    _seed_switch_folder(harness, 79, {"base.nsp": 4000, "dlc.xci": 10})

    selection = await harness.plugin.get_disc_selection(79)

    assert selection["default"] == {"kind": "file", "label": "base", "filename": "base.nsp"}
