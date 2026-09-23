"""The dispatcher's day: work the board, answer the phone, run it, sleep, repeat."""

from __future__ import annotations

from ..career_saves import save_career
from ..models.career_state import DispatchCareer
from ..models.hiring import applicants_for_day, hire
from ..models.phone import CallSchedule, DispatcherCall, apply_choice
from ..models.shift import HOS_MODE, DispatchShift
from .base import MenuItem, MenuState
from .radio import RadioMenuState


class ShiftState(MenuState):
    intro_help = (
        "You are on the dispatch desk. Work the load board, review your "
        "drivers, then run the board to see the day play out. The phone "
        "rings while you work; End the day sleeps the roster, saves the "
        "career, and opens tomorrow's board. F1 on any item explains it."
    )

    def __init__(self, ctx, career: DispatchCareer, shift: DispatchShift) -> None:
        super().__init__(ctx)
        self.career = career
        self.shift = shift
        self.board_run = False
        self.ringing: list[DispatcherCall] = []
        self.schedule = CallSchedule(career, shift)

    @property
    def title(self) -> str:  # type: ignore[override]
        return (
            f"Day {self.career.day}: {self.shift.option.carrier_name} dispatch, {self.shift.city}"
        )

    def announce_entry(self) -> None:
        covered = len(self.shift.assignments)
        self.ctx.say(
            f"{self.career.morning_spoken()} "
            f"{len(self.shift.jobs)} loads on the board, {covered} covered. "
            f"{self.current_text()}"
        )
        for line in self.career.take_overnight_news():
            self.ctx.say(line, interrupt=False)

    def update(self, dt: float) -> None:
        for call in self.schedule.ring(dt):
            self.ringing.append(call)
            self.refresh()
            self.ctx.say(call.ring_spoken(), interrupt=False)

    def build_items(self) -> list[MenuItem]:
        items: list[MenuItem] = []
        for call in self.ringing:
            items.append(
                MenuItem(
                    lambda c=call: (
                        f"Answer the phone: {c.ring_spoken().removeprefix('The phone is ringing: ')}"
                    ),
                    lambda c=call: self._answer(c),
                    help="Take the call and choose how to handle it.",
                )
            )
        items.extend(
            [
                MenuItem(
                    lambda: (
                        f"Load board: {len(self.shift.jobs)} loads, "
                        f"{len(self.shift.assignments)} covered"
                    ),
                    self._board,
                    help="Review each load and pick the driver to cover it.",
                ),
                MenuItem(
                    "Driver roster",
                    self._roster,
                    help="Hear every driver's level, record, endorsements, and legal hours.",
                ),
                MenuItem(
                    "Hiring office",
                    self._hiring,
                    help="Hear today's applicants and sign one out of the operating funds.",
                ),
                MenuItem(
                    "Terminal radio",
                    self._radio,
                    help="The radio on the desk: toggle, tune, and hear what reaches the terminal.",
                ),
                MenuItem(
                    "Market watch",
                    self._market,
                    help="Which cargo classes pay tight and which pay loose today.",
                ),
                MenuItem(
                    "Service record",
                    self._record,
                    help="Your career record: shifts, deliveries, on-time share, and margin.",
                ),
                MenuItem(
                    "Run the board",
                    self._run,
                    help="Commit the assignments. Deliveries play out and the day settles.",
                ),
                MenuItem(
                    "End the day",
                    self._end_day,
                    help="Sleep the roster, save the career, and open tomorrow's board.",
                ),
                MenuItem(
                    "Save and quit to title",
                    self._quit,
                    help="Save the career and leave the desk.",
                ),
            ]
        )
        return items

    # -- the phone ------------------------------------------------------------

    def _answer(self, call: DispatcherCall) -> None:
        self.ctx.push_state(PhoneCallState(self.ctx, self, call))

    def call_answered(self, call: DispatcherCall) -> None:
        if call in self.ringing:
            self.ringing.remove(call)
        self.refresh(keep_index=False)

    # -- desk actions -----------------------------------------------------------

    def _board(self) -> None:
        self.ctx.push_state(LoadBoardState(self.ctx, self.shift))

    def _roster(self) -> None:
        for driver in self.career.roster:
            out = "Out today. " if driver.out_today else ""
            self.ctx.say(out + driver.summary(HOS_MODE), interrupt=False)

    def _hiring(self) -> None:
        self.ctx.push_state(HiringOfficeState(self.ctx, self.career))

    def _radio(self) -> None:
        self.ctx.push_state(RadioMenuState(self.ctx, self.career))

    def _market(self) -> None:
        self.ctx.say(self.career.market.summary())

    def _record(self) -> None:
        self.ctx.say(f"Funds {self.career.funds:,.0f} dollars. {self.career.record.spoken()}")

    def _run(self) -> None:
        if self.board_run:
            self.ctx.say("The board has been run. End the day when you are ready.")
            return
        if not self.shift.assignments:
            self.ctx.say("Nothing is assigned yet. Cover at least one load first.")
            return
        summary = self.shift.resolve()
        self.board_run = True
        for outcome in summary.outcomes:
            self.ctx.say(outcome.spoken(), interrupt=False)
        for departure in summary.departures:
            self.ctx.say(departure.spoken(), interrupt=False)
        self.ctx.say(summary.spoken(), interrupt=False)
        self.career.record_shift(summary)
        self.ctx.say(f"Operating funds now {self.career.funds:,.0f} dollars.", interrupt=False)
        self.refresh()

    def _end_day(self) -> None:
        if not self.board_run:
            if self.shift.assignments:
                self.ctx.say(
                    "Assignments are still sitting on the board. Run the board "
                    "first, or pull the drivers off."
                )
                return
            self.career.record_skipped_board(len(self.shift.jobs))
            self.ctx.say(
                f"You walk away from {len(self.shift.jobs)} loads. It goes on the record.",
                interrupt=False,
            )
        self.career.end_day()
        save_career(self.career)
        shift = self.career.build_shift(self.ctx.world)
        self.ctx.reset_to(ShiftState(self.ctx, self.career, shift))

    def _quit(self) -> None:
        from .start import TitleState

        if self.board_run:
            # A settled board never replays: quitting after a run ends the day.
            self.career.end_day()
        save_career(self.career)
        self.ctx.say("Career saved.", interrupt=True)
        self.ctx.reset_to(TitleState(self.ctx))

    def go_back(self) -> None:
        self.ctx.say("Use Save and quit to title to leave the desk.")


