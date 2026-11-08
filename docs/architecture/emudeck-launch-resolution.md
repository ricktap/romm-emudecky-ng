# EmuDeck Launch Resolution

## Overview

The EmuDeck launch resolution system provides the bridge between the plugin's core/emulator selection logic and
EmuDeck's per-system launcher scripts. While `ActiveCoreResolver` determines _which_ emulator a game should use, the
EmuDeck launch resolution system determines _how_ to invoke that emulator through EmuDeck's launcher infrastructure.

The key insight: EmuDeck uses per-system launcher scripts at `<storagePath>/Emulation/tools/launchers/` instead of a
single monolithic launcher. Each system (NES, SNES, PS2, etc.) has its own launcher script that handles the appropriate
emulator invocation for that system.

## Architecture

### Component Overview

```
┌─────────────────────────────────────────────────────────────┐
│ ActiveCoreResolver                                          │
│   → determines which emulator (EmulatorInvocation)          │
└─────────────────────┬───────────────────────────────────────┘
                      │
                      ▼
┌─────────────────────────────────────────────────────────────┐
│ LaunchResolverFn Protocol                                   │
│   → resolves EmulatorInvocation to launch command           │
│   → uses EmuDeckPathsAdapter for path resolution            │
│   → uses EmuDeckLauncherAdapter for launcher discovery      │
└─────────────────────┬───────────────────────────────────────┘
                      │
                      ▼
┌─────────────────────────────────────────────────────────────┐
│ EmuDeck Launch Command                                      │
│   → <launcher_script> <args>                                │
│   → baked into Steam shortcut's launch_options              │
└─────────────────────────────────────────────────────────────┘
```

### EmuDeckPathsAdapter

**File:** `py_modules/adapters/emudeck_paths.py`

Reads EmuDeck's configuration and resolves the `Emulation/*` directory tree.

```python
class EmuDeckPathsAdapter:
    """Reads ~/.config/EmuDeck/settings.json for <storagePath> and resolves Emulation/* tree."""
    
    def __init__(self, settings_path: Path, clock: Clock):
        self._settings_path = settings_path
        self._clock = clock
        self._cache: EmuDeckPaths | None = None
    
    def get_paths(self) -> EmuDeckPaths:
        """Returns resolved EmuDeck paths, cached for the session."""
        if self._cache is None:
            self._cache = self._read_and_resolve()
        return self._cache
    
    def _read_and_resolve(self) -> EmuDeckPaths:
        """Reads settings.json and resolves the storage path."""
        settings = self._read_settings()
        storage_path = Path(settings.get("storagePath", ""))
        
        return EmuEmuDeckPaths(
            storage_path=storage_path,
            launchers_dir=storage_path / "Emulation" / "tools" / "launchers",
            bios_dir=storage_path / "Emulation" / "bios",
            saves_dir=storage_path / "Emulation" / "saves",
            # ... other paths
        )
```

**Key Configuration:**

- **Config location:** `~/.config/EmuDeck/settings.json`
- **Primary setting:** `storagePath` — root of the Emulation directory tree
- **Launcher scripts:** `<storagePath>/Emulation/tools/launchers/`

### EmuDeckLauncherAdapter

**File:** `py_modules/adapters/emudeck_launcher.py`

Disk-probe adapter that discovers available launcher scripts and resolves the appropriate one for each system.

```python
class EmuDeckLauncherAdapter:
    """Disk-probe adapter for EmuDeck launcher scripts."""
    
    def __init__(self, emu_deck_paths: EmuDeckPaths, clock: Clock):
        self._paths = emu_deck_paths
        self._clock = clock
        self._launchers_cache: dict[str, Path] | None = None
    
    def get_launcher(self, system: str) -> Path | None:
        """Returns the launcher script path for a system, or None if not found."""
        launchers = self._get_available_launchers()
        return launchers.get(system)
    
    def _get_available_launchers(self) -> dict[str, Path]:
        """Disk-probes the launchers directory and returns system→path map."""
        if self._launchers_cache is not None:
            return self._launchers_cache
        
        launchers = {}
        launchers_dir = self._paths.launchers_dir
        
        if launchers_dir.exists():
            for script in launchers_dir.glob("*.sh"):
                system = script.stem  # e.g., "retroarch", "pcsx2", "rpcs3"
                launchers[system] = script
        
        self._launchers_cache = launchers
        return launchers
```

**Key Behaviors:**

