#!/usr/bin/env python3
"""Composes and renders the Dispatch soundtrack as looping stems.

Each piece is written here as notes, rendered with FluidSynth through the
MuseScore General SoundFont, mixed, and cut into stems that loop without a
seam: every stem is rendered twice through, mastered as one continuous
take, and the second pass is kept, so the start of a stem already carries
the ringing tail of its own end. All stems of a piece are the same length
to the sample, which is what Hearken's stem player needs to keep them
locked together while it arranges them.

Run from the repository root:

    python3 audio/soundtrack.py            # renders every piece
    python3 audio/soundtrack.py rolling    # one piece
    python3 audio/soundtrack.py --wav      # also writes WAVs to listen to

Needs libfluidsynth, ffmpeg with libvorbis, the MuseScore General
SoundFont (Ubuntu: musescore-general-soundfont) and the Python packages
numpy, scipy, soundfile, pyfluidsynth and pedalboard.
"""

from __future__ import annotations

import os
import random
import subprocess
import sys
import tempfile
import zlib
from dataclasses import dataclass, field
from pathlib import Path

import ctypes

import fluidsynth
import numpy as np
import soundfile as sf
from pedalboard import (
    Compressor,
    HighpassFilter,
    HighShelfFilter,
    LowShelfFilter,
    Pedalboard,
    PeakFilter,
    Reverb,
)

SR = 44100
# Passes through the loop in one take: the second is kept as the stem, and
# the third is what the loop should sound like as it wraps, to check against.
PASSES = 3
ROOT = Path(__file__).resolve().parent.parent
OUT_DIR = ROOT / "assets" / "sounds" / "music"
# The sampled instruments, by the name a Part picks them with. Each can
# be pointed elsewhere with FF_DISPATCH_SF_<NAME>. MuseScore General is
# the Ubuntu package musescore-general-soundfont-lossless (the .sf3 of
# musescore-general-soundfont is the same instruments, compressed);
# GeneralUser GS is S. Christian Collins's bank, from
# https://github.com/mrbumpy409/GeneralUser-GS (GeneralUser-GS.sf2 at the
# root of the repository).
SOUNDFONTS = {
    "musescore": os.environ.get(
        "FF_DISPATCH_SF_MUSESCORE",
        next(
            (p for p in ["/usr/share/sounds/sf2/MuseScore_General_Full.sf2", "/usr/share/sounds/sf3/MuseScore_General_Full.sf3"] if os.path.exists(p)),
            "/usr/share/sounds/sf2/MuseScore_General_Full.sf2",
        ),
    ),
    "generaluser": os.environ.get("FF_DISPATCH_SF_GENERALUSER", str(Path.home() / "soundfonts" / "GeneralUser-GS.sf2")),
    "fluidr3": os.environ.get("FF_DISPATCH_SF_FLUIDR3", "/usr/share/sounds/sf2/FluidR3_GM.sf2"),
}
# Vorbis quality for the stems: music under speech, at the music bus's
# default of 40 percent, does not need more.
VORBIS_QUALITY = "3"
# `--wav` also writes each stem and the full mix as WAV here, to listen to.
WAV_DIR = Path(os.environ.get("FF_DISPATCH_MUSIC_WAVS", tempfile.gettempdir())) / "ff-dispatch-music"

NOTE_NAMES = {"C": 0, "D": 2, "E": 4, "F": 5, "G": 7, "A": 9, "B": 11}


def n(name: str) -> int:
    """MIDI number of a note name such as F#3 or Bb2 (C4 = 60)."""
    letter = name[0].upper()
    rest = name[1:]
    acc = 0
    while rest and rest[0] in "#b":
        acc += 1 if rest[0] == "#" else -1
        rest = rest[1:]
    return 12 * (int(rest) + 1) + NOTE_NAMES[letter] + acc


def chord(*names: str) -> list[int]:
    return [n(x) for x in names]


# GM drum notes (standard and brush kits share the map).
KICK = 36
SNARE = 38
SIDESTICK = 37
BRUSH_TAP = 38
BRUSH_SLAP = 39
BRUSH_SWIRL = 40
HAT = 42
HAT_PEDAL = 44
HAT_OPEN = 46
RIDE = 51
RIDE_BELL = 53
TAMBOURINE = 54
SHAKER = 82
CABASA = 69
CRASH = 49
TOM_LOW = 45
TOM_FLOOR = 41

# Where each drum sits, heard from the front of the kit: -1 left to 1 right.
DRUM_PAN = {
    HAT: 0.3,
    HAT_PEDAL: 0.3,
    HAT_OPEN: 0.3,
    RIDE: -0.35,
    RIDE_BELL: -0.35,
    TAMBOURINE: -0.25,
    SHAKER: 0.4,
    CABASA: 0.4,
    CRASH: -0.3,
    TOM_LOW: 0.15,
    TOM_FLOOR: -0.2,
}


@dataclass
class Note:
    beat: float
    dur: float
    pitch: int
    vel: int


@dataclass
class Part:
    """One instrument's notes for the whole loop."""

    program: int
    notes: list[Note] = field(default_factory=list)
    drums: bool = False
    # The SoundFont the instrument comes from: a key of SOUNDFONTS.
    font: str = "musescore"
    # Where the instrument sits, -1 left to 1 right, and how far its
    # notes fan out from there by pitch, low to the left: 0 keeps them
    # together, 1 puts the extremes an octave and a half apart at the
    # edges. A drum part sits by DRUM_PAN instead.
    pan: float = 0.0
    spread: float = 0.0
    # Expression (CC 11) for the part, 0..127: a quick way to balance a
    # soft pad against a lead without touching every velocity.
    expression: int = 127
    # Humanisation: timing jitter in seconds and velocity jitter.
    jitter_s: float = 0.006
    vel_jitter: int = 5
    # 0.5 is straight eighths; 0.58 is a light swing.
    swing: float = 0.5
    # How much of each note's written length is held.
    gate: float = 0.92
    # The piece's tempo, so a strum can be spelled in seconds.
    bpm: float = 100.0

    def add(self, beat: float, dur: float, pitch: int, vel: int) -> None:
        self.notes.append(Note(beat, dur, pitch, vel))

    def strum(
        self, beat: float, dur: float, pitches: list[int], vel: int, spread: float = 0.012, up: bool = False
    ) -> None:
        """A chord with its notes a few milliseconds apart, low to high."""
        order = list(pitches)
        if up:
            order.reverse()
        for i, p in enumerate(order):
            step = i * spread * self.bpm / 60.0
            self.add(beat + step, dur - step, p, vel)


@dataclass
class Stem:
    key: str
    parts: list[Part]
    # Target RMS of the stem in dBFS after mastering, and its place in the
    # mix: Hearken sums the stems, so these are the balance.
    rms_db: float
    # Mastering: pedalboard plugins applied after FluidSynth, per stem.
    chain: list = field(default_factory=list)
    reverb: float = 0.12
    reverb_size: float = 0.45


