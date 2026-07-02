"""Title and career start: pick your carrier, or import a Freight Fate driver."""

from __future__ import annotations

from freightverse.exchange import DriverRecord
from freightverse.models.start_options import START_OPTIONS

from .. import __version__
from ..models import driver_from_record, generate_roster
from ..models.shift import DispatchShift
from ..saves import find_freight_fate_drivers
from .base import MenuItem, MenuState

DEFAULT_CARRIER_KEY = "northstar"
ROSTER_SEED = 20260702


def _company_options():
    return [option for option in START_OPTIONS.values() if option.is_company_driver]


class TitleState(MenuState):
    title = "Freight Fate: Dispatch"

    def announce_entry(self) -> None:
        self.ctx.say(
            f"Freight Fate: Dispatch, version {__version__}. The other side of "
            f"the board. {self.current_text()}"
        )

    def build_items(self) -> list[MenuItem]:
        return [
            MenuItem(
                "New dispatcher career",
                self._new_career,
                help="Choose which carrier you dispatch for and take your first shift.",
            ),
            MenuItem(
                "Import a Freight Fate driver",
                self._import_driver,
                help="Bring a driver from your Freight Fate save onto the board, and "
                "dispatch for the carrier they drive for.",
            ),
            MenuItem("Quit", self._quit, help="Exit the game."),
        ]

    def _new_career(self) -> None:
        self.ctx.push_state(CarrierSelectState(self.ctx))

    def _import_driver(self) -> None:
        self.ctx.push_state(ImportDriverState(self.ctx))

    def _quit(self) -> None:
        self.ctx.pop_state()

    def go_back(self) -> None:
        self.ctx.say("Use Quit to exit.")


class CarrierSelectState(MenuState):
    title = "Choose your carrier"
    intro_help = (
        "Pick the carrier whose dispatch desk you run. Each carrier keeps its "
        "Freight Fate character: pay plan, freight emphasis, and deadline "
        "style shape the board you will work. Escape returns to the title."
    )

    def build_items(self) -> list[MenuItem]:
        items = [
            MenuItem(
                f"{option.carrier_name}: {option.menu_summary}",
                lambda key=option.key: self._start(key),
                help=option.help_text,
            )
            for option in _company_options()
        ]
        items.append(MenuItem("Back", self.go_back, help="Return to the title menu."))
        return items

    def _start(self, carrier_key: str) -> None:
        start_shift(self.ctx, carrier_key=carrier_key)


class ImportDriverState(MenuState):
    title = "Import a Freight Fate driver"
    intro_help = (
        "Freight Fate careers found on this machine. Importing puts that "
        "driver on your board and seats you at their carrier's dispatch desk. "
        "Their save is read, never changed. Escape returns to the title."
    )

    def __init__(self, ctx) -> None:
        super().__init__(ctx)
        self._found = find_freight_fate_drivers()

    def announce_entry(self) -> None:
        if not self._found:
            self.ctx.say(
                "No Freight Fate careers were found on this machine. Set the "
                "FF_DISPATCH_FREIGHT_FATE_SAVES environment variable to your "
                "Freight Fate saves folder if it lives somewhere unusual. " + self.current_text()
            )
            return
        n = len(self._found)
        self.ctx.say(
            f"Found {n} Freight Fate career{'s' if n != 1 else ''}. "
            f"{self.intro_help} {self.current_text()}"
        )

    def build_items(self) -> list[MenuItem]:
        items = [
            MenuItem(
                label,
                lambda record=record: self._import(record),
                help="Import this driver and dispatch for their carrier.",
            )
            for label, _path, record in self._found
        ]
        items.append(MenuItem("Back", self.go_back, help="Return to the title menu."))
        return items

    def _import(self, record: DriverRecord) -> None:
        carrier_key = record.carrier_key or DEFAULT_CARRIER_KEY
        if carrier_key not in START_OPTIONS or not START_OPTIONS[carrier_key].is_company_driver:
            carrier_key = DEFAULT_CARRIER_KEY
        self.ctx.say(
            f"{record.name} joins the board: level {record.level}, "
            f"reputation {record.reputation:.0f}, "
            f"{record.deliveries} deliveries on record.",
            interrupt=True,
        )
        start_shift(
            self.ctx,
            carrier_key=carrier_key,
            imported=record,
            city=record.home_city or None,
        )


def start_shift(
    ctx, *, carrier_key: str, imported: DriverRecord | None = None, city: str | None = None
) -> None:
    from .shift import ShiftState

    drivers = generate_roster(ROSTER_SEED, count=6)
    if imported is not None:
        drivers.insert(0, driver_from_record(imported))
    if city is not None and city not in ctx.world.cities:
        city = None
    shift = DispatchShift(
        ctx.world,
        carrier_key=carrier_key,
        city=city,
        seed=ROSTER_SEED,
        drivers=drivers,
    )
    ctx.reset_to(ShiftState(ctx, shift))
