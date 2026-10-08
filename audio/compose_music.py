"""Write audio/music.json, the genny batch for Dispatch's endless desk music.

Every segment is 8 bars at the same tempo and key, so the game can chain
them in any order. Deterministic: the same script writes the same batch.

    python audio/compose_music.py
    genny render audio/music.json --out-dir . --strict
    python audio/encode_music.py
"""

import json
import random
from pathlib import Path

BPM = 84
BEATS = 32

CHORDS = {
    "D": ["D3", "A3", "C#4", "F#4"],
    "Bm": ["B2", "F#3", "A3", "D4"],
    "G": ["G2", "D3", "F#3", "B3"],
    "A": ["A2", "E3", "G3", "C#4"],
    "Em": ["E3", "G3", "B3", "D4"],
    "F#m": ["F#3", "A3", "C#4", "E4"],
}
BASS = {"D": ("D2", "A2"), "Bm": ("B1", "F#2"), "G": ("G1", "D2"), "A": ("A1", "E2"),
        "Em": ("E2", "B2"), "F#m": ("F#2", "C#3")}
PENTA = ["A4", "B4", "D5", "E5", "F#5", "A5", "B5"]
CHORD_TONES = {"D": {"D", "F#", "A"}, "Bm": {"B", "D", "F#"}, "G": {"G", "B", "D"},
               "A": {"A", "C#", "E"}, "Em": {"E", "G", "B"}, "F#m": {"F#", "A", "C#"}}

PROGRESSIONS = [
    ["D", "Bm", "G", "A", "D", "Bm", "G", "A"],
    ["G", "D", "Em", "A", "G", "D", "A", "D"],
    ["Bm", "G", "D", "A", "Bm", "G", "Em", "A"],
    ["Em", "A", "D", "Bm", "Em", "A", "D", "D"],
    ["G", "A", "F#m", "Bm", "G", "A", "D", "D"],
    ["D", "G", "D", "A", "Bm", "G", "A", "D"],
]

RHYTHMS = [
    [1.5, 0.5, 2.0],
    [-1.0, 0.5, 0.5, 2.0],
    [1.0, 1.0, 1.0, 1.0],
    [-2.0, 1.0, 1.0],
    [0.5, 0.5, 1.0, 2.0],
    [-1.5, 0.5, 1.0, 1.0],
]


def melody(prog, rng, lo=0, hi=len(PENTA) - 1):
    idx = rng.randint(lo + 1, hi - 1)
    bars = []
    for chord in prog:
        tokens = []
        for dur in rng.choice(RHYTHMS):
            if dur < 0:
                tokens.append(f"-:{-dur:g}")
                continue
            idx = max(lo, min(hi, idx + rng.choice([-2, -1, -1, 0, 1, 1, 2])))
            note = PENTA[idx]
            if dur >= 2 and note[:-1] not in CHORD_TONES[chord]:
                for step in (1, -1, 2, -2):
                    j = idx + step
                    if lo <= j <= hi and PENTA[j][:-1] in CHORD_TONES[chord]:
                        idx, note = j, PENTA[j]
                        break
            tokens.append(f"{note}:{dur:g}")
        bars.append(" ".join(tokens))
    return " | ".join(bars)


def chords(prog, beats=4):
    return " | ".join(f"[{','.join(CHORDS[c])}]:{beats}" for c in prog)


def walking_bass(prog):
    return " | ".join(f"{BASS[c][0]}:1.5 {BASS[c][0]}:.5 {BASS[c][1]}:1 {BASS[c][0]}:1" for c in prog)


def roots(prog):
    return " | ".join(f"{BASS[c][0]}:2 {BASS[c][1]}:2" for c in prog)


def arpeggio(prog):
    out = []
    for c in prog:
        v = CHORDS[c]
        out.append(" ".join(f"{n}:.5" for n in [v[0], v[2], v[3], v[1], v[2], v[3], v[1], v[2]]))
    return " | ".join(out)


