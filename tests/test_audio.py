"""Prototype audio is generated locally and loops without a boundary click."""

import itertools
from array import array

from ff_dispatch.audio import SAMPLE_RATE, synthesize_cue, synthesize_music_loop


def _samples(pcm: bytes) -> array:
    result = array("h")
    result.frombytes(pcm)
    return result


def test_cues_are_deterministic_distinct_pcm_without_asset_files():
    move = synthesize_cue("move")
    confirm = synthesize_cue("confirm")
    assert move == synthesize_cue("move")
    assert move != confirm
    assert len(move) >= SAMPLE_RATE // 20 * 2
    assert any(_samples(move))


def test_music_is_deterministic_and_has_a_smooth_loop_seam():
    pcm = synthesize_music_loop(seed=17, seconds=4)
    samples = _samples(pcm)
    assert pcm == synthesize_music_loop(seed=17, seconds=4)
    assert len(samples) == SAMPLE_RATE * 4
    seam_jump = abs(samples[0] - samples[-1])
    ordinary_largest_jump = max(abs(b - a) for a, b in itertools.pairwise(samples))
    assert seam_jump <= ordinary_largest_jump