@dataclass
class Piece:
    id: str
    title: str
    bpm: float
    bars: int
    bed: list[Stem]
    voices: list[Stem]

    @property
    def beats(self) -> int:
        return self.bars * 4

    @property
    def loop_s(self) -> float:
        return self.beats * 60.0 / self.bpm

    @property
    def loop_frames(self) -> int:
        frames = self.loop_s * SR
        # FluidSynth takes note events at 64-frame block edges, so a loop
        # that is a whole number of blocks plays every pass on the same
        # edges and the take comes out exactly periodic.
        assert abs(frames - round(frames)) < 1e-6 and round(frames) % 64 == 0, (
            f"{self.id}: {self.bpm} BPM gives a loop of {frames} frames, not a whole number of 64-frame blocks"
        )
        return int(round(frames))


A_MAJOR = [9, 11, 1, 2, 4, 6, 8]


def diatonic_below(pitch: int, scale: list[int], steps: int) -> int:
    """The note `steps` scale degrees below `pitch` in `scale` (pitch classes)."""
    p = pitch
    while p % 12 not in scale:
        p -= 1
    for _ in range(steps):
        p -= 1
        while p % 12 not in scale:
            p -= 1
    return p


# ---------------------------------------------------------------------------
# Pattern helpers


def comp_keys(part: Part, bars: list[list[int]], pattern: list[tuple[float, float]], vel: int, var: random.Random) -> None:
    """Chord hits on `pattern` (beat offset, length) in every bar."""
    for bar, voicing in enumerate(bars):
        for off, length in pattern:
            v = vel + var.randint(-6, 6)
            part.strum(bar * 4 + off, length, voicing, v, spread=0.018)


def sustain_keys(part: Part, bars: list[list[int]], vel: int, hold: float = 4.0) -> None:
    """Each chord held for its bar; a repeated chord is held across."""
    i = 0
    while i < len(bars):
        j = i
        while j + 1 < len(bars) and bars[j + 1] == bars[i]:
            j += 1
        length = (j - i + 1) * hold
        part.strum(i * 4, length, bars[i], vel, spread=0.03)
        i = j + 1


def two_feel_bass(part: Part, roots: list[int], fifths: list[int], vel: int, var: random.Random, walk_bars: set[int] = frozenset()) -> None:
    for bar, (r, f) in enumerate(zip(roots, fifths)):
        if bar in walk_bars:
            nxt = roots[(bar + 1) % len(roots)]
            # A four-note walk up or down into the next root.
            steps = [r, r + 2 if nxt > r else r - 1, f if f > r else f - 12, nxt - 1 if nxt > r else nxt + 1]
            for k, p in enumerate(steps):
                part.add(bar * 4 + k, 1.0, p, vel + var.randint(-4, 4))
            continue
        part.add(bar * 4, 1.6, r, vel + var.randint(-4, 4))
        if var.random() < 0.35:
            part.add(bar * 4 + 1.5, 0.5, r, vel - 18)
        part.add(bar * 4 + 2, 1.6, f, vel - 4 + var.randint(-4, 4))
        if var.random() < 0.5:
            part.add(bar * 4 + 3.5, 0.5, r + (2 if var.random() < 0.5 else -1), vel - 14)


def arpeggio(part: Part, bars: list[list[int]], order: list[int], vel: int, var: random.Random, step: float = 0.5) -> None:
    """Fingerpicking: `order` indexes into each bar's chord tones."""
    for bar, tones in enumerate(bars):
        for k, idx in enumerate(order):
            p = tones[idx % len(tones)]
            part.add(bar * 4 + k * step, step * 1.6, p, vel + var.randint(-7, 7) - (0 if k % 2 == 0 else 8))


def melody(part: Part, line: str, vel: int, var: random.Random, start: float = 0.0) -> float:
    """A tune written as "G4:1 A4:0.5 -:0.5": pitch:beats, '-' rests.

    Returns the beat after the last note.
    """
    beat = start
    for tok in line.split():
        name, dur = tok.split(":")
        d = float(dur)
        if name != "-":
            part.add(beat, d, n(name), vel + var.randint(-6, 6))
        beat += d
    return beat


def drum_grid(part: Part, bars: int, note: int, grid: str, vel: int, var: random.Random, accent: str = "", jitter: int = 6) -> None:
    """A 16-step pattern per bar: 'x' hits, 'X' accents, '.' rests."""
    steps = len(grid)
    for bar in range(bars):
        for i, c in enumerate(grid):
            if c == ".":
                continue
            v = vel + (14 if c == "X" else 0) + var.randint(-jitter, jitter)
            part.add(bar * 4 + i * 4.0 / steps, 0.25, note, max(1, min(127, v)))


# ---------------------------------------------------------------------------
# The pieces


