"""State machine and the accessible menu base, in the Freight Fate idiom.

Players coming from Freight Fate already know this grammar: arrows with
wrap-around, Home/End, first-letter jump, Enter to activate, Escape to go
back, F1 for contextual help, Control to stop speech.
"""

from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass

import pygame


def end_sentence(text: str) -> str:
    text = text.rstrip()
    return text if text.endswith((".", "!", "?", ":")) else text + "."


class State:
    def __init__(self, ctx) -> None:
        self.ctx = ctx

    def enter(self) -> None: ...

    def exit(self) -> None: ...

    def handle_event(self, event: pygame.event.Event) -> None: ...

    def update(self, dt: float) -> None: ...

    def lines(self) -> list[str]:
        return []


@dataclass
class MenuItem:
    label: str | Callable[[], str]
    action: Callable[[], None]
    help: str = ""

    @property
    def text(self) -> str:
        return self.label() if callable(self.label) else self.label


class MenuState(State):
    title = "Menu"
    intro_help = "Use up and down arrows to navigate, Enter to select, Escape to go back."

    def __init__(self, ctx) -> None:
        super().__init__(ctx)
        self.items: list[MenuItem] = []
        self.index = 0

    def build_items(self) -> list[MenuItem]:
        raise NotImplementedError

    def enter(self) -> None:
        self.items = self.build_items()
        self.index = min(self.index, max(0, len(self.items) - 1))
        self.announce_entry()

    def announce_entry(self) -> None:
        self.ctx.say(f"{end_sentence(self.title)} {self.current_text()}")

    def refresh(self, keep_index: bool = True) -> None:
        old = self.index
        self.items = self.build_items()
        self.index = min(old if keep_index else 0, max(0, len(self.items) - 1))

    def current_text(self) -> str:
        if not self.items:
            return "No options available."
        return f"{end_sentence(self.items[self.index].text)} {self.index + 1} of {len(self.items)}."

    def speak_current(self) -> None:
        self.ctx.say(self.current_text())

    def current_help(self) -> str:
        if not self.items:
            return self.intro_help
        item = self.items[self.index]
        return item.help or f"{item.text}."

    def move(self, delta: int) -> None:
        if not self.items:
            return
        self.index = (self.index + delta) % len(self.items)
        self.speak_current()

    def jump(self, index: int) -> None:
        if not self.items:
            return
        self.index = max(0, min(index, len(self.items) - 1))
        self.speak_current()

    def activate(self) -> None:
        if self.items:
            self.items[self.index].action()

    def go_back(self) -> None:
        self.ctx.pop_state()

    def handle_event(self, event: pygame.event.Event) -> None:
        if event.type != pygame.KEYDOWN:
            return
        key = event.key
        if key == pygame.K_DOWN:
            self.move(1)
        elif key == pygame.K_UP:
            self.move(-1)
        elif key == pygame.K_HOME:
            self.jump(0)
        elif key == pygame.K_END:
            self.jump(len(self.items) - 1)
        elif key in (pygame.K_RETURN, pygame.K_SPACE, pygame.K_KP_ENTER):
            self.activate()
        elif key == pygame.K_ESCAPE:
            self.go_back()
        elif key == pygame.K_F1:
            self.ctx.say(self.current_help())
        elif key in (pygame.K_LCTRL, pygame.K_RCTRL):
            self.ctx.stop_speech()
        elif event.unicode and event.unicode.isalnum():
            self._first_letter_jump(event.unicode.lower())

    def _first_letter_jump(self, char: str) -> None:
        if not self.items:
            return
        n = len(self.items)
        for offset in range(1, n + 1):
            i = (self.index + offset) % n
            if self.items[i].text.lower().startswith(char):
                self.index = i
                self.speak_current()
                return

    def lines(self) -> list[str]:
        out = [self.title, ""]
        for i, item in enumerate(self.items):
            out.append(("> " if i == self.index else "  ") + item.text)
        return out
