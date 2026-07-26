"""Freight Fate save discovery for driver import.

Reads the raw save JSON (Freight Fate's HMAC signature is per-install, so
another program can never verify it -- and does not need to: the exchange
record carries career identity, not money or equipment, so a tampered save
buys nothing here).
"""

from __future__ import annotations

import json
import logging
import os
import sys
from pathlib import Path

from freightverse.exchange import DriverRecord, export_driver_from_save

from .models.shift import DispatchShift

log = logging.getLogger(__name__)


def dispatch_save_path() -> Path:
    """Return the per-user active-shift path, overridable for tests and portable builds."""
    override = os.environ.get("FF_DISPATCH_DATA_DIR")
    if override:
        root = Path(override)
    elif sys.platform == "win32" and os.environ.get("APPDATA"):
        root = Path(os.environ["APPDATA"]) / "FreightFateDispatch"
    elif sys.platform == "darwin":
        root = Path.home() / "Library" / "Application Support" / "FreightFateDispatch"
    else:
        root = Path(os.environ.get("XDG_DATA_HOME", Path.home() / ".local" / "share"))
        root /= "FreightFateDispatch"
    return root / "active-shift.json"


def save_shift(shift: DispatchShift, path: Path | None = None) -> Path:
    """Atomically save the active dispatch board as versioned JSON."""
    target = path or dispatch_save_path()
    target.parent.mkdir(parents=True, exist_ok=True)
    temporary = target.with_suffix(".tmp")
    temporary.write_text(json.dumps(shift.to_snapshot(), indent=2), encoding="utf-8")
    temporary.replace(target)
    return target


def load_shift(world, path: Path | None = None) -> DispatchShift | None:
    """Load an active board; corrupt or incompatible saves are simply unavailable."""
    target = path or dispatch_save_path()
    try:
        payload = json.loads(target.read_text(encoding="utf-8"))
        return DispatchShift.from_snapshot(world, payload)
    except (IndexError, KeyError, OSError, TypeError, ValueError):
        if target.exists():
            log.warning("Could not load dispatch save at %s", target)
        return None


def clear_saved_shift(path: Path | None = None) -> None:
    """Remove a completed active-shift snapshot, if one exists."""
    try:
        (path or dispatch_save_path()).unlink()
    except FileNotFoundError:
        pass
    except OSError:
        log.warning("Could not clear completed dispatch save", exc_info=True)


def _candidate_save_roots() -> list[Path]:
    roots: list[Path] = []
    env = os.environ.get("FREIGHT_FATE_DATA_DIR")
    if env:
        roots.append(Path(env))
    extra = os.environ.get("FF_DISPATCH_FREIGHT_FATE_SAVES")
    if extra:
        roots.append(Path(extra))
    home = Path.home()
    if sys.platform == "win32":
        appdata = os.environ.get("APPDATA")
        if appdata:
            roots.append(Path(appdata) / "FreightFate")
        # common portable checkout/install spots
        roots.append(home / "Freight-fate" / "saves")
        roots.append(home / "FreightFate" / "saves")
    elif sys.platform == "darwin":
        roots.append(home / "Library" / "Application Support" / "FreightFate")
    else:
        xdg = os.environ.get("XDG_DATA_HOME", str(home / ".local" / "share"))
        roots.append(Path(xdg) / "FreightFate")
    seen: set[Path] = set()
    unique = []
    for root in roots:
        if root not in seen:
            seen.add(root)
            unique.append(root)
    return unique


def find_freight_fate_drivers() -> list[tuple[str, Path, DriverRecord]]:
    """(label, save path, exchange record) for every readable Freight Fate save."""
    found: list[tuple[str, Path, DriverRecord]] = []
    for root in _candidate_save_roots():
        profile_dir = root / "profiles"
        if not profile_dir.is_dir():
            continue
        for path in sorted(profile_dir.glob("*.json")):
            try:
                data = json.loads(path.read_text(encoding="utf-8"))
            except (OSError, ValueError):
                continue
            if not isinstance(data, dict) or "career" not in data:
                continue
            record = export_driver_from_save(data)
            label = (
                f"{record.name}: level {record.level}, "
                f"{record.deliveries} deliveries, reputation {record.reputation:.0f}, "
                f"{record.carrier_name or 'independent'}"
            )
            found.append((label, path, record))
    return found