def first_light() -> Piece:
    """Morning at the desk: G major, 96, a light swing. Rhodes and organ
    under brushes, upright bass, a fingerpicked guitar, vibes carrying the
    tune and a jazz guitar answering it."""
    var = random.Random(11)
    bpm = 98.0
    bars = 16
    keys = [
        chord("B3", "D4", "F#4", "A4"),  # Gmaj9
        chord("B3", "D4", "F#4", "A4"),
        chord("G3", "B3", "D4", "F#4"),  # Em9
        chord("G3", "B3", "D4", "F#4"),
        chord("E3", "G3", "B3", "D4"),  # Cmaj7
        chord("E3", "G3", "B3", "D4"),
        chord("G3", "C4", "E4", "A4"),  # D7sus4 (D9sus)
        chord("F#3", "C4", "E4", "A4"),  # D9
        chord("B3", "D4", "F#4", "A4"),  # Gmaj9
        chord("B3", "D4", "F#4", "A4"),
        chord("D4", "F#4", "A4", "C#5"),  # Bm7
        chord("D4", "F#4", "A4", "C#5"),
        chord("E4", "G4", "B4", "D5"),  # Cmaj7
        chord("E4", "G4", "B4", "C5"),  # Am7/C -> Am9 colour
        chord("G3", "C4", "E4", "A4"),  # D7sus4
        chord("F#3", "C4", "E4", "A4"),  # D9
    ]
    roots = [n(x) for x in ["G2", "G2", "E2", "E2", "C2", "C2", "D2", "D2", "G2", "G2", "B1", "B1", "C2", "A1", "D2", "D2"]]
    fifths = [n(x) for x in ["D3", "D3", "B2", "B2", "G2", "G2", "A2", "A2", "D3", "D3", "F#2", "F#2", "G2", "E2", "A2", "A2"]]
    guitar_tones = [
        chord("G2", "D3", "B3", "D4", "G4"),
        chord("G2", "D3", "B3", "D4", "F#4"),
        chord("E2", "B2", "G3", "D4", "E4"),
        chord("E2", "B2", "G3", "D4", "F#4"),
        chord("C3", "G3", "E4", "G4", "B4"),
        chord("C3", "G3", "E4", "G4", "D5"),
        chord("D3", "A3", "G4", "C5", "E5"),
        chord("D3", "A3", "F#4", "C5", "E5"),
        chord("G2", "D3", "B3", "D4", "G4"),
        chord("G2", "D3", "B3", "D4", "A4"),
        chord("B2", "F#3", "D4", "A4", "C#5"),
        chord("B2", "F#3", "D4", "A4", "B4"),
        chord("C3", "G3", "E4", "B4", "D5"),
        chord("A2", "E3", "G4", "C5", "E5"),
        chord("D3", "A3", "G4", "C5", "E5"),
        chord("D3", "A3", "F#4", "C5", "E5"),
    ]

    rhodes = Part(bpm=bpm, program=4, expression=100, swing=0.57, gate=0.95, spread=0.8)
    comp_keys(
        rhodes,
        keys,
        [(0.0, 1.5), (1.5, 1.0), (3.0, 0.5), (3.5, 1.0)],
        64,
        var,
    )
    organ = Part(bpm=bpm, program=16, expression=52, jitter_s=0.0, vel_jitter=0, gate=1.0, pan=0.15, spread=0.5)
    sustain_keys(organ, [k[1:] for k in keys], 52)

    bass = Part(bpm=bpm, program=32, expression=118, swing=0.57, gate=0.85)
    two_feel_bass(bass, roots, fifths, 92, var, walk_bars={7, 15})

    drums = Part(bpm=bpm, program=40, drums=True, swing=0.57, expression=110, gate=0.5)
    drum_grid(drums, bars, KICK, "X.......x.......", 70, var)
    drum_grid(drums, bars, BRUSH_TAP, "....X.......X...", 62, var)
    drum_grid(drums, bars, BRUSH_SWIRL, "..x...x...x...x.", 34, var, jitter=10)
    drum_grid(drums, bars, RIDE, "x.x.x.x.x.x.x.x.", 44, var)
    shaker = Part(bpm=bpm, program=40, drums=True, swing=0.57, expression=70, gate=0.5)
    drum_grid(shaker, bars, SHAKER, "x.xXx.xXx.xXx.xX", 50, var, jitter=9)
    drums.notes += shaker.notes

    guitar = Part(bpm=bpm, program=25, expression=96, swing=0.57, gate=1.4, pan=-0.3, spread=0.5)
    arpeggio(guitar, guitar_tones, [0, 2, 1, 3, 0, 4, 1, 3], 58, var)

    vibes = Part(bpm=bpm, program=11, expression=112, swing=0.57, gate=1.3, pan=0.2, spread=0.6)
    tune = " ".join(
        [
            # Phrase A (bars 1-4): a question over G, answered on E minor.
            "-:1 D5:1 B4:0.5 A4:0.5 G4:1 | A4:0.5 B4:1.5 D5:1 -:1 | B4:1 G4:0.5 A4:0.5 B4:2 | -:4".replace("|", ""),
            # Phrase B (bars 5-8): rising over C, resting on the D.
            "E5:1 D5:0.5 B4:0.5 G4:1 A4:1 | B4:1.5 A4:0.5 G4:2 | -:1 A4:1 B4:0.5 C5:0.5 D5:1 | E5:1.5 D5:0.5 B4:2".replace("|", ""),
            # Phrase A' (bars 9-12): the question again, warmer.
            "-:1 D5:1 B4:0.5 A4:0.5 G4:1 | A4:0.5 B4:1.5 D5:1 E5:1 | F#5:1 D5:0.5 B4:0.5 A4:2 | -:4".replace("|", ""),
            # Phrase B' (bars 13-16): home, with the last note leading back.
            "G5:1 E5:0.5 D5:0.5 B4:1 A4:1 | G4:1.5 A4:0.5 B4:2 | -:1 A4:1 G4:0.5 A4:0.5 B4:1 | A4:1.5 G4:0.5 -:2".replace("|", ""),
        ]
    )
    melody(vibes, tune, 78, var)

    answers = Part(bpm=bpm, program=26, expression=84, swing=0.57, gate=1.1, pan=-0.4, spread=0.3)
    # Short replies in the gaps the vibes leave (bars 4, 8, 12, 16).
    melody(answers, "D4:0.5 E4:0.5 G4:1 F#4:1 D4:1", 62, var, start=12.0)
    melody(answers, "B3:0.5 D4:0.5 E4:1", 54, var, start=30.0)
    melody(answers, "A4:0.5 G4:0.5 E4:1 D4:1 B3:1", 62, var, start=44.0)
    melody(answers, "F#4:0.5 G4:0.5 A4:1", 64, var, start=62.0)

    warm = [HighpassFilter(60), LowShelfFilter(180, 1.5), HighShelfFilter(6000, -2.0)]
    return Piece(
        id="first_light",
        title="First Light",
        bpm=bpm,
        bars=bars,
        bed=[Stem("music/first_light/keys", [rhodes, organ], -26.5, warm, reverb=0.16)],
        voices=[
            Stem("music/first_light/bass", [bass], -23.5, [HighpassFilter(35), PeakFilter(90, 1.5, 1.0), Compressor(-18, 3.0, 8, 120)], reverb=0.03),
            Stem("music/first_light/drums", [drums], -29.5, [HighpassFilter(45), Compressor(-20, 2.5, 5, 80)], reverb=0.10),
            Stem("music/first_light/guitar", [guitar], -28.5, [HighpassFilter(80), PeakFilter(2800, 1.0, 1.2)], reverb=0.14),
            Stem("music/first_light/vibes", [vibes], -27.5, [HighpassFilter(150)], reverb=0.22, reverb_size=0.55),
            Stem("music/first_light/answers", [answers], -30.5, [HighpassFilter(120), HighShelfFilter(5000, -3.0)], reverb=0.18),
        ],
    )


