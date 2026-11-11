#!/usr/bin/env python3
"""Designs and renders every interface sound and effect in the game.

Each sound is built here from its parts: resonant bodies struck by short
bursts for clicks, clunks and bells; shaped noise for air, paper, wind and
radio; pulse trains through resonators for diesel engines; and short
musical stings played on sampled instruments through FluidSynth. Each is
then given a room, levelled and written to `assets/sounds/<key>.wav` (or
`.ogg` for the longer stings), where the game compiles it in by key.

Run from the repository root:

    python3 audio/effects.py              # every sound
    python3 audio/effects.py sfx/horn     # one sound, by key

Needs the same tools as soundtrack.py.
"""

from __future__ import annotations

import os
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

import numpy as np
import soundfile as sf
from pedalboard import Compressor, HighpassFilter, HighShelfFilter, LowpassFilter, PeakFilter, Pedalboard, Reverb
from scipy.signal import butter, lfilter, sosfilt

sys.path.insert(0, str(Path(__file__).resolve().parent))
from soundtrack import SR, Synth, n  # noqa: E402

ROOT = Path(__file__).resolve().parent.parent
OUT_DIR = ROOT / "assets" / "sounds"
# Longer sounds go out as Vorbis at this quality; short ones stay WAV.
OGG_FROM_S = 1.3
VORBIS_QUALITY = "5"


# ---------------------------------------------------------------------------
# Building blocks

def seconds(s: float) -> int:
    return int(round(s * SR))


def t(length_s: float) -> np.ndarray:
    return np.arange(seconds(length_s)) / SR


def silence(length_s: float) -> np.ndarray:
    return np.zeros(seconds(length_s), dtype=np.float64)


def sine(freq: float, length_s: float, phase: float = 0.0) -> np.ndarray:
    return np.sin(2 * np.pi * freq * t(length_s) + phase)


def sweep(f0: float, f1: float, length_s: float, curve: float = 1.0) -> np.ndarray:
    """A sine gliding from f0 to f1; curve > 1 moves early, < 1 late."""
    tt = t(length_s)
    k = (tt / tt[-1]) ** curve if len(tt) > 1 else tt
    freq = f0 + (f1 - f0) * k
    phase = 2 * np.pi * np.cumsum(freq) / SR
    return np.sin(phase)


def harmonics(freq: float, length_s: float, amps: list[float], detune: float = 0.0) -> np.ndarray:
    """A tone from its partials: amps[k] is the level of partial k+1."""
    out = np.zeros(seconds(length_s))
    tt = t(length_s)
    for k, a in enumerate(amps, start=1):
        f = freq * k * (1 + detune * k)
        if f < SR / 2 and a:
            out += a * np.sin(2 * np.pi * f * tt)
    return out


def saw(freq: float, length_s: float, partials: int = 40) -> np.ndarray:
    amps = [((-1) ** (k + 1)) / k for k in range(1, partials + 1)]
    return harmonics(freq, length_s, amps) * (2 / np.pi)


def square(freq: float, length_s: float, partials: int = 30) -> np.ndarray:
    amps = [(1 / k if k % 2 else 0.0) for k in range(1, partials + 1)]
    return harmonics(freq, length_s, amps) * (4 / np.pi)


def noise(length_s: float, seed: int = 1) -> np.ndarray:
    return np.random.default_rng(seed).standard_normal(seconds(length_s))


def pink(length_s: float, seed: int = 1) -> np.ndarray:
    """Pink noise by Paul Kellet's filter."""
    w = noise(length_s, seed)
    b0 = b1 = b2 = b3 = b4 = b5 = b6 = 0.0
    out = np.empty_like(w)
    for i, x in enumerate(w):
        b0 = 0.99886 * b0 + x * 0.0555179
        b1 = 0.99332 * b1 + x * 0.0750759
        b2 = 0.96900 * b2 + x * 0.1538520
        b3 = 0.86650 * b3 + x * 0.3104856
        b4 = 0.55000 * b4 + x * 0.5329522
        b5 = -0.7616 * b5 - x * 0.0168980
        out[i] = (b0 + b1 + b2 + b3 + b4 + b5 + b6 + x * 0.5362) * 0.11
        b6 = x * 0.115926
    return out


def brown(length_s: float, seed: int = 1) -> np.ndarray:
    w = noise(length_s, seed)
    out = lfilter([1.0], [1.0, -0.995], w)
    return out / (np.abs(out).max() + 1e-9)


def lowpass(x: np.ndarray, fc: float, order: int = 2) -> np.ndarray:
    sos = butter(order, min(fc, SR / 2 - 100), btype="low", fs=SR, output="sos")
    return sosfilt(sos, x)


def highpass(x: np.ndarray, fc: float, order: int = 2) -> np.ndarray:
    sos = butter(order, max(fc, 10), btype="high", fs=SR, output="sos")
    return sosfilt(sos, x)


def bandpass(x: np.ndarray, lo: float, hi: float, order: int = 2) -> np.ndarray:
    sos = butter(order, [max(lo, 10), min(hi, SR / 2 - 100)], btype="band", fs=SR, output="sos")
    return sosfilt(sos, x)


