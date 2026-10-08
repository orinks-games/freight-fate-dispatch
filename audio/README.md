# Interface sound sources

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
