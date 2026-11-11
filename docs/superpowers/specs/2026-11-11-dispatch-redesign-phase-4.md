# Dispatch redesign, phase 4: growth

Date: 2026-11-11. The last of four phases from the redesign proposal. The
weeks get something to add up to: tiers that open doors, dedicated
accounts, a second terminal, milestones and a career score.

## What changes for the player

- **Tiers.** Reputation reads as a tier on the new Company screen: new
  authority, known (60), established (75), preferred (90). Each tier adds
  a spot load a day to the board, an account the desk can serve (six, then
  seven, eight, nine) and a load a week to customers won on sales calls.
- **Reputation feeds.** Sunday's scorecard adds 2 points for a week at 90
  percent on time, 1 for 80, and takes 1 under 70 (five loads or more), and
  takes 1 while the safety score is over the audit line. Goals met and
  customer weeks move it as before, and the scorecard reads goals met.
- **Dedicated accounts.** From established, with four drivers, a customer
  with 30 loads delivered and no bad week on record calls on a Monday
  morning: take the account or pass. Taking it means 10 loads a week, 5
  percent on the rate, 30-day pay, and a 90 percent standard on acceptance
  and on time; two weeks under pulls it back to 4 loads a week. An
  uncovered dedicated load costs 2 reputation. A no is asked again in 4
  weeks.
- **Second terminal.** The Company screen's last row lists four cities 250
  to 900 air miles out, the busiest first. Opening one takes an established
  reputation, five drivers and $10,000, then $4,000 a month more in rent.
  Two accounts seed there, its board fills every morning, and drivers go
  home to the nearer terminal.
- **Milestones and score.** Milestones are news at the close and listed
  on the Company screen. The career score is deliveries, on-time
  deliveries, 10 an account, 50 a milestone and 5 a point of reputation;
  Sunday's scorecard reads it with the week's change and the tier line.

## Model

- `growth.rs`: `Tier`, `tier_line`, `Milestone`, `new_milestones`, `score`, `dedicated_offer`, `nearest_terminal`, `terminal_choices`, constants.
- `Customer` gains `dedicated`, `delivered_total`, `dedicated_asked_week`, `dedicated_weeks_under`; `customers_at` seeds accounts at any terminal.
- `Game` gains `milestones`, `terminal2`, `score_last`; `terminals()`, `dedicated_text`, `answer_dedicated`, `open_terminal`, `company_lines`.
- `Call::Dedicated` with `TakeIt` and `PassOnIt`; the bots take it.
- `Screen::Company` and `Screen::TerminalPick`; the desk has ten rows.
- `WeekTotals` gains `goals` and `goals_met`; `Tractor` gains `paid_off`.

## Out of scope

Tying the second terminal to the Freight Fate home terminal, drivers
hired at the second terminal, selling or moving a terminal, and brokers
that only work with preferred carriers.