class PhoneCallState(MenuState):
    title = "On the phone"
    intro_help = (
        "Pick your answer. Escape puts them on hold and returns to the desk; "
        "the call keeps ringing until you deal with it."
    )

    def __init__(self, ctx, desk: ShiftState, call: DispatcherCall) -> None:
        super().__init__(ctx)
        self.desk = desk
        self.call = call

    def announce_entry(self) -> None:
        self.ctx.say(f"{self.call.spoken()} {self.current_text()}")

    def build_items(self) -> list[MenuItem]:
        items = [
            MenuItem(
                choice.label,
                lambda key=choice.key: self._choose(key),
                help=choice.help,
            )
            for choice in self.call.choices()
        ]
        items.append(
            MenuItem(
                "Put them on hold",
                self.go_back,
                help="Back to the desk; the call keeps ringing.",
            )
        )
        return items

    def _choose(self, key: str) -> None:
        result = apply_choice(self.call, key, self.desk.shift, self.desk.career)
        self.desk.call_answered(self.call)
        self.ctx.pop_state()
        self.ctx.say(result, interrupt=True)


class HiringOfficeState(MenuState):
    title = "Hiring office"
    intro_help = (
        "Today's applicants. Signing costs come out of the operating funds; "
        "tomorrow brings a different pool. Escape returns to the desk."
    )

    def __init__(self, ctx, career: DispatchCareer) -> None:
        super().__init__(ctx)
        self.career = career

    def announce_entry(self) -> None:
        applicants = applicants_for_day(self.career)
        n = len(applicants)
        self.ctx.say(
            f"Hiring office. {n} applicant{'s' if n != 1 else ''} today. "
            f"Funds {self.career.funds:,.0f} dollars, "
            f"{len(self.career.roster)} drivers on the roster. "
            f"{self.current_text()}"
        )

    def build_items(self) -> list[MenuItem]:
        items = [
            MenuItem(
                f"Sign {applicant.driver.name}: level {applicant.driver.level}, "
                f"reputation {applicant.driver.career.reputation:.0f}, "
                f"{applicant.cost:,.0f} dollars",
                lambda a=applicant: self._hire(a),
                help=applicant.spoken(),
            )
            for applicant in applicants_for_day(self.career)
        ]
        items.append(MenuItem("Back to the desk", self.go_back))
        return items

    def _hire(self, applicant) -> None:
        result = hire(self.career, applicant)
        self.refresh(keep_index=False)
        self.ctx.say(result)


