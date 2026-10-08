# Interface sound and music sources

The WAV files in `assets/audio` are embedded into the executable. They were
generated deterministically from `genny.json` with
[genny](https://github.com/ogomez92/genny) commit
`54f409e6292119ad8a87212fc4409ecc1cdb1797`.

Regenerate them from a genny checkout with:

```text
uv run genny render /path/to/freight-fate-dispatch/audio/genny.json \
  --out-dir /path/to/freight-fate-dispatch --strict
```

Run `genny info assets/audio/*.wav` after changes. Tonal cues should remain
below 2.0 sharpness and 3 percent energy above 5 kHz.

## Music

The desk music is twelve 8-bar segments in D major at 84 BPM
(`assets/music/*.ogg`). Each is a sample-exact genny `"loop": "fold"` of
32 beats, so the game shuffles them back to back forever without a gap.
`compose_music.py` writes the genny batch `music.json`; it was rendered with
genny commit `54f409e6292119ad8a87212fc4409ecc1cdb1797`. Rebuild with:

```text
python3 audio/compose_music.py
genny render audio/music.json --out-dir . --strict
genny info audio/music_render/*.wav
python3 audio/encode_music.py
```

`encode_music.py` matches every segment to -20 LUFS and encodes Ogg Vorbis
with ffmpeg and oggenc (vorbis-tools). Keep segments mono 44.1 kHz and exactly
32 beats at 84 BPM; the game checks the length when decoding.
