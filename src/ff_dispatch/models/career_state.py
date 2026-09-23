"""The dispatcher's ongoing career: days, funds, roster, market, record.

Pure logic, no pygame. One ``DispatchCareer`` owns everything that survives
between shifts: the roster (each driver a real freightverse ``Career`` and
``HosClock``), the seeded ``Market`` that drifts day by day, the terminal
operating funds the margins feed, and the dispatcher's own service record.
``career_saves`` turns the payload into a file; this module never touches
disk.
"""

from __future__ import annotations

import random
import time
from dataclasses import dataclass, field

from freightverse.models import Market
from freightverse.models.start_options import start_option

from .drivers import (
    RosterDriver,
    driver_from_payload,
    driver_from_record,
    driver_payload,
    generate_roster,
)
from .shift import DispatchShift, ShiftSummary

CAREER_SCHEMA = 1
STARTING_FUNDS = 2_500.0  # the terminal's operating float on day one
STARTING_ROSTER = 6
ROSTER_CAP = 10


@dataclass
class ServiceRecord:
    """The numbers the dispatcher answers for."""

    shifts_worked: int = 0
    loads_delivered: int = 0
    on_time_deliveries: int = 0
    loads_left_on_board: int = 0
    total_margin: float = 0.0

    def spoken(self) -> str:
        if self.shifts_worked == 0:
            return "No shifts on your record yet."
        pct = (
            100.0 * self.on_time_deliveries / self.loads_delivered
            if self.loads_delivered
            else 100.0
        )
        return (
            f"{self.shifts_worked} shift{'s' if self.shifts_worked != 1 else ''} worked. "
            f"{self.loads_delivered} loads delivered, {pct:.0f} percent on time. "
            f"{self.loads_left_on_board} loads left on the board. "
            f"Lifetime margin {self.total_margin:,.0f} dollars."
        )

    def to_payload(self) -> dict:
        return {
            "shifts_worked": self.shifts_worked,
            "loads_delivered": self.loads_delivered,
            "on_time_deliveries": self.on_time_deliveries,
            "loads_left_on_board": self.loads_left_on_board,
            "total_margin": self.total_margin,
        }

    @classmethod
    def from_payload(cls, data) -> ServiceRecord:
        if not isinstance(data, dict):
            return cls()
        try:
            return cls(
                shifts_worked=int(data.get("shifts_worked", 0)),
                loads_delivered=int(data.get("loads_delivered", 0)),
                on_time_deliveries=int(data.get("on_time_deliveries", 0)),
                loads_left_on_board=int(data.get("loads_left_on_board", 0)),
                total_margin=float(data.get("total_margin", 0.0)),
            )
        except (TypeError, ValueError):
            return cls()


