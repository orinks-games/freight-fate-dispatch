"""The game boots headless and a career survives a full day loop and a reload."""

import os

os.environ.setdefault("FF_DISPATCH_NO_SPEECH", "1")

import pygame


def key_event(key):
    return pygame.event.Event(pygame.KEYDOWN, key=key, unicode="")


def select_item(state, label_prefix: str):
    """Move the menu cursor to the item starting with the label and activate it."""
    for i, item in enumerate(state.items):
        if item.text.startswith(label_prefix):
            state.index = i
            state.activate()
            return
    raise AssertionError(
        f"no menu item starting with {label_prefix!r}: {[item.text for item in state.items]}"
    )


def test_two_days_at_the_desk_and_a_reload(monkeypatch, tmp_path):
    monkeypatch.setenv("FF_DISPATCH_DATA_DIR", str(tmp_path))

    from ff_dispatch.app import App
    from ff_dispatch.states.shift import AssignState, LoadBoardState, ShiftState
    from ff_dispatch.states.start import (
        CarrierSelectState,
        ContinueCareerState,
        TitleState,
    )

    app = App()
    spoken = []
    monkeypatch.setattr(app.ctx, "say", lambda text, interrupt=True: spoken.append(text))
    try:
        app.push_state(TitleState(app.ctx))
        assert "Freight Fate: Dispatch" in app.state.lines()[0]

        app.state.handle_event(key_event(pygame.K_RETURN))  # New dispatcher career
        assert isinstance(app.state, CarrierSelectState)
        assert any("Northstar" in item.text for item in app.state.items)

        app.state.handle_event(key_event(pygame.K_RETURN))  # Northstar
        assert isinstance(app.state, ShiftState)
        assert app.state.career.day == 1
        assert any("Day 1" in text for text in spoken)
        # a new career is on disk from its first morning
        assert list((tmp_path / "careers").glob("*.json"))

        desk = app.state
        select_item(desk, "Load board")
        assert isinstance(app.state, LoadBoardState)
        assert len(app.state.items) > 1

        app.state.handle_event(key_event(pygame.K_RETURN))  # open first load
        assert isinstance(app.state, AssignState)
        labels = [item.text for item in app.state.items]
        assert any(
            label.startswith("Assign ") or label.startswith("Cannot use ") for label in labels
        )
        app.state.handle_event(key_event(pygame.K_ESCAPE))  # back to the board
        app.state.handle_event(key_event(pygame.K_ESCAPE))  # back to the desk
        assert app.state is desk

        # cover every coverable load through the model, then run it via the menu
        shift = desk.shift
        for i, _job in enumerate(shift.jobs):
            candidates = shift.assignable_drivers(i)
            if candidates:
                shift.assign(i, candidates[0])
        assert shift.assignments
        select_item(desk, "Run the board")
        assert desk.board_run
        assert any("Shift over" in text for text in spoken)

        select_item(desk, "End the day")
        assert isinstance(app.state, ShiftState)
        assert app.state is not desk
        assert app.state.career.day == 2
        assert any("Day 2" in text for text in spoken)

        # the desk has the new career systems on the menu
        desk_labels = [item.text for item in app.state.items]
        assert any(label.startswith("Hiring office") for label in desk_labels)
        assert any(label.startswith("Terminal radio") for label in desk_labels)
        assert any(label.startswith("Market watch") for label in desk_labels)
        assert any(label.startswith("Service record") for label in desk_labels)

        select_item(app.state, "Save and quit")
        assert isinstance(app.state, TitleState)

        select_item(app.state, "Continue a career")
        assert isinstance(app.state, ContinueCareerState)
        assert any("day 2" in item.text for item in app.state.items)

        app.state.handle_event(key_event(pygame.K_RETURN))  # resume the save
        assert isinstance(app.state, ShiftState)
        assert app.state.career.day == 2
        assert app.state.career.record.shifts_worked == 1

        app.render()  # one frame of every reachable lines() output
    finally:
        app.shutdown()