def moving_lowpass(x: np.ndarray, cutoffs: np.ndarray) -> np.ndarray:
    """A one-pole low-pass whose cutoff changes every sample."""
    a = np.exp(-2 * np.pi * np.clip(cutoffs, 20, SR / 2 - 100) / SR)
    out = np.empty_like(x)
    y = 0.0
    for i in range(len(x)):
        y = a[i] * y + (1 - a[i]) * x[i]
        out[i] = y
    return out


def decay(length_s: float, tau_s: float, attack_s: float = 0.0) -> np.ndarray:
    """An exponential fall from 1 with time constant tau, after a linear
    attack."""
    tt = t(length_s)
    env = np.exp(-tt / max(tau_s, 1e-4))
    if attack_s > 0:
        env *= np.clip(tt / attack_s, 0, 1)
    return env


def adsr(length_s: float, a: float, d: float, s: float, r: float) -> np.ndarray:
    nn = seconds(length_s)
    env = np.zeros(nn)
    ia, idd, ir = seconds(a), seconds(d), seconds(r)
    isus = max(0, nn - ia - idd - ir)
    i = 0
    env[i : i + ia] = np.linspace(0, 1, ia, endpoint=False)
    i += ia
    env[i : i + idd] = np.linspace(1, s, idd, endpoint=False)
    i += idd
    env[i : i + isus] = s
    i += isus
    env[i : i + ir] = np.linspace(s, 0, nn - i)
    return env


def modal(
    length_s: float,
    modes: list[tuple[float, float, float]],
    excite: np.ndarray | None = None,
    seed: int = 1,
) -> np.ndarray:
    """A body ringing at `modes` (frequency, time constant, level), struck
    by `excite` (a short burst; by default one sample)."""
    out = np.zeros(seconds(length_s))
    tt = t(length_s)
    rng = np.random.default_rng(seed)
    for f, tau, a in modes:
        if f >= SR / 2:
            continue
        out += a * np.exp(-tt / tau) * np.sin(2 * np.pi * f * tt + rng.uniform(0, 2 * np.pi))
    if excite is not None and len(excite) > 1:
        out = np.convolve(out, excite / (np.abs(excite).sum() + 1e-9))[: len(out)]
    return out


def burst(length_s: float, seed: int = 1, tau_s: float | None = None) -> np.ndarray:
    """A short noise burst with a fast fall: what strikes a body."""
    x = noise(length_s, seed)
    return x * decay(length_s, tau_s or length_s / 3)


def impulses(length_s: float, rate_hz: float, seed: int = 1, jitter: float = 0.3, amp_jitter: float = 0.3) -> np.ndarray:
    """Clicks at about `rate_hz`, each a little early or late and louder or softer."""
    rng = np.random.default_rng(seed)
    out = np.zeros(seconds(length_s))
    tt = 0.0
    while tt < length_s:
        i = seconds(tt)
        if i < len(out):
            out[i] += 1.0 + rng.uniform(-amp_jitter, amp_jitter)
        tt += (1 / rate_hz) * (1 + rng.uniform(-jitter, jitter))
    return out


def pulse_train(length_s: float, rates: np.ndarray, seed: int = 1, amp_jitter: float = 0.25) -> np.ndarray:
    """One click per cycle of a rate that changes every sample (an engine)."""
    rng = np.random.default_rng(seed)
    phase = np.cumsum(rates) / SR
    out = np.zeros(seconds(length_s))
    ticks = np.flatnonzero(np.diff(np.floor(phase)) > 0)
    for i in ticks:
        out[i] += 1.0 + rng.uniform(-amp_jitter, amp_jitter)
    return out


def place(canvas: np.ndarray, x: np.ndarray, at_s: float, gain: float = 1.0) -> np.ndarray:
    """Adds x into canvas starting at `at_s`, growing the canvas if needed."""
    i = seconds(at_s)
    end = i + len(x)
    if end > len(canvas):
        canvas = np.concatenate([canvas, np.zeros(end - len(canvas))])
    canvas[i:end] += gain * x
    return canvas


def mix(*layers: tuple[np.ndarray, float]) -> np.ndarray:
    length = max(len(x) for x, _ in layers)
    out = np.zeros(length)
    for x, g in layers:
        out[: len(x)] += g * x
    return out


def fade(x: np.ndarray, in_s: float = 0.0005, out_s: float = 0.01) -> np.ndarray:
    x = x.copy()
    i, o = seconds(in_s), seconds(out_s)
    if i > 0:
        x[:i] *= np.linspace(0, 1, i)
    if o > 0:
        x[-o:] *= np.linspace(1, 0, o)
    return x


def soft_clip(x: np.ndarray, drive: float = 1.0) -> np.ndarray:
    return np.tanh(x * drive) / np.tanh(drive)


def room(x: np.ndarray, wet: float = 0.12, size: float = 0.3, damping: float = 0.6) -> np.ndarray:
    board = Pedalboard([Reverb(room_size=size, damping=damping, wet_level=wet, dry_level=1.0, width=0.5)])
    stereo = np.stack([x, x]).astype(np.float32)
    out = board(stereo, SR)
    return out.mean(axis=0).astype(np.float64)


def eq(x: np.ndarray, *plugins) -> np.ndarray:
    board = Pedalboard(list(plugins))
    return board(x.astype(np.float32).reshape(1, -1), SR)[0].astype(np.float64)


