"""Dispatch career persistence: this game's own saves, day-granular.

Careers save at day boundaries (end of day, quit to title), so a reloaded
career always wakes up at the start of a morning and the seeded board
regenerates identically. Freight Fate driver saves stay read-only over in
``saves.py``; nothing here ever touches them.

Files live under ``FF_DISPATCH_DATA_DIR`` when set (tests), else the
platform's app-data directory, as plain versioned JSON with atomic writes.
"""

from __future__ import annotations

import json
import logging
import os
import sys
from pathlib import Path

from .models.career_state import DispatchCareer

log = logging.getLogger(__name__)

APP_DIR_NAME = "FreightFateDispatch"


def data_dir() -> Path:
    env = os.environ.get("FF_DISPATCH_DATA_DIR")
    if env:
        return Path(env)
    home = Path.home()
    if sys.platform == "win32":
        appdata = os.environ.get("APPDATA")
        base = Path(appdata) if appdata else home / "AppData" / "Roaming"
        return base / APP_DIR_NAME
    if sys.platform == "darwin":
        return home / "Library" / "Application Support" / APP_DIR_NAME
    xdg = os.environ.get("XDG_DATA_HOME", str(home / ".local" / "share"))
    return Path(xdg) / APP_DIR_NAME


def careers_dir() -> Path:
    return data_dir() / "careers"


def career_path(career: DispatchCareer) -> Path:
    return careers_dir() / f"{career.carrier_key}-{career.seed}.json"


def save_career(career: DispatchCareer) -> Path:
    """Write the career atomically; a crash mid-write never eats the save."""
    path = career_path(career)
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(".json.tmp")
    tmp.write_text(
        json.dumps(career.to_payload(), indent=2, sort_keys=True),
        encoding="utf-8",
    )
    tmp.replace(path)
    return path


def load_career(path: Path) -> DispatchCareer | None:
    try:
        data = json.loads(Path(path).read_text(encoding="utf-8"))
    except (OSError, ValueError):
        log.warning("Unreadable career save: %s", path)
        return None
    return DispatchCareer.from_payload(data)


def list_careers() -> list[tuple[str, Path, DispatchCareer]]:
    """(spoken label, path, career) for every readable save, newest day first."""
    folder = careers_dir()
    if not folder.is_dir():
        return []
    found: list[tuple[str, Path, DispatchCareer]] = []
    for path in sorted(folder.glob("*.json")):
        career = load_career(path)
        if career is not None:
            found.append((career.save_label(), path, career))
    found.sort(key=lambda item: item[2].day, reverse=True)
    return found


def delete_career(path: Path) -> bool:
    try:
        Path(path).unlink()
        return True
    except OSError:
        return False
