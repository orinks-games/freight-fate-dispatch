"""The dispatcher's phone: driver calls and broker hot loads, mid-shift.

Pure logic on top of the shift. Calls are scheduled deterministically per
career day (seconds of desk time), so a reloaded morning rings the same way.
The state layer owns the clock; it feeds elapsed desk seconds to
``CallSchedule.ring`` and presents each call's choices as a menu. Applying a
choice mutates the shift and career through the same rules everything else
uses — a driver sent home refuses loads exactly like any unavailable driver.
"""

from __future__ import annotations

import random
from dataclasses import dataclass, replace

from freightverse.models import JobBoard
from freightverse.models.jobs import Job

from .career_state import DispatchCareer
from .drivers import RosterDriver
from .shift import DispatchShift

CALL_OUT = "call_out"
DAY_OFF_REQUEST = "day_off_request"
HOT_LOAD = "hot_load"

HOT_LOAD_PAY_BONUS = 1.35  # brokers pay a premium to cover a dropped load
HOT_LOAD_DEADLINE_SQUEEZE = 0.85  # and the appointment is tighter for it
GOODWILL_GAIN = 2.0  # reputation for a request granted
GOODWILL_LOSS = 3.0  # reputation for a request refused
TOUGH_IT_OUT_LOSS = 4.0  # reputation for making a sick driver work

MAX_CALLS_PER_DAY = 2
_CALL_WINDOWS = ((20.0, 50.0), (75.0, 130.0))  # desk seconds for call 1, call 2


@dataclass
class Choice:
    key: str
    label: str
    help: str


@dataclass
class DispatcherCall:
    """One incoming call, waiting on the dispatcher's answer."""

    kind: str
    driver: RosterDriver | None = None
    job: Job | None = None
    job_index: int | None = None  # set once a hot load is on the board

    def ring_spoken(self) -> str:
        if self.kind == HOT_LOAD:
            return "The phone is ringing: a broker is on the line."
        name = self.driver.name if self.driver else "A driver"
        return f"The phone is ringing: {name} is on the line."

    def spoken(self) -> str:
        if self.kind == CALL_OUT:
            return (
                f"{self.driver.name} on the line: I can barely stand up, boss. "
                "I don't think I should be behind the wheel today."
            )
        if self.kind == DAY_OFF_REQUEST:
            return (
                f"{self.driver.name} on the line: I need a personal day. "
                "Family thing. Can you cover the board without me?"
            )
        assert self.job is not None
        return (
            "Broker on the line: a carrier dropped a load and the shipper is "
            f"desperate. {self.job.describe(pay_label='Premium pays')}"
        )

    def choices(self) -> list[Choice]:
        if self.kind == CALL_OUT:
            return [
                Choice(
                    "send_home",
                    f"Send {self.driver.name} home for the day",
                    "They sit today out and come back rested tomorrow. Any load "
                    "they were covering goes back on the board.",
                ),
                Choice(
                    "tough_it_out",
                    "Ask them to tough it out",
                    "They stay available, but a sick driver at the wheel costs "
                    "their reputation and worsens their on-time odds.",
                ),
            ]
        if self.kind == DAY_OFF_REQUEST:
            return [
                Choice(
                    "grant",
                    f"Give {self.driver.name} the day",
                    "They sit today out; goodwill nudges their reputation up. Any "
                    "load they were covering goes back on the board.",
                ),
                Choice(
                    "refuse",
                    "Tell them the board comes first",
                    "They stay available today, but the refusal costs reputation.",
                ),
            ]
        return [
            Choice(
                "take",
                "Take the load for the board",
                "The premium load lands on your board; cover it like any other.",
            ),
            Choice(
                "pass",
                "Pass on it",
                "The broker shops it elsewhere. No harm done.",
            ),
        ]