def rolling() -> Piece:
    """The afternoon, trucks moving: A major, 100, a train beat. Organ
    underneath, picked bass, a strummed steel guitar, harmonica on the tune
    and a clean electric answering in sixths."""
    var = random.Random(23)
    bpm = 100.0
    bars = 16
    # A | A | D | A | A | E7 | D | A  ||  F#m | D | A | E | F#m | D | A E | A
    names = ["A", "A", "D", "A", "A", "E", "D", "A", "F#m", "D", "A", "E", "F#m", "D", "AE", "A"]
    voicings = {
        "A": chord("C#4", "E4", "A4"),
        "D": chord("D4", "F#4", "A4"),
        "E": chord("D4", "E4", "G#4", "B4"),
        "F#m": chord("C#4", "F#4", "A4"),
    }
    organ_voicings = {
        "A": chord("E3", "A3", "C#4"),
        "D": chord("F#3", "A3", "D4"),
        "E": chord("E3", "G#3", "B3", "D4"),
        "F#m": chord("F#3", "A3", "C#4"),
    }
    strums = {
        "A": chord("A2", "E3", "A3", "C#4", "E4"),
        "D": chord("D3", "A3", "D4", "F#4"),
        "E": chord("E2", "B2", "E3", "G#3", "B3", "E4"),
        "F#m": chord("F#2", "C#3", "F#3", "A3", "C#4"),
    }
    root_of = {"A": n("A1"), "D": n("D2"), "E": n("E2"), "F#m": n("F#1")}
    fifth_of = {"A": n("E2"), "D": n("A2"), "E": n("B2"), "F#m": n("C#2")}

    def split(name: str) -> list[str]:
        return ["A", "E"] if name == "AE" else [name]

    organ = Part(bpm=bpm, program=16, expression=58, jitter_s=0.0, vel_jitter=0, gate=1.0, spread=0.6)
    for bar, name in enumerate(names):
        halves = split(name)
        for h, c in enumerate(halves):
            organ.strum(bar * 4 + h * (4 / len(halves)), 4 / len(halves), organ_voicings[c], 56, spread=0.02)

    bass = Part(bpm=bpm, program=33, expression=118, gate=0.8)
    for bar, name in enumerate(names):
        halves = split(name)
        for h, c in enumerate(halves):
            r, f = root_of[c], fifth_of[c]
            base = bar * 4 + h * (4 / len(halves))
            if len(halves) == 2:
                bass.add(base, 1.0, r, 96)
                bass.add(base + 1, 1.0, f, 88)
                continue
            bass.add(base, 1.0, r, 98 + var.randint(-3, 3))
            bass.add(base + 1, 1.0, f, 86 + var.randint(-3, 3))
            bass.add(base + 2, 1.0, r, 94 + var.randint(-3, 3))
            nxt = root_of[split(names[(bar + 1) % bars])[0]]
            if bar % 4 == 3 and nxt != r:
                # Walk into the next chord.
                bass.add(base + 3, 0.5, nxt - 2 if nxt > r else nxt + 2, 84)
                bass.add(base + 3.5, 0.5, nxt - 1 if nxt > r else nxt + 1, 86)
            else:
                bass.add(base + 3, 1.0, f, 84 + var.randint(-3, 3))

    drums = Part(bpm=bpm, program=0, drums=True, expression=112, gate=0.5)
    drum_grid(drums, bars, KICK, "X.......X.......", 92, var, jitter=3)
    drum_grid(drums, bars, SNARE, "....X.......X...", 86, var, jitter=4)
    # The train: brushed-feel ghost eighths on the snare between backbeats.
    drum_grid(drums, bars, SIDESTICK, "..x...x...x...x.", 46, var, jitter=8)
    drum_grid(drums, bars, HAT, "x.x.x.x.x.x.x.x.", 58, var, jitter=5)
    drum_grid(drums, bars, HAT_OPEN, "..............x.", 40, var)
    drum_grid(drums, bars, TAMBOURINE, "....x.......x...", 44, var)
    drums.notes.append(Note(bars * 4 - 0.5, 0.25, TOM_LOW, 70))
    drums.notes.append(Note(bars * 4 - 0.25, 0.25, TOM_FLOOR, 74))

    guitar = Part(bpm=bpm, program=25, expression=100, gate=0.9, pan=-0.35, spread=0.5)
    # Boom-chick: bass note on 1 and 3, strum on the ands, up-strums light.
    for bar, name in enumerate(names):
        halves = split(name)
        for h, c in enumerate(halves):
            tones = strums[c]
            base = bar * 4 + h * (4 / len(halves))
            beats = 4 / len(halves)
            guitar.add(base, 0.9, tones[0], 76 + var.randint(-4, 4))
            guitar.strum(base + 0.5, 0.45, tones[1:], 66, spread=0.014)
            guitar.strum(base + 1.0, 0.45, tones[1:], 58, spread=0.012, up=True)
            guitar.strum(base + 1.5, 0.45, tones[1:], 68, spread=0.014)
            if beats == 4:
                guitar.add(base + 2, 0.9, tones[1] if len(tones) > 5 else tones[0], 72 + var.randint(-4, 4))
                guitar.strum(base + 2.5, 0.45, tones[1:], 66, spread=0.014)
                guitar.strum(base + 3.0, 0.45, tones[1:], 58, spread=0.012, up=True)
                guitar.strum(base + 3.5, 0.45, tones[1:], 70, spread=0.014)

    harp = Part(bpm=bpm, program=22, expression=108, gate=1.1, pan=0.25, spread=0.2)
    tune = " ".join(
        [
            "-:2 E4:0.5 F#4:0.5 A4:1 | A4:0.5 C#5:1.5 A4:1 F#4:1 | E4:1 F#4:0.5 A4:0.5 B4:1 A4:1 | -:4".replace("|", ""),
            "-:2 A4:0.5 B4:0.5 C#5:1 | E5:1.5 C#5:0.5 A4:2 | B4:1 A4:0.5 F#4:0.5 E4:1 F#4:1 | A4:3 -:1".replace("|", ""),
            "C#5:1 C#5:0.5 E5:0.5 F#5:1 E5:1 | D5:1.5 C#5:0.5 A4:2 | B4:1 C#5:0.5 B4:0.5 A4:1 F#4:1 | G#4:2 B4:1 -:1".replace("|", ""),
            "C#5:1 A4:0.5 F#4:0.5 E4:1 F#4:1 | A4:1.5 F#4:0.5 D4:2 | E4:1 F#4:0.5 A4:0.5 B4:1 C#5:1 | A4:3 -:1".replace("|", ""),
        ]
    )
    melody(harp, tune, 84, var)

    sixths = Part(bpm=bpm, program=27, expression=86, gate=1.0, pan=-0.3, spread=0.4)
    # Country double-stops in the holes: bars 4, 8, 12, 16 and the AE bar.
    for start, line in [
        (12.0, "C#4:0.5 E4:0.5 F#4:1 E4:1 C#4:1"),
        (27.5, "E4:0.5 F#4:0.5 A4:1 B4:1 A4:1"),
        (44.0, "G#4:1 B4:1 A4:1 F#4:1"),
        (61.0, "F#4:0.5 E4:0.5 C#4:1 A3:1"),
    ]:
        beat = start
        for tok in line.split():
            name, dur = tok.split(":")
            d = float(dur)
            p = n(name)
            sixths.add(beat, d, p, 64 + var.randint(-5, 5))
            sixths.add(beat + 0.03, d, diatonic_below(p, A_MAJOR, 5), 56 + var.randint(-5, 5))
            beat += d

    return Piece(
        id="rolling",
        title="Rolling",
        bpm=bpm,
        bars=bars,
        bed=[Stem("music/rolling/organ", [organ], -29.5, [HighpassFilter(90), HighShelfFilter(4000, -3.0)], reverb=0.16)],
        voices=[
            Stem("music/rolling/bass", [bass], -23.5, [HighpassFilter(35), PeakFilter(100, 1.5, 1.0), Compressor(-18, 3.0, 8, 120)], reverb=0.02),
            Stem("music/rolling/drums", [drums], -26.5, [HighpassFilter(45), Compressor(-18, 2.5, 4, 70)], reverb=0.09),
            Stem("music/rolling/guitar", [guitar], -26.5, [HighpassFilter(90), PeakFilter(3000, 1.0, 1.2), Compressor(-20, 2.0, 10, 100)], reverb=0.12),
            Stem("music/rolling/harmonica", [harp], -27.5, [HighpassFilter(200), PeakFilter(1800, 1.0, 1.5)], reverb=0.18, reverb_size=0.5),
            Stem("music/rolling/sixths", [sixths], -30.5, [HighpassFilter(150), HighShelfFilter(5000, -2.0)], reverb=0.2),
        ],
    )


