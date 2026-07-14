# Release Sequencing and Freightverse Connection Plan

Written 2026-07-14, during Freight Fate 1.9 development. This captures the
agreed sequencing for shipping FF Dispatch and the staged plan for
connecting it to Freight Fate. FF Dispatch is unannounced: none of this
plan appears in Freight Fate's public roadmap or changelogs until launch.

## When to release

**After Freight Fate 1.9.0 reaches dev, shortly after.** Reasons:

- The freightverse driver-exchange format should freeze against 1.9's
  profile shape (the career-arc rebalance added fields: fleet tractor
  assignments, rebalanced XP/levels, endorsement history). Dispatch then
  imports a stable format instead of chasing a moving one.
- 1.9's release wave is the marketing moment: let 1.9 land, then ship
  Dispatch as the companion while attention is high.
- The 1.9 stabilization window (bug reports trickling in, no big FF
  features starting) is exactly when Dispatch can get the focused
  development time it has not had yet. Realistic shape: 1.9.0-dev ships,
  a few weeks of Dispatch polish and content depth on top of the playable
  first shift, then a companion beta.

Note for later: Freight Fate 1.10 is planned as "the working week and
home" (ELD 70-hour cycle, persisted home terminals -- see
`docs/eld-home-terminal-design.md` in the FF repo). Level 3 below depends
on it; nothing else here does.

## Connection levels (ascending ambition)

- **Level 0 -- save import (exists, ship with it).** An FF save seeds the
  Dispatch career: carrier, money, reputation. This is the launch hook --
  Dispatch feels like a continuation, not a separate game.
- **Level 1 -- one-way texture via orinks-net (build during beta).** FF's
  drivers-board presence already heartbeats to Convex. Dispatch shows real
  FF players as available owner-operators on its load board (names only,
  no interaction). Mirror flavor in FF: CB chatter occasionally names
  carriers that Dispatch players run. Shared universe by osmosis, no sync
  problems, respects the presence-consent settings FF already has.
- **Level 2 -- asynchronous cross-play (the flagship, once both games are
  stable).** Post a load in Dispatch; the next FF sync surfaces it on your
  FF board as a special dispatch from your own carrier. Deliver it in FF
  and the settlement flows back to the Dispatch ledger. Turn-based by
  nature, so the convoy time-sync problem that blocks live FF multiplayer
  does not apply. This is the feature to version the exchange format
  around from day one: every payload carries a schema version, and both
  games tolerate unknown fields (the same additive-optional rule FF's
  cloud saves follow).
- **Level 3 -- shared address (after FF 1.10).** The Dispatch carrier is
  headquartered at the player's FF home terminal; domicile relocation in
  either game shows up in the other.

## Format governance

The `freightverse` package (private, synced FROM Freight Fate via the
sync tool) owns the driver-exchange format. Rules that make the levels
safe to build independently:

- Schema-version every exchange payload from the first Dispatch release.
- New fields are always additive and optional; neither game may require a
  field the other's current release does not write.
- FF 1.10's additions (`home_terminal_city`, HOS cycle ledger) enter the
  format as optional the release they ship, so Dispatch can adopt Level 3
  without lockstep updates.

## Marketing/privacy guardrails

- No references to Dispatch in FF's public roadmap, changelog, commits, or
  PR comments until Dispatch is announced.
- FF-side hooks needed for Level 1+ (CB carrier-name flavor, board sync)
  land as generic freightverse plumbing without naming the game.