def synth_notes(notes: list[tuple[float, int, int, int, float]], length_s: float, gain: float = 1.0) -> np.ndarray:
    """Sampled instruments: (start s, program, pitch, velocity, duration s)
    rendered mono."""
    synth = Synth()
    fs = synth.fs
    programs = sorted({p for _, p, _, _, _ in notes})
    channel_of = {}
    for ch, p in enumerate(programs):
        channel = ch if ch < 9 else ch + 1
        fs.program_select(channel, synth.sfid, 0, p)
        fs.cc(channel, 91, 0)
        fs.cc(channel, 93, 0)
        channel_of[p] = channel
    block_s = Synth.BLOCK / SR
    total = int(np.ceil(length_s / block_s))
    events = []
    for start, prog, pitch, vel, dur in notes:
        on = int(round(start / block_s))
        off = max(on + 1, int(round((start + dur) / block_s)))
        events.append((on, 1, channel_of[prog], pitch, vel))
        events.append((off, 0, channel_of[prog], pitch, 0))
    events.sort()
    out = np.zeros(total * Synth.BLOCK)
    i = 0
    for b in range(total):
        while i < len(events) and events[i][0] == b:
            _, kind, ch, pitch, vel = events[i]
            if kind:
                fs.noteon(ch, pitch, vel)
            else:
                fs.noteoff(ch, pitch)
            i += 1
        out[b * Synth.BLOCK : (b + 1) * Synth.BLOCK] = synth.block().mean(axis=1)
    synth.close()
    return out[: seconds(length_s)] * gain


# ---------------------------------------------------------------------------
# The sounds


@dataclass
class Sound:
    key: str
    make: Callable[[], np.ndarray]
    # The loudest 50 ms, as RMS in dBFS, after the room: the mix between sounds.
    level_db: float
    # The room it sits in: wet share and size.
    wet: float = 0.08
    size: float = 0.25
    peak_db: float = -1.0


def ui_cursor() -> np.ndarray:
    tick = modal(0.06, [(1900, 0.010, 1.0), (3100, 0.005, 0.5), (760, 0.018, 0.6)], burst(0.0015, 3))
    thump = lowpass(modal(0.06, [(180, 0.012, 1.0)]), 400)
    return fade(mix((tick, 1.0), (thump, 0.3)))


def ui_type() -> np.ndarray:
    tick = modal(0.05, [(2400, 0.006, 1.0), (3900, 0.003, 0.4), (950, 0.012, 0.5)], burst(0.0012, 5))
    return fade(tick)


def ui_activate() -> np.ndarray:
    press = modal(0.08, [(2600, 0.006, 0.8), (4100, 0.003, 0.4), (430, 0.035, 1.0), (150, 0.03, 0.7)], burst(0.002, 7))
    release = modal(0.06, [(3000, 0.004, 0.5), (520, 0.02, 0.6)], burst(0.0015, 8))
    out = place(press, release, 0.055, 0.45)
    return fade(out)


def ui_back() -> np.ndarray:
    press = modal(0.08, [(2100, 0.005, 0.6), (360, 0.03, 1.0), (140, 0.03, 0.6)], burst(0.002, 9))
    return fade(press)


def ui_error() -> np.ndarray:
    thud = lowpass(modal(0.16, [(112, 0.06, 1.0), (168, 0.04, 0.6), (330, 0.015, 0.3)], burst(0.004, 11)), 500)
    out = place(thud.copy(), thud, 0.13, 0.85)
    return fade(out, out_s=0.03)


def ui_success() -> np.ndarray:
    return synth_notes([(0.0, 12, n("D5"), 92, 0.4), (0.1, 12, n("A5"), 96, 0.5)], 0.75, gain=1.5)


def ui_notify() -> np.ndarray:
    return synth_notes([(0.0, 8, n("A5"), 80, 0.5)], 0.7, gain=1.5)


def desk_phone_warble(length_s: float, seed: int) -> np.ndarray:
    """A modern desk phone's electronic ring: two tones traded 20 times a
    second through a small speaker."""
    tt = t(length_s)
    gate = (np.sin(2 * np.pi * 20 * tt) > 0).astype(float)
    tone = sine(1150, length_s) * gate + sine(1480, length_s) * (1 - gate)
    tone = harmonics(1150, length_s, [1.0, 0.25, 0.1]) * gate + harmonics(1480, length_s, [1.0, 0.25, 0.1]) * (1 - gate)
    speaker = bandpass(soft_clip(tone, 1.6), 600, 3800, order=2)
    env = adsr(length_s, 0.006, 0.0, 1.0, 0.02)
    return speaker * env


def sfx_phone_ring() -> np.ndarray:
    ring = desk_phone_warble(0.42, 1)
    out = silence(1.3)
    out = place(out, ring, 0.0)
    out = place(out, ring, 0.6)
    return fade(out, out_s=0.02)


def sfx_phone_pickup() -> np.ndarray:
    lift = modal(0.12, [(900, 0.008, 0.7), (1400, 0.005, 0.5), (260, 0.03, 1.0), (120, 0.025, 0.5)], burst(0.003, 13))
    line = modal(0.05, [(2200, 0.004, 0.5), (700, 0.01, 0.4)], burst(0.001, 14))
    out = place(lift, line, 0.11, 0.5)
    return fade(out)


def sfx_phone_hangup() -> np.ndarray:
    clunk = modal(0.2, [(600, 0.012, 0.6), (1100, 0.006, 0.4), (185, 0.05, 1.0), (95, 0.04, 0.7)], burst(0.004, 15))
    rattle = modal(0.1, [(1300, 0.005, 0.5), (420, 0.015, 0.5)], burst(0.002, 16))
    out = place(clunk, rattle, 0.07, 0.4)
    return fade(lowpass(out, 3500), out_s=0.03)


