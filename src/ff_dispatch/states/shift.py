"""The dispatcher's shift: work the board, cover the loads, run it, answer for it."""

from __future__ import annotations

from ..models.shift import HOS_MODE, DispatchShift
from .base import MenuItem, MenuState


class ShiftState(MenuState):
    intro_help = (
        "You are on the dispatch desk. Work the load board, review your "
        "drivers, then run the board to see the shift play out. F1 on any "
        "item explains it."
    )

    def __init__(self, ctx, shift: DispatchShift) -> None:
        super().__init__(ctx)
        self.shift = shift

    @property
    def title(self) -> str:  # type: ignore[override]
        return f"{self.shift.option.carrier_name} dispatch, {self.shift.city}"

    def announce_entry(self) -> None:
        covered = len(self.shift.assignments)
        self.ctx.say(
            f"{self.shift.option.carrier_name} dispatch desk, {self.shift.city}. "
            f"{len(self.shift.jobs)} loads on the board, {covered} covered, "
            f"{len(self.shift.drivers)} drivers on the roster. "
            f"{self.current_text()}"
        )

    def build_items(self) -> list[MenuItem]:
        return [
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
                "Run the board",
                self._run,
                help="Commit the assignments. Deliveries play out and the shift settles.",
            ),
            MenuItem(
                "Quit to title",
                self._quit,
                help="Abandon the shift and return to the title menu.",
            ),
        ]

    def _board(self) -> None:
        self.ctx.push_state(LoadBoardState(self.ctx, self.shift))

    def _roster(self) -> None:
        for driver in self.shift.drivers:
            self.ctx.say(driver.summary(HOS_MODE), interrupt=False)

    def _run(self) -> None:
        if not self.shift.assignments:
            self.ctx.say("Nothing is assigned yet. Cover at least one load first.")
            return
        summary = self.shift.resolve()
        for outcome in summary.outcomes:
            self.ctx.say(outcome.spoken(), interrupt=False)
        self.ctx.say(summary.spoken(), interrupt=False)
        self.refresh()

    def _quit(self) -> None:
        from .start import TitleState

        self.ctx.reset_to(TitleState(self.ctx))

    def go_back(self) -> None:
        self.ctx.say("Use Quit to title to leave the desk.")


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
