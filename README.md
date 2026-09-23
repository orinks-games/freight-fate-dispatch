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
- **Continue a career** — careers save at every day boundary and resume at
  the start of their next morning, board regenerated identically.
- **Import a Freight Fate driver** — reads your Freight Fate save (never
  writes it), puts that driver on your roster with their real level,
  reputation, endorsements, and on-time record, and seats you at their
  carrier's dispatch desk.

## The day loop

Every day is a shift at the desk: work the load board, cover loads with
legal drivers, run the board, end the day. Overnight the roster rests, the
freight market drifts, and multi-day hauls come home:

- **Multi-day runs** — a load too long for one duty window sends the driver
  out on the road. The settlement lands with the load, days later, as
  overnight news; the driver is off your board until they are back.
- **The phone** — drivers call out sick, ask for personal days, and brokers
  pitch premium hot loads mid-shift. Every answer has a cost somewhere:
  goodwill, reputation, or a hole in your board.
- **The hiring office** — a fresh applicant pool every day, signing costs
  paid from the terminal's operating funds, roster capped at ten.
- **The terminal radio** — the desk set picks up whatever reaches the
  terminal city, with the same station catalog as the Freight Fate cab.
  Speech-only for now, streamer-safe by default.
- **Market watch and the service record** — margins accumulate in the
  operating funds; every delivered, late, and abandoned load goes on your
  record.

## Running

```bash
uv sync --group dev
uv run freight-fate-dispatch      # play
uv run pytest                     # logic tests, headless
```

Speech uses Prism when available and falls back to the console. Set
`FF_DISPATCH_NO_SPEECH=1` for headless runs. Career saves live in the
platform app-data directory (override with `FF_DISPATCH_DATA_DIR`). If your
Freight Fate saves live somewhere unusual, point
`FF_DISPATCH_FREIGHT_FATE_SAVES` at the saves folder.

## Architecture

Everything that *is* the universe comes from the private
[freightverse](https://github.com/Orinks/freightverse) package: the world
graph, job generation, markets, carrier pay plans, career/HOS rules, the
radio catalog, and the driver exchange format. This repo owns only the
dispatcher's view of it:

- `models/drivers.py` — the roster. Every driver carries a real freightverse
  `Career` and `HosClock`, so reputation, XP, endorsements, and legal hours
  behave exactly as they do for the player in Freight Fate. `ActiveRun`
  tracks a driver out on a multi-day haul.
- `models/shift.py` — pure shift logic: board generation, refusal rules
  (endorsements, hours, experience caps — the same gates Freight Fate applies
  to the player), single-day settlement versus multi-day departures, on-time
  odds, and the shift summary.
- `models/career_state.py` — the multi-day career: funds, the day loop,
  overnight settlement of active runs, market drift, and the service record.
- `models/phone.py` / `models/hiring.py` — deterministic per-day call
  schedules and applicant pools, so a reloaded morning replays identically.
- `career_saves.py` — this game's own persistence: versioned JSON, atomic
  writes, tolerant loads, saved at day boundaries. (`saves.py` stays
  read-only over Freight Fate's saves.)
- `states/` — the audio-first menu grammar Freight Fate players already know:
  arrows, Enter, Escape, F1 help, first-letter jump, Control stops speech.

## CI

`.github/workflows/ci.yml` runs ruff and the headless test suite. Because
freightverse is private, the workflow needs a `FREIGHTVERSE_TOKEN` repository
secret: a fine-grained PAT with read access to `Orinks/freightverse`.

## Next

Driver morale and fatigue carrying across days, breakdowns and mid-run phone
calls, rival carriers bidding on the same freight, and audio playback for the
terminal radio.