- Discovers launcher scripts by scanning `<storagePath>/Emulation/tools/launchers/*.sh`
- Caches results for the session (launchers don't change during runtime)
- Returns `None` for systems without a launcher script (triggers fallback behavior)

### EmuDeckLaunchResolution

**File:** `py_modules/domain/emudeck_launch.py`

Pure value object representing the result of launch resolution.

```python
@dataclass(frozen=True)
class EmuDeckLaunchResolution:
    """Result of resolving a launch command through EmuDeck."""
    
    status: LaunchResolutionStatus
    launcher_path: Path | None = None
    args: list[str] = field(default_factory=list)
    error_message: str | None = None

class LaunchResolutionStatus(Enum):
    """Status of launch resolution."""
    OK = "ok"                    # Successfully resolved
    NEEDS_SETUP = "needs_setup"  # EmuDeck not configured or launcher missing
    LAUNCHER_MISSING = "launcher_missing"  # Specific launcher script not found
```

### LaunchResolverFn Protocol

**File:** `py_modules/services/protocols/__init__.py`

The single shared resolver protocol used by all bake sites.

```python
class LaunchResolverFn(Protocol):
    """Resolves an EmulatorInvocation to a concrete launch command."""
    
    def __call__(
        self, 
        emulator: EmulatorInvocation, 
        system: str,
        rom_path: str | None = None
    ) -> EmuDeckLaunchResolution:
        """Resolve the emulator invocation to a launch command."""
        ...
```

## Pure Resolver

**File:** `py_modules/domain/emudeck_launch.py`

The pure resolver builds the launch command without performing I/O. It takes pre-resolved paths and launcher information
and constructs the command.

```python
def resolve_launch_command(
    emulator: EmulatorInvocation,
    launcher_path: Path | None,
    system: str,
    rom_path: str | None = None,
) -> EmuDeckLaunchResolution:
    """Pure function: builds launch command from resolved inputs."""
    
    if launcher_path is None:
        return EmuDeckLaunchResolution(
            status=LaunchResolutionStatus.LAUNCHER_MISSING,
            error_message=f"No launcher script found for system: {system}"
        )
    
    # Build the command based on emulator type
    if emulator.kind == "libretro":
        # Libretro: retroarch.sh -L <core> <rom>
        args = ["-L", f"{emulator.core_so}"]
        if rom_path:
            args.append(rom_path)
    elif emulator.kind == "standalone":
        # Standalone: use the emulator's command directly
        args = emulator.command.split()
        if rom_path:
            # Replace %ROM% placeholder with actual path
            args = [arg.replace("%ROM%", rom_path) for arg in args]
    
    return EmuDeckLaunchResolution(
        status=LaunchResolutionStatus.OK,
        launcher_path=launcher_path,
        args=args
    )
```

## System Default Launchers

**File:** `py_modules/domain/emudeck_launch.py`

The `SYSTEM_DEFAULT_LAUNCHERS` table provides fallback launcher mappings when EmuDeck's launcher scripts are not
available or when a system doesn't have a dedicated launcher.

```python
# Empty by default — populated from verified EmuDeck installation
SYSTEM_DEFAULT_LAUNCHERS: dict[str, str] = {
    # Example structure (empty in practice):
    # "nes": "retroarch",
    # "snes": "retroarch",
    # "ps2": "pcsx2",
    # "ps3": "rpcs3",
}
```

**Design Decision:** The table is empty by default because:

1. EmuDeck installations vary (different versions, custom configurations)
2. Launcher scripts may be renamed or reorganized
3. The disk-probe approach (`EmuDeckLauncherAdapter`) is more reliable than hardcoded paths
4. The plugin should adapt to the user's actual EmuDeck installation, not assume a specific layout

## EmuDeckConfigHealth

**File:** `py_modules/domain/emudeck_launch.py`

Enum representing the health status of EmuDeck configuration.

```python
class EmuDeckConfigHealth(Enum):
    """Health status of EmuDeck configuration."""
    OK = "ok"                    # Configuration is valid and readable
    ABSENT = "absent"            # ~/.config/EmuDeck/settings.json not found
    UNREADABLE = "unreadable"    # File exists but cannot be parsed
    INVALID = "invalid"          # File is valid JSON but missing required fields
    ROOT_MISSING = "root_missing"  # storagePath exists but Emulation/ tree missing
```

**Health Check Flow:**

1. Check if `~/.config/EmuDeck/settings.json` exists
2. Attempt to parse the JSON
3. Validate required fields (especially `storagePath`)
4. Verify `<storagePath>/Emulation/` directory exists
5. Verify `<storagePath>/Emulation/tools/launchers/` exists

## Integration with Core Selection System

The EmuDeck launch resolution integrates with the existing core selection system through the following flow:

```
1. ActiveCoreResolver.active_emulator_for_rom(rom_id)
   → Returns EmulatorInvocation (kind, core_so, label, command)

2. LaunchResolverFn(emulator_invocation, system, rom_path)
   → Uses EmuDeckPathsAdapter to get storage paths
   → Uses EmuDeckLauncherAdapter to find launcher script
   → Calls pure resolver to build command
   → Returns EmuDeckLaunchResolution

3. resolve_emulator_invocation(rom, emulator) in domain/shortcut_data.py
   → Calls LaunchResolverFn
   → Formats final launch command for Steam shortcut
```

### Bake Sites

All three bake sites use the same `LaunchResolverFn`:

| Bake Site                                    | When It Runs                 | How It Resolves                                                           |
| -------------------------------------------- | ---------------------------- | ------------------------------------------------------------------------- |
| `SyncOrchestrator` → `_build_core_overrides` | Every sync (preview + apply) | Each ROM through `active_emulator_for_rom` → `LaunchResolverFn`           |
| `DownloadService` → `_resolve_bound_app_id`  | On download-complete         | ROM through `active_emulator_for_rom` → `LaunchResolverFn`                |
| `MigrationService` → `_build_relaunch_items` | On EmuDeck-home migration    | Each relocated ROM through `active_emulator_for_rom` → `LaunchResolverFn` |

### Example: Libretro Game

```python
# Input
emulator = EmulatorInvocation(
    kind="libretro",
    core_so="snes9x_libretro.so",
    label="Snes9x - Current"
)
system = "snes"
rom_path = "/path/to/game.sfc"

# Resolution
resolution = launch_resolver(emulator, system, rom_path)

# Output
EmuDeckLaunchResolution(
    status=LaunchResolutionStatus.OK,
    launcher_path=Path("/home/deck/Emulation/tools/launchers/retroarch.sh"),
    args=["-L", "snes9x_libretro.so", "/path/to/game.sfc"]
)

# Final launch command (baked into Steam shortcut)
"/home/deck/Emulation/tools/launchers/retroarch.sh -L snes9x_libretro.so /path/to/game.sfc"
```

### Example: Standalone Emulator

```python
# Input
emulator = EmulatorInvocation(
    kind="standalone",
    core_so=None,
    label="PCSX2",
    command="%EMULATOR_PCSX2% --no-gui %ROM%"
)
system = "ps2"
rom_path = "/path/to/game.iso"

# Resolution
resolution = launch_resolver(emulator, system, rom_path)

# Output
EmuDeckLaunchResolution(
    status=LaunchResolutionStatus.OK,
    launcher_path=Path("/home/deck/Emulation/tools/launchers/pcsx2.sh"),
    args=["--no-gui", "/path/to/game.iso"]
)

# Final launch command
"/home/deck/Emulation/tools/launchers/pcsx2.sh --no-gui /path/to/game.iso"
```

## Reference to ADR-0009

The EmuDeck launch resolution follows the launcher pure-exec wrapper model established in
[ADR-0009](https://github.com/danielcopper/decky-romm-sync/blob/main/docs/adr/0009-launcher-pure-exec-wrapper-baked-launch-options.md):

- The launcher (`bin/rom-launcher`) is a pure `exec "$@"` wrapper
- The full launch command lives in the Steam shortcut's `launch_options`
- No state, no path resolution, no emulator knowledge in the launcher itself
- The plugin bakes the complete command at sync/download time

The key difference from the original RetroDECK model:

- **RetroDECK:** `flatpak run net.retrodeck.retrodeck -e "<emulator_command>"`
- **EmuDeck:** `<storagePath>/Emulation/tools/launchers/<system>.sh <args>`

Both models follow the same principle: the launcher is a thin wrapper, and the plugin owns the full launch command.

---

**Related pages:**

- [Core and Emulator Selection](core-emulator-selection.md) — how the plugin decides which emulator to use
- [Backend Architecture](backend-architecture.md) — service/adapter layering, dependency diagram
- [Config Source Parsers](config-source-parsers.md) — how ES-DE configuration is read
- [Steam Non-Steam Shortcuts](steam-non-steam-shortcuts.md) — how launch commands are applied to shortcuts
