# Freight Fate: Dispatch

**Private and unannounced.** The other side of the board: an audio-first
dispatcher sim in the Freight Fate universe. You assign the loads, watch the
hours-of-service clocks, and answer for the service record — the chair every
Freight Fate driver has been sitting across from.

## Career start

- **New dispatcher career** — choose which carrier's desk you run: Northstar
  Freight Lines, Great Lakes Training Transport, Prairie Link Regional, or
  Summit Value Logistics. Each keeps its Freight Fate character: pay plan,
  freight emphasis, and deadline style shape the board you work.
- **Import a Freight Fate driver** — reads your Freight Fate save (never
  writes it), puts that driver on your roster with their real level,
  reputation, endorsements, and on-time record, and seats you at their
  carrier's dispatch desk.

## Running

```bash
uv sync --group dev
uv run freight-fate-dispatch      # play
uv run pytest                     # logic tests, headless
```

Speech uses Prism when available and falls back to the console. Set
`FF_DISPATCH_NO_SPEECH=1` for headless runs. If your Freight Fate saves live
somewhere unusual, point `FF_DISPATCH_FREIGHT_FATE_SAVES` at the saves folder.

## Architecture

Everything that *is* the universe comes from the private
[freightverse](https://github.com/Orinks/freightverse) package: the world
graph, job generation, markets, carrier pay plans, career/HOS rules, and the
driver exchange format. This repo owns only the dispatcher's view of it:

- `models/drivers.py` — the roster. Every driver carries a real freightverse
  `Career` and `HosClock`, so reputation, XP, endorsements, and legal hours
  behave exactly as they do for the player in Freight Fate.
- `models/shift.py` — pure shift logic: board generation, refusal rules
  (endorsements, hours, experience caps — the same gates Freight Fate applies
  to the player), on-time odds, settlement, and the shift summary.
- `states/` — the audio-first menu grammar Freight Fate players already know:
  arrows, Enter, Escape, F1 help, first-letter jump, Control stops speech.

## v0 scope

One shift: work the load board, cover loads with legal drivers, run the
board, hear the outcomes and your margin. Persistence, multi-day careers,
driver phone calls, hiring, and the radio come next.
