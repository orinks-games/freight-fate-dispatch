# Freight Fate: Dispatch — Rust port and first playable slice

Date: 2026-09-22. Target: companion beta, week of 2026-10-04.

## Goal

Ship an audio-first dispatcher sim, in Rust, on Freight Fate's own model
crate. The launch slice proves one hook: **the board talks back**. Assigning
loads is the setup; the game is the shift-long juggle of trouble on the road
(hours running out, breakdowns, real weather, detention), handled over the
phone against a desk clock.

## Decisions already made

- The Python prototype is retired. Its final state lives on the
  `python-prototype` branch (multi-day careers, phone, hiring, career saves)
  and in `main` history at `53bead5` (audio, FF save import). Both are the
  **rules reference** for the port; port behavior, not structure.
- `ff-core` (Freight Fate repo, `crates/ff-core`) is the shared universe
  crate for every Freight Fate game. The Python `freightverse` package is
  not ported; it retires once Rail is ported too. Freight Fate: Rail's port
  is out of scope here.
- Dispatch stays its own **private** repo. Freight Fate and `orinks-net` are
  public, so nothing Dispatch-specific lands in either before announcement
  (see `docs/release-and-freightverse-plan.md`, guardrails).
- Drivers carry the real `ff-core` hours-of-service clock. The dispatcher
  has no HOS; they have a desk clock with no regulatory limits.

## Architecture

One binary crate, `ff-dispatch`, in this repo.

- **Toolchain:** match Freight Fate's `rust-toolchain.toml` (1.98.0),
  edition 2021.
- **Dependencies:**
  - `ff-core` as a git dependency on `Orinks/Freight-fate`, pinned to a tag.
    Pin the latest `1.9-tester-*` tag until FF 1.9.0 ships, then the 1.9.0
    tag. Bump the pin deliberately with each FF release.
  - `prismer` for speech (the same Prism binding FF uses).
  - `sdl2` for window and keyboard, configured the way FF does it.
  - `ureq` for HTTP; `serde` and `serde_json` for saves; `keyring` for the
    online token.
- **World data:** load the same baked `world.ffdata` FF ships, via
  `ff_core::data::world::get_world`. The release build copies it from an FF
  build.

### Modules

Pure logic has no I/O and is tested headlessly with seeded RNG.

| Module | Responsibility |
| --- | --- |
| `roster` | `Driver`: name, `ff_core` `Career` and `HosClock`, status (idle / on load / off duty / out on a multi-day run), goodwill. |
| `board` | The day's loads, from `ff_core` `JobBoard::offers` at the terminal city. Refusal gates for endorsements, remaining hours and experience, the same gates FF applies to its player. |
| `desk` | The desk clock (06:00–18:00 game time), the action cost table, and the schedule of calls and incidents keyed by desk time. |
| `trip` | Loads in progress: position derived from desk time and the route's leg durations, HOS burn, ETA against the appointment. |
| `incidents` | The four incident types, their triggers, and the effect of each choice on the trip, funds, goodwill and service record. |
| `phone` | Driver calls (sick, personal day) and broker hot-load offers. |
| `hiring` | The daily applicant pool, signing costs, roster cap of 10. |
| `career` | Funds, the service record, the day loop, overnight settlement of multi-day runs, market drift. |
| `saves` | Versioned JSON, atomic writes, tolerant loads, saved at day boundaries. |
| `import` | Read-only FF profile import through `ff_core::models::profile::Profile::load`. Maps it to a roster `Driver` plus a carrier desk. |
| `online` | Device-code sign-in and the end-of-day profile update. Off unless the player signs in. |
| `ui` | Menu state machine and speech. Everything above is presented through it. |

## The shift loop

1. **Morning.** A spoken briefing lists drivers available, loads on the
   board, runs still out, and funds.
2. **Work the board.** Assign loads to legal drivers. Every action costs
   desk time: assign 10 min, take a call 5 min, hiring interview 15 min,
   check on a truck 2 min. The clock advances **only** on actions, never in
   real time.
3. **The board talks back.** As the clock passes the scheduled time of a
   call or incident, the phone rings, the player answers, and the choice
   applies.
4. **Close the day** by choice, or when the clock reaches 18:00. Trips still
   rolling finish by projection. A load whose projected delivery falls
   after today becomes a multi-day run.
5. **Overnight.** Drivers rest (HOS reset per `ff_core` rules), the market
   drifts, and multi-day runs that arrive settle as news. Then save.

Determinism: the day seed is derived from career seed + day number. The
board, calls, applicants and seeded incidents replay identically after
reloading a morning.

### Incidents (launch set)

Each incident has a trigger, a phone script, and 2–3 choices with concrete
effects.

