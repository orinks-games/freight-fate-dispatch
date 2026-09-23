"""The terminal radio: speech-only, streamer-safe, remembered on the career."""

import os

os.environ.setdefault("FF_DISPATCH_NO_SPEECH", "1")

import pytest

from freightverse.data import get_world

from ff_dispatch.models import DispatchCareer
from ff_dispatch.states.radio import RadioMenuState, desk_radio


class FakeCtx:
    """Just enough context for a menu state: a world and a transcript."""

    def __init__(self, world):
        self.world = world
        self.spoken: list[str] = []

    def say(self, text, interrupt=True):
        self.spoken.append(text)

    def stop_speech(self):
        pass

    def push_state(self, state):
        pass

    def pop_state(self):
        pass

    def reset_to(self, state):
        pass


@pytest.fixture(scope="session")
def world():
    return get_world()


@pytest.fixture
def career():
    return DispatchCareer.new(carrier_key="northstar", seed=31)


def test_desk_radio_sits_at_the_terminal_and_stays_safe(world, career):
    radio = desk_radio(FakeCtx(world), career)
    assert radio.position is not None  # Chicago has coordinates
    assert not radio.enabled  # off until the dispatcher turns it on
    assert radio.streamer_safe
    for reception in radio.receivable_stations():
        assert not reception.station.real_stream  # no network streams, ever


def test_radio_menu_toggles_tunes_and_persists(world, career):
    ctx = FakeCtx(world)
    state = RadioMenuState(ctx, career)
    state.enter()
    assert any("Terminal radio" in line for line in ctx.spoken)

    state._toggle()
    assert career.radio_enabled
    assert "Radio on" in ctx.spoken[-1]

    state._tune(1)
    assert career.radio_station_id  # the tuned station is remembered
    tuned_to = career.radio_station_id
    assert "Tuned to" in ctx.spoken[-1]

    state._status()
    assert "Radio on" in ctx.spoken[-1]

    state._toggle()
    assert not career.radio_enabled
    assert "Radio off" in ctx.spoken[-1]

    # a new desk set picks up where the career left off
    fresh = desk_radio(ctx, career)
    assert not fresh.enabled
    assert fresh.station_id == tuned_to


def test_station_list_speaks_every_receivable_station(world, career):
    ctx = FakeCtx(world)
    state = RadioMenuState(ctx, career)
    state.enter()
    before = len(ctx.spoken)
    state._list()
    lines = ctx.spoken[before:]
    assert lines
    assert len(lines) == len(state.radio.receivable_stations()[:12])