def squelch(length_s: float, seed: int, tau: float = 0.09) -> np.ndarray:
    hiss = bandpass(noise(length_s, seed), 420, 2600, order=3) * decay(length_s, tau, 0.002)
    crackle = bandpass(impulses(length_s, 180, seed + 1, 0.8, 0.6), 800, 3000) * decay(length_s, tau * 0.7)
    return soft_clip(mix((hiss, 1.0), (crackle, 0.35)), 1.4)


def sfx_radio() -> np.ndarray:
    key = modal(0.03, [(1800, 0.003, 1.0), (3200, 0.002, 0.5)], burst(0.001, 17))
    out = silence(0.8)
    out = place(out, key, 0.0, 0.5)
    out = place(out, squelch(0.3, 18), 0.012)
    out = place(out, key, 0.42, 0.35)
    out = place(out, squelch(0.3, 19, 0.06), 0.43, 0.6)
    return fade(out, out_s=0.05)


def diesel(length_s: float, rate_hz: np.ndarray, seed: int, body_hz: float = 70.0, gain_env: np.ndarray | None = None) -> np.ndarray:
    """A diesel engine: a click a firing stroke through the block and the
    cab, valve clatter over it, and a rumble underneath."""
    ticks = pulse_train(length_s, rate_hz, seed)
    block = modal(0.05, [(body_hz, 0.02, 1.0), (body_hz * 1.9, 0.012, 0.5), (body_hz * 3.3, 0.006, 0.3)])
    thump = np.convolve(ticks, block)[: len(ticks)]
    thump = lowpass(thump, 320, order=2)
    clatter = bandpass(np.convolve(ticks, burst(0.006, seed + 2)), 1500, 4500, order=2)[: len(ticks)]
    rumble = lowpass(brown(length_s, seed + 3), 110, order=2)
    out = mix((thump, 1.0), (clatter, 0.1), (rumble, 0.6))
    if gain_env is not None:
        out = out * gain_env
    return out


def sfx_truck_idle() -> np.ndarray:
    length = 2.0
    tt = t(length)
    rate = 30 + 1.5 * np.sin(2 * np.pi * 0.7 * tt)
    env = adsr(length, 0.08, 0.0, 1.0, 0.35)
    return diesel(length, rate, 21, gain_env=env)


def air_release(length_s: float, seed: int, tau: float = 0.22) -> np.ndarray:
    tt = t(length_s)
    cut = 9000 * np.exp(-tt / (tau * 1.6)) + 1200
    hiss = moving_lowpass(highpass(noise(length_s, seed), 900, order=2), cut)
    valve = modal(0.03, [(2400, 0.004, 1.0), (900, 0.01, 0.6)], burst(0.002, seed + 1))
    out = hiss * decay(length_s, tau, 0.004)
    return place(out, valve, 0.0, 0.8)


def sfx_air_brakes() -> np.ndarray:
    return fade(air_release(1.0, 23), out_s=0.1)


def sfx_truck_depart() -> np.ndarray:
    length = 2.6
    tt = t(length)
    rate = np.where(tt < 0.4, 30, 30 + 42 * np.clip((tt - 0.4) / 1.4, 0, 1))
    # A gear change at 1.3 s: the revs drop and climb again.
    rate = np.where((tt > 1.3) & (tt < 1.45), rate * 0.72, rate)
    env = adsr(length, 0.05, 0.0, 1.0, 0.9) * np.where(tt > 1.6, np.exp(-(tt - 1.6) / 0.7), 1.0)
    engine = diesel(length, rate, 25, gain_env=env)
    engine = moving_lowpass(engine, 3500 * np.exp(-np.clip(tt - 1.4, 0, None) / 0.5) + 300)
    out = place(engine, air_release(0.6, 26, 0.15), 0.0, 0.7)
    return fade(out, out_s=0.2)


def sfx_horn() -> np.ndarray:
    length = 1.3
    tt = t(length)
    env = adsr(length, 0.03, 0.0, 1.0, 0.22)
    out = np.zeros(seconds(length))
    for f, g in [(311.1, 1.0), (370.0, 0.9), (466.2, 0.5)]:
        bend = 1 + 0.03 * np.exp(-tt / 0.04) + 0.003 * np.sin(2 * np.pi * 5.5 * tt)
        phase = 2 * np.pi * np.cumsum(f * bend) / SR
        tone = sum(((-1) ** (k + 1)) / k * np.sin(k * phase) for k in range(1, 18))
        out += g * tone
    out = eq(out, PeakFilter(900, 6.0, 1.2), PeakFilter(2100, 4.0, 1.5), HighShelfFilter(5000, -6.0))
    out = soft_clip(out / (np.abs(out).max() + 1e-9) * 1.4, 1.2) * env
    return fade(out, out_s=0.05)


def paper_rustle(length_s: float, seed: int, lo: float = 700, hi: float = 6500) -> np.ndarray:
    tt = t(length_s)
    sweepy = bandpass(noise(length_s, seed), lo, hi, order=2)
    motion = 0.5 + 0.5 * np.sin(2 * np.pi * (3.0 + 2 * np.sin(2 * np.pi * 0.7 * tt)) * tt + 1.0)
    crackle = bandpass(impulses(length_s, 260, seed + 1, 0.9, 0.7), 1200, 7000)
    return mix((sweepy * motion, 1.0), (crackle, 0.5))


