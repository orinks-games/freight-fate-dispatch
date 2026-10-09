"""Level-match the rendered music segments and encode them for the game.

Reads audio/music_render/*.wav (from `genny render audio/music.json`),
brings each to the same integrated loudness and writes Ogg Vorbis to
assets/music/. Needs ffmpeg and oggenc (vorbis-tools).
"""

import re
import subprocess
from pathlib import Path

TARGET_LUFS = -20.0
MAX_BOOST_DB = 2.0  # genny normalizes peaks to -3 dBFS; stay clear of clipping
ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "audio" / "music_render"
DST = ROOT / "assets" / "music"


def loudness(path):
    result = subprocess.run(
        ["ffmpeg", "-hide_banner", "-nostats", "-i", str(path), "-af", "ebur128", "-f", "null", "-"],
        capture_output=True, text=True, check=True)
    return float(re.findall(r"I:\s+(-?[\d.]+) LUFS", result.stderr)[-1])


def main():
    DST.mkdir(parents=True, exist_ok=True)
    for wav in sorted(SRC.glob("*.wav")):
        gain = min(TARGET_LUFS - loudness(wav), MAX_BOOST_DB)
        tmp = wav.with_suffix(".level.wav")
        subprocess.run(["ffmpeg", "-y", "-loglevel", "error", "-i", str(wav), "-af",
                        f"volume={gain:.2f}dB", "-c:a", "pcm_s16le", str(tmp)], check=True)
        out = DST / f"{wav.stem}.ogg"
        subprocess.run(["oggenc", "--quiet", "-q", "3", "-o", str(out), str(tmp)], check=True)
        tmp.unlink()
        print(f"{out.relative_to(ROOT)}  {gain:+.1f} dB  {out.stat().st_size // 1024} KB")


if __name__ == "__main__":
    main()
