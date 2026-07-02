"""Application shell: pygame loop, state stack, and the speaking context."""

from __future__ import annotations

import logging
import os
import sys

import pygame

from freightverse.data import get_world

from . import __version__
from .speech import Speech

log = logging.getLogger(__name__)

WINDOW_SIZE = (900, 640)
FPS = 60
BG_COLOR = (14, 12, 18)
TEXT_COLOR = (235, 235, 225)
HILIGHT_COLOR = (120, 200, 255)


class GameContext:
    def __init__(self, app: App) -> None:
        self._app = app
        self.speech: Speech = app.speech
        self.world = app.world

    def say(self, text: str, interrupt: bool = True) -> None:
        self.speech.say(text, interrupt=interrupt)

    def stop_speech(self) -> None:
        self.speech.stop()

    def push_state(self, state) -> None:
        self._app.push_state(state)

    def pop_state(self) -> None:
        self._app.pop_state()

    def reset_to(self, state) -> None:
        self._app.reset_to(state)


class App:
    def __init__(self) -> None:
        os.environ.setdefault("PYGAME_HIDE_SUPPORT_PROMPT", "1")
        if os.environ.get("FF_DISPATCH_NO_SPEECH"):
            os.environ.setdefault("SDL_VIDEODRIVER", "dummy")
            os.environ.setdefault("SDL_AUDIODRIVER", "dummy")
        pygame.init()
        pygame.display.set_caption(f"Freight Fate: Dispatch {__version__}")
        self.screen = pygame.display.set_mode(WINDOW_SIZE)
        self.clock = pygame.time.Clock()
        self.font = pygame.font.SysFont("consolas", 22)
        self.speech = Speech()
        self.world = get_world()
        self.ctx = GameContext(self)
        self.states: list = []
        self.running = True

    @property
    def state(self):
        return self.states[-1] if self.states else None

    def push_state(self, state) -> None:
        self.states.append(state)
        state.enter()

    def pop_state(self) -> None:
        if self.states:
            self.states.pop().exit()
        if self.state is not None:
            self.state.enter()

    def reset_to(self, state) -> None:
        while self.states:
            self.states.pop().exit()
        self.push_state(state)

    def render(self) -> None:
        self.screen.fill(BG_COLOR)
        state = self.state
        if state is not None:
            y = 16
            for i, line in enumerate(state.lines()[:24]):
                color = HILIGHT_COLOR if line.startswith("> ") else TEXT_COLOR
                self.screen.blit(self.font.render(line, True, color), (16, y))
                y += 26
        pygame.display.flip()

    def run(self) -> int:
        from .states.start import TitleState

        self.push_state(TitleState(self.ctx))
        while self.running and self.states:
            dt = self.clock.tick(FPS) / 1000.0
            for event in pygame.event.get():
                if event.type == pygame.QUIT:
                    self.running = False
                elif self.state is not None:
                    self.state.handle_event(event)
            if self.state is not None:
                self.state.update(dt)
            self.render()
        self.shutdown()
        return 0

    def shutdown(self) -> None:
        self.speech.shutdown()
        pygame.quit()


def main() -> int:
    logging.basicConfig(level=logging.INFO, format="%(name)s: %(message)s")
    return App().run()


if __name__ == "__main__":
    sys.exit(main())
