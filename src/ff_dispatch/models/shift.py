"""One dispatcher shift: a board of loads, a roster, and the consequences.

Pure logic on top of freightverse — no pygame, fully testable. The dispatcher
assigns loads to drivers under the same rules Freight Fate enforces on the
player (endorsements, legal hours, experience-gated haul length), then runs
the board and answers for the outcomes.
"""

from __future__ import annotations

import math
import random
from dataclasses import dataclass, field

from freightverse.models import JobBoard, Market, build_business_settlement, company_driver_pay
from freightverse.models.career import xp_class_multiplier
from freightverse.models.jobs import Job, route_required_hours
from freightverse.models.start_options import start_option
from freightverse.sim.hos import LIMITS

from .drivers import ActiveRun, RosterDriver

PICKUP_WORK_HOURS = 1.25  # check-in plus loading, same as Freight Fate
HOS_MODE = "realistic"
MIDRUN_BREAK_HOURS = 0.5  # the 30-minute break a long run absorbs en route
DAILY_DRIVE_HOURS = 11.0  # one full rested day at the wheel
# Dispatch policy: nobody rolls out on a multi-day run with less than a solid
# half-day of legal driving left on today's clock.
MIN_DEPARTURE_DRIVE_HOURS = 4.0


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
class Departure:
    """A driver rolling out on a haul too long to settle today."""

    driver: RosterDriver
    job: Job
    days_total: int

    def spoken(self) -> str:
        return (
            f"{self.driver.name} is rolling: {self.job.cargo.label} to "
            f"{self.job.destination}, about {self.days_total} days out. "
            f"The settlement lands when the load does."
        )