| Incident | Trigger | Choices |
| --- | --- | --- |
| Hours running short | The projected drive time left exceeds the driver's legal hours before the appointment. | Relay to an idle driver within range (uses `ff_core::models::jobs::relay`); park and deliver late; call the receiver for a new appointment (costs reputation, succeeds by odds). |
| Breakdown | A flat seeded chance per trip leg (truck-age weighting comes after launch). | Roadside repair (money + delay); send another truck (the second driver's hours + deadhead); relay. |
| Weather / road closure | A real NWS alert on the route (see below), or a seeded storm when offline. | Hold until it clears (delay); reroute (longer, burns hours); push through (risk of an incident, a fine or a late load). |
| Detention | Seeded at pickup, more likely with some shippers. | Wait (burns the 14-hour window); bill detention (money, shipper goodwill); pull the driver (load abandoned, record hit). |

The launch slice does not include refused loads, cargo claims or
inspections. Those come after launch.

### Real weather

At the start of each day, if online, fetch NWS alerts for the corridor
states of the day's routes, using `ff_core::sim::real_weather_alerts`
(`parse_alerts`, `alert_effect`) with Dispatch's own `HttpTransport`
implementation over `ureq`. Store the parsed alerts in the day's save, so a
reload replays the same weather. If the fetch fails or the player is
offline, use seeded weather. Never block the morning on the network: use a
3-second timeout and fall back.

## Start menu

- **New career:** choose a carrier from `ff_core::models::start_options::START_OPTIONS`.
  Its pay plan (`pay_plan_for_key`), freight emphasis and deadline style
  shape the board. The player starts with 3 seeded drivers.
- **Continue:** list saved careers.
- **Import a Freight Fate driver:** list FF profiles found in FF's save
  locations (use `ff_core` settings/paths; `FF_DISPATCH_FREIGHT_FATE_SAVES`
  overrides). Load read-only. The driver joins the roster with their real
  level, reputation, endorsements, on-time record and remaining hours, and
  the career starts at their carrier.
  - Saves from an FF version newer than the pinned `ff-core`: say so plainly
    and refuse the import; never crash or write the save.
  - Saves with the integrity flag set import normally. This is a local
    single-player continuation.

## Online (optional, opt-in)

- **Sign-in:** the same device-code flow as FF (`/api/freight-fate/activate/start`
  and `activate/poll` on `https://www.orinks.net`), so the player uses their
  existing FF account. `FF_DISPATCH_ONLINE_URL` overrides the base URL (for
  staging). The token is stored in the OS keyring under Dispatch's own
  entry, never in a plain file.
- **Profile line:** at day end, post carrier, days worked and on-time
  percentage.
  - This needs a new endpoint in `orinks-net`, which is public, so it lands
    in release week alongside the announcement.
  - If the endpoint isn't live, the client skips it silently. Online
    failures never interrupt play; they are logged.
- Every payload carries a `schema_version`. Both sides tolerate unknown
  fields (the release plan's format governance rules).

## UI and accessibility

- Same menu grammar as Freight Fate: arrow keys with wraparound,
  Home/End, first-letter jump, Enter, Escape, F1 for help on the current
  item, Control to stop speech.
  - Every screen's text is also available as lines (for tests, and for a
    later text view).
- The phone interrupts the current menu: the ring is announced, and Enter
  answers. Unanswered calls stay queued, and the desk clock does not
  advance while a call is pending.
- No real-time pressure anywhere. Time only moves when the player acts.
- Speech via Prism. When Prism is unavailable, or with
  `FF_DISPATCH_NO_SPEECH=1`, fall back to printing to the console.
- The slice is speech-only: the ring is spoken. Sound effects via FF's
  `bass-sys` come after launch.

## Error handling

- Career saves are written atomically (temp file + rename). A corrupt save
  is reported by name and skipped in the Continue list, never deleted.
- An FF save is never opened for writing.
- World data missing at startup: spoken, and printed with the expected
  path, then the game exits.
- Every network call has a timeout, and failure falls back offline.

## Testing

- Headless logic tests for every pure module with seeded days: refusal
  gates, the trigger and each choice outcome for every incident, the desk
  clock and action costs, overnight settlement, and identical replay of a
  reloaded morning.
- Import round trip against a real `.ffsave` fixture copied from FF's test
  data, plus a newer-version save fixture that must be refused cleanly.
- The online client is tested against a fake transport. There are no live
  network calls in tests.
- A boot smoke test: start the binary with no speech, drive a scripted
  shift through the menu layer, and assert the spoken lines.
- CI: GitHub Actions on push (`cargo fmt --check`, `clippy`, `test`), and a
  Windows portable zip on `v*` tags.

## Release (week of 2026-10-04)

- A Windows portable zip containing the binary, `world.ffdata`, the Prism
  DLLs and a README.
- Linux and macOS builds follow the FF pattern after the beta.
- **Blocking dependency:** the release plan puts Dispatch *after* FF
  1.9.0. If FF 1.9.0 isn't out by then, ship as a tester beta pinned to the
  latest 1.9 tester tag, or move the date.

## Roadmap after launch

- **0.2 Climb:** dispatcher ranks (night desk, lead dispatcher, terminal
  manager, operations director) that unlock lanes, freight types and fleet
  size; achievements, posted through FF's existing
  `events/achievement` endpoint; driver loyalty, burnout and poaching.
  Release plan **Level 1**: real FF players, as shown by presence, appear
  as available owner-operators (names only).
- **0.3 Daily shift:** one shared seeded shift per day, with an on-time and
  margin leaderboard (one `orinks-net` endpoint).
- **0.4 Cross-play**, release plan **Level 2**: a dispatcher posts a load,
  and it surfaces on that player's FF dispatch board at the next sync.
  Delivery in FF settles back to the Dispatch ledger through FF's existing
  delivery event. Needs a Convex offer queue and an FF client change that
  lands as generic plumbing.
- **Later:** release plan **Level 3** (a shared home terminal, after FF
  1.10); Rail joins through intermodal ramps once it runs on `ff-core`;
  freightverse retires.

## Out of scope for the slice

Radio, music, cloud saves, ranks, achievements, the daily shift, owner-
operators, cross-play, refused loads, cargo claims, inspections, and
Linux/macOS builds.