def night_desk() -> Piece:
    """Evening, the board nearly cleared: E minor, 75, half time. Rhodes
    and slow strings as the bed, upright bass, brush swirls, a nylon
    guitar, piano on the tune and a muted trumpet far back."""
    var = random.Random(37)
    bpm = 75.0
    bars = 16
    keys = [
        chord("G3", "B3", "D4", "F#4"),  # Em9
        chord("G3", "B3", "D4", "F#4"),
        chord("E3", "G3", "B3", "D4"),  # Cmaj7
        chord("E3", "G3", "B3", "D4"),
        chord("D3", "G3", "B3", "D4"),  # G
        chord("F#3", "A3", "C4", "D4"),  # D/F#
        chord("E3", "G3", "C4", "E4"),  # Am7
        chord("F#3", "A3", "D#4", "E4"),  # B7sus4 -> B7
        chord("G3", "B3", "D4", "F#4"),  # Em9
        chord("E3", "G3", "B3", "D4"),  # Cmaj7
        chord("D3", "G3", "B3", "D4"),  # G
        chord("F#3", "A3", "C4", "D4"),  # D/F#
        chord("E3", "G3", "B3", "D4"),  # Cmaj7
        chord("D3", "G3", "B3", "D4"),  # G/B
        chord("E3", "G3", "C4", "E4"),  # Am7
        chord("F#3", "A3", "D#4", "E4"),  # B7
    ]
    roots = [n(x) for x in ["E2", "E2", "C2", "C2", "G2", "F#2", "A1", "B1", "E2", "C2", "G2", "F#2", "C2", "B1", "A1", "B1"]]
    fifths = [n(x) for x in ["B2", "B2", "G2", "G2", "D3", "D3", "E2", "F#2", "B2", "G2", "D3", "D3", "G2", "G2", "E2", "F#2"]]
    nylon_tones = [
        chord("E2", "B2", "G3", "D4", "F#4"),
        chord("E2", "B2", "G3", "D4", "E4"),
        chord("C3", "G3", "E4", "G4", "B4"),
        chord("C3", "G3", "E4", "G4", "D5"),
        chord("G2", "D3", "B3", "D4", "G4"),
        chord("F#2", "D3", "A3", "D4", "F#4"),
        chord("A2", "E3", "G3", "C4", "E4"),
        chord("B2", "F#3", "A3", "D#4", "F#4"),
        chord("E2", "B2", "G3", "D4", "F#4"),
        chord("C3", "G3", "E4", "G4", "B4"),
        chord("G2", "D3", "B3", "D4", "G4"),
        chord("F#2", "D3", "A3", "D4", "F#4"),
        chord("C3", "G3", "E4", "G4", "B4"),
        chord("B2", "G3", "D4", "G4", "B4"),
        chord("A2", "E3", "G3", "C4", "E4"),
        chord("B2", "F#3", "A3", "D#4", "F#4"),
    ]

    rhodes = Part(bpm=bpm, program=4, expression=96, gate=1.0, jitter_s=0.01, spread=0.8)
    for bar, voicing in enumerate(keys):
        rhodes.strum(bar * 4, 3.6, voicing, 58 + var.randint(-4, 4), spread=0.035)
        if bar % 2 == 1:
            rhodes.strum(bar * 4 + 2.5, 1.4, voicing[1:], 46 + var.randint(-4, 4), spread=0.03)
    strings = Part(bpm=bpm, program=49, expression=46, jitter_s=0.0, vel_jitter=0, gate=1.0, spread=0.7)
    sustain_keys(strings, [[k[0] - 12, k[2], k[3]] for k in keys], 50)

    bass = Part(bpm=bpm, program=32, expression=116, gate=0.9)
    for bar, (r, f) in enumerate(zip(roots, fifths)):
        bass.add(bar * 4, 2.4, r, 86 + var.randint(-4, 4))
        bass.add(bar * 4 + 2.5, 1.4, f, 74 + var.randint(-4, 4))
        if bar % 4 == 3:
            nxt = roots[(bar + 1) % bars]
            bass.add(bar * 4 + 3.5, 0.5, nxt + 1 if nxt < r else nxt - 1, 70)

    drums = Part(bpm=bpm, program=40, drums=True, expression=96, gate=0.6)
    drum_grid(drums, bars, KICK, "X...............", 58, var)
    drum_grid(drums, bars, BRUSH_SWIRL, "x...x...x...x...", 48, var, jitter=8)
    drum_grid(drums, bars, BRUSH_TAP, "........X.......", 50, var)
    drum_grid(drums, bars, RIDE, "..x...x...x...x.", 30, var, jitter=6)

    nylon = Part(bpm=bpm, program=24, expression=92, gate=1.5, jitter_s=0.01, pan=-0.3, spread=0.5)
    arpeggio(nylon, nylon_tones, [0, 2, 3, 4, 3, 2, 1, 3], 56, var)

    piano = Part(bpm=bpm, program=0, expression=100, gate=1.3, jitter_s=0.012, pan=0.1, spread=0.7)
    tune = " ".join(
        [
            "-:2 B4:1 E5:1 | D5:2 B4:1.5 G4:0.5 | A4:3 -:1 | -:4".replace("|", ""),
            "-:2 D5:1 G5:1 | F#5:2 D5:1.5 B4:0.5 | C5:1 B4:1 A4:2 | -:1 D#5:1 F#5:1 A5:1".replace("|", ""),
            "G5:2 F#5:1 E5:1 | D5:2 B4:2 | -:1 G4:1 A4:1 B4:1 | A4:2 F#4:2".replace("|", ""),
            "G4:1 E5:2 D5:1 | B4:2 D5:2 | C5:1.5 B4:0.5 A4:2 | B4:1.5 -:0.5 F#4:2".replace("|", ""),
        ]
    )
    melody(piano, tune, 64, var)

    trumpet = Part(bpm=bpm, program=59, expression=66, gate=1.1, jitter_s=0.015, pan=0.35)
    melody(trumpet, "B3:2 E4:2", 52, var, start=12.0)
    melody(trumpet, "G4:1.5 F#4:0.5 D4:2", 50, var, start=24.0)
    melody(trumpet, "E4:2 D4:1 B3:1", 52, var, start=44.0)
    melody(trumpet, "D#4:1 F#4:1 B4:2", 54, var, start=60.0)

    return Piece(
        id="night_desk",
        title="Night Desk",
        bpm=bpm,
        bars=bars,
        bed=[Stem("music/night_desk/keys", [rhodes, strings], -27.5, [HighpassFilter(60), HighShelfFilter(5000, -3.0)], reverb=0.22, reverb_size=0.6)],
        voices=[
            Stem("music/night_desk/bass", [bass], -24.5, [HighpassFilter(35), PeakFilter(85, 1.5, 1.0), Compressor(-18, 3.0, 8, 150)], reverb=0.04),
            Stem("music/night_desk/drums", [drums], -33.5, [HighpassFilter(50)], reverb=0.14),
            Stem("music/night_desk/nylon", [nylon], -29.5, [HighpassFilter(90), HighShelfFilter(6000, -2.0)], reverb=0.2, reverb_size=0.55),
            Stem("music/night_desk/piano", [piano], -28.5, [HighpassFilter(150)], reverb=0.26, reverb_size=0.65),
            Stem("music/night_desk/trumpet", [trumpet], -32.5, [HighpassFilter(200), HighShelfFilter(4000, -4.0)], reverb=0.3, reverb_size=0.7),
        ],
    )