@dataclass
class ShiftSummary:
    outcomes: list[AssignmentOutcome] = field(default_factory=list)
    departures: list[Departure] = field(default_factory=list)
    unassigned_loads: int = 0

    @property
    def total_margin(self) -> float:
        return round(sum(o.margin for o in self.outcomes), 2)

    @property
    def on_time_count(self) -> int:
        return sum(1 for o in self.outcomes if o.on_time)

    def spoken(self) -> str:
        if not self.outcomes and not self.departures:
            return "Shift over. Nothing moved; the board went unworked."
        delivered = len(self.outcomes)
        rolling = (
            f" {len(self.departures)} driver"
            f"{'s' if len(self.departures) != 1 else ''} out on multi-day runs."
            if self.departures
            else ""
        )
        return (
            f"Shift over. {delivered} load{'s' if delivered != 1 else ''} delivered, "
            f"{self.on_time_count} on time. Company margin {self.total_margin:,.0f} "
            f"dollars.{rolling} {self.unassigned_loads} load"
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
        if driver.active_run is not None:
            days = driver.active_run.days_left
            return (
                f"{driver.name} is out on a run, back in about "
                f"{days} day{'s' if days != 1 else ''}."
            )
        if driver.out_today:
            return f"{driver.name} is out today."
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
        needed_h = self.required_hours(job)
        if needed_h is not None:
            if self.fits_today(driver, needed_h):
                return ""
            # Too long for today's clock: legal only as a multi-day run, and
            # dispatch will not start one on a nearly spent clock.
            if self.run_days(driver, needed_h) > 1:
                drive_left_h = self._drive_left_h(driver)
                if drive_left_h < MIN_DEPARTURE_DRIVE_HOURS:
                    return (
                        f"{driver.name} has {max(0.0, drive_left_h):.1f} legal hours "
                        f"left; dispatch will not start a multi-day run on that clock."
                    )
        return ""

    # -- hours math -----------------------------------------------------------

    @staticmethod
    def _drive_left_h(driver: RosterDriver) -> float:
        drive_limit, _duty_limit, _break_after = LIMITS[HOS_MODE]
        return (drive_limit - driver.hos.driving_min) / 60.0

    def fits_today(self, driver: RosterDriver, needed_h: float) -> bool:
        """Can the whole run legally settle inside today's clock?

        The 30-minute break pauses a long run rather than forbidding it: judge
        the assignment against the drive limit and the duty window, with the
        break folded into the duty math when the run is long enough to need one.
        """
        drive_limit, duty_limit, break_after = LIMITS[HOS_MODE]
        drive_left_h = (drive_limit - driver.hos.driving_min) / 60.0
        duty_left_h = (duty_limit - driver.hos.duty_min) / 60.0
        drive_needed_h = max(0.0, needed_h - PICKUP_WORK_HOURS)
        duty_needed_h = needed_h + (
            MIDRUN_BREAK_HOURS if drive_needed_h > break_after / 60.0 else 0.0
        )
        return drive_needed_h <= drive_left_h and duty_needed_h <= duty_left_h

    def run_days(self, driver: RosterDriver, needed_h: float) -> int:
        """Calendar days the run takes this driver, given today's clock."""
        if self.fits_today(driver, needed_h):
            return 1
        drive_needed_h = max(0.0, needed_h - PICKUP_WORK_HOURS)
        first_day_h = max(0.0, min(self._drive_left_h(driver), DAILY_DRIVE_HOURS))
        remaining_h = max(0.0, drive_needed_h - first_day_h)
        extra_days = max(1, math.ceil(remaining_h / DAILY_DRIVE_HOURS))
        return 1 + extra_days

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

    def pull_driver(self, driver: RosterDriver) -> None:
        """Take a driver off whatever they are covering (sick call, day off)."""
        if driver.assigned_job_index is not None:
            self.unassign(driver.assigned_job_index)

    def add_job(self, job: Job) -> int:
        """A load lands on the board mid-shift (a broker's hot load); its index."""
        self.jobs.append(job)
        return len(self.jobs) - 1

    # -- running the board --------------------------------------------------------

    def on_time_probability(self, driver: RosterDriver, job: Job) -> float:
        """Slack in the appointment plus the driver's record, clamped sane."""
        needed = self.required_hours(job) or (job.distance_mi / 50.0)
        slack_ratio = job.deadline_game_h / max(0.5, needed)
        p = 0.30 + 0.50 * driver.reliability + 0.20 * min(1.0, slack_ratio - 1.0)
        return max(0.05, min(0.98, p))

    def resolve(self) -> ShiftSummary:
        """Run the board: every assignment plays out and the roster evolves.

        Runs that fit today's clock settle now. Longer hauls become active
        runs: the outcome is decided here (same odds, same settlement math),
        but the driver is out on the road and the money and career effects
        land when the load does, days later, at the career's day boundary.
        """
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
            days = self.run_days(driver, needed)
            driver.assigned_job_index = None
            if days == 1:
                driver.hos.drive(min(DAILY_DRIVE_HOURS * 60.0, needed * 60.0))
                driver.career.record_delivery(
                    job.distance_mi,
                    settlement.net_before_advance,
                    on_time,
                    0.0,
                    cargo_class_mult=xp_class_multiplier(job.cargo),
                )
                summary.outcomes.append(
                    AssignmentOutcome(
                        driver=driver,
                        job=job,
                        on_time=on_time,
                        company_gross=job.pay,
                        driver_wage=wage,
                    )
                )
            else:
                driver.hos.drive(min(self._drive_left_h(driver), DAILY_DRIVE_HOURS) * 60.0)
                driver.active_run = ActiveRun(
                    cargo_label=job.cargo.label,
                    destination=job.destination,
                    distance_mi=job.distance_mi,
                    days_left=days - 1,
                    on_time=on_time,
                    company_gross=job.pay,
                    driver_wage=wage,
                    career_pay=settlement.net_before_advance,
                    cargo_mult=xp_class_multiplier(job.cargo),
                )
                summary.departures.append(Departure(driver=driver, job=job, days_total=days))
        self.assignments.clear()
        return summary