def sfx_paper() -> np.ndarray:
    length = 0.55
    env = adsr(length, 0.02, 0.1, 0.55, 0.2)
    out = paper_rustle(length, 27) * env
    flick = bandpass(burst(0.03, 28), 1500, 7000) * 1.4
    out = place(out, flick, 0.22)
    return fade(out, out_s=0.05)


def stamp(seed: int) -> np.ndarray:
    thud = lowpass(modal(0.18, [(95, 0.04, 1.0), (150, 0.025, 0.6), (225, 0.03, 0.5)], burst(0.004, seed)), 600)
    slap = bandpass(burst(0.02, seed + 1), 900, 4500) * 0.9
    out = mix((thud, 1.0), (slap, 0.5))
    return fade(out, out_s=0.03)


def sfx_stamp() -> np.ndarray:
    return stamp(31)


def sfx_delivered() -> np.ndarray:
    out = place(silence(1.2), stamp(33), 0.0)
    chime = synth_notes([(0.0, 12, n("D5"), 88, 0.35), (0.11, 12, n("G5"), 94, 0.5)], 1.05, gain=1.3)
    return place(out, chime, 0.12)


def sfx_late() -> np.ndarray:
    out = place(silence(1.2), stamp(35), 0.0)
    chime = synth_notes([(0.0, 12, n("E4"), 80, 0.4), (0.14, 12, n("C4"), 84, 0.55)], 1.05, gain=1.3)
    return place(out, lowpass(chime, 2500), 0.12)


def sfx_clock() -> np.ndarray:
    tick = modal(0.06, [(2600, 0.005, 1.0), (4200, 0.003, 0.5), (640, 0.02, 0.4)], burst(0.0012, 37))
    tock = modal(0.06, [(2100, 0.005, 0.9), (3400, 0.003, 0.4), (520, 0.022, 0.5)], burst(0.0012, 38))
    out = silence(0.95)
    out = place(out, tick, 0.0)
    out = place(out, tock, 0.3, 0.85)
    out = place(out, tick, 0.6, 0.95)
    return fade(out, out_s=0.05)


def sfx_cash() -> np.ndarray:
    bell = modal(
        0.7,
        [(2210, 0.35, 1.0), (3290, 0.22, 0.55), (4720, 0.14, 0.3), (6100, 0.08, 0.15), (1100, 0.1, 0.2)],
        burst(0.0015, 39),
    )
    drawer = bandpass(noise(0.2, 40), 300, 1600, order=2) * adsr(0.2, 0.01, 0.0, 1.0, 0.08)
    stop = lowpass(modal(0.12, [(140, 0.03, 1.0), (260, 0.02, 0.5), (700, 0.008, 0.4)], burst(0.003, 41)), 1200)
    out = place(bell, drawer, 0.05, 0.35)
    out = place(out, stop, 0.24, 0.9)
    return fade(out, out_s=0.05)


def sfx_bill() -> np.ndarray:
    out = place(silence(0.65), paper_rustle(0.35, 43, 500, 5000) * adsr(0.35, 0.02, 0.05, 0.7, 0.15), 0.0, 0.8)
    thud = lowpass(modal(0.2, [(105, 0.05, 1.0), (160, 0.03, 0.5)], burst(0.004, 44)), 500)
    out = place(out, thud, 0.36, 0.9)
    return fade(out, out_s=0.05)


def sfx_hire() -> np.ndarray:
    length = 0.55
    tt = t(length)
    # Pen strokes at an uneven 9 to 13 a second.
    rate = 11 + 2 * np.sin(2 * np.pi * 1.7 * tt + 0.4) * np.sin(2 * np.pi * 0.6 * tt)
    stroke_phase = 2 * np.pi * np.cumsum(rate) / SR
    strokes = 0.5 + 0.5 * np.sign(np.sin(stroke_phase + 0.3)) * np.sin(stroke_phase) ** 2
    pen = bandpass(noise(length, 45), 1800, 7500, order=2) * strokes * adsr(length, 0.02, 0.0, 1.0, 0.08)
    out = place(silence(1.0), pen, 0.0, 0.7)
    out = place(out, stamp(46), 0.62)
    return fade(out, out_s=0.05)


def sfx_door() -> np.ndarray:
    slam = lowpass(modal(0.4, [(88, 0.09, 1.0), (140, 0.06, 0.7), (210, 0.04, 0.4), (330, 0.02, 0.3)], burst(0.006, 47)), 700)
    frame = bandpass(burst(0.03, 48), 500, 3000) * 0.8
    latch = modal(0.08, [(2800, 0.004, 0.8), (1500, 0.006, 0.6), (600, 0.012, 0.4)], burst(0.0015, 49))
    out = mix((slam, 1.0), (frame, 0.6))
    out = place(out, latch, 0.045, 0.45)
    return fade(out, out_s=0.08)


def beep(freq: float, length_s: float, seed: int = 1) -> np.ndarray:
    tone = harmonics(freq, length_s, [1.0, 0.18, 0.06])
    tone = bandpass(soft_clip(tone, 1.3), 500, 5000)
    return tone * adsr(length_s, 0.004, 0.0, 1.0, 0.012)


