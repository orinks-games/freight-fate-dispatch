"""The dispatch shift stands on freightverse: rosters, rules, outcomes, import."""

import os

os.environ.setdefault("FF_DISPATCH_NO_SPEECH", "1")

import pytest

from freightverse.data import get_world
from freightverse.exchange import DriverRecord

from ff_dispatch.models import DispatchShift, driver_from_record, generate_roster
from ff_dispatch.models.shift import HOS_MODE


@pytest.fixture(scope="session")
def world():
    return get_world()


@pytest.fixture
def shift(world):
    return DispatchShift(
        world,
        carrier_key="northstar",
        seed=42,
        drivers=generate_roster(42, count=6),
    )


def test_roster_is_deterministic_and_plausible():
    first = generate_roster(7, count=6)
    second = generate_roster(7, count=6)
    assert [d.name for d in first] == [d.name for d in second]
    assert len({d.name for d in first}) == 6
    for driver in first:
        assert 1 <= driver.level <= 12
        assert 0.0 <= driver.reliability <= 1.0
        assert driver.career.on_time_deliveries <= driver.career.deliveries


def test_shift_builds_a_board_at_the_carrier_terminal(shift):
    assert shift.city == "Chicago"  # Northstar's default terminal
    assert shift.jobs
    for job in shift.jobs:
        assert job.origin == "Chicago"


def test_refusals_follow_freight_fate_rules(shift):
    rookie = next(d for d in shift.drivers if d.level <= 3)
    specialty = [
        (i, j)
        for i, j in enumerate(shift.jobs)
        if j.cargo.endorsement and j.cargo.endorsement not in rookie.endorsements
    ]
    if specialty:
        index, job = specialty[0]
        reason = shift.refusal_reason(rookie, job)
        assert "endorsement" in reason or "level" in reason

    # exhausted driver refuses long freight
    tired = shift.drivers[0]
    tired.hos.drive(650.0)
    long_jobs = [(i, j) for i, j in enumerate(shift.jobs) if j.distance_mi > 150]
    if long_jobs:
        _, job = long_jobs[0]
        if not (job.cargo.endorsement and job.cargo.endorsement not in tired.endorsements):
            assert "legal hours" in shift.refusal_reason(tired, job)


def test_assignment_lifecycle_and_double_booking(shift):
    job_index = next(i for i, _ in enumerate(shift.jobs) if shift.assignable_drivers(i))
    driver = shift.assignable_drivers(job_index)[0]
    assert shift.assign(job_index, driver) == ""
    assert shift.assignments[job_index] is driver
    # the same driver cannot take a second load
    other = next((i for i, _ in enumerate(shift.jobs) if i != job_index), None)
    if other is not None:
        assert "already has a load" in shift.refusal_reason(driver, shift.jobs[other])
    shift.unassign(job_index)
    assert driver.assigned_job_index is None


def test_running_the_board_settles_and_evolves_drivers(shift):
    assigned = 0
    for i, _job in enumerate(shift.jobs):
        candidates = shift.assignable_drivers(i)
        if candidates:
            shift.assign(i, candidates[0])
            assigned += 1
    assert assigned > 0
    before = {d.name: d.career.deliveries for d in shift.drivers}
    summary = shift.resolve()

    assert len(summary.outcomes) == assigned
    assert summary.unassigned_loads == len(shift.jobs) - assigned
    for outcome in summary.outcomes:
        assert outcome.driver_wage > 0
        assert outcome.company_gross >= outcome.driver_wage * 0.3
        assert outcome.driver.career.deliveries == before[outcome.driver.name] + 1
        assert outcome.driver.hos.remaining_min(HOS_MODE) is not None
    assert "Shift over" in summary.spoken()


def test_on_time_odds_favor_reliable_drivers_and_slack(shift):
    job = shift.jobs[0]
    strong = shift.drivers[0]
    weak = shift.drivers[1]
    strong.career.reputation = 95.0
    strong.career.deliveries = 50
    strong.career.on_time_deliveries = 50
    weak.career.reputation = 45.0
    weak.career.deliveries = 50
    weak.career.on_time_deliveries = 30
    assert shift.on_time_probability(strong, job) > shift.on_time_probability(weak, job)


def test_imported_driver_joins_with_real_history():
    record = DriverRecord.from_payload(
        {
            "exchange_version": 1,
            "name": "Orinks",
            "home_city": "Denver",
            "business_status": "company_driver",
            "carrier_key": "northstar",
            "xp": 19_000.0,
            "reputation": 82.0,
            "deliveries": 40,
            "on_time_deliveries": 37,
            "purchased_endorsements": ["refrigerated"],
        }
    )
    driver = driver_from_record(record)
    assert driver.imported
    assert driver.level == 8
    assert "refrigerated" in driver.endorsements
    assert driver.reliability > 0.7
    assert "Imported Freight Fate driver" in driver.summary()