def title() -> Piece:
    """The start menu: D major, 84, a theme stated plainly. Strings and
    Rhodes as the bed, upright bass, brushes, a picked steel guitar, piano
    on the tune and a French horn answering it."""
    var = random.Random(53)
    bpm = 84.0
    bars = 16
    # D | A/C# | Bm7 | G | D/F# | G | A7sus | A7 | Bm | F#m | G | D/F# | Em7 | A7sus | D | A7sus
    keys = [
        chord("F#3", "A3", "D4", "F#4"),
        chord("E3", "A3", "C#4", "E4"),
        chord("F#3", "A3", "B3", "D4"),
        chord("D3", "G3", "B3", "D4"),
        chord("F#3", "A3", "D4", "F#4"),
        chord("G3", "B3", "D4", "G4"),
        chord("G3", "A3", "D4", "E4"),
        chord("G3", "A3", "C#4", "E4"),
        chord("F#3", "B3", "D4", "F#4"),
        chord("F#3", "A3", "C#4", "F#4"),
        chord("G3", "B3", "D4", "G4"),
        chord("F#3", "A3", "D4", "F#4"),
        chord("G3", "B3", "D4", "E4"),
        chord("G3", "A3", "D4", "E4"),
        chord("F#3", "A3", "D4", "F#4"),
        chord("G3", "A3", "C#4", "E4"),
    ]
    roots = [n(x) for x in ["D2", "C#2", "B1", "G2", "F#2", "G2", "A1", "A1", "B1", "F#2", "G2", "F#2", "E2", "A1", "D2", "A1"]]
    fifths = [n(x) for x in ["A2", "E2", "F#2", "D3", "C#3", "D3", "E2", "E2", "F#2", "C#3", "D3", "C#3", "B2", "E2", "A2", "E2"]]
    guitar_tones = [
        chord("D3", "A3", "D4", "F#4", "A4"),
        chord("C#3", "A3", "C#4", "E4", "A4"),
        chord("B2", "F#3", "D4", "F#4", "A4"),
        chord("G2", "D3", "G3", "B3", "D4"),
        chord("F#2", "D3", "A3", "D4", "F#4"),
        chord("G2", "D3", "B3", "D4", "G4"),
        chord("A2", "E3", "G3", "D4", "E4"),
        chord("A2", "E3", "G3", "C#4", "E4"),
        chord("B2", "F#3", "B3", "D4", "F#4"),
        chord("F#2", "C#3", "A3", "C#4", "F#4"),
        chord("G2", "D3", "B3", "D4", "G4"),
        chord("F#2", "D3", "A3", "D4", "F#4"),
        chord("E2", "B2", "G3", "D4", "E4"),
        chord("A2", "E3", "G3", "D4", "E4"),
        chord("D3", "A3", "D4", "F#4", "A4"),
        chord("A2", "E3", "G3", "C#4", "E4"),
    ]

    strings = Part(bpm=bpm, program=48, expression=54, jitter_s=0.0, vel_jitter=0, gate=1.0, spread=0.7)
    sustain_keys(strings, [[k[0] - 12, k[1], k[2], k[3]] for k in keys], 54)
    rhodes = Part(bpm=bpm, program=4, expression=88, gate=1.0, jitter_s=0.01, spread=0.8)
    for bar, voicing in enumerate(keys):
        rhodes.strum(bar * 4, 3.6, voicing, 54 + var.randint(-4, 4), spread=0.03)
        if bar % 2 == 1:
            rhodes.strum(bar * 4 + 2.5, 1.4, voicing[1:], 44 + var.randint(-4, 4), spread=0.025)

    bass = Part(bpm=bpm, program=32, expression=116, gate=0.88)
    two_feel_bass(bass, roots, fifths, 88, var, walk_bars={7, 15})

    drums = Part(bpm=bpm, program=40, drums=True, expression=100, gate=0.5)
    drum_grid(drums, bars, KICK, "X.......x.......", 62, var)
    drum_grid(drums, bars, BRUSH_TAP, "....X.......X...", 56, var)
    drum_grid(drums, bars, BRUSH_SWIRL, "..x...x...x...x.", 32, var, jitter=10)
    drum_grid(drums, bars, RIDE, "x.x.x.x.x.x.x.x.", 36, var)
    drums.notes.append(Note(bars * 4 - 1.0, 0.25, TOM_LOW, 58))
    drums.notes.append(Note(bars * 4 - 0.5, 0.25, TOM_FLOOR, 62))

    guitar = Part(bpm=bpm, program=25, expression=94, gate=1.5, jitter_s=0.008, pan=-0.3, spread=0.5)
    arpeggio(guitar, guitar_tones, [0, 2, 3, 4, 1, 3, 2, 3], 56, var)

    piano = Part(bpm=bpm, program=0, expression=104, gate=1.2, jitter_s=0.01, pan=0.1, spread=0.6)
    tune = " ".join(
        [
            # The theme (bars 1-4): up from the fifth and settling on the sixth.
            "-:1 A4:1 F#4:0.5 G4:0.5 A4:1 | B4:1.5 A4:0.5 E4:2 | F#4:1 D5:1 B4:0.5 A4:0.5 F#4:1 | G4:2 A4:1 B4:1".replace("|", ""),
            # Its answer (bars 5-8), coming to rest on the dominant.
            "A4:1.5 F#4:0.5 D4:2 | -:1 G4:0.5 A4:0.5 B4:1 D5:1 | E5:2 D5:1 B4:1 | A4:3 -:1".replace("|", ""),
            # Higher (bars 9-12), over the minor turn.
            "F#5:1 D5:1 B4:0.5 C#5:0.5 D5:1 | C#5:2 A4:2 | B4:1 D5:1 G5:1 F#5:1 | E5:1.5 D5:0.5 A4:2".replace("|", ""),
            # Home (bars 13-16), with a pickup that leads back to the top.
            "G4:1 B4:1 E5:1 D5:1 | B4:1.5 A4:0.5 G4:1 E4:1 | F#4:1 D4:1 -:2 | -:1 E4:1 F#4:1 G4:1".replace("|", ""),
        ]
    )
    melody(piano, tune, 70, var)

    horn = Part(bpm=bpm, program=60, expression=62, gate=1.05, jitter_s=0.015, pan=0.3)
    melody(horn, "D4:1 E4:1 F#4:1", 50, var, start=29.0)
    melody(horn, "B3:4 A3:4", 44, var, start=40.0)
    melody(horn, "D4:2 A3:2 G3:2 A3:2", 48, var, start=56.0)

    warm = [HighpassFilter(60), LowShelfFilter(180, 1.0), HighShelfFilter(6000, -2.5)]
    return Piece(
        id="title",
        title="Freight Fate",
        bpm=bpm,
        bars=bars,
        # The tune is in the bed: the start menu always opens with it.
        bed=[
            Stem("music/title/keys", [strings, rhodes], -27.5, warm, reverb=0.2, reverb_size=0.6),
            Stem("music/title/piano", [piano], -27.5, [HighpassFilter(140)], reverb=0.24, reverb_size=0.6),
        ],
        voices=[
            Stem("music/title/bass", [bass], -24.0, [HighpassFilter(35), PeakFilter(90, 1.5, 1.0), Compressor(-18, 3.0, 8, 120)], reverb=0.03),
            Stem("music/title/drums", [drums], -31.5, [HighpassFilter(45), Compressor(-20, 2.5, 5, 80)], reverb=0.12),
            Stem("music/title/guitar", [guitar], -28.5, [HighpassFilter(80), PeakFilter(2800, 1.0, 1.2)], reverb=0.16),
            Stem("music/title/horn", [horn], -31.0, [HighpassFilter(150), HighShelfFilter(4000, -3.0)], reverb=0.3, reverb_size=0.7),
        ],
    )