class LoadBoardState(MenuState):
    title = "Load board"
    intro_help = (
        "Each entry is one load. Enter opens the assignment menu for it; "
        "drivers who cannot take the load say why. Escape returns to the desk."
    )

    def __init__(self, ctx, shift: DispatchShift) -> None:
        super().__init__(ctx)
        self.shift = shift

    def build_items(self) -> list[MenuItem]:
        items = []
        for i, job in enumerate(self.shift.jobs):
            covered = self.shift.assignments.get(i)
            state = f" Covered by {covered.name}." if covered else ""
            hours = self.shift.required_hours(job)
            hours_text = f" About {hours:.1f} legal hours." if hours else ""
            items.append(
                MenuItem(
                    f"{job.describe(i + 1, len(self.shift.jobs))}{hours_text}{state}",
                    lambda index=i: self._open(index),
                    help="Enter to choose a driver for this load, or to pull the "
                    "current driver off it.",
                )
            )
        items.append(MenuItem("Back to the desk", self.go_back))
        return items

    def _open(self, index: int) -> None:
        self.ctx.push_state(AssignState(self.ctx, self.shift, index))


class AssignState(MenuState):
    intro_help = (
        "Pick the driver to cover this load. Drivers who cannot take it "
        "explain the refusal: endorsements, legal hours, or experience. "
        "Escape returns to the board."
    )

    def __init__(self, ctx, shift: DispatchShift, job_index: int) -> None:
        super().__init__(ctx)
        self.shift = shift
        self.job_index = job_index

    @property
    def title(self) -> str:  # type: ignore[override]
        job = self.shift.jobs[self.job_index]
        return f"Assign: {job.cargo.label} to {job.destination}"

    def build_items(self) -> list[MenuItem]:
        job = self.shift.jobs[self.job_index]
        items = []
        current = self.shift.assignments.get(self.job_index)
        if current is not None:
            items.append(
                MenuItem(
                    f"Pull {current.name} off this load",
                    self._unassign,
                    help="Free the driver and put the load back on the board.",
                )
            )
        for driver in self.shift.drivers:
            reason = self.shift.refusal_reason(driver, job)
            if reason:
                items.append(
                    MenuItem(
                        f"Cannot use {driver.name}: {reason}",
                        lambda r=reason: self.ctx.say(r),
                        help=reason,
                    )
                )
            else:
                odds = self.shift.on_time_probability(driver, job)
                items.append(
                    MenuItem(
                        f"Assign {driver.name}: level {driver.level}, reputation "
                        f"{driver.career.reputation:.0f}, about {odds * 100:.0f} "
                        "percent on-time odds",
                        lambda d=driver: self._assign(d),
                        help=driver.summary(),
                    )
                )
        items.append(MenuItem("Back to the board", self.go_back))
        return items

    def _assign(self, driver) -> None:
        problem = self.shift.assign(self.job_index, driver)
        if problem:
            self.ctx.say(problem)
            return
        job = self.shift.jobs[self.job_index]
        self.ctx.say(
            f"{driver.name} is covering {job.cargo.label} to {job.destination}.",
            interrupt=True,
        )
        self.ctx.pop_state()

    def _unassign(self) -> None:
        self.shift.unassign(self.job_index)
        self.ctx.say("Load is back on the board.")
        self.refresh(keep_index=False)