@dataclass
class DispatchCareer:
    """Everything about one dispatcher career that outlives a single shift."""

    carrier_key: str
    city: str
    seed: int
    day: int = 1
    funds: float = STARTING_FUNDS
    roster: list[RosterDriver] = field(default_factory=list)
    market: Market = None  # type: ignore[assignment]  # set in __post_init__
    record: ServiceRecord = field(default_factory=ServiceRecord)
    overnight_news: list[str] = field(default_factory=list)
    radio_enabled: bool = False
    radio_station_id: str = ""
    created_at: str = ""

    def __post_init__(self) -> None:
        if self.market is None:
            self.market = Market(seed=self.seed)
        if not self.created_at:
            self.created_at = time.strftime("%Y-%m-%dT%H:%M:%S")

    @property
    def option(self):
        return start_option(self.carrier_key)

    @classmethod
    def new(
        cls,
        *,
        carrier_key: str,
        city: str | None = None,
        seed: int | None = None,
        imported=None,
    ) -> DispatchCareer:
        """A fresh desk: seeded roster, day-one market, the starting float."""
        seed = random.randrange(2**31) if seed is None else seed
        option = start_option(carrier_key)
        roster = generate_roster(seed, count=STARTING_ROSTER)
        if imported is not None:
            roster.insert(0, driver_from_record(imported))
        return cls(
            carrier_key=option.key,
            city=city or option.default_city,
            seed=seed,
            roster=roster,
        )

    # -- the day loop ---------------------------------------------------------

    def day_seed(self) -> int:
        """Deterministic per-day seed so a reloaded morning replays identically."""
        return (self.seed ^ (self.day * 0x9E3779B1)) & 0x7FFFFFFF

    def build_shift(self, world) -> DispatchShift:
        """This morning's board at the terminal."""
        self.market.advance_to(self.day)
        return DispatchShift(
            world,
            carrier_key=self.carrier_key,
            city=self.city,
            seed=self.day_seed(),
            drivers=self.roster,
            market=self.market,
        )

    def record_shift(self, summary: ShiftSummary) -> None:
        """The settled board goes on the record and the margin hits the funds."""
        self.funds = round(self.funds + summary.total_margin, 2)
        self.record.shifts_worked += 1
        self.record.loads_delivered += len(summary.outcomes)
        self.record.on_time_deliveries += summary.on_time_count
        self.record.loads_left_on_board += summary.unassigned_loads
        self.record.total_margin = round(self.record.total_margin + summary.total_margin, 2)

    def record_skipped_board(self, load_count: int) -> None:
        """Walking away from an unworked board still goes on the record."""
        self.record.shifts_worked += 1
        self.record.loads_left_on_board += load_count

    def end_day(self) -> None:
        """Overnight: rests, market drift, and multi-day runs coming home.

        An active run whose days hit zero settles here — the outcome was
        decided at dispatch; the money, the delivery, and the driver's career
        effects land now, and the morning announces it as overnight news.
        """
        for driver in self.roster:
            run = driver.active_run
            if run is not None:
                run.days_left -= 1
                if run.days_left <= 0:
                    driver.career.record_delivery(
                        run.distance_mi,
                        run.career_pay,
                        run.on_time,
                        0.0,
                        cargo_class_mult=run.cargo_mult,
                    )
                    self.funds = round(self.funds + run.margin, 2)
                    self.record.loads_delivered += 1
                    if run.on_time:
                        self.record.on_time_deliveries += 1
                    self.record.total_margin = round(self.record.total_margin + run.margin, 2)
                    timing = "on time" if run.on_time else "late"
                    self.overnight_news.append(
                        f"Overnight: {driver.name} delivered {run.cargo_label} to "
                        f"{run.destination}, {timing}. Margin {run.margin:,.0f} dollars."
                    )
                    driver.active_run = None
            driver.hos.sleep(status="off_duty")
            driver.out_today = False
            driver.assigned_job_index = None
        self.day += 1
        self.market.advance_to(self.day)

    def take_overnight_news(self) -> list[str]:
        """The morning's news, spoken once and then gone."""
        news = self.overnight_news
        self.overnight_news = []
        return news

    # -- spoken summaries -------------------------------------------------------

    def morning_spoken(self) -> str:
        available = sum(1 for d in self.roster if not d.out_today and d.active_run is None)
        return (
            f"Day {self.day} at {self.option.carrier_name} dispatch, {self.city}. "
            f"Funds {self.funds:,.0f} dollars. {available} of {len(self.roster)} "
            f"drivers available. {self.market.summary()}"
        )

    def save_label(self) -> str:
        return (
            f"{self.option.carrier_name} desk, {self.city}: day {self.day}, "
            f"{len(self.roster)} drivers, {self.funds:,.0f} dollars"
        )

    # -- serialization -----------------------------------------------------------

    def to_payload(self) -> dict:
        return {
            "schema": CAREER_SCHEMA,
            "carrier_key": self.carrier_key,
            "city": self.city,
            "seed": self.seed,
            "day": self.day,
            "funds": self.funds,
            "created_at": self.created_at,
            "record": self.record.to_payload(),
            "overnight_news": list(self.overnight_news),
            "market": {
                "seed": self.market.seed,
                "day": self.market.day,
                "multipliers": dict(self.market.multipliers),
            },
            "radio": {
                "enabled": self.radio_enabled,
                "station_id": self.radio_station_id,
            },
            "drivers": [driver_payload(d) for d in self.roster],
        }

    @classmethod
    def from_payload(cls, data) -> DispatchCareer | None:
        """Tolerant load; None only when the payload is not a career at all."""
        if not isinstance(data, dict) or "carrier_key" not in data:
            return None
        try:
            seed = int(data.get("seed", 0))
            raw_market = data.get("market")
            if isinstance(raw_market, dict):
                multipliers = raw_market.get("multipliers")
                market = Market(
                    seed=int(raw_market.get("seed", seed)),
                    day=int(raw_market.get("day", 0)),
                    multipliers=(
                        {str(k): float(v) for k, v in multipliers.items()}
                        if isinstance(multipliers, dict)
                        else {}
                    ),
                )
            else:
                market = Market(seed=seed)
            roster = [
                driver
                for raw in data.get("drivers", [])
                if (driver := driver_from_payload(raw)) is not None
            ]
            radio = data.get("radio") if isinstance(data.get("radio"), dict) else {}
            option = start_option(str(data["carrier_key"]))
            return cls(
                carrier_key=option.key,
                city=str(data.get("city") or option.default_city),
                seed=seed,
                day=max(1, int(data.get("day", 1))),
                funds=float(data.get("funds", STARTING_FUNDS)),
                roster=roster,
                market=market,
                record=ServiceRecord.from_payload(data.get("record")),
                overnight_news=[str(line) for line in data.get("overnight_news", [])],
                radio_enabled=bool(radio.get("enabled", False)),
                radio_station_id=str(radio.get("station_id", "")),
                created_at=str(data.get("created_at", "")),
            )
        except (TypeError, ValueError, AttributeError):
            return None