PIECES = {p.id: p for p in [title(), first_light(), rolling(), night_desk()]}


# ---------------------------------------------------------------------------
# Rendering


class Synth:
    """FluidSynth rendered a block at a time through its C API, so every
    note lands on a block edge the same way in every pass. (The command
    line player times events in whole milliseconds, which drift against
    the 64-frame blocks and break the periodicity a loop needs.)"""

    BLOCK = 64

    def __init__(self) -> None:
        self.fs = fluidsynth.Synth(samplerate=SR, gain=0.9)
        self.fs.setting("synth.reverb.active", 0)
        self.fs.setting("synth.chorus.active", 0)
        self.fs.setting("synth.polyphony", 256)
        self._sfids: dict[str, int] = {}
        self.sfid = self.sfid_for("musescore")
        self._set_channel_type = fluidsynth.cfunc(
            "fluid_synth_set_channel_type",
            ctypes.c_int,
            ("synth", ctypes.c_void_p, 1),
            ("chan", ctypes.c_int, 1),
            ("type", ctypes.c_int, 1),
        )
        self._write = fluidsynth.cfunc(
            "fluid_synth_write_float",
            ctypes.c_int,
            ("synth", ctypes.c_void_p, 1),
            ("len", ctypes.c_int, 1),
            ("lout", ctypes.c_void_p, 1),
            ("loff", ctypes.c_int, 1),
            ("lincr", ctypes.c_int, 1),
            ("rout", ctypes.c_void_p, 1),
            ("roff", ctypes.c_int, 1),
            ("rincr", ctypes.c_int, 1),
        )
        self.buf = np.zeros(self.BLOCK * 2, dtype=np.float32)

    def sfid_for(self, font: str) -> int:
        """The loaded SoundFont `font` (a key of SOUNDFONTS)."""
        if font not in self._sfids:
            path = SOUNDFONTS[font]
            sfid = self.fs.sfload(path)
            if sfid < 0:
                raise SystemExit(f"could not load the {font} SoundFont at {path}; see the top of audio/soundtrack.py")
            self._sfids[font] = sfid
        return self._sfids[font]

    def program(self, channel: int, font: str, program: int, drums: bool = False, pan: float = 0.0) -> None:
        """Puts `program` from `font` on `channel`, at `pan`; a drum kit
        makes the channel a drum channel, whichever channel it is."""
        self._set_channel_type(self.fs.synth, channel, 1 if drums else 0)
        self.fs.program_select(channel, self.sfid_for(font), 128 if drums else 0, program)
        self.fs.cc(channel, 10, int(round(64 + 63 * max(-1.0, min(1.0, pan)))))
        self.fs.cc(channel, 91, 0)
        self.fs.cc(channel, 93, 0)

    def block(self) -> np.ndarray:
        self._write(self.fs.synth, self.BLOCK, self.buf.ctypes.data, 0, 2, self.buf.ctypes.data, 1, 2)
        return self.buf.reshape(-1, 2)

    def close(self) -> None:
        self.fs.delete()


