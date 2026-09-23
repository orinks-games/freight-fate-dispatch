"""Multi-day careers persist: the day loop, the record, and the save files."""

import os

os.environ.setdefault("FF_DISPATCH_NO_SPEECH", "1")

import pytest

from freightverse.data import get_world

from ff_dispatch.career_saves import list_careers, load_career, save_career
from ff_dispatch.models import DispatchCareer
from ff_dispatch.models.career_state import STARTING_FUNDS
from ff_dispatch.models.shift import HOS_MODE


@pytest.fixture(scope="session")
def world():
    return get_world()


@pytest.fixture
def career():
    return DispatchCareer.new(carrier_key="northstar", seed=1234)


def _work_a_full_board(shift):
    for i, _job in enumerate(shift.jobs):
        candidates = shift.assignable_drivers(i)
        if candidates:
            shift.assign(i, candidates[0])
    return shift.resolve()


def test_new_career_is_deterministic_per_seed():
    a = DispatchCareer.new(carrier_key="northstar", seed=7)
    b = DispatchCareer.new(carrier_key="northstar", seed=7)
    assert [d.name for d in a.roster] == [d.name for d in b.roster]
    assert a.city == "Chicago"
    assert a.day == 1
    assert a.funds == STARTING_FUNDS


def test_reloaded_morning_replays_the_same_board(world, career):
    first = career.build_shift(world)
    second = career.build_shift(world)
    assert [j.describe() for j in first.jobs] == [j.describe() for j in second.jobs]


def test_days_produce_different_boards(world, career):
    day1 = [j.describe() for j in career.build_shift(world).jobs]
    career.end_day()
    day2 = [j.describe() for j in career.build_shift(world).jobs]
    assert day1 != day2


def test_shift_settles_into_funds_and_record(world, career):
    shift = career.build_shift(world)
    summary = _work_a_full_board(shift)
    career.record_shift(summary)
    assert career.funds == round(STARTING_FUNDS + summary.total_margin, 2)
    assert career.record.shifts_worked == 1
    assert career.record.loads_delivered == len(summary.outcomes)
    assert "shift" in career.record.spoken()


def test_end_day_rests_the_roster_and_moves_the_market(world, career):
    shift = career.build_shift(world)
    _work_a_full_board(shift)
    career.roster[0].out_today = True
    market_before = dict(career.market.multipliers)
    career.end_day()
    assert career.day == 2
    assert career.market.day == 2
    assert market_before != career.market.multipliers  # the walk drifted
    for driver in career.roster:
        assert not driver.out_today
        assert driver.hos.driving_min == 0.0
        # a fresh clock's nearest limit is the 8-hour break rule
        assert driver.hos.remaining_min(HOS_MODE) == 8 * 60.0
    assert "Day 2" in career.morning_spoken()


def test_payload_roundtrip_preserves_the_career(world, career):
    shift = career.build_shift(world)
    summary = _work_a_full_board(shift)
    career.record_shift(summary)
    career.end_day()
    career.radio_enabled = True
    career.radio_station_id = "ff-night-line"

    reloaded = DispatchCareer.from_payload(career.to_payload())
    assert reloaded is not None
    assert reloaded.to_payload() == career.to_payload()
    assert [d.name for d in reloaded.roster] == [d.name for d in career.roster]
    # the reloaded morning regenerates the identical board
    assert [j.describe() for j in reloaded.build_shift(world).jobs] == [
        j.describe() for j in career.build_shift(world).jobs
    ]


def test_from_payload_tolerates_garbage():
    assert DispatchCareer.from_payload(None) is None
    assert DispatchCareer.from_payload({"not": "a career"}) is None
    sparse = DispatchCareer.from_payload({"carrier_key": "northstar"})
    assert sparse is not None
    assert sparse.day == 1
    assert sparse.city == "Chicago"


def test_save_list_load_roundtrip(tmp_path, monkeypatch, career):
    monkeypatch.setenv("FF_DISPATCH_DATA_DIR", str(tmp_path))
    path = save_career(career)
    assert path.is_file()

    found = list_careers()
    assert len(found) == 1
    label, found_path, loaded = found[0]
    assert found_path == path
    assert "Northstar" in label
    assert loaded.seed == career.seed
    assert load_career(path).to_payload() == career.to_payload()

    # unreadable files are skipped, not fatal
    (tmp_path / "careers" / "junk.json").write_text("{broken", encoding="utf-8")
    assert len(list_careers()) == 1
