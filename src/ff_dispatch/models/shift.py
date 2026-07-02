"""One dispatcher shift: a board of loads, a roster, and the consequences.

Pure logic on top of freightverse — no pygame, fully testable. The dispatcher
assigns loads to drivers under the same rules Freight Fate enforces on the
player (endorsements, legal hours, experience-gated haul length), then runs
the board and answers for the outcomes.
"""

from __future__ import annotations

import random
from dataclasses import dataclass, field

from freightverse.models import JobBoard, Market, build_business_settlement, company_driver_pay
from freightverse.models.career import xp_class_multiplier
from freightverse.models.jobs import Job, route_required_hours
from freightverse.models.start_options import start_option

from .drivers import RosterDriver

PICKUP_WORK_HOURS = 1.25  # check-in plus loading, same as Freight Fate
HOS_MODE = "realistic"


@dataclass
class AssignmentOutcome:
    driver: RosterDriver
    job: Job
    on_time: bool
    company_gross: float
    driver_wage: float

    @property
    def margin(self) -> float:
        return round(self.company_gross - self.driver_wage, 2)

    def spoken(self) -> str:
        timing = "on time" if self.on_time else "late"
        return (
            f"{self.driver.name} delivered {self.job.cargo.label} to "
            f"{self.job.destination}, {timing}. Gross {self.company_gross:,.0f}, "
            f"driver pay {self.driver_wage:,.0f}, margin {self.margin:,.0f} dollars."
        )


@dataclass
class ShiftSummary:
    outcomes: list[AssignmentOutcome] = field(default_factory=list)
    unassigned_loads: int = 0

    @property
    def total_margin(self) -> float:
        return round(sum(o.margin for o in self.outcomes), 2)

    @property
    def on_time_count(self) -> int:
        return sum(1 for o in self.outcomes if o.on_time)

    def spoken(self) -> str:
        if not self.outcomes:
            return "Shift over. Nothing moved; the board went unworked."
        delivered = len(self.outcomes)
        return (
            f"Shift over. {delivered} load{'s' if delivered != 1 else ''} delivered, "
            f"{self.on_time_count} on time. Company margin {self.total_margin:,.0f} "
            f"dollars. {self.unassigned_loads} load"
            f"{'s' if self.unassigned_loads != 1 else ''} left on the board."
        )


class DispatchShift:
    """The dispatcher's working state for one shift at one terminal."""

    def __init__(
        self,
        world,
        *,
        carrier_key: str,
        city: str | None = None,
        seed: int = 0,
        drivers: list[RosterDriver] | None = None,
        market: Market | None = None,
        board_size_level: int = 9,
    ) -> None:
        self.world = world
        self.carrier_key = carrier_key
        self.option = start_option(carrier_key)
        self.city = city or self.option.default_city
        self.seed = seed
        self.market = market or Market(seed=seed)
        self._rng = random.Random(seed ^ 0xD15FA7C4)
        self.drivers: list[RosterDriver] = drivers or []
        # The dispatcher sees the whole board; per-driver rules gate who can
        # actually haul each load. A senior generation level keeps specialty
        # freight in the mix.
        self.jobs: list[Job] = JobBoard(world, seed=seed).offers(
            self.city,
            {"refrigerated", "heavy_haul", "high_value"},
            level=board_size_level,
            market=self.market,
            carrier_key=carrier_key,
        )
        self.assignments: dict[int, RosterDriver] = {}

    # -- assignment rules ---------------------------------------------------------

    def refusal_reason(self, driver: RosterDriver, job: Job) -> str:
        """Why this driver cannot legally or sensibly take this load ('' = can)."""
        if driver.assigned_job_index is not None:
            return f"{driver.name} already has a load."
        missing = {job.cargo.endorsement} - driver.endorsements if job.cargo.endorsement else set()
        if job.cargo.min_level > driver.level:
            return f"Needs a level {job.cargo.min_level} driver; {driver.name} is level {driver.level}."
        if missing:
            return f"{driver.name} lacks the {job.cargo.endorsement.replace('_', ' ')} endorsement."
        if job.distance_mi > JobBoard.distance_cap(driver.level):
            return (
                f"That haul is {job.distance_mi:.0f} miles; dispatch policy caps a "
                f"level {driver.level} driver at {JobBoard.distance_cap(driver.level):.0f}."
            )
        remaining = driver.hos.remaining_min(HOS_MODE)
        needed_h = self.required_hours(job)
        if (
            remaining is not None
            and needed_h is not None
            and remaining / 60.0 < min(11.0, needed_h)
        ):
            return (
                f"{driver.name} has {remaining / 60.0:.1f} legal hours left; the run "
                f"needs about {needed_h:.1f} before a reset."
            )
        return ""

    def required_hours(self, job: Job) -> float | None:
        route = self.world.supported_route(job.origin, job.destination)
        if route is None:
            return None
        return PICKUP_WORK_HOURS + route_required_hours(route, world=self.world)

    def assignable_drivers(self, job_index: int) -> list[RosterDriver]:
        job = self.jobs[job_index]
        return [d for d in self.drivers if not self.refusal_reason(d, job)]

    def assign(self, job_index: int, driver: RosterDriver) -> str:
        job = self.jobs[job_index]
        reason = self.refusal_reason(driver, job)
        if reason:
            return reason
        if job_index in self.assignments:
            return "That load is already covered."
        driver.assigned_job_index = job_index
        self.assignments[job_index] = driver
        return ""

    def unassign(self, job_index: int) -> None:
        driver = self.assignments.pop(job_index, None)
        if driver is not None:
            driver.assigned_job_index = None

    # -- running the board --------------------------------------------------------

    def on_time_probability(self, driver: RosterDriver, job: Job) -> float:
        """Slack in the appointment plus the driver's record, clamped sane."""
        needed = self.required_hours(job) or (job.distance_mi / 50.0)
        slack_ratio = job.deadline_game_h / max(0.5, needed)
        p = 0.30 + 0.50 * driver.reliability + 0.20 * min(1.0, slack_ratio - 1.0)
        return max(0.05, min(0.98, p))

    def resolve(self) -> ShiftSummary:
        """Run the board: every assignment plays out and the roster evolves."""
        summary = ShiftSummary(unassigned_loads=len(self.jobs) - len(self.assignments))
        for job_index in sorted(self.assignments):
            driver = self.assignments[job_index]
            job = self.jobs[job_index]
            on_time = self._rng.random() < self.on_time_probability(driver, job)
            settlement = build_business_settlement(
                "company_driver",
                job,
                job.pay,
                on_time=on_time,
                driver_charges=0.0,
                carrier_key=self.carrier_key,
                reputation=driver.career.reputation,
            )
            wage = company_driver_pay(
                job, job.pay, on_time, self.carrier_key, driver.career.reputation
            )
            needed = self.required_hours(job) or (job.distance_mi / 50.0)
            driver.hos.drive(min(11.0 * 60.0, needed * 60.0))
            driver.career.record_delivery(
                job.distance_mi,
                settlement.net_before_advance,
                on_time,
                0.0,
                cargo_class_mult=xp_class_multiplier(job.cargo),
            )
            driver.assigned_job_index = None
            summary.outcomes.append(
                AssignmentOutcome(
                    driver=driver,
                    job=job,
                    on_time=on_time,
                    company_gross=job.pay,
                    driver_wage=wage,
                )
            )
        self.assignments.clear()
        return summary
