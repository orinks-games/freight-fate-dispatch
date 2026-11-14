# Agent instructions

## Writing rule: no accessibility or audio marketing talk

This applies to everything you write: READMEs, store and site descriptions,
release notes, changelogs, forum posts (audiogames.net and similar), commit
messages, PR text, and docs.

- Never describe a game or app as audio-first, blind-first, accessibility-first,
  truly accessible, designed with accessibility at the forefront, crafted for
  keyboard and screen reader use, specifically optimized for screen reader
  users, or anything that means the same thing.
- Exception for games: a game may say once, plainly, that it is an audio game
  for blind and visually impaired players (for example "An audio trucking game
  for blind and visually impaired players."). Say it one time, in the opening
  description, and nothing beyond that.
- Skip audio puffery too: "rich and immersive soundscapes", "a game designed to
  be heard", "sounds like you're really there", "your eyes don't matter, only
  your ears do", and the like.
- Why: the audience already takes accessibility and audio for granted. Saying
  so is filler. Describe what the thing actually does instead: its features,
  content, controls, and what changed.
- Facts are fine when they are specific and useful, for example "works with
  NVDA and JAWS" in a requirements list or a note about which keys open a menu.

## Changelog

`CHANGELOG.md` is what players read on a release page: the release workflow
publishes the tag's block as the release notes (`tools/release_notes.py`,
the way Freight Fate does it), and a tag with no notes fails before
anything is built.

- A change a player would notice adds an entry under `## Unreleased` in the
  same PR: what changed, and what it was before when that helps. Plain
  prose, grouped under `###` headings by area (Drivers, Freight, Hours of
  service, Money, Saves, Wording).
- A release renames `## Unreleased` to `## <version> - <date>` in the commit
  that bumps the version, and leaves a new empty `## Unreleased` above it.
- CI, tests, refactors and other changes no player sees need no entry.
