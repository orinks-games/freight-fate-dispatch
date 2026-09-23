"""The phone: deterministic schedules, and calls that change the board."""

import os

os.environ.setdefault("FF_DISPATCH_NO_SPEECH", "1")

import pytest

from freightverse.data import get_world

from ff_dispatch.models import DispatchCareer
from ff_dispatch.models.phone import (
    CALL_OUT,
    DAY_OFF_REQUEST,
    HOT_LOAD,
    CallSchedule,
    DispatcherCall,
    apply_choice,
    calls_for_day,
)


@pytest.fixture(scope="session")
def world():
    return get_world()


@pytest.fixture
def career():
    c = DispatchCareer.new(carrier_key="northstar", seed=99)
    c.end_day()  # day 1 is quiet by design; test on day 2
    return c


@pytest.fixture
def shift(world, career):
    return career.build_shift(world)


def test_day_one_is_quiet(world):
    fresh = DispatchCareer.new(carrier_key="northstar", seed=99)
    assert calls_for_day(fresh, fresh.build_shift(world)) == []


def test_schedule_is_deterministic_and_ordered(career, shift):
    first = calls_for_day(career, shift)
    second = calls_for_day(career, shift)
    assert len(first) >= 1
    assert [(t, c.kind) for t, c in first] == [(t, c.kind) for t, c in second]
    times = [t for t, _ in first]
    assert times == sorted(times)
    assert all(15.0 <= t <= 135.0 for t in times)


def test_call_schedule_rings_when_time_passes(career, shift):
    scheduled = calls_for_day(career, shift)
    schedule = CallSchedule(career, shift)
    assert schedule.ring(0.1) == []
    rang = schedule.ring(500.0)
    assert [c.kind for c in rang] == [c.kind for _, c in scheduled]
    assert schedule.pending_count == 0


def test_sick_driver_sent_home_leaves_the_board(career, shift):
    driver = shift.assignable_drivers(0)[0] if shift.assignable_drivers(0) else career.roster[0]
    job_index = next(
        (i for i, _ in enumerate(shift.jobs) if not shift.refusal_reason(driver, shift.jobs[i])),
        None,
    )
    if job_index is not None:
        assert shift.assign(job_index, driver) == ""
    call = DispatcherCall(kind=CALL_OUT, driver=driver)
    result = apply_choice(call, "send_home", shift, career)
    assert driver.out_today
    assert driver.assigned_job_index is None
    assert job_index is None or job_index not in shift.assignments
    assert "out for the day" in result
    assert "out today" in shift.refusal_reason(driver, shift.jobs[0])


def test_toughing_it_out_costs_reputation_not_availability(career, shift):
    driver = career.roster[0]
    before = driver.career.reputation
    call = DispatcherCall(kind=CALL_OUT, driver=driver)
    apply_choice(call, "tough_it_out", shift, career)
    assert not driver.out_today
    assert driver.career.reputation == max(0.0, before - 4.0)


def test_day_off_granted_buys_goodwill(career, shift):
    driver = career.roster[1]
    before = driver.career.reputation
    call = DispatcherCall(kind=DAY_OFF_REQUEST, driver=driver)
    apply_choice(call, "grant", shift, career)
    assert driver.out_today
    assert driver.career.reputation == min(100.0, before + 2.0)


class FakeCtx:
    """Just enough context for desk states: a world, a transcript, a stack."""

    def __init__(self, world):
        self.world = world
        self.spoken: list[str] = []
        self.stack: list = []

    def say(self, text, interrupt=True):
        self.spoken.append(text)

    def stop_speech(self):
        pass

    def push_state(self, state):
        self.stack.append(state)
        state.enter()

    def pop_state(self):
        if self.stack:
            self.stack.pop().exit()
        if self.stack:
            self.stack[-1].enter()

    def reset_to(self, state):
        self.stack = [state]
        state.enter()


def test_desk_rings_and_the_call_menu_resolves(world, career):
    from ff_dispatch.states.shift import PhoneCallState, ShiftState

    shift = career.build_shift(world)
    ctx = FakeCtx(world)
    desk = ShiftState(ctx, career, shift)
    ctx.push_state(desk)

    desk.update(500.0)  # the whole day's desk time passes at once
    assert desk.ringing
    assert any("phone is ringing" in text for text in ctx.spoken)
    assert any(item.text.startswith("Answer the phone") for item in desk.items)

    call = desk.ringing[0]
    desk._answer(call)
    assert isinstance(ctx.stack[-1], PhoneCallState)
    phone = ctx.stack[-1]
    assert len(phone.items) == len(call.choices()) + 1  # plus Put them on hold

    phone._choose(call.choices()[0].key)
    assert call not in desk.ringing
    assert ctx.stack[-1] is desk


def test_hot_load_lands_on_the_board(career, shift, world):
    # find a hot-load call on some day; the generator is seeded, so scan days
    call = None
    for _ in range(12):
        for _t, candidate in calls_for_day(career, shift):
            if candidate.kind == HOT_LOAD:
                call = candidate
                break
        if call is not None:
            break
        career.end_day()
        shift = career.build_shift(world)
    assert call is not None, "no hot load scheduled in 12 days of calls"
    board_before = len(shift.jobs)
    result = apply_choice(call, "take", shift, career)
    assert len(shift.jobs) == board_before + 1
    assert call.job_index == board_before
    assert shift.jobs[-1].pay == call.job.pay
    assert "hot load is on your board" in result
    # passing on a different call leaves the board alone
    other = DispatcherCall(kind=HOT_LOAD, job=call.job)
    assert "pass" in apply_choice(other, "pass", shift, career).lower()
    assert len(shift.jobs) == board_before + 1
