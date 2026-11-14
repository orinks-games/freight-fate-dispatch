#!/usr/bin/env python3
"""Write a release's notes from CHANGELOG.md, the way Freight Fate does.

    tools/release_notes.py stable --version v1.0.1 --output notes.md

The notes are the changelog's `## 1.0.1` block (a date after the version,
`## 1.0.1 - 2026-11-14`, is allowed), or the `## Unreleased` block when
that heading has not been cut yet. `###` headings inside the block become
`##` in the notes. A release with no notes is refused.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

CHANGELOG = Path(__file__).resolve().parent.parent / "CHANGELOG.md"


def extract_release_block(text: str, heading_pattern: str) -> str:
    match = re.search(heading_pattern, text, re.IGNORECASE | re.MULTILINE)
    if not match:
        return ""
    start = match.end()
    next_heading = re.search(r"^##\s+", text[start:], re.MULTILINE)
    end = start + next_heading.start() if next_heading else len(text)
    return text[start:end].strip()


def unreleased_block(text: str) -> str:
    return extract_release_block(text, r"^##\s+\[?Unreleased\]?\s*$")


def version_block(text: str, version: str) -> str:
    normalized = version.removeprefix("v")
    return extract_release_block(
        text,
        rf"^##\s+\[?v?{re.escape(normalized)}\]?(?:\s+-\s+\d{{4}}-\d{{2}}-\d{{2}})?\s*$",
    )


def stable_notes(text: str, version: str) -> str:
    block = version_block(text, version) or unreleased_block(text)
    return re.sub(r"^###(?=\s)", "##", block, flags=re.MULTILINE)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    commands = parser.add_subparsers(dest="command", required=True)
    stable = commands.add_parser("stable", help="Write a release's notes.")
    stable.add_argument("--version", required=True, help="The tag or version, such as v1.0.1.")
    stable.add_argument("--output", required=True, help="The file to write.")
    stable.add_argument("--changelog", default=str(CHANGELOG), help=argparse.SUPPRESS)
    args = parser.parse_args(argv)

    notes = stable_notes(Path(args.changelog).read_text(encoding="utf-8"), args.version)
    if not notes:
        print(
            f"CHANGELOG.md has no notes for {args.version}: add them under "
            f"'## {args.version.removeprefix('v')}' or '## Unreleased'.",
            file=sys.stderr,
        )
        return 1
    Path(args.output).write_text(notes + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())
