"""The game boots headless and the start flow reaches a working shift desk."""

import os

os.environ.setdefault("FF_DISPATCH_NO_SPEECH", "1")

import pygame


def key_event(key):
    return pygame.event.Event(pygame.KEYDOWN, key=key, unicode="")


def test_title_to_carrier_choice_to_shift_desk(monkeypatch):
    from ff_dispatch.app import App
    from ff_dispatch.states.shift import AssignState, LoadBoardState, ShiftState
    from ff_dispatch.states.start import CarrierSelectState, TitleState

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
        assert any("dispatch desk" in text for text in spoken)

        app.state.handle_event(key_event(pygame.K_RETURN))  # Load board
        assert isinstance(app.state, LoadBoardState)
        assert len(app.state.items) > 1

        app.state.handle_event(key_event(pygame.K_RETURN))  # open first load
        assert isinstance(app.state, AssignState)
        labels = [item.text for item in app.state.items]
        assert any(label.startswith(("Assign ", "Cannot use ")) for label in labels)

        app.render()  # one frame of every reachable lines() output
    finally:
        app.shutdown()


def test_keyboard_only_playthrough_announces_assignment_and_shift_result(monkeypatch, tmp_path):
    from ff_dispatch.app import App
    from ff_dispatch.saves import dispatch_save_path, save_shift
    from ff_dispatch.states.shift import AssignState, LoadBoardState, ShiftState
    from ff_dispatch.states.start import CarrierSelectState, TitleState

    monkeypatch.setenv("FF_DISPATCH_DATA_DIR", str(tmp_path))
    app = App()
    spoken = []
    monkeypatch.setattr(app.ctx, "say", lambda text, interrupt=True: spoken.append(text))
    try:
        app.push_state(TitleState(app.ctx))
        app.state.handle_event(key_event(pygame.K_RETURN))
        assert isinstance(app.state, CarrierSelectState)
        app.state.handle_event(key_event(pygame.K_RETURN))
        assert isinstance(app.state, ShiftState)
        app.state.handle_event(key_event(pygame.K_RETURN))
        assert isinstance(app.state, LoadBoardState)

        load_index = next(
            i for i in range(len(app.state.shift.jobs)) if app.state.shift.assignable_drivers(i)
        )
        app.state.jump(load_index)
        app.state.handle_event(key_event(pygame.K_RETURN))
        assert isinstance(app.state, AssignState)
        assign_index = next(
            i for i, item in enumerate(app.state.items) if item.text.startswith("Assign ")
        )
        app.state.jump(assign_index)
        app.state.handle_event(key_event(pygame.K_RETURN))
        assert isinstance(app.state, LoadBoardState)
        save_shift(app.state.shift)
        assert dispatch_save_path().is_file()
        app.state.handle_event(key_event(pygame.K_ESCAPE))
        assert isinstance(app.state, ShiftState)
        app.state.jump(2)  # Run the board.
        app.state.handle_event(key_event(pygame.K_RETURN))

        transcript = " ".join(spoken)
        assert "is covering" in transcript
        assert "delivered" in transcript
        assert "Shift over" in transcript
        assert not dispatch_save_path().exists()
    finally:
        app.shutdown()


def test_saved_shift_can_be_resumed_from_the_title(monkeypatch, tmp_path):
    from ff_dispatch.app import App
    from ff_dispatch.models import DispatchShift, generate_roster
    from ff_dispatch.saves import save_shift
    from ff_dispatch.states.shift import ShiftState
    from ff_dispatch.states.start import TitleState

    monkeypatch.setenv("FF_DISPATCH_DATA_DIR", str(tmp_path))
    app = App()
    spoken = []
    monkeypatch.setattr(app.ctx, "say", lambda text, interrupt=True: spoken.append(text))
    try:
        shift = DispatchShift(
            app.world,
            carrier_key="northstar",
            seed=111,
            drivers=generate_roster(111, count=4),
        )
        save_shift(shift)
        app.push_state(TitleState(app.ctx))

        continue_index = next(
            i for i, item in enumerate(app.state.items) if item.text == "Continue saved shift"
        )
        app.state.jump(continue_index)
        app.state.handle_event(key_event(pygame.K_RETURN))

        assert isinstance(app.state, ShiftState)
        assert app.state.shift.seed == 111
        assert any("Saved shift resumed" in text for text in spoken)
    finally:
        app.shutdown()
