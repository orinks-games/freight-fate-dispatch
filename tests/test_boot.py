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
        assert any(
            label.startswith("Assign ") or label.startswith("Cannot use ") for label in labels
        )

        app.render()  # one frame of every reachable lines() output
    finally:
        app.shutdown()