def lofi_drums(gain=1.0):
    return [
        {"type": "pattern", "kind": "kick", "steps": "X-----x---X-----", "bars": 8,
         "gain": 0.55 * gain, "swing": 0.16, "params": {"punch": 0.35, "decay": 0.3}},
        {"type": "pattern", "kind": "snap", "steps": "----x-------x---", "bars": 8,
         "gain": 0.45 * gain, "swing": 0.16},
        {"type": "pattern", "kind": "brush", "steps": "----x-------x---", "bars": 8,
         "gain": 0.3 * gain, "swing": 0.16},
        {"type": "pattern", "kind": "hihat", "steps": "x-x-x-x-x-x-x-xo", "bars": 8,
         "gain": 0.16 * gain, "swing": 0.16, "params": {"tone": 5500}},
    ]


def road_drums():
    return [
        {"type": "pattern", "kind": "kick", "steps": "X-------x-------", "bars": 8,
         "gain": 0.5, "params": {"punch": 0.3}},
        {"type": "pattern", "kind": "brush", "steps": "x-x-x-x-x-x-x-x-", "bars": 8,
         "gain": 0.22, "swing": 0.12},
        {"type": "pattern", "kind": "rim", "steps": "----x-------x---", "bars": 8,
         "gain": 0.3, "swing": 0.12},
    ]


MASTER = [
    {"type": "lowpass", "cutoff": 4500},
    {"type": "reverb", "mix": 0.16, "size": 0.5, "damp": 0.5},
    {"type": "compressor", "threshold": -16, "ratio": 2.5},
    {"type": "mono"},
]


def segment(name, layers):
    return {"out": f"{name}.wav", "bpm": BPM, "beats": BEATS, "loop": "fold",
            "normalize": -3.0, "fx": MASTER, "layers": layers}


def compose():
    rng = random.Random(84)
    sounds = []
    for n, prog in enumerate(PROGRESSIONS):
        sounds.append(segment(f"desk_{n + 1}", [
            {"type": "seq", "inst": "epiano", "steps": chords(prog), "gain": 0.42, "vel": 0.55,
             "strum": 0.03},
            {"type": "seq", "inst": "upright_bass", "steps": walking_bass(prog), "gain": 0.8},
            {"type": "seq", "inst": "felt_piano" if n % 2 else "vibraphone",
             "steps": melody(prog, rng), "gain": 0.5, "vel": 0.6},
            *lofi_drums(),
        ]))
    for n, prog in enumerate(PROGRESSIONS[:4]):
        lead = [{"type": "seq", "inst": "harmonica", "steps": melody(prog[4:], rng, 0, 4),
                 "gain": 0.32, "vel": 0.5, "at": 16}] if n == 1 else []
        sounds.append(segment(f"road_{n + 1}", [
            {"type": "seq", "inst": "steel_guitar", "steps": arpeggio(prog), "gain": 0.4,
             "vel": 0.55},
            {"type": "seq", "inst": "upright_bass", "steps": roots(prog), "gain": 0.8},
            {"type": "seq", "inst": "warm_pad", "steps": chords(prog), "gain": 0.22,
             "params": {"attack": 1.0, "release": 1.5}},
            *lead,
            *road_drums(),
        ]))
    for n, prog in enumerate(PROGRESSIONS[4:]):
        sounds.append(segment(f"night_{n + 1}", [
            {"type": "seq", "inst": "warm_pad", "steps": chords(prog), "gain": 0.4,
             "params": {"attack": 1.2, "release": 2.0}},
            {"type": "seq", "inst": "felt_piano", "steps": melody(prog, rng, 0, 5),
             "gain": 0.55, "vel": 0.55},
            {"type": "seq", "inst": "upright_bass", "steps": roots(prog), "gain": 0.6},
        ]))
    return {
        "out_dir": "audio/music_render",
        "sr": 44100,
        "sounds": sounds,
    }


if __name__ == "__main__":
    out = Path(__file__).with_name("music.json")
    out.write_text(json.dumps(compose(), indent=2) + "\n")
    print(f"wrote {out}")
