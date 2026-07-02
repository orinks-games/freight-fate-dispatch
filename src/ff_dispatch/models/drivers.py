"""Driver roster: AI company drivers, plus imported Freight Fate veterans.

Each roster driver carries a real freightverse ``Career`` and ``HosClock``,
so reputation, endorsements, XP, and legal hours behave exactly as they do
for the player in Freight Fate — the dispatcher is just seeing them from the
other side of the phone.
"""

from __future__ import annotations

import random
from dataclasses import dataclass, field

from freightverse.exchange import DriverRecord
from freightverse.models import LEVEL_XP, Career
from freightverse.sim import HosClock

_FIRST_NAMES = (
    "Marcus",
    "Denise",
    "Ray",
    "Tanya",
    "Walt",
    "Priya",
    "Cole",
    "Ines",
    "Dwayne",
    "Sandra",
    "Otis",
    "Kim",
    "Hector",
    "June",
    "Earl",
    "Rosa",
)
_LAST_NAMES = (
    "Bell",
    "Okafor",
    "Trujillo",
    "Kowalski",
    "Haynes",
    "Delgado",
    "Moss",
    "Lindqvist",
    "Carter",
    "Ng",
    "Sloane",
    "Petrov",
    "Whitaker",
    "Boone",
)


@dataclass
class RosterDriver:
    """One driver on the dispatcher's board."""

    name: str
    career: Career = field(default_factory=Career)
    hos: HosClock = field(default_factory=HosClock)
    home_city: str = ""
    imported: bool = False
    assigned_job_index: int | None = None

    @property
    def level(self) -> int:
        return self.career.level

    @property
    def endorsements(self) -> set[str]:
        return self.career.endorsements

    @property
    def reliability(self) -> float:
        """0..1 blend of on-time history and dispatcher-facing reputation."""
        if self.career.deliveries > 0:
            history = self.career.on_time_deliveries / self.career.deliveries
        else:
            history = 0.85  # a new hire gets the benefit of the doubt, barely
        return max(0.0, min(1.0, 0.55 * history + 0.45 * (self.career.reputation / 100.0)))

    def summary(self, hos_mode: str = "realistic") -> str:
        rank = self.career.rank
        remaining = self.hos.remaining_min(hos_mode)
        hours = "fresh clock" if remaining is None else f"{remaining / 60.0:.1f} legal hours left"
        endorsements = ", ".join(sorted(self.endorsements)) or "none"
        source = " Imported Freight Fate driver." if self.imported else ""
        return (
            f"{self.name}: level {self.level} {rank.title}, reputation "
            f"{self.career.reputation:.0f}, {self.career.deliveries} deliveries, "
            f"{hours}. Endorsements: {endorsements}.{source}"
        )


def driver_from_record(record: DriverRecord) -> RosterDriver:
    """A Freight Fate veteran joins the board with their real history."""
    career = Career(
        xp=record.xp,
        reputation=record.reputation,
        deliveries=record.deliveries,
        on_time_deliveries=record.on_time_deliveries,
        on_time_streak=record.on_time_streak,
        total_miles=record.total_miles,
        total_earnings=record.total_earnings,
        purchased_endorsements=list(record.purchased_endorsements),
    )
    return RosterDriver(
        name=record.name,
        career=career,
        home_city=record.home_city,
        imported=True,
    )


def generate_roster(seed: int, count: int = 6) -> list[RosterDriver]:
    """A deterministic board of company drivers with plausible spreads."""
    rng = random.Random(seed)
    names = rng.sample(
        [f"{first} {last}" for first in _FIRST_NAMES for last in _LAST_NAMES],
        count,
    )
    roster: list[RosterDriver] = []
    for name in names:
        level = rng.choice((1, 2, 3, 4, 5, 6, 8, 9, 10, 12))
        deliveries = max(0, int(level * rng.uniform(3.0, 9.0)))
        on_time = int(deliveries * rng.uniform(0.75, 1.0))
        career = Career(
            xp=LEVEL_XP[level - 1] + rng.uniform(0, 500),
            reputation=round(rng.uniform(48.0, 92.0), 1),
            deliveries=deliveries,
            on_time_deliveries=on_time,
            total_miles=deliveries * rng.uniform(180.0, 900.0),
        )
        hos = HosClock()
        # most of the board starts fresh; a couple of drivers are mid-shift
        used_min = rng.choice((0.0, 0.0, 0.0, 0.0, 90.0, 240.0))
        if used_min:
            hos.drive(used_min)
        roster.append(RosterDriver(name=name, career=career, hos=hos))
    return roster
