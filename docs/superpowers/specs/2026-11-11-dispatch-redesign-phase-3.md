# Dispatch redesign, phase 3: money and equipment

Date: 2026-11-11. The third of four phases from the redesign proposal.
Money arrives the way it does at a carrier, and the trucks become things
that wear out and get paid for.

## What changes for the player

- **Receivables.** A delivered load is billed, not paid: brokers pay in 30
  days, customers in 30 or 45 (set per account). The delivery line says
  what was billed, to whom, and when it is due. Payments arrive overnight
  as news.
- **Factoring.** A new authority starts with factoring on: every load pays
  the next day, less 3 percent. The Money screen switches it off. Old saves
  load with it on.
- **Operating costs.** Fuel at this week's diesel price (a slow weekly drift
  around $3.90, 6.5 miles a gallon) and road costs of $0.316 a mile come
  out at delivery. A contract load adds a fuel surcharge when diesel is
  over $3.50. The old flat truck cost becomes $40 a truck a day of overhead.
- **Settlements and bills.** Driver pay accrues and goes out on Friday. The
  first of each 30-day month pays the truck notes, insurance ($600 a truck,
  up 2 percent per safety point) and $4,000 terminal rent, plus 1.5 percent
  interest on a negative balance.
- **The line of credit.** Cash can fall to minus $40,000. Over the line for
  23 days brings a warning; 30 days ends the career. A safety score of 60
  or more for over a week is a shutdown order; the last driver leaving ends
  it too. A finished career says so on Continue and stays saved.
- **Trucks.** One per driver and a spare at the start, one old and owned,
  the rest on $1,400 notes. Each has age, miles and a service interval of
  15,000 miles; past it, breakdowns triple. Service is $600 and a day in
  the shop; a driver whose truck is in the shop can't take a load. A used
  tractor is $8,000 down and $1,400 a month for 60 months. Hiring needs a
  spare truck.
- **Money screen.** Cash and the line, what is owed and when the next
  payment lands, Friday's settlement, the first's bills, this week's diesel,
  factoring, and the Trucks screen.

## Model

- `money.rs`: constants, `Receivable`, `Tractor`, `starting_fleet`, `fuel_price`, `fuel_cost`, `surcharge`, `pay_day`, `insurance`, `Ending`, `is_settlement_day`, `is_bill_day`.
- `Game` gains `receivables`, `settlement_owed`, `factoring` (default on), `tractors`, `days_over_line`, `shutdown_days`, `ended`; `money_overnight` runs at the close; `service_truck`, `buy_truck`, `toggle_factoring`, `money_lines`.
- `Customer` gains `pay_days`.
- `Screen::Money` and `Screen::Trucks` under the desk's Money row.

## Balance

Over 30 days the planner bot ends near $45,000 cash on $95,000 gross with
factoring on; with it off, cash bottoms near minus $45,000 at the first of
the month before the receivables land, which is why a new career factors.

## Out of scope, later phases

Fuel cards with discounts, quick pay from brokers, selling a truck, trailers,
tolls by route, and a driver's pay while suspended.