def sfx_alert() -> np.ndarray:
    out = silence(0.95)
    for i, at in enumerate([0.0, 0.3, 0.6]):
        out = place(out, beep(1046.5, 0.09), at)
        out = place(out, beep(1318.5, 0.09), at + 0.1)
    return fade(out, out_s=0.03)


def sfx_hours() -> np.ndarray:
    def chime(f: float) -> np.ndarray:
        return modal(1.1, [(f, 0.25, 1.0), (f * 2.76, 0.12, 0.25), (f * 5.4, 0.06, 0.08)], lowpass(burst(0.002, 51), 4000))

    out = silence(1.6)
    for at, f in [(0.0, 1318.5), (0.22, 1046.5), (0.44, 880.0)]:
        out = place(out, chime(f), at)
    return fade(lowpass(out, 6000), out_s=0.05)


def sfx_breakdown() -> np.ndarray:
    length = 2.4
    tt = t(length)
    rate = 30 * np.clip(1 - (tt - 0.5) / 1.4, 0.35, 1.0)
    rng = np.random.default_rng(53)
    misfire = np.ones(len(tt))
    for at in [0.55, 0.9, 1.15, 1.5, 1.7]:
        i, j = seconds(at), seconds(at + rng.uniform(0.05, 0.12))
        misfire[i:j] = 0.15
    env = adsr(length, 0.05, 0.0, 1.0, 0.2) * misfire * np.where(tt > 1.75, np.exp(-(tt - 1.75) / 0.18), 1.0)
    engine = diesel(length, rate, 54, gain_env=env)
    out = place(engine, air_release(0.7, 55, 0.3), 1.85, 0.5)
    clank = modal(0.3, [(1800, 0.03, 1.0), (2900, 0.02, 0.6), (760, 0.05, 0.5), (430, 0.06, 0.4)], burst(0.003, 56))
    out = place(out, clank, 1.95, 0.5)
    return fade(out, out_s=0.1)


def sfx_weather() -> np.ndarray:
    length = 2.8
    tt = t(length)
    gust = 0.35 + 0.65 * np.clip(np.sin(2 * np.pi * 0.35 * tt - 0.9), 0, 1) ** 1.5
    centre = 350 + 500 * gust
    wind = moving_lowpass(highpass(pink(length, 57), 150), centre * 2.2)
    wind = highpass(wind, centre.mean() * 0.4) * gust
    thunder = lowpass(brown(length, 58), 160, order=3)
    thunder_env = np.where(tt > 0.9, np.exp(-(tt - 0.9) / 0.75) * np.clip((tt - 0.9) / 0.08, 0, 1), 0.0)
    rumble_mod = 0.7 + 0.3 * np.sin(2 * np.pi * 7.5 * tt) * np.sin(2 * np.pi * 2.1 * tt)
    out = mix((wind, 0.9), (thunder * thunder_env * rumble_mod, 2.2))
    return fade(out * adsr(length, 0.1, 0.0, 1.0, 0.6), out_s=0.3)


def sfx_dock() -> np.ndarray:
    length = 1.7
    out = silence(length)
    for at in [0.0, 0.5, 1.0]:
        b = beep(1000, 0.17, 1)
        out = place(out, lowpass(b, 2200), at, 0.6)
    door = lowpass(brown(length, 59), 90) * adsr(length, 0.4, 0.3, 0.5, 0.6)
    out = mix((out, 1.0), (door, 0.35))
    return fade(out, out_s=0.1)


def sfx_inspection() -> np.ndarray:
    length = 1.0
    tt = t(length)
    rise = 650 + 950 * np.clip(tt / 0.38, 0, 1) ** 0.8
    rise = np.where(tt > 0.38, 1600 - 250 * np.clip((tt - 0.38) / 0.25, 0, 1), rise)
    phase = 2 * np.pi * np.cumsum(rise) / SR
    tone = np.sin(phase) + 0.35 * np.sin(2 * phase) + 0.12 * np.sin(3 * phase)
    env = adsr(length, 0.01, 0.0, 1.0, 0.0)
    env = env * np.where(tt > 0.55, np.exp(-(tt - 0.55) / 0.12), 1.0)
    out = bandpass(soft_clip(tone * env, 1.5), 400, 5000)
    return fade(out, out_s=0.05)


def sfx_citation() -> np.ndarray:
    length = 0.9
    head = bandpass(impulses(0.42, 62, 63, 0.05, 0.3), 900, 3200) * adsr(0.42, 0.01, 0.0, 1.0, 0.03)
    head = np.convolve(head, burst(0.004, 64))[: seconds(0.42)]
    motor = bandpass(square(124, 0.42, 20), 300, 1500) * 0.25 * adsr(0.42, 0.01, 0.0, 1.0, 0.03)
    out = place(silence(length), mix((head, 1.0), (motor, 0.5)), 0.0)
    tear = paper_rustle(0.22, 65, 1200, 8000) * adsr(0.22, 0.005, 0.05, 0.6, 0.1)
    out = place(out, tear * 1.3, 0.5)
    return fade(out, out_s=0.05)


def sfx_out_of_service() -> np.ndarray:
    out = place(silence(1.4), air_release(0.9, 67, 0.25), 0.0, 1.0)
    clunk = lowpass(modal(0.3, [(120, 0.06, 1.0), (190, 0.04, 0.6), (900, 0.01, 0.5), (1700, 0.006, 0.3)], burst(0.005, 68)), 2500)
    out = place(out, clunk, 0.55, 0.9)
    out = place(out, clunk, 0.78, 0.7)
    return fade(out, out_s=0.1)


