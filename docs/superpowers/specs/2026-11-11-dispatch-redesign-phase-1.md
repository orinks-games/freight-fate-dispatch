# Dispatch redesign, phase 1: freight and the week

Date: 2026-11-11. The first of four phases from the redesign proposal
(customers, the week, drivers with records, the law in game time, money,
growth). This phase changes where freight comes from and gives the career a
week.

## What changes for the player

- **Customers.** A career starts with three contract customers at the
  terminal, each with two lanes (one of them inbound), a weekly volume of two
  to four loads, and a contract rate near the market. Up to six customers.
- **Tenders.** On day 1 the week's first tenders are on the desk at 6 AM.
  Every weekday at 5 PM the phone rings with tomorrow's tenders (Friday's are
  for Monday). Answer the call with accept all, review one by one on the new
  Tenders screen, or decline all. A tender left open at the close counts as
  declined. Accepted loads lead tomorrow's board, named after the customer,
  with a delivery appointment instead of a countdown.
- **Spot board.** Every other load is from a broker, named in the load's
  help line. On the "who takes it" screen a spot load gets one extra row,
  "Counter the rate", which asks 10 percent more and takes 5 minutes. The
  odds come from the carrier's reputation and that broker's trust in it.
  A refusal sometimes gives the load to another carrier.
- **Broker trust.** An on-time broker load raises that broker's trust;
  a late one or a canceled pickup lowers it. Trust is what makes counters
  land.
- **The week.** Day 1 is a Monday. Saturday's board is half size and
  Sunday's is three loads; no tenders arrive on the weekend. Sunday's close
  speaks the week's scorecard: loads, on-time percent, revenue, and each
  customer's tender acceptance and on-time record. A customer that gets 90
  percent acceptance and on-time adds a load a week after two good weeks
  (and 2 percent on the rate, to a cap). Two weeks under 80 percent
  acceptance cuts a load a week; four weeks under loses the customer.
- **Uncovered contract loads.** An accepted load still on the board at the
  close is a service failure: it counts against the customer's on-time
  record and costs a point of reputation.
- **Sales call.** The last row of the Tenders screen makes a sales call
  (30 minutes, once a day). The odds follow reputation; success adds a
  customer with one lane and two loads a week.

## What does not change

The desk clock, the phone, the strategy bots (they accept every tender and
every hot load), the smoke test's keys, and save compatibility: an older
save gets its customers seeded on load and its first tenders at 5 PM.

## Model

- `customers.rs`: `Customer { id, name, lanes, weekly_loads, rate_mult, week, weeks_under, good_weeks }`, `Tender { id, customer_id, job, for_day, appointment }`, `Source::{Spot { broker }, Contract { customer_id }}`, the broker and customer name pools, tender generation, the scorecard.
- `Load` gains `source`, `appointment` and `countered`; `Trip` gains `source`.
- `Game` gains `customers`, `accepted` (tenders waiting for their day), `broker_trust` and `week` totals, all with serde defaults.
- `Shift` gains `tenders` (open today), `tomorrow` (held until the 5 PM call) and `sales_called`.
- `Call::Tenders` with choices accept all, review, decline all.
- `Action::SalesCall` costs 30 minutes.

## Out of scope, later phases

Pairing drivers to tomorrow's loads tonight, detention at the receiver,
lumper fees, payment terms, dedicated accounts, a second terminal.
