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
class ActiveRun:
    """A multi-day haul in progress: decided at dispatch, settled on arrival."""

    cargo_label: str
    destination: str
    distance_mi: float
    days_left: int
    on_time: bool
    company_gross: float
    driver_wage: float
    career_pay: float  # settlement net credited to the driver's career record
    cargo_mult: float = 1.0

    @property
    def margin(self) -> float:
        return round(self.company_gross - self.driver_wage, 2)

    def to_dict(self) -> dict:
        return {
            "cargo_label": self.cargo_label,
            "destination": self.destination,
            "distance_mi": self.distance_mi,
            "days_left": self.days_left,
            "on_time": self.on_time,
            "company_gross": self.company_gross,
            "driver_wage": self.driver_wage,
            "career_pay": self.career_pay,
            "cargo_mult": self.cargo_mult,
        }

    @classmethod
    def from_dict(cls, data) -> ActiveRun | None:
        if not isinstance(data, dict):
            return None
        try:
            return cls(
                cargo_label=str(data.get("cargo_label", "freight")),
                destination=str(data.get("destination", "")),
                distance_mi=float(data.get("distance_mi", 0.0)),
                days_left=max(0, int(data.get("days_left", 0))),
                on_time=bool(data.get("on_time", False)),
                company_gross=float(data.get("company_gross", 0.0)),
                driver_wage=float(data.get("driver_wage", 0.0)),
                career_pay=float(data.get("career_pay", 0.0)),
                cargo_mult=float(data.get("cargo_mult", 1.0)),
            )
        except (TypeError, ValueError):
            return None


@dataclass
class RosterDriver:
    """One driver on the dispatcher's board."""

    name: str
    career: Career = field(default_factory=Career)
    hos: HosClock = field(default_factory=HosClock)
    home_city: str = ""
    imported: bool = False
    hired_day: int = 1
    out_today: bool = False  # called out sick or granted the day; back tomorrow
    active_run: ActiveRun | None = None  # out on a multi-day haul
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
        if self.active_run is not None:
            run = self.active_run
            days = f"{run.days_left} day{'s' if run.days_left != 1 else ''}"
            hours = f"out on a run: {run.cargo_label} to {run.destination}, back in about {days}"
        else:
            remaining = self.hos.remaining_min(hos_mode)
            hours = (
                "fresh clock" if remaining is None else f"{remaining / 60.0:.1f} legal hours left"
            )
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


_CAREER_FIELDS = (
    "xp",
    "reputation",
    "deliveries",
    "on_time_deliveries",
    "total_miles",
    "total_earnings",
    "dispatch_declines_used",
    "on_time_streak",
)


def driver_payload(driver: RosterDriver) -> dict:
    """A plain-JSON snapshot of one roster driver for the career save."""
    career = {name: getattr(driver.career, name) for name in _CAREER_FIELDS}
    career["purchased_endorsements"] = list(driver.career.purchased_endorsements)
    return {
        "name": driver.name,
        "home_city": driver.home_city,
        "imported": driver.imported,
        "hired_day": driver.hired_day,
        "career": career,
        "hos": driver.hos.to_dict(),
        "active_run": driver.active_run.to_dict() if driver.active_run else None,
    }


def driver_from_payload(data: dict) -> RosterDriver | None:
    """Tolerant load: an unreadable driver is dropped rather than crashing."""
    if not isinstance(data, dict) or not str(data.get("name", "")).strip():
        return None
    raw_career = data.get("career")
    if not isinstance(raw_career, dict):
        raw_career = {}
    try:
        career = Career(
            xp=float(raw_career.get("xp", 0.0)),
            reputation=float(raw_career.get("reputation", 50.0)),
            deliveries=int(raw_career.get("deliveries", 0)),
            on_time_deliveries=int(raw_career.get("on_time_deliveries", 0)),
            total_miles=float(raw_career.get("total_miles", 0.0)),
            total_earnings=float(raw_career.get("total_earnings", 0.0)),
            dispatch_declines_used=int(raw_career.get("dispatch_declines_used", 0)),
            on_time_streak=int(raw_career.get("on_time_streak", 0)),
            purchased_endorsements=[
                str(e) for e in (raw_career.get("purchased_endorsements") or [])
            ],
        )
        return RosterDriver(
            name=str(data["name"]),
            career=career,
            hos=HosClock.from_dict(data.get("hos")),
            home_city=str(data.get("home_city", "")),
            imported=bool(data.get("imported", False)),
            hired_day=int(data.get("hired_day", 1)),
            active_run=ActiveRun.from_dict(data.get("active_run")),
        )
    except (TypeError, ValueError):
        return None


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