def sfx_day_start() -> np.ndarray:
    rhodes = 4
    vibes = 11
    notes = [
        (0.0, rhodes, n("G2"), 70, 2.4),
        (0.0, rhodes, n("D3"), 68, 2.4),
        (0.03, rhodes, n("B3"), 72, 2.4),
        (0.06, rhodes, n("F#4"), 74, 2.4),
        (0.09, rhodes, n("A4"), 76, 2.4),
        (0.3, vibes, n("D5"), 80, 1.2),
        (0.5, vibes, n("G5"), 84, 1.2),
        (0.7, vibes, n("B5"), 88, 1.8),
    ]
    return synth_notes(notes, 2.8, gain=1.2)


def sfx_day_close() -> np.ndarray:
    rhodes = 4
    strings = 49
    notes = [
        (0.0, rhodes, n("E3"), 66, 1.1),
        (0.03, rhodes, n("G3"), 66, 1.1),
        (0.06, rhodes, n("D4"), 68, 1.1),
        (0.09, rhodes, n("F#4"), 70, 1.1),
        (1.2, rhodes, n("G2"), 64, 2.2),
        (1.23, rhodes, n("D3"), 62, 2.2),
        (1.26, rhodes, n("B3"), 64, 2.2),
        (1.29, rhodes, n("F#4"), 62, 2.2),
        (0.0, strings, n("E4"), 40, 1.1),
        (1.2, strings, n("D4"), 42, 2.2),
        (1.2, strings, n("G4"), 40, 2.2),
    ]
    return synth_notes(notes, 3.6, gain=1.3)


def sfx_goal() -> np.ndarray:
    rhodes = 4
    vibes = 11
    marimba = 12
    notes = [
        (0.0, rhodes, n("G3"), 64, 1.2),
        (0.0, rhodes, n("B3"), 64, 1.2),
        (0.0, rhodes, n("D4"), 64, 1.2),
        (0.0, marimba, n("G4"), 82, 0.3),
        (0.1, marimba, n("B4"), 84, 0.3),
        (0.2, marimba, n("D5"), 86, 0.3),
        (0.3, vibes, n("G5"), 92, 1.0),
    ]
    return synth_notes(notes, 1.5, gain=1.3)


def sfx_week_close() -> np.ndarray:
    guitar = 25
    vibes = 11
    bass = 32
    notes = []
    for at, names in [(0.0, ["A2", "E3", "A3", "C#4", "E4"]), (0.9, ["D3", "A3", "D4", "F#4"]), (1.8, ["A2", "E3", "A3", "C#4", "E4"])]:
        for i, name in enumerate(names):
            notes.append((at + i * 0.018, guitar, n(name), 74 - i * 2, 1.6 if at < 1.8 else 2.4))
    notes += [(0.0, bass, n("A1"), 80, 0.9), (0.9, bass, n("D2"), 76, 0.9), (1.8, bass, n("A1"), 80, 2.2)]
    notes += [(1.0, vibes, n("F#5"), 70, 0.8), (1.4, vibes, n("E5"), 72, 0.8), (1.8, vibes, n("C#5"), 76, 2.0)]
    return synth_notes(notes, 4.2, gain=1.1)


def sfx_lose() -> np.ndarray:
    piano = 0
    strings = 49
    notes = [
        (0.0, piano, n("E2"), 78, 3.5),
        (0.0, piano, n("B2"), 70, 3.5),
        (0.05, piano, n("G3"), 66, 3.5),
        (0.05, piano, n("F#4"), 60, 3.5),
        (0.2, strings, n("E3"), 48, 3.4),
        (0.2, strings, n("B3"), 46, 3.4),
        (0.2, strings, n("G4"), 44, 3.4),
    ]
    sting = synth_notes(notes, 4.2, gain=1.2)
    thud = lowpass(modal(0.6, [(60, 0.15, 1.0), (92, 0.1, 0.5)], burst(0.008, 71)), 300)
    return place(sting, thud, 2.6, 0.9)


