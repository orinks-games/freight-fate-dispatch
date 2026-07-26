"""Small procedural audio layer; every prototype sound is synthesized in memory."""

from __future__ import annotations

import math
import os
import random
import struct

import pygame

SAMPLE_RATE = 22_050
_MAX_AMPLITUDE = 32_767

_CUE_NOTES = {
    "move": (520.0, 660.0),
    "confirm": (660.0, 880.0),
    "alert": (330.0, 247.5),
    "result": (440.0, 660.0),
}


def _pcm(samples) -> bytes:
    return b"".join(struct.pack("<h", max(-32_768, min(32_767, round(s)))) for s in samples)


def synthesize_cue(name: str) -> bytes:
    """Return a short deterministic mono PCM cue, without reading an asset."""
    first, second = _CUE_NOTES.get(name, _CUE_NOTES["move"])
    frames = round(SAMPLE_RATE * 0.09)

    def samples():
        for frame in range(frames):
            t = frame / SAMPLE_RATE
            progress = frame / frames
            envelope = min(1.0, progress * 12.0) * (1.0 - progress) ** 2
            tone = math.sin(math.tau * first * t) + 0.45 * math.sin(math.tau * second * t)
            yield _MAX_AMPLITUDE * 0.18 * envelope * tone

    return _pcm(samples())


def synthesize_music_loop(seed: int = 0, seconds: int = 8) -> bytes:
    """Build a periodic dispatch-desk pulse whose final-to-first step is smooth."""
    if seconds < 1:
        raise ValueError("music loop must last at least one second")
    rng = random.Random(seed)
    root_cycles = rng.choice((196, 220, 247, 262)) * seconds
    cycle_counts = (root_cycles, root_cycles * 5 // 4, root_cycles * 3 // 2)
    frames = SAMPLE_RATE * seconds

    def samples():
        for frame in range(frames):
            phase = frame / frames
            pad = sum(
                weight * math.sin(math.tau * cycles * phase)
                for weight, cycles in zip((1.0, 0.55, 0.42), cycle_counts)
            )
            pulse = 0.72 + 0.28 * math.sin(math.tau * 8 * phase - math.pi / 2)
            yield _MAX_AMPLITUDE * 0.075 * pulse * pad

    return _pcm(samples())


def _mixer_buffer(mono_pcm: bytes) -> bytes:
    """Match synthesized mono frames to pygame's already-initialized channel count."""
    initialized = pygame.mixer.get_init()
    channels = initialized[2] if initialized is not None else 1
    if channels == 1:
        return mono_pcm
    frames = (mono_pcm[index : index + 2] for index in range(0, len(mono_pcm), 2))
    return b"".join(frame * channels for frame in frames)


class AudioEngine:
    """Graceful pygame playback facade for synthesized cues and looped music."""

    def __init__(self) -> None:
        self.enabled = not (
            os.environ.get("FF_DISPATCH_NO_AUDIO") or os.environ.get("FF_DISPATCH_NO_SPEECH")
        )
        self._cues: dict[str, pygame.mixer.Sound] = {}
        self._music: pygame.mixer.Sound | None = None
        if not self.enabled:
            return
        try:
            initialized = pygame.mixer.get_init()
            channels = initialized[2] if initialized is not None else 2
            if initialized is None or initialized[:2] != (SAMPLE_RATE, -16):
                pygame.mixer.quit()
                pygame.mixer.init(frequency=SAMPLE_RATE, size=-16, channels=channels)
            self._cues = {
                name: pygame.mixer.Sound(buffer=_mixer_buffer(synthesize_cue(name)))
                for name in _CUE_NOTES
            }
            self._music = pygame.mixer.Sound(buffer=_mixer_buffer(synthesize_music_loop()))
            self._music.set_volume(0.22)
        except pygame.error:
            self.enabled = False
            self._cues.clear()
            self._music = None

    def play(self, name: str) -> None:
        sound = self._cues.get(name)
        if sound is not None:
            sound.play()

    def start_music(self) -> None:
        if self._music is not None:
            self._music.play(loops=-1, fade_ms=500)

    def shutdown(self) -> None:
        if self._music is not None:
            self._music.stop()