def render_stem(piece: Piece, stem: Stem) -> np.ndarray:
    """The stem's parts, `PASSES` times through the loop and a tail, as
    stereo float32 at `SR`."""
    synth = Synth()
    fs = synth.fs
    beat_s = 60.0 / piece.bpm
    block_s = Synth.BLOCK / SR
    loop_blocks = piece.loop_frames // Synth.BLOCK
    tail_blocks = int(6.0 / block_s)
    total = PASSES * loop_blocks + tail_blocks
    # [on block, off block, channel, pitch, velocity] per note and pass.
    notes: list[list[int]] = []
    # (block, kind, channel, pitch, velocity): offs (0) before ons (1).
    events: list[tuple[int, int, int, int, int]] = []
    # A channel per instrument and position: a part's notes fan out by
    # pitch in five steps, and a kit's drums each sit where they would.
    channels: dict[tuple[int, float], int] = {}

    def channel_for(ch: int, part: Part, pitch: int) -> int:
        if part.drums:
            pan = DRUM_PAN.get(pitch, 0.0)
        else:
            slot = max(-2, min(2, round((pitch - 60) / 9)))
            pan = max(-1.0, min(1.0, part.pan + part.spread * slot / 2))
        key = (ch, pan)
        if key not in channels:
            index = len(channels)
            channel = index if index < 9 else index + 1
            assert channel < 16, f"{stem.key}: too many instrument positions"
            synth.program(channel, part.font, part.program, part.drums, pan)
            fs.cc(channel, 11, part.expression)
            channels[key] = channel
        return channels[key]

    for ch, part in enumerate(stem.parts):
        rng = random.Random(zlib.crc32(stem.key.encode()) + ch)
        # One humanised timing and velocity per note, used in every pass,
        # so the take is exactly periodic and the loop seam is silent.
        for note in part.notes:
            channel = channel_for(ch, part, note.pitch)
            beat = note.beat
            # Swing: the off-beat eighth is pushed late.
            if abs(beat % 1.0 - 0.5) < 1e-6 and part.swing != 0.5:
                beat += (part.swing - 0.5) * 2 * 0.5
            start_s = max(0.0, beat * beat_s + rng.gauss(0, part.jitter_s))
            dur_s = max(0.03, note.dur * part.gate * beat_s)
            vel = max(1, min(127, note.vel + rng.randint(-part.vel_jitter, part.vel_jitter)))
            on = int(round(start_s / block_s))
            off = max(on + 1, int(round((start_s + dur_s) / block_s)))
            for pass_no in range(PASSES):
                shift = pass_no * loop_blocks
                notes.append([on + shift, off + shift, channel, note.pitch, vel])
    # A note that is still sounding when the same pitch is struck again on
    # its channel ends there: FluidSynth would otherwise take the later
    # note-off for the newer voice and cut it short.
    notes.sort()
    last_on: dict[tuple[int, int], list[int]] = {}
    for item in notes:
        on, _, channel, pitch, _ = item
        prev = last_on.get((channel, pitch))
        if prev is not None and prev[1] > on:
            prev[1] = on
        last_on[(channel, pitch)] = item
    for on, off, channel, pitch, vel in notes:
        events.append((on, 1, channel, pitch, vel))
        events.append((off, 0, channel, pitch, 0))
    events.sort()
    out = np.zeros((total * Synth.BLOCK, 2), dtype=np.float32)
    i = 0
    for b in range(total):
        while i < len(events) and events[i][0] == b:
            _, kind, channel, pitch, vel = events[i]
            if kind == 1:
                fs.noteon(channel, pitch, vel)
            else:
                fs.noteoff(channel, pitch)
            i += 1
        out[b * Synth.BLOCK : (b + 1) * Synth.BLOCK] = synth.block()
    synth.close()
    return out


def master(audio: np.ndarray, stem: Stem) -> np.ndarray:
    """The stem's EQ and dynamics, then a shared room, as one time-invariant
    chain over the whole two-pass take."""
    board = Pedalboard(list(stem.chain))
    out = board(audio.T, SR).T
    if stem.reverb > 0:
        room = Pedalboard([Reverb(room_size=stem.reverb_size, damping=0.6, wet_level=stem.reverb, dry_level=1.0, width=1.0)])
        out = room(out.T, SR).T
    return out.astype(np.float32)


def cut_loop(audio: np.ndarray, piece: Piece) -> np.ndarray:
    frames = piece.loop_frames
    assert audio.shape[0] >= 3 * frames, "render shorter than three passes"
    return audio[frames : 2 * frames]


def seam_error(audio: np.ndarray, piece: Piece, seconds: float = 2.0) -> float:
    """How far the loop's start is from what follows its end in the take:
    the third pass's opening against the second's, as a share of the
    signal's own level. Near zero means the wrap is inaudible."""
    frames = piece.loop_frames
    w = int(seconds * SR)
    a = audio[frames : frames + w]
    b = audio[2 * frames : 2 * frames + w]
    level = float(np.sqrt(np.mean(a**2))) or 1e-9
    return float(np.sqrt(np.mean((a - b) ** 2))) / level


def set_level(audio: np.ndarray, rms_db: float) -> np.ndarray:
    rms = float(np.sqrt(np.mean(audio**2)))
    if rms <= 0:
        return audio
    gain = 10 ** (rms_db / 20) / rms
    return audio * gain


def encode_ogg(wav: Path, ogg: Path) -> None:
    subprocess.run(
        ["ffmpeg", "-y", "-loglevel", "error", "-i", str(wav), "-c:a", "libvorbis", "-q:a", VORBIS_QUALITY, str(ogg)],
        check=True,
    )


def render_piece(piece: Piece, keep_wavs: bool = False) -> None:
    out_dir = OUT_DIR / piece.id
    out_dir.mkdir(parents=True, exist_ok=True)
    loops: dict[str, np.ndarray] = {}
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = Path(tmp)
        for stem in piece.bed + piece.voices:
            name = stem.key.split("/")[-1]
            audio = master(render_stem(piece, stem), stem)
            seam = seam_error(audio, piece)
            loop = cut_loop(audio, piece)
            loop = set_level(loop, stem.rms_db)
            peak = float(np.abs(loop).max())
            if peak > 0.98:
                loop = loop * (0.98 / peak)
            loops[name] = loop
            wav = tmp_path / f"{name}.wav"
            sf.write(str(wav), loop, SR, subtype="PCM_24")
            ogg = out_dir / f"{name}.ogg"
            encode_ogg(wav, ogg)
            if keep_wavs:
                sf.write(str(WAV_DIR / f"{piece.id}_{name}.wav"), loop, SR, subtype="PCM_16")
            print(f"{ogg.relative_to(ROOT)}: {loop.shape[0]} frames, peak {peak:.3f}, seam error {seam * 100:.2f}%, {ogg.stat().st_size // 1024} KB")
            assert seam < 0.02, f"{stem.key}: the loop does not wrap cleanly"

    mix = sum(loops.values())
    print(
        f"{piece.id}: {piece.bars} bars at {piece.bpm:g}, loop {piece.loop_s:.3f} s; "
        f"full mix peak {float(np.abs(mix).max()):.3f}, rms {20 * np.log10(np.sqrt(np.mean(mix**2))):.1f} dBFS"
    )
    if keep_wavs:
        sf.write(str(WAV_DIR / f"{piece.id}_mix.wav"), np.clip(mix, -1, 1), SR, subtype="PCM_16")


def main(argv: list[str]) -> int:
    keep = "--wav" in argv
    if keep:
        WAV_DIR.mkdir(parents=True, exist_ok=True)
        print(f"stem and mix WAVs go to {WAV_DIR}")
    ids = [a for a in argv if not a.startswith("--")] or list(PIECES)
    for pid in ids:
        if pid not in PIECES:
            print(f"no piece {pid!r}; pieces: {', '.join(PIECES)}", file=sys.stderr)
            return 2
        render_piece(PIECES[pid], keep_wavs=keep)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