SOUNDS: list[Sound] = [
    Sound("ui/cursor", ui_cursor, -24.0, wet=0.03, size=0.15),
    Sound("ui/type", ui_type, -26.0, wet=0.02, size=0.15),
    Sound("ui/activate", ui_activate, -22.0, wet=0.03, size=0.15),
    Sound("ui/back", ui_back, -23.0, wet=0.03, size=0.15),
    Sound("ui/error", ui_error, -20.0, wet=0.05, size=0.2),
    Sound("ui/success", ui_success, -19.0, wet=0.15, size=0.35),
    Sound("ui/notify", ui_notify, -20.0, wet=0.18, size=0.4),
    Sound("sfx/phone_ring", sfx_phone_ring, -17.0, wet=0.1, size=0.3),
    Sound("sfx/phone_pickup", sfx_phone_pickup, -19.0, wet=0.06, size=0.2),
    Sound("sfx/phone_hangup", sfx_phone_hangup, -18.0, wet=0.06, size=0.2),
    Sound("sfx/radio", sfx_radio, -17.0, wet=0.04, size=0.2),
    Sound("sfx/truck_idle", sfx_truck_idle, -19.0, wet=0.1, size=0.5),
    Sound("sfx/air_brakes", sfx_air_brakes, -17.0, wet=0.12, size=0.5),
    Sound("sfx/truck_depart", sfx_truck_depart, -18.0, wet=0.12, size=0.55),
    Sound("sfx/horn", sfx_horn, -14.0, wet=0.2, size=0.7),
    Sound("sfx/paper", sfx_paper, -21.0, wet=0.05, size=0.2),
    Sound("sfx/stamp", sfx_stamp, -17.0, wet=0.07, size=0.25),
    Sound("sfx/delivered", sfx_delivered, -17.0, wet=0.1, size=0.3),
    Sound("sfx/late", sfx_late, -18.0, wet=0.1, size=0.3),
    Sound("sfx/clock", sfx_clock, -22.0, wet=0.06, size=0.25),
    Sound("sfx/cash", sfx_cash, -17.0, wet=0.1, size=0.3),
    Sound("sfx/bill", sfx_bill, -19.0, wet=0.06, size=0.25),
    Sound("sfx/hire", sfx_hire, -19.0, wet=0.06, size=0.25),
    Sound("sfx/door", sfx_door, -15.0, wet=0.12, size=0.4),
    Sound("sfx/alert", sfx_alert, -19.0, wet=0.05, size=0.2),
    Sound("sfx/hours", sfx_hours, -19.0, wet=0.1, size=0.3),
    Sound("sfx/breakdown", sfx_breakdown, -18.0, wet=0.12, size=0.55),
    Sound("sfx/weather", sfx_weather, -17.0, wet=0.15, size=0.8),
    Sound("sfx/dock", sfx_dock, -22.0, wet=0.35, size=0.85),
    Sound("sfx/inspection", sfx_inspection, -17.0, wet=0.18, size=0.7),
    Sound("sfx/citation", sfx_citation, -20.0, wet=0.05, size=0.2),
    Sound("sfx/out_of_service", sfx_out_of_service, -17.0, wet=0.12, size=0.5),
    Sound("sfx/day_start", sfx_day_start, -16.0, wet=0.22, size=0.5),
    Sound("sfx/day_close", sfx_day_close, -17.0, wet=0.25, size=0.6),
    Sound("sfx/goal", sfx_goal, -16.0, wet=0.2, size=0.45),
    Sound("sfx/week_close", sfx_week_close, -16.0, wet=0.2, size=0.5),
    Sound("sfx/lose", sfx_lose, -16.0, wet=0.3, size=0.8),
]


def loudest(x: np.ndarray, window_s: float = 0.05) -> float:
    """The RMS of the loudest `window_s` of the sound: what it sounds as
    loud as, which a whole-file RMS understates for a click with a tail."""
    w = max(1, seconds(window_s))
    if len(x) <= w:
        return float(np.sqrt(np.mean(x**2))) or 1e-9
    power = np.convolve(x**2, np.ones(w) / w, mode="valid")
    return float(np.sqrt(power.max())) or 1e-9


def finish(sound: Sound, x: np.ndarray) -> np.ndarray:
    """A room, a trim of DC and rumble, the level, and a ceiling."""
    x = x - x.mean()
    x = highpass(x, 25, order=2)
    if sound.wet > 0:
        x = room(x, sound.wet, sound.size)
    x = eq(x, HighpassFilter(25))
    x = x * (10 ** (sound.level_db / 20) / loudest(x))
    ceiling = 10 ** (sound.peak_db / 20)
    peak = float(np.abs(x).max())
    if peak > ceiling:
        x = x * (ceiling / peak)
    # Nothing ends mid-wave.
    x = fade(x, in_s=0.0, out_s=min(0.02, len(x) / SR / 4))
    return x


def write(sound: Sound, x: np.ndarray) -> Path:
    path = OUT_DIR / sound.key
    path.parent.mkdir(parents=True, exist_ok=True)
    length_s = len(x) / SR
    if length_s >= OGG_FROM_S:
        out = path.with_suffix(".ogg")
        with tempfile.TemporaryDirectory() as tmp:
            wav = Path(tmp) / "s.wav"
            sf.write(str(wav), x.astype(np.float32), SR, subtype="PCM_24")
            subprocess.run(
                ["ffmpeg", "-y", "-loglevel", "error", "-i", str(wav), "-c:a", "libvorbis", "-q:a", VORBIS_QUALITY, str(out)],
                check=True,
            )
        path.with_suffix(".wav").unlink(missing_ok=True)
    else:
        out = path.with_suffix(".wav")
        sf.write(str(out), x.astype(np.float32), SR, subtype="PCM_16")
        path.with_suffix(".ogg").unlink(missing_ok=True)
    return out


def main(argv: list[str]) -> int:
    wanted = set(argv)
    chosen = [s for s in SOUNDS if not wanted or s.key in wanted]
    if wanted and len(chosen) != len(wanted):
        missing = wanted - {s.key for s in chosen}
        print(f"no such sound: {', '.join(sorted(missing))}", file=sys.stderr)
        return 2
    for sound in chosen:
        x = finish(sound, sound.make())
        out = write(sound, x)
        print(f"{out.relative_to(ROOT)}: {len(x) / SR:.2f} s, peak {20 * np.log10(np.abs(x).max() + 1e-9):.1f} dBFS, {out.stat().st_size // 1024} KB")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
