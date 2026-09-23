"""The terminal radio: freightverse's in-cab radio, sitting on the desk.

Speech-only for now — no playback backend — so every action speaks the same
``RadioAction`` messages Freight Fate players hear in the cab: station,
format, and signal. Reception is judged from the terminal city's coordinates,
and streamer-safe defaults keep real network streams out entirely. What the
dispatcher last tuned persists on the career.
"""

from __future__ import annotations

from freightverse.radio import RadioState

from ..models.career_state import DispatchCareer
from .base import MenuItem, MenuState


def desk_radio(ctx, career: DispatchCareer) -> RadioState:
    """The desk set, tuned how the career left it, placed at the terminal."""
    radio = RadioState(
        enabled=career.radio_enabled,
        station_id=career.radio_station_id or "route_playlist",
    )
    city = ctx.world.cities.get(career.city)
    if city is not None:
        radio.update_position((city.lat, city.lon))
    return radio


class RadioMenuState(MenuState):
    title = "Terminal radio"
    intro_help = (
        "The radio on the dispatch desk. Toggle it, tune across whatever "
        "reaches the terminal, or hear the station list. Escape returns to "
        "the desk."
    )

    def __init__(self, ctx, career: DispatchCareer) -> None:
        super().__init__(ctx)
        self.career = career
        self.radio = desk_radio(ctx, career)

    def announce_entry(self) -> None:
        self.ctx.say(f"Terminal radio. {self.radio.status_text()} {self.current_text()}")

    def build_items(self) -> list[MenuItem]:
        return [
            MenuItem(
                lambda: f"Radio power: {'on' if self.radio.enabled else 'off'}",
                self._toggle,
                help="Turn the desk radio on or off.",
            ),
            MenuItem("Next station", lambda: self._tune(1), help="Tune up the dial."),
            MenuItem("Previous station", lambda: self._tune(-1), help="Tune down the dial."),
            MenuItem(
                lambda: f"Station list: {len(self.radio.receivable_stations())} receivable",
                self._list,
                help="Hear every station the terminal can receive right now.",
            ),
            MenuItem("Radio status", self._status, help="Hear the full radio status."),
            MenuItem("Back to the desk", self.go_back),
        ]

    def _remember(self) -> None:
        self.career.radio_enabled = self.radio.enabled
        self.career.radio_station_id = self.radio.station_id

    def _toggle(self) -> None:
        action = self.radio.toggle()
        self._remember()
        self.refresh()
        self.ctx.say(action.message)

    def _tune(self, direction: int) -> None:
        action = self.radio.tune(direction)
        self._remember()
        self.refresh()
        self.ctx.say(action.message)

    def _list(self) -> None:
        for line in self.radio.station_list_lines():
            self.ctx.say(line, interrupt=False)

    def _status(self) -> None:
        self.ctx.say(self.radio.status_text())
