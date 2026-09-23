"""The hiring office: daily pools, signing costs, and roster limits."""

import os

os.environ.setdefault("FF_DISPATCH_NO_SPEECH", "1")

import pytest

from ff_dispatch.models import DispatchCareer, applicants_for_day, hire, signing_cost
from ff_dispatch.models.career_state import ROSTER_CAP


@pytest.fixture
def career():
    return DispatchCareer.new(carrier_key="northstar", seed=555)


def test_applicants_are_deterministic_per_day_and_change_daily(career):
    today = applicants_for_day(career)
    again = applicants_for_day(career)
    assert [a.driver.name for a in today] == [a.driver.name for a in again]
    assert all(a.cost == signing_cost(a.driver) for a in today)
    career.end_day()
    tomorrow = applicants_for_day(career)
    assert [a.driver.name for a in today] != [a.driver.name for a in tomorrow]


def test_signing_cost_scales_with_experience(career):
    applicants = sorted(applicants_for_day(career), key=lambda a: a.driver.level)
    assert applicants[0].cost > 0
    if applicants[0].driver.level != applicants[-1].driver.level:
        assert applicants[0].cost < applicants[-1].cost


def test_hiring_deducts_funds_and_grows_the_roster(career):
    applicant = applicants_for_day(career)[0]
    funds_before = career.funds
    roster_before = len(career.roster)
    result = hire(career, applicant)
    assert "signs on" in result
    assert len(career.roster) == roster_before + 1
    assert career.funds == round(funds_before - applicant.cost, 2)
    # hired applicants drop out of today's pool
    assert applicant.driver.name not in [a.driver.name for a in applicants_for_day(career)]
    # and cannot be double-hired
    assert "already on your roster" in hire(career, applicant)


def test_hiring_needs_funds_and_roster_room(career):
    applicant = applicants_for_day(career)[0]
    career.funds = 1.0
    refused = hire(career, applicant)
    assert "cannot cover it" in refused
    assert applicant.driver.name not in [d.name for d in career.roster]

    career.funds = 1_000_000.0
    while len(career.roster) < ROSTER_CAP:
        pool = applicants_for_day(career)
        if not pool:
            career.end_day()
            continue
        hire(career, pool[0])
    pool = applicants_for_day(career)
    while not pool:
        career.end_day()
        pool = applicants_for_day(career)
    assert "roster is full" in hire(career, pool[0])