def apply_choice(
    call: DispatcherCall,
    choice_key: str,
    shift: DispatchShift,
    career: DispatchCareer,
) -> str:
    """Resolve the dispatcher's answer; returns the spoken outcome."""
    if call.kind in (CALL_OUT, DAY_OFF_REQUEST):
        driver = call.driver
        assert driver is not None
        had_load = driver.assigned_job_index is not None
        if choice_key in ("send_home", "grant"):
            shift.pull_driver(driver)
            driver.out_today = True
            if call.kind == DAY_OFF_REQUEST:
                driver.career.reputation = min(100.0, driver.career.reputation + GOODWILL_GAIN)
            back = " Their load is back on the board." if had_load else ""
            return f"{driver.name} is out for the day.{back}"
        loss = TOUGH_IT_OUT_LOSS if call.kind == CALL_OUT else GOODWILL_LOSS
        driver.career.reputation = max(0.0, driver.career.reputation - loss)
        if call.kind == CALL_OUT:
            return (
                f"{driver.name} stays on the board, unhappy about it. "
                "Their reputation takes the hit, and so do their odds."
            )
        return f"{driver.name} goes quiet and stays on the board. That one will be remembered."
    assert call.job is not None
    if choice_key == "take":
        call.job_index = shift.add_job(call.job)
        return (
            f"The hot load is on your board: {call.job.cargo.label} to "
            f"{call.job.destination}, {call.job.pay:,.0f} dollars."
        )
    return "You pass. The broker hangs up and shops it down the street."


def _hot_load(shift: DispatchShift, rng: random.Random) -> Job | None:
    """A premium load built from the same generator as the morning board."""
    offers = JobBoard(shift.world, seed=rng.randrange(2**31)).offers(
        shift.city,
        {"refrigerated", "heavy_haul", "high_value"},
        level=9,
        market=shift.market,
        carrier_key=shift.carrier_key,
    )
    if not offers:
        return None
    job = max(offers, key=lambda j: j.pay)
    return replace(
        job,
        pay=round(job.pay * HOT_LOAD_PAY_BONUS, 2),
        deadline_game_h=round(job.deadline_game_h * HOT_LOAD_DEADLINE_SQUEEZE, 1),
    )


def calls_for_day(
    career: DispatchCareer, shift: DispatchShift
) -> list[tuple[float, DispatcherCall]]:
    """(desk seconds, call) for today, deterministic per career day.

    Day one stays quiet so a new dispatcher learns the desk before the phone
    starts. After that, up to two calls a day, at seeded times.
    """
    if career.day <= 1 or not career.roster:
        return []
    rng = random.Random(f"phone:{career.seed}:{career.day}")
    count = rng.choice((1, 1, 2, 2, MAX_CALLS_PER_DAY))
    scheduled: list[tuple[float, DispatcherCall]] = []
    callers: list[RosterDriver] = [
        d for d in career.roster if not d.out_today and d.active_run is None
    ]
    for slot in range(min(count, MAX_CALLS_PER_DAY)):
        low, high = _CALL_WINDOWS[slot]
        at = rng.uniform(low, high)
        kind = rng.choice((CALL_OUT, DAY_OFF_REQUEST, HOT_LOAD, HOT_LOAD))
        if kind == HOT_LOAD:
            job = _hot_load(shift, rng)
            if job is None:
                continue
            scheduled.append((at, DispatcherCall(kind=HOT_LOAD, job=job)))
        else:
            if not callers:
                continue
            driver = callers.pop(rng.randrange(len(callers)))
            scheduled.append((at, DispatcherCall(kind=kind, driver=driver)))
    scheduled.sort(key=lambda item: item[0])
    return scheduled


class CallSchedule:
    """Feeds ringing calls to the desk as desk time passes."""

    def __init__(self, career: DispatchCareer, shift: DispatchShift) -> None:
        self._pending = calls_for_day(career, shift)
        self.elapsed = 0.0

    def ring(self, dt: float) -> list[DispatcherCall]:
        """Advance the desk clock; calls whose time arrived start ringing."""
        self.elapsed += dt
        ringing: list[DispatcherCall] = []
        while self._pending and self._pending[0][0] <= self.elapsed:
            ringing.append(self._pending.pop(0)[1])
        return ringing

    @property
    def pending_count(self) -> int:
        return len(self._pending)
